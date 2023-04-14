use fil_actors_runtime::INIT_ACTOR_ADDR;
use fvm_ipld_blockstore::MemoryBlockstore;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::METHOD_SEND;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;

#[test]
fn test_hookup() {
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
    let mut bench = builder.build().unwrap();

    let mut wrangler = ExecutionWrangler::new_default(&mut *bench);
    let result = wrangler
        .execute(
            genesis.faucet_address(),
            INIT_ACTOR_ADDR,
            METHOD_SEND,
            RawBytes::default(),
            TokenAmount::zero(),
        )
        .unwrap();

    assert_eq!(ExitCode::OK, result.receipt.exit_code);
    println!("{}", result.trace.format());
    let analysis = TraceAnalysis::build(result.trace);
    println!("{}", analysis.format_spans());
}
