use fvm::executor::{ApplyKind, ApplyRet, DefaultExecutor, Executor};
use fvm::DefaultKernel;
use fvm::call_manager::DefaultCallManager;
use fvm::machine::DefaultMachine;
use fvm_ipld_blockstore::Blockstore;
use fvm::externs::Externs;
use fvm_shared::message::Message;

pub type BenchExecutor<B, E> =
    DefaultExecutor<DefaultKernel<DefaultCallManager<DefaultMachine<B, E>>>>;

pub struct Bench<B, E>
where
    B: Blockstore + 'static,
    E: Externs + 'static,
{
    executor: BenchExecutor<B, E>,
}

impl<B, E> Bench<B, E>
where
    B: Blockstore,
    E: Externs,
{
    pub fn new(executor: BenchExecutor<B, E>) -> Self {
        Self { executor }
    }

    // Explicit messages may only come from account actors and charge the sending account for gas consumed.
    // Implicit messages may come from any actor, ignore the nonce, and charge no gas (but still account for it).
    pub fn execute(&mut self, msg: Message) -> anyhow::Result<ApplyRet> {
        let msg_length = 1;
        self.executor.execute_message(msg, ApplyKind::Explicit, msg_length)
    }

    pub fn execute_implicit(&mut self, msg: Message) -> anyhow::Result<ApplyRet> {
        let msg_length = 1;
        self.executor.execute_message(msg, ApplyKind::Implicit, msg_length)
    }
}
