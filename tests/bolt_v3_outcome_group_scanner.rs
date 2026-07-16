use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_outcome_group_proofs::{NegRiskGroupingProof, PolymarketDiscoveryScopeEvidence},
    bolt_v3_outcome_group_scanner::{
        OutcomeGroupCandidateLeg, OutcomeGroupDepthSnapshot, OutcomeGroupScanBlockReason,
        OutcomeGroupScanInput, scan_outcome_group_candidate,
    },
    bolt_v3_outcome_groups::{
        AttestedLegRef, AttestedPayoutVector, CanonicalField, GroupingProof,
        NormalizedPriceScaleEvidence, OrderConstraintSource, OutcomeGroup, OutcomeGroupSourceKind,
        OutcomeLeg, OutcomeLegOrderConstraints, OutcomeLegRole, PositiveSideBinding,
        PriceScaleAssertionSource, RoleBindingProof, SettlementRules, SettlementSourceKind,
        TerminalPayoutDerivation, TerminalState, TerminalStateConvention, TerminalStateKind,
        ValidatedOutcomeGroup, build_leg_map, canonical_fingerprint, derive_standard_payout_matrix,
        expected_metadata_fingerprint, payout_vector_attestation_sha256,
        role_binding_attestation_sha256,
    },
};
use nautilus_model::{
    data::{BookOrder, DEPTH10_LEN, OrderBookDepth10},
    enums::{BookType, OrderSide},
    identifiers::{InstrumentId, Venue},
    orderbook::{BookLevel, OrderBook},
    types::{Price, Quantity},
};
use rust_decimal::Decimal;

fn decimal_literal(value: &str) -> Decimal {
    Decimal::from_str_exact(&value.replace('_', "")).expect("decimal literal should parse")
}

macro_rules! dec {
    ($($value:tt)+) => {
        decimal_literal(stringify!($($value)+))
    };
}

#[test]
fn scanner_accepts_all_true_complete_set_when_edge_exceeds_threshold() {
    let group = fixture_group();
    let result = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![
            candidate("home-positive", dec!(0.4)),
            candidate("away-positive", dec!(0.4)),
        ],
        books([
            book("home-positive", "0.39", "20", "0.40", "20", Some(1_000)),
            book("away-positive", "0.39", "20", "0.40", "20", Some(1_000)),
        ]),
    ));

    assert!(result.admissible);
    assert_eq!(result.block_reason, None);
    assert_eq!(result.guaranteed_payout.round_dp(6), dec!(1));
    assert_eq!(result.total_adjusted_cost.round_dp(6), dec!(0.808));
    assert_eq!(result.absolute_edge.round_dp(6), dec!(0.192));
    assert_eq!(result.edge_bps.round_dp(0), dec!(2376));
    assert_eq!(
        result
            .state_payouts
            .get("void_refund")
            .expect("void payout row"),
        &dec!(2)
    );
}

#[test]
fn scanner_evaluates_all_false_and_mixed_role_baskets_from_payout_matrix() {
    let group = fixture_group();
    let all_false = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![
            candidate("home-negative", dec!(0.3)),
            candidate("away-negative", dec!(0.3)),
        ],
        books([
            book("home-negative", "0.29", "20", "0.30", "20", Some(1_000)),
            book("away-negative", "0.29", "20", "0.30", "20", Some(1_000)),
        ]),
    ));
    assert!(all_false.admissible);
    assert_eq!(all_false.guaranteed_payout.round_dp(6), dec!(1));
    assert_eq!(all_false.absolute_edge.round_dp(6), dec!(0.394));

    let mixed = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![
            candidate("home-positive", dec!(0.4)),
            candidate("away-negative", dec!(0.3)),
        ],
        books([
            book("home-positive", "0.39", "20", "0.40", "20", Some(1_000)),
            book("away-negative", "0.29", "20", "0.30", "20", Some(1_000)),
        ]),
    ));
    assert!(!mixed.admissible);
    assert_eq!(
        mixed.block_reason,
        Some(OutcomeGroupScanBlockReason::NonPositiveEdge)
    );
    assert_eq!(mixed.guaranteed_payout.round_dp(6), dec!(0));
}

#[test]
fn scanner_adapts_nt_depth10_order_book_and_book_levels_without_persistent_book() {
    let instrument_id = instrument_id("home-positive");
    let depth = depth10(instrument_id, "0.39", "3", "0.40", "3", 1_000_000);
    let depth_snapshot =
        OutcomeGroupDepthSnapshot::from_depth10(&depth).expect("depth10 should adapt");
    assert_eq!(depth_snapshot.observed_unix_ms, Some(1));
    assert_eq!(depth_snapshot.best_ask, Some(0.40));

    let mut order_book = OrderBook::new(instrument_id, BookType::L2_MBP);
    order_book.add(
        BookOrder::new(OrderSide::Buy, Price::from("0.39"), Quantity::from("3"), 1),
        0,
        1,
        2_000_000.into(),
    );
    order_book.add(
        BookOrder::new(OrderSide::Sell, Price::from("0.40"), Quantity::from("3"), 2),
        0,
        2,
        2_000_000.into(),
    );
    let order_book_snapshot =
        OutcomeGroupDepthSnapshot::from_order_book(&order_book).expect("OrderBook should adapt");
    assert_eq!(order_book_snapshot.observed_unix_ms, Some(2));
    assert_eq!(order_book_snapshot.best_bid, Some(0.39));

    let bid_level = BookLevel::from_order(BookOrder::new(
        OrderSide::Buy,
        Price::from("0.39"),
        Quantity::from("3"),
        3,
    ));
    let ask_level = BookLevel::from_order(BookOrder::new(
        OrderSide::Sell,
        Price::from("0.40"),
        Quantity::from("3"),
        4,
    ));
    let level_snapshot = OutcomeGroupDepthSnapshot::from_book_levels(
        instrument_id,
        Some(3),
        vec![bid_level],
        vec![ask_level],
    )
    .expect("BookLevel sides should adapt");
    assert_eq!(level_snapshot.observed_unix_ms, Some(3));
    assert_eq!(level_snapshot.best_ask, Some(0.40));
}

#[test]
fn scanner_blocks_insufficient_depth_stale_books_and_missing_timestamps() {
    let group = fixture_group();
    let insufficient = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "0.5",
            Some(1_000),
        )]),
    ));
    assert_eq!(
        insufficient.block_reason,
        Some(OutcomeGroupScanBlockReason::InsufficientDepth)
    );

    let stale = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book("home-positive", "0.39", "20", "0.40", "20", Some(100))]),
    ));
    assert_eq!(
        stale.block_reason,
        Some(OutcomeGroupScanBlockReason::StaleBook)
    );

    let future_timestamp = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_251),
        )]),
    ));
    assert_eq!(
        future_timestamp.block_reason,
        Some(OutcomeGroupScanBlockReason::StaleBook)
    );

    let missing_timestamp = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book("home-positive", "0.39", "20", "0.40", "20", None)]),
    ));
    assert_eq!(
        missing_timestamp.block_reason,
        Some(OutcomeGroupScanBlockReason::MissingBookTimestamp)
    );
}

#[test]
fn scanner_applies_fee_slippage_and_minimum_depth_sizing() {
    let group = fixture_group();
    let result = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(100)),
        vec![
            candidate("home-positive", dec!(0.4)),
            candidate("away-positive", dec!(0.4)),
        ],
        books([
            book("home-positive", "0.39", "20", "0.40", "20", Some(1_000)),
            book("away-positive", "0.39", "20", "0.40", "20", Some(1_000)),
        ]),
    ));

    assert!(result.admissible);
    assert_eq!(result.total_fee_cost.round_dp(6), dec!(0.008));
    assert_eq!(result.total_slippage_buffer.round_dp(6), dec!(0.008));
    assert_eq!(result.total_adjusted_cost.round_dp(6), dec!(0.816));
    assert_eq!(result.min_depth_quantity.round_dp(6), dec!(1));
}

#[test]
fn scanner_rejects_order_constraints_and_fee_boundaries() {
    let mut group = fixture_group();
    group
        .tradable_legs
        .get_mut("home-positive")
        .expect("leg")
        .order_constraints
        .min_quantity = dec!(2);
    let min_quantity = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));
    assert_eq!(
        min_quantity.block_reason,
        Some(OutcomeGroupScanBlockReason::MinQuantity)
    );

    let mut group = fixture_group();
    group
        .tradable_legs
        .get_mut("home-positive")
        .expect("leg")
        .order_constraints
        .quantity_step = dec!(2);
    let quantity_step = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));
    assert_eq!(
        quantity_step.block_reason,
        Some(OutcomeGroupScanBlockReason::QuantityStep)
    );

    let missing_fee = scan_outcome_group_candidate(scan_input(
        &fixture_group(),
        BTreeMap::new(),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));
    assert_eq!(
        missing_fee.block_reason,
        Some(OutcomeGroupScanBlockReason::FeeUnavailable)
    );
}

#[test]
fn scanner_rejects_non_positive_cost_edge_threshold_and_invalid_price_scale() {
    let group = fixture_group();
    let threshold = scan_outcome_group_candidate(OutcomeGroupScanInput {
        min_edge_bps: dec!(3_000),
        ..scan_input(
            &group,
            fees(&group, dec!(0)),
            vec![
                candidate("home-positive", dec!(0.4)),
                candidate("away-positive", dec!(0.4)),
            ],
            books([
                book("home-positive", "0.39", "20", "0.40", "20", Some(1_000)),
                book("away-positive", "0.39", "20", "0.40", "20", Some(1_000)),
            ]),
        )
    });
    assert_eq!(
        threshold.block_reason,
        Some(OutcomeGroupScanBlockReason::EdgeThreshold)
    );

    let invalid_cost = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));
    assert_eq!(
        invalid_cost.block_reason,
        Some(OutcomeGroupScanBlockReason::InvalidCost)
    );

    let mut invalid_scale = fixture_group();
    let leg = invalid_scale
        .tradable_legs
        .get_mut("home-positive")
        .expect("leg");
    leg.price_scale = NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
        settlement_asset_id: "USDC".to_string(),
        payout_per_contract: dec!(0),
        price_units_per_payout: dec!(1),
        assertion_source: PriceScaleAssertionSource::VenueStructuredFields {
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["bad", "scale"],
                "zero",
            )]),
        },
    };
    let invalid_scale = scan_outcome_group_candidate(scan_input(
        &invalid_scale,
        fees(&invalid_scale, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));
    assert_eq!(
        invalid_scale.block_reason,
        Some(OutcomeGroupScanBlockReason::InvalidPriceScale)
    );
}

#[test]
fn scanner_rejects_sell_candidates_before_payout_evaluation() {
    let group = fixture_group();
    let sell_candidate = OutcomeGroupCandidateLeg {
        order_side: OrderSide::Sell,
        ..candidate("home-positive", dec!(0.4))
    };
    let result = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![sell_candidate],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));

    assert!(!result.admissible);
    assert_eq!(
        result.block_reason,
        Some(OutcomeGroupScanBlockReason::UnsupportedOrderSide)
    );
    assert!(
        result.leg_costs.is_empty(),
        "sell candidates must not reach pricing or payout evaluation"
    );
}

#[test]
fn scanner_rejects_incomplete_candidate_baskets_before_payout_evaluation() {
    let group = fixture_group();
    let result = scan_outcome_group_candidate(scan_input(
        &group,
        fees(&group, dec!(0)),
        vec![candidate("home-positive", dec!(0.4))],
        books([book(
            "home-positive",
            "0.39",
            "20",
            "0.40",
            "20",
            Some(1_000),
        )]),
    ));

    assert!(!result.admissible);
    assert_eq!(
        result.block_reason,
        Some(OutcomeGroupScanBlockReason::IncompleteCandidate)
    );
    assert!(
        result.state_payouts.is_empty(),
        "incomplete candidates must not treat missing legs as zero-quantity payout inputs"
    );
}

fn scan_input<'a>(
    group: &'a OutcomeGroup,
    fee_bps: BTreeMap<InstrumentId, Decimal>,
    candidate_legs: Vec<OutcomeGroupCandidateLeg>,
    books: BTreeMap<InstrumentId, OutcomeGroupDepthSnapshot>,
) -> OutcomeGroupScanInput<'a> {
    OutcomeGroupScanInput {
        group,
        candidate_legs,
        books,
        fee_bps,
        now_unix_ms: 1_000,
        max_book_age_ms: 100,
        min_edge_bps: dec!(100),
        vwap_depth_limit_bps: 2_000,
        slippage_buffer_bps: 100,
    }
}

fn fees(group: &OutcomeGroup, fee_bps: Decimal) -> BTreeMap<InstrumentId, Decimal> {
    group
        .tradable_legs
        .values()
        .map(|leg| (leg.instrument_id, fee_bps))
        .collect()
}

fn candidate(leg_id: &str, target_notional: Decimal) -> OutcomeGroupCandidateLeg {
    OutcomeGroupCandidateLeg {
        leg_id: leg_id.to_string(),
        order_side: OrderSide::Buy,
        target_notional,
    }
}

fn books<const N: usize>(
    snapshots: [OutcomeGroupDepthSnapshot; N],
) -> BTreeMap<InstrumentId, OutcomeGroupDepthSnapshot> {
    snapshots
        .into_iter()
        .map(|snapshot| (snapshot.instrument_id, snapshot))
        .collect()
}

fn book(
    leg_id: &str,
    bid_price: &str,
    bid_size: &str,
    ask_price: &str,
    ask_size: &str,
    observed_unix_ms: Option<u64>,
) -> OutcomeGroupDepthSnapshot {
    OutcomeGroupDepthSnapshot::from_book_levels(
        instrument_id(leg_id),
        observed_unix_ms,
        vec![BookLevel::from_order(BookOrder::new(
            OrderSide::Buy,
            Price::from(bid_price),
            Quantity::from(bid_size),
            1,
        ))],
        vec![BookLevel::from_order(BookOrder::new(
            OrderSide::Sell,
            Price::from(ask_price),
            Quantity::from(ask_size),
            2,
        ))],
    )
    .expect("test book levels should adapt")
}

fn depth10(
    instrument_id: InstrumentId,
    bid_price: &str,
    bid_size: &str,
    ask_price: &str,
    ask_size: &str,
    ts_event_nanos: u64,
) -> OrderBookDepth10 {
    let bid = BookOrder::new(
        OrderSide::Buy,
        Price::from(bid_price),
        Quantity::from(bid_size),
        1,
    );
    let ask = BookOrder::new(
        OrderSide::Sell,
        Price::from(ask_price),
        Quantity::from(ask_size),
        2,
    );
    OrderBookDepth10::new(
        instrument_id,
        [bid; DEPTH10_LEN],
        [ask; DEPTH10_LEN],
        [1; DEPTH10_LEN],
        [1; DEPTH10_LEN],
        0,
        1,
        ts_event_nanos.into(),
        ts_event_nanos.into(),
    )
}

fn fixture_group() -> OutcomeGroup {
    let mut terminal_states = BTreeMap::new();
    for state in ["home", "away"] {
        terminal_states.insert(
            state.to_string(),
            TerminalState {
                state_id: state.to_string(),
                label: state.to_string(),
                kind: TerminalStateKind::Standard,
            },
        );
    }
    terminal_states.insert(
        "void_refund".to_string(),
        TerminalState {
            state_id: "void_refund".to_string(),
            label: "void_refund".to_string(),
            kind: TerminalStateKind::Void,
        },
    );

    let legs = build_leg_map(vec![
        leg(
            "home-positive",
            "home",
            "true",
            OutcomeLegRole::PaysOnTerminalState("home".to_string()),
        ),
        leg(
            "away-positive",
            "away",
            "true",
            OutcomeLegRole::PaysOnTerminalState("away".to_string()),
        ),
        leg(
            "home-negative",
            "home",
            "false",
            OutcomeLegRole::PaysUnlessTerminalState("home".to_string()),
        ),
        leg(
            "away-negative",
            "away",
            "false",
            OutcomeLegRole::PaysUnlessTerminalState("away".to_string()),
        ),
    ])
    .expect("fixture leg ids are unique");

    let mut payout_matrix = derive_standard_payout_matrix(
        &terminal_states,
        &legs,
        TerminalStateConvention::ExactlyOneWinner,
    )
    .expect("standard payout matrix should derive");
    let void_cols = payout_matrix
        .cols
        .iter()
        .map(|leg_id| {
            let leg = &legs[leg_id];
            AttestedLegRef::OutcomeAndSide {
                outcome_label: leg.outcome_label.clone(),
                side_label: leg.side_label.clone(),
            }
        })
        .collect::<Vec<_>>();
    let void_payouts = vec![dec!(1); void_cols.len()];
    payout_matrix
        .payout_per_unit_by_state
        .insert("void_refund".to_string(), void_payouts.clone());
    let void_vector = AttestedPayoutVector {
        terminal_state_id: "void_refund".to_string(),
        label: "void_refund".to_string(),
        cols: void_cols.clone(),
        payouts: void_payouts.clone(),
        refund_convention: "operator_attested_static_payout_per_unit".to_string(),
        attestation_sha256: payout_vector_attestation_sha256(
            "void_refund",
            "void_refund",
            &void_cols,
            &void_payouts,
            "operator_attested_static_payout_per_unit",
        ),
    };
    let bindings = vec![
        PositiveSideBinding {
            terminal_state_label: "home".to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId("home-positive".to_string()),
            pays_unless_leg: AttestedLegRef::NativeLegId("home-negative".to_string()),
        },
        PositiveSideBinding {
            terminal_state_label: "away".to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId("away-positive".to_string()),
            pays_unless_leg: AttestedLegRef::NativeLegId("away-negative".to_string()),
        },
    ];
    let mut group = OutcomeGroup {
        group_id: "fixture-neg-risk".to_string(),
        source_client_id: "polymarket_main".into(),
        venue: Venue::from("POLYMARKET"),
        source_kind: OutcomeGroupSourceKind::Polymarket,
        settlement_asset_id: "USDC".to_string(),
        terminal_states,
        tradable_legs: legs,
        payout_matrix,
        grouping_proof: Some(GroupingProof::PolymarketNegRisk(NegRiskGroupingProof {
            neg_risk_market_id: "fixture-neg-risk".to_string(),
            discovery_scope: PolymarketDiscoveryScopeEvidence {
                source_id: "fixture-source".to_string(),
                event_slugs: Vec::new(),
                market_slugs: Vec::new(),
                gamma_query_fingerprint: None,
                cache_key_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                    ["cache_key", "fixture"],
                    "scanner",
                )]),
            },
            market_slugs: vec!["home-market".to_string(), "away-market".to_string()],
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["grouping", "neg_risk_market_id"],
                "fixture-neg-risk",
            )]),
        })),
        role_binding_proof: Some(RoleBindingProof::OperatorAttested {
            attestation_id: "fixture-source".to_string(),
            positive_side_bindings: bindings.clone(),
            attestation_sha256: role_binding_attestation_sha256(&bindings),
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["role_binding", "source_id"],
                "fixture-source",
            )]),
        }),
        settlement_rules: SettlementRules {
            terminal_state_convention: TerminalStateConvention::ExactlyOneWinner,
            settlement_source_kind: SettlementSourceKind::VenueStructuredFields,
            non_standard_terminal_payouts: vec![void_vector],
            terminal_payout_derivation: TerminalPayoutDerivation::StandardRowsPlusAttestedVectors,
        },
        freshness_source_id: "fixture-source".to_string(),
        metadata_fingerprint: String::new(),
    };
    group.metadata_fingerprint = expected_metadata_fingerprint(&group);
    ValidatedOutcomeGroup::validate(&group).expect("fixture group should validate");
    group
}

fn leg(leg_id: &str, outcome_label: &str, side_label: &str, role: OutcomeLegRole) -> OutcomeLeg {
    OutcomeLeg {
        leg_id: leg_id.to_string(),
        instrument_id: instrument_id(leg_id),
        native_leg_id: leg_id.to_string(),
        settlement_asset_id: "USDC".to_string(),
        outcome_label: outcome_label.to_string(),
        side_label: side_label.to_string(),
        leg_role: role,
        price_scale: NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id: "USDC".to_string(),
            payout_per_contract: dec!(1),
            price_units_per_payout: dec!(1),
            assertion_source: PriceScaleAssertionSource::VenueStructuredFields {
                proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                    ["price_scale", "native_leg_id"],
                    leg_id,
                )]),
            },
        },
        order_constraints: OutcomeLegOrderConstraints {
            min_quantity: dec!(1),
            min_notional: Some(dec!(0.1)),
            quantity_step: dec!(1),
            constraint_source: OrderConstraintSource::ConfigFloorWithNtPrecision {
                source_id: "fixture-source".to_string(),
            },
        },
    }
}

fn instrument_id(leg_id: &str) -> InstrumentId {
    InstrumentId::from(format!("{leg_id}.POLYMARKET"))
}
