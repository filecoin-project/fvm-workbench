use fil_actors_runtime::INIT_ACTOR_ADDR;
use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::METHOD_SEND;
use fvm_workbench_api::analysis::TraceAnalysis;
// use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::bench::WorkbenchBuilder;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_builtin_actors::genesis::{
    create_genesis_actors, GenesisSpec, BUILTIN_ACTORS_BUNDLE,
};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;
use fvm_workbench_vm::primitives::FakePrimitives;
use vm_api::VM;

#[test]
fn test_hookup() {
    let store = SharedMemoryBlockstore::new();
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        store.clone(),
        FakeExterns::new(),
        NetworkVersion::V18,
        StateTreeVersion::V5,
        BUILTIN_ACTORS_BUNDLE,
    )
    .unwrap();

    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let bench = builder.build().unwrap();
    let wrangler =
        ExecutionWrangler::new_default(bench, Box::new(store), Box::new(FakePrimitives {}));

    let result = wrangler
        .execute_message(
            &genesis.faucet_address(),
            &INIT_ACTOR_ADDR,
            &TokenAmount::zero(),
            METHOD_SEND,
            None,
        )
        .unwrap();

    assert_eq!(ExitCode::OK, result.code);

    let traces = wrangler.peek_execution_trace();
    let trace = traces.get(0).unwrap();
    println!("{}", trace.format());
    let analysis = TraceAnalysis::build(trace.clone());
    println!("{}", analysis.format_spans());
}
