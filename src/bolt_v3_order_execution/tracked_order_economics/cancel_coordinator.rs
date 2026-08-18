use anyhow::Result;
use std::sync::RwLockWriteGuard;

use nautilus_model::{
    enums::{OrderSide, OrderStatus},
    identifiers::{ClientId, ClientOrderId, InstrumentId, VenueOrderId},
    orders::{Order, OrderAny},
};
use rust_decimal::Decimal;

use super::{
    BoltV3OrderEconomicsHandle, BoltV3RestingOrderDrainCapability, RestingRegistrationState,
    RestingRegistryHealth, RestingRegistryLifecycle, RetentionHorizonCapability,
    TrackedMakerOrderRecord, TrackedMakerOrderRegistry, apply_retention_horizon,
};
#[cfg(test)]
use super::{MakerQuoteOrderAuthority, MakerQuoteRetainedTerminal};
use crate::bolt_v3_numeric::NANOS_PER_MILLI_U64;
use crate::bolt_v3_order_execution::{BoltV3NtVenueMutationSink, BoltV3OrderExecutionPolicy};
use crate::bolt_v3_quote_lifecycle::{Leg, MakerQuoteLifecycleHandle, MarketQuote};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancelOperationKind {
    Cancel,
    Query,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancelRoutingState {
    Ready,
    Attempting {
        generation: u64,
        operation: CancelOperationKind,
        not_before_ns: u64,
    },
    Backoff {
        not_before_ns: u64,
    },
    PendingCancel {
        not_before_ns: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancelObservation {
    MissingUnqueryable,
    MissingQueryable,
    Retryable,
    PendingCancelUnqueryable,
    PendingCancelQueryable,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancelTransition {
    NoOperation,
    Begin(CancelOperationKind),
    Remove,
}

enum CancelEvent<'a> {
    TimerObserved {
        cached: Option<&'a OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    },
    PassiveObserved {
        cached: Option<&'a OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    },
    OperationSucceeded {
        generation: u64,
        cached: Option<&'a OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    },
    OperationUnobserved {
        generation: u64,
    },
    ReservationGranted {
        operation: CancelOperationKind,
        now_ns: u64,
        retry_timeout_ns: u64,
        escalation_attempts: u32,
    },
    ReservationDenied {
        now_ns: u64,
        retry_timeout_ns: u64,
    },
    RequoteReservationDenied,
}

#[derive(Clone, Debug)]
enum CancelEffect {
    None,
    Remove,
    Cancel {
        generation: u64,
    },
    Query {
        generation: u64,
        seed: Box<OrderAny>,
    },
    ReservationRequired {
        operation: CancelOperationKind,
    },
    RetireIntent,
}

enum CancelOperationCompletion<'a> {
    Unobserved(anyhow::Error),
    Observed {
        cached: Option<&'a OrderAny>,
        now_ns: u64,
    },
}

pub(super) struct CancelDriveInput<'a> {
    pub(super) execution_client_id: &'a str,
    pub(super) client_order_id: ClientOrderId,
    pub(super) cached: Option<&'a OrderAny>,
    pub(super) now_ns: u64,
    pub(super) command_participant:
        Option<Box<dyn super::BoltV3RestingRegistrationCommitParticipant>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RecoveryIdentityConflict {
    captured: VenueOrderId,
    observed: VenueOrderId,
}

impl BoltV3RecoveryIdentityConflict {
    pub const fn captured(&self) -> VenueOrderId {
        self.captured
    }

    pub const fn observed(&self) -> VenueOrderId {
        self.observed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3CancellationLivenessFailure {
    CancellationDeadlineExceeded,
    StuckPendingCancel,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct RestingOrderCancelHealth {
    recovery_identity_unavailable: bool,
    recovery_identity_conflict: Option<BoltV3RecoveryIdentityConflict>,
    retry_escalated: bool,
    liveness: Option<BoltV3CancellationLivenessFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RestingOrderCancelHealthSnapshot {
    client_order_id: ClientOrderId,
    total_recovery_attempts: u32,
    recovery_identity_unavailable: bool,
    recovery_identity_conflict: Option<BoltV3RecoveryIdentityConflict>,
    retry_escalated: bool,
    liveness: Option<BoltV3CancellationLivenessFailure>,
}

impl BoltV3RestingOrderCancelHealthSnapshot {
    pub const fn client_order_id(&self) -> ClientOrderId {
        self.client_order_id
    }

    pub const fn total_recovery_attempts(&self) -> u32 {
        self.total_recovery_attempts
    }

    pub const fn recovery_identity_unavailable(&self) -> bool {
        self.recovery_identity_unavailable
    }

    pub const fn recovery_identity_conflict(&self) -> Option<&BoltV3RecoveryIdentityConflict> {
        self.recovery_identity_conflict.as_ref()
    }

    pub const fn retry_escalated(&self) -> bool {
        self.retry_escalated
    }

    pub const fn liveness(&self) -> Option<BoltV3CancellationLivenessFailure> {
        self.liveness
    }

    pub(super) fn runtime_error(&self) -> Option<anyhow::Error> {
        if !self.recovery_identity_unavailable
            && self.recovery_identity_conflict.is_none()
            && !self.retry_escalated
            && self.liveness.is_none()
        {
            return None;
        }

        let mut facets = Vec::new();
        if self.recovery_identity_unavailable {
            facets.push("recovery_identity_unavailable=true".to_string());
        }
        if let Some(conflict) = &self.recovery_identity_conflict {
            facets.push(format!(
                "recovery_identity_conflict={{captured={},observed={}}}",
                conflict.captured, conflict.observed
            ));
        }
        if self.retry_escalated {
            facets.push("retry_escalated=true".to_string());
        }
        if let Some(liveness) = self.liveness {
            facets.push(format!("liveness={liveness:?}"));
        }
        Some(anyhow::anyhow!(
            "resting cancellation health failure: client_order_id={} total_recovery_attempts={} {}",
            self.client_order_id,
            self.total_recovery_attempts,
            facets.join(" ")
        ))
    }
}

#[derive(Clone, Debug)]
pub(super) struct NtOrderQuerySeed {
    order: OrderAny,
}

impl NtOrderQuerySeed {
    pub(super) fn new(order: OrderAny) -> Self {
        Self { order }
    }

    fn as_query_order(&self) -> &OrderAny {
        &self.order
    }

    fn venue_order_id(&self) -> Option<VenueOrderId> {
        self.order.venue_order_id()
    }

    fn instrument_id(&self) -> nautilus_model::identifiers::InstrumentId {
        self.order.instrument_id()
    }

    fn order_side(&self) -> nautilus_model::enums::OrderSide {
        self.order.order_side()
    }

    fn reconcile_cached_identity(
        &mut self,
        cached: Option<&OrderAny>,
    ) -> Result<IdentityTransition> {
        let Some(cached) = cached else {
            return Ok(IdentityTransition::Preserved);
        };
        anyhow::ensure!(
            cached.client_order_id() == self.order.client_order_id()
                && cached.instrument_id() == self.order.instrument_id(),
            "cached order identity does not match its cancellation query seed"
        );
        match (self.order.venue_order_id(), cached.venue_order_id()) {
            (None, None) => Ok(IdentityTransition::Unchanged),
            (None, Some(_)) => {
                self.order = cached.clone();
                Ok(IdentityTransition::Captured)
            }
            (Some(_), None) => Ok(IdentityTransition::Preserved),
            (Some(captured), Some(observed)) if captured == observed => {
                Ok(IdentityTransition::Unchanged)
            }
            (Some(captured), Some(observed)) => Ok(IdentityTransition::Conflict(
                BoltV3RecoveryIdentityConflict { captured, observed },
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct TrackedOrderCancellation {
    query_seed: NtOrderQuerySeed,
    intent: Option<RestingOrderCancelRecord>,
}

impl TrackedOrderCancellation {
    pub(super) fn new(order: OrderAny) -> Self {
        Self {
            query_seed: NtOrderQuerySeed::new(order),
            intent: None,
        }
    }

    pub(super) fn request_intent(&mut self, quote_deadline_ns: u64) {
        self.intent
            .get_or_insert_with(|| RestingOrderCancelRecord::new(quote_deadline_ns));
    }

    pub(super) const fn is_requested(&self) -> bool {
        self.intent.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IdentityTransition {
    Unchanged,
    Captured,
    Preserved,
    Conflict(BoltV3RecoveryIdentityConflict),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RestingOrderCancelRecord {
    routing_state: CancelRoutingState,
    generation: u64,
    total_recovery_attempts: u32,
    cancel_attempts: u32,
    query_attempts: u32,
    quote_deadline_ns: u64,
    last_observed_ns: Option<u64>,
    health: RestingOrderCancelHealth,
}

impl RestingOrderCancelRecord {
    pub(super) fn new(quote_deadline_ns: u64) -> Self {
        Self {
            routing_state: CancelRoutingState::Ready,
            generation: 0,
            total_recovery_attempts: 0,
            cancel_attempts: 0,
            query_attempts: 0,
            quote_deadline_ns,
            last_observed_ns: None,
            health: RestingOrderCancelHealth::default(),
        }
    }

    fn health_snapshot(
        &self,
        client_order_id: ClientOrderId,
    ) -> BoltV3RestingOrderCancelHealthSnapshot {
        BoltV3RestingOrderCancelHealthSnapshot {
            client_order_id,
            total_recovery_attempts: self.total_recovery_attempts,
            recovery_identity_unavailable: self.health.recovery_identity_unavailable,
            recovery_identity_conflict: self.health.recovery_identity_conflict.clone(),
            retry_escalated: self.health.retry_escalated,
            liveness: self.health.liveness,
        }
    }

    fn apply_event(
        &mut self,
        seed: &mut NtOrderQuerySeed,
        event: CancelEvent<'_>,
    ) -> Result<CancelEffect> {
        match event {
            CancelEvent::TimerObserved {
                cached,
                now_ns,
                retry_timeout_ns,
            } => {
                let transition = self.reconcile(seed, cached, now_ns, retry_timeout_ns)?;
                match transition {
                    CancelTransition::NoOperation => Ok(CancelEffect::None),
                    CancelTransition::Remove => Ok(CancelEffect::Remove),
                    CancelTransition::Begin(operation) => {
                        Ok(CancelEffect::ReservationRequired { operation })
                    }
                }
            }
            CancelEvent::PassiveObserved {
                cached,
                now_ns,
                retry_timeout_ns,
            } => self
                .reconcile(seed, cached, now_ns, retry_timeout_ns)
                .map(passive_effect),
            CancelEvent::OperationSucceeded {
                generation,
                cached,
                now_ns,
                retry_timeout_ns,
            } => {
                if !matches!(
                    self.routing_state,
                    CancelRoutingState::Attempting {
                        generation: active_generation,
                        ..
                    } if active_generation == generation
                ) {
                    return Ok(CancelEffect::None);
                }
                match self.reconcile(seed, cached, now_ns, retry_timeout_ns) {
                    Ok(transition) => Ok(passive_effect(transition)),
                    Err(error) => {
                        self.settle_unobserved_generation(generation);
                        Err(error)
                    }
                }
            }
            CancelEvent::OperationUnobserved { generation } => {
                self.settle_unobserved_generation(generation);
                Ok(CancelEffect::None)
            }
            CancelEvent::ReservationGranted {
                operation,
                now_ns,
                retry_timeout_ns,
                escalation_attempts,
            } => {
                let generation =
                    self.begin_operation(operation, now_ns, retry_timeout_ns, escalation_attempts)?;
                Ok(match operation {
                    CancelOperationKind::Cancel => CancelEffect::Cancel { generation },
                    CancelOperationKind::Query => CancelEffect::Query {
                        generation,
                        seed: Box::new(seed.as_query_order().clone()),
                    },
                })
            }
            CancelEvent::ReservationDenied {
                now_ns,
                retry_timeout_ns,
            } => {
                let not_before_ns = now_ns
                    .checked_add(retry_timeout_ns)
                    .ok_or_else(|| anyhow::anyhow!("cancel recovery deadline overflow"))?;
                self.routing_state = CancelRoutingState::Backoff { not_before_ns };
                Ok(CancelEffect::None)
            }
            CancelEvent::RequoteReservationDenied => Ok(CancelEffect::RetireIntent),
        }
    }

    fn settle_unobserved_generation(&mut self, generation: u64) {
        if let CancelRoutingState::Attempting {
            generation: active_generation,
            not_before_ns,
            ..
        } = self.routing_state
            && active_generation == generation
        {
            self.routing_state = CancelRoutingState::Backoff { not_before_ns };
        }
    }

    fn reconcile(
        &mut self,
        seed: &mut NtOrderQuerySeed,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    ) -> Result<CancelTransition> {
        if self.last_observed_ns.is_some_and(|prior| now_ns < prior) {
            anyhow::bail!(
                "resting cancellation actor clock regressed: prior_ns={} observed_ns={now_ns}",
                self.last_observed_ns.unwrap_or_default()
            );
        }
        if self.health.recovery_identity_conflict.is_some() {
            return Ok(self.observe_identity_conflict_hold(now_ns));
        }
        match seed.reconcile_cached_identity(cached)? {
            IdentityTransition::Conflict(conflict) => {
                self.health.recovery_identity_conflict = Some(conflict);
                return Ok(self.observe_identity_conflict_hold(now_ns));
            }
            IdentityTransition::Unchanged
            | IdentityTransition::Captured
            | IdentityTransition::Preserved => {}
        }

        let observation = classify_order(cached, seed.venue_order_id().is_some());
        if observation == CancelObservation::Terminal {
            self.last_observed_ns = Some(now_ns);
            return Ok(CancelTransition::Remove);
        }
        if matches!(observation, CancelObservation::MissingUnqueryable) {
            self.health.recovery_identity_unavailable = true;
        }
        self.observe_due_liveness(
            now_ns,
            matches!(
                observation,
                CancelObservation::PendingCancelUnqueryable
                    | CancelObservation::PendingCancelQueryable
            ),
        );
        let next_deadline_ns = now_ns
            .checked_add(retry_timeout_ns)
            .ok_or_else(|| anyhow::anyhow!("cancel recovery deadline overflow"))?;
        let (next_state, transition) =
            transition(self.routing_state, observation, now_ns, next_deadline_ns);
        self.routing_state = next_state;
        self.last_observed_ns = Some(now_ns);
        Ok(transition)
    }

    fn observe_identity_conflict_hold(&mut self, now_ns: u64) -> CancelTransition {
        self.observe_due_liveness(
            now_ns,
            matches!(self.routing_state, CancelRoutingState::PendingCancel { .. }),
        );
        self.last_observed_ns = Some(now_ns);
        CancelTransition::NoOperation
    }

    fn observe_due_liveness(&mut self, now_ns: u64, locally_pending: bool) {
        if now_ns < self.quote_deadline_ns {
            return;
        }
        let failure = if locally_pending {
            BoltV3CancellationLivenessFailure::StuckPendingCancel
        } else {
            BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded
        };
        self.health.liveness.get_or_insert(failure);
    }

    fn begin_operation(
        &mut self,
        kind: CancelOperationKind,
        now_ns: u64,
        retry_timeout_ns: u64,
        escalation_attempts: u32,
    ) -> Result<u64> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("cancel recovery generation overflow"))?;
        let total_recovery_attempts = self
            .total_recovery_attempts
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("cancel recovery attempt counter overflow"))?;
        let not_before_ns = now_ns
            .checked_add(retry_timeout_ns)
            .ok_or_else(|| anyhow::anyhow!("cancel recovery deadline overflow"))?;
        match kind {
            CancelOperationKind::Cancel => {
                self.cancel_attempts = self
                    .cancel_attempts
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("cancel attempt counter overflow"))?;
            }
            CancelOperationKind::Query => {
                self.query_attempts = self
                    .query_attempts
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("query attempt counter overflow"))?;
            }
        }
        self.generation = generation;
        self.total_recovery_attempts = total_recovery_attempts;
        self.routing_state = CancelRoutingState::Attempting {
            generation,
            operation: kind,
            not_before_ns,
        };
        if total_recovery_attempts >= escalation_attempts {
            self.health.retry_escalated = true;
        }
        Ok(generation)
    }
}

fn passive_effect(transition: CancelTransition) -> CancelEffect {
    match transition {
        CancelTransition::Remove => CancelEffect::Remove,
        CancelTransition::NoOperation | CancelTransition::Begin(_) => CancelEffect::None,
    }
}

#[derive(Clone, Copy)]
enum CancelCallbackOrigin {
    OrderEvent,
    FillVoid,
}

impl CancelCallbackOrigin {
    const fn as_str(self) -> &'static str {
        match self {
            Self::OrderEvent => "order-event",
            Self::FillVoid => "fill-void",
        }
    }
}

#[derive(Clone, Copy)]
enum CachedMakerOrderObservation<'a> {
    Missing,
    Working(&'a OrderAny),
    PendingCancel(&'a OrderAny),
    Terminal {
        order: &'a OrderAny,
        disposition: crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition,
    },
}

impl<'a> CachedMakerOrderObservation<'a> {
    fn classify(cached: Option<&'a OrderAny>) -> Result<Self> {
        match cached {
            None => Ok(Self::Missing),
            Some(order)
                if order.is_closed() || order.leaves_qty().as_decimal() == Decimal::ZERO =>
            {
                Ok(Self::Terminal {
                    order,
                    disposition: super::maker_terminal_disposition(order)?,
                })
            }
            Some(order) if order.status() == OrderStatus::PendingCancel => {
                Ok(Self::PendingCancel(order))
            }
            Some(order) => Ok(Self::Working(order)),
        }
    }

    const fn order(self) -> Option<&'a OrderAny> {
        match self {
            Self::Missing => None,
            Self::Working(order) | Self::PendingCancel(order) | Self::Terminal { order, .. } => {
                Some(order)
            }
        }
    }
}

fn quarantine_missing_money_relevant_authority(
    registry: &mut TrackedMakerOrderRegistry,
    order: &OrderAny,
    now_ns: u64,
) -> Vec<MakerQuoteLifecycleHandle> {
    registry.health = RestingRegistryHealth::MissingMoneyRelevantAuthority;
    let mut affected_lifecycles = Vec::new();
    for record in registry.records.values_mut() {
        let Some(governed) = record.governed() else {
            continue;
        };
        if !governed.maker_lifecycle.scope.matches(order) {
            continue;
        }
        let quote_deadline_ns = governed
            .economics
            .as_ref()
            .map(|economics| economics.admission.quote().valid_until_ns())
            .unwrap_or(now_ns);
        let lifecycle = governed.maker_lifecycle.lifecycle.clone();
        record.cancellation.request_intent(quote_deadline_ns);
        if !affected_lifecycles
            .iter()
            .any(|retained: &MakerQuoteLifecycleHandle| {
                retained.shares_lifecycle_scope_with(&lifecycle)
            })
        {
            affected_lifecycles.push(lifecycle);
        }
    }
    for retained in registry.retained_terminal_orders.values() {
        if retained.scope.matches(order)
            && !affected_lifecycles
                .iter()
                .any(|lifecycle| lifecycle.shares_lifecycle_scope_with(&retained.lifecycle))
        {
            affected_lifecycles.push(retained.lifecycle.clone());
        }
    }
    affected_lifecycles
}

fn quarantine_missing_reopened_authority(
    registry: &mut TrackedMakerOrderRegistry,
    order: &OrderAny,
    now_ns: u64,
) -> Vec<MakerQuoteLifecycleHandle> {
    let affected_lifecycles = quarantine_missing_money_relevant_authority(registry, order, now_ns);
    let client_order_id = order.client_order_id();
    let strategy_id = order.strategy_id();
    let requote_budget = registry
        .requote_budgets_by_strategy
        .get(&strategy_id)
        .cloned();
    registry.records.entry(client_order_id).or_insert_with(|| {
        TrackedMakerOrderRecord::new_cancellation_only(order.clone(), requote_budget, now_ns)
    });
    affected_lifecycles
}

fn hold_missing_money_relevant_authority(lifecycles: Vec<MakerQuoteLifecycleHandle>) -> Result<()> {
    for lifecycle in lifecycles {
        anyhow::ensure!(
            lifecycle.hold_missing_money_moving_truth(),
            "missing money-relevant maker authority could not hold its lifecycle scope"
        );
    }
    Ok(())
}

impl BoltV3RestingOrderDrainCapability {
    pub fn request_cancellation_at_ns(&self, now_ns: u64) -> Result<usize> {
        let client_order_ids = self.handle.resting_order_ids()?;
        for client_order_id in &client_order_ids {
            self.handle
                .request_cancel_intent(*client_order_id, now_ns)?;
        }
        Ok(client_order_ids.len())
    }

    pub fn resting_order_ids(&self) -> Result<Vec<ClientOrderId>> {
        self.handle.resting_order_ids()
    }

    pub fn finalize_retention_horizon(&mut self) -> Result<usize> {
        apply_retention_horizon(
            &self.handle.tracked_orders,
            RetentionHorizonCapability::ComponentStop {
                drain_generation: self.generation,
            },
        )
    }

    pub fn reopen_after_component_start(&mut self) -> Result<()> {
        let mut registry = self
            .handle
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        anyhow::ensure!(
            registry.lifecycle
                == (RestingRegistryLifecycle::Stopped {
                    generation: self.generation,
                }),
            "resting registration can reopen only from the exact stopped drain generation"
        );
        registry.lifecycle = RestingRegistryLifecycle::Open;
        Ok(())
    }
}

impl BoltV3OrderEconomicsHandle {
    pub fn latch_resting_order_drain_at_ns(
        &self,
        now_ns: u64,
    ) -> Result<(BoltV3RestingOrderDrainCapability, usize)> {
        let generation = {
            let mut registry = self
                .tracked_orders
                .write()
                .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
            anyhow::ensure!(
                registry.lifecycle == RestingRegistryLifecycle::Open,
                "resting order drain latch can only close an open registry"
            );
            let generation = registry
                .next_drain_generation
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("resting order drain generation overflow"))?;
            registry.next_drain_generation = generation;
            registry.lifecycle = RestingRegistryLifecycle::Draining { generation };
            generation
        };
        let capability = BoltV3RestingOrderDrainCapability {
            handle: self.clone(),
            generation,
        };
        let count = capability.request_cancellation_at_ns(now_ns)?;
        Ok((capability, count))
    }

    pub(crate) fn close_maker_quote_scope(
        &self,
        market: &MarketQuote,
        now_ns: u64,
    ) -> Result<usize> {
        let mut closed_market = market.clone();
        let _ = closed_market.close_retention_scope();
        let lifecycles = [
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::No),
        ];
        apply_retention_horizon(
            &self.tracked_orders,
            RetentionHorizonCapability::ScopeClosure {
                lifecycles: &lifecycles,
                now_ns,
            },
        )
    }

    pub(super) fn request_cancel_intent(
        &self,
        client_order_id: ClientOrderId,
        now_ns: u64,
    ) -> Result<bool> {
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let quote_deadline_ns = registry
            .records
            .get(&client_order_id)
            .and_then(TrackedMakerOrderRecord::governed)
            .and_then(|authority| authority.economics.as_ref())
            .map(|economics| economics.admission.quote().valid_until_ns())
            .unwrap_or(now_ns);
        let Some(cancellation) = registry.cancellation_mut(&client_order_id) else {
            return Ok(false);
        };
        cancellation.request_intent(quote_deadline_ns);
        Ok(true)
    }

    pub(super) fn request_cancel_scope(
        &self,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        now_ns: u64,
    ) -> Result<Vec<ClientOrderId>> {
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let selected = registry
            .records
            .iter()
            .filter_map(|(client_order_id, record)| {
                if record.cancellation.query_seed.instrument_id() != instrument_id
                    || order_side
                        .is_some_and(|side| record.cancellation.query_seed.order_side() != side)
                {
                    return None;
                }
                Some(*client_order_id)
            })
            .collect::<Vec<_>>();
        for client_order_id in &selected {
            let quote_deadline_ns = registry
                .records
                .get(client_order_id)
                .and_then(TrackedMakerOrderRecord::governed)
                .and_then(|authority| authority.economics.as_ref())
                .map(|economics| economics.admission.quote().valid_until_ns())
                .unwrap_or(now_ns);
            registry
                .cancellation_mut(client_order_id)
                .expect("selected cancellation must remain tracked")
                .request_intent(quote_deadline_ns);
        }
        Ok(selected)
    }

    pub fn resting_cancel_health(&self) -> Result<Vec<BoltV3RestingOrderCancelHealthSnapshot>> {
        let registry = self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let mut health = registry
            .records
            .iter()
            .filter_map(|(client_order_id, record)| {
                record
                    .cancellation
                    .intent
                    .as_ref()
                    .map(|cancel| cancel.health_snapshot(*client_order_id))
            })
            .collect::<Vec<_>>();
        health.sort_unstable_by_key(BoltV3RestingOrderCancelHealthSnapshot::client_order_id);
        Ok(health)
    }

    pub fn reconcile_tracked_order_at(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<OrderAny>,
        now_ns: u64,
    ) -> Result<()> {
        self.reconcile_tracked_order_inner(
            client_order_id,
            cached,
            now_ns,
            CancelCallbackOrigin::OrderEvent,
        )
    }

    pub fn reconcile_fill_void_at(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<OrderAny>,
        now_ns: u64,
    ) -> Result<()> {
        self.reconcile_tracked_order_inner(
            client_order_id,
            cached,
            now_ns,
            CancelCallbackOrigin::FillVoid,
        )
    }

    fn reconcile_tracked_order_inner(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<OrderAny>,
        now_ns: u64,
        origin: CancelCallbackOrigin,
    ) -> Result<()> {
        let observation = CachedMakerOrderObservation::classify(cached.as_ref())?;
        let registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        match registry.records.get(&client_order_id) {
            Some(_) => {
                self.reconcile_registered_order(registry, client_order_id, observation, now_ns)
            }
            None => self.reconcile_untracked_order(
                registry,
                client_order_id,
                observation,
                now_ns,
                origin,
            ),
        }
    }

    fn reconcile_untracked_order(
        &self,
        registry: RwLockWriteGuard<'_, TrackedMakerOrderRegistry>,
        client_order_id: ClientOrderId,
        observation: CachedMakerOrderObservation<'_>,
        now_ns: u64,
        origin: CancelCallbackOrigin,
    ) -> Result<()> {
        match observation {
            CachedMakerOrderObservation::Missing => Ok(()),
            CachedMakerOrderObservation::Terminal { order, disposition } => self
                .reconcile_untracked_terminal(
                    registry,
                    client_order_id,
                    order,
                    disposition,
                    now_ns,
                ),
            CachedMakerOrderObservation::Working(order)
            | CachedMakerOrderObservation::PendingCancel(order) => self
                .reconcile_untracked_reopening(
                    registry,
                    client_order_id,
                    order,
                    observation,
                    now_ns,
                    origin,
                ),
        }
    }

    fn reconcile_untracked_terminal(
        &self,
        mut registry: RwLockWriteGuard<'_, TrackedMakerOrderRegistry>,
        client_order_id: ClientOrderId,
        order: &OrderAny,
        disposition: crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition,
        now_ns: u64,
    ) -> Result<()> {
        match registry.retained_terminal_orders.remove(&client_order_id) {
            Some(authority) => {
                drop(registry);
                super::settle_maker_terminal_authority(
                    &self.tracked_orders,
                    client_order_id,
                    authority,
                    disposition,
                    now_ns,
                )
            }
            None => match disposition {
                crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Filled
                | crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Voided => {
                    let affected_lifecycles =
                        quarantine_missing_money_relevant_authority(&mut registry, order, now_ns);
                    drop(registry);
                    hold_missing_money_relevant_authority(affected_lifecycles)?;
                    anyhow::bail!(
                        "money-moving maker terminal refinement has no surviving per-order authority: client_order_id={client_order_id} disposition={disposition:?}"
                    )
                }
                crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Denied
                | crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Rejected
                | crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Canceled
                | crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Expired => {
                    log::warn!(
                        "non-money maker terminal refinement has no surviving per-order authority: client_order_id={client_order_id} disposition={disposition:?}"
                    );
                    Ok(())
                }
            },
        }
    }

    fn reconcile_untracked_reopening(
        &self,
        mut registry: RwLockWriteGuard<'_, TrackedMakerOrderRegistry>,
        client_order_id: ClientOrderId,
        order: &OrderAny,
        observation: CachedMakerOrderObservation<'_>,
        now_ns: u64,
        origin: CancelCallbackOrigin,
    ) -> Result<()> {
        let mut authority = match registry.retained_terminal_orders.remove(&client_order_id) {
            Some(authority) => authority,
            None => {
                let affected_lifecycles =
                    quarantine_missing_reopened_authority(&mut registry, order, now_ns);
                drop(registry);
                hold_missing_money_relevant_authority(affected_lifecycles)?;
                anyhow::bail!(
                    "reopened maker order has no identity-fenced lifecycle association: client_order_id={client_order_id} origin={}",
                    origin.as_str(),
                );
            }
        };
        let reopening_event = authority.reopening_event()?;
        drop(registry);
        let outcome = authority.lifecycle.refine(reopening_event);
        let refinement_result = super::consume_lifecycle_refinement_outcome(outcome, "reopening");

        let strategy_id = order.strategy_id();
        let mut coordinator = TrackedOrderCancellation::new(order.clone());
        coordinator.request_intent(now_ns);
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let requote_budget = registry
            .requote_budgets_by_strategy
            .get(&strategy_id)
            .cloned();
        match registry.records.entry(client_order_id) {
            std::collections::btree_map::Entry::Occupied(_) => {}
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(TrackedMakerOrderRecord::new_governed(
                    authority.registration_generation,
                    RestingRegistrationState::Committed,
                    None,
                    requote_budget,
                    authority,
                    coordinator,
                ));
            }
        }
        drop(registry);
        refinement_result?;

        let registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        self.reconcile_registered_order(registry, client_order_id, observation, now_ns)
    }

    fn reconcile_registered_order(
        &self,
        mut registry: RwLockWriteGuard<'_, TrackedMakerOrderRegistry>,
        client_order_id: ClientOrderId,
        observation: CachedMakerOrderObservation<'_>,
        now_ns: u64,
    ) -> Result<()> {
        let retry_timeout_ns = self.economics.cancel_retry_timeout_ns()?;
        let Some(record) = registry.records.get_mut(&client_order_id) else {
            return Ok(());
        };
        match observation {
            CachedMakerOrderObservation::PendingCancel(_) => {
                let quote_deadline_ns = record
                    .governed()
                    .and_then(|authority| authority.economics.as_ref())
                    .map(|economics| economics.admission.quote().valid_until_ns())
                    .unwrap_or(now_ns);
                record.cancellation.request_intent(quote_deadline_ns);
            }
            CachedMakerOrderObservation::Missing
            | CachedMakerOrderObservation::Working(_)
            | CachedMakerOrderObservation::Terminal { .. } => {}
        }
        let TrackedOrderCancellation { query_seed, intent } = &mut record.cancellation;
        let effect = match intent.as_mut() {
            Some(cancellation) => Some(cancellation.apply_event(
                query_seed,
                CancelEvent::PassiveObserved {
                    cached: observation.order(),
                    now_ns,
                    retry_timeout_ns,
                },
            )?),
            None => None,
        };
        match (effect, observation) {
            (
                None | Some(CancelEffect::Remove),
                CachedMakerOrderObservation::Terminal { disposition, .. },
            ) => self.settle_tracked_terminal(registry, client_order_id, disposition, now_ns),
            (None | Some(CancelEffect::None), _) => Ok(()),
            (Some(CancelEffect::Remove), _) => {
                anyhow::bail!("callback reconciliation removed a non-terminal order")
            }
            (Some(CancelEffect::Cancel { .. } | CancelEffect::Query { .. }), _) => {
                anyhow::bail!("callback reconciliation produced an NT operation")
            }
            (Some(CancelEffect::ReservationRequired { .. }), _) => {
                anyhow::bail!("callback reconciliation requested a REST reservation")
            }
            (Some(CancelEffect::RetireIntent), _) => {
                anyhow::bail!("callback reconciliation retired a requote intent")
            }
        }
    }

    fn settle_tracked_terminal(
        &self,
        mut registry: RwLockWriteGuard<'_, TrackedMakerOrderRegistry>,
        client_order_id: ClientOrderId,
        disposition: crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition,
        now_ns: u64,
    ) -> Result<()> {
        let removed = registry.remove_terminal_record(&client_order_id);
        drop(registry);
        super::settle_maker_terminal(
            &self.tracked_orders,
            client_order_id,
            removed,
            disposition,
            now_ns,
        )
    }

    pub(super) fn drive_cancel_intent<S>(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        sink: &mut S,
        input: CancelDriveInput<'_>,
    ) -> Result<()>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        let CancelDriveInput {
            execution_client_id,
            client_order_id,
            cached,
            now_ns,
            mut command_participant,
        } = input;
        let retry_timeout_ns = self.economics.cancel_retry_timeout_ns()?;
        let escalation_attempts = self.economics.cancel_recovery_escalation_attempts();
        let effect = match self.reduce_cancel_drive(
            policy,
            client_order_id,
            cached,
            now_ns,
            retry_timeout_ns,
        ) {
            Ok(effect) => effect,
            Err(error) => {
                return self.finish_cancel_drive(client_order_id, vec![error.to_string()]);
            }
        };
        let effect = match effect {
            CancelEffect::ReservationRequired { operation, .. } => {
                if operation == CancelOperationKind::Cancel
                    && let Some(participant) = command_participant.as_mut()
                {
                    let generation = self.next_cancel_operation_generation(client_order_id)?;
                    let identity = super::MakerQuoteLifecycleIdentity::new(
                        client_order_id.as_str(),
                        generation,
                    );
                    let lifecycle = participant.maker_lifecycle();
                    if participant.arm_at_identity(identity.clone()).is_err() {
                        let retired = self.settle_cancel_reservation(
                            client_order_id,
                            CancelEvent::RequoteReservationDenied,
                        )?;
                        anyhow::ensure!(
                            matches!(retired, CancelEffect::RetireIntent),
                            "requote reservation denial did not retire its cancel intent"
                        );
                        return self.finish_cancel_drive(client_order_id, Vec::new());
                    }
                    let armed = self.settle_cancel_reservation_with_lifecycle(
                        client_order_id,
                        CancelEvent::ReservationGranted {
                            operation,
                            now_ns,
                            retry_timeout_ns,
                            escalation_attempts,
                        },
                        identity,
                        lifecycle,
                    )?;
                    anyhow::ensure!(
                        matches!(armed, CancelEffect::Cancel { generation: armed_generation } if armed_generation == generation),
                        "cancel participant generation did not match the armed coordinator attempt"
                    );
                    (armed, None)
                } else {
                    drop(command_participant.take());
                    let now_ms = now_ns / NANOS_PER_MILLI_U64;
                    let reservation =
                        self.cancel_requote_budget(client_order_id)?
                            .and_then(|budget| {
                                budget
                                    .propose_rest(now_ms)
                                    .and_then(|proposal| budget.reserve(proposal))
                                    .ok()
                            });
                    let Some(reservation) = reservation else {
                        self.settle_cancel_reservation(
                            client_order_id,
                            CancelEvent::ReservationDenied {
                                now_ns,
                                retry_timeout_ns,
                            },
                        )?;
                        return self.finish_cancel_drive(client_order_id, Vec::new());
                    };
                    let armed = self.settle_cancel_reservation(
                        client_order_id,
                        CancelEvent::ReservationGranted {
                            operation,
                            now_ns,
                            retry_timeout_ns,
                            escalation_attempts,
                        },
                    )?;
                    (armed, Some(reservation))
                }
            }
            other => {
                drop(command_participant.take());
                (other, None)
            }
        };
        let (effect, mut reservation) = effect;
        // True means the NT mutation method was invoked. It does not prove that a
        // network request left the process, so the reservation remains charged.
        let (generation, operation_result, nt_mutation_invoked) = match effect {
            CancelEffect::None => {
                return self.finish_cancel_drive(client_order_id, Vec::new());
            }
            CancelEffect::Remove => return Ok(()),
            CancelEffect::RetireIntent => {
                return self.finish_cancel_drive(client_order_id, Vec::new());
            }
            CancelEffect::Cancel { generation } => {
                let pre_sink = (|| -> Result<()> {
                    let pre_sink_now_ns = sink.actor_time_ns()?;
                    if let Some(participant) = command_participant.as_mut() {
                        participant.mark_sink_invoked(pre_sink_now_ns)?;
                    }
                    if let Some(reservation) = reservation.as_mut() {
                        reservation
                            .mark_sink_invoked_at(pre_sink_now_ns / NANOS_PER_MILLI_U64)
                            .map_err(|error| {
                                anyhow::anyhow!(
                                    "cancel REST reservation sink accounting failed: {error:?}"
                                )
                            })?;
                    }
                    Ok(())
                })();
                match pre_sink {
                    Ok(()) => (
                        generation,
                        policy
                            .route_cancel_with_sink(
                                sink,
                                client_order_id,
                                Some(ClientId::from(execution_client_id)),
                                None,
                            )
                            .map(|_| ()),
                        true,
                    ),
                    Err(error) => (generation, Err(error), false),
                }
            }
            CancelEffect::Query { generation, seed } => {
                debug_assert!(command_participant.is_none());
                let pre_sink = (|| -> Result<()> {
                    let pre_sink_now_ns = sink.actor_time_ns()?;
                    let reservation = reservation
                        .as_mut()
                        .expect("query operation must own its REST reservation");
                    reservation
                        .mark_sink_invoked_at(pre_sink_now_ns / NANOS_PER_MILLI_U64)
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "query REST reservation sink accounting failed: {error:?}"
                            )
                        })?;
                    Ok(())
                })();
                match pre_sink {
                    Ok(()) => (
                        generation,
                        sink.query_order_via_nt(
                            &seed,
                            Some(ClientId::from(execution_client_id)),
                            None,
                        ),
                        true,
                    ),
                    Err(error) => (generation, Err(error), false),
                }
            }
            CancelEffect::ReservationRequired { .. } => {
                anyhow::bail!("REST reservation settlement did not arm an operation")
            }
        };
        let mut settlement_failures = Vec::new();
        if let Some(mut participant) = command_participant
            && let Err(error) = match nt_mutation_invoked {
                true => participant.settle_nt_mutation_invoked(generation),
                false => participant.abort_pre_sink(generation),
            }
        {
            settlement_failures.push(error.to_string());
        }
        if nt_mutation_invoked
            && let Some(reservation) = reservation
            && let Err(denial) = reservation.commit()
        {
            settlement_failures.push(format!("REST reservation settlement failed: {denial:?}"));
        }
        let operation_settlement = match operation_result {
            Err(error) => self.settle_cancel_operation(
                client_order_id,
                generation,
                CancelOperationCompletion::Unobserved(error),
                retry_timeout_ns,
            ),
            Ok(()) => {
                let settle_now_ns = match sink.actor_time_ns() {
                    Ok(now_ns) => now_ns,
                    Err(error) => {
                        return self.settle_cancel_operation(
                            client_order_id,
                            generation,
                            CancelOperationCompletion::Unobserved(error),
                            retry_timeout_ns,
                        );
                    }
                };
                match sink.cached_order(client_order_id) {
                    Ok(cached_after) => self.settle_cancel_operation(
                        client_order_id,
                        generation,
                        CancelOperationCompletion::Observed {
                            cached: cached_after.as_ref(),
                            now_ns: settle_now_ns,
                        },
                        retry_timeout_ns,
                    ),
                    Err(error) => self.settle_cancel_operation(
                        client_order_id,
                        generation,
                        CancelOperationCompletion::Unobserved(error),
                        retry_timeout_ns,
                    ),
                }
            }
        };
        if let Err(error) = operation_settlement {
            settlement_failures.push(error.to_string());
        }
        finish_cancel_failures(settlement_failures)
    }

    fn settle_cancel_operation(
        &self,
        client_order_id: ClientOrderId,
        generation: u64,
        completion: CancelOperationCompletion<'_>,
        retry_timeout_ns: u64,
    ) -> Result<()> {
        let mut failures = Vec::new();
        let terminal_observation = match &completion {
            CancelOperationCompletion::Observed {
                cached: Some(cached),
                now_ns,
            } if cached.is_closed() || cached.leaves_qty().as_decimal() == Decimal::ZERO => {
                Some((super::maker_terminal_disposition(cached)?, *now_ns))
            }
            CancelOperationCompletion::Observed { cached: None, .. }
            | CancelOperationCompletion::Observed {
                cached: Some(_), ..
            }
            | CancelOperationCompletion::Unobserved(_) => None,
        };
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(tracked_cancellation) = registry.cancellation_mut(&client_order_id) else {
            if let CancelOperationCompletion::Unobserved(error) = completion {
                failures.push(error.to_string());
            }
            return finish_cancel_failures(failures);
        };

        let TrackedOrderCancellation { query_seed, intent } = tracked_cancellation;
        let Some(cancellation) = intent.as_mut() else {
            anyhow::bail!(
                "armed cancellation operation lost its coordinator record: client_order_id={client_order_id}"
            );
        };
        let effect = match completion {
            CancelOperationCompletion::Unobserved(error) => {
                failures.push(error.to_string());
                cancellation
                    .apply_event(query_seed, CancelEvent::OperationUnobserved { generation })
            }
            CancelOperationCompletion::Observed { cached, now_ns } => cancellation.apply_event(
                query_seed,
                CancelEvent::OperationSucceeded {
                    generation,
                    cached,
                    now_ns,
                    retry_timeout_ns,
                },
            ),
        };
        let mut removed = None;
        match effect {
            Ok(CancelEffect::Remove) => {
                removed = registry.remove_terminal_record(&client_order_id);
            }
            Ok(CancelEffect::None) => {}
            Ok(CancelEffect::Cancel { .. } | CancelEffect::Query { .. }) => {
                failures.push("operation settlement produced another NT operation".to_string());
            }
            Ok(CancelEffect::ReservationRequired { .. }) => {
                failures
                    .push("operation settlement requested another REST reservation".to_string());
            }
            Ok(CancelEffect::RetireIntent) => {
                failures.push("operation settlement retired a requote intent".to_string());
            }
            Err(error) => failures.push(error.to_string()),
        }
        drop(registry);
        match (removed, terminal_observation) {
            (Some(record), Some((disposition, now_ns))) => {
                if let Err(error) = super::settle_maker_terminal(
                    &self.tracked_orders,
                    client_order_id,
                    Some(record),
                    disposition,
                    now_ns,
                ) {
                    failures.push(error.to_string());
                }
            }
            (Some(_), None) => {
                failures.push("terminal cancel settlement lost its disposition".to_string());
            }
            (None, Some(_) | None) => {}
        }
        self.finish_cancel_drive(client_order_id, failures)
    }

    fn reduce_cancel_drive(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        client_order_id: ClientOrderId,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    ) -> Result<CancelEffect> {
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(tracked_cancellation) = registry.cancellation_mut(&client_order_id) else {
            return Ok(CancelEffect::None);
        };
        let TrackedOrderCancellation { query_seed, intent } = tracked_cancellation;
        let Some(cancellation) = intent.as_mut() else {
            return Ok(CancelEffect::None);
        };
        let effect = if policy.allows_venue_mutation() {
            cancellation.apply_event(
                query_seed,
                CancelEvent::TimerObserved {
                    cached,
                    now_ns,
                    retry_timeout_ns,
                },
            )
        } else {
            cancellation.apply_event(
                query_seed,
                CancelEvent::PassiveObserved {
                    cached,
                    now_ns,
                    retry_timeout_ns,
                },
            )
        }?;
        match effect {
            CancelEffect::Remove => {
                let disposition = super::maker_terminal_disposition(
                    cached.expect("terminal cancel drive requires an order"),
                )?;
                let removed = registry.remove_terminal_record(&client_order_id);
                drop(registry);
                if let Some(removed) = removed {
                    super::settle_maker_terminal(
                        &self.tracked_orders,
                        client_order_id,
                        Some(removed),
                        disposition,
                        now_ns,
                    )?;
                }
                Ok(CancelEffect::Remove)
            }
            other => Ok(other),
        }
    }

    fn cancel_requote_budget(
        &self,
        client_order_id: ClientOrderId,
    ) -> Result<Option<crate::bolt_v3_requote_budget::RequoteBudgetPair>> {
        let registry = self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        Ok(registry.requote_budget(&client_order_id))
    }

    fn next_cancel_operation_generation(&self, client_order_id: ClientOrderId) -> Result<u64> {
        let registry = self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let cancellation = registry
            .cancellation(&client_order_id)
            .and_then(|cancellation| cancellation.intent.as_ref())
            .ok_or_else(|| anyhow::anyhow!("cancel participant lost its coordinator record"))?;
        cancellation
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("cancel recovery generation overflow"))
    }

    fn settle_cancel_reservation(
        &self,
        client_order_id: ClientOrderId,
        event: CancelEvent<'_>,
    ) -> Result<CancelEffect> {
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        Self::apply_cancel_reservation_event(&mut registry, client_order_id, event)
    }

    fn settle_cancel_reservation_with_lifecycle(
        &self,
        client_order_id: ClientOrderId,
        event: CancelEvent<'_>,
        identity: super::MakerQuoteLifecycleIdentity,
        active: crate::bolt_v3_quote_lifecycle::MakerQuoteLifecycleHandle,
    ) -> Result<CancelEffect> {
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        {
            let Some(record) = registry.records.get(&client_order_id) else {
                return Ok(CancelEffect::None);
            };
            let governed = record.governed().ok_or_else(|| {
                anyhow::anyhow!(
                    "cancel lifecycle participant cannot bind an authority-less reopening"
                )
            })?;
            anyhow::ensure!(
                identity.client_order_id() == client_order_id.as_str(),
                "cancel lifecycle identity does not match its tracked order"
            );
            let tracked = &governed.maker_lifecycle;
            anyhow::ensure!(
                tracked.lifecycle.shares_authority_with(&active),
                "cancel lifecycle participant does not own the tracked lifecycle authority"
            );
        }
        let effect = Self::apply_cancel_reservation_event(&mut registry, client_order_id, event)?;
        anyhow::ensure!(
            matches!(effect, CancelEffect::Cancel { generation } if generation == identity.generation()),
            "cancel lifecycle identity did not match the armed coordinator attempt"
        );
        registry
            .records
            .get_mut(&client_order_id)
            .expect("validated governed maker order must remain tracked")
            .governed_mut()
            .expect("validated governed maker order must retain exact authority")
            .maker_lifecycle
            .rebind_identity(identity)?;
        Ok(effect)
    }

    fn apply_cancel_reservation_event(
        registry: &mut TrackedMakerOrderRegistry,
        client_order_id: ClientOrderId,
        event: CancelEvent<'_>,
    ) -> Result<CancelEffect> {
        let Some(cancellation) = registry.cancellation_mut(&client_order_id) else {
            return Ok(CancelEffect::None);
        };
        let TrackedOrderCancellation { query_seed, intent } = cancellation;
        let Some(cancel) = intent.as_mut() else {
            return Ok(CancelEffect::None);
        };
        let effect = cancel.apply_event(query_seed, event)?;
        if matches!(effect, CancelEffect::RetireIntent) {
            *intent = None;
        }
        Ok(effect)
    }

    fn finish_cancel_drive(
        &self,
        client_order_id: ClientOrderId,
        mut failures: Vec<String>,
    ) -> Result<()> {
        let registry = self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        if let Some(error) = registry
            .cancellation(&client_order_id)
            .and_then(|cancellation| cancellation.intent.as_ref())
            .and_then(|cancel| cancel.health_snapshot(client_order_id).runtime_error())
        {
            failures.push(error.to_string());
        }
        finish_cancel_failures(failures)
    }
}

fn finish_cancel_failures(failures: Vec<String>) -> Result<()> {
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{}", failures.join(" | "))
    }
}

fn classify_order(order: Option<&OrderAny>, queryable: bool) -> CancelObservation {
    let Some(order) = order else {
        return if queryable {
            CancelObservation::MissingQueryable
        } else {
            CancelObservation::MissingUnqueryable
        };
    };
    if order.leaves_qty().as_decimal().is_zero() {
        return CancelObservation::Terminal;
    }
    classify_status(order.status(), queryable)
}

fn classify_status(status: OrderStatus, queryable: bool) -> CancelObservation {
    match status {
        OrderStatus::Initialized
        | OrderStatus::Emulated
        | OrderStatus::Released
        | OrderStatus::Submitted
        | OrderStatus::Accepted
        | OrderStatus::Triggered
        | OrderStatus::PendingUpdate
        | OrderStatus::PartiallyFilled => CancelObservation::Retryable,
        OrderStatus::PendingCancel => {
            if queryable {
                CancelObservation::PendingCancelQueryable
            } else {
                CancelObservation::PendingCancelUnqueryable
            }
        }
        OrderStatus::Denied
        | OrderStatus::Rejected
        | OrderStatus::Canceled
        | OrderStatus::Expired
        | OrderStatus::Filled
        | OrderStatus::Voided => CancelObservation::Terminal,
    }
}

fn transition(
    state: CancelRoutingState,
    observation: CancelObservation,
    now_ns: u64,
    next_deadline_ns: u64,
) -> (CancelRoutingState, CancelTransition) {
    use CancelObservation::{
        MissingQueryable, MissingUnqueryable, PendingCancelQueryable, PendingCancelUnqueryable,
        Retryable, Terminal,
    };
    use CancelOperationKind::{Cancel, Query};
    use CancelRoutingState::{Attempting, Backoff, PendingCancel, Ready};
    use CancelTransition::{Begin, NoOperation, Remove};

    match (state, observation) {
        (Ready, MissingUnqueryable) => (Ready, NoOperation),
        (Ready, MissingQueryable) => (Ready, Begin(Query)),
        (Ready, Retryable) => (Ready, Begin(Cancel)),
        (Ready, PendingCancelUnqueryable | PendingCancelQueryable) => (
            PendingCancel {
                not_before_ns: next_deadline_ns,
            },
            NoOperation,
        ),
        (Ready, Terminal) => (Ready, Remove),

        (Attempting { not_before_ns, .. }, MissingUnqueryable | MissingQueryable | Retryable) => {
            (Backoff { not_before_ns }, NoOperation)
        }
        (Attempting { not_before_ns, .. }, PendingCancelUnqueryable | PendingCancelQueryable) => {
            (PendingCancel { not_before_ns }, NoOperation)
        }
        (Attempting { .. }, Terminal) => (Ready, Remove),

        (Backoff { not_before_ns }, MissingUnqueryable) => (Backoff { not_before_ns }, NoOperation),
        (Backoff { not_before_ns }, MissingQueryable) => (
            Backoff { not_before_ns },
            begin_when_due(Query, now_ns, not_before_ns),
        ),
        (Backoff { not_before_ns }, Retryable) => (
            Backoff { not_before_ns },
            begin_when_due(Cancel, now_ns, not_before_ns),
        ),
        (Backoff { not_before_ns }, PendingCancelUnqueryable | PendingCancelQueryable) => {
            (PendingCancel { not_before_ns }, NoOperation)
        }
        (Backoff { .. }, Terminal) => (Ready, Remove),

        (PendingCancel { not_before_ns }, MissingUnqueryable) => {
            (Backoff { not_before_ns }, NoOperation)
        }
        (PendingCancel { not_before_ns }, MissingQueryable) => (
            Backoff { not_before_ns },
            begin_when_due(Query, now_ns, not_before_ns),
        ),
        (PendingCancel { not_before_ns }, Retryable) => (
            Backoff { not_before_ns },
            begin_when_due(Cancel, now_ns, not_before_ns),
        ),
        (PendingCancel { not_before_ns }, PendingCancelUnqueryable) => {
            (PendingCancel { not_before_ns }, NoOperation)
        }
        (PendingCancel { not_before_ns }, PendingCancelQueryable) => (
            PendingCancel { not_before_ns },
            begin_when_due(Query, now_ns, not_before_ns),
        ),
        (PendingCancel { .. }, Terminal) => (Ready, Remove),
    }
}

fn begin_when_due(
    operation: CancelOperationKind,
    now_ns: u64,
    not_before_ns: u64,
) -> CancelTransition {
    match now_ns >= not_before_ns {
        true => CancelTransition::Begin(operation),
        false => CancelTransition::NoOperation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::Params;
    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{LiquiditySide, OrderSide, OrderType},
        events::{OrderEventAny, OrderFilled},
        identifiers::{AccountId, InstrumentId, StrategyId, TradeId, TraderId},
        orders::{LimitOrder, stubs::TestOrderEventStubs},
        types::{Currency, Price, Quantity},
    };

    use crate::{
        bolt_v3_maker_order_dispatch::{
            MakerQuoteTransactionContext, maker_quote_transaction_participant_for_test,
        },
        bolt_v3_maker_quote_control::{QuoteControlInput, drive_quote_leg},
        bolt_v3_order_execution::{
            BoltV3NtVenueMutationSink, BoltV3RestingRegistrationCommitParticipant,
            BoltV3RestingRegistrationRejectionKind, BoltV3RestingSubmitTransactionOutcome,
            BoltV3SubmitContext,
        },
        bolt_v3_quote_lifecycle::{
            Leg, LegEvent, LegState, LifecycleAction, MakerOrderLifecycleScopeIdentity,
            MakerQuoteBudgetProposal, MakerQuoteLifecycleHandle, MakerQuoteLifecycleIdentity,
            MarketAction, MarketQuote,
        },
        bolt_v3_requote_budget::{RequoteBudget, RequoteBudgetPair},
    };

    #[derive(Debug)]
    struct CoordinatorSink {
        now_ns: u64,
        cached: Option<OrderAny>,
        cancel_calls: usize,
        query_calls: usize,
        cancel_error: Option<&'static str>,
    }

    impl BoltV3NtVenueMutationSink for CoordinatorSink {
        fn actor_time_ns(&mut self) -> Result<u64> {
            Ok(self.now_ns)
        }

        fn cached_order(&mut self, _client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
            Ok(self.cached.clone())
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
            _order: OrderAny,
            _context: BoltV3SubmitContext,
        ) -> Result<()> {
            anyhow::bail!("coordinator test must not submit")
        }

        fn cancel_order_via_nt(
            &mut self,
            _client_order_id: ClientOrderId,
            _client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.cancel_calls += 1;
            match self.cancel_error {
                Some(error) => anyhow::bail!(error),
                None => Ok(()),
            }
        }

        fn modify_order_via_nt(
            &mut self,
            _client_order_id: ClientOrderId,
            _quantity: Quantity,
            _price: Price,
            _client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            anyhow::bail!("coordinator test must not modify")
        }
    }

    fn coordinator_handle_with_budget(
        order: OrderAny,
        rest_cap: u64,
    ) -> BoltV3OrderEconomicsHandle {
        let handle =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let client_order_id = order.client_order_id();
        let strategy_id = order.strategy_id();
        let maker_lifecycle = MakerQuoteOrderAuthority::new(
            &order,
            1,
            MakerQuoteLifecycleHandle::new(MarketQuote::new_for_test(false), Leg::Yes),
        );
        let budget = RequoteBudgetPair::new(
            RequoteBudget::new(1, 60_000, 0),
            RequoteBudget::new(rest_cap, 60_000, 0),
        );
        let mut registry = handle.tracked_orders.write().expect("registry should lock");
        registry
            .requote_budgets_by_strategy
            .insert(strategy_id, budget.clone());
        registry.records.insert(
            client_order_id,
            TrackedMakerOrderRecord::new_governed(
                1,
                RestingRegistrationState::Committed,
                None,
                Some(budget),
                maker_lifecycle,
                TrackedOrderCancellation::new(order),
            ),
        );
        drop(registry);
        handle
    }

    fn requote_cancel_participant() -> (
        Box<dyn BoltV3RestingRegistrationCommitParticipant>,
        MarketQuote,
        RequoteBudgetPair,
    ) {
        let mut market = MarketQuote::new_for_test(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: true,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let mut budget = RequoteBudgetPair::new(
            RequoteBudget::new(1, 60_000, 0),
            RequoteBudget::new(2, 60_000, 0),
        );
        let decision = drive_quote_leg(
            &mut market,
            &mut budget,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: Some(0.5),
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: 1,
            },
        );
        let participant =
            maker_quote_transaction_participant_for_test(MakerQuoteTransactionContext::new(
                market.clone(),
                budget.clone(),
                decision.proposal.expect("requote cancel must be proposed"),
            ));
        (participant, market, budget)
    }

    fn initialized_order(client_order_id: &str) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("INSTRUMENT.SOURCE"),
                ClientOrderId::from(client_order_id),
                nautilus_model::enums::OrderSide::Buy,
                Quantity::new(1.0, 2),
                Price::new(0.50, 2),
                nautilus_model::enums::TimeInForce::Gtc,
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
                UUID4::new(),
                UnixNanos::from(1),
            )
            .unwrap(),
        )
    }

    fn accepted_order(client_order_id: &str, venue_order_id: &str) -> OrderAny {
        let mut order = initialized_order(client_order_id);
        let event = TestOrderEventStubs::accepted(
            &order,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from(venue_order_id),
        );
        order.apply(event).unwrap();
        order
    }

    fn fill_canceled_order(order: &mut OrderAny, venue_order_id: &str) {
        fill_canceled_order_with_quantity(order, venue_order_id, Quantity::new(1.0, 2));
    }

    fn fill_canceled_order_with_quantity(
        order: &mut OrderAny,
        venue_order_id: &str,
        last_qty: Quantity,
    ) {
        let fill = OrderFilled::new(
            order.trader_id(),
            order.strategy_id(),
            order.instrument_id(),
            order.client_order_id(),
            VenueOrderId::from(venue_order_id),
            AccountId::from("ACCOUNT-001"),
            TradeId::from("TRADE-LATE-FILL"),
            OrderSide::Buy,
            OrderType::Limit,
            last_qty,
            Price::new(0.50, 2),
            Currency::USD(),
            LiquiditySide::Maker,
            UUID4::new(),
            UnixNanos::from(2_u64),
            UnixNanos::from(2_u64),
            false,
            None,
            None,
            None,
        );
        order
            .apply(OrderEventAny::Filled(fill))
            .expect("pinned NT must permit Canceled -> Filled");
    }

    fn retained_canceled_requote(
        client_order_id: &str,
        venue_order_id: &str,
        lifecycle_generation: u64,
    ) -> (
        BoltV3OrderEconomicsHandle,
        MarketQuote,
        RequoteBudgetPair,
        OrderAny,
    ) {
        let mut order = accepted_order(client_order_id, venue_order_id);
        let client_order_id = order.client_order_id();
        let mut market = MarketQuote::new_for_test(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let budget = RequoteBudgetPair::new(
            RequoteBudget::new(8, 60_000, 0),
            RequoteBudget::new(8, 60_000, 0),
        );
        let cancel = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("resting requote should propose a cancel");
        let budget_proposal = MakerQuoteBudgetProposal::Reserve(
            budget
                .propose_cancel_resubmit(1)
                .expect("requote budget should be available"),
        );
        market
            .arm_leg_transaction(
                cancel,
                budget.clone(),
                budget_proposal,
                MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), lifecycle_generation),
            )
            .expect("requote cancel should arm");
        market
            .mark_leg_transaction_sink_invoked(cancel, lifecycle_generation, 1)
            .expect("requote cancel should reach the sink");
        assert!(market.commit_leg_transaction(cancel, lifecycle_generation));

        let handle = coordinator_handle_with_budget(order.clone(), 8);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("tracked record should exist")
            .governed_mut()
            .expect("tracked record should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new_at_lifecycle_generation(
            &order,
            1,
            lifecycle_generation,
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
        );

        let canceled = TestOrderEventStubs::canceled(
            &order,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from(venue_order_id)),
        );
        order.apply(canceled).expect("cancel should apply");
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 1)
            .expect("cancel callback should retain refinable per-order truth");
        (handle, market, budget, order)
    }

    fn arm_replacement(
        market: &MarketQuote,
        budget: RequoteBudgetPair,
        client_order_id: ClientOrderId,
        lifecycle_generation: u64,
    ) -> crate::bolt_v3_quote_lifecycle::QuoteLegTransitionProposal {
        let replacement = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("retained replacement capacity should propose a submit");
        let prepaid_generation = market
            .prepaid_generation(Leg::Yes)
            .expect("replacement capacity should remain prepaid");
        market
            .arm_leg_transaction(
                replacement,
                budget,
                MakerQuoteBudgetProposal::Prepaid {
                    generation: prepaid_generation,
                    now_ms: 2,
                },
                MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), lifecycle_generation),
            )
            .expect("replacement should arm");
        replacement
    }

    fn track_order_with_lifecycle(
        handle: &BoltV3OrderEconomicsHandle,
        order: OrderAny,
        registration_generation: u64,
        lifecycle_generation: u64,
        market: &MarketQuote,
    ) {
        let client_order_id = order.client_order_id();
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .insert(
                client_order_id,
                TrackedMakerOrderRecord::new_governed(
                    registration_generation,
                    RestingRegistrationState::Committed,
                    None,
                    None,
                    MakerQuoteOrderAuthority::new_at_lifecycle_generation(
                        &order,
                        registration_generation,
                        lifecycle_generation,
                        MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
                    ),
                    TrackedOrderCancellation::new(order),
                ),
            );
    }

    #[test]
    fn newer_terminal_order_cannot_evict_an_older_reopenable_order() {
        let a_venue_order_id = "VENUE-REOPEN-A";
        let (handle, mut market, budget, mut order_a) =
            retained_canceled_requote("retained-reopen-a", a_venue_order_id, 7);
        let order_a_id = order_a.client_order_id();
        let mut order_b = accepted_order("newer-terminal-b", "VENUE-TERMINAL-B");
        let order_b_id = order_b.client_order_id();
        let replacement = arm_replacement(&market, budget, order_b_id, 8);
        market
            .mark_leg_transaction_sink_invoked(replacement, 8, 2)
            .expect("replacement should reach the sink");
        assert!(market.commit_leg_transaction(replacement, 8));
        assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
        track_order_with_lifecycle(&handle, order_b.clone(), 2, 8, &market);

        let canceled_b = TestOrderEventStubs::canceled(
            &order_b,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from("VENUE-TERMINAL-B")),
        );
        order_b
            .apply(canceled_b)
            .expect("newer cancel should apply");
        handle
            .reconcile_tracked_order_at(order_b_id, Some(order_b), 2)
            .expect("newer terminal callback should reconcile");

        fill_canceled_order_with_quantity(&mut order_a, a_venue_order_id, Quantity::new(0.5, 2));
        handle
            .reconcile_tracked_order_at(order_a_id, Some(order_a), 3)
            .expect("ordinary order-event reopening must restore cancel tracking");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        let reopened = registry
            .records
            .get(&order_a_id)
            .expect("older reopened order must return to active tracking");
        assert_eq!(
            reopened
                .governed()
                .expect("reopened order should retain exact authority")
                .maker_lifecycle
                .client_order_id,
            order_a_id
        );
        assert!(reopened.cancellation.is_requested());
        drop(registry);
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
    }

    #[test]
    fn replacement_sink_rejection_cannot_destroy_prior_order_reopening_truth() {
        let venue_order_id = "VENUE-REOPEN-AFTER-REJECT";
        let (handle, market, budget, mut order_a) =
            retained_canceled_requote("retained-reopen-after-reject", venue_order_id, 7);
        let order_a_id = order_a.client_order_id();
        let replacement_id = ClientOrderId::from("sink-rejected-replacement-b");
        let replacement = arm_replacement(&market, budget, replacement_id, 8);
        market
            .mark_leg_transaction_sink_invoked(replacement, 8, 2)
            .expect("replacement should reach the sink");
        assert!(market.reject_leg_transaction_at_sink(replacement, 8));

        fill_canceled_order_with_quantity(&mut order_a, venue_order_id, Quantity::new(0.5, 2));
        handle
            .reconcile_tracked_order_at(order_a_id, Some(order_a), 3)
            .expect("prior order truth must survive replacement sink rejection");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        let reopened = registry
            .records
            .get(&order_a_id)
            .expect("reopened order must return to active tracking");
        assert_eq!(
            reopened
                .governed()
                .expect("reopened order should retain exact authority")
                .maker_lifecycle
                .client_order_id,
            order_a_id
        );
        assert!(reopened.cancellation.is_requested());
        drop(registry);
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
    }

    #[test]
    fn late_fill_refines_its_own_retained_order_while_replacement_is_armed() {
        let venue_order_id = "VENUE-LATE-FILL-OWN-RECORD";
        let (handle, market, budget, mut order_a) =
            retained_canceled_requote("late-fill-own-record-a", venue_order_id, 7);
        let order_a_id = order_a.client_order_id();
        let replacement = arm_replacement(
            &market,
            budget,
            ClientOrderId::from("armed-replacement-b"),
            8,
        );
        let before = (
            market.leg_state(Leg::Yes),
            market.prepaid_generation(Leg::Yes),
        );

        fill_canceled_order(&mut order_a, venue_order_id);
        handle
            .reconcile_tracked_order_at(order_a_id, Some(order_a), 3)
            .expect("late fill must settle against order A's retained authority");

        assert_eq!(
            (
                market.leg_state(Leg::Yes),
                market.prepaid_generation(Leg::Yes),
            ),
            before,
            "order A's refinement cannot mutate armed replacement B"
        );
        assert!(
            handle
                .tracked_orders
                .read()
                .expect("registry should lock")
                .retained_terminal_orders
                .contains_key(&order_a_id),
            "Filled remains retained for the pinned Filled -> Voided refinement"
        );
        let _ = replacement;
    }

    #[test]
    fn missing_money_moving_terminal_truth_poison_holds_the_affected_leg() {
        let venue_order_id = "VENUE-MISSING-FILL-A";
        let (handle, mut market, budget, mut order_a) =
            retained_canceled_requote("missing-fill-a", venue_order_id, 7);
        let order_a_id = order_a.client_order_id();
        let order_b = accepted_order("affected-live-b", "VENUE-AFFECTED-LIVE-B");
        let order_b_id = order_b.client_order_id();
        let replacement = arm_replacement(&market, budget, order_b_id, 8);
        market
            .mark_leg_transaction_sink_invoked(replacement, 8, 2)
            .expect("replacement should reach the sink");
        assert!(market.commit_leg_transaction(replacement, 8));
        assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
        track_order_with_lifecycle(&handle, order_b, 2, 8, &market);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .retained_terminal_orders
            .remove(&order_a_id);

        fill_canceled_order(&mut order_a, venue_order_id);
        handle
            .reconcile_tracked_order_at(order_a_id, Some(order_a), 3)
            .expect_err("a fill with no surviving per-order truth must fail closed");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        let affected = registry
            .records
            .get(&order_b_id)
            .expect("the exact-scope live order must remain tracked");
        assert!(
            affected.cancellation.is_requested(),
            "missing money-moving truth must request cancellation for the exact affected scope"
        );
        drop(registry);

        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert!(
            market
                .propose_leg_event(
                    Leg::Yes,
                    LegEvent::QuoteTrigger {
                        requote_needed: true,
                    },
                )
                .is_none(),
            "typed unhealthy hold must reject the next quote trigger"
        );
        assert_ne!(
            handle
                .tracked_orders
                .read()
                .expect("registry should lock")
                .health,
            super::super::RestingRegistryHealth::Healthy
        );
        let rejected_order =
            super::super::tests::post_only_limit_order("missing-truth-subsequent-submit");
        let rejected = handle.route_resting_submit(
            rejected_order.clone(),
            super::super::tests::sealed_admission(&handle, &rejected_order),
            super::super::tests::test_participant(),
            |_| panic!("poisoned registry must reject before invoking the real route closure"),
        );
        assert!(matches!(
            rejected,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind() == BoltV3RestingRegistrationRejectionKind::RegistryUnavailable
        ));
    }

    fn assert_post_horizon_reopening_without_authority_fails_closed(origin: CancelCallbackOrigin) {
        let venue_order_id = "VENUE-MISSING-REOPEN-A";
        let (handle, mut market, budget, mut order_a) =
            retained_canceled_requote("missing-reopen-a", venue_order_id, 7);
        let order_a_id = order_a.client_order_id();
        let order_b = accepted_order("missing-reopen-live-b", "VENUE-MISSING-REOPEN-B");
        let order_b_id = order_b.client_order_id();
        let replacement = arm_replacement(&market, budget, order_b_id, 8);
        market
            .mark_leg_transaction_sink_invoked(replacement, 8, 2)
            .expect("replacement should reach the sink");
        assert!(market.commit_leg_transaction(replacement, 8));
        assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
        track_order_with_lifecycle(&handle, order_b, 2, 8, &market);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .retained_terminal_orders
            .remove(&order_a_id);

        fill_canceled_order_with_quantity(&mut order_a, venue_order_id, Quantity::new(0.5, 2));
        assert_eq!(order_a.status(), OrderStatus::PartiallyFilled);
        let error = handle
            .reconcile_tracked_order_inner(order_a_id, Some(order_a.clone()), 3, origin)
            .expect_err("a live reopening without retained authority must fail closed");
        assert!(error.to_string().contains("reopened maker order"));
        assert!(error.to_string().contains(origin.as_str()));

        assert!(
            handle
                .resting_order_ids()
                .expect("cancellation tracking should remain readable")
                .contains(&order_a_id),
            "the authority-less reopened order itself must remain tracked for cancellation"
        );
        assert!(
            handle
                .resting_cancel_health()
                .expect("cancellation health should remain readable")
                .iter()
                .any(|snapshot| snapshot.client_order_id() == order_a_id),
            "the authority-less reopened order must own a real cancellation intent"
        );
        let mut sink = CoordinatorSink {
            now_ns: 4,
            cached: Some(order_a.clone()),
            cancel_calls: 0,
            query_calls: 0,
            cancel_error: None,
        };
        let _ = handle.drive_cancel_intent(
            BoltV3OrderExecutionPolicy::live(),
            &mut sink,
            CancelDriveInput {
                execution_client_id: "execution_client",
                client_order_id: order_a_id,
                cached: Some(&order_a),
                now_ns: 4,
                command_participant: None,
            },
        );
        assert_eq!(
            sink.cancel_calls, 1,
            "the authority-less reopening must use the shared per-order cancellation route"
        );

        let registry = handle.tracked_orders.read().expect("registry should lock");
        assert_eq!(
            registry.health,
            super::super::RestingRegistryHealth::MissingMoneyRelevantAuthority
        );
        let affected = registry
            .records
            .get(&order_b_id)
            .expect("the same-scope live order must remain tracked for cancellation");
        assert!(
            affected.cancellation.is_requested(),
            "the absent-authority reopening must request cancellation for its live scope"
        );
        drop(registry);

        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert!(
            market
                .propose_leg_event(
                    Leg::Yes,
                    LegEvent::QuoteTrigger {
                        requote_needed: true,
                    },
                )
                .is_none(),
            "the poisoned lifecycle must reject the next quote trigger"
        );
    }

    #[test]
    fn post_horizon_order_and_fill_void_reopening_without_authority_fail_closed() {
        assert_post_horizon_reopening_without_authority_fails_closed(
            CancelCallbackOrigin::OrderEvent,
        );
        assert_post_horizon_reopening_without_authority_fails_closed(
            CancelCallbackOrigin::FillVoid,
        );
    }

    #[test]
    fn stop_horizon_releases_retained_terminal_authority_and_prepaid_capacity() {
        let (handle, market, budget, _order) =
            retained_canceled_requote("stop-retained-a", "VENUE-STOP-RETAINED-A", 7);
        assert!(market.prepaid_generation(Leg::Yes).is_some());
        let (mut drain, active) = handle.latch_resting_order_drain_at_ns(10).unwrap();
        assert_eq!(active, 0);
        drain.finalize_retention_horizon().unwrap();

        assert!(
            handle
                .tracked_orders
                .read()
                .expect("registry should lock")
                .retained_terminal_orders
                .is_empty(),
            "stop is the final governed retention horizon"
        );
        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
    }

    #[test]
    fn active_authority_survives_a_stop_horizon_until_the_drain_is_empty() {
        let (handle, _market, _budget, _order) =
            retained_canceled_requote("stop-retained-before-active", "VENUE-STOP-A", 7);
        let retained_id = ClientOrderId::from("stop-retained-before-active");
        let active = accepted_order("stop-active-b", "VENUE-STOP-B");
        let active_id = active.client_order_id();
        track_order_with_lifecycle(&handle, active, 2, 8, &MarketQuote::new_for_test(false));

        let (mut drain, active) = handle
            .latch_resting_order_drain_at_ns(10)
            .expect("stop must latch before the horizon is available");
        assert_eq!(active, 1);
        let error = drain
            .finalize_retention_horizon()
            .expect_err("the retention horizon is unavailable while active authority exists");
        assert!(error.to_string().contains("active"));
        let registry = handle.tracked_orders.read().expect("registry should lock");
        assert!(registry.records.contains_key(&active_id));
        assert!(registry.retained_terminal_orders.contains_key(&retained_id));
    }

    #[test]
    fn cadence_scope_horizon_finalizes_only_the_retired_lifecycle() {
        let (retired, retired_market, retired_budget, _order) =
            retained_canceled_requote("cadence-retired", "VENUE-CADENCE-RETIRED", 7);
        let filled_order = accepted_order("cadence-retired-filled", "VENUE-CADENCE-FILLED");
        let filled_id = filled_order.client_order_id();
        let mut filled_authority = MakerQuoteOrderAuthority::new_at_lifecycle_generation(
            &filled_order,
            2,
            8,
            MakerQuoteLifecycleHandle::new(retired_market.clone(), Leg::No),
        );
        filled_authority.retained = Some(MakerQuoteRetainedTerminal::Terminal(
            crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Filled,
        ));
        let live_order = accepted_order("cadence-live", "VENUE-CADENCE-LIVE");
        let live_id = live_order.client_order_id();
        let live_market = MarketQuote::new_for_test(false);
        let mut live_authority = MakerQuoteOrderAuthority::new_at_lifecycle_generation(
            &live_order,
            2,
            8,
            MakerQuoteLifecycleHandle::new(live_market, Leg::Yes),
        );
        live_authority.retained = Some(MakerQuoteRetainedTerminal::Terminal(
            crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Canceled,
        ));
        {
            let mut registry = retired
                .tracked_orders
                .write()
                .expect("registry should lock");
            registry
                .retained_terminal_orders
                .insert(filled_id, filled_authority);
            registry
                .retained_terminal_orders
                .insert(live_id, live_authority);
        }

        retired
            .close_maker_quote_scope(&retired_market, 10)
            .expect("cadence rollover should close the retired lifecycle scope");

        let registry = retired.tracked_orders.read().expect("registry should lock");
        assert_eq!(registry.retained_terminal_orders.len(), 1);
        assert!(registry.retained_terminal_orders.contains_key(&live_id));
        assert!(!registry.retained_terminal_orders.contains_key(&filled_id));
        drop(registry);
        assert_eq!(retired_market.prepaid_generation(Leg::Yes), None);
        assert_eq!(retired_budget.outstanding_submit_cost(), 0);
        assert_eq!(retired_budget.outstanding_rest_cost(), 0);
    }

    #[test]
    fn cadence_scope_closure_requests_cancellation_for_a_working_order() {
        let handle =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let market = MarketQuote::new_for_test(false);
        let order = accepted_order("cadence-working", "VENUE-CADENCE-WORKING");
        let client_order_id = order.client_order_id();
        track_order_with_lifecycle(&handle, order, 1, 1, &market);

        handle
            .close_maker_quote_scope(&market, 10)
            .expect("scope closure should latch and request cancellation");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        let working = registry
            .records
            .get(&client_order_id)
            .expect("the working order remains tracked until terminal");
        assert!(
            working.cancellation.is_requested(),
            "scope closure must request cancellation before retaining finality"
        );
        drop(registry);
        assert!(
            MakerQuoteLifecycleHandle::new(market, Leg::Yes).retention_scope_is_closed(),
            "replacement registration must remain gated while the old scope drains"
        );
    }

    #[test]
    fn scope_closure_uses_observed_actor_time_for_new_cancel_deadline() {
        let handle =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let mut market = MarketQuote::new_for_test(false);
        let mut terminal = accepted_order("cadence-terminal", "VENUE-CADENCE-TERMINAL");
        let terminal_id = terminal.client_order_id();
        let working = accepted_order("cadence-sibling", "VENUE-CADENCE-SIBLING");
        let working_id = working.client_order_id();
        track_order_with_lifecycle(&handle, terminal.clone(), 1, 1, &market);
        track_order_with_lifecycle(&handle, working.clone(), 2, 2, &market);
        let _ = market.close_retention_scope();

        let canceled = TestOrderEventStubs::canceled(
            &terminal,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from("VENUE-CADENCE-TERMINAL")),
        );
        terminal.apply(canceled).expect("cancel should apply");
        handle
            .reconcile_tracked_order_at(terminal_id, Some(terminal), 100)
            .expect("terminal observation should close the retained scope");
        handle
            .reconcile_tracked_order_at(working_id, Some(working), 50)
            .expect("pre-deadline observation should remain healthy");

        let health = handle
            .resting_cancel_health()
            .expect("cancel health should remain available");
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].client_order_id(), working_id);
        assert_eq!(health[0].liveness(), None);
    }

    #[test]
    fn cadence_scope_horizon_rejects_later_registration_for_the_retired_lifecycle() {
        let (handle, retired_market, budget, _order) =
            retained_canceled_requote("cadence-closed", "VENUE-CADENCE-CLOSED", 7);
        let participant =
            super::super::tests::maker_submit_participant(&retired_market, &budget, Leg::No);
        handle
            .close_maker_quote_scope(&retired_market, 10)
            .expect("cadence rollover should close the retired lifecycle scope");

        let order = super::super::tests::post_only_limit_order("cadence-after-close");
        let rejected = handle.route_resting_submit(
            order.clone(),
            super::super::tests::sealed_admission(&handle, &order),
            participant,
            |_| panic!("a closed lifecycle must reject before invoking the route closure"),
        );

        assert!(matches!(
            rejected,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::RetentionScopeClosed
        ));
    }

    #[test]
    fn late_fill_refines_its_order_while_later_generation_occupancy_is_unchanged() {
        const FILTER: &str = "bolt_v3_order_execution::tracked_order_economics::cancel_coordinator::tests::late_fill_refines_its_order_while_later_generation_occupancy_is_unchanged";
        const CASE: &str = "late-maker-refinement-generation-fence";
        if !crate::bolt_v3_test_log_capture::enter_isolated_log_capture(FILTER, CASE) {
            return;
        }

        let client_order_id = "late-fill-after-remove";
        let venue_order_id = "VENUE-LATE-FILL";
        let mut order = accepted_order(client_order_id, venue_order_id);
        let client_order_id = order.client_order_id();
        let mut market = MarketQuote::new_for_test(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let lifecycle_budget = RequoteBudgetPair::new(
            RequoteBudget::new(8, 60_000, 0),
            RequoteBudget::new(8, 60_000, 0),
        );
        let proposal = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("resting requote should propose a cancel");
        let budget_proposal = MakerQuoteBudgetProposal::Reserve(
            lifecycle_budget
                .propose_cancel_resubmit(1)
                .expect("requote budget should be available"),
        );
        market
            .arm_leg_transaction(
                proposal,
                lifecycle_budget.clone(),
                budget_proposal,
                MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), 1),
            )
            .expect("requote cancel should arm");
        market
            .mark_leg_transaction_sink_invoked(proposal, 1, 1)
            .expect("requote cancel should reach the sink");
        assert!(market.commit_leg_transaction(proposal, 1));
        assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);

        let handle = coordinator_handle_with_budget(order.clone(), 8);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("tracked record should exist")
            .governed_mut()
            .expect("tracked record should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new(
            &order,
            1,
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
        );

        let canceled = TestOrderEventStubs::canceled(
            &order,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from(venue_order_id)),
        );
        order.apply(canceled).expect("cancel should apply");
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 1)
            .expect("cancel callback should reconcile");
        assert!(handle.resting_order_ids().unwrap().is_empty());
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );

        let replacement = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("the retained replacement capacity should propose the next submit");
        let prepaid_generation = market
            .prepaid_generation(Leg::Yes)
            .expect("the replacement should retain prepaid capacity");
        market
            .arm_leg_transaction(
                replacement,
                lifecycle_budget,
                MakerQuoteBudgetProposal::Prepaid {
                    generation: prepaid_generation,
                    now_ms: 2,
                },
                MakerQuoteLifecycleIdentity::new("later-generation-order", 2),
            )
            .expect("the later replacement generation should arm");
        let before = (
            market.leg_state(Leg::Yes),
            market.prepaid_generation(Leg::Yes),
        );

        fill_canceled_order(&mut order, venue_order_id);
        let (result, records) = crate::bolt_v3_test_log_capture::with_captured_logs(|| {
            handle.reconcile_tracked_order_at(client_order_id, Some(order), 2)
        });
        result.expect("per-order refinement should not mutate the later leg occupancy");

        assert_eq!(
            (
                market.leg_state(Leg::Yes),
                market.prepaid_generation(Leg::Yes),
            ),
            before,
            "removed order A cannot mutate armed generation B"
        );
        assert!(records.iter().any(|(level, message)| {
            *level == log::Level::Info
                && message.contains("per-order refinement left current leg occupancy unchanged")
                && message.contains("client_order_id=late-fill-after-remove")
                && message.contains("generation=1")
        }));
        assert!(
            handle
                .tracked_orders
                .read()
                .expect("registry should lock")
                .retained_terminal_orders
                .contains_key(&client_order_id)
        );
    }

    #[test]
    fn late_partial_fill_reopens_cancel_tracking_with_its_lifecycle() {
        let client_order_id = "late-partial-after-remove";
        let venue_order_id = "VENUE-LATE-PARTIAL";
        let mut order = accepted_order(client_order_id, venue_order_id);
        let client_order_id = order.client_order_id();
        let mut market = MarketQuote::new_for_test(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let lifecycle_budget = RequoteBudgetPair::new(
            RequoteBudget::new(8, 60_000, 0),
            RequoteBudget::new(8, 60_000, 0),
        );
        let proposal = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("resting requote should propose a cancel");
        let budget_proposal = MakerQuoteBudgetProposal::Reserve(
            lifecycle_budget
                .propose_cancel_resubmit(1)
                .expect("requote budget should be available"),
        );
        market
            .arm_leg_transaction(
                proposal,
                lifecycle_budget,
                budget_proposal,
                MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), 7),
            )
            .expect("requote cancel should arm");
        market
            .mark_leg_transaction_sink_invoked(proposal, 7, 1)
            .expect("requote cancel should reach the sink");
        assert!(market.commit_leg_transaction(proposal, 7));

        let handle = coordinator_handle_with_budget(order.clone(), 8);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("tracked record should exist")
            .governed_mut()
            .expect("tracked record should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new_at_lifecycle_generation(
            &order,
            1,
            7,
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
        );

        let canceled = TestOrderEventStubs::canceled(
            &order,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from(venue_order_id)),
        );
        order.apply(canceled).expect("cancel should apply");
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 1)
            .expect("cancel callback should retire the resting record");
        assert!(
            handle
                .tracked_orders
                .read()
                .expect("registry should lock")
                .retained_terminal_orders
                .contains_key(&client_order_id)
        );

        fill_canceled_order_with_quantity(&mut order, venue_order_id, Quantity::new(0.5, 2));
        assert_eq!(order.status(), OrderStatus::PartiallyFilled);
        assert!(order.leaves_qty().as_decimal() > Decimal::ZERO);
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order), 2)
            .expect("ordinary fill callback should restore cancellation tracking");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        let reopened = registry
            .records
            .get(&client_order_id)
            .expect("reopened working order must be tracked");
        let governed = reopened
            .governed()
            .expect("reopened order should retain exact authority");
        assert_eq!(governed.registration_generation, 1);
        assert_eq!(governed.maker_lifecycle.client_order_id, client_order_id);
        assert!(reopened.cancellation.is_requested());
        assert!(
            !registry
                .retained_terminal_orders
                .contains_key(&client_order_id)
        );
        drop(registry);
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
        assert!(
            market
                .propose_leg_event(
                    Leg::Yes,
                    LegEvent::QuoteTrigger {
                        requote_needed: true,
                    },
                )
                .is_none()
        );
    }

    #[test]
    fn cancel_arm_rebinds_the_tracked_lifecycle_generation() {
        let order = accepted_order("cancel-lifecycle-rebind", "VENUE-CANCEL-REBIND");
        let client_order_id = order.client_order_id();
        let mut market = MarketQuote::new_for_test(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let lifecycle_budget = RequoteBudgetPair::new(
            RequoteBudget::new(8, 60_000, 0),
            RequoteBudget::new(8, 60_000, 0),
        );
        let proposal = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("resting requote should propose a cancel");
        let budget_proposal = MakerQuoteBudgetProposal::Reserve(
            lifecycle_budget
                .propose_cancel_resubmit(1)
                .expect("requote budget should be available"),
        );
        let identity = MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), 7);
        market
            .arm_leg_transaction(
                proposal,
                lifecycle_budget,
                budget_proposal,
                identity.clone(),
            )
            .expect("requote cancel should arm");
        market
            .mark_leg_transaction_sink_invoked(proposal, 7, 1)
            .expect("requote cancel should reach the sink");
        assert!(market.commit_leg_transaction(proposal, 7));

        let lifecycle = MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes);
        let handle = coordinator_handle_with_budget(order.clone(), 8);
        {
            let mut registry = handle.tracked_orders.write().expect("registry should lock");
            let record = registry
                .records
                .get_mut(&client_order_id)
                .expect("tracked record should exist");
            record
                .governed_mut()
                .expect("tracked record should retain exact authority")
                .maker_lifecycle = MakerQuoteOrderAuthority::new(&order, 1, lifecycle.clone());
            record.cancellation.request_intent(0);
            record
                .cancellation
                .intent
                .as_mut()
                .expect("cancel intent should exist")
                .generation = 6;
        }

        let effect = handle
            .settle_cancel_reservation_with_lifecycle(
                client_order_id,
                CancelEvent::ReservationGranted {
                    operation: CancelOperationKind::Cancel,
                    now_ns: 1,
                    retry_timeout_ns: 1,
                    escalation_attempts: 1,
                },
                identity,
                lifecycle,
            )
            .expect("cancel reservation should bind the exact lifecycle generation");
        assert!(matches!(effect, CancelEffect::Cancel { generation: 7 }));

        let removed = handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .remove_terminal_record(&client_order_id);
        super::super::settle_maker_terminal(
            &handle.tracked_orders,
            client_order_id,
            removed,
            crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Canceled,
            1,
        )
        .expect("terminal callback should refine the exact cancel generation");
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
    }

    #[test]
    fn canceled_initial_terminal_callback_retains_per_order_refinement_authority() {
        let client_order_id = "final-initial-terminal";
        let venue_order_id = "VENUE-FINAL-INITIAL";
        let mut order = accepted_order(client_order_id, venue_order_id);
        let client_order_id = order.client_order_id();
        let mut market = MarketQuote::new_for_test(false);
        let lifecycle_budget = RequoteBudgetPair::new(
            RequoteBudget::new(8, 60_000, 0),
            RequoteBudget::new(8, 60_000, 0),
        );
        let proposal = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: false,
                },
            )
            .expect("fresh submit should be proposed");
        let budget_proposal = MakerQuoteBudgetProposal::Reserve(
            lifecycle_budget
                .propose_fresh_submit(1)
                .expect("fresh submit budget should be available"),
        );
        market
            .arm_leg_transaction(
                proposal,
                lifecycle_budget,
                budget_proposal,
                MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), 1),
            )
            .expect("fresh submit should arm");
        market
            .mark_leg_transaction_sink_invoked(proposal, 1, 1)
            .expect("fresh submit should reach the sink");
        assert!(market.commit_leg_transaction(proposal, 1));
        assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
        assert_eq!(
            market.cancel_leg(Leg::Yes),
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            })
        );
        let handle = coordinator_handle_with_budget(order.clone(), 8);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("tracked record should exist")
            .governed_mut()
            .expect("tracked record should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new(
            &order,
            1,
            MakerQuoteLifecycleHandle::new(market, Leg::Yes),
        );

        let canceled = TestOrderEventStubs::canceled(
            &order,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from(venue_order_id)),
        );
        order.apply(canceled).expect("cancel should apply");
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order), 1)
            .expect("terminal callback should settle");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        assert!(registry.records.is_empty());
        assert!(
            registry
                .retained_terminal_orders
                .contains_key(&client_order_id)
        );
    }

    #[test]
    fn finalized_registration_epoch_retires_older_refinable_truth() {
        let first = accepted_order("retention-generation-one", "VENUE-RETENTION-ONE");
        let first_id = first.client_order_id();
        let second = accepted_order("retention-generation-two", "VENUE-RETENTION-TWO");
        let second_id = second.client_order_id();
        let market = MarketQuote::new_for_test(false);
        let lifecycle = MakerQuoteLifecycleHandle::new(market, Leg::Yes);
        let handle = coordinator_handle_with_budget(first.clone(), 8);
        let mut registry = handle.tracked_orders.write().expect("registry should lock");
        registry
            .records
            .get_mut(&first_id)
            .expect("first generation should be tracked")
            .governed_mut()
            .expect("first generation should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new(&first, 1, lifecycle.clone());
        let first_removed = registry
            .remove_terminal_record(&first_id)
            .expect("first generation should retire");
        drop(registry);
        super::super::settle_maker_terminal(
            &handle.tracked_orders,
            first_id,
            Some(first_removed),
            crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Canceled,
            1,
        )
        .expect("first refinable terminal should settle");

        let mut registry = handle.tracked_orders.write().expect("registry should lock");
        registry.records.insert(
            second_id,
            TrackedMakerOrderRecord::new_governed(
                2,
                RestingRegistrationState::Committed,
                None,
                None,
                MakerQuoteOrderAuthority::new(&second, 2, lifecycle),
                TrackedOrderCancellation::new(second.clone()),
            ),
        );
        let lifecycle = registry
            .records
            .get(&second_id)
            .expect("second generation should be tracked")
            .governed()
            .expect("second generation should retain exact authority")
            .maker_lifecycle
            .lifecycle
            .clone();
        drop(registry);
        super::super::apply_retention_horizon(
            &handle.tracked_orders,
            super::super::RetentionHorizonCapability::RegistrationEpochFinal {
                lifecycle: &lifecycle,
                registration_generation: 2,
                current_client_order_id: second_id,
            },
        )
        .expect("final registration epoch should retire the prior refinable truth");
        let mut registry = handle.tracked_orders.write().expect("registry should lock");
        let second_removed = registry
            .remove_terminal_record(&second_id)
            .expect("second generation should retire");
        drop(registry);
        super::super::settle_maker_terminal(
            &handle.tracked_orders,
            second_id,
            Some(second_removed),
            crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Canceled,
            2,
        )
        .expect("second refinable terminal should settle");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        assert_eq!(registry.retained_terminal_orders.len(), 1);
        assert!(!registry.retained_terminal_orders.contains_key(&first_id));
        assert!(registry.retained_terminal_orders.contains_key(&second_id));
    }

    #[test]
    fn registration_finality_matches_distinct_authorities_by_their_sealed_scope() {
        let scope = MakerOrderLifecycleScopeIdentity::new(
            7,
            InstrumentId::from("SEALED-SCOPE-YES.SIM"),
            InstrumentId::from("SEALED-SCOPE-NO.SIM"),
        );
        let first = accepted_order("sealed-scope-generation-one", "VENUE-SEALED-SCOPE-ONE");
        let first_id = first.client_order_id();
        let second = accepted_order("sealed-scope-generation-two", "VENUE-SEALED-SCOPE-TWO");
        let second_id = second.client_order_id();
        let first_lifecycle =
            MakerQuoteLifecycleHandle::new(MarketQuote::new(scope, false), Leg::Yes);
        let second_lifecycle =
            MakerQuoteLifecycleHandle::new(MarketQuote::new(scope, false), Leg::Yes);
        assert!(
            !first_lifecycle.shares_authority_with(&second_lifecycle),
            "the fixture requires distinct Arc authorities"
        );

        let handle = coordinator_handle_with_budget(first.clone(), 8);
        let mut registry = handle.tracked_orders.write().expect("registry should lock");
        registry
            .records
            .get_mut(&first_id)
            .expect("first generation should be tracked")
            .governed_mut()
            .expect("first generation should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new(&first, 1, first_lifecycle);
        let first_removed = registry
            .remove_terminal_record(&first_id)
            .expect("first generation should retire");
        drop(registry);
        super::super::settle_maker_terminal(
            &handle.tracked_orders,
            first_id,
            Some(first_removed),
            crate::bolt_v3_quote_lifecycle::MakerQuoteTerminalDisposition::Canceled,
            1,
        )
        .expect("first refinable terminal should settle");

        let mut registry = handle.tracked_orders.write().expect("registry should lock");
        registry.records.insert(
            second_id,
            TrackedMakerOrderRecord::new_governed(
                2,
                RestingRegistrationState::Committed,
                None,
                None,
                MakerQuoteOrderAuthority::new(&second, 2, second_lifecycle.clone()),
                TrackedOrderCancellation::new(second),
            ),
        );
        drop(registry);

        super::super::apply_retention_horizon(
            &handle.tracked_orders,
            super::super::RetentionHorizonCapability::RegistrationEpochFinal {
                lifecycle: &second_lifecycle,
                registration_generation: 2,
                current_client_order_id: second_id,
            },
        )
        .expect("sealed scope identity should finalize the older generation");

        let registry = handle.tracked_orders.read().expect("registry should lock");
        assert!(
            !registry.retained_terminal_orders.contains_key(&first_id),
            "Arc identity must not strand older truth in the same typed scope"
        );
        assert!(registry.records.contains_key(&second_id));
    }

    #[test]
    fn old_terminal_cannot_retire_a_newer_winding_down_poison() {
        let client_order_id = "old-terminal-new-poison";
        let venue_order_id = "VENUE-OLD-TERMINAL";
        let mut order = accepted_order(client_order_id, venue_order_id);
        let client_order_id = order.client_order_id();
        let mut market = MarketQuote::new_for_test(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let lifecycle_budget = RequoteBudgetPair::new(
            RequoteBudget::new(8, 60_000, 0),
            RequoteBudget::new(8, 60_000, 0),
        );
        let cancel = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("resting requote should propose a cancel");
        let budget_proposal = MakerQuoteBudgetProposal::Reserve(
            lifecycle_budget
                .propose_cancel_resubmit(1)
                .expect("requote budget should be available"),
        );
        market
            .arm_leg_transaction(
                cancel,
                lifecycle_budget.clone(),
                budget_proposal,
                MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), 1),
            )
            .expect("requote cancel should arm");
        market
            .mark_leg_transaction_sink_invoked(cancel, 1, 1)
            .expect("requote cancel should reach the sink");
        assert!(market.commit_leg_transaction(cancel, 1));

        let handle = coordinator_handle_with_budget(order.clone(), 8);
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("tracked record should exist")
            .governed_mut()
            .expect("tracked record should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new(
            &order,
            1,
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
        );
        let canceled = TestOrderEventStubs::canceled(
            &order,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from(venue_order_id)),
        );
        order.apply(canceled).expect("cancel should apply");
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 1)
            .expect("cancel callback should retire the resting record");

        let replacement = market
            .propose_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            )
            .expect("retained replacement capacity should propose a submit");
        let prepaid_generation = market
            .prepaid_generation(Leg::Yes)
            .expect("replacement capacity should remain prepaid");
        market
            .arm_leg_transaction(
                replacement,
                lifecycle_budget,
                MakerQuoteBudgetProposal::Prepaid {
                    generation: prepaid_generation,
                    now_ms: 2,
                },
                MakerQuoteLifecycleIdentity::new("new-poison-order", 2),
            )
            .expect("replacement should arm at a later generation");
        market
            .mark_leg_transaction_sink_invoked(replacement, 2, 2)
            .expect("replacement should reach the sink");
        assert_eq!(
            market.cancel_leg(Leg::Yes),
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            })
        );
        assert!(market.unwind_post_sink_leg_transaction(replacement, 2));
        let before = (
            market.leg_state(Leg::Yes),
            market.prepaid_generation(Leg::Yes),
        );
        assert_eq!(before.0, LegState::PoisonedReconciliationHold);

        fill_canceled_order(&mut order, venue_order_id);
        handle
            .reconcile_tracked_order_at(client_order_id, Some(order), 2)
            .expect("stale terminal should be dropped without retiring current poison");

        assert_eq!(
            (
                market.leg_state(Leg::Yes),
                market.prepaid_generation(Leg::Yes),
            ),
            before
        );
    }

    fn apply_timer<'a>(
        record: &mut RestingOrderCancelRecord,
        seed: &mut NtOrderQuerySeed,
        cached: Option<&'a OrderAny>,
        now_ns: u64,
    ) -> Result<CancelEffect> {
        record.apply_event(
            seed,
            CancelEvent::TimerObserved {
                cached,
                now_ns,
                retry_timeout_ns: 10,
            },
        )
    }

    #[test]
    fn venue_identity_gate_covers_capture_absence_equality_and_conflict() {
        let initialized = initialized_order("identity-gate");
        let mut seed = NtOrderQuerySeed::new(initialized.clone());
        assert_eq!(
            seed.reconcile_cached_identity(None).unwrap(),
            IdentityTransition::Preserved
        );
        assert_eq!(
            seed.reconcile_cached_identity(Some(&initialized)).unwrap(),
            IdentityTransition::Unchanged
        );
        let accepted_a = accepted_order("identity-gate", "VENUE-A");
        assert_eq!(
            seed.reconcile_cached_identity(Some(&accepted_a)).unwrap(),
            IdentityTransition::Captured
        );
        assert_eq!(
            seed.reconcile_cached_identity(Some(&accepted_a)).unwrap(),
            IdentityTransition::Unchanged
        );
        let accepted_b = accepted_order("identity-gate", "VENUE-B");
        assert!(matches!(
            seed.reconcile_cached_identity(Some(&accepted_b)).unwrap(),
            IdentityTransition::Conflict(_)
        ));
        assert_eq!(seed.venue_order_id(), Some(VenueOrderId::from("VENUE-A")));
    }

    #[test]
    fn venue_identity_conflict_is_a_monotonic_routing_hold() {
        let accepted_a = accepted_order("identity-conflict", "VENUE-A");
        let accepted_b = accepted_order("identity-conflict", "VENUE-B");
        let mut seed = NtOrderQuerySeed::new(accepted_a.clone());
        let mut record = RestingOrderCancelRecord::new(100);
        record.routing_state = CancelRoutingState::Backoff { not_before_ns: 40 };
        record.generation = 7;
        record.total_recovery_attempts = 3;
        record.cancel_attempts = 2;
        record.query_attempts = 1;
        record.last_observed_ns = Some(20);
        record.health.retry_escalated = true;
        let before = record.clone();

        let effect = apply_timer(&mut record, &mut seed, Some(&accepted_b), 30).unwrap();
        assert!(matches!(effect, CancelEffect::None));
        assert_eq!(record.routing_state, before.routing_state);
        assert_eq!(record.generation, before.generation);
        assert_eq!(
            record.total_recovery_attempts,
            before.total_recovery_attempts
        );
        assert_eq!(record.last_observed_ns, Some(30));
        assert!(record.health.retry_escalated);
        assert!(record.health.recovery_identity_conflict.is_some());

        let mut terminal = accepted_a;
        let canceled = TestOrderEventStubs::canceled(
            &terminal,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from("VENUE-A")),
        );
        terminal.apply(canceled).unwrap();
        let effect = apply_timer(&mut record, &mut seed, Some(&terminal), 40).unwrap();
        assert!(matches!(effect, CancelEffect::None));
        assert_eq!(record.routing_state, before.routing_state);
        assert_eq!(record.last_observed_ns, Some(40));
    }

    #[test]
    fn venue_identity_conflict_holds_routing_without_bypassing_health_or_clock_checks() {
        let accepted_a = accepted_order("identity-conflict-health", "VENUE-A");
        let accepted_b = accepted_order("identity-conflict-health", "VENUE-B");
        let mut seed = NtOrderQuerySeed::new(accepted_a);
        let mut record = RestingOrderCancelRecord::new(100);
        record.routing_state = CancelRoutingState::Backoff { not_before_ns: 40 };
        record.last_observed_ns = Some(20);

        let effect = apply_timer(&mut record, &mut seed, Some(&accepted_b), 30).unwrap();
        assert!(matches!(effect, CancelEffect::None));
        let health = record.health_snapshot(ClientOrderId::from("identity-conflict-health"));
        assert!(health.recovery_identity_conflict().is_some());
        assert_eq!(health.liveness(), None);

        let effect = apply_timer(&mut record, &mut seed, Some(&accepted_b), 100).unwrap();
        assert!(matches!(effect, CancelEffect::None));
        let health = record.health_snapshot(ClientOrderId::from("identity-conflict-health"));
        assert_eq!(
            health.liveness(),
            Some(BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded)
        );
        assert_eq!(
            record.routing_state,
            CancelRoutingState::Backoff { not_before_ns: 40 }
        );

        let error = apply_timer(&mut record, &mut seed, Some(&accepted_b), 19).unwrap_err();
        assert!(error.to_string().contains("clock regressed"));
    }

    #[test]
    fn pending_cancel_identity_conflict_reports_stuck_pending_at_the_deadline() {
        let accepted_a = accepted_order("identity-conflict-pending", "VENUE-A");
        let accepted_b = accepted_order("identity-conflict-pending", "VENUE-B");
        let mut seed = NtOrderQuerySeed::new(accepted_a);
        let mut record = RestingOrderCancelRecord::new(100);
        record.routing_state = CancelRoutingState::PendingCancel { not_before_ns: 90 };

        let effect = apply_timer(&mut record, &mut seed, Some(&accepted_b), 100).unwrap();
        assert!(matches!(effect, CancelEffect::None));
        assert_eq!(
            record
                .health_snapshot(ClientOrderId::from("identity-conflict-pending"))
                .liveness(),
            Some(BoltV3CancellationLivenessFailure::StuckPendingCancel)
        );
        assert_eq!(
            record.routing_state,
            CancelRoutingState::PendingCancel { not_before_ns: 90 }
        );
    }

    #[test]
    fn order_status_partition_covers_every_pinned_nt_variant() {
        let retryable = [
            OrderStatus::Initialized,
            OrderStatus::Emulated,
            OrderStatus::Released,
            OrderStatus::Submitted,
            OrderStatus::Accepted,
            OrderStatus::Triggered,
            OrderStatus::PendingUpdate,
            OrderStatus::PartiallyFilled,
        ];
        let terminal = [
            OrderStatus::Denied,
            OrderStatus::Rejected,
            OrderStatus::Canceled,
            OrderStatus::Expired,
            OrderStatus::Filled,
            OrderStatus::Voided,
        ];
        assert!(
            retryable
                .into_iter()
                .all(|status| classify_status(status, false) == CancelObservation::Retryable)
        );
        assert_eq!(
            classify_status(OrderStatus::PendingCancel, false),
            CancelObservation::PendingCancelUnqueryable
        );
        assert!(
            terminal
                .into_iter()
                .all(|status| classify_status(status, false) == CancelObservation::Terminal)
        );
        assert_eq!(retryable.len() + terminal.len() + 1, 15);
    }

    #[test]
    fn every_cancel_state_observation_pair_has_one_explicit_transition() {
        let states = [
            CancelRoutingState::Ready,
            CancelRoutingState::Attempting {
                generation: 7,
                operation: CancelOperationKind::Cancel,
                not_before_ns: 20,
            },
            CancelRoutingState::Backoff { not_before_ns: 20 },
            CancelRoutingState::PendingCancel { not_before_ns: 20 },
        ];
        let observations = [
            CancelObservation::MissingUnqueryable,
            CancelObservation::MissingQueryable,
            CancelObservation::Retryable,
            CancelObservation::PendingCancelUnqueryable,
            CancelObservation::PendingCancelQueryable,
            CancelObservation::Terminal,
        ];
        let mut covered = 0;
        for state in states {
            for observation in observations {
                let _ = transition(state, observation, 20, 30);
                covered += 1;
            }
        }
        assert_eq!(covered, 24);
    }

    #[test]
    fn reservation_denial_precedes_attempt_arming_and_preserves_counters() {
        let order = accepted_order("event-reducer", "VENUE-EVENT");
        let mut seed = NtOrderQuerySeed::new(order.clone());
        let mut record = RestingOrderCancelRecord::new(100);

        let effect = record
            .apply_event(
                &mut seed,
                CancelEvent::TimerObserved {
                    cached: Some(&order),
                    now_ns: 10,
                    retry_timeout_ns: 20,
                },
            )
            .unwrap();
        assert!(matches!(
            effect,
            CancelEffect::ReservationRequired {
                operation: CancelOperationKind::Cancel
            }
        ));
        assert_eq!(record.routing_state, CancelRoutingState::Ready);
        assert_eq!(record.generation, 0);
        assert_eq!(record.total_recovery_attempts, 0);
        assert_eq!(record.cancel_attempts, 0);
        assert!(!record.health.retry_escalated);

        assert!(matches!(
            record
                .apply_event(
                    &mut seed,
                    CancelEvent::ReservationDenied {
                        now_ns: 10,
                        retry_timeout_ns: 20,
                    },
                )
                .unwrap(),
            CancelEffect::None
        ));
        assert_eq!(
            record.routing_state,
            CancelRoutingState::Backoff { not_before_ns: 30 }
        );
        assert_eq!(record.generation, 0);
        assert_eq!(record.total_recovery_attempts, 0);
        assert_eq!(record.cancel_attempts, 0);
        assert!(!record.health.retry_escalated);

        assert!(matches!(
            apply_timer(&mut record, &mut seed, Some(&order), 29).unwrap(),
            CancelEffect::None
        ));
        let effect = apply_timer(&mut record, &mut seed, Some(&order), 30).unwrap();
        assert!(matches!(effect, CancelEffect::ReservationRequired { .. }));
        let effect = record
            .apply_event(
                &mut seed,
                CancelEvent::ReservationGranted {
                    operation: CancelOperationKind::Cancel,
                    now_ns: 30,
                    retry_timeout_ns: 20,
                    escalation_attempts: 3,
                },
            )
            .unwrap();
        assert!(matches!(effect, CancelEffect::Cancel { generation: 1 }));
        assert_eq!(
            record.routing_state,
            CancelRoutingState::Attempting {
                generation: 1,
                operation: CancelOperationKind::Cancel,
                not_before_ns: 50,
            }
        );
        assert_eq!(record.total_recovery_attempts, 1);
        assert_eq!(record.cancel_attempts, 1);

        assert!(matches!(
            record
                .apply_event(
                    &mut seed,
                    CancelEvent::OperationUnobserved { generation: 1 },
                )
                .unwrap(),
            CancelEffect::None
        ));
        assert_eq!(
            record.routing_state,
            CancelRoutingState::Backoff { not_before_ns: 50 }
        );
    }

    #[test]
    fn failed_operation_success_reconciliation_settles_the_armed_generation() {
        let order = initialized_order("operation-success-reconciliation-failure");
        let mut seed = NtOrderQuerySeed::new(order.clone());
        let mut record = RestingOrderCancelRecord::new(100);
        record.last_observed_ns = Some(20);
        record.routing_state = CancelRoutingState::Attempting {
            generation: 7,
            operation: CancelOperationKind::Cancel,
            not_before_ns: 30,
        };

        let error = record
            .apply_event(
                &mut seed,
                CancelEvent::OperationSucceeded {
                    generation: 7,
                    cached: Some(&order),
                    now_ns: 19,
                    retry_timeout_ns: 10,
                },
            )
            .expect_err("a regressed post-operation observation must fail closed");

        assert!(error.to_string().contains("clock regressed"));
        assert_eq!(record.last_observed_ns, Some(20));
        assert_eq!(
            record.routing_state,
            CancelRoutingState::Backoff { not_before_ns: 30 },
            "a rejected success observation must not strand the operation in flight"
        );
    }

    #[test]
    fn composed_cancel_health_snapshot_is_the_complete_runtime_report() {
        let mut record = RestingOrderCancelRecord::new(5);
        record.total_recovery_attempts = 3;
        record.health.recovery_identity_unavailable = true;
        record.health.retry_escalated = true;
        record.health.liveness =
            Some(BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded);
        record.health.recovery_identity_conflict = Some(BoltV3RecoveryIdentityConflict {
            captured: VenueOrderId::from("A"),
            observed: VenueOrderId::from("B"),
        });
        let snapshot = record.health_snapshot(ClientOrderId::from("C"));
        assert_eq!(snapshot.total_recovery_attempts(), 3);
        assert!(snapshot.recovery_identity_unavailable());
        assert!(snapshot.retry_escalated());
        assert_eq!(
            snapshot.liveness(),
            Some(BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded)
        );
        assert_eq!(
            snapshot.recovery_identity_conflict().unwrap().captured(),
            VenueOrderId::from("A")
        );
        assert_eq!(
            snapshot.runtime_error().unwrap().to_string(),
            "resting cancellation health failure: client_order_id=C total_recovery_attempts=3 recovery_identity_unavailable=true recovery_identity_conflict={captured=A,observed=B} retry_escalated=true liveness=CancellationDeadlineExceeded"
        );

        let healthy =
            RestingOrderCancelRecord::new(5).health_snapshot(ClientOrderId::from("HEALTHY"));
        assert_eq!(healthy.total_recovery_attempts(), 0);
        assert!(healthy.runtime_error().is_none());
    }

    #[test]
    fn callbacks_cannot_overwrite_a_newer_attempt_generation() {
        let order = initialized_order("generation-guard");
        let mut seed = NtOrderQuerySeed::new(order.clone());
        let mut record = RestingOrderCancelRecord::new(100);
        let effect = apply_timer(&mut record, &mut seed, Some(&order), 10).unwrap();
        assert!(matches!(effect, CancelEffect::ReservationRequired { .. }));
        let effect = record
            .apply_event(
                &mut seed,
                CancelEvent::ReservationGranted {
                    operation: CancelOperationKind::Cancel,
                    now_ns: 10,
                    retry_timeout_ns: 10,
                    escalation_attempts: 3,
                },
            )
            .unwrap();
        let CancelEffect::Cancel { generation } = effect else {
            panic!("retryable order must emit one cancel effect");
        };
        assert!(matches!(
            record
                .apply_event(&mut seed, CancelEvent::OperationUnobserved { generation },)
                .unwrap(),
            CancelEffect::None
        ));
        let settled = record.routing_state;
        assert!(matches!(
            record
                .apply_event(
                    &mut seed,
                    CancelEvent::OperationSucceeded {
                        generation,
                        cached: Some(&order),
                        now_ns: 11,
                        retry_timeout_ns: 10,
                    },
                )
                .unwrap(),
            CancelEffect::None
        ));
        assert_eq!(record.routing_state, settled);
    }

    #[test]
    fn nt_cancel_error_after_invocation_retains_charge_and_enters_backoff() {
        let order = accepted_order("cancel-nt-error", "VENUE-CANCEL-NT-ERROR");
        let client_order_id = order.client_order_id();
        let handle = coordinator_handle_with_budget(order.clone(), 8);
        let (participant, market, budget) = requote_cancel_participant();
        handle
            .tracked_orders
            .write()
            .expect("registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("tracked record should exist")
            .governed_mut()
            .expect("tracked record should retain exact authority")
            .maker_lifecycle = MakerQuoteOrderAuthority::new(
            &order,
            1,
            MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes),
        );
        let now_ns = NANOS_PER_MILLI_U64;
        handle
            .request_cancel_intent(client_order_id, now_ns)
            .expect("cancel intent should register");
        let mut sink = CoordinatorSink {
            now_ns,
            cached: Some(order.clone()),
            cancel_calls: 0,
            query_calls: 0,
            cancel_error: Some("configured NT cancel error"),
        };

        let error = handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: Some(&order),
                    now_ns,
                    command_participant: Some(participant),
                },
            )
            .expect_err("an NT cancel error must remain loud and retryable");

        assert!(error.to_string().contains("configured NT cancel error"));
        assert_eq!(sink.cancel_calls, 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
        assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
        let registry = handle.tracked_orders.read().expect("registry should lock");
        let routing_state = registry
            .records
            .get(&client_order_id)
            .expect("tracked record should remain retryable")
            .cancellation
            .intent
            .as_ref()
            .expect("cancel intent should remain armed")
            .routing_state;
        assert!(matches!(routing_state, CancelRoutingState::Backoff { .. }));
    }

    #[test]
    fn cancel_retry_cap_denial_makes_zero_calls_and_preserves_attempt_health_until_capacity_returns()
     {
        let order = accepted_order("cancel-cap", "VENUE-CANCEL-CAP");
        let client_order_id = order.client_order_id();
        let handle = coordinator_handle_with_budget(order.clone(), 1);
        let retry_ns = handle.economics.cancel_retry_timeout_ns().unwrap();
        let first_ns = NANOS_PER_MILLI_U64;
        handle
            .request_cancel_intent(client_order_id, first_ns)
            .unwrap();
        let mut sink = CoordinatorSink {
            now_ns: first_ns,
            cached: Some(order.clone()),
            cancel_calls: 0,
            query_calls: 0,
            cancel_error: None,
        };

        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: Some(&order),
                    now_ns: first_ns,
                    command_participant: None,
                },
            )
            .expect_err("past-deadline cancellation remains loud after routing");
        assert_eq!(sink.cancel_calls, 1);
        let after_first = handle.resting_cancel_health().unwrap()[0].clone();
        assert_eq!(after_first.total_recovery_attempts(), 1);

        let denied_ns = first_ns + retry_ns;
        sink.now_ns = denied_ns;
        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: Some(&order),
                    now_ns: denied_ns,
                    command_participant: None,
                },
            )
            .expect_err("reservation denial preserves the existing liveness failure");
        assert_eq!(sink.cancel_calls, 1);
        assert_eq!(handle.resting_cancel_health().unwrap()[0], after_first);

        let second_denied_ns = denied_ns + retry_ns;
        sink.now_ns = second_denied_ns;
        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: Some(&order),
                    now_ns: second_denied_ns,
                    command_participant: None,
                },
            )
            .expect_err("repeated denial preserves the existing liveness failure");
        assert_eq!(sink.cancel_calls, 1);
        assert_eq!(handle.resting_cancel_health().unwrap()[0], after_first);

        let resumed_ns = (60_002 * NANOS_PER_MILLI_U64).max(second_denied_ns + retry_ns);
        sink.now_ns = resumed_ns;
        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: Some(&order),
                    now_ns: resumed_ns,
                    command_participant: None,
                },
            )
            .expect_err("capacity recovery routes but does not erase liveness evidence");
        assert_eq!(sink.cancel_calls, 2);
        assert_eq!(
            handle.resting_cancel_health().unwrap()[0].total_recovery_attempts(),
            2
        );
    }

    #[test]
    fn query_retry_uses_the_same_reservation_before_arm_and_charges_each_routed_attempt() {
        let order = accepted_order("query-cap", "VENUE-QUERY-CAP");
        let client_order_id = order.client_order_id();
        let handle = coordinator_handle_with_budget(order, 1);
        let retry_ns = handle.economics.cancel_retry_timeout_ns().unwrap();
        let first_ns = NANOS_PER_MILLI_U64;
        handle
            .request_cancel_intent(client_order_id, first_ns)
            .unwrap();
        let mut sink = CoordinatorSink {
            now_ns: first_ns,
            cached: None,
            cancel_calls: 0,
            query_calls: 0,
            cancel_error: None,
        };

        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: None,
                    now_ns: first_ns,
                    command_participant: None,
                },
            )
            .expect_err("past-deadline query recovery remains loud after routing");
        assert_eq!(sink.query_calls, 1);
        assert_eq!(sink.cancel_calls, 0);
        let after_first = handle.resting_cancel_health().unwrap()[0].clone();

        let denied_ns = first_ns + retry_ns;
        sink.now_ns = denied_ns;
        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: None,
                    now_ns: denied_ns,
                    command_participant: None,
                },
            )
            .expect_err("query reservation denial preserves liveness evidence");
        assert_eq!(sink.query_calls, 1);
        assert_eq!(handle.resting_cancel_health().unwrap()[0], after_first);

        let resumed_ns = (60_002 * NANOS_PER_MILLI_U64).max(denied_ns + retry_ns);
        sink.now_ns = resumed_ns;
        handle
            .drive_cancel_intent(
                BoltV3OrderExecutionPolicy::live(),
                &mut sink,
                CancelDriveInput {
                    execution_client_id: "execution_client",
                    client_order_id,
                    cached: None,
                    now_ns: resumed_ns,
                    command_participant: None,
                },
            )
            .expect_err("capacity recovery routes query but retains liveness evidence");
        assert_eq!(sink.query_calls, 2);
        assert_eq!(
            handle.resting_cancel_health().unwrap()[0].total_recovery_attempts(),
            2
        );
    }

    #[test]
    fn coordinator_rejects_clock_regression_without_state_change() {
        let order = initialized_order("clock-regression");
        let mut seed = NtOrderQuerySeed::new(order.clone());
        let mut record = RestingOrderCancelRecord::new(100);
        record.last_observed_ns = Some(20);
        record.routing_state = CancelRoutingState::Backoff { not_before_ns: 30 };
        let before = record.clone();
        let error = apply_timer(&mut record, &mut seed, Some(&order), 19).unwrap_err();
        assert!(error.to_string().contains("clock regressed"));
        assert_eq!(record, before);
    }
}
