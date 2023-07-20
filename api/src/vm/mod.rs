/*!
 * The VM module is replicated in this code tree temporarily. This is the high-level abstract interface
 * for a virtual-machine that can execute Filecoin WASM actors. It defines the high-level virtual-machine
 * interface, associated error and trace types and an interface to inject/override the behaviour of
 * certain primitives for the purpose of running tests.
 *
 * TODO(alexytsu): It should eventually be moved to an external location so that it can be shared
 * with the builtin-actors integration tests' implementation
 */

use std::error::Error;
use std::fmt;

use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::crypto::hash::SupportedHashes;
use fvm_shared::crypto::signature::{
    Signature, SECP_PUB_LEN, SECP_SIG_LEN, SECP_SIG_MESSAGE_HASH_SIZE,
};
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::ExitCode;
use fvm_shared::piece::PieceInfo;
use fvm_shared::sector::RegisteredSealProof;
use fvm_shared::MethodNum;

use anyhow::anyhow;
use cid::multihash::MultihashDigest;
use cid::Cid;
use multihash::Code;

use self::test_utils::{make_piece_cid, recover_secp_public_key};
use crate::trace::InvocationTrace;
use crate::{ActorState, ExecutionResult};

pub mod test_utils;

/// An abstract VM that is injected into integration tests
pub trait VM {
    /// Returns the underlying blockstore of the VM
    fn blockstore(&self) -> &dyn Blockstore;

    /// Get the current chain epoch
    fn epoch(&self) -> ChainEpoch;

    /// Get the balance of the specified actor
    fn balance(&self, address: &Address) -> TokenAmount;

    /// Get the ID for the specified address
    fn resolve_id_address(&self, address: &Address) -> Option<Address>;

    /// Send a message between the two specified actors
    fn execute_message(
        &self,
        from: &Address,
        to: &Address,
        value: &TokenAmount,
        method: MethodNum,
        params: Option<IpldBlock>,
    ) -> Result<MessageResult, TestVMError>;

    /// Send a message without charging gas
    fn execute_message_implicit(
        &self,
        from: &Address,
        to: &Address,
        value: &TokenAmount,
        method: MethodNum,
        params: Option<IpldBlock>,
    ) -> Result<MessageResult, TestVMError>;

    /// Sets the epoch to the specified value
    fn set_epoch(&self, epoch: ChainEpoch);

    /// Take all the invocations that have been made since the last call to this method
    fn take_invocations(&self) -> Vec<InvocationTrace>;

    /// Get information about an actor
    fn actor(&self, address: &Address) -> Option<ActorState>;

    /// Provides access to VM primitives
    fn primitives(&self) -> &dyn Primitives;
}

impl From<ExecutionResult> for MessageResult {
    fn from(value: ExecutionResult) -> Self {
        Self {
            code: value.receipt.exit_code,
            message: value.message,
            ret: value.receipt.return_data.into(),
        }
    }
}

/// Pure functions implemented as primitives by the runtime.
pub trait Primitives {
    /// Hashes input data using blake2b with 256 bit output.
    fn hash_blake2b(&self, data: &[u8]) -> [u8; 32];

    /// Hashes input data using a supported hash function.
    fn hash(&self, hasher: SupportedHashes, data: &[u8]) -> Vec<u8>;

    /// Hashes input into a 64 byte buffer
    fn hash_64(&self, hasher: SupportedHashes, data: &[u8]) -> ([u8; 64], usize);

    /// Computes an unsealed sector CID (CommD) from its constituent piece CIDs (CommPs) and sizes.
    fn compute_unsealed_sector_cid(
        &self,
        proof_type: RegisteredSealProof,
        pieces: &[PieceInfo],
    ) -> Result<Cid, anyhow::Error>;

    /// Verifies that a signature is valid for an address and plaintext.
    fn verify_signature(
        &self,
        signature: &Signature,
        signer: &Address,
        plaintext: &[u8],
    ) -> Result<(), anyhow::Error>;

    fn recover_secp_public_key(
        &self,
        hash: &[u8; SECP_SIG_MESSAGE_HASH_SIZE],
        signature: &[u8; SECP_SIG_LEN],
    ) -> Result<[u8; SECP_PUB_LEN], anyhow::Error>;
}

// Fake implementation of runtime primitives.
// Struct members can be added here to provide configurable functionality.
pub struct FakePrimitives {}

impl Primitives for FakePrimitives {
    fn hash_blake2b(&self, data: &[u8]) -> [u8; 32] {
        blake2b_simd::Params::new()
            .hash_length(32)
            .to_state()
            .update(data)
            .finalize()
            .as_bytes()
            .try_into()
            .unwrap()
    }

    fn hash(&self, hasher: SupportedHashes, data: &[u8]) -> Vec<u8> {
        let hasher = Code::try_from(hasher as u64).unwrap(); // supported hashes are all implemented in multihash
        hasher.digest(data).digest().to_owned()
    }

    fn hash_64(&self, hasher: SupportedHashes, data: &[u8]) -> ([u8; 64], usize) {
        let hasher = Code::try_from(hasher as u64).unwrap();
        let (len, buf, ..) = hasher.digest(data).into_inner();
        (buf, len as usize)
    }

    fn compute_unsealed_sector_cid(
        &self,
        _proof_type: RegisteredSealProof,
        _pieces: &[PieceInfo],
    ) -> Result<Cid, anyhow::Error> {
        Ok(make_piece_cid(b"test data"))
    }

    fn verify_signature(
        &self,
        signature: &Signature,
        _signer: &Address,
        plaintext: &[u8],
    ) -> Result<(), anyhow::Error> {
        if signature.bytes != plaintext {
            return Err(anyhow::format_err!(
                "invalid signature (mock sig validation expects siggy bytes to be equal to plaintext)"
            ));
        }
        Ok(())
    }

    fn recover_secp_public_key(
        &self,
        hash: &[u8; SECP_SIG_MESSAGE_HASH_SIZE],
        signature: &[u8; SECP_SIG_LEN],
    ) -> Result<[u8; SECP_PUB_LEN], anyhow::Error> {
        recover_secp_public_key(hash, signature)
            .map_err(|_| anyhow!("failed to recover secp public key"))
    }
}

#[derive(Debug)]
pub struct TestVMError {
    pub msg: String,
}

impl fmt::Display for TestVMError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl Error for TestVMError {
    fn description(&self) -> &str {
        &self.msg
    }
}

impl From<fvm_ipld_hamt::Error> for TestVMError {
    fn from(h_err: fvm_ipld_hamt::Error) -> Self {
        vm_err(h_err.to_string().as_str())
    }
}

pub fn vm_err(msg: &str) -> TestVMError {
    TestVMError { msg: msg.to_string() }
}

pub fn actor(code: Cid, state: Cid, sequence: u64, balance: TokenAmount) -> ActorState {
    ActorState { code, state, sequence, balance }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MessageResult {
    pub code: ExitCode,
    pub message: String,
    pub ret: Option<IpldBlock>,
}
