// use fil_actors_runtime::{
//      BURNT_FUNDS_ACTOR_ADDR, BURNT_FUNDS_ACTOR_ID, CRON_ACTOR_ID,
//     DATACAP_TOKEN_ACTOR_ID, INIT_ACTOR_ADDR, INIT_ACTOR_ID, REWARD_ACTOR_ID,
//     STORAGE_MARKET_ACTOR_ADDR, STORAGE_MARKET_ACTOR_ID, STORAGE_POWER_ACTOR_ADDR,
//     STORAGE_POWER_ACTOR_ID, SYSTEM_ACTOR_ID, VERIFIED_REGISTRY_ACTOR_ID,
// };
use fil_actors_runtime::INIT_ACTOR_ADDR;
use fvm::trace::ExecutionTrace;
use fvm_ipld_blockstore::MemoryBlockstore;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::message::Message;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::{BLOCK_GAS_LIMIT, METHOD_SEND};
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::Bench;
use fvm_workbench_vm::{BenchBuilder, ExecutionWrangler, FakeExterns};

#[test]
fn test_hookup() {
    let blockstore = MemoryBlockstore::new();
    let externs = FakeExterns::new();
    let (mut builder, manifest_data_cid) = BenchBuilder::new_with_bundle(
        blockstore,
        externs,
        NetworkVersion::V16,
        StateTreeVersion::V4,
        actors_v10::BUNDLE_CAR,
    )
    .unwrap();

    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let mut bench = builder.build().unwrap();

    let mut wrangler = ExecutionWrangler::new_default(&mut bench);
    let ret = wrangler.execute(
        genesis.faucet_address(),
        INIT_ACTOR_ADDR.clone(),
        METHOD_SEND,
        RawBytes::default(),
        TokenAmount::zero(),
    ).unwrap();

    assert_eq!(ExitCode::OK, ret.receipt.exit_code);
    println!("trace: {:?}", format_trace(&ret.trace));
}

fn format_trace(trace: &ExecutionTrace) {
    for event in trace {
        println!("event: {:?}", event);
    }
}
