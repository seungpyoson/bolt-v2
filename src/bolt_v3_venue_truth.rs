use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
};

use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::OrderSide,
    events::OrderEventAny,
    identifiers::{AccountId, VenueOrderId},
    types::Money,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub type VenueTruthSnapshotFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<VenueTruthSnapshot>> + Send + 'a>>;

pub const VENUE_TRUTH_CAPTURE_AGGREGATE_ENDPOINT: &str = "venue_truth_snapshot";
pub const VENUE_TRUTH_CAPTURE_ERROR_CLASS_UNKNOWN: &str = "unknown";
pub const VENUE_TRUTH_DIVERGENCE_FIELD_ACCOUNT_ID: &str = "account_id";
pub const VENUE_TRUTH_DIVERGENCE_FIELD_COLLATERAL_ALLOWANCE: &str = "collateral_allowance";
pub const VENUE_TRUTH_DIVERGENCE_FIELD_COLLATERAL_BALANCE: &str = "collateral_balance";
pub const VENUE_TRUTH_DIVERGENCE_FIELD_OPEN_ORDERS: &str = "open_orders";
pub const VENUE_TRUTH_DIVERGENCE_FIELD_ORDERING: &str = "order_event_observed_at_ns";
pub const VENUE_TRUTH_DIVERGENCE_FIELD_POSITIONS: &str = "positions_by_product_id";
pub const VENUE_TRUTH_DIVERGENCE_PRIOR_ACCEPTED_VALUE_MISSING: &str = "no_prior_accepted_snapshot";
pub const VENUE_TRUTH_DIVERGENCE_REASON_ACCOUNT_CHANGED: &str = "account_changed";
pub const VENUE_TRUTH_DIVERGENCE_REASON_COLLATERAL_ALLOWANCE: &str =
    "unexplained_collateral_allowance_delta";
pub const VENUE_TRUTH_DIVERGENCE_REASON_COLLATERAL_BALANCE: &str =
    "unexplained_collateral_balance_delta";
pub const VENUE_TRUTH_DIVERGENCE_REASON_OPEN_ORDERS: &str = "unexplained_open_order_delta";
pub const VENUE_TRUTH_DIVERGENCE_REASON_ORDERING: &str = "ordering_violation";
pub const VENUE_TRUTH_DIVERGENCE_REASON_POSITIONS: &str = "unexplained_position_delta";

pub trait VenueTruthSnapshotSource: std::fmt::Debug + Send + Sync {
    fn snapshot(&self, captured_at: UnixNanos) -> VenueTruthSnapshotFuture<'_>;
}

pub trait VenueTruthOrderEventMapper: std::fmt::Debug + Send + Sync {
    fn map_order_event(&self, event: &OrderEventAny) -> Option<VenueTruthOrderEvent>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthSnapshot {
    pub captured_at: UnixNanos,
    pub account_id: AccountId,
    pub collateral_balance: Money,
    pub collateral_allowance: Money,
    pub open_orders: BTreeMap<VenueOrderId, VenueTruthOpenOrder>,
    pub positions_by_product_id: BTreeMap<String, Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthOpenOrder {
    pub venue_order_id: VenueOrderId,
    pub market_id: String,
    pub product_id: String,
    pub side: OrderSide,
    pub original_size: Decimal,
    pub size_matched: Decimal,
    pub open_size: Decimal,
    pub price: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueTruthOrderEvent {
    Accepted {
        client_order_id: String,
        venue_order_id: VenueOrderId,
        observed_at_ns: UnixNanos,
    },
    Filled {
        venue_order_id: VenueOrderId,
        product_id: String,
        side: OrderSide,
        quantity: Decimal,
        fill_price: Decimal,
        fee: Decimal,
        observed_at_ns: UnixNanos,
    },
    Terminal {
        client_order_id: String,
        venue_order_id: Option<VenueOrderId>,
        observed_at_ns: UnixNanos,
        timestamp_domain: VenueTruthOrderEventTimestampDomain,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthOrderEventTimestampDomain {
    Venue,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthReconciliation {
    BaselineAccepted,
    DeltaExplained,
    DeltaPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthDivergenceKind {
    AccountChanged,
    OrderingViolation,
    UnexplainedOpenOrderDelta,
    UnexplainedPositionDelta,
    UnexplainedCollateralDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VenueTruthCollateralDivergenceField {
    Balance,
    Allowance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VenueTruthDivergenceCause {
    kind: VenueTruthDivergenceKind,
    collateral_field: Option<VenueTruthCollateralDivergenceField>,
}

impl VenueTruthDivergenceCause {
    fn collateral(field: VenueTruthCollateralDivergenceField) -> Self {
        Self {
            kind: VenueTruthDivergenceKind::UnexplainedCollateralDelta,
            collateral_field: Some(field),
        }
    }
}

impl From<VenueTruthDivergenceKind> for VenueTruthDivergenceCause {
    fn from(kind: VenueTruthDivergenceKind) -> Self {
        Self {
            kind,
            collateral_field: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueTruthDivergenceAlarmClass {
    TrueDivergence,
    OrderingViolation,
    SilentChannel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueTruthCaptureFailureEvidence {
    pub source: String,
    pub observed_at_ns: u64,
    pub endpoint: String,
    pub error_class: String,
    pub captures_missed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueTruthDivergenceEvidence {
    pub source: String,
    pub observed_at_ns: u64,
    pub account_id: String,
    pub field: String,
    pub venue_value: String,
    pub prior_accepted_value: String,
    pub missing_explanation: String,
    pub alarm_class: VenueTruthDivergenceAlarmClass,
}

#[derive(Debug)]
pub struct VenueTruthCaptureEndpointError {
    endpoint: &'static str,
    error_class: &'static str,
    source: anyhow::Error,
}

impl VenueTruthCaptureEndpointError {
    pub fn new(endpoint: &'static str, error_class: &'static str, source: anyhow::Error) -> Self {
        Self {
            endpoint,
            error_class,
            source,
        }
    }

    #[must_use]
    pub const fn endpoint(&self) -> &'static str {
        self.endpoint
    }

    #[must_use]
    pub const fn error_class(&self) -> &'static str {
        self.error_class
    }
}

impl fmt::Display for VenueTruthCaptureEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "venue truth capture endpoint `{}` failed with error class `{}`: {:#}",
            self.endpoint, self.error_class, self.source
        )
    }
}

impl Error for VenueTruthCaptureEndpointError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.source()
    }
}

#[must_use]
pub fn venue_truth_capture_failure_parts(error: &anyhow::Error) -> (&'static str, &'static str) {
    error
        .downcast_ref::<VenueTruthCaptureEndpointError>()
        .map(|error| (error.endpoint(), error.error_class()))
        .unwrap_or((
            VENUE_TRUTH_CAPTURE_AGGREGATE_ENDPOINT,
            VENUE_TRUTH_CAPTURE_ERROR_CLASS_UNKNOWN,
        ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthDivergence {
    pub kind: VenueTruthDivergenceKind,
    pub alarm_class: VenueTruthDivergenceAlarmClass,
    pub previous_captured_at: Option<UnixNanos>,
    pub current_captured_at: UnixNanos,
    pub account_id: String,
    pub field: String,
    pub venue_value: String,
    pub prior_accepted_value: String,
    pub missing_explanation: String,
}

impl VenueTruthDivergence {
    #[must_use]
    pub fn evidence(&self, source: impl Into<String>) -> VenueTruthDivergenceEvidence {
        VenueTruthDivergenceEvidence {
            source: source.into(),
            observed_at_ns: self.current_captured_at.as_u64(),
            account_id: self.account_id.clone(),
            field: self.field.clone(),
            venue_value: self.venue_value.clone(),
            prior_accepted_value: self.prior_accepted_value.clone(),
            missing_explanation: self.missing_explanation.clone(),
            alarm_class: self.alarm_class,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthReconciliationResult {
    pub capture_number: u64,
    pub outcome: VenueTruthReconciliation,
    pub accepted_snapshot: Option<VenueTruthSnapshot>,
}

#[derive(Debug, Clone)]
pub struct VenueTruthReconciler {
    previous_snapshot: Option<VenueTruthSnapshot>,
    event_projection: VenueTruthEventProjection,
    next_capture_number: u64,
    completed_captures: VecDeque<VenueTruthCompletedCapture>,
    pending_capture: Option<VenueTruthPendingCapture>,
}

#[derive(Debug, Clone)]
struct VenueTruthEventProjection {
    event_count: u64,
    venue_event_count: u64,
    local_event_count: u64,
    last_venue_observed_at_ns: Option<UnixNanos>,
    ordering_violation: bool,
    accepted_venue_order_ids: BTreeSet<VenueOrderId>,
    client_to_venue_order_id: BTreeMap<String, VenueOrderId>,
    terminal_venue_order_ids: BTreeSet<VenueOrderId>,
    fill_quantity_by_venue_order_id: BTreeMap<VenueOrderId, Decimal>,
    fill_collateral_lots: Vec<VenueTruthFillCollateralLot>,
    drainable_collateral_balance_delta: Decimal,
    drainable_collateral_allowance_delta: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueTruthFillCollateralLot {
    product_id: String,
    side: OrderSide,
    remaining_quantity: Decimal,
    remaining_collateral_balance_delta: Decimal,
    remaining_collateral_allowance_delta: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueTruthCompletedCapture {
    capture_number: u64,
    event_count_at_completion: u64,
    venue_event_count_at_completion: u64,
    snapshot: VenueTruthSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueTruthPendingCapture {
    capture: VenueTruthCompletedCapture,
    event_count_at_first_judgment: u64,
    venue_event_count_at_first_judgment: u64,
}

// The bolt-v3 legacy-default fence forbids a `Default` impl on the production
// surface, so the no-argument `new` is sanctioned with an explicit allow rather
// than satisfying `clippy::new_without_default` by adding a forbidden `Default`.
#[allow(clippy::new_without_default)]
impl VenueTruthReconciler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            previous_snapshot: None,
            event_projection: VenueTruthEventProjection::empty(),
            next_capture_number: 1,
            completed_captures: VecDeque::new(),
            pending_capture: None,
        }
    }

    pub fn record_order_event(&mut self, event: VenueTruthOrderEvent) {
        self.event_projection.record_order_event(event);
    }

    #[must_use]
    pub fn latest_accepted_snapshot(&self) -> Option<&VenueTruthSnapshot> {
        self.previous_snapshot.as_ref()
    }

    pub fn record_snapshot_completion(
        &mut self,
        snapshot: VenueTruthSnapshot,
    ) -> Result<Vec<VenueTruthReconciliationResult>, Box<VenueTruthDivergence>> {
        self.record_snapshot_completion_without_processing(snapshot);
        self.process_completed_captures()
    }

    pub fn record_snapshot_completion_without_processing(&mut self, snapshot: VenueTruthSnapshot) {
        let capture = VenueTruthCompletedCapture {
            capture_number: self.next_capture_number,
            event_count_at_completion: self.event_projection.event_count,
            venue_event_count_at_completion: self.event_projection.venue_event_count,
            snapshot,
        };
        self.next_capture_number += 1;
        self.completed_captures.push_back(capture);
    }

    pub fn process_completed_captures(
        &mut self,
    ) -> Result<Vec<VenueTruthReconciliationResult>, Box<VenueTruthDivergence>> {
        let mut results = Vec::new();
        loop {
            if self.pending_capture.is_some() {
                let fence_completed = self.completed_captures.front().is_some_and(|capture| {
                    capture.capture_number
                        > self
                            .pending_capture
                            .as_ref()
                            .expect("pending capture checked")
                            .capture
                            .capture_number
                });
                if !fence_completed {
                    break;
                }
                let pending = self
                    .pending_capture
                    .take()
                    .expect("pending capture checked");
                match self.try_accept_capture(&pending.capture, true) {
                    Ok(result) => {
                        results.push(result);
                        continue;
                    }
                    Err(cause) => {
                        return Err(Box::new(self.classified_divergence(
                            cause,
                            &pending.capture,
                            pending.event_count_at_first_judgment,
                            pending.venue_event_count_at_first_judgment,
                        )));
                    }
                }
            }

            let Some(capture) = self.completed_captures.pop_front() else {
                break;
            };
            let fence_already_completed = self
                .completed_captures
                .front()
                .is_some_and(|next| next.capture_number > capture.capture_number);
            match self.try_accept_capture(&capture, fence_already_completed) {
                Ok(result) => results.push(result),
                Err(cause)
                    if fence_already_completed
                        || cause.kind == VenueTruthDivergenceKind::OrderingViolation =>
                {
                    return Err(Box::new(self.classified_divergence(
                        cause,
                        &capture,
                        capture.event_count_at_completion,
                        capture.venue_event_count_at_completion,
                    )));
                }
                Err(_kind) => {
                    self.pending_capture = Some(VenueTruthPendingCapture {
                        capture,
                        event_count_at_first_judgment: self.event_projection.event_count,
                        venue_event_count_at_first_judgment: self
                            .event_projection
                            .venue_event_count,
                    });
                    let pending = self
                        .pending_capture
                        .as_ref()
                        .expect("pending capture just inserted");
                    results.push(VenueTruthReconciliationResult {
                        capture_number: pending.capture.capture_number,
                        outcome: VenueTruthReconciliation::DeltaPending,
                        accepted_snapshot: None,
                    });
                    break;
                }
            }
        }
        Ok(results)
    }

    pub fn reconcile_snapshot(
        &mut self,
        snapshot: VenueTruthSnapshot,
    ) -> Result<VenueTruthReconciliation, Box<VenueTruthDivergence>> {
        let results = self.record_snapshot_completion(snapshot)?;
        let Some(result) = results.last() else {
            return Ok(VenueTruthReconciliation::DeltaPending);
        };
        Ok(result.outcome)
    }

    fn try_accept_capture(
        &mut self,
        capture: &VenueTruthCompletedCapture,
        allow_deferred_collateral: bool,
    ) -> Result<VenueTruthReconciliationResult, VenueTruthDivergenceCause> {
        let snapshot = capture.snapshot.clone();
        let Some(previous) = &self.previous_snapshot else {
            self.event_projection
                .prune_after_capture_acceptance(&snapshot);
            self.previous_snapshot = Some(snapshot.clone());
            return Ok(VenueTruthReconciliationResult {
                capture_number: capture.capture_number,
                outcome: VenueTruthReconciliation::BaselineAccepted,
                accepted_snapshot: Some(snapshot),
            });
        };
        if previous.account_id != snapshot.account_id {
            return Err(VenueTruthDivergenceKind::AccountChanged.into());
        }

        let mut projection = self.event_projection.clone();
        explain_open_order_delta(previous, &snapshot, &mut projection)
            .map_err(VenueTruthDivergenceCause::from)?;
        explain_position_delta(previous, &snapshot, &mut projection)
            .map_err(VenueTruthDivergenceCause::from)?;
        explain_collateral_delta(
            previous,
            &snapshot,
            &mut projection,
            allow_deferred_collateral,
        )?;
        if projection.ordering_violation {
            return Err(VenueTruthDivergenceKind::OrderingViolation.into());
        }

        projection.prune_after_capture_acceptance(&snapshot);
        self.event_projection = projection;
        self.previous_snapshot = Some(snapshot.clone());
        Ok(VenueTruthReconciliationResult {
            capture_number: capture.capture_number,
            outcome: VenueTruthReconciliation::DeltaExplained,
            accepted_snapshot: Some(snapshot),
        })
    }

    fn classified_divergence(
        &self,
        cause: VenueTruthDivergenceCause,
        capture: &VenueTruthCompletedCapture,
        _earlier_event_count: u64,
        earlier_venue_event_count: u64,
    ) -> VenueTruthDivergence {
        let alarm_class = if self.event_projection.ordering_violation {
            VenueTruthDivergenceAlarmClass::OrderingViolation
        } else if self.event_projection.venue_event_count == earlier_venue_event_count {
            VenueTruthDivergenceAlarmClass::SilentChannel
        } else {
            VenueTruthDivergenceAlarmClass::TrueDivergence
        };
        divergence(
            cause,
            alarm_class,
            self.previous_snapshot.as_ref(),
            &capture.snapshot,
        )
    }
}

impl VenueTruthEventProjection {
    fn empty() -> Self {
        Self {
            event_count: 0,
            venue_event_count: 0,
            local_event_count: 0,
            last_venue_observed_at_ns: None,
            ordering_violation: false,
            accepted_venue_order_ids: BTreeSet::new(),
            client_to_venue_order_id: BTreeMap::new(),
            terminal_venue_order_ids: BTreeSet::new(),
            fill_quantity_by_venue_order_id: BTreeMap::new(),
            fill_collateral_lots: Vec::new(),
            drainable_collateral_balance_delta: Decimal::ZERO,
            drainable_collateral_allowance_delta: Decimal::ZERO,
        }
    }

    fn record_order_event(&mut self, event: VenueTruthOrderEvent) {
        if let Some(observed_at_ns) = event.venue_observed_at_ns() {
            if self
                .last_venue_observed_at_ns
                .is_some_and(|previous| observed_at_ns < previous)
            {
                self.ordering_violation = true;
            }
            self.last_venue_observed_at_ns = Some(observed_at_ns);
            self.venue_event_count += 1;
        } else {
            self.local_event_count += 1;
        }
        self.event_count += 1;
        match event {
            VenueTruthOrderEvent::Accepted {
                client_order_id,
                venue_order_id,
                ..
            } => {
                self.accepted_venue_order_ids.insert(venue_order_id);
                self.client_to_venue_order_id
                    .insert(client_order_id, venue_order_id);
            }
            VenueTruthOrderEvent::Filled {
                venue_order_id,
                product_id,
                side,
                quantity,
                fill_price,
                fee,
                ..
            } => {
                add_decimal(
                    &mut self.fill_quantity_by_venue_order_id,
                    venue_order_id,
                    quantity,
                );
                if matches!(side, OrderSide::Buy | OrderSide::Sell) && quantity > Decimal::ZERO {
                    self.fill_collateral_lots.push(VenueTruthFillCollateralLot {
                        product_id,
                        side,
                        remaining_quantity: quantity,
                        remaining_collateral_balance_delta: collateral_balance_delta_for_fill_event(
                            side, quantity, fill_price, fee,
                        ),
                        remaining_collateral_allowance_delta:
                            collateral_allowance_delta_for_fill_event(
                                side, quantity, fill_price, fee,
                            ),
                    });
                }
            }
            VenueTruthOrderEvent::Terminal {
                client_order_id,
                venue_order_id,
                ..
            } => {
                if let Some(venue_order_id) = venue_order_id
                    .or_else(|| self.client_to_venue_order_id.get(&client_order_id).copied())
                {
                    self.terminal_venue_order_ids.insert(venue_order_id);
                }
            }
        }
    }

    fn prune_after_capture_acceptance(&mut self, snapshot: &VenueTruthSnapshot) {
        let open_venue_order_ids = snapshot
            .open_orders
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        self.terminal_venue_order_ids
            .retain(|venue_order_id| open_venue_order_ids.contains(venue_order_id));
        self.accepted_venue_order_ids
            .retain(|venue_order_id| open_venue_order_ids.contains(venue_order_id));
        self.fill_quantity_by_venue_order_id
            .retain(|venue_order_id, _| open_venue_order_ids.contains(venue_order_id));
        let retained_terminal_venue_order_ids = self.terminal_venue_order_ids.clone();
        let accepted_venue_order_ids = self.accepted_venue_order_ids.clone();
        let fill_venue_order_ids = self
            .fill_quantity_by_venue_order_id
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        self.client_to_venue_order_id.retain(|_, venue_order_id| {
            open_venue_order_ids.contains(venue_order_id)
                || retained_terminal_venue_order_ids.contains(venue_order_id)
                || accepted_venue_order_ids.contains(venue_order_id)
                || fill_venue_order_ids.contains(venue_order_id)
        });
    }

    fn consume_position_fill(
        &mut self,
        product_id: &str,
        side: OrderSide,
        amount: Decimal,
    ) -> bool {
        if amount <= Decimal::ZERO {
            return amount == Decimal::ZERO;
        }
        let mut remaining = amount;
        for lot in &mut self.fill_collateral_lots {
            if remaining == Decimal::ZERO {
                break;
            }
            if lot.product_id != product_id
                || lot.side != side
                || lot.remaining_quantity <= Decimal::ZERO
            {
                continue;
            }
            let consumed_quantity = if lot.remaining_quantity <= remaining {
                lot.remaining_quantity
            } else {
                remaining
            };
            let consumed_collateral_balance_delta = if consumed_quantity == lot.remaining_quantity {
                lot.remaining_collateral_balance_delta
            } else {
                lot.remaining_collateral_balance_delta * consumed_quantity / lot.remaining_quantity
            };
            let consumed_collateral_allowance_delta = if consumed_quantity == lot.remaining_quantity
            {
                lot.remaining_collateral_allowance_delta
            } else {
                lot.remaining_collateral_allowance_delta * consumed_quantity
                    / lot.remaining_quantity
            };
            lot.remaining_quantity -= consumed_quantity;
            lot.remaining_collateral_balance_delta -= consumed_collateral_balance_delta;
            lot.remaining_collateral_allowance_delta -= consumed_collateral_allowance_delta;
            remaining -= consumed_quantity;
            self.drainable_collateral_balance_delta += consumed_collateral_balance_delta;
            self.drainable_collateral_allowance_delta += consumed_collateral_allowance_delta;
        }
        self.fill_collateral_lots
            .retain(|lot| lot.remaining_quantity > Decimal::ZERO);
        remaining == Decimal::ZERO
    }

    fn consume_collateral_allowance_delta(&mut self, amount: Decimal) -> bool {
        if amount == Decimal::ZERO {
            return true;
        }
        let available = self.drainable_collateral_allowance_delta;
        if available == Decimal::ZERO {
            return false;
        }
        if available > Decimal::ZERO {
            if amount < Decimal::ZERO || amount > available {
                return false;
            }
        } else if amount > Decimal::ZERO || amount < available {
            return false;
        }
        self.drainable_collateral_allowance_delta -= amount;
        true
    }

    fn consume_collateral_balance_delta(
        &mut self,
        amount: Decimal,
        allow_deferred_collateral: bool,
    ) -> bool {
        let available = self.drainable_collateral_balance_delta;
        if amount == Decimal::ZERO {
            return allow_deferred_collateral || available == Decimal::ZERO;
        }
        if available == Decimal::ZERO {
            return false;
        }
        if available > Decimal::ZERO {
            if amount < Decimal::ZERO || amount > available {
                return false;
            }
        } else if amount > Decimal::ZERO || amount < available {
            return false;
        }
        self.drainable_collateral_balance_delta -= amount;
        true
    }
}

impl VenueTruthOrderEvent {
    fn venue_observed_at_ns(&self) -> Option<UnixNanos> {
        match self {
            Self::Accepted { observed_at_ns, .. }
            | Self::Filled { observed_at_ns, .. }
            | Self::Terminal {
                observed_at_ns,
                timestamp_domain: VenueTruthOrderEventTimestampDomain::Venue,
                ..
            } => Some(*observed_at_ns),
            Self::Terminal {
                timestamp_domain: VenueTruthOrderEventTimestampDomain::Local,
                ..
            } => None,
        }
    }
}

fn explain_open_order_delta(
    previous: &VenueTruthSnapshot,
    current: &VenueTruthSnapshot,
    projection: &mut VenueTruthEventProjection,
) -> Result<(), VenueTruthDivergenceKind> {
    for (venue_order_id, current_order) in &current.open_orders {
        match previous.open_orders.get(venue_order_id) {
            None => {
                if !projection.accepted_venue_order_ids.remove(venue_order_id) {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
                if current_order.size_matched > Decimal::ZERO
                    && !consume_decimal(
                        &mut projection.fill_quantity_by_venue_order_id,
                        venue_order_id,
                        current_order.size_matched,
                    )
                {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
            }
            Some(previous_order) => {
                if previous_order.market_id != current_order.market_id
                    || previous_order.product_id != current_order.product_id
                    || previous_order.side != current_order.side
                    || previous_order.original_size != current_order.original_size
                    || previous_order.price != current_order.price
                    || current_order.size_matched < previous_order.size_matched
                    || current_order.open_size > previous_order.open_size
                {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
                let matched_delta = current_order.size_matched - previous_order.size_matched;
                if matched_delta > Decimal::ZERO
                    && !consume_decimal(
                        &mut projection.fill_quantity_by_venue_order_id,
                        venue_order_id,
                        matched_delta,
                    )
                {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
            }
        }
    }
    for (venue_order_id, previous_order) in &previous.open_orders {
        if current.open_orders.contains_key(venue_order_id) {
            continue;
        }
        if projection.terminal_venue_order_ids.remove(venue_order_id) {
            continue;
        }
        if consume_decimal(
            &mut projection.fill_quantity_by_venue_order_id,
            venue_order_id,
            previous_order.open_size,
        ) {
            continue;
        }
        return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
    }
    Ok(())
}

fn explain_position_delta(
    previous: &VenueTruthSnapshot,
    current: &VenueTruthSnapshot,
    projection: &mut VenueTruthEventProjection,
) -> Result<bool, VenueTruthDivergenceKind> {
    let mut product_ids: BTreeSet<&str> = BTreeSet::new();
    product_ids.extend(previous.positions_by_product_id.keys().map(String::as_str));
    product_ids.extend(current.positions_by_product_id.keys().map(String::as_str));

    let mut explained = false;
    for product_id in product_ids {
        let previous_size = previous
            .positions_by_product_id
            .get(product_id)
            .copied()
            .unwrap_or(Decimal::ZERO);
        let current_size = current
            .positions_by_product_id
            .get(product_id)
            .copied()
            .unwrap_or(Decimal::ZERO);
        let delta = current_size - previous_size;
        if delta > Decimal::ZERO {
            if !projection.consume_position_fill(product_id, OrderSide::Buy, delta) {
                return Err(VenueTruthDivergenceKind::UnexplainedPositionDelta);
            }
            explained = true;
        } else if delta < Decimal::ZERO {
            if !projection.consume_position_fill(product_id, OrderSide::Sell, -delta) {
                return Err(VenueTruthDivergenceKind::UnexplainedPositionDelta);
            }
            explained = true;
        }
    }
    Ok(explained)
}

fn explain_collateral_delta(
    previous: &VenueTruthSnapshot,
    current: &VenueTruthSnapshot,
    projection: &mut VenueTruthEventProjection,
    allow_deferred_collateral: bool,
) -> Result<(), VenueTruthDivergenceCause> {
    let collateral_balance_delta =
        current.collateral_balance.as_decimal() - previous.collateral_balance.as_decimal();
    if !projection
        .consume_collateral_balance_delta(collateral_balance_delta, allow_deferred_collateral)
    {
        return Err(VenueTruthDivergenceCause::collateral(
            VenueTruthCollateralDivergenceField::Balance,
        ));
    }
    // Allowance is an operator-changeable on-chain approval, not a venue-invariant fact.
    // The 2026-07-04 evidence search found no captured allowance-spanning-fill artifact,
    // so this path is assumption-free: consumed fills may explain bounded allowance
    // decreases, zero deltas are no-ops, and any unexplained increase or re-approval halts.
    let collateral_allowance_delta =
        current.collateral_allowance.as_decimal() - previous.collateral_allowance.as_decimal();
    if !projection.consume_collateral_allowance_delta(collateral_allowance_delta) {
        return Err(VenueTruthDivergenceCause::collateral(
            VenueTruthCollateralDivergenceField::Allowance,
        ));
    }
    Ok(())
}

fn collateral_balance_delta_for_fill_event(
    side: OrderSide,
    quantity: Decimal,
    fill_price: Decimal,
    fee: Decimal,
) -> Decimal {
    match side {
        OrderSide::Buy => -(quantity * fill_price) - fee,
        OrderSide::Sell => (quantity * fill_price) - fee,
        _ => Decimal::ZERO,
    }
}

fn collateral_allowance_delta_for_fill_event(
    side: OrderSide,
    quantity: Decimal,
    fill_price: Decimal,
    fee: Decimal,
) -> Decimal {
    match side {
        OrderSide::Buy => -(quantity * fill_price) - fee,
        _ => Decimal::ZERO,
    }
}

fn divergence(
    cause: VenueTruthDivergenceCause,
    alarm_class: VenueTruthDivergenceAlarmClass,
    previous: Option<&VenueTruthSnapshot>,
    current: &VenueTruthSnapshot,
) -> VenueTruthDivergence {
    let (field, venue_value, prior_accepted_value, missing_explanation) =
        divergence_evidence_fields(cause, previous, current);
    VenueTruthDivergence {
        kind: cause.kind,
        alarm_class,
        previous_captured_at: previous.map(|snapshot| snapshot.captured_at),
        current_captured_at: current.captured_at,
        account_id: current.account_id.to_string(),
        field,
        venue_value,
        prior_accepted_value,
        missing_explanation,
    }
}

fn divergence_evidence_fields(
    cause: VenueTruthDivergenceCause,
    previous: Option<&VenueTruthSnapshot>,
    current: &VenueTruthSnapshot,
) -> (String, String, String, String) {
    match cause.kind {
        VenueTruthDivergenceKind::AccountChanged => (
            VENUE_TRUTH_DIVERGENCE_FIELD_ACCOUNT_ID.to_string(),
            current.account_id.to_string(),
            prior_accepted_value(previous, |snapshot| snapshot.account_id.to_string()),
            VENUE_TRUTH_DIVERGENCE_REASON_ACCOUNT_CHANGED.to_string(),
        ),
        VenueTruthDivergenceKind::OrderingViolation => (
            VENUE_TRUTH_DIVERGENCE_FIELD_ORDERING.to_string(),
            current.captured_at.as_u64().to_string(),
            prior_accepted_value(previous, |snapshot| {
                snapshot.captured_at.as_u64().to_string()
            }),
            VENUE_TRUTH_DIVERGENCE_REASON_ORDERING.to_string(),
        ),
        VenueTruthDivergenceKind::UnexplainedOpenOrderDelta => (
            VENUE_TRUTH_DIVERGENCE_FIELD_OPEN_ORDERS.to_string(),
            format_open_order_ids(&current.open_orders),
            prior_accepted_value(previous, |snapshot| {
                format_open_order_ids(&snapshot.open_orders)
            }),
            VENUE_TRUTH_DIVERGENCE_REASON_OPEN_ORDERS.to_string(),
        ),
        VenueTruthDivergenceKind::UnexplainedPositionDelta => (
            VENUE_TRUTH_DIVERGENCE_FIELD_POSITIONS.to_string(),
            format!("{:?}", current.positions_by_product_id),
            prior_accepted_value(previous, |snapshot| {
                format!("{:?}", snapshot.positions_by_product_id)
            }),
            VENUE_TRUTH_DIVERGENCE_REASON_POSITIONS.to_string(),
        ),
        VenueTruthDivergenceKind::UnexplainedCollateralDelta => match cause
            .collateral_field
            .unwrap_or(VenueTruthCollateralDivergenceField::Balance)
        {
            VenueTruthCollateralDivergenceField::Allowance => (
                VENUE_TRUTH_DIVERGENCE_FIELD_COLLATERAL_ALLOWANCE.to_string(),
                current.collateral_allowance.as_decimal().to_string(),
                prior_accepted_value(previous, |snapshot| {
                    snapshot.collateral_allowance.as_decimal().to_string()
                }),
                VENUE_TRUTH_DIVERGENCE_REASON_COLLATERAL_ALLOWANCE.to_string(),
            ),
            VenueTruthCollateralDivergenceField::Balance => (
                VENUE_TRUTH_DIVERGENCE_FIELD_COLLATERAL_BALANCE.to_string(),
                current.collateral_balance.as_decimal().to_string(),
                prior_accepted_value(previous, |snapshot| {
                    snapshot.collateral_balance.as_decimal().to_string()
                }),
                VENUE_TRUTH_DIVERGENCE_REASON_COLLATERAL_BALANCE.to_string(),
            ),
        },
    }
}

fn prior_accepted_value(
    previous: Option<&VenueTruthSnapshot>,
    format_snapshot: impl FnOnce(&VenueTruthSnapshot) -> String,
) -> String {
    previous.map_or_else(
        || VENUE_TRUTH_DIVERGENCE_PRIOR_ACCEPTED_VALUE_MISSING.to_string(),
        format_snapshot,
    )
}

fn format_open_order_ids(open_orders: &BTreeMap<VenueOrderId, VenueTruthOpenOrder>) -> String {
    let ids = open_orders
        .keys()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    format!("{ids:?}")
}

fn add_decimal<K>(map: &mut BTreeMap<K, Decimal>, key: K, amount: Decimal)
where
    K: Ord,
{
    map.entry(key)
        .and_modify(|current| *current += amount)
        .or_insert(amount);
}

fn consume_decimal<K, Q>(map: &mut BTreeMap<K, Decimal>, key: &Q, amount: Decimal) -> bool
where
    K: Ord + std::borrow::Borrow<Q>,
    Q: Ord + ?Sized,
{
    if amount <= Decimal::ZERO {
        return true;
    }
    let mut remove = false;
    if let Some(current) = map.get_mut(key) {
        if *current < amount {
            return false;
        }
        *current -= amount;
        if *current == Decimal::ZERO {
            remove = true;
        }
    } else {
        return false;
    }
    if remove {
        map.remove(key);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_model::types::Currency;

    #[test]
    fn terminal_marks_only_and_capture_acceptance_prunes_projection_maps() {
        let mut reconciler = VenueTruthReconciler::new();
        let venue_order_id = VenueOrderId::from("venue-order-1");
        reconciler.record_order_event(VenueTruthOrderEvent::Accepted {
            client_order_id: "client-order-1".to_string(),
            venue_order_id,
            observed_at_ns: UnixNanos::from(1),
        });
        reconciler.record_order_event(VenueTruthOrderEvent::Filled {
            venue_order_id,
            product_id: "product-1".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::new(1, 0),
            fill_price: Decimal::new(5, 1),
            fee: Decimal::ZERO,
            observed_at_ns: UnixNanos::from(2),
        });

        assert!(
            reconciler
                .event_projection
                .client_to_venue_order_id
                .contains_key("client-order-1")
        );
        assert!(
            reconciler
                .event_projection
                .accepted_venue_order_ids
                .contains(&venue_order_id)
        );
        assert!(
            reconciler
                .event_projection
                .fill_quantity_by_venue_order_id
                .contains_key(&venue_order_id)
        );

        reconciler.record_order_event(VenueTruthOrderEvent::Terminal {
            client_order_id: "client-order-1".to_string(),
            venue_order_id: None,
            observed_at_ns: UnixNanos::from(3),
            timestamp_domain: VenueTruthOrderEventTimestampDomain::Venue,
        });

        assert!(
            reconciler
                .event_projection
                .client_to_venue_order_id
                .contains_key("client-order-1"),
            "terminal observation must keep explanation state until the snapshot boundary"
        );
        assert!(
            reconciler
                .event_projection
                .accepted_venue_order_ids
                .contains(&venue_order_id),
            "terminal observation must keep accepted-order explanation until the snapshot boundary"
        );
        assert!(
            reconciler
                .event_projection
                .fill_quantity_by_venue_order_id
                .contains_key(&venue_order_id),
            "terminal observation must keep fill explanation until the snapshot boundary"
        );
        assert!(
            reconciler
                .event_projection
                .terminal_venue_order_ids
                .contains(&venue_order_id),
            "terminal marker remains until a structural venue snapshot boundary"
        );

        assert_eq!(
            reconciler
                .reconcile_snapshot(empty_snapshot(4))
                .expect("baseline snapshot should reconcile"),
            VenueTruthReconciliation::BaselineAccepted
        );
        assert!(
            reconciler
                .event_projection
                .terminal_venue_order_ids
                .is_empty(),
            "accepted capture boundary should clear terminal markers for non-open venue orders"
        );
        assert!(
            !reconciler
                .event_projection
                .client_to_venue_order_id
                .contains_key("client-order-1"),
            "accepted capture boundary should clear stale client-to-venue projection"
        );
        assert!(
            !reconciler
                .event_projection
                .accepted_venue_order_ids
                .contains(&venue_order_id),
            "accepted capture boundary should clear stale accepted-order projection"
        );
        assert!(
            !reconciler
                .event_projection
                .fill_quantity_by_venue_order_id
                .contains_key(&venue_order_id),
            "accepted capture boundary should clear stale fill-quantity projection"
        );
    }

    #[test]
    fn fok_fill_projection_maps_prune_at_acceptance_boundary_without_terminal() {
        let mut reconciler = VenueTruthReconciler::new();
        let venue_order_id = VenueOrderId::from("venue-order-1");
        assert_eq!(
            reconciler
                .reconcile_snapshot(empty_snapshot(1))
                .expect("baseline should reconcile"),
            VenueTruthReconciliation::BaselineAccepted
        );
        reconciler.record_order_event(VenueTruthOrderEvent::Accepted {
            client_order_id: "client-order-1".to_string(),
            venue_order_id,
            observed_at_ns: UnixNanos::from(2),
        });
        reconciler.record_order_event(VenueTruthOrderEvent::Filled {
            venue_order_id,
            product_id: "product-1".to_string(),
            side: OrderSide::Buy,
            quantity: Decimal::new(1, 0),
            fill_price: Decimal::new(5, 1),
            fee: Decimal::ZERO,
            observed_at_ns: UnixNanos::from(3),
        });

        assert_eq!(
            reconciler
                .reconcile_snapshot(snapshot_with_position(4))
                .expect("FOK fill should reconcile through position and collateral deltas"),
            VenueTruthReconciliation::DeltaExplained
        );

        assert!(
            reconciler
                .event_projection
                .accepted_venue_order_ids
                .is_empty(),
            "FOK accepted-order projection must not leak after the accepted snapshot boundary"
        );
        assert!(
            reconciler
                .event_projection
                .fill_quantity_by_venue_order_id
                .is_empty(),
            "FOK fill-quantity projection must not leak after the accepted snapshot boundary"
        );
        assert!(
            reconciler
                .event_projection
                .client_to_venue_order_id
                .is_empty(),
            "FOK client mapping must not leak after the accepted snapshot boundary"
        );
    }

    fn empty_snapshot(captured_at: u64) -> VenueTruthSnapshot {
        let currency = Currency::from("USD");
        VenueTruthSnapshot {
            captured_at: UnixNanos::from(captured_at),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_balance: Money::new(100.0, currency),
            collateral_allowance: Money::new(100.0, currency),
            open_orders: BTreeMap::new(),
            positions_by_product_id: BTreeMap::new(),
        }
    }

    fn snapshot_with_position(captured_at: u64) -> VenueTruthSnapshot {
        let currency = Currency::from("USD");
        let mut positions_by_product_id = BTreeMap::new();
        positions_by_product_id.insert("product-1".to_string(), Decimal::new(1, 0));
        VenueTruthSnapshot {
            captured_at: UnixNanos::from(captured_at),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_balance: Money::new(99.5, currency),
            collateral_allowance: Money::new(100.0, currency),
            open_orders: BTreeMap::new(),
            positions_by_product_id,
        }
    }
}
