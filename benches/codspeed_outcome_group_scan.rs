mod support;

use std::{collections::BTreeMap, sync::OnceLock};

use bolt_v2::{
    bolt_v3_outcome_group_proofs::StructuredOutcomeGroupingProof,
    bolt_v3_outcome_group_scanner::{
        OutcomeGroupCandidateLeg, OutcomeGroupDepthSnapshot, OutcomeGroupScanInput,
        scan_outcome_group_candidate,
    },
    bolt_v3_outcome_groups::{
        CanonicalField, GroupingProof, NormalizedPriceScaleEvidence, OrderConstraintSource,
        OutcomeGroup, OutcomeGroupSourceKind, OutcomeLeg, OutcomeLegOrderConstraints,
        OutcomeLegRole, PriceScaleAssertionSource, RoleBindingProof, SettlementRules,
        SettlementSourceKind, TerminalPayoutDerivation, TerminalState, TerminalStateConvention,
        TerminalStateKind, ValidatedOutcomeGroup, build_leg_map, canonical_fingerprint,
        derive_standard_payout_matrix, expected_metadata_fingerprint,
    },
};
use nautilus_model::{
    data::BookOrder,
    enums::OrderSide,
    identifiers::{ClientId, InstrumentId, Venue},
    orderbook::BookLevel,
    types::{Price, Quantity},
};
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    outcome_group_scan: ScanFixture,
}

#[derive(Deserialize)]
struct ScanFixture {
    group_id: String,
    source_client_id: String,
    venue: String,
    settlement_asset_id: String,
    freshness_source_id: String,
    grouping_question: u32,
    outcome_indices: Vec<u32>,
    payout_per_contract: String,
    price_units_per_payout: String,
    min_quantity: String,
    min_notional: String,
    quantity_step: String,
    fee_bps: String,
    now_unix_ms: u64,
    max_book_age_ms: u64,
    admissible_min_edge_bps: String,
    blocked_min_edge_bps: String,
    vwap_depth_limit_bps: u64,
    slippage_buffer_bps: u64,
    states: Vec<StateFixture>,
    legs: Vec<LegFixture>,
    candidates: Vec<CandidateFixture>,
    books: Vec<BookFixture>,
}

#[derive(Deserialize)]
struct StateFixture {
    state_id: String,
    label: String,
}

#[derive(Deserialize)]
struct LegFixture {
    leg_id: String,
    instrument_id: String,
    native_leg_id: String,
    outcome_label: String,
    side_label: String,
    terminal_state_id: String,
}

#[derive(Deserialize)]
struct CandidateFixture {
    leg_id: String,
    order_side: String,
    target_notional: String,
}

#[derive(Deserialize)]
struct BookFixture {
    leg_id: String,
    bid_price: String,
    bid_quantity: String,
    ask_price: String,
    ask_quantity: String,
    observed_unix_ms: u64,
    bid_order_id: u64,
    ask_order_id: u64,
}

struct ScanCase {
    group: OutcomeGroup,
    candidate_legs: Vec<OutcomeGroupCandidateLeg>,
    books: BTreeMap<InstrumentId, OutcomeGroupDepthSnapshot>,
    fee_bps: BTreeMap<InstrumentId, Decimal>,
    now_unix_ms: u64,
    max_book_age_ms: u64,
    min_edge_bps: Decimal,
    vwap_depth_limit_bps: u64,
    slippage_buffer_bps: u64,
}

fn main() {
    divan::main();
}

fn fixture() -> &'static ScanFixture {
    static FIXTURE: OnceLock<ScanFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| support::decode_fixtures::<FixtureFile>().outcome_group_scan)
}

#[divan::bench]
fn admissible_candidate(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| scan_case(fixture(), &fixture().admissible_min_edge_bps))
        .bench_values(|case| divan::black_box(run_scan(divan::black_box(case))));
}

#[divan::bench]
fn edge_threshold_candidate(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| scan_case(fixture(), &fixture().blocked_min_edge_bps))
        .bench_values(|case| divan::black_box(run_scan(divan::black_box(case))));
}

fn scan_case(fixture: &ScanFixture, min_edge_bps: &str) -> ScanCase {
    let group = outcome_group(fixture);
    let candidate_legs = fixture
        .candidates
        .iter()
        .map(|candidate| OutcomeGroupCandidateLeg {
            leg_id: candidate.leg_id.clone(),
            order_side: order_side(&candidate.order_side),
            target_notional: decimal(&candidate.target_notional),
        })
        .collect();
    let books = fixture
        .books
        .iter()
        .map(|book| {
            let instrument_id = group
                .tradable_legs
                .get(&book.leg_id)
                .expect("book fixture must reference a configured leg")
                .instrument_id;
            let snapshot = OutcomeGroupDepthSnapshot::from_book_levels(
                instrument_id,
                Some(book.observed_unix_ms),
                vec![BookLevel::from_order(BookOrder::new(
                    OrderSide::Buy,
                    Price::from(book.bid_price.as_str()),
                    Quantity::from(book.bid_quantity.as_str()),
                    book.bid_order_id,
                ))],
                vec![BookLevel::from_order(BookOrder::new(
                    OrderSide::Sell,
                    Price::from(book.ask_price.as_str()),
                    Quantity::from(book.ask_quantity.as_str()),
                    book.ask_order_id,
                ))],
            )
            .expect("book fixture must contain valid depth");
            (instrument_id, snapshot)
        })
        .collect();
    let fee = decimal(&fixture.fee_bps);
    let fee_bps = group
        .tradable_legs
        .values()
        .map(|leg| (leg.instrument_id, fee))
        .collect();
    ScanCase {
        group,
        candidate_legs,
        books,
        fee_bps,
        now_unix_ms: fixture.now_unix_ms,
        max_book_age_ms: fixture.max_book_age_ms,
        min_edge_bps: decimal(min_edge_bps),
        vwap_depth_limit_bps: fixture.vwap_depth_limit_bps,
        slippage_buffer_bps: fixture.slippage_buffer_bps,
    }
}

fn run_scan(case: ScanCase) -> bolt_v2::bolt_v3_outcome_group_scanner::OutcomeGroupScanEvidence {
    let ScanCase {
        group,
        candidate_legs,
        books,
        fee_bps,
        now_unix_ms,
        max_book_age_ms,
        min_edge_bps,
        vwap_depth_limit_bps,
        slippage_buffer_bps,
    } = case;
    scan_outcome_group_candidate(OutcomeGroupScanInput {
        group: &group,
        candidate_legs,
        books,
        fee_bps,
        now_unix_ms,
        max_book_age_ms,
        min_edge_bps,
        vwap_depth_limit_bps,
        slippage_buffer_bps,
    })
}

fn outcome_group(fixture: &ScanFixture) -> OutcomeGroup {
    let terminal_states = fixture
        .states
        .iter()
        .map(|state| {
            (
                state.state_id.clone(),
                TerminalState {
                    state_id: state.state_id.clone(),
                    label: state.label.clone(),
                    kind: TerminalStateKind::Standard,
                },
            )
        })
        .collect();
    let legs = build_leg_map(
        fixture
            .legs
            .iter()
            .map(|leg| outcome_leg(fixture, leg))
            .collect(),
    )
    .expect("outcome-group fixture leg ids must be unique");
    let payout_matrix = derive_standard_payout_matrix(
        &terminal_states,
        &legs,
        TerminalStateConvention::ExactlyOneWinner,
    )
    .expect("outcome-group fixture must derive a standard payout matrix");
    let mut group = OutcomeGroup {
        group_id: fixture.group_id.clone(),
        source_client_id: ClientId::from(fixture.source_client_id.as_str()),
        venue: Venue::from(fixture.venue.as_str()),
        source_kind: OutcomeGroupSourceKind::Hyperliquid,
        settlement_asset_id: fixture.settlement_asset_id.clone(),
        terminal_states,
        tradable_legs: legs,
        payout_matrix,
        grouping_proof: Some(GroupingProof::HyperliquidOutcome(
            StructuredOutcomeGroupingProof {
                question: fixture.grouping_question,
                outcome_indices: fixture.outcome_indices.clone(),
                proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                    ["grouping", "group_id"],
                    &fixture.group_id,
                )]),
            },
        )),
        role_binding_proof: Some(RoleBindingProof::VenueStructuredFields {
            source_id: fixture.freshness_source_id.clone(),
            question: fixture.grouping_question,
            outcome_indices: fixture.outcome_indices.clone(),
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["role_binding", "source_id"],
                &fixture.freshness_source_id,
            )]),
        }),
        settlement_rules: SettlementRules {
            terminal_state_convention: TerminalStateConvention::ExactlyOneWinner,
            settlement_source_kind: SettlementSourceKind::VenueStructuredFields,
            non_standard_terminal_payouts: Vec::new(),
            terminal_payout_derivation: TerminalPayoutDerivation::StandardRowsPlusAttestedVectors,
        },
        freshness_source_id: fixture.freshness_source_id.clone(),
        metadata_fingerprint: String::new(),
    };
    group.metadata_fingerprint = expected_metadata_fingerprint(&group);
    ValidatedOutcomeGroup::validate(&group)
        .expect("outcome-group fixture must pass production validation");
    group
}

fn outcome_leg(fixture: &ScanFixture, leg: &LegFixture) -> OutcomeLeg {
    OutcomeLeg {
        leg_id: leg.leg_id.clone(),
        instrument_id: InstrumentId::from(leg.instrument_id.as_str()),
        native_leg_id: leg.native_leg_id.clone(),
        settlement_asset_id: fixture.settlement_asset_id.clone(),
        outcome_label: leg.outcome_label.clone(),
        side_label: leg.side_label.clone(),
        leg_role: OutcomeLegRole::PaysOnTerminalState(leg.terminal_state_id.clone()),
        price_scale: NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id: fixture.settlement_asset_id.clone(),
            payout_per_contract: decimal(&fixture.payout_per_contract),
            price_units_per_payout: decimal(&fixture.price_units_per_payout),
            assertion_source: PriceScaleAssertionSource::VenueStructuredFields {
                proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                    ["price_scale", "native_leg_id"],
                    &leg.native_leg_id,
                )]),
            },
        },
        order_constraints: OutcomeLegOrderConstraints {
            min_quantity: decimal(&fixture.min_quantity),
            min_notional: Some(decimal(&fixture.min_notional)),
            quantity_step: decimal(&fixture.quantity_step),
            constraint_source: OrderConstraintSource::ConfigFloorWithNtPrecision {
                source_id: fixture.freshness_source_id.clone(),
            },
        },
    }
}

fn decimal(value: &str) -> Decimal {
    Decimal::from_str_exact(value).expect("benchmark decimal fixture must be valid")
}

fn order_side(value: &str) -> OrderSide {
    match value {
        "buy" => OrderSide::Buy,
        value => panic!("unsupported benchmark order side: {value}"),
    }
}
