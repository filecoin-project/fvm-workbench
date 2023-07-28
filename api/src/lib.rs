use cid::Cid;
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::ser::Serialize;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::receipt::Receipt;
use fvm_shared::ActorID;
use vm_api::{ActorState, MessageResult};

pub mod analysis;
pub mod bench;
pub mod blockstore;
pub mod trace;
pub mod wrangler;

use trace::ExecutionTrace;

/// A factory for workbench instances.
/// Built-in actors must be installed before the workbench can be created.
// TODO: Configuration of default circulating supply, base fee etc.
pub trait WorkbenchBuilder {
    type B: Blockstore;

    /// Returns a reference to the blockstore underlying this builder.
    fn store(&self) -> &Self::B;

    /// Creates a singleton built-in actor using code specified in the manifest.
    /// A singleton actor does not have a robust/key address resolved via the Init actor.
    fn create_singleton_actor(
        &mut self,
        type_id: u32,
        id: ActorID,
        state: &impl Serialize,
        balance: TokenAmount,
    ) -> anyhow::Result<()>;

    /// Creates a non-singleton built-in actor using code specified in the manifest.
    /// Returns the assigned ActorID.
    fn create_builtin_actor(
        &mut self,
        type_id: u32,
        address: &Address,
        state: &impl Serialize,
        balance: TokenAmount,
    ) -> anyhow::Result<ActorID>;

    /// Creates a workbench ready to execute messages.
    /// The System and Init actors must be created before a workbench can be built or used.
    fn build(&mut self) -> anyhow::Result<Box<dyn Bench>>;
}

/// A VM workbench that can execute messages to actors.
pub trait Bench {
    /// Executes a message on the workbench VM.
    /// Explicit messages increment the sender's nonce and charge for gas consumed.
    fn execute(&mut self, msg: Message, msg_length: usize) -> anyhow::Result<ExecutionResult>;
    /// Implicit messages ignore the nonce and charge no gas (but still account for it).
    fn execute_implicit(
        &mut self,
        msg: Message,
        msg_length: usize,
    ) -> anyhow::Result<ExecutionResult>;

    /// Returns the VM's current epoch.
    fn epoch(&self) -> ChainEpoch;
    /// Replaces the VM in the workbench with a new set to the specified epoch
    fn set_epoch(&mut self, epoch: ChainEpoch);
    /// Returns a reference to the VM's blockstore.
    fn store(&self) -> &dyn Blockstore;
    /// Looks up a top-level actor state object in the VM.
    /// Returns None if no such actor is found.
    fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<ActorState>>;
    /// Resolves an address to an actor ID.
    /// Returns None if the address cannot be resolved.
    fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>>;
    /// Flush underlying storage
    fn flush(&mut self) -> Cid;
}

/// The result of a message execution.
/// This duplicates a lot from an FVM-internal type, but is independent of VM.
#[derive(Clone, Debug)]
pub struct ExecutionResult {
    /// Message receipt for the transaction.
    pub receipt: Receipt,
    /// Gas penalty from transaction, if any.
    pub penalty: TokenAmount,
    /// Tip given to miner from message.
    pub miner_tip: TokenAmount,

    // Gas tracing
    pub gas_burned: u64,
    pub base_fee_burn: TokenAmount,
    pub over_estimation_burn: TokenAmount,

    /// Execution trace information, for debugging.
    pub trace: ExecutionTrace,
    pub message: String,
}

impl From<ExecutionResult> for MessageResult {
    fn from(execution_res: ExecutionResult) -> MessageResult {
        MessageResult {
            code: execution_res.receipt.exit_code,
            ret: execution_res.receipt.return_data.into(),
            message: execution_res.message,
        }
    }
}
