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
    },
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
    UnexplainedOpenOrderDelta,
    UnexplainedPositionDelta,
    UnexplainedCollateralDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    last_observed_at_ns: Option<UnixNanos>,
    ordering_violation: bool,
    accepted_venue_order_ids: BTreeSet<VenueOrderId>,
    client_to_venue_order_id: BTreeMap<String, VenueOrderId>,
    terminal_venue_order_ids: BTreeSet<VenueOrderId>,
    fill_quantity_by_venue_order_id: BTreeMap<VenueOrderId, Decimal>,
    buy_fill_quantity_by_product_id: BTreeMap<String, Decimal>,
    sell_fill_quantity_by_product_id: BTreeMap<String, Decimal>,
    collateral_balance_delta: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueTruthCompletedCapture {
    capture_number: u64,
    event_count_at_completion: u64,
    snapshot: VenueTruthSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueTruthPendingCapture {
    capture: VenueTruthCompletedCapture,
    event_count_at_first_judgment: u64,
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
    ) -> Result<Vec<VenueTruthReconciliationResult>, VenueTruthDivergence> {
        self.record_snapshot_completion_without_processing(snapshot);
        self.process_completed_captures()
    }

    pub fn record_snapshot_completion_without_processing(&mut self, snapshot: VenueTruthSnapshot) {
        let capture = VenueTruthCompletedCapture {
            capture_number: self.next_capture_number,
            event_count_at_completion: self.event_projection.event_count,
            snapshot,
        };
        self.next_capture_number += 1;
        self.completed_captures.push_back(capture);
    }

    pub fn process_completed_captures(
        &mut self,
    ) -> Result<Vec<VenueTruthReconciliationResult>, VenueTruthDivergence> {
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
                match self.try_accept_capture(&pending.capture) {
                    Ok(result) => {
                        results.push(result);
                        continue;
                    }
                    Err(kind) => {
                        return Err(self.classified_divergence(
                            kind,
                            &pending.capture,
                            pending.event_count_at_first_judgment,
                        ));
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
            match self.try_accept_capture(&capture) {
                Ok(result) => results.push(result),
                Err(kind) if fence_already_completed => {
                    return Err(self.classified_divergence(
                        kind,
                        &capture,
                        capture.event_count_at_completion,
                    ));
                }
                Err(_kind) => {
                    self.pending_capture = Some(VenueTruthPendingCapture {
                        capture,
                        event_count_at_first_judgment: self.event_projection.event_count,
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
    ) -> Result<VenueTruthReconciliation, VenueTruthDivergence> {
        let results = self.record_snapshot_completion(snapshot)?;
        let Some(result) = results.last() else {
            return Ok(VenueTruthReconciliation::DeltaPending);
        };
        Ok(result.outcome)
    }

    fn try_accept_capture(
        &mut self,
        capture: &VenueTruthCompletedCapture,
    ) -> Result<VenueTruthReconciliationResult, VenueTruthDivergenceKind> {
        let snapshot = capture.snapshot.clone();
        let Some(previous) = &self.previous_snapshot else {
            self.previous_snapshot = Some(snapshot.clone());
            return Ok(VenueTruthReconciliationResult {
                capture_number: capture.capture_number,
                outcome: VenueTruthReconciliation::BaselineAccepted,
                accepted_snapshot: Some(snapshot),
            });
        };
        if previous.account_id != snapshot.account_id {
            return Err(VenueTruthDivergenceKind::AccountChanged);
        }

        let mut projection = self.event_projection.clone();
        explain_open_order_delta(previous, &snapshot, &mut projection)?;
        explain_position_delta(previous, &snapshot, &mut projection)?;
        explain_collateral_delta(previous, &snapshot, &mut projection)?;

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
        kind: VenueTruthDivergenceKind,
        capture: &VenueTruthCompletedCapture,
        earlier_event_count: u64,
    ) -> VenueTruthDivergence {
        let alarm_class = if self.event_projection.ordering_violation {
            VenueTruthDivergenceAlarmClass::OrderingViolation
        } else if self.event_projection.event_count == earlier_event_count {
            VenueTruthDivergenceAlarmClass::SilentChannel
        } else {
            VenueTruthDivergenceAlarmClass::TrueDivergence
        };
        divergence(
            kind,
            alarm_class,
            self.previous_snapshot
                .as_ref()
                .map(|snapshot| snapshot.captured_at),
            capture.snapshot.captured_at,
        )
    }
}

impl VenueTruthEventProjection {
    fn empty() -> Self {
        Self {
            event_count: 0,
            last_observed_at_ns: None,
            ordering_violation: false,
            accepted_venue_order_ids: BTreeSet::new(),
            client_to_venue_order_id: BTreeMap::new(),
            terminal_venue_order_ids: BTreeSet::new(),
            fill_quantity_by_venue_order_id: BTreeMap::new(),
            buy_fill_quantity_by_product_id: BTreeMap::new(),
            sell_fill_quantity_by_product_id: BTreeMap::new(),
            collateral_balance_delta: Decimal::ZERO,
        }
    }

    fn record_order_event(&mut self, event: VenueTruthOrderEvent) {
        let observed_at_ns = event.observed_at_ns();
        if self
            .last_observed_at_ns
            .is_some_and(|previous| observed_at_ns < previous)
        {
            self.ordering_violation = true;
        }
        self.last_observed_at_ns = Some(observed_at_ns);
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
                match side {
                    OrderSide::Buy => add_decimal(
                        &mut self.buy_fill_quantity_by_product_id,
                        product_id,
                        quantity,
                    ),
                    OrderSide::Sell => add_decimal(
                        &mut self.sell_fill_quantity_by_product_id,
                        product_id,
                        quantity,
                    ),
                    _ => {}
                }
                self.collateral_balance_delta +=
                    collateral_balance_delta_for_fill_event(side, quantity, fill_price, fee);
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

    fn consume_collateral_balance_delta(&mut self, amount: Decimal) -> bool {
        if amount == Decimal::ZERO {
            return true;
        }
        if self.collateral_balance_delta != amount {
            return false;
        }
        self.collateral_balance_delta = Decimal::ZERO;
        true
    }
}

impl VenueTruthOrderEvent {
    fn observed_at_ns(&self) -> UnixNanos {
        match self {
            Self::Accepted { observed_at_ns, .. }
            | Self::Filled { observed_at_ns, .. }
            | Self::Terminal { observed_at_ns, .. } => *observed_at_ns,
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
                if matched_delta > Decimal::ZERO {
                    if !consume_decimal(
                        &mut projection.fill_quantity_by_venue_order_id,
                        venue_order_id,
                        matched_delta,
                    ) {
                        return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                    }
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
            if !consume_decimal(
                &mut projection.buy_fill_quantity_by_product_id,
                product_id,
                delta,
            ) {
                return Err(VenueTruthDivergenceKind::UnexplainedPositionDelta);
            }
            explained = true;
        } else if delta < Decimal::ZERO {
            if !consume_decimal(
                &mut projection.sell_fill_quantity_by_product_id,
                product_id,
                -delta,
            ) {
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
) -> Result<(), VenueTruthDivergenceKind> {
    if previous.collateral_allowance != current.collateral_allowance {
        return Err(VenueTruthDivergenceKind::UnexplainedCollateralDelta);
    }
    let collateral_balance_delta =
        current.collateral_balance.as_decimal() - previous.collateral_balance.as_decimal();
    if !projection.consume_collateral_balance_delta(collateral_balance_delta) {
        return Err(VenueTruthDivergenceKind::UnexplainedCollateralDelta);
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

fn divergence(
    kind: VenueTruthDivergenceKind,
    alarm_class: VenueTruthDivergenceAlarmClass,
    previous_captured_at: Option<UnixNanos>,
    current_captured_at: UnixNanos,
) -> VenueTruthDivergence {
    VenueTruthDivergence {
        kind,
        alarm_class,
        previous_captured_at,
        current_captured_at,
    }
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
