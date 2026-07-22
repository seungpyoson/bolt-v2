use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        OrderLifecycleFact, OrderLifecycleOutcome, OrderLifecycleTransition, OutcomeSide,
        RecoveryFact, SettlementBookingErrorFact, SettlementBookingErrorReason, SettlementFact,
        TerminalSettlementFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, current_utc_ns, decode, encode_line, validate_envelope,
    validate_nonempty,
};

pub(super) fn encode_settlement(
    fact: SettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_nonempty(
        "settlement",
        [
            fact.strategy_id.as_str(),
            fact.settlement_key.as_str(),
            fact.market_id.as_str(),
            fact.position_id.as_str(),
            fact.instrument_id.as_str(),
            fact.product_id.as_str(),
            fact.entry_order_side.as_str(),
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
    let purpose = KnownPurpose::Settlement;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &SettlementLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns: current_utc_ns()?,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            settlement: SettlementV1::from_fact(fact),
        },
    )
}

pub(super) fn encode_booking_error(
    fact: SettlementBookingErrorFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_booking_error(&fact)?;
    let purpose = KnownPurpose::SettlementBookingError;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &BookingErrorLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns: current_utc_ns()?,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            booking_error: BookingErrorV1::from_fact(fact),
        },
    )
}

pub(super) fn encode_terminal(
    fact: TerminalSettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_terminal(&fact)?;
    let purpose = KnownPurpose::TerminalSettlement;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &TerminalLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns: current_utc_ns()?,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            terminal_settlement: TerminalV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_settlement(line: &str, line_number: usize) -> Result<RecoveryFact> {
    let decoded: SettlementLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::SettlementV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    Ok(RecoveryFact::Settlement(decoded.settlement.into_fact()))
}

pub(super) fn decode_booking_error(line: &str, line_number: usize) -> Result<RecoveryFact> {
    let decoded: BookingErrorLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::SettlementBookingErrorV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    Ok(RecoveryFact::BookingError {
        settlement_key: decoded.booking_error.settlement_key,
    })
}

pub(super) fn decode_terminal(line: &str, line_number: usize) -> Result<RecoveryFact> {
    let decoded: TerminalLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::TerminalSettlementV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    if let Some(booking_error) = decoded.terminal_settlement.booking_error.as_ref() {
        ensure!(
            booking_error.settlement_key == decoded.terminal_settlement.settlement_key,
            "terminal settlement booking-error key mismatch at machine evidence line {line_number}"
        );
    }
    Ok(RecoveryFact::TerminalSettlement {
        settlement_key: decoded.terminal_settlement.settlement_key,
        has_booking_error: decoded.terminal_settlement.booking_error.is_some(),
    })
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
        || fact.lifecycle.source.trim().is_empty()
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
            fact.lifecycle.order_side.as_deref(),
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
    if let Some(booking_error) = fact.booking_error.as_ref() {
        validate_booking_error(booking_error)?;
        if booking_error.settlement_key != fact.settlement_key {
            return Err(RecordFailure::Rejected(anyhow::anyhow!(
                "terminal settlement booking-error key does not match canonical key"
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
    entry_order_side: String,
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
            entry_order_side: fact.entry_order_side,
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
            entry_order_side: self.entry_order_side,
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
enum OutcomeSideV1 {
    Up,
    Down,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BookingErrorLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    booking_error: BookingErrorV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BookingErrorV1 {
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

impl BookingErrorV1 {
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
    booking_error: Option<TerminalBookingErrorV1>,
    lifecycle: TerminalLifecycleV1,
}

impl TerminalV1 {
    fn from_fact(fact: TerminalSettlementFact) -> Self {
        Self {
            settlement_key: fact.settlement_key,
            booking_error: fact.booking_error.map(TerminalBookingErrorV1::from_fact),
            lifecycle: TerminalLifecycleV1::from_fact(fact.lifecycle),
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
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalLifecycleV1 {
    strategy_id: String,
    transition: LifecycleTransitionV1,
    outcome: LifecycleOutcomeV1,
    source: String,
    market_id: Option<String>,
    instrument_id: Option<String>,
    position_id: Option<String>,
    client_order_id: Option<String>,
    prior_client_order_id: Option<String>,
    raw_reason_text: Option<String>,
    order_side: Option<String>,
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
            source: fact.source,
            market_id: fact.market_id,
            instrument_id: fact.instrument_id,
            position_id: fact.position_id,
            client_order_id: fact.client_order_id,
            prior_client_order_id: fact.prior_client_order_id,
            raw_reason_text: fact.raw_reason_text,
            order_side: fact.order_side,
            filled_quantity: fact.filled_quantity,
            residual_quantity: fact.residual_quantity,
            ts_event_ns: fact.ts_event_ns,
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
        }
    }
}
