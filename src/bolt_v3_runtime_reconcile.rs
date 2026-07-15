//! Venue-neutral runtime reconciliation projections.
//!
//! This module deliberately owns no NautilusTrader actor or cache handles. A
//! runtime adapter observes typed order/position snapshots, asks these helpers
//! what should happen, and remains responsible for applying the returned action
//! through its venue/runtime boundary.

use nautilus_model::{
    enums::{OrderSide, OrderStatus, PositionSide},
    identifiers::{ClientOrderId, InstrumentId, PositionId},
    types::Quantity,
};

use crate::bolt_v3_decision_evidence::{
    BoltV3OrderLifecycleOutcome, BoltV3OrderLifecycleTransition,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReconcileOrder {
    pub client_order_id: ClientOrderId,
    pub submitted_at_ms: Option<u64>,
    pub instrument_id: InstrumentId,
    pub market_id: Option<String>,
    pub position_id: Option<PositionId>,
    pub failure_outcome: BoltV3OrderLifecycleOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueVenueOrderQuery {
    pub client_order_id: ClientOrderId,
    pub instrument_id: InstrumentId,
    pub market_id: Option<String>,
    pub position_id: Option<PositionId>,
    pub failure_outcome: BoltV3OrderLifecycleOutcome,
}

impl From<&RuntimeReconcileOrder> for IssueVenueOrderQuery {
    fn from(query: &RuntimeReconcileOrder) -> Self {
        Self {
            client_order_id: query.client_order_id,
            instrument_id: query.instrument_id,
            market_id: query.market_id.clone(),
            position_id: query.position_id,
            failure_outcome: query.failure_outcome,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueOrderQueryFailure {
    RuntimeNotRegistered,
    CachedOrderMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueOrderQueryDecision {
    Issue(IssueVenueOrderQuery),
    FailClosed {
        action: IssueVenueOrderQuery,
        failure: VenueOrderQueryFailure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CachedPositionSnapshot {
    pub instrument_id: InstrumentId,
    pub position_id: PositionId,
    pub entry_order_side: OrderSide,
    pub side: PositionSide,
    pub quantity: Quantity,
    pub avg_px_open: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterializeCachedPosition {
    pub instrument_id: InstrumentId,
    pub position_id: PositionId,
    pub entry_order_side: OrderSide,
    pub side: PositionSide,
    pub quantity: Quantity,
    pub avg_px_open: f64,
}

/// Projects a due runtime query. `submitted_at_ms=None` intentionally remains
/// immediately due, preserving restart/recovery behavior.
pub fn reconcile_runtime_venue_state(
    query: Option<&RuntimeReconcileOrder>,
    now_ms: u64,
    minimum_age_ms: u64,
) -> Option<IssueVenueOrderQuery> {
    let query = query?;
    let due = query
        .submitted_at_ms
        .is_none_or(|submitted_at_ms| now_ms.saturating_sub(submitted_at_ms) >= minimum_age_ms);
    due.then(|| IssueVenueOrderQuery::from(query))
}

/// Decides whether the adapter may issue a venue query. The adapter still owns
/// the cached order handle and the actual query side effect.
pub fn query_order_for_reconcile(
    action: IssueVenueOrderQuery,
    runtime_registered: bool,
    cached_order_present: bool,
) -> VenueOrderQueryDecision {
    let failure = if !runtime_registered {
        Some(VenueOrderQueryFailure::RuntimeNotRegistered)
    } else if !cached_order_present {
        Some(VenueOrderQueryFailure::CachedOrderMissing)
    } else {
        None
    };
    match failure {
        Some(failure) => VenueOrderQueryDecision::FailClosed { action, failure },
        None => VenueOrderQueryDecision::Issue(action),
    }
}

/// Materializes a cache observation only when it belongs to the queried
/// instrument and the runtime independently observed that position as open.
pub fn materialize_cached_position(
    query: &IssueVenueOrderQuery,
    cached: Option<CachedPositionSnapshot>,
    cached_position_is_open: bool,
) -> Option<MaterializeCachedPosition> {
    let cached = cached?;
    if cached.instrument_id != query.instrument_id || !cached_position_is_open {
        return None;
    }
    Some(MaterializeCachedPosition {
        instrument_id: cached.instrument_id,
        position_id: cached.position_id,
        entry_order_side: cached.entry_order_side,
        side: cached.side,
        quantity: cached.quantity,
        avg_px_open: cached.avg_px_open,
    })
}

/// Unknown and non-terminal statuses fail closed by producing no transition.
pub fn reconcile_transition_for_order_status(
    status: OrderStatus,
) -> Option<BoltV3OrderLifecycleTransition> {
    match status {
        OrderStatus::Denied => Some(BoltV3OrderLifecycleTransition::OrderDenied),
        OrderStatus::Rejected => Some(BoltV3OrderLifecycleTransition::OrderRejected),
        OrderStatus::Canceled => Some(BoltV3OrderLifecycleTransition::OrderCanceled),
        OrderStatus::Expired => Some(BoltV3OrderLifecycleTransition::OrderExpired),
        OrderStatus::Filled => Some(BoltV3OrderLifecycleTransition::OrderFilled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(submitted_at_ms: Option<u64>) -> RuntimeReconcileOrder {
        RuntimeReconcileOrder {
            client_order_id: ClientOrderId::from("O-1"),
            submitted_at_ms,
            instrument_id: InstrumentId::from("YES.VENUE"),
            market_id: Some("MKT-1".to_string()),
            position_id: Some(PositionId::from("P-1")),
            failure_outcome: BoltV3OrderLifecycleOutcome::ExitPending,
        }
    }

    #[test]
    fn unknown_order_status_fails_closed() {
        assert_eq!(
            reconcile_transition_for_order_status(OrderStatus::Accepted),
            None
        );
    }

    #[test]
    fn due_query_projects_neutral_venue_action() {
        let query = query(Some(1_000));
        assert!(reconcile_runtime_venue_state(Some(&query), 1_099, 100).is_none());
        let action = reconcile_runtime_venue_state(Some(&query), 1_100, 100)
            .expect("query should become due at the minimum age");
        assert_eq!(action.client_order_id, query.client_order_id);
        assert_eq!(action.instrument_id, query.instrument_id);
        assert_eq!(action.market_id, query.market_id);
        assert_eq!(action.position_id, query.position_id);
        assert_eq!(action.failure_outcome, query.failure_outcome);
    }

    #[test]
    fn cached_position_materialization_requires_matching_open_snapshot() {
        let action = IssueVenueOrderQuery::from(&query(None));
        let snapshot = CachedPositionSnapshot {
            instrument_id: action.instrument_id,
            position_id: PositionId::from("P-1"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::from("2.5"),
            avg_px_open: 0.42,
        };
        assert!(materialize_cached_position(&action, Some(snapshot), false).is_none());
        let materialized = materialize_cached_position(&action, Some(snapshot), true)
            .expect("matching open cache snapshot should materialize");
        assert_eq!(materialized.position_id, snapshot.position_id);
        assert_eq!(materialized.quantity, snapshot.quantity);
    }
}
