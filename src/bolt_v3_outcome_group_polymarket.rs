use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use nautilus_model::identifiers::{InstrumentId, Venue};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_outcome_group_sources::{
        OutcomeGroupNonStandardTerminalPayoutBlock, OutcomeGroupRefundConvention,
        OutcomeGroupRoleBindingsBlock, OutcomeGroupSettlementSourceKind, OutcomeGroupSourceConfig,
        OutcomeGroupSourceKind as SourceConfigKind, OutcomeGroupVoidPolicy,
        outcome_group_observation_is_fresh,
    },
    bolt_v3_outcome_groups::{
        AttestedLegRef, AttestedPayoutVector, CanonicalField, GroupingProof,
        NormalizedPriceScaleEvidence, OrderConstraintSource, OutcomeGroup,
        OutcomeGroupSourceKind as SharedSourceKind, OutcomeGroupValidationError, OutcomeLeg,
        OutcomeLegOrderConstraints, OutcomeLegRole, PayoutMatrix, PolymarketDiscoveryScopeEvidence,
        PositiveSideBinding, PriceScaleAssertionSource, RoleBindingProof, SettlementRules,
        SettlementSourceKind, TerminalPayoutDerivation, TerminalState, TerminalStateConvention,
        TerminalStateKind, ValidatedOutcomeGroup, build_leg_map, canonical_fingerprint,
        derive_standard_payout_matrix, expected_metadata_fingerprint,
    },
    bolt_v3_providers::polymarket,
};

#[derive(Debug, Clone)]
pub struct PolymarketOutcomeGroupInput<'a> {
    pub source: &'a OutcomeGroupSourceConfig,
    pub metadata_loaded_unix_ms: u64,
    pub now_unix_ms: u64,
    pub metadata_ttl_ms: u64,
    pub markets: Vec<PolymarketGammaMarketMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketGammaMarketMetadata {
    pub condition_id: String,
    pub market_slug: String,
    pub question: String,
    pub terminal_state_label: String,
    pub neg_risk_market_id: Option<String>,
    pub legs: Vec<PolymarketGammaLegMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketGammaLegMetadata {
    pub native_leg_id: String,
    pub instrument_id: InstrumentId,
    pub outcome_label: String,
    pub side_label: String,
    pub settlement_asset_id: String,
    pub quantity_step: Decimal,
    pub payout_per_contract: Decimal,
    pub price_units_per_payout: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolymarketOutcomeGroupError {
    EmptyMarkets,
    StaleMetadata,
    MissingNegRiskMarketId { market_slug: String },
    GroupingIdentityConflict,
    ExpectedNegRiskMismatch,
    TerminalStateMismatch,
    RoleBindingMismatch,
    UnmappedRoleBinding,
    MissingNonStandardPayout,
    MissingOrderConstraints,
    MixedSettlementAsset,
    InvalidDecimal { field: &'static str, value: String },
    Validation(OutcomeGroupValidationError),
    UnsupportedSourceKind,
}

impl PolymarketOutcomeGroupError {
    pub fn is_missing_neg_risk_market_id(&self) -> bool {
        matches!(self, Self::MissingNegRiskMarketId { .. })
    }

    pub fn is_grouping_identity_conflict(&self) -> bool {
        matches!(self, Self::GroupingIdentityConflict)
    }

    pub fn is_expected_neg_risk_mismatch(&self) -> bool {
        matches!(self, Self::ExpectedNegRiskMismatch)
    }

    pub fn is_role_binding_mismatch(&self) -> bool {
        matches!(self, Self::RoleBindingMismatch)
    }

    pub fn is_unmapped_role_binding(&self) -> bool {
        matches!(self, Self::UnmappedRoleBinding)
    }

    pub fn is_terminal_state_mismatch(&self) -> bool {
        matches!(self, Self::TerminalStateMismatch)
    }

    pub fn is_missing_non_standard_payout(&self) -> bool {
        matches!(self, Self::MissingNonStandardPayout)
    }

    pub fn is_stale_metadata(&self) -> bool {
        matches!(self, Self::StaleMetadata)
    }
}

impl From<OutcomeGroupValidationError> for PolymarketOutcomeGroupError {
    fn from(value: OutcomeGroupValidationError) -> Self {
        Self::Validation(value)
    }
}

pub fn normalize_polymarket_outcome_group(
    input: PolymarketOutcomeGroupInput<'_>,
) -> Result<OutcomeGroup, PolymarketOutcomeGroupError> {
    validate_metadata_freshness(&input)?;
    validate_source_kind(input.source.kind)?;

    let neg_risk_market_id = shared_neg_risk_market_id(&input.markets)?;
    if input
        .source
        .expected_neg_risk_market_id
        .as_ref()
        .is_some_and(|expected| expected != &neg_risk_market_id)
    {
        return Err(PolymarketOutcomeGroupError::ExpectedNegRiskMismatch);
    }

    let standard_terminal_labels = standard_terminal_labels(input.source)?;
    validate_market_terminal_states(&input.markets, &standard_terminal_labels)?;

    let settlement_rules_block = input
        .source
        .settlement_rules
        .as_ref()
        .ok_or(PolymarketOutcomeGroupError::MissingNonStandardPayout)?;
    let non_standard_payouts = settlement_rules_block
        .non_standard_terminal_payouts
        .as_ref()
        .filter(|payouts| !payouts.is_empty())
        .ok_or(PolymarketOutcomeGroupError::MissingNonStandardPayout)?;
    let terminal_states = build_terminal_states(
        &standard_terminal_labels,
        non_standard_payouts,
        settlement_rules_block.void_policy,
    )?;

    let native_leg_ids = native_leg_ids(&input.markets)?;
    let role_bindings = input
        .source
        .role_bindings
        .as_ref()
        .ok_or(PolymarketOutcomeGroupError::RoleBindingMismatch)?;
    let positive_side_bindings = positive_side_bindings(role_bindings);
    let role_by_native_leg_id =
        role_by_native_leg_id(role_bindings, &standard_terminal_labels, &native_leg_ids)?;

    let settlement_asset_id = shared_settlement_asset_id(&input.markets)?;
    let tradable_legs = build_tradable_legs(
        input.source,
        &input.markets,
        &settlement_asset_id,
        &role_by_native_leg_id,
        &standard_terminal_labels,
    )?;
    let mut payout_matrix = derive_standard_payout_matrix(
        &terminal_states,
        &tradable_legs,
        TerminalStateConvention::ExactlyOneWinner,
    )?;
    let attested_vectors =
        build_non_standard_vectors(non_standard_payouts, &tradable_legs, &mut payout_matrix)?;

    let role_binding_proof = RoleBindingProof::OperatorAttested {
        attestation_id: input.source.source_id.clone(),
        positive_side_bindings,
        attestation_sha256: role_bindings.attestation_sha256.clone(),
        proof_fingerprint: role_binding_proof_fingerprint(input.source, role_bindings),
    };
    let grouping_proof = grouping_proof(input.source, &input.markets, &neg_risk_market_id)?;

    let mut group = OutcomeGroup {
        group_id: neg_risk_market_id,
        source_client_id: input.source.client_id,
        venue: Venue::from(polymarket::KEY),
        source_kind: SharedSourceKind::Polymarket,
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
    input: &PolymarketOutcomeGroupInput<'_>,
) -> Result<(), PolymarketOutcomeGroupError> {
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
        return Err(PolymarketOutcomeGroupError::StaleMetadata);
    }
    Ok(())
}

fn validate_source_kind(kind: SourceConfigKind) -> Result<(), PolymarketOutcomeGroupError> {
    match kind {
        SourceConfigKind::GammaEvent
        | SourceConfigKind::GammaMarketSlug
        | SourceConfigKind::GammaQuery => Ok(()),
        SourceConfigKind::Hip4 => Err(PolymarketOutcomeGroupError::UnsupportedSourceKind),
    }
}

fn shared_neg_risk_market_id(
    markets: &[PolymarketGammaMarketMetadata],
) -> Result<String, PolymarketOutcomeGroupError> {
    let mut shared = None::<String>;
    for market in markets {
        let value = market
            .neg_risk_market_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| PolymarketOutcomeGroupError::MissingNegRiskMarketId {
                market_slug: market.market_slug.clone(),
            })?;
        match shared.as_ref() {
            Some(previous) if previous != value => {
                return Err(PolymarketOutcomeGroupError::GroupingIdentityConflict);
            }
            Some(_) => {}
            None => shared = Some(value.clone()),
        }
    }
    shared.ok_or(PolymarketOutcomeGroupError::EmptyMarkets)
}

fn standard_terminal_labels(
    source: &OutcomeGroupSourceConfig,
) -> Result<BTreeSet<String>, PolymarketOutcomeGroupError> {
    let labels = source
        .terminal_state_labels
        .as_ref()
        .filter(|labels| !labels.is_empty())
        .ok_or(PolymarketOutcomeGroupError::TerminalStateMismatch)?;
    let mut set = BTreeSet::new();
    for label in labels {
        if label.trim().is_empty() || !set.insert(label.clone()) {
            return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
        }
    }
    Ok(set)
}

fn validate_market_terminal_states(
    markets: &[PolymarketGammaMarketMetadata],
    standard_terminal_labels: &BTreeSet<String>,
) -> Result<(), PolymarketOutcomeGroupError> {
    let mut market_labels = BTreeSet::new();
    for market in markets {
        if !market_labels.insert(market.terminal_state_label.clone()) {
            return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
        }
        for leg in &market.legs {
            if leg.outcome_label != market.terminal_state_label {
                return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
            }
        }
    }
    if &market_labels != standard_terminal_labels {
        return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
    }
    Ok(())
}

fn build_terminal_states(
    standard_terminal_labels: &BTreeSet<String>,
    non_standard_payouts: &BTreeMap<String, OutcomeGroupNonStandardTerminalPayoutBlock>,
    void_policy: OutcomeGroupVoidPolicy,
) -> Result<BTreeMap<String, TerminalState>, PolymarketOutcomeGroupError> {
    let mut terminal_states = BTreeMap::new();
    for label in standard_terminal_labels {
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
        if terminal_state_id != &payout.terminal_state_label {
            return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
        }
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
            return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
        }
    }

    Ok(terminal_states)
}

fn native_leg_ids(
    markets: &[PolymarketGammaMarketMetadata],
) -> Result<BTreeSet<String>, PolymarketOutcomeGroupError> {
    let mut native_leg_ids = BTreeSet::new();
    for market in markets {
        for leg in &market.legs {
            if leg.native_leg_id.trim().is_empty()
                || !native_leg_ids.insert(leg.native_leg_id.clone())
            {
                return Err(PolymarketOutcomeGroupError::RoleBindingMismatch);
            }
        }
    }
    Ok(native_leg_ids)
}

fn positive_side_bindings(
    role_bindings: &OutcomeGroupRoleBindingsBlock,
) -> Vec<PositiveSideBinding> {
    role_bindings
        .legs
        .iter()
        .map(|leg| PositiveSideBinding {
            terminal_state_label: leg.terminal_state_label.clone(),
            pays_on_leg: AttestedLegRef::NativeLegId(
                leg.pays_on_terminal_state_native_leg_id.clone(),
            ),
            pays_unless_leg: AttestedLegRef::NativeLegId(
                leg.pays_unless_terminal_state_native_leg_id.clone(),
            ),
        })
        .collect()
}

fn role_by_native_leg_id(
    role_bindings: &OutcomeGroupRoleBindingsBlock,
    standard_terminal_labels: &BTreeSet<String>,
    native_leg_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, OutcomeLegRole>, PolymarketOutcomeGroupError> {
    let mut seen_terminal_labels = BTreeSet::new();
    let mut roles = BTreeMap::new();

    for leg in &role_bindings.legs {
        if !standard_terminal_labels.contains(&leg.terminal_state_label)
            || !seen_terminal_labels.insert(leg.terminal_state_label.clone())
        {
            return Err(PolymarketOutcomeGroupError::RoleBindingMismatch);
        }
        if !native_leg_ids.contains(&leg.pays_on_terminal_state_native_leg_id)
            || !native_leg_ids.contains(&leg.pays_unless_terminal_state_native_leg_id)
        {
            return Err(PolymarketOutcomeGroupError::UnmappedRoleBinding);
        }
        if roles
            .insert(
                leg.pays_on_terminal_state_native_leg_id.clone(),
                OutcomeLegRole::PaysOnTerminalState(leg.terminal_state_label.clone()),
            )
            .is_some()
            || roles
                .insert(
                    leg.pays_unless_terminal_state_native_leg_id.clone(),
                    OutcomeLegRole::PaysUnlessTerminalState(leg.terminal_state_label.clone()),
                )
                .is_some()
        {
            return Err(PolymarketOutcomeGroupError::RoleBindingMismatch);
        }
    }

    if &seen_terminal_labels != standard_terminal_labels || roles.len() != native_leg_ids.len() {
        return Err(PolymarketOutcomeGroupError::RoleBindingMismatch);
    }
    Ok(roles)
}

fn shared_settlement_asset_id(
    markets: &[PolymarketGammaMarketMetadata],
) -> Result<String, PolymarketOutcomeGroupError> {
    let mut settlement_asset_id = None::<String>;
    for market in markets {
        for leg in &market.legs {
            match settlement_asset_id.as_ref() {
                Some(previous) if previous != &leg.settlement_asset_id => {
                    return Err(PolymarketOutcomeGroupError::MixedSettlementAsset);
                }
                Some(_) => {}
                None => settlement_asset_id = Some(leg.settlement_asset_id.clone()),
            }
        }
    }
    settlement_asset_id.ok_or(PolymarketOutcomeGroupError::EmptyMarkets)
}

fn build_tradable_legs(
    source: &OutcomeGroupSourceConfig,
    markets: &[PolymarketGammaMarketMetadata],
    settlement_asset_id: &str,
    role_by_native_leg_id: &BTreeMap<String, OutcomeLegRole>,
    standard_terminal_labels: &BTreeSet<String>,
) -> Result<BTreeMap<String, OutcomeLeg>, PolymarketOutcomeGroupError> {
    let mut legs = Vec::new();
    for market in markets {
        for leg in &market.legs {
            if !standard_terminal_labels.contains(&leg.outcome_label) {
                return Err(PolymarketOutcomeGroupError::TerminalStateMismatch);
            }
            let leg_role = role_by_native_leg_id
                .get(&leg.native_leg_id)
                .cloned()
                .ok_or(PolymarketOutcomeGroupError::RoleBindingMismatch)?;
            legs.push(OutcomeLeg {
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
    }
    build_leg_map(legs).map_err(PolymarketOutcomeGroupError::Validation)
}

fn price_scale_evidence(leg: &PolymarketGammaLegMetadata) -> NormalizedPriceScaleEvidence {
    NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
        settlement_asset_id: leg.settlement_asset_id.clone(),
        payout_per_contract: leg.payout_per_contract,
        price_units_per_payout: leg.price_units_per_payout,
        assertion_source: PriceScaleAssertionSource::VenueStructuredFields {
            proof_fingerprint: canonical_fingerprint(vec![
                CanonicalField::new(["price_scale", "native_leg_id"], &leg.native_leg_id),
                CanonicalField::new(
                    ["price_scale", "settlement_asset_id"],
                    &leg.settlement_asset_id,
                ),
                CanonicalField::new(
                    ["price_scale", "payout_per_contract"],
                    leg.payout_per_contract,
                ),
                CanonicalField::new(
                    ["price_scale", "price_units_per_payout"],
                    leg.price_units_per_payout,
                ),
            ]),
        },
    }
}

fn order_constraints_for_leg(
    source: &OutcomeGroupSourceConfig,
    leg: &PolymarketGammaLegMetadata,
) -> Result<OutcomeLegOrderConstraints, PolymarketOutcomeGroupError> {
    let constraints = source
        .order_constraints
        .as_ref()
        .ok_or(PolymarketOutcomeGroupError::MissingOrderConstraints)?;
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
        constraint_source: OrderConstraintSource::ConfigFloorWithNtPrecision {
            source_id: source.source_id.clone(),
        },
    })
}

fn build_non_standard_vectors(
    non_standard_payouts: &BTreeMap<String, OutcomeGroupNonStandardTerminalPayoutBlock>,
    tradable_legs: &BTreeMap<String, OutcomeLeg>,
    payout_matrix: &mut PayoutMatrix,
) -> Result<Vec<AttestedPayoutVector>, PolymarketOutcomeGroupError> {
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
                return Err(PolymarketOutcomeGroupError::MissingNonStandardPayout);
            }
        }

        let mut cols = Vec::with_capacity(payout_matrix.cols.len());
        let mut payouts = Vec::with_capacity(payout_matrix.cols.len());
        for leg_id in &payout_matrix.cols {
            let leg = &tradable_legs[leg_id];
            let key = (leg.outcome_label.clone(), leg.side_label.clone());
            let payout_per_unit = configured_payouts
                .remove(&key)
                .ok_or(PolymarketOutcomeGroupError::MissingNonStandardPayout)?;
            cols.push(AttestedLegRef::OutcomeAndSide {
                outcome_label: leg.outcome_label.clone(),
                side_label: leg.side_label.clone(),
            });
            payouts.push(payout_per_unit);
        }
        if !configured_payouts.is_empty() {
            return Err(PolymarketOutcomeGroupError::MissingNonStandardPayout);
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

fn grouping_proof(
    source: &OutcomeGroupSourceConfig,
    markets: &[PolymarketGammaMarketMetadata],
    neg_risk_market_id: &str,
) -> Result<GroupingProof, PolymarketOutcomeGroupError> {
    let market_slugs = markets
        .iter()
        .map(|market| market.market_slug.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let condition_ids = markets
        .iter()
        .map(|market| market.condition_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(GroupingProof::PolymarketNegRisk {
        neg_risk_market_id: neg_risk_market_id.to_string(),
        discovery_scope: PolymarketDiscoveryScopeEvidence {
            source_id: source.source_id.clone(),
            event_slugs: cloned_vec(source.event_slugs.as_ref()),
            market_slugs: cloned_vec(source.market_slugs.as_ref()),
            gamma_query_fingerprint: source.gamma_query.as_ref().map(gamma_query_fingerprint),
            cache_key_fingerprint: polymarket_cache_key_fingerprint(
                &source.source_id,
                neg_risk_market_id,
                &market_slugs,
                &condition_ids,
            ),
        },
        market_slugs: market_slugs.clone(),
        proof_fingerprint: polymarket_grouping_proof_fingerprint(
            &source.source_id,
            neg_risk_market_id,
            &market_slugs,
            &condition_ids,
        ),
    })
}

fn polymarket_grouping_proof_fingerprint(
    source_id: &str,
    neg_risk_market_id: &str,
    market_slugs: &[String],
    condition_ids: &[String],
) -> String {
    let mut fields = vec![
        CanonicalField::new(["grouping", "source_id"], source_id),
        CanonicalField::new(["grouping", "neg_risk_market_id"], neg_risk_market_id),
    ];
    for (index, market_slug) in market_slugs.iter().enumerate() {
        fields.push(CanonicalField::owned(
            vec![
                "grouping".to_string(),
                "market_slugs".to_string(),
                index.to_string(),
            ],
            market_slug,
        ));
    }
    for (index, condition_id) in condition_ids.iter().enumerate() {
        fields.push(CanonicalField::owned(
            vec![
                "grouping".to_string(),
                "condition_ids".to_string(),
                index.to_string(),
            ],
            condition_id,
        ));
    }
    canonical_fingerprint(fields)
}

fn polymarket_cache_key_fingerprint(
    source_id: &str,
    neg_risk_market_id: &str,
    market_slugs: &[String],
    condition_ids: &[String],
) -> String {
    let mut fields = vec![
        CanonicalField::new(["cache_key", "source_id"], source_id),
        CanonicalField::new(["cache_key", "neg_risk_market_id"], neg_risk_market_id),
    ];
    for (index, market_slug) in market_slugs.iter().enumerate() {
        fields.push(CanonicalField::owned(
            vec![
                "cache_key".to_string(),
                "market_slugs".to_string(),
                index.to_string(),
            ],
            market_slug,
        ));
    }
    for (index, condition_id) in condition_ids.iter().enumerate() {
        fields.push(CanonicalField::owned(
            vec![
                "cache_key".to_string(),
                "condition_ids".to_string(),
                index.to_string(),
            ],
            condition_id,
        ));
    }
    canonical_fingerprint(fields)
}

fn gamma_query_fingerprint(
    query: &crate::bolt_v3_outcome_group_sources::GammaQueryBlock,
) -> String {
    canonical_fingerprint(vec![
        CanonicalField::new(
            ["gamma_query", "search"],
            optional_string(query.search.as_ref()),
        ),
        CanonicalField::new(
            ["gamma_query", "event_query"],
            optional_string(query.event_query.as_ref()),
        ),
        CanonicalField::new(
            ["gamma_query", "market_query"],
            optional_string(query.market_query.as_ref()),
        ),
        CanonicalField::new(
            ["gamma_query", "tag_id"],
            optional_string(query.tag_id.as_ref()),
        ),
        CanonicalField::new(
            ["gamma_query", "sports_market_types"],
            joined_optional_values(query.sports_market_types.as_ref()),
        ),
        CanonicalField::new(
            ["gamma_query", "max_events"],
            optional_usize_string(query.max_events),
        ),
        CanonicalField::new(["gamma_query", "max_markets"], query.max_markets),
    ])
}

fn cloned_vec(values: Option<&Vec<String>>) -> Vec<String> {
    match values {
        Some(values) => values.clone(),
        None => Vec::new(),
    }
}

fn optional_string(value: Option<&String>) -> String {
    match value {
        Some(value) => value.clone(),
        None => String::new(),
    }
}

fn joined_optional_values(values: Option<&Vec<String>>) -> String {
    match values {
        Some(values) => values.join(","),
        None => String::new(),
    }
}

fn optional_usize_string(value: Option<usize>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn role_binding_proof_fingerprint(
    source: &OutcomeGroupSourceConfig,
    role_bindings: &OutcomeGroupRoleBindingsBlock,
) -> String {
    canonical_fingerprint(vec![
        CanonicalField::new(["role_binding", "source_id"], &source.source_id),
        CanonicalField::new(
            ["role_binding", "attestation_sha256"],
            &role_bindings.attestation_sha256,
        ),
        CanonicalField::new(["role_binding", "binding_count"], role_bindings.legs.len()),
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
) -> Result<Decimal, PolymarketOutcomeGroupError> {
    let value = value.ok_or(PolymarketOutcomeGroupError::MissingOrderConstraints)?;
    parse_decimal(field, value)
}

fn optional_decimal(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<Decimal>, PolymarketOutcomeGroupError> {
    value.map(|value| parse_decimal(field, value)).transpose()
}

fn parse_decimal(field: &'static str, value: &str) -> Result<Decimal, PolymarketOutcomeGroupError> {
    Decimal::from_str(value).map_err(|_| PolymarketOutcomeGroupError::InvalidDecimal {
        field,
        value: value.to_string(),
    })
}
