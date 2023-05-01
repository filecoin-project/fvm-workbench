use cid::Cid;
use fil_actor_market::{DealSpaces, SectorDealData};

use fil_actors_runtime::runtime::Policy;

use fvm_ipld_bitfield::BitField;
use fvm_ipld_blockstore::{Blockstore, MemoryBlockstore};
use fvm_ipld_encoding::ipld_block::IpldBlock;
use fvm_ipld_encoding::CBOR;
use fvm_shared::address::Address;
use fvm_shared::bigint::{BigInt, Zero};
use fvm_shared::clock::ChainEpoch;
use fvm_shared::commcid::FIL_COMMITMENT_SEALED;
use fvm_shared::deal::DealID;
use fvm_shared::sector::RegisteredSealProof;
use fvm_shared::ActorID;
use fvm_shared::{
    crypto::signature::SignatureType, econ::TokenAmount, state::StateTreeVersion,
    version::NetworkVersion,
};

use fvm_workbench_api::{wrangler::ExecutionWrangler, WorkbenchBuilder};
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisSpec};
use fvm_workbench_vm::{builder::FvmBenchBuilder, externs::FakeExterns};
use multihash::MultihashDigest;

use crate::util::*;
use crate::workflows::*;
mod util;

use fil_actor_miner::{
    Method, PreCommitSectorParams, SectorPreCommitOnChainInfo, State as MinerState,
};

// an expiration ~10 days greater than effective min expiration taking into account 30 days max
// between pre and prove commit
const DEFAULT_SECTOR_EXPIRATION: ChainEpoch = 220;
fn make_sealed_cid(input: &[u8]) -> Cid {
    // Note: multihash library doesn't support Poseidon hashing, so we fake it
    let h = MhCode::PoseidonFake.digest(input);
    Cid::new_v1(FIL_COMMITMENT_SEALED, h)
}

fn make_pre_commit_params(
    seal_proof_type: RegisteredSealProof,
    sector_no: u64,
    challenge: ChainEpoch,
    expiration: ChainEpoch,
    sector_deal_ids: Vec<DealID>,
) -> PreCommitSectorParams {
    PreCommitSectorParams {
        seal_proof: seal_proof_type,
        sector_number: sector_no,
        sealed_cid: make_sealed_cid(b"commr"),
        seal_rand_epoch: challenge,
        deal_ids: sector_deal_ids,
        expiration,
        // unused
        replace_capacity: false,
        replace_sector_deadline: 0,
        replace_sector_partition: 0,
        replace_sector_number: 0,
    }
}

#[derive(Default)]
pub struct PreCommitConfig(pub SectorDealData);

#[allow(dead_code)]
impl PreCommitConfig {
    pub fn empty() -> PreCommitConfig {
        Self::new(None)
    }

    pub fn new(commd: Option<Cid>) -> PreCommitConfig {
        PreCommitConfig { 0: SectorDealData { commd } }
    }

    pub fn default() -> PreCommitConfig {
        Self::empty()
    }
}

struct BlockstoreWrapper<'a>(&'a dyn Blockstore);

impl<'a> Blockstore for BlockstoreWrapper<'a> {
    fn get(&self, k: &Cid) -> anyhow::Result<Option<Vec<u8>>> {
        self.0.get(k)
    }

    fn put_keyed(&self, k: &Cid, block: &[u8]) -> anyhow::Result<()> {
        self.0.put_keyed(k, block)
    }
}

fn precommit_sector_and_get(
    w: &mut ExecutionWrangler,
    miner: ActorID,
    worker: ActorID,
    params: PreCommitSectorParams,
    conf: PreCommitConfig,
    first: bool,
) -> SectorPreCommitOnChainInfo {
    // TODO: expect this result to be empty?
    // TODO: miner_id might not be suitable as the worker id here..
    let _result = pre_commit_sector(w, miner, worker, params.clone(), conf, first);
    let state: MinerState = w.find_actor_state(miner).unwrap().unwrap();
    state
        .get_precommitted_sector(&BlockstoreWrapper(w.bench.borrow().store()), params.sector_number)
        .unwrap()
        .unwrap()
}

fn pre_commit_sector(
    w: &mut ExecutionWrangler,
    miner: ActorID,
    worker: ActorID,
    params: PreCommitSectorParams,
    conf: PreCommitConfig,
    first: bool,
) -> Result<Option<IpldBlock>, String> {
    // let state: MinerState = w.find_actor_state(miner).unwrap().unwrap();

    // if first {
    //     let dlinfo = new_deadline_info_from_offset_and_epoch(
    //         &Policy::default(), // TODO: maybe pass this in
    //         state.proving_period_start,
    //         w.epoch(),
    //     );
    //     let cron_params = make_deadline_cron_event_params(dlinfo.last());
    // }

    let result = apply_ok(
        w,
        Address::new_id(worker),
        Address::new_id(miner),
        TokenAmount::zero(),
        Method::PreCommitSector as u64,
        &params,
    )
    .unwrap();

    let ret = result.receipt.return_data;
    match ret.is_empty() {
        true => Ok(None),
        false => Ok(Some(IpldBlock { codec: CBOR, data: ret.to_vec() })),
    }
}

/* Mint a token for client and transfer it to a receiver. */
#[test]
fn valid_precommits_then_aggregate_provecommit() {
    // constants/parameters
    let period_offset = ChainEpoch::from(100);
    let policy = Policy::default();
    let proof_type = RegisteredSealProof::StackedDRG32GiBV1P1;

    // =========  Workbench setup
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        MemoryBlockstore::new(),
        FakeExterns::new(),
        NetworkVersion::V18,
        StateTreeVersion::V5,
        actors_v11::BUNDLE_CAR,
    )
    .unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let bench = builder.build().unwrap();
    let mut w = ExecutionWrangler::new_default(bench);

    // create an owner account
    let owner = create_accounts(
        &mut w,
        genesis.faucet_id,
        1,
        TokenAmount::from_whole(10_000),
        SignatureType::BLS,
    )
    .unwrap()[0]
        .clone();

    let precommit_epoch = period_offset + 1;
    w.set_epoch(precommit_epoch);

    // create a miner account
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;
    let (miner_id, miner) = create_miner(
        &mut w,
        // TODO: have different owner and workers?
        owner.id,
        owner.id,
        seal_proof.registered_window_post_proof().unwrap(),
        TokenAmount::from_whole(1_000),
    )
    .unwrap();

    // actor.deadline()
    let state: MinerState = w.find_actor_state(miner_id).unwrap().unwrap();
    let dl_info = state.recorded_deadline_info(&policy, w.epoch());
    println!("{:?}", state);

    // make a good commitment for the proof to target
    let prove_commit_epoch = precommit_epoch + policy.pre_commit_challenge_delay + 1;
    // something on deadline boundary but > 180 days
    let verified_deal_space = seal_proof.sector_size().unwrap() as u64; // TODO: check this
    let expiration = dl_info.period_end() + policy.wpost_proving_period * DEFAULT_SECTOR_EXPIRATION;
    // fill the sector with verified seals
    let duration = expiration - prove_commit_epoch;
    let deal_spaces = DealSpaces {
        deal_space: BigInt::zero(),
        verified_deal_space: BigInt::from(verified_deal_space),
    };

    let mut precommits = vec![];
    let mut sector_nos_bf = BitField::new();
    for i in 0..10u64 {
        sector_nos_bf.set(i);
        let precommit_params =
            make_pre_commit_params(proof_type, i, precommit_epoch - 1, expiration, vec![1]);
        let config = PreCommitConfig::new(Some(make_piece_cid("1".as_bytes())));
        println!("precommitting");
        let precommit =
            precommit_sector_and_get(&mut w, miner_id, owner.id, precommit_params, config, i == 0);
        println!("precommitted {}", i);
        precommits.push(precommit);
    }

    // ========= Workbench setup complete
}
