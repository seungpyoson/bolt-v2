use rust_decimal::Decimal;

const CANARY_PROOF_CLAIM: &str = "proof_only";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryProofOrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofCandidate {
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub candidate_score: Decimal,
    pub source_refs: Vec<String>,
    pub sizing_price: Decimal,
    pub constraints: CanaryProofInstrumentConstraints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryProofSizingMode {
    BaseQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofInstrumentConstraints {
    pub sizing_mode: CanaryProofSizingMode,
    pub quantity_step: Decimal,
    pub min_quantity: Option<Decimal>,
    pub min_notional: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofPolicyInput {
    pub proof_claim: String,
    pub proof_notional: Decimal,
    pub max_notional_per_order: Decimal,
    pub allow_negative_expected_ev: bool,
    pub source_ready: bool,
    pub current_source_ref: String,
    pub candidates: Vec<CanaryProofCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofSelection {
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub proof_claim: String,
    pub sizing: CanaryProofOrderSizing,
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
    ProofCandidateSourceMismatch,
    ProofPolicyNegativeEvDisallowed,
    InstrumentConstraintsBelowMinQuantity,
    InstrumentConstraintsBelowMinNotional,
    InstrumentConstraintsInvalidQuantityStep,
    InstrumentConstraintsInvalidSizingPrice,
    NoProofCandidate,
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

    let source_bound_candidates = input
        .candidates
        .iter()
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
        instrument_id: selected.instrument_id.clone(),
        order_side: selected.order_side,
        proof_claim: input.proof_claim.clone(),
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
            let rounded_quantity = quantity_units * selected.constraints.quantity_step;
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
