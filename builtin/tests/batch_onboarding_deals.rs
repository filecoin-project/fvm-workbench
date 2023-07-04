use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_ipld_hamt::BytesKey;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::crypto::signature::{Signature, SignatureType};
use fvm_shared::deal::DealID;
use fvm_shared::econ::TokenAmount;
use fvm_shared::piece::PaddedPieceSize;
use fvm_shared::sector::{RegisteredSealProof, StoragePower};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::bench::{ExecutionResult, WorkbenchBuilder};
use fvm_workbench_api::blockstore::DynBlockstore;
use fvm_workbench_api::vm::VM;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::bench::kernel::UNSEALED_SECTOR_CID_INPUT;
use fvm_workbench_vm::bench::primitives::FakePrimitives;
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;
use num_traits::Zero;

use fil_actor_market::{
    deal_id_key, ClientDealProposal, DealProposal, Label, Method as MarketMethod,
    PublishStorageDealsParams, PublishStorageDealsReturn,
};
use fil_actor_miner::{
    max_prove_commit_duration, power_for_sector, CompactCommD, SectorPreCommitOnChainInfo,
    State as MinerState,
};
use fil_actor_miner::{Method as MinerMethod, ProveCommitAggregateParams};
use fil_actors_runtime::cbor::serialize;
use fil_actors_runtime::runtime::policy::policy_constants::PRE_COMMIT_CHALLENGE_DELAY;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{STORAGE_MARKET_ACTOR_ADDR};

const BATCH_SIZE: usize = 8;
const PRECOMMIT_V2: bool = true;
const SEAL_PROOF: RegisteredSealProof = RegisteredSealProof::StackedDRG32GiBV1P1;

use crate::util::*;
use crate::workflows::*;
mod util;

// TODO: this test should be deleted and imported externally. the imported test should use a generic VM trait
#[test]
fn batch_onboarding_deals() {
    // create the execution wrangler
    let store = SharedMemoryBlockstore::new();
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        store.clone(),
        FakeExterns::new(),
        NetworkVersion::V18,
        StateTreeVersion::V5,
        actors_v12::BUNDLE_CAR,
    )
    .unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let bench = builder.build().unwrap();
    let mut w = ExecutionWrangler::new_default(bench, Box::new(store), Box::new(FakePrimitives {}));

    let deal_duration: ChainEpoch = Policy::default().min_sector_expiration;
    let sector_duration: ChainEpoch =
        deal_duration + Policy::default().market_default_allocation_term_buffer;

    let addrs = create_accounts(
        &mut w,
        genesis.faucet_id,
        3,
        TokenAmount::from_whole(10_000),
        SignatureType::BLS,
    )
    .unwrap();
    let (owner, verifier, client) = (addrs[0].clone(), addrs[1].clone(), addrs[2].clone());
    let worker = owner;
    let owner_addr = worker.id_addr();

    // Create miner
    let (_miner_id, miner) = create_miner(
        &mut w,
        worker.id, // worker is the owner in this case
        worker.id,
        SEAL_PROOF.registered_window_post_proof().unwrap(),
        TokenAmount::from_whole(1000),
    )
    .unwrap();

    // Create FIL verifier and client.
    verifreg_add_verifier(
        &mut w,
        verifier.id,
        StoragePower::from((1000_u64 << 30) as u128),
        genesis.verifreg_root_address(),
        genesis.verifreg_signer_address(),
    );
    verifreg_add_client(
        &mut w,
        verifier.id_addr(),
        client.id_addr(),
        StoragePower::from((1000_u64 << 30) as u128),
    );

    // Fund storage market accounts.
    market_add_balance(&mut w, &owner_addr, &miner, &TokenAmount::from_whole(1000));
    market_add_balance(
        &mut w,
        &client.id_addr(),
        &client.id_addr(),
        &TokenAmount::from_whole(1000),
    );

    // Publish a deal for each sector.
    let deals =
        publish_deals(&mut w, client.id_addr(), miner, worker.id_addr(), deal_duration, BATCH_SIZE);
    assert_eq!(BATCH_SIZE, deals.len());

    println!("deals: {:?}", deals);

    // Verify datacap allocations.
    let mut market_state: fil_actor_market::State =
        get_state(&w, &STORAGE_MARKET_ACTOR_ADDR).unwrap();
    let deal_keys: Vec<BytesKey> = deals.iter().map(|(id, _)| deal_id_key(*id)).collect();
    let alloc_ids = market_state
        .get_pending_deal_allocation_ids(&DynBlockstore::new(w.blockstore()), &deal_keys)
        .unwrap();
    assert_eq!(BATCH_SIZE, alloc_ids.len());

    println!("alloc_ids: {:?}", alloc_ids);

    // Associate deals with sectors.
    let sector_precommit_data = deals
        .into_iter()
        .map(|(id, _deal)| PrecommitMetadata {
            deals: vec![id],
            commd: CompactCommD(Some(make_piece_cid(&UNSEALED_SECTOR_CID_INPUT))),
        })
        .collect();

    // Pre-commit as single batch.
    let precommits = precommit_sectors_v2(
        &mut w,
        BATCH_SIZE,
        BATCH_SIZE,
        sector_precommit_data,
        &worker.id_addr(),
        &miner,
        SEAL_PROOF,
        0,
        true,
        Some(sector_duration),
        PRECOMMIT_V2,
    );
    let first_sector_no = precommits[0].info.sector_number;

    // Prove-commit as a single aggregate.
    w.set_epoch(w.epoch() + PRE_COMMIT_CHALLENGE_DELAY + 1);

    println!("================================= Running PCA ===============================");
    let pca_result = prove_commit_aggregate(&mut w, &worker.id_addr(), &miner, precommits);

    println!("=================================== PCA Trace ===============================");
    let analysis = TraceAnalysis::build(pca_result.trace);
    println!("{}", analysis.format_spans());

    // Submit Window PoST to activate power.
    let (dline_info, p_idx) = advance_to_proving_deadline(&mut w, &miner, 0);

    let sector_size = SEAL_PROOF.sector_size().unwrap();
    let st: MinerState = get_state(&w, &miner).unwrap();
    let sector =
        st.get_sector(&DynBlockstore::new(w.blockstore()), first_sector_no).unwrap().unwrap();
    let mut expect_new_power = power_for_sector(sector_size, &sector);
    // Confirm the verified deal resulted in QA power.
    assert_eq!(&expect_new_power.raw * 10, expect_new_power.qa);
    expect_new_power.raw *= BATCH_SIZE;
    expect_new_power.qa *= BATCH_SIZE;
    submit_windowed_post(
        &mut w,
        &worker.id_addr(),
        &miner,
        dline_info,
        p_idx,
        Some(expect_new_power.clone()),
    );

    // Verify state expectations.
    let balances = get_miner_balance(&mut w, &miner);
    assert!(balances.initial_pledge.is_positive());

    // let network_stats = get_network_stats(v);
    // assert_eq!(
    //     network_stats.total_bytes_committed,
    //     BigInt::from(sector_size as usize * BATCH_SIZE)
    // );
    // assert_eq!(network_stats.total_qa_bytes_committed, network_stats.total_bytes_committed * 10);
    // assert!(network_stats.total_pledge_collateral.is_positive());
}

fn publish_deals(
    w: &mut ExecutionWrangler,
    client: Address,
    provider: Address,
    worker: Address,
    duration: ChainEpoch,
    count: usize,
) -> Vec<(DealID, DealProposal)> {
    let deal_start = w.epoch() + max_prove_commit_duration(&Policy::default(), SEAL_PROOF).unwrap();
    let deals: Vec<ClientDealProposal> = (0..count)
        .map(|i| {
            // Label is integer as bytes.
            let proposal = DealProposal {
                piece_cid: make_piece_cid(&i.to_be_bytes()),
                piece_size: PaddedPieceSize(32 << 30),
                verified_deal: true,
                client,
                provider,
                label: Label::Bytes(vec![]),
                start_epoch: deal_start,
                end_epoch: deal_start + duration,
                storage_price_per_epoch: TokenAmount::zero(),
                provider_collateral: TokenAmount::from_whole(1),
                client_collateral: TokenAmount::zero(),
            };
            let client_signature = Signature::new_bls(
                serialize(&proposal, "serializing deal proposal").unwrap().to_vec(),
            );
            ClientDealProposal { proposal, client_signature }
        })
        .collect();

    let publish_params = PublishStorageDealsParams { deals: deals.clone() };
    let ret: PublishStorageDealsReturn = apply_ok(
        w,
        worker,
        STORAGE_MARKET_ACTOR_ADDR,
        TokenAmount::zero(),
        MarketMethod::PublishStorageDeals as u64,
        &publish_params,
    )
    .unwrap()
    .receipt
    .return_data
    .deserialize()
    .unwrap();

    let good_inputs = bf_all(ret.valid_deals);
    assert_eq!((0..count as u64).collect::<Vec<u64>>(), good_inputs);
    return ret.ids.into_iter().zip(deals.iter().map(|p| p.proposal.clone())).collect();
}

// TODO: unify with util::prove_commit_sectors, by plumbing through the information
// necessary to check expectations of deal activation and FIL+ claims.
pub fn prove_commit_aggregate(
    w: &mut ExecutionWrangler,
    worker: &Address,
    maddr: &Address,
    precommits: Vec<SectorPreCommitOnChainInfo>,
) -> ExecutionResult {
    let sector_nos: Vec<u64> = precommits.iter().map(|p| p.info.sector_number).collect();
    let prove_commit_aggregate_params = ProveCommitAggregateParams {
        sector_numbers: make_bitfield(sector_nos.as_slice()),
        aggregate_proof: vec![],
    };

    apply_ok(
        w,
        *worker,
        *maddr,
        TokenAmount::zero(),
        MinerMethod::ProveCommitAggregate as u64,
        &prove_commit_aggregate_params,
    )
    .unwrap()
}
