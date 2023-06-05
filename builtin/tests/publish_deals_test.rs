use fil_actor_market::PublishStorageDealsReturn;
use fvm_ipld_blockstore::MemoryBlockstore;

use fvm_shared::address::Address;
use fvm_shared::bigint::Zero;
use fvm_shared::clock::ChainEpoch;
use fvm_shared::crypto::signature::SignatureType;
use fvm_shared::econ::TokenAmount;

use fil_actor_market::Method as MarketMethod;
use fil_actor_miner::max_prove_commit_duration;
use fil_actor_verifreg::{AddVerifiedClientParams, Method as VerifregMethod};
use fil_actors_runtime::network::EPOCHS_IN_DAY;
use fil_actors_runtime::runtime::Policy;
use fil_actors_runtime::{STORAGE_MARKET_ACTOR_ADDR, VERIFIED_REGISTRY_ACTOR_ADDR};
use fvm_shared::sector::{RegisteredSealProof, StoragePower};
use fvm_shared::state::StateTreeVersion;
use fvm_shared::version::NetworkVersion;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_api::wrangler::ExecutionWrangler;
use fvm_workbench_api::Bench;
use fvm_workbench_api::WorkbenchBuilder;
use fvm_workbench_builtin_actors::genesis::create_genesis_actors;
use fvm_workbench_builtin_actors::genesis::GenesisResult;
use fvm_workbench_builtin_actors::genesis::GenesisSpec;
use fvm_workbench_vm::builder::FvmBenchBuilder;
use fvm_workbench_vm::externs::FakeExterns;

use crate::util::deals::DealBatcher;
use crate::util::deals::DealOptions;
use crate::util::*;
use crate::workflows::*;
mod util;

pub struct Addrs {
    pub worker: Address,
    pub client1: Address,
    pub client2: Address,
    pub not_miner: Address,
    pub cheap_client: Address,
    pub maddr: Address,
    pub verified_client: Address,
}

const _DEAL_LIFETIME: ChainEpoch = 181 * EPOCHS_IN_DAY;

// create miner and client and add collateral
fn setup(
    bench: &'_ mut dyn Bench,
    genesis: GenesisResult,
) -> (ExecutionWrangler<'_>, Addrs, ChainEpoch) {
    let mut w = ExecutionWrangler::new_default(bench);
    let addrs = create_accounts(
        &mut w,
        genesis.faucet_id,
        7,
        TokenAmount::from_whole(10_000),
        SignatureType::BLS,
    )
    .unwrap();
    let (worker, client1, client2, not_miner, cheap_client, verifier, verified_client) = (
        addrs[0].clone(),
        addrs[1].clone(),
        addrs[2].clone(),
        addrs[3].clone(),
        addrs[4].clone(),
        addrs[5].clone(),
        addrs[6].clone(),
    );
    let owner = worker.clone();

    // setup provider
    let miner_balance = TokenAmount::from_whole(100);
    let seal_proof = RegisteredSealProof::StackedDRG32GiBV1P1;

    let (_miner, maddr) = create_miner(
        &mut w,
        owner.id,
        worker.id,
        seal_proof.registered_window_post_proof().unwrap(),
        miner_balance,
    )
    .unwrap();

    // setup verified client
    verifreg_add_verifier(
        &mut w,
        verifier.id,
        StoragePower::from((32_u64 << 40) as u128),
        genesis.verifreg_root_address(),
        genesis.verifreg_signer_address(),
    );
    let add_client_params = AddVerifiedClientParams {
        address: verified_client.id_addr(),
        allowance: StoragePower::from(1_u64 << 32),
    };
    apply_ok(
        &mut w,
        verifier.id_addr(),
        VERIFIED_REGISTRY_ACTOR_ADDR,
        TokenAmount::zero(),
        VerifregMethod::AddVerifiedClient as u64,
        &add_client_params,
    )
    .unwrap();

    let client_collateral = TokenAmount::from_whole(100);
    apply_ok(
        &mut w,
        client1.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        client_collateral.clone(),
        MarketMethod::AddBalance as u64,
        &client1.id_addr(),
    )
    .unwrap();
    apply_ok(
        &mut w,
        client2.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        client_collateral.clone(),
        MarketMethod::AddBalance as u64,
        &client2.id_addr(),
    )
    .unwrap();
    apply_ok(
        &mut w,
        verified_client.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        client_collateral,
        MarketMethod::AddBalance as u64,
        &verified_client.id_addr(),
    )
    .unwrap();

    let miner_collateral = TokenAmount::from_whole(100);
    apply_ok(
        &mut w,
        worker.id_addr(),
        STORAGE_MARKET_ACTOR_ADDR,
        miner_collateral,
        MarketMethod::AddBalance as u64,
        &maddr,
    )
    .unwrap();

    let deal_start = w.epoch() + max_prove_commit_duration(&Policy::default(), seal_proof).unwrap();
    (
        w,
        Addrs {
            worker: worker.id_addr(),
            client1: client1.id_addr(),
            client2: client2.id_addr(),
            not_miner: not_miner.id_addr(),
            cheap_client: cheap_client.id_addr(),
            maddr,
            verified_client: verified_client.id_addr(),
        },
        deal_start,
    )
}

#[test]
fn publish_storage_deals() {
    let (mut builder, manifest_data_cid) = FvmBenchBuilder::new_with_bundle(
        MemoryBlockstore::new(),
        FakeExterns::new(),
        NetworkVersion::V18,
        StateTreeVersion::V5,
        actors_v12::BUNDLE_CAR,
    )
    .unwrap();
    let spec = GenesisSpec::default(manifest_data_cid);
    let genesis = create_genesis_actors(&mut builder, &spec).unwrap();
    let mut bench = builder.build().unwrap();

    let (mut w, a, deal_start) = setup(&mut *bench, genesis);
    publish_storage_deals_test(&mut w, a, deal_start, 8);
}

fn publish_storage_deals_test(
    w: &mut ExecutionWrangler<'_>,
    a: Addrs,
    deal_start: i64,
    num_deals: usize,
) {
    let opts = DealOptions { deal_start, ..DealOptions::default() };
    let mut batcher = DealBatcher::new(w, opts);

    for _ in 0..num_deals {
        // good deal
        batcher.stage(a.client1, a.maddr);
    }

    let execution_ret = batcher.publish_ok(a.worker);

    let analysis = TraceAnalysis::build(execution_ret.trace);
    println!("{}", analysis.format_spans());

    let deal_ret: PublishStorageDealsReturn =
        execution_ret.receipt.return_data.deserialize().unwrap();
    let good_inputs = bf_all(deal_ret.valid_deals);
    assert_eq!(vec![0, 1, 2, 3, 4, 5, 6, 7], good_inputs);
}

// #[test]
// fn psd_bad_piece_size() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);

//     psd_bad_piece_size_test(&v, a, deal_start);
// }

// fn psd_bad_piece_size_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());

//     // bad deal piece size too small
//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { piece_size: PaddedPieceSize(0), ..opts },
//     );
//     // good deal
//     batcher.stage(a.client1, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![1], good_inputs);
// }

// #[test]
// fn psd_start_time_in_past() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_start_time_in_past_test(&v, a, deal_start);
// }

// fn psd_start_time_in_past_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());

//     let bad_deal_start = v.epoch() - 1;
//     batcher.stage_with_opts(a.client1, a.maddr, DealOptions { deal_start: bad_deal_start, ..opts });
//     batcher.stage(a.client1, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![1], good_inputs);
// }

// #[test]
// fn psd_client_address_cannot_be_resolved() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_client_address_cannot_be_resolved_test(&v, a, deal_start);
// }

// fn psd_client_address_cannot_be_resolved_test<BS: Blockstore>(
//     v: &dyn VM<BS>,
//     a: Addrs,
//     deal_start: i64,
// ) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts);
//     let bad_client = Address::new_id(5_000_000);
//     batcher.stage(a.client1, a.maddr);
//     batcher.stage(bad_client, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0], good_inputs);
// }

// #[test]
// fn psd_no_client_lockup() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_no_client_lockup_test(&v, a, deal_start);
// }

// fn psd_no_client_lockup_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts);
//     batcher.stage(a.cheap_client, a.maddr);
//     batcher.stage(a.client1, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![1], good_inputs);
// }

// #[test]
// fn psd_not_enough_client_lockup_for_batch() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);

//     psd_not_enough_client_lockup_for_batch_test(&v, a, deal_start);
// }

// fn psd_not_enough_client_lockup_for_batch_test(
//     w: &ExecutionWrangler<'_>,
//     a: Addrs,
//     deal_start: i64,
// ) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(w, opts.clone());

//     // Add one lifetime cost to cheap_client's market balance but attempt to make 3 deals
//     let one_lifetime_cost = opts.client_collateral + DEAL_LIFETIME * opts.price_per_epoch;
//     apply_ok(
//         &mut w,
//         &a.cheap_client,
//         &STORAGE_MARKET_ACTOR_ADDR,
//         &one_lifetime_cost,
//         MarketMethod::AddBalance as u64,
//         &a.cheap_client,
//     );

//     // good
//     batcher.stage(a.cheap_client, a.maddr);
//     // bad -- insufficient funds
//     batcher.stage(a.cheap_client, a.maddr);
//     batcher.stage(a.cheap_client, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0], good_inputs);
// }

// #[test]
// fn psd_not_enough_provider_lockup_for_batch() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);

//     psd_not_enough_provider_lockup_for_batch_test(&v, deal_start, a);
// }

// fn psd_not_enough_provider_lockup_for_batch_test(w: &dyn VM<BS>, deal_start: i64, a: Addrs) {
//     // note different seed, different address
//     let cheap_worker = create_accounts_seeded(w, 1, &TokenAmount::from_whole(10_000), 444)[0];
//     let cheap_maddr = create_miner(
//         w,
//         &cheap_worker,
//         &cheap_worker,
//         fvm_shared::sector::RegisteredPoStProof::StackedDRGWindow32GiBV1P1,
//         &TokenAmount::from_whole(100),
//     )
//     .0;
//     // add one deal of collateral to provider's market account
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(w, opts.clone());

//     apply_ok(
//         w,
//         &cheap_worker,
//         &STORAGE_MARKET_ACTOR_ADDR,
//         &opts.provider_collateral,
//         MarketMethod::AddBalance as u64,
//         Some(cheap_maddr),
//     );
//     // good deal
//     batcher.stage(a.client1, cheap_maddr);
//     // bad deal insufficient funds on provider
//     batcher.stage(a.client2, cheap_maddr);
//     let deal_ret = batcher.publish_ok(cheap_worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0], good_inputs);

//     assert_invariants(w)
// }

// #[test]
// fn psd_duplicate_deal_in_batch() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_duplicate_deal_in_batch_test(&v, a, deal_start);
// }

// fn psd_duplicate_deal_in_batch_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts);

//     // good deals
//     batcher.stage_with_label(a.client1, a.maddr, "deal0".to_string());
//     batcher.stage_with_label(a.client1, a.maddr, "deal1".to_string());

//     // bad duplicates
//     batcher.stage_with_label(a.client1, a.maddr, "deal0".to_string());
//     batcher.stage_with_label(a.client1, a.maddr, "deal0".to_string());

//     // good
//     batcher.stage_with_label(a.client1, a.maddr, "deal2".to_string());

//     // bad
//     batcher.stage_with_label(a.client1, a.maddr, "deal1".to_string());

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0, 1, 4], good_inputs);

//     assert_invariants(v)
// }

// #[test]
// fn psd_duplicate_deal_in_state() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_duplicate_deal_in_state_test(&v, a, deal_start);
// }

// fn psd_duplicate_deal_in_state_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());

//     batcher.stage(a.client2, a.maddr);
//     let deal_ret1 = batcher.publish_ok(a.worker);
//     let good_inputs1 = bf_all(deal_ret1.valid_deals);
//     assert_eq!(vec![0], good_inputs1);

//     let mut batcher = DealBatcher::new(v, opts);
//     // duplicate in state from previous dealer
//     batcher.stage(a.client2, a.maddr);
//     // duplicate in batch
//     batcher.stage_with_label(a.client2, a.maddr, "deal1".to_string());
//     batcher.stage_with_label(a.client2, a.maddr, "deal1".to_string());

//     let deal_ret2 = batcher.publish_ok(a.worker);
//     let good_inputs2 = bf_all(deal_ret2.valid_deals);
//     assert_eq!(vec![1], good_inputs2);

//     assert_invariants(v)
// }

// #[test]
// fn psd_verified_deal_fails_getting_datacap() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_verified_deal_fails_getting_datacap_test(&v, a, deal_start);
// }

// fn psd_verified_deal_fails_getting_datacap_test<BS: Blockstore>(
//     v: &dyn VM<BS>,
//     a: Addrs,
//     deal_start: i64,
// ) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());

//     batcher.stage(a.verified_client, a.maddr);
//     // good verified deal that uses up all data cap
//     batcher.stage_with_opts(
//         a.verified_client,
//         a.maddr,
//         DealOptions { piece_size: PaddedPieceSize(1 << 32), verified: true, ..opts.clone() },
//     );
//     // bad verified deal, no data cap left
//     batcher.stage_with_opts(
//         a.verified_client,
//         a.maddr,
//         DealOptions { piece_size: PaddedPieceSize(1 << 32), verified: true, ..opts },
//     );

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0, 1], good_inputs);

//     assert_invariants(v)
// }

// #[test]
// fn psd_random_assortment_of_failures() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_random_assortment_of_failures_test(&v, a, deal_start);
// }

// fn psd_random_assortment_of_failures_test<BS: Blockstore>(
//     v: &dyn VM<BS>,
//     a: Addrs,
//     deal_start: i64,
// ) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());
//     // Add one lifetime cost to cheap_client's market balance but attempt to make 3 deals
//     let one_lifetime_cost = &opts.client_collateral + DEAL_LIFETIME * &opts.price_per_epoch;
//     apply_ok(
//         v,
//         &a.cheap_client,
//         &STORAGE_MARKET_ACTOR_ADDR,
//         &one_lifetime_cost,
//         MarketMethod::AddBalance as u64,
//         Some(a.cheap_client),
//     );
//     let broke_client = create_accounts_seeded(v, 1, &TokenAmount::zero(), 555)[0];

//     batcher.stage_with_opts_label(
//         a.verified_client,
//         a.maddr,
//         "foo".to_string(),
//         DealOptions { piece_size: PaddedPieceSize(1 << 32), verified: true, ..opts.clone() },
//     );
//     // duplicate
//     batcher.stage_with_opts_label(
//         a.verified_client,
//         a.maddr,
//         "foo".to_string(),
//         DealOptions { piece_size: PaddedPieceSize(1 << 32), verified: true, ..opts.clone() },
//     );
//     batcher.stage(a.cheap_client, a.maddr);
//     // no client funds
//     batcher.stage(broke_client, a.maddr);
//     // provider addr does not match
//     batcher.stage(a.client1, a.client2);
//     // insufficient data cap
//     batcher.stage_with_opts(
//         a.verified_client,
//         a.maddr,
//         DealOptions { verified: true, ..opts.clone() },
//     );
//     // cheap client out of funds
//     batcher.stage(a.cheap_client, a.maddr);
//     // provider collateral too low
//     batcher.stage_with_opts(
//         a.client2,
//         a.maddr,
//         DealOptions { provider_collateral: TokenAmount::zero(), ..opts },
//     );
//     batcher.stage(a.client1, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0, 2, 8], good_inputs);

//     assert_invariants(v)
// }

// #[test]
// fn psd_all_deals_are_bad() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_all_deals_are_bad_test(&v, a, deal_start);
// }

// fn psd_all_deals_are_bad_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());
//     let bad_client = Address::new_id(1000);

//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { provider_collateral: TokenAmount::zero(), ..opts.clone() },
//     );
//     batcher.stage(a.client1, a.client2);
//     batcher.stage_with_opts(a.client1, a.maddr, DealOptions { verified: true, ..opts.clone() });
//     batcher.stage(bad_client, a.maddr);
//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { piece_size: PaddedPieceSize(0), ..opts },
//     );

//     batcher.publish_fail(a.worker);
//     assert_invariants(v)
// }

// #[test]
// fn psd_bad_sig() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_bad_sig_test(&v, a, deal_start);
// }

// fn psd_bad_sig_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let DealOptions { price_per_epoch, provider_collateral, client_collateral, .. } =
//         DealOptions::default();
//     let deal_label = "deal0".to_string();
//     let proposal = DealProposal {
//         piece_cid: make_piece_cid(deal_label.as_bytes()),
//         piece_size: PaddedPieceSize(1 << 30),
//         verified_deal: false,
//         client: a.client1,
//         provider: a.maddr,
//         label: Label::String(deal_label),
//         start_epoch: deal_start,
//         end_epoch: deal_start + DEAL_LIFETIME,
//         storage_price_per_epoch: price_per_epoch,
//         provider_collateral,
//         client_collateral,
//     };

//     let invalid_sig_bytes = "very_invalid_sig".as_bytes().to_vec();

//     let publish_params = PublishStorageDealsParams {
//         deals: vec![ClientDealProposal {
//             proposal: proposal.clone(),
//             client_signature: Signature {
//                 sig_type: SignatureType::BLS,
//                 bytes: invalid_sig_bytes.clone(),
//             },
//         }],
//     };
//     let ret = v
//         .execute_message(
//             &a.worker,
//             &STORAGE_MARKET_ACTOR_ADDR,
//             &TokenAmount::zero(),
//             MarketMethod::PublishStorageDeals as u64,
//             Some(serialize_ok(&publish_params)),
//         )
//         .unwrap();
//     assert_eq!(ExitCode::USR_ILLEGAL_ARGUMENT, ret.code);

//     ExpectInvocation {
//         from: a.worker,
//         to: STORAGE_MARKET_ACTOR_ADDR,
//         method: MarketMethod::PublishStorageDeals as u64,
//         subinvocs: Some(vec![
//             Expect::miner_is_controlling_address(STORAGE_MARKET_ACTOR_ADDR, a.maddr, a.worker),
//             Expect::reward_this_epoch(STORAGE_MARKET_ACTOR_ADDR),
//             Expect::power_current_total(STORAGE_MARKET_ACTOR_ADDR),
//             ExpectInvocation {
//                 from: STORAGE_MARKET_ACTOR_ADDR,
//                 to: a.client1,
//                 method: AccountMethod::AuthenticateMessageExported as u64,
//                 params: Some(
//                     IpldBlock::serialize_cbor(&AuthenticateMessageParams {
//                         signature: invalid_sig_bytes,
//                         message: serialize(&proposal, "deal proposal").unwrap().to_vec(),
//                     })
//                     .unwrap(),
//                 ),
//                 code: ExitCode::USR_ILLEGAL_ARGUMENT,
//                 ..Default::default()
//             },
//         ]),
//         code: ExitCode::USR_ILLEGAL_ARGUMENT,
//         ..Default::default()
//     }
//     .matches(v.take_invocations().last().unwrap());

//     assert_invariants(v)
// }

// #[test]
// fn psd_all_deals_are_good() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     all_deals_are_good_test(&v, a, deal_start);
// }

// fn all_deals_are_good_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts);

//     // good deals
//     batcher.stage(a.client1, a.maddr);
//     batcher.stage(a.client1, a.maddr);
//     batcher.stage(a.client1, a.maddr);
//     batcher.stage(a.client1, a.maddr);
//     batcher.stage(a.client1, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0, 1, 2, 3, 4], good_inputs);

//     assert_invariants(v)
// }

// #[test]
// fn psd_valid_deals_with_ones_longer_than_540() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_valid_deals_with_ones_longer_than_540_test(&v, a, deal_start);
// }

// fn psd_valid_deals_with_ones_longer_than_540_test<BS: Blockstore>(
//     v: &dyn VM<BS>,
//     a: Addrs,
//     deal_start: i64,
// ) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());

//     // good deals
//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { deal_lifetime: 541 * EPOCHS_IN_DAY, ..opts.clone() },
//     );
//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { deal_lifetime: 1278 * EPOCHS_IN_DAY, ..opts },
//     );
//     batcher.stage(a.client1, a.maddr);

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0, 1, 2], good_inputs);

//     assert_invariants(v)
// }

// #[test]
// fn psd_deal_duration_too_long() {
//     let store = MemoryBlockstore::new();
//     let (v, a, deal_start) = setup(&store);
//     psd_deal_duration_too_long_test(&v, a, deal_start);
// }

// fn psd_deal_duration_too_long_test<BS: Blockstore>(v: &dyn VM<BS>, a: Addrs, deal_start: i64) {
//     let opts = DealOptions { deal_start, ..DealOptions::default() };
//     let mut batcher = DealBatcher::new(v, opts.clone());

//     // good deals
//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { deal_lifetime: 541 * EPOCHS_IN_DAY, ..opts.clone() },
//     );
//     batcher.stage(a.client1, a.maddr);

//     //bad deal - duration > max deal
//     batcher.stage_with_opts(
//         a.client1,
//         a.maddr,
//         DealOptions { deal_lifetime: 1279 * EPOCHS_IN_DAY, ..opts },
//     );

//     let deal_ret = batcher.publish_ok(a.worker);
//     let good_inputs = bf_all(deal_ret.valid_deals);
//     assert_eq!(vec![0, 1], good_inputs);

//     assert_invariants(v)
// }
