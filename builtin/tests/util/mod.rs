#![allow(dead_code)]
// Code used only in tests is treated as "dead"
use bls_signatures::Serialize as BLS_Serialize;
use cid::Cid;
use fil_actor_miner::DeadlineInfo;
use fil_actor_miner::{
    max_prove_commit_duration, new_deadline_info, CompactCommD, Method as MinerMethod,
    PreCommitSectorBatchParams, PreCommitSectorBatchParams2, PreCommitSectorParams,
    SectorPreCommitInfo, State as MinerState,
};
use fil_actors_runtime::runtime::Policy;
use fvm_ipld_bitfield::BitField;
use fvm_ipld_encoding::de::DeserializeOwned;
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_ipld_encoding::serde::Serialize;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::{Address, Protocol, FIRST_NON_SINGLETON_ADDR};
use fvm_shared::clock::{ChainEpoch, QuantSpec};
use fvm_shared::commcid::{FIL_COMMITMENT_SEALED, FIL_COMMITMENT_UNSEALED};
use fvm_shared::crypto::signature::Signature;
use fvm_shared::deal::DealID;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::sector::{RegisteredSealProof, SectorNumber};
use fvm_shared::{ActorID, MethodNum};
use fvm_workbench_api::blockstore::DynBlockstore;
use fvm_workbench_api::wrangler::VM;
use multihash::derive::Multihash;
use multihash::MultihashDigest;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;

pub mod deals;
pub mod hookup;
pub mod workflows;

// accounts for verifreg root signer and msig
// pub const VERIFREG_ROOT_KEY: &[u8] = &[200; fvm_shared::address::BLS_PUB_LEN];
pub const TEST_VERIFREG_ROOT_SIGNER_ADDR: Address = Address::new_id(FIRST_NON_SINGLETON_ADDR);
pub const TEST_VERIFREG_ROOT_ADDR: Address = Address::new_id(FIRST_NON_SINGLETON_ADDR + 1);
// account actor seeding funds created by new_with_singletons
// pub const FAUCET_ROOT_KEY: &[u8] = &[153; fvm_shared::address::BLS_PUB_LEN];
pub const TEST_FAUCET_ADDR: Address = Address::new_id(FIRST_NON_SINGLETON_ADDR - 2); // TODO: this should be dynamically read from genesis not hardcoded
pub const FIRST_TEST_USER_ADDR: ActorID = FIRST_NON_SINGLETON_ADDR + 2;

pub fn apply_ok<S: Serialize>(
    v: &mut dyn VM,
    from: &Address,
    to: &Address,
    value: &TokenAmount,
    method: MethodNum,
    params: Option<S>,
) -> RawBytes {
    apply_code(v, from, to, value, method, params, ExitCode::OK)
}

pub fn apply_code<S: Serialize>(
    v: &mut dyn VM,
    from: &Address,
    to: &Address,
    value: &TokenAmount,
    method: MethodNum,
    params: Option<S>,
    code: ExitCode,
) -> RawBytes {
    let params = params.map(|p| IpldBlock::serialize_cbor(&p).unwrap().unwrap());
    let res = v.execute_message(from, to, value, method, params).unwrap();
    assert_eq!(code, res.code, "expected code {}, got {} ({})", code, res.code, res.message);
    res.ret.map_or(RawBytes::default(), |b| RawBytes::new(b.data))
}

pub fn apply_ok_implicit<S: Serialize>(
    v: &mut dyn VM,
    from: &Address,
    to: &Address,
    value: &TokenAmount,
    method: MethodNum,
    params: Option<S>,
) -> RawBytes {
    apply_code_implicit(v, from, to, value, method, params, ExitCode::OK)
}

pub fn apply_code_implicit<S: Serialize>(
    v: &mut dyn VM,
    from: &Address,
    to: &Address,
    value: &TokenAmount,
    method: MethodNum,
    params: Option<S>,
    code: ExitCode,
) -> RawBytes {
    let params = params.map(|p| IpldBlock::serialize_cbor(&p).unwrap().unwrap());
    let res = v.execute_message_implicit(from, to, value, method, params).unwrap();
    assert_eq!(code, res.code, "expected code {}, got {} ({})", code, res.code, res.message);
    res.ret.map_or(RawBytes::default(), |b| RawBytes::new(b.data))
}

/// A crypto-key backed account, including the secret key.
/// Currently always a SECP256k1 key.
#[derive(Debug, Clone)]
pub struct Account {
    pub id: ActorID,
    pub key: AccountKey,
}

impl Account {
    pub fn id_addr(&self) -> Address {
        Address::new_id(self.id)
    }

    pub fn key_addr(&self) -> Address {
        self.key.addr
    }

    pub fn sign(&self, msg: &[u8]) -> anyhow::Result<Signature> {
        self.key.sign(msg)
    }
}

#[derive(Debug, Clone)]
pub struct AccountKey {
    pub addr: Address,
    pub secret_key: Vec<u8>,
}

impl AccountKey {
    pub fn new_secp(secret_key: libsecp256k1::SecretKey) -> anyhow::Result<Self> {
        let pubkey = libsecp256k1::PublicKey::from_secret_key(&secret_key);
        let addr = Address::new_secp256k1(&pubkey.serialize())?;
        Ok(Self { addr, secret_key: secret_key.serialize().to_vec() })
    }

    pub fn new_bls(secret_key: bls_signatures::PrivateKey) -> anyhow::Result<Self> {
        let pubkey = secret_key.public_key();
        let addr = Address::new_bls(&pubkey.as_bytes())?;
        Ok(Self { addr, secret_key: secret_key.as_bytes() })
    }

    pub fn sign(&self, msg: &[u8]) -> anyhow::Result<Signature> {
        match self.addr.protocol() {
            Protocol::Secp256k1 => self.sign_secp(msg),
            Protocol::BLS => {
                unimplemented!("BLS signing not implemented")
            }
            Protocol::ID => {
                panic!("cannot sign with ID address")
            }
            Protocol::Actor => {
                panic!("cannot sign with actor address")
            }
            Protocol::Delegated => {
                panic!("delegated signing not implemented")
            }
        }
    }

    pub fn sign_secp(&self, msg: &[u8]) -> anyhow::Result<Signature> {
        let key = libsecp256k1::SecretKey::parse_slice(&self.secret_key)?;
        let hash = blake2b_simd::Params::new().hash_length(32).to_state().update(msg).finalize();
        let message = libsecp256k1::Message::parse_slice(hash.as_bytes())?;
        let (sig, recovery_id) = libsecp256k1::sign(&message, &key);
        let mut signature = [0; 65];
        signature[..64].copy_from_slice(&sig.serialize());
        signature[64] = recovery_id.serialize();
        Ok(Signature::new_secp256k1(signature.to_vec()))
    }
}

// Generate count SECP256k1 addresses by seeding an rng.
pub fn make_secp_keys(seed: u64, count: u64) -> Vec<AccountKey> {
    let mut rng = rng_from_seed(seed);
    (0..count)
        .map(|_| {
            let sk = libsecp256k1::SecretKey::random(&mut rng);
            AccountKey::new_secp(sk).unwrap()
        })
        .collect()
}

pub fn make_bls_keys(seed: u64, count: u64) -> Vec<AccountKey> {
    let mut rng = rng_from_seed(seed);
    (0..count)
        .map(|_| {
            let sk = bls_signatures::PrivateKey::generate(&mut rng);
            AccountKey::new_bls(sk).unwrap()
        })
        .collect()
}

fn rng_from_seed(seed: u64) -> ChaCha8Rng {
    let mut seed_arr = [0u8; 32];
    for (i, b) in seed.to_ne_bytes().iter().enumerate() {
        seed_arr[i] = *b;
    }
    ChaCha8Rng::from_seed(seed_arr)
}

pub fn make_cid(input: &[u8], prefix: u64, hash: MhCode) -> Cid {
    let hash = hash.digest(input);
    Cid::new_v1(prefix, hash)
}

pub fn make_cid_sha(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::Sha256TruncPaddedFake)
}

pub fn make_piece_cid(input: &[u8]) -> Cid {
    make_cid_sha(input, FIL_COMMITMENT_UNSEALED)
}

// multihash library doesn't support poseidon hashing, so we fake it
#[derive(Clone, Copy, Debug, PartialEq, Eq, Multihash)]
#[mh(alloc_size = 64)]
pub enum MhCode {
    #[mh(code = 0xb401, hasher = multihash::Sha2_256)]
    PoseidonFake,
    #[mh(code = 0x1012, hasher = multihash::Sha2_256)]
    Sha256TruncPaddedFake,
}

pub fn bf_all(bf: BitField) -> Vec<u64> {
    bf.bounded_iter(Policy::default().addressed_sectors_max).unwrap().collect()
}

#[derive(Debug)]
pub struct MinerBalances {
    pub available_balance: TokenAmount,
    pub vesting_balance: TokenAmount,
    pub initial_pledge: TokenAmount,
    pub pre_commit_deposit: TokenAmount,
}

#[derive(Debug)]
pub struct SectorInfo {
    pub number: SectorNumber,
    pub deadline_info: DeadlineInfo,
    pub partition_index: u64,
}

#[derive(Debug)]
pub struct MinerInfo {
    pub seal_proof: RegisteredSealProof,
    pub worker: Address,
    pub miner_id: Address,
    pub owner: Address,
    pub miner_robust: Address,
}

pub fn make_sealed_cid(input: &[u8]) -> Cid {
    make_cid_poseidon(input, FIL_COMMITMENT_SEALED)
}

pub fn make_cid_poseidon(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::PoseidonFake)
}

pub fn make_bitfield(bits: &[u64]) -> BitField {
    BitField::try_from_bits(bits.iter().copied()).unwrap()
}

pub fn miner_dline_info(v: &mut dyn VM, m: &Address) -> DeadlineInfo {
    let st: MinerState = get_state(v, m).unwrap();
    new_deadline_info_from_offset_and_epoch(&Policy::default(), st.proving_period_start, v.epoch())
}

pub fn sector_deadline(v: &mut dyn VM, m: &Address, s: SectorNumber) -> (u64, u64) {
    let st: MinerState = get_state(v, m).unwrap();
    st.find_sector(&Policy::default(), &DynBlockstore::new(v.blockstore()), s).unwrap()
}

pub fn get_state<T: DeserializeOwned>(v: &mut dyn VM, a: &Address) -> Option<T> {
    let cid = v.actor_root(a).unwrap();
    v.blockstore().get(&cid).unwrap().map(|slice| fvm_ipld_encoding::from_slice(&slice).unwrap())
}

pub fn new_deadline_info_from_offset_and_epoch(
    policy: &Policy,
    period_start_seed: ChainEpoch,
    current_epoch: ChainEpoch,
) -> DeadlineInfo {
    let q = QuantSpec { unit: policy.wpost_proving_period, offset: period_start_seed };
    let current_period_start = q.quantize_down(current_epoch);
    let current_deadline_idx = ((current_epoch - current_period_start)
        / policy.wpost_challenge_window) as u64
        % policy.wpost_period_deadlines;
    new_deadline_info(policy, current_period_start, current_deadline_idx, current_epoch)
}
pub fn miner_balance(v: &mut dyn VM, m: &Address) -> MinerBalances {
    let st: MinerState = get_state(v, m).unwrap();
    MinerBalances {
        available_balance: st.get_available_balance(&v.balance(m)).unwrap(),
        vesting_balance: st.locked_funds,
        initial_pledge: st.initial_pledge,
        pre_commit_deposit: st.pre_commit_deposits,
    }
}

/// Convenience function to create an IpldBlock from a serializable object
pub fn serialize_ok<S: Serialize>(s: &S) -> IpldBlock {
    IpldBlock::serialize_cbor(s).unwrap().unwrap()
}

// pub fn get_network_stats<BS: Blockstore>(vm: &dyn VM<BS>) -> NetworkStats {
//     let power_state: PowerState = get_state(vm, &STORAGE_POWER_ACTOR_ADDR).unwrap();
//     let reward_state: RewardState = get_state(vm, &REWARD_ACTOR_ADDR).unwrap();
//     let market_state: MarketState = get_state(vm, &STORAGE_MARKET_ACTOR_ADDR).unwrap();

//     NetworkStats {
//         total_raw_byte_power: power_state.total_raw_byte_power,
//         total_bytes_committed: power_state.total_bytes_committed,
//         total_quality_adj_power: power_state.total_quality_adj_power,
//         total_qa_bytes_committed: power_state.total_qa_bytes_committed,
//         total_pledge_collateral: power_state.total_pledge_collateral,
//         this_epoch_raw_byte_power: power_state.this_epoch_raw_byte_power,
//         this_epoch_quality_adj_power: power_state.this_epoch_quality_adj_power,
//         this_epoch_pledge_collateral: power_state.this_epoch_pledge_collateral,
//         miner_count: power_state.miner_count,
//         miner_above_min_power_count: power_state.miner_above_min_power_count,
//         this_epoch_reward: reward_state.this_epoch_reward,
//         this_epoch_reward_smoothed: reward_state.this_epoch_reward_smoothed,
//         this_epoch_baseline_power: reward_state.this_epoch_baseline_power,
//         total_storage_power_reward: reward_state.total_storage_power_reward,
//         total_client_locked_collateral: market_state.total_client_locked_collateral,
//         total_provider_locked_collateral: market_state.total_provider_locked_collateral,
//         total_client_storage_fee: market_state.total_client_storage_fee,
//     }
// }

#[derive(Debug, Clone)]
pub struct PrecommitMetadata {
    pub deals: Vec<DealID>,
    pub commd: CompactCommD,
}
