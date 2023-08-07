use fvm_shared::econ::TokenAmount;
use fvm_shared::receipt::Receipt;
use vm_api::{ActorState, MessageResult};

pub mod analysis;
pub mod bench;
pub mod blockstore;
pub mod trace;
pub mod wrangler;

use trace::ExecutionTrace;

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
