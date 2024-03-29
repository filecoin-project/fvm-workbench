use cid::Cid;
use fvm::call_manager::CallManager;
use fvm::gas::{Gas, GasTimer, PriceList};
use fvm::kernel::{
    ActorOps, BlockId, BlockRegistry, BlockStat, CircSupplyOps, CryptoOps, DebugOps, EventOps,
    ExecutionError, GasOps, IpldBlockOps, LimiterOps, MessageOps, NetworkOps, RandomnessOps,
    SelfOps, SendResult,
};

use fvm::{DefaultKernel, Kernel};
use fvm_shared::address::{Address, SECP_PUB_LEN};
use fvm_shared::clock::ChainEpoch;
use fvm_shared::commcid::FIL_COMMITMENT_UNSEALED;
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
use fvm_shared::{ActorID, MethodNum};

use multihash::derive::Multihash;
use multihash::{MultihashDigest, MultihashGeneric};

pub const TEST_VM_RAND_ARRAY: [u8; 32] = [
    1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32,
];

// TODO: extend test configuration to allow dynamic toggling for certain features to be intercepted
// and the behaviour once intercepted
// https://github.com/anorth/fvm-workbench/issues/9
#[derive(Debug, Clone)]
pub struct KernelConfiguration {
    unsealed_sector_cid: Option<Cid>,
}

pub type ExecutionResult<T> = std::result::Result<T, ExecutionError>;

/// A BenchKernel wraps a default kernel, delegating most functionality to it but intercepting certain
/// methods to return static data.
pub struct BenchKernel<C: CallManager> {
    inner_kernel: DefaultKernel<C>,
    kernel_overrides: KernelConfiguration,
}

pub fn make_cid(input: &[u8], prefix: u64, hash: MhCode) -> Cid {
    let hash = hash.digest(input);
    Cid::new_v1(prefix, hash)
}

pub fn make_cid_sha(input: &[u8], prefix: u64) -> Cid {
    make_cid(input, prefix, MhCode::Sha256TruncPaddedFake)
}

pub fn make_piece_cid(input: &[u8]) -> Cid {
    make_cid_sha(input, FIL_COMMITMENT_UNSEALED)
}

// multihash library doesn't support poseidon hashing, so we fake it
#[derive(Clone, Copy, Debug, PartialEq, Eq, Multihash)]
#[mh(alloc_size = 64)]
pub enum MhCode {
    #[mh(code = 0xb401, hasher = multihash::Sha2_256)]
    PoseidonFake,
    #[mh(code = 0x1012, hasher = multihash::Sha2_256)]
    Sha256TruncPaddedFake,
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
        BenchKernel {
            inner_kernel: DefaultKernel::new(
                mgr,
                blocks,
                caller,
                actor_id,
                method,
                value_received,
                read_only,
            ),
            kernel_overrides: KernelConfiguration {
                // We don't actually control when a new Kernel is constructed so we can't pass this value in dynamically
                // TODO: establish a pattern for intercepting Kernel behaviour in a more dynamic way
                // https://github.com/anorth/fvm-workbench/issues/9
                unsealed_sector_cid: Some(make_piece_cid(b"test data")),
            },
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
        self.inner_kernel.send::<K>(recipient, method, params, value, gas_limit, flags)
    }
}

/// All ActorOps are forwarded to the DefaultKernel
impl<C> ActorOps for BenchKernel<C>
where
    C: CallManager,
{
    fn resolve_address(&self, address: &Address) -> ExecutionResult<ActorID> {
        self.inner_kernel.resolve_address(address)
    }

    fn get_actor_code_cid(&self, id: ActorID) -> ExecutionResult<Cid> {
        self.inner_kernel.get_actor_code_cid(id)
    }

    fn next_actor_address(&self) -> ExecutionResult<Address> {
        self.inner_kernel.next_actor_address()
    }

    fn create_actor(
        &mut self,
        code_id: Cid,
        actor_id: ActorID,
        delegated_address: Option<Address>,
    ) -> ExecutionResult<()> {
        self.inner_kernel.create_actor(code_id, actor_id, delegated_address)
    }

    fn get_builtin_actor_type(&self, code_cid: &Cid) -> ExecutionResult<u32> {
        self.inner_kernel.get_builtin_actor_type(code_cid)
    }

    fn get_code_cid_for_type(&self, typ: u32) -> ExecutionResult<Cid> {
        self.inner_kernel.get_code_cid_for_type(typ)
    }

    fn balance_of(&self, actor_id: ActorID) -> ExecutionResult<TokenAmount> {
        self.inner_kernel.balance_of(actor_id)
    }

    fn lookup_delegated_address(&self, actor_id: ActorID) -> ExecutionResult<Option<Address>> {
        self.inner_kernel.lookup_delegated_address(actor_id)
    }
}

/// All IpldBlockOps are forwarded to the DefaultKernel
impl<C> IpldBlockOps for BenchKernel<C>
where
    C: CallManager,
{
    fn block_open(&mut self, cid: &Cid) -> std::result::Result<(u32, BlockStat), ExecutionError> {
        self.inner_kernel.block_open(cid)
    }

    fn block_create(&mut self, codec: u64, data: &[u8]) -> ExecutionResult<BlockId> {
        self.inner_kernel.block_create(codec, data)
    }

    fn block_link(&mut self, id: BlockId, hash_fun: u64, hash_len: u32) -> ExecutionResult<Cid> {
        self.inner_kernel.block_link(id, hash_fun, hash_len)
    }

    fn block_read(&self, id: BlockId, offset: u32, buf: &mut [u8]) -> ExecutionResult<i32> {
        self.inner_kernel.block_read(id, offset, buf)
    }

    fn block_stat(&self, id: BlockId) -> ExecutionResult<BlockStat> {
        self.inner_kernel.block_stat(id)
    }
}

/// All CircSupplyOps are forwarded to the DefaultKernel
impl<C> CircSupplyOps for BenchKernel<C>
where
    C: CallManager,
{
    fn total_fil_circ_supply(&self) -> ExecutionResult<TokenAmount> {
        self.inner_kernel.total_fil_circ_supply()
    }
}

/// Some CryptoOps are faked so that proofs do not need to be calculated in tests
impl<C> CryptoOps for BenchKernel<C>
where
    C: CallManager,
{
    // forwarded
    fn hash(&self, code: u64, data: &[u8]) -> ExecutionResult<MultihashGeneric<64>> {
        self.inner_kernel.hash(code, data)
    }

    // NOT forwarded - returns a static cid
    fn compute_unsealed_sector_cid(
        &self,
        proof_type: RegisteredSealProof,
        pieces: &[PieceInfo],
    ) -> ExecutionResult<Cid> {
        let charge =
            self.inner_kernel.price_list().on_compute_unsealed_sector_cid(proof_type, pieces);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        self.kernel_overrides
            .unsealed_sector_cid
            .ok_or(ExecutionError::Fatal(anyhow::format_err!("unsealed sector cid not set")))
    }

    // NOT forwarded - treats signatures that match plaintext as valid
    fn verify_signature(
        &self,
        sig_type: SignatureType,
        signature: &[u8],
        _signer: &Address,
        plaintext: &[u8],
    ) -> ExecutionResult<bool> {
        let charge = self.inner_kernel.price_list().on_verify_signature(sig_type, signature.len());
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        if signature != plaintext {
            return Err(ExecutionError::Fatal(anyhow::format_err!(
                "invalid signature (mock sig validation expects siggy bytes to be equal to plaintext)"
            )));
        }
        Ok(true)
    }

    // forwarded
    fn recover_secp_public_key(
        &self,
        hash: &[u8; SECP_SIG_MESSAGE_HASH_SIZE],
        signature: &[u8; SECP_SIG_LEN],
    ) -> ExecutionResult<[u8; SECP_PUB_LEN]> {
        self.inner_kernel.recover_secp_public_key(hash, signature)
    }

    // NOT forwarded - verifications always succeed
    fn batch_verify_seals(&self, vis: &[SealVerifyInfo]) -> ExecutionResult<Vec<bool>> {
        for vi in vis {
            let charge = self.inner_kernel.price_list().on_verify_seal(vi);
            let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        }
        Ok(vec![true; vis.len()])
    }

    // NOT forwarded - verification always succeeds
    fn verify_post(&self, vi: &WindowPoStVerifyInfo) -> ExecutionResult<bool> {
        let charge = self.inner_kernel.price_list().on_verify_post(vi);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }

    // NOT forwarded - no fault detected
    fn verify_consensus_fault(
        &self,
        h1: &[u8],
        h2: &[u8],
        extra: &[u8],
    ) -> ExecutionResult<Option<ConsensusFault>> {
        let charge = self.inner_kernel.price_list().on_verify_consensus_fault(
            h1.len(),
            h2.len(),
            extra.len(),
        );
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(None)
    }

    // NOT forwarded - all proofs are valid
    fn verify_aggregate_seals(
        &self,
        agg: &AggregateSealVerifyProofAndInfos,
    ) -> ExecutionResult<bool> {
        let charge = self.inner_kernel.price_list().on_verify_aggregate_seals(agg);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }

    // NOT forwarded - all replicas are verified
    fn verify_replica_update(&self, rep: &ReplicaUpdateInfo) -> ExecutionResult<bool> {
        let charge = self.inner_kernel.price_list().on_verify_replica_update(rep);
        let _ = self.inner_kernel.charge_gas(&charge.name, charge.total())?;
        Ok(true)
    }
}

/// All DebugOps are forwarded to the DefaultKernel
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

    fn store_artifact(&self, name: &str, data: &[u8]) -> ExecutionResult<()> {
        self.inner_kernel.store_artifact(name, data)
    }
}

/// All GasOps are forwarded to the DefaultKernel
impl<C> GasOps for BenchKernel<C>
where
    C: CallManager,
{
    fn gas_used(&self) -> Gas {
        self.inner_kernel.gas_used()
    }

    fn charge_gas(&self, name: &str, compute: Gas) -> ExecutionResult<GasTimer> {
        self.inner_kernel.charge_gas(name, compute)
    }

    fn price_list(&self) -> &PriceList {
        self.inner_kernel.price_list()
    }

    fn gas_available(&self) -> Gas {
        self.inner_kernel.gas_available()
    }
}

/// All MessageOps are forwarded to the DefaultKernel
impl<C> MessageOps for BenchKernel<C>
where
    C: CallManager,
{
    fn msg_context(&self) -> ExecutionResult<MessageContext> {
        self.inner_kernel.msg_context()
    }
}

/// All NetworkOps are forwarded to the DefaultKernel
impl<C> NetworkOps for BenchKernel<C>
where
    C: CallManager,
{
    fn network_context(&self) -> ExecutionResult<NetworkContext> {
        self.inner_kernel.network_context()
    }

    fn tipset_cid(&self, epoch: ChainEpoch) -> ExecutionResult<Cid> {
        self.inner_kernel.tipset_cid(epoch)
    }
}

impl<C> RandomnessOps for BenchKernel<C>
where
    C: CallManager,
{
    // NOT forwarded
    // TODO: perhaps should be implemented via faking externs https://github.com/anorth/fvm-workbench/issues/10
    fn get_randomness_from_tickets(
        &self,
        rand_epoch: ChainEpoch,
    ) -> ExecutionResult<[u8; RANDOMNESS_LENGTH]> {
        self.inner_kernel.get_randomness_from_tickets(rand_epoch)?;
        Ok(TEST_VM_RAND_ARRAY)
    }

    fn get_randomness_from_beacon(
        &self,
        rand_epoch: ChainEpoch,
    ) -> ExecutionResult<[u8; RANDOMNESS_LENGTH]> {
        self.inner_kernel.get_randomness_from_beacon(rand_epoch)
    }
}

/// All SelfOps are forwarded to the DefaultKernel
impl<C> SelfOps for BenchKernel<C>
where
    C: CallManager,
{
    fn root(&mut self) -> ExecutionResult<Cid> {
        self.inner_kernel.root()
    }

    fn set_root(&mut self, root: Cid) -> ExecutionResult<()> {
        self.inner_kernel.set_root(root)
    }

    fn current_balance(&self) -> ExecutionResult<TokenAmount> {
        self.inner_kernel.current_balance()
    }

    fn self_destruct(&mut self, burn_unspent: bool) -> ExecutionResult<()> {
        self.inner_kernel.self_destruct(burn_unspent)
    }
}

/// All EventOps are forwarded to the DefaultKernel
impl<C> EventOps for BenchKernel<C>
where
    C: CallManager,
{
    fn emit_event(
        &mut self,
        event_headers: &[fvm_shared::sys::EventEntry],
        raw_key: &[u8],
        raw_val: &[u8],
    ) -> fvm::kernel::Result<()> {
        self.inner_kernel.emit_event(event_headers, raw_key, raw_val)
    }
}

/// All LimiterOps are forwarded to the DefaultKernel
impl<C> LimiterOps for BenchKernel<C>
where
    C: CallManager,
{
    type Limiter = <DefaultKernel<C> as LimiterOps>::Limiter;

    fn limiter_mut(&mut self) -> &mut Self::Limiter {
        self.inner_kernel.limiter_mut()
    }
}
