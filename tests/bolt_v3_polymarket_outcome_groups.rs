use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::GateProviderFreshnessBlock,
    bolt_v3_outcome_group_polymarket::{
        PolymarketGammaLegMetadata, PolymarketGammaMarketMetadata, PolymarketOutcomeGroupInput,
        normalize_polymarket_outcome_group,
    },
    bolt_v3_outcome_group_proofs::NegRiskGroupingProof,
    bolt_v3_outcome_group_sources::{
        GammaQueryBlock, OutcomeGroupNonStandardPayoutLegBlock,
        OutcomeGroupNonStandardTerminalPayoutBlock, OutcomeGroupOrderConstraintsBlock,
        OutcomeGroupRefundConvention, OutcomeGroupRoleBindingKind, OutcomeGroupRoleBindingLegBlock,
        OutcomeGroupRoleBindingsBlock, OutcomeGroupRoundingPolicy,
        OutcomeGroupSettlementRulesBlock, OutcomeGroupSettlementSourceKind,
        OutcomeGroupSourceConfig, OutcomeGroupSourceKind as SourceConfigKind,
        OutcomeGroupTerminalStateConvention, OutcomeGroupTimingPolicy, OutcomeGroupVoidPolicy,
    },
    bolt_v3_outcome_groups::{
        AttestedLegRef, GroupingProof, OutcomeGroupSourceKind, OutcomeLegRole, PositiveSideBinding,
        RoleBindingProof, TerminalStateKind, ValidatedOutcomeGroup, is_lowercase_sha256,
        payout_vector_attestation_sha256, role_binding_attestation_sha256,
    },
};
use nautilus_model::identifiers::{ClientId, InstrumentId};
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
fn polymarket_normalizer_builds_three_way_neg_risk_group_from_attested_roles() {
    let source = event_source();
    let group = normalize_polymarket_outcome_group(valid_input(&source))
        .expect("valid synthetic Gamma metadata should normalize");

    ValidatedOutcomeGroup::validate(&group).expect("normalizer output should satisfy shared model");
    assert_eq!(group.source_kind, OutcomeGroupSourceKind::Polymarket);
    assert_eq!(group.source_client_id, ClientId::from("polymarket_main"));
    assert_eq!(group.terminal_states.len(), 4);
    assert_eq!(
        group.terminal_states["home"].kind,
        TerminalStateKind::Standard
    );
    assert_eq!(
        group.terminal_states["void_refund"].kind,
        TerminalStateKind::Void
    );
    assert_eq!(
        group.tradable_legs.keys().cloned().collect::<Vec<_>>(),
        vec![
            "away-negative-token".to_string(),
            "away-positive-token".to_string(),
            "draw-negative-token".to_string(),
            "draw-positive-token".to_string(),
            "home-negative-token".to_string(),
            "home-positive-token".to_string(),
        ],
        "both sides of every native market must be preserved as tradable legs"
    );

    assert!(matches!(
        group.tradable_legs["home-positive-token"].leg_role,
        OutcomeLegRole::PaysOnTerminalState(ref state) if state == "home"
    ));
    assert!(matches!(
        group.tradable_legs["home-negative-token"].leg_role,
        OutcomeLegRole::PaysUnlessTerminalState(ref state) if state == "home"
    ));
    assert_eq!(
        group.payout_matrix.payout_per_unit_by_state["home"].len(),
        6
    );

    let GroupingProof::PolymarketNegRisk(NegRiskGroupingProof {
        neg_risk_market_id,
        discovery_scope,
        market_slugs,
        ..
    }) = group
        .grouping_proof
        .as_ref()
        .expect("grouping proof should exist")
    else {
        panic!("expected Polymarket neg-risk grouping proof");
    };
    assert_eq!(neg_risk_market_id, "neg-risk-123");
    assert_eq!(
        discovery_scope.event_slugs,
        vec!["world-cup-final".to_string()]
    );
    assert!(is_lowercase_sha256(&discovery_scope.cache_key_fingerprint));
    assert_eq!(
        market_slugs,
        &vec![
            "away-market".to_string(),
            "draw-market".to_string(),
            "home-market".to_string(),
        ]
    );

    let RoleBindingProof::OperatorAttested {
        positive_side_bindings,
        ..
    } = group
        .role_binding_proof
        .as_ref()
        .expect("role binding proof should exist")
    else {
        panic!("expected Polymarket operator-attested role binding proof");
    };
    assert_eq!(positive_side_bindings.len(), 3);
    assert!(
        group
            .tradable_legs
            .values()
            .all(|leg| leg.side_label != "Yes" && leg.side_label != "No")
    );
}

#[test]
fn polymarket_normalizer_allows_market_slug_and_gamma_query_sources_without_event_membership() {
    for source in [market_slug_source(), gamma_query_source()] {
        let group = normalize_polymarket_outcome_group(valid_input(&source))
            .expect("event membership is not required grouping proof");
        let GroupingProof::PolymarketNegRisk(NegRiskGroupingProof {
            discovery_scope, ..
        }) = group.grouping_proof.as_ref().expect("grouping proof")
        else {
            panic!("expected Polymarket neg-risk grouping proof");
        };
        assert!(
            discovery_scope.event_slugs.is_empty(),
            "event membership should be optional discovery evidence"
        );
        assert!(is_lowercase_sha256(&discovery_scope.cache_key_fingerprint));
        assert_eq!(group.tradable_legs.len(), 6);
    }
}

#[test]
fn polymarket_normalizer_rejects_missing_or_conflicting_neg_risk_metadata() {
    let source = event_source();
    let mut missing = valid_markets();
    missing[1].neg_risk_market_id = None;
    let error = normalize_polymarket_outcome_group(input_with_markets(&source, missing))
        .expect_err("missing negRiskMarketID must reject before grouping by slug");
    assert!(error.is_missing_neg_risk_market_id());

    let mut conflicting = valid_markets();
    conflicting[2].neg_risk_market_id = Some("other-neg-risk".to_string());
    let error = normalize_polymarket_outcome_group(input_with_markets(&source, conflicting))
        .expect_err("mixed negRiskMarketID values must reject");
    assert!(error.is_grouping_identity_conflict());

    let mismatch = source_with_expected_neg_risk("operator-expected-id");
    let error = normalize_polymarket_outcome_group(valid_input(&mismatch))
        .expect_err("configured expected neg-risk id is only a checked expectation");
    assert!(error.is_expected_neg_risk_mismatch());
}

#[test]
fn polymarket_normalizer_rejects_role_binding_and_terminal_state_mismatches() {
    let mut missing_binding = event_source();
    missing_binding
        .role_bindings
        .as_mut()
        .expect("role bindings")
        .legs
        .retain(|leg| leg.terminal_state_label != "draw");
    let error = normalize_polymarket_outcome_group(valid_input(&missing_binding))
        .expect_err("every standard terminal state must have one role binding");
    assert!(error.is_role_binding_mismatch());

    let mut rekeyed = event_source();
    rekeyed.role_bindings.as_mut().expect("role bindings").legs[0]
        .pays_on_terminal_state_native_leg_id = "home-rekeyed-token".to_string();
    let error = normalize_polymarket_outcome_group(valid_input(&rekeyed))
        .expect_err("attested native leg ids must be non-rekeyable");
    assert!(error.is_unmapped_role_binding());

    let mut extra_label = event_source();
    extra_label
        .terminal_state_labels
        .as_mut()
        .expect("terminal labels")
        .push("extra".to_string());
    let error = normalize_polymarket_outcome_group(valid_input(&extra_label))
        .expect_err("configured terminal labels must match Gamma outcomes exactly");
    assert!(error.is_terminal_state_mismatch());
}

#[test]
fn polymarket_normalizer_rejects_missing_void_policy_and_stale_metadata() {
    let mut missing_void = event_source();
    missing_void
        .settlement_rules
        .as_mut()
        .expect("settlement rules")
        .non_standard_terminal_payouts = None;
    let error = normalize_polymarket_outcome_group(valid_input(&missing_void))
        .expect_err("void or fallback payout vector is required");
    assert!(error.is_missing_non_standard_payout());

    let source = event_source();
    let mut stale = valid_input(&source);
    stale.now_unix_ms = 5_001;
    stale.metadata_ttl_ms = 4_000;
    let error = normalize_polymarket_outcome_group(stale)
        .expect_err("Gamma metadata older than metadata TTL must reject");
    assert!(error.is_stale_metadata());

    let mut future = valid_input(&source);
    future.metadata_loaded_unix_ms = 2_251;
    let error = normalize_polymarket_outcome_group(future)
        .expect_err("Gamma metadata beyond configured clock skew must reject");
    assert!(error.is_stale_metadata());
}

fn valid_input(source: &OutcomeGroupSourceConfig) -> PolymarketOutcomeGroupInput<'_> {
    input_with_markets(source, valid_markets())
}

fn input_with_markets(
    source: &OutcomeGroupSourceConfig,
    markets: Vec<PolymarketGammaMarketMetadata>,
) -> PolymarketOutcomeGroupInput<'_> {
    PolymarketOutcomeGroupInput {
        source,
        metadata_loaded_unix_ms: 1_000,
        now_unix_ms: 2_000,
        metadata_ttl_ms: 4_000,
        markets,
    }
}

fn event_source() -> OutcomeGroupSourceConfig {
    source(SourceConfigKind::GammaEvent)
}

fn market_slug_source() -> OutcomeGroupSourceConfig {
    source(SourceConfigKind::GammaMarketSlug)
}

fn gamma_query_source() -> OutcomeGroupSourceConfig {
    source(SourceConfigKind::GammaQuery)
}

fn source_with_expected_neg_risk(expected: &str) -> OutcomeGroupSourceConfig {
    let mut source = event_source();
    source.expected_neg_risk_market_id = Some(expected.to_string());
    source
}

fn source(kind: SourceConfigKind) -> OutcomeGroupSourceConfig {
    let terminal_state_labels = states().into_iter().map(str::to_string).collect::<Vec<_>>();
    let role_legs = role_binding_legs();
    let role_attestation = role_binding_attestation_sha256(&positive_side_bindings());
    let payout_cols = payout_cols();
    let payout_values = vec![dec!(1); payout_cols.len()];
    let payout_attestation = payout_vector_attestation_sha256(
        "void_refund",
        "void_refund",
        &payout_cols,
        &payout_values,
        "operator_attested_static_payout_per_unit",
    );
    let mut payouts = BTreeMap::new();
    payouts.insert(
        "void_refund".to_string(),
        OutcomeGroupNonStandardTerminalPayoutBlock {
            convention: OutcomeGroupRefundConvention::OperatorAttestedStaticPayoutPerUnit,
            terminal_state_label: "void_refund".to_string(),
            legs: payout_cols
                .iter()
                .zip(payout_values.iter())
                .map(|(col, payout)| match col {
                    AttestedLegRef::OutcomeAndSide {
                        outcome_label,
                        side_label,
                    } => OutcomeGroupNonStandardPayoutLegBlock {
                        outcome_label: outcome_label.clone(),
                        side_label: side_label.clone(),
                        payout_per_unit: payout.to_string(),
                    },
                    AttestedLegRef::NativeLegId(_) => {
                        panic!("test payout columns use outcome/side references")
                    }
                })
                .collect(),
            attestation_sha256: payout_attestation,
        },
    );
    let (event_slugs, market_slugs, gamma_query) = match kind {
        SourceConfigKind::GammaEvent => (Some(vec!["world-cup-final".to_string()]), None, None),
        SourceConfigKind::GammaMarketSlug => (
            None,
            Some(vec![
                "home-market".to_string(),
                "draw-market".to_string(),
                "away-market".to_string(),
            ]),
            None,
        ),
        SourceConfigKind::GammaQuery => (
            None,
            None,
            Some(GammaQueryBlock {
                search: Some("configured event search".to_string()),
                event_query: None,
                market_query: None,
                tag_id: Some("sports-tag".to_string()),
                sports_market_types: Some(vec!["moneyline".to_string()]),
                max_events: Some(1),
                max_markets: 3,
            }),
        ),
        SourceConfigKind::Hip4 => panic!("Polymarket test source cannot be HIP-4"),
    };
    OutcomeGroupSourceConfig {
        source_id: "poly_world_cup".to_string(),
        client_id: ClientId::from("polymarket_main"),
        kind,
        event_slugs,
        market_slugs,
        sports_market_types: Some(vec!["moneyline".to_string()]),
        gamma_query,
        question: None,
        expected_neg_risk_market_id: Some("neg-risk-123".to_string()),
        terminal_state_labels: Some(terminal_state_labels),
        max_markets: Some(3),
        max_groups: None,
        enabled: true,
        freshness: Some(GateProviderFreshnessBlock {
            max_age_ms: Some(500),
            max_clock_skew_ms: Some(250),
        }),
        order_constraints: Some(OutcomeGroupOrderConstraintsBlock {
            default_min_quantity: Some("5".to_string()),
            default_min_notional: Some("1".to_string()),
            per_leg: None,
        }),
        role_bindings: Some(OutcomeGroupRoleBindingsBlock {
            kind: OutcomeGroupRoleBindingKind::OperatorAttestedPositiveSide,
            attestation_sha256: role_attestation,
            legs: role_legs,
        }),
        settlement_rules: Some(OutcomeGroupSettlementRulesBlock {
            settlement_contract_id: "ctf-world-cup-final".to_string(),
            settlement_source_kind: OutcomeGroupSettlementSourceKind::CtfUma,
            terminal_state_convention: OutcomeGroupTerminalStateConvention::ExactlyOneWinner,
            void_policy: OutcomeGroupVoidPolicy::RefundAllLegs,
            rounding_policy: OutcomeGroupRoundingPolicy::DecimalExact,
            timing_policy: OutcomeGroupTimingPolicy::VenueFinalResolution,
            attestation_sha256: hash("settlement"),
            non_standard_terminal_payouts: Some(payouts),
        }),
    }
}

fn valid_markets() -> Vec<PolymarketGammaMarketMetadata> {
    states()
        .into_iter()
        .map(|state| PolymarketGammaMarketMetadata {
            condition_id: format!("{state}-condition"),
            market_slug: format!("{state}-market"),
            question: format!("{state} terminal state market"),
            terminal_state_label: state.to_string(),
            neg_risk_market_id: Some("neg-risk-123".to_string()),
            legs: vec![
                PolymarketGammaLegMetadata {
                    native_leg_id: format!("{state}-positive-token"),
                    instrument_id: InstrumentId::from(format!("{state}-positive-token.POLYMARKET")),
                    outcome_label: state.to_string(),
                    side_label: format!("supports-{state}"),
                    settlement_asset_id: "USDC".to_string(),
                    quantity_step: dec!(1),
                    payout_per_contract: dec!(1),
                    price_units_per_payout: dec!(1),
                },
                PolymarketGammaLegMetadata {
                    native_leg_id: format!("{state}-negative-token"),
                    instrument_id: InstrumentId::from(format!("{state}-negative-token.POLYMARKET")),
                    outcome_label: state.to_string(),
                    side_label: format!("opposes-{state}"),
                    settlement_asset_id: "USDC".to_string(),
                    quantity_step: dec!(1),
                    payout_per_contract: dec!(1),
                    price_units_per_payout: dec!(1),
                },
            ],
        })
        .collect()
}

fn states() -> Vec<&'static str> {
    vec!["home", "draw", "away"]
}

fn role_binding_legs() -> Vec<OutcomeGroupRoleBindingLegBlock> {
    states()
        .into_iter()
        .map(|state| OutcomeGroupRoleBindingLegBlock {
            terminal_state_label: state.to_string(),
            pays_on_terminal_state_native_leg_id: format!("{state}-positive-token"),
            pays_unless_terminal_state_native_leg_id: format!("{state}-negative-token"),
        })
        .collect()
}

fn positive_side_bindings() -> Vec<PositiveSideBinding> {
    states()
        .into_iter()
        .map(|state| PositiveSideBinding {
            terminal_state_label: state.to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId(format!("{state}-positive-token")),
            pays_unless_leg: AttestedLegRef::NativeLegId(format!("{state}-negative-token")),
        })
        .collect()
}

fn payout_cols() -> Vec<AttestedLegRef> {
    ["away", "draw", "home"]
        .into_iter()
        .flat_map(|state| {
            [
                AttestedLegRef::OutcomeAndSide {
                    outcome_label: state.to_string(),
                    side_label: format!("opposes-{state}"),
                },
                AttestedLegRef::OutcomeAndSide {
                    outcome_label: state.to_string(),
                    side_label: format!("supports-{state}"),
                },
            ]
        })
        .collect()
}

fn hash(label: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(label.as_bytes()))
}
