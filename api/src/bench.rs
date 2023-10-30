use std::collections::BTreeMap;

use cid::Cid;
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::ser::Serialize;
use fvm_shared::address::Address;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::econ::TokenAmount;
use fvm_shared::message::Message;
use fvm_shared::ActorID;

use crate::{ActorState, ExecutionResult};

/// A factory for workbench instances.
/// Built-in actors must be installed before the workbench can be created.
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
    fn build(&mut self, circulating_supply: TokenAmount) -> anyhow::Result<Box<dyn Bench>>;
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

    /// Returns a reference to the VM's blockstore.
    fn store(&self) -> &dyn Blockstore;

    /// Looks up a top-level actor state object in the VM.
    /// Returns None if no such actor is found.
    fn find_actor(&self, id: ActorID) -> anyhow::Result<Option<ActorState>>;

    /// Replaces the state of the actor at the specified address
    fn set_actor(&mut self, key: &Address, state: ActorState);

    /// Resolves an address to an actor ID.
    /// Returns None if the address cannot be resolved.
    fn resolve_address(&self, addr: &Address) -> anyhow::Result<Option<ActorID>>;

    /// Flush the underlying executor. This is useful to force pending changes in the executor's
    /// BufferedBlockstore to be immediately written into the underlying Blockstore (which may be
    /// referenced elsewhere)
    fn flush(&mut self) -> Cid;

    /// Get a manifest of the builtin actors
    fn builtin_actors_manifest(&self) -> BTreeMap<Cid, vm_api::builtin::Type>;

    /// Get a map of all address -> actor mappings in the state tree
    fn actor_states(&self) -> BTreeMap<Address, ActorState>;

    /// Get the VM's current epoch
    fn epoch(&self) -> ChainEpoch;

    /// Set the VM's current epoch
    fn set_epoch(&mut self, epoch: ChainEpoch);

    /// Get the current circulating supply
    fn circulating_supply(&self) -> TokenAmount;

    /// Set the current circulating supply
    fn set_circulating_supply(&mut self, amount: TokenAmount);

    /// Get the current base fee
    fn base_fee(&self) -> TokenAmount;

    /// Set the current base fee
    fn set_base_fee(&mut self, amount: TokenAmount);

    /// Get the current timestamp
    fn timestamp(&self) -> u64;

    /// Set the current timestamp
    fn set_timestamp(&mut self, timestamp: u64);

    /// Get the initial state root of the block
    fn initial_state_root(&self) -> Cid;

    /// Set the initial state root of the block
    fn set_initial_state_root(&mut self, state_root: Cid);

    /// Toggle execution traces in the VM (default: true in the workbench)
    fn set_tracing(&mut self, tracing: bool);
}
