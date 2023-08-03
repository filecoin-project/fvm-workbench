use fil_actors_runtime::INIT_ACTOR_ADDR;
use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::METHOD_SEND;
// use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
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
        fil_builtin_actors_bundle::BUNDLE_CAR,
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

    // TODO: re-enable traces after https://github.com/anorth/fvm-workbench/issues/19 is completed
    // println!("{}", result.trace.format());
    // let analysis = TraceAnalysis::build(result.trace);
    // println!("{}", analysis.format_spans());
}
