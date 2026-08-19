use rust_decimal::Decimal;

use crate::bolt_v3_capital_admission_state::{
    CapitalAdmissionStateError, CapitalAdmissionStateEvidence, CapitalAdmissionStateEvidenceKind,
    NtDerivedCapitalAdmissionState, validate_nt_derived_capital_admission_state,
};
use crate::bolt_v3_capital_reservation::{
    CapitalPoolSnapshot, ReservationDecision, ReservationLedger, ReservationRejectionReason,
    ReservationRequest, RetainedReservation,
};
use crate::bolt_v3_loss_governor::{LossGovernorPolicy, LossHaltReason, evaluate_loss_admission};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionPolicy {
    pub min_remaining_pool_balance: Option<Decimal>,
    pub fee_slippage_policy: Option<FeeSlippagePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSlippagePolicy {
    pub max_fee_liability: Decimal,
    pub max_slippage_liability: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionRequest {
    pub intent_id: String,
    pub strategy_id: String,
    pub instrument_id: String,
    pub pool_id: String,
    pub product_kind: ProductKind,
    pub side: IntentSide,
    pub quantity: Decimal,
    pub limit_price: Decimal,
    pub order_kind: IntentOrderKind,
    pub liquidity: IntentLiquidity,
    pub quote_set_id: Option<String>,
    pub now_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentOrderKind {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentLiquidity {
    Taker,
    RestingMaker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductKind {
    PredictionMarketBinary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductAdmissionSnapshot {
    PredictionMarketBinary(PredictionMarketAdmissionSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionMarketAdmissionSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub yes_instrument_id: String,
    pub no_instrument_id: String,
    pub yes_position: Decimal,
    pub no_position: Decimal,
    pub collateral_allowance: Decimal,
    pub collateral_coupled_group_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiabilityQuote {
    pub original_quantity: Decimal,
    pub accepted_quantity: Decimal,
    pub calculated_liability: Decimal,
    pub reserved_liability: Decimal,
    pub evidence_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiabilityError {
    MissingMarketState,
    MissingFeePolicy,
    MissingSlippagePolicy,
    InvalidIntentPrice,
    InvalidIntentQuantity,
    MissingQuoteSetId,
    InsufficientAllowance,
    InsufficientInventory,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictionMarketBinaryLiabilityCalculator;

pub struct CapitalAdmissionInputs<'a> {
    pub request: &'a CapitalAdmissionRequest,
    pub state: Option<&'a NtDerivedCapitalAdmissionState>,
    pub policy: &'a CapitalAdmissionPolicy,
    pub loss_policy: Option<&'a LossGovernorPolicy>,
    pub capital_pool: &'a CapitalPoolSnapshot,
    pub reservation_ledger: &'a mut ReservationLedger,
}

pub struct CapitalAdmissionGateInputs<'a> {
    pub request: &'a CapitalAdmissionRequest,
    pub state: Option<&'a NtDerivedCapitalAdmissionState>,
    pub policy: &'a CapitalAdmissionPolicy,
    pub loss_policy: Option<&'a LossGovernorPolicy>,
    pub capital_pool: &'a CapitalPoolSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionRebuildDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub rebuilt_reservation_count: usize,
    pub live_reserved_liability: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapitalAdmissionReservationEvidence {
    NtOpenOrder(ReservationRequest),
    RetainedLifecycleReservation(RetainedReservation),
}

impl From<ReservationRequest> for CapitalAdmissionReservationEvidence {
    fn from(request: ReservationRequest) -> Self {
        Self::NtOpenOrder(request)
    }
}

/// Serialize access to this gate behind one actor, mutex, or exclusive borrow.
#[derive(Debug, Clone)]
pub struct CapitalAdmissionGate {
    reservation_ledger: ReservationLedger,
}

impl CapitalAdmissionGate {
    pub fn unreconciled() -> Self {
        Self {
            reservation_ledger: ReservationLedger::unreconciled(),
        }
    }

    pub fn reconciled() -> Self {
        Self {
            reservation_ledger: ReservationLedger::reconciled(),
        }
    }

    pub fn is_reconciled(&self) -> bool {
        self.reservation_ledger.is_reconciled()
    }

    pub fn invalidate_reconciliation(&mut self) {
        self.reservation_ledger.invalidate_reconciliation();
    }

    pub fn rebuild_open_order_reservations(
        &mut self,
        pool: &CapitalPoolSnapshot,
        reservation_evidence: &[CapitalAdmissionReservationEvidence],
        now_ns: u64,
        min_remaining_pool_balance: Option<Decimal>,
    ) -> CapitalAdmissionRebuildDecision {
        // Rebuild off to the side. A rejected candidate invalidates new admission without erasing
        // liabilities already owned by the live ledger.
        let mut rebuilt_ledger = ReservationLedger::reconciled();
        for (index, evidence) in reservation_evidence.iter().cloned().enumerate() {
            let candidate = match evidence {
                CapitalAdmissionReservationEvidence::NtOpenOrder(reservation)
                    if pool.pool_id != reservation.pool_id =>
                {
                    Err(ReservationRejectionReason::PoolMismatch)
                }
                CapitalAdmissionReservationEvidence::NtOpenOrder(reservation) => {
                    let decision = rebuilt_ledger.reserve(
                        pool,
                        &reservation,
                        now_ns,
                        min_remaining_pool_balance,
                    );
                    match decision.accepted {
                        true => Ok(()),
                        false => Err(decision
                            .reason
                            .unwrap_or(ReservationRejectionReason::InvalidRequest)),
                    }
                }
                CapitalAdmissionReservationEvidence::RetainedLifecycleReservation(retained) => {
                    rebuilt_ledger
                        .carry_retained(&pool.pool_id, retained)
                        .map(drop)
                }
            };
            if let Err(reason) = candidate {
                self.invalidate_reconciliation();
                return CapitalAdmissionRebuildDecision {
                    accepted: false,
                    reason: Some(reason),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: index,
                    live_reserved_liability: self.live_reserved_liability(&pool.pool_id),
                };
            }
        }

        self.reservation_ledger = rebuilt_ledger;
        CapitalAdmissionRebuildDecision {
            accepted: true,
            reason: None,
            attempted_reservation_count: reservation_evidence.len(),
            rebuilt_reservation_count: reservation_evidence.len(),
            live_reserved_liability: self.live_reserved_liability(&pool.pool_id),
        }
    }

    pub fn retained_reservation(
        &self,
        pool_id: &str,
        collateral_group_id: &str,
        reservation_id: &str,
    ) -> Result<RetainedReservation, ReservationRejectionReason> {
        self.reservation_ledger
            .retained_reservation(pool_id, collateral_group_id, reservation_id)
    }

    pub fn remove_existing(
        &mut self,
        pool_id: &str,
        collateral_group_id: &str,
        reservation_id: &str,
    ) -> crate::bolt_v3_capital_reservation::ReservationReleaseDecision {
        self.reservation_ledger
            .remove_existing(pool_id, collateral_group_id, reservation_id)
    }

    pub fn evaluate_and_reserve(
        &mut self,
        inputs: CapitalAdmissionGateInputs<'_>,
    ) -> CapitalAdmissionDecision {
        evaluate_capital_admission(CapitalAdmissionInputs {
            request: inputs.request,
            state: inputs.state,
            policy: inputs.policy,
            loss_policy: inputs.loss_policy,
            capital_pool: inputs.capital_pool,
            reservation_ledger: &mut self.reservation_ledger,
        })
    }

    pub fn live_reserved_liability(&self, pool_id: &str) -> Decimal {
        self.reservation_ledger.live_reserved_liability(pool_id)
    }

    pub fn rollback_uncommitted_reservation(
        &mut self,
        pool_id: &str,
        request_id: &str,
    ) -> Option<Decimal> {
        self.reservation_ledger
            .rollback_uncommitted(pool_id, request_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapitalAdmissionReason {
    Loss(LossHaltReason),
    Reservation(ReservationRejectionReason),
    Liability(LiabilityError),
    Invariant(CapitalAdmissionInvariant),
    MissingNtState,
    StaleNtState(CapitalAdmissionStateEvidenceKind),
    UnattributedNtState(CapitalAdmissionStateEvidenceKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionInvariant {
    AcceptedReservationHasRejectionReason,
    ContradictoryReservationDecisionContext,
    MissingReservationRejectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidatedReservationDecision {
    Accepted,
    Rejected(ReservationRejectionReason),
}

fn validate_reservation_decision(
    decision: &ReservationDecision,
) -> Result<ValidatedReservationDecision, CapitalAdmissionInvariant> {
    match (decision.accepted, decision.reason, decision.available_after) {
        (true, None, Some(_)) => Ok(ValidatedReservationDecision::Accepted),
        (false, Some(reason), None) => Ok(ValidatedReservationDecision::Rejected(reason)),
        (false, None, _) => Err(CapitalAdmissionInvariant::MissingReservationRejectionReason),
        (true, Some(_), _) => Err(CapitalAdmissionInvariant::AcceptedReservationHasRejectionReason),
        _ => Err(CapitalAdmissionInvariant::ContradictoryReservationDecisionContext),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionDecision {
    pub accepted: bool,
    pub original_quantity: Decimal,
    pub accepted_quantity: Option<Decimal>,
    pub calculated_liability: Option<Decimal>,
    pub reserved_liability: Option<Decimal>,
    pub pool_id: String,
    pub evidence: CapitalAdmissionEvidence,
    pub reasons: Vec<CapitalAdmissionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionEvidenceKind {
    Portfolio,
    ProviderCollateralAllowance,
    OrderLifecycle,
    ProductState,
    ReservationLedger,
    LossSnapshot,
    LiabilityCalculator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionEvidenceSource {
    pub kind: CapitalAdmissionEvidenceKind,
    pub source: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionEvidence {
    pub sources: Vec<CapitalAdmissionEvidenceSource>,
    pub original_quantity: Decimal,
    pub accepted_quantity: Option<Decimal>,
    pub calculated_liability: Option<Decimal>,
    pub reserved_liability: Option<Decimal>,
}

pub fn evaluate_capital_admission(inputs: CapitalAdmissionInputs<'_>) -> CapitalAdmissionDecision {
    let original_quantity = inputs.request.quantity;
    let pool_id = inputs.request.pool_id.clone();
    let max_snapshot_age_ns = inputs.capital_pool.max_snapshot_age_ns;

    if inputs.capital_pool.pool_id != pool_id {
        return rejected_capital_admission(
            original_quantity,
            pool_id,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::PoolMismatch,
            )],
        );
    }

    let Some(state) = inputs.state else {
        return rejected_capital_admission(
            original_quantity,
            pool_id,
            vec![CapitalAdmissionReason::MissingNtState],
        );
    };

    let state_evidence = match validate_nt_derived_capital_admission_state(
        Some(state),
        inputs.request.now_ns,
        max_snapshot_age_ns,
    ) {
        Ok(state_evidence) => state_evidence,
        Err(error) => {
            return rejected_capital_admission(
                original_quantity,
                pool_id,
                vec![state_error_reason(error)],
            );
        }
    };

    if let Some(loss_policy) = inputs.loss_policy {
        let loss_decision = evaluate_loss_admission(
            loss_policy,
            state.loss_snapshot.as_ref(),
            inputs.request.now_ns,
        );
        if !loss_decision.accepted {
            return rejected_capital_admission(
                original_quantity,
                pool_id,
                loss_decision
                    .halt_reasons
                    .into_iter()
                    .map(CapitalAdmissionReason::Loss)
                    .collect(),
            );
        }
    }

    let calculator = PredictionMarketBinaryLiabilityCalculator;
    let liability_quote = match calculator.worst_case_liability(
        inputs.request,
        &state.product_state,
        inputs.policy,
    ) {
        Ok(liability_quote) => liability_quote,
        Err(error) => {
            return rejected_capital_admission(
                original_quantity,
                pool_id,
                vec![CapitalAdmissionReason::Liability(error)],
            );
        }
    };

    let reservation_request = ReservationRequest {
        request_id: inputs.request.intent_id.clone(),
        pool_id: inputs.request.pool_id.clone(),
        collateral_group_id: collateral_group_id(&state.product_state),
        liability: liability_quote.reserved_liability,
        observed_at_ns: inputs.request.now_ns,
        evidence_label: liability_quote.evidence_label.clone(),
    };
    let reservation_decision = inputs.reservation_ledger.reserve(
        inputs.capital_pool,
        &reservation_request,
        inputs.request.now_ns,
        inputs.policy.min_remaining_pool_balance,
    );
    let reservation_decision = match validate_reservation_decision(&reservation_decision) {
        Ok(decision) => decision,
        Err(invariant) => {
            inputs
                .reservation_ledger
                .rollback_uncommitted(&inputs.request.pool_id, &inputs.request.intent_id);
            return rejected_capital_admission_with_liability(
                original_quantity,
                pool_id,
                liability_quote.calculated_liability,
                liability_quote.reserved_liability,
                admission_evidence(&state_evidence, inputs.request.now_ns, &liability_quote),
                vec![CapitalAdmissionReason::Invariant(invariant)],
            );
        }
    };
    if let ValidatedReservationDecision::Rejected(reason) = reservation_decision {
        return rejected_capital_admission_with_liability(
            original_quantity,
            pool_id,
            liability_quote.calculated_liability,
            liability_quote.reserved_liability,
            admission_evidence(&state_evidence, inputs.request.now_ns, &liability_quote),
            vec![CapitalAdmissionReason::Reservation(reason)],
        );
    }

    CapitalAdmissionDecision {
        accepted: true,
        original_quantity,
        accepted_quantity: Some(liability_quote.accepted_quantity),
        calculated_liability: Some(liability_quote.calculated_liability),
        reserved_liability: Some(liability_quote.reserved_liability),
        pool_id,
        evidence: admission_evidence(&state_evidence, inputs.request.now_ns, &liability_quote),
        reasons: Vec::new(),
    }
}

fn rejected_capital_admission(
    original_quantity: Decimal,
    pool_id: String,
    reasons: Vec<CapitalAdmissionReason>,
) -> CapitalAdmissionDecision {
    CapitalAdmissionDecision {
        accepted: false,
        original_quantity,
        accepted_quantity: None,
        calculated_liability: None,
        reserved_liability: None,
        pool_id,
        evidence: empty_evidence(original_quantity),
        reasons,
    }
}

fn rejected_capital_admission_with_liability(
    original_quantity: Decimal,
    pool_id: String,
    calculated_liability: Decimal,
    reserved_liability: Decimal,
    evidence: CapitalAdmissionEvidence,
    reasons: Vec<CapitalAdmissionReason>,
) -> CapitalAdmissionDecision {
    CapitalAdmissionDecision {
        accepted: false,
        original_quantity,
        accepted_quantity: None,
        calculated_liability: Some(calculated_liability),
        reserved_liability: Some(reserved_liability),
        pool_id,
        evidence,
        reasons,
    }
}

fn empty_evidence(original_quantity: Decimal) -> CapitalAdmissionEvidence {
    CapitalAdmissionEvidence {
        sources: Vec::new(),
        original_quantity,
        accepted_quantity: None,
        calculated_liability: None,
        reserved_liability: None,
    }
}

fn admission_evidence(
    state_evidence: &CapitalAdmissionStateEvidence,
    liability_observed_at_ns: u64,
    liability_quote: &LiabilityQuote,
) -> CapitalAdmissionEvidence {
    let mut sources = state_evidence
        .sources
        .iter()
        .filter_map(|source| {
            Some(CapitalAdmissionEvidenceSource {
                kind: capital_admission_evidence_kind(source.kind)?,
                source: source.source.clone(),
                observed_at_ns: source.observed_at_ns,
            })
        })
        .collect::<Vec<_>>();
    sources.push(CapitalAdmissionEvidenceSource {
        kind: CapitalAdmissionEvidenceKind::LiabilityCalculator,
        source: liability_quote.evidence_label.clone(),
        observed_at_ns: liability_observed_at_ns,
    });

    CapitalAdmissionEvidence {
        sources,
        original_quantity: liability_quote.original_quantity,
        accepted_quantity: Some(liability_quote.accepted_quantity),
        calculated_liability: Some(liability_quote.calculated_liability),
        reserved_liability: Some(liability_quote.reserved_liability),
    }
}

fn capital_admission_evidence_kind(
    kind: CapitalAdmissionStateEvidenceKind,
) -> Option<CapitalAdmissionEvidenceKind> {
    match kind {
        CapitalAdmissionStateEvidenceKind::State => None,
        CapitalAdmissionStateEvidenceKind::Portfolio => {
            Some(CapitalAdmissionEvidenceKind::Portfolio)
        }
        CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance => {
            Some(CapitalAdmissionEvidenceKind::ProviderCollateralAllowance)
        }
        CapitalAdmissionStateEvidenceKind::OrderLifecycle => {
            Some(CapitalAdmissionEvidenceKind::OrderLifecycle)
        }
        CapitalAdmissionStateEvidenceKind::ProductState => {
            Some(CapitalAdmissionEvidenceKind::ProductState)
        }
        CapitalAdmissionStateEvidenceKind::ReservationLedger => {
            Some(CapitalAdmissionEvidenceKind::ReservationLedger)
        }
        CapitalAdmissionStateEvidenceKind::LossSnapshot => {
            Some(CapitalAdmissionEvidenceKind::LossSnapshot)
        }
    }
}

fn state_error_reason(error: CapitalAdmissionStateError) -> CapitalAdmissionReason {
    match error {
        CapitalAdmissionStateError::MissingNtState => CapitalAdmissionReason::MissingNtState,
        CapitalAdmissionStateError::StaleNtState(kind) => {
            CapitalAdmissionReason::StaleNtState(kind)
        }
        CapitalAdmissionStateError::UnattributedState(kind) => {
            CapitalAdmissionReason::UnattributedNtState(kind)
        }
    }
}

fn collateral_group_id(state: &ProductAdmissionSnapshot) -> String {
    match state {
        ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => {
            snapshot.collateral_coupled_group_id.clone()
        }
    }
}

impl PredictionMarketBinaryLiabilityCalculator {
    pub fn worst_case_liability(
        &self,
        request: &CapitalAdmissionRequest,
        state: &ProductAdmissionSnapshot,
        policy: &CapitalAdmissionPolicy,
    ) -> Result<LiabilityQuote, LiabilityError> {
        let ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) = state;
        validate_request(request)?;
        validate_liquidity(request)?;
        let fee_policy = policy
            .fee_slippage_policy
            .as_ref()
            .ok_or(LiabilityError::MissingFeePolicy)?;
        validate_fee_slippage_policy(fee_policy)?;

        let base_liability = match request.side {
            IntentSide::Buy => request
                .quantity
                .checked_mul(request.limit_price)
                .ok_or(LiabilityError::ArithmeticOverflow)?,
            IntentSide::Sell => Decimal::ZERO,
        };
        let liability = base_liability
            .checked_add(fee_policy.max_fee_liability)
            .and_then(|liability| liability.checked_add(fee_policy.max_slippage_liability))
            .ok_or(LiabilityError::ArithmeticOverflow)?;

        match request.side {
            IntentSide::Buy => {
                if snapshot.collateral_allowance < liability {
                    return Err(LiabilityError::InsufficientAllowance);
                }
            }
            IntentSide::Sell => {
                let outcome_position = if request.instrument_id == snapshot.yes_instrument_id {
                    snapshot.yes_position
                } else if request.instrument_id == snapshot.no_instrument_id {
                    snapshot.no_position
                } else {
                    return Err(LiabilityError::InsufficientInventory);
                };
                if outcome_position < request.quantity {
                    return Err(LiabilityError::InsufficientInventory);
                }
            }
        }

        Ok(LiabilityQuote {
            original_quantity: request.quantity,
            accepted_quantity: request.quantity,
            calculated_liability: liability,
            reserved_liability: liability,
            evidence_label: snapshot.source.clone(),
        })
    }
}

fn validate_request(request: &CapitalAdmissionRequest) -> Result<(), LiabilityError> {
    if request.quantity <= Decimal::ZERO {
        return Err(LiabilityError::InvalidIntentQuantity);
    }
    if request.limit_price < Decimal::ZERO || request.limit_price > Decimal::ONE {
        return Err(LiabilityError::InvalidIntentPrice);
    }
    Ok(())
}

fn validate_liquidity(request: &CapitalAdmissionRequest) -> Result<(), LiabilityError> {
    if request.liquidity == IntentLiquidity::RestingMaker
        && request.quote_set_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(LiabilityError::MissingQuoteSetId);
    }
    Ok(())
}

fn validate_fee_slippage_policy(policy: &FeeSlippagePolicy) -> Result<(), LiabilityError> {
    if policy.max_fee_liability < Decimal::ZERO {
        return Err(LiabilityError::MissingFeePolicy);
    }
    if policy.max_slippage_liability < Decimal::ZERO {
        return Err(LiabilityError::MissingSlippagePolicy);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::bolt_v3_capital_admission_state::{
        CapitalAdmissionStateEvidenceKind, NtDerivedCapitalAdmissionState,
        OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
        ProviderCollateralAllowanceSnapshot, ReservationLedgerSnapshot,
    };
    use crate::bolt_v3_capital_reservation::{
        CapitalPoolSnapshot, ReservationDecision, ReservationLedger, ReservationRejectionReason,
        ReservationRequest,
    };
    use crate::bolt_v3_loss_governor::{
        LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSnapshotSource,
        LossSourceObservationTimestamps,
    };

    use super::{
        CapitalAdmissionEvidenceKind, CapitalAdmissionGate, CapitalAdmissionGateInputs,
        CapitalAdmissionInputs, CapitalAdmissionInvariant, CapitalAdmissionPolicy,
        CapitalAdmissionReason, CapitalAdmissionRequest, CapitalAdmissionReservationEvidence,
        FeeSlippagePolicy, IntentLiquidity, IntentOrderKind, IntentSide, LiabilityError,
        PredictionMarketAdmissionSnapshot, PredictionMarketBinaryLiabilityCalculator,
        ProductAdmissionSnapshot, ProductKind, ValidatedReservationDecision,
        evaluate_capital_admission, validate_reservation_decision,
    };

    fn policy() -> CapitalAdmissionPolicy {
        CapitalAdmissionPolicy {
            min_remaining_pool_balance: None,
            fee_slippage_policy: Some(FeeSlippagePolicy {
                max_fee_liability: Decimal::new(10, 2),
                max_slippage_liability: Decimal::new(20, 2),
            }),
        }
    }

    fn request(side: IntentSide, liquidity: IntentLiquidity) -> CapitalAdmissionRequest {
        CapitalAdmissionRequest {
            intent_id: "intent-1".to_string(),
            strategy_id: "strategy-1".to_string(),
            instrument_id: "instrument-1".to_string(),
            pool_id: "pool-1".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            side,
            quantity: Decimal::new(10, 0),
            limit_price: Decimal::new(40, 2),
            order_kind: IntentOrderKind::Limit,
            liquidity,
            quote_set_id: None,
            now_ns: 1_000,
        }
    }

    fn state() -> ProductAdmissionSnapshot {
        ProductAdmissionSnapshot::PredictionMarketBinary(PredictionMarketAdmissionSnapshot {
            source: "nt_account_and_position_snapshot".to_string(),
            observed_at_ns: 900,
            yes_instrument_id: "instrument-1".to_string(),
            no_instrument_id: "instrument-1-no".to_string(),
            yes_position: Decimal::new(10, 0),
            no_position: Decimal::ZERO,
            collateral_allowance: Decimal::new(100, 0),
            collateral_coupled_group_id: "group-1".to_string(),
        })
    }

    fn nt_state(loss_snapshot: Option<LossSnapshot>) -> NtDerivedCapitalAdmissionState {
        NtDerivedCapitalAdmissionState {
            source: "nt_capital_admission_state".to_string(),
            observed_at_ns: 1_000,
            portfolio: PortfolioCapitalAdmissionSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 1_000,
                venue_id: "venue-a".to_string(),
                account_id: "account-1".to_string(),
                collateral_currency: "USD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            provider_collateral_allowance: ProviderCollateralAllowanceSnapshot {
                source: "operator-venue-allowance".to_string(),
                observed_at_ns: 1_000,
                venue_id: "venue-a".to_string(),
                account_id: "account-1".to_string(),
                collateral_currency: "USD".to_string(),
                collateral_allowance: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 1_000,
                open_order_count: 0,
                all_open_orders_attributed: true,
            },
            product_state: state(),
            reservation_snapshot: ReservationLedgerSnapshot {
                source: "bolt_reservation_ledger".to_string(),
                observed_at_ns: 1_000,
                all_live_reservations_attributed: true,
            },
            loss_snapshot,
        }
    }

    fn loss_policy() -> LossGovernorPolicy {
        LossGovernorPolicy {
            max_snapshot_age_ns: 100,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: None,
            max_rolling_loss: None,
            max_drawdown: None,
        }
    }

    fn capital_pool() -> CapitalPoolSnapshot {
        CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "pool-1".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        }
    }

    fn small_capital_pool() -> CapitalPoolSnapshot {
        CapitalPoolSnapshot {
            max_pool_liability: Decimal::new(4, 0),
            ..capital_pool()
        }
    }

    fn request_with_intent(intent_id: &str) -> CapitalAdmissionRequest {
        CapitalAdmissionRequest {
            intent_id: intent_id.to_string(),
            ..request(IntentSide::Buy, IntentLiquidity::Taker)
        }
    }

    fn single_order_capital_pool() -> CapitalPoolSnapshot {
        CapitalPoolSnapshot {
            max_pool_liability: Decimal::new(8, 0),
            ..capital_pool()
        }
    }

    fn rebuilt_open_order_reservation(intent_id: &str) -> ReservationRequest {
        ReservationRequest {
            request_id: intent_id.to_string(),
            pool_id: "pool-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            liability: Decimal::new(430, 2),
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_rebuild".to_string(),
        }
    }

    fn invalid_rebuilt_open_order_reservation(intent_id: &str) -> ReservationRequest {
        ReservationRequest {
            evidence_label: String::new(),
            ..rebuilt_open_order_reservation(intent_id)
        }
    }

    #[test]
    fn reservation_decision_context_is_structurally_validated() {
        let decision = |accepted, reason, available_after| ReservationDecision {
            accepted,
            reason,
            requested_liability: Decimal::ONE,
            available_before: Decimal::new(10, 0),
            available_after,
        };

        assert_eq!(
            validate_reservation_decision(&decision(
                false,
                Some(ReservationRejectionReason::OverBudget),
                None,
            )),
            Ok(ValidatedReservationDecision::Rejected(
                ReservationRejectionReason::OverBudget,
            ))
        );
        assert_eq!(
            validate_reservation_decision(&decision(false, None, None)),
            Err(CapitalAdmissionInvariant::MissingReservationRejectionReason)
        );
        assert_eq!(
            validate_reservation_decision(&decision(
                true,
                Some(ReservationRejectionReason::OverBudget),
                Some(Decimal::ZERO),
            )),
            Err(CapitalAdmissionInvariant::AcceptedReservationHasRejectionReason)
        );
        assert_eq!(
            validate_reservation_decision(&decision(true, None, None)),
            Err(CapitalAdmissionInvariant::ContradictoryReservationDecisionContext)
        );
    }

    #[test]
    fn prediction_market_binary_liability_formula_is_pinned() {
        let calculator = PredictionMarketBinaryLiabilityCalculator;

        let buy = calculator
            .worst_case_liability(
                &request(IntentSide::Buy, IntentLiquidity::Taker),
                &state(),
                &policy(),
            )
            .expect("fresh buy state should price liability");
        assert_eq!(buy.calculated_liability, Decimal::new(430, 2));
        assert_eq!(buy.reserved_liability, Decimal::new(430, 2));

        let sell = calculator
            .worst_case_liability(
                &request(IntentSide::Sell, IntentLiquidity::Taker),
                &state(),
                &policy(),
            )
            .expect("fresh sell state should price liability");
        assert_eq!(sell.calculated_liability, Decimal::new(30, 2));
        assert_eq!(sell.reserved_liability, Decimal::new(30, 2));

        let missing_fee_policy = CapitalAdmissionPolicy {
            fee_slippage_policy: None,
            ..policy()
        };
        assert_eq!(
            calculator
                .worst_case_liability(
                    &request(IntentSide::Buy, IntentLiquidity::Taker),
                    &state(),
                    &missing_fee_policy,
                )
                .expect_err("missing fee/slippage policy must fail closed"),
            LiabilityError::MissingFeePolicy
        );

        assert_eq!(
            calculator
                .worst_case_liability(
                    &request(IntentSide::Buy, IntentLiquidity::RestingMaker),
                    &state(),
                    &policy(),
                )
                .expect_err("resting maker intent must identify its quote set"),
            LiabilityError::MissingQuoteSetId
        );
    }

    #[test]
    fn prediction_market_binary_liability_overflow_rejects_fail_closed() {
        let calculator = PredictionMarketBinaryLiabilityCalculator;
        let mut overflow_policy = policy();
        overflow_policy.fee_slippage_policy = Some(FeeSlippagePolicy {
            max_fee_liability: Decimal::MAX,
            max_slippage_liability: Decimal::ZERO,
        });
        let mut overflow_state = state();
        let ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) = &mut overflow_state;
        snapshot.collateral_allowance = Decimal::MAX;

        assert_eq!(
            calculator
                .worst_case_liability(
                    &request(IntentSide::Buy, IntentLiquidity::Taker),
                    &overflow_state,
                    &overflow_policy,
                )
                .expect_err("overflowing liability arithmetic must fail closed"),
            LiabilityError::ArithmeticOverflow
        );
    }

    #[test]
    fn sizer_rejects_when_loss_governor_rejects() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-10, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &policy(),
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Loss(
                LossHaltReason::PerTradeLossLimit
            )]
        );
        assert_eq!(decision.accepted_quantity, None);
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn sizer_rejects_when_capital_reservation_rejects() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &policy(),
            loss_policy: Some(&loss_policy()),
            capital_pool: &small_capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(decision.calculated_liability, Some(Decimal::new(430, 2)));
        assert_eq!(decision.reserved_liability, Some(Decimal::new(430, 2)));
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn sizer_accepts_when_loss_liability_and_reservation_pass() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &policy(),
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(decision.accepted);
        assert!(decision.reasons.is_empty());
        assert_eq!(decision.original_quantity, Decimal::new(10, 0));
        assert_eq!(decision.accepted_quantity, Some(Decimal::new(10, 0)));
        assert_eq!(decision.calculated_liability, Some(Decimal::new(430, 2)));
        assert_eq!(decision.reserved_liability, Some(Decimal::new(430, 2)));
        assert_eq!(
            decision
                .evidence
                .sources
                .iter()
                .map(|source| source.kind)
                .collect::<Vec<_>>(),
            vec![
                CapitalAdmissionEvidenceKind::Portfolio,
                CapitalAdmissionEvidenceKind::ProviderCollateralAllowance,
                CapitalAdmissionEvidenceKind::OrderLifecycle,
                CapitalAdmissionEvidenceKind::ProductState,
                CapitalAdmissionEvidenceKind::ReservationLedger,
                CapitalAdmissionEvidenceKind::LossSnapshot,
                CapitalAdmissionEvidenceKind::LiabilityCalculator,
            ]
        );
        assert_eq!(
            ledger.live_reserved_liability("pool-1"),
            Decimal::new(430, 2)
        );
    }

    #[test]
    fn sizer_rejects_stale_nt_state_without_loss_governor() {
        let mut state = nt_state(None);
        state.portfolio.observed_at_ns = 899;
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &policy(),
            loss_policy: None,
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::StaleNtState(
                CapitalAdmissionStateEvidenceKind::Portfolio
            )]
        );
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn sizer_rejects_when_min_remaining_pool_balance_would_be_breached() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let mut floor_policy = policy();
        floor_policy.min_remaining_pool_balance = Some(Decimal::new(96, 0));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &floor_policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(decision.reserved_liability, Some(Decimal::new(430, 2)));
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn admission_rejects_pool_mismatch_before_nt_state_validation() {
        let mut foreign_pool_request = request(IntentSide::Buy, IntentLiquidity::Taker);
        foreign_pool_request.pool_id = "pool-2".to_string();
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &foreign_pool_request,
            state: None,
            policy: &policy(),
            loss_policy: None,
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::PoolMismatch
            )]
        );
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
        assert_eq!(ledger.live_reserved_liability("pool-2"), Decimal::ZERO);
    }

    #[test]
    fn restart_requires_rebuilt_open_order_reservations_before_admission() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let mut unreconciled_ledger = ReservationLedger::unreconciled();

        let decision = evaluate_capital_admission(CapitalAdmissionInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &policy(),
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
            reservation_ledger: &mut unreconciled_ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(
            unreconciled_ledger.live_reserved_liability("pool-1"),
            Decimal::ZERO
        );
    }

    #[test]
    fn concurrent_reservations_share_one_serialized_budget() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let first_request = request_with_intent("intent-1");
        let second_request = request_with_intent("intent-2");
        let mut gate = CapitalAdmissionGate::reconciled();

        let first = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &first_request,
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });
        let second = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &second_request,
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(first.accepted);
        assert!(!second.accepted);
        assert_eq!(
            second.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }

    #[test]
    fn startup_rebuild_rejects_foreign_pool_reservation_fail_closed() {
        let capital_pool = capital_pool();
        let mut foreign_reservation = rebuilt_open_order_reservation("intent-foreign");
        foreign_reservation.pool_id = "pool-2".to_string();
        let mut gate = CapitalAdmissionGate::reconciled();

        let rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[foreign_reservation.into()],
            1_000,
            None,
        );

        assert!(!rebuild.accepted);
        assert_eq!(
            rebuild.reason,
            Some(ReservationRejectionReason::PoolMismatch)
        );
        assert_eq!(rebuild.attempted_reservation_count, 1);
        assert_eq!(rebuild.rebuilt_reservation_count, 0);
        assert_eq!(rebuild.live_reserved_liability, Decimal::ZERO);
        assert!(!gate.is_reconciled());
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);
        assert_eq!(gate.live_reserved_liability("pool-2"), Decimal::ZERO);
    }

    #[test]
    fn admission_gate_fails_closed_until_reconciled() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let request = request_with_intent("intent-1");
        let mut gate = CapitalAdmissionGate::unreconciled();

        let decision = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &request,
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn restart_rebuilds_open_order_reservations_before_reopening_gate() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let open_reservation = rebuilt_open_order_reservation("intent-open");
        let mut gate = CapitalAdmissionGate::unreconciled();

        let rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[open_reservation.into()],
            1_000,
            None,
        );

        assert!(rebuild.accepted);
        assert_eq!(rebuild.rebuilt_reservation_count, 1);
        assert_eq!(rebuild.live_reserved_liability, Decimal::new(430, 2));

        let decision = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }

    #[test]
    fn restart_rebuild_with_no_open_orders_reopens_gate_with_zero_reservations() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = capital_pool();
        let mut gate = CapitalAdmissionGate::unreconciled();

        let rebuild = gate.rebuild_open_order_reservations(&capital_pool, &[], 1_000, None);

        assert!(rebuild.accepted);
        assert_eq!(rebuild.attempted_reservation_count, 0);
        assert_eq!(rebuild.rebuilt_reservation_count, 0);
        assert_eq!(rebuild.live_reserved_liability, Decimal::ZERO);

        let decision = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(decision.accepted);
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }

    #[test]
    fn failed_restart_rebuild_keeps_gate_closed_without_partial_reservations() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let invalid_reservation = invalid_rebuilt_open_order_reservation("intent-open");
        let mut gate = CapitalAdmissionGate::unreconciled();

        let rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[invalid_reservation.into()],
            1_000,
            None,
        );

        assert!(!rebuild.accepted);
        assert_eq!(
            rebuild.reason,
            Some(ReservationRejectionReason::MissingEvidence)
        );
        assert_eq!(rebuild.attempted_reservation_count, 1);
        assert_eq!(rebuild.rebuilt_reservation_count, 0);
        assert_eq!(rebuild.live_reserved_liability, Decimal::ZERO);

        let decision = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn failed_restart_rebuild_discards_reservations_staged_before_later_failure() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let open_reservation = rebuilt_open_order_reservation("intent-open");
        let invalid_reservation = invalid_rebuilt_open_order_reservation("intent-invalid");
        let mut gate = CapitalAdmissionGate::unreconciled();

        let rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[open_reservation.into(), invalid_reservation.into()],
            1_000,
            None,
        );

        assert!(!rebuild.accepted);
        assert_eq!(
            rebuild.reason,
            Some(ReservationRejectionReason::MissingEvidence)
        );
        assert_eq!(rebuild.attempted_reservation_count, 2);
        assert_eq!(rebuild.rebuilt_reservation_count, 1);
        assert_eq!(rebuild.live_reserved_liability, Decimal::ZERO);

        let decision = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn failed_restart_rebuild_after_prior_success_preserves_live_reservations() {
        let loss_snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let open_reservation = rebuilt_open_order_reservation("intent-open");
        let invalid_reservation = invalid_rebuilt_open_order_reservation("intent-invalid");
        let mut gate = CapitalAdmissionGate::unreconciled();

        let initial_rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[open_reservation.into()],
            1_000,
            None,
        );
        assert!(initial_rebuild.accepted);
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));

        let failed_rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[invalid_reservation.into()],
            1_000,
            None,
        );

        assert!(!failed_rebuild.accepted);
        assert_eq!(
            failed_rebuild.reason,
            Some(ReservationRejectionReason::MissingEvidence)
        );
        assert_eq!(failed_rebuild.live_reserved_liability, Decimal::new(430, 2));
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
        assert!(!gate.is_reconciled());

        let decision = gate.evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }

    #[test]
    fn retained_lifecycle_reservation_carries_exact_live_liability_without_readmission() {
        let initial_pool = single_order_capital_pool();
        let reservation = rebuilt_open_order_reservation("intent-retained");
        let mut gate = CapitalAdmissionGate::unreconciled();
        let initial =
            gate.rebuild_open_order_reservations(&initial_pool, &[reservation.into()], 1_000, None);
        assert!(initial.accepted);

        gate.invalidate_reconciliation();
        let retained = gate
            .retained_reservation("pool-1", "group-1", "intent-retained")
            .expect("exact live reservation should remain available while unreconciled");
        let shrunken_newer_pool = CapitalPoolSnapshot {
            observed_at_ns: 2_000,
            max_pool_liability: Decimal::ONE,
            ..initial_pool
        };
        let evidence =
            [CapitalAdmissionReservationEvidence::RetainedLifecycleReservation(retained)];

        let rebuilt =
            gate.rebuild_open_order_reservations(&shrunken_newer_pool, &evidence, 2_000, None);

        assert!(rebuilt.accepted);
        assert_eq!(rebuilt.rebuilt_reservation_count, 1);
        assert_eq!(rebuilt.live_reserved_liability, Decimal::new(430, 2));
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }
}
