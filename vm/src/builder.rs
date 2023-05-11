use anyhow::{anyhow, Context};
use cid::Cid;
use futures::executor::block_on;
use fvm::call_manager::DefaultCallManager;
use fvm::engine::EnginePool;
use fvm::executor::DefaultExecutor;
use fvm::machine::{DefaultMachine, MachineContext, Manifest, NetworkConfig};
use fvm::state_tree::{ActorState, StateTree};
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_car::load_car_unchecked;
use fvm_ipld_encoding::ser::Serialize;
use fvm_ipld_encoding::CborStore;
use fvm_shared::address::Address;
use fvm_shared::econ::TokenAmount;
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_shared::ActorID;
use fvm_workbench_api::{Bench, WorkbenchBuilder};
use multihash::Code;

use crate::bench::{BenchKernel, FvmBench};
use crate::externs::FakeExterns;

/// A factory for workbench instances backed by a real FVM.
/// Code for built-in actors may be loaded from either a bundle or a manifest, before
/// actors can then be constructed.
pub struct FvmBenchBuilder<B>
where
    B: Blockstore + Clone + 'static,
{
    externs: FakeExterns,
    machine_ctx: MachineContext,
    state_tree: StateTree<B>,
    builtin_manifest_data_cid: Option<Cid>,
    builtin_manifest: Option<Manifest>,
}

impl<B> FvmBenchBuilder<B>
where
    B: Blockstore + Clone,
{
    /// Create a new BenchBuilder and loads built-in actor code from a bundle.
    /// Returns the builder and manifest data CID.
    pub fn new_with_bundle(
        blockstore: B,
        externs: FakeExterns,
        nv: NetworkVersion,
        state_tree_version: StateTreeVersion,
        builtin_bundle: &[u8],
    ) -> anyhow::Result<(Self, Cid)> {
        let mut bb = FvmBenchBuilder::new_bare(blockstore, externs, nv, state_tree_version)?;
        let manifest_data_cid = bb.install_builtin_actor_bundle(builtin_bundle)?;
        Ok((bb, manifest_data_cid))
    }

    /// Creates a new BenchBuilder with no installed code for built-in actors.
    pub fn new_bare(
        blockstore: B,
        externs: FakeExterns,
        nv: NetworkVersion,
        state_tree_version: StateTreeVersion,
    ) -> anyhow::Result<Self> {
        let mut network_conf = NetworkConfig::new(nv);
        // Enabling debugging is necessary to get log events in traces.
        network_conf.enable_actor_debugging();
        let machine_ctx = MachineContext {
            network: network_conf,
            epoch: 0,
            timestamp: 0,
            base_fee: TokenAmount::from_atto(100),
            initial_state_root: Default::default(),
            circ_supply: TokenAmount::from_whole(1_000_000),
            tracing: true,
        };
        let state_tree =
            StateTree::new(blockstore, state_tree_version).map_err(anyhow::Error::from)?;

        Ok(Self {
            externs,
            machine_ctx,
            state_tree,
            builtin_manifest_data_cid: None,
            builtin_manifest: None,
        })
    }

    /// Imports built-in actor code and manifest into the state tree from a bundle in CAR format.
    /// After this, built-in actors can be created from the code thus installed.
    /// Does not create any actors.
    /// Returns the manifest data CID.
    pub fn install_builtin_actor_bundle(&mut self, bundle_data: &[u8]) -> anyhow::Result<Cid> {
        if self.builtin_manifest.is_some() {
            return Err(anyhow!("built-in actors already installed"));
        }
        let store = self.state_tree.store();
        let bundle_root = import_bundle(store, bundle_data).unwrap();

        let (manifest_version, manifest_data_cid): (u32, Cid) = match store
            .get_cbor(&bundle_root)?
        {
            Some((manifest_version, manifest_data)) => (manifest_version, manifest_data),
            None => return Err(anyhow!("no manifest information in bundle root {}", bundle_root)),
        };
        self.builtin_manifest_data_cid = Some(manifest_data_cid);
        self.builtin_manifest = Some(Manifest::load(store, &manifest_data_cid, manifest_version)?);
        Ok(manifest_data_cid)
    }

    /// Installs built-in actors code from a manifest provided directly.
    pub fn install_builtin_manifest(&mut self, _manifest: &Manifest) -> anyhow::Result<()> {
        // Write manifest data to blockstore
        // Set local manifest data cid
        // Caller will also need to install the actor code for each actor in the manifest
        todo!()
    }

    ///// Private helpers /////

    fn create_builtin_actor_internal(
        &mut self,
        type_id: u32,
        id: ActorID,
        state: &impl Serialize,
        balance: TokenAmount,
    ) -> anyhow::Result<()> {
        if let Some(manifest) = self.builtin_manifest.as_ref() {
            let code_cid = manifest.code_by_id(type_id).unwrap();
            let code = *code_cid;
            let state_cid = self
                .state_tree
                .store()
                .put_cbor(state, Code::Blake2b256)
                .context("failed to put actor state while installing")?;

            let actor_state = ActorState {
                code,
                state: state_cid,
                sequence: 0,
                balance,
                delegated_address: None,
            };
            self.state_tree.set_actor(id, actor_state);
            Ok(())
        } else {
            Err(anyhow!("built-in actor manifest not loaded"))
        }
    }
}

impl<B> WorkbenchBuilder for FvmBenchBuilder<B>
where
    B: Blockstore + Clone,
{
    type B = B;

    fn store(&self) -> &B {
        self.state_tree.store()
    }

    /// Creates a singleton built-in actor using code specified in the manifest.
    /// A singleton actor does not have a robust/key address resolved via the Init actor.
    fn create_singleton_actor(
        &mut self,
        type_id: u32,
        id: ActorID,
        state: &impl Serialize,
        balance: TokenAmount,
    ) -> anyhow::Result<()> {
        self.create_builtin_actor_internal(type_id, id, state, balance)
    }

    /// Creates a non-singleton built-in actor using code specified in the manifest.
    /// Returns the assigned ActorID.
    fn create_builtin_actor(
        &mut self,
        type_id: u32,
        address: &Address,
        state: &impl Serialize,
        balance: TokenAmount,
    ) -> anyhow::Result<ActorID> {
        let new_id = self.state_tree.register_new_address(address)?;
        self.create_builtin_actor_internal(type_id, new_id, &state, balance)?;
        Ok(new_id)
    }

    /// Creates a workbench with the current state tree.
    /// The System and Init actors must be created before the workbench can be built or used.
    fn build(&mut self) -> anyhow::Result<Box<dyn Bench>> {
        // Clone the context so the builder can be re-used for a new bench.
        let mut machine_ctx = self.machine_ctx.clone();

        // Flush the state tree to store and calculate the initial root.
        let state_root = self.state_tree.flush().map_err(anyhow::Error::from)?;
        machine_ctx.initial_state_root = state_root;

        let engine_conf = (&machine_ctx.network).into();
        let machine = DefaultMachine::new(
            &machine_ctx,
            self.state_tree.store().clone(),
            self.externs.clone(),
        )?;
        let executor = DefaultExecutor::<
            BenchKernel<B, DefaultCallManager<DefaultMachine<B, FakeExterns>>>,
        >::new(EnginePool::new_default(engine_conf)?, machine)?;
        Ok(Box::new(FvmBench::new(executor)))
    }
}

fn import_bundle(blockstore: &impl Blockstore, bundle: &[u8]) -> anyhow::Result<Cid> {
    match &*block_on(async { load_car_unchecked(blockstore, bundle).await })? {
        [root] => Ok(*root),
        _ => Err(anyhow!("multiple root CIDs in bundle")),
    }
}
