use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::anyhow;
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::{de, from_slice, RawBytes};
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::{ActorID, MethodNum, BLOCK_GAS_LIMIT};

pub use crate::bench::{ActorState, Bench, ExecutionResult};

/// High level wrapper of a Bench for convenience
pub struct ExecutionWrangler {
    bench: RefCell<Box<dyn Bench>>,
    store: Box<dyn Blockstore>,
    version: u64,
    gas_limit: u64,
    gas_fee_cap: TokenAmount,
    gas_premium: TokenAmount,
    sequences: RefCell<HashMap<Address, u64>>,
    msg_length: usize,
    compute_msg_length: bool,
}

impl ExecutionWrangler {
    /// Creates a new wrangler wrapping a given Bench. The store passed here must be a handle that
    /// operates on the same underlying storage as the bench
    pub fn new(
        bench: Box<dyn Bench>,
        store: Box<dyn Blockstore>,
        version: u64,
        gas_limit: u64,
        gas_fee_cap: TokenAmount,
        gas_premium: TokenAmount,
        compute_msg_length: bool,
    ) -> Self {
        Self {
            bench: RefCell::new(bench),
            store,
            version,
            gas_limit,
            gas_fee_cap,
            gas_premium,
            sequences: RefCell::new(HashMap::new()),
            msg_length: 0,
            compute_msg_length,
        }
    }

    /// Creates a new wrangler wrapping a given Bench. The store passed here must be a handle that
    /// operates on the same underlying storage as the bench
    pub fn new_default(bench: Box<dyn Bench>, store: Box<dyn Blockstore>) -> Self {
        Self::new(bench, store, 0, BLOCK_GAS_LIMIT, TokenAmount::zero(), TokenAmount::zero(), true)
    }

    pub fn execute(
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

    pub fn execute_implicit(
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

    pub fn epoch(&self) -> ChainEpoch {
        self.bench.borrow().epoch()
    }

    pub fn set_epoch(&self, epoch: ChainEpoch) {
        self.bench.borrow_mut().set_epoch(epoch);
    }

    pub fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<ActorState>> {
        self.bench.borrow().find_actor(id)
    }

    pub fn find_actor_state<T: de::DeserializeOwned>(
        &self,
        id: ActorID,
    ) -> anyhow::Result<Option<T>> {
        let actor = self.bench.borrow().find_actor(id)?;
        Ok(match actor {
            Some(actor) => {
                let block = self
                    .bench
                    .borrow()
                    .store()
                    .get(&actor.state)
                    .map_err(|e| anyhow!("failed to load state for actor {}: {}", id, e))?;

                block
                    .map(|s| {
                        from_slice(&s)
                            .map_err(|e| anyhow!("failed to deserialize actor state: {}", e))
                    })
                    .transpose()?
            }
            None => None,
        })
    }

    pub fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>> {
        self.bench.borrow().resolve_address(addr)
    }

    /// Returns a reference to the underlying blockstore
    /// The blockstore handle here is intended to be short-lived as some executors may buffer changes leading to this
    /// handle drifting out of sync
    // TODO: https://github.com/anorth/fvm-workbench/issues/15
    pub fn store(&self) -> &dyn Blockstore {
        // It's unfortunate that we need to call flush here everytime we need the wrangler to give out a blockstore reference
        // However, the state_tree inside Executor wraps whatever blockstore it's given with a BufferedBlockstore
        // Since the store held by the Wrangler was cloned prior to being wrapped in the BufferedBlockstore, it's possible
        // there are pending changes to the underlying blockstore held in the BufferedBlockstore's cache that are not
        // visible via our handle.
        self.bench.borrow_mut().flush();
        self.store.as_ref()
    }

    ///// Private helpers /////
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
