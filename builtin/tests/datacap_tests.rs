// Copied and adapted from datacap_tests in builtin-actors.
// The aim is to find APIs so that this test can be invoked directly in built-in actors from here,
// using a workbench.


use anyhow::anyhow;
use fvm_ipld_blockstore::{Blockstore, MemoryBlockstore};
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::piece::PaddedPieceSize;
use fvm_shared::sector::RegisteredSealProof;

use fil_actor_verifreg::{AllocationRequest, AllocationRequests};
use fil_actors_runtime::cbor::serialize;
use fil_actors_runtime::runtime::policy_constants::MINIMUM_VERIFIED_ALLOCATION_SIZE;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{DATACAP_TOKEN_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR};
use fvm_shared::error::ExitCode;

use fil_actor_datacap::{Method as DataCapMethod, MintParams};
use frc46_token::token::types::{GetAllowanceParams, TransferFromParams};
use fvm_ipld_encoding::{Cbor, de, RawBytes, ser};
use fvm_shared::address::{Address, BLS_PUB_LEN};
use fvm_shared::{ActorID, METHOD_SEND, MethodNum};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::{Bench, BenchBuilder, ExecutionWrangler, FakeExterns};

/* Mint a token for client and transfer it to a receiver, exercising error cases */
#[test]
fn datacap_transfer_scenario() {
    // let policy = Policy::default();

    let (mut builder, manifest_data_cid) = BenchBuilder::new_with_bundle(
        MemoryBlockstore::new(),
        FakeExterns::new(),
        NetworkVersion::V16,
        StateTreeVersion::V4,
        actors_v10::BUNDLE_CAR,
    ).unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();



    let mut bench = builder.build().unwrap();
    let mut w = ExecutionWrangler::new_default(&mut bench);

    let addrs = create_accounts(&mut w, genesis.faucet_id, 3, TokenAmount::from_whole(10_000)).unwrap();
    // let (client, operator, owner) = (addrs[0], addrs[1], addrs[2]);

    // // need to allocate to an actual miner actor to pass verifreg receiver hook checks
    // let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
    // let (maddr, _) = create_miner(
    //     &mut v,
    //     owner,
    //     owner,
    //     seal_proof.registered_window_post_proof().unwrap(),
    //     TokenAmount::from_whole(1_000),
    // );
    //
    // let data_cap_amt = TokenAmount::from_whole(
    //     MINIMUM_VERIFIED_ALLOCATION_SIZE + MINIMUM_VERIFIED_ALLOCATION_SIZE / 2,
    // );
    // let mint_params = MintParams { to: client, amount: data_cap_amt, operators: vec![operator] };
    //
    // // cannot mint from non-verifreg
    // apply_code(
    //     &v,
    //     operator,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::Mint as u64,
    //     mint_params.clone(),
    //     ExitCode::USR_FORBIDDEN,
    // );
    //
    // // mint datacap for client
    // apply_ok(
    //     &v,
    //     VERIFIED_REGISTRY_ACTOR_ADDR,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::Mint as u64,
    //     mint_params,
    // );
    //
    // // confirm allowance was set to infinity
    // apply_ok(
    //     &v,
    //     // anyone can call Allowance
    //     owner,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::Allowance as u64,
    //     GetAllowanceParams { owner: client, operator },
    // );
    //
    // let alloc = AllocationRequest {
    //     provider: maddr,
    //     data: make_piece_cid("datacap-test-alloc".as_bytes()),
    //     size: PaddedPieceSize(MINIMUM_VERIFIED_ALLOCATION_SIZE as u64),
    //     term_min: policy.minimum_verified_allocation_term,
    //     term_max: policy.maximum_verified_allocation_term,
    //     expiration: v.get_epoch() + policy.maximum_verified_allocation_expiration,
    // };
    // let transfer_from_params = TransferFromParams {
    //     to: VERIFIED_REGISTRY_ACTOR_ADDR,
    //     from: client,
    //     amount: TokenAmount::from_whole(MINIMUM_VERIFIED_ALLOCATION_SIZE),
    //     operator_data: serialize(
    //         &AllocationRequests { allocations: vec![alloc.clone()], extensions: vec![] },
    //         "operator data",
    //     )
    //         .unwrap(),
    // };
    // let clone_params = |x: &TransferFromParams| -> TransferFromParams {
    //     TransferFromParams {
    //         to: x.to,
    //         from: x.from,
    //         amount: x.amount.clone(),
    //         operator_data: x.operator_data.clone(),
    //     }
    // };
    //
    // // bad operator data caught in verifreg receiver hook and propagated
    // // 1. piece size too small
    // let mut bad_alloc = alloc.clone();
    // bad_alloc.size = PaddedPieceSize(MINIMUM_VERIFIED_ALLOCATION_SIZE as u64 - 1);
    // let mut params_piece_too_small = clone_params(&transfer_from_params);
    // params_piece_too_small.operator_data = serialize(
    //     &AllocationRequests { allocations: vec![bad_alloc], extensions: vec![] },
    //     "operator data",
    // )
    //     .unwrap();
    // apply_code(
    //     &v,
    //     operator,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     params_piece_too_small,
    //     ExitCode::USR_ILLEGAL_ARGUMENT,
    // );
    //
    // // 2. mismatch more datacap than piece needs
    // let mut params_mismatched_datacap = clone_params(&transfer_from_params);
    // params_mismatched_datacap.amount =
    //     TokenAmount::from_whole(MINIMUM_VERIFIED_ALLOCATION_SIZE + 1);
    // apply_code(
    //     &v,
    //     operator,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     params_mismatched_datacap,
    //     ExitCode::USR_ILLEGAL_ARGUMENT,
    // );
    //
    // // 3. invalid term
    // let mut bad_alloc = alloc;
    // bad_alloc.term_max = policy.maximum_verified_allocation_term + 1;
    // let mut params_bad_term = clone_params(&transfer_from_params);
    // params_bad_term.operator_data = serialize(
    //     &AllocationRequests { allocations: vec![bad_alloc], extensions: vec![] },
    //     "operator data",
    // )
    //     .unwrap();
    // apply_code(
    //     &v,
    //     operator,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     params_bad_term,
    //     ExitCode::USR_ILLEGAL_ARGUMENT,
    // );
    //
    // // cannot transfer from operator to non-verifreg
    // let mut params_bad_receiver = clone_params(&transfer_from_params);
    // params_bad_receiver.to = owner;
    // apply_code(
    //     &v,
    //     owner,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     clone_params(&params_bad_receiver),
    //     ExitCode::USR_FORBIDDEN, // ExitCode(19) because non-operator has insufficient allowance
    // );
    //
    // // cannot transfer with non-operator caller
    // apply_code(
    //     &v,
    //     owner,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     clone_params(&transfer_from_params),
    //     ExitCode::USR_INSUFFICIENT_FUNDS, // ExitCode(19) because non-operator has insufficient allowance
    // );
    //
    // apply_ok(
    //     &v,
    //     operator,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     clone_params(&transfer_from_params),
    // );
    //
    // // Datacap already spent, not enough left
    // apply_code(
    //     &v,
    //     operator,
    //     DATACAP_TOKEN_ACTOR_ADDR,
    //     TokenAmount::zero(),
    //     DataCapMethod::TransferFrom as u64,
    //     transfer_from_params,
    //     ExitCode::USR_INSUFFICIENT_FUNDS,
    // );
}

fn create_accounts(w: &mut ExecutionWrangler, faucet: ActorID, count: u64, balance: TokenAmount) -> anyhow::Result<Vec<ActorID>>
{
    create_accounts_seeded(w, faucet, count, balance, ACCOUNT_SEED)
}

pub fn create_accounts_seeded(w: &mut ExecutionWrangler, faucet: ActorID, count: u64, balance: TokenAmount, seed: u64) -> anyhow::Result<Vec<ActorID>> {
    let pk_addrs = pk_addrs_from(seed, count);
    // Send funds from faucet to pk address, creating account actor
    for pk_addr in pk_addrs.clone() {
        apply_ok(w, Address::new_id(faucet), pk_addr, balance.clone(), METHOD_SEND, &RawBytes::default())?;
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

pub fn apply_ok< T: ser::Serialize + ?Sized>(
    w: &mut ExecutionWrangler,
    from: Address,
    to: Address,
    value: TokenAmount,
    method: MethodNum,
    params: &T,
) -> anyhow::Result<RawBytes> {
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
) -> anyhow::Result<RawBytes> {
    let ret = w.execute(from, to, method, serialize(params, "params").unwrap(),  value)?;
    assert_eq!(code, ret.receipt.exit_code, "expected code {}, got {} ({})", code, ret.receipt.exit_code, ret.message);
    Ok(ret.receipt.return_data)
}