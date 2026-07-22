use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReservationMetadataFact {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: String,
    pub collateral_currency: String,
    pub capital_pool_id: String,
    pub collateral_group_id: String,
    pub instrument_id: String,
    pub side: String,
    pub submitted_quantity: String,
    pub liability_factor: String,
    pub additive_liability: String,
    pub reserved_liability: String,
    pub observed_at_ns: u64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReservationFillFact {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub trade_id: String,
    pub instrument_id: String,
    pub side: String,
    pub fill_quantity: String,
    pub observed_at_ns: u64,
    pub reconciliation: bool,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderIntentClampNotEvaluatedReason {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderIntentClampOutcome {
    WithinBounds,
    Clamped {
        original_quantity: String,
    },
    Rejected,
    NotEvaluated {
        reason: OrderIntentClampNotEvaluatedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentOrderFields {
    pub order_type: String,
    pub time_in_force: String,
    pub price: Option<String>,
    pub trigger_price: Option<String>,
    pub activation_price: Option<String>,
    pub trigger_type: Option<String>,
    pub trigger_instrument_id: Option<String>,
    pub trailing_offset: Option<String>,
    pub trailing_offset_type: Option<String>,
    pub expire_time_unix_nanos: Option<String>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentDetails {
    pub strategy_id: String,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
    pub clamp_outcome: Option<OrderIntentClampOutcome>,
    pub order_fields: OrderIntentOrderFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryOrderIntentFact {
    pub details: OrderIntentDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReducingExitOrderIntentFact {
    pub details: OrderIntentDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionDetails {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub basket_id: String,
    pub group_id: String,
    pub leg_instrument_ids: Vec<String>,
    pub total_notional: String,
    pub leg_order_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionGrantedFact {
    pub details: BasketAdmissionDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasketAdmissionRejectionReason {
    BasketNotionalCapExceeded,
    MaxOpenBasketCapExceeded,
    StaleScannerEvidence,
    StaleSubmitRecheck,
    NonPositiveCandidateCost,
    NonPositiveEdge,
    EdgeThreshold,
    MissingGroupingProof,
    MissingSettlementRules,
    RetryBudgetExceeded,
    SubmitSlots,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionRejectedFact {
    pub details: BasketAdmissionDetails,
    pub reason: BasketAdmissionRejectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSide {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFact {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: String,
    pub position_id: String,
    pub instrument_id: String,
    pub product_id: String,
    pub outcome_side: OutcomeSide,
    pub entry_order_side: String,
    pub quantity: String,
    pub entry_price: String,
    pub family_key: String,
    pub strike_price: String,
    pub resolution_instrument_id: String,
    pub resolution_ts_event_ns: u64,
    pub reference_close_price: String,
    pub payout_per_share: String,
    pub terminal_value: String,
    pub realized_pnl: String,
    pub settlement_currency: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementBookingErrorReason {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementBookingErrorFact {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub instrument_id: Option<String>,
    pub resolution_instrument_id: Option<String>,
    pub reason: SettlementBookingErrorReason,
    pub detail: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderLifecycleTransition {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderLifecycleOutcome {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLifecycleFact {
    pub strategy_id: String,
    pub transition: OrderLifecycleTransition,
    pub outcome: OrderLifecycleOutcome,
    pub source: String,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub position_id: Option<String>,
    pub client_order_id: Option<String>,
    pub prior_client_order_id: Option<String>,
    pub raw_reason_text: Option<String>,
    pub order_side: Option<String>,
    pub filled_quantity: Option<String>,
    pub residual_quantity: Option<String>,
    pub ts_event_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSettlementFact {
    pub settlement_key: String,
    pub booking_error: Option<SettlementBookingErrorFact>,
    pub lifecycle: OrderLifecycleFact,
}

#[derive(Debug, Default)]
pub struct StartupRecoveryFacts {
    reservation_metadata: BTreeMap<String, SubmitReservationMetadataFact>,
    reservation_fill_trade_ids: BTreeMap<(String, String), BTreeSet<String>>,
    settlements: BTreeMap<String, SettlementFact>,
    booking_error_keys: BTreeSet<String>,
    terminal_settlement_keys: BTreeSet<String>,
}

impl StartupRecoveryFacts {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.reservation_metadata.is_empty()
            && self.reservation_fill_trade_ids.is_empty()
            && self.settlements.is_empty()
            && self.booking_error_keys.is_empty()
            && self.terminal_settlement_keys.is_empty()
    }

    #[must_use]
    pub fn settlements(&self) -> &BTreeMap<String, SettlementFact> {
        &self.settlements
    }

    pub(super) fn apply(&mut self, fact: RecoveryFact) -> Result<()> {
        match fact {
            RecoveryFact::ReservationMetadata(metadata) => {
                ensure!(
                    !self
                        .reservation_metadata
                        .contains_key(&metadata.client_order_id),
                    "duplicate submit-reservation metadata for client_order_id `{}`",
                    metadata.client_order_id
                );
                self.reservation_metadata
                    .insert(metadata.client_order_id.clone(), metadata);
            }
            RecoveryFact::ReservationFill(fill) => {
                self.reservation_fill_trade_ids
                    .entry((fill.client_order_id, fill.submit_reservation_id))
                    .or_default()
                    .insert(fill.trade_id);
            }
            RecoveryFact::Settlement(settlement) => {
                ensure!(
                    !self.settlements.contains_key(&settlement.settlement_key),
                    "duplicate settlement evidence for settlement_key `{}`",
                    settlement.settlement_key
                );
                self.settlements
                    .insert(settlement.settlement_key.clone(), settlement);
            }
            RecoveryFact::BookingError(booking_error) => {
                self.booking_error_keys.insert(booking_error.settlement_key);
            }
            RecoveryFact::TerminalSettlement(terminal) => {
                if terminal.booking_error.is_some() {
                    self.booking_error_keys
                        .insert(terminal.settlement_key.clone());
                }
                self.terminal_settlement_keys
                    .insert(terminal.settlement_key);
            }
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        for (client_order_id, submit_reservation_id) in self.reservation_fill_trade_ids.keys() {
            let metadata = self
                .reservation_metadata
                .get(client_order_id)
                .with_context(|| {
                    format!(
                        "submit-reservation fill has no metadata for client_order_id `{client_order_id}`"
                    )
                })?;
            ensure!(
                metadata.submit_reservation_id == *submit_reservation_id,
                "submit-reservation fill `{client_order_id}` reservation `{submit_reservation_id}` does not match submit-reservation metadata `{}`",
                metadata.submit_reservation_id
            );
        }
        Ok(())
    }
}

pub(super) enum RecoveryFact {
    ReservationMetadata(SubmitReservationMetadataFact),
    ReservationFill(SubmitReservationFillFact),
    Settlement(SettlementFact),
    BookingError(SettlementBookingErrorFact),
    TerminalSettlement(TerminalSettlementFact),
}
