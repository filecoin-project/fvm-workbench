use std::collections::BTreeMap;

use anyhow::anyhow;

use cid::Cid;
use fvm::call_manager::DefaultCallManager;
use fvm::engine::EnginePool;
use fvm::executor::{ApplyKind, ApplyRet, DefaultExecutor, Executor};
use fvm::machine::{DefaultMachine, Machine};
use fvm::state_tree::StateTree;
use fvm::trace::ExecutionEvent;
use fvm_ipld_blockstore::Blockstore;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::ActorID;
use fvm_workbench_api::blockstore::DynBlockstore;
use fvm_workbench_api::trace::ExecutionEvent::{
    Call, CallError, CallReturn, GasCharge, InvokeActor,
};
use fvm_workbench_api::trace::ExecutionTrace;
use fvm_workbench_api::{bench::Bench, ExecutionResult};
use vm_api::ActorState;

use crate::externs::FakeExterns;

pub use self::kernel::BenchKernel;

pub mod kernel;

/// A workbench instance backed by a real FVM.
pub struct FvmBench<B>
where
    B: Blockstore + Clone + 'static,
{
    executor: BenchExecutor<B>,
}

type BenchExecutor<B> =
    DefaultExecutor<BenchKernel<DefaultCallManager<DefaultMachine<B, FakeExterns>>>>;

impl<B> FvmBench<B>
where
    B: Blockstore + Clone,
{
    pub fn new(executor: BenchExecutor<B>) -> Self {
        Self { executor }
    }
}

impl<B> Bench for FvmBench<B>
where
    B: Blockstore + Clone,
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
            .get_actor(id)
            .map_err(|e| anyhow!("failed to load actor {}: {}", id, e.to_string()))?;
        // convert from an internal FVM ActorState type to the API ActorState type
        Ok(raw.map(|a| ActorState {
            code: a.code,
            state: a.state,
            sequence: a.sequence,
            balance: a.balance,
            delegated_address: a.delegated_address,
        }))
    }

    fn set_actor(&mut self, key: &Address, state: ActorState) {
        let id = self.resolve_address(key).unwrap().unwrap();
        self.executor.state_tree_mut().set_actor(
            id,
            // convert from API Actor State to internal FVM ActorState type
            fvm::state_tree::ActorState {
                balance: state.balance,
                code: state.code,
                delegated_address: state.delegated_address,
                sequence: state.sequence,
                state: state.state,
            },
        );
    }

    fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>> {
        self.executor
            .state_tree()
            .lookup_id(addr)
            .map_err(|e| anyhow!("failed to resolve address {}: {}", addr, e.to_string()))
    }

    fn set_epoch(&mut self, epoch: ChainEpoch) {
        replace_with::replace_with_or_abort(&mut self.executor, |e| {
            let mut machine = e.into_machine().unwrap();
            let engine_conf = (&machine.context().network).into();
            let mut machine_ctx = machine.context().clone();
            machine_ctx.epoch = epoch;
            machine_ctx.initial_state_root = machine.flush().unwrap();

            // TODO: there is currently no way to get the externs out of the machine.
            // Machine::externs(&self) does exist but since the above line machine.into_store() takes ownership of the
            // machine we cannot borrow it again.
            //
            // Alternatives here that would allow us to keep the generic flexibility over externs
            //
            // - add a function to Machine to allow a single function that takes ownership and returns a tuple of blockstore, externs
            // - add a function to Machine that allows explicit mutation of the MachineContext. Though this seems like a bit of an anti-pattern. My understanding is that the Machine shouldn't really mutate but rather new machines should be instantiated per epoch. But maybe this is ok.
            // - have FakeExterns implement Clone and then clone the externs out of the machine before taking ownership of the machine
            // - have FakeExterns be an indirection to user-provided functionality
            let machine = DefaultMachine::new(
                &machine_ctx,
                machine.into_store().into_inner(),
                FakeExterns::new(),
            )
            .unwrap();

            DefaultExecutor::<BenchKernel<DefaultCallManager<DefaultMachine<B, FakeExterns>>>>::new(
                EnginePool::new_default(engine_conf).unwrap(),
                machine,
            )
            .unwrap()
        });
    }

    fn flush(&mut self) -> Cid {
        self.executor.flush().unwrap()
    }

    fn builtin_actors_manifest(&self) -> BTreeMap<Cid, vm_api::builtin::Type> {
        let manifest = self.executor.builtin_actors();
        let mut map = BTreeMap::new();

        let system = manifest.code_by_id(1);
        if let Some(code) = system {
            map.insert(*code, vm_api::builtin::Type::System);
        }

        let system = manifest.code_by_id(1);
        if let Some(code) = system {
            map.insert(*code, vm_api::builtin::Type::System);
        }

        let init = manifest.code_by_id(2);
        if let Some(code) = init {
            map.insert(*code, vm_api::builtin::Type::Init);
        }

        let cron = manifest.code_by_id(3);
        if let Some(code) = cron {
            map.insert(*code, vm_api::builtin::Type::Cron);
        }

        let account = manifest.code_by_id(4);
        if let Some(code) = account {
            map.insert(*code, vm_api::builtin::Type::Account);
        }

        let power = manifest.code_by_id(5);
        if let Some(code) = power {
            map.insert(*code, vm_api::builtin::Type::Power);
        }

        let miner = manifest.code_by_id(6);
        if let Some(code) = miner {
            map.insert(*code, vm_api::builtin::Type::Miner);
        }

        let market = manifest.code_by_id(7);
        if let Some(code) = market {
            map.insert(*code, vm_api::builtin::Type::Market);
        }

        let payment_channel = manifest.code_by_id(8);
        if let Some(code) = payment_channel {
            map.insert(*code, vm_api::builtin::Type::PaymentChannel);
        }

        let multisig = manifest.code_by_id(9);
        if let Some(code) = multisig {
            map.insert(*code, vm_api::builtin::Type::Multisig);
        }

        let reward = manifest.code_by_id(10);
        if let Some(code) = reward {
            map.insert(*code, vm_api::builtin::Type::Reward);
        }

        let verifreg = manifest.code_by_id(11);
        if let Some(code) = verifreg {
            map.insert(*code, vm_api::builtin::Type::VerifiedRegistry);
        }

        let datacap = manifest.code_by_id(12);
        if let Some(code) = datacap {
            map.insert(*code, vm_api::builtin::Type::DataCap);
        }

        let placeholder = manifest.code_by_id(13);
        if let Some(code) = placeholder {
            map.insert(*code, vm_api::builtin::Type::Placeholder);
        }

        let evm = manifest.code_by_id(14);
        if let Some(code) = evm {
            map.insert(*code, vm_api::builtin::Type::EVM);
        }

        let eam = manifest.code_by_id(15);
        if let Some(code) = eam {
            map.insert(*code, vm_api::builtin::Type::EAM);
        }

        let ethaccount = manifest.code_by_id(16);
        if let Some(code) = ethaccount {
            map.insert(*code, vm_api::builtin::Type::EthAccount);
        }

        map
    }

    fn circulating_supply(&self) -> TokenAmount {
        self.executor.context().circ_supply.clone()
    }

    fn set_circulating_supply(&mut self, amount: TokenAmount) {
        replace_with::replace_with_or_abort(&mut self.executor, |e| {
            let mut machine = e.into_machine().unwrap();
            let engine_conf = (&machine.context().network).into();
            let mut machine_ctx = machine.context().clone();
            machine_ctx.circ_supply = amount;
            machine_ctx.initial_state_root = machine.flush().unwrap();

            let machine = DefaultMachine::new(
                &machine_ctx,
                machine.into_store().into_inner(),
                FakeExterns::new(),
            )
            .unwrap();

            DefaultExecutor::<BenchKernel<DefaultCallManager<DefaultMachine<B, FakeExterns>>>>::new(
                EnginePool::new_default(engine_conf).unwrap(),
                machine,
            )
            .unwrap()
        });
    }

    fn actor_states(&mut self) -> BTreeMap<Address, ActorState> {
        let fvm_root_cid = self.flush();
        let tree =
            StateTree::new_from_root(DynBlockstore::new(self.store()), &fvm_root_cid).unwrap();
        let mut actor_states = BTreeMap::new();
        tree.for_each(|key, state| {
            actor_states.insert(
                key,
                ActorState {
                    code: state.code,
                    state: state.state,
                    sequence: state.sequence,
                    balance: state.balance.clone(),
                    delegated_address: state.delegated_address,
                },
            );
            Ok(())
        })
        .unwrap();
        actor_states
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
                other_milli: e.other_gas.as_milligas(),
            }),
            ExecutionEvent::Call { from, to, method, params, value, gas_limit, read_only } => {
                events.push(Call { from, to, method, params, value, gas_limit, read_only })
            }
            ExecutionEvent::CallReturn(exit_code, return_value) => {
                events.push(CallReturn { exit_code, return_value })
            }
            ExecutionEvent::CallError(e) => events.push(CallError { reason: e.0, errno: e.1 }),
            ExecutionEvent::InvokeActor(cid) => events.push(InvokeActor { cid }),
            _ => todo!(),
        }
    }
    ExecutionTrace::new(events)
}
