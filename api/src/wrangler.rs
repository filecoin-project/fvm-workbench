use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;

use anyhow::anyhow;
use bimap::BiBTreeMap;
use cid::multihash::{Code, MultihashDigest};
use cid::Cid;
use fvm_shared::crypto::signature::{
    Signature, SECP_PUB_LEN, SECP_SIG_LEN, SECP_SIG_MESSAGE_HASH_SIZE,
};
use fvm_shared::error::ExitCode;
use fvm_shared::piece::PieceInfo;
use fvm_shared::sector::RegisteredSealProof;
use std::fmt;

use fil_actors_runtime::runtime::builtins::Type;
use fil_actors_runtime::runtime::{Policy, Primitives};
use fil_actors_runtime::test_utils::{make_piece_cid, recover_secp_public_key};
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::crypto::hash::SupportedHashes;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::{ActorID, MethodNum, BLOCK_GAS_LIMIT};

use crate::trace::InvocationTrace;
pub use crate::{Bench, ExecutionResult};

pub struct ExecutionWrangler<BS: Blockstore> {
    bench: RefCell<Box<dyn Bench>>,
    version: u64,
    gas_limit: u64,
    gas_fee_cap: TokenAmount,
    gas_premium: TokenAmount,
    sequences: RefCell<HashMap<Address, u64>>,
    msg_length: usize,
    compute_msg_length: bool,
    primitives: Box<dyn Primitives>,
    store: BS,
}

impl<BS> ExecutionWrangler<BS>
where
    BS: Blockstore,
{
    pub fn new(
        bench: Box<dyn Bench>,
        version: u64,
        gas_limit: u64,
        gas_fee_cap: TokenAmount,
        gas_premium: TokenAmount,
        compute_msg_length: bool,
        store: BS,
    ) -> Self {
        Self {
            bench: RefCell::new(bench),
            version,
            gas_limit,
            gas_fee_cap,
            gas_premium,
            sequences: RefCell::new(HashMap::new()),
            msg_length: 0,
            compute_msg_length,
            primitives: Box::new(FakePrimitives {}),
            store,
        }
    }

    pub fn new_default(bench: Box<dyn Bench>, blockstore: BS) -> Self {
        Self::new(
            bench,
            0,
            BLOCK_GAS_LIMIT,
            TokenAmount::zero(),
            TokenAmount::zero(),
            true,
            blockstore,
        )
    }

    ///// Private helpers - functionality should be accessed via the VM trait /////

    fn execute(
        &self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
    ) -> anyhow::Result<ExecutionResult> {
        let sequence = *self.sequences.borrow().get(&from).unwrap_or(&0);
        let (msg, msg_length) = self.make_msg(from, to, method, params, value, sequence);
        let ret = self.bench.borrow_mut().execute(msg, msg_length);
        if ret.is_ok() {
            self.sequences.borrow_mut().insert(from, sequence + 1);
        }
        ret
    }

    fn execute_implicit(
        &self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
    ) -> anyhow::Result<ExecutionResult> {
        let sequence = *self.sequences.borrow().get(&from).unwrap_or(&0);
        let (msg, msg_length) = self.make_msg(from, to, method, params, value, sequence);
        let ret = self.bench.borrow_mut().execute_implicit(msg, msg_length);
        if ret.is_ok() {
            self.sequences.borrow_mut().insert(from, sequence + 1);
        }
        ret
    }

    fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<Actor>> {
        self.bench.borrow().find_actor(id)
    }

    // pub fn find_actor_state<T: de::DeserializeOwned>(
    //     &mut self,
    //     id: ActorID,
    // ) -> anyhow::Result<Option<T>> {
    //     let actor = self.bench.borrow().find_actor(id)?;
    //     Ok(match actor {
    //         Some(actor) => {
    //             let block = self
    //                 .bench
    //                 .borrow()
    //                 .store()
    //                 .get(&actor.code)
    //                 .map_err(|e| anyhow!("failed to load state for actor {}: {}", id, e))?;

    //             block
    //                 .map(|s| {
    //                     from_slice(&s)
    //                         .map_err(|e| anyhow!("failed to deserialize actor state: {}", e))
    //                 })
    //                 .transpose()?
    //         }
    //         None => None,
    //     })
    // }

    fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>> {
        self.bench.borrow().resolve_address(addr)
    }

    fn make_msg(
        &self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
        sequence: u64,
    ) -> (Message, usize) {
        let msg = Message {
            from,
            to,
            sequence,
            value,
            method_num: method,
            params,
            version: self.version,
            gas_limit: self.gas_limit,
            gas_fee_cap: self.gas_fee_cap.clone(),
            gas_premium: self.gas_premium.clone(),
        };
        let msg_length = if self.compute_msg_length {
            self.msg_length
        } else {
            0 // FIXME serialize and size
        };
        (msg, msg_length)
    }
}

impl<BS> VM for ExecutionWrangler<BS>
where
    BS: Blockstore,
{
    fn blockstore(&self) -> &dyn Blockstore {
        // It's unfortunate that we need to call flush here everytime we need the blockstore reference
        // However, the state_tree inside Executor wraps whatever blockstore it's given with a BufferedBlockstore
        // Since the store held by the Wrangler was cloned prior to being wrapped in the BufferedBlockstore, it's possible
        // there are pending changes to the underlying blockstore held in the BufferedBlockstore's cache that are not
        // visible via our handle
        self.bench.borrow_mut().flush();
        &self.store
    }

    fn actor_root(&self, address: &Address) -> Option<Cid> {
        let maybe_address = self.resolve_address(address).ok()?;
        let maybe_head = maybe_address.map(|id| {
            let maybe_actor = self.find_actor(id).ok().unwrap_or_default();
            maybe_actor.map(|actor| actor.head)
        });
        maybe_head?
    }

    fn epoch(&self) -> ChainEpoch {
        self.bench.borrow().epoch()
    }

    fn balance(&self, address: &Address) -> TokenAmount {
        let maybe_address = self.resolve_address(address).unwrap();
        let maybe_balance = maybe_address.map(|id| {
            let maybe_actor = self.find_actor(id).ok().unwrap_or_default();
            maybe_actor.map(|actor| actor.balance)
        });
        maybe_balance.unwrap().unwrap()
    }

    fn resolve_id_address(&self, address: &Address) -> Option<Address> {
        let maybe_address = self.resolve_address(address).ok()?;
        maybe_address.map(Address::new_id)
    }

    fn execute_message(
        &self,
        from: &Address,
        to: &Address,
        value: &TokenAmount,
        method: MethodNum,
        params: Option<IpldBlock>,
    ) -> Result<MessageResult, TestVMError> {
        let raw_params = params.map_or(RawBytes::default(), |block| RawBytes::from(block.data));
        match self.execute(*from, *to, method, raw_params, value.clone()) {
            Ok(res) => Ok(res.into()),
            Err(e) => Err(TestVMError { msg: e.to_string() }),
        }
    }

    fn execute_message_implicit(
        &self,
        from: &Address,
        to: &Address,
        value: &TokenAmount,
        method: MethodNum,
        params: Option<IpldBlock>,
    ) -> Result<MessageResult, TestVMError> {
        let raw_params = params.map_or(RawBytes::default(), |block| RawBytes::from(block.data));
        match self.execute_implicit(*from, *to, method, raw_params, value.clone()) {
            Ok(res) => Ok(res.into()),
            Err(e) => Err(TestVMError { msg: e.to_string() }),
        }
    }

    fn set_epoch(&self, epoch: ChainEpoch) {
        self.bench.borrow_mut().set_epoch(epoch)
    }

    fn take_invocations(&self) -> Vec<InvocationTrace> {
        todo!()
    }

    fn actor(&self, address: &Address) -> Option<Actor> {
        let id = self.bench.borrow().resolve_address(address).ok()??;
        self.bench.borrow().find_actor(id).ok()?
    }

    fn actor_manifest(&self) -> BiBTreeMap<Cid, Type> {
        todo!()
    }

    fn primitives(&self) -> &dyn Primitives {
        self.primitives.as_ref()
    }

    fn policy(&self) -> Policy {
        Policy::default()
    }

    fn state_root(&self) -> Cid {
        self.bench.borrow_mut().state_root()
    }

    fn total_fil(&self) -> TokenAmount {
        self.bench.borrow().total_fil()
    }
}

#[derive(Debug)]
pub struct TestVMError {
    msg: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Actor {
    pub code: Cid,
    pub head: Cid,
    pub call_seq_num: u64,
    pub balance: TokenAmount,
    pub predictable_address: Option<Address>,
}

pub fn actor(
    code: Cid,
    head: Cid,
    call_seq_num: u64,
    balance: TokenAmount,
    predictable_address: Option<Address>,
) -> Actor {
    Actor { code, head, call_seq_num, balance, predictable_address }
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MessageResult {
    pub code: ExitCode,
    pub message: String,
    pub ret: Option<IpldBlock>,
}

/// An abstract VM that is injected into integration tests
pub trait VM {
    /// Returns the underlying blockstore of the VM
    fn blockstore(&self) -> &dyn Blockstore;

    /// Get the state root of the specified actor
    fn actor_root(&self, address: &Address) -> Option<Cid>;

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
    fn actor(&self, address: &Address) -> Option<Actor>;

    /// Build a map of all actors in the system and their type
    fn actor_manifest(&self) -> BiBTreeMap<Cid, Type>;

    /// Provides access to VM primitives
    fn primitives(&self) -> &dyn Primitives;

    /// Get the current runtime policy
    fn policy(&self) -> Policy;

    /// Get the root Cid of the state tree
    fn state_root(&self) -> Cid;

    /// Get the total amount of FIL in circulation
    fn total_fil(&self) -> TokenAmount;
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
        recover_secp_public_key(hash, signature).map_err(|_| anyhow!("failed to recover pubkey"))
    }
}
