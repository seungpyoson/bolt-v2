use std::collections::BTreeSet;

use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{ModelRiskEvaluationScope, RiskAssessment, RiskStateVersion},
    risk_classifier::ConcentrationBucket,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskKernel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskKernelInput {
    pub risk_state_version: RiskStateVersion,
    pub portfolio: RiskPortfolioSnapshot,
    pub candidate: RiskCandidate,
    pub evaluation_scope: RiskEvaluationScope,
    pub portfolio_scope_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskPortfolioSnapshot {
    pub positions: Vec<RiskExposure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskCandidate {
    pub instrument_id: String,
    pub buckets: BTreeSet<ConcentrationBucket>,
    pub quantity: Decimal,
    pub conservative_liquidation_value: Decimal,
    pub governor_cost_basis: Decimal,
    pub terminal_cash_flows: Vec<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskExposure {
    pub instrument_id: String,
    pub buckets: BTreeSet<ConcentrationBucket>,
    pub quantity: Decimal,
    pub conservative_liquidation_value: Decimal,
    pub governor_cost_basis: Decimal,
    pub terminal_cash_flows: Vec<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskEvaluationScope {
    CandidateInstrument { instrument_id: String },
    ConcentrationBucket(ConcentrationBucket),
    Portfolio { scope_id: String },
}

impl From<&ModelRiskEvaluationScope> for RiskEvaluationScope {
    fn from(scope: &ModelRiskEvaluationScope) -> Self {
        match scope {
            ModelRiskEvaluationScope::CandidateInstrument { instrument_id } => {
                Self::CandidateInstrument {
                    instrument_id: instrument_id.clone(),
                }
            }
            ModelRiskEvaluationScope::ConcentrationBucket(bucket) => {
                Self::ConcentrationBucket(bucket.clone())
            }
            ModelRiskEvaluationScope::Portfolio { scope_id } => Self::Portfolio {
                scope_id: scope_id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskKernelError {
    InvalidRiskInput,
    UnrecognizedEvaluationScope,
}

impl RiskKernel {
    /// Evaluates authoritative risk quantities from immutable caller-supplied facts only.
    ///
    /// Worst-case complexity: for `P` current positions, at most `B` buckets per
    /// exposure, and at most `T` terminal cash-flow states per exposure, this
    /// runs in `O(P * (B + T))` time and `O(1)` extra space after the input
    /// collections are constructed by the caller. It performs no I/O, no mutable
    /// reads, no lock acquisition, and no unbounded search.
    pub fn evaluate(input: &RiskKernelInput) -> Result<RiskAssessment, RiskKernelError> {
        validate_input(input)?;
        if !scope_is_recognized(input) {
            return Err(RiskKernelError::UnrecognizedEvaluationScope);
        }

        let candidate_equity_floor_stress_loss = equity_floor_stress_loss(&input.candidate)?;
        let candidate_governor_realized_loss = governor_realized_loss(&input.candidate)?;
        let current_scope_equity_floor_stress_loss = input
            .portfolio
            .positions
            .iter()
            .filter(|position| exposure_matches_scope(position, &input.evaluation_scope))
            .try_fold(Decimal::ZERO, |acc, position| {
                Ok::<Decimal, RiskKernelError>(acc + equity_floor_stress_loss(position)?)
            })?;
        let post_candidate_scope_equity_floor_stress_loss = current_scope_equity_floor_stress_loss
            + if candidate_matches_scope(&input.candidate, &input.evaluation_scope) {
                candidate_equity_floor_stress_loss
            } else {
                Decimal::ZERO
            };

        Ok(RiskAssessment {
            risk_state_version: input.risk_state_version,
            accepted: true,
            collateral_required: input.candidate.governor_cost_basis,
            equity_floor_stress_loss: candidate_equity_floor_stress_loss,
            current_scope_equity_floor_stress_loss,
            post_candidate_scope_equity_floor_stress_loss,
            governor_realized_loss: candidate_governor_realized_loss,
            rejection_reason: None,
        })
    }
}

fn validate_input(input: &RiskKernelInput) -> Result<(), RiskKernelError> {
    if !is_clean_runtime_value(&input.portfolio_scope_id) {
        return Err(RiskKernelError::InvalidRiskInput);
    }
    validate_candidate(&input.candidate)?;
    for exposure in &input.portfolio.positions {
        validate_exposure(exposure)?;
    }
    match &input.evaluation_scope {
        RiskEvaluationScope::CandidateInstrument { instrument_id } => {
            if !is_clean_runtime_value(instrument_id) {
                return Err(RiskKernelError::InvalidRiskInput);
            }
        }
        RiskEvaluationScope::ConcentrationBucket(_) => {}
        RiskEvaluationScope::Portfolio { scope_id } => {
            if !is_clean_runtime_value(scope_id) {
                return Err(RiskKernelError::InvalidRiskInput);
            }
        }
    }
    Ok(())
}

fn validate_candidate(candidate: &RiskCandidate) -> Result<(), RiskKernelError> {
    validate_exposure_facts(candidate)
}

fn validate_exposure(exposure: &RiskExposure) -> Result<(), RiskKernelError> {
    validate_exposure_facts(exposure)
}

fn validate_exposure_facts(exposure: &impl RiskExposureFacts) -> Result<(), RiskKernelError> {
    if !is_clean_runtime_value(exposure.instrument_id())
        || exposure.buckets().is_empty()
        || exposure.quantity() <= Decimal::ZERO
        || exposure.conservative_liquidation_value() < Decimal::ZERO
        || exposure.governor_cost_basis() < Decimal::ZERO
        || exposure.terminal_cash_flows().is_empty()
    {
        return Err(RiskKernelError::InvalidRiskInput);
    }
    Ok(())
}

fn scope_is_recognized(input: &RiskKernelInput) -> bool {
    match &input.evaluation_scope {
        RiskEvaluationScope::CandidateInstrument { instrument_id } => {
            instrument_id == &input.candidate.instrument_id
        }
        RiskEvaluationScope::ConcentrationBucket(bucket) => {
            input.candidate.buckets.contains(bucket)
                || input
                    .portfolio
                    .positions
                    .iter()
                    .any(|position| position.buckets.contains(bucket))
        }
        RiskEvaluationScope::Portfolio { scope_id } => scope_id == &input.portfolio_scope_id,
    }
}

fn candidate_matches_scope(candidate: &RiskCandidate, scope: &RiskEvaluationScope) -> bool {
    match scope {
        RiskEvaluationScope::CandidateInstrument { instrument_id } => {
            instrument_id == &candidate.instrument_id
        }
        RiskEvaluationScope::ConcentrationBucket(bucket) => candidate.buckets.contains(bucket),
        RiskEvaluationScope::Portfolio { .. } => true,
    }
}

fn exposure_matches_scope(exposure: &RiskExposure, scope: &RiskEvaluationScope) -> bool {
    match scope {
        RiskEvaluationScope::CandidateInstrument { instrument_id } => {
            instrument_id == &exposure.instrument_id
        }
        RiskEvaluationScope::ConcentrationBucket(bucket) => exposure.buckets.contains(bucket),
        RiskEvaluationScope::Portfolio { .. } => true,
    }
}

fn equity_floor_stress_loss(exposure: &impl RiskExposureFacts) -> Result<Decimal, RiskKernelError> {
    Ok(positive_part(
        exposure.conservative_liquidation_value() - worst_terminal_cash_flow(exposure)?,
    ))
}

fn governor_realized_loss(exposure: &impl RiskExposureFacts) -> Result<Decimal, RiskKernelError> {
    Ok(positive_part(
        exposure.governor_cost_basis() - worst_terminal_cash_flow(exposure)?,
    ))
}

fn worst_terminal_cash_flow(exposure: &impl RiskExposureFacts) -> Result<Decimal, RiskKernelError> {
    let mut terminal_cash_flows = exposure.terminal_cash_flows().iter().copied();
    let Some(mut worst) = terminal_cash_flows.next() else {
        return Err(RiskKernelError::InvalidRiskInput);
    };
    for cash_flow in terminal_cash_flows {
        if cash_flow < worst {
            worst = cash_flow;
        }
    }
    Ok(worst * exposure.quantity())
}

fn positive_part(value: Decimal) -> Decimal {
    if value > Decimal::ZERO {
        value
    } else {
        Decimal::ZERO
    }
}

fn is_clean_runtime_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

trait RiskExposureFacts {
    fn instrument_id(&self) -> &str;
    fn buckets(&self) -> &BTreeSet<ConcentrationBucket>;
    fn quantity(&self) -> Decimal;
    fn conservative_liquidation_value(&self) -> Decimal;
    fn governor_cost_basis(&self) -> Decimal;
    fn terminal_cash_flows(&self) -> &[Decimal];
}

impl RiskExposureFacts for RiskCandidate {
    fn instrument_id(&self) -> &str {
        &self.instrument_id
    }

    fn buckets(&self) -> &BTreeSet<ConcentrationBucket> {
        &self.buckets
    }

    fn quantity(&self) -> Decimal {
        self.quantity
    }

    fn conservative_liquidation_value(&self) -> Decimal {
        self.conservative_liquidation_value
    }

    fn governor_cost_basis(&self) -> Decimal {
        self.governor_cost_basis
    }

    fn terminal_cash_flows(&self) -> &[Decimal] {
        &self.terminal_cash_flows
    }
}

impl RiskExposureFacts for RiskExposure {
    fn instrument_id(&self) -> &str {
        &self.instrument_id
    }

    fn buckets(&self) -> &BTreeSet<ConcentrationBucket> {
        &self.buckets
    }

    fn quantity(&self) -> Decimal {
        self.quantity
    }

    fn conservative_liquidation_value(&self) -> Decimal {
        self.conservative_liquidation_value
    }

    fn governor_cost_basis(&self) -> Decimal {
        self.governor_cost_basis
    }

    fn terminal_cash_flows(&self) -> &[Decimal] {
        &self.terminal_cash_flows
    }
}
