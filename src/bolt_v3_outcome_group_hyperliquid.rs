use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use nautilus_core::Params;
use nautilus_model::{
    identifiers::{InstrumentId, Venue},
    instruments::{BinaryOption, InstrumentAny},
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_outcome_group_sources::{
        OutcomeGroupNonStandardTerminalPayoutBlock, OutcomeGroupRefundConvention,
        OutcomeGroupSettlementSourceKind, OutcomeGroupSourceConfig,
        OutcomeGroupSourceKind as SourceConfigKind, OutcomeGroupVoidPolicy,
        outcome_group_observation_is_fresh,
    },
    bolt_v3_outcome_groups::{
        AttestedLegRef, AttestedPayoutVector, CanonicalField, GroupingProof,
        NormalizedPriceScaleEvidence, OrderConstraintSource, OutcomeGroup,
        OutcomeGroupValidationError, OutcomeLeg, OutcomeLegOrderConstraints, OutcomeLegRole,
        PayoutMatrix, PriceScaleAssertionSource, RoleBindingProof, SettlementRules,
        SettlementSourceKind, TerminalPayoutDerivation, TerminalState, TerminalStateConvention,
        TerminalStateKind, ValidatedOutcomeGroup, build_leg_map, canonical_fingerprint,
        derive_standard_payout_matrix, expected_metadata_fingerprint,
        native_identity_from_provider_key,
    },
    bolt_v3_providers::hyperliquid,
};

#[derive(Debug, Clone)]
pub struct HyperliquidHip4OutcomeGroupInput<'a> {
    pub source: &'a OutcomeGroupSourceConfig,
    pub metadata_loaded_unix_ms: u64,
    pub now_unix_ms: u64,
    pub metadata_ttl_ms: u64,
    pub instruments: Vec<InstrumentAny>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperliquidHip4OutcomeGroupError {
    StaleMetadata,
    UnsupportedSourceKind,
    MissingConfiguredQuestion,
    MissingParentQuestion,
    MissingStructuredField {
        field: &'static str,
        instrument_id: String,
    },
    NumericRange {
        field: &'static str,
        instrument_id: String,
    },
    UnsupportedOutcomeSide {
        instrument_id: String,
        outcome_side: u8,
    },
    EmptyQuestion,
    TerminalStateMismatch,
    MissingNonStandardPayout,
    MissingOrderConstraints,
    MixedSettlementAsset,
    InvalidDecimal {
        field: &'static str,
        value: String,
    },
    Validation(OutcomeGroupValidationError),
}

impl HyperliquidHip4OutcomeGroupError {
    pub fn is_missing_parent_question(&self) -> bool {
        matches!(self, Self::MissingParentQuestion)
    }
}

impl From<OutcomeGroupValidationError> for HyperliquidHip4OutcomeGroupError {
    fn from(value: OutcomeGroupValidationError) -> Self {
        Self::Validation(value)
    }
}

#[derive(Debug, Clone)]
struct Hip4LegMetadata {
    native_leg_id: String,
    instrument_id: InstrumentId,
    outcome_index: u32,
    outcome_side: u8,
    named_index: u32,
    outcome_label: String,
    side_label: String,
    settlement_asset_id: String,
    quantity_step: Decimal,
}

pub fn normalize_hyperliquid_hip4_outcome_group(
    input: HyperliquidHip4OutcomeGroupInput<'_>,
) -> Result<OutcomeGroup, HyperliquidHip4OutcomeGroupError> {
    validate_metadata_freshness(&input)?;
    validate_source_kind(input.source.kind)?;
    let question = input
        .source
        .question
        .ok_or(HyperliquidHip4OutcomeGroupError::MissingConfiguredQuestion)?;
    let terminal_labels = standard_terminal_labels(input.source)?;
    let legs = matching_question_legs(input.instruments, question, &terminal_labels)?;
    if legs.is_empty() {
        return Err(HyperliquidHip4OutcomeGroupError::EmptyQuestion);
    }

    let settlement_rules_block = input
        .source
        .settlement_rules
        .as_ref()
        .ok_or(HyperliquidHip4OutcomeGroupError::MissingNonStandardPayout)?;
    let non_standard_payouts = settlement_rules_block
        .non_standard_terminal_payouts
        .as_ref()
        .filter(|payouts| !payouts.is_empty())
        .ok_or(HyperliquidHip4OutcomeGroupError::MissingNonStandardPayout)?;
    let terminal_states = build_terminal_states(
        &terminal_labels,
        non_standard_payouts,
        settlement_rules_block.void_policy,
    )?;
    let settlement_asset_id = shared_settlement_asset_id(&legs)?;
    let outcome_indices = standard_outcome_indices(&legs);
    let tradable_legs =
        build_tradable_legs(input.source, &legs, &settlement_asset_id, &terminal_labels)?;
    let mut payout_matrix = derive_standard_payout_matrix(
        &terminal_states,
        &tradable_legs,
        TerminalStateConvention::ExactlyOneWinner,
    )?;
    let attested_vectors =
        build_non_standard_vectors(non_standard_payouts, &tradable_legs, &mut payout_matrix)?;

    let role_binding_proof = RoleBindingProof::VenueStructuredFields {
        source_id: input.source.source_id.clone(),
        question,
        outcome_indices: outcome_indices.clone(),
        proof_fingerprint: role_binding_proof_fingerprint(input.source, question, &outcome_indices),
    };
    let grouping_proof = GroupingProof::HyperliquidOutcome {
        question,
        outcome_indices,
        proof_fingerprint: grouping_proof_fingerprint(input.source, question, &legs),
    };

    let mut group = OutcomeGroup {
        group_id: native_identity_from_provider_key(hyperliquid::KEY, question),
        source_client_id: input.source.client_id,
        venue: Venue::from(hyperliquid::KEY),
        source_kind: crate::bolt_v3_outcome_groups::OutcomeGroupSourceKind::Hyperliquid,
        settlement_asset_id,
        terminal_states,
        tradable_legs,
        payout_matrix,
        grouping_proof: Some(grouping_proof),
        role_binding_proof: Some(role_binding_proof),
        settlement_rules: SettlementRules {
            terminal_state_convention: TerminalStateConvention::ExactlyOneWinner,
            settlement_source_kind: settlement_source_kind(
                settlement_rules_block.settlement_source_kind,
            ),
            non_standard_terminal_payouts: attested_vectors,
            terminal_payout_derivation: TerminalPayoutDerivation::StandardRowsPlusAttestedVectors,
        },
        freshness_source_id: input.source.source_id.clone(),
        metadata_fingerprint: String::new(),
    };
    group.metadata_fingerprint = expected_metadata_fingerprint(&group);
    ValidatedOutcomeGroup::validate(&group)?;
    Ok(group)
}

fn validate_metadata_freshness(
    input: &HyperliquidHip4OutcomeGroupInput<'_>,
) -> Result<(), HyperliquidHip4OutcomeGroupError> {
    let max_clock_skew_ms = input
        .source
        .freshness
        .as_ref()
        .and_then(|freshness| freshness.max_clock_skew_ms);
    if !outcome_group_observation_is_fresh(
        input.now_unix_ms,
        input.metadata_loaded_unix_ms,
        input.metadata_ttl_ms,
        max_clock_skew_ms,
    ) {
        return Err(HyperliquidHip4OutcomeGroupError::StaleMetadata);
    }
    Ok(())
}

fn validate_source_kind(kind: SourceConfigKind) -> Result<(), HyperliquidHip4OutcomeGroupError> {
    match kind {
        SourceConfigKind::Hip4 => Ok(()),
        SourceConfigKind::GammaEvent
        | SourceConfigKind::GammaMarketSlug
        | SourceConfigKind::GammaQuery => {
            Err(HyperliquidHip4OutcomeGroupError::UnsupportedSourceKind)
        }
    }
}

fn standard_terminal_labels(
    source: &OutcomeGroupSourceConfig,
) -> Result<Vec<String>, HyperliquidHip4OutcomeGroupError> {
    let labels = source
        .terminal_state_labels
        .as_ref()
        .filter(|labels| !labels.is_empty())
        .ok_or(HyperliquidHip4OutcomeGroupError::TerminalStateMismatch)?;
    Ok(labels.clone())
}

fn matching_question_legs(
    instruments: Vec<InstrumentAny>,
    question: u32,
    terminal_labels: &[String],
) -> Result<Vec<Hip4LegMetadata>, HyperliquidHip4OutcomeGroupError> {
    let mut saw_parentless_outcome = false;
    let mut legs = Vec::new();
    for instrument in instruments {
        let InstrumentAny::BinaryOption(binary) = instrument else {
            continue;
        };
        let Some(info) = binary.info.clone() else {
            saw_parentless_outcome = true;
            continue;
        };
        let Some(candidate_question) = info.get_u64("question") else {
            saw_parentless_outcome = true;
            continue;
        };
        if candidate_question != u64::from(question) {
            continue;
        }
        if info.get_bool("is_fallback").unwrap_or(false) {
            continue;
        }
        legs.push(leg_from_binary_option(binary, &info, terminal_labels)?);
    }
    if legs.is_empty() && saw_parentless_outcome {
        return Err(HyperliquidHip4OutcomeGroupError::MissingParentQuestion);
    }
    Ok(legs)
}

fn leg_from_binary_option(
    binary: BinaryOption,
    info: &Params,
    terminal_labels: &[String],
) -> Result<Hip4LegMetadata, HyperliquidHip4OutcomeGroupError> {
    let instrument_id = binary.id.to_string();
    let outcome_index = info_u32(info, "outcome_index", &instrument_id)?;
    let outcome_side = info_u8(info, "outcome_side", &instrument_id)?;
    if outcome_side > 1 {
        return Err(HyperliquidHip4OutcomeGroupError::UnsupportedOutcomeSide {
            instrument_id,
            outcome_side,
        });
    }
    let named_index = info_u32(info, "named_index", &instrument_id)?;
    let outcome_label = terminal_labels
        .get(usize::try_from(named_index).map_err(|_| {
            HyperliquidHip4OutcomeGroupError::NumericRange {
                field: "named_index",
                instrument_id: instrument_id.clone(),
            }
        })?)
        .cloned()
        .ok_or(HyperliquidHip4OutcomeGroupError::TerminalStateMismatch)?;
    let side_label = info
        .get_str("side_name")
        .map(str::to_string)
        .or_else(|| binary.outcome.as_ref().map(|value| value.to_string()))
        .unwrap_or_else(|| format!("side_{outcome_side}"));
    Ok(Hip4LegMetadata {
        native_leg_id: instrument_id.clone(),
        instrument_id: binary.id,
        outcome_index,
        outcome_side,
        named_index,
        outcome_label,
        side_label,
        settlement_asset_id: binary.currency.to_string(),
        quantity_step: binary.size_increment.as_decimal(),
    })
}

fn info_u32(
    info: &Params,
    field: &'static str,
    instrument_id: &str,
) -> Result<u32, HyperliquidHip4OutcomeGroupError> {
    let value = info.get_u64(field).ok_or_else(|| {
        HyperliquidHip4OutcomeGroupError::MissingStructuredField {
            field,
            instrument_id: instrument_id.to_string(),
        }
    })?;
    u32::try_from(value).map_err(|_| HyperliquidHip4OutcomeGroupError::NumericRange {
        field,
        instrument_id: instrument_id.to_string(),
    })
}

fn info_u8(
    info: &Params,
    field: &'static str,
    instrument_id: &str,
) -> Result<u8, HyperliquidHip4OutcomeGroupError> {
    let value = info.get_u64(field).ok_or_else(|| {
        HyperliquidHip4OutcomeGroupError::MissingStructuredField {
            field,
            instrument_id: instrument_id.to_string(),
        }
    })?;
    u8::try_from(value).map_err(|_| HyperliquidHip4OutcomeGroupError::NumericRange {
        field,
        instrument_id: instrument_id.to_string(),
    })
}

fn build_terminal_states(
    standard_labels: &[String],
    non_standard_payouts: &BTreeMap<String, OutcomeGroupNonStandardTerminalPayoutBlock>,
    void_policy: OutcomeGroupVoidPolicy,
) -> Result<BTreeMap<String, TerminalState>, HyperliquidHip4OutcomeGroupError> {
    let mut terminal_states = BTreeMap::new();
    for label in standard_labels {
        terminal_states.insert(
            label.clone(),
            TerminalState {
                state_id: label.clone(),
                label: label.clone(),
                kind: TerminalStateKind::Standard,
            },
        );
    }
    let non_standard_kind = match void_policy {
        OutcomeGroupVoidPolicy::RefundAllLegs => TerminalStateKind::Void,
        OutcomeGroupVoidPolicy::OperatorAttestedFallback => TerminalStateKind::Fallback,
    };
    for (terminal_state_id, payout) in non_standard_payouts {
        if terminal_states
            .insert(
                terminal_state_id.clone(),
                TerminalState {
                    state_id: terminal_state_id.clone(),
                    label: payout.terminal_state_label.clone(),
                    kind: non_standard_kind,
                },
            )
            .is_some()
        {
            return Err(HyperliquidHip4OutcomeGroupError::TerminalStateMismatch);
        }
    }
    Ok(terminal_states)
}

fn shared_settlement_asset_id(
    legs: &[Hip4LegMetadata],
) -> Result<String, HyperliquidHip4OutcomeGroupError> {
    let mut settlement_asset_id = None::<String>;
    for leg in legs {
        match settlement_asset_id.as_ref() {
            Some(previous) if previous != &leg.settlement_asset_id => {
                return Err(HyperliquidHip4OutcomeGroupError::MixedSettlementAsset);
            }
            Some(_) => {}
            None => settlement_asset_id = Some(leg.settlement_asset_id.clone()),
        }
    }
    settlement_asset_id.ok_or(HyperliquidHip4OutcomeGroupError::EmptyQuestion)
}

fn standard_outcome_indices(legs: &[Hip4LegMetadata]) -> Vec<u32> {
    legs.iter()
        .map(|leg| leg.outcome_index)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn build_tradable_legs(
    source: &OutcomeGroupSourceConfig,
    legs: &[Hip4LegMetadata],
    settlement_asset_id: &str,
    terminal_labels: &[String],
) -> Result<BTreeMap<String, OutcomeLeg>, HyperliquidHip4OutcomeGroupError> {
    let mut out = Vec::new();
    let mut seen_side = BTreeSet::new();
    for leg in legs {
        if !seen_side.insert((leg.outcome_index, leg.outcome_side)) {
            return Err(HyperliquidHip4OutcomeGroupError::TerminalStateMismatch);
        }
        let terminal_label = terminal_labels
            .get(usize::try_from(leg.named_index).map_err(|_| {
                HyperliquidHip4OutcomeGroupError::NumericRange {
                    field: "named_index",
                    instrument_id: leg.native_leg_id.clone(),
                }
            })?)
            .ok_or(HyperliquidHip4OutcomeGroupError::TerminalStateMismatch)?;
        let leg_role = match leg.outcome_side {
            0 => OutcomeLegRole::PaysOnTerminalState(terminal_label.clone()),
            1 => OutcomeLegRole::PaysUnlessTerminalState(terminal_label.clone()),
            outcome_side => {
                return Err(HyperliquidHip4OutcomeGroupError::UnsupportedOutcomeSide {
                    instrument_id: leg.native_leg_id.clone(),
                    outcome_side,
                });
            }
        };
        out.push(OutcomeLeg {
            leg_id: leg.native_leg_id.clone(),
            instrument_id: leg.instrument_id,
            native_leg_id: leg.native_leg_id.clone(),
            settlement_asset_id: settlement_asset_id.to_string(),
            outcome_label: leg.outcome_label.clone(),
            side_label: leg.side_label.clone(),
            leg_role,
            price_scale: price_scale_evidence(leg),
            order_constraints: order_constraints_for_leg(source, leg)?,
        });
    }
    build_leg_map(out).map_err(HyperliquidHip4OutcomeGroupError::Validation)
}

fn price_scale_evidence(leg: &Hip4LegMetadata) -> NormalizedPriceScaleEvidence {
    NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
        settlement_asset_id: leg.settlement_asset_id.clone(),
        payout_per_contract: Decimal::ONE,
        price_units_per_payout: Decimal::ONE,
        assertion_source: PriceScaleAssertionSource::VenueStructuredFields {
            proof_fingerprint: canonical_fingerprint(vec![
                CanonicalField::new(["price_scale", "native_leg_id"], &leg.native_leg_id),
                CanonicalField::new(
                    ["price_scale", "outcome_index"],
                    leg.outcome_index.to_string(),
                ),
                CanonicalField::new(
                    ["price_scale", "outcome_side"],
                    leg.outcome_side.to_string(),
                ),
                CanonicalField::new(
                    ["price_scale", "settlement_asset_id"],
                    &leg.settlement_asset_id,
                ),
            ]),
        },
    }
}

fn order_constraints_for_leg(
    source: &OutcomeGroupSourceConfig,
    leg: &Hip4LegMetadata,
) -> Result<OutcomeLegOrderConstraints, HyperliquidHip4OutcomeGroupError> {
    let constraints = source
        .order_constraints
        .as_ref()
        .ok_or(HyperliquidHip4OutcomeGroupError::MissingOrderConstraints)?;
    let per_leg = constraints.per_leg.as_ref().and_then(|per_leg| {
        per_leg
            .iter()
            .find(|constraint| constraint.native_leg_id == leg.native_leg_id)
    });
    let min_quantity = match per_leg.and_then(|constraint| constraint.min_quantity.as_deref()) {
        Some(value) => parse_decimal("order_constraints.per_leg.min_quantity", value)?,
        None => parse_required_decimal(
            "order_constraints.default_min_quantity",
            constraints.default_min_quantity.as_deref(),
        )?,
    };
    let min_notional = match per_leg.and_then(|constraint| constraint.min_notional.as_deref()) {
        Some(value) => Some(parse_decimal(
            "order_constraints.per_leg.min_notional",
            value,
        )?),
        None => optional_decimal(
            "order_constraints.default_min_notional",
            constraints.default_min_notional.as_deref(),
        )?,
    };
    Ok(OutcomeLegOrderConstraints {
        min_quantity,
        min_notional,
        quantity_step: leg.quantity_step,
        constraint_source: OrderConstraintSource::NtInstrumentWithConfigFloor {
            source_id: source.source_id.clone(),
        },
    })
}

fn build_non_standard_vectors(
    non_standard_payouts: &BTreeMap<String, OutcomeGroupNonStandardTerminalPayoutBlock>,
    tradable_legs: &BTreeMap<String, OutcomeLeg>,
    payout_matrix: &mut PayoutMatrix,
) -> Result<Vec<AttestedPayoutVector>, HyperliquidHip4OutcomeGroupError> {
    let mut vectors = Vec::new();
    for (terminal_state_id, payout) in non_standard_payouts {
        let mut configured_payouts = BTreeMap::<(String, String), Decimal>::new();
        for leg in &payout.legs {
            let payout_per_unit = parse_decimal(
                "non_standard_terminal_payouts.payout_per_unit",
                &leg.payout_per_unit,
            )?;
            if configured_payouts
                .insert(
                    (leg.outcome_label.clone(), leg.side_label.clone()),
                    payout_per_unit,
                )
                .is_some()
            {
                return Err(HyperliquidHip4OutcomeGroupError::MissingNonStandardPayout);
            }
        }

        let mut cols = Vec::with_capacity(payout_matrix.cols.len());
        let mut payouts = Vec::with_capacity(payout_matrix.cols.len());
        for leg_id in &payout_matrix.cols {
            let leg = &tradable_legs[leg_id];
            let key = (leg.outcome_label.clone(), leg.side_label.clone());
            let payout_per_unit = configured_payouts
                .remove(&key)
                .ok_or(HyperliquidHip4OutcomeGroupError::MissingNonStandardPayout)?;
            cols.push(AttestedLegRef::OutcomeAndSide {
                outcome_label: leg.outcome_label.clone(),
                side_label: leg.side_label.clone(),
            });
            payouts.push(payout_per_unit);
        }
        if !configured_payouts.is_empty() {
            return Err(HyperliquidHip4OutcomeGroupError::MissingNonStandardPayout);
        }
        payout_matrix
            .payout_per_unit_by_state
            .insert(terminal_state_id.clone(), payouts.clone());
        vectors.push(AttestedPayoutVector {
            terminal_state_id: terminal_state_id.clone(),
            label: payout.terminal_state_label.clone(),
            cols,
            payouts,
            refund_convention: refund_convention_label(payout.convention).to_string(),
            attestation_sha256: payout.attestation_sha256.clone(),
        });
    }
    Ok(vectors)
}

fn grouping_proof_fingerprint(
    source: &OutcomeGroupSourceConfig,
    question: u32,
    legs: &[Hip4LegMetadata],
) -> String {
    canonical_fingerprint(vec![
        CanonicalField::new(["grouping", "source_id"], &source.source_id),
        CanonicalField::new(["grouping", "question"], question.to_string()),
        CanonicalField::new(
            ["grouping", "outcome_indices"],
            standard_outcome_indices(legs)
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
    ])
}

fn role_binding_proof_fingerprint(
    source: &OutcomeGroupSourceConfig,
    question: u32,
    outcome_indices: &[u32],
) -> String {
    canonical_fingerprint(vec![
        CanonicalField::new(["role_binding", "source_id"], &source.source_id),
        CanonicalField::new(["role_binding", "question"], question.to_string()),
        CanonicalField::new(
            ["role_binding", "outcome_indices"],
            outcome_indices
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
    ])
}

fn settlement_source_kind(kind: OutcomeGroupSettlementSourceKind) -> SettlementSourceKind {
    match kind {
        OutcomeGroupSettlementSourceKind::CtfUma
        | OutcomeGroupSettlementSourceKind::OutcomeQuestion => {
            SettlementSourceKind::VenueStructuredFields
        }
        OutcomeGroupSettlementSourceKind::OperatorAttestedContract => {
            SettlementSourceKind::OperatorAttested
        }
    }
}

fn refund_convention_label(convention: OutcomeGroupRefundConvention) -> &'static str {
    match convention {
        OutcomeGroupRefundConvention::OperatorAttestedStaticPayoutPerUnit => {
            "operator_attested_static_payout_per_unit"
        }
    }
}

fn parse_required_decimal(
    field: &'static str,
    value: Option<&str>,
) -> Result<Decimal, HyperliquidHip4OutcomeGroupError> {
    let value = value.ok_or(HyperliquidHip4OutcomeGroupError::MissingOrderConstraints)?;
    parse_decimal(field, value)
}

fn optional_decimal(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<Decimal>, HyperliquidHip4OutcomeGroupError> {
    value.map(|value| parse_decimal(field, value)).transpose()
}

fn parse_decimal(
    field: &'static str,
    value: &str,
) -> Result<Decimal, HyperliquidHip4OutcomeGroupError> {
    Decimal::from_str(value).map_err(|_| HyperliquidHip4OutcomeGroupError::InvalidDecimal {
        field,
        value: value.to_string(),
    })
}
