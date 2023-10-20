use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use anyhow::anyhow;
use cid::Cid;
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::de;
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_ipld_encoding::{from_slice, RawBytes};
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::{ActorID, MethodNum, BLOCK_GAS_LIMIT};
use vm_api::trace::InvocationTrace;
use vm_api::{vm_err, ActorState, MessageResult, Primitives, VMError, VM};

pub use crate::{bench::Bench, trace::ExecutionTrace, ExecutionResult};

pub struct ExecutionWrangler {
    bench: RefCell<Box<dyn Bench>>,
    store: Box<dyn Blockstore>,
    primitives: Box<dyn Primitives>,
    version: u64,
    gas_limit: u64,
    gas_fee_cap: TokenAmount,
    gas_premium: TokenAmount,
    sequences: RefCell<HashMap<Address, u64>>,
    msg_length: usize,
    compute_msg_length: bool,
    execution_results: RefCell<Vec<ExecutionTrace>>,
}

impl ExecutionWrangler {
    /// Creates a new wrangler wrapping a given Bench. The store passed here must be a handle that
    /// operates on the same underlying storage as the bench
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bench: Box<dyn Bench>,
        store: Box<dyn Blockstore>,
        primitives: Box<dyn Primitives>,
        version: u64,
        gas_limit: u64,
        gas_fee_cap: TokenAmount,
        gas_premium: TokenAmount,
        compute_msg_length: bool,
    ) -> Self {
        Self {
            bench: RefCell::new(bench),
            store,
            primitives,
            version,
            gas_limit,
            gas_fee_cap,
            gas_premium,
            sequences: RefCell::new(HashMap::new()),
            msg_length: 0,
            compute_msg_length,
            execution_results: RefCell::new(Vec::new()),
        }
    }

    /// Creates a new wrangler wrapping a given Bench. The store passed here must be a handle that
    /// operates on the same underlying storage as the bench
    pub fn new_default(
        bench: Box<dyn Bench>,
        store: Box<dyn Blockstore>,
        primitives: Box<dyn Primitives>,
    ) -> Self {
        Self::new(
            bench,
            store,
            primitives,
            0,
            BLOCK_GAS_LIMIT,
            TokenAmount::zero(),
            TokenAmount::zero(),
            true,
        )
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

    /// Returns a copy of the last execution trace if any exist
    /// For test assertions you probably want VM::take_invocations instead
    /// NOTE: These traces will be cleared if take_invocations was called earlier
    pub fn peek_execution_trace(&self) -> Option<ExecutionTrace> {
        self.execution_results.borrow().last().cloned()
    }
}

// Private helpers
impl ExecutionWrangler {
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

    fn execute(
        &self,
        params: Option<IpldBlock>,
        from: &Address,
        to: &Address,
        method: u64,
        value: &TokenAmount,
        implicit: bool,
    ) -> Result<MessageResult, VMError> {
        let raw_params = params.map_or(RawBytes::default(), |block| RawBytes::from(block.data));
        let sequence = *self.sequences.borrow().get(from).unwrap_or(&0);
        let (msg, msg_length) =
            self.make_msg(*from, *to, method, raw_params, value.clone(), sequence);
        let ret = match implicit {
            true => self.bench.borrow_mut().execute_implicit(msg, msg_length),
            false => self.bench.borrow_mut().execute(msg, msg_length),
        };
        if ret.is_ok() {
            self.sequences.borrow_mut().insert(*from, sequence + 1);
        }
        match ret {
            Ok(res) => {
                self.execution_results.borrow_mut().push(res.trace.clone());
                Ok(res.into())
            }
            Err(e) => Err(vm_err(&e.to_string())),
        }
    }
}

impl VM for ExecutionWrangler {
    fn blockstore(&self) -> &dyn Blockstore {
        // It's unfortunate that we need to call flush here everytime we need the blockstore reference
        // However, the state_tree inside Executor wraps whatever blockstore it's given with a BufferedBlockstore
        // Since the store held by the Wrangler was cloned prior to being wrapped in the BufferedBlockstore, it's possible
        // there are pending changes to the underlying blockstore held in the BufferedBlockstore's cache that are not
        // visible via our handle
        self.bench.borrow_mut().flush();
        self.store.as_ref()
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
    ) -> Result<MessageResult, VMError> {
        self.execute(params, from, to, method, value, false)
    }

    fn execute_message_implicit(
        &self,
        from: &Address,
        to: &Address,
        value: &TokenAmount,
        method: MethodNum,
        params: Option<IpldBlock>,
    ) -> Result<MessageResult, VMError> {
        self.execute(params, from, to, method, value, true)
    }

    fn set_epoch(&self, epoch: ChainEpoch) {
        self.bench.borrow_mut().set_epoch(epoch)
    }

    /// Note: this is derived from the underlying ExecutionTraces, so it will clear those when taken
    fn take_invocations(&self) -> Vec<InvocationTrace> {
        self.execution_results.take().into_iter().map(InvocationTrace::from).collect()
    }

    fn actor(&self, address: &Address) -> Option<ActorState> {
        let id = self.bench.borrow().resolve_address(address).ok()??;
        self.bench.borrow().find_actor(id).ok()?
    }

    fn primitives(&self) -> &dyn Primitives {
        self.primitives.as_ref()
    }

    fn set_circulating_supply(&self, supply: TokenAmount) {
        self.bench.borrow_mut().set_circulating_supply(supply);
    }

    fn circulating_supply(&self) -> TokenAmount {
        self.bench.borrow().circulating_supply().clone()
    }

    fn actor_manifest(&self) -> BTreeMap<Cid, vm_api::builtin::Type> {
        self.bench.borrow().builtin_actors_manifest()
    }

    fn actor_states(&self) -> BTreeMap<Address, ActorState> {
        self.bench.borrow_mut().actor_states()
    }

    fn set_actor(&self, key: &Address, state: ActorState) {
        self.bench.borrow_mut().set_actor(key, state)
    }
}
