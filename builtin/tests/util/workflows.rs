use fil_actor_cron::Method as CronMethod;
use fil_actor_miner::{DeadlineInfo, SectorPreCommitOnChainInfo};
use fil_actor_multisig::{Method as MultisigMethod, ProposeParams};
use fil_actor_power::{CreateMinerParams, CreateMinerReturn};
use fil_actor_verifreg::{AddVerifiedClientParams, Method as VerifregMethod, VerifierParams};
use fil_actors_runtime::builtin::singletons::STORAGE_POWER_ACTOR_ADDR;
use fil_actors_runtime::util::cbor::serialize;
use fil_actors_runtime::{CRON_ACTOR_ADDR, SYSTEM_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR};
use fvm_ipld_encoding::{BytesDe, RawBytes};
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;
use fvm_shared::sector::{RegisteredPoStProof, RegisteredSealProof, SectorNumber, StoragePower};
use fvm_shared::{ActorID, METHOD_SEND};
use fvm_workbench_api::wrangler::ExecutionWrangler;

use super::*;

const ACCOUNT_SEED: u64 = 93837778;

pub fn create_accounts(
    w: &mut ExecutionWrangler,
    faucet: ActorID,
    count: u64,
    balance: TokenAmount,
    typ: SignatureType,
) -> anyhow::Result<Vec<Account>> {
    create_accounts_seeded(w, faucet, count, balance, typ, ACCOUNT_SEED)
}

pub fn create_accounts_seeded(
    w: &mut ExecutionWrangler,
    faucet: ActorID,
    count: u64,
    balance: TokenAmount,
    typ: SignatureType,
    seed: u64,
) -> anyhow::Result<Vec<Account>> {
    let keys = match typ {
        SignatureType::Secp256k1 => make_secp_keys(seed, count),
        SignatureType::BLS => make_bls_keys(seed, count),
    };

    // Send funds from faucet to pk address, creating account actor
    for key in keys.iter() {
        apply_ok(
            w,
            Address::new_id(faucet),
            key.addr,
            balance.clone(),
            METHOD_SEND,
            &RawBytes::default(),
        )?;
    }
    // Resolve pk address to return ID of account actor
    let ids: Vec<ActorID> =
        keys.iter().map(|key| w.resolve_address(&key.addr).unwrap().unwrap()).collect();
    let accounts =
        keys.into_iter().enumerate().map(|(i, key)| Account { id: ids[i], key }).collect();
    Ok(accounts)
}

pub fn create_miner(
    w: &mut ExecutionWrangler,
    owner: ActorID,
    worker: ActorID,
    post_proof_type: RegisteredPoStProof,
    balance: TokenAmount,
) -> anyhow::Result<(ActorID, Address)> {
    let multiaddrs = vec![BytesDe("multiaddr".as_bytes().to_vec())];
    let peer_id = "miner".as_bytes().to_vec();
    let owner = Address::new_id(owner);
    let params = CreateMinerParams {
        owner,
        worker: Address::new_id(worker),
        window_post_proof_type: post_proof_type,
        peer: peer_id,
        multiaddrs,
    };

    let res: CreateMinerReturn = apply_ok(
        w,
        owner,
        STORAGE_POWER_ACTOR_ADDR,
        balance,
        fil_actor_power::Method::CreateMiner as u64,
        &params,
    )?
    .receipt
    .return_data
    .deserialize()?;
    Ok((res.id_address.id().unwrap(), res.robust_address))
}

#[allow(dead_code)]
pub fn verifreg_add_verifier(
    w: &mut ExecutionWrangler,
    verifier: ActorID,
    data_cap: StoragePower,
    root: Address,
    root_signer: Address,
) {
    let add_verifier_params =
        VerifierParams { address: Address::new_id(verifier), allowance: data_cap };
    // root address is msig, send proposal from root key
    let proposal = ProposeParams {
        to: VERIFIED_REGISTRY_ACTOR_ADDR,
        value: TokenAmount::zero(),
        method: VerifregMethod::AddVerifier as u64,
        params: serialize(&add_verifier_params, "verifreg add verifier params").unwrap(),
    };

    apply_ok(w, root_signer, root, TokenAmount::zero(), MultisigMethod::Propose as u64, &proposal)
        .unwrap();
}

#[allow(dead_code)]
pub fn verifreg_add_client(
    w: &mut ExecutionWrangler,
    verifier: Address,
    client: Address,
    allowance: StoragePower,
) {
    let add_client_params = AddVerifiedClientParams { address: client, allowance };
    apply_ok(
        w,
        verifier,
        VERIFIED_REGISTRY_ACTOR_ADDR,
        TokenAmount::zero(),
        VerifregMethod::AddVerifiedClient as u64,
        &add_client_params,
    )
    .unwrap();
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

#[allow(clippy::too_many_arguments)]
pub fn precommit_sectors_v2(
    w: &mut ExecutionWrangler,
    count: u64,
    batch_size: i64,
    worker: &Address,
    maddr: &Address,
    seal_proof: RegisteredSealProof,
    sector_number_base: SectorNumber,
    _expect_cron_enroll: bool,
    exp: Option<ChainEpoch>,
    v2: bool,
) -> Vec<SectorPreCommitOnChainInfo> {
    let mid = w.resolve_address(maddr).unwrap().unwrap();

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
        }
    }
    // extract chain state
    let mstate: MinerState = w.find_actor_state(mid).unwrap().unwrap();
    (0..count)
        .map(|i| {
            mstate
                .get_precommitted_sector(&BlockstoreWrapper::new(w.store()), sector_number_base + i)
                .unwrap()
                .unwrap()
        })
        .collect()
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
