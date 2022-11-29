use fil_actors_runtime::runtime::builtins::Type;
use fil_actors_runtime::{
    ActorError, BURNT_FUNDS_ACTOR_ADDR, CRON_ACTOR_ADDR, FIRST_NON_SINGLETON_ADDR, INIT_ACTOR_ADDR,
    REWARD_ACTOR_ADDR, STORAGE_MARKET_ACTOR_ADDR, STORAGE_POWER_ACTOR_ADDR, SYSTEM_ACTOR_ADDR,
    VERIFIED_REGISTRY_ACTOR_ADDR,
};

use fvm::externs::Externs;
use fvm::trace::ExecutionTrace;
use fvm_ipld_blockstore::{Blockstore, MemoryBlockstore};
use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::message::Message;
use fvm_shared::sector::StoragePower;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::BLOCK_GAS_LIMIT;
use fvm_workbench_builtin_actors::genesis::{GenesisResult, GenesisSpec};
use fvm_workbench_vm::FakeExterns;
use fvm_workbench_vm::BenchBuilder;

#[test]
fn test_hookup() {
    let blockstore = MemoryBlockstore::new();
    let externs = FakeExterns::new();
    let mut builder = BenchBuilder::new_with_bundle(
        blockstore,
        externs,
        NetworkVersion::V16,
        StateTreeVersion::V4,
        actors_v10::BUNDLE_CAR,
    )
    .unwrap();

    let spec = GenesisSpec {
        reward_balance: TokenAmount::from_whole(1_100_000_000),
        faucet_balance: TokenAmount::from_whole(900_000_000),
        verifreg_signer: Address::new_bls(&[200; fvm_shared::address::BLS_PUB_LEN]).unwrap(),
    };

    builder.create_system_actors().unwrap();
    let genesis = create_singletons(&mut builder, &spec).unwrap();

    let mut bench = builder.build().unwrap();

    let gas_fee_cap = TokenAmount::from_atto(1000) * BLOCK_GAS_LIMIT;

    let msg = Message {
        version: 0,
        from: genesis.faucet_address(),
        to: INIT_ACTOR_ADDR.clone(),
        sequence: 0,
        value: TokenAmount::zero(),
        method_num: 0,
        params: RawBytes::default(),
        gas_limit: BLOCK_GAS_LIMIT,
        gas_fee_cap,
        gas_premium: TokenAmount::zero(),
    };
    let ret = bench.execute_implicit(msg).unwrap();

    assert_eq!(ExitCode::OK, ret.msg_receipt.exit_code);
    println!("trace: {:?}", format_trace(&ret.exec_trace));
}

fn create_singletons<B, E>(builder: &mut BenchBuilder<B, E>, spec: &GenesisSpec) -> anyhow::Result<GenesisResult>
where
    B: Blockstore + Clone,
    E: Externs + Clone,

{
    // Reward actor
    let reward_state = fil_actor_reward::State::new(StoragePower::zero());
    builder.create_singleton_actor(
        Type::Reward as u32,
        &fil_actors_runtime::REWARD_ACTOR_ADDR,
        &reward_state,
        spec.reward_balance.clone(),
    )?;

    // Cron actor
    let cron_state = fil_actor_cron::State { entries: vec![
        fil_actor_cron::Entry {
            receiver: fil_actors_runtime::STORAGE_POWER_ACTOR_ADDR,
            method_num: fil_actor_power::Method::OnEpochTickEnd as u64,
        },
        fil_actor_cron::Entry {
            receiver: fil_actors_runtime::STORAGE_MARKET_ACTOR_ADDR,
            method_num: fil_actor_market::Method::CronTick as u64,
        },
    ]};
    builder.create_singleton_actor(Type::Cron as u32, &fil_actors_runtime::CRON_ACTOR_ADDR, &cron_state, TokenAmount::zero())?;

    // Power actor
    let power_state = fil_actor_power::State::new(builder.store())?;
    builder.create_singleton_actor(Type::Power as u32, &fil_actors_runtime::STORAGE_POWER_ACTOR_ADDR, &power_state, TokenAmount::zero())?;

    // Market actor
    let market_state = fil_actor_market::State::new(builder.store())?;
    builder.create_singleton_actor(Type::Market as u32, &fil_actors_runtime::STORAGE_MARKET_ACTOR_ADDR, &market_state, TokenAmount::zero())?;

    // A multisig and signer to act as verified registry root.
    let verifreg_signer_state = fil_actor_account::State { address: spec.verifreg_signer.clone() };
    let verifreg_signer_id = builder.create_builtin_actor(Type::Account as u32, &spec.verifreg_signer, &verifreg_signer_state, TokenAmount::zero())?;
    let verifreg_root_state = fil_actor_multisig::State{
        signers: vec![Address::new_id(verifreg_signer_id)],
        num_approvals_threshold: 1,
        next_tx_id: Default::default(),
        initial_balance: Default::default(),
        start_epoch: 0,
        unlock_duration: 0,
        pending_txs: Default::default(),
    };
    let verifreg_root_id = builder.create_builtin_actor(Type::Multisig as u32, &fil_actors_runtime::VERIFIED_REGISTRY_ACTOR_ADDR, &verifreg_root_state, TokenAmount::zero())?;
    
    // Verified registry itself.
    let verifreg_state = fil_actor_verifreg::State::new(builder.store(), Address::new_id(verifreg_root_id))?;
    builder.create_singleton_actor(Type::VerifiedRegistry as u32, &fil_actors_runtime::VERIFIED_REGISTRY_ACTOR_ADDR, &verifreg_state, TokenAmount::zero())?;

    // Datacap actor
    let datacap_state = fil_actor_datacap::State::new(builder.store(), fil_actors_runtime::VERIFIED_REGISTRY_ACTOR_ADDR)?;
    builder.create_singleton_actor(Type::DataCap as u32, &fil_actors_runtime::DATACAP_TOKEN_ACTOR_ADDR, &datacap_state, TokenAmount::zero())?;

    let burnt_state = fil_actor_account::State{ address: BURNT_FUNDS_ACTOR_ADDR };
    builder.create_singleton_actor(Type::Account as u32, &BURNT_FUNDS_ACTOR_ADDR, &burnt_state, TokenAmount::zero())?;

    let faucet_id = BURNT_FUNDS_ACTOR_ADDR.id().unwrap() - 1;
    let faucet_address = Address::new_id(faucet_id);
    let faucet_state = fil_actor_account::State{ address: faucet_address };
    builder.create_singleton_actor(Type::Account as u32, &faucet_address, &faucet_state, spec.faucet_balance.clone())?;
    Ok(GenesisResult{
        verifreg_signer_id,
        verifreg_root_id,
        faucet_id,
    })
}

fn format_trace(trace: &ExecutionTrace) {
    for event in trace {
        println!("event: {:?}", event);
    }
}
