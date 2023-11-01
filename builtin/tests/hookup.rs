use fil_actors_integration_tests::util::assert_invariants;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::test_utils::FakePrimitives;
use fil_actors_runtime::INIT_ACTOR_ADDR;
use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::METHOD_SEND;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::bench::WorkbenchBuilder;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_builtin_actors::genesis::{
    create_genesis_actors, GenesisSpec, BUILTIN_ACTORS_BUNDLE,
};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;
use vm_api::VM;

#[test]
fn test_hookup() {
    let store = SharedMemoryBlockstore::new();
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        store.clone(),
        FakeExterns::new(),
        NetworkVersion::V21,
        StateTreeVersion::V5,
        BUILTIN_ACTORS_BUNDLE,
    )
    .unwrap();

    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let faucet_addr = genesis.faucet_address();
    let bench = builder.build(genesis.circulating_supply).unwrap();
    let wrangler =
        ExecutionWrangler::new_default(bench, Box::new(store), Box::<FakePrimitives>::default());

    let result = wrangler
        .execute_message(&faucet_addr, &INIT_ACTOR_ADDR, &TokenAmount::zero(), METHOD_SEND, None)
        .unwrap();

    assert_eq!(ExitCode::OK, result.code);

    let trace = wrangler.peek_execution_trace().unwrap();
    println!("{}", trace.format());
    let analysis = TraceAnalysis::build(trace.clone());
    println!("{}", analysis.format_spans());

    // check that the genesis state obeys state-invariants
    assert_invariants(&wrangler, &Policy::default(), Some(genesis.total_supply));
}
