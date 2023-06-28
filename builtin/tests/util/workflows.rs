use fil_actor_cron::Method as CronMethod;
use fil_actor_market::{Method as MarketMethod, SectorDeals};
use fil_actor_miner::{
    DeadlineInfo, PoStPartition, PowerPair, SectorPreCommitOnChainInfo, SubmitWindowedPoStParams,
};
use fil_actor_multisig::{Method as MultisigMethod, ProposeParams};
use fil_actor_power::{CreateMinerParams, CreateMinerReturn};
use fil_actor_verifreg::{AddVerifiedClientParams, Method as VerifregMethod, VerifierParams};
use fil_actors_runtime::builtin::singletons::STORAGE_POWER_ACTOR_ADDR;
use fil_actors_runtime::util::cbor::serialize;
use fil_actors_runtime::{
    CRON_ACTOR_ADDR, STORAGE_MARKET_ACTOR_ADDR, SYSTEM_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR,
};
use fvm_ipld_encoding::{BytesDe, RawBytes};
use fvm_shared::address::{Address, BLS_PUB_LEN};
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::randomness::Randomness;
use fvm_shared::sector::{
    PoStProof, RegisteredPoStProof, RegisteredSealProof, SectorNumber, StoragePower,
};
use fvm_shared::METHOD_SEND;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::wrangler::VM;
use fvm_workbench_vm::bench::kernel::TEST_VM_RAND_ARRAY;
use rand_chacha::rand_core::RngCore;

use super::*;

// Generate count addresses by seeding an rng
pub fn pk_addrs_from(seed: u64, count: u64) -> Vec<Address> {
    let mut seed_arr = [0u8; 32];
    for (i, b) in seed.to_ne_bytes().iter().enumerate() {
        seed_arr[i] = *b;
    }
    let mut rng = ChaCha8Rng::from_seed(seed_arr);
    (0..count).map(|_| new_bls_from_rng(&mut rng)).collect()
}

// Generate nice 32 byte arrays sampled uniformly at random based off of a u64 seed
fn new_bls_from_rng(rng: &mut ChaCha8Rng) -> Address {
    let mut bytes = [0u8; BLS_PUB_LEN];
    rng.fill_bytes(&mut bytes);
    Address::new_bls(&bytes).unwrap()
}

const ACCOUNT_SEED: u64 = 93837778;

pub fn create_accounts(v: &mut dyn VM, count: u64, balance: &TokenAmount) -> Vec<Address> {
    create_accounts_seeded(v, count, balance, ACCOUNT_SEED)
}

pub fn create_accounts_seeded(
    v: &mut dyn VM,
    count: u64,
    balance: &TokenAmount,
    seed: u64,
) -> Vec<Address> {
    let pk_addrs = pk_addrs_from(seed, count);
    // Send funds from faucet to pk address, creating account actor
    for pk_addr in pk_addrs.clone() {
        apply_ok_implicit(v, &TEST_FAUCET_ADDR, &pk_addr, balance, METHOD_SEND, None::<RawBytes>);
    }
    // Normalize pk address to return id address of account actor
    pk_addrs.iter().map(|pk_addr| v.resolve_id_address(pk_addr).unwrap()).collect()
}

pub fn market_add_balance(
    v: &mut dyn VM,
    sender: &Address,
    beneficiary: &Address,
    amount: &TokenAmount,
) {
    apply_ok(
        v,
        sender,
        &STORAGE_MARKET_ACTOR_ADDR,
        amount,
        MarketMethod::AddBalance as u64,
        Some(beneficiary),
    );
}
pub fn create_miner(
    v: &mut dyn VM,
    owner: &Address,
    worker: &Address,
    post_proof_type: RegisteredPoStProof,
    balance: &TokenAmount,
) -> (Address, Address) {
    let multiaddrs = vec![BytesDe("multiaddr".as_bytes().to_vec())];
    let peer_id = "miner".as_bytes().to_vec();
    let params = CreateMinerParams {
        owner: *owner,
        worker: *worker,
        window_post_proof_type: post_proof_type,
        peer: peer_id,
        multiaddrs,
    };

    let params = IpldBlock::serialize_cbor(&params).unwrap().unwrap();
    let res: CreateMinerReturn = v
        .execute_message(
            owner,
            &STORAGE_POWER_ACTOR_ADDR,
            balance,
            fil_actor_power::Method::CreateMiner as u64,
            Some(params),
        )
        .unwrap()
        .ret
        .unwrap()
        .deserialize()
        .unwrap();
    (res.id_address, res.robust_address)
}

pub fn verifreg_add_verifier(v: &mut dyn VM, verifier: &Address, data_cap: StoragePower) {
    let add_verifier_params = VerifierParams { address: *verifier, allowance: data_cap };
    // root address is msig, send proposal from root key
    let proposal = ProposeParams {
        to: VERIFIED_REGISTRY_ACTOR_ADDR,
        value: TokenAmount::zero(),
        method: VerifregMethod::AddVerifier as u64,
        params: serialize(&add_verifier_params, "verifreg add verifier params").unwrap(),
    };

    apply_ok(
        v,
        &TEST_VERIFREG_ROOT_SIGNER_ADDR,
        &TEST_VERIFREG_ROOT_ADDR,
        &TokenAmount::zero(),
        MultisigMethod::Propose as u64,
        Some(proposal),
    );
    // ExpectInvocation {
    //     from: TEST_VERIFREG_ROOT_SIGNER_ADDR,
    //     to: TEST_VERIFREG_ROOT_ADDR,
    //     method: MultisigMethod::Propose as u64,
    //     subinvocs: Some(vec![ExpectInvocation {
    //         from: TEST_VERIFREG_ROOT_ADDR,
    //         to: VERIFIED_REGISTRY_ACTOR_ADDR,
    //         method: VerifregMethod::AddVerifier as u64,
    //         params: Some(IpldBlock::serialize_cbor(&add_verifier_params).unwrap()),
    //         subinvocs: Some(vec![Expect::frc42_balance(
    //             VERIFIED_REGISTRY_ACTOR_ADDR,
    //             DATACAP_TOKEN_ACTOR_ADDR,
    //             *verifier,
    //         )]),
    //         ..Default::default()
    //     }]),
    //     ..Default::default()
    // }
    // .matches(v.take_invocations().last().unwrap());
}

pub fn verifreg_add_client(
    v: &mut dyn VM,
    verifier: &Address,
    client: &Address,
    allowance: StoragePower,
) {
    let add_client_params =
        AddVerifiedClientParams { address: *client, allowance: allowance.clone() };
    apply_ok(
        v,
        verifier,
        &VERIFIED_REGISTRY_ACTOR_ADDR,
        &TokenAmount::zero(),
        VerifregMethod::AddVerifiedClient as u64,
        Some(add_client_params),
    );
    let allowance_tokens = TokenAmount::from_whole(allowance);
    // ExpectInvocation {
    //     from: *verifier,
    //     to: VERIFIED_REGISTRY_ACTOR_ADDR,
    //     method: VerifregMethod::AddVerifiedClient as u64,
    //     subinvocs: Some(vec![ExpectInvocation {
    //         from: VERIFIED_REGISTRY_ACTOR_ADDR,
    //         to: DATACAP_TOKEN_ACTOR_ADDR,
    //         method: DataCapMethod::MintExported as u64,
    //         params: Some(
    //             IpldBlock::serialize_cbor(&MintParams {
    //                 to: *client,
    //                 amount: allowance_tokens.clone(),
    //                 operators: vec![STORAGE_MARKET_ACTOR_ADDR],
    //             })
    //             .unwrap(),
    //         ),
    //         subinvocs: Some(vec![Expect::frc46_receiver(
    //             DATACAP_TOKEN_ACTOR_ADDR,
    //             *client,
    //             DATACAP_TOKEN_ACTOR_ID,
    //             client.id().unwrap(),
    //             VERIFIED_REGISTRY_ACTOR_ID,
    //             allowance_tokens,
    //             None,
    //         )]),
    //         ..Default::default()
    //     }]),
    //     ..Default::default()
    // }
    // .matches(v.take_invocations().last().unwrap());
}

#[allow(clippy::too_many_arguments)]
pub fn precommit_sectors(
    w: &mut ExecutionWrangler,
    count: usize,
    batch_size: usize,
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
        vec![], // no deals
        worker,
        maddr,
        seal_proof,
        sector_number_base,
        expect_cron_enroll,
        exp,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn precommit_sectors_v2(
    v: &mut dyn VM,
    count: usize,
    batch_size: usize,
    metadata: Vec<PrecommitMetadata>, // Per-sector deal metadata, or empty vector for no deals.
    worker: &Address,
    maddr: &Address,
    seal_proof: RegisteredSealProof,
    sector_number_base: SectorNumber,
    expect_cron_enroll: bool,
    exp: Option<ChainEpoch>,
    v2: bool,
) -> Vec<SectorPreCommitOnChainInfo> {
    let mid = v.resolve_id_address(maddr).unwrap();
    let expiration = match exp {
        None => {
            v.epoch()
                + Policy::default().min_sector_expiration
                + max_prove_commit_duration(&Policy::default(), seal_proof).unwrap()
        }
        Some(e) => e,
    };

    let mut sector_idx: usize = 0;
    let no_deals = PrecommitMetadata { deals: vec![], commd: CompactCommD::default() };
    let mut sectors_with_deals: Vec<SectorDeals> = vec![];
    while sector_idx < count {
        let msg_sector_idx_base = sector_idx;
        // let mut invocs = vec![Expect::reward_this_epoch(mid), Expect::power_current_total(mid)];
        if !v2 {
            let mut param_sectors = Vec::<PreCommitSectorParams>::new();
            let mut j = 0;
            while j < batch_size && sector_idx < count {
                let sector_number = sector_number_base + sector_idx as u64;
                let sector_meta = metadata.get(sector_idx).unwrap_or(&no_deals);
                param_sectors.push(PreCommitSectorParams {
                    seal_proof,
                    sector_number,
                    sealed_cid: make_sealed_cid(format!("sn: {}", sector_number).as_bytes()),
                    seal_rand_epoch: v.epoch() - 1,
                    deal_ids: sector_meta.deals.clone().clone(),
                    expiration,
                    ..Default::default()
                });
                if !sector_meta.deals.is_empty() {
                    sectors_with_deals.push(SectorDeals {
                        sector_type: seal_proof,
                        sector_expiry: expiration,
                        deal_ids: sector_meta.deals.clone(),
                    });
                }
                sector_idx += 1;
                j += 1;
            }
            if !sectors_with_deals.is_empty() {
                // invocs.push(Expect::market_verify_deals(mid, sectors_with_deals.clone()));
            }
            if param_sectors.len() > 1 {
                // invocs.push(Expect::burn(
                //     mid,
                //     Some(aggregate_pre_commit_network_fee(
                //         param_sectors.len() as i64,
                //         &TokenAmount::zero(),
                //     )),
                // ));
            }
            if expect_cron_enroll && msg_sector_idx_base == 0 {
                // invocs.push(Expect::power_enrol_cron(mid));
            }

            apply_ok(
                v,
                worker,
                maddr,
                &TokenAmount::zero(),
                MinerMethod::PreCommitSectorBatch as u64,
                Some(PreCommitSectorBatchParams { sectors: param_sectors.clone() }),
            );
            // let expect = ExpectInvocation {
            //     from: *worker,
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
                let sector_number = sector_number_base + sector_idx as u64;
                let sector_meta = metadata.get(sector_idx).unwrap_or(&no_deals);
                param_sectors.push(SectorPreCommitInfo {
                    seal_proof,
                    sector_number,
                    sealed_cid: make_sealed_cid(format!("sn: {}", sector_number).as_bytes()),
                    seal_rand_epoch: v.epoch() - 1,
                    deal_ids: sector_meta.deals.clone(),
                    expiration,
                    unsealed_cid: sector_meta.commd.clone(),
                });
                if !sector_meta.deals.is_empty() {
                    sectors_with_deals.push(SectorDeals {
                        sector_type: seal_proof,
                        sector_expiry: expiration,
                        deal_ids: sector_meta.deals.clone(),
                    });
                }
                sector_idx += 1;
                j += 1;
            }
            if !sectors_with_deals.is_empty() {
                // invocs.push(Expect::market_verify_deals(mid, sectors_with_deals.clone()));
            }
            if param_sectors.len() > 1 {
                // invocs.push(Expect::burn(
                //     mid,
                //     Some(aggregate_pre_commit_network_fee(
                //         param_sectors.len() as i64,
                //         &TokenAmount::zero(),
                //     )),
                // ));
            }
            if expect_cron_enroll && msg_sector_idx_base == 0 {
                // invocs.push(Expect::power_enrol_cron(mid));
            }

            apply_ok(
                v,
                worker,
                maddr,
                &TokenAmount::zero(),
                MinerMethod::PreCommitSectorBatch2 as u64,
                Some(PreCommitSectorBatchParams2 { sectors: param_sectors.clone() }),
            );

            // let expect = ExpectInvocation {
            //     from: *worker,
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
    let mstate: MinerState = get_state(v, &mid).unwrap();
    (0..count)
        .map(|i| {
            mstate
                .get_precommitted_sector(
                    &DynBlockstore::new(v.blockstore()),
                    sector_number_base + i as u64,
                )
                .unwrap()
                .unwrap()
        })
        .collect()
}

pub fn submit_windowed_post(
    v: &mut dyn VM,
    worker: &Address,
    maddr: &Address,
    dline_info: DeadlineInfo,
    partition_idx: u64,
    new_power: Option<PowerPair>,
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
        v,
        worker,
        maddr,
        &TokenAmount::zero(),
        MinerMethod::SubmitWindowedPoSt as u64,
        Some(params),
    );
    // let mut subinvocs = None; // Unchecked unless provided
    // if let Some(new_pow) = new_power {
    //     if new_pow == PowerPair::zero() {
    //         subinvocs = Some(vec![])
    //     } else {
    //         subinvocs = Some(vec![Expect::power_update_claim(*maddr, new_pow)])
    //     }
    // }

    // ExpectInvocation {
    //     from: *worker,
    //     to: *maddr,
    //     method: MinerMethod::SubmitWindowedPoSt as u64,
    //     subinvocs,
    //     ..Default::default()
    // }
    // .matches(v.take_invocations().last().unwrap());
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

fn advance_by_deadline<F>(v: &mut dyn VM, maddr: &Address, more: F) -> DeadlineInfo
where
    F: Fn(DeadlineInfo) -> bool,
{
    loop {
        let dline_info = miner_dline_info(v, maddr);
        if !more(dline_info) {
            return dline_info;
        }
        v.set_epoch(dline_info.last());
        cron_tick(v);
        let next = v.epoch() + 1;
        v.set_epoch(next);
    }
}

pub fn advance_to_proving_deadline(
    v: &mut dyn VM,
    maddr: &Address,
    s: SectorNumber,
) -> (DeadlineInfo, u64) {
    let (d, p) = sector_deadline(v, maddr, s);
    let dline_info = advance_by_deadline_to_index(v, maddr, d);
    v.set_epoch(dline_info.open);
    (dline_info, p)
}

pub fn advance_by_deadline_to_index(v: &mut dyn VM, maddr: &Address, i: u64) -> DeadlineInfo {
    advance_by_deadline(v, maddr, |dline_info| dline_info.index != i)
}

pub fn cron_tick(v: &mut dyn VM) {
    apply_ok_implicit(
        v,
        &SYSTEM_ACTOR_ADDR,
        &CRON_ACTOR_ADDR,
        &TokenAmount::zero(),
        CronMethod::EpochTick as u64,
        None::<RawBytes>,
    );
}
