use fil_actor_market::{
    ClientDealProposal, DealProposal, Label, Method as MarketMethod, PublishStorageDealsParams,
    PublishStorageDealsReturn,
};
use fil_actor_miner::max_prove_commit_duration;
use fil_actor_verifreg::{AddVerifiedClientParams, Method as VerifregMethod};
use fil_actors_runtime::cbor::serialize;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{EPOCHS_IN_DAY, STORAGE_MARKET_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR};
use fvm_ipld_blockstore::MemoryBlockstore;
use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::crypto::signature::ops::verify_secp256k1_sig;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;
use fvm_shared::piece::PaddedPieceSize;
use fvm_shared::sector::{RegisteredSealProof, StoragePower};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::ExecutionResult;
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::{create_genesis_actors, GenesisResult, GenesisSpec};
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;

use crate::util::*;
use crate::workflows::*;

mod util;

#[allow(dead_code)]
struct Addrs {
    worker: Account,
    client1: Account,
    client2: Account,
    not_miner: Account,
    cheap_client: Account,
    maddr: Address,
    verified_client: Account,
}

const DEAL_LIFETIME: ChainEpoch = 181 * EPOCHS_IN_DAY;

fn token_defaults() -> (TokenAmount, TokenAmount, TokenAmount) {
    let price_per_epoch = TokenAmount::from_atto(1 << 20);
    let provider_collateral = TokenAmount::from_whole(2);
    let client_collateral = TokenAmount::from_whole(1);
    (price_per_epoch, provider_collateral, client_collateral)
}

/* Publish some storage deals. */
#[test]
fn publish_storage_deals() {
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

    let mut bench = builder.build().unwrap();
    let mut w = ExecutionWrangler::new_default(&mut *bench);

    let (a, deal_start) = setup(&mut w, &genesis);
    let mut batcher =
        DealBatcher::new(a.maddr, PaddedPieceSize(1 << 30), false, deal_start, DEAL_LIFETIME);

    let options = DealOptions { verified: Some(true), ..Default::default() };
    batcher.stage(&a.verified_client, "deal0", options.clone());
    batcher.stage(&a.verified_client, "deal1", options.clone());
    batcher.stage(&a.verified_client, "deal2", options.clone());
    batcher.stage(&a.verified_client, "deal3", options.clone());
    batcher.stage(&a.verified_client, "deal4", options);

    let result = batcher.publish_ok(&mut w, a.worker.id_addr());
    let ret: PublishStorageDealsReturn = result.receipt.return_data.deserialize().unwrap();
    let good_inputs = bf_all(ret.valid_deals);
    assert_eq!(vec![0, 1, 2, 3, 4], good_inputs);

    println!("{}", result.trace.format());
    let analysis = TraceAnalysis::build(result.trace);
    println!("{}", analysis.format_spans());
}

// create miner and client and add collateral
fn setup(w: &mut ExecutionWrangler, genesis: &GenesisResult) -> (Addrs, ChainEpoch) {
    let balance = TokenAmount::from_whole(10_000);
    let worker = create_accounts(w, genesis.faucet_id, 1, balance.clone(), SignatureType::BLS)
        .unwrap()[0]
        .clone();
    let owner = worker.clone();
    let accounts =
        create_accounts(w, genesis.faucet_id, 6, balance, SignatureType::Secp256k1).unwrap();
    let (client1, client2, not_miner, cheap_client, verifier, verified_client) = (
        accounts[0].clone(),
        accounts[1].clone(),
        accounts[2].clone(),
        accounts[3].clone(),
        accounts[4].clone(),
        accounts[5].clone(),
    );

    // setup provider
    let miner_balance = TokenAmount::from_whole(100);
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;

    let (maddr, _) = create_miner(
        w,
        owner.id,
        worker.id,
        seal_proof.registered_window_post_proof().unwrap(),
        miner_balance,
    )
    .unwrap();

    // setup FIL+ verifier
    verifreg_add_verifier(
        w,
        verifier.id,
        StoragePower::from((32_u64 << 40) as u128),
        genesis.verifreg_root_address(),
        genesis.verifreg_signer_address(),
    );
    let add_client_params = AddVerifiedClientParams {
        address: verified_client.id_addr(),
        allowance: StoragePower::from(1000_u64 << 30),
    };
    apply_ok(
        w,
        verifier.id_addr(),
        VERIFIED_REGISTRY_ACTOR_ADDR,
        TokenAmount::zero(),
        VerifregMethod::AddVerifiedClient as u64,
        &add_client_params,
    )
    .unwrap();

    let client_collateral = TokenAmount::from_whole(100);
    apply_ok(
        w,
        client1.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        client_collateral.clone(),
        MarketMethod::AddBalance as u64,
        &client1.id_addr(),
    )
    .unwrap();
    apply_ok(
        w,
        client2.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        client_collateral.clone(),
        MarketMethod::AddBalance as u64,
        &client2.id_addr(),
    )
    .unwrap();
    apply_ok(
        w,
        verified_client.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        client_collateral,
        MarketMethod::AddBalance as u64,
        &verified_client.id_addr(),
    )
    .unwrap();

    let miner_collateral = TokenAmount::from_whole(100);
    apply_ok(
        w,
        worker.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        miner_collateral,
        MarketMethod::AddBalance as u64,
        &Address::new_id(maddr),
    )
    .unwrap();

    let deal_start = w.epoch() + max_prove_commit_duration(&Policy::default(), seal_proof).unwrap();
    (
        Addrs {
            worker,
            client1,
            client2,
            not_miner,
            cheap_client,
            maddr: Address::new_id(maddr),
            verified_client,
        },
        deal_start,
    )
}

#[derive(Clone, Default)]
struct DealOptions {
    provider: Option<Address>,
    piece_size: Option<PaddedPieceSize>,
    verified: Option<bool>,
    deal_start: Option<ChainEpoch>,
    deal_lifetime: Option<ChainEpoch>,
    price_per_epoch: Option<TokenAmount>,
    provider_collateral: Option<TokenAmount>,
    client_collateral: Option<TokenAmount>,
}

struct DealBatcher {
    deals: Vec<ClientDealProposal>,
    default_provider: Address,
    default_piece_size: PaddedPieceSize,
    default_verified: bool,
    default_deal_start: ChainEpoch,
    default_deal_lifetime: ChainEpoch,
    default_price_per_epoch: TokenAmount,
    default_provider_collateral: TokenAmount,
    default_client_collateral: TokenAmount,
}

impl DealBatcher {
    fn new(
        default_provider: Address,
        default_piece_size: PaddedPieceSize,
        default_verified: bool,
        default_deal_start: ChainEpoch,
        default_deal_lifetime: ChainEpoch,
    ) -> Self {
        let (default_price_per_epoch, default_provider_collateral, default_client_collateral) =
            token_defaults();
        DealBatcher {
            deals: vec![],
            default_provider,
            default_piece_size,
            default_verified,
            default_deal_start,
            default_deal_lifetime,
            default_price_per_epoch,
            default_provider_collateral,
            default_client_collateral,
        }
    }

    pub fn stage(&mut self, client: &Account, deal_label: &str, opts: DealOptions) {
        let opts = self.default_opts(opts);
        let label = Label::String(deal_label.to_string());
        let proposal = DealProposal {
            piece_cid: make_piece_cid(deal_label.as_bytes()),
            piece_size: opts.piece_size.unwrap(),
            verified_deal: opts.verified.unwrap(),
            client: client.id_addr(),
            provider: opts.provider.unwrap(),
            label,
            start_epoch: opts.deal_start.unwrap(),
            end_epoch: opts.deal_start.unwrap() + opts.deal_lifetime.unwrap(),
            storage_price_per_epoch: opts.price_per_epoch.unwrap(),
            provider_collateral: opts.provider_collateral.unwrap(),
            client_collateral: opts.client_collateral.unwrap(),
        };
        let payload = serialize(&proposal, "proposal").unwrap();
        let client_signature = client.sign(&payload).unwrap();

        verify_secp256k1_sig(client_signature.bytes(), &payload, &client.key_addr())
            .expect("valid");

        self.deals.push(ClientDealProposal { proposal, client_signature });
    }

    pub fn default_opts(&self, in_opts: DealOptions) -> DealOptions {
        let mut opts = in_opts.clone();
        if in_opts.provider.is_none() {
            opts.provider = Some(self.default_provider)
        }
        if in_opts.piece_size.is_none() {
            opts.piece_size = Some(self.default_piece_size)
        }
        if in_opts.verified.is_none() {
            opts.verified = Some(self.default_verified)
        }
        if in_opts.deal_start.is_none() {
            opts.deal_start = Some(self.default_deal_start)
        }
        if in_opts.deal_lifetime.is_none() {
            opts.deal_lifetime = Some(self.default_deal_lifetime)
        }
        if in_opts.price_per_epoch.is_none() {
            opts.price_per_epoch = Some(self.default_price_per_epoch.clone())
        }
        if in_opts.provider_collateral.is_none() {
            opts.provider_collateral = Some(self.default_provider_collateral.clone())
        }
        if in_opts.client_collateral.is_none() {
            opts.client_collateral = Some(self.default_client_collateral.clone())
        }
        opts
    }

    pub fn publish_ok(&mut self, w: &mut ExecutionWrangler, sender: Address) -> ExecutionResult {
        let publish_params = PublishStorageDealsParams { deals: self.deals.clone() };
        apply_ok(
            w,
            sender,
            STORAGE_MARKET_ACTOR_ADDR,
            TokenAmount::zero(),
            MarketMethod::PublishStorageDeals as u64,
            &publish_params,
        )
        .unwrap()
    }
}
