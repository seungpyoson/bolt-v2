use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::Result;
use nautilus_common::actor::DataActorNative;
#[cfg(any(test, feature = "test-current-evidence-inspection"))]
use nautilus_model::types::{Price, Quantity};
use nautilus_model::{
    enums::{OrderSide, OrderStatus, PositionSide as NtPositionSide},
    identifiers::{ClientOrderId, InstrumentId, PositionId, StrategyId},
    orders::{Order, OrderAny},
};
use nautilus_trading::{Strategy, StrategyNative};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_economics_runtime::{
        BoundExecutionEconomics, EconomicsAdmission, EconomicsAdmissionIntent,
        EconomicsAdmissionPolicy, EconomicsSizingIntent, EconomicsSizingQuote,
        RestingOrderEconomicsRefresh, refresh_resting_order_economics,
    },
    bolt_v3_quote_lifecycle::{
        MakerQuoteLifecycleHandle, MakerQuoteLifecycleIdentity, MakerQuoteLifecycleRefinement,
        MakerQuoteLifecycleRefinementEvent, MakerQuoteLifecycleRefinementOutcome,
        MakerQuoteTerminalDisposition,
    },
    bolt_v3_requote_budget::RequoteBudgetPair,
    bolt_v3_submit_admission::{
        build_submit_admission_request_from_economics, order_admission_facts,
    },
    economics::{LifecyclePath, PlannedFillNotional, PositionContext},
    integrations::nautilus::economics::{
        NautilusEconomicsIntent, NautilusEstimateLiquidityRole, NautilusPlannedFillLeg,
        economics_request_from_nautilus,
    },
};

use super::{
    BoltV3FinalOrderEconomicsInput, BoltV3FinalOrderEconomicsScenario, BoltV3NtVenueMutationSink,
    BoltV3OrderExecutionPolicy, BoltV3RouteAttemptCompletion, BoltV3RouteAttemptParticipant,
    BoltV3SubmitAttemptKind, BoltV3SubmitAttemptOutcome, BoltV3TakerEconomicsSizingInput,
    NtStrategyVenueMutationSink, economics_basis::seal_final_order_economics_basis,
};

mod cancel_coordinator;

pub use cancel_coordinator::{
    BoltV3CancellationLivenessFailure, BoltV3RecoveryIdentityConflict,
    BoltV3RestingOrderCancelHealthSnapshot,
};
use cancel_coordinator::{CancelDriveInput, TrackedOrderCancellation};

#[derive(Clone)]
pub struct BoltV3OrderEconomicsHandle {
    economics: BoundExecutionEconomics,
    tracked_orders: Arc<RwLock<TrackedMakerOrderRegistry>>,
}

#[derive(Debug, Default)]
struct TrackedMakerOrderRegistry {
    records: BTreeMap<ClientOrderId, TrackedMakerOrderRecord>,
    retired_provisional: BTreeMap<ClientOrderId, u64>,
    retained_terminal_orders: BTreeMap<ClientOrderId, MakerQuoteOrderAuthority>,
    requote_budgets_by_strategy: BTreeMap<StrategyId, RequoteBudgetPair>,
    lifecycle: RestingRegistryLifecycle,
    next_drain_generation: u64,
    next_generation: u64,
    health: RestingRegistryHealth,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RestingRegistryLifecycle {
    #[default]
    Open,
    Draining {
        generation: u64,
    },
    Stopped {
        generation: u64,
    },
}

pub struct BoltV3RestingOrderDrainCapability {
    handle: BoltV3OrderEconomicsHandle,
    generation: u64,
}

impl std::fmt::Debug for BoltV3RestingOrderDrainCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoltV3RestingOrderDrainCapability")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
enum RetentionHorizonCapability<'a> {
    RegistrationEpochFinal {
        lifecycle: &'a MakerQuoteLifecycleHandle,
        registration_generation: u64,
        current_client_order_id: ClientOrderId,
    },
    ScopeClosure {
        lifecycles: &'a [MakerQuoteLifecycleHandle],
        now_ns: u64,
    },
    ComponentStop {
        drain_generation: u64,
    },
}

impl TrackedMakerOrderRegistry {
    fn allocate_generation(&mut self) -> Option<u64> {
        let generation = self.next_generation.checked_add(1)?;
        self.next_generation = generation;
        Some(generation)
    }

    fn remove_registration_record(
        &mut self,
        client_order_id: &ClientOrderId,
    ) -> Option<TrackedMakerOrderRecord> {
        self.records.remove(client_order_id)
    }

    fn remove_terminal_record(
        &mut self,
        client_order_id: &ClientOrderId,
    ) -> Option<TrackedMakerOrderRecord> {
        let record = self.remove_registration_record(client_order_id);
        if let Some(governed) = record.as_ref().and_then(TrackedMakerOrderRecord::governed)
            && governed.registration_state == RestingRegistrationState::Provisional
        {
            self.retired_provisional
                .insert(*client_order_id, governed.registration_generation);
        }
        record
    }

    fn cancellation(&self, client_order_id: &ClientOrderId) -> Option<&TrackedOrderCancellation> {
        self.records
            .get(client_order_id)
            .map(|record| &record.cancellation)
    }

    fn cancellation_mut(
        &mut self,
        client_order_id: &ClientOrderId,
    ) -> Option<&mut TrackedOrderCancellation> {
        self.records
            .get_mut(client_order_id)
            .map(|record| &mut record.cancellation)
    }

    fn requote_budget(&self, client_order_id: &ClientOrderId) -> Option<RequoteBudgetPair> {
        self.records
            .get(client_order_id)
            .and_then(|record| record.requote_budget.clone())
    }

    fn tracked_order_ids(&self) -> Vec<ClientOrderId> {
        self.records.keys().copied().collect()
    }
}

#[derive(Clone, Debug)]
struct MakerQuoteOrderAuthority {
    client_order_id: ClientOrderId,
    registration_generation: u64,
    identity: MakerQuoteLifecycleIdentity,
    lifecycle: MakerQuoteLifecycleHandle,
    scope: MakerQuoteOrderScope,
    retained: Option<MakerQuoteRetainedTerminal>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MakerQuoteOrderScope {
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    order_side: OrderSide,
}

impl MakerQuoteOrderScope {
    fn from_order(order: &OrderAny) -> Self {
        Self {
            strategy_id: order.strategy_id(),
            instrument_id: order.instrument_id(),
            order_side: order.order_side(),
        }
    }

    fn matches(&self, order: &OrderAny) -> bool {
        *self == Self::from_order(order)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MakerQuoteRetainedTerminal {
    Terminal(MakerQuoteTerminalDisposition),
    ReopenedFrom(MakerQuoteTerminalDisposition),
}

impl MakerQuoteRetainedTerminal {
    fn can_refine(self) -> bool {
        match self {
            Self::Terminal(disposition) => disposition.can_refine(),
            Self::ReopenedFrom(_) => true,
        }
    }
}

impl MakerQuoteOrderAuthority {
    fn new(order: &OrderAny, generation: u64, lifecycle: MakerQuoteLifecycleHandle) -> Self {
        Self::new_at_lifecycle_generation(order, generation, generation, lifecycle)
    }

    fn new_at_lifecycle_generation(
        order: &OrderAny,
        registration_generation: u64,
        lifecycle_generation: u64,
        lifecycle: MakerQuoteLifecycleHandle,
    ) -> Self {
        let client_order_id = order.client_order_id();
        Self {
            client_order_id,
            registration_generation,
            identity: MakerQuoteLifecycleIdentity::new(
                client_order_id.as_str(),
                lifecycle_generation,
            ),
            lifecycle,
            scope: MakerQuoteOrderScope::from_order(order),
            retained: None,
        }
    }

    fn rebind_identity(&mut self, identity: MakerQuoteLifecycleIdentity) -> Result<()> {
        anyhow::ensure!(
            identity.client_order_id() == self.client_order_id.as_str(),
            "maker quote lifecycle identity does not match its tracked order"
        );
        self.identity = identity;
        Ok(())
    }

    fn terminal_event(
        &mut self,
        disposition: MakerQuoteTerminalDisposition,
    ) -> MakerQuoteLifecycleRefinementEvent {
        let prior = self.retained;
        let next = match prior {
            None => MakerQuoteRetainedTerminal::Terminal(disposition),
            Some(MakerQuoteRetainedTerminal::Terminal(previous)) => {
                MakerQuoteRetainedTerminal::Terminal(previous.refine_terminal_with(disposition))
            }
            Some(MakerQuoteRetainedTerminal::ReopenedFrom(_)) => {
                MakerQuoteRetainedTerminal::Terminal(disposition)
            }
        };
        let (stable_effect, closes_reopened) = match (prior, next) {
            (None, MakerQuoteRetainedTerminal::Terminal(authoritative)) => {
                (Some(authoritative), false)
            }
            (
                Some(MakerQuoteRetainedTerminal::Terminal(previous)),
                MakerQuoteRetainedTerminal::Terminal(authoritative),
            ) if previous != authoritative => (Some(authoritative), false),
            (
                Some(MakerQuoteRetainedTerminal::ReopenedFrom(
                    MakerQuoteTerminalDisposition::Canceled,
                )),
                MakerQuoteRetainedTerminal::Terminal(MakerQuoteTerminalDisposition::Filled),
            ) => (Some(MakerQuoteTerminalDisposition::Filled), true),
            (
                Some(
                    MakerQuoteRetainedTerminal::Terminal(_)
                    | MakerQuoteRetainedTerminal::ReopenedFrom(_),
                ),
                MakerQuoteRetainedTerminal::Terminal(_),
            ) => (
                None,
                matches!(prior, Some(MakerQuoteRetainedTerminal::ReopenedFrom(_))),
            ),
            (_, MakerQuoteRetainedTerminal::ReopenedFrom(_)) => unreachable!(),
        };
        self.retained = Some(next);
        MakerQuoteLifecycleRefinementEvent::new(
            self.identity.clone(),
            MakerQuoteLifecycleRefinement::Terminal {
                stable_effect,
                closes_reopened,
            },
        )
    }

    fn reopening_event(&mut self) -> Result<MakerQuoteLifecycleRefinementEvent> {
        let retained = self.retained.ok_or_else(|| {
            anyhow::anyhow!("maker quote reopening requires retained per-order terminal truth")
        })?;
        let reopened_from = match retained {
            MakerQuoteRetainedTerminal::Terminal(
                disposition @ (MakerQuoteTerminalDisposition::Canceled
                | MakerQuoteTerminalDisposition::Filled),
            )
            | MakerQuoteRetainedTerminal::ReopenedFrom(
                disposition @ (MakerQuoteTerminalDisposition::Canceled
                | MakerQuoteTerminalDisposition::Filled),
            ) => disposition,
            MakerQuoteRetainedTerminal::Terminal(
                MakerQuoteTerminalDisposition::Denied
                | MakerQuoteTerminalDisposition::Rejected
                | MakerQuoteTerminalDisposition::Expired
                | MakerQuoteTerminalDisposition::Voided,
            )
            | MakerQuoteRetainedTerminal::ReopenedFrom(
                MakerQuoteTerminalDisposition::Denied
                | MakerQuoteTerminalDisposition::Rejected
                | MakerQuoteTerminalDisposition::Expired
                | MakerQuoteTerminalDisposition::Voided,
            ) => anyhow::bail!("maker quote final terminal truth cannot reopen"),
        };
        self.retained = Some(MakerQuoteRetainedTerminal::ReopenedFrom(reopened_from));
        Ok(MakerQuoteLifecycleRefinementEvent::new(
            self.identity.clone(),
            MakerQuoteLifecycleRefinement::Reopened,
        ))
    }

    fn retention_horizon_event(&self) -> MakerQuoteLifecycleRefinementEvent {
        MakerQuoteLifecycleRefinementEvent::new(
            self.identity.clone(),
            MakerQuoteLifecycleRefinement::RetentionHorizon,
        )
    }

    fn can_refine(&self) -> bool {
        self.retained
            .is_some_and(MakerQuoteRetainedTerminal::can_refine)
    }

    fn shares_lifecycle_scope_with(&self, lifecycle: &MakerQuoteLifecycleHandle) -> bool {
        self.lifecycle.shares_lifecycle_scope_with(lifecycle)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RestingRegistryHealth {
    #[default]
    Healthy,
    Poisoned,
    MissingMoneyRelevantAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RestingOrderEconomicsRecord {
    admission: EconomicsAdmission,
    authorized_quantity_ceiling: Decimal,
}

#[derive(Clone, Debug)]
struct TrackedMakerOrderRecord {
    authority: TrackedMakerOrderAuthority,
    requote_budget: Option<RequoteBudgetPair>,
    cancellation: TrackedOrderCancellation,
}

#[derive(Clone, Debug)]
enum TrackedMakerOrderAuthority {
    Governed(Box<GovernedMakerOrderAuthority>),
    MissingAfterRetentionHorizon,
}

#[derive(Clone, Debug)]
struct GovernedMakerOrderAuthority {
    registration_generation: u64,
    registration_state: RestingRegistrationState,
    economics: Option<RestingOrderEconomicsRecord>,
    maker_lifecycle: MakerQuoteOrderAuthority,
}

impl TrackedMakerOrderRecord {
    fn new_governed(
        registration_generation: u64,
        registration_state: RestingRegistrationState,
        economics: Option<RestingOrderEconomicsRecord>,
        requote_budget: Option<RequoteBudgetPair>,
        maker_lifecycle: MakerQuoteOrderAuthority,
        cancellation: TrackedOrderCancellation,
    ) -> Self {
        Self {
            authority: TrackedMakerOrderAuthority::Governed(Box::new(
                GovernedMakerOrderAuthority {
                    registration_generation,
                    registration_state,
                    economics,
                    maker_lifecycle,
                },
            )),
            requote_budget,
            cancellation,
        }
    }

    fn new_cancellation_only(
        order: OrderAny,
        requote_budget: Option<RequoteBudgetPair>,
        quote_deadline_ns: u64,
    ) -> Self {
        let mut cancellation = TrackedOrderCancellation::new(order);
        cancellation.request_intent(quote_deadline_ns);
        Self {
            authority: TrackedMakerOrderAuthority::MissingAfterRetentionHorizon,
            requote_budget,
            cancellation,
        }
    }

    fn governed(&self) -> Option<&GovernedMakerOrderAuthority> {
        match &self.authority {
            TrackedMakerOrderAuthority::Governed(authority) => Some(authority.as_ref()),
            TrackedMakerOrderAuthority::MissingAfterRetentionHorizon => None,
        }
    }

    fn governed_mut(&mut self) -> Option<&mut GovernedMakerOrderAuthority> {
        match &mut self.authority {
            TrackedMakerOrderAuthority::Governed(authority) => Some(authority.as_mut()),
            TrackedMakerOrderAuthority::MissingAfterRetentionHorizon => None,
        }
    }

    fn into_governed(self) -> Option<GovernedMakerOrderAuthority> {
        match self.authority {
            TrackedMakerOrderAuthority::Governed(authority) => Some(*authority),
            TrackedMakerOrderAuthority::MissingAfterRetentionHorizon => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestingRegistrationState {
    Provisional,
    Committed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3RestingRegistrationRejectionKind {
    InvalidPlannedFillShape,
    NonPositiveQuantity,
    RegistryUnavailable,
    RetentionScopeClosed,
    DuplicateClientOrderId,
    GenerationOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RestingRegistrationRejection {
    kind: BoltV3RestingRegistrationRejectionKind,
    diagnostic: String,
}

impl BoltV3RestingRegistrationRejection {
    fn new(
        kind: BoltV3RestingRegistrationRejectionKind,
        diagnostic: impl std::fmt::Display,
    ) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.to_string(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> BoltV3RestingRegistrationRejectionKind {
        self.kind
    }

    #[must_use]
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3RestingRollbackInvariantFailure {
    RegistryUnavailable,
    RegistrationGenerationReplaced,
    ParticipantSettlementFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RoutedNonSubmittedOutcome(BoltV3SubmitAttemptOutcome);

impl BoltV3RoutedNonSubmittedOutcome {
    fn try_new(
        outcome: BoltV3SubmitAttemptOutcome,
    ) -> std::result::Result<Self, BoltV3SubmitAttemptOutcome> {
        match outcome.kind() {
            BoltV3SubmitAttemptKind::RouteValidationRejected
            | BoltV3SubmitAttemptKind::IntentEvidenceRejected
            | BoltV3SubmitAttemptKind::AdmissionRejected
            | BoltV3SubmitAttemptKind::PolicySkipped
            | BoltV3SubmitAttemptKind::PreSinkRejected
            | BoltV3SubmitAttemptKind::SinkRejected => Ok(Self(outcome)),
            BoltV3SubmitAttemptKind::Submitted => Err(outcome),
        }
    }

    #[must_use]
    pub fn kind(&self) -> BoltV3SubmitAttemptKind {
        self.0.kind()
    }

    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.0.diagnostic()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3RestingSubmitTransactionOutcome {
    RegistrationRejected(BoltV3RestingRegistrationRejection),
    Attempt(BoltV3SubmitAttemptOutcome),
    RollbackInvariantFailed {
        original: BoltV3RoutedNonSubmittedOutcome,
        reason: BoltV3RestingRollbackInvariantFailure,
    },
}

impl BoltV3RestingSubmitTransactionOutcome {
    #[must_use]
    pub fn is_submitted(&self) -> bool {
        match self {
            Self::Attempt(outcome) => outcome.is_submitted(),
            Self::RegistrationRejected(_) | Self::RollbackInvariantFailed { .. } => false,
        }
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn submitted_for_test() -> Self {
        Self::Attempt(BoltV3SubmitAttemptOutcome::submitted_for_test())
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn submitted_with_linkage_for_test(
        instrument_id: InstrumentId,
        order_side: OrderSide,
        price: Price,
        quantity: Quantity,
        client_order_id: ClientOrderId,
    ) -> Self {
        Self::Attempt(BoltV3SubmitAttemptOutcome::submitted_with_linkage_for_test(
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        ))
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn policy_skipped_for_test() -> Self {
        Self::Attempt(BoltV3SubmitAttemptOutcome::policy_skipped_for_test())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3RestingRegistrationCapability {
    PreSink,
    SinkInvoked,
    Settled,
}

pub trait BoltV3RestingRegistrationCommitParticipant: std::fmt::Debug {
    fn requote_budget(&self) -> Option<RequoteBudgetPair>;
    fn maker_lifecycle(&self) -> MakerQuoteLifecycleHandle;
    fn arm_at_identity(&mut self, identity: MakerQuoteLifecycleIdentity) -> Result<()>;
    fn mark_sink_invoked(&mut self, actor_now_ns: u64) -> Result<()>;
    fn registration_capability(&self, generation: u64) -> BoltV3RestingRegistrationCapability;
    fn settle_submitted(&mut self, generation: u64) -> Result<()>;
    fn settle_command_issued(&mut self, generation: u64) -> Result<()>;
    fn settle_sink_rejected(&mut self, generation: u64) -> Result<()>;
    fn settle_callback_retired(&mut self, generation: u64) -> Result<()>;
    fn abort_pre_sink(&mut self, generation: u64) -> Result<()>;
    fn fail_pre_sink_invariant(&mut self, generation: u64) -> Result<()>;
    fn fail_post_sink_invariant(&mut self, generation: u64) -> Result<()>;
}

#[derive(Debug)]
struct RestingRegistrationTransaction {
    registry: Arc<RwLock<TrackedMakerOrderRegistry>>,
    client_order_id: ClientOrderId,
    generation: u64,
    identity: MakerQuoteLifecycleIdentity,
    lifecycle: MakerQuoteLifecycleHandle,
    participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    rollback_failure: Arc<Mutex<Option<BoltV3RestingRollbackInvariantFailure>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestingRegistrationOwnership {
    Active,
    RetiredByCallback,
    Replaced,
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnsettledRestingRegistrationCapability {
    PreSink,
    SinkInvoked,
}

impl RestingRegistrationTransaction {
    fn ownership(&self, registry: &TrackedMakerOrderRegistry) -> RestingRegistrationOwnership {
        let active_generation = registry
            .records
            .get(&self.client_order_id)
            .and_then(TrackedMakerOrderRecord::governed)
            .map(|authority| authority.registration_generation);
        let retired_generation = registry
            .retired_provisional
            .get(&self.client_order_id)
            .copied();
        match (active_generation, retired_generation) {
            (Some(generation), _) if generation == self.generation => {
                RestingRegistrationOwnership::Active
            }
            (None, Some(generation)) if generation == self.generation => {
                RestingRegistrationOwnership::RetiredByCallback
            }
            (Some(_), _) => RestingRegistrationOwnership::Replaced,
            (None, _) => RestingRegistrationOwnership::Missing,
        }
    }

    fn record_settlement_failure(
        &self,
        ownership_failure: Option<BoltV3RestingRollbackInvariantFailure>,
        settlement: Result<()>,
        retention_horizon: Result<usize>,
    ) {
        let failure = match (ownership_failure, settlement, retention_horizon) {
            (Some(reason), _, _) => Some(reason),
            (None, Err(_), _) | (None, Ok(()), Err(_)) => {
                Some(BoltV3RestingRollbackInvariantFailure::ParticipantSettlementFailed)
            }
            (None, Ok(()), Ok(_)) => None,
        };
        if let Some(reason) = failure {
            self.record_failure(reason);
        }
    }

    fn settle_route(&mut self, completion: BoltV3RouteAttemptCompletion) {
        let mut registry = match self.registry.write() {
            Ok(registry) => registry,
            Err(poisoned) => {
                let mut registry = poisoned.into_inner();
                registry.health = RestingRegistryHealth::Poisoned;
                registry
            }
        };
        let ownership = self.ownership(&registry);
        let failure = match ownership {
            RestingRegistrationOwnership::Active => match completion {
                BoltV3RouteAttemptCompletion::Submitted => {
                    let record = registry
                        .records
                        .get_mut(&self.client_order_id)
                        .expect("owned resting registration must remain present");
                    record
                        .governed_mut()
                        .expect("owned resting registration must retain exact authority")
                        .registration_state = RestingRegistrationState::Committed;
                    None
                }
                BoltV3RouteAttemptCompletion::SinkRejected => {
                    registry.remove_registration_record(&self.client_order_id);
                    None
                }
            },
            RestingRegistrationOwnership::RetiredByCallback => {
                registry.retired_provisional.remove(&self.client_order_id);
                None
            }
            RestingRegistrationOwnership::Replaced => {
                registry.health = RestingRegistryHealth::Poisoned;
                Some(BoltV3RestingRollbackInvariantFailure::RegistrationGenerationReplaced)
            }
            RestingRegistrationOwnership::Missing => {
                registry.health = RestingRegistryHealth::Poisoned;
                Some(BoltV3RestingRollbackInvariantFailure::RegistryUnavailable)
            }
        };
        drop(registry);
        let settlement = match ownership {
            RestingRegistrationOwnership::Active => match completion {
                BoltV3RouteAttemptCompletion::Submitted => {
                    self.participant.settle_submitted(self.generation)
                }
                BoltV3RouteAttemptCompletion::SinkRejected => {
                    self.participant.settle_sink_rejected(self.generation)
                }
            },
            RestingRegistrationOwnership::RetiredByCallback => {
                self.participant.settle_callback_retired(self.generation)
            }
            RestingRegistrationOwnership::Replaced | RestingRegistrationOwnership::Missing => {
                match self.participant.registration_capability(self.generation) {
                    BoltV3RestingRegistrationCapability::PreSink => {
                        self.participant.fail_pre_sink_invariant(self.generation)
                    }
                    BoltV3RestingRegistrationCapability::SinkInvoked => {
                        self.participant.fail_post_sink_invariant(self.generation)
                    }
                    BoltV3RestingRegistrationCapability::Settled => Ok(()),
                }
            }
        };
        let retention_horizon = match (ownership, completion, &settlement, failure) {
            (
                RestingRegistrationOwnership::Active
                | RestingRegistrationOwnership::RetiredByCallback,
                BoltV3RouteAttemptCompletion::Submitted,
                Ok(()),
                None,
            ) => apply_retention_horizon(
                &self.registry,
                RetentionHorizonCapability::RegistrationEpochFinal {
                    lifecycle: &self.lifecycle,
                    registration_generation: self.generation,
                    current_client_order_id: self.client_order_id,
                },
            ),
            (
                RestingRegistrationOwnership::Active
                | RestingRegistrationOwnership::RetiredByCallback
                | RestingRegistrationOwnership::Replaced
                | RestingRegistrationOwnership::Missing,
                BoltV3RouteAttemptCompletion::Submitted
                | BoltV3RouteAttemptCompletion::SinkRejected,
                Ok(()) | Err(_),
                Some(_) | None,
            ) => Ok(0),
        };
        self.record_settlement_failure(failure, settlement, retention_horizon);
    }

    fn record_failure(&self, reason: BoltV3RestingRollbackInvariantFailure) {
        match self.registry.write() {
            Ok(mut registry) => registry.health = RestingRegistryHealth::Poisoned,
            Err(poisoned) => poisoned.into_inner().health = RestingRegistryHealth::Poisoned,
        }
        *self
            .rollback_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason);
    }
}

impl BoltV3RouteAttemptParticipant for RestingRegistrationTransaction {
    fn consume_at_pre_sink(&mut self) -> Result<()> {
        self.participant.arm_at_identity(self.identity.clone())
    }

    fn mark_sink_invoked(&mut self, actor_now_ns: u64) -> Result<()> {
        self.participant.mark_sink_invoked(actor_now_ns)
    }

    fn complete(&mut self, completion: BoltV3RouteAttemptCompletion) {
        self.settle_route(completion);
    }
}

impl Drop for RestingRegistrationTransaction {
    fn drop(&mut self) {
        let capability = match self.participant.registration_capability(self.generation) {
            BoltV3RestingRegistrationCapability::PreSink => {
                UnsettledRestingRegistrationCapability::PreSink
            }
            BoltV3RestingRegistrationCapability::SinkInvoked => {
                UnsettledRestingRegistrationCapability::SinkInvoked
            }
            BoltV3RestingRegistrationCapability::Settled => return,
        };
        let mut registry = match self.registry.write() {
            Ok(registry) => registry,
            Err(poisoned) => {
                let mut registry = poisoned.into_inner();
                registry.health = RestingRegistryHealth::Poisoned;
                registry
            }
        };
        let ownership = self.ownership(&registry);
        let failure = match (ownership, capability) {
            (
                RestingRegistrationOwnership::Active,
                UnsettledRestingRegistrationCapability::PreSink,
            ) => {
                registry.remove_registration_record(&self.client_order_id);
                None
            }
            (
                RestingRegistrationOwnership::Active,
                UnsettledRestingRegistrationCapability::SinkInvoked,
            ) => {
                registry.health = RestingRegistryHealth::Poisoned;
                None
            }
            (RestingRegistrationOwnership::RetiredByCallback, _) => {
                registry.retired_provisional.remove(&self.client_order_id);
                None
            }
            (RestingRegistrationOwnership::Replaced, _) => {
                registry.health = RestingRegistryHealth::Poisoned;
                Some(BoltV3RestingRollbackInvariantFailure::RegistrationGenerationReplaced)
            }
            (RestingRegistrationOwnership::Missing, _) => {
                Some(BoltV3RestingRollbackInvariantFailure::RegistryUnavailable)
            }
        };
        drop(registry);
        let settlement = match (ownership, capability) {
            (
                RestingRegistrationOwnership::Active,
                UnsettledRestingRegistrationCapability::PreSink,
            ) => self.participant.abort_pre_sink(self.generation),
            (
                RestingRegistrationOwnership::Active,
                UnsettledRestingRegistrationCapability::SinkInvoked,
            ) => self.participant.fail_post_sink_invariant(self.generation),
            (RestingRegistrationOwnership::RetiredByCallback, _) => {
                self.participant.settle_callback_retired(self.generation)
            }
            (
                RestingRegistrationOwnership::Replaced | RestingRegistrationOwnership::Missing,
                UnsettledRestingRegistrationCapability::PreSink,
            ) => self.participant.fail_pre_sink_invariant(self.generation),
            (
                RestingRegistrationOwnership::Replaced | RestingRegistrationOwnership::Missing,
                UnsettledRestingRegistrationCapability::SinkInvoked,
            ) => self.participant.fail_post_sink_invariant(self.generation),
        };
        self.record_settlement_failure(failure, settlement, Ok(0));
    }
}

impl BoltV3OrderEconomicsHandle {
    pub fn new(economics: BoundExecutionEconomics) -> Self {
        Self {
            economics,
            tracked_orders: Arc::new(RwLock::new(TrackedMakerOrderRegistry::default())),
        }
    }

    pub fn validate_cancel_recovery_cadence(&self, cadence_ns: u64) -> Result<()> {
        let margin_ns = self.economics.resting_order_refresh_margin_ns()?;
        let retry_timeout_ns = self.economics.cancel_retry_timeout_ns()?;
        anyhow::ensure!(
            cadence_ns > 0,
            "cancel-recovery cadence must be positive: cadence_ns={cadence_ns}"
        );
        let retry_intervals = retry_timeout_ns
            .checked_div(cadence_ns)
            .and_then(|quotient| {
                quotient.checked_add(u64::from(retry_timeout_ns % cadence_ns != 0))
            })
            .ok_or_else(|| anyhow::anyhow!("cancel-recovery cadence arithmetic overflow"))?;
        let rounded_retry_ns = retry_intervals
            .checked_mul(cadence_ns)
            .ok_or_else(|| anyhow::anyhow!("cancel-recovery cadence arithmetic overflow"))?;
        let required_margin_ns = cadence_ns
            .checked_add(rounded_retry_ns)
            .ok_or_else(|| anyhow::anyhow!("cancel-recovery cadence arithmetic overflow"))?;
        anyhow::ensure!(
            required_margin_ns < margin_ns,
            "cancel-recovery cadence must leave strict pre-expiry margin: cadence_ns={cadence_ns} retry_timeout_ns={retry_timeout_ns} required_margin_ns={required_margin_ns} margin_ns={margin_ns}"
        );
        Ok(())
    }

    pub fn drive_all_resting_order_economics_at_ms<S>(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        strategy: &mut S,
        execution_client_id: &str,
        now_ms: u64,
    ) -> Result<()>
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let now_ns = now_ms
            .checked_mul(crate::bolt_v3_numeric::NANOS_PER_MILLI_U64)
            .ok_or_else(|| anyhow::anyhow!("resting economics clock overflow"))?;
        let observations = self
            .resting_order_ids()?
            .into_iter()
            .map(|client_order_id| {
                let order = strategy.cache().order(&client_order_id);
                (client_order_id, order)
            })
            .collect();
        let mut sink = NtStrategyVenueMutationSink { strategy };
        drive_observed_resting_order_economics(
            self,
            policy,
            &mut sink,
            execution_client_id,
            observations,
            now_ns,
        )
    }

    pub fn resting_order_ids(&self) -> Result<Vec<ClientOrderId>> {
        Ok(self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("resting economics state lock poisoned"))?
            .tracked_order_ids())
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    pub fn retained_terminal_order_ids(&self) -> Result<Vec<ClientOrderId>> {
        Ok(self
            .tracked_orders
            .read()
            .map_err(|_| anyhow::anyhow!("resting economics state lock poisoned"))?
            .retained_terminal_orders
            .keys()
            .copied()
            .collect())
    }

    #[cfg(test)]
    pub(super) fn attach_requote_budget_for_test(
        &self,
        client_order_id: ClientOrderId,
        budget: RequoteBudgetPair,
    ) {
        self.tracked_orders
            .write()
            .expect("test registry should lock")
            .records
            .get_mut(&client_order_id)
            .expect("test tracked record should exist")
            .requote_budget = Some(budget);
    }

    #[cfg(test)]
    pub(crate) fn poison_tracked_order_registry_for_test(&self) {
        let registry = self.tracked_orders.clone();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = registry
                .write()
                .expect("test registry should initially lock");
            panic!("poison tracked order registry for behavior test");
        }));
        assert!(poisoned.is_err(), "test registry poison must take effect");
    }

    #[cfg(test)]
    pub(super) fn track_cancel_coordinator_order_for_test(
        &self,
        order: OrderAny,
        quote_deadline_ns: u64,
    ) {
        let client_order_id = order.client_order_id();
        let strategy_id = order.strategy_id();
        let lifecycle = MakerQuoteLifecycleHandle::new(
            crate::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false),
            crate::bolt_v3_quote_lifecycle::Leg::Yes,
        );
        let mut registry = self
            .tracked_orders
            .write()
            .expect("test registry should lock");
        let generation = registry
            .allocate_generation()
            .expect("test registration generation should remain available");
        let maker_lifecycle = MakerQuoteOrderAuthority::new(&order, generation, lifecycle);
        let mut coordinator = TrackedOrderCancellation::new(order);
        coordinator.request_intent(quote_deadline_ns);
        let requote_budget = registry
            .requote_budgets_by_strategy
            .get(&strategy_id)
            .cloned();
        registry.records.insert(
            client_order_id,
            TrackedMakerOrderRecord::new_governed(
                generation,
                RestingRegistrationState::Committed,
                None,
                requote_budget,
                maker_lifecycle,
                coordinator,
            ),
        );
    }

    fn refresh_tracked_economics(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<&OrderAny>,
        now_ns: u64,
    ) -> Result<()> {
        let mut registry = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        if cached.is_some_and(|order| {
            order.is_closed() || order.leaves_qty().as_decimal() == Decimal::ZERO
        }) {
            let disposition = maker_terminal_disposition(
                cached.expect("terminal cached-order branch requires an order"),
            )?;
            let removed = registry.remove_terminal_record(&client_order_id);
            drop(registry);
            settle_maker_terminal(&self.tracked_orders, client_order_id, removed, disposition)?;
            return Ok(());
        }
        let Some(record) = registry.records.get_mut(&client_order_id) else {
            return Ok(());
        };
        if record.cancellation.is_requested() {
            return Ok(());
        };
        let Some(governed) = record.governed_mut() else {
            return Ok(());
        };
        let Some(economics) = governed.economics.as_mut() else {
            return Ok(());
        };
        let Some(order) = cached else {
            let quote_deadline_ns = economics.admission.quote().valid_until_ns();
            record.cancellation.request_intent(quote_deadline_ns);
            return Ok(());
        };
        match refresh_resting_order_economics(
            &self.economics,
            &economics.admission,
            order.leaves_qty().as_decimal(),
            economics.authorized_quantity_ceiling,
            order.is_post_only(),
            now_ns,
        ) {
            RestingOrderEconomicsRefresh::NotDue => {}
            RestingOrderEconomicsRefresh::Complete => {
                let disposition = maker_terminal_disposition(order)?;
                let removed = registry.remove_terminal_record(&client_order_id);
                drop(registry);
                settle_maker_terminal(&self.tracked_orders, client_order_id, removed, disposition)?;
                return Ok(());
            }
            RestingOrderEconomicsRefresh::Refreshed {
                admission,
                forecast_drift,
            } => {
                if let Some(drift) = forecast_drift {
                    log::info!(
                        "resting order forecast economics changed without changing admission authority: client_order_id={client_order_id} drift={drift:?}"
                    );
                }
                economics.admission = *admission;
            }
            RestingOrderEconomicsRefresh::CancelRequired(reason) => {
                log::warn!(
                    "resting order economics requires cancellation: client_order_id={client_order_id} reason={reason:?}"
                );
                let quote_deadline_ns = economics.admission.quote().valid_until_ns();
                record.cancellation.request_intent(quote_deadline_ns);
            }
        }
        Ok(())
    }

    pub(super) fn route_resting_submit<F>(
        &self,
        order: OrderAny,
        admission: EconomicsAdmission,
        participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
        route: F,
    ) -> BoltV3RestingSubmitTransactionOutcome
    where
        F: FnOnce(Box<dyn BoltV3RouteAttemptParticipant>) -> BoltV3SubmitAttemptOutcome,
    {
        let rollback_failure = Arc::new(Mutex::new(None));
        let transaction = match self.begin_resting_registration(
            order,
            admission,
            participant,
            rollback_failure.clone(),
        ) {
            Ok(transaction) => transaction,
            Err(rejection) => {
                return BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection);
            }
        };
        let outcome = route(Box::new(transaction));
        match BoltV3RoutedNonSubmittedOutcome::try_new(outcome) {
            Err(submitted) => BoltV3RestingSubmitTransactionOutcome::Attempt(submitted),
            Ok(non_submitted) => match *rollback_failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                Some(reason) => BoltV3RestingSubmitTransactionOutcome::RollbackInvariantFailed {
                    original: non_submitted,
                    reason,
                },
                None => BoltV3RestingSubmitTransactionOutcome::Attempt(non_submitted.0),
            },
        }
    }

    fn begin_resting_registration(
        &self,
        order: OrderAny,
        admission: EconomicsAdmission,
        participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
        rollback_failure: Arc<Mutex<Option<BoltV3RestingRollbackInvariantFailure>>>,
    ) -> std::result::Result<RestingRegistrationTransaction, BoltV3RestingRegistrationRejection>
    {
        let client_order_id = order.client_order_id();
        let [leg] = admission.request().planned_fill_legs.as_slice() else {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::InvalidPlannedFillShape,
                "resting economics registration requires exactly one planned fill leg",
            ));
        };
        if leg.quantity <= Decimal::ZERO {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::NonPositiveQuantity,
                "resting economics registration requires positive quantity",
            ));
        }
        let authorized_quantity_ceiling = leg.quantity;
        let requote_budget = participant.requote_budget();
        let maker_lifecycle = participant.maker_lifecycle();
        let mut registry = match self.tracked_orders.write() {
            Ok(registry) => registry,
            Err(poisoned) => {
                poisoned.into_inner().health = RestingRegistryHealth::Poisoned;
                return Err(BoltV3RestingRegistrationRejection::new(
                    BoltV3RestingRegistrationRejectionKind::RegistryUnavailable,
                    "resting economics registry lock is poisoned",
                ));
            }
        };
        if registry.health != RestingRegistryHealth::Healthy {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::RegistryUnavailable,
                "resting economics registry health is poisoned",
            ));
        }
        if registry.lifecycle != RestingRegistryLifecycle::Open {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::RetentionScopeClosed,
                "resting economics registration is closed by the component drain latch",
            ));
        }
        if maker_lifecycle.retention_scope_is_closed() {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::RetentionScopeClosed,
                "resting economics registration lifecycle scope is closing",
            ));
        }
        if registry.records.contains_key(&client_order_id)
            || registry
                .retained_terminal_orders
                .contains_key(&client_order_id)
        {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::DuplicateClientOrderId,
                format_args!(
                    "resting economics registration rejected duplicate client order id: {client_order_id}"
                ),
            ));
        }
        let Some(generation) = registry.allocate_generation() else {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::GenerationOverflow,
                "resting economics registration generation overflow",
            ));
        };
        let strategy_id = order.strategy_id();
        let identity = MakerQuoteLifecycleIdentity::new(client_order_id.as_str(), generation);
        let maker_lifecycle = MakerQuoteOrderAuthority::new(&order, generation, maker_lifecycle);
        let lifecycle = maker_lifecycle.lifecycle.clone();
        let coordinator = TrackedOrderCancellation::new(order);
        if let Some(requote_budget) = requote_budget.as_ref() {
            registry
                .requote_budgets_by_strategy
                .insert(strategy_id, requote_budget.clone());
        }
        registry.records.insert(
            client_order_id,
            TrackedMakerOrderRecord::new_governed(
                generation,
                RestingRegistrationState::Provisional,
                Some(RestingOrderEconomicsRecord {
                    admission,
                    authorized_quantity_ceiling,
                }),
                requote_budget,
                maker_lifecycle,
                coordinator,
            ),
        );
        drop(registry);
        Ok(RestingRegistrationTransaction {
            registry: self.tracked_orders.clone(),
            client_order_id,
            generation,
            identity,
            lifecycle,
            participant,
            rollback_failure,
        })
    }

    pub(super) fn route_tracked_cancel<S>(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        sink: &mut S,
        execution_client_id: &str,
        client_order_id: ClientOrderId,
        participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> Result<()>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        let now_ns = sink.actor_time_ns()?;
        let tracked = self.request_cancel_intent(client_order_id, now_ns)?;
        if !tracked {
            anyhow::ensure!(
                !policy.allows_venue_mutation(),
                "tracked maker cancellation rejected unknown client order id: {client_order_id}"
            );
            return Ok(());
        }
        let cached = sink.cached_order(client_order_id)?;
        self.refresh_tracked_economics(client_order_id, cached.as_ref(), now_ns)?;
        self.drive_cancel_intent(
            policy,
            sink,
            CancelDriveInput {
                execution_client_id,
                client_order_id,
                cached: cached.as_ref(),
                now_ns,
                command_participant: Some(participant),
            },
        )
    }

    pub fn quote_taker_sizing(
        &self,
        intent: BoltV3TakerEconomicsSizingInput<'_>,
    ) -> Result<EconomicsSizingQuote> {
        let authority = self
            .economics
            .request_authority(&intent.instrument_id.to_string())?;
        anyhow::ensure!(
            !authority.carry_required,
            "taker entry sizing does not support a carry-bearing product surface"
        );
        anyhow::ensure!(
            intent.order_side == OrderSide::Buy,
            "terminal-value taker entry sizing requires a buy order"
        );
        let planned_fill_legs = intent
            .planned_fill_legs
            .into_iter()
            .map(|leg| NautilusPlannedFillLeg {
                price: leg.price,
                quantity: leg.quantity,
            })
            .collect::<Vec<_>>();
        let request = economics_request_from_nautilus(NautilusEconomicsIntent {
            execution_client_id: &authority.execution_client_id,
            account_id: authority.account_id.as_str(),
            instrument_id: intent.instrument_id,
            product_surface_id: authority.product_surface_id.as_str(),
            reporting_policy_id: authority.reporting_policy_id.as_str(),
            reporting_currency: authority.reporting_currency.as_str(),
            edge_basis_policy_id: authority.edge_basis_policy_id.as_str(),
            decision_correlation_id: intent.decision_correlation_id,
            side: intent.order_side,
            liquidity_role: NautilusEstimateLiquidityRole::Taker,
            planned_fill_legs: &planned_fill_legs,
            routing_attachment_id: None,
            position: None,
            lifecycle_path: LifecyclePath::HoldToRedemption,
            requested_at_ns: intent.requested_at_ns,
        })
        .map_err(|error| anyhow::anyhow!(error))?;
        let gross_expected_value = BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
            intent.terminal_value_entry.clone(),
        )
        .gross_expected_value(&planned_fill_legs)?;
        let reservation_basis =
            PlannedFillNotional::from_legs(&request.planned_fill_legs)?.amount();
        self.economics
            .quote_sizing(EconomicsSizingIntent::new(
                request,
                EconomicsAdmissionPolicy::TradingEdge {
                    minimum_core_edge_ratio: intent.terminal_value_entry.minimum_core_edge_ratio(),
                },
                gross_expected_value,
                reservation_basis,
            ))
            .map_err(Into::into)
    }

    pub(crate) fn planned_exit_position(
        &self,
        position_id: PositionId,
        side: NtPositionSide,
        quantity: Decimal,
    ) -> Result<PositionContext> {
        let side = match side {
            NtPositionSide::Long => crate::economics::PositionSide::Long,
            NtPositionSide::Short => crate::economics::PositionSide::Short,
            NtPositionSide::Flat | NtPositionSide::NoPositionSide => {
                anyhow::bail!("economics planned exit requires an open sided position")
            }
        };
        Ok(PositionContext {
            position_id: crate::economics::PositionId::try_new(position_id.to_string())?,
            side,
            quantity,
            holding_horizon_ns: self.economics.planned_exit_horizon_ns()?,
        })
    }
}

pub fn build_order_economics_submit_admission(
    economics: &BoltV3OrderEconomicsHandle,
    input: BoltV3FinalOrderEconomicsInput<'_>,
) -> Result<crate::bolt_v3_submit_admission::BoltV3EconomicsSubmitAdmission> {
    let BoltV3FinalOrderEconomicsInput {
        execution_client_id,
        intent,
        order,
        valuation,
        risk_reducing_exit_position,
        scenario,
        candidate_fill_levels,
        requested_at_ns,
        decision_correlation_id,
    } = input;
    let submit_intent_kind = scenario.intent_kind();
    let request = crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequestInput {
        execution_client_id,
        intent,
        intent_kind: submit_intent_kind,
        order,
        valuation,
        risk_reducing_exit_position,
    };
    let facts = order_admission_facts(&request)?;
    anyhow::ensure!(
        economics.economics.execution_client_id() == execution_client_id,
        "economics execution client does not match the final order route"
    );
    let liquidity_role = if order.is_post_only() {
        NautilusEstimateLiquidityRole::GuaranteedMaker
    } else {
        NautilusEstimateLiquidityRole::Taker
    };
    let authority = economics
        .economics
        .request_authority(&order.instrument_id().to_string())?;
    let basis = seal_final_order_economics_basis(
        order,
        request.valuation.instrument,
        facts,
        &scenario,
        candidate_fill_levels,
    )?;
    let position = if authority.carry_required {
        Some(basis.position().ok_or_else(|| {
            anyhow::anyhow!("carry economics requires a position and holding horizon")
        })?)
    } else {
        None
    };
    let economics_request = economics_request_from_nautilus(NautilusEconomicsIntent {
        execution_client_id: &authority.execution_client_id,
        account_id: authority.account_id.as_str(),
        instrument_id: order.instrument_id(),
        product_surface_id: authority.product_surface_id.as_str(),
        reporting_policy_id: authority.reporting_policy_id.as_str(),
        reporting_currency: authority.reporting_currency.as_str(),
        edge_basis_policy_id: authority.edge_basis_policy_id.as_str(),
        decision_correlation_id,
        side: order.order_side(),
        liquidity_role,
        planned_fill_legs: basis.normalized_fill_legs(),
        routing_attachment_id: None,
        position,
        lifecycle_path: basis.lifecycle_path(),
        requested_at_ns,
    })
    .map_err(|error| anyhow::anyhow!(error))?;
    anyhow::ensure!(
        PlannedFillNotional::from_legs(&economics_request.planned_fill_legs)?
            == basis.planned_fill_notional(),
        "sealed planned-fill notional diverged from the provider request"
    );
    let admission = economics
        .economics
        .quote_admission(EconomicsAdmissionIntent::new(
            economics_request,
            basis.order_binding().clone(),
            basis.policy(),
            basis.gross_expected_value(),
            basis.reservation_basis(),
        ))
        .map_err(|error| {
            anyhow::anyhow!(
                "final-order economics quote failed at requested_at_ns={requested_at_ns}: {error}"
            )
        })?;
    build_submit_admission_request_from_economics(
        request,
        admission,
        economics.economics.resting_order_refresh_margin_ns()?,
    )
}

fn maker_terminal_disposition(order: &OrderAny) -> Result<MakerQuoteTerminalDisposition> {
    let disposition = match order.status() {
        OrderStatus::Denied => MakerQuoteTerminalDisposition::Denied,
        OrderStatus::Rejected => MakerQuoteTerminalDisposition::Rejected,
        OrderStatus::Canceled => MakerQuoteTerminalDisposition::Canceled,
        OrderStatus::Expired => MakerQuoteTerminalDisposition::Expired,
        OrderStatus::Filled => MakerQuoteTerminalDisposition::Filled,
        OrderStatus::Voided => MakerQuoteTerminalDisposition::Voided,
        OrderStatus::Initialized
        | OrderStatus::Emulated
        | OrderStatus::Released
        | OrderStatus::Submitted
        | OrderStatus::Accepted
        | OrderStatus::Triggered
        | OrderStatus::PendingUpdate
        | OrderStatus::PendingCancel
        | OrderStatus::PartiallyFilled => {
            anyhow::ensure!(
                order.leaves_qty().as_decimal() == Decimal::ZERO,
                "tracked maker terminal settlement requires a terminal status or zero leaves"
            );
            MakerQuoteTerminalDisposition::Filled
        }
    };
    Ok(disposition)
}

fn apply_retention_horizon(
    registry: &Arc<RwLock<TrackedMakerOrderRegistry>>,
    capability: RetentionHorizonCapability<'_>,
) -> Result<usize> {
    let (retained, operation) = {
        let mut registry = registry
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let selected = match capability {
            RetentionHorizonCapability::RegistrationEpochFinal {
                lifecycle,
                registration_generation,
                current_client_order_id,
            } => registry
                .retained_terminal_orders
                .iter()
                .filter_map(|(client_order_id, authority)| {
                    (authority.shares_lifecycle_scope_with(lifecycle)
                        && authority.registration_generation < registration_generation
                        && *client_order_id != current_client_order_id)
                        .then_some(*client_order_id)
                })
                .collect::<Vec<_>>(),
            RetentionHorizonCapability::ScopeClosure { lifecycles, now_ns } => {
                for record in registry.records.values_mut() {
                    let Some(governed) = record.governed() else {
                        continue;
                    };
                    if !lifecycles.iter().any(|lifecycle| {
                        governed
                            .maker_lifecycle
                            .shares_lifecycle_scope_with(lifecycle)
                    }) {
                        continue;
                    }
                    let quote_deadline_ns = governed
                        .economics
                        .as_ref()
                        .map(|economics| economics.admission.quote().valid_until_ns())
                        .unwrap_or(now_ns);
                    record.cancellation.request_intent(quote_deadline_ns);
                }
                registry
                    .retained_terminal_orders
                    .iter()
                    .filter_map(|(client_order_id, authority)| {
                        lifecycles
                            .iter()
                            .any(|lifecycle| authority.shares_lifecycle_scope_with(lifecycle))
                            .then_some(*client_order_id)
                    })
                    .collect::<Vec<_>>()
            }
            RetentionHorizonCapability::ComponentStop { drain_generation } => {
                anyhow::ensure!(
                    registry.lifecycle
                        == RestingRegistryLifecycle::Draining {
                            generation: drain_generation,
                        },
                    "resting retention horizon requires the exact component drain capability"
                );
                anyhow::ensure!(
                    registry.records.is_empty(),
                    "resting retention horizon cannot finalize while active tracked records exist"
                );
                registry.lifecycle = RestingRegistryLifecycle::Stopped {
                    generation: drain_generation,
                };
                registry
                    .retained_terminal_orders
                    .keys()
                    .copied()
                    .collect::<Vec<_>>()
            }
        };
        let operation = match capability {
            RetentionHorizonCapability::RegistrationEpochFinal { .. } => "registration-epoch-final",
            RetentionHorizonCapability::ScopeClosure { .. } => "scope-closure",
            RetentionHorizonCapability::ComponentStop { .. } => "component-stop",
        };
        let retained = selected
            .into_iter()
            .filter_map(|client_order_id| {
                registry.retained_terminal_orders.remove(&client_order_id)
            })
            .collect::<Vec<_>>();
        (retained, operation)
    };

    let count = retained.len();
    let mut failures: Vec<String> = Vec::new();
    for authority in retained {
        let outcome = authority
            .lifecycle
            .refine(authority.retention_horizon_event());
        if let Err(error) = consume_lifecycle_refinement_outcome(outcome, operation) {
            failures.push(error.to_string());
        }
    }
    if !failures.is_empty() {
        match registry.write() {
            Ok(mut registry) => registry.health = RestingRegistryHealth::Poisoned,
            Err(poisoned) => poisoned.into_inner().health = RestingRegistryHealth::Poisoned,
        }
        anyhow::bail!(
            "tracked maker retention horizon failed: {}",
            failures.join(" | ")
        );
    }
    Ok(count)
}

fn settle_maker_terminal(
    registry: &Arc<RwLock<TrackedMakerOrderRegistry>>,
    client_order_id: ClientOrderId,
    record: Option<TrackedMakerOrderRecord>,
    disposition: MakerQuoteTerminalDisposition,
) -> Result<()> {
    let Some(record) = record else {
        return Ok(());
    };
    let Some(governed) = record.into_governed() else {
        return Ok(());
    };
    settle_maker_terminal_authority(
        registry,
        client_order_id,
        governed.maker_lifecycle,
        disposition,
    )
}

fn settle_maker_terminal_authority(
    registry_state: &Arc<RwLock<TrackedMakerOrderRegistry>>,
    client_order_id: ClientOrderId,
    mut authority: MakerQuoteOrderAuthority,
    disposition: MakerQuoteTerminalDisposition,
) -> Result<()> {
    anyhow::ensure!(
        authority.client_order_id == client_order_id,
        "maker quote lifecycle association does not match the terminal order"
    );
    let event = authority.terminal_event(disposition);
    let outcome = authority.lifecycle.refine(event);
    let result = consume_lifecycle_refinement_outcome(outcome, "terminal");
    let mut registry = registry_state
        .write()
        .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
    let scope_is_closing = authority.lifecycle.retention_scope_is_closed();
    if scope_is_closing {
        drop(registry);
        result?;
        let lifecycle = [authority.lifecycle.clone()];
        let mut registry = registry_state
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        registry
            .retained_terminal_orders
            .insert(client_order_id, authority);
        drop(registry);
        apply_retention_horizon(
            registry_state,
            RetentionHorizonCapability::ScopeClosure {
                lifecycles: &lifecycle,
                now_ns: 0,
            },
        )?;
        return Ok(());
    }
    if authority.can_refine() {
        registry
            .retained_terminal_orders
            .insert(client_order_id, authority);
    } else {
        registry.retained_terminal_orders.remove(&client_order_id);
    }
    drop(registry);
    result
}

fn consume_lifecycle_refinement_outcome(
    outcome: MakerQuoteLifecycleRefinementOutcome,
    operation: &'static str,
) -> Result<()> {
    match outcome {
        MakerQuoteLifecycleRefinementOutcome::Applied => Ok(()),
        MakerQuoteLifecycleRefinementOutcome::Unaffected { event, active } => {
            log::info!(
                "maker quote per-order refinement left current leg occupancy unchanged: operation={operation} client_order_id={} generation={} active_client_order_id={:?} active_generation={:?}",
                event.client_order_id(),
                event.generation(),
                active
                    .as_ref()
                    .map(MakerQuoteLifecycleIdentity::client_order_id),
                active.as_ref().map(MakerQuoteLifecycleIdentity::generation),
            );
            Ok(())
        }
        MakerQuoteLifecycleRefinementOutcome::Invalid { event, active } => {
            anyhow::bail!(
                "maker quote {operation} consequence rejected by lifecycle reducer: client_order_id={} generation={} active_client_order_id={:?} active_generation={:?}",
                event.client_order_id(),
                event.generation(),
                active
                    .as_ref()
                    .map(MakerQuoteLifecycleIdentity::client_order_id),
                active.as_ref().map(MakerQuoteLifecycleIdentity::generation),
            )
        }
    }
}

pub(super) fn drive_observed_resting_order_economics<S>(
    order_economics: &BoltV3OrderEconomicsHandle,
    policy: BoltV3OrderExecutionPolicy,
    sink: &mut S,
    execution_client_id: &str,
    observations: Vec<(ClientOrderId, Option<OrderAny>)>,
    now_ns: u64,
) -> Result<()>
where
    S: BoltV3NtVenueMutationSink + ?Sized,
{
    let mut failures = Vec::new();
    for (client_order_id, cached) in observations {
        if let Err(error) =
            order_economics.refresh_tracked_economics(client_order_id, cached.as_ref(), now_ns)
        {
            failures.push(error.to_string());
            continue;
        }
        if let Err(error) = order_economics.drive_cancel_intent(
            policy,
            sink,
            CancelDriveInput {
                execution_client_id,
                client_order_id,
                cached: cached.as_ref(),
                now_ns,
                command_participant: None,
            },
        ) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "tracked maker cancellation reconciliation failed: {}",
            failures.join(" | ")
        )
    }
}

pub(super) fn route_tracked_cancel_all<S>(
    order_economics: &BoltV3OrderEconomicsHandle,
    policy: BoltV3OrderExecutionPolicy,
    sink: &mut S,
    execution_client_id: &str,
    instrument_id: InstrumentId,
    order_side: Option<OrderSide>,
) -> Result<()>
where
    S: BoltV3NtVenueMutationSink + ?Sized,
{
    if !policy.allows_venue_mutation() {
        log::info!(
            "tracked maker cancellation scope skipped by execution policy: mode=shadow execution_client_id={execution_client_id} instrument_id={instrument_id} order_side={order_side:?}"
        );
        return Ok(());
    }
    let now_ns = sink.actor_time_ns()?;
    let selected = order_economics.request_cancel_scope(instrument_id, order_side, now_ns)?;
    let mut observations = Vec::with_capacity(selected.len());
    let mut failures = Vec::new();
    for client_order_id in selected {
        match sink.cached_order(client_order_id) {
            Ok(cached) => observations.push((client_order_id, cached)),
            Err(error) => failures.push(error.to_string()),
        }
    }
    if let Err(error) = drive_observed_resting_order_economics(
        order_economics,
        policy,
        sink,
        execution_client_id,
        observations,
        now_ns,
    ) {
        failures.push(error.to_string());
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("tracked maker cancel-all failed: {}", failures.join(" | "))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{Arc, Mutex, RwLock},
    };

    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{OrderSide, TimeInForce},
        identifiers::{AccountId, ClientOrderId, InstrumentId, StrategyId, TraderId, VenueOrderId},
        orders::{LimitOrder, Order, OrderAny, stubs::TestOrderEventStubs},
        types::{Price, Quantity},
    };
    use rust_decimal::Decimal;

    use super::{
        BoltV3FinalOrderEconomicsInput, BoltV3FinalOrderEconomicsScenario,
        BoltV3RestingRegistrationCapability, BoltV3RestingRegistrationCommitParticipant,
        BoltV3RestingRegistrationRejectionKind, BoltV3RestingRollbackInvariantFailure,
        BoltV3RestingSubmitTransactionOutcome, BoltV3RouteAttemptCompletion,
        BoltV3RouteAttemptParticipant, BoltV3SubmitAttemptKind, BoltV3SubmitAttemptOutcome,
        MakerQuoteLifecycleIdentity, RestingRegistryHealth, TrackedMakerOrderRegistry,
        build_order_economics_submit_admission,
    };

    #[derive(Debug)]
    struct TestRegistrationParticipant {
        capability: Cell<BoltV3RestingRegistrationCapability>,
        lifecycle: MakerQuoteLifecycleHandle,
    }

    impl TestRegistrationParticipant {
        fn settle(&self) {
            self.capability
                .set(BoltV3RestingRegistrationCapability::Settled);
        }
    }

    impl BoltV3RestingRegistrationCommitParticipant for TestRegistrationParticipant {
        fn requote_budget(&self) -> Option<crate::bolt_v3_requote_budget::RequoteBudgetPair> {
            None
        }

        fn maker_lifecycle(&self) -> MakerQuoteLifecycleHandle {
            self.lifecycle.clone()
        }

        fn arm_at_identity(
            &mut self,
            _identity: MakerQuoteLifecycleIdentity,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn mark_sink_invoked(&mut self, _actor_now_ns: u64) -> anyhow::Result<()> {
            self.capability
                .set(BoltV3RestingRegistrationCapability::SinkInvoked);
            Ok(())
        }

        fn registration_capability(&self, _generation: u64) -> BoltV3RestingRegistrationCapability {
            self.capability.get()
        }

        fn settle_submitted(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }

        fn settle_command_issued(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }

        fn settle_sink_rejected(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }

        fn settle_callback_retired(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }

        fn abort_pre_sink(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }

        fn fail_pre_sink_invariant(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }

        fn fail_post_sink_invariant(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle();
            Ok(())
        }
    }

    pub(super) fn test_participant() -> Box<dyn BoltV3RestingRegistrationCommitParticipant> {
        Box::new(TestRegistrationParticipant {
            capability: Cell::new(BoltV3RestingRegistrationCapability::PreSink),
            lifecycle: MakerQuoteLifecycleHandle::new(MarketQuote::new_for_test(false), Leg::Yes),
        })
    }

    #[derive(Debug)]
    struct ReentrantSettlementParticipant {
        registry: Arc<RwLock<TrackedMakerOrderRegistry>>,
        settled_without_registry_lock: Arc<Mutex<bool>>,
        capability: Cell<BoltV3RestingRegistrationCapability>,
        lifecycle: MakerQuoteLifecycleHandle,
    }

    impl ReentrantSettlementParticipant {
        fn settle(&self) -> anyhow::Result<()> {
            let unlocked = self.registry.try_read().is_ok();
            *self
                .settled_without_registry_lock
                .lock()
                .expect("settlement observation should lock") = unlocked;
            anyhow::ensure!(
                unlocked,
                "participant settled while the registry lock was held"
            );
            self.capability
                .set(BoltV3RestingRegistrationCapability::Settled);
            Ok(())
        }
    }

    impl BoltV3RestingRegistrationCommitParticipant for ReentrantSettlementParticipant {
        fn requote_budget(&self) -> Option<crate::bolt_v3_requote_budget::RequoteBudgetPair> {
            None
        }

        fn maker_lifecycle(&self) -> MakerQuoteLifecycleHandle {
            self.lifecycle.clone()
        }

        fn arm_at_identity(
            &mut self,
            _identity: MakerQuoteLifecycleIdentity,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn mark_sink_invoked(&mut self, _actor_now_ns: u64) -> anyhow::Result<()> {
            self.capability
                .set(BoltV3RestingRegistrationCapability::SinkInvoked);
            Ok(())
        }

        fn registration_capability(&self, _generation: u64) -> BoltV3RestingRegistrationCapability {
            self.capability.get()
        }

        fn settle_submitted(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }

        fn settle_command_issued(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }

        fn settle_sink_rejected(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }

        fn settle_callback_retired(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }

        fn abort_pre_sink(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }

        fn fail_pre_sink_invariant(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }

        fn fail_post_sink_invariant(&mut self, _generation: u64) -> anyhow::Result<()> {
            self.settle()
        }
    }

    fn finish_route_attempt(
        mut participant: Box<dyn BoltV3RouteAttemptParticipant>,
        outcome: BoltV3SubmitAttemptOutcome,
    ) -> BoltV3SubmitAttemptOutcome {
        match outcome.kind() {
            BoltV3SubmitAttemptKind::Submitted | BoltV3SubmitAttemptKind::SinkRejected => {
                participant
                    .consume_at_pre_sink()
                    .expect("test participant should arm");
                participant
                    .mark_sink_invoked(0)
                    .expect("test participant should reach the sink");
                let completion = match outcome.kind() {
                    BoltV3SubmitAttemptKind::Submitted => BoltV3RouteAttemptCompletion::Submitted,
                    BoltV3SubmitAttemptKind::SinkRejected => {
                        BoltV3RouteAttemptCompletion::SinkRejected
                    }
                    _ => unreachable!(),
                };
                participant.complete(completion);
            }
            BoltV3SubmitAttemptKind::RouteValidationRejected
            | BoltV3SubmitAttemptKind::IntentEvidenceRejected
            | BoltV3SubmitAttemptKind::AdmissionRejected
            | BoltV3SubmitAttemptKind::PolicySkipped
            | BoltV3SubmitAttemptKind::PreSinkRejected => {}
        }
        outcome
    }
    use crate::{
        bolt_v3_maker_order_dispatch::{
            MakerQuoteTransactionContext, maker_quote_transaction_participant_for_test,
        },
        bolt_v3_maker_quote_control::{QuoteControlInput, drive_quote_leg},
        bolt_v3_order_execution::{
            BoltV3PlannedFillLeg, BoltV3TerminalValueEntry, BoltV3TerminalValueEntryPolicy,
            order_intent_details_from_compiled_order,
        },
        bolt_v3_quote_lifecycle::{Leg, LegEvent, MakerQuoteLifecycleHandle, MarketQuote},
        bolt_v3_requote_budget::{RequoteBudget, RequoteBudgetPair},
        bolt_v3_submit_admission::OrderValuationContext,
    };

    pub(super) fn maker_submit_participant(
        market: &MarketQuote,
        budget: &RequoteBudgetPair,
        leg: Leg,
    ) -> Box<dyn BoltV3RestingRegistrationCommitParticipant> {
        let mut market_handle = market.clone();
        let mut budget_handle = budget.clone();
        let decision = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg,
                desired_price: 0.5,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: 1_000,
            },
        );
        maker_quote_transaction_participant_for_test(MakerQuoteTransactionContext::new(
            market.clone(),
            budget.clone(),
            decision.proposal.expect("maker submit must be proposed"),
        ))
    }

    #[test]
    fn budget_denial_keeps_registry_healthy_for_a_subsequent_registration() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let market = MarketQuote::new_for_test(false);
        let budget = RequoteBudgetPair::new(
            RequoteBudget::new(1, 60_000, 0),
            RequoteBudget::new(1, 60_000, 0),
        );
        let first_participant = maker_submit_participant(&market, &budget, Leg::Yes);
        let denied_participant = maker_submit_participant(&market, &budget, Leg::No);

        let first_order = post_only_limit_order("MAKER-BEFORE-BUDGET-DENIAL");
        let first = economics.route_resting_submit(
            first_order.clone(),
            sealed_admission(&economics, &first_order),
            first_participant,
            |participant| {
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            },
        );
        assert!(first.is_submitted());

        let denied_order = post_only_limit_order("MAKER-BUDGET-DENIED");
        let denied = economics.route_resting_submit(
            denied_order.clone(),
            sealed_admission(&economics, &denied_order),
            denied_participant,
            |mut participant| {
                participant
                    .consume_at_pre_sink()
                    .expect_err("the second proposal must exhaust the shared budget");
                BoltV3SubmitAttemptOutcome::rejected_for_test(
                    BoltV3SubmitAttemptKind::PreSinkRejected,
                    "budget denied",
                )
            },
        );
        assert!(matches!(
            denied,
            BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                if attempt.kind() == BoltV3SubmitAttemptKind::PreSinkRejected
        ));

        let next_market = MarketQuote::new_for_test(false);
        let next_budget = RequoteBudgetPair::new(
            RequoteBudget::new(1, 60_000, 0),
            RequoteBudget::new(1, 60_000, 0),
        );
        let next_order = post_only_limit_order("MAKER-AFTER-BUDGET-DENIAL");
        let next = economics.route_resting_submit(
            next_order.clone(),
            sealed_admission(&economics, &next_order),
            maker_submit_participant(&next_market, &next_budget, Leg::Yes),
            |participant| {
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            },
        );
        assert!(next.is_submitted());
    }

    #[test]
    fn resting_submit_releases_registry_before_a_reentrant_nt_callback() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-REENTRANT-SUBMIT");
        let client_order_id = order.client_order_id();
        let admission = sealed_admission(&economics, &order);
        let callback_order = order.clone();

        let outcome =
            economics.route_resting_submit(order, admission, test_participant(), |participant| {
                economics
                    .reconcile_tracked_order_at(client_order_id, Some(callback_order), 1)
                    .expect("the re-entrant callback should reconcile");
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            });

        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                if attempt.kind() == BoltV3SubmitAttemptKind::Submitted
        ));

        assert_eq!(
            economics.resting_order_ids().unwrap(),
            vec![client_order_id]
        );
    }

    #[test]
    fn synchronous_terminal_callback_is_associated_before_the_sink_returns() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let market = MarketQuote::new_for_test(false);
        let budget = RequoteBudgetPair::new(
            RequoteBudget::new(2, 60_000, 0),
            RequoteBudget::new(2, 60_000, 0),
        );
        let mut order = post_only_limit_order("MAKER-SYNC-TERMINAL");
        let accepted = TestOrderEventStubs::accepted(
            &order,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from("VENUE-SYNC-TERMINAL"),
        );
        order.apply(accepted).expect("accept should apply");
        let client_order_id = order.client_order_id();
        let callback_order = order.clone();
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            maker_submit_participant(&market, &budget, Leg::Yes),
            |mut participant| {
                participant
                    .consume_at_pre_sink()
                    .expect("maker transaction should arm");
                participant
                    .mark_sink_invoked(1)
                    .expect("maker transaction should enter the sink window");
                let mut terminal = callback_order;
                let canceled = TestOrderEventStubs::canceled(
                    &terminal,
                    AccountId::from("ACCOUNT-001"),
                    Some(VenueOrderId::from("VENUE-SYNC-TERMINAL")),
                );
                terminal.apply(canceled).expect("cancel should apply");
                economics
                    .reconcile_tracked_order_at(client_order_id, Some(terminal), 2)
                    .expect("synchronous terminal callback should settle economics");
                participant.complete(BoltV3RouteAttemptCompletion::Submitted);
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );

        assert!(outcome.is_submitted());
        let registry = economics
            .tracked_orders
            .read()
            .expect("registry should remain healthy");
        assert_eq!(registry.health, RestingRegistryHealth::Healthy);
        assert!(registry.records.is_empty());
        assert!(
            registry
                .retained_terminal_orders
                .contains_key(&client_order_id),
            "synchronous Canceled truth must remain refinable"
        );
        let authority = registry
            .retained_terminal_orders
            .get(&client_order_id)
            .expect("synchronous terminal truth must retain its exact authority");
        assert_eq!(authority.client_order_id, client_order_id);
        assert_eq!(
            authority.identity.client_order_id(),
            client_order_id.as_str(),
            "the retained lifecycle identity must name the registered order"
        );
        assert_eq!(
            authority.identity.generation(),
            authority.registration_generation,
            "association-at-birth must bind the lifecycle and registration generations"
        );
        drop(registry);
        assert_eq!(
            market.leg_state(Leg::Yes),
            crate::bolt_v3_quote_lifecycle::LegState::Idle
        );
    }

    #[test]
    fn repeated_terminal_quote_cycles_retain_only_the_latest_scope_epoch() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let mut market = MarketQuote::new_for_test(false);
        let budget = RequoteBudgetPair::new(
            RequoteBudget::new(16, 60_000, 0),
            RequoteBudget::new(16, 60_000, 0),
        );

        for cycle in 1..=4 {
            let order_id = format!("MAKER-CHURN-{cycle}");
            let venue_id = format!("VENUE-CHURN-{cycle}");
            let mut order = post_only_limit_order(&order_id);
            let client_order_id = order.client_order_id();
            let outcome = economics.route_resting_submit(
                order.clone(),
                sealed_admission(&economics, &order),
                maker_submit_participant(&market, &budget, Leg::Yes),
                |participant| {
                    finish_route_attempt(
                        participant,
                        BoltV3SubmitAttemptOutcome::submitted_for_test(),
                    )
                },
            );
            assert!(outcome.is_submitted());
            assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);

            let accepted = TestOrderEventStubs::accepted(
                &order,
                AccountId::from("ACCOUNT-001"),
                VenueOrderId::from(venue_id.as_str()),
            );
            order.apply(accepted).expect("accept should apply");
            let canceled = TestOrderEventStubs::canceled(
                &order,
                AccountId::from("ACCOUNT-001"),
                Some(VenueOrderId::from(venue_id.as_str())),
            );
            order.apply(canceled).expect("cancel should apply");
            economics
                .reconcile_tracked_order_at(client_order_id, Some(order), cycle)
                .expect("terminal quote cycle should reconcile");

            let registry = economics
                .tracked_orders
                .read()
                .expect("registry should remain healthy");
            assert_eq!(
                registry.retained_terminal_orders.len(),
                1,
                "one active leg scope retains at most its latest refinable terminal epoch"
            );
            assert!(
                registry
                    .retained_terminal_orders
                    .contains_key(&client_order_id),
                "the latest terminal epoch remains refinable"
            );
        }
    }

    #[test]
    fn participant_settlement_runs_outside_the_registry_lock() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-REENTRANT-PARTICIPANT");
        let observed = Arc::new(Mutex::new(false));
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            Box::new(ReentrantSettlementParticipant {
                registry: economics.tracked_orders.clone(),
                settled_without_registry_lock: observed.clone(),
                capability: Cell::new(BoltV3RestingRegistrationCapability::PreSink),
                lifecycle: MakerQuoteLifecycleHandle::new(
                    MarketQuote::new_for_test(false),
                    Leg::Yes,
                ),
            }),
            |participant| {
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            },
        );
        assert!(outcome.is_submitted());
        assert!(*observed.lock().expect("settlement observation should lock"));
    }

    #[test]
    fn resting_registration_rejects_invalid_shape_and_quantity_before_routing() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let route_calls = Cell::new(0_u32);

        let shape_order = post_only_limit_order("MAKER-INVALID-SHAPE");
        let shape_admission =
            sealed_admission(&economics, &shape_order).with_planned_fill_legs_for_test(Vec::new());
        let shape = economics.route_resting_submit(
            shape_order,
            shape_admission,
            test_participant(),
            |_participant| {
                route_calls.set(route_calls.get() + 1);
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );
        assert!(matches!(
            shape,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::InvalidPlannedFillShape
        ));

        let quantity_order = post_only_limit_order("MAKER-NONPOSITIVE-QUANTITY");
        let quantity_admission = sealed_admission(&economics, &quantity_order)
            .with_planned_fill_legs_for_test(vec![crate::economics::PlannedFillLeg {
                price: Decimal::new(5, 1),
                quantity: Decimal::ZERO,
            }]);
        let quantity = economics.route_resting_submit(
            quantity_order,
            quantity_admission,
            test_participant(),
            |_participant| {
                route_calls.set(route_calls.get() + 1);
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );
        assert!(matches!(
            quantity,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::NonPositiveQuantity
        ));
        assert_eq!(route_calls.get(), 0);
        assert!(economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn resting_registration_rejects_duplicate_and_generation_overflow_before_routing() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-DUPLICATE");
        let first = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |participant| {
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            },
        );
        assert!(first.is_submitted());

        let route_calls = Cell::new(0_u32);
        let duplicate = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |_participant| {
                route_calls.set(route_calls.get() + 1);
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );
        assert!(matches!(
            duplicate,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::DuplicateClientOrderId
        ));
        assert_eq!(route_calls.get(), 0);

        let overflow =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        overflow
            .tracked_orders
            .write()
            .expect("registry should lock")
            .next_generation = u64::MAX;
        let overflow_order = post_only_limit_order("MAKER-GENERATION-OVERFLOW");
        let overflow_outcome = overflow.route_resting_submit(
            overflow_order.clone(),
            sealed_admission(&overflow, &overflow_order),
            test_participant(),
            |_participant| {
                route_calls.set(route_calls.get() + 1);
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );
        assert!(matches!(
            overflow_outcome,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::GenerationOverflow
        ));
        assert_eq!(route_calls.get(), 0);
    }

    #[test]
    fn resting_registration_rejects_initial_poison_before_routing() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let registry = economics.tracked_orders.clone();
        let poisoned = catch_unwind(AssertUnwindSafe(move || {
            let _guard = registry.write().expect("registry should initially lock");
            panic!("poison registry for the behavior test");
        }));
        assert!(poisoned.is_err());

        let route_calls = Cell::new(0_u32);
        let order = post_only_limit_order("MAKER-POISONED-REGISTRY");
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |_participant| {
                route_calls.set(route_calls.get() + 1);
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );
        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::RegistryUnavailable
        ));
        assert_eq!(route_calls.get(), 0);
    }

    #[test]
    fn every_routed_non_submission_removes_only_its_provisional_generation() {
        let kinds = [
            BoltV3SubmitAttemptKind::RouteValidationRejected,
            BoltV3SubmitAttemptKind::IntentEvidenceRejected,
            BoltV3SubmitAttemptKind::AdmissionRejected,
            BoltV3SubmitAttemptKind::PolicySkipped,
            BoltV3SubmitAttemptKind::PreSinkRejected,
            BoltV3SubmitAttemptKind::SinkRejected,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            let economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
                "execution_client",
            );
            let order = post_only_limit_order(&format!("MAKER-NON-SUBMITTED-{index}"));
            let routed = match kind {
                BoltV3SubmitAttemptKind::PolicySkipped => {
                    BoltV3SubmitAttemptOutcome::policy_skipped()
                }
                BoltV3SubmitAttemptKind::RouteValidationRejected
                | BoltV3SubmitAttemptKind::IntentEvidenceRejected
                | BoltV3SubmitAttemptKind::AdmissionRejected
                | BoltV3SubmitAttemptKind::PreSinkRejected
                | BoltV3SubmitAttemptKind::SinkRejected => {
                    BoltV3SubmitAttemptOutcome::rejected_for_test(kind, "typed rejection")
                }
                BoltV3SubmitAttemptKind::Submitted => unreachable!(),
            };
            let outcome = economics.route_resting_submit(
                order.clone(),
                sealed_admission(&economics, &order),
                test_participant(),
                |participant| finish_route_attempt(participant, routed),
            );
            assert!(matches!(
                outcome,
                BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                    if attempt.kind() == kind
            ));
            assert!(economics.resting_order_ids().unwrap().is_empty());
        }
    }

    #[test]
    fn callback_retirement_is_authoritative_during_non_submitted_rollback() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-CALLBACK-RETIRED");
        let client_order_id = order.client_order_id();
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |_participant| {
                economics
                    .reconcile_tracked_order_at(client_order_id, None, 1)
                    .expect("terminal callback should retire the provisional generation");
                BoltV3SubmitAttemptOutcome::policy_skipped()
            },
        );
        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                if attempt.kind() == BoltV3SubmitAttemptKind::PolicySkipped
        ));
        assert!(economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn rollback_conflict_preserves_original_outcome_and_replacement_generation() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-ROLLBACK-CONFLICT");
        let client_order_id = order.client_order_id();
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |_participant| {
                let mut registry = economics
                    .tracked_orders
                    .write()
                    .expect("registry should lock");
                registry
                    .records
                    .get_mut(&client_order_id)
                    .expect("provisional generation should exist")
                    .governed_mut()
                    .expect("provisional generation should retain exact authority")
                    .registration_generation += 1;
                BoltV3SubmitAttemptOutcome::policy_skipped()
            },
        );
        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::RollbackInvariantFailed {
                original,
                reason: BoltV3RestingRollbackInvariantFailure::RegistrationGenerationReplaced,
            } if original.kind() == BoltV3SubmitAttemptKind::PolicySkipped
        ));
        let registry = economics
            .tracked_orders
            .read()
            .expect("registry should lock");
        assert!(registry.records.contains_key(&client_order_id));
    }

    #[test]
    fn drop_backstop_never_removes_a_replacement_generation() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-DROP-BACKSTOP-CONFLICT");
        let client_order_id = order.client_order_id();
        let transaction = economics
            .begin_resting_registration(
                order.clone(),
                sealed_admission(&economics, &order),
                test_participant(),
                Arc::new(Mutex::new(None)),
            )
            .expect("provisional registration should begin");
        let replacement_generation = transaction
            .generation
            .checked_add(1)
            .expect("test generation should advance");
        {
            let mut registry = economics
                .tracked_orders
                .write()
                .expect("registry should lock");
            registry
                .records
                .get_mut(&client_order_id)
                .expect("provisional record should exist")
                .governed_mut()
                .expect("provisional record should retain exact authority")
                .registration_generation = replacement_generation;
            registry
                .retired_provisional
                .insert(client_order_id, replacement_generation);
        }

        drop(transaction);

        let registry = economics
            .tracked_orders
            .read()
            .expect("registry should lock");
        assert_eq!(
            registry
                .records
                .get(&client_order_id)
                .and_then(|record| record.governed())
                .map(|authority| authority.registration_generation),
            Some(replacement_generation)
        );
        assert_eq!(
            registry.retired_provisional.get(&client_order_id).copied(),
            Some(replacement_generation)
        );
        assert_eq!(registry.health, RestingRegistryHealth::Poisoned);
    }

    #[test]
    fn submitted_commit_conflict_poisoning_prevents_a_second_registration() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-COMMIT-CONFLICT");
        let client_order_id = order.client_order_id();
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |participant| {
                economics
                    .tracked_orders
                    .write()
                    .expect("registry should lock")
                    .records
                    .get_mut(&client_order_id)
                    .expect("provisional generation should exist")
                    .governed_mut()
                    .expect("provisional generation should retain exact authority")
                    .registration_generation += 1;
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            },
        );
        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                if attempt.kind() == BoltV3SubmitAttemptKind::Submitted
        ));

        let next = post_only_limit_order("MAKER-AFTER-COMMIT-CONFLICT");
        let next_outcome = economics.route_resting_submit(
            next.clone(),
            sealed_admission(&economics, &next),
            test_participant(),
            |participant| {
                finish_route_attempt(
                    participant,
                    BoltV3SubmitAttemptOutcome::submitted_for_test(),
                )
            },
        );
        assert!(matches!(
            next_outcome,
            BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection)
                if rejection.kind()
                    == BoltV3RestingRegistrationRejectionKind::RegistryUnavailable
        ));
    }

    #[test]
    fn poisoned_rollback_removes_exact_generation_and_marks_registry_unhealthy() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-POISONED-ROLLBACK");
        let registry = economics.tracked_orders.clone();
        let outcome = economics.route_resting_submit(
            order.clone(),
            sealed_admission(&economics, &order),
            test_participant(),
            |_participant| {
                let poisoned = catch_unwind(AssertUnwindSafe(|| {
                    let _guard = registry.write().expect("registry should lock");
                    panic!("poison registry after provisional registration");
                }));
                assert!(poisoned.is_err());
                BoltV3SubmitAttemptOutcome::policy_skipped()
            },
        );
        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                if attempt.kind() == BoltV3SubmitAttemptKind::PolicySkipped
        ));
        let registry = economics
            .tracked_orders
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(registry.records.is_empty());
        assert_eq!(registry.health, RestingRegistryHealth::Poisoned);
    }

    pub(super) fn sealed_admission(
        economics: &crate::bolt_v3_order_execution::BoltV3OrderEconomicsHandle,
        order: &OrderAny,
    ) -> crate::bolt_v3_economics_runtime::EconomicsAdmission {
        let intent = order_intent_details_from_compiled_order(
            "strategy-a".to_string(),
            "0.50".to_string(),
            order,
        );
        build_order_economics_submit_admission(
            economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    BoltV3TerminalValueEntry::try_new(
                        Decimal::new(7, 1),
                        BoltV3TerminalValueEntryPolicy::Breakeven,
                    )
                    .expect("terminal value should construct"),
                ),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: Decimal::new(5, 1),
                    quantity: Decimal::ONE,
                }],
                requested_at_ns: 1,
                decision_correlation_id: "maker-registration-test",
            },
        )
        .expect("maker economics should seal")
        .economics()
        .clone()
    }

    pub(super) fn post_only_limit_order(client_order_id: &str) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("INSTRUMENT.SOURCE"),
                ClientOrderId::from(client_order_id),
                OrderSide::Buy,
                Quantity::new(1.0, 2),
                Price::new(0.50, 2),
                TimeInForce::Gtc,
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
                UnixNanos::from(1_u64),
            )
            .expect("post-only limit order should be valid"),
        )
    }
}
