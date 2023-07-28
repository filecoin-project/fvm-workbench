use std::borrow::Cow;
use std::fmt::Debug;

use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_shared::address::Address;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::{ErrorNumber, ExitCode};
use fvm_shared::{ActorID, MethodNum};
use itertools::Itertools;

/// A trace of a single message execution comprising a series of events.
/// An execution trace is easily produced by any abstract VM and can be used for low-level analysis
/// of gas costs and other call-events.
#[derive(Clone, Debug)]
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
    GasCharge {
        name: Cow<'static, str>,
        compute_milli: u64,
        other_milli: u64,
    },
    Call {
        from: ActorID,
        to: Address,
        method: MethodNum,
        params: Option<IpldBlock>,
        value: TokenAmount,
    },
    CallReturn {
        return_value: Option<IpldBlock>,
        exit_code: ExitCode,
    },
    CallAbort {
        exit_code: ExitCode,
    },
    CallError {
        reason: String,
        errno: ErrorNumber,
    },
    Log {
        msg: String,
    },
}
