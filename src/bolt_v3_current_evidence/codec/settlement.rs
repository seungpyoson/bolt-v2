use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        EvidenceOrderSide, OrderLifecycleFact, OrderLifecycleOutcome, OrderLifecycleSource,
        OrderLifecycleTransition, OutcomeSide, SettlementBookingErrorFact,
        SettlementBookingErrorReason, SettlementFact, TerminalSettlementFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_nonempty,
    validate_recorded_at,
};

pub(super) fn encode_settlement(
    fact: SettlementFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_settlement(&fact)?;
    let purpose = KnownPurpose::Settlement;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &SettlementLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            settlement: SettlementV1::from_fact(fact),
        },
    )
}

fn validate_settlement(fact: &SettlementFact) -> Result<(), RecordFailure> {
    validate_nonempty(
        "settlement",
        [
            fact.strategy_id.as_str(),
            fact.settlement_key.as_str(),
            fact.market_id.as_str(),
            fact.position_id.as_str(),
            fact.instrument_id.as_str(),
            fact.product_id.as_str(),
            fact.quantity.as_str(),
            fact.entry_price.as_str(),
            fact.family_key.as_str(),
            fact.strike_price.as_str(),
            fact.resolution_instrument_id.as_str(),
            fact.reference_close_price.as_str(),
            fact.payout_per_share.as_str(),
            fact.terminal_value.as_str(),
            fact.realized_pnl.as_str(),
            fact.settlement_currency.as_str(),
        ],
        fact.resolution_ts_event_ns,
    )?;
    if fact.entry_order_side == EvidenceOrderSide::Unspecified {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "settlement entry order side must be specified"
        )));
    }
    Ok(())
}

pub(super) fn encode_terminal(
    fact: TerminalSettlementFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_terminal(&fact)?;
    let purpose = KnownPurpose::TerminalSettlement;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &TerminalLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            terminal_settlement: TerminalV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_settlement(line: &str, line_number: usize) -> Result<SettlementFact> {
    let decoded: SettlementLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::SettlementV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.settlement.into_fact();
    validate_settlement(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

pub(super) fn decode_terminal(line: &str, line_number: usize) -> Result<TerminalSettlementFact> {
    let decoded: TerminalLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::TerminalSettlementV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    ensure!(
        decoded.terminal_settlement.booking_error.settlement_key
            == decoded.terminal_settlement.settlement_key,
        "terminal settlement booking-error key mismatch at machine evidence line {line_number}"
    );
    let fact = decoded.terminal_settlement.into_fact();
    validate_terminal(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_booking_error(fact: &SettlementBookingErrorFact) -> Result<(), RecordFailure> {
    validate_nonempty(
        "settlement booking error",
        [
            fact.strategy_id.as_str(),
            fact.settlement_key.as_str(),
            fact.detail.as_str(),
        ]
        .into_iter()
        .chain(
            [
                fact.market_id.as_deref(),
                fact.position_id.as_deref(),
                fact.instrument_id.as_deref(),
                fact.resolution_instrument_id.as_deref(),
            ]
            .into_iter()
            .flatten(),
        ),
        fact.observed_at_ns,
    )
}

fn validate_terminal(fact: &TerminalSettlementFact) -> Result<(), RecordFailure> {
    if fact.settlement_key.trim().is_empty()
        || fact.lifecycle.strategy_id.trim().is_empty()
        || fact.lifecycle.order_side == Some(EvidenceOrderSide::Unspecified)
        || fact
            .lifecycle
            .ts_event_ns
            .is_some_and(|timestamp| timestamp == 0)
        || [
            fact.lifecycle.market_id.as_deref(),
            fact.lifecycle.instrument_id.as_deref(),
            fact.lifecycle.position_id.as_deref(),
            fact.lifecycle.client_order_id.as_deref(),
            fact.lifecycle.prior_client_order_id.as_deref(),
            fact.lifecycle.raw_reason_text.as_deref(),
            fact.lifecycle.filled_quantity.as_deref(),
            fact.lifecycle.residual_quantity.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| value.trim().is_empty())
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "terminal settlement contains an empty or invalid lifecycle field"
        )));
    }
    if fact.lifecycle.transition != OrderLifecycleTransition::SettlementBookingTerminal
        || !matches!(
            fact.lifecycle.outcome,
            OrderLifecycleOutcome::ExitPending | OrderLifecycleOutcome::Flat
        )
        || fact.lifecycle.source != OrderLifecycleSource::SettlementBookingTerminal
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "terminal settlement lifecycle must be settlement-booking-terminal with exit-pending or flat outcome"
        )));
    }
    let booking_error = &fact.booking_error;
    validate_booking_error(booking_error)?;
    if booking_error.settlement_key != fact.settlement_key {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "terminal settlement booking-error key does not match canonical key"
        )));
    }
    for (booking_value, lifecycle_value, field) in [
        (
            booking_error.market_id.as_deref(),
            fact.lifecycle.market_id.as_deref(),
            "market_id",
        ),
        (
            booking_error.position_id.as_deref(),
            fact.lifecycle.position_id.as_deref(),
            "position_id",
        ),
        (
            booking_error.instrument_id.as_deref(),
            fact.lifecycle.instrument_id.as_deref(),
            "instrument_id",
        ),
    ] {
        if let Some(booking_value) = booking_value
            && Some(booking_value) != lifecycle_value
        {
            return Err(RecordFailure::Rejected(anyhow::anyhow!(
                "terminal settlement booking-error {field} does not match lifecycle"
            )));
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    settlement: SettlementV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementV1 {
    strategy_id: String,
    settlement_key: String,
    market_id: String,
    position_id: String,
    instrument_id: String,
    product_id: String,
    outcome_side: OutcomeSideV1,
    entry_order_side: SettlementOrderSideV1,
    quantity: String,
    entry_price: String,
    family_key: String,
    strike_price: String,
    resolution_instrument_id: String,
    resolution_ts_event_ns: u64,
    reference_close_price: String,
    payout_per_share: String,
    terminal_value: String,
    realized_pnl: String,
    settlement_currency: String,
}

impl SettlementV1 {
    fn from_fact(fact: SettlementFact) -> Self {
        Self {
            strategy_id: fact.strategy_id,
            settlement_key: fact.settlement_key,
            market_id: fact.market_id,
            position_id: fact.position_id,
            instrument_id: fact.instrument_id,
            product_id: fact.product_id,
            outcome_side: match fact.outcome_side {
                OutcomeSide::Up => OutcomeSideV1::Up,
                OutcomeSide::Down => OutcomeSideV1::Down,
            },
            entry_order_side: SettlementOrderSideV1::from_fact(fact.entry_order_side),
            quantity: fact.quantity,
            entry_price: fact.entry_price,
            family_key: fact.family_key,
            strike_price: fact.strike_price,
            resolution_instrument_id: fact.resolution_instrument_id,
            resolution_ts_event_ns: fact.resolution_ts_event_ns,
            reference_close_price: fact.reference_close_price,
            payout_per_share: fact.payout_per_share,
            terminal_value: fact.terminal_value,
            realized_pnl: fact.realized_pnl,
            settlement_currency: fact.settlement_currency,
        }
    }

    fn into_fact(self) -> SettlementFact {
        SettlementFact {
            strategy_id: self.strategy_id,
            settlement_key: self.settlement_key,
            market_id: self.market_id,
            position_id: self.position_id,
            instrument_id: self.instrument_id,
            product_id: self.product_id,
            outcome_side: match self.outcome_side {
                OutcomeSideV1::Up => OutcomeSide::Up,
                OutcomeSideV1::Down => OutcomeSide::Down,
            },
            entry_order_side: self.entry_order_side.into_fact(),
            quantity: self.quantity,
            entry_price: self.entry_price,
            family_key: self.family_key,
            strike_price: self.strike_price,
            resolution_instrument_id: self.resolution_instrument_id,
            resolution_ts_event_ns: self.resolution_ts_event_ns,
            reference_close_price: self.reference_close_price,
            payout_per_share: self.payout_per_share,
            terminal_value: self.terminal_value,
            realized_pnl: self.realized_pnl,
            settlement_currency: self.settlement_currency,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettlementOrderSideV1 {
    Unspecified,
    Buy,
    Sell,
}

impl SettlementOrderSideV1 {
    fn from_fact(value: EvidenceOrderSide) -> Self {
        match value {
            EvidenceOrderSide::Unspecified => Self::Unspecified,
            EvidenceOrderSide::Buy => Self::Buy,
            EvidenceOrderSide::Sell => Self::Sell,
        }
    }

    fn into_fact(self) -> EvidenceOrderSide {
        match self {
            Self::Unspecified => EvidenceOrderSide::Unspecified,
            Self::Buy => EvidenceOrderSide::Buy,
            Self::Sell => EvidenceOrderSide::Sell,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutcomeSideV1 {
    Up,
    Down,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BookingErrorReasonV1 {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

impl BookingErrorReasonV1 {
    fn from_fact(reason: SettlementBookingErrorReason) -> Self {
        match reason {
            SettlementBookingErrorReason::ResolutionFeedMissing => Self::ResolutionFeedMissing,
            SettlementBookingErrorReason::SettlementAlreadyBooked => Self::SettlementAlreadyBooked,
            SettlementBookingErrorReason::SettlementInputInvalid => Self::SettlementInputInvalid,
            SettlementBookingErrorReason::SettlementBlocked => Self::SettlementBlocked,
        }
    }

    fn into_fact(self) -> SettlementBookingErrorReason {
        match self {
            Self::ResolutionFeedMissing => SettlementBookingErrorReason::ResolutionFeedMissing,
            Self::SettlementAlreadyBooked => SettlementBookingErrorReason::SettlementAlreadyBooked,
            Self::SettlementInputInvalid => SettlementBookingErrorReason::SettlementInputInvalid,
            Self::SettlementBlocked => SettlementBookingErrorReason::SettlementBlocked,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    terminal_settlement: TerminalV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalV1 {
    settlement_key: String,
    booking_error: TerminalBookingErrorV1,
    lifecycle: TerminalLifecycleV1,
}

impl TerminalV1 {
    fn from_fact(fact: TerminalSettlementFact) -> Self {
        Self {
            settlement_key: fact.settlement_key,
            booking_error: TerminalBookingErrorV1::from_fact(fact.booking_error),
            lifecycle: TerminalLifecycleV1::from_fact(fact.lifecycle),
        }
    }

    fn into_fact(self) -> TerminalSettlementFact {
        TerminalSettlementFact {
            settlement_key: self.settlement_key,
            booking_error: self.booking_error.into_fact(),
            lifecycle: self.lifecycle.into_fact(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalBookingErrorV1 {
    strategy_id: String,
    settlement_key: String,
    market_id: Option<String>,
    position_id: Option<String>,
    instrument_id: Option<String>,
    resolution_instrument_id: Option<String>,
    reason: BookingErrorReasonV1,
    detail: String,
    observed_at_ns: u64,
}

impl TerminalBookingErrorV1 {
    fn from_fact(fact: SettlementBookingErrorFact) -> Self {
        Self {
            strategy_id: fact.strategy_id,
            settlement_key: fact.settlement_key,
            market_id: fact.market_id,
            position_id: fact.position_id,
            instrument_id: fact.instrument_id,
            resolution_instrument_id: fact.resolution_instrument_id,
            reason: BookingErrorReasonV1::from_fact(fact.reason),
            detail: fact.detail,
            observed_at_ns: fact.observed_at_ns,
        }
    }

    fn into_fact(self) -> SettlementBookingErrorFact {
        SettlementBookingErrorFact {
            strategy_id: self.strategy_id,
            settlement_key: self.settlement_key,
            market_id: self.market_id,
            position_id: self.position_id,
            instrument_id: self.instrument_id,
            resolution_instrument_id: self.resolution_instrument_id,
            reason: self.reason.into_fact(),
            detail: self.detail,
            observed_at_ns: self.observed_at_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalLifecycleSourceV1 {
    SelectionBoundary,
    EntryFill,
    PositionEvent,
    RestartBootstrap,
    OrderDenied,
    OrderRejected,
    OrderCanceled,
    OrderExpired,
    SettlementEvidenceRecovery,
    SettlementBookingTerminal,
    ReconcilePass,
    OrderFillVoided,
    PositionClosed,
}

impl TerminalLifecycleSourceV1 {
    fn from_fact(value: OrderLifecycleSource) -> Self {
        match value {
            OrderLifecycleSource::SelectionBoundary => Self::SelectionBoundary,
            OrderLifecycleSource::EntryFill => Self::EntryFill,
            OrderLifecycleSource::PositionEvent => Self::PositionEvent,
            OrderLifecycleSource::RestartBootstrap => Self::RestartBootstrap,
            OrderLifecycleSource::OrderDenied => Self::OrderDenied,
            OrderLifecycleSource::OrderRejected => Self::OrderRejected,
            OrderLifecycleSource::OrderCanceled => Self::OrderCanceled,
            OrderLifecycleSource::OrderExpired => Self::OrderExpired,
            OrderLifecycleSource::SettlementEvidenceRecovery => Self::SettlementEvidenceRecovery,
            OrderLifecycleSource::SettlementBookingTerminal => Self::SettlementBookingTerminal,
            OrderLifecycleSource::ReconcilePass => Self::ReconcilePass,
            OrderLifecycleSource::OrderFillVoided => Self::OrderFillVoided,
            OrderLifecycleSource::PositionClosed => Self::PositionClosed,
        }
    }

    fn into_fact(self) -> OrderLifecycleSource {
        match self {
            Self::SelectionBoundary => OrderLifecycleSource::SelectionBoundary,
            Self::EntryFill => OrderLifecycleSource::EntryFill,
            Self::PositionEvent => OrderLifecycleSource::PositionEvent,
            Self::RestartBootstrap => OrderLifecycleSource::RestartBootstrap,
            Self::OrderDenied => OrderLifecycleSource::OrderDenied,
            Self::OrderRejected => OrderLifecycleSource::OrderRejected,
            Self::OrderCanceled => OrderLifecycleSource::OrderCanceled,
            Self::OrderExpired => OrderLifecycleSource::OrderExpired,
            Self::SettlementEvidenceRecovery => OrderLifecycleSource::SettlementEvidenceRecovery,
            Self::SettlementBookingTerminal => OrderLifecycleSource::SettlementBookingTerminal,
            Self::ReconcilePass => OrderLifecycleSource::ReconcilePass,
            Self::OrderFillVoided => OrderLifecycleSource::OrderFillVoided,
            Self::PositionClosed => OrderLifecycleSource::PositionClosed,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalLifecycleOrderSideV1 {
    Unspecified,
    Buy,
    Sell,
}

impl TerminalLifecycleOrderSideV1 {
    fn from_fact(value: EvidenceOrderSide) -> Self {
        match value {
            EvidenceOrderSide::Unspecified => Self::Unspecified,
            EvidenceOrderSide::Buy => Self::Buy,
            EvidenceOrderSide::Sell => Self::Sell,
        }
    }

    fn into_fact(self) -> EvidenceOrderSide {
        match self {
            Self::Unspecified => EvidenceOrderSide::Unspecified,
            Self::Buy => EvidenceOrderSide::Buy,
            Self::Sell => EvidenceOrderSide::Sell,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalLifecycleV1 {
    strategy_id: String,
    transition: LifecycleTransitionV1,
    outcome: LifecycleOutcomeV1,
    source: TerminalLifecycleSourceV1,
    market_id: Option<String>,
    instrument_id: Option<String>,
    position_id: Option<String>,
    client_order_id: Option<String>,
    prior_client_order_id: Option<String>,
    raw_reason_text: Option<String>,
    order_side: Option<TerminalLifecycleOrderSideV1>,
    filled_quantity: Option<String>,
    residual_quantity: Option<String>,
    ts_event_ns: Option<u64>,
}

impl TerminalLifecycleV1 {
    fn from_fact(fact: OrderLifecycleFact) -> Self {
        Self {
            strategy_id: fact.strategy_id,
            transition: LifecycleTransitionV1::from_fact(fact.transition),
            outcome: LifecycleOutcomeV1::from_fact(fact.outcome),
            source: TerminalLifecycleSourceV1::from_fact(fact.source),
            market_id: fact.market_id,
            instrument_id: fact.instrument_id,
            position_id: fact.position_id,
            client_order_id: fact.client_order_id,
            prior_client_order_id: fact.prior_client_order_id,
            raw_reason_text: fact.raw_reason_text,
            order_side: fact.order_side.map(TerminalLifecycleOrderSideV1::from_fact),
            filled_quantity: fact.filled_quantity,
            residual_quantity: fact.residual_quantity,
            ts_event_ns: fact.ts_event_ns,
        }
    }

    fn into_fact(self) -> OrderLifecycleFact {
        OrderLifecycleFact {
            strategy_id: self.strategy_id,
            transition: self.transition.into_fact(),
            outcome: self.outcome.into_fact(),
            source: self.source.into_fact(),
            market_id: self.market_id,
            instrument_id: self.instrument_id,
            position_id: self.position_id,
            client_order_id: self.client_order_id,
            prior_client_order_id: self.prior_client_order_id,
            raw_reason_text: self.raw_reason_text,
            order_side: self.order_side.map(TerminalLifecycleOrderSideV1::into_fact),
            filled_quantity: self.filled_quantity,
            residual_quantity: self.residual_quantity,
            ts_event_ns: self.ts_event_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleTransitionV1 {
    BoundaryReclassification,
    EntryFillMaterialized,
    EntryReconcilePending,
    PositionTruthRematerialized,
    PositionClosed,
    ResidualRemanaged,
    RestartOpenOrderAdopted,
    RestartOpenOrderRecoveryBlocked,
    SettlementEvidenceRecoveryBlocked,
    SettlementBookingTerminal,
    OrderDenied,
    OrderRejected,
    OrderCanceled,
    OrderExpired,
    OrderFilled,
    ReconcileQueryFailed,
    ExposureQuarantined,
    PositionIdentityConflict,
    ReplacementAdopted,
    CanonicalPositionAwaiting,
    CanonicalPositionMultiplicity,
    OperationSinkUnknownEntered,
    OperationSinkUnknownResolved,
    HistoricalExitCorrectionDeferred,
    ExposureObligationSaturated,
}

impl LifecycleTransitionV1 {
    fn from_fact(transition: OrderLifecycleTransition) -> Self {
        match transition {
            OrderLifecycleTransition::BoundaryReclassification => Self::BoundaryReclassification,
            OrderLifecycleTransition::EntryFillMaterialized => Self::EntryFillMaterialized,
            OrderLifecycleTransition::EntryReconcilePending => Self::EntryReconcilePending,
            OrderLifecycleTransition::PositionTruthRematerialized => {
                Self::PositionTruthRematerialized
            }
            OrderLifecycleTransition::PositionClosed => Self::PositionClosed,
            OrderLifecycleTransition::ResidualRemanaged => Self::ResidualRemanaged,
            OrderLifecycleTransition::RestartOpenOrderAdopted => Self::RestartOpenOrderAdopted,
            OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked => {
                Self::RestartOpenOrderRecoveryBlocked
            }
            OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked => {
                Self::SettlementEvidenceRecoveryBlocked
            }
            OrderLifecycleTransition::SettlementBookingTerminal => Self::SettlementBookingTerminal,
            OrderLifecycleTransition::OrderDenied => Self::OrderDenied,
            OrderLifecycleTransition::OrderRejected => Self::OrderRejected,
            OrderLifecycleTransition::OrderCanceled => Self::OrderCanceled,
            OrderLifecycleTransition::OrderExpired => Self::OrderExpired,
            OrderLifecycleTransition::OrderFilled => Self::OrderFilled,
            OrderLifecycleTransition::ReconcileQueryFailed => Self::ReconcileQueryFailed,
            OrderLifecycleTransition::ExposureQuarantined => Self::ExposureQuarantined,
            OrderLifecycleTransition::PositionIdentityConflict => Self::PositionIdentityConflict,
            OrderLifecycleTransition::ReplacementAdopted => Self::ReplacementAdopted,
            OrderLifecycleTransition::CanonicalPositionAwaiting => Self::CanonicalPositionAwaiting,
            OrderLifecycleTransition::CanonicalPositionMultiplicity => {
                Self::CanonicalPositionMultiplicity
            }
            OrderLifecycleTransition::OperationSinkUnknownEntered => {
                Self::OperationSinkUnknownEntered
            }
            OrderLifecycleTransition::OperationSinkUnknownResolved => {
                Self::OperationSinkUnknownResolved
            }
            OrderLifecycleTransition::HistoricalExitCorrectionDeferred => {
                Self::HistoricalExitCorrectionDeferred
            }
            OrderLifecycleTransition::ExposureObligationSaturated => {
                Self::ExposureObligationSaturated
            }
        }
    }

    fn into_fact(self) -> OrderLifecycleTransition {
        match self {
            Self::BoundaryReclassification => OrderLifecycleTransition::BoundaryReclassification,
            Self::EntryFillMaterialized => OrderLifecycleTransition::EntryFillMaterialized,
            Self::EntryReconcilePending => OrderLifecycleTransition::EntryReconcilePending,
            Self::PositionTruthRematerialized => {
                OrderLifecycleTransition::PositionTruthRematerialized
            }
            Self::PositionClosed => OrderLifecycleTransition::PositionClosed,
            Self::ResidualRemanaged => OrderLifecycleTransition::ResidualRemanaged,
            Self::RestartOpenOrderAdopted => OrderLifecycleTransition::RestartOpenOrderAdopted,
            Self::RestartOpenOrderRecoveryBlocked => {
                OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked
            }
            Self::SettlementEvidenceRecoveryBlocked => {
                OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked
            }
            Self::SettlementBookingTerminal => OrderLifecycleTransition::SettlementBookingTerminal,
            Self::OrderDenied => OrderLifecycleTransition::OrderDenied,
            Self::OrderRejected => OrderLifecycleTransition::OrderRejected,
            Self::OrderCanceled => OrderLifecycleTransition::OrderCanceled,
            Self::OrderExpired => OrderLifecycleTransition::OrderExpired,
            Self::OrderFilled => OrderLifecycleTransition::OrderFilled,
            Self::ReconcileQueryFailed => OrderLifecycleTransition::ReconcileQueryFailed,
            Self::ExposureQuarantined => OrderLifecycleTransition::ExposureQuarantined,
            Self::PositionIdentityConflict => OrderLifecycleTransition::PositionIdentityConflict,
            Self::ReplacementAdopted => OrderLifecycleTransition::ReplacementAdopted,
            Self::CanonicalPositionAwaiting => OrderLifecycleTransition::CanonicalPositionAwaiting,
            Self::CanonicalPositionMultiplicity => {
                OrderLifecycleTransition::CanonicalPositionMultiplicity
            }
            Self::OperationSinkUnknownEntered => {
                OrderLifecycleTransition::OperationSinkUnknownEntered
            }
            Self::OperationSinkUnknownResolved => {
                OrderLifecycleTransition::OperationSinkUnknownResolved
            }
            Self::HistoricalExitCorrectionDeferred => {
                OrderLifecycleTransition::HistoricalExitCorrectionDeferred
            }
            Self::ExposureObligationSaturated => {
                OrderLifecycleTransition::ExposureObligationSaturated
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleOutcomeV1 {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
    Quarantined,
    ReplacementConflict,
    OperationSinkUnknown,
    ObligationSaturated,
}

impl LifecycleOutcomeV1 {
    fn from_fact(outcome: OrderLifecycleOutcome) -> Self {
        match outcome {
            OrderLifecycleOutcome::PendingEntry => Self::PendingEntry,
            OrderLifecycleOutcome::Managed => Self::Managed,
            OrderLifecycleOutcome::ExitPending => Self::ExitPending,
            OrderLifecycleOutcome::EntryReconcilePending => Self::EntryReconcilePending,
            OrderLifecycleOutcome::UnsupportedObserved => Self::UnsupportedObserved,
            OrderLifecycleOutcome::BlindRecovery => Self::BlindRecovery,
            OrderLifecycleOutcome::Flat => Self::Flat,
            OrderLifecycleOutcome::Quarantined => Self::Quarantined,
            OrderLifecycleOutcome::ReplacementConflict => Self::ReplacementConflict,
            OrderLifecycleOutcome::OperationSinkUnknown => Self::OperationSinkUnknown,
            OrderLifecycleOutcome::ObligationSaturated => Self::ObligationSaturated,
        }
    }

    fn into_fact(self) -> OrderLifecycleOutcome {
        match self {
            Self::PendingEntry => OrderLifecycleOutcome::PendingEntry,
            Self::Managed => OrderLifecycleOutcome::Managed,
            Self::ExitPending => OrderLifecycleOutcome::ExitPending,
            Self::EntryReconcilePending => OrderLifecycleOutcome::EntryReconcilePending,
            Self::UnsupportedObserved => OrderLifecycleOutcome::UnsupportedObserved,
            Self::BlindRecovery => OrderLifecycleOutcome::BlindRecovery,
            Self::Flat => OrderLifecycleOutcome::Flat,
            Self::Quarantined => OrderLifecycleOutcome::Quarantined,
            Self::ReplacementConflict => OrderLifecycleOutcome::ReplacementConflict,
            Self::OperationSinkUnknown => OrderLifecycleOutcome::OperationSinkUnknown,
            Self::ObligationSaturated => OrderLifecycleOutcome::ObligationSaturated,
        }
    }
}
