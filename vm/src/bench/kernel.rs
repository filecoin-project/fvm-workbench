use cid::Cid;
use fvm::call_manager::CallManager;
use fvm::gas::{Gas, GasTimer, PriceList};
use fvm::kernel::{
    ActorOps, BlockId, BlockRegistry, BlockStat, CircSupplyOps, CryptoOps, DebugOps, EventOps,
    ExecutionError, GasOps, IpldBlockOps, LimiterOps, MessageOps, NetworkOps, RandomnessOps,
    SelfOps, SendResult,
};
use fvm::machine::limiter::MemoryLimiter;

use fvm::{DefaultKernel, Kernel};
use fvm_shared::address::{Address, SECP_PUB_LEN};
use fvm_shared::clock::ChainEpoch;
use fvm_shared::consensus::ConsensusFault;
use fvm_shared::crypto::signature::{SignatureType, SECP_SIG_LEN, SECP_SIG_MESSAGE_HASH_SIZE};
use fvm_shared::econ::TokenAmount;

use fvm_shared::piece::PieceInfo;
use fvm_shared::randomness::RANDOMNESS_LENGTH;
use fvm_shared::sector::{
    AggregateSealVerifyProofAndInfos, RegisteredSealProof, ReplicaUpdateInfo, SealVerifyInfo,
    WindowPoStVerifyInfo,
};
use fvm_shared::sys::out::network::NetworkContext;
use fvm_shared::sys::out::vm::MessageContext;
use fvm_shared::sys::SendFlags;
use fvm_shared::{ActorID, MethodNum, TOTAL_FILECOIN};

use multihash::MultihashGeneric;

pub const TEST_VM_RAND_ARRAY: [u8; 32] = [
    1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32,
];

// TODO: extend test configuration to allow dynamic toggling for certain features to be intercepted
// and the behaviour once intercepted
// https://github.com/anorth/fvm-workbench/issues/9
#[derive(Debug, Clone)]
pub struct TestConfiguration {
    circ_supply: TokenAmount,
    price_list: PriceList,
}

pub type Result<T> = std::result::Result<T, ExecutionError>;

pub struct BenchKernel<C: CallManager> {
    inner_kernel: DefaultKernel<C>,
    test_config: TestConfiguration,
}

impl<C> Kernel for BenchKernel<C>
where
    C: CallManager,
{
    type CallManager = C;

    fn into_inner(self) -> (Self::CallManager, BlockRegistry)
    where
        Self: Sized,
    {
        self.inner_kernel.into_inner()
    }

    fn new(
        mgr: Self::CallManager,
        blocks: BlockRegistry,
        caller: ActorID,
        actor_id: ActorID,
        method: MethodNum,
        value_received: TokenAmount,
        read_only: bool,
    ) -> Self
    where
        Self: Sized,
    {
        let default_kernel =
            DefaultKernel::new(mgr, blocks, caller, actor_id, method, value_received, read_only);
        let price_list = default_kernel.price_list().clone();
        BenchKernel {
            inner_kernel: default_kernel,
            test_config: TestConfiguration { circ_supply: TOTAL_FILECOIN.clone(), price_list },
        }
    }

    fn machine(&self) -> &<Self::CallManager as fvm::call_manager::CallManager>::Machine {
        self.inner_kernel.machine()
    }

    fn send<K: Kernel<CallManager = Self::CallManager>>(
        &mut self,
        recipient: &Address,
        method: u64,
        params: BlockId,
        value: &TokenAmount,
        gas_limit: Option<Gas>,
        flags: SendFlags,
    ) -> fvm::kernel::Result<SendResult> {
        self.inner_kernel.send::<BenchKernel<C>>(recipient, method, params, value, gas_limit, flags)
    }
}

impl<C> ActorOps for BenchKernel<C>
where
    C: CallManager,
{
    fn resolve_address(&self, address: &Address) -> Result<ActorID> {
        self.inner_kernel.resolve_address(address)
    }

    fn get_actor_code_cid(&self, id: ActorID) -> Result<Cid> {
        self.inner_kernel.get_actor_code_cid(id)
    }

    fn next_actor_address(&self) -> Result<Address> {
        self.inner_kernel.next_actor_address()
    }

    fn create_actor(
        &mut self,
        code_id: Cid,
        actor_id: ActorID,
        delegated_address: Option<Address>,
    ) -> Result<()> {
        self.inner_kernel.create_actor(code_id, actor_id, delegated_address)
    }

    fn get_builtin_actor_type(&self, code_cid: &Cid) -> Result<u32> {
        self.inner_kernel.get_builtin_actor_type(code_cid)
    }

    fn get_code_cid_for_type(&self, typ: u32) -> Result<Cid> {
        self.inner_kernel.get_code_cid_for_type(typ)
    }

    fn balance_of(&self, actor_id: ActorID) -> Result<TokenAmount> {
        self.inner_kernel.balance_of(actor_id)
    }

    fn lookup_delegated_address(&self, actor_id: ActorID) -> Result<Option<Address>> {
        self.inner_kernel.lookup_delegated_address(actor_id)
    }
}

impl<C> IpldBlockOps for BenchKernel<C>
where
    C: CallManager,
{
    fn block_open(&mut self, cid: &Cid) -> std::result::Result<(u32, BlockStat), ExecutionError> {
        self.inner_kernel.block_open(cid)
    }

    fn block_create(&mut self, codec: u64, data: &[u8]) -> Result<BlockId> {
        self.inner_kernel.block_create(codec, data)
    }

    fn block_link(&mut self, id: BlockId, hash_fun: u64, hash_len: u32) -> Result<Cid> {
        self.inner_kernel.block_link(id, hash_fun, hash_len)
    }

    fn block_read(&self, id: BlockId, offset: u32, buf: &mut [u8]) -> Result<i32> {
        self.inner_kernel.block_read(id, offset, buf)
    }

    fn block_stat(&self, id: BlockId) -> Result<BlockStat> {
        self.inner_kernel.block_stat(id)
    }
}

impl<C> CircSupplyOps for BenchKernel<C>
where
    C: CallManager,
{
    // Not forwarded. Circulating supply is taken from the TestData.
    fn total_fil_circ_supply(&self) -> Result<TokenAmount> {
        Ok(self.test_config.circ_supply.clone())
    }
}

impl<C> CryptoOps for BenchKernel<C>
where
    C: CallManager,
{
    // forwarded
    fn hash(&self, code: u64, data: &[u8]) -> Result<MultihashGeneric<64>> {
        self.inner_kernel.hash(code, data)
    }

    // forwarded
    fn compute_unsealed_sector_cid(
        &self,
        proof_type: RegisteredSealProof,
        pieces: &[PieceInfo],
    ) -> Result<Cid> {
        self.inner_kernel.compute_unsealed_sector_cid(proof_type, pieces)
    }

    // forwarded
    fn verify_signature(
        &self,
        sig_type: SignatureType,
        signature: &[u8],
        signer: &Address,
        plaintext: &[u8],
    ) -> Result<bool> {
        self.inner_kernel.verify_signature(sig_type, signature, signer, plaintext)
    }

    // forwarded
    fn recover_secp_public_key(
        &self,
        hash: &[u8; SECP_SIG_MESSAGE_HASH_SIZE],
        signature: &[u8; SECP_SIG_LEN],
    ) -> Result<[u8; SECP_PUB_LEN]> {
        self.inner_kernel.recover_secp_public_key(hash, signature)
    }

    // NOT forwarded
    fn batch_verify_seals(&self, vis: &[SealVerifyInfo]) -> Result<Vec<bool>> {
        for vi in vis {
            let charge = self.test_config.price_list.on_verify_seal(vi);
            let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        }
        Ok(vec![true; vis.len()])
    }

    // NOT forwarded
    fn verify_seal(&self, vi: &SealVerifyInfo) -> Result<bool> {
        let charge = self.test_config.price_list.on_verify_seal(vi);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }

    // NOT forwarded
    fn verify_post(&self, vi: &WindowPoStVerifyInfo) -> Result<bool> {
        let charge = self.test_config.price_list.on_verify_post(vi);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }

    // NOT forwarded
    fn verify_consensus_fault(
        &self,
        h1: &[u8],
        h2: &[u8],
        extra: &[u8],
    ) -> Result<Option<ConsensusFault>> {
        let charge =
            self.test_config.price_list.on_verify_consensus_fault(h1.len(), h2.len(), extra.len());
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(None)
    }

    // NOT forwarded
    fn verify_aggregate_seals(&self, agg: &AggregateSealVerifyProofAndInfos) -> Result<bool> {
        let charge = self.test_config.price_list.on_verify_aggregate_seals(agg);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }

    // NOT forwarded
    fn verify_replica_update(&self, rep: &ReplicaUpdateInfo) -> Result<bool> {
        let charge = self.test_config.price_list.on_verify_replica_update(rep);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }
}

impl<C> DebugOps for BenchKernel<C>
where
    C: CallManager,
{
    fn log(&self, msg: String) {
        self.inner_kernel.log(msg)
    }

    fn debug_enabled(&self) -> bool {
        self.inner_kernel.debug_enabled()
    }

    fn store_artifact(&self, name: &str, data: &[u8]) -> Result<()> {
        self.inner_kernel.store_artifact(name, data)
    }
}

impl<C> GasOps for BenchKernel<C>
where
    C: CallManager,
{
    fn gas_used(&self) -> Gas {
        self.inner_kernel.gas_used()
    }

    fn charge_gas(&self, name: &str, compute: Gas) -> Result<GasTimer> {
        self.inner_kernel.charge_gas(name, compute)
    }

    fn price_list(&self) -> &PriceList {
        self.inner_kernel.price_list()
    }

    fn gas_available(&self) -> Gas {
        self.inner_kernel.gas_available()
    }
}

impl<C> MessageOps for BenchKernel<C>
where
    C: CallManager,
{
    fn msg_context(&self) -> Result<MessageContext> {
        self.inner_kernel.msg_context()
    }
}

impl<C> NetworkOps for BenchKernel<C>
where
    C: CallManager,
{
    fn network_context(&self) -> Result<NetworkContext> {
        self.inner_kernel.network_context()
    }

    fn tipset_cid(&self, epoch: ChainEpoch) -> Result<Cid> {
        self.inner_kernel.tipset_cid(epoch)
    }
}

impl<C> RandomnessOps for BenchKernel<C>
where
    C: CallManager,
{
    // NOT forwarded
    fn get_randomness_from_tickets(
        &self,
        _personalization: i64,
        _rand_epoch: ChainEpoch,
        entropy: &[u8],
    ) -> Result<[u8; RANDOMNESS_LENGTH]> {
        let charge = self.test_config.price_list.on_get_randomness(entropy.len());
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(TEST_VM_RAND_ARRAY)
    }

    fn get_randomness_from_beacon(
        &self,
        personalization: i64,
        rand_epoch: ChainEpoch,
        entropy: &[u8],
    ) -> Result<[u8; RANDOMNESS_LENGTH]> {
        self.inner_kernel.get_randomness_from_beacon(personalization, rand_epoch, entropy)
    }
}

impl<C> SelfOps for BenchKernel<C>
where
    C: CallManager,
{
    fn root(&self) -> Result<Cid> {
        self.inner_kernel.root()
    }

    fn set_root(&mut self, root: Cid) -> Result<()> {
        self.inner_kernel.set_root(root)
    }

    fn current_balance(&self) -> Result<TokenAmount> {
        self.inner_kernel.current_balance()
    }

    fn self_destruct(&mut self, beneficiary: &Address) -> Result<()> {
        self.inner_kernel.self_destruct(beneficiary)
    }
}

impl<C> EventOps for BenchKernel<C>
where
    C: CallManager,
{
    fn emit_event(&mut self, raw_evt: &[u8]) -> Result<()> {
        self.inner_kernel.emit_event(raw_evt)
    }
}

impl<C> LimiterOps for BenchKernel<C>
where
    C: CallManager,
{
    type Limiter = <DefaultKernel<C> as LimiterOps>::Limiter;

    fn limiter_mut(&mut self) -> &mut Self::Limiter {
        self.inner_kernel.limiter_mut()
    }
}

#[derive(Default)]
pub struct DummyLimiter {
    curr_exec_memory_bytes: usize,
}

impl MemoryLimiter for DummyLimiter {
    fn with_stack_frame<T, G, F, R>(t: &mut T, g: G, f: F) -> R
    where
        G: Fn(&mut T) -> &mut Self,
        F: FnOnce(&mut T) -> R,
    {
        let memory_bytes = g(t).curr_exec_memory_bytes;
        let ret = f(t);
        g(t).curr_exec_memory_bytes = memory_bytes;
        ret
    }

    fn memory_used(&self) -> usize {
        self.curr_exec_memory_bytes
    }

    fn grow_memory(&mut self, bytes: usize) -> bool {
        self.curr_exec_memory_bytes += bytes;
        true
    }
}
