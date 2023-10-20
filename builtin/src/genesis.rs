use cid::Cid;
use fil_actors_integration_tests::TEST_FAUCET_ADDR;
use fil_actors_runtime::runtime::builtins::Type;
use fil_actors_runtime::{
    make_empty_map, BURNT_FUNDS_ACTOR_ADDR, BURNT_FUNDS_ACTOR_ID, CRON_ACTOR_ID,
    DATACAP_TOKEN_ACTOR_ID, EAM_ACTOR_ID, INIT_ACTOR_ID, REWARD_ACTOR_ID,
    STORAGE_MARKET_ACTOR_ADDR, STORAGE_MARKET_ACTOR_ID, STORAGE_POWER_ACTOR_ADDR,
    STORAGE_POWER_ACTOR_ID, SYSTEM_ACTOR_ID, VERIFIED_REGISTRY_ACTOR_ID,
};
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::econ::TokenAmount;
use fvm_shared::sector::StoragePower;
use fvm_shared::{ActorID, HAMT_BIT_WIDTH};

pub use fil_builtin_actors_bundle::BUNDLE_CAR as BUILTIN_ACTORS_BUNDLE;
pub use fvm_workbench_api::bench::WorkbenchBuilder;

/// A specification for installing built-in actors to seed a VM.
pub struct GenesisSpec {
    pub system_manifest_cid: Cid,
    pub reward_balance: TokenAmount,
    pub faucet_balance: TokenAmount,
    pub verifreg_signer: Address,
    pub faucet: Address,
}

impl GenesisSpec {
    pub fn default(manifest_data_cid: Cid) -> Self {
        GenesisSpec {
            system_manifest_cid: manifest_data_cid,
            reward_balance: TokenAmount::from_whole(1_100_000_000),
            faucet_balance: TokenAmount::from_whole(900_000_000),
            verifreg_signer: Address::new_bls(&[200; fvm_shared::address::BLS_PUB_LEN]).unwrap(),
            // Faucet is installed in user-actor address space and needs a BLS/SECP address
            faucet: Address::new_bls(&[201; fvm_shared::address::BLS_PUB_LEN]).unwrap(),
        }
    }
}

pub struct GenesisResult {
    pub verifreg_signer_id: ActorID,
    pub verifreg_root_id: ActorID,
    pub faucet_id: ActorID,
}

impl GenesisResult {
    pub fn verifreg_signer_address(&self) -> Address {
        Address::new_id(self.verifreg_signer_id)
    }
    pub fn verifreg_root_address(&self) -> Address {
        Address::new_id(self.verifreg_root_id)
    }
    pub fn faucet_address(&self) -> Address {
        Address::new_id(self.faucet_id)
    }
}

pub fn create_genesis_actors<B: WorkbenchBuilder>(
    builder: &mut B,
    spec: &GenesisSpec,
) -> anyhow::Result<GenesisResult> {
    // System actor
    let system_state = fil_actor_system::State { builtin_actors: spec.system_manifest_cid };
    builder.create_singleton_actor(
        Type::System as u32,
        SYSTEM_ACTOR_ID,
        &system_state,
        TokenAmount::zero(),
    )?;

    // Init actor
    let init_state = fil_actor_init::State::new(builder.store(), "workbench".to_string())?;
    builder.create_singleton_actor(
        Type::Init as u32,
        INIT_ACTOR_ID,
        &init_state,
        TokenAmount::zero(),
    )?;

    // Reward actor
    let reward_state = fil_actor_reward::State::new(StoragePower::zero());
    builder.create_singleton_actor(
        Type::Reward as u32,
        REWARD_ACTOR_ID,
        &reward_state,
        spec.reward_balance.clone(),
    )?;

    // Cron actor
    let cron_state = fil_actor_cron::State {
        entries: vec![
            fil_actor_cron::Entry {
                receiver: STORAGE_POWER_ACTOR_ADDR,
                method_num: fil_actor_power::Method::OnEpochTickEnd as u64,
            },
            fil_actor_cron::Entry {
                receiver: STORAGE_MARKET_ACTOR_ADDR,
                method_num: fil_actor_market::Method::CronTick as u64,
            },
        ],
    };
    builder.create_singleton_actor(
        Type::Cron as u32,
        CRON_ACTOR_ID,
        &cron_state,
        TokenAmount::zero(),
    )?;

    // Power actor
    let power_state = fil_actor_power::State::new(builder.store())?;
    builder.create_singleton_actor(
        Type::Power as u32,
        STORAGE_POWER_ACTOR_ID,
        &power_state,
        TokenAmount::zero(),
    )?;

    // Market actor
    let market_state = fil_actor_market::State::new(builder.store())?;
    builder.create_singleton_actor(
        Type::Market as u32,
        STORAGE_MARKET_ACTOR_ID,
        &market_state,
        TokenAmount::zero(),
    )?;

    // A multisig and signer to act as verified registry root.
    let verifreg_signer_state = fil_actor_account::State { address: spec.verifreg_signer };
    let verifreg_signer_id = builder.create_builtin_actor(
        Type::Account as u32,
        &spec.verifreg_signer,
        &verifreg_signer_state,
        TokenAmount::zero(),
    )?;
    let empty_root = make_empty_map::<_, ()>(builder.store(), HAMT_BIT_WIDTH).flush().unwrap();
    let verifreg_root_state = fil_actor_multisig::State {
        signers: vec![Address::new_id(verifreg_signer_id)],
        num_approvals_threshold: 1,
        next_tx_id: Default::default(),
        initial_balance: Default::default(),
        start_epoch: 0,
        unlock_duration: 0,
        pending_txs: empty_root,
    };
    let verifreg_root_id = builder.create_builtin_actor(
        Type::Multisig as u32,
        &Address::new_actor(b"VerifiedRegistryRoot"),
        &verifreg_root_state,
        TokenAmount::zero(),
    )?;

    // Verified registry itself.
    let verifreg_state =
        fil_actor_verifreg::State::new(builder.store(), Address::new_id(verifreg_root_id))?;
    builder.create_singleton_actor(
        Type::VerifiedRegistry as u32,
        VERIFIED_REGISTRY_ACTOR_ID,
        &verifreg_state,
        TokenAmount::zero(),
    )?;

    // Datacap actor
    let datacap_state = fil_actor_datacap::State::new(
        builder.store(),
        fil_actors_runtime::VERIFIED_REGISTRY_ACTOR_ADDR,
    )?;
    builder.create_singleton_actor(
        Type::DataCap as u32,
        DATACAP_TOKEN_ACTOR_ID,
        &datacap_state,
        TokenAmount::zero(),
    )?;

    // EAM actor
    let empty_state_array: &[u8; 0] = &[];
    builder.create_singleton_actor(
        Type::EAM as u32,
        EAM_ACTOR_ID,
        empty_state_array,
        TokenAmount::zero(),
    )?;

    // Burnt funds account
    let burnt_state = fil_actor_account::State { address: BURNT_FUNDS_ACTOR_ADDR };
    builder.create_singleton_actor(
        Type::Account as u32,
        BURNT_FUNDS_ACTOR_ID,
        &burnt_state,
        TokenAmount::zero(),
    )?;

    // Faucet account
    let faucet_state = fil_actor_account::State { address: spec.faucet };
    let faucet_id = builder.create_builtin_actor(
        Type::Account as u32,
        &spec.faucet,
        &faucet_state,
        spec.faucet_balance.clone(),
    )?;
    // match builtin-actor's test expectation of a FAUCET_ACTOR as the first user-space actor
    assert_eq!(faucet_id, TEST_FAUCET_ADDR.id().unwrap());

    Ok(GenesisResult { verifreg_signer_id, verifreg_root_id, faucet_id })
}
