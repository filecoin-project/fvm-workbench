use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::ser::Serialize;
use fvm_shared::ActorID;
use fvm_shared::address::Address;
use fvm_shared::econ::TokenAmount;

pub trait WorkbenchBuilder
{
    type B: Blockstore;

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
}

