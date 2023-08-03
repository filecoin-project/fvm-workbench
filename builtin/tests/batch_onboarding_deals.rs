use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_ipld_hamt::BytesKey;
use fvm_shared::address::Address;
use fvm_shared::bigint::BigInt;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::deal::DealID;
use fvm_shared::econ::TokenAmount;
use fvm_shared::piece::{PaddedPieceSize, PieceInfo};
use fvm_shared::sector::{RegisteredSealProof, StoragePower};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_workbench_api::blockstore::DynBlockstore;
use fvm_workbench_api::{wrangler::ExecutionWrangler, WorkbenchBuilder};
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;
use fvm_workbench_vm::primitives::FakePrimitives;
use num_traits::Zero;
use vm_api::VM;

use fil_actor_market::{deal_id_key, DealProposal};
use fil_actor_miner::{
    max_prove_commit_duration, power_for_sector, CompactCommD, SectorPreCommitOnChainInfo,
    State as MinerState,
};
use fil_actor_miner::{Method as MinerMethod, ProveCommitAggregateParams};
use fil_actors_runtime::runtime::policy::policy_constants::PRE_COMMIT_CHALLENGE_DELAY;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::STORAGE_MARKET_ACTOR_ADDR;

const BATCH_SIZE: usize = 8;
const PRECOMMIT_V2: bool = true;
const SEAL_PROOF: RegisteredSealProof = RegisteredSealProof::StackedDRG32GiBV1P1;

use crate::util::*;
use crate::workflows::*;
use fil_actors_integration_tests::deals::{DealBatcher, DealOptions};
mod util;

#[test]
fn batch_onboarding_deals() {
    // create the execution wrangler
    let store = SharedMemoryBlockstore::new();
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        store.clone(),
        FakeExterns::new(),
        NetworkVersion::V18,
        StateTreeVersion::V5,
        fil_builtin_actors_bundle::BUNDLE_CAR,
    )
    .unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let _genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let bench = builder.build().unwrap();
    let w = ExecutionWrangler::new_default(bench, Box::new(store), Box::new(FakePrimitives {}));

    batch_onboarding_deals_test(&w);
}

// Tests batch onboarding of sectors with verified deals.
pub fn batch_onboarding_deals_test(v: &dyn VM) {
    let deal_duration: ChainEpoch = Policy::default().min_sector_expiration;
    let sector_duration: ChainEpoch =
        deal_duration + Policy::default().market_default_allocation_term_buffer;

    let addrs = create_accounts(v, 3, &TokenAmount::from_whole(10_000));
    let (owner, verifier, client) = (addrs[0], addrs[1], addrs[2]);
    let worker = owner;

    // Create miner
    let (miner, _) = create_miner(
        v,
        &owner,
        &worker,
        SEAL_PROOF.registered_window_post_proof().unwrap(),
        &TokenAmount::from_whole(1000),
    );

    // Create FIL verifier and client.
    verifreg_add_verifier(v, &verifier, StoragePower::from((1000_u64 << 30) as u128));
    verifreg_add_client(v, &verifier, &client, StoragePower::from((1000_u64 << 30) as u128));

    // Fund storage market accounts.
    market_add_balance(v, &owner, &miner, &TokenAmount::from_whole(1000));
    market_add_balance(v, &client, &client, &TokenAmount::from_whole(1000));

    // Publish a deal for each sector.
    let deals = publish_deals(v, client, miner, worker, deal_duration, BATCH_SIZE);
    assert_eq!(BATCH_SIZE, deals.len());

    // Verify datacap allocations.
    let mut market_state: fil_actor_market::State =
        get_state(v, &STORAGE_MARKET_ACTOR_ADDR).unwrap();
    let deal_keys: Vec<BytesKey> = deals.iter().map(|(id, _)| deal_id_key(*id)).collect();
    let alloc_ids = market_state
        .get_pending_deal_allocation_ids(&DynBlockstore::new(v.blockstore()), &deal_keys)
        .unwrap();
    assert_eq!(BATCH_SIZE, alloc_ids.len());

    // Associate deals with sectors.
    let sector_precommit_data = deals
        .into_iter()
        .map(|(id, deal)| PrecommitMetadata {
            deals: vec![id],
            commd: CompactCommD::of(
                v.primitives()
                    .compute_unsealed_sector_cid(
                        SEAL_PROOF,
                        &[PieceInfo { size: deal.piece_size, cid: deal.piece_cid }],
                    )
                    .unwrap(),
            ),
        })
        .collect();

    // Pre-commit as single batch.
    let precommits = precommit_sectors_v2(
        v,
        BATCH_SIZE,
        BATCH_SIZE,
        sector_precommit_data,
        &worker,
        &miner,
        SEAL_PROOF,
        0,
        true,
        Some(sector_duration),
        PRECOMMIT_V2,
    );
    let first_sector_no = precommits[0].info.sector_number;

    // Prove-commit as a single aggregate.
    v.set_epoch(v.epoch() + PRE_COMMIT_CHALLENGE_DELAY + 1);
    prove_commit_aggregate(v, &worker, &miner, precommits);

    // Submit Window PoST to activate power.
    let (dline_info, p_idx) = advance_to_proving_deadline(v, &miner, 0);

    let sector_size = SEAL_PROOF.sector_size().unwrap();
    let st: MinerState = get_state(v, &miner).unwrap();
    let sector =
        st.get_sector(&DynBlockstore::new(v.blockstore()), first_sector_no).unwrap().unwrap();
    let mut expect_new_power = power_for_sector(sector_size, &sector);
    // Confirm the verified deal resulted in QA power.
    assert_eq!(&expect_new_power.raw * 10, expect_new_power.qa);
    expect_new_power.raw *= BATCH_SIZE;
    expect_new_power.qa *= BATCH_SIZE;
    submit_windowed_post(v, &worker, &miner, dline_info, p_idx, Some(expect_new_power.clone()));

    // Verify state expectations.
    let balances = miner_balance(v, &miner);
    assert!(balances.initial_pledge.is_positive());

    let network_stats = get_network_stats(v);
    assert_eq!(
        network_stats.total_bytes_committed,
        BigInt::from(sector_size as usize * BATCH_SIZE)
    );
    assert_eq!(network_stats.total_qa_bytes_committed, network_stats.total_bytes_committed * 10);
    assert!(network_stats.total_pledge_collateral.is_positive());
}

fn publish_deals(
    v: &dyn VM,
    client: Address,
    provider: Address,
    worker: Address,
    duration: ChainEpoch,
    count: usize,
) -> Vec<(DealID, DealProposal)> {
    let deal_opts = DealOptions {
        piece_size: PaddedPieceSize(32 * (1 << 30)),
        verified: true,
        deal_start: v.epoch() + max_prove_commit_duration(&Policy::default(), SEAL_PROOF).unwrap(),
        deal_lifetime: duration,
        ..DealOptions::default()
    };
    let mut batcher = DealBatcher::new(v, deal_opts);
    (0..count).for_each(|_| batcher.stage(client, provider));
    let ret = batcher.publish_ok(worker);
    let good_inputs = bf_all(ret.valid_deals);
    assert_eq!((0..count as u64).collect::<Vec<u64>>(), good_inputs);
    return ret.ids.into_iter().zip(batcher.proposals().iter().cloned()).collect();
}

// This method doesn't check any trace expectations.
// We can do so by unifying with util::prove_commit_sectors, and plumbing through
// the information necessary to check expectations of deal activation and FIL+ claims.
// https://github.com/filecoin-project/builtin-actors/issues/1302
pub fn prove_commit_aggregate(
    v: &dyn VM,
    worker: &Address,
    maddr: &Address,
    precommits: Vec<SectorPreCommitOnChainInfo>,
) {
    let sector_nos: Vec<u64> = precommits.iter().map(|p| p.info.sector_number).collect();
    let prove_commit_aggregate_params = ProveCommitAggregateParams {
        sector_numbers: make_bitfield(sector_nos.as_slice()),
        aggregate_proof: vec![],
    };

    apply_ok(
        v,
        worker,
        maddr,
        &TokenAmount::zero(),
        MinerMethod::ProveCommitAggregate as u64,
        Some(prove_commit_aggregate_params),
    );
}
