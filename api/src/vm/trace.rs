use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_shared::{address::Address, econ::TokenAmount, error::ExitCode, MethodNum};

/// A trace of an actor method invocation.
#[derive(Clone, Debug)]
pub struct InvocationTrace {
    pub from: Address,
    pub to: Address,
    pub value: TokenAmount,
    pub method: MethodNum,
    pub params: Option<IpldBlock>,
    pub code: ExitCode,
    pub ret: Option<IpldBlock>,
    pub subinvocations: Vec<InvocationTrace>,
}
