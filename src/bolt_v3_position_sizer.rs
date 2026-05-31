use rust_decimal::Decimal;

use crate::bolt_v3_capital_reservation::{
    CapitalPoolSnapshot, ReservationLedger, ReservationRejectionReason, ReservationReleaseDecision,
    ReservationRequest,
};
use crate::bolt_v3_loss_governor::{LossGovernorPolicy, LossHaltReason, evaluate_loss_admission};
use crate::bolt_v3_sizing_state::{
    NtDerivedSizingState, SizingStateError, SizingStateEvidence, SizingStateEvidenceKind,
    validate_nt_derived_sizing_state,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizingPolicy {
    pub mode: SizingMode,
    pub max_order_liability: Option<Decimal>,
    pub min_remaining_pool_balance: Option<Decimal>,
    pub fee_slippage_policy: Option<FeeSlippagePolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingMode {
    RejectOnly,
    ExplicitClipToAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSlippagePolicy {
    pub max_fee_liability: Decimal,
    pub max_slippage_liability: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSizingRequest {
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
pub enum ProductSizingSnapshot {
    PredictionMarketBinary(PredictionMarketSizingSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionMarketSizingSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub yes_position: Decimal,
    pub no_position: Decimal,
    pub pusd_allowance: Decimal,
    pub conditional_token_allowance: Decimal,
    pub collateral_coupled_group_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiabilityQuote {
    pub original_quantity: Decimal,
    pub sized_quantity: Decimal,
    pub liability_before_sizing: Decimal,
    pub liability_after_sizing: Decimal,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictionMarketBinaryLiabilityCalculator;

pub struct PositionSizingInputs<'a> {
    pub request: &'a PositionSizingRequest,
    pub state: Option<&'a NtDerivedSizingState>,
    pub policy: &'a SizingPolicy,
    pub loss_policy: Option<&'a LossGovernorPolicy>,
    pub capital_pool: &'a CapitalPoolSnapshot,
    pub reservation_ledger: &'a mut ReservationLedger,
}

pub struct PositionSizingGateInputs<'a> {
    pub request: &'a PositionSizingRequest,
    pub state: Option<&'a NtDerivedSizingState>,
    pub policy: &'a SizingPolicy,
    pub loss_policy: Option<&'a LossGovernorPolicy>,
    pub capital_pool: &'a CapitalPoolSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSizingRebuildDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub rebuilt_reservation_count: usize,
    pub live_reserved_liability: Decimal,
}

#[derive(Debug)]
pub struct PositionSizingAdmissionGate {
    reservation_ledger: ReservationLedger,
}

impl PositionSizingAdmissionGate {
    /// Own this gate behind one actor, mutex, or exclusive borrow for live submit admission.
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

    pub fn rebuild_open_order_reservations(
        &mut self,
        pool: &CapitalPoolSnapshot,
        open_order_reservations: &[ReservationRequest],
        now_ns: u64,
        min_remaining_pool_balance: Option<Decimal>,
    ) -> PositionSizingRebuildDecision {
        let mut rebuilt_ledger = ReservationLedger::reconciled();
        for (index, reservation) in open_order_reservations.iter().enumerate() {
            let decision = rebuilt_ledger.reserve(
                pool,
                reservation,
                now_ns,
                pool.max_snapshot_age_ns,
                min_remaining_pool_balance,
            );
            if !decision.accepted {
                let attempted_reservation_count = open_order_reservations[..=index].len();
                return PositionSizingRebuildDecision {
                    accepted: false,
                    reason: decision.reason,
                    attempted_reservation_count,
                    rebuilt_reservation_count: index,
                    live_reserved_liability: self.live_reserved_liability(&pool.pool_id),
                };
            }
        }

        self.reservation_ledger = rebuilt_ledger;
        PositionSizingRebuildDecision {
            accepted: true,
            reason: None,
            attempted_reservation_count: open_order_reservations.len(),
            rebuilt_reservation_count: open_order_reservations.len(),
            live_reserved_liability: self.live_reserved_liability(&pool.pool_id),
        }
    }

    pub fn evaluate_and_reserve(
        &mut self,
        inputs: PositionSizingGateInputs<'_>,
    ) -> SizedAdmissionDecision {
        evaluate_position_sizing(PositionSizingInputs {
            request: inputs.request,
            state: inputs.state,
            policy: inputs.policy,
            loss_policy: inputs.loss_policy,
            capital_pool: inputs.capital_pool,
            reservation_ledger: &mut self.reservation_ledger,
        })
    }

    pub fn release_pending_reservation(
        &mut self,
        intent_id: &str,
        evidence_label: &str,
    ) -> ReservationReleaseDecision {
        self.reservation_ledger.release(intent_id, evidence_label)
    }

    pub fn live_reserved_liability(&self, pool_id: &str) -> Decimal {
        self.reservation_ledger.live_reserved_liability(pool_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizedAdmissionReason {
    Loss(LossHaltReason),
    Reservation(ReservationRejectionReason),
    Liability(LiabilityError),
    MissingNtState,
    StaleNtState(SizingStateEvidenceKind),
    UnattributedNtState(SizingStateEvidenceKind),
    OverMaxOrderLiability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizedAdmissionDecision {
    pub accepted: bool,
    pub original_quantity: Decimal,
    pub sized_quantity: Option<Decimal>,
    pub liability_before_sizing: Option<Decimal>,
    pub liability_after_sizing: Option<Decimal>,
    pub pool_id: String,
    pub evidence: SizedAdmissionEvidence,
    pub reasons: Vec<SizedAdmissionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingEvidenceKind {
    Portfolio,
    OrderLifecycle,
    ProductState,
    ReservationLedger,
    LossSnapshot,
    LiabilityCalculator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizingEvidenceSource {
    pub kind: SizingEvidenceKind,
    pub source: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizedAdmissionEvidence {
    pub sources: Vec<SizingEvidenceSource>,
    pub original_quantity: Decimal,
    pub sized_quantity: Option<Decimal>,
    pub liability_before_sizing: Option<Decimal>,
    pub liability_after_sizing: Option<Decimal>,
}

pub fn evaluate_position_sizing(inputs: PositionSizingInputs<'_>) -> SizedAdmissionDecision {
    let original_quantity = inputs.request.quantity;
    let pool_id = inputs.request.pool_id.clone();
    let max_snapshot_age_ns = inputs.capital_pool.max_snapshot_age_ns;

    let Some(state) = inputs.state else {
        return rejected_sizing(
            original_quantity,
            pool_id,
            vec![SizedAdmissionReason::MissingNtState],
        );
    };

    let state_evidence = match validate_nt_derived_sizing_state(
        Some(state),
        inputs.request.now_ns,
        max_snapshot_age_ns,
    ) {
        Ok(state_evidence) => state_evidence,
        Err(error) => {
            return rejected_sizing(original_quantity, pool_id, vec![state_error_reason(error)]);
        }
    };

    if let Some(loss_policy) = inputs.loss_policy {
        let loss_decision = evaluate_loss_admission(
            loss_policy,
            state.loss_snapshot.as_ref(),
            inputs.request.now_ns,
        );
        if !loss_decision.accepted {
            return rejected_sizing(
                original_quantity,
                pool_id,
                loss_decision
                    .halt_reasons
                    .into_iter()
                    .map(SizedAdmissionReason::Loss)
                    .collect(),
            );
        }
    }

    let calculator = PredictionMarketBinaryLiabilityCalculator;
    let mut liability_quote = match calculator.worst_case_liability(
        inputs.request,
        &state.product_state,
        inputs.policy,
    ) {
        Ok(liability_quote) => liability_quote,
        Err(error) => {
            return rejected_sizing(
                original_quantity,
                pool_id,
                vec![SizedAdmissionReason::Liability(error)],
            );
        }
    };

    if let Some(max_order_liability) = inputs.policy.max_order_liability
        && liability_quote.liability_after_sizing > max_order_liability
    {
        match inputs.policy.mode {
            SizingMode::RejectOnly => {
                return rejected_sizing_with_liability(
                    original_quantity,
                    pool_id,
                    liability_quote.liability_before_sizing,
                    liability_quote.liability_after_sizing,
                    admission_evidence(&state_evidence, inputs.request.now_ns, &liability_quote),
                    vec![SizedAdmissionReason::OverMaxOrderLiability],
                );
            }
            SizingMode::ExplicitClipToAvailable => {
                liability_quote = match clipped_liability_quote(
                    inputs.request,
                    inputs.policy,
                    &liability_quote,
                    max_order_liability,
                ) {
                    Ok(clipped_quote) => clipped_quote,
                    Err(reason) => {
                        return rejected_sizing_with_liability(
                            original_quantity,
                            pool_id,
                            liability_quote.liability_before_sizing,
                            liability_quote.liability_after_sizing,
                            admission_evidence(
                                &state_evidence,
                                inputs.request.now_ns,
                                &liability_quote,
                            ),
                            vec![reason],
                        );
                    }
                };
            }
        }
    }

    let reservation_request = ReservationRequest {
        request_id: inputs.request.intent_id.clone(),
        pool_id: inputs.request.pool_id.clone(),
        collateral_group_id: collateral_group_id(&state.product_state),
        liability: liability_quote.liability_after_sizing,
        observed_at_ns: inputs.request.now_ns,
        evidence_label: liability_quote.evidence_label.clone(),
    };
    let reservation_decision = inputs.reservation_ledger.reserve(
        inputs.capital_pool,
        &reservation_request,
        inputs.request.now_ns,
        max_snapshot_age_ns,
        inputs.policy.min_remaining_pool_balance,
    );
    if !reservation_decision.accepted {
        return rejected_sizing_with_liability(
            original_quantity,
            pool_id,
            liability_quote.liability_before_sizing,
            liability_quote.liability_after_sizing,
            admission_evidence(&state_evidence, inputs.request.now_ns, &liability_quote),
            vec![SizedAdmissionReason::Reservation(
                reservation_decision
                    .reason
                    .expect("rejected reservation decisions carry a reason"),
            )],
        );
    }

    SizedAdmissionDecision {
        accepted: true,
        original_quantity,
        sized_quantity: Some(liability_quote.sized_quantity),
        liability_before_sizing: Some(liability_quote.liability_before_sizing),
        liability_after_sizing: Some(liability_quote.liability_after_sizing),
        pool_id,
        evidence: admission_evidence(&state_evidence, inputs.request.now_ns, &liability_quote),
        reasons: Vec::new(),
    }
}

fn rejected_sizing(
    original_quantity: Decimal,
    pool_id: String,
    reasons: Vec<SizedAdmissionReason>,
) -> SizedAdmissionDecision {
    SizedAdmissionDecision {
        accepted: false,
        original_quantity,
        sized_quantity: None,
        liability_before_sizing: None,
        liability_after_sizing: None,
        pool_id,
        evidence: empty_evidence(original_quantity),
        reasons,
    }
}

fn rejected_sizing_with_liability(
    original_quantity: Decimal,
    pool_id: String,
    liability_before_sizing: Decimal,
    liability_after_sizing: Decimal,
    evidence: SizedAdmissionEvidence,
    reasons: Vec<SizedAdmissionReason>,
) -> SizedAdmissionDecision {
    SizedAdmissionDecision {
        accepted: false,
        original_quantity,
        sized_quantity: None,
        liability_before_sizing: Some(liability_before_sizing),
        liability_after_sizing: Some(liability_after_sizing),
        pool_id,
        evidence,
        reasons,
    }
}

fn empty_evidence(original_quantity: Decimal) -> SizedAdmissionEvidence {
    SizedAdmissionEvidence {
        sources: Vec::new(),
        original_quantity,
        sized_quantity: None,
        liability_before_sizing: None,
        liability_after_sizing: None,
    }
}

fn admission_evidence(
    state_evidence: &SizingStateEvidence,
    liability_observed_at_ns: u64,
    liability_quote: &LiabilityQuote,
) -> SizedAdmissionEvidence {
    let mut sources = state_evidence
        .sources
        .iter()
        .filter_map(|source| {
            Some(SizingEvidenceSource {
                kind: sizing_evidence_kind(source.kind)?,
                source: source.source.clone(),
                observed_at_ns: source.observed_at_ns,
            })
        })
        .collect::<Vec<_>>();
    sources.push(SizingEvidenceSource {
        kind: SizingEvidenceKind::LiabilityCalculator,
        source: liability_quote.evidence_label.clone(),
        observed_at_ns: liability_observed_at_ns,
    });

    SizedAdmissionEvidence {
        sources,
        original_quantity: liability_quote.original_quantity,
        sized_quantity: Some(liability_quote.sized_quantity),
        liability_before_sizing: Some(liability_quote.liability_before_sizing),
        liability_after_sizing: Some(liability_quote.liability_after_sizing),
    }
}

fn sizing_evidence_kind(kind: SizingStateEvidenceKind) -> Option<SizingEvidenceKind> {
    match kind {
        SizingStateEvidenceKind::State => None,
        SizingStateEvidenceKind::Portfolio => Some(SizingEvidenceKind::Portfolio),
        SizingStateEvidenceKind::OrderLifecycle => Some(SizingEvidenceKind::OrderLifecycle),
        SizingStateEvidenceKind::ProductState => Some(SizingEvidenceKind::ProductState),
        SizingStateEvidenceKind::ReservationLedger => Some(SizingEvidenceKind::ReservationLedger),
        SizingStateEvidenceKind::LossSnapshot => Some(SizingEvidenceKind::LossSnapshot),
    }
}

fn state_error_reason(error: SizingStateError) -> SizedAdmissionReason {
    match error {
        SizingStateError::MissingNtState => SizedAdmissionReason::MissingNtState,
        SizingStateError::StaleNtState(kind) => SizedAdmissionReason::StaleNtState(kind),
        SizingStateError::UnattributedState(kind) => {
            SizedAdmissionReason::UnattributedNtState(kind)
        }
    }
}

fn collateral_group_id(state: &ProductSizingSnapshot) -> String {
    match state {
        ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
            snapshot.collateral_coupled_group_id.clone()
        }
    }
}

fn clipped_liability_quote(
    request: &PositionSizingRequest,
    policy: &SizingPolicy,
    quote: &LiabilityQuote,
    max_order_liability: Decimal,
) -> Result<LiabilityQuote, SizedAdmissionReason> {
    let fee_policy = policy
        .fee_slippage_policy
        .as_ref()
        .ok_or(SizedAdmissionReason::Liability(
            LiabilityError::MissingFeePolicy,
        ))?;
    let additive_liability = fee_policy.max_fee_liability + fee_policy.max_slippage_liability;
    let available_base_liability = max_order_liability - additive_liability;
    if available_base_liability <= Decimal::ZERO {
        return Err(SizedAdmissionReason::OverMaxOrderLiability);
    }

    let liability_factor = match request.side {
        IntentSide::Buy => request.limit_price,
        IntentSide::Sell => Decimal::ONE - request.limit_price,
    };
    if liability_factor <= Decimal::ZERO {
        return Err(SizedAdmissionReason::OverMaxOrderLiability);
    }

    let candidate_quantity = available_base_liability / liability_factor;
    if candidate_quantity <= Decimal::ZERO {
        return Err(SizedAdmissionReason::OverMaxOrderLiability);
    }
    let sized_quantity = if candidate_quantity > request.quantity {
        request.quantity
    } else {
        candidate_quantity
    };
    let liability_after_sizing = sized_quantity * liability_factor + additive_liability;

    Ok(LiabilityQuote {
        original_quantity: quote.original_quantity,
        sized_quantity,
        liability_before_sizing: quote.liability_before_sizing,
        liability_after_sizing,
        evidence_label: quote.evidence_label.clone(),
    })
}

impl PredictionMarketBinaryLiabilityCalculator {
    pub fn worst_case_liability(
        &self,
        request: &PositionSizingRequest,
        state: &ProductSizingSnapshot,
        policy: &SizingPolicy,
    ) -> Result<LiabilityQuote, LiabilityError> {
        let ProductSizingSnapshot::PredictionMarketBinary(snapshot) = state;
        validate_request(request)?;
        validate_liquidity(request)?;
        let fee_policy = policy
            .fee_slippage_policy
            .as_ref()
            .ok_or(LiabilityError::MissingFeePolicy)?;
        validate_fee_slippage_policy(fee_policy)?;

        let base_liability = match request.side {
            IntentSide::Buy => request.quantity * request.limit_price,
            IntentSide::Sell => request.quantity * (Decimal::ONE - request.limit_price),
        };
        let liability =
            base_liability + fee_policy.max_fee_liability + fee_policy.max_slippage_liability;

        match request.side {
            IntentSide::Buy => {
                if snapshot.pusd_allowance < liability {
                    return Err(LiabilityError::InsufficientAllowance);
                }
            }
            IntentSide::Sell => {
                if snapshot.conditional_token_allowance < request.quantity {
                    return Err(LiabilityError::InsufficientAllowance);
                }
                if snapshot.yes_position < request.quantity {
                    return Err(LiabilityError::InsufficientInventory);
                }
            }
        }

        Ok(LiabilityQuote {
            original_quantity: request.quantity,
            sized_quantity: request.quantity,
            liability_before_sizing: liability,
            liability_after_sizing: liability,
            evidence_label: snapshot.source.clone(),
        })
    }
}

fn validate_request(request: &PositionSizingRequest) -> Result<(), LiabilityError> {
    if request.quantity <= Decimal::ZERO {
        return Err(LiabilityError::InvalidIntentQuantity);
    }
    if request.limit_price < Decimal::ZERO || request.limit_price > Decimal::ONE {
        return Err(LiabilityError::InvalidIntentPrice);
    }
    Ok(())
}

fn validate_liquidity(request: &PositionSizingRequest) -> Result<(), LiabilityError> {
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

    use crate::bolt_v3_capital_reservation::{
        CapitalPoolSnapshot, ReservationLedger, ReservationRejectionReason, ReservationRequest,
    };
    use crate::bolt_v3_loss_governor::{LossGovernorPolicy, LossHaltReason, LossSnapshot};
    use crate::bolt_v3_sizing_state::{
        NtDerivedSizingState, OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot,
        ReservationLedgerSnapshot, SizingStateEvidenceKind,
    };

    use super::{
        FeeSlippagePolicy, IntentLiquidity, IntentOrderKind, IntentSide, LiabilityError,
        PositionSizingAdmissionGate, PositionSizingGateInputs, PositionSizingInputs,
        PositionSizingRequest, PredictionMarketBinaryLiabilityCalculator,
        PredictionMarketSizingSnapshot, ProductKind, ProductSizingSnapshot, SizedAdmissionReason,
        SizingEvidenceKind, SizingMode, SizingPolicy, evaluate_position_sizing,
    };

    fn policy() -> SizingPolicy {
        SizingPolicy {
            mode: SizingMode::RejectOnly,
            max_order_liability: None,
            min_remaining_pool_balance: None,
            fee_slippage_policy: Some(FeeSlippagePolicy {
                max_fee_liability: Decimal::new(10, 2),
                max_slippage_liability: Decimal::new(20, 2),
            }),
        }
    }

    fn request(side: IntentSide, liquidity: IntentLiquidity) -> PositionSizingRequest {
        PositionSizingRequest {
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

    fn state() -> ProductSizingSnapshot {
        ProductSizingSnapshot::PredictionMarketBinary(PredictionMarketSizingSnapshot {
            source: "nt_account_and_position_snapshot".to_string(),
            observed_at_ns: 900,
            yes_position: Decimal::new(10, 0),
            no_position: Decimal::ZERO,
            pusd_allowance: Decimal::new(100, 0),
            conditional_token_allowance: Decimal::new(10, 0),
            collateral_coupled_group_id: "group-1".to_string(),
        })
    }

    fn nt_state(loss_snapshot: Option<LossSnapshot>) -> NtDerivedSizingState {
        NtDerivedSizingState {
            source: "nt_sizing_state".to_string(),
            observed_at_ns: 1_000,
            portfolio: PortfolioSizingSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 1_000,
                venue_id: "polymarket-clob".to_string(),
                account_id: "account-1".to_string(),
                collateral_currency: "PUSD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleSizingSnapshot {
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

    fn request_with_intent(intent_id: &str) -> PositionSizingRequest {
        PositionSizingRequest {
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
    fn prediction_market_binary_liability_formula_is_pinned() {
        let calculator = PredictionMarketBinaryLiabilityCalculator;

        let buy = calculator
            .worst_case_liability(
                &request(IntentSide::Buy, IntentLiquidity::Taker),
                &state(),
                &policy(),
            )
            .expect("fresh buy state should price liability");
        assert_eq!(buy.liability_before_sizing, Decimal::new(430, 2));
        assert_eq!(buy.liability_after_sizing, Decimal::new(430, 2));

        let sell = calculator
            .worst_case_liability(
                &request(IntentSide::Sell, IntentLiquidity::Taker),
                &state(),
                &policy(),
            )
            .expect("fresh sell state should price liability");
        assert_eq!(sell.liability_before_sizing, Decimal::new(630, 2));
        assert_eq!(sell.liability_after_sizing, Decimal::new(630, 2));

        let missing_fee_policy = SizingPolicy {
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
    fn sizer_rejects_when_loss_governor_rejects() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-10, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
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
            vec![SizedAdmissionReason::Loss(
                LossHaltReason::PerTradeLossLimit
            )]
        );
        assert_eq!(decision.sized_quantity, None);
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn sizer_rejects_when_capital_reservation_rejects() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
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
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(decision.liability_before_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(decision.liability_after_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn sizer_accepts_when_loss_liability_and_reservation_pass() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
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
        assert_eq!(decision.sized_quantity, Some(Decimal::new(10, 0)));
        assert_eq!(decision.liability_before_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(decision.liability_after_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(
            decision
                .evidence
                .sources
                .iter()
                .map(|source| source.kind)
                .collect::<Vec<_>>(),
            vec![
                SizingEvidenceKind::Portfolio,
                SizingEvidenceKind::OrderLifecycle,
                SizingEvidenceKind::ProductState,
                SizingEvidenceKind::ReservationLedger,
                SizingEvidenceKind::LossSnapshot,
                SizingEvidenceKind::LiabilityCalculator,
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

        let decision = evaluate_position_sizing(PositionSizingInputs {
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
            vec![SizedAdmissionReason::StaleNtState(
                SizingStateEvidenceKind::Portfolio
            )]
        );
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn sizer_rejects_when_min_remaining_pool_balance_would_be_breached() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut floor_policy = policy();
        floor_policy.min_remaining_pool_balance = Some(Decimal::new(96, 0));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
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
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(decision.liability_after_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn reject_only_mode_does_not_silently_clip_order_size() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut capped_policy = policy();
        capped_policy.max_order_liability = Some(Decimal::new(4, 0));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &capped_policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![SizedAdmissionReason::OverMaxOrderLiability]
        );
        assert_eq!(decision.original_quantity, Decimal::new(10, 0));
        assert_eq!(decision.sized_quantity, None);
        assert_eq!(decision.liability_before_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(decision.liability_after_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::ZERO);
    }

    #[test]
    fn explicit_clip_to_available_records_original_and_sized_quantity() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut clipping_policy = policy();
        clipping_policy.mode = SizingMode::ExplicitClipToAvailable;
        clipping_policy.max_order_liability = Some(Decimal::new(4, 0));
        let mut ledger = ReservationLedger::reconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
            request: &request(IntentSide::Buy, IntentLiquidity::Taker),
            state: Some(&state),
            policy: &clipping_policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
            reservation_ledger: &mut ledger,
        });

        assert!(decision.accepted);
        assert!(decision.reasons.is_empty());
        assert_eq!(decision.original_quantity, Decimal::new(10, 0));
        assert_eq!(decision.sized_quantity, Some(Decimal::new(925, 2)));
        assert_eq!(decision.liability_before_sizing, Some(Decimal::new(430, 2)));
        assert_eq!(decision.liability_after_sizing, Some(Decimal::new(4, 0)));
        assert_eq!(decision.evidence.original_quantity, Decimal::new(10, 0));
        assert_eq!(decision.evidence.sized_quantity, Some(Decimal::new(925, 2)));
        assert_eq!(ledger.live_reserved_liability("pool-1"), Decimal::new(4, 0));
    }

    #[test]
    fn restart_requires_rebuilt_open_order_reservations_before_admission() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let mut unreconciled_ledger = ReservationLedger::unreconciled();

        let decision = evaluate_position_sizing(PositionSizingInputs {
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
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(
            unreconciled_ledger.live_reserved_liability("pool-1"),
            Decimal::ZERO
        );
    }

    #[test]
    fn reserve_to_submit_is_single_serialized_critical_section() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let first_request = request_with_intent("intent-1");
        let second_request = request_with_intent("intent-2");
        let mut gate = PositionSizingAdmissionGate::reconciled();

        let first = gate.evaluate_and_reserve(PositionSizingGateInputs {
            request: &first_request,
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });
        let second = gate.evaluate_and_reserve(PositionSizingGateInputs {
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
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));

        let release = gate.release_pending_reservation("intent-1", "nt-submit-rejected");
        assert!(release.accepted);
        assert_eq!(release.released_liability, Some(Decimal::new(430, 2)));
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);

        let retry = gate.evaluate_and_reserve(PositionSizingGateInputs {
            request: &second_request,
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(retry.accepted);
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }

    #[test]
    fn admission_gate_fails_closed_until_reconciled_and_rejects_unknown_release() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let request = request_with_intent("intent-1");
        let mut gate = PositionSizingAdmissionGate::unreconciled();

        let decision = gate.evaluate_and_reserve(PositionSizingGateInputs {
            request: &request,
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool(),
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);

        let release = gate.release_pending_reservation("intent-1", "nt-submit-rejected");
        assert!(!release.accepted);
        assert_eq!(
            release.reason,
            Some(ReservationRejectionReason::UnknownRelease)
        );
        assert_eq!(release.released_liability, None);
    }

    #[test]
    fn restart_rebuilds_open_order_reservations_before_reopening_gate() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let open_reservation = rebuilt_open_order_reservation("intent-open");
        let mut gate = PositionSizingAdmissionGate::unreconciled();

        let rebuild =
            gate.rebuild_open_order_reservations(&capital_pool, &[open_reservation], 1_000, None);

        assert!(rebuild.accepted);
        assert_eq!(rebuild.rebuilt_reservation_count, 1);
        assert_eq!(rebuild.live_reserved_liability, Decimal::new(430, 2));

        let decision = gate.evaluate_and_reserve(PositionSizingGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::new(430, 2));
    }

    #[test]
    fn failed_restart_rebuild_keeps_gate_closed_without_partial_reservations() {
        let loss_snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::new(-5, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        let state = nt_state(Some(loss_snapshot));
        let policy = policy();
        let capital_pool = single_order_capital_pool();
        let invalid_reservation = invalid_rebuilt_open_order_reservation("intent-open");
        let mut gate = PositionSizingAdmissionGate::unreconciled();

        let rebuild = gate.rebuild_open_order_reservations(
            &capital_pool,
            &[invalid_reservation],
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

        let decision = gate.evaluate_and_reserve(PositionSizingGateInputs {
            request: &request_with_intent("intent-new"),
            state: Some(&state),
            policy: &policy,
            loss_policy: Some(&loss_policy()),
            capital_pool: &capital_pool,
        });

        assert!(!decision.accepted);
        assert_eq!(
            decision.reasons,
            vec![SizedAdmissionReason::Reservation(
                ReservationRejectionReason::ReconciliationRequired
            )]
        );
        assert_eq!(gate.live_reserved_liability("pool-1"), Decimal::ZERO);
    }
}
