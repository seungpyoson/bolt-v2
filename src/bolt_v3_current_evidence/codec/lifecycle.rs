use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{OrderLifecycleFact, OrderLifecycleOutcome, OrderLifecycleTransition},
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode(
    fact: OrderLifecycleFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fact(&fact)?;
    let purpose = KnownPurpose::OrderLifecycle;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &LineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            lifecycle: LifecycleV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_fact(line: &str, line_number: usize) -> Result<OrderLifecycleFact> {
    let decoded: LineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::OrderLifecycleV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.lifecycle.into_fact();
    validate_fact(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_fact(fact: &OrderLifecycleFact) -> Result<(), RecordFailure> {
    let optional = [
        fact.market_id.as_deref(),
        fact.instrument_id.as_deref(),
        fact.position_id.as_deref(),
        fact.client_order_id.as_deref(),
        fact.prior_client_order_id.as_deref(),
        fact.raw_reason_text.as_deref(),
        fact.order_side.as_deref(),
        fact.filled_quantity.as_deref(),
        fact.residual_quantity.as_deref(),
    ];
    if fact.strategy_id.trim().is_empty()
        || fact.source.trim().is_empty()
        || optional
            .into_iter()
            .flatten()
            .any(|value| value.trim().is_empty())
        || fact.ts_event_ns.is_some_and(|timestamp| timestamp == 0)
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "order lifecycle contains an empty or invalid field"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    lifecycle: LifecycleV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleV1 {
    strategy_id: String,
    transition: TransitionV1,
    outcome: OutcomeV1,
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

impl LifecycleV1 {
    fn from_fact(fact: OrderLifecycleFact) -> Self {
        Self {
            strategy_id: fact.strategy_id,
            transition: TransitionV1::from_fact(fact.transition),
            outcome: OutcomeV1::from_fact(fact.outcome),
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

    fn into_fact(self) -> OrderLifecycleFact {
        OrderLifecycleFact {
            strategy_id: self.strategy_id,
            transition: self.transition.into_fact(),
            outcome: self.outcome.into_fact(),
            source: self.source,
            market_id: self.market_id,
            instrument_id: self.instrument_id,
            position_id: self.position_id,
            client_order_id: self.client_order_id,
            prior_client_order_id: self.prior_client_order_id,
            raw_reason_text: self.raw_reason_text,
            order_side: self.order_side,
            filled_quantity: self.filled_quantity,
            residual_quantity: self.residual_quantity,
            ts_event_ns: self.ts_event_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransitionV1 {
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

impl TransitionV1 {
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
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutcomeV1 {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
}

impl OutcomeV1 {
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

    fn into_fact(self) -> OrderLifecycleOutcome {
        match self {
            Self::PendingEntry => OrderLifecycleOutcome::PendingEntry,
            Self::Managed => OrderLifecycleOutcome::Managed,
            Self::ExitPending => OrderLifecycleOutcome::ExitPending,
            Self::EntryReconcilePending => OrderLifecycleOutcome::EntryReconcilePending,
            Self::UnsupportedObserved => OrderLifecycleOutcome::UnsupportedObserved,
            Self::BlindRecovery => OrderLifecycleOutcome::BlindRecovery,
            Self::Flat => OrderLifecycleOutcome::Flat,
        }
    }
}
