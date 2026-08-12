use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use anyhow::Result;
use nautilus_common::actor::DataActorNative;
#[cfg(any(test, feature = "test-current-evidence-inspection"))]
use nautilus_model::types::{Price, Quantity};
use nautilus_model::{
    enums::{OrderSide, PositionSide as NtPositionSide},
    identifiers::{ClientOrderId, InstrumentId, PositionId},
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
    BoltV3OrderExecutionPolicy, BoltV3SubmitAttemptKind, BoltV3SubmitAttemptOutcome,
    BoltV3TakerEconomicsSizingInput, NtStrategyVenueMutationSink,
    economics_basis::seal_final_order_economics_basis,
};

mod cancel_coordinator;

use cancel_coordinator::TrackedOrderCancellation;
pub use cancel_coordinator::{
    BoltV3CancellationLivenessFailure, BoltV3RecoveryIdentityConflict,
    BoltV3RestingOrderCancelHealthSnapshot,
};

#[derive(Clone)]
pub struct BoltV3OrderEconomicsHandle {
    economics: BoundExecutionEconomics,
    tracked_orders: Arc<RwLock<TrackedMakerOrderRegistry>>,
}

#[derive(Debug, Default)]
struct TrackedMakerOrderRegistry {
    records: BTreeMap<ClientOrderId, TrackedMakerOrderRecord>,
    retired_provisional: BTreeMap<ClientOrderId, u64>,
    next_generation: u64,
    health: RestingRegistryHealth,
}

impl TrackedMakerOrderRegistry {
    fn allocate_generation(&mut self) -> Option<u64> {
        let generation = self.next_generation.checked_add(1)?;
        self.next_generation = generation;
        Some(generation)
    }

    fn remove_record(
        &mut self,
        client_order_id: &ClientOrderId,
    ) -> Option<TrackedMakerOrderRecord> {
        let record = self.records.remove(client_order_id)?;
        if record.registration_state == RestingRegistrationState::Provisional {
            self.retired_provisional
                .insert(*client_order_id, record.registration_generation);
        }
        Some(record)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RestingRegistryHealth {
    #[default]
    Healthy,
    Poisoned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RestingOrderEconomicsRecord {
    admission: EconomicsAdmission,
    authorized_quantity_ceiling: Decimal,
}

#[derive(Clone, Debug)]
struct TrackedMakerOrderRecord {
    registration_generation: u64,
    registration_state: RestingRegistrationState,
    economics: Option<RestingOrderEconomicsRecord>,
    cancellation: TrackedOrderCancellation,
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

struct RestingRegistrationTransaction {
    registry: Arc<RwLock<TrackedMakerOrderRegistry>>,
    client_order_id: ClientOrderId,
    generation: u64,
    state: RestingRegistrationTransactionState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestingRegistrationTransactionState {
    Active,
    Settled,
}

impl RestingRegistrationTransaction {
    fn commit(mut self) {
        let mut registry = match self.registry.write() {
            Ok(registry) => registry,
            Err(poisoned) => {
                let mut registry = poisoned.into_inner();
                registry.health = RestingRegistryHealth::Poisoned;
                registry
            }
        };
        let ownership = (
            registry
                .records
                .get(&self.client_order_id)
                .map(|record| record.registration_generation),
            registry
                .retired_provisional
                .get(&self.client_order_id)
                .copied(),
        );
        match ownership {
            (Some(generation), _) if generation == self.generation => {
                registry
                    .records
                    .get_mut(&self.client_order_id)
                    .expect("owned resting registration must remain present")
                    .registration_state = RestingRegistrationState::Committed;
            }
            (None, Some(generation)) if generation == self.generation => {
                // A synchronous authoritative terminal callback retired this exact
                // provisional generation before the NT submit call returned.
            }
            _ => {
                registry.health = RestingRegistryHealth::Poisoned;
                log::error!(
                    "resting registration commit lost generation ownership: client_order_id={} generation={}",
                    self.client_order_id,
                    self.generation
                );
            }
        }
        if registry
            .retired_provisional
            .get(&self.client_order_id)
            .is_some_and(|generation| *generation == self.generation)
        {
            registry.retired_provisional.remove(&self.client_order_id);
        }
        self.state = RestingRegistrationTransactionState::Settled;
    }

    fn abort(
        mut self,
        original: BoltV3RoutedNonSubmittedOutcome,
    ) -> BoltV3RestingSubmitTransactionOutcome {
        let (mut registry, lock_was_poisoned) = match self.registry.write() {
            Ok(registry) => (registry, false),
            Err(poisoned) => (poisoned.into_inner(), true),
        };
        if lock_was_poisoned {
            registry.health = RestingRegistryHealth::Poisoned;
        }

        let rollback = match registry.records.get(&self.client_order_id) {
            Some(record) if record.registration_generation == self.generation => {
                registry.records.remove(&self.client_order_id);
                registry.retired_provisional.remove(&self.client_order_id);
                Ok(())
            }
            Some(_) => Err(BoltV3RestingRollbackInvariantFailure::RegistrationGenerationReplaced),
            None if registry
                .retired_provisional
                .get(&self.client_order_id)
                .is_some_and(|generation| *generation == self.generation) =>
            {
                registry.retired_provisional.remove(&self.client_order_id);
                Ok(())
            }
            None => Err(BoltV3RestingRollbackInvariantFailure::RegistryUnavailable),
        };
        self.state = RestingRegistrationTransactionState::Settled;
        drop(registry);

        match rollback {
            Ok(()) => BoltV3RestingSubmitTransactionOutcome::Attempt(original.0),
            Err(reason) => {
                BoltV3RestingSubmitTransactionOutcome::RollbackInvariantFailed { original, reason }
            }
        }
    }
}

impl Drop for RestingRegistrationTransaction {
    fn drop(&mut self) {
        match self.state {
            RestingRegistrationTransactionState::Settled => return,
            RestingRegistrationTransactionState::Active => {}
        }
        let mut registry = match self.registry.write() {
            Ok(registry) => registry,
            Err(poisoned) => {
                let mut registry = poisoned.into_inner();
                registry.health = RestingRegistryHealth::Poisoned;
                registry
            }
        };
        let record_ownership = registry
            .records
            .get(&self.client_order_id)
            .map(|record| record.registration_generation);
        match record_ownership {
            Some(generation) if generation == self.generation => {
                registry.records.remove(&self.client_order_id);
            }
            Some(_) => registry.health = RestingRegistryHealth::Poisoned,
            None => {}
        }
        let retired_ownership = registry
            .retired_provisional
            .get(&self.client_order_id)
            .copied();
        match retired_ownership {
            Some(generation) if generation == self.generation => {
                registry.retired_provisional.remove(&self.client_order_id);
            }
            Some(_) => registry.health = RestingRegistryHealth::Poisoned,
            None => {}
        }
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
            .records
            .keys()
            .copied()
            .collect())
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
        let Some(record) = registry.records.get_mut(&client_order_id) else {
            return Ok(());
        };
        if cached.is_some_and(|order| {
            order.is_closed() || order.leaves_qty().as_decimal() == Decimal::ZERO
        }) {
            registry.remove_record(&client_order_id);
            return Ok(());
        }
        if record.cancellation.is_requested() {
            return Ok(());
        }
        let Some(economics) = record.economics.as_mut() else {
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
                registry.remove_record(&client_order_id);
            }
            RestingOrderEconomicsRefresh::Refreshed(admission) => {
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
        policy: BoltV3OrderExecutionPolicy,
        order: OrderAny,
        admission: EconomicsAdmission,
        route: F,
    ) -> BoltV3RestingSubmitTransactionOutcome
    where
        F: FnOnce() -> BoltV3SubmitAttemptOutcome,
    {
        if !policy.allows_venue_mutation() {
            return BoltV3RestingSubmitTransactionOutcome::Attempt(route());
        }
        let transaction = match self.begin_resting_registration(order, admission) {
            Ok(transaction) => transaction,
            Err(rejection) => {
                return BoltV3RestingSubmitTransactionOutcome::RegistrationRejected(rejection);
            }
        };
        let outcome = route();
        match BoltV3RoutedNonSubmittedOutcome::try_new(outcome) {
            Err(submitted) => {
                transaction.commit();
                BoltV3RestingSubmitTransactionOutcome::Attempt(submitted)
            }
            Ok(non_submitted) => transaction.abort(non_submitted),
        }
    }

    fn begin_resting_registration(
        &self,
        order: OrderAny,
        admission: EconomicsAdmission,
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
        if registry.health == RestingRegistryHealth::Poisoned {
            return Err(BoltV3RestingRegistrationRejection::new(
                BoltV3RestingRegistrationRejectionKind::RegistryUnavailable,
                "resting economics registry health is poisoned",
            ));
        }
        if registry.records.contains_key(&client_order_id) {
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
        registry.records.insert(
            client_order_id,
            TrackedMakerOrderRecord {
                registration_generation: generation,
                registration_state: RestingRegistrationState::Provisional,
                economics: Some(RestingOrderEconomicsRecord {
                    admission,
                    authorized_quantity_ceiling,
                }),
                cancellation: TrackedOrderCancellation::new(order),
            },
        );
        drop(registry);
        Ok(RestingRegistrationTransaction {
            registry: self.tracked_orders.clone(),
            client_order_id,
            generation,
            state: RestingRegistrationTransactionState::Active,
        })
    }

    pub(super) fn route_tracked_cancel<S>(
        &self,
        policy: BoltV3OrderExecutionPolicy,
        sink: &mut S,
        execution_client_id: &str,
        client_order_id: ClientOrderId,
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
        drive_observed_resting_order_economics(
            self,
            policy,
            sink,
            execution_client_id,
            vec![(client_order_id, cached)],
            now_ns,
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
            execution_client_id,
            client_order_id,
            cached.as_ref(),
            now_ns,
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
    };

    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{OrderSide, TimeInForce},
        identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
        orders::{LimitOrder, Order, OrderAny},
        types::{Price, Quantity},
    };
    use rust_decimal::Decimal;

    use super::{
        BoltV3FinalOrderEconomicsInput, BoltV3FinalOrderEconomicsScenario,
        BoltV3OrderExecutionPolicy, BoltV3RestingRegistrationRejectionKind,
        BoltV3RestingRollbackInvariantFailure, BoltV3RestingSubmitTransactionOutcome,
        BoltV3SubmitAttemptKind, BoltV3SubmitAttemptOutcome, RestingRegistryHealth,
        build_order_economics_submit_admission,
    };
    use crate::{
        bolt_v3_order_execution::{
            BoltV3PlannedFillLeg, BoltV3TerminalValueEntry,
            order_intent_details_from_compiled_order,
        },
        bolt_v3_submit_admission::OrderValuationContext,
    };

    #[test]
    fn resting_submit_releases_registry_before_a_reentrant_nt_callback() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("MAKER-REENTRANT-SUBMIT");
        let client_order_id = order.client_order_id();
        let admission = sealed_admission(&economics, &order);
        let callback_order = order.clone();

        let outcome = economics.route_resting_submit(
            BoltV3OrderExecutionPolicy::live(),
            order,
            admission,
            || {
                economics
                    .reconcile_tracked_order_at(client_order_id, Some(callback_order), 1)
                    .expect("the re-entrant callback should reconcile");
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );

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
    fn resting_registration_rejects_invalid_shape_and_quantity_before_routing() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let route_calls = Cell::new(0_u32);

        let shape_order = post_only_limit_order("MAKER-INVALID-SHAPE");
        let shape_admission =
            sealed_admission(&economics, &shape_order).with_planned_fill_legs_for_test(Vec::new());
        let shape = economics.route_resting_submit(
            BoltV3OrderExecutionPolicy::live(),
            shape_order,
            shape_admission,
            || {
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
            BoltV3OrderExecutionPolicy::live(),
            quantity_order,
            quantity_admission,
            || {
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
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            BoltV3SubmitAttemptOutcome::submitted_for_test,
        );
        assert!(first.is_submitted());

        let route_calls = Cell::new(0_u32);
        let duplicate = economics.route_resting_submit(
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            || {
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
            BoltV3OrderExecutionPolicy::live(),
            overflow_order.clone(),
            sealed_admission(&overflow, &overflow_order),
            || {
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
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            || {
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
                BoltV3OrderExecutionPolicy::live(),
                order.clone(),
                sealed_admission(&economics, &order),
                || routed,
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
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            || {
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
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            || {
                let mut registry = economics
                    .tracked_orders
                    .write()
                    .expect("registry should lock");
                registry
                    .records
                    .get_mut(&client_order_id)
                    .expect("provisional generation should exist")
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
            .begin_resting_registration(order.clone(), sealed_admission(&economics, &order))
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
                .map(|record| record.registration_generation),
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
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            || {
                economics
                    .tracked_orders
                    .write()
                    .expect("registry should lock")
                    .records
                    .get_mut(&client_order_id)
                    .expect("provisional generation should exist")
                    .registration_generation += 1;
                BoltV3SubmitAttemptOutcome::submitted_for_test()
            },
        );
        assert!(matches!(
            outcome,
            BoltV3RestingSubmitTransactionOutcome::Attempt(attempt)
                if attempt.kind() == BoltV3SubmitAttemptKind::Submitted
        ));

        let next = post_only_limit_order("MAKER-AFTER-COMMIT-CONFLICT");
        let next_outcome = economics.route_resting_submit(
            BoltV3OrderExecutionPolicy::live(),
            next.clone(),
            sealed_admission(&economics, &next),
            BoltV3SubmitAttemptOutcome::submitted_for_test,
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
            BoltV3OrderExecutionPolicy::live(),
            order.clone(),
            sealed_admission(&economics, &order),
            || {
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

    fn sealed_admission(
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
                    BoltV3TerminalValueEntry::try_new(Decimal::new(7, 1), Decimal::ZERO)
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

    fn post_only_limit_order(client_order_id: &str) -> OrderAny {
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
