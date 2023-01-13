use fvm_ipld_encoding::RawBytes;
use fvm_shared::address::Address;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::{ErrorNumber, ExitCode};
use fvm_shared::{ActorID, MethodNum};
use itertools::Itertools;
use std::borrow::Cow;
use std::fmt::{Debug};

/// A trace of a single message execution.
/// A trace is a sequence of events.
pub struct ExecutionTrace {
    events: Vec<ExecutionEvent>,
}

impl ExecutionTrace {
    pub fn new(events: Vec<ExecutionEvent>) -> Self {
        Self { events }
    }

    pub fn events(&self) -> &[ExecutionEvent] {
        &self.events
    }

    pub fn format(&self) -> String {
        self.events.iter().map(|e| format!("{:?}", e)).join("\n")
    }
}

/// An event forming part of an execution trace.
/// This is closely modelled on the FVM's internal execution event type,
/// but usable without depending on the FVM directly.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ExecutionEvent {
    GasCharge { name: Cow<'static, str>, compute_milli: i64, storage_milli: i64 },
    Call { from: ActorID, to: Address, method: MethodNum, params: RawBytes, value: TokenAmount },
    CallReturn { return_value: RawBytes },
    CallAbort { exit_code: ExitCode },
    CallError { reason: String, errno: ErrorNumber },
    Log { msg: String },
}
