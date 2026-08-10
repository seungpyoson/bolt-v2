use anyhow::Result;
use nautilus_model::{
    enums::OrderStatus,
    identifiers::{ClientOrderId, VenueOrderId},
    orders::{Order, OrderAny},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CancelOperationKind {
    Cancel,
    Query,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CancelRoutingState {
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
pub(super) enum CancelObservation {
    MissingUnqueryable,
    MissingQueryable,
    Retryable,
    PendingCancelUnqueryable,
    PendingCancelQueryable,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CancelTransition {
    NoOperation,
    Begin(CancelOperationKind),
    Remove,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArmedCancelOperation {
    pub kind: CancelOperationKind,
    pub generation: u64,
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
    recovery_identity_unavailable: bool,
    recovery_identity_conflict: Option<BoltV3RecoveryIdentityConflict>,
    retry_escalated: bool,
    liveness: Option<BoltV3CancellationLivenessFailure>,
}

impl BoltV3RestingOrderCancelHealthSnapshot {
    pub const fn client_order_id(&self) -> ClientOrderId {
        self.client_order_id
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
}

#[derive(Clone, Debug)]
pub(super) struct NtOrderQuerySeed {
    order: OrderAny,
}

impl NtOrderQuerySeed {
    pub fn new(order: OrderAny) -> Self {
        Self { order }
    }

    pub fn as_query_order(&self) -> &OrderAny {
        &self.order
    }

    pub fn venue_order_id(&self) -> Option<VenueOrderId> {
        self.order.venue_order_id()
    }

    pub fn instrument_id(&self) -> nautilus_model::identifiers::InstrumentId {
        self.order.instrument_id()
    }

    pub fn order_side(&self) -> nautilus_model::enums::OrderSide {
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
    pub fn new(quote_deadline_ns: u64) -> Self {
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

    pub fn health_snapshot(
        &self,
        client_order_id: ClientOrderId,
    ) -> BoltV3RestingOrderCancelHealthSnapshot {
        BoltV3RestingOrderCancelHealthSnapshot {
            client_order_id,
            recovery_identity_unavailable: self.health.recovery_identity_unavailable,
            recovery_identity_conflict: self.health.recovery_identity_conflict.clone(),
            retry_escalated: self.health.retry_escalated,
            liveness: self.health.liveness,
        }
    }

    pub fn plan_drive(
        &mut self,
        seed: &mut NtOrderQuerySeed,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
        escalation_attempts: u32,
    ) -> Result<(CancelTransition, Option<ArmedCancelOperation>)> {
        let transition = self.reconcile(seed, cached, now_ns, retry_timeout_ns)?;
        if let CancelTransition::Begin(kind) = transition {
            let operation =
                self.begin_operation(kind, now_ns, retry_timeout_ns, escalation_attempts)?;
            Ok((transition, Some(operation)))
        } else {
            Ok((transition, None))
        }
    }

    pub fn reconcile_callback(
        &mut self,
        seed: &mut NtOrderQuerySeed,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    ) -> Result<CancelTransition> {
        let transition = self.reconcile(seed, cached, now_ns, retry_timeout_ns)?;
        Ok(match transition {
            CancelTransition::Begin(_) => CancelTransition::NoOperation,
            other => other,
        })
    }

    pub fn settle_operation(
        &mut self,
        generation: u64,
        seed: &mut NtOrderQuerySeed,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    ) -> Result<CancelTransition> {
        if !matches!(
            self.routing_state,
            CancelRoutingState::Attempting {
                generation: active_generation,
                ..
            } if active_generation == generation
        ) {
            return Ok(CancelTransition::NoOperation);
        }
        self.reconcile_callback(seed, cached, now_ns, retry_timeout_ns)
    }

    pub fn settle_synchronous_failure(&mut self, generation: u64) {
        if let CancelRoutingState::Attempting {
            generation: active_generation,
            not_before_ns,
            ..
        } = self.routing_state
        {
            if active_generation == generation {
                self.routing_state = CancelRoutingState::Backoff { not_before_ns };
            }
        }
    }

    pub fn primary_error(&self, client_order_id: ClientOrderId) -> Option<anyhow::Error> {
        if let Some(conflict) = &self.health.recovery_identity_conflict {
            return Some(anyhow::anyhow!(
                "resting cancellation venue identity conflict: client_order_id={client_order_id} captured={} observed={}",
                conflict.captured,
                conflict.observed
            ));
        }
        if self.health.recovery_identity_unavailable {
            return Some(anyhow::anyhow!(
                "resting cancellation recovery identity unavailable: client_order_id={client_order_id}"
            ));
        }
        if self.health.retry_escalated {
            return Some(anyhow::anyhow!(
                "resting cancellation recovery attempts escalated: client_order_id={client_order_id} attempts={}",
                self.total_recovery_attempts
            ));
        }
        self.health.liveness.map(|failure| {
            anyhow::anyhow!(
                "resting cancellation liveness failure: client_order_id={client_order_id} failure={failure:?}"
            )
        })
    }

    fn reconcile(
        &mut self,
        seed: &mut NtOrderQuerySeed,
        cached: Option<&OrderAny>,
        now_ns: u64,
        retry_timeout_ns: u64,
    ) -> Result<CancelTransition> {
        if self.health.recovery_identity_conflict.is_some() {
            return Ok(CancelTransition::NoOperation);
        }
        if self.last_observed_ns.is_some_and(|prior| now_ns < prior) {
            anyhow::bail!(
                "resting cancellation actor clock regressed: prior_ns={} observed_ns={now_ns}",
                self.last_observed_ns.unwrap_or_default()
            );
        }
        match seed.reconcile_cached_identity(cached)? {
            IdentityTransition::Conflict(conflict) => {
                self.health.recovery_identity_conflict = Some(conflict);
                return Ok(CancelTransition::NoOperation);
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
        if now_ns >= self.quote_deadline_ns {
            let failure = if matches!(
                observation,
                CancelObservation::PendingCancelUnqueryable
                    | CancelObservation::PendingCancelQueryable
            ) {
                BoltV3CancellationLivenessFailure::StuckPendingCancel
            } else {
                BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded
            };
            self.health.liveness.get_or_insert(failure);
        }
        let next_deadline_ns = now_ns
            .checked_add(retry_timeout_ns)
            .ok_or_else(|| anyhow::anyhow!("cancel recovery deadline overflow"))?;
        let (next_state, transition) =
            transition(self.routing_state, observation, now_ns, next_deadline_ns);
        self.routing_state = next_state;
        self.last_observed_ns = Some(now_ns);
        Ok(transition)
    }

    fn begin_operation(
        &mut self,
        kind: CancelOperationKind,
        now_ns: u64,
        retry_timeout_ns: u64,
        escalation_attempts: u32,
    ) -> Result<ArmedCancelOperation> {
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
        Ok(ArmedCancelOperation { kind, generation })
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
            if now_ns >= not_before_ns {
                Begin(Query)
            } else {
                NoOperation
            },
        ),
        (Backoff { not_before_ns }, Retryable) => (
            Backoff { not_before_ns },
            if now_ns >= not_before_ns {
                Begin(Cancel)
            } else {
                NoOperation
            },
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
            if now_ns >= not_before_ns {
                Begin(Query)
            } else {
                NoOperation
            },
        ),
        (PendingCancel { not_before_ns }, Retryable) => (
            Backoff { not_before_ns },
            if now_ns >= not_before_ns {
                Begin(Cancel)
            } else {
                NoOperation
            },
        ),
        (PendingCancel { not_before_ns }, PendingCancelUnqueryable) => {
            (PendingCancel { not_before_ns }, NoOperation)
        }
        (PendingCancel { not_before_ns }, PendingCancelQueryable) => (
            PendingCancel { not_before_ns },
            if now_ns >= not_before_ns {
                Begin(Query)
            } else {
                NoOperation
            },
        ),
        (PendingCancel { .. }, Terminal) => (Ready, Remove),
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
    fn venue_identity_conflict_is_a_monotonic_fail_atomic_hold() {
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

        let (transition, operation) = record
            .plan_drive(&mut seed, Some(&accepted_b), 30, 10, 3)
            .unwrap();
        assert_eq!(transition, CancelTransition::NoOperation);
        assert!(operation.is_none());
        assert_eq!(record.routing_state, before.routing_state);
        assert_eq!(record.generation, before.generation);
        assert_eq!(
            record.total_recovery_attempts,
            before.total_recovery_attempts
        );
        assert_eq!(record.last_observed_ns, before.last_observed_ns);
        assert!(record.health.retry_escalated);
        assert!(record.health.recovery_identity_conflict.is_some());

        let mut terminal = accepted_a;
        let canceled = TestOrderEventStubs::canceled(
            &terminal,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from("VENUE-A")),
        );
        terminal.apply(canceled).unwrap();
        let (transition, operation) = record
            .plan_drive(&mut seed, Some(&terminal), 40, 10, 3)
            .unwrap();
        assert_eq!(transition, CancelTransition::NoOperation);
        assert!(operation.is_none());
        assert_eq!(record.routing_state, before.routing_state);
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
    fn retry_escalation_recoverability_conflict_and_liveness_compose() {
        let mut record = RestingOrderCancelRecord::new(5);
        record.health.recovery_identity_unavailable = true;
        record.health.retry_escalated = true;
        record.health.liveness =
            Some(BoltV3CancellationLivenessFailure::CancellationDeadlineExceeded);
        record.health.recovery_identity_conflict = Some(BoltV3RecoveryIdentityConflict {
            captured: VenueOrderId::from("A"),
            observed: VenueOrderId::from("B"),
        });
        let snapshot = record.health_snapshot(ClientOrderId::from("C"));
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
    }

    #[test]
    fn callbacks_cannot_overwrite_a_newer_attempt_generation() {
        let order = initialized_order("generation-guard");
        let mut seed = NtOrderQuerySeed::new(order.clone());
        let mut record = RestingOrderCancelRecord::new(100);
        let (_, operation) = record
            .plan_drive(&mut seed, Some(&order), 10, 10, 3)
            .unwrap();
        let operation = operation.unwrap();
        record.settle_synchronous_failure(operation.generation);
        let settled = record.routing_state;
        assert_eq!(
            record
                .settle_operation(operation.generation, &mut seed, Some(&order), 11, 10,)
                .unwrap(),
            CancelTransition::NoOperation
        );
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
        let error = record
            .plan_drive(&mut seed, Some(&order), 19, 10, 3)
            .unwrap_err();
        assert!(error.to_string().contains("clock regressed"));
        assert_eq!(record, before);
    }
}
