use cid::Cid;
use fil_actor_cron::Method as CronMethod;

use fil_actor_miner::{
    max_prove_commit_duration, new_deadline_info, power_for_sector, CompactCommD, DeadlineInfo,
    Method as MinerMethod, PoStPartition, PowerPair, PreCommitSectorBatchParams,
    PreCommitSectorBatchParams2, PreCommitSectorParams, ProveCommitSectorParams,
    SectorPreCommitInfo, SectorPreCommitOnChainInfo, State as MinerState, SubmitWindowedPoStParams,
};
use fil_actor_power::State as PowerState;

use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{CRON_ACTOR_ADDR, STORAGE_POWER_ACTOR_ADDR, SYSTEM_ACTOR_ADDR};
use fvm_ipld_bitfield::BitField;
use fvm_ipld_blockstore::{Blockstore, MemoryBlockstore};
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::{ChainEpoch, QuantSpec};
use fvm_shared::commcid::FIL_COMMITMENT_SEALED;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;

use fvm_shared::error::ExitCode;
use fvm_shared::randomness::Randomness;
use fvm_shared::sector::{PoStProof, RegisteredPoStProof, RegisteredSealProof, SectorNumber};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::ActorID;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;

use crate::util::*;
use crate::workflows::*;
mod util;

#[derive(Debug)]
pub struct MinerBalances {
    pub available_balance: TokenAmount,
    pub vesting_balance: TokenAmount,
    pub initial_pledge: TokenAmount,
    pub pre_commit_deposit: TokenAmount,
}
struct SectorInfo {
    number: SectorNumber,
    deadline_info: DeadlineInfo,
    partition_index: u64,
}

struct MinerInfo {
    seal_proof: RegisteredSealProof,
    _owner: Address,
    worker: Address,
    miner_id: Address,
    _miner_robust: Address,
}

struct BlockstoreWrapper<'a>(&'a dyn Blockstore);

impl<'a> Blockstore for BlockstoreWrapper<'a> {
    fn get(&self, k: &Cid) -> anyhow::Result<Option<Vec<u8>>> {
        self.0.get(k)
    }

    fn put_keyed(&self, k: &Cid, block: &[u8]) -> anyhow::Result<()> {
        self.0.put_keyed(k, block)
    }
}

pub fn make_sealed_cid(input: &[u8]) -> Cid {
    make_cid_poseidon(input, FIL_COMMITMENT_SEALED)
}

pub fn make_cid_poseidon(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::PoseidonFake)
}

pub fn sector_deadline(w: &mut ExecutionWrangler, m: &Address, s: SectorNumber) -> (u64, u64) {
    let m = w.resolve_address(m).unwrap().unwrap();
    let st: MinerState = w.find_actor_state(m).unwrap().unwrap();
    st.find_sector(&Policy::default(), &BlockstoreWrapper(w.bench.borrow().store()), s).unwrap()
}

pub fn advance_to_proving_deadline(
    w: &mut ExecutionWrangler,
    maddr: &Address,
    s: SectorNumber,
) -> (DeadlineInfo, u64) {
    let (d, p) = sector_deadline(w, maddr, s);
    let dline_info = advance_by_deadline_to_index(w, maddr, d);
    w.set_epoch(dline_info.open);
    (dline_info, p)
}

pub fn advance_by_deadline_to_index(
    w: &mut ExecutionWrangler,
    maddr: &Address,
    i: u64,
) -> DeadlineInfo {
    advance_by_deadline(w, maddr, |dline_info| dline_info.index != i)
}

#[allow(clippy::too_many_arguments)]
pub fn precommit_sectors_v2(
    w: &mut ExecutionWrangler,
    count: u64,
    batch_size: i64,
    worker: &Address,
    maddr: &Address,
    seal_proof: RegisteredSealProof,
    sector_number_base: SectorNumber,
    expect_cron_enroll: bool,
    exp: Option<ChainEpoch>,
    v2: bool,
) -> Vec<SectorPreCommitOnChainInfo> {
    let mid = w.resolve_address(maddr).unwrap().unwrap();
    // let invocs_common = || -> Vec<ExpectInvocation> {
    //     vec![
    //         ExpectInvocation {
    //             to: REWARD_ACTOR_ADDR,
    //             method: RewardMethod::ThisEpochReward as u64,
    //             ..Default::default()
    //         },
    //         ExpectInvocation {
    //             to: STORAGE_POWER_ACTOR_ADDR,
    //             method: PowerMethod::CurrentTotalPower as u64,
    //             ..Default::default()
    //         },
    //     ]
    // };
    // let invoc_first = || -> ExpectInvocation {
    //     ExpectInvocation {
    //         to: STORAGE_POWER_ACTOR_ADDR,
    //         method: PowerMethod::EnrollCronEvent as u64,
    //         ..Default::default()
    //     }
    // };
    // let invoc_net_fee = |fee: TokenAmount| -> ExpectInvocation {
    //     ExpectInvocation {
    //         to: BURNT_FUNDS_ACTOR_ADDR,
    //         method: METHOD_SEND,
    //         value: Some(fee),
    //         ..Default::default()
    //     }
    // };

    let expiration = match exp {
        None => {
            w.epoch()
                + Policy::default().min_sector_expiration
                + max_prove_commit_duration(&Policy::default(), seal_proof).unwrap()
        }
        Some(e) => e,
    };

    let mut sector_idx = 0u64;
    while sector_idx < count {
        let msg_sector_idx_base = sector_idx;
        // let mut invocs = invocs_common();
        if !v2 {
            let mut param_sectors = Vec::<PreCommitSectorParams>::new();
            let mut j = 0;
            while j < batch_size && sector_idx < count {
                let sector_number = sector_number_base + sector_idx;
                param_sectors.push(PreCommitSectorParams {
                    seal_proof,
                    sector_number,
                    sealed_cid: make_sealed_cid(format!("sn: {}", sector_number).as_bytes()),
                    seal_rand_epoch: w.epoch() - 1,
                    deal_ids: vec![],
                    expiration,
                    ..Default::default()
                });
                sector_idx += 1;
                j += 1;
            }
            if param_sectors.len() > 1 {
                // invocs.push(invoc_net_fee(aggregate_pre_commit_network_fee(
                //     param_sectors.len() as i64,
                //     &TokenAmount::zero(),
                // )));
            }
            if expect_cron_enroll && msg_sector_idx_base == 0 {
                // invocs.push(invoc_first());
            }

            let res = apply_ok(
                w,
                *worker,
                *maddr,
                TokenAmount::zero(),
                MinerMethod::PreCommitSectorBatch as u64,
                &PreCommitSectorBatchParams { sectors: param_sectors.clone() },
            )
            .unwrap();
            assert_eq!(
                ExitCode::OK,
                res.receipt.exit_code,
                "PreCommitSectorBatch failed {:?}",
                res
            );
            // let expect = ExpectInvocation {
            //     to: mid,
            //     method: MinerMethod::PreCommitSectorBatch as u64,
            //     params: Some(
            //         IpldBlock::serialize_cbor(&PreCommitSectorBatchParams {
            //             sectors: param_sectors,
            //         })
            //         .unwrap(),
            //     ),
            //     subinvocs: Some(invocs),
            //     ..Default::default()
            // };
            // expect.matches(v.take_invocations().last().unwrap())
        } else {
            let mut param_sectors = Vec::<SectorPreCommitInfo>::new();
            let mut j = 0;
            while j < batch_size && sector_idx < count {
                let sector_number = sector_number_base + sector_idx;
                param_sectors.push(SectorPreCommitInfo {
                    seal_proof,
                    sector_number,
                    sealed_cid: make_sealed_cid(format!("sn: {}", sector_number).as_bytes()),
                    seal_rand_epoch: w.epoch() - 1,
                    deal_ids: vec![],
                    expiration,
                    unsealed_cid: CompactCommD::new(None),
                });
                sector_idx += 1;
                j += 1;
            }
            if param_sectors.len() > 1 {
                // invocs.push(invoc_net_fee(aggregate_pre_commit_network_fee(
                //     param_sectors.len() as i64,
                //     &TokenAmount::zero(),
                // )));
            }
            if expect_cron_enroll && msg_sector_idx_base == 0 {
                // invocs.push(invoc_first());
            }

            let res = apply_ok(
                w,
                *worker,
                *maddr,
                TokenAmount::zero(),
                MinerMethod::PreCommitSectorBatch2 as u64,
                &PreCommitSectorBatchParams2 { sectors: param_sectors.clone() },
            )
            .unwrap();
            assert_eq!(
                ExitCode::OK,
                res.receipt.exit_code,
                "PreCommitSectorBatch2 failed {:?}",
                res
            );

            // let expect = ExpectInvocation {
            //     to: mid,
            //     method: MinerMethod::PreCommitSectorBatch2 as u64,
            //     params: Some(
            //         IpldBlock::serialize_cbor(&PreCommitSectorBatchParams2 {
            //             sectors: param_sectors,
            //         })
            //         .unwrap(),
            //     ),
            //     subinvocs: Some(invocs),
            //     ..Default::default()
            // };
            // expect.matches(v.take_invocations().last().unwrap())
        }
    }
    // extract chain state
    let mstate: MinerState = w.find_actor_state(mid).unwrap().unwrap();
    (0..count)
        .map(|i| {
            mstate
                .get_precommitted_sector(
                    &BlockstoreWrapper(w.bench.borrow().store()),
                    sector_number_base + i,
                )
                .unwrap()
                .unwrap()
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn precommit_sectors(
    w: &mut ExecutionWrangler,
    count: u64,
    batch_size: i64,
    worker: &Address,
    maddr: &Address,
    seal_proof: RegisteredSealProof,
    sector_number_base: SectorNumber,
    expect_cron_enroll: bool,
    exp: Option<ChainEpoch>,
) -> Vec<SectorPreCommitOnChainInfo> {
    precommit_sectors_v2(
        w,
        count,
        batch_size,
        worker,
        maddr,
        seal_proof,
        sector_number_base,
        expect_cron_enroll,
        exp,
        false,
    )
}

pub fn advance_by_deadline_to_epoch(
    w: &mut ExecutionWrangler,
    maddr: &Address,
    e: ChainEpoch,
) -> DeadlineInfo {
    // keep advancing until the epoch of interest is within the deadline
    // if e is dline.last() == dline.close -1 cron is not run
    let dline_info = advance_by_deadline(w, maddr, |dline_info| dline_info.close < e);
    w.set_epoch(e);
    dline_info
}

fn advance_by_deadline<F>(w: &mut ExecutionWrangler, maddr: &Address, more: F) -> DeadlineInfo
where
    F: Fn(DeadlineInfo) -> bool,
{
    loop {
        let dline_info = miner_dline_info(w, maddr);
        if !more(dline_info) {
            return dline_info;
        }
        w.set_epoch(dline_info.last());
        cron_tick(w);
        let next = w.epoch() + 1;
        w.set_epoch(next);
    }
}

pub fn miner_dline_info(w: &mut ExecutionWrangler, maddr: &Address) -> DeadlineInfo {
    let m_id = w.resolve_address(maddr).unwrap().unwrap();
    let st: MinerState = w.find_actor_state(m_id).unwrap().unwrap();
    new_deadline_info_from_offset_and_epoch(&Policy::default(), st.proving_period_start, w.epoch())
}

pub fn new_deadline_info_from_offset_and_epoch(
    policy: &Policy,
    period_start_seed: ChainEpoch,
    current_epoch: ChainEpoch,
) -> DeadlineInfo {
    let q = QuantSpec { unit: policy.wpost_proving_period, offset: period_start_seed };
    let current_period_start = q.quantize_down(current_epoch);
    let current_deadline_idx = ((current_epoch - current_period_start)
        / policy.wpost_challenge_window) as u64
        % policy.wpost_period_deadlines;
    new_deadline_info(policy, current_period_start, current_deadline_idx, current_epoch)
}

pub fn cron_tick(w: &mut ExecutionWrangler) {
    println!("cron_tick: epoch {}", w.epoch());
    let res = w
        .execute_implicit(
            SYSTEM_ACTOR_ADDR,
            CRON_ACTOR_ADDR,
            CronMethod::EpochTick as u64,
            RawBytes::default(),
            TokenAmount::zero(),
        )
        .unwrap();
    if !res.receipt.exit_code.is_success() {
        println!("cron_tick: failed: {:?}", res.receipt);
        println!("cron_tick: failed: {:?}", res.trace.format());
    }
}

fn setup() -> (ExecutionWrangler, MinerInfo, SectorInfo) {
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
    let bench = builder.build().unwrap();
    let mut w = ExecutionWrangler::new_default(bench);

    // create an owner account
    let addrs = create_accounts(
        &mut w,
        genesis.faucet_id,
        1,
        TokenAmount::from_whole(10_000),
        SignatureType::BLS,
    )
    .unwrap();
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
    let (owner, worker) = (addrs[0].clone(), addrs[0].clone());
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
    let analysis = TraceAnalysis::build(res.trace);
    println!("analysis {}", analysis.format_spans());
    // ExpectInvocation {
    //     to: miner_id,
    //     method: MinerMethod::ProveCommitSector as u64,
    //     params: Some(prove_params_ser),
    //     subinvocs: Some(vec![ExpectInvocation {
    //         to: STORAGE_POWER_ACTOR_ADDR,
    //         method: PowerMethod::SubmitPoRepForBulkVerify as u64,
    //         ..Default::default()
    //     }]),
    //     ..Default::default()
    // }
    // .matches(v.take_invocations().last().unwrap());
    println!("running cron job");
    let res = w
        .execute_implicit(
            SYSTEM_ACTOR_ADDR,
            CRON_ACTOR_ADDR,
            CronMethod::EpochTick as u64,
            RawBytes::default(),
            TokenAmount::zero(),
        )
        .unwrap();
    // println!("res {:?}", res);
    let analysis = TraceAnalysis::build(res.trace);
    println!("analysis {}", analysis.format_spans());
    assert_eq!(ExitCode::OK, res.receipt.exit_code);

    // ExpectInvocation {
    //     to: CRON_ACTOR_ADDR,
    //     method: CronMethod::EpochTick as u64,
    //     subinvocs: Some(vec![
    //         ExpectInvocation {
    //             to: STORAGE_POWER_ACTOR_ADDR,
    //             method: PowerMethod::OnEpochTickEnd as u64,
    //             subinvocs: Some(vec![
    //                 ExpectInvocation {
    //                     to: REWARD_ACTOR_ADDR,
    //                     method: RewardMethod::ThisEpochReward as u64,
    //                     ..Default::default()
    //                 },
    //                 FIXME: This invocation is missing?
    //                 ExpectInvocation {
    //                     to: miner_id,
    //                     method: MinerMethod::ConfirmSectorProofsValid as u64,
    //                     subinvocs: Some(vec![ExpectInvocation {
    //                         to: STORAGE_POWER_ACTOR_ADDR,
    //                         method: PowerMethod::UpdatePledgeTotal as u64,
    //                         ..Default::default()
    //                     }]),
    //                     ..Default::default()
    //                 },
    //                 ExpectInvocation {
    //                     to: REWARD_ACTOR_ADDR,
    //                     method: RewardMethod::UpdateNetworkKPI as u64,
    //                     ..Default::default()
    //                 },
    //             ]),
    //             ..Default::default()
    //         },
    //         ExpectInvocation {
    //             to: STORAGE_MARKET_ACTOR_ADDR,
    //             method: MarketMethod::CronTick as u64,
    //             ..Default::default()
    //         },
    //     ]),
    //     ..Default::default()
    // }
    // .matches(v.take_invocations().last().unwrap());

    // pcd is released ip is added
    let balances = get_miner_balance(&mut w, miner_id);
    assert!(balances.initial_pledge.is_positive());
    assert!(balances.pre_commit_deposit.is_zero());

    // power unproven so network stats are the same

    // let network_stats = v.get_network_stats();
    // assert!(network_stats.total_bytes_committed.is_zero());
    // assert!(network_stats.total_pledge_collateral.is_positive());

    let (deadline_info, partition_index) =
        advance_to_proving_deadline(&mut w, &miner_addr, sector_number);
    (
        w,
        MinerInfo {
            seal_proof,
            worker: worker.id_addr(),
            _owner: owner.id_addr(),
            miner_id: Address::new_id(miner_id),
            _miner_robust: miner_addr,
        },
        SectorInfo { number: sector_number, deadline_info, partition_index },
    )
}

fn get_miner_balance(w: &mut ExecutionWrangler, miner_id: ActorID) -> MinerBalances {
    let a = w.find_actor(miner_id).unwrap().unwrap();
    let st: MinerState = w.find_actor_state(miner_id).unwrap().unwrap();
    MinerBalances {
        available_balance: st.get_available_balance(&a.balance).unwrap(),
        vesting_balance: st.locked_funds,
        initial_pledge: st.initial_pledge,
        pre_commit_deposit: st.pre_commit_deposits,
    }
}
pub const TEST_VM_RAND_ARRAY: [u8; 32] = [
    1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32,
];

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
    apply_ok(
        w,
        *worker,
        *maddr,
        TokenAmount::zero(),
        MinerMethod::SubmitWindowedPoSt as u64,
        &params,
    )
    .unwrap();
    // let mut subinvocs = None;
    // if let Some(new_pow) = new_power {
    //     if new_pow == PowerPair::zero() {
    //         subinvocs = Some(vec![])
    //     } else {
    //         let update_power_params = IpldBlock::serialize_cbor(&UpdateClaimedPowerParams {
    //             raw_byte_delta: new_pow.raw,
    //             quality_adjusted_delta: new_pow.qa,
    //         })
    //         .unwrap();
    //         // subinvocs = Some(vec![ExpectInvocation {
    //         //     to: STORAGE_POWER_ACTOR_ADDR,
    //         //     method: PowerMethod::UpdateClaimedPower as u64,
    //         //     params: Some(update_power_params),
    //         //     ..Default::default()
    //         // }]);
    //     }
    // }

    // ExpectInvocation {
    //     to: *maddr,
    //     method: MinerMethod::SubmitWindowedPoSt as u64,
    //     subinvocs,
    //     ..Default::default()
    // }
    // .matches(w.take_invocations().last().unwrap());
}

#[test]
fn submit_post_succeeds() {
    let (mut w, miner_info, sector_info) = setup();
    // submit post
    let st = w.find_actor_state::<MinerState>(miner_info.miner_id.id().unwrap()).unwrap().unwrap();
    let sector = st
        .get_sector(&BlockstoreWrapper(w.bench.borrow().store()), sector_info.number)
        .unwrap()
        .unwrap();
    let sector_power = power_for_sector(miner_info.seal_proof.sector_size().unwrap(), &sector);
    submit_windowed_post(
        &mut w,
        &miner_info.worker,
        &miner_info.miner_id,
        sector_info.deadline_info,
        sector_info.partition_index,
        Some(sector_power.clone()),
    );
    let balances = get_miner_balance(&mut w, miner_info.miner_id.id().unwrap());
    assert!(balances.initial_pledge.is_positive(), "{:?}", balances);
    let p_st =
        w.find_actor_state::<PowerState>(STORAGE_POWER_ACTOR_ADDR.id().unwrap()).unwrap().unwrap();
    assert_eq!(sector_power.raw, p_st.total_bytes_committed);

    // v.assert_state_invariants();
}

// #[test]
// fn skip_sector() {
//     let store = MemoryBlockstore::new();
//     let (v, miner_info, sector_info) = setup(&store);
//     // submit post, but skip the only sector in it
//     let params = SubmitWindowedPoStParams {
//         deadline: sector_info.deadline_info.index,
//         partitions: vec![PoStPartition {
//             index: sector_info.partition_index,
//             skipped: BitField::try_from_bits([sector_info.number].iter().copied()).unwrap(),
//         }],
//         proofs: vec![PoStProof {
//             post_proof: miner_info.seal_proof.registered_window_post_proof().unwrap(),
//             proof_bytes: vec![],
//         }],
//         chain_commit_epoch: sector_info.deadline_info.challenge,
//         chain_commit_rand: Randomness(TEST_VM_RAND_ARRAY.into()),
//     };

//     // PoSt is rejected for skipping all sectors.
//     apply_code(
//         &v,
//         &miner_info.worker,
//         &miner_info.miner_id,
//         &TokenAmount::zero(),
//         MinerMethod::SubmitWindowedPoSt as u64,
//         Some(params),
//         ExitCode::USR_ILLEGAL_ARGUMENT,
//     );

//     // miner still has initial pledge
//     let balances = v.get_miner_balance(&miner_info.miner_id);
//     assert!(balances.initial_pledge.is_positive());

//     // power unproven so network stats are the same
//     let network_stats = v.get_network_stats();
//     assert!(network_stats.total_bytes_committed.is_zero());
//     assert!(network_stats.total_pledge_collateral.is_positive());

//     v.assert_state_invariants();
// }

// #[test]
// fn missed_first_post_deadline() {
//     let store = MemoryBlockstore::new();
//     let (v, miner_info, sector_info) = setup(&store);

//     // move to proving period end
//     v.set_epoch(sector_info.deadline_info.last());

//     // Run cron to detect missing PoSt

//     apply_ok(
//         &v,
//         &SYSTEM_ACTOR_ADDR,
//         &CRON_ACTOR_ADDR,
//         &TokenAmount::zero(),
//         CronMethod::EpochTick as u64,
//         None::<RawBytes>,
//     );

//     ExpectInvocation {
//         to: CRON_ACTOR_ADDR,
//         method: CronMethod::EpochTick as u64,
//         params: None,
//         subinvocs: Some(vec![
//             ExpectInvocation {
//                 to: STORAGE_POWER_ACTOR_ADDR,
//                 method: PowerMethod::OnEpochTickEnd as u64,
//                 subinvocs: Some(vec![
//                     ExpectInvocation {
//                         to: REWARD_ACTOR_ADDR,
//                         method: RewardMethod::ThisEpochReward as u64,
//                         ..Default::default()
//                     },
//                     ExpectInvocation {
//                         to: miner_info.miner_id,
//                         method: MinerMethod::OnDeferredCronEvent as u64,
//                         subinvocs: Some(vec![ExpectInvocation {
//                             to: STORAGE_POWER_ACTOR_ADDR,
//                             method: PowerMethod::EnrollCronEvent as u64,
//                             ..Default::default()
//                         }]),
//                         ..Default::default()
//                     },
//                     ExpectInvocation {
//                         to: REWARD_ACTOR_ADDR,
//                         method: RewardMethod::UpdateNetworkKPI as u64,
//                         ..Default::default()
//                     },
//                 ]),
//                 ..Default::default()
//             },
//             ExpectInvocation {
//                 to: STORAGE_MARKET_ACTOR_ADDR,
//                 method: MarketMethod::CronTick as u64,
//                 ..Default::default()
//             },
//         ]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());

//     // power unproven so network stats are the same
//     let network_stats = v.get_network_stats();
//     assert!(network_stats.total_bytes_committed.is_zero());
//     assert!(network_stats.total_pledge_collateral.is_positive());

//     v.expect_state_invariants(
//         &[invariant_failure_patterns::REWARD_STATE_EPOCH_MISMATCH.to_owned()],
//     );
// }

// #[test]
// fn overdue_precommit() {
//     let store = MemoryBlockstore::new();
//     let policy = &Policy::default();
//     let v = TestVM::<MemoryBlockstore>::new_with_singletons(&store);
//     let addrs = create_accounts(&v, 1, &TokenAmount::from_whole(10_000));
//     let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
//     let (owner, worker) = (addrs[0], addrs[0]);
//     let id_addr = create_miner(
//         &v,
//         &owner,
//         &worker,
//         seal_proof.registered_window_post_proof().unwrap(),
//         &TokenAmount::from_whole(10_000),
//     )
//     .0;
//     let v = v.with_epoch(200);

//     // precommit and advance to prove commit time
//     let sector_number: SectorNumber = 100;
//     let precommit =
//         precommit_sectors(&v, 1, 1, &worker, &id_addr, seal_proof, sector_number, true, None)
//             .get(0)
//             .unwrap()
//             .clone();

//     let balances = v.get_miner_balance(&id_addr);
//     assert!(balances.pre_commit_deposit.is_positive());

//     let prove_time = v.epoch() + max_prove_commit_duration(policy, seal_proof).unwrap() + 1;
//     advance_by_deadline_to_epoch(&v, &id_addr, prove_time);

//     //
//     // overdue precommit
//     //

//     // advance time to precommit clean up epoch
//     let cleanup_time = prove_time + policy.expired_pre_commit_clean_up_delay;
//     let deadline_info = advance_by_deadline_to_epoch(&v, &id_addr, cleanup_time);

//     // advance one more deadline so precommit clean up is reached
//     v.set_epoch(deadline_info.close);

//     // run cron which should clean up precommit
//     apply_ok(
//         &v,
//         &SYSTEM_ACTOR_ADDR,
//         &CRON_ACTOR_ADDR,
//         &TokenAmount::zero(),
//         CronMethod::EpochTick as u64,
//         None::<RawBytes>,
//     );

//     ExpectInvocation {
//         to: CRON_ACTOR_ADDR,
//         method: CronMethod::EpochTick as u64,
//         params: None,
//         subinvocs: Some(vec![
//             ExpectInvocation {
//                 to: STORAGE_POWER_ACTOR_ADDR,
//                 method: PowerMethod::OnEpochTickEnd as u64,
//                 subinvocs: Some(vec![
//                     ExpectInvocation {
//                         to: REWARD_ACTOR_ADDR,
//                         method: RewardMethod::ThisEpochReward as u64,
//                         ..Default::default()
//                     },
//                     ExpectInvocation {
//                         to: id_addr,
//                         method: MinerMethod::OnDeferredCronEvent as u64,
//                         subinvocs: Some(vec![
//                             ExpectInvocation {
//                                 // The call to burnt funds indicates the overdue precommit has been penalized
//                                 to: BURNT_FUNDS_ACTOR_ADDR,
//                                 method: METHOD_SEND,
//                                 value: Option::from(precommit.pre_commit_deposit),
//                                 ..Default::default()
//                             },
//                             // No re-enrollment of cron because burning of PCD discontinues miner cron scheduling
//                         ]),
//                         ..Default::default()
//                     },
//                     ExpectInvocation {
//                         to: REWARD_ACTOR_ADDR,
//                         method: RewardMethod::UpdateNetworkKPI as u64,
//                         ..Default::default()
//                     },
//                 ]),
//                 ..Default::default()
//             },
//             ExpectInvocation {
//                 to: STORAGE_MARKET_ACTOR_ADDR,
//                 method: MarketMethod::CronTick as u64,
//                 ..Default::default()
//             },
//         ]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());

//     let balances = v.get_miner_balance(&id_addr);
//     assert!(balances.initial_pledge.is_zero());
//     assert!(balances.pre_commit_deposit.is_zero());

//     let network_stats = v.get_network_stats();
//     assert!(network_stats.total_bytes_committed.is_zero());
//     assert!(network_stats.total_pledge_collateral.is_zero());
//     assert!(network_stats.total_raw_byte_power.is_zero());
//     assert!(network_stats.total_quality_adj_power.is_zero());

//     v.expect_state_invariants(
//         &[invariant_failure_patterns::REWARD_STATE_EPOCH_MISMATCH.to_owned()],
//     );
// }

// #[test]
// fn aggregate_bad_sector_number() {
//     let store = MemoryBlockstore::new();
//     let v = TestVM::<MemoryBlockstore>::new_with_singletons(&store);
//     let addrs = create_accounts(&v, 1, &TokenAmount::from_whole(10_000));
//     let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
//     let (owner, worker) = (addrs[0], addrs[0]);
//     let (id_addr, robust_addr) = create_miner(
//         &v,
//         &owner,
//         &worker,
//         seal_proof.registered_window_post_proof().unwrap(),
//         &TokenAmount::from_whole(10_000),
//     );
//     let v = v.with_epoch(200);
//     let policy = &Policy::default();

//     //
//     // precommit good sectors
//     //

//     // precommit and advance to prove commit time
//     let sector_number: SectorNumber = 100;
//     let mut precommited_sector_nos = BitField::try_from_bits(
//         precommit_sectors(
//             &v,
//             4,
//             policy.pre_commit_sector_batch_max_size as i64,
//             &worker,
//             &id_addr,
//             seal_proof,
//             sector_number,
//             true,
//             None,
//         )
//         .iter()
//         .map(|info| info.info.sector_number),
//     )
//     .unwrap();

//     //
//     // attempt proving with invalid args
//     //

//     // advance time to max seal duration

//     let prove_time = v.epoch() + policy.pre_commit_challenge_delay + 1;
//     advance_by_deadline_to_epoch(&v, &id_addr, prove_time);

//     // construct invalid bitfield with a non-committed sector number > abi.MaxSectorNumber

//     precommited_sector_nos.set(MAX_SECTOR_NUMBER + 1);

//     let prove_params = ProveCommitAggregateParams {
//         sector_numbers: precommited_sector_nos,
//         aggregate_proof: vec![],
//     };
//     let prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
//     apply_code(
//         &v,
//         &worker,
//         &robust_addr,
//         &TokenAmount::zero(),
//         MinerMethod::ProveCommitAggregate as u64,
//         Some(prove_params),
//         ExitCode::USR_ILLEGAL_ARGUMENT,
//     );
//     ExpectInvocation {
//         to: id_addr,
//         method: MinerMethod::ProveCommitAggregate as u64,
//         params: Some(prove_params_ser),
//         subinvocs: Some(vec![]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());
//     v.expect_state_invariants(
//         &[invariant_failure_patterns::REWARD_STATE_EPOCH_MISMATCH.to_owned()],
//     );
// }

// #[test]
// fn aggregate_size_limits() {
//     let oversized_batch = 820;
//     let store = MemoryBlockstore::new();
//     let v = TestVM::<MemoryBlockstore>::new_with_singletons(&store);
//     let addrs = create_accounts(&v, 1, &TokenAmount::from_whole(100_000));
//     let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
//     let (owner, worker) = (addrs[0], addrs[0]);
//     let (id_addr, robust_addr) = create_miner(
//         &v,
//         &owner,
//         &worker,
//         seal_proof.registered_window_post_proof().unwrap(),
//         &TokenAmount::from_whole(100_000),
//     );
//     let v = v.with_epoch(200);
//     let policy = &Policy::default();

//     //
//     // precommit good sectors
//     //

//     // precommit and advance to prove commit time
//     let sector_number: SectorNumber = 100;
//     let precommited_sector_nos = BitField::try_from_bits(
//         precommit_sectors(
//             &v,
//             oversized_batch,
//             policy.pre_commit_sector_batch_max_size as i64,
//             &worker,
//             &id_addr,
//             seal_proof,
//             sector_number,
//             true,
//             None,
//         )
//         .iter()
//         .map(|info| info.info.sector_number),
//     )
//     .unwrap();

//     //
//     // attempt proving with invalid args
//     //

//     // advance time to max seal duration

//     let prove_time = v.epoch() + policy.pre_commit_challenge_delay + 1;
//     advance_by_deadline_to_epoch(&v, &id_addr, prove_time);

//     // Fail with too many sectors

//     let mut prove_params = ProveCommitAggregateParams {
//         sector_numbers: precommited_sector_nos.clone(),
//         aggregate_proof: vec![],
//     };
//     let mut prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
//     apply_code(
//         &v,
//         &worker,
//         &robust_addr,
//         &TokenAmount::zero(),
//         MinerMethod::ProveCommitAggregate as u64,
//         Some(prove_params),
//         ExitCode::USR_ILLEGAL_ARGUMENT,
//     );
//     ExpectInvocation {
//         to: id_addr,
//         method: MinerMethod::ProveCommitAggregate as u64,
//         params: Some(prove_params_ser),
//         subinvocs: Some(vec![]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());

//     // Fail with too few sectors

//     let too_few_sector_nos_bf =
//         precommited_sector_nos.slice(0, policy.min_aggregated_sectors - 1).unwrap();
//     prove_params = ProveCommitAggregateParams {
//         sector_numbers: too_few_sector_nos_bf,
//         aggregate_proof: vec![],
//     };
//     prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
//     apply_code(
//         &v,
//         &worker,
//         &robust_addr,
//         &TokenAmount::zero(),
//         MinerMethod::ProveCommitAggregate as u64,
//         Some(prove_params),
//         ExitCode::USR_ILLEGAL_ARGUMENT,
//     );
//     ExpectInvocation {
//         to: id_addr,
//         method: MinerMethod::ProveCommitAggregate as u64,
//         params: Some(prove_params_ser),
//         subinvocs: Some(vec![]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());

//     // Fail with proof too big

//     let just_right_sectors_no_bf =
//         precommited_sector_nos.slice(0, policy.max_aggregated_sectors).unwrap();
//     prove_params = ProveCommitAggregateParams {
//         sector_numbers: just_right_sectors_no_bf,
//         aggregate_proof: vec![0; policy.max_aggregated_proof_size + 1],
//     };

//     prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
//     apply_code(
//         &v,
//         &worker,
//         &robust_addr,
//         &TokenAmount::zero(),
//         MinerMethod::ProveCommitAggregate as u64,
//         Some(prove_params),
//         ExitCode::USR_ILLEGAL_ARGUMENT,
//     );
//     ExpectInvocation {
//         to: id_addr,
//         method: MinerMethod::ProveCommitAggregate as u64,
//         params: Some(prove_params_ser),
//         subinvocs: Some(vec![]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());
//     v.expect_state_invariants(
//         &[invariant_failure_patterns::REWARD_STATE_EPOCH_MISMATCH.to_owned()],
//     );
// }

// #[test]
// fn aggregate_bad_sender() {
//     let store = MemoryBlockstore::new();
//     let v = TestVM::<MemoryBlockstore>::new_with_singletons(&store);
//     let addrs = create_accounts(&v, 2, &TokenAmount::from_whole(10_000));
//     let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
//     let (owner, worker) = (addrs[0], addrs[0]);
//     let (id_addr, robust_addr) = create_miner(
//         &v,
//         &owner,
//         &worker,
//         seal_proof.registered_window_post_proof().unwrap(),
//         &TokenAmount::from_whole(10_000),
//     );
//     let v = v.with_epoch(200);
//     let policy = &Policy::default();

//     //
//     // precommit good sectors
//     //

//     // precommit and advance to prove commit time
//     let sector_number: SectorNumber = 100;
//     let precommited_sector_nos = BitField::try_from_bits(
//         precommit_sectors(
//             &v,
//             4,
//             policy.pre_commit_sector_batch_max_size as i64,
//             &worker,
//             &id_addr,
//             seal_proof,
//             sector_number,
//             true,
//             None,
//         )
//         .iter()
//         .map(|info| info.info.sector_number),
//     )
//     .unwrap();

//     //
//     // attempt proving with invalid args
//     //

//     // advance time to max seal duration

//     let prove_time = v.epoch() + policy.pre_commit_challenge_delay + 1;
//     advance_by_deadline_to_epoch(&v, &id_addr, prove_time);

//     let prove_params = ProveCommitAggregateParams {
//         sector_numbers: precommited_sector_nos,
//         aggregate_proof: vec![],
//     };
//     let prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
//     apply_code(
//         &v,
//         &addrs[1],
//         &robust_addr,
//         &TokenAmount::zero(),
//         MinerMethod::ProveCommitAggregate as u64,
//         Some(prove_params),
//         ExitCode::USR_FORBIDDEN,
//     );
//     ExpectInvocation {
//         to: id_addr,
//         method: MinerMethod::ProveCommitAggregate as u64,
//         params: Some(prove_params_ser),
//         subinvocs: Some(vec![]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());
//     v.expect_state_invariants(
//         &[invariant_failure_patterns::REWARD_STATE_EPOCH_MISMATCH.to_owned()],
//     );
// }

// #[test]
// fn aggregate_one_precommit_expires() {
//     let store = MemoryBlockstore::new();
//     let v = TestVM::<MemoryBlockstore>::new_with_singletons(&store);
//     let addrs = create_accounts(&v, 1, &TokenAmount::from_whole(10_000));
//     let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
//     let (owner, worker) = (addrs[0], addrs[0]);
//     let (id_addr, robust_addr) = create_miner(
//         &v,
//         &owner,
//         &worker,
//         seal_proof.registered_window_post_proof().unwrap(),
//         &TokenAmount::from_whole(10_000),
//     );
//     let v = v.with_epoch(200);
//     let policy = &Policy::default();

//     //
//     // precommit sectors
//     //

//     let sector_number: SectorNumber = 100;

//     // early precommit
//     let early_precommit_time = v.epoch();
//     let early_precommits = precommit_sectors(
//         &v,
//         1,
//         policy.pre_commit_sector_batch_max_size as i64,
//         &worker,
//         &id_addr,
//         seal_proof,
//         sector_number,
//         true,
//         None,
//     );

//     let early_pre_commit_invalid =
//         early_precommit_time + max_prove_commit_duration(policy, seal_proof).unwrap() + 1;

//     advance_by_deadline_to_epoch(&v, &id_addr, early_pre_commit_invalid);

//     // later precommits

//     let later_precommits = precommit_sectors(
//         &v,
//         3,
//         policy.pre_commit_sector_batch_max_size as i64,
//         &worker,
//         &id_addr,
//         seal_proof,
//         sector_number + 1,
//         false,
//         None,
//     );

//     let all_precommits = [early_precommits, later_precommits].concat();

//     let sector_nos_bf =
//         BitField::try_from_bits(all_precommits.iter().map(|info| info.info.sector_number)).unwrap();

//     // Advance minimum epochs past later precommits for later commits to be valid

//     let prove_time = v.epoch() + policy.pre_commit_challenge_delay + 1;
//     let deadline_info = advance_by_deadline_to_epoch(&v, &id_addr, prove_time);
//     advance_by_deadline_to_epoch(&v, &id_addr, deadline_info.close);

//     // Assert that precommit should not yet be cleaned up. This makes fixing this test easier if parameters change.
//     assert!(
//         prove_time
//             < early_precommit_time
//                 + max_prove_commit_duration(policy, seal_proof).unwrap()
//                 + policy.expired_pre_commit_clean_up_delay
//     );

//     // Assert that we have a valid aggregate batch size
//     let agg_setors_count = sector_nos_bf.len();
//     assert!(
//         agg_setors_count >= policy.min_aggregated_sectors
//             && agg_setors_count < policy.max_aggregated_sectors
//     );

//     let prove_params =
//         ProveCommitAggregateParams { sector_numbers: sector_nos_bf, aggregate_proof: vec![] };
//     let prove_params_ser = IpldBlock::serialize_cbor(&prove_params).unwrap();
//     apply_ok(
//         &v,
//         &worker,
//         &robust_addr,
//         &TokenAmount::zero(),
//         MinerMethod::ProveCommitAggregate as u64,
//         Some(prove_params),
//     );
//     ExpectInvocation {
//         to: id_addr,
//         method: MinerMethod::ProveCommitAggregate as u64,
//         params: Some(prove_params_ser),
//         subinvocs: Some(vec![
//             ExpectInvocation {
//                 to: REWARD_ACTOR_ADDR,
//                 method: RewardMethod::ThisEpochReward as u64,
//                 ..Default::default()
//             },
//             ExpectInvocation {
//                 to: STORAGE_POWER_ACTOR_ADDR,
//                 method: PowerMethod::CurrentTotalPower as u64,
//                 ..Default::default()
//             },
//             ExpectInvocation {
//                 to: STORAGE_POWER_ACTOR_ADDR,
//                 method: PowerMethod::UpdatePledgeTotal as u64,
//                 ..Default::default()
//             },
//             ExpectInvocation {
//                 to: BURNT_FUNDS_ACTOR_ADDR,
//                 method: METHOD_SEND,
//                 ..Default::default()
//             },
//         ]),
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());

//     let balances = v.get_miner_balance(&id_addr);
//     assert!(balances.initial_pledge.is_positive());
//     assert!(balances.pre_commit_deposit.is_positive());

//     v.expect_state_invariants(
//         &[invariant_failure_patterns::REWARD_STATE_EPOCH_MISMATCH.to_owned()],
//     );
// }
