use bls_signatures::Serialize as BLS_Serialize;
use cid::Cid;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::util::cbor::serialize;
use fvm_ipld_bitfield::BitField;
use fvm_ipld_encoding::ser;
use fvm_shared::address::{Address, Protocol};
use fvm_shared::commcid::FIL_COMMITMENT_UNSEALED;
use fvm_shared::crypto::signature::Signature;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::{ActorID, MethodNum};
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::ExecutionResult;
use multihash::derive::Multihash;
use multihash::MultihashDigest;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;

pub mod hookup;
pub mod workflows;

pub fn apply_ok<T: ser::Serialize + ?Sized>(
    w: &mut ExecutionWrangler,
    from: Address,
    to: Address,
    value: TokenAmount,
    method: MethodNum,
    params: &T,
) -> anyhow::Result<ExecutionResult> {
    apply_code(w, from, to, value, method, params, ExitCode::OK)
}

pub fn apply_code<T: ser::Serialize + ?Sized>(
    w: &mut ExecutionWrangler,
    from: Address,
    to: Address,
    value: TokenAmount,
    method: MethodNum,
    params: &T,
    code: ExitCode,
) -> anyhow::Result<ExecutionResult> {
    // Implicit execution is used because tests often trigger messages from non-account actors.
    let ret = w.execute_implicit(from, to, method, serialize(params, "params").unwrap(), value)?;
    if ret.receipt.exit_code != code {
        println!("{}", ret.trace.format());
    }
    assert_eq!(
        code, ret.receipt.exit_code,
        "expected code {}, got {} ({})",
        code, ret.receipt.exit_code, ret.message
    );
    Ok(ret)
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
