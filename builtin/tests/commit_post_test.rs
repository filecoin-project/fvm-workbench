use fil_actor_cron::Method as CronMethod;
use fil_actor_miner::{
    power_for_sector, DeadlineInfo, Method as MinerMethod, PoStPartition, PowerPair,
    ProveCommitSectorParams, State as MinerState, SubmitWindowedPoStParams,
};
use fil_actor_power::State as PowerState;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{CRON_ACTOR_ADDR, STORAGE_POWER_ACTOR_ADDR, SYSTEM_ACTOR_ADDR};
use fvm_ipld_bitfield::BitField;
use fvm_ipld_blockstore::MemoryBlockstore;
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::randomness::Randomness;
use fvm_shared::sector::{PoStProof, RegisteredPoStProof, RegisteredSealProof, SectorNumber};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::ActorID;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::blockstore::BlockstoreWrapper;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::{Bench, WorkbenchBuilder};
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;

use crate::util::*;
use crate::workflows::*;
mod util;

/// Precommits a sector, then submits a proof for it, then runs cron till the proof is verified
fn setup(
    bench: &'_ mut dyn Bench,
    faucet_id: ActorID,
) -> (ExecutionWrangler<'_>, MinerInfo, SectorInfo) {
    let mut w = ExecutionWrangler::new_default(bench);

    // create an owner account
    let addrs =
        create_accounts(&mut w, faucet_id, 1, TokenAmount::from_whole(10_000), SignatureType::BLS)
            .unwrap();

    let (owner, worker) = (addrs[0].clone(), addrs[0].clone());
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
    let (miner_id, miner_addr) = create_miner(
        &mut w,
        owner.id,
        worker.id,
        seal_proof.registered_window_post_proof().unwrap(),
        TokenAmount::from_whole(10_000),
    )
    .unwrap();
    w.set_epoch(200);

    // precommit and advance to prove commit time
    let sector_number: SectorNumber = 100;
    precommit_sectors(
        &mut w,
        1,
        1,
        &worker.id_addr(),
        &miner_addr,
        seal_proof,
        sector_number,
        true,
        None,
    );

    let balances = get_miner_balance(&mut w, miner_id);
    assert!(balances.pre_commit_deposit.is_positive());

    let prove_time = w.epoch() + Policy::default().pre_commit_challenge_delay + 1;
    advance_by_deadline_to_epoch(&mut w, &miner_addr, prove_time);

    // prove commit, cron, advance to post time
    let prove_params = ProveCommitSectorParams { sector_number, proof: vec![] };
    let _prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
    println!("Running prove_commit_sector");
    let res = apply_ok(
        &mut w,
        worker.id_addr(),
        miner_addr,
        TokenAmount::zero(),
        MinerMethod::ProveCommitSector as u64,
        &prove_params,
    )
    .unwrap();
    assert_eq!(ExitCode::OK, res.receipt.exit_code, "ProveCommitSector failed {:?}", res);

    println!("Running cron job");
    let res = w
        .execute_implicit(
            SYSTEM_ACTOR_ADDR,
            CRON_ACTOR_ADDR,
            CronMethod::EpochTick as u64,
            RawBytes::default(),
            TokenAmount::zero(),
        )
        .unwrap();
    assert_eq!(ExitCode::OK, res.receipt.exit_code);

    let analysis = TraceAnalysis::build(res.trace);
    println!("{}", analysis.format_spans());

    // pcd is released ip is added
    let balances = get_miner_balance(&mut w, miner_id);
    assert!(balances.initial_pledge.is_positive());
    assert!(balances.pre_commit_deposit.is_zero());

    println!("PCD verified, initial pledge is positive {}", balances.initial_pledge);

    // power unproven so network stats are the same
    // let network_stats = v.get_network_stats();
    // assert!(network_stats.total_bytes_committed.is_zero());
    // assert!(network_stats.total_pledge_collateral.is_positive());

    let (deadline_info, partition_index) =
        advance_to_proving_deadline(&mut w, &miner_addr, sector_number);

    println!("Setup complete");

    (
        w,
        MinerInfo {
            seal_proof,
            worker: worker.id_addr(),
            owner: owner.id_addr(),
            miner_id: Address::new_id(miner_id),
            miner_robust: miner_addr,
        },
        SectorInfo { number: sector_number, deadline_info, partition_index },
    )
}

pub fn submit_windowed_post(
    w: &mut ExecutionWrangler,
    worker: &Address,
    maddr: &Address,
    dline_info: DeadlineInfo,
    partition_idx: u64,
    _new_power: Option<PowerPair>,
) {
    let params = SubmitWindowedPoStParams {
        deadline: dline_info.index,
        partitions: vec![PoStPartition { index: partition_idx, skipped: BitField::new() }],
        proofs: vec![PoStProof {
            post_proof: RegisteredPoStProof::StackedDRGWindow32GiBV1P1,
            proof_bytes: vec![],
        }],
        chain_commit_epoch: dline_info.challenge,
        chain_commit_rand: Randomness(TEST_VM_RAND_ARRAY.into()),
    };
    let result = apply_ok(
        w,
        *worker,
        *maddr,
        TokenAmount::zero(),
        MinerMethod::SubmitWindowedPoSt as u64,
        &params,
    )
    .unwrap();

    // println!("{}", result.trace.format());
    let analysis = TraceAnalysis::build(result.trace);
    println!("{}", analysis.format_spans());
}

#[test]
fn submit_post_succeeds() {
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        MemoryBlockstore::new(),
        FakeExterns::new(),
        NetworkVersion::V18,
        StateTreeVersion::V5,
        actors_v11::BUNDLE_CAR,
    )
    .unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let mut bench = builder.build().unwrap();
    let (mut w, miner_info, sector_info) = setup(&mut *bench, genesis.faucet_id);

    // submit post
    let st = w.find_actor_state::<MinerState>(miner_info.miner_id.id().unwrap()).unwrap().unwrap();
    let sector =
        st.get_sector(&BlockstoreWrapper::new(w.store()), sector_info.number).unwrap().unwrap();
    let sector_power = power_for_sector(miner_info.seal_proof.sector_size().unwrap(), &sector);
    submit_windowed_post(
        &mut w,
        &miner_info.worker,
        &miner_info.miner_id,
        sector_info.deadline_info,
        sector_info.partition_index,
        Some(sector_power.clone()),
    );
    println!("Submitted windowed PoSt");

    let balances = get_miner_balance(&mut w, miner_info.miner_id.id().unwrap());
    assert!(balances.initial_pledge.is_positive(), "{:?}", balances);
    let p_st =
        w.find_actor_state::<PowerState>(STORAGE_POWER_ACTOR_ADDR.id().unwrap()).unwrap().unwrap();
    assert_eq!(sector_power.raw, p_st.total_bytes_committed);
    println!("Sector power increased");
}
