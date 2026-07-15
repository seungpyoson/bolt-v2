use std::collections::{BTreeMap, BTreeSet};

use nautilus_model::identifiers::{ClientId, InstrumentId, Venue};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

use crate::bolt_v3_outcome_group_proofs::{NegRiskGroupingProof, StructuredOutcomeGroupingProof};

const DISALLOWED_OPERATOR_FORMAT_CHARS: &str = "\u{00ad}\u{034f}\u{061c}\u{115f}\u{1160}\u{17b4}\u{17b5}\u{180e}\u{200b}\u{200c}\u{200d}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2060}\u{2061}\u{2062}\u{2063}\u{2064}\u{2066}\u{2067}\u{2068}\u{2069}\u{206a}\u{206b}\u{206c}\u{206d}\u{206e}\u{206f}\u{feff}";

pub type OutcomeGroupId = String;
pub type OutcomeLegId = String;
pub type TerminalStateId = String;
pub type SettlementAssetId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeGroup {
    pub group_id: OutcomeGroupId,
    pub source_client_id: ClientId,
    pub venue: Venue,
    pub source_kind: OutcomeGroupSourceKind,
    pub settlement_asset_id: SettlementAssetId,
    pub terminal_states: BTreeMap<TerminalStateId, TerminalState>,
    pub tradable_legs: BTreeMap<OutcomeLegId, OutcomeLeg>,
    pub payout_matrix: PayoutMatrix,
    pub grouping_proof: Option<GroupingProof>,
    pub role_binding_proof: Option<RoleBindingProof>,
    pub settlement_rules: SettlementRules,
    pub freshness_source_id: String,
    pub metadata_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutcomeGroupSourceKind {
    Polymarket,
    Hyperliquid,
    OperatorAttested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeLeg {
    pub leg_id: OutcomeLegId,
    pub instrument_id: InstrumentId,
    pub native_leg_id: String,
    pub settlement_asset_id: SettlementAssetId,
    pub outcome_label: String,
    pub side_label: String,
    pub leg_role: OutcomeLegRole,
    pub price_scale: NormalizedPriceScaleEvidence,
    pub order_constraints: OutcomeLegOrderConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeLegRole {
    PaysOnTerminalState(TerminalStateId),
    PaysUnlessTerminalState(TerminalStateId),
    UnsupportedMultiState {
        terminal_state_ids: Vec<TerminalStateId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalState {
    pub state_id: TerminalStateId,
    pub label: String,
    pub kind: TerminalStateKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TerminalStateKind {
    Standard,
    Void,
    Fallback,
}

impl TerminalStateKind {
    fn is_standard(self) -> bool {
        self == Self::Standard
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutMatrix {
    pub cols: Vec<OutcomeLegId>,
    pub payout_per_unit_by_state: BTreeMap<TerminalStateId, Vec<Decimal>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupingProof {
    PolymarketNegRisk(NegRiskGroupingProof),
    HyperliquidOutcome(StructuredOutcomeGroupingProof),
    OperatorAttested {
        settlement_contract_id: String,
        attestation_id: String,
        attestation_sha256: String,
        proof_fingerprint: String,
    },
}

enum ConcreteGroupingProofRef<'a> {
    NegRisk(&'a NegRiskGroupingProof),
    StructuredOutcome(&'a StructuredOutcomeGroupingProof),
}

impl GroupingProof {
    fn concrete_payload(&self) -> Option<ConcreteGroupingProofRef<'_>> {
        match self {
            Self::PolymarketNegRisk(proof) => Some(ConcreteGroupingProofRef::NegRisk(proof)),
            Self::HyperliquidOutcome(proof) => {
                Some(ConcreteGroupingProofRef::StructuredOutcome(proof))
            }
            Self::OperatorAttested { .. } => None,
        }
    }

    fn native_identity(&self) -> String {
        match self.concrete_payload() {
            Some(ConcreteGroupingProofRef::NegRisk(proof)) => proof.native_identity(),
            Some(ConcreteGroupingProofRef::StructuredOutcome(proof)) => proof.native_identity(),
            None => {
                let Self::OperatorAttested {
                    settlement_contract_id,
                    ..
                } = self
                else {
                    unreachable!()
                };
                format!("operator:{settlement_contract_id}")
            }
        }
    }

    fn validate_sha_fields(&self) -> Result<(), OutcomeGroupValidationError> {
        match self.concrete_payload() {
            Some(ConcreteGroupingProofRef::NegRisk(proof)) => {
                let NegRiskGroupingProof {
                    proof_fingerprint,
                    discovery_scope,
                    ..
                } = proof;
                validate_sha256_field("grouping_proof.proof_fingerprint", proof_fingerprint)?;
                if let Some(fingerprint) = discovery_scope.gamma_query_fingerprint.as_deref() {
                    validate_sha256_field(
                        "grouping_proof.discovery_scope.gamma_query_fingerprint",
                        fingerprint,
                    )?;
                }
                validate_sha256_field(
                    "grouping_proof.discovery_scope.cache_key_fingerprint",
                    &discovery_scope.cache_key_fingerprint,
                )?;
                return Ok(());
            }
            Some(ConcreteGroupingProofRef::StructuredOutcome(proof)) => {
                validate_structured_outcome_sha_fields(proof)?;
                return Ok(());
            }
            None => {}
        }
        match self {
            Self::OperatorAttested {
                attestation_sha256,
                proof_fingerprint,
                ..
            } => {
                validate_sha256_field("grouping_proof.attestation_sha256", attestation_sha256)?;
                validate_sha256_field("grouping_proof.proof_fingerprint", proof_fingerprint)?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

fn validate_structured_outcome_sha_fields(
    proof: &StructuredOutcomeGroupingProof,
) -> Result<(), OutcomeGroupValidationError> {
    let StructuredOutcomeGroupingProof {
        proof_fingerprint, ..
    } = proof;
    validate_sha256_field("grouping_proof.proof_fingerprint", proof_fingerprint)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleBindingProof {
    OperatorAttested {
        attestation_id: String,
        positive_side_bindings: Vec<PositiveSideBinding>,
        attestation_sha256: String,
        proof_fingerprint: String,
    },
    VenueStructuredFields {
        source_id: String,
        question: u32,
        outcome_indices: Vec<u32>,
        proof_fingerprint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositiveSideBinding {
    pub terminal_state_label: String,
    pub pays_on_leg: AttestedLegRef,
    pub pays_unless_leg: AttestedLegRef,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttestedLegRef {
    NativeLegId(String),
    OutcomeAndSide {
        outcome_label: String,
        side_label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementRules {
    pub terminal_state_convention: TerminalStateConvention,
    pub settlement_source_kind: SettlementSourceKind,
    pub non_standard_terminal_payouts: Vec<AttestedPayoutVector>,
    pub terminal_payout_derivation: TerminalPayoutDerivation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalStateConvention {
    ExactlyOneWinner,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SettlementSourceKind {
    VenueStructuredFields,
    OperatorAttested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalPayoutDerivation {
    StandardRowsPlusAttestedVectors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedPayoutVector {
    pub terminal_state_id: TerminalStateId,
    pub label: String,
    pub cols: Vec<AttestedLegRef>,
    pub payouts: Vec<Decimal>,
    pub refund_convention: String,
    pub attestation_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedPriceScaleEvidence {
    BinaryOnePayoutEqualsOneSettlementUnit {
        settlement_asset_id: SettlementAssetId,
        payout_per_contract: Decimal,
        price_units_per_payout: Decimal,
        assertion_source: PriceScaleAssertionSource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriceScaleAssertionSource {
    VenueStructuredFields { proof_fingerprint: String },
    OperatorAttested { attestation_sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeLegOrderConstraints {
    pub min_quantity: Decimal,
    pub min_notional: Option<Decimal>,
    pub quantity_step: Decimal,
    pub constraint_source: OrderConstraintSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderConstraintSource {
    ConfigFloorWithNtPrecision { source_id: String },
    NtInstrumentWithConfigFloor { source_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalField {
    path: Vec<String>,
    value: String,
}

impl CanonicalField {
    pub fn new<const N: usize>(path: [&str; N], value: impl ToString) -> Self {
        Self {
            path: path.into_iter().map(str::to_string).collect(),
            value: value.to_string(),
        }
    }

    pub(crate) fn owned(path: Vec<String>, value: impl ToString) -> Self {
        Self {
            path,
            value: value.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeGroupValidationError {
    MissingGroupingProof,
    DuplicateGroupingIdentityConflict,
    DuplicateLegId(OutcomeLegId),
    EmptyTerminalStates,
    UnsupportedTerminalStateConvention,
    UnsupportedMultiStateLegRole(OutcomeLegId),
    MissingRoleBindingProof,
    MissingPositiveSideBinding(TerminalStateId),
    DuplicatePositiveSideBinding(TerminalStateId),
    UnknownTerminalStateLabel(String),
    MissingPayoutRow(TerminalStateId),
    MissingNonStandardPayoutVector(TerminalStateId),
    DuplicateNonStandardPayoutVector(TerminalStateId),
    PayoutMatrixDimensionMismatch { terminal_state_id: TerminalStateId },
    PayoutMatrixValueMismatch { terminal_state_id: TerminalStateId },
    UnknownTerminalState(TerminalStateId),
    UnknownLeg(OutcomeLegId),
    AttestedLegUnknown,
    AttestedLegAmbiguous,
    AttestedColumnOrderMismatch,
    AttestationHashMismatch { field: &'static str },
    InvalidSha256 { field: &'static str },
    OutOfBoundsPayout { terminal_state_id: TerminalStateId },
    MixedSettlementAsset { leg_id: OutcomeLegId },
    InvalidNormalizedPriceScaleEvidence,
    InvalidOrderConstraintFloor { leg_id: OutcomeLegId },
    InvalidOperatorString,
    MetadataFingerprintMismatch,
}

impl OutcomeGroupValidationError {
    pub fn is_missing_grouping_proof(&self) -> bool {
        matches!(self, Self::MissingGroupingProof)
    }

    pub fn is_grouping_identity_conflict(&self) -> bool {
        matches!(self, Self::DuplicateGroupingIdentityConflict)
    }

    pub fn is_duplicate_leg_id(&self) -> bool {
        matches!(self, Self::DuplicateLegId(_))
    }

    pub fn is_empty_terminal_states(&self) -> bool {
        matches!(self, Self::EmptyTerminalStates)
    }

    pub fn is_unsupported_convention(&self) -> bool {
        matches!(self, Self::UnsupportedTerminalStateConvention)
    }

    pub fn is_multi_state_leg_role(&self) -> bool {
        matches!(self, Self::UnsupportedMultiStateLegRole(_))
    }

    pub fn is_missing_role_binding_proof(&self) -> bool {
        matches!(self, Self::MissingRoleBindingProof)
    }

    pub fn is_missing_payout_row(&self) -> bool {
        matches!(self, Self::MissingPayoutRow(_))
    }

    pub fn is_missing_non_standard_vector(&self) -> bool {
        matches!(self, Self::MissingNonStandardPayoutVector(_))
    }

    pub fn is_duplicate_non_standard_vector(&self) -> bool {
        matches!(self, Self::DuplicateNonStandardPayoutVector(_))
    }

    pub fn is_matrix_dimension_mismatch(&self) -> bool {
        matches!(self, Self::PayoutMatrixDimensionMismatch { .. })
    }

    pub fn is_matrix_value_mismatch(&self) -> bool {
        matches!(self, Self::PayoutMatrixValueMismatch { .. })
    }

    pub fn is_unknown_terminal_state(&self) -> bool {
        matches!(self, Self::UnknownTerminalState(_))
    }

    pub fn is_unknown_leg(&self) -> bool {
        matches!(self, Self::UnknownLeg(_))
    }

    pub fn is_attested_leg_unknown(&self) -> bool {
        matches!(self, Self::AttestedLegUnknown)
    }

    pub fn is_attested_leg_ambiguous(&self) -> bool {
        matches!(self, Self::AttestedLegAmbiguous)
    }

    pub fn is_attested_column_order_mismatch(&self) -> bool {
        matches!(self, Self::AttestedColumnOrderMismatch)
    }

    pub fn is_attestation_hash_mismatch(&self) -> bool {
        matches!(self, Self::AttestationHashMismatch { .. })
    }

    pub fn is_out_of_bounds_payout(&self) -> bool {
        matches!(self, Self::OutOfBoundsPayout { .. })
    }

    pub fn is_invalid_price_scale(&self) -> bool {
        matches!(self, Self::InvalidNormalizedPriceScaleEvidence)
    }

    pub fn is_invalid_order_constraint(&self) -> bool {
        matches!(self, Self::InvalidOrderConstraintFloor { .. })
    }

    pub fn is_invalid_operator_string(&self) -> bool {
        matches!(self, Self::InvalidOperatorString)
    }

    pub fn is_mixed_settlement_asset(&self) -> bool {
        matches!(self, Self::MixedSettlementAsset { .. })
    }

    pub fn is_invalid_sha256(&self) -> bool {
        matches!(self, Self::InvalidSha256 { .. })
    }

    pub fn is_metadata_fingerprint_mismatch(&self) -> bool {
        matches!(self, Self::MetadataFingerprintMismatch)
    }

    pub fn is_unknown_terminal_label(&self) -> bool {
        matches!(self, Self::UnknownTerminalStateLabel(_))
    }
}

pub struct ValidatedOutcomeGroup;

impl ValidatedOutcomeGroup {
    pub fn validate(group: &OutcomeGroup) -> Result<(), OutcomeGroupValidationError> {
        if group.terminal_states.is_empty() {
            return Err(OutcomeGroupValidationError::EmptyTerminalStates);
        }

        let grouping_proof = group
            .grouping_proof
            .as_ref()
            .ok_or(OutcomeGroupValidationError::MissingGroupingProof)?;
        grouping_proof.validate_sha_fields()?;

        if !matches!(
            group.settlement_rules.terminal_state_convention,
            TerminalStateConvention::ExactlyOneWinner
        ) {
            return Err(OutcomeGroupValidationError::UnsupportedTerminalStateConvention);
        }

        validate_operator_strings(group)?;
        validate_leg_assets_scales_and_constraints(group)?;
        validate_payout_matrix_shape(group)?;
        validate_non_standard_payouts(group)?;
        validate_standard_rows(group)?;
        validate_role_bindings(group)?;

        let expected_metadata = expected_metadata_fingerprint(group);
        if !is_lowercase_sha256(&group.metadata_fingerprint)
            || group.metadata_fingerprint != expected_metadata
        {
            return Err(OutcomeGroupValidationError::MetadataFingerprintMismatch);
        }

        Ok(())
    }
}

pub fn build_leg_map(
    legs: Vec<OutcomeLeg>,
) -> Result<BTreeMap<OutcomeLegId, OutcomeLeg>, OutcomeGroupValidationError> {
    let mut map = BTreeMap::new();
    for leg in legs {
        let leg_id = leg.leg_id.clone();
        if map.insert(leg_id.clone(), leg).is_some() {
            return Err(OutcomeGroupValidationError::DuplicateLegId(leg_id));
        }
    }
    Ok(map)
}

pub fn validate_grouping_identity_set<'a>(
    groups: impl IntoIterator<Item = &'a OutcomeGroup>,
) -> Result<(), OutcomeGroupValidationError> {
    let mut seen = BTreeMap::<String, String>::new();
    for group in groups {
        let Some(grouping_proof) = group.grouping_proof.as_ref() else {
            continue;
        };
        let identity = grouping_proof.native_identity();
        let settlement_fingerprint = settlement_identity_fingerprint(group);
        if let Some(previous) = seen.insert(identity, settlement_fingerprint.clone())
            && previous != settlement_fingerprint
        {
            return Err(OutcomeGroupValidationError::DuplicateGroupingIdentityConflict);
        }
    }
    Ok(())
}

pub fn derive_standard_payout_matrix(
    terminal_states: &BTreeMap<TerminalStateId, TerminalState>,
    legs: &BTreeMap<OutcomeLegId, OutcomeLeg>,
    convention: TerminalStateConvention,
) -> Result<PayoutMatrix, OutcomeGroupValidationError> {
    if !matches!(convention, TerminalStateConvention::ExactlyOneWinner) {
        return Err(OutcomeGroupValidationError::UnsupportedTerminalStateConvention);
    }

    let cols = legs.keys().cloned().collect::<Vec<_>>();
    let mut rows = BTreeMap::new();
    for (terminal_state_id, terminal_state) in terminal_states {
        if !terminal_state.kind.is_standard() {
            continue;
        }
        let mut row = Vec::with_capacity(cols.len());
        for (leg_id, leg) in legs {
            row.push(payout_for_role(leg_id, &leg.leg_role, terminal_state_id)?);
        }
        rows.insert(terminal_state_id.clone(), row);
    }

    Ok(PayoutMatrix {
        cols,
        payout_per_unit_by_state: rows,
    })
}

pub fn expected_metadata_fingerprint(group: &OutcomeGroup) -> String {
    let mut fields = vec![
        CanonicalField::new(["group", "group_id"], &group.group_id),
        CanonicalField::new(
            ["group", "source_client_id"],
            group.source_client_id.to_string(),
        ),
        CanonicalField::new(["group", "venue"], group.venue.to_string()),
        CanonicalField::new(
            ["group", "source_kind"],
            source_kind_label(&group.source_kind),
        ),
        CanonicalField::new(["group", "settlement_asset_id"], &group.settlement_asset_id),
        CanonicalField::new(["group", "freshness_source_id"], &group.freshness_source_id),
    ];

    if let Some(grouping_proof) = group.grouping_proof.as_ref() {
        append_grouping_identity_fields(&mut fields, grouping_proof);
    }

    for (state_id, state) in &group.terminal_states {
        fields.push(CanonicalField::owned(
            vec![
                "terminal_states".to_string(),
                state_id.clone(),
                "label".to_string(),
            ],
            &state.label,
        ));
        fields.push(CanonicalField::owned(
            vec![
                "terminal_states".to_string(),
                state_id.clone(),
                "kind".to_string(),
            ],
            terminal_state_kind_label(state.kind),
        ));
    }

    for (leg_id, leg) in &group.tradable_legs {
        fields.push(CanonicalField::owned(
            vec![
                "legs".to_string(),
                leg_id.clone(),
                "instrument_id".to_string(),
            ],
            leg.instrument_id.to_string(),
        ));
        fields.push(CanonicalField::owned(
            vec![
                "legs".to_string(),
                leg_id.clone(),
                "native_leg_id".to_string(),
            ],
            &leg.native_leg_id,
        ));
        fields.push(CanonicalField::owned(
            vec![
                "legs".to_string(),
                leg_id.clone(),
                "settlement_asset_id".to_string(),
            ],
            &leg.settlement_asset_id,
        ));
        fields.push(CanonicalField::owned(
            vec![
                "legs".to_string(),
                leg_id.clone(),
                "outcome_label".to_string(),
            ],
            &leg.outcome_label,
        ));
        fields.push(CanonicalField::owned(
            vec!["legs".to_string(), leg_id.clone(), "side_label".to_string()],
            &leg.side_label,
        ));
        append_role_fields(
            &mut fields,
            vec!["legs".to_string(), leg_id.clone()],
            &leg.leg_role,
        );
        append_price_scale_metadata_fields(
            &mut fields,
            vec![
                "legs".to_string(),
                leg_id.clone(),
                "price_scale".to_string(),
            ],
            &leg.price_scale,
        );
    }
    append_payout_matrix_fields(
        &mut fields,
        vec!["payout_matrix".to_string()],
        &group.payout_matrix,
    );
    append_settlement_rules_fields(
        &mut fields,
        vec!["settlement_rules".to_string()],
        &group.settlement_rules,
    );
    if let Some(role_binding_proof) = group.role_binding_proof.as_ref() {
        append_role_binding_proof_fields(
            &mut fields,
            vec!["role_binding_proof".to_string()],
            role_binding_proof,
        );
    }

    canonical_fingerprint(fields)
}

pub fn canonical_fingerprint(fields: impl IntoIterator<Item = CanonicalField>) -> String {
    hex::encode(Sha256::digest(canonical_bytes(fields)))
}

pub fn role_binding_attestation_sha256(bindings: &[PositiveSideBinding]) -> String {
    let mut fields = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let prefix = vec!["positive_side_bindings".to_string(), index.to_string()];
        fields.push(CanonicalField::owned(
            prefixed_path(&prefix, "terminal_state_label"),
            &binding.terminal_state_label,
        ));
        append_attested_leg_ref_fields(
            &mut fields,
            prefixed_path(&prefix, "pays_on_leg"),
            &binding.pays_on_leg,
        );
        append_attested_leg_ref_fields(
            &mut fields,
            prefixed_path(&prefix, "pays_unless_leg"),
            &binding.pays_unless_leg,
        );
    }
    canonical_fingerprint(fields)
}

pub fn payout_vector_attestation_sha256(
    terminal_state_id: &str,
    label: &str,
    cols: &[AttestedLegRef],
    payouts: &[Decimal],
    refund_convention: &str,
) -> String {
    let mut fields = vec![
        CanonicalField::new(["terminal_state_id"], terminal_state_id),
        CanonicalField::new(["label"], label),
        CanonicalField::new(["refund_convention"], refund_convention),
    ];
    for (index, col) in cols.iter().enumerate() {
        append_attested_leg_ref_fields(
            &mut fields,
            vec!["cols".to_string(), index.to_string()],
            col,
        );
    }
    for (index, payout) in payouts.iter().enumerate() {
        fields.push(CanonicalField::owned(
            vec!["payouts".to_string(), index.to_string()],
            normalize_decimal(*payout),
        ));
    }
    canonical_fingerprint(fields)
}

pub fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_bytes(fields: impl IntoIterator<Item = CanonicalField>) -> Vec<u8> {
    let mut fields = fields.into_iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.value.cmp(&right.value))
    });

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"bolt-v3-outcome-group-canonical-v2\0");
    push_len(&mut bytes, fields.len());
    for field in fields {
        push_len(&mut bytes, field.path.len());
        for segment in field.path {
            push_bytes(&mut bytes, segment.as_bytes());
        }
        push_bytes(&mut bytes, field.value.as_bytes());
    }
    bytes
}

fn push_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    push_len(bytes, value.len());
    bytes.extend_from_slice(value);
}

fn push_len(bytes: &mut Vec<u8>, value: usize) {
    let value = value as u64;
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn validate_leg_assets_scales_and_constraints(
    group: &OutcomeGroup,
) -> Result<(), OutcomeGroupValidationError> {
    for (leg_id, leg) in &group.tradable_legs {
        if leg.settlement_asset_id != group.settlement_asset_id {
            return Err(OutcomeGroupValidationError::MixedSettlementAsset {
                leg_id: leg_id.clone(),
            });
        }
        validate_price_scale(&leg.price_scale, &group.settlement_asset_id)?;
        validate_order_constraints(leg_id, &leg.order_constraints)?;
        if matches!(leg.leg_role, OutcomeLegRole::UnsupportedMultiState { .. }) {
            return Err(OutcomeGroupValidationError::UnsupportedMultiStateLegRole(
                leg_id.clone(),
            ));
        }
    }
    Ok(())
}

fn validate_price_scale(
    price_scale: &NormalizedPriceScaleEvidence,
    group_settlement_asset_id: &str,
) -> Result<(), OutcomeGroupValidationError> {
    match price_scale {
        NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id,
            payout_per_contract,
            price_units_per_payout,
            assertion_source,
        } => {
            if settlement_asset_id != group_settlement_asset_id
                || *payout_per_contract != Decimal::ONE
                || *price_units_per_payout != Decimal::ONE
            {
                return Err(OutcomeGroupValidationError::InvalidNormalizedPriceScaleEvidence);
            }
            match assertion_source {
                PriceScaleAssertionSource::VenueStructuredFields { proof_fingerprint } => {
                    validate_sha256_field("price_scale.proof_fingerprint", proof_fingerprint)?;
                }
                PriceScaleAssertionSource::OperatorAttested { attestation_sha256 } => {
                    validate_sha256_field("price_scale.attestation_sha256", attestation_sha256)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_order_constraints(
    leg_id: &str,
    constraints: &OutcomeLegOrderConstraints,
) -> Result<(), OutcomeGroupValidationError> {
    if constraints.min_quantity <= Decimal::ZERO
        || constraints.quantity_step <= Decimal::ZERO
        || constraints
            .min_notional
            .is_some_and(|min_notional| min_notional <= Decimal::ZERO)
    {
        return Err(OutcomeGroupValidationError::InvalidOrderConstraintFloor {
            leg_id: leg_id.to_string(),
        });
    }
    Ok(())
}

fn validate_operator_strings(group: &OutcomeGroup) -> Result<(), OutcomeGroupValidationError> {
    validate_operator_text(&group.group_id)?;
    validate_operator_text(&group.settlement_asset_id)?;
    validate_operator_text(&group.freshness_source_id)?;

    for (terminal_state_id, terminal_state) in &group.terminal_states {
        validate_operator_text(terminal_state_id)?;
        validate_operator_text(&terminal_state.state_id)?;
        validate_operator_text(&terminal_state.label)?;
    }

    for (leg_id, leg) in &group.tradable_legs {
        validate_operator_text(leg_id)?;
        validate_operator_text(&leg.instrument_id.to_string())?;
        validate_operator_text(&leg.native_leg_id)?;
        validate_operator_text(&leg.settlement_asset_id)?;
        validate_operator_text(&leg.outcome_label)?;
        validate_operator_text(&leg.side_label)?;
        validate_leg_role_text(&leg.leg_role)?;
    }

    for col in &group.payout_matrix.cols {
        validate_operator_text(col)?;
    }
    for terminal_state_id in group.payout_matrix.payout_per_unit_by_state.keys() {
        validate_operator_text(terminal_state_id)?;
    }

    validate_grouping_proof_text(
        group
            .grouping_proof
            .as_ref()
            .ok_or(OutcomeGroupValidationError::MissingGroupingProof)?,
    )?;
    if let Some(role_binding_proof) = &group.role_binding_proof {
        validate_role_binding_proof_text(role_binding_proof)?;
    }
    for vector in &group.settlement_rules.non_standard_terminal_payouts {
        validate_operator_text(&vector.terminal_state_id)?;
        validate_operator_text(&vector.label)?;
        validate_operator_text(&vector.refund_convention)?;
        for col in &vector.cols {
            validate_attested_leg_ref_text(col)?;
        }
    }
    if let TerminalStateConvention::Unsupported(value) =
        &group.settlement_rules.terminal_state_convention
    {
        validate_operator_text(value)?;
    }

    Ok(())
}

fn validate_grouping_proof_text(
    grouping_proof: &GroupingProof,
) -> Result<(), OutcomeGroupValidationError> {
    if let Some(payload) = grouping_proof.concrete_payload() {
        if let ConcreteGroupingProofRef::NegRisk(proof) = payload {
            let NegRiskGroupingProof {
                neg_risk_market_id,
                discovery_scope,
                market_slugs,
                ..
            } = proof;
            validate_operator_text(neg_risk_market_id)?;
            validate_operator_text(&discovery_scope.source_id)?;
            for slug in &discovery_scope.event_slugs {
                validate_operator_text(slug)?;
            }
            for slug in &discovery_scope.market_slugs {
                validate_operator_text(slug)?;
            }
            for slug in market_slugs {
                validate_operator_text(slug)?;
            }
        }
        return Ok(());
    }
    match grouping_proof {
        GroupingProof::OperatorAttested {
            settlement_contract_id,
            attestation_id,
            ..
        } => {
            validate_operator_text(settlement_contract_id)?;
            validate_operator_text(attestation_id)?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn validate_role_binding_proof_text(
    role_binding_proof: &RoleBindingProof,
) -> Result<(), OutcomeGroupValidationError> {
    match role_binding_proof {
        RoleBindingProof::OperatorAttested {
            attestation_id,
            positive_side_bindings,
            ..
        } => {
            validate_operator_text(attestation_id)?;
            for binding in positive_side_bindings {
                validate_operator_text(&binding.terminal_state_label)?;
                validate_attested_leg_ref_text(&binding.pays_on_leg)?;
                validate_attested_leg_ref_text(&binding.pays_unless_leg)?;
            }
        }
        RoleBindingProof::VenueStructuredFields { source_id, .. } => {
            validate_operator_text(source_id)?;
        }
    }
    Ok(())
}

fn validate_leg_role_text(role: &OutcomeLegRole) -> Result<(), OutcomeGroupValidationError> {
    match role {
        OutcomeLegRole::PaysOnTerminalState(terminal_state_id)
        | OutcomeLegRole::PaysUnlessTerminalState(terminal_state_id) => {
            validate_operator_text(terminal_state_id)
        }
        OutcomeLegRole::UnsupportedMultiState { terminal_state_ids } => {
            for terminal_state_id in terminal_state_ids {
                validate_operator_text(terminal_state_id)?;
            }
            Ok(())
        }
    }
}

fn validate_attested_leg_ref_text(
    leg_ref: &AttestedLegRef,
) -> Result<(), OutcomeGroupValidationError> {
    match leg_ref {
        AttestedLegRef::NativeLegId(native_leg_id) => validate_operator_text(native_leg_id),
        AttestedLegRef::OutcomeAndSide {
            outcome_label,
            side_label,
        } => {
            validate_operator_text(outcome_label)?;
            validate_operator_text(side_label)
        }
    }
}

fn validate_operator_text(value: &str) -> Result<(), OutcomeGroupValidationError> {
    if value
        .chars()
        .any(|ch| ch.is_control() || DISALLOWED_OPERATOR_FORMAT_CHARS.contains(ch))
    {
        return Err(OutcomeGroupValidationError::InvalidOperatorString);
    }
    Ok(())
}

fn validate_payout_matrix_shape(group: &OutcomeGroup) -> Result<(), OutcomeGroupValidationError> {
    let mut seen_cols = BTreeSet::new();
    for leg_id in &group.payout_matrix.cols {
        if !group.tradable_legs.contains_key(leg_id) {
            return Err(OutcomeGroupValidationError::UnknownLeg(leg_id.clone()));
        }
        seen_cols.insert(leg_id);
    }
    if seen_cols.len() != group.payout_matrix.cols.len()
        || group.payout_matrix.cols.len() != group.tradable_legs.len()
    {
        return Err(OutcomeGroupValidationError::PayoutMatrixDimensionMismatch {
            terminal_state_id: String::new(),
        });
    }

    for terminal_state_id in group.payout_matrix.payout_per_unit_by_state.keys() {
        if !group.terminal_states.contains_key(terminal_state_id) {
            return Err(OutcomeGroupValidationError::UnknownTerminalState(
                terminal_state_id.clone(),
            ));
        }
    }

    for terminal_state_id in group.terminal_states.keys() {
        if !group
            .payout_matrix
            .payout_per_unit_by_state
            .contains_key(terminal_state_id)
        {
            return Err(OutcomeGroupValidationError::MissingPayoutRow(
                terminal_state_id.clone(),
            ));
        }
    }

    for (terminal_state_id, row) in &group.payout_matrix.payout_per_unit_by_state {
        if row.len() != group.payout_matrix.cols.len() {
            return Err(OutcomeGroupValidationError::PayoutMatrixDimensionMismatch {
                terminal_state_id: terminal_state_id.clone(),
            });
        }
        if row
            .iter()
            .any(|payout| *payout < Decimal::ZERO || *payout > Decimal::ONE)
        {
            return Err(OutcomeGroupValidationError::OutOfBoundsPayout {
                terminal_state_id: terminal_state_id.clone(),
            });
        }
    }

    Ok(())
}

fn validate_non_standard_payouts(group: &OutcomeGroup) -> Result<(), OutcomeGroupValidationError> {
    let mut non_standard_states = BTreeSet::new();
    for (terminal_state_id, terminal_state) in &group.terminal_states {
        if !terminal_state.kind.is_standard() {
            non_standard_states.insert(terminal_state_id.clone());
        }
    }

    let mut seen_vectors = BTreeSet::new();
    for vector in &group.settlement_rules.non_standard_terminal_payouts {
        if !non_standard_states.contains(&vector.terminal_state_id) {
            return Err(OutcomeGroupValidationError::UnknownTerminalState(
                vector.terminal_state_id.clone(),
            ));
        }
        if !seen_vectors.insert(vector.terminal_state_id.clone()) {
            return Err(
                OutcomeGroupValidationError::DuplicateNonStandardPayoutVector(
                    vector.terminal_state_id.clone(),
                ),
            );
        }
        validate_attested_payout_vector(group, vector)?;
    }

    for terminal_state_id in non_standard_states {
        if !seen_vectors.contains(&terminal_state_id) {
            return Err(OutcomeGroupValidationError::MissingNonStandardPayoutVector(
                terminal_state_id,
            ));
        }
    }

    Ok(())
}

fn validate_attested_payout_vector(
    group: &OutcomeGroup,
    vector: &AttestedPayoutVector,
) -> Result<(), OutcomeGroupValidationError> {
    validate_sha256_field(
        "non_standard_terminal_payouts.attestation_sha256",
        &vector.attestation_sha256,
    )?;
    let expected_hash = payout_vector_attestation_sha256(
        &vector.terminal_state_id,
        &vector.label,
        &vector.cols,
        &vector.payouts,
        &vector.refund_convention,
    );
    if expected_hash != vector.attestation_sha256 {
        return Err(OutcomeGroupValidationError::AttestationHashMismatch {
            field: "non_standard_terminal_payouts.attestation_sha256",
        });
    }
    if vector.payouts.len() != group.payout_matrix.cols.len() {
        return Err(OutcomeGroupValidationError::PayoutMatrixDimensionMismatch {
            terminal_state_id: vector.terminal_state_id.clone(),
        });
    }
    let resolved_cols = vector
        .cols
        .iter()
        .map(|leg_ref| resolve_attested_leg(group, leg_ref))
        .collect::<Result<Vec<_>, _>>()?;
    if resolved_cols != group.payout_matrix.cols {
        return Err(OutcomeGroupValidationError::AttestedColumnOrderMismatch);
    }
    if vector.payouts != group.payout_matrix.payout_per_unit_by_state[&vector.terminal_state_id] {
        return Err(OutcomeGroupValidationError::PayoutMatrixValueMismatch {
            terminal_state_id: vector.terminal_state_id.clone(),
        });
    }
    Ok(())
}

fn validate_standard_rows(group: &OutcomeGroup) -> Result<(), OutcomeGroupValidationError> {
    let expected = derive_standard_payout_matrix(
        &group.terminal_states,
        &group.tradable_legs,
        group.settlement_rules.terminal_state_convention.clone(),
    )?;

    if expected.cols != group.payout_matrix.cols {
        return Err(OutcomeGroupValidationError::PayoutMatrixValueMismatch {
            terminal_state_id: String::new(),
        });
    }

    for (terminal_state_id, expected_row) in expected.payout_per_unit_by_state {
        if group
            .payout_matrix
            .payout_per_unit_by_state
            .get(&terminal_state_id)
            != Some(&expected_row)
        {
            return Err(OutcomeGroupValidationError::PayoutMatrixValueMismatch {
                terminal_state_id,
            });
        }
    }

    Ok(())
}

fn validate_role_bindings(group: &OutcomeGroup) -> Result<(), OutcomeGroupValidationError> {
    let role_binding_proof = group
        .role_binding_proof
        .as_ref()
        .ok_or(OutcomeGroupValidationError::MissingRoleBindingProof)?;
    match role_binding_proof {
        RoleBindingProof::OperatorAttested {
            positive_side_bindings,
            attestation_sha256,
            proof_fingerprint,
            ..
        } => {
            validate_sha256_field("role_binding_proof.attestation_sha256", attestation_sha256)?;
            validate_sha256_field("role_binding_proof.proof_fingerprint", proof_fingerprint)?;
            if role_binding_attestation_sha256(positive_side_bindings) != *attestation_sha256 {
                return Err(OutcomeGroupValidationError::AttestationHashMismatch {
                    field: "role_binding_proof.attestation_sha256",
                });
            }
            validate_positive_side_bindings(group, positive_side_bindings)?;
        }
        RoleBindingProof::VenueStructuredFields {
            proof_fingerprint, ..
        } => {
            validate_sha256_field("role_binding_proof.proof_fingerprint", proof_fingerprint)?;
        }
    }
    Ok(())
}

fn validate_positive_side_bindings(
    group: &OutcomeGroup,
    bindings: &[PositiveSideBinding],
) -> Result<(), OutcomeGroupValidationError> {
    let mut label_to_state = BTreeMap::new();
    for (state_id, state) in &group.terminal_states {
        if label_to_state
            .insert(state.label.clone(), state_id.clone())
            .is_some()
        {
            return Err(OutcomeGroupValidationError::UnknownTerminalStateLabel(
                state.label.clone(),
            ));
        }
    }

    let mut seen_states = BTreeSet::new();
    for binding in bindings {
        let terminal_state_id = label_to_state
            .get(&binding.terminal_state_label)
            .ok_or_else(|| {
                OutcomeGroupValidationError::UnknownTerminalStateLabel(
                    binding.terminal_state_label.clone(),
                )
            })?
            .clone();
        if !group.terminal_states[&terminal_state_id].kind.is_standard() {
            return Err(OutcomeGroupValidationError::PayoutMatrixValueMismatch {
                terminal_state_id,
            });
        }
        if !seen_states.insert(terminal_state_id.clone()) {
            return Err(OutcomeGroupValidationError::DuplicatePositiveSideBinding(
                terminal_state_id,
            ));
        }
        let pays_on_leg_id = resolve_attested_leg(group, &binding.pays_on_leg)?;
        let pays_unless_leg_id = resolve_attested_leg(group, &binding.pays_unless_leg)?;
        let pays_on_leg = &group.tradable_legs[&pays_on_leg_id];
        let pays_unless_leg = &group.tradable_legs[&pays_unless_leg_id];
        if pays_on_leg.leg_role != OutcomeLegRole::PaysOnTerminalState(terminal_state_id.clone())
            || pays_unless_leg.leg_role
                != OutcomeLegRole::PaysUnlessTerminalState(terminal_state_id.clone())
        {
            return Err(OutcomeGroupValidationError::PayoutMatrixValueMismatch {
                terminal_state_id,
            });
        }
    }

    for (terminal_state_id, terminal_state) in &group.terminal_states {
        if terminal_state.kind.is_standard() && !seen_states.contains(terminal_state_id) {
            return Err(OutcomeGroupValidationError::MissingPositiveSideBinding(
                terminal_state_id.clone(),
            ));
        }
    }

    Ok(())
}

fn resolve_attested_leg(
    group: &OutcomeGroup,
    leg_ref: &AttestedLegRef,
) -> Result<OutcomeLegId, OutcomeGroupValidationError> {
    let matches = group
        .tradable_legs
        .iter()
        .filter_map(|(leg_id, leg)| {
            let matches = match leg_ref {
                AttestedLegRef::NativeLegId(native_leg_id) => leg.native_leg_id == *native_leg_id,
                AttestedLegRef::OutcomeAndSide {
                    outcome_label,
                    side_label,
                } => leg.outcome_label == *outcome_label && leg.side_label == *side_label,
            };
            matches.then(|| leg_id.clone())
        })
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => Err(OutcomeGroupValidationError::AttestedLegUnknown),
        [leg_id] => Ok(leg_id.clone()),
        _ => Err(OutcomeGroupValidationError::AttestedLegAmbiguous),
    }
}

fn payout_for_role(
    leg_id: &str,
    role: &OutcomeLegRole,
    row_terminal_state_id: &str,
) -> Result<Decimal, OutcomeGroupValidationError> {
    match role {
        OutcomeLegRole::PaysOnTerminalState(terminal_state_id) => {
            Ok(if terminal_state_id == row_terminal_state_id {
                Decimal::ONE
            } else {
                Decimal::ZERO
            })
        }
        OutcomeLegRole::PaysUnlessTerminalState(terminal_state_id) => {
            Ok(if terminal_state_id == row_terminal_state_id {
                Decimal::ZERO
            } else {
                Decimal::ONE
            })
        }
        OutcomeLegRole::UnsupportedMultiState { .. } => Err(
            OutcomeGroupValidationError::UnsupportedMultiStateLegRole(leg_id.to_string()),
        ),
    }
}

fn validate_sha256_field(
    field: &'static str,
    value: &str,
) -> Result<(), OutcomeGroupValidationError> {
    if is_lowercase_sha256(value) {
        Ok(())
    } else {
        Err(OutcomeGroupValidationError::InvalidSha256 { field })
    }
}

fn normalize_decimal(value: Decimal) -> String {
    if value == Decimal::ZERO {
        return "0".to_string();
    }
    value.normalize().to_string()
}

fn source_kind_label(source_kind: &OutcomeGroupSourceKind) -> &'static str {
    match source_kind {
        OutcomeGroupSourceKind::Polymarket => "polymarket",
        OutcomeGroupSourceKind::Hyperliquid => "hyperliquid",
        OutcomeGroupSourceKind::OperatorAttested => "operator_attested",
    }
}

fn terminal_state_kind_label(kind: TerminalStateKind) -> &'static str {
    match kind {
        TerminalStateKind::Standard => "standard",
        TerminalStateKind::Void => "void",
        TerminalStateKind::Fallback => "fallback",
    }
}

fn settlement_source_kind_label(kind: &SettlementSourceKind) -> &'static str {
    match kind {
        SettlementSourceKind::VenueStructuredFields => "venue_structured_fields",
        SettlementSourceKind::OperatorAttested => "operator_attested",
    }
}

fn terminal_payout_derivation_label(derivation: &TerminalPayoutDerivation) -> &'static str {
    match derivation {
        TerminalPayoutDerivation::StandardRowsPlusAttestedVectors => {
            "standard_rows_plus_attested_vectors"
        }
    }
}

fn append_grouping_identity_fields(
    fields: &mut Vec<CanonicalField>,
    grouping_proof: &GroupingProof,
) {
    if let Some(payload) = grouping_proof.concrete_payload() {
        match payload {
            ConcreteGroupingProofRef::NegRisk(proof) => {
                let NegRiskGroupingProof {
                    neg_risk_market_id,
                    discovery_scope,
                    market_slugs,
                    ..
                } = proof;
                fields.push(CanonicalField::new(
                    ["grouping", "kind"],
                    "polymarket_neg_risk",
                ));
                fields.push(CanonicalField::new(
                    ["grouping", "neg_risk_market_id"],
                    neg_risk_market_id,
                ));
                fields.push(CanonicalField::new(
                    ["grouping", "source_id"],
                    &discovery_scope.source_id,
                ));
                fields.push(CanonicalField::new(
                    ["grouping", "cache_key_fingerprint"],
                    &discovery_scope.cache_key_fingerprint,
                ));
                append_string_list_fields(
                    fields,
                    vec!["grouping".to_string(), "market_slugs".to_string()],
                    market_slugs,
                );
            }
            ConcreteGroupingProofRef::StructuredOutcome(proof) => {
                let StructuredOutcomeGroupingProof {
                    question,
                    outcome_indices,
                    ..
                } = proof;
                fields.push(CanonicalField::new(
                    ["grouping", "kind"],
                    "hyperliquid_outcome",
                ));
                fields.push(CanonicalField::new(
                    ["grouping", "question"],
                    question.to_string(),
                ));
                for (index, outcome_index) in outcome_indices.iter().enumerate() {
                    fields.push(CanonicalField::owned(
                        vec![
                            "grouping".to_string(),
                            "outcome_indices".to_string(),
                            index.to_string(),
                        ],
                        outcome_index.to_string(),
                    ));
                }
            }
        }
        return;
    }
    match grouping_proof {
        GroupingProof::OperatorAttested {
            settlement_contract_id,
            attestation_id,
            ..
        } => {
            fields.push(CanonicalField::new(
                ["grouping", "kind"],
                "operator_attested",
            ));
            fields.push(CanonicalField::new(
                ["grouping", "settlement_contract_id"],
                settlement_contract_id,
            ));
            fields.push(CanonicalField::new(
                ["grouping", "attestation_id"],
                attestation_id,
            ));
        }
        _ => unreachable!(),
    }
}

fn append_role_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    role: &OutcomeLegRole,
) {
    match role {
        OutcomeLegRole::PaysOnTerminalState(terminal_state_id) => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "role_kind"),
                "pays_on_terminal_state",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "terminal_state_id"),
                terminal_state_id,
            ));
        }
        OutcomeLegRole::PaysUnlessTerminalState(terminal_state_id) => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "role_kind"),
                "pays_unless_terminal_state",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "terminal_state_id"),
                terminal_state_id,
            ));
        }
        OutcomeLegRole::UnsupportedMultiState { terminal_state_ids } => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "role_kind"),
                "unsupported_multi_state",
            ));
            append_string_list_fields(
                fields,
                prefixed_path(&prefix, "terminal_state_ids"),
                terminal_state_ids,
            );
        }
    }
}

fn append_price_scale_metadata_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    price_scale: &NormalizedPriceScaleEvidence,
) {
    match price_scale {
        NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id,
            payout_per_contract,
            price_units_per_payout,
            assertion_source,
        } => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "kind"),
                "binary_one_payout_equals_one_settlement_unit",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "settlement_asset_id"),
                settlement_asset_id,
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "payout_per_contract"),
                normalize_decimal(*payout_per_contract),
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "price_units_per_payout"),
                normalize_decimal(*price_units_per_payout),
            ));
            match assertion_source {
                PriceScaleAssertionSource::VenueStructuredFields { proof_fingerprint } => {
                    fields.push(CanonicalField::owned(
                        prefixed_path(&prefix, "assertion_source_kind"),
                        "venue_structured_fields",
                    ));
                    fields.push(CanonicalField::owned(
                        prefixed_path(&prefix, "assertion_source_fingerprint"),
                        proof_fingerprint,
                    ));
                }
                PriceScaleAssertionSource::OperatorAttested { attestation_sha256 } => {
                    fields.push(CanonicalField::owned(
                        prefixed_path(&prefix, "assertion_source_kind"),
                        "operator_attested",
                    ));
                    fields.push(CanonicalField::owned(
                        prefixed_path(&prefix, "assertion_source_fingerprint"),
                        attestation_sha256,
                    ));
                }
            }
        }
    }
}

fn append_payout_matrix_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    payout_matrix: &PayoutMatrix,
) {
    append_string_list_fields(fields, prefixed_path(&prefix, "cols"), &payout_matrix.cols);
    for (terminal_state_id, payouts) in &payout_matrix.payout_per_unit_by_state {
        for (index, payout) in payouts.iter().enumerate() {
            fields.push(CanonicalField::owned(
                [
                    prefix.clone(),
                    vec![
                        "payout_per_unit_by_state".to_string(),
                        terminal_state_id.clone(),
                        index.to_string(),
                    ],
                ]
                .concat(),
                normalize_decimal(*payout),
            ));
        }
    }
}

fn append_settlement_rules_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    rules: &SettlementRules,
) {
    match &rules.terminal_state_convention {
        TerminalStateConvention::ExactlyOneWinner => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "terminal_state_convention"),
                "exactly_one_winner",
            ));
        }
        TerminalStateConvention::Unsupported(value) => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "terminal_state_convention"),
                "unsupported",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "terminal_state_convention_value"),
                value,
            ));
        }
    }
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "settlement_source_kind"),
        settlement_source_kind_label(&rules.settlement_source_kind),
    ));
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "terminal_payout_derivation"),
        terminal_payout_derivation_label(&rules.terminal_payout_derivation),
    ));
    for (index, vector) in rules.non_standard_terminal_payouts.iter().enumerate() {
        append_attested_payout_vector_fields(
            fields,
            [
                prefix.clone(),
                vec![
                    "non_standard_terminal_payouts".to_string(),
                    index.to_string(),
                ],
            ]
            .concat(),
            vector,
        );
    }
}

fn append_role_binding_proof_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    proof: &RoleBindingProof,
) {
    match proof {
        RoleBindingProof::OperatorAttested {
            attestation_id,
            positive_side_bindings,
            attestation_sha256,
            proof_fingerprint,
        } => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "kind"),
                "operator_attested",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "attestation_id"),
                attestation_id,
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "attestation_sha256"),
                attestation_sha256,
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "proof_fingerprint"),
                proof_fingerprint,
            ));
            for (index, binding) in positive_side_bindings.iter().enumerate() {
                append_positive_side_binding_fields(
                    fields,
                    [
                        prefix.clone(),
                        vec!["positive_side_bindings".to_string(), index.to_string()],
                    ]
                    .concat(),
                    binding,
                );
            }
        }
        RoleBindingProof::VenueStructuredFields {
            source_id,
            question,
            outcome_indices,
            proof_fingerprint,
        } => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "kind"),
                "venue_structured_fields",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "source_id"),
                source_id,
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "question"),
                question.to_string(),
            ));
            for (index, outcome_index) in outcome_indices.iter().enumerate() {
                fields.push(CanonicalField::owned(
                    [
                        prefix.clone(),
                        vec!["outcome_indices".to_string(), index.to_string()],
                    ]
                    .concat(),
                    outcome_index.to_string(),
                ));
            }
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "proof_fingerprint"),
                proof_fingerprint,
            ));
        }
    }
}

fn append_positive_side_binding_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    binding: &PositiveSideBinding,
) {
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "terminal_state_label"),
        &binding.terminal_state_label,
    ));
    append_attested_leg_ref_fields(
        fields,
        prefixed_path(&prefix, "pays_on_leg"),
        &binding.pays_on_leg,
    );
    append_attested_leg_ref_fields(
        fields,
        prefixed_path(&prefix, "pays_unless_leg"),
        &binding.pays_unless_leg,
    );
}

fn append_attested_payout_vector_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    vector: &AttestedPayoutVector,
) {
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "terminal_state_id"),
        &vector.terminal_state_id,
    ));
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "label"),
        &vector.label,
    ));
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "refund_convention"),
        &vector.refund_convention,
    ));
    fields.push(CanonicalField::owned(
        prefixed_path(&prefix, "attestation_sha256"),
        &vector.attestation_sha256,
    ));
    for (index, col) in vector.cols.iter().enumerate() {
        append_attested_leg_ref_fields(
            fields,
            [prefix.clone(), vec!["cols".to_string(), index.to_string()]].concat(),
            col,
        );
    }
    for (index, payout) in vector.payouts.iter().enumerate() {
        fields.push(CanonicalField::owned(
            [
                prefix.clone(),
                vec!["payouts".to_string(), index.to_string()],
            ]
            .concat(),
            normalize_decimal(*payout),
        ));
    }
}

fn append_attested_leg_ref_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    leg_ref: &AttestedLegRef,
) {
    match leg_ref {
        AttestedLegRef::NativeLegId(native_leg_id) => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "kind"),
                "native_leg_id",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "native_leg_id"),
                native_leg_id,
            ));
        }
        AttestedLegRef::OutcomeAndSide {
            outcome_label,
            side_label,
        } => {
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "kind"),
                "outcome_and_side",
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "outcome_label"),
                outcome_label,
            ));
            fields.push(CanonicalField::owned(
                prefixed_path(&prefix, "side_label"),
                side_label,
            ));
        }
    }
}

fn append_string_list_fields(
    fields: &mut Vec<CanonicalField>,
    prefix: Vec<String>,
    values: &[String],
) {
    for (index, value) in values.iter().enumerate() {
        fields.push(CanonicalField::owned(
            [prefix.clone(), vec![index.to_string()]].concat(),
            value,
        ));
    }
}

fn prefixed_path(prefix: &[String], field: &str) -> Vec<String> {
    [prefix.to_vec(), vec![field.to_string()]].concat()
}

fn settlement_identity_fingerprint(group: &OutcomeGroup) -> String {
    let mut fields = vec![CanonicalField::new(
        ["settlement", "asset_id"],
        &group.settlement_asset_id,
    )];
    for (terminal_state_id, terminal_state) in &group.terminal_states {
        fields.push(CanonicalField::owned(
            vec![
                "terminal_states".to_string(),
                terminal_state_id.clone(),
                "label".to_string(),
            ],
            &terminal_state.label,
        ));
        fields.push(CanonicalField::owned(
            vec![
                "terminal_states".to_string(),
                terminal_state_id.clone(),
                "kind".to_string(),
            ],
            terminal_state_kind_label(terminal_state.kind),
        ));
    }
    append_payout_matrix_fields(
        &mut fields,
        vec!["payout_matrix".to_string()],
        &group.payout_matrix,
    );
    append_settlement_rules_fields(
        &mut fields,
        vec!["settlement_rules".to_string()],
        &group.settlement_rules,
    );
    canonical_fingerprint(fields)
}
