use anyhow::Result;
use nautilus_model::{
    enums::{OrderSide, OrderStatus},
    identifiers::{ClientId, ClientOrderId, InstrumentId, VenueOrderId},
    orders::{Order, OrderAny},
};
use rust_decimal::Decimal;

use super::{
    BoltV3NtVenueMutationSink, BoltV3OrderEconomicsHandle, BoltV3OrderExecutionPolicy,
    TrackedMakerOrderRecord,
};

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
        escalation_attempts: u32,
    },
    CallbackObserved {
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
}

enum CancelOperationCompletion<'a> {
    Unobserved(anyhow::Error),
    Observed {
        cached: Option<&'a OrderAny>,
        now_ns: u64,
    },
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
                escalation_attempts,
            } => {
                let transition = self.reconcile(seed, cached, now_ns, retry_timeout_ns)?;
                match transition {
                    CancelTransition::NoOperation => Ok(CancelEffect::None),
                    CancelTransition::Remove => Ok(CancelEffect::Remove),
                    CancelTransition::Begin(kind) => {
                        let generation = self.begin_operation(
                            kind,
                            now_ns,
                            retry_timeout_ns,
                            escalation_attempts,
                        )?;
                        Ok(match kind {
                            CancelOperationKind::Cancel => CancelEffect::Cancel { generation },
                            CancelOperationKind::Query => CancelEffect::Query {
                                generation,
                                seed: Box::new(seed.as_query_order().clone()),
                            },
                        })
                    }
                }
            }
            CancelEvent::CallbackObserved {
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

impl BoltV3OrderEconomicsHandle {
    pub fn begin_resting_order_drain_at_ns(&self, now_ns: u64) -> Result<usize> {
        let client_order_ids = self.resting_order_ids()?;
        for client_order_id in &client_order_ids {
            self.request_cancel_intent(*client_order_id, now_ns)?;
        }
        Ok(client_order_ids.len())
    }

    pub(super) fn request_cancel_intent(
        &self,
        client_order_id: ClientOrderId,
        now_ns: u64,
    ) -> Result<bool> {
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(&client_order_id) else {
            return Ok(false);
        };
        let quote_deadline_ns = record
            .economics
            .as_ref()
            .map(|economics| economics.admission.quote().valid_until_ns())
            .unwrap_or(now_ns);
        record
            .cancellation
            .get_or_insert_with(|| RestingOrderCancelRecord::new(quote_deadline_ns));
        Ok(true)
    }

    pub(super) fn request_cancel_scope(
        &self,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        now_ns: u64,
    ) -> Result<Vec<ClientOrderId>> {
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let mut selected = Vec::new();
        for (client_order_id, record) in records.iter_mut() {
            if record.query_seed.instrument_id() != instrument_id
                || order_side.is_some_and(|side| record.query_seed.order_side() != side)
            {
                continue;
            }
            let quote_deadline_ns = record
                .economics
                .as_ref()
                .map(|economics| economics.admission.quote().valid_until_ns())
                .unwrap_or(now_ns);
            record
                .cancellation
                .get_or_insert_with(|| RestingOrderCancelRecord::new(quote_deadline_ns));
            selected.push(*client_order_id);
        }
        Ok(selected)
    }

    pub fn resting_cancel_health(&self) -> Result<Vec<BoltV3RestingOrderCancelHealthSnapshot>> {
        Ok(self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?
            .iter()
            .filter_map(|(client_order_id, record)| {
                record
                    .cancellation
                    .as_ref()
                    .map(|cancel| cancel.health_snapshot(*client_order_id))
            })
            .collect())
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
        let retry_timeout_ns = self.economics.cancel_retry_timeout_ns()?;
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        if let std::collections::btree_map::Entry::Vacant(entry) = records.entry(client_order_id) {
            let Some(order) = cached.as_ref() else {
                return Ok(());
            };
            if !matches!(origin, CancelCallbackOrigin::FillVoid)
                || order.is_closed()
                || order.leaves_qty().as_decimal() == Decimal::ZERO
            {
                return Ok(());
            }
            entry.insert(TrackedMakerOrderRecord {
                economics: None,
                query_seed: NtOrderQuerySeed::new(order.clone()),
                cancellation: Some(RestingOrderCancelRecord::new(now_ns)),
            });
        }
        let Some(record) = records.get_mut(&client_order_id) else {
            return Ok(());
        };
        if record.cancellation.is_none()
            && cached
                .as_ref()
                .is_some_and(|order| order.status() == OrderStatus::PendingCancel)
        {
            let quote_deadline_ns = record
                .economics
                .as_ref()
                .map(|economics| economics.admission.quote().valid_until_ns())
                .unwrap_or(now_ns);
            record.cancellation = Some(RestingOrderCancelRecord::new(quote_deadline_ns));
        }
        let Some(cancellation) = record.cancellation.as_mut() else {
            if cached.as_ref().is_some_and(|order| {
                order.is_closed() || order.leaves_qty().as_decimal() == Decimal::ZERO
            }) {
                records.remove(&client_order_id);
            }
            return Ok(());
        };
        let effect = cancellation.apply_event(
            &mut record.query_seed,
            CancelEvent::CallbackObserved {
                cached: cached.as_ref(),
                now_ns,
                retry_timeout_ns,
            },
        )?;
        match effect {
            CancelEffect::Remove => {
                records.remove(&client_order_id);
            }
            CancelEffect::None => {}
            CancelEffect::Cancel { .. } | CancelEffect::Query { .. } => {
                anyhow::bail!("callback reconciliation produced an NT operation")
            }
        }
        Ok(())
    }

    pub(super) fn drive_cancel_intent<S>(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        sink: &mut S,
        execution_client_id: &str,
        client_order_id: ClientOrderId,
        cached: Option<&OrderAny>,
        now_ns: u64,
    ) -> Result<()>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        let retry_timeout_ns = self.economics.cancel_retry_timeout_ns()?;
        let escalation_attempts = self.economics.cancel_recovery_escalation_attempts();
        let effect = match self.reduce_cancel_drive(
            policy,
            client_order_id,
            cached,
            now_ns,
            retry_timeout_ns,
            escalation_attempts,
        ) {
            Ok(effect) => effect,
            Err(error) => {
                return self.finish_cancel_drive(client_order_id, vec![error.to_string()]);
            }
        };
        let (generation, operation_result) = match effect {
            CancelEffect::None => {
                return self.finish_cancel_drive(client_order_id, Vec::new());
            }
            CancelEffect::Remove => return Ok(()),
            CancelEffect::Cancel { generation } => (
                generation,
                policy
                    .route_cancel_with_sink(
                        sink,
                        client_order_id,
                        Some(ClientId::from(execution_client_id)),
                        None,
                    )
                    .map(|_| ()),
            ),
            CancelEffect::Query { generation, seed } => (
                generation,
                sink.query_order_via_nt(&seed, Some(ClientId::from(execution_client_id)), None),
            ),
        };
        match operation_result {
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
        }
    }

    fn settle_cancel_operation(
        &self,
        client_order_id: ClientOrderId,
        generation: u64,
        completion: CancelOperationCompletion<'_>,
        retry_timeout_ns: u64,
    ) -> Result<()> {
        let mut failures = Vec::new();
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(&client_order_id) else {
            if let CancelOperationCompletion::Unobserved(error) = completion {
                failures.push(error.to_string());
            }
            return finish_cancel_failures(failures);
        };

        let Some(cancellation) = record.cancellation.as_mut() else {
            anyhow::bail!(
                "armed cancellation operation lost its coordinator record: client_order_id={client_order_id}"
            );
        };
        let effect = match completion {
            CancelOperationCompletion::Unobserved(error) => {
                failures.push(error.to_string());
                cancellation.apply_event(
                    &mut record.query_seed,
                    CancelEvent::OperationUnobserved { generation },
                )
            }
            CancelOperationCompletion::Observed { cached, now_ns } => cancellation.apply_event(
                &mut record.query_seed,
                CancelEvent::OperationSucceeded {
                    generation,
                    cached,
                    now_ns,
                    retry_timeout_ns,
                },
            ),
        };
        match effect {
            Ok(CancelEffect::Remove) => {
                records.remove(&client_order_id);
            }
            Ok(CancelEffect::None) => {}
            Ok(CancelEffect::Cancel { .. } | CancelEffect::Query { .. }) => {
                failures.push("operation settlement produced another NT operation".to_string());
            }
            Err(error) => failures.push(error.to_string()),
        }
        drop(records);
        self.finish_cancel_drive(client_order_id, failures)
    }

    fn reduce_cancel_drive(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        client_order_id: ClientOrderId,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
        escalation_attempts: u32,
    ) -> Result<CancelEffect> {
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(&client_order_id) else {
            return Ok(CancelEffect::None);
        };
        let TrackedMakerOrderRecord {
            query_seed,
            cancellation,
            ..
        } = record;
        let Some(cancellation) = cancellation.as_mut() else {
            return Ok(CancelEffect::None);
        };
        let effect = if policy.allows_venue_mutation() {
            cancellation.apply_event(
                query_seed,
                CancelEvent::TimerObserved {
                    cached,
                    now_ns,
                    retry_timeout_ns,
                    escalation_attempts,
                },
            )
        } else {
            cancellation.apply_event(
                query_seed,
                CancelEvent::CallbackObserved {
                    cached,
                    now_ns,
                    retry_timeout_ns,
                },
            )
        }?;
        match effect {
            CancelEffect::Remove => {
                records.remove(&client_order_id);
                Ok(CancelEffect::Remove)
            }
            other => Ok(other),
        }
    }

    fn finish_cancel_drive(
        &self,
        client_order_id: ClientOrderId,
        mut failures: Vec<String>,
    ) -> Result<()> {
        let records = self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        if let Some(error) = records
            .get(&client_order_id)
            .and_then(|record| record.cancellation.as_ref())
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
    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        identifiers::{AccountId, InstrumentId, StrategyId, TraderId},
        orders::{LimitOrder, stubs::TestOrderEventStubs},
        types::{Price, Quantity},
    };

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
                escalation_attempts: 3,
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
    fn event_reducer_owns_timer_and_unobserved_operation_transitions() {
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
                not_before_ns: 30,
            }
        );

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
            CancelRoutingState::Backoff { not_before_ns: 30 }
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
