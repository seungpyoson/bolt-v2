use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::GateProviderFreshnessBlock,
    bolt_v3_outcome_group_hyperliquid::{
        HyperliquidHip4OutcomeGroupError, HyperliquidHip4OutcomeGroupInput,
        normalize_hyperliquid_hip4_outcome_group,
    },
    bolt_v3_outcome_group_proofs::StructuredOutcomeGroupingProof,
    bolt_v3_outcome_group_sources::{
        OutcomeGroupNonStandardPayoutLegBlock, OutcomeGroupNonStandardTerminalPayoutBlock,
        OutcomeGroupOrderConstraintsBlock, OutcomeGroupRefundConvention,
        OutcomeGroupRoundingPolicy, OutcomeGroupSettlementRulesBlock,
        OutcomeGroupSettlementSourceKind, OutcomeGroupSourceConfig,
        OutcomeGroupSourceKind as SourceConfigKind, OutcomeGroupTerminalStateConvention,
        OutcomeGroupTimingPolicy, OutcomeGroupVoidPolicy,
    },
    bolt_v3_outcome_groups::{
        AttestedLegRef, GroupingProof, OutcomeGroupSourceKind, OutcomeLegRole,
        PriceScaleAssertionSource, RoleBindingProof, TerminalStateKind, ValidatedOutcomeGroup,
        payout_vector_attestation_sha256,
    },
    bolt_v3_providers::hyperliquid,
};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    enums::AssetClass,
    identifiers::{ClientId, InstrumentId, Symbol},
    instruments::{BinaryOption, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use serde_json::json;
use ustr::Ustr;

#[test]
fn hyperliquid_normalizer_builds_question_group_from_nt_binary_option_info() {
    let source = hip4_source();
    let group = normalize_hyperliquid_hip4_outcome_group(valid_input(&source))
        .expect("valid synthetic NT HIP-4 BinaryOption metadata should normalize");

    ValidatedOutcomeGroup::validate(&group).expect("normalizer output should validate");
    assert_eq!(group.source_kind, OutcomeGroupSourceKind::Hyperliquid);
    assert_eq!(group.source_client_id, ClientId::from("hyperliquid_main"));
    assert_eq!(group.venue.as_str(), hyperliquid::KEY);
    assert_eq!(group.group_id, "hyperliquid:9001");
    assert_eq!(group.terminal_states.len(), 3);
    assert_eq!(
        group.terminal_states["home"].kind,
        TerminalStateKind::Standard
    );
    assert_eq!(
        group.terminal_states["fallback_refund"].kind,
        TerminalStateKind::Fallback
    );
    assert_eq!(group.tradable_legs.len(), 4);

    assert!(matches!(
        group.tradable_legs["HIP4-9001-101-0.HYPERLIQUID"].leg_role,
        OutcomeLegRole::PaysOnTerminalState(ref state) if state == "home"
    ));
    assert!(matches!(
        group.tradable_legs["HIP4-9001-101-1.HYPERLIQUID"].leg_role,
        OutcomeLegRole::PaysUnlessTerminalState(ref state) if state == "home"
    ));
    assert!(matches!(
        group.tradable_legs["HIP4-9001-102-0.HYPERLIQUID"].leg_role,
        OutcomeLegRole::PaysOnTerminalState(ref state) if state == "away"
    ));
    assert!(matches!(
        group.tradable_legs["HIP4-9001-102-1.HYPERLIQUID"].leg_role,
        OutcomeLegRole::PaysUnlessTerminalState(ref state) if state == "away"
    ));

    let GroupingProof::HyperliquidOutcome(StructuredOutcomeGroupingProof {
        question,
        outcome_indices,
        ..
    }) = group.grouping_proof.as_ref().expect("grouping proof")
    else {
        panic!("expected Hyperliquid outcome grouping proof");
    };
    assert_eq!(*question, 9001);
    assert_eq!(outcome_indices, &vec![101, 102]);

    let RoleBindingProof::VenueStructuredFields {
        question,
        outcome_indices,
        ..
    } = group
        .role_binding_proof
        .as_ref()
        .expect("role binding proof")
    else {
        panic!("expected venue-structured HIP-4 role binding proof");
    };
    assert_eq!(*question, 9001);
    assert_eq!(outcome_indices, &vec![101, 102]);

    for leg in group.tradable_legs.values() {
        match &leg.price_scale {
            bolt_v2::bolt_v3_outcome_groups::NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
                assertion_source,
                ..
            } => assert!(matches!(
                assertion_source,
                PriceScaleAssertionSource::VenueStructuredFields { .. }
            )),
        }
    }
}

#[test]
fn hyperliquid_normalizer_filters_loaded_surface_by_configured_question() {
    let source = hip4_source();
    let mut instruments = valid_instruments();
    instruments.extend(question_instruments(7777, [201, 202], ["red", "blue"]));

    let group = normalize_hyperliquid_hip4_outcome_group(HyperliquidHip4OutcomeGroupInput {
        source: &source,
        metadata_loaded_unix_ms: 1_000,
        now_unix_ms: 2_000,
        metadata_ttl_ms: 4_000,
        instruments,
    })
    .expect("configured question should be selected from a surface-wide HIP-4 load");

    assert_eq!(group.group_id, "hyperliquid:9001");
    assert_eq!(group.tradable_legs.len(), 4);
    assert!(
        group
            .tradable_legs
            .keys()
            .all(|leg_id| leg_id.contains("9001")),
        "normalizer must not leak instruments from unconfigured questions"
    );
}

#[test]
fn hyperliquid_normalizer_rejects_standalone_outcomes_without_parent_question() {
    let source = hip4_source();
    let error = normalize_hyperliquid_hip4_outcome_group(HyperliquidHip4OutcomeGroupInput {
        source: &source,
        metadata_loaded_unix_ms: 1_000,
        now_unix_ms: 2_000,
        metadata_ttl_ms: 4_000,
        instruments: vec![standalone_outcome_instrument(301, 0)],
    })
    .expect_err("HIP-4 source needs structured OutcomeQuestion metadata");

    assert!(error.is_missing_parent_question());
}

#[test]
fn hyperliquid_normalizer_rejects_future_metadata_beyond_configured_clock_skew() {
    let source = hip4_source();
    let mut input = valid_input(&source);
    input.metadata_loaded_unix_ms = 2_251;
    let error = normalize_hyperliquid_hip4_outcome_group(input)
        .expect_err("HIP-4 metadata beyond configured clock skew must reject");

    assert_eq!(error, HyperliquidHip4OutcomeGroupError::StaleMetadata);
}

#[test]
fn hyperliquid_provider_supports_outcome_group_only_through_hip4_surface() {
    assert!(
        hyperliquid::SUPPORTED_MARKET_FAMILIES
            .contains(&bolt_v2::bolt_v3_market_families::outcome_group::KEY),
        "Hyperliquid must explicitly opt in to outcome_group provider binding"
    );
    let hip4 = hyperliquid::hyperliquid_product_matrix()
        .iter()
        .find(|entry| entry.product_surface == hyperliquid::HyperliquidProductSurface::Hip4Outcomes)
        .expect("HIP-4 product matrix entry should exist");
    assert!(
        hip4.discovery_sources
            .contains(&"nautilus_hyperliquid::http::parse::parse_outcome_instruments"),
        "HIP-4 discovery should remain the NT adapter outcomeMeta parse path"
    );
}

fn valid_input(source: &OutcomeGroupSourceConfig) -> HyperliquidHip4OutcomeGroupInput<'_> {
    HyperliquidHip4OutcomeGroupInput {
        source,
        metadata_loaded_unix_ms: 1_000,
        now_unix_ms: 2_000,
        metadata_ttl_ms: 4_000,
        instruments: valid_instruments(),
    }
}

fn valid_instruments() -> Vec<InstrumentAny> {
    question_instruments(9001, [101, 102], ["home", "away"])
}

fn question_instruments(
    question: u32,
    outcome_indices: [u32; 2],
    terminal_labels: [&str; 2],
) -> Vec<InstrumentAny> {
    outcome_indices
        .into_iter()
        .zip(terminal_labels)
        .flat_map(|(outcome_index, terminal_label)| {
            [
                hip4_binary_option(
                    question,
                    outcome_index,
                    0,
                    terminal_label,
                    "supports",
                    false,
                ),
                hip4_binary_option(question, outcome_index, 1, terminal_label, "opposes", false),
            ]
        })
        .collect()
}

fn standalone_outcome_instrument(outcome_index: u32, outcome_side: u8) -> InstrumentAny {
    let mut info = Params::new();
    info.insert("outcome_index".to_string(), json!(outcome_index));
    info.insert("outcome_side".to_string(), json!(outcome_side));
    info.insert("market_name".to_string(), json!("standalone"));
    info.insert("side_name".to_string(), json!("standalone-side"));
    binary_option(
        InstrumentId::from(format!(
            "HIP4-STANDALONE-{outcome_index}-{outcome_side}.HYPERLIQUID"
        )),
        Some(info),
        "standalone",
        "standalone-side",
    )
}

fn hip4_binary_option(
    question: u32,
    outcome_index: u32,
    outcome_side: u8,
    terminal_label: &str,
    side_label: &str,
    is_fallback: bool,
) -> InstrumentAny {
    let mut info = Params::new();
    info.insert("question".to_string(), json!(question));
    info.insert("question_name".to_string(), json!("World Cup winner"));
    info.insert("outcome_index".to_string(), json!(outcome_index));
    info.insert("outcome_side".to_string(), json!(outcome_side));
    info.insert(
        "named_index".to_string(),
        json!(named_index(terminal_label)),
    );
    info.insert("market_name".to_string(), json!(terminal_label));
    info.insert(
        "side_name".to_string(),
        json!(format!("{side_label}-{terminal_label}")),
    );
    if is_fallback {
        info.insert("is_fallback".to_string(), json!(true));
    }
    binary_option(
        InstrumentId::from(format!(
            "HIP4-{question}-{outcome_index}-{outcome_side}.HYPERLIQUID"
        )),
        Some(info),
        terminal_label,
        &format!("{side_label}-{terminal_label}"),
    )
}

fn binary_option(
    instrument_id: InstrumentId,
    info: Option<Params>,
    outcome: &str,
    side_label: &str,
) -> InstrumentAny {
    let description = format!("{outcome} {side_label}");
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        Symbol::from(format!("{outcome}-{side_label}")),
        AssetClass::Alternative,
        Currency::USD(),
        UnixNanos::default(),
        UnixNanos::from(10_000_000_000_u64),
        3,
        2,
        Price::from("0.001"),
        Quantity::from("0.01"),
        Some(Ustr::from(outcome)),
        Some(Ustr::from(description.as_str())),
        None,
        Some(Quantity::from("0.01")),
        None,
        Some(nautilus_model::types::Money::new(1.0, Currency::USD())),
        Some(Price::from("0.999")),
        None,
        None,
        None,
        None,
        None,
        None,
        info,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn hip4_source() -> OutcomeGroupSourceConfig {
    let terminal_state_labels = vec!["home".to_string(), "away".to_string()];
    let payout_cols = payout_cols();
    let payout_values = vec![Decimal::ONE; payout_cols.len()];
    let payout_attestation = payout_vector_attestation_sha256(
        "fallback_refund",
        "fallback_refund",
        &payout_cols,
        &payout_values,
        "operator_attested_static_payout_per_unit",
    );
    let mut payouts = BTreeMap::new();
    payouts.insert(
        "fallback_refund".to_string(),
        OutcomeGroupNonStandardTerminalPayoutBlock {
            convention: OutcomeGroupRefundConvention::OperatorAttestedStaticPayoutPerUnit,
            terminal_state_label: "fallback_refund".to_string(),
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
    OutcomeGroupSourceConfig {
        source_id: "hl_world_cup".to_string(),
        client_id: ClientId::from("hyperliquid_main"),
        kind: SourceConfigKind::Hip4,
        event_slugs: None,
        market_slugs: None,
        sports_market_types: None,
        gamma_query: None,
        question: Some(9001),
        expected_neg_risk_market_id: None,
        terminal_state_labels: Some(terminal_state_labels),
        max_markets: None,
        max_groups: Some(1),
        enabled: true,
        freshness: Some(GateProviderFreshnessBlock {
            max_age_ms: Some(500),
            max_clock_skew_ms: Some(250),
        }),
        order_constraints: Some(OutcomeGroupOrderConstraintsBlock {
            default_min_quantity: Some("0.01".to_string()),
            default_min_notional: Some("1".to_string()),
            per_leg: None,
        }),
        role_bindings: None,
        settlement_rules: Some(OutcomeGroupSettlementRulesBlock {
            settlement_contract_id: "hl-world-cup-question-9001".to_string(),
            settlement_source_kind: OutcomeGroupSettlementSourceKind::OutcomeQuestion,
            terminal_state_convention: OutcomeGroupTerminalStateConvention::ExactlyOneWinner,
            void_policy: OutcomeGroupVoidPolicy::OperatorAttestedFallback,
            rounding_policy: OutcomeGroupRoundingPolicy::DecimalExact,
            timing_policy: OutcomeGroupTimingPolicy::VenueFinalResolution,
            attestation_sha256: hash("settlement"),
            non_standard_terminal_payouts: Some(payouts),
        }),
    }
}

fn payout_cols() -> Vec<AttestedLegRef> {
    ["home", "away"]
        .into_iter()
        .flat_map(|state| {
            [
                AttestedLegRef::OutcomeAndSide {
                    outcome_label: state.to_string(),
                    side_label: format!("supports-{state}"),
                },
                AttestedLegRef::OutcomeAndSide {
                    outcome_label: state.to_string(),
                    side_label: format!("opposes-{state}"),
                },
            ]
        })
        .collect()
}

fn named_index(terminal_label: &str) -> u32 {
    match terminal_label {
        "home" | "red" => 0,
        "away" | "blue" => 1,
        other => panic!("unexpected test terminal label {other}"),
    }
}

fn hash(label: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(label.as_bytes()))
}
