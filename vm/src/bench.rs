use anyhow::anyhow;
use fvm::call_manager::DefaultCallManager;
use fvm::executor::{ApplyKind, ApplyRet, DefaultExecutor, Executor};
use fvm::externs::Externs;
use fvm::machine::{DefaultMachine, Machine};
use fvm::trace::ExecutionEvent;
use fvm::DefaultKernel;
use fvm_ipld_blockstore::Blockstore;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::message::Message;
use fvm_shared::ActorID;
use fvm_workbench_api::trace::ExecutionEvent::{
    Call, CallAbort, CallError, CallReturn, GasCharge, Log,
};
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

type BenchExecutor<B, E> = DefaultExecutor<DefaultKernel<DefaultCallManager<DefaultMachine<B, E>>>>;

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
            ExecutionEvent::Log(msg) => events.push(Log { msg }),
            _ => {} // Drop unexpected events silently
        }
    }
    ExecutionTrace::new(events)
}
