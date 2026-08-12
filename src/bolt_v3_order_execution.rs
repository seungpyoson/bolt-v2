use std::{
    cell::{RefCell, RefMut},
    collections::BTreeSet,
    rc::Rc,
};

use anyhow::{Context, Result};
use nautilus_common::actor::DataActorNative;
use nautilus_common::factories::OrderFactory;
use nautilus_core::Params;
use nautilus_model::{
    enums::{
        OrderSide, OrderType, PositionSide as NtPositionSide, PositionSideSpecified, TimeInForce,
        TrailingOffsetType, TriggerType,
    },
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId, TradeId, Venue},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};
use nautilus_trading::{Strategy, StrategyNative};
use rust_decimal::{Decimal, RoundingStrategy, prelude::FromPrimitive};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::bolt_v3_economics_runtime::EconomicsAdmissionPurpose;
use crate::{
    bolt_v3_current_evidence::{
        EntryOrderIntentFact, EvidenceOrderSide, EvidenceOrderType, EvidenceTimeInForce,
        EvidenceTrailingOffsetType, EvidenceTriggerType, NonBlockingRecordOutcome,
        OrderExecutionEvidence, OrderIntentClampOutcome, OrderIntentDetails,
        OrderIntentOrderFields, PreparedOrderLinkage, RecordFailure,
        RiskReducingExitOrderIntentFact, SubmittedOrderLinkage,
    },
    bolt_v3_economics_runtime::EconomicsAdmission,
    bolt_v3_executable_cost::{ExecutableBookQuote, compile_bounded_risk_reducing_ioc},
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandFailure, MakerOrderCommandSink, MakerOrderDispatchInput,
        MakerOrderDispatchOutcome, dispatch_maker_order_command,
    },
    bolt_v3_position_authority_feed::{
        BoltV3CanonicalPositionAuthority, BoltV3PositionAuthorityCapability,
        BoltV3PositionAuthorityLease, BoltV3PositionAuthorityLeaseState,
        BoltV3SealedPositionAuthority,
    },
    bolt_v3_providers::normalize_base_order_quantity_for_execution_venue,
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderAdmissionEvidence, BoltV3CompiledOrderKind,
        BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide, BoltV3CompiledProductKind,
        BoltV3EconomicsSubmitAdmission, BoltV3RiskReducingExitPositionInput,
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest,
        BoltV3SubmitAdmissionRequestInput, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        OrderValuationContext, PredictionMarketOutcomeSide, order_admission_facts,
        validate_economics_remaining_margin_at, validate_economics_submit_authority,
    },
    integrations::nautilus::economics::economics_order_binding,
};

mod economics_basis;
mod tracked_order_economics;

pub use economics_basis::{
    BoltV3FinalOrderEconomicsScenario, BoltV3TerminalValueEntry, BoltV3TerminalValueEntryPolicy,
};
use tracked_order_economics::route_tracked_cancel_all;
pub use tracked_order_economics::{
    BoltV3CancellationLivenessFailure, BoltV3OrderEconomicsHandle, BoltV3RecoveryIdentityConflict,
    BoltV3RestingOrderCancelHealthSnapshot, BoltV3RestingRegistrationRejection,
    BoltV3RestingRegistrationRejectionKind, BoltV3RestingRollbackInvariantFailure,
    BoltV3RestingSubmitTransactionOutcome, BoltV3RoutedNonSubmittedOutcome,
    build_order_economics_submit_admission,
};

pub struct BoltV3FinalOrderEconomicsInput<'a> {
    pub execution_client_id: &'a str,
    pub intent: &'a OrderIntentDetails,
    pub order: &'a OrderAny,
    pub valuation: OrderValuationContext<'a>,
    pub risk_reducing_exit_position: Option<BoltV3RiskReducingExitPositionInput<'a>>,
    pub scenario: BoltV3FinalOrderEconomicsScenario,
    pub candidate_fill_levels: Vec<BoltV3PlannedFillLeg>,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}

pub struct BoltV3TakerEconomicsSizingInput<'a> {
    pub instrument_id: InstrumentId,
    pub order_side: OrderSide,
    pub planned_fill_legs: Vec<BoltV3PlannedFillLeg>,
    pub terminal_value_entry: BoltV3TerminalValueEntry,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BoltV3BoundedRiskReducingIoc {
    pub quantity: Quantity,
    pub worst_executable_price: Price,
    pub retained_fill_legs: Vec<BoltV3PlannedFillLeg>,
}

pub(crate) fn compile_bounded_risk_reducing_ioc_for_execution(
    execution_venue: Venue,
    instrument: &InstrumentAny,
    book: &ExecutableBookQuote<'_>,
    order_side: OrderSide,
    requested_quantity: Quantity,
    vwap_depth_limit_bps: u64,
) -> Result<BoltV3BoundedRiskReducingIoc> {
    let executable = compile_bounded_risk_reducing_ioc(
        book,
        order_side,
        requested_quantity.as_f64(),
        vwap_depth_limit_bps,
    )
    .map_err(|reason| anyhow::anyhow!("risk-reducing IOC fill plan is unavailable: {reason}"))?;
    let covered_quantity = Decimal::from_f64(executable.vwap_quantity)
        .ok_or_else(|| anyhow::anyhow!("executable quantity is not representable"))?;
    let venue_normalized =
        normalize_base_order_quantity_for_execution_venue(execution_venue, covered_quantity)
            .ok_or_else(|| anyhow::anyhow!("executable quantity is below venue precision"))?;
    let aligned = economics_basis::floor_to_size_increment(venue_normalized, instrument)?;
    anyhow::ensure!(
        aligned > Decimal::ZERO,
        "executable quantity aligns to zero"
    );
    if let Some(minimum) = instrument.min_quantity() {
        anyhow::ensure!(
            aligned >= minimum.as_decimal(),
            "executable quantity is below instrument minimum: quantity={aligned} minimum={minimum}"
        );
    }
    if let Some(maximum) = instrument.max_quantity() {
        anyhow::ensure!(
            aligned <= maximum.as_decimal(),
            "executable quantity exceeds instrument maximum: quantity={aligned} maximum={maximum}"
        );
    }
    let quantity = Quantity::from_decimal_dp(aligned, instrument.size_precision())
        .map_err(|error| anyhow::anyhow!(error))?;
    let quantity = instrument
        .try_normalize_qty(quantity)
        .map_err(|error| anyhow::anyhow!(error))?;

    let mut remaining = aligned;
    let mut retained_fill_legs = Vec::new();
    for level in executable.candidate_levels {
        if remaining.is_zero() {
            break;
        }
        let price = Decimal::from_f64(level.price)
            .ok_or_else(|| anyhow::anyhow!("executable fill price is not representable"))?;
        let available = Decimal::from_f64(level.quantity)
            .ok_or_else(|| anyhow::anyhow!("executable fill quantity is not representable"))?;
        anyhow::ensure!(
            price > Decimal::ZERO && available > Decimal::ZERO,
            "executable fill level must be positive"
        );
        let retained = remaining.min(available);
        retained_fill_legs.push(BoltV3PlannedFillLeg {
            price,
            quantity: retained,
        });
        remaining = remaining
            .checked_sub(retained)
            .ok_or_else(|| anyhow::anyhow!("executable fill subtraction overflow"))?;
    }
    anyhow::ensure!(
        remaining.is_zero(),
        "executable fill levels do not cover the aligned quantity"
    );
    let retained_sum = retained_fill_legs
        .iter()
        .try_fold(Decimal::ZERO, |sum, leg| sum.checked_add(leg.quantity))
        .ok_or_else(|| anyhow::anyhow!("executable fill quantity sum overflow"))?;
    anyhow::ensure!(
        retained_sum == aligned,
        "executable fill levels do not sum to the aligned quantity"
    );
    let worst_price = retained_fill_legs
        .last()
        .map(|leg| leg.price)
        .ok_or_else(|| anyhow::anyhow!("executable fill plan is empty"))?;
    let worst_executable_price = Price::from_decimal_dp(worst_price, instrument.price_precision())
        .map_err(|error| anyhow::anyhow!(error))?;

    Ok(BoltV3BoundedRiskReducingIoc {
        quantity,
        worst_executable_price,
        retained_fill_legs,
    })
}

pub(crate) struct BoltV3CompileAndSealRiskReducingIocInput<'a> {
    pub economics: &'a BoltV3OrderEconomicsHandle,
    pub execution_venue: Venue,
    pub execution_client_id: &'a str,
    pub instrument: &'a InstrumentAny,
    pub book: ExecutableBookQuote<'a>,
    pub vwap_depth_limit_bps: u64,
    pub intent: OrderIntentDetails,
    pub requested_order: OrderAny,
    pub position_id: PositionId,
    pub position_authority: &'a BoltV3PositionAuthorityCapability,
    pub position_side: NtPositionSide,
    pub prediction_market_outcome: PredictionMarketOutcomeSide,
    pub stored_entry_cost_per_unit: Decimal,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}

pub(crate) struct BoltV3CompiledRiskReducingIocSubmission {
    intent: OrderIntentDetails,
    order: OrderAny,
    sealed: BoltV3EconomicsSubmitAdmission,
    compiled: BoltV3BoundedRiskReducingIoc,
    position_authority: BoltV3SealedPositionAuthority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoltV3RiskReducingIocPreparationStage {
    OrderTemplate,
    PositionAuthority,
    ExecutableLiquidity,
    EconomicsSeal,
}

#[derive(Debug)]
pub(crate) struct BoltV3RiskReducingIocPreparationError {
    stage: BoltV3RiskReducingIocPreparationStage,
    source: anyhow::Error,
}

impl BoltV3RiskReducingIocPreparationError {
    #[must_use]
    pub(crate) const fn stage(&self) -> BoltV3RiskReducingIocPreparationStage {
        self.stage
    }
}

impl std::fmt::Display for BoltV3RiskReducingIocPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl std::error::Error for BoltV3RiskReducingIocPreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn risk_reducing_ioc_preparation_error(
    stage: BoltV3RiskReducingIocPreparationStage,
    source: impl Into<anyhow::Error>,
) -> BoltV3RiskReducingIocPreparationError {
    BoltV3RiskReducingIocPreparationError {
        stage,
        source: source.into(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3PositionReductionRelease {
    AwaitingAuthority,
    Residual { signed_quantity: Decimal },
    Flat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3ExitAuthorityRecoveryRelease {
    AwaitingAuthority,
    Flat,
}

#[derive(Clone)]
pub(crate) struct BoltV3ExitAuthorityRecoveryHandle {
    inner: Rc<BoltV3ExitAuthorityRecoveryState>,
}

struct BoltV3ExitAuthorityRecoveryState {
    lease: BoltV3PositionAuthorityLease,
    instrument_id: InstrumentId,
    position_id: PositionId,
    proof_floor_generation: u64,
}

impl std::fmt::Debug for BoltV3ExitAuthorityRecoveryHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoltV3ExitAuthorityRecoveryHandle")
            .field("authority_key", self.inner.lease.key())
            .field("proof_floor_generation", &self.inner.proof_floor_generation)
            .finish()
    }
}

impl PartialEq for BoltV3ExitAuthorityRecoveryHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl BoltV3ExitAuthorityRecoveryHandle {
    pub(crate) fn acquire(
        capability: &BoltV3PositionAuthorityCapability,
        instrument_id: InstrumentId,
        position_id: PositionId,
    ) -> Result<Self> {
        let lease = capability.acquire_for_position(position_id, instrument_id)?;
        let proof_floor_generation = lease.coherent_generation()?.unwrap_or(0);
        Ok(Self {
            inner: Rc::new(BoltV3ExitAuthorityRecoveryState {
                lease,
                instrument_id,
                position_id,
                proof_floor_generation,
            }),
        })
    }

    pub(crate) fn release_flat(
        &self,
        capability: &BoltV3PositionAuthorityCapability,
    ) -> Result<BoltV3ExitAuthorityRecoveryRelease> {
        let canonical =
            capability.canonical_position(self.inner.position_id, self.inner.instrument_id)?;
        self.release_flat_with_canonical(canonical.as_ref())
    }

    fn release_flat_with_canonical(
        &self,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<BoltV3ExitAuthorityRecoveryRelease> {
        let Some(canonical) = canonical else {
            return Ok(BoltV3ExitAuthorityRecoveryRelease::AwaitingAuthority);
        };
        if !canonical.is_exact_target()
            || !canonical.signed_quantity().is_zero()
            || canonical.side() != PositionSideSpecified::Flat
        {
            return Ok(BoltV3ExitAuthorityRecoveryRelease::AwaitingAuthority);
        }
        let observation = self.inner.lease.observation()?;
        if let Some(stale) = observation.stale_health {
            anyhow::bail!("exit recovery position authority is stale: {stale:?}");
        }
        let snapshot = match observation.state {
            BoltV3PositionAuthorityLeaseState::Awaiting => {
                return Ok(BoltV3ExitAuthorityRecoveryRelease::AwaitingAuthority);
            }
            BoltV3PositionAuthorityLeaseState::Conflicted(conflict) => {
                anyhow::bail!("exit recovery position authority conflicts: {conflict:?}")
            }
            BoltV3PositionAuthorityLeaseState::Coherent(snapshot) => snapshot,
        };
        if snapshot.generation <= self.inner.proof_floor_generation
            || !snapshot.signed_quantity.is_zero()
            || snapshot.position_side != PositionSideSpecified::Flat
        {
            return Ok(BoltV3ExitAuthorityRecoveryRelease::AwaitingAuthority);
        }
        Ok(BoltV3ExitAuthorityRecoveryRelease::Flat)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3ExitOrderLifecycleReduction {
    Working,
    TerminalZeroFill,
    TerminalAwaitingPosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3ExitOrderCorrection {
    Unchanged,
    FillAuthorityChanged,
}

#[derive(Clone)]
pub(crate) struct BoltV3PositionReductionFence {
    baseline_signed_quantity: Decimal,
    baseline_side: PositionSideSpecified,
    effective_filled_quantity: Decimal,
    required_trade_ids: BTreeSet<TradeId>,
    fill_set_proof: BoltV3FillSetProof,
    latest_terminal_or_correction_ns: u64,
    proof_floor_generation: u64,
}

struct BoltV3ExitOrderAuthorityState {
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    position_id: PositionId,
    latest_effective_filled_quantity: Decimal,
    latest_fill_ids: BTreeSet<TradeId>,
    lease: BoltV3PositionAuthorityLease,
    progress: BoltV3ExitOrderAuthorityProgress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BoltV3RecoveredExitBaseline {
    AwaitingAuthoritativeBaseline,
    Coherent {
        report_generation: u64,
        signed_quantity: Decimal,
        side: PositionSideSpecified,
        cumulative_order_fills: Decimal,
        order_fill_ids: BTreeSet<TradeId>,
    },
}

enum BoltV3ExitOrderAuthorityProgress {
    Working,
    WorkingFenced(BoltV3PositionReductionFence),
    TerminalFenced(BoltV3PositionReductionFence),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoltV3FillSetProof {
    Eligible,
    RequiresPostEventReport,
}

enum BoltV3ExitOrderAuthority {
    LocallySubmitted {
        state: BoltV3ExitOrderAuthorityState,
        baseline_signed_quantity: Decimal,
        baseline_side: PositionSideSpecified,
        compiled_quantity: Quantity,
    },
    Recovered {
        state: BoltV3ExitOrderAuthorityState,
        cause: BoltV3RecoveredExitCause,
        adopted_signed_ceiling: Decimal,
        adopted_side: PositionSideSpecified,
        adopted_order_quantity: Quantity,
        baseline: BoltV3RecoveredExitBaseline,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3RecoveredExitCause {
    StartupAdoption,
    FillVoidReopen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3ExitOrderAuthorityOrigin {
    LocallySubmitted,
    Recovered { cause: BoltV3RecoveredExitCause },
}

#[derive(Clone)]
pub(crate) struct BoltV3ExitOrderAuthorityHandle {
    inner: Rc<RefCell<BoltV3ExitOrderAuthority>>,
}

impl std::fmt::Debug for BoltV3ExitOrderAuthorityHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let authority = self.inner.borrow();
        let state = authority.state();
        formatter
            .debug_struct("BoltV3ExitOrderAuthorityHandle")
            .field("client_order_id", &state.client_order_id)
            .field("instrument_id", &state.instrument_id)
            .field("position_id", &state.position_id)
            .field("origin", &authority.origin())
            .field(
                "terminal",
                &matches!(
                    state.progress,
                    BoltV3ExitOrderAuthorityProgress::TerminalFenced(_)
                ),
            )
            .finish()
    }
}

impl PartialEq for BoltV3ExitOrderAuthorityHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl BoltV3ExitOrderAuthorityHandle {
    pub(crate) fn instrument_id(&self) -> InstrumentId {
        self.inner.borrow().state().instrument_id
    }

    pub(crate) fn position_id(&self) -> PositionId {
        self.inner.borrow().state().position_id
    }

    pub(crate) fn locally_submitted(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
        compiled_quantity: Quantity,
        position_authority: BoltV3SealedPositionAuthority,
    ) -> Result<Self> {
        let (canonical, lease) = position_authority.into_parts();
        anyhow::ensure!(
            canonical.is_exact_target(),
            "local exit authority requires exact canonical position scope"
        );
        Self::locally_submitted_from_parts(
            client_order_id,
            instrument_id,
            position_id,
            canonical.signed_quantity(),
            canonical.side(),
            compiled_quantity,
            lease,
        )
    }

    #[cfg(test)]
    pub(crate) fn locally_submitted_for_test(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
        baseline_signed_quantity: Decimal,
        baseline_side: PositionSideSpecified,
        compiled_quantity: Quantity,
        lease: BoltV3PositionAuthorityLease,
    ) -> Result<Self> {
        Self::locally_submitted_from_parts(
            client_order_id,
            instrument_id,
            position_id,
            baseline_signed_quantity,
            baseline_side,
            compiled_quantity,
            lease,
        )
    }

    fn locally_submitted_from_parts(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
        baseline_signed_quantity: Decimal,
        baseline_side: PositionSideSpecified,
        compiled_quantity: Quantity,
        lease: BoltV3PositionAuthorityLease,
    ) -> Result<Self> {
        anyhow::ensure!(
            matches!(
                baseline_side,
                PositionSideSpecified::Long | PositionSideSpecified::Short
            ),
            "local exit authority requires a non-flat baseline side"
        );
        anyhow::ensure!(
            compiled_quantity.as_decimal() > Decimal::ZERO,
            "local exit authority requires positive compiled quantity"
        );
        Ok(Self {
            inner: Rc::new(RefCell::new(BoltV3ExitOrderAuthority::LocallySubmitted {
                state: BoltV3ExitOrderAuthorityState {
                    client_order_id,
                    instrument_id,
                    position_id,
                    latest_effective_filled_quantity: Decimal::ZERO,
                    latest_fill_ids: BTreeSet::new(),
                    lease,
                    progress: BoltV3ExitOrderAuthorityProgress::Working,
                },
                baseline_signed_quantity,
                baseline_side,
                compiled_quantity,
            })),
        })
    }

    pub(crate) fn observe_order(
        &self,
        order: &OrderAny,
        latest_terminal_or_correction_ns: u64,
        correction: BoltV3ExitOrderCorrection,
    ) -> Result<BoltV3ExitOrderLifecycleReduction> {
        let mut authority = self.inner.borrow_mut();
        let quantity_ceiling = authority.quantity_ceiling();
        let fence_basis = authority.fence_basis();
        let state = authority.state_mut();
        anyhow::ensure!(
            order.client_order_id() == state.client_order_id
                && order.instrument_id() == state.instrument_id,
            "exit authority order identity mismatch"
        );
        if let Some(order_position_id) = order.position_id() {
            anyhow::ensure!(
                order_position_id == state.position_id,
                "exit authority position identity mismatch"
            );
        }
        anyhow::ensure!(
            order.quantity() == quantity_ceiling,
            "exit authority order quantity changed after authority construction"
        );
        let effective_filled_quantity = order.filled_qty().as_decimal();
        anyhow::ensure!(
            effective_filled_quantity <= quantity_ceiling.as_decimal(),
            "exit authority cumulative fills exceed the authorized quantity ceiling"
        );
        let required_trade_ids = order
            .trade_ids()
            .into_iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let order_authority_changed = effective_filled_quantity
            != state.latest_effective_filled_quantity
            || required_trade_ids != state.latest_fill_ids;

        match &state.progress {
            BoltV3ExitOrderAuthorityProgress::Working => {}
            BoltV3ExitOrderAuthorityProgress::WorkingFenced(fence)
            | BoltV3ExitOrderAuthorityProgress::TerminalFenced(fence) => {
                let mut next_fence = fence.clone();
                let (fence_filled_quantity, fence_trade_ids) = fence_basis
                    .reduction_authority(effective_filled_quantity, &required_trade_ids)?;
                if correction == BoltV3ExitOrderCorrection::FillAuthorityChanged
                    || order_authority_changed
                    || order.is_closed()
                {
                    next_fence.observe_terminal_or_correction(
                        &state.lease,
                        fence_filled_quantity,
                        fence_trade_ids,
                        correction,
                        latest_terminal_or_correction_ns,
                    )?;
                }
                state.latest_effective_filled_quantity = effective_filled_quantity;
                state.latest_fill_ids = required_trade_ids;
                state.progress = if order.is_closed() {
                    BoltV3ExitOrderAuthorityProgress::TerminalFenced(next_fence)
                } else {
                    BoltV3ExitOrderAuthorityProgress::WorkingFenced(next_fence)
                };
                return Ok(if order.is_closed() {
                    BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
                } else {
                    BoltV3ExitOrderLifecycleReduction::Working
                });
            }
        }
        if !order.is_closed() {
            state.latest_effective_filled_quantity = effective_filled_quantity;
            state.latest_fill_ids = required_trade_ids;
            return Ok(BoltV3ExitOrderLifecycleReduction::Working);
        }
        if matches!(&fence_basis, BoltV3ExitFenceBasis::Local { .. })
            && effective_filled_quantity.is_zero()
            && correction == BoltV3ExitOrderCorrection::Unchanged
        {
            state.latest_effective_filled_quantity = effective_filled_quantity;
            state.latest_fill_ids = required_trade_ids;
            return Ok(BoltV3ExitOrderLifecycleReduction::TerminalZeroFill);
        }
        let (
            baseline_signed_quantity,
            baseline_side,
            fill_set_proof,
            minimum_proof_floor_generation,
        ) = match &fence_basis {
            BoltV3ExitFenceBasis::Local {
                baseline_signed_quantity,
                baseline_side,
            } => (
                *baseline_signed_quantity,
                *baseline_side,
                match correction {
                    BoltV3ExitOrderCorrection::Unchanged => BoltV3FillSetProof::Eligible,
                    BoltV3ExitOrderCorrection::FillAuthorityChanged => {
                        BoltV3FillSetProof::RequiresPostEventReport
                    }
                },
                0,
            ),
            BoltV3ExitFenceBasis::RecoveredAwaiting {
                adopted_signed_ceiling,
                adopted_side,
            } => (
                *adopted_signed_ceiling,
                *adopted_side,
                BoltV3FillSetProof::RequiresPostEventReport,
                0,
            ),
            BoltV3ExitFenceBasis::RecoveredCoherent {
                report_generation,
                signed_quantity,
                side,
                ..
            } => (
                *signed_quantity,
                *side,
                match correction {
                    BoltV3ExitOrderCorrection::Unchanged => BoltV3FillSetProof::Eligible,
                    BoltV3ExitOrderCorrection::FillAuthorityChanged => {
                        BoltV3FillSetProof::RequiresPostEventReport
                    }
                },
                *report_generation,
            ),
        };
        let (fill_delta, required_trade_ids) =
            fence_basis.reduction_authority(effective_filled_quantity, &required_trade_ids)?;
        let fence = BoltV3PositionReductionFence::local(
            &state.lease,
            baseline_signed_quantity,
            baseline_side,
            fill_delta,
            required_trade_ids,
            fill_set_proof,
            latest_terminal_or_correction_ns,
            minimum_proof_floor_generation,
        )?;
        state.latest_effective_filled_quantity = effective_filled_quantity;
        state.latest_fill_ids = order
            .trade_ids()
            .into_iter()
            .copied()
            .collect::<BTreeSet<_>>();
        state.progress = BoltV3ExitOrderAuthorityProgress::TerminalFenced(fence);
        Ok(BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition)
    }

    pub(crate) fn refresh_recovered_baseline(
        &self,
        capability: &BoltV3PositionAuthorityCapability,
    ) -> Result<()> {
        let authority = self.inner.borrow();
        let state = authority.state();
        let canonical = capability.canonical_position(state.position_id, state.instrument_id)?;
        drop(authority);
        self.refresh_recovered_baseline_with_canonical(canonical.as_ref())
    }

    #[cfg(test)]
    pub(crate) fn refresh_recovered_baseline_with_canonical_for_test(
        &self,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<()> {
        self.refresh_recovered_baseline_with_canonical(canonical)
    }

    fn refresh_recovered_baseline_with_canonical(
        &self,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<()> {
        self.inner
            .borrow_mut()
            .establish_recovered_baseline(canonical)
    }

    pub(crate) fn release(
        &self,
        capability: &BoltV3PositionAuthorityCapability,
    ) -> Result<BoltV3PositionReductionRelease> {
        let authority = self.inner.borrow();
        let state = authority.state();
        let canonical = capability.canonical_position(state.position_id, state.instrument_id)?;
        drop(authority);
        self.release_with_canonical(canonical.as_ref())
    }

    #[cfg(test)]
    pub(crate) fn release_with_canonical_for_test(
        &self,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<BoltV3PositionReductionRelease> {
        self.release_with_canonical(canonical)
    }

    fn release_with_canonical(
        &self,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<BoltV3PositionReductionRelease> {
        let authority = self.inner.borrow();
        let state = authority.state();
        let BoltV3ExitOrderAuthorityProgress::TerminalFenced(fence) = &state.progress else {
            return Ok(BoltV3PositionReductionRelease::AwaitingAuthority);
        };
        fence.release(&state.lease, canonical)
    }

    pub(crate) fn recovered(
        cause: BoltV3RecoveredExitCause,
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
        order: &OrderAny,
        position_authority: BoltV3SealedPositionAuthority,
    ) -> Result<Self> {
        let (canonical, lease) = position_authority.into_parts();
        Self::recovered_from_parts(
            cause,
            client_order_id,
            instrument_id,
            position_id,
            canonical.signed_quantity(),
            canonical.side(),
            order,
            lease,
        )
    }

    #[cfg(test)]
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn recovered_for_test(
        cause: BoltV3RecoveredExitCause,
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
        adopted_signed_ceiling: Decimal,
        adopted_side: PositionSideSpecified,
        order: &OrderAny,
        lease: BoltV3PositionAuthorityLease,
    ) -> Result<Self> {
        Self::recovered_from_parts(
            cause,
            client_order_id,
            instrument_id,
            position_id,
            adopted_signed_ceiling,
            adopted_side,
            order,
            lease,
        )
    }

    #[expect(clippy::too_many_arguments)]
    fn recovered_from_parts(
        cause: BoltV3RecoveredExitCause,
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
        adopted_signed_ceiling: Decimal,
        adopted_side: PositionSideSpecified,
        order: &OrderAny,
        lease: BoltV3PositionAuthorityLease,
    ) -> Result<Self> {
        anyhow::ensure!(
            order.client_order_id() == client_order_id && order.instrument_id() == instrument_id,
            "recovered exit authority order identity mismatch"
        );
        anyhow::ensure!(
            order.quantity().as_decimal() > Decimal::ZERO,
            "recovered exit authority requires positive order quantity"
        );
        Ok(Self {
            inner: Rc::new(RefCell::new(BoltV3ExitOrderAuthority::Recovered {
                state: BoltV3ExitOrderAuthorityState {
                    client_order_id,
                    instrument_id,
                    position_id,
                    latest_effective_filled_quantity: order.filled_qty().as_decimal(),
                    latest_fill_ids: order.trade_ids().into_iter().copied().collect(),
                    lease,
                    progress: BoltV3ExitOrderAuthorityProgress::Working,
                },
                cause,
                adopted_signed_ceiling,
                adopted_side,
                adopted_order_quantity: order.quantity(),
                baseline: BoltV3RecoveredExitBaseline::AwaitingAuthoritativeBaseline,
            })),
        })
    }
}

#[derive(Clone)]
enum BoltV3ExitFenceBasis {
    Local {
        baseline_signed_quantity: Decimal,
        baseline_side: PositionSideSpecified,
    },
    RecoveredAwaiting {
        adopted_signed_ceiling: Decimal,
        adopted_side: PositionSideSpecified,
    },
    RecoveredCoherent {
        report_generation: u64,
        signed_quantity: Decimal,
        side: PositionSideSpecified,
        cumulative_order_fills: Decimal,
        order_fill_ids: BTreeSet<TradeId>,
    },
}

impl BoltV3ExitFenceBasis {
    fn reduction_authority(
        &self,
        cumulative_filled_quantity: Decimal,
        cumulative_trade_ids: &BTreeSet<TradeId>,
    ) -> Result<(Decimal, BTreeSet<TradeId>)> {
        match self {
            Self::Local { .. } => Ok((cumulative_filled_quantity, cumulative_trade_ids.clone())),
            Self::RecoveredAwaiting { .. } => Ok((Decimal::ZERO, BTreeSet::new())),
            Self::RecoveredCoherent {
                cumulative_order_fills,
                order_fill_ids,
                ..
            } => Ok((
                cumulative_filled_quantity
                    .checked_sub(*cumulative_order_fills)
                    .context("recovered exit cumulative fill regressed")?,
                cumulative_trade_ids
                    .difference(order_fill_ids)
                    .copied()
                    .collect(),
            )),
        }
    }
}

impl BoltV3ExitOrderAuthority {
    fn state(&self) -> &BoltV3ExitOrderAuthorityState {
        match self {
            Self::LocallySubmitted { state, .. } | Self::Recovered { state, .. } => state,
        }
    }

    fn state_mut(&mut self) -> &mut BoltV3ExitOrderAuthorityState {
        match self {
            Self::LocallySubmitted { state, .. } | Self::Recovered { state, .. } => state,
        }
    }

    fn origin(&self) -> BoltV3ExitOrderAuthorityOrigin {
        match self {
            Self::LocallySubmitted { .. } => BoltV3ExitOrderAuthorityOrigin::LocallySubmitted,
            Self::Recovered { cause, .. } => {
                BoltV3ExitOrderAuthorityOrigin::Recovered { cause: *cause }
            }
        }
    }

    fn quantity_ceiling(&self) -> Quantity {
        match self {
            Self::LocallySubmitted {
                compiled_quantity, ..
            } => *compiled_quantity,
            Self::Recovered {
                adopted_order_quantity,
                ..
            } => *adopted_order_quantity,
        }
    }

    fn fence_basis(&self) -> BoltV3ExitFenceBasis {
        match self {
            Self::LocallySubmitted {
                baseline_signed_quantity,
                baseline_side,
                ..
            } => BoltV3ExitFenceBasis::Local {
                baseline_signed_quantity: *baseline_signed_quantity,
                baseline_side: *baseline_side,
            },
            Self::Recovered {
                adopted_signed_ceiling,
                adopted_side,
                baseline: BoltV3RecoveredExitBaseline::AwaitingAuthoritativeBaseline,
                ..
            } => BoltV3ExitFenceBasis::RecoveredAwaiting {
                adopted_signed_ceiling: *adopted_signed_ceiling,
                adopted_side: *adopted_side,
            },
            Self::Recovered {
                baseline:
                    BoltV3RecoveredExitBaseline::Coherent {
                        report_generation,
                        signed_quantity,
                        side,
                        cumulative_order_fills,
                        order_fill_ids,
                    },
                ..
            } => BoltV3ExitFenceBasis::RecoveredCoherent {
                report_generation: *report_generation,
                signed_quantity: *signed_quantity,
                side: *side,
                cumulative_order_fills: *cumulative_order_fills,
                order_fill_ids: order_fill_ids.clone(),
            },
        }
    }

    fn establish_recovered_baseline(
        &mut self,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<()> {
        let Self::Recovered {
            state,
            adopted_signed_ceiling,
            adopted_side,
            baseline,
            ..
        } = self
        else {
            return Ok(());
        };
        if !matches!(
            baseline,
            BoltV3RecoveredExitBaseline::AwaitingAuthoritativeBaseline
        ) || !matches!(state.progress, BoltV3ExitOrderAuthorityProgress::Working)
        {
            return Ok(());
        }
        let Some(canonical) = canonical else {
            return Ok(());
        };
        if !canonical.is_exact_target() {
            return Ok(());
        }
        let observation = state.lease.observation()?;
        if let Some(stale) = observation.stale_health {
            anyhow::bail!("recovered exit position authority is stale: {stale:?}");
        }
        let snapshot = match observation.state {
            BoltV3PositionAuthorityLeaseState::Awaiting => return Ok(()),
            BoltV3PositionAuthorityLeaseState::Conflicted(conflict) => {
                anyhow::bail!("recovered exit position authority conflicts: {conflict:?}")
            }
            BoltV3PositionAuthorityLeaseState::Coherent(snapshot) => snapshot,
        };
        if snapshot.signed_quantity != canonical.signed_quantity()
            || snapshot.position_side != canonical.side()
            || !position_is_within_adopted_ceiling(
                *adopted_signed_ceiling,
                *adopted_side,
                canonical.signed_quantity(),
                canonical.side(),
            )
        {
            return Ok(());
        }
        *baseline = BoltV3RecoveredExitBaseline::Coherent {
            report_generation: snapshot.generation,
            signed_quantity: canonical.signed_quantity(),
            side: canonical.side(),
            cumulative_order_fills: state.latest_effective_filled_quantity,
            order_fill_ids: state.latest_fill_ids.clone(),
        };
        Ok(())
    }
}

impl BoltV3PositionReductionFence {
    fn observe_terminal_or_correction(
        &mut self,
        lease: &BoltV3PositionAuthorityLease,
        effective_filled_quantity: Decimal,
        required_trade_ids: BTreeSet<TradeId>,
        correction: BoltV3ExitOrderCorrection,
        latest_terminal_or_correction_ns: u64,
    ) -> Result<()> {
        anyhow::ensure!(
            effective_filled_quantity >= Decimal::ZERO,
            "position reduction fill quantity must be non-negative"
        );
        let advances_authority = effective_filled_quantity != self.effective_filled_quantity
            || required_trade_ids != self.required_trade_ids
            || (correction == BoltV3ExitOrderCorrection::FillAuthorityChanged
                && self.fill_set_proof == BoltV3FillSetProof::Eligible)
            || latest_terminal_or_correction_ns > self.latest_terminal_or_correction_ns;
        if !advances_authority {
            return Ok(());
        }
        anyhow::ensure!(
            latest_terminal_or_correction_ns >= self.latest_terminal_or_correction_ns,
            "position reduction authority observation regressed in event time"
        );
        self.effective_filled_quantity = effective_filled_quantity;
        self.required_trade_ids = required_trade_ids;
        if correction == BoltV3ExitOrderCorrection::FillAuthorityChanged {
            self.fill_set_proof = BoltV3FillSetProof::RequiresPostEventReport;
        }
        self.latest_terminal_or_correction_ns = latest_terminal_or_correction_ns;
        self.proof_floor_generation = lease.coherent_generation()?.unwrap_or(0);
        Ok(())
    }
}

impl BoltV3PositionReductionFence {
    #[expect(clippy::too_many_arguments)]
    fn local(
        lease: &BoltV3PositionAuthorityLease,
        baseline_signed_quantity: Decimal,
        baseline_side: PositionSideSpecified,
        effective_filled_quantity: Decimal,
        required_trade_ids: BTreeSet<TradeId>,
        fill_set_proof: BoltV3FillSetProof,
        latest_terminal_or_correction_ns: u64,
        minimum_proof_floor_generation: u64,
    ) -> Result<Self> {
        anyhow::ensure!(
            effective_filled_quantity >= Decimal::ZERO,
            "position reduction effective fill quantity must be non-negative"
        );
        let proof_floor_generation = lease
            .coherent_generation()?
            .unwrap_or(0)
            .max(minimum_proof_floor_generation);
        Ok(Self {
            baseline_signed_quantity,
            baseline_side,
            effective_filled_quantity,
            required_trade_ids,
            fill_set_proof,
            latest_terminal_or_correction_ns,
            proof_floor_generation,
        })
    }

    pub(crate) fn release(
        &self,
        lease: &BoltV3PositionAuthorityLease,
        canonical: Option<&BoltV3CanonicalPositionAuthority>,
    ) -> Result<BoltV3PositionReductionRelease> {
        let Some(canonical) = canonical else {
            return Ok(BoltV3PositionReductionRelease::AwaitingAuthority);
        };
        if !canonical.is_exact_target() {
            return Ok(BoltV3PositionReductionRelease::AwaitingAuthority);
        }
        if !position_is_within_reduction_bound(
            self.baseline_signed_quantity,
            self.baseline_side,
            self.effective_filled_quantity,
            canonical.signed_quantity(),
            canonical.side(),
        )? {
            return Ok(BoltV3PositionReductionRelease::AwaitingAuthority);
        }

        let complete_fill_application = self.fill_set_proof == BoltV3FillSetProof::Eligible
            && (self.effective_filled_quantity.is_zero() || !self.required_trade_ids.is_empty())
            && self
                .required_trade_ids
                .iter()
                .all(|trade_id| canonical.trade_ids().contains(trade_id));
        let observation = lease.observation()?;
        if let Some(stale) = observation.stale_health {
            anyhow::bail!("position reduction authority is stale: {stale:?}");
        }
        let post_event_report = match observation.state {
            BoltV3PositionAuthorityLeaseState::Coherent(snapshot) => {
                snapshot.generation > self.proof_floor_generation
                    && snapshot.ts_last.as_u64() >= self.latest_terminal_or_correction_ns
                    && snapshot.signed_quantity == canonical.signed_quantity()
                    && snapshot.position_side == canonical.side()
            }
            BoltV3PositionAuthorityLeaseState::Awaiting => false,
            BoltV3PositionAuthorityLeaseState::Conflicted(conflict) => {
                anyhow::bail!("position reduction authority conflicts: {conflict:?}")
            }
        };
        if !complete_fill_application && !post_event_report {
            return Ok(BoltV3PositionReductionRelease::AwaitingAuthority);
        }
        if canonical.signed_quantity().is_zero() {
            Ok(BoltV3PositionReductionRelease::Flat)
        } else {
            Ok(BoltV3PositionReductionRelease::Residual {
                signed_quantity: canonical.signed_quantity(),
            })
        }
    }
}

fn position_is_within_adopted_ceiling(
    adopted_signed_quantity: Decimal,
    adopted_side: PositionSideSpecified,
    observed_signed_quantity: Decimal,
    observed_side: PositionSideSpecified,
) -> bool {
    match adopted_side {
        PositionSideSpecified::Long => {
            matches!(
                observed_side,
                PositionSideSpecified::Long | PositionSideSpecified::Flat
            ) && observed_signed_quantity >= Decimal::ZERO
                && observed_signed_quantity <= adopted_signed_quantity
        }
        PositionSideSpecified::Short => {
            matches!(
                observed_side,
                PositionSideSpecified::Short | PositionSideSpecified::Flat
            ) && observed_signed_quantity <= Decimal::ZERO
                && observed_signed_quantity >= adopted_signed_quantity
        }
        PositionSideSpecified::Flat => {
            observed_side == PositionSideSpecified::Flat && observed_signed_quantity.is_zero()
        }
    }
}

fn position_is_within_reduction_bound(
    baseline_signed_quantity: Decimal,
    baseline_side: PositionSideSpecified,
    effective_filled_quantity: Decimal,
    observed_signed_quantity: Decimal,
    observed_side: PositionSideSpecified,
) -> Result<bool> {
    let valid = match baseline_side {
        PositionSideSpecified::Long => {
            let maximum_residual = baseline_signed_quantity
                .checked_sub(effective_filled_quantity)
                .ok_or_else(|| anyhow::anyhow!("long position reduction bound overflow"))?;
            maximum_residual >= Decimal::ZERO
                && matches!(
                    observed_side,
                    PositionSideSpecified::Long | PositionSideSpecified::Flat
                )
                && observed_signed_quantity >= Decimal::ZERO
                && observed_signed_quantity <= maximum_residual
        }
        PositionSideSpecified::Short => {
            let minimum_residual = baseline_signed_quantity
                .checked_add(effective_filled_quantity)
                .ok_or_else(|| anyhow::anyhow!("short position reduction bound overflow"))?;
            minimum_residual <= Decimal::ZERO
                && matches!(
                    observed_side,
                    PositionSideSpecified::Short | PositionSideSpecified::Flat
                )
                && observed_signed_quantity <= Decimal::ZERO
                && observed_signed_quantity >= minimum_residual
        }
        PositionSideSpecified::Flat => {
            baseline_signed_quantity.is_zero()
                && effective_filled_quantity.is_zero()
                && observed_signed_quantity.is_zero()
                && observed_side == PositionSideSpecified::Flat
        }
    };
    Ok(valid)
}

impl BoltV3CompiledRiskReducingIocSubmission {
    pub(crate) fn into_parts(
        self,
    ) -> (
        OrderIntentDetails,
        OrderAny,
        BoltV3EconomicsSubmitAdmission,
        BoltV3BoundedRiskReducingIoc,
        BoltV3SealedPositionAuthority,
    ) {
        (
            self.intent,
            self.order,
            self.sealed,
            self.compiled,
            self.position_authority,
        )
    }
}

pub(crate) fn compile_and_seal_risk_reducing_ioc(
    input: BoltV3CompileAndSealRiskReducingIocInput<'_>,
) -> std::result::Result<
    BoltV3CompiledRiskReducingIocSubmission,
    BoltV3RiskReducingIocPreparationError,
> {
    let BoltV3CompileAndSealRiskReducingIocInput {
        economics,
        execution_venue,
        execution_client_id,
        instrument,
        book,
        vwap_depth_limit_bps,
        intent,
        requested_order,
        position_id,
        position_authority,
        position_side,
        prediction_market_outcome,
        stored_entry_cost_per_unit,
        requested_at_ns,
        decision_correlation_id,
    } = input;
    ensure_supported_risk_reducing_ioc_template(&requested_order).map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::OrderTemplate,
            error,
        )
    })?;
    let sealed_position_authority = position_authority
        .acquire_canonical_position(position_id, requested_order.instrument_id())
        .map_err(|error| {
            risk_reducing_ioc_preparation_error(
                BoltV3RiskReducingIocPreparationStage::PositionAuthority,
                error,
            )
        })?;
    let canonical_quantity_at_compile = require_canonical_exit_position_snapshot(
        sealed_position_authority.canonical(),
        position_id,
        requested_order.instrument_id(),
        position_side,
    )
    .map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::PositionAuthority,
            error,
        )
    })?;
    let (clamped_intent, mut final_order) = clamp_risk_reducing_exit_to_position_quantity(
        intent,
        requested_order,
        canonical_quantity_at_compile,
    )
    .map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::PositionAuthority,
            error,
        )
    })?;
    let compiled = compile_bounded_risk_reducing_ioc_for_execution(
        execution_venue,
        instrument,
        &book,
        final_order.order_side(),
        final_order.quantity(),
        vwap_depth_limit_bps,
    )
    .map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::ExecutableLiquidity,
            error,
        )
    })?;
    final_order.set_quantity(compiled.quantity);
    final_order.set_leaves_qty(compiled.quantity);

    let canonical_position = require_canonical_exit_position_at_seal(
        position_authority,
        sealed_position_authority.canonical(),
        position_id,
        final_order.instrument_id(),
        position_side,
        canonical_quantity_at_compile,
        final_order.quantity(),
    )
    .map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::PositionAuthority,
            error,
        )
    })?;
    let mut final_intent = order_intent_details_from_compiled_order(
        clamped_intent.strategy_id,
        compiled.worst_executable_price.to_string(),
        &final_order,
    );
    final_intent.clamp_outcome = clamped_intent.clamp_outcome;

    let position = economics
        .planned_exit_position(
            position_id,
            position_side,
            final_order.quantity().as_decimal(),
        )
        .map_err(|error| {
            risk_reducing_ioc_preparation_error(
                BoltV3RiskReducingIocPreparationStage::EconomicsSeal,
                error,
            )
        })?;
    let scenario = BoltV3FinalOrderEconomicsScenario::planned_risk_reducing_exit(
        stored_entry_cost_per_unit,
        position,
    )
    .map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::EconomicsSeal,
            error,
        )
    })?;
    let position_id_string = position_id.to_string();
    let instrument_id_string = final_order.instrument_id().to_string();
    let risk_reducing_exit_position = BoltV3RiskReducingExitPositionInput {
        position_id: position_id_string.as_str(),
        instrument_id: instrument_id_string.as_str(),
        position_side,
        position_quantity: canonical_position,
    };
    let sealed = build_order_economics_submit_admission(
        economics,
        BoltV3FinalOrderEconomicsInput {
            execution_client_id,
            intent: &final_intent,
            order: &final_order,
            valuation: OrderValuationContext {
                last_quote: None,
                last_trade: None,
                instrument: Some(instrument),
            },
            risk_reducing_exit_position: Some(risk_reducing_exit_position),
            scenario,
            candidate_fill_levels: compiled.retained_fill_legs.clone(),
            requested_at_ns,
            decision_correlation_id,
        },
    )
    .map_err(|error| {
        risk_reducing_ioc_preparation_error(
            BoltV3RiskReducingIocPreparationStage::EconomicsSeal,
            error,
        )
    })?;
    let side = match final_order.order_side() {
        OrderSide::Buy => BoltV3CompiledOrderSide::Buy,
        OrderSide::Sell => BoltV3CompiledOrderSide::Sell,
        OrderSide::NoOrderSide => {
            return Err(risk_reducing_ioc_preparation_error(
                BoltV3RiskReducingIocPreparationStage::EconomicsSeal,
                anyhow::anyhow!("risk-reducing IOC admission evidence requires a sided order"),
            ));
        }
    };
    let sealed = sealed
        .with_compiled_order_admission_evidence(BoltV3CompiledOrderAdmissionEvidence {
            venue_id: execution_venue.to_string(),
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side,
            quantity: final_order.quantity().as_decimal(),
            effective_price: compiled.worst_executable_price.as_decimal(),
            order_kind: BoltV3CompiledOrderKind::Market,
            liquidity: BoltV3CompiledOrderLiquidity::Taker,
            quote_set_id: None,
            prediction_market_outcome: Some(prediction_market_outcome),
        })
        .map_err(|error| {
            risk_reducing_ioc_preparation_error(
                BoltV3RiskReducingIocPreparationStage::EconomicsSeal,
                error,
            )
        })?;

    Ok(BoltV3CompiledRiskReducingIocSubmission {
        intent: final_intent,
        order: final_order,
        sealed,
        compiled,
        position_authority: sealed_position_authority,
    })
}

fn ensure_supported_risk_reducing_ioc_template(order: &OrderAny) -> Result<()> {
    anyhow::ensure!(
        order.order_type() == OrderType::Market
            && order.time_in_force() == TimeInForce::Ioc
            && !order.is_quote_quantity()
            && !order.is_post_only()
            && order.trigger_price().is_none()
            && order.activation_price().is_none()
            && order.trigger_type().is_none()
            && order.trigger_instrument_id().is_none()
            && order.trailing_offset().is_none()
            && order.trailing_offset_type().is_none(),
        "risk-reducing IOC requires the validated market-IOC base-quantity template"
    );
    Ok(())
}

fn require_canonical_exit_position_snapshot(
    canonical: &BoltV3CanonicalPositionAuthority,
    position_id: PositionId,
    instrument_id: InstrumentId,
    position_side: NtPositionSide,
) -> Result<Decimal> {
    anyhow::ensure!(
        canonical.is_exact_target(),
        "risk-reducing IOC position authority has ambiguous netting scope: position_id={position_id} instrument_id={instrument_id}"
    );
    let signed = canonical.signed_quantity();
    match position_side {
        NtPositionSide::Long if signed > Decimal::ZERO => Ok(signed),
        NtPositionSide::Short if signed < Decimal::ZERO => Ok(-signed),
        NtPositionSide::Long | NtPositionSide::Short => {
            anyhow::bail!(
                "risk-reducing IOC canonical position side mismatch: position_id={position_id} instrument_id={instrument_id} expected={position_side:?} signed_quantity={signed}"
            )
        }
        NtPositionSide::Flat | NtPositionSide::NoPositionSide => anyhow::bail!(
            "risk-reducing IOC canonical position must be non-flat: position_id={position_id} instrument_id={instrument_id}"
        ),
    }
}

fn require_canonical_exit_position_at_seal(
    position_authority: &BoltV3PositionAuthorityCapability,
    canonical_at_compile: &BoltV3CanonicalPositionAuthority,
    position_id: PositionId,
    instrument_id: InstrumentId,
    position_side: NtPositionSide,
    canonical_quantity_at_compile: Decimal,
    final_quantity: Quantity,
) -> Result<Decimal> {
    let canonical_at_seal = position_authority
        .canonical_position(position_id, instrument_id)?
        .with_context(|| {
            format!(
                "position authority cache is missing position_id={position_id} instrument_id={instrument_id}"
            )
        })?;
    let canonical = require_canonical_exit_position_snapshot(
        &canonical_at_seal,
        position_id,
        instrument_id,
        position_side,
    )?;
    anyhow::ensure!(
        canonical_at_seal == *canonical_at_compile,
        "canonical NT position changed after IOC compilation: instrument_id={instrument_id} compile_quantity={canonical_quantity_at_compile} seal_quantity={canonical}"
    );
    anyhow::ensure!(
        canonical >= final_quantity.as_decimal(),
        "compiled IOC quantity exceeds the unchanged canonical NT position: instrument_id={instrument_id} compiled_quantity={} canonical_quantity={canonical}",
        final_quantity
    );
    Ok(canonical)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoltV3PlannedFillLeg {
    pub price: Decimal,
    pub quantity: Decimal,
}

pub trait OrderIntentEvidence {
    fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<crate::bolt_v3_current_evidence::AppendReceipt, RecordFailure>;

    fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome;
}

impl OrderIntentEvidence for OrderExecutionEvidence {
    fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<crate::bolt_v3_current_evidence::AppendReceipt, RecordFailure> {
        self.record_entry_order_intent(fact)
    }

    fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome {
        self.record_risk_reducing_exit_order_intent(fact)
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl OrderIntentEvidence for crate::bolt_v3_current_evidence::DecisionEvidenceRecorder {
    fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<crate::bolt_v3_current_evidence::AppendReceipt, RecordFailure> {
        self.record_entry_order_intent(fact)
    }

    fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome {
        self.record_risk_reducing_exit_order_intent(fact)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderExecutionMode {
    Live,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3OrderExecutionPolicy {
    mode: BoltV3OrderExecutionMode,
}

impl BoltV3OrderExecutionPolicy {
    pub const fn from_mode(mode: BoltV3OrderExecutionMode) -> Self {
        Self { mode }
    }

    pub const fn live() -> Self {
        Self::from_mode(BoltV3OrderExecutionMode::Live)
    }

    pub const fn shadow() -> Self {
        Self::from_mode(BoltV3OrderExecutionMode::Shadow)
    }

    pub const fn mode(self) -> BoltV3OrderExecutionMode {
        self.mode
    }

    pub const fn allows_venue_mutation(self) -> bool {
        matches!(self.mode, BoltV3OrderExecutionMode::Live)
    }

    pub fn route_submit<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        strategy: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> BoltV3SubmitAttemptOutcome
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_submit_with_sink(routing, &mut sink, order, context)
    }

    pub(crate) fn route_submit_with_sink<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        sink: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> BoltV3SubmitAttemptOutcome
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        match self.try_route_submit_with_sink(routing, sink, order, context) {
            Ok(BoltV3SubmitRouteSuccess::Submitted(prepared_order)) => {
                BoltV3SubmitAttemptOutcome::submitted(prepared_order)
            }
            Ok(BoltV3SubmitRouteSuccess::PolicySkipped) => {
                BoltV3SubmitAttemptOutcome::policy_skipped()
            }
            Err(rejected) => rejected,
        }
    }

    fn try_route_submit_with_sink<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        sink: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> std::result::Result<BoltV3SubmitRouteSuccess, BoltV3SubmitAttemptOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        let BoltV3SubmitRoutingRequest {
            decision_evidence,
            submit_admission,
            intent,
            request,
            economics,
            required_remaining_margin_ns,
        } = routing;
        let prepared_order = prepared_order_linkage(&intent);
        let intent_kind = request.intent_kind;
        let execution_client_id = context
            .client_id
            .as_ref()
            .map(ClientId::as_str)
            .ok_or(BoltV3SubmitAdmissionError::EconomicsOrderMismatch)
            .map_err(|error| {
                BoltV3SubmitAttemptOutcome::rejected(
                    BoltV3SubmitRejectionKind::RouteValidation,
                    error,
                )
            })?;
        let route_now_ns = sink.actor_time_ns().map_err(|error| {
            BoltV3SubmitAttemptOutcome::rejected(BoltV3SubmitRejectionKind::RouteValidation, error)
        })?;
        validate_economics_submit_authority(&request, &economics, &order, execution_client_id)
            .map_err(|error| {
                BoltV3SubmitAttemptOutcome::rejected(
                    BoltV3SubmitRejectionKind::RouteValidation,
                    error,
                )
            })?;
        validate_economics_remaining_margin_at(
            &economics,
            required_remaining_margin_ns,
            route_now_ns,
        )
        .map_err(|error| {
            BoltV3SubmitAttemptOutcome::rejected(BoltV3SubmitRejectionKind::RouteValidation, error)
        })?;
        record_order_intent(decision_evidence, intent_kind, intent.clone()).map_err(|error| {
            BoltV3SubmitAttemptOutcome::rejected(BoltV3SubmitRejectionKind::IntentEvidence, error)
        })?;
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                let permit = submit_admission
                    .admit_with_economics_at(&request, &economics, route_now_ns)
                    .map_err(|error| {
                        BoltV3SubmitAttemptOutcome::rejected(
                            BoltV3SubmitRejectionKind::Admission,
                            error,
                        )
                    })?;
                let pre_sink_now_ns = sink.actor_time_ns().map_err(|error| {
                    BoltV3SubmitAttemptOutcome::rejected(BoltV3SubmitRejectionKind::PreSink, error)
                })?;
                validate_economics_remaining_margin_at(
                    &economics,
                    required_remaining_margin_ns,
                    pre_sink_now_ns,
                )
                .map_err(|error| {
                    BoltV3SubmitAttemptOutcome::rejected(BoltV3SubmitRejectionKind::PreSink, error)
                })?;
                sink.submit_order_via_nt(order, context).map_err(|error| {
                    BoltV3SubmitAttemptOutcome::rejected(BoltV3SubmitRejectionKind::Sink, error)
                })?;
                permit.commit_submitted();
                Ok(BoltV3SubmitRouteSuccess::Submitted(prepared_order))
            }
            BoltV3OrderExecutionMode::Shadow => {
                submit_admission
                    .evaluate_and_record_without_consuming_capacity_with_economics_at(
                        &request,
                        &economics,
                        route_now_ns,
                    )
                    .map_err(|error| {
                        BoltV3SubmitAttemptOutcome::rejected(
                            BoltV3SubmitRejectionKind::Admission,
                            error,
                        )
                    })?;
                log::info!(
                    "bolt-v3 submit skipped by execution policy: mode=shadow strategy_id={} client_order_id={}",
                    request.strategy_id,
                    request.client_order_id,
                );
                Ok(BoltV3SubmitRouteSuccess::PolicySkipped)
            }
        }
    }

    pub fn route_cancel<S>(
        self,
        strategy: &mut S,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelRoutingOutcome>
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_cancel_with_sink(&mut sink, client_order_id, client_id, params)
    }

    fn route_cancel_with_sink<S>(
        self,
        sink: &mut S,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                sink.cancel_order_via_nt(client_order_id, client_id, params)?;
                Ok(BoltV3CancelRoutingOutcome::Canceled)
            }
            BoltV3OrderExecutionMode::Shadow => {
                log::info!(
                    "bolt-v3 cancel skipped by execution policy: mode=shadow client_order_id={client_order_id}"
                );
                Ok(BoltV3CancelRoutingOutcome::SkippedByPolicy)
            }
        }
    }

    fn route_modify_with_sink<S>(
        self,
        _sink: &mut S,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3ModifyRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        // FAIL-CLOSED in Live (see #835). Unlike submit — which builds a
        // `BoltV3SubmitAdmissionRequest`, records intent evidence, and consumes
        // admission capacity before mutating the venue — an in-place modify carries
        // NONE of those admission/reservation/fee/intent checks. Routing one to the
        // venue in Live would bypass the risk gate: a live amend could lift a resting
        // order's economics-backed reservation past the operator notional limit a submit
        // would block, with no capital-reservation delta recorded. Until the
        // admission-gated in-place modify lands (#835), the Live arm REFUSES the venue
        // mutation; the maker requotes through the already-admitted cancel+resubmit
        // path (the deployed venue contract has `supports_modify=false`, so the maker
        // FSM never emits a Modify and this arm is unreachable from it — this is the
        // structural guard if that capability is ever turned on). Shadow stays
        // suppressed (logged, no NT call), as before.
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                // The amend params are intentionally NOT applied (the modify is
                // refused); consume them so the fail-closed arm is warning-clean.
                let _ = (quantity, price, client_id, params);
                Err(anyhow::anyhow!(
                    "bolt-v3 in-place modify is fail-closed in Live (not admission-gated; see #835): refusing un-admitted venue mutation for client_order_id={client_order_id}"
                ))
            }
            BoltV3OrderExecutionMode::Shadow => {
                log::info!(
                    "bolt-v3 modify skipped by execution policy: mode=shadow client_order_id={client_order_id}"
                );
                Ok(BoltV3ModifyRoutingOutcome::SkippedByPolicy)
            }
        }
    }
}

pub(crate) fn record_order_intent(
    recorder: &dyn OrderIntentEvidence,
    intent_kind: BoltV3SubmitIntentKind,
    details: OrderIntentDetails,
) -> Result<()> {
    match intent_kind {
        BoltV3SubmitIntentKind::Entry => recorder
            .record_entry_order_intent(EntryOrderIntentFact { details })
            .map(|_| ())
            .map_err(anyhow::Error::from),
        BoltV3SubmitIntentKind::RiskReducingExit
        | BoltV3SubmitIntentKind::KillSwitchForcedReduction => match recorder
            .record_risk_reducing_exit_order_intent(RiskReducingExitOrderIntentFact { details })
        {
            NonBlockingRecordOutcome::Appended(_) => Ok(()),
            NonBlockingRecordOutcome::Failed(error) => Err(anyhow::Error::msg(error.to_string())),
        },
    }
}

pub fn order_intent_details_from_compiled_order(
    strategy_id: String,
    fallback_price: String,
    order: &OrderAny,
) -> OrderIntentDetails {
    OrderIntentDetails {
        strategy_id,
        instrument_id: order.instrument_id().to_string(),
        client_order_id: order.client_order_id().to_string(),
        order_side: evidence_order_side(order.order_side()),
        price: order
            .price()
            .map(|price| price.to_string())
            .or_else(|| order.trigger_price().map(|price| price.to_string()))
            .or_else(|| order.activation_price().map(|price| price.to_string()))
            .unwrap_or(fallback_price),
        quantity: order.quantity().to_string(),
        clamp_outcome: None,
        order_fields: order_intent_order_fields(order),
    }
}

pub(crate) fn prepared_order_linkage(intent: &OrderIntentDetails) -> PreparedOrderLinkage {
    PreparedOrderLinkage {
        instrument_id: intent.instrument_id.clone(),
        order_side: intent.order_side,
        price: intent.price.clone(),
        quantity: intent.quantity.clone(),
        client_order_id: intent.client_order_id.clone(),
    }
}

fn order_intent_order_fields(order: &OrderAny) -> OrderIntentOrderFields {
    OrderIntentOrderFields {
        order_type: evidence_order_type(order.order_type()),
        time_in_force: evidence_time_in_force(order.time_in_force()),
        price: order.price().map(|price| price.to_string()),
        trigger_price: order.trigger_price().map(|price| price.to_string()),
        activation_price: order.activation_price().map(|price| price.to_string()),
        trigger_type: order.trigger_type().map(evidence_trigger_type),
        trigger_instrument_id: order.trigger_instrument_id().map(|value| value.to_string()),
        trailing_offset: order.trailing_offset().map(|value| value.to_string()),
        trailing_offset_type: order
            .trailing_offset_type()
            .map(evidence_trailing_offset_type),
        expire_time_unix_nanos: order.expire_time().map(|value| value.as_u64().to_string()),
        is_post_only: order.is_post_only(),
        is_reduce_only: order.is_reduce_only(),
        is_quote_quantity: order.is_quote_quantity(),
    }
}

fn evidence_order_side(value: OrderSide) -> EvidenceOrderSide {
    match value {
        OrderSide::NoOrderSide => EvidenceOrderSide::Unspecified,
        OrderSide::Buy => EvidenceOrderSide::Buy,
        OrderSide::Sell => EvidenceOrderSide::Sell,
    }
}

fn evidence_order_type(value: OrderType) -> EvidenceOrderType {
    match value {
        OrderType::Market => EvidenceOrderType::Market,
        OrderType::Limit => EvidenceOrderType::Limit,
        OrderType::StopMarket => EvidenceOrderType::StopMarket,
        OrderType::StopLimit => EvidenceOrderType::StopLimit,
        OrderType::MarketToLimit => EvidenceOrderType::MarketToLimit,
        OrderType::MarketIfTouched => EvidenceOrderType::MarketIfTouched,
        OrderType::LimitIfTouched => EvidenceOrderType::LimitIfTouched,
        OrderType::TrailingStopMarket => EvidenceOrderType::TrailingStopMarket,
        OrderType::TrailingStopLimit => EvidenceOrderType::TrailingStopLimit,
    }
}

fn evidence_time_in_force(value: TimeInForce) -> EvidenceTimeInForce {
    match value {
        TimeInForce::Gtc => EvidenceTimeInForce::Gtc,
        TimeInForce::Ioc => EvidenceTimeInForce::Ioc,
        TimeInForce::Fok => EvidenceTimeInForce::Fok,
        TimeInForce::Gtd => EvidenceTimeInForce::Gtd,
        TimeInForce::Day => EvidenceTimeInForce::Day,
        TimeInForce::AtTheOpen => EvidenceTimeInForce::AtTheOpen,
        TimeInForce::AtTheClose => EvidenceTimeInForce::AtTheClose,
    }
}

fn evidence_trigger_type(value: TriggerType) -> EvidenceTriggerType {
    match value {
        TriggerType::NoTrigger => EvidenceTriggerType::NoTrigger,
        TriggerType::Default => EvidenceTriggerType::Default,
        TriggerType::LastPrice => EvidenceTriggerType::LastPrice,
        TriggerType::MarkPrice => EvidenceTriggerType::MarkPrice,
        TriggerType::IndexPrice => EvidenceTriggerType::IndexPrice,
        TriggerType::BidAsk => EvidenceTriggerType::BidAsk,
        TriggerType::DoubleLast => EvidenceTriggerType::DoubleLast,
        TriggerType::DoubleBidAsk => EvidenceTriggerType::DoubleBidAsk,
        TriggerType::LastOrBidAsk => EvidenceTriggerType::LastOrBidAsk,
        TriggerType::MidPoint => EvidenceTriggerType::MidPoint,
    }
}

fn evidence_trailing_offset_type(value: TrailingOffsetType) -> EvidenceTrailingOffsetType {
    match value {
        TrailingOffsetType::NoTrailingOffset => EvidenceTrailingOffsetType::NoTrailingOffset,
        TrailingOffsetType::Price => EvidenceTrailingOffsetType::Price,
        TrailingOffsetType::BasisPoints => EvidenceTrailingOffsetType::BasisPoints,
        TrailingOffsetType::Ticks => EvidenceTrailingOffsetType::Ticks,
        TrailingOffsetType::PriceTier => EvidenceTrailingOffsetType::PriceTier,
    }
}

fn clamp_risk_reducing_exit_to_position_quantity(
    mut intent: OrderIntentDetails,
    mut order: OrderAny,
    venue_position: Decimal,
) -> Result<(OrderIntentDetails, OrderAny)> {
    let order_quantity = order.quantity().as_decimal();
    let instrument_id = order.instrument_id().to_string();
    if order_quantity <= venue_position {
        intent.clamp_outcome = Some(OrderIntentClampOutcome::WithinBounds);
        return Ok((intent, order));
    }
    if venue_position <= Decimal::ZERO {
        anyhow::bail!(
            "risk-reducing exit rejected: no venue-held position to submit: instrument_id={}",
            instrument_id
        );
    }

    let original_order_quantity = order_quantity;
    let clamped_decimal =
        floor_decimal_to_quantity_precision(venue_position, order.quantity().precision)?;
    if clamped_decimal <= Decimal::ZERO {
        anyhow::bail!(
            "risk-reducing exit rejected: venue position is below order quantity precision: instrument_id={}",
            instrument_id
        );
    }
    let clamped_quantity = match Quantity::from_decimal_dp(
        clamped_decimal,
        order.quantity().precision,
    ) {
        Ok(quantity) => quantity,
        Err(error) => {
            anyhow::bail!(
                "risk-reducing exit venue position could not be represented as NT quantity: {error}"
            );
        }
    };
    let submitted_quantity = clamped_quantity.as_decimal();
    if submitted_quantity > venue_position {
        anyhow::bail!(
            "risk-reducing exit clamp exceeded venue position: instrument_id={}",
            instrument_id
        );
    }

    order.set_quantity(clamped_quantity);
    order.set_leaves_qty(clamped_quantity);
    intent.quantity = order.quantity().to_string();
    intent.clamp_outcome = Some(OrderIntentClampOutcome::Clamped {
        original_quantity: original_order_quantity.to_string(),
    });
    intent.order_fields = order_intent_order_fields(&order);

    Ok((intent, order))
}

fn floor_decimal_to_quantity_precision(value: Decimal, precision: u8) -> Result<Decimal> {
    Ok(value.round_dp_with_strategy(u32::from(precision), RoundingStrategy::ToZero))
}

pub struct BoltV3SubmitRoutingRequest<'a> {
    decision_evidence: &'a dyn OrderIntentEvidence,
    submit_admission: &'a BoltV3SubmitAdmissionState,
    intent: OrderIntentDetails,
    request: BoltV3SubmitAdmissionRequest,
    economics: EconomicsAdmission,
    required_remaining_margin_ns: u64,
}

impl<'a> BoltV3SubmitRoutingRequest<'a> {
    pub fn with_economics(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
        sealed: BoltV3EconomicsSubmitAdmission,
    ) -> Self {
        let (request, economics, required_remaining_margin_ns) = sealed.into_parts();
        Self {
            decision_evidence,
            submit_admission,
            intent,
            request,
            economics,
            required_remaining_margin_ns,
        }
    }

    #[cfg(test)]
    fn for_test(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
        request: BoltV3SubmitAdmissionRequest,
        order: &OrderAny,
    ) -> Self {
        Self::for_test_with_timing(
            decision_evidence,
            submit_admission,
            intent,
            request,
            order,
            u64::MAX,
            1,
        )
    }

    #[cfg(test)]
    fn for_test_with_timing(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
        request: BoltV3SubmitAdmissionRequest,
        order: &OrderAny,
        valid_until_ns: u64,
        required_remaining_margin_ns: u64,
    ) -> Self {
        let purpose = match request.intent_kind {
            BoltV3SubmitIntentKind::Entry => EconomicsAdmissionPurpose::TradingEdge,
            BoltV3SubmitIntentKind::RiskReducingExit
            | BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
                EconomicsAdmissionPurpose::RiskReduction
            }
        };
        let order_side = match order.order_side() {
            OrderSide::Buy => crate::economics::OrderSide::Buy,
            OrderSide::Sell => crate::economics::OrderSide::Sell,
            OrderSide::NoOrderSide => panic!("routing-test order must be sided"),
        };
        let economics = EconomicsAdmission::for_routing_test_with_validity(
            &request.execution_client_id,
            &request.instrument_id,
            order_side,
            economics_order_binding(order).expect("routing-test order should serialize"),
            purpose,
            request.notional,
            request.notional,
            valid_until_ns,
        );
        Self {
            decision_evidence,
            submit_admission,
            intent,
            request,
            economics,
            required_remaining_margin_ns,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoltV3SubmitContext {
    pub(crate) client_id: Option<ClientId>,
    pub(crate) position_id: Option<PositionId>,
    pub(crate) params: Option<Params>,
}

impl BoltV3SubmitContext {
    pub fn from_parts(
        client_id: Option<ClientId>,
        position_id: Option<PositionId>,
        params: Option<Params>,
    ) -> Self {
        Self {
            client_id,
            position_id,
            params,
        }
    }

    pub fn with_client_id(client_id: ClientId) -> Self {
        Self::from_parts(Some(client_id), None, None)
    }

    pub fn with_client_id_and_position_id(client_id: ClientId, position_id: PositionId) -> Self {
        Self::from_parts(Some(client_id), Some(position_id), None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3SubmitAttemptKind {
    RouteValidationRejected,
    IntentEvidenceRejected,
    AdmissionRejected,
    PolicySkipped,
    PreSinkRejected,
    SinkRejected,
    Submitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoltV3SubmitRejectionKind {
    RouteValidation,
    IntentEvidence,
    Admission,
    PreSink,
    Sink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BoltV3SubmitRouteSuccess {
    PolicySkipped,
    Submitted(PreparedOrderLinkage),
}

impl From<BoltV3SubmitRejectionKind> for BoltV3SubmitAttemptKind {
    fn from(value: BoltV3SubmitRejectionKind) -> Self {
        match value {
            BoltV3SubmitRejectionKind::RouteValidation => Self::RouteValidationRejected,
            BoltV3SubmitRejectionKind::IntentEvidence => Self::IntentEvidenceRejected,
            BoltV3SubmitRejectionKind::Admission => Self::AdmissionRejected,
            BoltV3SubmitRejectionKind::PreSink => Self::PreSinkRejected,
            BoltV3SubmitRejectionKind::Sink => Self::SinkRejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoltV3SubmitAttemptState {
    RouteValidationRejected(String),
    IntentEvidenceRejected(String),
    AdmissionRejected(String),
    PolicySkipped,
    PreSinkRejected(String),
    SinkRejected(String),
    Submitted(SubmittedOrderLinkage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitAttemptOutcome {
    state: BoltV3SubmitAttemptState,
}

impl BoltV3SubmitAttemptOutcome {
    fn submitted(prepared_order: PreparedOrderLinkage) -> Self {
        Self {
            state: BoltV3SubmitAttemptState::Submitted(prepared_order.into()),
        }
    }

    fn policy_skipped() -> Self {
        Self {
            state: BoltV3SubmitAttemptState::PolicySkipped,
        }
    }

    fn rejected(kind: BoltV3SubmitRejectionKind, error: impl std::fmt::Display) -> Self {
        let diagnostic = error.to_string();
        let state = match kind {
            BoltV3SubmitRejectionKind::RouteValidation => {
                BoltV3SubmitAttemptState::RouteValidationRejected(diagnostic)
            }
            BoltV3SubmitRejectionKind::IntentEvidence => {
                BoltV3SubmitAttemptState::IntentEvidenceRejected(diagnostic)
            }
            BoltV3SubmitRejectionKind::Admission => {
                BoltV3SubmitAttemptState::AdmissionRejected(diagnostic)
            }
            BoltV3SubmitRejectionKind::PreSink => {
                BoltV3SubmitAttemptState::PreSinkRejected(diagnostic)
            }
            BoltV3SubmitRejectionKind::Sink => BoltV3SubmitAttemptState::SinkRejected(diagnostic),
        };
        Self { state }
    }

    #[must_use]
    pub const fn kind(&self) -> BoltV3SubmitAttemptKind {
        match &self.state {
            BoltV3SubmitAttemptState::RouteValidationRejected(_) => {
                BoltV3SubmitAttemptKind::RouteValidationRejected
            }
            BoltV3SubmitAttemptState::IntentEvidenceRejected(_) => {
                BoltV3SubmitAttemptKind::IntentEvidenceRejected
            }
            BoltV3SubmitAttemptState::AdmissionRejected(_) => {
                BoltV3SubmitAttemptKind::AdmissionRejected
            }
            BoltV3SubmitAttemptState::PolicySkipped => BoltV3SubmitAttemptKind::PolicySkipped,
            BoltV3SubmitAttemptState::PreSinkRejected(_) => {
                BoltV3SubmitAttemptKind::PreSinkRejected
            }
            BoltV3SubmitAttemptState::SinkRejected(_) => BoltV3SubmitAttemptKind::SinkRejected,
            BoltV3SubmitAttemptState::Submitted(_) => BoltV3SubmitAttemptKind::Submitted,
        }
    }

    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        match &self.state {
            BoltV3SubmitAttemptState::RouteValidationRejected(diagnostic)
            | BoltV3SubmitAttemptState::IntentEvidenceRejected(diagnostic)
            | BoltV3SubmitAttemptState::AdmissionRejected(diagnostic)
            | BoltV3SubmitAttemptState::PreSinkRejected(diagnostic)
            | BoltV3SubmitAttemptState::SinkRejected(diagnostic) => Some(diagnostic),
            BoltV3SubmitAttemptState::PolicySkipped | BoltV3SubmitAttemptState::Submitted(_) => {
                None
            }
        }
    }

    #[must_use]
    pub const fn is_submitted(&self) -> bool {
        matches!(&self.state, BoltV3SubmitAttemptState::Submitted(_))
    }

    #[must_use]
    pub fn submitted_order(&self) -> Option<&SubmittedOrderLinkage> {
        match &self.state {
            BoltV3SubmitAttemptState::Submitted(linkage) => Some(linkage),
            BoltV3SubmitAttemptState::RouteValidationRejected(_)
            | BoltV3SubmitAttemptState::IntentEvidenceRejected(_)
            | BoltV3SubmitAttemptState::AdmissionRejected(_)
            | BoltV3SubmitAttemptState::PolicySkipped
            | BoltV3SubmitAttemptState::PreSinkRejected(_)
            | BoltV3SubmitAttemptState::SinkRejected(_) => None,
        }
    }

    pub(crate) fn into_state(self) -> BoltV3SubmitAttemptState {
        self.state
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn submitted_for_test() -> Self {
        Self::submitted(PreparedOrderLinkage {
            instrument_id: "TEST-INSTRUMENT.TEST".to_string(),
            order_side: EvidenceOrderSide::Buy,
            price: "1".to_string(),
            quantity: "1".to_string(),
            client_order_id: "TEST-ORDER".to_string(),
        })
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn submitted_with_linkage_for_test(
        instrument_id: InstrumentId,
        order_side: OrderSide,
        price: Price,
        quantity: Quantity,
        client_order_id: ClientOrderId,
    ) -> Self {
        let order_side = match order_side {
            OrderSide::Buy => EvidenceOrderSide::Buy,
            OrderSide::Sell => EvidenceOrderSide::Sell,
            OrderSide::NoOrderSide => panic!("test submitted linkage requires a sided order"),
        };
        Self::submitted(PreparedOrderLinkage {
            instrument_id: instrument_id.to_string(),
            order_side,
            price: price.to_string(),
            quantity: quantity.to_string(),
            client_order_id: client_order_id.to_string(),
        })
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn policy_skipped_for_test() -> Self {
        Self::policy_skipped()
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn rejected_for_test(kind: BoltV3SubmitAttemptKind, diagnostic: impl Into<String>) -> Self {
        match kind {
            BoltV3SubmitAttemptKind::RouteValidationRejected
            | BoltV3SubmitAttemptKind::IntentEvidenceRejected
            | BoltV3SubmitAttemptKind::AdmissionRejected
            | BoltV3SubmitAttemptKind::PreSinkRejected
            | BoltV3SubmitAttemptKind::SinkRejected => {
                let diagnostic = diagnostic.into();
                let rejection = match kind {
                    BoltV3SubmitAttemptKind::RouteValidationRejected => {
                        BoltV3SubmitRejectionKind::RouteValidation
                    }
                    BoltV3SubmitAttemptKind::IntentEvidenceRejected => {
                        BoltV3SubmitRejectionKind::IntentEvidence
                    }
                    BoltV3SubmitAttemptKind::AdmissionRejected => {
                        BoltV3SubmitRejectionKind::Admission
                    }
                    BoltV3SubmitAttemptKind::PreSinkRejected => BoltV3SubmitRejectionKind::PreSink,
                    BoltV3SubmitAttemptKind::SinkRejected => BoltV3SubmitRejectionKind::Sink,
                    BoltV3SubmitAttemptKind::PolicySkipped | BoltV3SubmitAttemptKind::Submitted => {
                        unreachable!()
                    }
                };
                Self::rejected(rejection, diagnostic)
            }
            BoltV3SubmitAttemptKind::PolicySkipped | BoltV3SubmitAttemptKind::Submitted => {
                panic!("rejected_for_test requires a rejection kind")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CancelRoutingOutcome {
    Canceled,
    SkippedByPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3ModifyRoutingOutcome {
    Modified,
    SkippedByPolicy,
}

#[derive(Clone)]
pub struct BoltV3MakerOrderRoutingContext<'a> {
    pub strategy_id: &'a str,
    pub execution_client_id: &'a str,
    pub order_economics: &'a BoltV3OrderEconomicsHandle,
    pub terminal_value_entry: Option<BoltV3TerminalValueEntry>,
}

pub fn route_maker_order_command<S>(
    policy: BoltV3OrderExecutionPolicy,
    strategy: &mut S,
    decision_evidence: &dyn OrderIntentEvidence,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'_>,
    input: MakerOrderDispatchInput<'_>,
) -> std::result::Result<MakerOrderDispatchOutcome, MakerOrderCommandFailure>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    let mut runtime = NtStrategyMakerOrderRuntime { strategy };
    route_maker_order_command_with_runtime(
        policy,
        &mut runtime,
        decision_evidence,
        submit_admission,
        context,
        input,
    )
}

pub(crate) trait BoltV3NtVenueMutationSink {
    fn actor_time_ns(&mut self) -> Result<u64>;

    fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>>;

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()>;

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;

    // The venue's in-place modify capability. Option A (#835) fail-closes the only
    // routing path (`route_modify_with_sink` refuses live modifies and the maker FSM
    // never emits a Modify while `supports_modify=false`), so this method is
    // intentionally uncalled today. The wiring is retained for #835 (admission-gated
    // in-place modify) and to keep the fail-closed differential tests load-bearing
    // (reverting the fail-close to a venue call must still flip `modify_calls` 0->1).
    // `expect` (not `allow`) is self-cleaning: when #835 wires a real caller the
    // expectation goes unfulfilled and clippy forces this attribute removed.
    #[expect(dead_code)]
    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;
}

struct NtStrategyVenueMutationSink<'a, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyVenueMutationSink<'_, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    fn actor_time_ns(&mut self) -> Result<u64> {
        Ok(self.strategy.clock().timestamp_ns().as_u64())
    }

    fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
        Ok(self.strategy.cache().order(&client_order_id))
    }

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy.query_order(seed, client_id, params)
    }

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        self.strategy.submit_order(
            order,
            context.position_id,
            context.client_id,
            context.params,
        )
    }

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy
            .cancel_order(client_order_id, client_id, params)
    }

    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        // NT's `modify_order` is the single owner of the in-place amend command
        // (NT-FIRST, NO DUAL PATHS); the maker only supplies the new price and
        // quantity. `trigger_price` is `None` — maker quotes are post-only limits
        // with no trigger.
        self.strategy.modify_order(
            client_order_id,
            Some(quantity),
            Some(price),
            None,
            client_id,
            params,
        )
    }
}

trait BoltV3MakerOrderRuntime: BoltV3NtVenueMutationSink {
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory>;
}

struct NtStrategyMakerOrderRuntime<'a, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    fn actor_time_ns(&mut self) -> Result<u64> {
        Ok(self.strategy.clock().timestamp_ns().as_u64())
    }

    fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
        Ok(self.strategy.cache().order(&client_order_id))
    }

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy.query_order(seed, client_id, params)
    }

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        self.strategy.submit_order(
            order,
            context.position_id,
            context.client_id,
            context.params,
        )
    }

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy
            .cancel_order(client_order_id, client_id, params)
    }

    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy.modify_order(
            client_order_id,
            Some(quantity),
            Some(price),
            None,
            client_id,
            params,
        )
    }
}

impl<S> BoltV3MakerOrderRuntime for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.strategy.order_factory()
    }
}

fn route_maker_order_command_with_runtime<R>(
    policy: BoltV3OrderExecutionPolicy,
    runtime: &mut R,
    decision_evidence: &dyn OrderIntentEvidence,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'_>,
    input: MakerOrderDispatchInput<'_>,
) -> std::result::Result<MakerOrderDispatchOutcome, MakerOrderCommandFailure>
where
    R: BoltV3MakerOrderRuntime + ?Sized,
{
    let mut sink = BoltV3MakerOrderPolicySink {
        policy,
        runtime,
        decision_evidence,
        submit_admission,
        context,
    };
    dispatch_maker_order_command(input, &mut sink)
}

struct BoltV3MakerOrderPolicySink<'a, R>
where
    R: BoltV3MakerOrderRuntime + ?Sized,
{
    policy: BoltV3OrderExecutionPolicy,
    runtime: &'a mut R,
    decision_evidence: &'a dyn OrderIntentEvidence,
    submit_admission: &'a BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'a>,
}

struct PreparedMakerOrderSubmission {
    order: OrderAny,
    intent: OrderIntentDetails,
    sealed: BoltV3EconomicsSubmitAdmission,
    admission: EconomicsAdmission,
}

impl<R> MakerOrderCommandSink for BoltV3MakerOrderPolicySink<'_, R>
where
    R: BoltV3MakerOrderRuntime + ?Sized,
{
    type PreparedSubmit = PreparedMakerOrderSubmission;

    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.runtime.order_factory()
    }

    fn prepare_maker_order(&mut self, order: OrderAny) -> Result<Self::PreparedSubmit> {
        let fallback_price = order
            .price()
            .map(|price| price.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bolt-v3 maker submit requires a limit price for client_order_id={}",
                    order.client_order_id()
                )
            })?;
        let intent = order_intent_details_from_compiled_order(
            self.context.strategy_id.to_string(),
            fallback_price,
            &order,
        );
        let admission_input = BoltV3SubmitAdmissionRequestInput {
            execution_client_id: self.context.execution_client_id,
            intent: &intent,
            intent_kind: BoltV3SubmitIntentKind::Entry,
            order: &order,
            valuation: crate::bolt_v3_submit_admission::OrderValuationContext::empty(),
            risk_reducing_exit_position: None,
        };
        let facts = order_admission_facts(&admission_input)?;
        let sealed = build_order_economics_submit_admission(
            self.context.order_economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: self.context.execution_client_id,
                intent: &intent,
                order: &order,
                valuation: admission_input.valuation,
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    self.context.terminal_value_entry.clone().ok_or_else(|| {
                        anyhow::anyhow!("maker submit requires a terminal-value economics scenario")
                    })?,
                ),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: facts.price,
                    quantity: facts.order_quantity,
                }],
                requested_at_ns: order.ts_init().as_u64(),
                decision_correlation_id: order.client_order_id().as_str(),
            },
        )?;
        let admission = sealed.economics().clone();
        Ok(PreparedMakerOrderSubmission {
            order,
            intent,
            sealed,
            admission,
        })
    }

    fn submit_maker_order(
        &mut self,
        prepared: Self::PreparedSubmit,
    ) -> BoltV3RestingSubmitTransactionOutcome {
        let PreparedMakerOrderSubmission {
            order,
            intent,
            sealed,
            admission,
        } = prepared;
        let submit_context =
            BoltV3SubmitContext::with_client_id(ClientId::from(self.context.execution_client_id));
        let order_to_route = order.clone();
        let route = || {
            self.policy.route_submit_with_sink(
                BoltV3SubmitRoutingRequest::with_economics(
                    self.decision_evidence,
                    self.submit_admission,
                    intent,
                    sealed,
                ),
                self.runtime,
                order_to_route,
                submit_context,
            )
        };
        self.context
            .order_economics
            .route_resting_submit(self.policy, order, admission, route)
    }

    fn cancel_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    ) -> Result<()> {
        self.context.order_economics.route_tracked_cancel(
            self.policy,
            self.runtime,
            self.context.execution_client_id,
            client_order_id,
        )
    }

    fn cancel_all_maker_orders(
        &mut self,
        _leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<()> {
        route_tracked_cancel_all(
            self.context.order_economics,
            self.policy,
            self.runtime,
            self.context.execution_client_id,
            instrument_id,
            order_side,
        )
    }

    fn modify_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Result<()> {
        // Routes the in-place amend through the execution-policy boundary. Under
        // Option A (#835) the Live arm is FAIL-CLOSED: an in-place modify does not
        // pass submit admission, so `route_modify_with_sink` returns `Err`, the `?`
        // below propagates it, and the venue is never reached; Shadow stays
        // suppressed. The FSM only emits a Modify for a modify-capable venue (the
        // `supports_modify` capability is threaded into the leg state machine), and
        // the deployed venue contract has `supports_modify=false`, so the maker
        // requotes via cancel+resubmit and a no-modify venue never reaches this path.
        self.policy.route_modify_with_sink(
            self.runtime,
            client_order_id,
            quantity,
            price,
            Some(ClientId::from(self.context.execution_client_id)),
            None,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{RefCell, RefMut},
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
        sync::Arc,
    };

    use anyhow::Result;
    use nautilus_common::{
        clock::{Clock, TestClock},
        factories::OrderFactory,
    };
    use nautilus_core::Params;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::{
            AssetClass, LiquiditySide, OmsType, OrderSide, OrderStatus, OrderType, PositionSide,
            PositionSideSpecified, TimeInForce,
        },
        events::{
            OrderCanceled, OrderDenied, OrderEventAny, OrderFilled,
            order::spec::OrderFillVoidedSpec,
        },
        identifiers::{
            AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, Symbol,
            TradeId, TraderId, Venue, VenueOrderId,
        },
        instruments::{BinaryOption, Instrument, InstrumentAny},
        orders::{LimitOrder, MarketOrder, Order, OrderAny, stubs::TestOrderEventStubs},
        position::Position,
        reports::PositionStatusReport,
        types::{Currency, Money, Price, Quantity},
    };
    use rust_decimal::Decimal;
    use ustr::Ustr;

    const FIXTURE_CANCEL_RETRY_TIMEOUT_NS: u64 = 1_000_000_000;

    fn position_authority_with_canonical_position(
        instrument: &InstrumentAny,
        position_id: PositionId,
        quantity: Quantity,
    ) -> BoltV3PositionAuthorityCapability {
        let account_id = AccountId::from("ACCOUNT-001");
        let execution_client_id = ClientId::from("execution_client");
        let mut fill = OrderFilled::new(
            TraderId::from("TRADER-001"),
            StrategyId::from("STRATEGY-001"),
            instrument.id(),
            ClientOrderId::from("ENTRY-001"),
            VenueOrderId::from("VENUE-ENTRY-001"),
            account_id,
            TradeId::from("ENTRY-TRADE-001"),
            OrderSide::Buy,
            OrderType::Market,
            quantity,
            Price::new(0.40, instrument.price_precision()),
            Currency::USDC(),
            LiquiditySide::Taker,
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_u64),
            UnixNanos::from(1_u64),
            false,
            None,
            Some(Money::new(0.0, Currency::USDC())),
            None,
        );
        fill.position_id = Some(position_id);
        let position = Position::new(instrument, fill);
        let cache = Rc::new(RefCell::new(nautilus_common::cache::Cache::default()));
        cache
            .borrow_mut()
            .add_position(&position, OmsType::Netting)
            .expect("position authority fixture should cache its canonical position");
        let feed = BoltV3PositionAuthorityFeed::try_new_with_cache(
            [(account_id, execution_client_id, instrument.id().venue)],
            cache,
        )
        .expect("position authority fixture should build");
        BoltV3PositionAuthorityCapability::new(
            feed,
            execution_client_id,
            account_id,
            OmsType::Netting,
        )
    }

    fn position_authority_with_ambiguous_netting_positions(
        instrument: &InstrumentAny,
        target_position_id: PositionId,
        other_position_id: PositionId,
    ) -> BoltV3PositionAuthorityCapability {
        let account_id = AccountId::from("ACCOUNT-001");
        let execution_client_id = ClientId::from("execution_client");
        let cache = Rc::new(RefCell::new(nautilus_common::cache::Cache::default()));
        for (position_id, client_order_id, venue_order_id, trade_id) in [
            (
                target_position_id,
                "ENTRY-TARGET",
                "VENUE-ENTRY-TARGET",
                "ENTRY-TRADE-TARGET",
            ),
            (
                other_position_id,
                "ENTRY-OTHER",
                "VENUE-ENTRY-OTHER",
                "ENTRY-TRADE-OTHER",
            ),
        ] {
            let mut fill = OrderFilled::new(
                TraderId::from("TRADER-001"),
                StrategyId::from("STRATEGY-001"),
                instrument.id(),
                ClientOrderId::from(client_order_id),
                VenueOrderId::from(venue_order_id),
                account_id,
                TradeId::from(trade_id),
                OrderSide::Buy,
                OrderType::Market,
                Quantity::new(10.0, 2),
                Price::new(0.40, instrument.price_precision()),
                Currency::USDC(),
                LiquiditySide::Taker,
                nautilus_core::UUID4::new(),
                UnixNanos::from(1_u64),
                UnixNanos::from(1_u64),
                false,
                None,
                Some(Money::new(0.0, Currency::USDC())),
                None,
            );
            fill.position_id = Some(position_id);
            let position = Position::new(instrument, fill);
            cache
                .borrow_mut()
                .add_position(&position, OmsType::Netting)
                .expect("ambiguous position fixture should cache each netting position");
        }
        let feed = BoltV3PositionAuthorityFeed::try_new_with_cache(
            [(account_id, execution_client_id, instrument.id().venue)],
            cache,
        )
        .expect("position authority fixture should build");
        BoltV3PositionAuthorityCapability::new(
            feed,
            execution_client_id,
            account_id,
            OmsType::Netting,
        )
    }

    use super::{
        BoltV3CancelRoutingOutcome, BoltV3CancellationLivenessFailure,
        BoltV3CanonicalPositionAuthority, BoltV3CompileAndSealRiskReducingIocInput,
        BoltV3ExitOrderAuthorityHandle, BoltV3ExitOrderCorrection,
        BoltV3ExitOrderLifecycleReduction, BoltV3FillSetProof, BoltV3FinalOrderEconomicsInput,
        BoltV3FinalOrderEconomicsScenario, BoltV3MakerOrderRoutingContext, BoltV3MakerOrderRuntime,
        BoltV3ModifyRoutingOutcome, BoltV3NtVenueMutationSink, BoltV3OrderExecutionMode,
        BoltV3OrderExecutionPolicy, BoltV3PlannedFillLeg, BoltV3PositionReductionFence,
        BoltV3PositionReductionRelease, BoltV3RecoveredExitCause,
        BoltV3RestingSubmitTransactionOutcome, BoltV3SubmitAttemptKind, BoltV3SubmitContext,
        BoltV3SubmitRoutingRequest, BoltV3TakerEconomicsSizingInput, BoltV3TerminalValueEntry,
        BoltV3TerminalValueEntryPolicy, EconomicsAdmissionPurpose,
        build_order_economics_submit_admission, clamp_risk_reducing_exit_to_position_quantity,
        compile_and_seal_risk_reducing_ioc, compile_bounded_risk_reducing_ioc_for_execution,
        order_intent_details_from_compiled_order, route_maker_order_command_with_runtime,
    };
    use crate::{
        bolt_v3_capital_admission::{
            CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
            ProductAdmissionSnapshot, ProductKind,
        },
        bolt_v3_capital_admission_runtime_feed::POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE,
        bolt_v3_capital_admission_state::{
            OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
            ProviderCollateralAllowanceSnapshot,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_current_evidence::{
            AdmittedEntryAdmissionFact, CurrentFact, DecisionEvidenceRecorder,
            OrderIntentClampOutcome, OrderIntentDetails, RejectedEntryAdmissionFact,
        },
        bolt_v3_kill_switch::KillSwitchState,
        bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
        bolt_v3_maker_order_dispatch::{MakerOrderDispatchInput, MakerOrderDispatchOutcome},
        bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
        bolt_v3_position_authority_feed::{
            BoltV3PositionAuthorityCapability, BoltV3PositionAuthorityFeed,
        },
        bolt_v3_quote_lifecycle::Leg,
        bolt_v3_submit_admission::{
            BoltV3CompiledOrderAdmissionEvidence, BoltV3CompiledOrderKind,
            BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide, BoltV3CompiledProductKind,
            BoltV3LiveSubmitApprovalLimits, BoltV3RiskReducingExitProof,
            BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
            BoltV3SubmitCapitalAdmissionConfig, BoltV3SubmitCapitalAdmissionNtComponents,
            BoltV3SubmitIntentKind, OrderValuationContext, PredictionMarketOutcomeSide,
        },
        economics::{LifecyclePath, LiquidityRole},
        integrations::nautilus::economics::economics_order_binding,
    };

    trait RecordedCurrentEvidence {
        fn order_intents(&self) -> Vec<OrderIntentDetails>;
        fn admitted_entry_admissions(&self) -> Vec<AdmittedEntryAdmissionFact>;
        fn rejected_entry_admissions(&self) -> Vec<RejectedEntryAdmissionFact>;
        fn admission_count(&self) -> usize;
    }

    impl RecordedCurrentEvidence for DecisionEvidenceRecorder {
        fn order_intents(&self) -> Vec<OrderIntentDetails> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::EntryOrderIntent(fact) => Some(fact.details),
                    CurrentFact::RiskReducingExitOrderIntent(fact) => Some(fact.details),
                    _ => None,
                })
                .collect()
        }

        fn admitted_entry_admissions(&self) -> Vec<AdmittedEntryAdmissionFact> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::AdmittedEntryAdmission(fact) => Some(*fact),
                    _ => None,
                })
                .collect()
        }

        fn rejected_entry_admissions(&self) -> Vec<RejectedEntryAdmissionFact> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::RejectedEntryAdmission(fact) => Some(*fact),
                    _ => None,
                })
                .collect()
        }

        fn admission_count(&self) -> usize {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter(|fact| {
                    matches!(
                        fact,
                        CurrentFact::AdmittedEntryAdmission(_)
                            | CurrentFact::RejectedEntryAdmission(_)
                            | CurrentFact::RiskReducingExitAdmission(_)
                            | CurrentFact::ForcedReductionAdmission(_)
                    )
                })
                .count()
        }
    }

    #[derive(Debug)]
    struct RecordingMakerRuntime {
        order_factory: RefCell<OrderFactory>,
        venue_sink: RecordingVenueMutationSink,
    }

    impl RecordingMakerRuntime {
        fn new() -> Self {
            Self {
                order_factory: RefCell::new(generic_order_factory()),
                venue_sink: RecordingVenueMutationSink::default(),
            }
        }
    }

    impl BoltV3NtVenueMutationSink for RecordingMakerRuntime {
        fn actor_time_ns(&mut self) -> Result<u64> {
            self.venue_sink.actor_time_ns()
        }

        fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
            self.venue_sink.cached_order(client_order_id)
        }

        fn query_order_via_nt(
            &mut self,
            seed: &OrderAny,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink.query_order_via_nt(seed, client_id, params)
        }

        fn submit_order_via_nt(
            &mut self,
            order: OrderAny,
            context: BoltV3SubmitContext,
        ) -> Result<()> {
            self.venue_sink.submit_order_via_nt(order, context)
        }

        fn cancel_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink
                .cancel_order_via_nt(client_order_id, client_id, params)
        }

        fn modify_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            quantity: Quantity,
            price: Price,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink
                .modify_order_via_nt(client_order_id, quantity, price, client_id, params)
        }
    }

    impl BoltV3MakerOrderRuntime for RecordingMakerRuntime {
        fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
            self.order_factory.borrow_mut()
        }
    }

    #[test]
    fn maker_submit_routes_through_shared_execution_policy_and_admission() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let mut runtime = RecordingMakerRuntime::new();
        let command = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("MAKER-YES-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should route through shared execution policy");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::submitted_for_test(
                Leg::Yes,
                InstrumentId::from("YES.INSTRUMENT"),
                ClientOrderId::from("MAKER-YES-1"),
                Price::new(0.40, 2),
                Quantity::new(2.0, 2),
            )
        );
        assert_eq!(runtime.venue_sink.submit_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.order_intents()[0].strategy_id, "maker-strategy");
        assert_eq!(
            writer.order_intents()[0].instrument_id,
            InstrumentId::from("YES.INSTRUMENT").to_string()
        );
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 1);
        assert_eq!(
            order_economics.resting_order_ids().unwrap(),
            vec![ClientOrderId::from("MAKER-YES-1")]
        );

        let mut cancel_sink = RecordingVenueMutationSink::default();
        let error = super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut cancel_sink,
            "maker_execution_client",
            vec![(ClientOrderId::from("MAKER-YES-1"), None)],
            1,
        )
        .expect_err("a missing unqueryable order must become loud without a fake cancel");
        assert!(
            error
                .to_string()
                .contains("recovery_identity_unavailable=true")
        );
        assert_eq!(cancel_sink.cancel_calls, 0);
        assert!(
            order_economics.resting_cancel_health().unwrap()[0].recovery_identity_unavailable()
        );

        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut cancel_sink,
            "maker_execution_client",
            vec![(ClientOrderId::from("MAKER-YES-1"), None)],
            2,
        )
        .expect_err("unresolved missing identity must remain loud without venue churn");
        assert_eq!(cancel_sink.cancel_calls, 0);
    }

    #[test]
    fn healthy_resting_order_survives_timer_drives_without_a_cancel_intent() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let mut runtime = RecordingMakerRuntime::new();
        let command = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("HEALTHY-MAKER-YES-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();

        let cached = runtime
            .venue_sink
            .cached_order(ClientOrderId::from("HEALTHY-MAKER-YES-1"))
            .unwrap();
        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(ClientOrderId::from("HEALTHY-MAKER-YES-1"), cached)],
            2,
        )
        .unwrap();

        assert_eq!(runtime.venue_sink.cancel_calls, 0);
        assert_eq!(runtime.venue_sink.query_calls, 0);
        assert!(order_economics.resting_cancel_health().unwrap().is_empty());
        assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);
    }

    #[test]
    fn maker_cancel_routes_through_shared_execution_policy_with_configured_client() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::No,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("MAKER-NO-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should establish tracked cancellation identity");
        let command = MakerCompiledOrderCommand::Cancel {
            leg: Leg::No,
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            client_order_id: ClientOrderId::from("MAKER-NO-1"),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker cancel should route through shared execution policy");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::Canceled {
                leg: Leg::No,
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                client_order_id: ClientOrderId::from("MAKER-NO-1"),
            }
        );
        assert_eq!(runtime.venue_sink.cancel_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
    }

    #[test]
    fn repeated_cancel_origins_merge_without_resetting_exact_retry_boundary() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-RETRY-1");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();
        runtime.venue_sink.fail_cancel_ids.insert(client_order_id);
        let cancel = MakerCompiledOrderCommand::Cancel {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            client_order_id,
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &cancel,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect_err("the first synchronous cancel failure must remain retryable");
        assert_eq!(runtime.venue_sink.cancel_calls, 1);

        let retry_timeout_ns = FIXTURE_CANCEL_RETRY_TIMEOUT_NS;
        order_economics
            .begin_resting_order_drain_at_ns(retry_timeout_ns / 2)
            .expect("a second cancellation origin must merge into the existing intent");
        let cached = runtime.venue_sink.cached_order(client_order_id).unwrap();
        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, cached.clone())],
            retry_timeout_ns,
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.cancel_calls, 1);

        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, cached)],
            retry_timeout_ns + 1,
        )
        .expect_err("the exact armed boundary should perform one bounded retry");
        assert_eq!(runtime.venue_sink.cancel_calls, 2);
    }

    #[test]
    fn partial_fill_retains_tracking_and_fill_void_recreates_cancel_only_tracking() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-FILL-VOID-1");
        let instrument_id = InstrumentId::from("YES.INSTRUMENT");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id,
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();

        let mut order = runtime
            .venue_sink
            .cached_order(client_order_id)
            .unwrap()
            .unwrap();
        let submitted = TestOrderEventStubs::submitted(&order, AccountId::from("ACCOUNT-001"));
        order.apply(submitted).unwrap();
        let accepted = TestOrderEventStubs::accepted(
            &order,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from("VENUE-FILL-VOID-1"),
        );
        order.apply(accepted).unwrap();
        let instrument = binary_option_with_max_price(instrument_id);
        let partial_fill = TestOrderEventStubs::filled(
            &order,
            &instrument,
            Some(TradeId::from("TRADE-PARTIAL-1")),
            None,
            Some(Price::new(0.40, 2)),
            Some(Quantity::new(1.0, 2)),
            Some(LiquiditySide::Maker),
            None,
            Some(UnixNanos::from(2_u64)),
            Some(AccountId::from("ACCOUNT-001")),
        );
        order.apply(partial_fill).unwrap();
        assert_eq!(order.status(), OrderStatus::PartiallyFilled);
        order_economics
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 2)
            .unwrap();
        assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);

        let full_fill = TestOrderEventStubs::filled(
            &order,
            &instrument,
            Some(TradeId::from("TRADE-FULL-1")),
            None,
            Some(Price::new(0.40, 2)),
            Some(Quantity::new(1.0, 2)),
            Some(LiquiditySide::Maker),
            None,
            Some(UnixNanos::from(3_u64)),
            Some(AccountId::from("ACCOUNT-001")),
        );
        order.apply(full_fill).unwrap();
        assert_eq!(order.status(), OrderStatus::Filled);
        order_economics
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 3)
            .unwrap();
        assert!(order_economics.resting_order_ids().unwrap().is_empty());

        let fill_voided = OrderEventAny::FillVoided(
            OrderFillVoidedSpec::builder()
                .trader_id(order.trader_id())
                .strategy_id(order.strategy_id())
                .instrument_id(instrument_id)
                .client_order_id(client_order_id)
                .venue_order_id(VenueOrderId::from("VENUE-FILL-VOID-1"))
                .account_id(AccountId::from("ACCOUNT-001"))
                .trade_id(TradeId::from("TRADE-FULL-1"))
                .voided_qty(Quantity::new(1.0, 2))
                .commission_voided(Money::from("2 USD"))
                .order_side(OrderSide::Buy)
                .order_type(OrderType::Limit)
                .last_px(Price::new(0.40, 2))
                .currency(Currency::USD())
                .liquidity_side(LiquiditySide::Maker)
                .position_id(PositionId::new("1"))
                .is_reopened(true)
                .build(),
        );
        order.apply(fill_voided).unwrap();
        assert_eq!(order.status(), OrderStatus::PartiallyFilled);
        order_economics
            .reconcile_fill_void_at(client_order_id, Some(order.clone()), 4)
            .unwrap();
        assert_eq!(
            order_economics.resting_order_ids().unwrap(),
            vec![client_order_id]
        );
        assert_eq!(
            order_economics.resting_cancel_health().unwrap()[0].liveness(),
            Some(BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded)
        );

        runtime
            .venue_sink
            .cached_orders
            .insert(client_order_id, order.clone());
        runtime.venue_sink.actor_times_ns.push_back(4);
        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, Some(order))],
            4,
        )
        .expect_err("a reopened fill routes cancellation and remains loudly past its deadline");
        assert_eq!(runtime.venue_sink.cancel_calls, 1);
    }

    #[test]
    fn captured_identity_routes_query_and_only_authoritative_cache_state_retires() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-QUERY-1");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();
        let mut accepted = runtime
            .venue_sink
            .cached_order(client_order_id)
            .unwrap()
            .unwrap();
        let submitted_event =
            TestOrderEventStubs::submitted(&accepted, AccountId::from("ACCOUNT-001"));
        accepted.apply(submitted_event).unwrap();
        let accepted_event = TestOrderEventStubs::accepted(
            &accepted,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from("VENUE-QUERY-1"),
        );
        accepted.apply(accepted_event).unwrap();
        runtime
            .venue_sink
            .cached_orders
            .insert(client_order_id, accepted.clone());
        let cancel = MakerCompiledOrderCommand::Cancel {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            client_order_id,
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &cancel,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.cancel_calls, 1);

        runtime.venue_sink.cached_orders.remove(&client_order_id);
        let retry_timeout_ns = FIXTURE_CANCEL_RETRY_TIMEOUT_NS;
        runtime
            .venue_sink
            .actor_times_ns
            .push_back(retry_timeout_ns + 1);
        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, None)],
            retry_timeout_ns + 1,
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.query_calls, 1);
        assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);

        let mut terminal = accepted;
        let canceled_event = TestOrderEventStubs::canceled(
            &terminal,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from("VENUE-QUERY-1")),
        );
        terminal.apply(canceled_event).unwrap();
        runtime
            .venue_sink
            .cached_orders
            .insert(client_order_id, terminal.clone());
        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, Some(terminal))],
            retry_timeout_ns + 2,
        )
        .unwrap();
        assert!(order_economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn one_failing_record_does_not_starve_due_siblings() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            BTreeMap::from([(
                "maker_execution_client".to_string(),
                BoltV3LiveSubmitApprovalLimits {
                    max_order_count: 2,
                    max_order_notional: Decimal::new(100, 0),
                },
            )]),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let first = ClientOrderId::from("MAKER-SIBLING-A");
        let second = ClientOrderId::from("MAKER-SIBLING-B");
        for (client_order_id, instrument_id, leg) in [
            (first, InstrumentId::from("YES.INSTRUMENT"), Leg::Yes),
            (second, InstrumentId::from("YES.INSTRUMENT"), Leg::No),
        ] {
            let submit = MakerCompiledOrderCommand::Submit {
                leg,
                template: Box::new(maker_limit_post_only_template()),
                inputs: NtOrderBuildInputs {
                    instrument_id,
                    order_side: OrderSide::Buy,
                    quantity: Quantity::new(1.0, 2),
                    price: Some(Price::new(0.40, 2)),
                    client_order_id,
                },
                fallback_price: Price::new(0.40, 2),
            };
            route_maker_order_command_with_runtime(
                BoltV3OrderExecutionPolicy::live(),
                &mut runtime,
                writer.as_ref(),
                admission.as_ref(),
                maker_routing_context(&order_economics),
                MakerOrderDispatchInput {
                    command: &submit,
                    submit_order_prefix: "maker_submit",
                },
            )
            .unwrap();
        }
        runtime.venue_sink.fail_cancel_ids.insert(first);
        order_economics.begin_resting_order_drain_at_ns(1).unwrap();
        let first_cached = runtime.venue_sink.cached_order(first).unwrap();
        let second_cached = runtime.venue_sink.cached_order(second).unwrap();

        super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(first, first_cached), (second, second_cached)],
            1,
        )
        .expect_err("one failing record must be aggregated after its sibling is processed");

        assert_eq!(runtime.venue_sink.cancel_calls, 2);
        let health = order_economics.resting_cancel_health().unwrap();
        assert_eq!(health.len(), 2);
        assert!(
            health
                .iter()
                .any(|snapshot| snapshot.client_order_id() == first)
        );
        assert!(
            health
                .iter()
                .any(|snapshot| snapshot.client_order_id() == second)
        );
    }

    #[test]
    fn cancel_health_aggregate_reports_post_settlement_facets_once_and_processes_due_siblings() {
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let conflicted_id = ClientOrderId::from("HEALTH-POST-A");
        let sibling_id = ClientOrderId::from("HEALTH-POST-B");
        let conflicted_a = accepted_limit_order("HEALTH-POST-A", "VENUE-A");
        let conflicted_b = accepted_limit_order("HEALTH-POST-A", "VENUE-B");
        let sibling = accepted_limit_order("HEALTH-POST-B", "VENUE-SIBLING");
        order_economics
            .reconcile_fill_void_at(conflicted_id, Some(conflicted_a.clone()), 100)
            .unwrap();
        order_economics
            .reconcile_fill_void_at(sibling_id, Some(sibling.clone()), 100)
            .unwrap();
        let mut sink = RecordingVenueMutationSink::default();
        sink.cached_orders
            .insert(conflicted_id, conflicted_a.clone());
        sink.cached_orders.insert(sibling_id, sibling.clone());
        sink.cancel_replacement_orders
            .insert(conflicted_id, conflicted_b);
        sink.actor_times_ns.extend([100, 100]);

        let error = super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut sink,
            "maker_execution_client",
            vec![
                (conflicted_id, Some(conflicted_a)),
                (sibling_id, Some(sibling)),
            ],
            100,
        )
        .expect_err("post-settlement health must be reported by the initiating drive");
        let message = error.to_string();

        assert_eq!(sink.cancel_calls, 2, "the due sibling must still route");
        assert!(message.contains("recovery_identity_conflict={captured=VENUE-A,observed=VENUE-B}"));
        assert!(message.contains("liveness=CancellationDeadlineExceeded"));
        assert_eq!(message.matches("client_order_id=HEALTH-POST-A").count(), 1);
    }

    #[test]
    fn synchronous_cancel_failure_settles_before_composed_health_collection() {
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("HEALTH-SYNC-FAIL");
        let order = accepted_limit_order("HEALTH-SYNC-FAIL", "VENUE-SYNC-FAIL");
        order_economics
            .reconcile_fill_void_at(client_order_id, Some(order.clone()), 100)
            .unwrap();
        let mut sink = RecordingVenueMutationSink {
            fail_actor_time: true,
            ..RecordingVenueMutationSink::default()
        };
        sink.cached_orders.insert(client_order_id, order.clone());
        sink.fail_cancel_ids.insert(client_order_id);

        let error = super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut sink,
            "maker_execution_client",
            vec![(client_order_id, Some(order))],
            100,
        )
        .expect_err("synchronous failure must settle without a post-operation observation");
        let message = error.to_string();

        assert!(message.contains("synthetic NT cancel failure: HEALTH-SYNC-FAIL"));
        assert!(message.contains("liveness=CancellationDeadlineExceeded"));
        assert!(!message.contains("synthetic actor-time failure"));
    }

    #[test]
    fn post_operation_observation_failure_settles_before_health_collection() {
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("HEALTH-OBSERVE-FAIL");
        let order = accepted_limit_order("HEALTH-OBSERVE-FAIL", "VENUE-OBSERVE-FAIL");
        order_economics
            .reconcile_fill_void_at(client_order_id, Some(order.clone()), 100)
            .unwrap();
        let mut sink = RecordingVenueMutationSink {
            fail_actor_time: true,
            ..RecordingVenueMutationSink::default()
        };
        sink.cached_orders.insert(client_order_id, order.clone());

        let error = super::tracked_order_economics::drive_observed_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut sink,
            "maker_execution_client",
            vec![(client_order_id, Some(order))],
            100,
        )
        .expect_err("post-operation observation failure must settle the armed generation");
        let message = error.to_string();

        assert_eq!(sink.cancel_calls, 1);
        assert!(message.contains("synthetic actor-time failure"));
        assert!(message.contains("liveness=CancellationDeadlineExceeded"));
    }

    #[test]
    fn empty_cancel_all_scope_does_not_reconcile_uncovered_orders() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-NO-EMPTY-SCOPE");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::No,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should establish an unrelated tracked order");
        runtime.venue_sink.cached_orders.remove(&client_order_id);

        let mismatched_scope = MakerCompiledOrderCommand::CancelAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            order_side: Some(OrderSide::Sell),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &mismatched_scope,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("an empty cancel-all scope must remain an exact no-op");

        assert_eq!(runtime.venue_sink.cancel_calls, 0);
        assert_eq!(runtime.venue_sink.query_calls, 0);
        assert!(order_economics.resting_cancel_health().unwrap().is_empty());
        assert_eq!(
            order_economics.resting_order_ids().unwrap(),
            vec![client_order_id]
        );
    }

    #[test]
    fn cancel_all_cache_failure_does_not_widen_to_uncovered_orders() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            BTreeMap::from([(
                "maker_execution_client".to_string(),
                BoltV3LiveSubmitApprovalLimits {
                    max_order_count: 2,
                    max_order_notional: Decimal::new(100, 0),
                },
            )]),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let selected_id = ClientOrderId::from("A-SELECTED-CACHE-FAIL");
        let uncovered_id = ClientOrderId::from("Z-UNCOVERED-MISSING");
        for (leg, instrument_id, client_order_id) in [
            (Leg::Yes, "YES.INSTRUMENT", selected_id),
            (Leg::No, "NO.INSTRUMENT", uncovered_id),
        ] {
            let submit = MakerCompiledOrderCommand::Submit {
                leg,
                template: Box::new(maker_limit_post_only_template()),
                inputs: NtOrderBuildInputs {
                    instrument_id: InstrumentId::from(instrument_id),
                    order_side: OrderSide::Buy,
                    quantity: Quantity::new(2.0, 2),
                    price: Some(Price::new(0.40, 2)),
                    client_order_id,
                },
                fallback_price: Price::new(0.40, 2),
            };
            route_maker_order_command_with_runtime(
                BoltV3OrderExecutionPolicy::live(),
                &mut runtime,
                writer.as_ref(),
                admission.as_ref(),
                maker_routing_context(&order_economics),
                MakerOrderDispatchInput {
                    command: &submit,
                    submit_order_prefix: "maker_submit",
                },
            )
            .expect("maker submit should establish tracked cancel-all records");
        }
        runtime.venue_sink.cached_orders.remove(&uncovered_id);
        runtime
            .venue_sink
            .fail_cached_order_once
            .insert(selected_id);

        let selected_scope = MakerCompiledOrderCommand::CancelAll {
            leg: Some(Leg::Yes),
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            order_side: Some(OrderSide::Buy),
        };
        let error = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &selected_scope,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect_err("the selected cache-read failure must remain scoped");

        assert!(error.to_string().contains("synthetic cached-order failure"));
        assert_eq!(runtime.venue_sink.cancel_calls, 0);
        assert_eq!(runtime.venue_sink.query_calls, 0);
        assert_eq!(
            order_economics
                .resting_cancel_health()
                .unwrap()
                .into_iter()
                .map(|health| health.client_order_id())
                .collect::<Vec<_>>(),
            vec![selected_id]
        );
    }

    #[test]
    fn one_side_cancel_all_marks_only_matching_records_after_nt_accepts() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::No,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("MAKER-NO-ALL-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should establish tracked cancel-all scope");
        let mismatched_side = MakerCompiledOrderCommand::CancelAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            order_side: Some(OrderSide::Sell),
        };
        let mismatched_outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &mismatched_side,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("a side-mismatched cancel-all should be a scoped no-op");
        assert_eq!(
            mismatched_outcome,
            MakerOrderDispatchOutcome::CanceledAll {
                leg: Some(Leg::No),
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: Some(OrderSide::Sell),
            }
        );
        assert_eq!(runtime.venue_sink.cancel_calls, 0);
        assert!(order_economics.resting_cancel_health().unwrap().is_empty());

        let command = MakerCompiledOrderCommand::CancelAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            order_side: Some(OrderSide::Buy),
        };
        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker cancel-all should route through shared execution policy");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::CanceledAll {
                leg: Some(Leg::No),
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: Some(OrderSide::Buy),
            }
        );
        assert_eq!(runtime.venue_sink.cancel_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
        assert_eq!(
            order_economics.resting_cancel_health().unwrap()[0].client_order_id(),
            ClientOrderId::from("MAKER-NO-ALL-1")
        );

        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("repeated cancel-all origin should merge into the existing backoff");

        assert_eq!(
            runtime.venue_sink.cancel_calls, 1,
            "a repeated cancel-all origin must not bypass coordinator backoff"
        );
    }

    #[derive(Debug, Default)]
    struct RecordingVenueMutationSink {
        actor_times_ns: std::collections::VecDeque<u64>,
        cached_orders: BTreeMap<ClientOrderId, OrderAny>,
        submit_calls: usize,
        submitted_order_quantities: Vec<Quantity>,
        cancel_calls: usize,
        query_calls: usize,
        modify_calls: usize,
        modify_requests: Vec<(ClientOrderId, Quantity, Price, Option<ClientId>)>,
        fail_submits: bool,
        fail_actor_time: bool,
        fail_cached_order_once: BTreeSet<ClientOrderId>,
        fail_cancel_ids: BTreeSet<ClientOrderId>,
        cancel_replacement_orders: BTreeMap<ClientOrderId, OrderAny>,
    }

    impl BoltV3NtVenueMutationSink for RecordingVenueMutationSink {
        fn actor_time_ns(&mut self) -> Result<u64> {
            if self.fail_actor_time {
                anyhow::bail!("synthetic actor-time failure");
            }
            Ok(self.actor_times_ns.pop_front().unwrap_or(1))
        }

        fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
            match self.fail_cached_order_once.remove(&client_order_id) {
                true => anyhow::bail!("synthetic cached-order failure: {client_order_id}"),
                false => Ok(self.cached_orders.get(&client_order_id).cloned()),
            }
        }

        fn query_order_via_nt(
            &mut self,
            _seed: &OrderAny,
            _client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.query_calls += 1;
            Ok(())
        }

        fn submit_order_via_nt(
            &mut self,
            order: OrderAny,
            _context: BoltV3SubmitContext,
        ) -> Result<()> {
            self.submit_calls += 1;
            self.submitted_order_quantities.push(order.quantity());
            if self.fail_submits {
                anyhow::bail!("synthetic NT submit failure");
            }
            self.cached_orders.insert(order.client_order_id(), order);
            Ok(())
        }

        fn cancel_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            _client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.cancel_calls += 1;
            if self.fail_cancel_ids.contains(&client_order_id) {
                anyhow::bail!("synthetic NT cancel failure: {client_order_id}");
            }
            if let Some(replacement) = self.cancel_replacement_orders.remove(&client_order_id) {
                self.cached_orders.insert(client_order_id, replacement);
            }
            Ok(())
        }

        fn modify_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            quantity: Quantity,
            price: Price,
            client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.modify_calls += 1;
            self.modify_requests
                .push((client_order_id, quantity, price, client_id));
            Ok(())
        }
    }

    #[test]
    fn live_submit_records_evidence_consumes_capacity_and_calls_nt_submit_once() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-LIVE-1");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let outcome = policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(outcome.kind(), BoltV3SubmitAttemptKind::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 1);
    }

    #[test]
    fn total_lifetime_cannot_hide_insufficient_remaining_margin() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("remaining-margin-delayed");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([90]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test_with_timing(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
                100,
                20,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(
            outcome.kind(),
            BoltV3SubmitAttemptKind::RouteValidationRejected
        );
        assert!(
            outcome
                .diagnostic()
                .unwrap()
                .contains("lacks remaining lifetime")
        );
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn source_horizon_shorter_than_remaining_margin_fails_before_evidence() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("remaining-margin-source");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([1]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test_with_timing(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
                15,
                20,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(
            outcome.kind(),
            BoltV3SubmitAttemptKind::RouteValidationRejected
        );
        assert!(
            outcome
                .diagnostic()
                .unwrap()
                .contains("lacks remaining lifetime")
        );
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn exact_remaining_margin_boundary_is_accepted() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("remaining-margin-exact");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([80]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test_with_timing(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
                100,
                20,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(outcome.kind(), BoltV3SubmitAttemptKind::PolicySkipped);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn pre_sink_clock_advance_rolls_back_permit_and_registration() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let mut runtime = RecordingMakerRuntime::new();
        runtime.venue_sink.actor_times_ns = std::collections::VecDeque::from([1, u64::MAX]);
        let command = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("remaining-margin-pre-sink"),
            },
            fallback_price: Price::new(0.40, 2),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("pre-sink rejection is a typed submit outcome");

        let MakerOrderDispatchOutcome::SubmitAttempt { transaction, .. } = outcome else {
            panic!("expected a submit attempt outcome");
        };
        let BoltV3RestingSubmitTransactionOutcome::Attempt(route) = transaction else {
            panic!("exact rollback must preserve the original route outcome");
        };
        assert_eq!(route.kind(), BoltV3SubmitAttemptKind::PreSinkRejected);
        assert!(
            route
                .diagnostic()
                .expect("rejection carries diagnostics")
                .contains("lacks remaining lifetime")
        );
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(runtime.venue_sink.submit_calls, 0);
        assert!(order_economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn actor_clock_regression_fails_before_evidence() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("actor-clock-regression");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([0]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test_with_timing(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
                100,
                20,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(
            outcome.kind(),
            BoltV3SubmitAttemptKind::RouteValidationRejected
        );
        assert!(
            outcome
                .diagnostic()
                .unwrap()
                .contains("lacks remaining lifetime")
        );
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn production_economics_route_uses_only_injected_actor_time() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("actor-time-only");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([1]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test_with_timing(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
                21,
                20,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(outcome.kind(), BoltV3SubmitAttemptKind::PolicySkipped);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
    }

    #[test]
    fn live_submit_rejected_by_latched_kill_switch_never_calls_nt() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        admission.replace_kill_switch_state(KillSwitchState::FailedManualIntervention {
            halt_id: "halt-latched".to_string(),
            reason: "operator intervention required".to_string(),
        });
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-LATCHED-1");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));

        let outcome = BoltV3OrderExecutionPolicy::live().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(outcome.kind(), BoltV3SubmitAttemptKind::AdmissionRejected);
        assert!(
            outcome
                .diagnostic()
                .unwrap()
                .contains("blocked by kill-switch state FailedManualIntervention"),
            "unexpected latched kill-switch rejection: {outcome:?}"
        );
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.rejected_entry_admissions().len(), 1);
        assert_eq!(
            writer.rejected_entry_admissions()[0].reason,
            crate::bolt_v3_current_evidence::AdmissionRejectionReason::KillSwitchLatched
        );
    }

    #[test]
    fn live_submit_failure_rolls_back_capital_admission_reservation() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer.clone(),
            capital_admission_config(),
        ));
        admission.update_capital_admission_nt_components(capital_admission_components());
        let rebuild =
            admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1);
        assert!(rebuild.accepted);

        let mut sink = RecordingVenueMutationSink {
            fail_submits: true,
            ..RecordingVenueMutationSink::default()
        };
        let order = limit_order("O-19700101-000000-001-ROLLBACK-1");
        let intent = intent_for_order(&order);
        let request = admission_evidence_submit_request_for_order(&order);
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let result = policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution-client-a")),
        );

        assert_eq!(result.kind(), BoltV3SubmitAttemptKind::SinkRejected);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(
            admission.capital_admission_live_reserved_liability(),
            Some(Decimal::ZERO)
        );
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(
            !admission.capital_admission_has_live_reservation("O-19700101-000000-001-ROLLBACK-1")
        );
    }

    #[test]
    fn live_risk_reducing_exit_clamps_submitted_quantity_to_venue_position() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );

        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order("O-19700101-000000-001-EXIT-CLAMP-1", Quantity::new(5.0, 2));
        let intent = exit_intent_for_order(&order);
        let (intent, order) =
            clamp_risk_reducing_exit_to_position_quantity(intent, order, Decimal::new(3, 0))
                .expect("risk-reducing exit should clamp before economics is sealed");
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let outcome = policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(outcome.kind(), BoltV3SubmitAttemptKind::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(3.0, 2)]);
        assert_eq!(admission.admitted_order_count(), 1);
        let records = writer.order_intents();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].quantity, Quantity::new(3.0, 2).to_string());
        assert_eq!(
            records[0].clamp_outcome,
            Some(OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            })
        );
    }

    #[test]
    fn bounded_risk_reducing_ioc_aligns_thin_depth_and_retains_exact_fill_sum() {
        let instrument = binary_option_with_quantity_rules("0.05", "0.05");
        let bid_levels = BTreeMap::from([(Price::new(0.50, 2), 2.03), (Price::new(0.60, 2), 3.0)]);
        let ask_levels = BTreeMap::from([(Price::new(0.70, 2), 100.0)]);
        let book = crate::bolt_v3_executable_cost::ExecutableBookQuote {
            best_bid: Some(0.60),
            best_ask: Some(0.70),
            bid_levels: &bid_levels,
            ask_levels: &ask_levels,
        };

        let compiled = compile_bounded_risk_reducing_ioc_for_execution(
            Venue::from("POLYMARKET"),
            &instrument,
            &book,
            OrderSide::Sell,
            Quantity::new(10.0, 2),
            2_000,
        )
        .expect("thin depth should compile to the largest aligned reduction");

        assert_eq!(compiled.quantity, Quantity::new(5.0, 2));
        assert_eq!(compiled.worst_executable_price, Price::new(0.50, 2));
        assert_eq!(
            compiled
                .retained_fill_legs
                .iter()
                .map(|leg| leg.quantity)
                .sum::<Decimal>(),
            Decimal::new(5, 0)
        );
    }

    #[test]
    fn bounded_risk_reducing_ioc_rejects_sub_increment_and_below_minimum_depth() {
        let ask_levels = BTreeMap::from([(Price::new(0.70, 2), 100.0)]);

        let sub_increment = binary_option_with_quantity_rules("0.05", "0.05");
        let sub_increment_bids = BTreeMap::from([(Price::new(0.60, 2), 0.04)]);
        let sub_increment_book = crate::bolt_v3_executable_cost::ExecutableBookQuote {
            best_bid: Some(0.60),
            best_ask: Some(0.70),
            bid_levels: &sub_increment_bids,
            ask_levels: &ask_levels,
        };
        assert!(
            compile_bounded_risk_reducing_ioc_for_execution(
                Venue::from("POLYMARKET"),
                &sub_increment,
                &sub_increment_book,
                OrderSide::Sell,
                Quantity::new(1.0, 2),
                2_000,
            )
            .expect_err("sub-increment depth must fail closed")
            .to_string()
            .contains("aligns to zero")
        );

        let below_minimum = binary_option_with_quantity_rules("0.05", "0.10");
        let below_minimum_bids = BTreeMap::from([(Price::new(0.60, 2), 0.05)]);
        let below_minimum_book = crate::bolt_v3_executable_cost::ExecutableBookQuote {
            best_bid: Some(0.60),
            best_ask: Some(0.70),
            bid_levels: &below_minimum_bids,
            ask_levels: &ask_levels,
        };
        assert!(
            compile_bounded_risk_reducing_ioc_for_execution(
                Venue::from("POLYMARKET"),
                &below_minimum,
                &below_minimum_book,
                OrderSide::Sell,
                Quantity::new(1.0, 2),
                2_000,
            )
            .expect_err("below-minimum depth must fail closed")
            .to_string()
            .contains("below instrument minimum")
        );
    }

    #[test]
    fn shared_risk_reducing_choke_seals_the_compiled_thin_book_quantity() {
        let instrument = binary_option_with_max_price(InstrumentId::from("instrument-yes.VENUE-A"));
        let order = market_exit_order("O-19700101-000000-001-THIN-IOC-1", Quantity::new(10.0, 2));
        let intent = exit_intent_for_order(&order);
        let bid_levels = BTreeMap::from([(Price::new(0.60, 2), 3.0), (Price::new(0.50, 2), 2.0)]);
        let ask_levels = BTreeMap::from([(Price::new(0.70, 2), 100.0)]);
        let position_authority = position_authority_with_canonical_position(
            &instrument,
            PositionId::from("1"),
            Quantity::new(10.0, 2),
        );

        let compiled =
            compile_and_seal_risk_reducing_ioc(BoltV3CompileAndSealRiskReducingIocInput {
                economics: kill_switch_order_economics(),
                execution_venue: Venue::from("POLYMARKET"),
                execution_client_id: "execution_client",
                instrument: &instrument,
                book: crate::bolt_v3_executable_cost::ExecutableBookQuote {
                    best_bid: Some(0.60),
                    best_ask: Some(0.70),
                    bid_levels: &bid_levels,
                    ask_levels: &ask_levels,
                },
                vwap_depth_limit_bps: 2_000,
                intent,
                requested_order: order,
                position_id: PositionId::from("1"),
                position_authority: &position_authority,
                position_side: PositionSide::Long,
                prediction_market_outcome: PredictionMarketOutcomeSide::Yes,
                stored_entry_cost_per_unit: Decimal::new(40, 2),
                requested_at_ns: 1,
                decision_correlation_id: "decision-thin-ioc",
            })
            .expect("thin depth should compile and seal one coherent five-unit submission");
        let (intent, order, sealed, executable, sealed_position_authority) = compiled.into_parts();

        assert_eq!(order.quantity(), Quantity::new(5.0, 2));
        assert_eq!(intent.quantity, Quantity::new(5.0, 2).to_string());
        assert_eq!(intent.price, Price::new(0.50, 2).to_string());
        assert_eq!(sealed.request().order_quantity, Decimal::new(5, 0));
        assert_eq!(sealed.request().notional, Decimal::new(50_375, 4));
        assert_eq!(executable.quantity, Quantity::new(5.0, 2));
        assert_eq!(
            sealed_position_authority.canonical().signed_quantity(),
            Decimal::new(10, 0),
            "the shared compiler must preserve the canonical pre-submit baseline"
        );
        assert_eq!(
            executable
                .retained_fill_legs
                .iter()
                .map(|leg| leg.quantity)
                .sum::<Decimal>(),
            Decimal::new(5, 0)
        );

        let mut terminal_order =
            accepted_exit_order("O-19700101-000000-001-THIN-IOC-1", Quantity::new(5.0, 2));
        let authority = BoltV3ExitOrderAuthorityHandle::locally_submitted(
            terminal_order.client_order_id(),
            terminal_order.instrument_id(),
            PositionId::from("1"),
            terminal_order.quantity(),
            sealed_position_authority,
        )
        .expect("the compiler-owned snapshot should construct the exit authority");
        let trade_id = apply_exit_fill(
            &mut terminal_order,
            Quantity::new(2.0, 2),
            "TRADE-THIN-IOC-1",
            100,
        );
        terminal_order
            .apply(OrderEventAny::Canceled(order_canceled_event(
                "O-19700101-000000-001-THIN-IOC-1",
                150,
            )))
            .expect("partially filled IOC should become terminal");
        assert_eq!(
            authority
                .observe_order(&terminal_order, 150, BoltV3ExitOrderCorrection::Unchanged,)
                .expect("terminal partial fill should arm the shared fence"),
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(8, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::from([trade_id]),
                    ),
                ))
                .expect(
                    "the compiler's ten-unit baseline should authorize the eight-unit residual"
                ),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(8, 0)
            }
        );
    }

    #[test]
    fn shared_risk_reducing_choke_rejects_ambiguous_netting_position_authority() {
        let instrument = binary_option_with_max_price(InstrumentId::from("instrument-yes.VENUE-A"));
        let order = market_exit_order(
            "O-19700101-000000-001-AMBIGUOUS-NETTING",
            Quantity::new(10.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let bid_levels = BTreeMap::from([(Price::new(0.60, 2), 10.0)]);
        let ask_levels = BTreeMap::from([(Price::new(0.70, 2), 100.0)]);
        let position_authority = position_authority_with_ambiguous_netting_positions(
            &instrument,
            PositionId::from("POSITION-TARGET"),
            PositionId::from("POSITION-OTHER"),
        );

        let result = compile_and_seal_risk_reducing_ioc(BoltV3CompileAndSealRiskReducingIocInput {
            economics: kill_switch_order_economics(),
            execution_venue: Venue::from("POLYMARKET"),
            execution_client_id: "execution_client",
            instrument: &instrument,
            book: crate::bolt_v3_executable_cost::ExecutableBookQuote {
                best_bid: Some(0.60),
                best_ask: Some(0.70),
                bid_levels: &bid_levels,
                ask_levels: &ask_levels,
            },
            vwap_depth_limit_bps: 2_000,
            intent,
            requested_order: order,
            position_id: PositionId::from("POSITION-TARGET"),
            position_authority: &position_authority,
            position_side: PositionSide::Long,
            prediction_market_outcome: PredictionMarketOutcomeSide::Yes,
            stored_entry_cost_per_unit: Decimal::new(40, 2),
            requested_at_ns: 1,
            decision_correlation_id: "decision-ambiguous-netting",
        });
        let Err(failure) = result else {
            panic!("ambiguous netting scope must fail before economics sealing");
        };
        assert_eq!(
            failure.stage(),
            super::BoltV3RiskReducingIocPreparationStage::PositionAuthority
        );
        assert!(failure.to_string().contains("ambiguous netting"));
    }

    #[test]
    fn canonical_position_shrink_after_compilation_rejects_before_seal_or_routing() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(10, 0),
        );
        let instrument = binary_option_with_max_price(InstrumentId::from("instrument-yes.VENUE-A"));
        let bid_levels = BTreeMap::from([(Price::new(0.60, 2), 3.0), (Price::new(0.50, 2), 2.0)]);
        let ask_levels = BTreeMap::from([(Price::new(0.70, 2), 100.0)]);
        let compiled = compile_bounded_risk_reducing_ioc_for_execution(
            Venue::from("POLYMARKET"),
            &instrument,
            &crate::bolt_v3_executable_cost::ExecutableBookQuote {
                best_bid: Some(0.60),
                best_ask: Some(0.70),
                bid_levels: &bid_levels,
                ask_levels: &ask_levels,
            },
            OrderSide::Sell,
            Quantity::new(10.0, 2),
            2_000,
        )
        .expect("the first canonical snapshot should compile five executable units");
        assert_eq!(compiled.quantity, Quantity::new(5.0, 2));

        let admissions_before = admission.admitted_order_count();
        let reserved_before = admission.capital_admission_live_reserved_liability();
        let mut shrunken = capital_admission_components();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &mut shrunken.product_state;
        product.source = POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string();
        product.yes_position = Decimal::new(7, 0);
        admission.update_capital_admission_nt_components(shrunken);
        let rebuild =
            admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 2);
        assert!(rebuild.accepted);
        let facts_before_seal = writer
            .recorded_facts()
            .expect("recorded evidence should decode")
            .len();

        let compile_capability = position_authority_with_canonical_position(
            &instrument,
            PositionId::from("POSITION-001"),
            Quantity::new(10.0, 2),
        );
        let compile_canonical = compile_capability
            .canonical_position(PositionId::from("POSITION-001"), instrument.id())
            .unwrap()
            .expect("compile authority should exist");
        let failure = super::require_canonical_exit_position_at_seal(
            &position_authority_with_canonical_position(
                &instrument,
                PositionId::from("POSITION-001"),
                Quantity::new(7.0, 2),
            ),
            &compile_canonical,
            PositionId::from("POSITION-001"),
            instrument.id(),
            PositionSide::Long,
            Decimal::new(10, 0),
            compiled.quantity,
        )
        .expect_err("a position shrink between compilation and sealing must fail closed");
        assert!(
            failure
                .to_string()
                .contains("canonical NT position changed after IOC compilation")
        );
        assert_eq!(admission.admitted_order_count(), admissions_before);
        assert_eq!(
            admission.capital_admission_live_reserved_liability(),
            reserved_before
        );
        assert_eq!(
            writer
                .recorded_facts()
                .expect("recorded evidence should decode")
                .len(),
            facts_before_seal,
            "the second canonical check must reject before intent or admission evidence"
        );
    }

    #[test]
    fn position_reduction_fence_rejects_mixed_projected_and_applied_fill_state() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed.clone(),
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let lease = capability
            .acquire_for_position(
                PositionId::from("1"),
                InstrumentId::from("instrument-yes.VENUE-A"),
            )
            .expect("position authority lease should acquire");
        let projected_trade = TradeId::from("PROJECTED-A");
        let applied_trade = TradeId::from("APPLIED-B");
        let fence = BoltV3PositionReductionFence::local(
            &lease,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            Decimal::new(5, 0),
            BTreeSet::from([projected_trade, applied_trade]),
            BoltV3FillSetProof::Eligible,
            100,
            0,
        )
        .expect("local reduction fence should build");
        let stale_position = super::BoltV3CanonicalPositionAuthority::exact_for_test(
            Decimal::new(7, 0),
            PositionSideSpecified::Long,
            BTreeSet::from([applied_trade]),
        );
        assert_eq!(
            fence.release(&lease, Some(&stale_position)).unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority,
            "one applied terminal fill cannot conceal another projected fill"
        );

        feed.observe(&PositionStatusReport::new(
            AccountId::from("ACCOUNT-001"),
            InstrumentId::from("instrument-yes.VENUE-A"),
            PositionSideSpecified::Long,
            Quantity::new(5.0, 2),
            UnixNanos::from(200_u64),
            UnixNanos::from(200_u64),
            None,
            None,
            None,
        ))
        .expect("post-terminal position report should be observed");
        let converged_position = super::BoltV3CanonicalPositionAuthority::exact_for_test(
            Decimal::new(5, 0),
            PositionSideSpecified::Long,
            BTreeSet::from([projected_trade, applied_trade]),
        );
        assert_eq!(
            fence.release(&lease, Some(&converged_position)).unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(5, 0)
            }
        );
    }

    #[test]
    fn position_reduction_fence_rejects_ambiguous_netting_aggregate() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed,
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let lease = capability
            .acquire_for_position(
                PositionId::from("1"),
                InstrumentId::from("instrument-yes.VENUE-A"),
            )
            .expect("position authority lease should acquire");
        let trade_id = TradeId::from("AMBIGUOUS-NETTING-FILL");
        let fence = BoltV3PositionReductionFence::local(
            &lease,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            Decimal::new(2, 0),
            BTreeSet::from([trade_id]),
            BoltV3FillSetProof::Eligible,
            100,
            0,
        )
        .expect("local reduction fence should build");

        assert_eq!(
            fence
                .release(
                    &lease,
                    Some(&BoltV3CanonicalPositionAuthority::ambiguous_for_test(
                        Decimal::new(8, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::from([trade_id]),
                    )),
                )
                .unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority,
            "a matching target position cannot release an account/instrument aggregate shared by multiple netting positions"
        );
    }

    #[test]
    fn position_reduction_fence_surfaces_stale_health_until_new_authority_arrives() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed.clone(),
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");
        let lease = capability
            .acquire_for_position(PositionId::new("1"), instrument_id)
            .expect("position authority lease should acquire");
        let trade_id = TradeId::from("TRADE-STALE-AUTHORITY");
        let fence = BoltV3PositionReductionFence::local(
            &lease,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            Decimal::new(2, 0),
            BTreeSet::from([trade_id]),
            BoltV3FillSetProof::Eligible,
            150,
            0,
        )
        .expect("local reduction fence should build");
        let report = |ts_last| {
            PositionStatusReport::new(
                AccountId::from("ACCOUNT-001"),
                instrument_id,
                PositionSideSpecified::Long,
                Quantity::new(8.0, 2),
                UnixNanos::from(ts_last),
                UnixNanos::from(ts_last),
                None,
                None,
                None,
            )
        };
        feed.observe(&report(200))
            .expect("new authority should be observed");
        feed.observe(&report(100))
            .expect("stale authority should produce health rather than replace state");
        let canonical = BoltV3CanonicalPositionAuthority::exact_for_test(
            Decimal::new(8, 0),
            PositionSideSpecified::Long,
            BTreeSet::from([trade_id]),
        );
        assert!(
            fence
                .release(&lease, Some(&canonical))
                .expect_err("stale authority must be loud")
                .to_string()
                .contains("stale")
        );

        feed.observe(&report(300))
            .expect("new coherent authority should clear stale health");
        assert_eq!(
            fence.release(&lease, Some(&canonical)).unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(8, 0)
            }
        );
    }

    #[test]
    fn local_exit_authority_tracks_partial_fill_and_requires_causal_position_state() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed,
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");
        let position_id = PositionId::new("1");
        let lease = capability
            .acquire_for_position(position_id, instrument_id)
            .expect("local exit lease should acquire");
        let authority = BoltV3ExitOrderAuthorityHandle::locally_submitted_for_test(
            ClientOrderId::from("EXIT-LOCAL-1"),
            instrument_id,
            position_id,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            Quantity::new(5.0, 2),
            lease,
        )
        .expect("local exit authority should build");
        let (mut order, trade_id) = partially_filled_exit_order(
            "EXIT-LOCAL-1",
            Quantity::new(5.0, 2),
            Quantity::new(2.0, 2),
            "TRADE-LOCAL-1",
            100,
        );

        assert_eq!(
            authority
                .observe_order(&order, 100, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::Working
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(10, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::new(),
                    ),
                ))
                .unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority
        );

        order
            .apply(OrderEventAny::Canceled(order_canceled_event(
                "EXIT-LOCAL-1",
                150,
            )))
            .expect("partial exit should accept terminal cancellation");
        assert_eq!(
            authority
                .observe_order(&order, 150, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(10, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::new(),
                    ),
                ))
                .unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority,
            "an order-only projected fill must not release against the stale position"
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(8, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::from([trade_id]),
                    ),
                ))
                .unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(8, 0)
            }
        );
    }

    #[test]
    fn local_denied_exit_with_authoritative_zero_fill_remanages_without_a_fence() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed,
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");
        let position_id = PositionId::new("1");
        let lease = capability
            .acquire_for_position(position_id, instrument_id)
            .expect("local exit lease should acquire");
        let authority = BoltV3ExitOrderAuthorityHandle::locally_submitted_for_test(
            ClientOrderId::from("EXIT-DENIED-ZERO-FILL"),
            instrument_id,
            position_id,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            Quantity::new(5.0, 2),
            lease,
        )
        .expect("local exit authority should build");
        let mut order = market_exit_order("EXIT-DENIED-ZERO-FILL", Quantity::new(5.0, 2));
        order
            .apply(OrderEventAny::Denied(OrderDenied::new(
                order.trader_id(),
                order.strategy_id(),
                instrument_id,
                order.client_order_id(),
                "venue denied before submission".into(),
                nautilus_core::UUID4::new(),
                UnixNanos::from(100_u64),
                UnixNanos::from(100_u64),
            )))
            .expect("initialized exit order should accept denial");

        assert_eq!(order.status(), OrderStatus::Denied);
        assert_eq!(
            authority
                .observe_order(&order, 100, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::TerminalZeroFill
        );
    }

    #[test]
    fn recovered_exit_coherent_baseline_uses_only_post_baseline_fills() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed.clone(),
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");
        let position_id = PositionId::new("1");
        let lease = capability
            .acquire_for_position(PositionId::new("1"), instrument_id)
            .expect("recovered exit lease should acquire");
        feed.observe(&PositionStatusReport::new(
            AccountId::from("ACCOUNT-001"),
            instrument_id,
            PositionSideSpecified::Long,
            Quantity::new(10.0, 2),
            UnixNanos::from(50_u64),
            UnixNanos::from(50_u64),
            None,
            None,
            None,
        ))
        .expect("recovered baseline report should be observed");
        let mut order = accepted_exit_order("EXIT-RECOVERED-1", Quantity::new(5.0, 2));
        let authority = BoltV3ExitOrderAuthorityHandle::recovered_for_test(
            BoltV3RecoveredExitCause::StartupAdoption,
            order.client_order_id(),
            instrument_id,
            position_id,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            &order,
            lease,
        )
        .expect("recovered exit authority should build");
        authority
            .refresh_recovered_baseline_with_canonical_for_test(Some(
                &BoltV3CanonicalPositionAuthority::exact_for_test(
                    Decimal::new(10, 0),
                    PositionSideSpecified::Long,
                    BTreeSet::new(),
                ),
            ))
            .expect("recovered baseline should become coherent before terminality");
        let trade_id = apply_exit_fill(&mut order, Quantity::new(2.0, 2), "TRADE-RECOVERED-1", 100);
        assert_eq!(
            authority
                .observe_order(&order, 100, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::Working
        );
        order
            .apply(OrderEventAny::Canceled(order_canceled_event(
                "EXIT-RECOVERED-1",
                150,
            )))
            .expect("recovered partial exit should accept cancellation");
        assert_eq!(
            authority
                .observe_order(&order, 150, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(8, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::from([trade_id]),
                    ),
                ))
                .unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(8, 0)
            }
        );
    }

    #[test]
    fn recovered_exit_terminal_before_baseline_requires_post_terminal_report() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed.clone(),
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");
        let position_id = PositionId::new("1");
        let lease = capability
            .acquire_for_position(position_id, instrument_id)
            .expect("recovered exit lease should acquire");
        let mut order = accepted_exit_order("EXIT-RECOVERED-2", Quantity::new(5.0, 2));
        let authority = BoltV3ExitOrderAuthorityHandle::recovered_for_test(
            BoltV3RecoveredExitCause::StartupAdoption,
            order.client_order_id(),
            instrument_id,
            position_id,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            &order,
            lease,
        )
        .expect("recovered exit authority should build without a report");
        let trade_id = apply_exit_fill(&mut order, Quantity::new(2.0, 2), "TRADE-RECOVERED-2", 100);
        order
            .apply(OrderEventAny::Canceled(order_canceled_event(
                "EXIT-RECOVERED-2",
                150,
            )))
            .expect("recovered partial exit should accept cancellation");
        assert_eq!(
            authority
                .observe_order(&order, 150, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
        );
        let converged = BoltV3CanonicalPositionAuthority::exact_for_test(
            Decimal::new(8, 0),
            PositionSideSpecified::Long,
            BTreeSet::from([trade_id]),
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(&converged))
                .unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority,
            "an exit terminal before baseline cannot use the fill-set shortcut"
        );

        feed.observe(&PositionStatusReport::new(
            AccountId::from("ACCOUNT-001"),
            instrument_id,
            PositionSideSpecified::Long,
            Quantity::new(8.0, 2),
            UnixNanos::from(200_u64),
            UnixNanos::from(200_u64),
            None,
            None,
            None,
        ))
        .expect("post-terminal position report should be observed");
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(&converged))
                .unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(8, 0)
            }
        );
    }

    #[test]
    fn fill_void_reopen_advances_fence_and_cannot_reuse_fill_set_proof() {
        let feed = BoltV3PositionAuthorityFeed::try_new([(
            AccountId::from("ACCOUNT-001"),
            ClientId::from("execution_client"),
            Venue::from("VENUE-A"),
        )])
        .expect("position authority fixture should build");
        let capability = BoltV3PositionAuthorityCapability::new(
            feed.clone(),
            ClientId::from("execution_client"),
            AccountId::from("ACCOUNT-001"),
            OmsType::Netting,
        );
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");
        let position_id = PositionId::new("1");
        let lease = capability
            .acquire_for_position(position_id, instrument_id)
            .expect("fill-void exit lease should acquire");
        let authority = BoltV3ExitOrderAuthorityHandle::locally_submitted_for_test(
            ClientOrderId::from("EXIT-FILL-VOID-1"),
            instrument_id,
            position_id,
            Decimal::new(10, 0),
            PositionSideSpecified::Long,
            Quantity::new(5.0, 2),
            lease,
        )
        .expect("local exit authority should build");
        let mut order = accepted_exit_order("EXIT-FILL-VOID-1", Quantity::new(5.0, 2));
        let first_trade =
            apply_exit_fill(&mut order, Quantity::new(5.0, 2), "TRADE-FILL-VOID-1", 100);
        assert_eq!(
            authority
                .observe_order(&order, 100, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(5, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::from([first_trade]),
                    ),
                ))
                .unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(5, 0)
            }
        );

        let fill_voided = OrderEventAny::FillVoided(
            OrderFillVoidedSpec::builder()
                .trader_id(order.trader_id())
                .strategy_id(order.strategy_id())
                .instrument_id(instrument_id)
                .client_order_id(order.client_order_id())
                .venue_order_id(VenueOrderId::from("venue-order-1"))
                .account_id(AccountId::from("ACCOUNT-001"))
                .trade_id(first_trade)
                .voided_qty(Quantity::new(5.0, 2))
                .commission_voided(Money::from("2 USD"))
                .order_side(OrderSide::Sell)
                .order_type(OrderType::Limit)
                .last_px(Price::new(0.50, 2))
                .currency(Currency::USD())
                .liquidity_side(LiquiditySide::Taker)
                .position_id(position_id)
                .is_reopened(true)
                .ts_event(UnixNanos::from(150_u64))
                .ts_init(UnixNanos::from(150_u64))
                .build(),
        );
        order
            .apply(fill_voided)
            .expect("terminal exit should reopen after fill void");
        assert_eq!(
            authority
                .observe_order(&order, 150, BoltV3ExitOrderCorrection::FillAuthorityChanged,)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::Working
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(
                    &BoltV3CanonicalPositionAuthority::exact_for_test(
                        Decimal::new(10, 0),
                        PositionSideSpecified::Long,
                        BTreeSet::new(),
                    ),
                ))
                .unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority
        );

        let second_trade =
            apply_exit_fill(&mut order, Quantity::new(5.0, 2), "TRADE-FILL-VOID-2", 200);
        assert_eq!(
            authority
                .observe_order(&order, 200, BoltV3ExitOrderCorrection::Unchanged)
                .unwrap(),
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
        );
        let converged = BoltV3CanonicalPositionAuthority::exact_for_test(
            Decimal::new(5, 0),
            PositionSideSpecified::Long,
            BTreeSet::from([second_trade]),
        );
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(&converged))
                .unwrap(),
            BoltV3PositionReductionRelease::AwaitingAuthority,
            "a corrected authority cannot take the fill-set shortcut"
        );
        feed.observe(&PositionStatusReport::new(
            AccountId::from("ACCOUNT-001"),
            instrument_id,
            PositionSideSpecified::Long,
            Quantity::new(5.0, 2),
            UnixNanos::from(300_u64),
            UnixNanos::from(300_u64),
            None,
            None,
            None,
        ))
        .expect("post-correction report should be observed");
        assert_eq!(
            authority
                .release_with_canonical_for_test(Some(&converged))
                .unwrap(),
            BoltV3PositionReductionRelease::Residual {
                signed_quantity: Decimal::new(5, 0)
            }
        );
    }

    #[test]
    fn risk_reducing_intent_evidence_failure_is_a_typed_non_submitted_outcome() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        writer.fail_purpose_on_attempt_for_test(
            crate::bolt_v3_current_evidence::CurrentEvidenceTestPurpose::RiskReducingExitOrderIntent,
            1,
        );
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order(
            "O-19700101-000000-001-EXIT-EVIDENCE-FAILURE-1",
            Quantity::new(3.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let (intent, order) =
            clamp_risk_reducing_exit_to_position_quantity(intent, order, Decimal::new(3, 0))
                .expect("within-bounds exit should be sealed after clamp evaluation");
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );

        let outcome = BoltV3OrderExecutionPolicy::live().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(
            outcome.kind(),
            BoltV3SubmitAttemptKind::IntentEvidenceRejected
        );
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(
            writer.order_intents().is_empty(),
            "the targeted order-intent write must fail before appending evidence"
        );
    }

    #[test]
    fn entry_intent_evidence_failure_is_a_typed_non_submitted_outcome() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        writer.fail_purpose_on_attempt_for_test(
            crate::bolt_v3_current_evidence::CurrentEvidenceTestPurpose::EntryOrderIntent,
            1,
        );
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-ENTRY-EVIDENCE-FAILURE-1");

        let outcome = BoltV3OrderExecutionPolicy::live().route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent_for_order(&order),
                submit_request_for_order(&order, Decimal::new(50, 0)),
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(
            outcome.kind(),
            BoltV3SubmitAttemptKind::IntentEvidenceRejected
        );
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(writer.order_intents().is_empty());
    }

    #[test]
    fn shadow_submit_records_evidence_without_consuming_capacity_or_calling_nt_submit() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-SHADOW-1");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Shadow);

        let outcome = policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
        );

        assert_eq!(outcome.kind(), BoltV3SubmitAttemptKind::PolicySkipped);
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 0);
    }

    #[test]
    fn live_and_shadow_cancel_route_through_the_same_policy_boundary() {
        let mut sink = RecordingVenueMutationSink::default();
        let live_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);
        let shadow_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Shadow);

        let live_outcome = live_policy
            .route_cancel_with_sink(
                &mut sink,
                ClientOrderId::from("O-19700101-000000-001-CANCEL-1"),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("live cancel should call NT");
        let shadow_outcome = shadow_policy
            .route_cancel_with_sink(
                &mut sink,
                ClientOrderId::from("O-19700101-000000-001-CANCEL-2"),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("shadow cancel should be suppressed by policy");

        assert_eq!(live_outcome, BoltV3CancelRoutingOutcome::Canceled);
        assert_eq!(shadow_outcome, BoltV3CancelRoutingOutcome::SkippedByPolicy);
        assert_eq!(sink.cancel_calls, 1);
    }

    #[test]
    fn live_modify_is_fail_closed_and_shadow_is_suppressed() {
        // Option A (#835): an in-place modify does NOT pass submit admission, so a
        // Live amend would bypass the risk gate. The Live arm is FAIL-CLOSED — it
        // returns `Err` and never reaches the venue; Shadow stays suppressed
        // (`SkippedByPolicy`, no NT call). Asserts through the recording sink's
        // `modify_calls` side-effect channel (stays 0 for BOTH arms), not just the
        // return value — forcing the Live arm back to a venue call turns this red.
        let mut sink = RecordingVenueMutationSink::default();
        let live_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);
        let shadow_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Shadow);

        let live_result = live_policy.route_modify_with_sink(
            &mut sink,
            ClientOrderId::from("O-19700101-000000-001-MODIFY-1"),
            Quantity::new(2.0, 2),
            Price::new(0.41, 2),
            Some(ClientId::from("execution_client")),
            None,
        );
        let shadow_outcome = shadow_policy
            .route_modify_with_sink(
                &mut sink,
                ClientOrderId::from("O-19700101-000000-001-MODIFY-2"),
                Quantity::new(3.0, 2),
                Price::new(0.42, 2),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("shadow modify should be suppressed by policy");

        assert!(
            live_result.is_err(),
            "live in-place modify must be fail-closed (not admission-gated; #835)"
        );
        assert_eq!(shadow_outcome, BoltV3ModifyRoutingOutcome::SkippedByPolicy);
        // Neither arm reached the venue: Live refused (fail-closed), Shadow suppressed.
        assert_eq!(sink.modify_calls, 0);
        assert!(sink.modify_requests.is_empty());
    }

    #[test]
    fn maker_modify_dispatch_is_fail_closed_in_live_not_admission_gated() {
        // Option A (#835): a compiled `Modify` routed Live is FAIL-CLOSED at the
        // execution seam — the dispatch returns `Err` and the venue modify is never
        // called (`modify_calls` stays 0), because an in-place modify does not pass
        // the submit admission/reservation/fee checks. The maker requotes via the
        // already-admitted cancel+resubmit path (the deployed venue contract has
        // `supports_modify=false`, so the FSM never emits a Modify). No venue mutation
        // occurs, so no intent/admission is recorded. Forcing the Live arm back to a
        // venue call turns this red.
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let command = MakerCompiledOrderCommand::Modify {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
            price: Price::new(0.41, 2),
            quantity: Quantity::new(2.0, 2),
        };

        let result = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        );

        assert!(
            result.is_err(),
            "live maker modify dispatch must be fail-closed (not admission-gated; #835)"
        );
        assert_eq!(runtime.venue_sink.modify_calls, 0);
        // No venue mutation → no order intent / admission recorded.
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
    }

    #[test]
    fn maker_modify_dispatch_in_shadow_suppresses_the_venue_modify() {
        // The Shadow arm of the same dispatch path: the dispatcher still reports the
        // `Modified` command shape, but the execution policy suppresses the venue
        // call, so `modify_calls` stays 0. Pre-fix the path bailed in BOTH modes; a
        // shadow run that leaked a venue modify (counter > 0) also fails here.
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let command = MakerCompiledOrderCommand::Modify {
            leg: Leg::No,
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            client_order_id: ClientOrderId::from("MAKER-NO-1"),
            price: Price::new(0.39, 2),
            quantity: Quantity::new(1.0, 2),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::shadow(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker modify should route in shadow without bailing");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::Modified {
                leg: Leg::No,
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                client_order_id: ClientOrderId::from("MAKER-NO-1"),
                price: Price::new(0.39, 2),
                quantity: Quantity::new(1.0, 2),
            }
        );
        assert_eq!(
            runtime.venue_sink.modify_calls, 0,
            "shadow mode must not leak a venue modify"
        );
    }

    fn live_submit_cap() -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
        BTreeMap::from([(
            "execution_client".to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count: 1,
                max_order_notional: Decimal::new(100, 0),
            },
        )])
    }

    fn live_submit_cap_for_client(
        client_id: &str,
    ) -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
        BTreeMap::from([(
            client_id.to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count: 1,
                max_order_notional: Decimal::new(100, 0),
            },
        )])
    }

    fn intent_for_order(order: &OrderAny) -> OrderIntentDetails {
        order_intent_details_from_compiled_order(
            "strategy-a".to_string(),
            "0.50".to_string(),
            order,
        )
    }

    fn exit_intent_for_order(order: &OrderAny) -> OrderIntentDetails {
        order_intent_details_from_compiled_order(
            "strategy-a".to_string(),
            "0.50".to_string(),
            order,
        )
    }

    fn submit_request_for_order(
        order: &OrderAny,
        notional: Decimal,
    ) -> BoltV3SubmitAdmissionRequest {
        BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional,
            order_side: OrderSide::Buy,
            order_quantity: Decimal::new(1, 0),
            intent_kind: BoltV3SubmitIntentKind::Entry,
            risk_reducing_exit_proof: None,
            kill_switch_forced_reduction: None,
            admission_evidence: None,
        }
    }

    fn admission_evidence_submit_request_for_order(
        order: &OrderAny,
    ) -> BoltV3SubmitAdmissionRequest {
        let mut request = submit_request_for_order(order, Decimal::new(4, 0));
        request.admission_evidence = Some(BoltV3CompiledOrderAdmissionEvidence {
            venue_id: "VENUE-A".to_string(),
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side: BoltV3CompiledOrderSide::Buy,
            quantity: Decimal::new(1, 0),
            effective_price: Decimal::new(40, 2),
            order_kind: BoltV3CompiledOrderKind::Limit,
            liquidity: BoltV3CompiledOrderLiquidity::Taker,
            quote_set_id: None,
            prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
        });
        request.instrument_id = "instrument-yes.VENUE-A".to_string();
        request.execution_client_id = "execution-client-a".to_string();
        request
    }

    fn risk_reducing_exit_submit_request_for_order(
        order: &OrderAny,
        order_quantity: Decimal,
        position_quantity: Decimal,
    ) -> BoltV3SubmitAdmissionRequest {
        let notional = order
            .price()
            .expect("risk-reducing test order must have a limit price")
            .as_decimal()
            .checked_mul(order_quantity)
            .expect("risk-reducing test notional must not overflow");
        BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional,
            order_side: OrderSide::Sell,
            order_quantity,
            intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
            risk_reducing_exit_proof: Some(BoltV3RiskReducingExitProof {
                position_id: "POSITION-001".to_string(),
                instrument_id: order.instrument_id().to_string(),
                position_side: PositionSide::Long,
                exit_order_side: OrderSide::Sell,
                position_quantity,
                exit_quantity: order_quantity,
            }),
            kill_switch_forced_reduction: None,
            admission_evidence: Some(BoltV3CompiledOrderAdmissionEvidence {
                venue_id: "VENUE-A".to_string(),
                product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
                side: BoltV3CompiledOrderSide::Sell,
                quantity: order_quantity,
                effective_price: Decimal::new(50, 2),
                order_kind: BoltV3CompiledOrderKind::Limit,
                liquidity: BoltV3CompiledOrderLiquidity::Taker,
                quote_set_id: None,
                prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
            }),
        }
    }

    fn generic_order_factory() -> OrderFactory {
        let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
        OrderFactory::new(
            TraderId::new("TRADER-001"),
            StrategyId::new("maker-strategy"),
            None,
            None,
            clock,
            false,
            true,
        )
    }

    fn maker_limit_post_only_template() -> NtOrderTemplate {
        NtOrderTemplate {
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            expire_time: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: true,
            is_reduce_only: false,
            is_quote_quantity: false,
        }
    }

    fn maker_routing_context(
        order_economics: &super::BoltV3OrderEconomicsHandle,
    ) -> BoltV3MakerOrderRoutingContext<'_> {
        BoltV3MakerOrderRoutingContext {
            strategy_id: "maker-strategy",
            execution_client_id: "maker_execution_client",
            order_economics,
            terminal_value_entry: Some(
                BoltV3TerminalValueEntry::try_new(
                    Decimal::ONE,
                    BoltV3TerminalValueEntryPolicy::Breakeven,
                )
                .expect("maker terminal value should construct"),
            ),
        }
    }

    fn kill_switch_order_economics() -> &'static super::BoltV3OrderEconomicsHandle {
        static HANDLE: std::sync::OnceLock<super::BoltV3OrderEconomicsHandle> =
            std::sync::OnceLock::new();
        HANDLE.get_or_init(|| {
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client")
        })
    }

    fn capital_admission_config() -> BoltV3SubmitCapitalAdmissionConfig {
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "test-capital-pool".to_string(),
                observed_at_ns: 0,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: u64::MAX,
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
            },
        }
    }

    fn capital_admission_components() -> BoltV3SubmitCapitalAdmissionNtComponents {
        BoltV3SubmitCapitalAdmissionNtComponents {
            source: "nt_capital_admission_state".to_string(),
            observed_at_ns: 0,
            portfolio: PortfolioCapitalAdmissionSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            provider_collateral_allowance: ProviderCollateralAllowanceSnapshot {
                source: "nt_account_free_collateral".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                collateral_allowance: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 0,
                open_order_count: 0,
                all_open_orders_attributed: true,
            },
            product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
                PredictionMarketAdmissionSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns: 0,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::new(100, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            loss_snapshot: None,
        }
    }

    fn provider_collateral_allowance_admission_with_yes_position(
        writer: Arc<DecisionEvidenceRecorder>,
        yes_position: Decimal,
    ) -> Arc<BoltV3SubmitAdmissionState> {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer,
            capital_admission_config(),
        ));
        let mut components = capital_admission_components();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) =
            &mut components.product_state;
        product.source = POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string();
        product.yes_position = yes_position;
        admission.update_capital_admission_nt_components(components);
        let rebuild =
            admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1);
        assert!(rebuild.accepted);
        admission
    }

    fn order_canceled_event(client_order_id: &str, ts_event: u64) -> OrderCanceled {
        OrderCanceled::new(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from("instrument-yes.VENUE-A"),
            ClientOrderId::from(client_order_id),
            nautilus_core::UUID4::new(),
            UnixNanos::from(ts_event),
            UnixNanos::from(ts_event),
            false,
            Some(VenueOrderId::from("venue-order-1")),
            Some(AccountId::from("ACCOUNT-001")),
        )
    }

    fn binary_option_with_max_price(instrument_id: InstrumentId) -> InstrumentAny {
        InstrumentAny::BinaryOption(BinaryOption::new(
            instrument_id,
            Symbol::from("instrument-yes"),
            AssetClass::Alternative,
            Currency::USD(),
            UnixNanos::from(1_u64),
            UnixNanos::from(2_u64),
            2,
            2,
            Price::from("0.01"),
            Quantity::from("0.01"),
            Some(Ustr::from("YES")),
            None,
            None,
            Some(Quantity::from("0.01")),
            None,
            None,
            Some(Price::from("1.00")),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            UnixNanos::from(1_u64),
            UnixNanos::from(1_u64),
        ))
    }

    fn binary_option_with_quantity_rules(
        size_increment: &str,
        minimum_quantity: &str,
    ) -> InstrumentAny {
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from("RISK-REDUCTION.POLYMARKET"),
            Symbol::from("RISK-REDUCTION"),
            AssetClass::Alternative,
            Currency::USD(),
            UnixNanos::from(1_u64),
            UnixNanos::from(2_u64),
            2,
            2,
            Price::from("0.01"),
            Quantity::from(size_increment),
            Some(Ustr::from("YES")),
            None,
            None,
            Some(Quantity::from(minimum_quantity)),
            None,
            None,
            Some(Price::from("1.00")),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            UnixNanos::from(1_u64),
            UnixNanos::from(1_u64),
        ))
    }

    fn limit_order(client_order_id: &str) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("INSTRUMENT.SOURCE"),
                ClientOrderId::from(client_order_id),
                OrderSide::Buy,
                Quantity::new(1.0, 2),
                Price::new(0.50, 2),
                TimeInForce::Gtc,
                None,
                false,
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        )
    }

    fn accepted_limit_order(client_order_id: &str, venue_order_id: &str) -> OrderAny {
        let mut order = limit_order(client_order_id);
        let submitted = TestOrderEventStubs::submitted(&order, AccountId::from("ACCOUNT-001"));
        order.apply(submitted).unwrap();
        let accepted = TestOrderEventStubs::accepted(
            &order,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from(venue_order_id),
        );
        order.apply(accepted).unwrap();
        order
    }

    fn post_only_limit_order(client_order_id: &str) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("INSTRUMENT.SOURCE"),
                ClientOrderId::from(client_order_id),
                OrderSide::Buy,
                Quantity::new(1.0, 2),
                Price::new(0.50, 2),
                TimeInForce::Gtc,
                None,
                true,
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("post-only limit order should be valid"),
        )
    }

    #[test]
    fn edge_candidate_and_final_entry_share_terminal_value_scenario() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let terminal_value_entry = BoltV3TerminalValueEntry::try_new(
            Decimal::new(7, 1),
            BoltV3TerminalValueEntryPolicy::Breakeven,
        )
        .expect("terminal value should construct");
        let candidate_fill_levels = vec![BoltV3PlannedFillLeg {
            price: Decimal::new(5, 1),
            quantity: Decimal::ONE,
        }];
        let sizing = economics
            .quote_taker_sizing(BoltV3TakerEconomicsSizingInput {
                instrument_id: InstrumentId::from("INSTRUMENT.SOURCE"),
                order_side: OrderSide::Buy,
                planned_fill_legs: candidate_fill_levels.clone(),
                terminal_value_entry: terminal_value_entry.clone(),
                requested_at_ns: 1,
                decision_correlation_id: "edge-candidate",
            })
            .expect("candidate sizing should quote from terminal value");
        let order = limit_order("edge-final-entry");
        let intent = intent_for_order(&order);
        let sealed = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    terminal_value_entry,
                ),
                candidate_fill_levels,
                requested_at_ns: 1,
                decision_correlation_id: "edge-final",
            },
        )
        .expect("final entry should seal from the same terminal value");

        assert_eq!(sizing.net_edge().gross_expected_value, Decimal::new(2, 1));
        assert_eq!(
            sealed.economics().net_edge().gross_expected_value,
            Decimal::new(2, 1)
        );
        assert_eq!(sealed.request().intent_kind, BoltV3SubmitIntentKind::Entry);
        assert_eq!(
            sealed.economics().request().lifecycle_path,
            LifecyclePath::HoldToRedemption
        );
        assert_eq!(
            sealed.economics().request().liquidity_role,
            LiquidityRole::Taker
        );
        assert_eq!(
            sealed.economics().purpose(),
            EconomicsAdmissionPurpose::TradingEdge
        );
        assert_eq!(
            sealed.economics().order_binding(),
            &economics_order_binding(&order).expect("final order should bind")
        );
    }

    #[test]
    fn maker_submit_derives_gross_from_terminal_value_and_final_order() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("maker-terminal-entry");
        let intent = intent_for_order(&order);
        let sealed = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    BoltV3TerminalValueEntry::try_new(
                        Decimal::new(7, 1),
                        BoltV3TerminalValueEntryPolicy::Breakeven,
                    )
                    .expect("terminal value should construct"),
                ),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: Decimal::new(5, 1),
                    quantity: Decimal::ONE,
                }],
                requested_at_ns: 1,
                decision_correlation_id: "maker-terminal-entry",
            },
        )
        .expect("maker entry should seal from terminal value");

        assert_eq!(
            sealed.economics().net_edge().gross_expected_value,
            Decimal::new(2, 1)
        );
        assert_eq!(
            sealed.economics().request().liquidity_role,
            LiquidityRole::GuaranteedMaker
        );
        assert_eq!(sealed.request().intent_kind, BoltV3SubmitIntentKind::Entry);
        assert_eq!(
            sealed.economics().request().lifecycle_path,
            LifecyclePath::HoldToRedemption
        );
        assert_eq!(
            sealed.economics().purpose(),
            EconomicsAdmissionPurpose::TradingEdge
        );

        let rejected = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    BoltV3TerminalValueEntry::try_new(
                        Decimal::new(4, 1),
                        BoltV3TerminalValueEntryPolicy::Breakeven,
                    )
                    .expect("negative-gross terminal value should remain a valid scenario"),
                ),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: Decimal::new(5, 1),
                    quantity: Decimal::ONE,
                }],
                requested_at_ns: 1,
                decision_correlation_id: "maker-negative-terminal-entry",
            },
        )
        .expect_err("maker breakeven policy must reject negative terminal gross");
        assert!(
            rejected
                .to_string()
                .contains("does not exceed required minimum 0"),
            "{rejected:#}"
        );
    }

    #[test]
    fn forced_reduction_derives_zero_gross_and_risk_reduction_purpose() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = limit_exit_order("forced-reduction-scenario", Quantity::new(1.0, 2));
        let intent = exit_intent_for_order(&order);
        let position = economics
            .planned_exit_position(
                PositionId::from("POSITION-001"),
                PositionSide::Long,
                Decimal::ONE,
            )
            .expect("forced-reduction position should construct");
        let sealed = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::forced_reduction(position)
                    .expect("forced-reduction scenario should construct"),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: Decimal::new(5, 1),
                    quantity: Decimal::ONE,
                }],
                requested_at_ns: 1,
                decision_correlation_id: "forced-reduction-scenario",
            },
        )
        .expect("forced reduction should seal without caller-selected gross or lifecycle");

        assert_eq!(
            sealed.economics().net_edge().gross_expected_value,
            Decimal::ZERO
        );
        assert_eq!(
            sealed.request().intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
        assert_eq!(
            sealed.economics().request().lifecycle_path,
            LifecyclePath::PlannedExit
        );
        assert_eq!(
            sealed.economics().purpose(),
            EconomicsAdmissionPurpose::RiskReduction
        );
    }

    fn limit_exit_order(client_order_id: &str, quantity: Quantity) -> OrderAny {
        limit_exit_order_for_instrument(
            client_order_id,
            InstrumentId::from("instrument-yes.VENUE-A"),
            quantity,
        )
    }

    fn accepted_exit_order(client_order_id: &str, quantity: Quantity) -> OrderAny {
        let mut order = limit_exit_order(client_order_id, quantity);
        order
            .apply(TestOrderEventStubs::submitted(
                &order,
                AccountId::from("ACCOUNT-001"),
            ))
            .expect("exit order should accept submitted state");
        order
            .apply(TestOrderEventStubs::accepted(
                &order,
                AccountId::from("ACCOUNT-001"),
                VenueOrderId::from("venue-order-1"),
            ))
            .expect("exit order should accept venue acceptance");
        order
    }

    fn apply_exit_fill(
        order: &mut OrderAny,
        quantity: Quantity,
        trade_id: &str,
        ts_event_ns: u64,
    ) -> TradeId {
        let trade_id = TradeId::from(trade_id);
        let instrument = binary_option_with_max_price(order.instrument_id());
        let fill = TestOrderEventStubs::filled(
            order,
            &instrument,
            Some(trade_id),
            None,
            Some(Price::new(0.50, 2)),
            Some(quantity),
            Some(LiquiditySide::Taker),
            None,
            Some(UnixNanos::from(ts_event_ns)),
            Some(AccountId::from("ACCOUNT-001")),
        );
        order
            .apply(fill)
            .expect("exit order should accept the fill event");
        trade_id
    }

    fn partially_filled_exit_order(
        client_order_id: &str,
        order_quantity: Quantity,
        fill_quantity: Quantity,
        trade_id: &str,
        ts_event_ns: u64,
    ) -> (OrderAny, TradeId) {
        let mut order = accepted_exit_order(client_order_id, order_quantity);
        let trade_id = apply_exit_fill(&mut order, fill_quantity, trade_id, ts_event_ns);
        (order, trade_id)
    }

    fn market_exit_order(client_order_id: &str, quantity: Quantity) -> OrderAny {
        OrderAny::Market(
            MarketOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("instrument-yes.VENUE-A"),
                ClientOrderId::from(client_order_id),
                OrderSide::Sell,
                quantity,
                TimeInForce::Ioc,
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("market exit order should be valid"),
        )
    }

    fn limit_exit_order_for_instrument(
        client_order_id: &str,
        instrument_id: InstrumentId,
        quantity: Quantity,
    ) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                instrument_id,
                ClientOrderId::from(client_order_id),
                OrderSide::Sell,
                quantity,
                Price::new(0.50, 2),
                TimeInForce::Gtc,
                None,
                false,
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit exit order should be valid"),
        )
    }

    #[test]
    fn economics_order_binding_changes_when_the_final_quantity_changes() {
        let original = limit_order("economics-binding");
        let original_binding = economics_order_binding(&original)
            .expect("the original order should have a canonical binding");
        let mut changed = original.clone();
        changed.set_quantity(Quantity::new(0.5, 2));
        changed.set_leaves_qty(Quantity::new(0.5, 2));

        assert_ne!(
            economics_order_binding(&changed)
                .expect("the changed order should have a canonical binding"),
            original_binding
        );
    }
}
