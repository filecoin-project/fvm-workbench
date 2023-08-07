use fvm_actor_utils::shared_blockstore::SharedMemoryBlockstore;
use fvm_shared::{state::StateTreeVersion, version::NetworkVersion};
use fvm_workbench_api::{bench::WorkbenchBuilder, wrangler::ExecutionWrangler};
use fvm_workbench_vm::{
    builder::FvmBenchBuilder, externs::FakeExterns, primitives::FakePrimitives,
};
use genesis::{create_genesis_actors, GenesisSpec};

pub mod genesis;

/// Create an ExecutionWrangler with sensible genesis state and defaults for running imported
/// tests from builtin-actors
pub fn setup() -> ExecutionWrangler {
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
    ExecutionWrangler::new_default(bench, Box::new(store), Box::new(FakePrimitives {}))
}
