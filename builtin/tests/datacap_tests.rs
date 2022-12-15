// Copied, adapted and compressed from datacap_tests in builtin-actors.
// The aim is to find APIs so that this test can be invoked directly in built-in actors from here,
// using a workbench.

use anyhow::anyhow;
use cid::Cid;
use cid::multihash::{Code, Multihash as OtherMultihash};
use fil_actor_datacap::MintParams;
use fil_actor_power::{CreateMinerParams, CreateMinerReturn};
use fil_actor_verifreg::{AllocationRequest, AllocationRequests};
use fil_actors_runtime::{
    DATACAP_TOKEN_ACTOR_ADDR, STORAGE_POWER_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR,
};
use fil_actors_runtime::cbor::serialize;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::runtime::policy_constants::MINIMUM_VERIFIED_ALLOCATION_SIZE;
use frc46_token::token::types::TransferFromParams;
use fvm_ipld_blockstore::{Blockstore, MemoryBlockstore};
use fvm_ipld_encoding::{BytesDe, RawBytes, ser};
use fvm_shared::{ActorID, METHOD_SEND, MethodNum};
use fvm_shared::address::{Address, BLS_PUB_LEN};
use fvm_shared::bigint::Zero;
use fvm_shared::commcid::{FIL_COMMITMENT_SEALED, FIL_COMMITMENT_UNSEALED};
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::piece::PaddedPieceSize;
use fvm_shared::sector::{RegisteredPoStProof, RegisteredSealProof};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};

use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::bench::ExecutionWrangler;
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;
use multihash::derive::Multihash;
use multihash::MultihashDigest;
use fvm_workbench_api::ExecutionResult;
use fvm_workbench_api::trace::format_trace;

/* Mint a token for client and transfer it to a receiver, exercising error cases */
#[test]
fn datacap_transfer_scenario() {
    let policy = Policy::default();

    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        MemoryBlockstore::new(),
        FakeExterns::new(),
        NetworkVersion::V16,
        StateTreeVersion::V4,
        actors_v10::BUNDLE_CAR,
    )
    .unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();

    let mut bench = builder.build().unwrap();
    let mut w = ExecutionWrangler::new_default(&mut bench);

    let accts =
        create_accounts(&mut w, genesis.faucet_id, 3, TokenAmount::from_whole(10_000)).unwrap();
    let (client, operator, owner) = (accts[0], accts[1], accts[2]);
    let operator_address = Address::new_id(operator);
    let client_address = Address::new_id(client);

    // // need to allocate to an actual miner actor to pass verifreg receiver hook checks
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
    let (maddr, _) = create_miner(
        &mut w,
        owner,
        owner,
        seal_proof.registered_window_post_proof().unwrap(),
        TokenAmount::from_whole(1_000),
    )
    .unwrap();

    let data_cap_amt = TokenAmount::from_whole(
        MINIMUM_VERIFIED_ALLOCATION_SIZE + MINIMUM_VERIFIED_ALLOCATION_SIZE / 2,
    );
    let mint_params = MintParams {
        to: Address::new_id(client),
        amount: data_cap_amt,
        operators: vec![operator_address],
    };

    // mint datacap for client
    apply_ok(
        &mut w,
        VERIFIED_REGISTRY_ACTOR_ADDR,
        DATACAP_TOKEN_ACTOR_ADDR,
        TokenAmount::zero(),
        fil_actor_datacap::Method::Mint as u64,
        &mint_params,
    )
    .unwrap();

    let alloc = AllocationRequest {
        provider: maddr,
        data: make_piece_cid("datacap-test-alloc".as_bytes()),
        size: PaddedPieceSize(MINIMUM_VERIFIED_ALLOCATION_SIZE as u64),
        term_min: policy.minimum_verified_allocation_term,
        term_max: policy.maximum_verified_allocation_term,
        expiration: w.epoch() + policy.maximum_verified_allocation_expiration,
    };
    let transfer_from_params = TransferFromParams {
        to: VERIFIED_REGISTRY_ACTOR_ADDR,
        from: client_address,
        amount: TokenAmount::from_whole(MINIMUM_VERIFIED_ALLOCATION_SIZE),
        operator_data: serialize(
            &AllocationRequests { allocations: vec![alloc.clone()], extensions: vec![] },
            "operator data",
        )
        .unwrap(),
    };

    // Create allocation by transferring datacap
    let result = apply_ok(
        &mut w,
        operator_address,
        DATACAP_TOKEN_ACTOR_ADDR,
        TokenAmount::zero(),
        fil_actor_datacap::Method::TransferFrom as u64,
        &transfer_from_params,
    )
    .unwrap();
    assert_eq!(ExitCode::OK, result.receipt.exit_code);
    println!("trace: {}", format_trace(&result.trace));
}

pub fn apply_ok<T: ser::Serialize + ?Sized>(
    w: &mut ExecutionWrangler,
    from: Address,
    to: Address,
    value: TokenAmount,
    method: MethodNum,
    params: &T,
) -> anyhow::Result<ExecutionResult> {
    apply_code(w, from, to, value, method, params, ExitCode::OK)
}

pub fn apply_code<T: ser::Serialize + ?Sized>(
    w: &mut ExecutionWrangler,
    from: Address,
    to: Address,
    value: TokenAmount,
    method: MethodNum,
    params: &T,
    code: ExitCode,
) -> anyhow::Result<ExecutionResult> {
    // Implicit execution is used because tests often trigger messages from non-account actors.
    let ret = w.execute_implicit(from, to, method, serialize(params, "params").unwrap(), value)?;
    assert_eq!(
        code, ret.receipt.exit_code,
        "expected code {}, got {} ({})",
        code, ret.receipt.exit_code, ret.message
    );
    Ok(ret)
}

fn create_accounts(
    w: &mut ExecutionWrangler,
    faucet: ActorID,
    count: u64,
    balance: TokenAmount,
) -> anyhow::Result<Vec<ActorID>> {
    create_accounts_seeded(w, faucet, count, balance, ACCOUNT_SEED)
}

pub fn create_accounts_seeded(
    w: &mut ExecutionWrangler,
    faucet: ActorID,
    count: u64,
    balance: TokenAmount,
    seed: u64,
) -> anyhow::Result<Vec<ActorID>> {
    let pk_addrs = pk_addrs_from(seed, count);
    // Send funds from faucet to pk address, creating account actor
    for pk_addr in pk_addrs.clone() {
        apply_ok(
            w,
            Address::new_id(faucet),
            pk_addr,
            balance.clone(),
            METHOD_SEND,
            &RawBytes::default(),
        )?;
    }
    // Resolve pk address to return id address of account actor
    Ok(pk_addrs.iter().map(|&pk_addr| w.resolve_address(&pk_addr).unwrap().unwrap()).collect())
}

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

pub fn create_miner(
    w: &mut ExecutionWrangler,
    owner: ActorID,
    worker: ActorID,
    post_proof_type: RegisteredPoStProof,
    balance: TokenAmount,
) -> anyhow::Result<(Address, Address)> {
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
    Ok((res.id_address, res.robust_address))
}

fn make_cid(input: &[u8], prefix: u64, hash: MhCode) -> Cid {
    let hash = hash.digest(input);
    Cid::new_v1(prefix, hash)
}

pub fn make_cid_sha(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::Sha256TruncPaddedFake)
}

pub fn make_cid_poseidon(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::PoseidonFake)
}

pub fn make_piece_cid(input: &[u8]) -> Cid {
    make_cid_sha(input, FIL_COMMITMENT_UNSEALED)
}

pub fn make_sealed_cid(input: &[u8]) -> Cid {
    make_cid_poseidon(input, FIL_COMMITMENT_SEALED)
}

// multihash library doesn't support poseidon hashing, so we fake it
#[derive(Clone, Copy, Debug, PartialEq, Eq, Multihash)]
#[mh(alloc_size = 64)]
enum MhCode {
    #[mh(code = 0xb401, hasher = multihash::Sha2_256)]
    PoseidonFake,
    #[mh(code = 0x1012, hasher = multihash::Sha2_256)]
    Sha256TruncPaddedFake,
}
