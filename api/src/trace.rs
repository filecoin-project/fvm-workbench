use itertools::Itertools;
use std::borrow::Cow;
use std::fmt::Debug;

use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_shared::address::Address;
use fvm_shared::econ::TokenAmount;
use fvm_shared::error::{ErrorNumber, ExitCode};
use fvm_shared::{ActorID, MethodNum};
use vm_api::trace::InvocationTrace;

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
    CallError {
        reason: String,
        errno: ErrorNumber,
    },
    Log {
        msg: String,
    },
}

impl From<ExecutionTrace> for InvocationTrace {
    fn from(e_trace: ExecutionTrace) -> InvocationTrace {
        InvocationTrace::from(&e_trace)
    }
}

impl From<&ExecutionTrace> for InvocationTrace {
    fn from(e_trace: &ExecutionTrace) -> InvocationTrace {
        let mut invocation_stack: Vec<InvocationTrace> = Vec::new();
        let mut root_invocation: Option<InvocationTrace> = None;

        for event in e_trace.events.iter() {
            match event {
                ExecutionEvent::GasCharge { .. } | ExecutionEvent::Log { .. } => {}
                ExecutionEvent::Call { from, to, method, params, value } => {
                    invocation_stack.push(InvocationTrace {
                        from: Address::new_id(*from),
                        to: *to,
                        method: *method,
                        params: params.clone(),
                        value: value.clone(),
                        code: ExitCode::OK, // Placeholder, will be updated during call return
                        ret: None,          // Placeholder, will be updated during return
                        subinvocations: Vec::new(), // Placeholder, will be updated if subinvocations are made
                    });
                }
                ExecutionEvent::CallReturn { return_value, exit_code } => {
                    let mut current_invocation = invocation_stack
                        .pop()
                        .unwrap_or_else(|| panic!("Unmatched CallReturn: {:?}", e_trace));

                    current_invocation.code = *exit_code;
                    current_invocation.ret = return_value.clone();

                    if let Some(parent_invocation) = invocation_stack.last_mut() {
                        parent_invocation.subinvocations.push(current_invocation);
                    } else {
                        // At root invocation, assign to the root call
                        if root_invocation.is_some() {
                            panic!("Attempting to assign multiple root invocations: {:?}", e_trace);
                        }
                        root_invocation = Some(current_invocation);
                    }
                }
                ExecutionEvent::CallError { .. } => {
                    let mut current_invocation = invocation_stack
                        .pop()
                        .unwrap_or_else(|| panic!("Unmatched CallError: {:?}", e_trace));

                    // TODO(alexytsu): have invocation trace store ErrorNumber | ExitCode
                    // blocked by: https://github.com/filecoin-project/builtin-actors/issues/1365
                    current_invocation.code = ExitCode::SYS_ASSERTION_FAILED;

                    if let Some(parent_invocation) = invocation_stack.last_mut() {
                        parent_invocation.subinvocations.push(current_invocation);
                    } else {
                        // At root invocation, assign to the root call
                        if root_invocation.is_some() {
                            panic!("Attempting to assign multiple root invocations: {:?}", e_trace);
                        }
                        root_invocation = Some(current_invocation);
                    }
                }
            }
        }

        if !invocation_stack.is_empty() {
            panic!("Malformed ExecutionTrace, leftover invocations in stack: {:?}", e_trace);
        }

        root_invocation.expect("Malformed ExecutionTrace, missing root invocation")
    }
}
