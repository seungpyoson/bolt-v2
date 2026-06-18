use std::collections::BTreeMap;

use bolt_v2::bolt_v3_outcome_groups::{
    AttestedLegRef, AttestedPayoutVector, CanonicalField, GroupingProof,
    NormalizedPriceScaleEvidence, OrderConstraintSource, OutcomeGroup, OutcomeGroupSourceKind,
    OutcomeLeg, OutcomeLegOrderConstraints, OutcomeLegRole, PayoutMatrix,
    PolymarketDiscoveryScopeEvidence, PositiveSideBinding, PriceScaleAssertionSource,
    RoleBindingProof, SettlementRules, SettlementSourceKind, TerminalPayoutDerivation,
    TerminalState, TerminalStateConvention, TerminalStateKind, ValidatedOutcomeGroup,
    build_leg_map, canonical_fingerprint, derive_standard_payout_matrix,
    expected_metadata_fingerprint, is_lowercase_sha256, payout_vector_attestation_sha256,
    role_binding_attestation_sha256, validate_grouping_identity_set,
};
use nautilus_model::identifiers::{ClientId, InstrumentId, Venue};
use rust_decimal::Decimal;

fn dec(value: i64) -> Decimal {
    Decimal::new(value, 0)
}

fn d(value: i64, scale: u32) -> Decimal {
    Decimal::new(value, scale)
}

fn hash(seed: char) -> String {
    seed.to_string().repeat(64)
}

fn terminal_states(include_void: bool) -> BTreeMap<String, TerminalState> {
    let mut states = BTreeMap::from([
        (
            "home".to_string(),
            TerminalState {
                state_id: "home".to_string(),
                label: "Home wins".to_string(),
                kind: TerminalStateKind::Standard,
            },
        ),
        (
            "draw".to_string(),
            TerminalState {
                state_id: "draw".to_string(),
                label: "Draw".to_string(),
                kind: TerminalStateKind::Standard,
            },
        ),
    ]);
    if include_void {
        states.insert(
            "void".to_string(),
            TerminalState {
                state_id: "void".to_string(),
                label: "Refund".to_string(),
                kind: TerminalStateKind::Void,
            },
        );
    }
    states
}

fn price_scale(asset: &str) -> NormalizedPriceScaleEvidence {
    NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
        settlement_asset_id: asset.to_string(),
        payout_per_contract: dec(1),
        price_units_per_payout: dec(1),
        assertion_source: PriceScaleAssertionSource::OperatorAttested {
            attestation_sha256: hash('a'),
        },
    }
}

fn order_constraints(source_id: &str) -> OutcomeLegOrderConstraints {
    OutcomeLegOrderConstraints {
        min_quantity: d(1, 3),
        min_notional: Some(d(1, 0)),
        quantity_step: d(1, 3),
        constraint_source: OrderConstraintSource::ConfigFloorWithNtPrecision {
            source_id: source_id.to_string(),
        },
    }
}

fn leg(
    leg_id: &str,
    native_leg_id: &str,
    outcome_label: &str,
    side_label: &str,
    role: OutcomeLegRole,
) -> OutcomeLeg {
    OutcomeLeg {
        leg_id: leg_id.to_string(),
        instrument_id: InstrumentId::from(format!("{leg_id}.POLYMARKET").as_str()),
        native_leg_id: native_leg_id.to_string(),
        settlement_asset_id: "USDC".to_string(),
        outcome_label: outcome_label.to_string(),
        side_label: side_label.to_string(),
        leg_role: role,
        price_scale: price_scale("USDC"),
        order_constraints: order_constraints("world-cup-source"),
    }
}

fn role_bindings() -> Vec<PositiveSideBinding> {
    vec![
        PositiveSideBinding {
            terminal_state_label: "Home wins".to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId("native-home".to_string()),
            pays_unless_leg: AttestedLegRef::OutcomeAndSide {
                outcome_label: "Home".to_string(),
                side_label: "Other".to_string(),
            },
        },
        PositiveSideBinding {
            terminal_state_label: "Draw".to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId("native-draw".to_string()),
            pays_unless_leg: AttestedLegRef::OutcomeAndSide {
                outcome_label: "Draw".to_string(),
                side_label: "Other".to_string(),
            },
        },
    ]
}

fn payout_vector() -> AttestedPayoutVector {
    let cols = vec![
        AttestedLegRef::NativeLegId("native-draw".to_string()),
        AttestedLegRef::NativeLegId("native-draw-other".to_string()),
        AttestedLegRef::NativeLegId("native-home".to_string()),
        AttestedLegRef::NativeLegId("native-home-other".to_string()),
    ];
    let payouts = vec![dec(1), dec(1), dec(1), dec(1)];
    let attestation_sha256 =
        payout_vector_attestation_sha256("void", "refund", &cols, &payouts, "pro-rata-refund");
    AttestedPayoutVector {
        terminal_state_id: "void".to_string(),
        label: "refund".to_string(),
        cols,
        payouts,
        refund_convention: "pro-rata-refund".to_string(),
        attestation_sha256,
    }
}

fn valid_group() -> OutcomeGroup {
    let states = terminal_states(true);
    let legs = build_leg_map(vec![
        leg(
            "home-leg",
            "native-home",
            "Home",
            "Primary",
            OutcomeLegRole::PaysOnTerminalState("home".to_string()),
        ),
        leg(
            "draw-leg",
            "native-draw",
            "Draw",
            "Primary",
            OutcomeLegRole::PaysOnTerminalState("draw".to_string()),
        ),
        leg(
            "home-other-leg",
            "native-home-other",
            "Home",
            "Other",
            OutcomeLegRole::PaysUnlessTerminalState("home".to_string()),
        ),
        leg(
            "draw-other-leg",
            "native-draw-other",
            "Draw",
            "Other",
            OutcomeLegRole::PaysUnlessTerminalState("draw".to_string()),
        ),
    ])
    .expect("fixture leg ids are unique");
    let mut payout_matrix =
        derive_standard_payout_matrix(&states, &legs, TerminalStateConvention::ExactlyOneWinner)
            .expect("standard rows derive");
    payout_matrix
        .payout_per_unit_by_state
        .insert("void".to_string(), vec![dec(1), dec(1), dec(1), dec(1)]);
    let bindings = role_bindings();
    let role_binding_proof = RoleBindingProof::OperatorAttested {
        attestation_id: "role-binding-2026-06-14".to_string(),
        positive_side_bindings: bindings.clone(),
        attestation_sha256: role_binding_attestation_sha256(&bindings),
        proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
            ["role_binding", "source"],
            "operator-attested",
        )]),
    };
    let mut group = OutcomeGroup {
        group_id: "world-cup-three-way".to_string(),
        source_client_id: ClientId::from("polymarket_main"),
        venue: Venue::from("POLYMARKET"),
        source_kind: OutcomeGroupSourceKind::Polymarket,
        settlement_asset_id: "USDC".to_string(),
        terminal_states: states,
        tradable_legs: legs,
        payout_matrix,
        grouping_proof: Some(GroupingProof::PolymarketNegRisk {
            neg_risk_market_id: "neg-risk-market-123".to_string(),
            discovery_scope: PolymarketDiscoveryScopeEvidence {
                source_id: "world-cup-source".to_string(),
                event_slugs: Vec::new(),
                market_slugs: vec!["home-market".to_string(), "draw-market".to_string()],
                gamma_query_fingerprint: Some(hash('b')),
                cache_key_fingerprint: hash('c'),
            },
            market_slugs: vec!["home-market".to_string(), "draw-market".to_string()],
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["grouping", "neg_risk_market_id"],
                "neg-risk-market-123",
            )]),
        }),
        role_binding_proof: Some(role_binding_proof),
        settlement_rules: SettlementRules {
            terminal_state_convention: TerminalStateConvention::ExactlyOneWinner,
            settlement_source_kind: SettlementSourceKind::VenueStructuredFields,
            non_standard_terminal_payouts: vec![payout_vector()],
            terminal_payout_derivation: TerminalPayoutDerivation::StandardRowsPlusAttestedVectors,
        },
        freshness_source_id: "world-cup-source".to_string(),
        metadata_fingerprint: String::new(),
    };
    group.metadata_fingerprint = expected_metadata_fingerprint(&group);
    group
}

#[test]
fn valid_group_with_void_vector_passes_and_derives_standard_rows() {
    let group = valid_group();

    assert_eq!(ValidatedOutcomeGroup::validate(&group), Ok(()));
    assert!(is_lowercase_sha256(&group.metadata_fingerprint));
    assert_eq!(
        group.payout_matrix.payout_per_unit_by_state["home"],
        vec![dec(0), dec(1), dec(1), dec(0)]
    );
    assert_eq!(
        group.payout_matrix.payout_per_unit_by_state["draw"],
        vec![dec(1), dec(0), dec(0), dec(1)]
    );
}

#[test]
fn operator_strings_reject_control_characters_before_metadata_fingerprint_acceptance() {
    let mut group = valid_group();
    group.freshness_source_id = "world-cup-source\0shadow".to_string();

    assert!(
        ValidatedOutcomeGroup::validate(&group)
            .is_err_and(|error| error.is_invalid_operator_string())
    );
}

#[test]
fn operator_strings_reject_zero_width_format_characters_before_hashing() {
    let mut group = valid_group();
    let Some(GroupingProof::PolymarketNegRisk {
        discovery_scope, ..
    }) = &mut group.grouping_proof
    else {
        panic!("fixture should use Polymarket grouping proof");
    };
    discovery_scope.market_slugs[0] = "home\u{200b}-market".to_string();

    assert!(
        ValidatedOutcomeGroup::validate(&group)
            .is_err_and(|error| error.is_invalid_operator_string())
    );
}

#[test]
fn duplicate_or_extraneous_non_standard_payout_vectors_reject() {
    let mut duplicate = valid_group();
    duplicate
        .settlement_rules
        .non_standard_terminal_payouts
        .push(duplicate.settlement_rules.non_standard_terminal_payouts[0].clone());
    assert!(
        ValidatedOutcomeGroup::validate(&duplicate)
            .is_err_and(|err| err.is_duplicate_non_standard_vector())
    );

    let mut extraneous = valid_group();
    let mut vector = payout_vector();
    vector.terminal_state_id = "ghost".to_string();
    extraneous
        .settlement_rules
        .non_standard_terminal_payouts
        .push(vector);
    assert!(
        ValidatedOutcomeGroup::validate(&extraneous)
            .is_err_and(|err| err.is_unknown_terminal_state())
    );
}

#[test]
fn positive_side_bindings_must_target_standard_terminal_states() {
    let mut group = valid_group();
    if let Some(RoleBindingProof::OperatorAttested {
        positive_side_bindings,
        attestation_sha256,
        ..
    }) = group.role_binding_proof.as_mut()
    {
        positive_side_bindings[0].terminal_state_label = "Refund".to_string();
        *attestation_sha256 = role_binding_attestation_sha256(positive_side_bindings);
    }

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_matrix_value_mismatch())
    );
}

#[test]
fn missing_grouping_proof_rejects() {
    let mut group = valid_group();
    group.grouping_proof = None;

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_missing_grouping_proof())
    );
}

#[test]
fn duplicate_source_native_grouping_identity_with_conflicting_settlement_rejects() {
    let first = valid_group();
    let mut second = valid_group();
    second.settlement_asset_id = "USDT".to_string();
    second.metadata_fingerprint = expected_metadata_fingerprint(&second);

    let result = validate_grouping_identity_set([first, second].iter());

    assert!(result.is_err_and(|err| err.is_grouping_identity_conflict()));
}

#[test]
fn duplicate_source_native_grouping_identity_with_conflicting_payouts_rejects() {
    let first = valid_group();
    let mut second = valid_group();
    second
        .payout_matrix
        .payout_per_unit_by_state
        .insert("home".to_string(), vec![dec(1), dec(1), dec(1), dec(0)]);
    second.metadata_fingerprint = expected_metadata_fingerprint(&second);

    let result = validate_grouping_identity_set([first, second].iter());

    assert!(result.is_err_and(|err| err.is_grouping_identity_conflict()));
}

#[test]
fn duplicate_leg_ids_reject_before_map_construction() {
    let duplicate = leg(
        "same-leg",
        "native-home",
        "Home",
        "Primary",
        OutcomeLegRole::PaysOnTerminalState("home".to_string()),
    );

    let result = build_leg_map(vec![duplicate.clone(), duplicate]);

    assert!(result.is_err_and(|err| err.is_duplicate_leg_id()));
}

#[test]
fn empty_terminal_states_reject() {
    let mut group = valid_group();
    group.terminal_states.clear();
    group.metadata_fingerprint = expected_metadata_fingerprint(&group);

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_empty_terminal_states())
    );
}

#[test]
fn standard_payout_derivation_supports_all_role_shapes() {
    let states = terminal_states(false);
    let all_on = build_leg_map(vec![
        leg(
            "home-leg",
            "native-home",
            "Home",
            "Primary",
            OutcomeLegRole::PaysOnTerminalState("home".to_string()),
        ),
        leg(
            "draw-leg",
            "native-draw",
            "Draw",
            "Primary",
            OutcomeLegRole::PaysOnTerminalState("draw".to_string()),
        ),
    ])
    .expect("unique all-on legs");
    let all_unless = build_leg_map(vec![
        leg(
            "home-other-leg",
            "native-home-other",
            "Home",
            "Other",
            OutcomeLegRole::PaysUnlessTerminalState("home".to_string()),
        ),
        leg(
            "draw-other-leg",
            "native-draw-other",
            "Draw",
            "Other",
            OutcomeLegRole::PaysUnlessTerminalState("draw".to_string()),
        ),
    ])
    .expect("unique all-unless legs");

    let all_on_matrix =
        derive_standard_payout_matrix(&states, &all_on, TerminalStateConvention::ExactlyOneWinner)
            .expect("all-on rows derive");
    let all_unless_matrix = derive_standard_payout_matrix(
        &states,
        &all_unless,
        TerminalStateConvention::ExactlyOneWinner,
    )
    .expect("all-unless rows derive");

    assert_eq!(
        all_on_matrix.payout_per_unit_by_state["home"],
        vec![dec(0), dec(1)]
    );
    assert_eq!(
        all_on_matrix.payout_per_unit_by_state["draw"],
        vec![dec(1), dec(0)]
    );
    assert_eq!(
        all_unless_matrix.payout_per_unit_by_state["home"],
        vec![dec(1), dec(0)]
    );
    assert_eq!(
        all_unless_matrix.payout_per_unit_by_state["draw"],
        vec![dec(0), dec(1)]
    );
}

#[test]
fn unsupported_terminal_state_convention_rejects() {
    let mut group = valid_group();
    group.settlement_rules.terminal_state_convention =
        TerminalStateConvention::Unsupported("two_winners".to_string());

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_unsupported_convention())
    );
}

#[test]
fn multi_state_leg_role_rejects() {
    let mut group = valid_group();
    group
        .tradable_legs
        .get_mut("home-leg")
        .expect("home leg exists")
        .leg_role = OutcomeLegRole::UnsupportedMultiState {
        terminal_state_ids: vec!["home".to_string(), "draw".to_string()],
    };

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_multi_state_leg_role())
    );
}

#[test]
fn missing_role_binding_proof_rejects() {
    let mut group = valid_group();
    group.role_binding_proof = None;

    assert!(
        ValidatedOutcomeGroup::validate(&group)
            .is_err_and(|err| err.is_missing_role_binding_proof())
    );
}

#[test]
fn missing_void_row_or_attested_vector_rejects() {
    let mut missing_row = valid_group();
    missing_row
        .payout_matrix
        .payout_per_unit_by_state
        .remove("void");
    assert!(
        ValidatedOutcomeGroup::validate(&missing_row).is_err_and(|err| err.is_missing_payout_row())
    );

    let mut missing_vector = valid_group();
    missing_vector
        .settlement_rules
        .non_standard_terminal_payouts
        .clear();
    assert!(
        ValidatedOutcomeGroup::validate(&missing_vector)
            .is_err_and(|err| err.is_missing_non_standard_vector())
    );
}

#[test]
fn non_standard_payout_vector_length_rejects() {
    let mut group = valid_group();
    group.settlement_rules.non_standard_terminal_payouts[0]
        .payouts
        .pop();
    group.settlement_rules.non_standard_terminal_payouts[0].attestation_sha256 =
        payout_vector_attestation_sha256(
            "void",
            "refund",
            &group.settlement_rules.non_standard_terminal_payouts[0].cols,
            &group.settlement_rules.non_standard_terminal_payouts[0].payouts,
            "pro-rata-refund",
        );

    assert!(
        ValidatedOutcomeGroup::validate(&group)
            .is_err_and(|err| err.is_matrix_dimension_mismatch())
    );
}

#[test]
fn payout_matrix_shape_unknown_ids_and_transposition_reject() {
    let mut dimension = valid_group();
    dimension
        .payout_matrix
        .payout_per_unit_by_state
        .insert("home".to_string(), vec![dec(1)]);
    assert!(
        ValidatedOutcomeGroup::validate(&dimension)
            .is_err_and(|err| err.is_matrix_dimension_mismatch())
    );

    let mut unknown_state = valid_group();
    unknown_state
        .payout_matrix
        .payout_per_unit_by_state
        .insert("unknown".to_string(), vec![dec(0), dec(0), dec(0), dec(0)]);
    assert!(
        ValidatedOutcomeGroup::validate(&unknown_state)
            .is_err_and(|err| err.is_unknown_terminal_state())
    );

    let mut unknown_leg = valid_group();
    unknown_leg
        .payout_matrix
        .cols
        .push("missing-leg".to_string());
    unknown_leg
        .payout_matrix
        .payout_per_unit_by_state
        .values_mut()
        .for_each(|row| row.push(dec(0)));
    assert!(ValidatedOutcomeGroup::validate(&unknown_leg).is_err_and(|err| err.is_unknown_leg()));

    let mut transposed = valid_group();
    transposed
        .payout_matrix
        .payout_per_unit_by_state
        .insert("home".to_string(), vec![dec(1), dec(0), dec(1), dec(1)]);
    assert!(
        ValidatedOutcomeGroup::validate(&transposed)
            .is_err_and(|err| err.is_matrix_value_mismatch())
    );
}

#[test]
fn attested_leg_resolution_rejects_unknown_ambiguous_and_column_reorder() {
    let mut unknown = valid_group();
    if let Some(RoleBindingProof::OperatorAttested {
        positive_side_bindings,
        attestation_sha256,
        ..
    }) = unknown.role_binding_proof.as_mut()
    {
        positive_side_bindings[0].pays_on_leg = AttestedLegRef::NativeLegId("missing".to_string());
        *attestation_sha256 = role_binding_attestation_sha256(positive_side_bindings);
    }
    assert!(
        ValidatedOutcomeGroup::validate(&unknown).is_err_and(|err| err.is_attested_leg_unknown())
    );

    let mut ambiguous = valid_group();
    ambiguous
        .tradable_legs
        .get_mut("home-leg")
        .expect("home leg exists")
        .side_label = "Other".to_string();
    ambiguous.metadata_fingerprint = expected_metadata_fingerprint(&ambiguous);
    assert!(
        ValidatedOutcomeGroup::validate(&ambiguous)
            .is_err_and(|err| err.is_attested_leg_ambiguous())
    );

    let mut reordered = valid_group();
    reordered.settlement_rules.non_standard_terminal_payouts[0]
        .cols
        .swap(0, 1);
    reordered.settlement_rules.non_standard_terminal_payouts[0].attestation_sha256 =
        payout_vector_attestation_sha256(
            "void",
            "refund",
            &reordered.settlement_rules.non_standard_terminal_payouts[0].cols,
            &reordered.settlement_rules.non_standard_terminal_payouts[0].payouts,
            "pro-rata-refund",
        );
    assert!(
        ValidatedOutcomeGroup::validate(&reordered)
            .is_err_and(|err| err.is_attested_column_order_mismatch())
    );
}

#[test]
fn rekeyed_attested_vectors_and_positive_bindings_reject() {
    let mut vector = valid_group();
    vector.settlement_rules.non_standard_terminal_payouts[0].cols[0] =
        AttestedLegRef::NativeLegId("native-home".to_string());
    assert!(
        ValidatedOutcomeGroup::validate(&vector)
            .is_err_and(|err| err.is_attestation_hash_mismatch())
    );

    let mut binding = valid_group();
    if let Some(RoleBindingProof::OperatorAttested {
        positive_side_bindings,
        ..
    }) = binding.role_binding_proof.as_mut()
    {
        positive_side_bindings[0].pays_on_leg =
            AttestedLegRef::NativeLegId("native-draw".to_string());
    }
    assert!(
        ValidatedOutcomeGroup::validate(&binding)
            .is_err_and(|err| err.is_attestation_hash_mismatch())
    );
}

#[test]
fn canonical_bytes_are_separator_safe_and_stable() {
    let one_field = canonical_fingerprint(vec![CanonicalField::new(["a=b\nc"], "value")]);
    let two_fields = canonical_fingerprint(vec![
        CanonicalField::new(["a"], "b\nc=value"),
        CanonicalField::new(["a", "b\nc"], "value"),
    ]);
    let stable = canonical_fingerprint(vec![
        CanonicalField::new(["decimal"], Decimal::new(100, 2).normalize().to_string()),
        CanonicalField::new(["path", "0"], "alpha"),
    ]);

    assert_ne!(one_field, two_fields);
    assert_eq!(
        stable,
        canonical_fingerprint(vec![
            CanonicalField::new(["path", "0"], "alpha"),
            CanonicalField::new(["decimal"], "1"),
        ])
    );
}

#[test]
fn invalid_payout_price_scale_order_constraints_and_assets_reject() {
    let mut payout = valid_group();
    payout
        .payout_matrix
        .payout_per_unit_by_state
        .insert("void".to_string(), vec![dec(2), dec(1), dec(1), dec(1)]);
    assert!(
        ValidatedOutcomeGroup::validate(&payout).is_err_and(|err| err.is_out_of_bounds_payout())
    );

    let mut price_scale_group = valid_group();
    if let Some(leg) = price_scale_group.tradable_legs.get_mut("home-leg") {
        leg.price_scale = NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id: "USDC".to_string(),
            payout_per_contract: Decimal::ZERO,
            price_units_per_payout: dec(1),
            assertion_source: PriceScaleAssertionSource::OperatorAttested {
                attestation_sha256: hash('a'),
            },
        };
    }
    assert!(
        ValidatedOutcomeGroup::validate(&price_scale_group)
            .is_err_and(|err| err.is_invalid_price_scale())
    );

    let mut non_unit_price_scale = valid_group();
    if let Some(leg) = non_unit_price_scale.tradable_legs.get_mut("home-leg") {
        leg.price_scale = NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id: "USDC".to_string(),
            payout_per_contract: dec(2),
            price_units_per_payout: dec(1),
            assertion_source: PriceScaleAssertionSource::OperatorAttested {
                attestation_sha256: hash('a'),
            },
        };
    }
    assert!(
        ValidatedOutcomeGroup::validate(&non_unit_price_scale)
            .is_err_and(|err| err.is_invalid_price_scale())
    );

    let mut constraints = valid_group();
    constraints
        .tradable_legs
        .get_mut("home-leg")
        .expect("home leg exists")
        .order_constraints
        .min_quantity = Decimal::ZERO;
    assert!(
        ValidatedOutcomeGroup::validate(&constraints)
            .is_err_and(|err| err.is_invalid_order_constraint())
    );

    let mut asset = valid_group();
    asset
        .tradable_legs
        .get_mut("home-leg")
        .expect("home leg exists")
        .settlement_asset_id = "USDT".to_string();
    asset.metadata_fingerprint = expected_metadata_fingerprint(&asset);
    assert!(
        ValidatedOutcomeGroup::validate(&asset).is_err_and(|err| err.is_mixed_settlement_asset())
    );
}

#[test]
fn metadata_fingerprint_excludes_operator_policy_but_covers_grouping_identity() {
    let mut base = valid_group();
    let mut policy_only = base.clone();
    policy_only
        .tradable_legs
        .get_mut("home-leg")
        .expect("home leg exists")
        .order_constraints
        .min_quantity = d(5, 3);

    assert_eq!(
        expected_metadata_fingerprint(&base),
        expected_metadata_fingerprint(&policy_only)
    );

    if let Some(GroupingProof::PolymarketNegRisk {
        neg_risk_market_id, ..
    }) = base.grouping_proof.as_mut()
    {
        *neg_risk_market_id = "different-neg-risk".to_string();
    }

    assert_ne!(
        expected_metadata_fingerprint(&base),
        expected_metadata_fingerprint(&policy_only)
    );
}

#[test]
fn metadata_fingerprint_covers_settlement_rules_and_role_binding_proof() {
    let base = valid_group();
    let mut payout_changed = base.clone();
    payout_changed
        .settlement_rules
        .non_standard_terminal_payouts[0]
        .refund_convention = "full-refund".to_string();
    payout_changed.metadata_fingerprint = expected_metadata_fingerprint(&payout_changed);

    assert_ne!(
        expected_metadata_fingerprint(&base),
        expected_metadata_fingerprint(&payout_changed)
    );

    let mut role_binding_changed = base.clone();
    if let Some(RoleBindingProof::OperatorAttested {
        proof_fingerprint, ..
    }) = role_binding_changed.role_binding_proof.as_mut()
    {
        *proof_fingerprint = hash('c');
    }
    role_binding_changed.metadata_fingerprint =
        expected_metadata_fingerprint(&role_binding_changed);

    assert_ne!(
        expected_metadata_fingerprint(&base),
        expected_metadata_fingerprint(&role_binding_changed)
    );
}

#[test]
fn proof_fingerprints_and_sha_fields_are_fail_closed() {
    let mut bad_sha = valid_group();
    bad_sha.settlement_rules.non_standard_terminal_payouts[0].attestation_sha256 =
        "NOT_LOWER_HEX".to_string();
    assert!(ValidatedOutcomeGroup::validate(&bad_sha).is_err_and(|err| err.is_invalid_sha256()));

    let mut bad_metadata = valid_group();
    bad_metadata.metadata_fingerprint = hash('f');
    assert!(
        ValidatedOutcomeGroup::validate(&bad_metadata)
            .is_err_and(|err| err.is_metadata_fingerprint_mismatch())
    );
}

#[test]
fn manual_matrix_builder_rejects_unknown_outcome_labels() {
    let mut group = valid_group();
    if let Some(RoleBindingProof::OperatorAttested {
        positive_side_bindings,
        attestation_sha256,
        ..
    }) = group.role_binding_proof.as_mut()
    {
        positive_side_bindings[0].terminal_state_label = "Not a state".to_string();
        *attestation_sha256 = role_binding_attestation_sha256(positive_side_bindings);
    }

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_unknown_terminal_label())
    );
}

#[test]
fn payout_matrix_columns_must_match_declared_leg_order() {
    let mut group = valid_group();
    group.payout_matrix = PayoutMatrix {
        cols: vec![
            "draw-leg".to_string(),
            "home-leg".to_string(),
            "home-other-leg".to_string(),
            "draw-other-leg".to_string(),
        ],
        payout_per_unit_by_state: group.payout_matrix.payout_per_unit_by_state.clone(),
    };
    group.settlement_rules.non_standard_terminal_payouts[0].cols = vec![
        AttestedLegRef::NativeLegId("native-draw".to_string()),
        AttestedLegRef::NativeLegId("native-home".to_string()),
        AttestedLegRef::NativeLegId("native-home-other".to_string()),
        AttestedLegRef::NativeLegId("native-draw-other".to_string()),
    ];
    group.settlement_rules.non_standard_terminal_payouts[0].attestation_sha256 =
        payout_vector_attestation_sha256(
            "void",
            "refund",
            &group.settlement_rules.non_standard_terminal_payouts[0].cols,
            &group.settlement_rules.non_standard_terminal_payouts[0].payouts,
            "pro-rata-refund",
        );

    assert!(
        ValidatedOutcomeGroup::validate(&group).is_err_and(|err| err.is_matrix_value_mismatch())
    );
}
