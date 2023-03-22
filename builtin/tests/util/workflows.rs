use fil_actor_multisig::{Method as MultisigMethod, ProposeParams};
use fil_actor_power::{CreateMinerParams, CreateMinerReturn};
use fil_actor_verifreg::{Method as VerifregMethod, VerifierParams};
use fil_actors_runtime::builtin::singletons::STORAGE_POWER_ACTOR_ADDR;
use fil_actors_runtime::util::cbor::serialize;
use fil_actors_runtime::VERIFIED_REGISTRY_ACTOR_ADDR;
use fvm_ipld_encoding::{BytesDe, RawBytes};
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;
use fvm_shared::sector::{RegisteredPoStProof, StoragePower};
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

// pub fn verifreg_add_client(
//     w: &mut ExecutionWrangler,
//     verifier: Address,
//     client: Address,
//     allowance: StoragePower,
// ) {
//     let add_client_params =
//         AddVerifiedClientParams { address: client, allowance: allowance.clone() };
//     apply_ok(
//         w,
//         verifier,
//         VERIFIED_REGISTRY_ACTOR_ADDR,
//         TokenAmount::zero(),
//         VerifregMethod::AddVerifiedClient as u64,
//         &add_client_params,
//     )
//     .unwrap();
// }
