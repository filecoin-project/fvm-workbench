// Copied, adapted and compressed from datacap_tests in builtin-actors.
// The aim is to find APIs so that this test can be invoked directly in built-in actors from here,
// using a workbench.

use fil_actor_datacap::MintParams;
use fil_actor_verifreg::{AllocationRequest, AllocationRequests};
use fil_actors_runtime::cbor::serialize;
use fil_actors_runtime::runtime::policy_constants::MINIMUM_VERIFIED_ALLOCATION_SIZE;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{DATACAP_TOKEN_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR};
use frc46_token::token::types::TransferFromParams;
use fvm_ipld_blockstore::MemoryBlockstore;
use fvm_shared::bigint::Zero;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;
use fvm_shared::piece::PaddedPieceSize;
use fvm_shared::sector::RegisteredSealProof;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;

use crate::util::*;
use crate::workflows::*;
mod util;

/* Mint a token for client and transfer it to a receiver. */
#[test]
fn datacap_transfer_scenario() {
    let policy = Policy::default();

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

    let accts = create_accounts(
        &mut w,
        genesis.faucet_id,
        3,
        TokenAmount::from_whole(10_000),
        SignatureType::BLS,
    )
    .unwrap();
    let (client, operator, owner) = (accts[0].clone(), accts[1].clone(), accts[2].clone());

    // // need to allocate to an actual miner actor to pass verifreg receiver hook checks
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
    let (maddr, _) = create_miner(
        &mut w,
        owner.id,
        owner.id,
        seal_proof.registered_window_post_proof().unwrap(),
        TokenAmount::from_whole(1_000),
    )
    .unwrap();

    let data_cap_amt = TokenAmount::from_whole(
        MINIMUM_VERIFIED_ALLOCATION_SIZE + MINIMUM_VERIFIED_ALLOCATION_SIZE / 2,
    );
    let mint_params = MintParams {
        to: client.id_addr(),
        amount: data_cap_amt,
        operators: vec![operator.id_addr()],
    };

    // mint datacap for client
    apply_ok(
        &mut w,
        VERIFIED_REGISTRY_ACTOR_ADDR,
        DATACAP_TOKEN_ACTOR_ADDR,
        TokenAmount::zero(),
        fil_actor_datacap::Method::MintExported as u64,
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
        from: client.id_addr(),
        amount: TokenAmount::from_whole(MINIMUM_VERIFIED_ALLOCATION_SIZE),
        operator_data: serialize(
            &AllocationRequests { allocations: vec![alloc], extensions: vec![] },
            "operator data",
        )
        .unwrap(),
    };

    // Create allocation by transferring datacap
    let result = apply_ok(
        &mut w,
        operator.id_addr(),
        DATACAP_TOKEN_ACTOR_ADDR,
        TokenAmount::zero(),
        fil_actor_datacap::Method::TransferFromExported as u64,
        &transfer_from_params,
    )
    .unwrap();

    println!("{}", result.trace.format());
    let analysis = TraceAnalysis::build(result.trace);
    println!("{}", analysis.format_spans());
}
