use std::collections::HashMap;

use anyhow::anyhow;
use fvm::call_manager::DefaultCallManager;
use fvm::executor::{ApplyKind, ApplyRet, DefaultExecutor, Executor};
use fvm::externs::Externs;
use fvm::machine::{DefaultMachine, Machine};
use fvm::trace::ExecutionEvent;
use fvm::DefaultKernel;
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::{de, from_slice, RawBytes};
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::{ActorID, BLOCK_GAS_LIMIT, MethodNum};
use fvm_workbench_api::trace::ExecutionEvent::{Call, CallAbort, CallError, CallReturn, GasCharge};
use fvm_workbench_api::trace::ExecutionTrace;
use fvm_workbench_api::{ActorState, Bench, ExecutionResult};

/// A workbench instance backed by a real FVM.
pub struct FvmBench<B, E>
where
    B: Blockstore + 'static,
    E: Externs + 'static,
{
    executor: BenchExecutor<B, E>,
}

type BenchExecutor<B, E> =
DefaultExecutor<DefaultKernel<DefaultCallManager<DefaultMachine<B, E>>>>;

impl<B, E> FvmBench<B, E>
where
    B: Blockstore,
    E: Externs,
{
    pub fn new(executor: BenchExecutor<B, E>) -> Self {
        Self { executor }
    }
}

impl<B, E> Bench for FvmBench<B, E>
where
    B: Blockstore,
    E: Externs,
{
    fn execute(&mut self, msg: Message, msg_length: usize) -> anyhow::Result<ExecutionResult> {
        self.executor.execute_message(msg, ApplyKind::Explicit, msg_length).map(ret_as_result)
    }

    fn execute_implicit(
        &mut self,
        msg: Message,
        msg_length: usize,
    ) -> anyhow::Result<ExecutionResult> {
        self.executor.execute_message(msg, ApplyKind::Implicit, msg_length).map(ret_as_result)
    }

    fn epoch(&self) -> ChainEpoch {
        self.executor.context().epoch
    }

    fn store(&self) -> &dyn Blockstore {
        self.executor.blockstore()
    }

    fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<ActorState>> {
        let raw = self
            .executor
            .state_tree()
            .get_actor_id(id)
            .map_err(|e| anyhow!("failed to load actor {}: {}", id, e.to_string()))?;
        Ok(raw.map(|a| ActorState {
            code: a.code,
            state: a.state,
            sequence: a.sequence,
            balance: a.balance,
        }))
    }

    fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>> {
        self.executor
            .state_tree()
            .lookup_id(addr)
            .map_err(|e| anyhow!("failed to resolve address {}: {}", addr, e.to_string()))
    }
}

// Converts an FVM-internal application result to an API execution result.
fn ret_as_result(ret: ApplyRet) -> ExecutionResult {
    ExecutionResult {
        receipt: ret.msg_receipt,
        penalty: ret.penalty,
        miner_tip: ret.miner_tip,
        gas_burned: ret.gas_burned,
        base_fee_burn: ret.base_fee_burn,
        over_estimation_burn: ret.over_estimation_burn,
        trace: trace_as_trace(ret.exec_trace),
        message: ret.failure_info.map_or("".to_string(), |f| f.to_string()),
    }
}

// Converts an FVM-internal trace to a workbench API trace.
fn trace_as_trace(fvm_trace: fvm::trace::ExecutionTrace) -> ExecutionTrace {
    let mut events = Vec::new();
    for e in fvm_trace {
        match e {
            ExecutionEvent::GasCharge(e) => events.push(GasCharge {
                name: e.name,
                compute_milli: e.compute_gas.as_milligas(),
                storage_milli: e.storage_gas.as_milligas(),
            }),
            ExecutionEvent::Call { from, to, method, params, value } => {
                events.push(Call { from, to, method, params, value })
            }
            ExecutionEvent::CallReturn(return_value) => events.push(CallReturn { return_value }),
            ExecutionEvent::CallAbort(exit_code) => events.push(CallAbort { exit_code }),
            ExecutionEvent::CallError(e) => events.push(CallError { reason: e.0, errno: e.1 }),
            _ => {} // Drop unexpected events silently
        }
    }
    ExecutionTrace::new(events)
}

pub struct ExecutionWrangler<'a> {
    bench: &'a mut dyn Bench,
    version: i64,
    gas_limit: i64,
    gas_fee_cap: TokenAmount,
    gas_premium: TokenAmount,
    sequences: HashMap<Address, u64>,
    msg_length: usize,
    compute_msg_length: bool,
}

impl<'a> ExecutionWrangler<'a> {
    pub fn new(
        bench: &'a mut dyn Bench,
        version: i64,
        gas_limit: i64,
        gas_fee_cap: TokenAmount,
        gas_premium: TokenAmount,
        compute_msg_length: bool,
    ) -> Self {
        Self {
            bench,
            version,
            gas_limit,
            gas_fee_cap,
            gas_premium,
            sequences: HashMap::new(),
            msg_length: 0,
            compute_msg_length,
        }
    }

    pub fn new_default(bench: &'a mut dyn Bench) -> Self {
        Self::new(bench, 0, BLOCK_GAS_LIMIT, TokenAmount::zero(), TokenAmount::zero(), true)
    }

    pub fn execute(
        &mut self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
    ) -> anyhow::Result<ExecutionResult> {
        let sequence = self.sequences.get(&from).unwrap_or(&0);
        let (msg, msg_length) = self.make_msg(from, to, method, params, value, *sequence);
        let ret = self.bench.execute(msg, msg_length);
        if ret.is_ok() {
            self.sequences.insert(from, sequence + 1);
        }
        ret
    }

    pub fn execute_implicit(
        &mut self,
        from: Address,
        to: Address,
        method: MethodNum,
        params: RawBytes,
        value: TokenAmount,
    ) -> anyhow::Result<ExecutionResult> {
        let sequence = self.sequences.get(&from).unwrap_or(&0);
        let (msg, msg_length) = self.make_msg(from, to, method, params, value, *sequence);
        let ret = self.bench.execute_implicit(msg, msg_length);
        if ret.is_ok() {
            self.sequences.insert(from, sequence + 1);
        }
        ret
    }

    pub fn epoch(&self) -> ChainEpoch {
        self.bench.epoch()
    }

    pub fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<ActorState>> {
        self.bench.find_actor(id)
    }

    pub fn find_actor_state<T: de::DeserializeOwned>(
        &self,
        id: ActorID,
    ) -> anyhow::Result<Option<T>> {
        let actor = self.bench.find_actor(id)?;
        Ok(match actor {
            Some(actor) => {
                let block = self
                    .bench
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
        self.bench.resolve_address(addr)
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
