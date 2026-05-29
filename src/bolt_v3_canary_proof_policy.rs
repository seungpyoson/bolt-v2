use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub const CANARY_PROOF_CANDIDATE_SOURCE_RECORD_KIND: &str = "bolt_v3_canary_proof_candidate_source";
pub const CANARY_PROOF_ORDER_INTENT_RECORD_KIND: &str = "bolt_v3_canary_proof_order_intent";
pub const CANARY_PROOF_CLAIM: &str = "proof_only";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanaryProofOrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryProofCandidate {
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub candidate_score: Decimal,
    pub source_refs: Vec<String>,
    pub sizing_price: Decimal,
    pub constraints: CanaryProofInstrumentConstraints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanaryProofSizingMode {
    BaseQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryProofInstrumentConstraints {
    pub sizing_mode: CanaryProofSizingMode,
    pub quantity_step: Decimal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notional_step: Option<Decimal>,
    pub min_quantity: Option<Decimal>,
    pub min_notional: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryProofPolicyInput {
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub proof_claim: String,
    pub proof_notional: Decimal,
    pub max_notional_per_order: Decimal,
    pub allow_negative_expected_ev: bool,
    pub source_ready: bool,
    pub current_source_ref: String,
    pub candidates: Vec<CanaryProofCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryProofSourcePacket {
    pub current_source_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryProofCandidateSourceArtifact {
    pub record_kind: String,
    pub proof_claim: String,
    pub current_source_ref: String,
    pub candidate_count: u32,
    pub candidates: Vec<CanaryProofCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofSelection {
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub proof_claim: String,
    pub source_refs: Vec<String>,
    pub sizing: CanaryProofOrderSizing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryProofOrderIntentArtifact {
    pub record_kind: String,
    pub proof_claim: String,
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub notional: Decimal,
    pub quantity: Decimal,
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofOrderSizing {
    notional: Decimal,
    quantity: Decimal,
}

impl CanaryProofOrderSizing {
    pub fn notional_for_submit_admission(&self) -> Decimal {
        self.notional
    }

    pub fn quantity_for_submit(&self) -> Decimal {
        self.quantity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryProofPolicyRejection {
    InvalidProofClaim,
    ProofNotionalNonPositive,
    ProofNotionalExceedsCap,
    ProofPolicySourceNotReady,
    ProofPolicyStrategyMismatch,
    ProofPolicyExecutionClientMismatch,
    ProofCandidateSourceMismatch,
    ProofPolicyNegativeEvDisallowed,
    InstrumentConstraintsBelowMinQuantity,
    InstrumentConstraintsBelowMinNotional,
    InstrumentConstraintsInvalidQuantityStep,
    InstrumentConstraintsInvalidNotionalStep,
    InstrumentConstraintsInvalidSizingPrice,
    NoProofCandidate,
}

pub fn build_canary_proof_candidate_source_artifact(
    source_packet: &CanaryProofSourcePacket,
    candidates: Vec<CanaryProofCandidate>,
) -> Result<CanaryProofCandidateSourceArtifact, CanaryProofPolicyRejection> {
    if candidates.iter().any(|candidate| {
        !candidate
            .source_refs
            .contains(&source_packet.current_source_ref)
    }) {
        return Err(CanaryProofPolicyRejection::ProofCandidateSourceMismatch);
    }

    Ok(CanaryProofCandidateSourceArtifact {
        record_kind: CANARY_PROOF_CANDIDATE_SOURCE_RECORD_KIND.to_string(),
        proof_claim: CANARY_PROOF_CLAIM.to_string(),
        current_source_ref: source_packet.current_source_ref.clone(),
        candidate_count: candidates.len() as u32,
        candidates,
    })
}

pub fn build_canary_proof_order_intent_artifact(
    candidate_source: &CanaryProofCandidateSourceArtifact,
    input: &CanaryProofPolicyInput,
) -> Result<CanaryProofOrderIntentArtifact, CanaryProofPolicyRejection> {
    if candidate_source.current_source_ref != input.current_source_ref
        || candidate_source.candidate_count as usize != candidate_source.candidates.len()
    {
        return Err(CanaryProofPolicyRejection::ProofCandidateSourceMismatch);
    }

    let source_bound_input = CanaryProofPolicyInput {
        candidates: candidate_source.candidates.clone(),
        ..input.clone()
    };
    let selected = select_canary_proof_candidate(&source_bound_input)?;

    Ok(CanaryProofOrderIntentArtifact {
        record_kind: CANARY_PROOF_ORDER_INTENT_RECORD_KIND.to_string(),
        proof_claim: CANARY_PROOF_CLAIM.to_string(),
        strategy_instance_id: selected.strategy_instance_id,
        execution_client_id: selected.execution_client_id,
        instrument_id: selected.instrument_id,
        order_side: selected.order_side,
        notional: selected.sizing.notional_for_submit_admission(),
        quantity: selected.sizing.quantity_for_submit(),
        source_refs: selected.source_refs,
    })
}

pub fn select_canary_proof_candidate(
    input: &CanaryProofPolicyInput,
) -> Result<CanaryProofSelection, CanaryProofPolicyRejection> {
    if input.proof_claim != CANARY_PROOF_CLAIM {
        return Err(CanaryProofPolicyRejection::InvalidProofClaim);
    }
    if input.proof_notional <= Decimal::ZERO {
        return Err(CanaryProofPolicyRejection::ProofNotionalNonPositive);
    }
    if input.proof_notional > input.max_notional_per_order {
        return Err(CanaryProofPolicyRejection::ProofNotionalExceedsCap);
    }
    if !input.source_ready {
        return Err(CanaryProofPolicyRejection::ProofPolicySourceNotReady);
    }

    let strategy_bound_candidates = input
        .candidates
        .iter()
        .filter(|candidate| candidate.strategy_instance_id == input.strategy_instance_id)
        .collect::<Vec<_>>();

    if strategy_bound_candidates.is_empty() && !input.candidates.is_empty() {
        return Err(CanaryProofPolicyRejection::ProofPolicyStrategyMismatch);
    }

    let execution_bound_candidates = strategy_bound_candidates
        .into_iter()
        .filter(|candidate| candidate.execution_client_id == input.execution_client_id)
        .collect::<Vec<_>>();

    if execution_bound_candidates.is_empty() && !input.candidates.is_empty() {
        return Err(CanaryProofPolicyRejection::ProofPolicyExecutionClientMismatch);
    }

    let source_bound_candidates = execution_bound_candidates
        .into_iter()
        .filter(|candidate| candidate.source_refs.contains(&input.current_source_ref))
        .collect::<Vec<_>>();

    if source_bound_candidates.is_empty() {
        return if input.candidates.is_empty() {
            Err(CanaryProofPolicyRejection::NoProofCandidate)
        } else {
            Err(CanaryProofPolicyRejection::ProofCandidateSourceMismatch)
        };
    }

    let selected = source_bound_candidates
        .into_iter()
        .filter(|candidate| {
            input.allow_negative_expected_ev || candidate.candidate_score >= Decimal::ZERO
        })
        .max_by(|left, right| left.candidate_score.cmp(&right.candidate_score))
        .ok_or(CanaryProofPolicyRejection::ProofPolicyNegativeEvDisallowed)?;

    Ok(CanaryProofSelection {
        strategy_instance_id: selected.strategy_instance_id.clone(),
        execution_client_id: selected.execution_client_id.clone(),
        instrument_id: selected.instrument_id.clone(),
        order_side: selected.order_side,
        proof_claim: input.proof_claim.clone(),
        source_refs: selected.source_refs.clone(),
        sizing: normalize_order_sizing(input.proof_notional, selected)?,
    })
}

fn normalize_order_sizing(
    proof_notional: Decimal,
    selected: &CanaryProofCandidate,
) -> Result<CanaryProofOrderSizing, CanaryProofPolicyRejection> {
    match selected.constraints.sizing_mode {
        CanaryProofSizingMode::BaseQuantity => {
            if selected.sizing_price <= Decimal::ZERO {
                return Err(CanaryProofPolicyRejection::InstrumentConstraintsInvalidSizingPrice);
            }
            if selected.constraints.quantity_step <= Decimal::ZERO {
                return Err(CanaryProofPolicyRejection::InstrumentConstraintsInvalidQuantityStep);
            }
            let raw_quantity = proof_notional / selected.sizing_price;
            let quantity_units = (raw_quantity / selected.constraints.quantity_step).floor();
            let mut rounded_quantity = quantity_units * selected.constraints.quantity_step;
            if let Some(notional_step) = selected.constraints.notional_step {
                rounded_quantity = align_quantity_to_notional_step(
                    rounded_quantity,
                    selected.constraints.quantity_step,
                    selected.sizing_price,
                    notional_step,
                )?;
            }
            if selected
                .constraints
                .min_quantity
                .is_some_and(|minimum| rounded_quantity < minimum)
            {
                return Err(CanaryProofPolicyRejection::InstrumentConstraintsBelowMinQuantity);
            }
            let rounded_notional = rounded_quantity * selected.sizing_price;
            if selected
                .constraints
                .min_notional
                .is_some_and(|minimum| rounded_notional < minimum)
            {
                return Err(CanaryProofPolicyRejection::InstrumentConstraintsBelowMinNotional);
            }
            Ok(CanaryProofOrderSizing {
                notional: rounded_notional,
                quantity: rounded_quantity,
            })
        }
    }
}

fn align_quantity_to_notional_step(
    rounded_quantity: Decimal,
    quantity_step: Decimal,
    sizing_price: Decimal,
    notional_step: Decimal,
) -> Result<Decimal, CanaryProofPolicyRejection> {
    if notional_step <= Decimal::ZERO {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsInvalidNotionalStep);
    }
    let unit_multiple =
        notional_alignment_unit_multiple(quantity_step, sizing_price, notional_step)?;
    let quantity_units = (rounded_quantity / quantity_step).floor();
    let unit_multiple_decimal = Decimal::from(unit_multiple);
    let aligned_units = (quantity_units / unit_multiple_decimal).floor() * unit_multiple_decimal;
    Ok(aligned_units * quantity_step)
}

fn notional_alignment_unit_multiple(
    quantity_step: Decimal,
    sizing_price: Decimal,
    notional_step: Decimal,
) -> Result<u64, CanaryProofPolicyRejection> {
    let ratio = (quantity_step * sizing_price / notional_step).normalize();
    if ratio <= Decimal::ZERO {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsInvalidNotionalStep);
    }
    let numerator = u128::try_from(ratio.mantissa())
        .map_err(|_| CanaryProofPolicyRejection::InstrumentConstraintsInvalidNotionalStep)?;
    let denominator = decimal_scale_denominator(ratio.scale())
        .ok_or(CanaryProofPolicyRejection::InstrumentConstraintsInvalidNotionalStep)?;
    let divisor = gcd_u128(numerator, denominator);
    u64::try_from(denominator / divisor)
        .map_err(|_| CanaryProofPolicyRejection::InstrumentConstraintsInvalidNotionalStep)
}

fn decimal_scale_denominator(scale: u32) -> Option<u128> {
    let mut denominator = 1u128;
    for _ in 0..scale {
        denominator = denominator.checked_mul(10)?;
    }
    Some(denominator)
}

fn gcd_u128(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}
