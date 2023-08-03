use fil_actors_integration_tests::tests::withdraw_balance_success_test;
use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_shared::{state::StateTreeVersion, version::NetworkVersion};
use fvm_workbench_api::{wrangler::ExecutionWrangler, WorkbenchBuilder};
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::{
    builder::FvmBenchBuilder, externs::FakeExterns, primitives::FakePrimitives,
};

#[test]
fn withdraw_balance_test() {
    // create the execution wrangler
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
    let _genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let bench = builder.build().unwrap();
    let w = ExecutionWrangler::new_default(bench, Box::new(store), Box::new(FakePrimitives {}));

    withdraw_balance_success_test(&w);
}
