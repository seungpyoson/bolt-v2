use std::{
    any::type_name,
    cell::RefMut,
    collections::BTreeMap,
    sync::{Arc, RwLock, RwLockWriteGuard},
};

use anyhow::Result;
use nautilus_common::actor::DataActorNative;
use nautilus_common::{
    factories::OrderFactory,
    messages::execution::{
        BatchCancelOrders, CancelAllOrders, CancelOrder, ModifyOrder, SubmitOrderList,
    },
};
use nautilus_core::Params;
use nautilus_model::{
    enums::{
        OrderSide, OrderStatus, OrderType, PositionSide as NtPositionSide, TimeInForce,
        TrailingOffsetType, TriggerType,
    },
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId},
    instruments::InstrumentAny,
    orders::{Order, OrderAny, OrderList},
    types::{Price, Quantity},
};
use nautilus_trading::{Strategy, StrategyNative};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::bolt_v3_economics_runtime::EconomicsAdmissionPurpose;
use crate::{
    bolt_v3_capital_admission::ProductAdmissionSnapshot,
    bolt_v3_current_evidence::{
        EntryOrderIntentFact, EvidenceOrderSide, EvidenceOrderType, EvidenceTimeInForce,
        EvidenceTrailingOffsetType, EvidenceTriggerType, NonBlockingRecordOutcome,
        OrderExecutionEvidence, OrderIntentClampNotEvaluatedReason, OrderIntentClampOutcome,
        OrderIntentDetails, OrderIntentOrderFields, RecordFailure, RiskReducingExitOrderIntentFact,
    },
    bolt_v3_economics_runtime::{
        BoundExecutionEconomics, EconomicsAdmission, EconomicsAdmissionIntent,
        EconomicsAdmissionPolicy, EconomicsSizingIntent, EconomicsSizingQuote,
        RestingOrderEconomicsRefresh, refresh_resting_order_economics,
    },
    bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand,
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        dispatch_maker_order_command,
    },
    bolt_v3_order_intent::{NtOrderBuildInputs, build_nt_order},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_submit_admission::{
        BoltV3EconomicsSubmitAdmission, BoltV3RiskReducingExitPositionInput,
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest,
        BoltV3SubmitAdmissionRequestInput, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        OrderValuationContext, build_submit_admission_request_from_economics,
        order_admission_facts, validate_economics_remaining_margin_at,
        validate_economics_submit_authority,
    },
    economics::{LifecyclePath, PlannedFillNotional, PositionContext},
    integrations::nautilus::economics::{
        NautilusEconomicsIntent, NautilusEstimateLiquidityRole, NautilusPlannedFillLeg,
        economics_order_binding, economics_request_from_nautilus,
    },
};

mod cancel_coordinator;
mod economics_basis;

pub use cancel_coordinator::{
    BoltV3CancellationLivenessFailure, BoltV3RecoveryIdentityConflict,
    BoltV3RestingOrderCancelHealthSnapshot,
};
use cancel_coordinator::{
    CancelOperationKind, CancelTransition, NtOrderQuerySeed, RestingOrderCancelRecord,
};
use economics_basis::seal_final_order_economics_basis;
pub use economics_basis::{BoltV3FinalOrderEconomicsScenario, BoltV3TerminalValueEntry};

#[derive(Clone)]
pub struct BoltV3OrderEconomicsHandle {
    economics: BoundExecutionEconomics,
    tracked_orders: Arc<RwLock<BTreeMap<ClientOrderId, TrackedMakerOrderRecord>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RestingOrderEconomicsRecord {
    admission: EconomicsAdmission,
    authorized_quantity_ceiling: Decimal,
}

#[derive(Clone, Debug)]
struct TrackedMakerOrderRecord {
    economics: Option<RestingOrderEconomicsRecord>,
    query_seed: NtOrderQuerySeed,
    cancellation: Option<RestingOrderCancelRecord>,
}

struct RestingOrderRegistrationGuard<'a> {
    records: RwLockWriteGuard<'a, BTreeMap<ClientOrderId, TrackedMakerOrderRecord>>,
    client_order_id: ClientOrderId,
    committed: bool,
}

impl RestingOrderRegistrationGuard<'_> {
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for RestingOrderRegistrationGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.records.remove(&self.client_order_id);
        }
    }
}

pub struct BoltV3FinalOrderEconomicsInput<'a> {
    pub execution_client_id: &'a str,
    pub intent: &'a OrderIntentDetails,
    pub order: &'a OrderAny,
    pub valuation: OrderValuationContext<'a>,
    pub risk_reducing_exit_position: Option<BoltV3RiskReducingExitPositionInput<'a>>,
    pub scenario: BoltV3FinalOrderEconomicsScenario,
    pub candidate_fill_levels: Vec<BoltV3PlannedFillLeg>,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}

pub struct BoltV3TakerEconomicsSizingInput<'a> {
    pub instrument_id: InstrumentId,
    pub order_side: OrderSide,
    pub planned_fill_legs: Vec<BoltV3PlannedFillLeg>,
    pub terminal_value_entry: BoltV3TerminalValueEntry,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoltV3PlannedFillLeg {
    pub price: Decimal,
    pub quantity: Decimal,
}

impl BoltV3OrderEconomicsHandle {
    pub fn new(economics: BoundExecutionEconomics) -> Self {
        Self {
            economics,
            tracked_orders: Arc::new(RwLock::new(BTreeMap::new())),
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

    pub fn drive_resting_order_economics_at_ms<S>(
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
        drive_resting_order_economics(
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
            .keys()
            .copied()
            .collect())
    }

    pub fn begin_resting_order_drain_at_ns(&self, now_ns: u64) -> Result<usize> {
        let client_order_ids = self.resting_order_ids()?;
        for client_order_id in &client_order_ids {
            self.request_cancel_intent(*client_order_id, now_ns)?;
        }
        Ok(client_order_ids.len())
    }

    fn request_cancel_intent(&self, client_order_id: ClientOrderId, now_ns: u64) -> Result<bool> {
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

    fn request_cancel_scope(
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

    fn refresh_tracked_economics(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<&OrderAny>,
        now_ns: u64,
    ) -> Result<()> {
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(&client_order_id) else {
            return Ok(());
        };
        if cached.is_some_and(|order| {
            order.is_closed() || order.leaves_qty().as_decimal() == Decimal::ZERO
        }) {
            records.remove(&client_order_id);
            return Ok(());
        }
        if record.cancellation.is_some() {
            return Ok(());
        }
        let Some(economics) = record.economics.as_mut() else {
            return Ok(());
        };
        let Some(order) = cached else {
            let quote_deadline_ns = economics.admission.quote().valid_until_ns();
            record.cancellation = Some(RestingOrderCancelRecord::new(quote_deadline_ns));
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
                records.remove(&client_order_id);
            }
            RestingOrderEconomicsRefresh::Refreshed(admission) => {
                economics.admission = *admission;
            }
            RestingOrderEconomicsRefresh::CancelRequired(reason) => {
                log::warn!(
                    "resting order economics requires cancellation: client_order_id={client_order_id} reason={reason:?}"
                );
                let quote_deadline_ns = economics.admission.quote().valid_until_ns();
                record.cancellation = Some(RestingOrderCancelRecord::new(quote_deadline_ns));
            }
        }
        Ok(())
    }

    fn prepare_resting_order_registration(
        &self,
        order: OrderAny,
        admission: EconomicsAdmission,
    ) -> Result<RestingOrderRegistrationGuard<'_>> {
        let client_order_id = order.client_order_id();
        let [leg] = admission.request().planned_fill_legs.as_slice() else {
            anyhow::bail!("resting economics registration requires exactly one planned fill leg");
        };
        anyhow::ensure!(
            leg.quantity > Decimal::ZERO,
            "resting economics registration requires positive quantity"
        );
        let authorized_quantity_ceiling = leg.quantity;
        let mut records = self
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("resting economics state lock poisoned"))?;
        anyhow::ensure!(
            !records.contains_key(&client_order_id),
            "resting economics registration rejected duplicate client order id: {client_order_id}"
        );
        records.insert(
            client_order_id,
            TrackedMakerOrderRecord {
                economics: Some(RestingOrderEconomicsRecord {
                    admission,
                    authorized_quantity_ceiling,
                }),
                query_seed: NtOrderQuerySeed::new(order),
                cancellation: None,
            },
        );
        Ok(RestingOrderRegistrationGuard {
            records,
            client_order_id,
            committed: false,
        })
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
        self.reconcile_tracked_order_inner(client_order_id, cached, now_ns, false)
    }

    pub fn reconcile_fill_void_at(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<OrderAny>,
        now_ns: u64,
    ) -> Result<()> {
        self.reconcile_tracked_order_inner(client_order_id, cached, now_ns, true)
    }

    fn reconcile_tracked_order_inner(
        &self,
        client_order_id: ClientOrderId,
        cached: Option<OrderAny>,
        now_ns: u64,
        fill_void: bool,
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
            if !fill_void || order.is_closed() || order.leaves_qty().as_decimal() == Decimal::ZERO {
                return Ok(());
            }
            let recovery_deadline_ns = now_ns
                .checked_add(retry_timeout_ns)
                .ok_or_else(|| anyhow::anyhow!("fill-void cancellation deadline overflow"))?;
            entry.insert(TrackedMakerOrderRecord {
                economics: None,
                query_seed: NtOrderQuerySeed::new(order.clone()),
                cancellation: Some(RestingOrderCancelRecord::new(recovery_deadline_ns)),
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
        match cancellation.reconcile_callback(
            &mut record.query_seed,
            cached.as_ref(),
            now_ns,
            retry_timeout_ns,
        )? {
            CancelTransition::Remove => {
                records.remove(&client_order_id);
            }
            CancelTransition::NoOperation | CancelTransition::Begin(_) => {}
        }
        Ok(())
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
            .quote_sizing(EconomicsSizingIntent {
                request,
                policy: EconomicsAdmissionPolicy::TradingEdge {
                    minimum_core_edge_ratio: intent.terminal_value_entry.minimum_core_edge_ratio(),
                },
                gross_expected_value,
                reservation_basis,
            })
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
) -> Result<BoltV3EconomicsSubmitAdmission> {
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
    let request = BoltV3SubmitAdmissionRequestInput {
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
        .quote_admission(EconomicsAdmissionIntent {
            request: economics_request,
            order_binding: basis.order_binding().clone(),
            policy: basis.policy(),
            gross_expected_value: basis.gross_expected_value(),
            reservation_basis: basis.reservation_basis(),
        })
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

fn drive_resting_order_economics<S>(
    order_economics: &BoltV3OrderEconomicsHandle,
    policy: BoltV3OrderExecutionPolicy,
    sink: &mut S,
    execution_client_id: &str,
    mut observations: Vec<(ClientOrderId, Option<OrderAny>)>,
    now_ns: u64,
) -> Result<()>
where
    S: BoltV3NtVenueMutationSink + ?Sized,
{
    if observations.is_empty() {
        observations = order_economics
            .resting_order_ids()?
            .into_iter()
            .map(|client_order_id| {
                sink.cached_order(client_order_id)
                    .map(|order| (client_order_id, order))
            })
            .collect::<Result<Vec<_>>>()?;
    }
    let retry_timeout_ns = order_economics.economics.cancel_retry_timeout_ns()?;
    let escalation_attempts = order_economics
        .economics
        .cancel_recovery_escalation_attempts();
    let mut failures = Vec::new();
    for (client_order_id, cached) in observations {
        if let Err(error) =
            order_economics.refresh_tracked_economics(client_order_id, cached.as_ref(), now_ns)
        {
            failures.push(error.to_string());
            continue;
        }

        let planned = {
            let mut records = order_economics
                .tracked_orders
                .write()
                .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
            let Some(record) = records.get_mut(&client_order_id) else {
                continue;
            };
            let TrackedMakerOrderRecord {
                query_seed,
                cancellation,
                ..
            } = record;
            let Some(cancellation) = cancellation.as_mut() else {
                continue;
            };
            let result = if policy.allows_venue_mutation() {
                cancellation.plan_drive(
                    query_seed,
                    cached.as_ref(),
                    now_ns,
                    retry_timeout_ns,
                    escalation_attempts,
                )
            } else {
                cancellation
                    .reconcile_callback(query_seed, cached.as_ref(), now_ns, retry_timeout_ns)
                    .map(|transition| (transition, None))
            };
            match result {
                Ok((CancelTransition::Remove, _)) => {
                    records.remove(&client_order_id);
                    None
                }
                Ok((_, operation)) => operation.map(|operation| {
                    (
                        operation,
                        record.query_seed.as_query_order().clone(),
                        cancellation.primary_error(client_order_id),
                    )
                }),
                Err(error) => {
                    failures.push(error.to_string());
                    None
                }
            }
        };

        let Some((operation, query_seed, health_error)) = planned else {
            let health_error = match order_economics.tracked_orders.read() {
                Ok(records) => records
                    .get(&client_order_id)
                    .and_then(|record| record.cancellation.as_ref())
                    .and_then(|cancel| cancel.primary_error(client_order_id)),
                Err(_) => None,
            };
            if let Some(error) = health_error {
                failures.push(error.to_string());
            }
            continue;
        };
        if let Some(error) = health_error {
            failures.push(error.to_string());
        }

        let operation_result = match operation.kind {
            CancelOperationKind::Cancel => policy
                .route_cancel_with_sink(
                    sink,
                    client_order_id,
                    Some(ClientId::from(execution_client_id)),
                    None,
                )
                .map(|_| ()),
            CancelOperationKind::Query => sink.query_order_via_nt(
                &query_seed,
                Some(ClientId::from(execution_client_id)),
                None,
            ),
        };
        let settle_now_ns = sink.actor_time_ns()?;
        let cached_after = sink.cached_order(client_order_id)?;
        let mut records = order_economics
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(&client_order_id) else {
            if let Err(error) = operation_result {
                failures.push(error.to_string());
            }
            continue;
        };
        if let Err(error) = operation_result {
            if let Some(cancellation) = record.cancellation.as_mut() {
                cancellation.settle_synchronous_failure(operation.generation);
            }
            failures.push(error.to_string());
            continue;
        }
        let transition = match record.cancellation.as_mut() {
            Some(cancellation) => cancellation.settle_operation(
                operation.generation,
                &mut record.query_seed,
                cached_after.as_ref(),
                settle_now_ns,
                retry_timeout_ns,
            ),
            None => Ok(CancelTransition::NoOperation),
        };
        match transition {
            Ok(CancelTransition::Remove) => {
                records.remove(&client_order_id);
            }
            Ok(_) => {}
            Err(error) => failures.push(error.to_string()),
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

fn route_tracked_cancel_all<S>(
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
        policy.route_cancel_all_with_sink(
            sink,
            instrument_id,
            order_side,
            Some(ClientId::from(execution_client_id)),
            None,
        )?;
        return Ok(());
    }
    let now_ns = sink.actor_time_ns()?;
    let selected = order_economics.request_cancel_scope(instrument_id, order_side, now_ns)?;
    let retry_timeout_ns = order_economics.economics.cancel_retry_timeout_ns()?;
    let escalation_attempts = order_economics
        .economics
        .cancel_recovery_escalation_attempts();
    let mut armed = Vec::new();
    let mut failures = Vec::new();
    for client_order_id in &selected {
        let cached = sink.cached_order(*client_order_id)?;
        let mut records = order_economics
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(client_order_id) else {
            continue;
        };
        let Some(cancellation) = record.cancellation.as_mut() else {
            continue;
        };
        match cancellation.plan_drive(
            &mut record.query_seed,
            cached.as_ref(),
            now_ns,
            retry_timeout_ns,
            escalation_attempts,
        ) {
            Ok((CancelTransition::Remove, _)) => {
                records.remove(client_order_id);
            }
            Ok((_, Some(operation))) if operation.kind == CancelOperationKind::Cancel => {
                armed.push((*client_order_id, operation.generation));
            }
            Ok(_) => {}
            Err(error) => failures.push(error.to_string()),
        }
    }

    let route = policy.route_cancel_all_with_sink(
        sink,
        instrument_id,
        order_side,
        Some(ClientId::from(execution_client_id)),
        None,
    );
    for (client_order_id, generation) in armed {
        let cached = sink.cached_order(client_order_id)?;
        let settle_now_ns = sink.actor_time_ns()?;
        let mut records = order_economics
            .tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("tracked maker order state lock poisoned"))?;
        let Some(record) = records.get_mut(&client_order_id) else {
            continue;
        };
        let Some(cancellation) = record.cancellation.as_mut() else {
            continue;
        };
        if route.is_err() {
            cancellation.settle_synchronous_failure(generation);
            continue;
        }
        match cancellation.settle_operation(
            generation,
            &mut record.query_seed,
            cached.as_ref(),
            settle_now_ns,
            retry_timeout_ns,
        ) {
            Ok(CancelTransition::Remove) => {
                records.remove(&client_order_id);
            }
            Ok(_) => {}
            Err(error) => failures.push(error.to_string()),
        }
    }
    if let Err(error) = route {
        failures.push(error.to_string());
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("tracked maker cancel-all failed: {}", failures.join(" | "))
    }
}

pub trait OrderIntentEvidence {
    fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<crate::bolt_v3_current_evidence::AppendReceipt, RecordFailure>;

    fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome;
}

impl OrderIntentEvidence for OrderExecutionEvidence {
    fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<crate::bolt_v3_current_evidence::AppendReceipt, RecordFailure> {
        self.record_entry_order_intent(fact)
    }

    fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome {
        self.record_risk_reducing_exit_order_intent(fact)
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl OrderIntentEvidence for crate::bolt_v3_current_evidence::DecisionEvidenceRecorder {
    fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<crate::bolt_v3_current_evidence::AppendReceipt, RecordFailure> {
        self.record_entry_order_intent(fact)
    }

    fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome {
        self.record_risk_reducing_exit_order_intent(fact)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderExecutionMode {
    Live,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3OrderExecutionPolicy {
    mode: BoltV3OrderExecutionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NtOrderManagementContract {
    pub order_list_type: &'static str,
    pub submit_order_list_type: &'static str,
    pub cancel_order_type: &'static str,
    pub batch_cancel_orders_type: &'static str,
    pub cancel_all_orders_type: &'static str,
    pub modify_order_type: &'static str,
}

pub fn nt_order_management_contract() -> BoltV3NtOrderManagementContract {
    BoltV3NtOrderManagementContract {
        order_list_type: type_name::<OrderList>(),
        submit_order_list_type: type_name::<SubmitOrderList>(),
        cancel_order_type: type_name::<CancelOrder>(),
        batch_cancel_orders_type: type_name::<BatchCancelOrders>(),
        cancel_all_orders_type: type_name::<CancelAllOrders>(),
        modify_order_type: type_name::<ModifyOrder>(),
    }
}

impl BoltV3OrderExecutionPolicy {
    pub const fn from_mode(mode: BoltV3OrderExecutionMode) -> Self {
        Self { mode }
    }

    pub const fn live() -> Self {
        Self::from_mode(BoltV3OrderExecutionMode::Live)
    }

    pub const fn shadow() -> Self {
        Self::from_mode(BoltV3OrderExecutionMode::Shadow)
    }

    pub const fn mode(self) -> BoltV3OrderExecutionMode {
        self.mode
    }

    pub const fn allows_venue_mutation(self) -> bool {
        matches!(self.mode, BoltV3OrderExecutionMode::Live)
    }

    pub fn route_submit<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        strategy: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> Result<BoltV3SubmitRoutingOutcome>
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_submit_with_sink(routing, &mut sink, order, context)
    }

    pub(crate) fn route_submit_with_sink<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        sink: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> Result<BoltV3SubmitRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        let BoltV3SubmitRoutingRequest {
            decision_evidence,
            submit_admission,
            intent,
            request,
            economics,
            required_remaining_margin_ns,
        } = routing;
        let intent_kind = request.intent_kind;
        let execution_client_id = context
            .client_id
            .as_ref()
            .map(ClientId::as_str)
            .ok_or(BoltV3SubmitAdmissionError::EconomicsOrderMismatch)?;
        let route_now_ns = sink.actor_time_ns()?;
        validate_economics_submit_authority(&request, &economics, &order, execution_client_id)?;
        validate_economics_remaining_margin_at(
            &economics,
            required_remaining_margin_ns,
            route_now_ns,
        )?;
        record_order_intent(decision_evidence, intent_kind, intent.clone())?;
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                let permit =
                    submit_admission.admit_with_economics_at(&request, &economics, route_now_ns)?;
                let pre_sink_now_ns = sink.actor_time_ns()?;
                validate_economics_remaining_margin_at(
                    &economics,
                    required_remaining_margin_ns,
                    pre_sink_now_ns,
                )?;
                sink.submit_order_via_nt(order, context)?;
                permit.commit_submitted();
                Ok(BoltV3SubmitRoutingOutcome::Submitted)
            }
            BoltV3OrderExecutionMode::Shadow => {
                submit_admission.evaluate_and_record_without_consuming_capacity_with_economics_at(
                    &request,
                    &economics,
                    route_now_ns,
                )?;
                log::info!(
                    "bolt-v3 submit skipped by execution policy: mode=shadow strategy_id={} client_order_id={}",
                    request.strategy_id,
                    request.client_order_id,
                );
                Ok(BoltV3SubmitRoutingOutcome::SkippedByPolicy)
            }
        }
    }

    pub fn route_cancel<S>(
        self,
        strategy: &mut S,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelRoutingOutcome>
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_cancel_with_sink(&mut sink, client_order_id, client_id, params)
    }

    fn route_cancel_with_sink<S>(
        self,
        sink: &mut S,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                sink.cancel_order_via_nt(client_order_id, client_id, params)?;
                Ok(BoltV3CancelRoutingOutcome::Canceled)
            }
            BoltV3OrderExecutionMode::Shadow => {
                log::info!(
                    "bolt-v3 cancel skipped by execution policy: mode=shadow client_order_id={client_order_id}"
                );
                Ok(BoltV3CancelRoutingOutcome::SkippedByPolicy)
            }
        }
    }

    pub fn route_modify<S>(
        self,
        strategy: &mut S,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3ModifyRoutingOutcome>
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_modify_with_sink(
            &mut sink,
            client_order_id,
            quantity,
            price,
            client_id,
            params,
        )
    }

    fn route_modify_with_sink<S>(
        self,
        _sink: &mut S,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3ModifyRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        // FAIL-CLOSED in Live (see #835). Unlike submit — which builds a
        // `BoltV3SubmitAdmissionRequest`, records intent evidence, and consumes
        // admission capacity before mutating the venue — an in-place modify carries
        // NONE of those admission/reservation/fee/intent checks. Routing one to the
        // venue in Live would bypass the risk gate: a live amend could lift a resting
        // order's economics-backed reservation past the operator notional limit a submit
        // would block, with no capital-reservation delta recorded. Until the
        // admission-gated in-place modify lands (#835), the Live arm REFUSES the venue
        // mutation; the maker requotes through the already-admitted cancel+resubmit
        // path (the deployed venue contract has `supports_modify=false`, so the maker
        // FSM never emits a Modify and this arm is unreachable from it — this is the
        // structural guard if that capability is ever turned on). Shadow stays
        // suppressed (logged, no NT call), as before.
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                // The amend params are intentionally NOT applied (the modify is
                // refused); consume them so the fail-closed arm is warning-clean.
                let _ = (quantity, price, client_id, params);
                Err(anyhow::anyhow!(
                    "bolt-v3 in-place modify is fail-closed in Live (not admission-gated; see #835): refusing un-admitted venue mutation for client_order_id={client_order_id}"
                ))
            }
            BoltV3OrderExecutionMode::Shadow => {
                log::info!(
                    "bolt-v3 modify skipped by execution policy: mode=shadow client_order_id={client_order_id}"
                );
                Ok(BoltV3ModifyRoutingOutcome::SkippedByPolicy)
            }
        }
    }

    pub fn route_cancel_all<S>(
        self,
        strategy: &mut S,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelAllRoutingOutcome>
    where
        S: Strategy + StrategyNative + DataActorNative + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_cancel_all_with_sink(&mut sink, instrument_id, order_side, client_id, params)
    }

    fn route_cancel_all_with_sink<S>(
        self,
        sink: &mut S,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelAllRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                sink.cancel_all_orders_via_nt(instrument_id, order_side, client_id, params)?;
                Ok(BoltV3CancelAllRoutingOutcome::CanceledAll)
            }
            BoltV3OrderExecutionMode::Shadow => {
                log::info!(
                    "bolt-v3 cancel-all skipped by execution policy: mode=shadow instrument_id={instrument_id}"
                );
                Ok(BoltV3CancelAllRoutingOutcome::SkippedByPolicy)
            }
        }
    }
}

pub(crate) fn record_order_intent(
    recorder: &dyn OrderIntentEvidence,
    intent_kind: BoltV3SubmitIntentKind,
    details: OrderIntentDetails,
) -> Result<()> {
    match intent_kind {
        BoltV3SubmitIntentKind::Entry => recorder
            .record_entry_order_intent(EntryOrderIntentFact { details })
            .map(|_| ())
            .map_err(anyhow::Error::from),
        BoltV3SubmitIntentKind::RiskReducingExit
        | BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
            if let NonBlockingRecordOutcome::Failed(error) = recorder
                .record_risk_reducing_exit_order_intent(RiskReducingExitOrderIntentFact { details })
            {
                log::error!("risk-reducing order intent evidence failed: {error}");
            }
            Ok(())
        }
    }
}

pub fn order_intent_details_from_compiled_order(
    strategy_id: String,
    fallback_price: String,
    order: &OrderAny,
) -> OrderIntentDetails {
    OrderIntentDetails {
        strategy_id,
        instrument_id: order.instrument_id().to_string(),
        client_order_id: order.client_order_id().to_string(),
        order_side: evidence_order_side(order.order_side()),
        price: order
            .price()
            .map(|price| price.to_string())
            .or_else(|| order.trigger_price().map(|price| price.to_string()))
            .or_else(|| order.activation_price().map(|price| price.to_string()))
            .unwrap_or(fallback_price),
        quantity: order.quantity().to_string(),
        clamp_outcome: None,
        order_fields: order_intent_order_fields(order),
    }
}

fn order_intent_order_fields(order: &OrderAny) -> OrderIntentOrderFields {
    OrderIntentOrderFields {
        order_type: evidence_order_type(order.order_type()),
        time_in_force: evidence_time_in_force(order.time_in_force()),
        price: order.price().map(|price| price.to_string()),
        trigger_price: order.trigger_price().map(|price| price.to_string()),
        activation_price: order.activation_price().map(|price| price.to_string()),
        trigger_type: order.trigger_type().map(evidence_trigger_type),
        trigger_instrument_id: order.trigger_instrument_id().map(|value| value.to_string()),
        trailing_offset: order.trailing_offset().map(|value| value.to_string()),
        trailing_offset_type: order
            .trailing_offset_type()
            .map(evidence_trailing_offset_type),
        expire_time_unix_nanos: order.expire_time().map(|value| value.as_u64().to_string()),
        is_post_only: order.is_post_only(),
        is_reduce_only: order.is_reduce_only(),
        is_quote_quantity: order.is_quote_quantity(),
    }
}

fn evidence_order_side(value: OrderSide) -> EvidenceOrderSide {
    match value {
        OrderSide::NoOrderSide => EvidenceOrderSide::Unspecified,
        OrderSide::Buy => EvidenceOrderSide::Buy,
        OrderSide::Sell => EvidenceOrderSide::Sell,
    }
}

fn evidence_order_type(value: OrderType) -> EvidenceOrderType {
    match value {
        OrderType::Market => EvidenceOrderType::Market,
        OrderType::Limit => EvidenceOrderType::Limit,
        OrderType::StopMarket => EvidenceOrderType::StopMarket,
        OrderType::StopLimit => EvidenceOrderType::StopLimit,
        OrderType::MarketToLimit => EvidenceOrderType::MarketToLimit,
        OrderType::MarketIfTouched => EvidenceOrderType::MarketIfTouched,
        OrderType::LimitIfTouched => EvidenceOrderType::LimitIfTouched,
        OrderType::TrailingStopMarket => EvidenceOrderType::TrailingStopMarket,
        OrderType::TrailingStopLimit => EvidenceOrderType::TrailingStopLimit,
    }
}

fn evidence_time_in_force(value: TimeInForce) -> EvidenceTimeInForce {
    match value {
        TimeInForce::Gtc => EvidenceTimeInForce::Gtc,
        TimeInForce::Ioc => EvidenceTimeInForce::Ioc,
        TimeInForce::Fok => EvidenceTimeInForce::Fok,
        TimeInForce::Gtd => EvidenceTimeInForce::Gtd,
        TimeInForce::Day => EvidenceTimeInForce::Day,
        TimeInForce::AtTheOpen => EvidenceTimeInForce::AtTheOpen,
        TimeInForce::AtTheClose => EvidenceTimeInForce::AtTheClose,
    }
}

fn evidence_trigger_type(value: TriggerType) -> EvidenceTriggerType {
    match value {
        TriggerType::NoTrigger => EvidenceTriggerType::NoTrigger,
        TriggerType::Default => EvidenceTriggerType::Default,
        TriggerType::LastPrice => EvidenceTriggerType::LastPrice,
        TriggerType::MarkPrice => EvidenceTriggerType::MarkPrice,
        TriggerType::IndexPrice => EvidenceTriggerType::IndexPrice,
        TriggerType::BidAsk => EvidenceTriggerType::BidAsk,
        TriggerType::DoubleLast => EvidenceTriggerType::DoubleLast,
        TriggerType::DoubleBidAsk => EvidenceTriggerType::DoubleBidAsk,
        TriggerType::LastOrBidAsk => EvidenceTriggerType::LastOrBidAsk,
        TriggerType::MidPoint => EvidenceTriggerType::MidPoint,
    }
}

fn evidence_trailing_offset_type(value: TrailingOffsetType) -> EvidenceTrailingOffsetType {
    match value {
        TrailingOffsetType::NoTrailingOffset => EvidenceTrailingOffsetType::NoTrailingOffset,
        TrailingOffsetType::Price => EvidenceTrailingOffsetType::Price,
        TrailingOffsetType::BasisPoints => EvidenceTrailingOffsetType::BasisPoints,
        TrailingOffsetType::Ticks => EvidenceTrailingOffsetType::Ticks,
        TrailingOffsetType::PriceTier => EvidenceTrailingOffsetType::PriceTier,
    }
}

pub(crate) fn clamp_risk_reducing_exit_to_venue_position(
    submit_admission: &BoltV3SubmitAdmissionState,
    intent_kind: BoltV3SubmitIntentKind,
    mut intent: OrderIntentDetails,
    mut order: OrderAny,
) -> std::result::Result<(OrderIntentDetails, OrderAny), BoltV3ExitClampError> {
    let order_quantity = order.quantity().as_decimal();
    if !intent_kind.is_venue_position_exit_clamp_eligible() || order_quantity <= Decimal::ZERO {
        return Ok((intent, order));
    }
    if order.order_side() != OrderSide::Sell {
        intent.clamp_outcome = Some(OrderIntentClampOutcome::NotEvaluated {
            reason: OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
        });
        return Ok((intent, order));
    }
    let instrument_id = order.instrument_id().to_string();
    let venue_position = match canonical_nt_exit_position(submit_admission, &instrument_id) {
        CanonicalNtExitPosition::Position(position) => position,
        CanonicalNtExitPosition::Missing => {
            intent.clamp_outcome = Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::NoCanonicalNtPosition,
            });
            return Ok((intent, order));
        }
        CanonicalNtExitPosition::ForeignInstrument => {
            intent.clamp_outcome = Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            });
            return Ok((intent, order));
        }
    };
    if order_quantity <= venue_position {
        intent.clamp_outcome = Some(OrderIntentClampOutcome::WithinBounds);
        return Ok((intent, order));
    }
    if venue_position <= Decimal::ZERO {
        return Err(rejected_exit_clamp(
            intent,
            anyhow::anyhow!(
                "risk-reducing exit rejected: no venue-held position to submit: instrument_id={}",
                instrument_id
            ),
        ));
    }

    let original_order_quantity = order_quantity;
    let clamped_decimal =
        match floor_decimal_to_quantity_precision(venue_position, order.quantity().precision) {
            Ok(value) => value,
            Err(error) => return Err(rejected_exit_clamp(intent, error)),
        };
    if clamped_decimal <= Decimal::ZERO {
        return Err(rejected_exit_clamp(
            intent,
            anyhow::anyhow!(
                "risk-reducing exit rejected: venue position is below order quantity precision: instrument_id={}",
                instrument_id
            ),
        ));
    }
    let clamped_quantity = match Quantity::from_decimal_dp(
        clamped_decimal,
        order.quantity().precision,
    ) {
        Ok(quantity) => quantity,
        Err(error) => {
            return Err(rejected_exit_clamp(
                intent,
                anyhow::anyhow!(
                    "risk-reducing exit venue position could not be represented as NT quantity: {error}"
                ),
            ));
        }
    };
    let submitted_quantity = clamped_quantity.as_decimal();
    if submitted_quantity > venue_position {
        return Err(rejected_exit_clamp(
            intent,
            anyhow::anyhow!(
                "risk-reducing exit clamp exceeded venue position: instrument_id={}",
                instrument_id
            ),
        ));
    }

    order.set_quantity(clamped_quantity);
    order.set_leaves_qty(clamped_quantity);
    intent.quantity = order.quantity().to_string();
    intent.clamp_outcome = Some(OrderIntentClampOutcome::Clamped {
        original_quantity: original_order_quantity.to_string(),
    });
    intent.order_fields = order_intent_order_fields(&order);

    Ok((intent, order))
}

#[derive(Debug)]
pub(crate) struct BoltV3ExitClampError {
    intent: Box<OrderIntentDetails>,
    error: anyhow::Error,
}

impl BoltV3ExitClampError {
    pub(crate) fn intent(&self) -> &OrderIntentDetails {
        self.intent.as_ref()
    }

    pub(crate) fn into_error(self) -> anyhow::Error {
        self.error
    }
}

fn rejected_exit_clamp(
    mut intent: OrderIntentDetails,
    error: anyhow::Error,
) -> BoltV3ExitClampError {
    intent.clamp_outcome = Some(OrderIntentClampOutcome::Rejected);
    BoltV3ExitClampError {
        intent: Box::new(intent),
        error,
    }
}

enum CanonicalNtExitPosition {
    Position(Decimal),
    Missing,
    ForeignInstrument,
}

fn canonical_nt_exit_position(
    submit_admission: &BoltV3SubmitAdmissionState,
    instrument_id: &str,
) -> CanonicalNtExitPosition {
    let Some(state) = submit_admission.capital_admission_state_snapshot() else {
        return CanonicalNtExitPosition::Missing;
    };
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    if instrument_id == product.yes_instrument_id {
        CanonicalNtExitPosition::Position(product.yes_position)
    } else if instrument_id == product.no_instrument_id {
        CanonicalNtExitPosition::Position(product.no_position)
    } else {
        CanonicalNtExitPosition::ForeignInstrument
    }
}

fn floor_decimal_to_quantity_precision(value: Decimal, precision: u8) -> Result<Decimal> {
    Ok(value.round_dp_with_strategy(u32::from(precision), RoundingStrategy::ToZero))
}

pub struct BoltV3SubmitRoutingRequest<'a> {
    decision_evidence: &'a dyn OrderIntentEvidence,
    submit_admission: &'a BoltV3SubmitAdmissionState,
    intent: OrderIntentDetails,
    request: BoltV3SubmitAdmissionRequest,
    economics: EconomicsAdmission,
    required_remaining_margin_ns: u64,
}

impl<'a> BoltV3SubmitRoutingRequest<'a> {
    pub fn with_economics(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
        sealed: BoltV3EconomicsSubmitAdmission,
    ) -> Self {
        let (request, economics, required_remaining_margin_ns) = sealed.into_parts();
        Self {
            decision_evidence,
            submit_admission,
            intent,
            request,
            economics,
            required_remaining_margin_ns,
        }
    }

    #[cfg(test)]
    fn for_test(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
        request: BoltV3SubmitAdmissionRequest,
        order: &OrderAny,
    ) -> Self {
        Self::for_test_with_timing(
            decision_evidence,
            submit_admission,
            intent,
            request,
            order,
            u64::MAX,
            1,
        )
    }

    #[cfg(test)]
    fn for_test_with_timing(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
        request: BoltV3SubmitAdmissionRequest,
        order: &OrderAny,
        valid_until_ns: u64,
        required_remaining_margin_ns: u64,
    ) -> Self {
        let purpose = match request.intent_kind {
            BoltV3SubmitIntentKind::Entry => EconomicsAdmissionPurpose::TradingEdge,
            BoltV3SubmitIntentKind::RiskReducingExit
            | BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
                EconomicsAdmissionPurpose::RiskReduction
            }
        };
        let order_side = match order.order_side() {
            OrderSide::Buy => crate::economics::OrderSide::Buy,
            OrderSide::Sell => crate::economics::OrderSide::Sell,
            OrderSide::NoOrderSide => panic!("routing-test order must be sided"),
        };
        let economics = EconomicsAdmission::for_routing_test_with_validity(
            &request.execution_client_id,
            &request.instrument_id,
            order_side,
            economics_order_binding(order).expect("routing-test order should serialize"),
            purpose,
            request.notional,
            request.notional,
            valid_until_ns,
        );
        Self {
            decision_evidence,
            submit_admission,
            intent,
            request,
            economics,
            required_remaining_margin_ns,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoltV3SubmitContext {
    pub(crate) client_id: Option<ClientId>,
    pub(crate) position_id: Option<PositionId>,
    pub(crate) params: Option<Params>,
}

impl BoltV3SubmitContext {
    pub fn from_parts(
        client_id: Option<ClientId>,
        position_id: Option<PositionId>,
        params: Option<Params>,
    ) -> Self {
        Self {
            client_id,
            position_id,
            params,
        }
    }

    pub fn with_client_id(client_id: ClientId) -> Self {
        Self::from_parts(Some(client_id), None, None)
    }

    pub fn with_client_id_and_position_id(client_id: ClientId, position_id: PositionId) -> Self {
        Self::from_parts(Some(client_id), Some(position_id), None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3SubmitRoutingOutcome {
    Submitted,
    SkippedByPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CancelRoutingOutcome {
    Canceled,
    SkippedByPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CancelAllRoutingOutcome {
    CanceledAll,
    SkippedByPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3ModifyRoutingOutcome {
    Modified,
    SkippedByPolicy,
}

#[derive(Clone)]
pub struct BoltV3MakerOrderRoutingContext<'a> {
    pub strategy_id: &'a str,
    pub execution_client_id: &'a str,
    pub order_economics: &'a BoltV3OrderEconomicsHandle,
    pub terminal_value_entry: Option<BoltV3TerminalValueEntry>,
}

#[derive(Clone, Copy)]
pub struct BoltV3KillSwitchFlattenRoutingContext<'a> {
    pub execution_client_id: &'a str,
    pub fallback_price: &'a str,
    pub instrument: Option<&'a InstrumentAny>,
    pub order_economics: &'a BoltV3OrderEconomicsHandle,
}

pub(crate) fn route_kill_switch_flatten_command_with_sink<S>(
    policy: BoltV3OrderExecutionPolicy,
    sink: &mut S,
    order_factory: &mut OrderFactory,
    decision_evidence: &dyn OrderIntentEvidence,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3KillSwitchFlattenRoutingContext<'_>,
    command: &BoltV3KillSwitchFlattenCommand,
) -> Result<BoltV3SubmitRoutingOutcome>
where
    S: BoltV3NtVenueMutationSink + ?Sized,
{
    let client_order_id = flatten_client_order_id(command);
    let order = build_nt_order(
        order_factory,
        command.action_id(),
        command.order_template(),
        NtOrderBuildInputs {
            instrument_id: command.instrument_id(),
            order_side: command.order_side(),
            quantity: command.quantity(),
            price: None,
            client_order_id,
        },
    )?;
    let intent = order_intent_details_from_compiled_order(
        command.strategy_id().to_string(),
        context.fallback_price.to_string(),
        &order,
    );
    let (intent, order) = match clamp_risk_reducing_exit_to_venue_position(
        submit_admission,
        BoltV3SubmitIntentKind::KillSwitchForcedReduction,
        intent,
        order,
    ) {
        Ok(clamped) => clamped,
        Err(error) => {
            record_order_intent(
                decision_evidence,
                BoltV3SubmitIntentKind::KillSwitchForcedReduction,
                error.intent().clone(),
            )?;
            return Err(error.into_error());
        }
    };
    let admission_input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: context.execution_client_id,
        intent: &intent,
        intent_kind: BoltV3SubmitIntentKind::KillSwitchForcedReduction,
        order: &order,
        valuation: crate::bolt_v3_submit_admission::OrderValuationContext {
            instrument: context.instrument,
            ..crate::bolt_v3_submit_admission::OrderValuationContext::empty()
        },
        risk_reducing_exit_position: None,
    };
    let facts = order_admission_facts(&admission_input)?;
    let position = context.order_economics.planned_exit_position(
        command.position_id(),
        command.position_side(),
        facts.order_quantity,
    )?;
    let sealed = build_order_economics_submit_admission(
        context.order_economics,
        BoltV3FinalOrderEconomicsInput {
            execution_client_id: context.execution_client_id,
            intent: &intent,
            order: &order,
            valuation: admission_input.valuation,
            risk_reducing_exit_position: None,
            scenario: BoltV3FinalOrderEconomicsScenario::forced_reduction(position)?,
            candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                price: facts.price,
                quantity: facts.order_quantity,
            }],
            requested_at_ns: command.source_timestamp_unix_nanos(),
            decision_correlation_id: command.action_id(),
        },
    )?
    .with_kill_switch_forced_reduction(command.forced_reduction_claim().clone());

    policy.route_submit_with_sink(
        BoltV3SubmitRoutingRequest::with_economics(
            decision_evidence,
            submit_admission,
            intent,
            sealed,
        ),
        sink,
        order,
        BoltV3SubmitContext::with_client_id_and_position_id(
            ClientId::from(context.execution_client_id),
            command.position_id(),
        ),
    )
}

fn flatten_client_order_id(command: &BoltV3KillSwitchFlattenCommand) -> ClientOrderId {
    let mut value = String::new();
    value.push_str(command.halt_id());
    value.push('-');
    value.push_str(command.action_id());
    value.push('-');
    value.push_str(command.position_id().as_str());
    ClientOrderId::from(value)
}

pub fn route_maker_order_command<S>(
    policy: BoltV3OrderExecutionPolicy,
    strategy: &mut S,
    decision_evidence: &dyn OrderIntentEvidence,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'_>,
    input: MakerOrderDispatchInput<'_>,
) -> Result<MakerOrderDispatchOutcome>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    let mut runtime = NtStrategyMakerOrderRuntime { strategy };
    route_maker_order_command_with_runtime(
        policy,
        &mut runtime,
        decision_evidence,
        submit_admission,
        context,
        input,
    )
}

pub(crate) trait BoltV3NtVenueMutationSink {
    fn actor_time_ns(&mut self) -> Result<u64>;

    fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>>;

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()>;

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;

    fn cancel_all_orders_via_nt(
        &mut self,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;

    // The venue's in-place modify capability. Option A (#835) fail-closes the only
    // routing path (`route_modify_with_sink` refuses live modifies and the maker FSM
    // never emits a Modify while `supports_modify=false`), so this method is
    // intentionally uncalled today. The wiring is retained for #835 (admission-gated
    // in-place modify) and to keep the fail-closed differential tests load-bearing
    // (reverting the fail-close to a venue call must still flip `modify_calls` 0->1).
    // `expect` (not `allow`) is self-cleaning: when #835 wires a real caller the
    // expectation goes unfulfilled and clippy forces this attribute removed.
    #[expect(dead_code)]
    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;
}

pub(crate) struct BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    actor_time_ns: u64,
    dispatch: F,
}

impl<F> BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    pub(crate) fn new(actor_time_ns: u64, dispatch: F) -> Self {
        Self {
            actor_time_ns,
            dispatch,
        }
    }
}

impl<F> BoltV3NtVenueMutationSink for BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    fn actor_time_ns(&mut self) -> Result<u64> {
        Ok(self.actor_time_ns)
    }

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        (self.dispatch)(order, context)
    }

    fn cached_order(&mut self, _client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
        Ok(None)
    }

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        _client_id: Option<ClientId>,
        _params: Option<Params>,
    ) -> Result<()> {
        anyhow::bail!(
            "kill switch flatten submit sink cannot query client_order_id={}",
            seed.client_order_id()
        )
    }

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        _client_id: Option<ClientId>,
        _params: Option<Params>,
    ) -> Result<()> {
        anyhow::bail!(
            "kill switch flatten submit sink cannot cancel client_order_id={client_order_id}"
        )
    }

    fn cancel_all_orders_via_nt(
        &mut self,
        instrument_id: InstrumentId,
        _order_side: Option<OrderSide>,
        _client_id: Option<ClientId>,
        _params: Option<Params>,
    ) -> Result<()> {
        anyhow::bail!(
            "kill switch flatten submit sink cannot cancel-all instrument_id={instrument_id}"
        )
    }

    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        _quantity: Quantity,
        _price: Price,
        _client_id: Option<ClientId>,
        _params: Option<Params>,
    ) -> Result<()> {
        anyhow::bail!(
            "kill switch flatten submit sink cannot modify client_order_id={client_order_id}"
        )
    }
}

struct NtStrategyVenueMutationSink<'a, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyVenueMutationSink<'_, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    fn actor_time_ns(&mut self) -> Result<u64> {
        Ok(self.strategy.clock().timestamp_ns().as_u64())
    }

    fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
        Ok(self.strategy.cache().order(&client_order_id))
    }

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy.query_order(seed, client_id, params)
    }

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        self.strategy.submit_order(
            order,
            context.position_id,
            context.client_id,
            context.params,
        )
    }

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy
            .cancel_order(client_order_id, client_id, params)
    }

    fn cancel_all_orders_via_nt(
        &mut self,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy
            .cancel_all_orders(instrument_id, order_side, client_id, params)
    }

    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        // NT's `modify_order` is the single owner of the in-place amend command
        // (NT-FIRST, NO DUAL PATHS); the maker only supplies the new price and
        // quantity. `trigger_price` is `None` — maker quotes are post-only limits
        // with no trigger.
        self.strategy.modify_order(
            client_order_id,
            Some(quantity),
            Some(price),
            None,
            client_id,
            params,
        )
    }
}

trait BoltV3MakerOrderRuntime: BoltV3NtVenueMutationSink {
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory>;
}

struct NtStrategyMakerOrderRuntime<'a, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    fn actor_time_ns(&mut self) -> Result<u64> {
        Ok(self.strategy.clock().timestamp_ns().as_u64())
    }

    fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
        Ok(self.strategy.cache().order(&client_order_id))
    }

    fn query_order_via_nt(
        &mut self,
        seed: &OrderAny,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy.query_order(seed, client_id, params)
    }

    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        self.strategy.submit_order(
            order,
            context.position_id,
            context.client_id,
            context.params,
        )
    }

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy
            .cancel_order(client_order_id, client_id, params)
    }

    fn cancel_all_orders_via_nt(
        &mut self,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy
            .cancel_all_orders(instrument_id, order_side, client_id, params)
    }

    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.strategy.modify_order(
            client_order_id,
            Some(quantity),
            Some(price),
            None,
            client_id,
            params,
        )
    }
}

impl<S> BoltV3MakerOrderRuntime for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + StrategyNative + DataActorNative + ?Sized,
{
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.strategy.order_factory()
    }
}

fn route_maker_order_command_with_runtime<R>(
    policy: BoltV3OrderExecutionPolicy,
    runtime: &mut R,
    decision_evidence: &dyn OrderIntentEvidence,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'_>,
    input: MakerOrderDispatchInput<'_>,
) -> Result<MakerOrderDispatchOutcome>
where
    R: BoltV3MakerOrderRuntime + ?Sized,
{
    let mut sink = BoltV3MakerOrderPolicySink {
        policy,
        runtime,
        decision_evidence,
        submit_admission,
        context,
    };
    dispatch_maker_order_command(input, &mut sink)
}

struct BoltV3MakerOrderPolicySink<'a, R>
where
    R: BoltV3MakerOrderRuntime + ?Sized,
{
    policy: BoltV3OrderExecutionPolicy,
    runtime: &'a mut R,
    decision_evidence: &'a dyn OrderIntentEvidence,
    submit_admission: &'a BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'a>,
}

impl<R> MakerOrderCommandSink for BoltV3MakerOrderPolicySink<'_, R>
where
    R: BoltV3MakerOrderRuntime + ?Sized,
{
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.runtime.order_factory()
    }

    fn submit_maker_order(&mut self, order: OrderAny) -> Result<()> {
        let fallback_price = order
            .price()
            .map(|price| price.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bolt-v3 maker submit requires a limit price for client_order_id={}",
                    order.client_order_id()
                )
            })?;
        let intent = order_intent_details_from_compiled_order(
            self.context.strategy_id.to_string(),
            fallback_price,
            &order,
        );
        let admission_input = BoltV3SubmitAdmissionRequestInput {
            execution_client_id: self.context.execution_client_id,
            intent: &intent,
            intent_kind: BoltV3SubmitIntentKind::Entry,
            order: &order,
            valuation: crate::bolt_v3_submit_admission::OrderValuationContext::empty(),
            risk_reducing_exit_position: None,
        };
        let facts = order_admission_facts(&admission_input)?;
        let sealed = build_order_economics_submit_admission(
            self.context.order_economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: self.context.execution_client_id,
                intent: &intent,
                order: &order,
                valuation: admission_input.valuation,
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    self.context.terminal_value_entry.clone().ok_or_else(|| {
                        anyhow::anyhow!("maker submit requires a terminal-value economics scenario")
                    })?,
                ),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: facts.price,
                    quantity: facts.order_quantity,
                }],
                requested_at_ns: order.ts_init().as_u64(),
                decision_correlation_id: order.client_order_id().as_str(),
            },
        )?;
        let retained_economics = self
            .policy
            .allows_venue_mutation()
            .then(|| sealed.economics().clone());
        let registration = retained_economics
            .map(|admission| {
                self.context
                    .order_economics
                    .prepare_resting_order_registration(order.clone(), admission)
            })
            .transpose()?;
        let submit_context =
            BoltV3SubmitContext::with_client_id(ClientId::from(self.context.execution_client_id));
        let outcome = self.policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::with_economics(
                self.decision_evidence,
                self.submit_admission,
                intent,
                sealed,
            ),
            self.runtime,
            order,
            submit_context,
        );
        match (outcome, registration) {
            (Ok(BoltV3SubmitRoutingOutcome::Submitted), Some(registration)) => {
                registration.commit();
                Ok(())
            }
            (Ok(_), None) => Ok(()),
            (Ok(_), Some(_)) => {
                anyhow::bail!("resting economics registration state mismatch")
            }
            (Err(error), _) => Err(error),
        }
    }

    fn cancel_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    ) -> Result<()> {
        let now_ns = self.runtime.actor_time_ns()?;
        let tracked = self
            .context
            .order_economics
            .request_cancel_intent(client_order_id, now_ns)?;
        if !tracked {
            anyhow::ensure!(
                !self.policy.allows_venue_mutation(),
                "tracked maker cancellation rejected unknown client order id: {client_order_id}"
            );
            return Ok(());
        }
        let cached = self.runtime.cached_order(client_order_id)?;
        drive_resting_order_economics(
            self.context.order_economics,
            self.policy,
            self.runtime,
            self.context.execution_client_id,
            vec![(client_order_id, cached)],
            now_ns,
        )
    }

    fn cancel_all_maker_orders(
        &mut self,
        _leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<()> {
        route_tracked_cancel_all(
            self.context.order_economics,
            self.policy,
            self.runtime,
            self.context.execution_client_id,
            instrument_id,
            order_side,
        )
    }

    fn modify_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Result<()> {
        // Routes the in-place amend through the execution-policy boundary. Under
        // Option A (#835) the Live arm is FAIL-CLOSED: an in-place modify does not
        // pass submit admission, so `route_modify_with_sink` returns `Err`, the `?`
        // below propagates it, and the venue is never reached; Shadow stays
        // suppressed. The FSM only emits a Modify for a modify-capable venue (the
        // `supports_modify` capability is threaded into the leg state machine), and
        // the deployed venue contract has `supports_modify=false`, so the maker
        // requotes via cancel+resubmit and a no-modify venue never reaches this path.
        self.policy.route_modify_with_sink(
            self.runtime,
            client_order_id,
            quantity,
            price,
            Some(ClientId::from(self.context.execution_client_id)),
            None,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{RefCell, RefMut},
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
        sync::Arc,
    };

    use anyhow::Result;
    use nautilus_common::{
        clock::{Clock, TestClock},
        factories::OrderFactory,
    };
    use nautilus_core::Params;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::{
            AssetClass, LiquiditySide, OrderSide, OrderStatus, OrderType, PositionSide,
            TimeInForce, TradingState,
        },
        events::{OrderCanceled, OrderEventAny, order::spec::OrderFillVoidedSpec},
        identifiers::{
            AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, Symbol,
            TradeId, TraderId, VenueOrderId,
        },
        instruments::{BinaryOption, InstrumentAny},
        orders::{LimitOrder, Order, OrderAny, stubs::TestOrderEventStubs},
        types::{Currency, Money, Price, Quantity},
    };
    use rust_decimal::Decimal;
    use ustr::Ustr;

    use super::{
        BoltV3CancelAllRoutingOutcome, BoltV3CancelRoutingOutcome, BoltV3FinalOrderEconomicsInput,
        BoltV3FinalOrderEconomicsScenario, BoltV3MakerOrderRoutingContext, BoltV3MakerOrderRuntime,
        BoltV3ModifyRoutingOutcome, BoltV3NtVenueMutationSink, BoltV3OrderExecutionMode,
        BoltV3OrderExecutionPolicy, BoltV3PlannedFillLeg, BoltV3SubmitContext,
        BoltV3SubmitRoutingOutcome, BoltV3SubmitRoutingRequest, BoltV3TakerEconomicsSizingInput,
        BoltV3TerminalValueEntry, EconomicsAdmissionPurpose, LifecyclePath, NtOrderQuerySeed,
        TrackedMakerOrderRecord, build_order_economics_submit_admission,
        clamp_risk_reducing_exit_to_venue_position, economics_order_binding,
        order_intent_details_from_compiled_order, route_kill_switch_flatten_command_with_sink,
        route_maker_order_command_with_runtime,
    };
    use crate::{
        bolt_v3_capital_admission::{
            CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
            ProductAdmissionSnapshot, ProductKind,
        },
        bolt_v3_capital_admission_runtime_feed::{
            CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
            POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE,
        },
        bolt_v3_capital_admission_state::{
            OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
            ProviderCollateralAllowanceSnapshot,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_current_evidence::{
            AdmittedEntryAdmissionFact, CurrentFact, DecisionEvidenceRecorder,
            ForcedReductionAdmissionFact, OrderIntentClampNotEvaluatedReason,
            OrderIntentClampOutcome, OrderIntentDetails, RejectedEntryAdmissionFact,
        },
        bolt_v3_kill_switch::KillSwitchState,
        bolt_v3_kill_switch_flatten::{
            BoltV3KillSwitchFlattenCandidate, BoltV3KillSwitchFlattenPlanRequest,
            BoltV3KillSwitchFlattenPolicy, BoltV3KillSwitchFlattenPositionEvidenceKind,
            BoltV3KillSwitchFlattenPositionState, BoltV3KillSwitchFlattenRouteKind,
            BoltV3KillSwitchFlattenRouteProof, BoltV3KillSwitchFlattenSnapshot,
            BoltV3KillSwitchFlattenSupervisor,
        },
        bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
        bolt_v3_maker_order_dispatch::{MakerOrderDispatchInput, MakerOrderDispatchOutcome},
        bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
        bolt_v3_quote_lifecycle::Leg,
        bolt_v3_submit_admission::{
            BoltV3CompiledOrderAdmissionEvidence, BoltV3CompiledOrderKind,
            BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide, BoltV3CompiledProductKind,
            BoltV3KillSwitchForcedReductionClaim, BoltV3KillSwitchForcedReductionPolicy,
            BoltV3LiveSubmitApprovalLimits, BoltV3RiskReducingExitProof,
            BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
            BoltV3SubmitCapitalAdmissionConfig, BoltV3SubmitCapitalAdmissionNtComponents,
            BoltV3SubmitIntentKind, OrderValuationContext, PredictionMarketOutcomeSide,
        },
        economics::LiquidityRole,
    };

    trait RecordedCurrentEvidence {
        fn order_intents(&self) -> Vec<OrderIntentDetails>;
        fn admitted_entry_admissions(&self) -> Vec<AdmittedEntryAdmissionFact>;
        fn rejected_entry_admissions(&self) -> Vec<RejectedEntryAdmissionFact>;
        fn forced_reduction_admissions(&self) -> Vec<ForcedReductionAdmissionFact>;
        fn admission_count(&self) -> usize;
    }

    impl RecordedCurrentEvidence for DecisionEvidenceRecorder {
        fn order_intents(&self) -> Vec<OrderIntentDetails> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::EntryOrderIntent(fact) => Some(fact.details),
                    CurrentFact::RiskReducingExitOrderIntent(fact) => Some(fact.details),
                    _ => None,
                })
                .collect()
        }

        fn admitted_entry_admissions(&self) -> Vec<AdmittedEntryAdmissionFact> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::AdmittedEntryAdmission(fact) => Some(*fact),
                    _ => None,
                })
                .collect()
        }

        fn rejected_entry_admissions(&self) -> Vec<RejectedEntryAdmissionFact> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::RejectedEntryAdmission(fact) => Some(*fact),
                    _ => None,
                })
                .collect()
        }

        fn forced_reduction_admissions(&self) -> Vec<ForcedReductionAdmissionFact> {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter_map(|fact| match fact {
                    CurrentFact::ForcedReductionAdmission(fact) => Some(*fact),
                    _ => None,
                })
                .collect()
        }

        fn admission_count(&self) -> usize {
            self.recorded_facts()
                .expect("recorded current evidence must decode")
                .into_iter()
                .filter(|fact| {
                    matches!(
                        fact,
                        CurrentFact::AdmittedEntryAdmission(_)
                            | CurrentFact::RejectedEntryAdmission(_)
                            | CurrentFact::RiskReducingExitAdmission(_)
                            | CurrentFact::ForcedReductionAdmission(_)
                    )
                })
                .count()
        }
    }

    #[derive(Debug)]
    struct RecordingMakerRuntime {
        order_factory: RefCell<OrderFactory>,
        venue_sink: RecordingVenueMutationSink,
    }

    impl RecordingMakerRuntime {
        fn new() -> Self {
            Self {
                order_factory: RefCell::new(generic_order_factory()),
                venue_sink: RecordingVenueMutationSink::default(),
            }
        }
    }

    impl BoltV3NtVenueMutationSink for RecordingMakerRuntime {
        fn actor_time_ns(&mut self) -> Result<u64> {
            self.venue_sink.actor_time_ns()
        }

        fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
            self.venue_sink.cached_order(client_order_id)
        }

        fn query_order_via_nt(
            &mut self,
            seed: &OrderAny,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink.query_order_via_nt(seed, client_id, params)
        }

        fn submit_order_via_nt(
            &mut self,
            order: OrderAny,
            context: BoltV3SubmitContext,
        ) -> Result<()> {
            self.venue_sink.submit_order_via_nt(order, context)
        }

        fn cancel_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink
                .cancel_order_via_nt(client_order_id, client_id, params)
        }

        fn cancel_all_orders_via_nt(
            &mut self,
            instrument_id: InstrumentId,
            order_side: Option<OrderSide>,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink
                .cancel_all_orders_via_nt(instrument_id, order_side, client_id, params)
        }

        fn modify_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            quantity: Quantity,
            price: Price,
            client_id: Option<ClientId>,
            params: Option<Params>,
        ) -> Result<()> {
            self.venue_sink
                .modify_order_via_nt(client_order_id, quantity, price, client_id, params)
        }
    }

    impl BoltV3MakerOrderRuntime for RecordingMakerRuntime {
        fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
            self.order_factory.borrow_mut()
        }
    }

    #[test]
    fn maker_submit_routes_through_shared_execution_policy_and_admission() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let mut runtime = RecordingMakerRuntime::new();
        let command = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("MAKER-YES-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should route through shared execution policy");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::Submitted {
                leg: Leg::Yes,
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                client_order_id: ClientOrderId::from("MAKER-YES-1"),
                price: Price::new(0.40, 2),
                quantity: Quantity::new(2.0, 2),
            }
        );
        assert_eq!(runtime.venue_sink.submit_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.order_intents()[0].strategy_id, "maker-strategy");
        assert_eq!(
            writer.order_intents()[0].instrument_id,
            InstrumentId::from("YES.INSTRUMENT").to_string()
        );
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 1);
        assert_eq!(
            order_economics.resting_order_ids().unwrap(),
            vec![ClientOrderId::from("MAKER-YES-1")]
        );

        let mut cancel_sink = RecordingVenueMutationSink::default();
        let error = super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut cancel_sink,
            "maker_execution_client",
            vec![(ClientOrderId::from("MAKER-YES-1"), None)],
            1,
        )
        .expect_err("a missing unqueryable order must become loud without a fake cancel");
        assert!(error.to_string().contains("identity unavailable"));
        assert_eq!(cancel_sink.cancel_calls, 0);
        assert!(
            order_economics.resting_cancel_health().unwrap()[0].recovery_identity_unavailable()
        );

        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut cancel_sink,
            "maker_execution_client",
            vec![(ClientOrderId::from("MAKER-YES-1"), None)],
            2,
        )
        .expect_err("unresolved missing identity must remain loud without venue churn");
        assert_eq!(cancel_sink.cancel_calls, 0);
    }

    #[test]
    fn healthy_resting_order_survives_timer_drives_without_a_cancel_intent() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let mut runtime = RecordingMakerRuntime::new();
        let command = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("HEALTHY-MAKER-YES-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();

        let cached = runtime
            .venue_sink
            .cached_order(ClientOrderId::from("HEALTHY-MAKER-YES-1"))
            .unwrap();
        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(ClientOrderId::from("HEALTHY-MAKER-YES-1"), cached)],
            2,
        )
        .unwrap();

        assert_eq!(runtime.venue_sink.cancel_calls, 0);
        assert_eq!(runtime.venue_sink.query_calls, 0);
        assert!(order_economics.resting_cancel_health().unwrap().is_empty());
        assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);
    }

    #[test]
    fn maker_cancel_routes_through_shared_execution_policy_with_configured_client() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::No,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("MAKER-NO-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should establish tracked cancellation identity");
        let command = MakerCompiledOrderCommand::Cancel {
            leg: Leg::No,
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            client_order_id: ClientOrderId::from("MAKER-NO-1"),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker cancel should route through shared execution policy");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::Canceled {
                leg: Leg::No,
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                client_order_id: ClientOrderId::from("MAKER-NO-1"),
            }
        );
        assert_eq!(runtime.venue_sink.cancel_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
    }

    #[test]
    fn repeated_cancel_origins_merge_without_resetting_exact_retry_boundary() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-RETRY-1");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();
        runtime.venue_sink.fail_cancel_ids.insert(client_order_id);
        let cancel = MakerCompiledOrderCommand::Cancel {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            client_order_id,
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &cancel,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect_err("the first synchronous cancel failure must remain retryable");
        assert_eq!(runtime.venue_sink.cancel_calls, 1);

        let retry_timeout_ns = order_economics.economics.cancel_retry_timeout_ns().unwrap();
        order_economics
            .begin_resting_order_drain_at_ns(retry_timeout_ns / 2)
            .expect("a second cancellation origin must merge into the existing intent");
        let cached = runtime.venue_sink.cached_order(client_order_id).unwrap();
        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, cached.clone())],
            retry_timeout_ns,
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.cancel_calls, 1);

        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, cached)],
            retry_timeout_ns + 1,
        )
        .expect_err("the exact armed boundary should perform one bounded retry");
        assert_eq!(runtime.venue_sink.cancel_calls, 2);
    }

    #[test]
    fn partial_fill_retains_tracking_and_fill_void_recreates_cancel_only_tracking() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-FILL-VOID-1");
        let instrument_id = InstrumentId::from("YES.INSTRUMENT");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id,
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();

        let mut order = runtime
            .venue_sink
            .cached_order(client_order_id)
            .unwrap()
            .unwrap();
        let submitted = TestOrderEventStubs::submitted(&order, AccountId::from("ACCOUNT-001"));
        order.apply(submitted).unwrap();
        let accepted = TestOrderEventStubs::accepted(
            &order,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from("VENUE-FILL-VOID-1"),
        );
        order.apply(accepted).unwrap();
        let instrument = binary_option_with_max_price(instrument_id);
        let partial_fill = TestOrderEventStubs::filled(
            &order,
            &instrument,
            Some(TradeId::from("TRADE-PARTIAL-1")),
            None,
            Some(Price::new(0.40, 2)),
            Some(Quantity::new(1.0, 2)),
            Some(LiquiditySide::Maker),
            None,
            Some(UnixNanos::from(2_u64)),
            Some(AccountId::from("ACCOUNT-001")),
        );
        order.apply(partial_fill).unwrap();
        assert_eq!(order.status(), OrderStatus::PartiallyFilled);
        order_economics
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 2)
            .unwrap();
        assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);

        let full_fill = TestOrderEventStubs::filled(
            &order,
            &instrument,
            Some(TradeId::from("TRADE-FULL-1")),
            None,
            Some(Price::new(0.40, 2)),
            Some(Quantity::new(1.0, 2)),
            Some(LiquiditySide::Maker),
            None,
            Some(UnixNanos::from(3_u64)),
            Some(AccountId::from("ACCOUNT-001")),
        );
        order.apply(full_fill).unwrap();
        assert_eq!(order.status(), OrderStatus::Filled);
        order_economics
            .reconcile_tracked_order_at(client_order_id, Some(order.clone()), 3)
            .unwrap();
        assert!(order_economics.resting_order_ids().unwrap().is_empty());

        let fill_voided = OrderEventAny::FillVoided(
            OrderFillVoidedSpec::builder()
                .trader_id(order.trader_id())
                .strategy_id(order.strategy_id())
                .instrument_id(instrument_id)
                .client_order_id(client_order_id)
                .venue_order_id(VenueOrderId::from("VENUE-FILL-VOID-1"))
                .account_id(AccountId::from("ACCOUNT-001"))
                .trade_id(TradeId::from("TRADE-FULL-1"))
                .voided_qty(Quantity::new(1.0, 2))
                .commission_voided(Money::from("2 USD"))
                .order_side(OrderSide::Buy)
                .order_type(OrderType::Limit)
                .last_px(Price::new(0.40, 2))
                .currency(Currency::USD())
                .liquidity_side(LiquiditySide::Maker)
                .position_id(PositionId::new("1"))
                .is_reopened(true)
                .build(),
        );
        order.apply(fill_voided).unwrap();
        assert_eq!(order.status(), OrderStatus::PartiallyFilled);
        order_economics
            .reconcile_fill_void_at(client_order_id, Some(order.clone()), 4)
            .unwrap();
        {
            let records = order_economics.tracked_orders.read().unwrap();
            let recreated = records.get(&client_order_id).unwrap();
            assert!(recreated.economics.is_none());
            assert!(recreated.cancellation.is_some());
        }

        runtime
            .venue_sink
            .cached_orders
            .insert(client_order_id, order.clone());
        runtime.venue_sink.actor_times_ns.push_back(4);
        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, Some(order))],
            4,
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.cancel_calls, 1);
    }

    #[test]
    fn fill_void_recovery_deadline_overflow_fails_before_tracking() {
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let order = limit_order("MAKER-FILL-VOID-OVERFLOW");
        let client_order_id = order.client_order_id();

        let error = order_economics
            .reconcile_fill_void_at(client_order_id, Some(order), u64::MAX)
            .expect_err("a fill-void deadline overflow must fail before registration");

        assert!(error.to_string().contains("deadline overflow"));
        assert!(order_economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn captured_identity_routes_query_and_only_authoritative_cache_state_retires() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let client_order_id = ClientOrderId::from("MAKER-QUERY-1");
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id,
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();
        let mut accepted = runtime
            .venue_sink
            .cached_order(client_order_id)
            .unwrap()
            .unwrap();
        let submitted_event =
            TestOrderEventStubs::submitted(&accepted, AccountId::from("ACCOUNT-001"));
        accepted.apply(submitted_event).unwrap();
        let accepted_event = TestOrderEventStubs::accepted(
            &accepted,
            AccountId::from("ACCOUNT-001"),
            VenueOrderId::from("VENUE-QUERY-1"),
        );
        accepted.apply(accepted_event).unwrap();
        runtime
            .venue_sink
            .cached_orders
            .insert(client_order_id, accepted.clone());
        let cancel = MakerCompiledOrderCommand::Cancel {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            client_order_id,
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &cancel,
                submit_order_prefix: "maker_submit",
            },
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.cancel_calls, 1);

        runtime.venue_sink.cached_orders.remove(&client_order_id);
        let retry_timeout_ns = order_economics.economics.cancel_retry_timeout_ns().unwrap();
        runtime
            .venue_sink
            .actor_times_ns
            .push_back(retry_timeout_ns + 1);
        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, None)],
            retry_timeout_ns + 1,
        )
        .unwrap();
        assert_eq!(runtime.venue_sink.query_calls, 1);
        assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);

        let mut terminal = accepted;
        let canceled_event = TestOrderEventStubs::canceled(
            &terminal,
            AccountId::from("ACCOUNT-001"),
            Some(VenueOrderId::from("VENUE-QUERY-1")),
        );
        terminal.apply(canceled_event).unwrap();
        runtime
            .venue_sink
            .cached_orders
            .insert(client_order_id, terminal.clone());
        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(client_order_id, Some(terminal))],
            retry_timeout_ns + 2,
        )
        .unwrap();
        assert!(order_economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn one_failing_record_does_not_starve_due_siblings() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            BTreeMap::from([(
                "maker_execution_client".to_string(),
                BoltV3LiveSubmitApprovalLimits {
                    max_order_count: 2,
                    max_order_notional: Decimal::new(100, 0),
                },
            )]),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let first = ClientOrderId::from("MAKER-SIBLING-A");
        let second = ClientOrderId::from("MAKER-SIBLING-B");
        for (client_order_id, instrument_id, leg) in [
            (first, InstrumentId::from("YES.INSTRUMENT"), Leg::Yes),
            (second, InstrumentId::from("YES.INSTRUMENT"), Leg::No),
        ] {
            let submit = MakerCompiledOrderCommand::Submit {
                leg,
                template: Box::new(maker_limit_post_only_template()),
                inputs: NtOrderBuildInputs {
                    instrument_id,
                    order_side: OrderSide::Buy,
                    quantity: Quantity::new(1.0, 2),
                    price: Some(Price::new(0.40, 2)),
                    client_order_id,
                },
                fallback_price: Price::new(0.40, 2),
            };
            route_maker_order_command_with_runtime(
                BoltV3OrderExecutionPolicy::live(),
                &mut runtime,
                writer.as_ref(),
                admission.as_ref(),
                maker_routing_context(&order_economics),
                MakerOrderDispatchInput {
                    command: &submit,
                    submit_order_prefix: "maker_submit",
                },
            )
            .unwrap();
        }
        runtime.venue_sink.fail_cancel_ids.insert(first);
        order_economics.begin_resting_order_drain_at_ns(1).unwrap();
        let first_cached = runtime.venue_sink.cached_order(first).unwrap();
        let second_cached = runtime.venue_sink.cached_order(second).unwrap();

        super::drive_resting_order_economics(
            &order_economics,
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            "maker_execution_client",
            vec![(first, first_cached), (second, second_cached)],
            1,
        )
        .expect_err("one failing record must be aggregated after its sibling is processed");

        assert_eq!(runtime.venue_sink.cancel_calls, 2);
        let records = order_economics.tracked_orders.read().unwrap();
        assert!(records.get(&first).unwrap().cancellation.is_some());
        assert!(records.get(&second).unwrap().cancellation.is_some());
    }

    #[test]
    fn one_side_cancel_all_marks_only_matching_records_after_nt_accepts() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let submit = MakerCompiledOrderCommand::Submit {
            leg: Leg::No,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("MAKER-NO-ALL-1"),
            },
            fallback_price: Price::new(0.40, 2),
        };
        route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &submit,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker submit should establish tracked cancel-all scope");
        let sell_client_order_id = ClientOrderId::from("MAKER-NO-ALL-SELL");
        let sell = limit_exit_order_for_instrument(
            sell_client_order_id.as_str(),
            InstrumentId::from("NO.INSTRUMENT"),
            Quantity::new(1.0, 2),
        );
        let cloned_economics = order_economics
            .tracked_orders
            .read()
            .unwrap()
            .get(&ClientOrderId::from("MAKER-NO-ALL-1"))
            .unwrap()
            .economics
            .clone();
        order_economics.tracked_orders.write().unwrap().insert(
            sell_client_order_id,
            TrackedMakerOrderRecord {
                economics: cloned_economics,
                query_seed: NtOrderQuerySeed::new(sell.clone()),
                cancellation: None,
            },
        );
        runtime
            .venue_sink
            .cached_orders
            .insert(sell_client_order_id, sell);
        let command = MakerCompiledOrderCommand::CancelAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            order_side: Some(OrderSide::Buy),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker cancel-all should route through shared execution policy");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::CanceledAll {
                leg: Some(Leg::No),
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                order_side: Some(OrderSide::Buy),
            }
        );
        assert_eq!(runtime.venue_sink.cancel_all_calls, 1);
        assert_eq!(
            runtime.venue_sink.cancel_all_requests,
            vec![(
                InstrumentId::from("NO.INSTRUMENT"),
                Some(OrderSide::Buy),
                Some(ClientId::from("maker_execution_client")),
            )]
        );
        assert_eq!(runtime.venue_sink.cancel_calls, 0);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
        let records = order_economics.tracked_orders.read().unwrap();
        assert!(
            records
                .get(&ClientOrderId::from("MAKER-NO-ALL-1"))
                .unwrap()
                .cancellation
                .is_some()
        );
        assert!(
            records
                .get(&sell_client_order_id)
                .unwrap()
                .cancellation
                .is_none()
        );
    }

    #[derive(Debug, Default)]
    struct RecordingVenueMutationSink {
        actor_times_ns: std::collections::VecDeque<u64>,
        cached_orders: BTreeMap<ClientOrderId, OrderAny>,
        submit_calls: usize,
        submitted_order_quantities: Vec<Quantity>,
        cancel_calls: usize,
        query_calls: usize,
        cancel_all_calls: usize,
        cancel_all_requests: Vec<(InstrumentId, Option<OrderSide>, Option<ClientId>)>,
        modify_calls: usize,
        modify_requests: Vec<(ClientOrderId, Quantity, Price, Option<ClientId>)>,
        fail_submits: bool,
        fail_cancel_ids: BTreeSet<ClientOrderId>,
    }

    impl BoltV3NtVenueMutationSink for RecordingVenueMutationSink {
        fn actor_time_ns(&mut self) -> Result<u64> {
            Ok(self.actor_times_ns.pop_front().unwrap_or(1))
        }

        fn cached_order(&mut self, client_order_id: ClientOrderId) -> Result<Option<OrderAny>> {
            Ok(self.cached_orders.get(&client_order_id).cloned())
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
            order: OrderAny,
            _context: BoltV3SubmitContext,
        ) -> Result<()> {
            self.submit_calls += 1;
            self.submitted_order_quantities.push(order.quantity());
            if self.fail_submits {
                anyhow::bail!("synthetic NT submit failure");
            }
            self.cached_orders.insert(order.client_order_id(), order);
            Ok(())
        }

        fn cancel_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            _client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.cancel_calls += 1;
            if self.fail_cancel_ids.contains(&client_order_id) {
                anyhow::bail!("synthetic NT cancel failure: {client_order_id}");
            }
            Ok(())
        }

        fn cancel_all_orders_via_nt(
            &mut self,
            instrument_id: InstrumentId,
            order_side: Option<OrderSide>,
            client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.cancel_all_calls += 1;
            self.cancel_all_requests
                .push((instrument_id, order_side, client_id));
            Ok(())
        }

        fn modify_order_via_nt(
            &mut self,
            client_order_id: ClientOrderId,
            quantity: Quantity,
            price: Price,
            client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.modify_calls += 1;
            self.modify_requests
                .push((client_order_id, quantity, price, client_id));
            Ok(())
        }
    }

    #[test]
    fn live_submit_records_evidence_consumes_capacity_and_calls_nt_submit_once() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-LIVE-1");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let outcome = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("live submit should route through NT");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 1);
    }

    #[test]
    fn total_lifetime_cannot_hide_insufficient_remaining_margin() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("remaining-margin-delayed");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([90]),
            ..RecordingVenueMutationSink::default()
        };

        let error = BoltV3OrderExecutionPolicy::shadow()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test_with_timing(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                    100,
                    20,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("delayed routing must use remaining, not total, quote lifetime");

        assert!(error.to_string().contains("lacks remaining lifetime"));
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn source_horizon_shorter_than_remaining_margin_fails_before_evidence() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("remaining-margin-source");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([1]),
            ..RecordingVenueMutationSink::default()
        };

        let error = BoltV3OrderExecutionPolicy::shadow()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test_with_timing(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                    15,
                    20,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("a short source horizon must fail before evidence");

        assert!(error.to_string().contains("lacks remaining lifetime"));
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn exact_remaining_margin_boundary_is_accepted() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("remaining-margin-exact");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([80]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test_with_timing(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                    100,
                    20,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("the exact remaining-margin boundary should be accepted");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::SkippedByPolicy);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn pre_sink_clock_advance_rolls_back_permit_and_registration() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let mut runtime = RecordingMakerRuntime::new();
        runtime.venue_sink.actor_times_ns = std::collections::VecDeque::from([1, u64::MAX]);
        let command = MakerCompiledOrderCommand::Submit {
            leg: Leg::Yes,
            template: Box::new(maker_limit_post_only_template()),
            inputs: NtOrderBuildInputs {
                instrument_id: InstrumentId::from("YES.INSTRUMENT"),
                order_side: OrderSide::Buy,
                quantity: Quantity::new(2.0, 2),
                price: Some(Price::new(0.40, 2)),
                client_order_id: ClientOrderId::from("remaining-margin-pre-sink"),
            },
            fallback_price: Price::new(0.40, 2),
        };

        let error = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect_err("the fresh pre-sink time must veto an expired remaining margin");

        assert!(error.to_string().contains("lacks remaining lifetime"));
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(runtime.venue_sink.submit_calls, 0);
        assert!(order_economics.resting_order_ids().unwrap().is_empty());
    }

    #[test]
    fn actor_clock_regression_fails_before_evidence() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("actor-clock-regression");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([0]),
            ..RecordingVenueMutationSink::default()
        };

        let error = BoltV3OrderExecutionPolicy::shadow()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test_with_timing(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                    100,
                    20,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("actor time before order time must fail closed");

        assert!(error.to_string().contains("lacks remaining lifetime"));
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(sink.submit_calls, 0);
    }

    #[test]
    fn production_economics_route_uses_only_injected_actor_time() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let order = limit_order("actor-time-only");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([1]),
            ..RecordingVenueMutationSink::default()
        };

        let outcome = BoltV3OrderExecutionPolicy::shadow()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test_with_timing(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                    21,
                    20,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("wall time must not affect an actor-time economics route");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::SkippedByPolicy);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admission_count(), 1);
    }

    #[test]
    fn live_submit_rejected_by_latched_kill_switch_never_calls_nt() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        admission.replace_kill_switch_state(KillSwitchState::FailedManualIntervention {
            halt_id: "halt-latched".to_string(),
            reason: "operator intervention required".to_string(),
        });
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-LATCHED-1");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));

        let error = BoltV3OrderExecutionPolicy::live()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("latched kill switch must reject before NT submit");

        assert!(
            error
                .to_string()
                .contains("blocked by kill-switch state FailedManualIntervention"),
            "unexpected latched kill-switch rejection: {error:#}"
        );
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.rejected_entry_admissions().len(), 1);
        assert_eq!(
            writer.rejected_entry_admissions()[0].reason,
            crate::bolt_v3_current_evidence::AdmissionRejectionReason::KillSwitchLatched
        );
    }

    #[test]
    fn live_submit_failure_rolls_back_capital_admission_reservation() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer.clone(),
            capital_admission_config(),
        ));
        admission.update_capital_admission_nt_components(capital_admission_components());
        let rebuild =
            admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1);
        assert!(rebuild.accepted);

        let mut sink = RecordingVenueMutationSink {
            fail_submits: true,
            ..RecordingVenueMutationSink::default()
        };
        let order = limit_order("O-19700101-000000-001-ROLLBACK-1");
        let intent = intent_for_order(&order);
        let request = admission_evidence_submit_request_for_order(&order);
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let result = policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::for_test(
                writer.as_ref(),
                admission.as_ref(),
                intent,
                request,
                &order,
            ),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution-client-a")),
        );

        assert!(result.is_err());
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(
            admission.capital_admission_live_reserved_liability(),
            Some(Decimal::ZERO)
        );
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(
            !admission.capital_admission_has_live_reservation("O-19700101-000000-001-ROLLBACK-1")
        );
    }

    #[test]
    fn live_risk_reducing_exit_clamps_submitted_quantity_to_venue_position() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );

        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order("O-19700101-000000-001-EXIT-CLAMP-1", Quantity::new(5.0, 2));
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::RiskReducingExit,
            intent,
            order,
        )
        .expect("risk-reducing exit should clamp before economics is sealed");
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let outcome = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("live risk-reducing exit should submit with clamped economics authority");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(3.0, 2)]);
        assert_eq!(admission.admitted_order_count(), 1);
        let records = writer.order_intents();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].quantity, Quantity::new(3.0, 2).to_string());
        assert_eq!(
            records[0].clamp_outcome,
            Some(OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            })
        );
    }

    #[test]
    fn live_risk_reducing_exit_reaches_nt_when_order_intent_evidence_fails() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        writer.fail_purpose_on_attempt_for_test(
            crate::bolt_v3_current_evidence::CurrentEvidenceTestPurpose::RiskReducingExitOrderIntent,
            1,
        );
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order(
            "O-19700101-000000-001-EXIT-EVIDENCE-FAILURE-1",
            Quantity::new(3.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::RiskReducingExit,
            intent,
            order,
        )
        .expect("within-bounds exit should be sealed after clamp evaluation");
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );

        let outcome = BoltV3OrderExecutionPolicy::live()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("risk-reducing evidence failure must not block the NT submit");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(admission.admitted_order_count(), 1);
        assert!(
            writer.order_intents().is_empty(),
            "the targeted order-intent write must fail before appending evidence"
        );
    }

    #[test]
    fn risk_reducing_exit_without_provider_collateral_allowance_records_not_evaluated_reason() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order(
            "O-19700101-000000-001-EXIT-NO-TRUTH-1",
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::RiskReducingExit,
            intent,
            order,
        )
        .expect("missing canonical position should preserve the order with explicit evidence");
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let outcome = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect(
                "missing provider collateral allowance should pass through with explicit evidence",
            );

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(5.0, 2)]);
        let records = writer.order_intents();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].clamp_outcome,
            Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::NoCanonicalNtPosition,
            })
        );
    }

    #[test]
    fn risk_reducing_exit_for_foreign_instrument_records_not_evaluated_reason() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission =
            provider_collateral_allowance_admission_with_yes_position(writer, Decimal::new(3, 0));
        let order = limit_exit_order_for_instrument(
            "O-19700101-000000-001-EXIT-FOREIGN-1",
            InstrumentId::from("instrument-foreign.VENUE-A"),
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::RiskReducingExit,
            intent,
            order,
        )
        .expect("foreign instrument should pass through with explicit evidence");

        assert_eq!(order.quantity(), Quantity::new(5.0, 2));
        assert_eq!(
            intent.clamp_outcome,
            Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            })
        );
    }

    #[test]
    fn clamp_eligible_non_sell_order_records_not_evaluated_reason() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission =
            provider_collateral_allowance_admission_with_yes_position(writer, Decimal::new(3, 0));
        let order = limit_order("O-19700101-000000-001-FLAT-BUY-1");
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::KillSwitchForcedReduction,
            intent,
            order,
        )
        .expect("non-Sell forced reduction should pass through with explicit evidence");

        assert_eq!(order.order_side(), OrderSide::Buy);
        assert_eq!(
            intent.clamp_outcome,
            Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
            })
        );
    }

    #[test]
    fn zero_venue_position_rejects_with_rejected_clamp_evidence() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::ZERO,
        );
        let sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order(
            "O-19700101-000000-001-EXIT-REJECTED-1",
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let error = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::RiskReducingExit,
            intent,
            order,
        )
        .expect_err("zero venue position should reject before economics is sealed");
        super::record_order_intent(
            writer.as_ref(),
            BoltV3SubmitIntentKind::RiskReducingExit,
            error.intent().clone(),
        )
        .expect("rejected clamp intent should record");
        let error = error.into_error();

        assert!(
            error
                .to_string()
                .contains("no venue-held position to submit"),
            "unexpected clamp rejection: {error:#}"
        );
        assert_eq!(sink.submit_calls, 0);
        let records = writer.order_intents();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].clamp_outcome,
            Some(OrderIntentClampOutcome::Rejected)
        );
    }

    #[test]
    fn kill_switch_forced_reduction_clamps_to_venue_position() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission =
            provider_collateral_allowance_admission_with_yes_position(writer, Decimal::new(3, 0));
        let order = limit_exit_order(
            "O-19700101-000000-001-FORCED-CLAMP-1",
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::KillSwitchForcedReduction,
            intent,
            order,
        )
        .expect("forced reduction should share the venue-position clamp");
        let mut request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );
        request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
        request.risk_reducing_exit_proof = None;

        assert_eq!(
            request.intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
        assert_eq!(order.quantity(), Quantity::new(3.0, 2));
        assert_eq!(request.order_quantity, Decimal::new(3, 0));
        assert_eq!(request.notional, Decimal::new(15, 1));
        assert_eq!(
            request
                .admission_evidence
                .as_ref()
                .expect("admission evidence should remain attached")
                .quantity,
            Decimal::new(3, 0)
        );
        assert_eq!(
            intent.clamp_outcome,
            Some(OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            })
        );
    }

    #[test]
    fn kill_switch_forced_reduction_within_venue_position_records_within_bounds() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission =
            provider_collateral_allowance_admission_with_yes_position(writer, Decimal::new(8, 0));
        let order = limit_exit_order(
            "O-19700101-000000-001-FORCED-WITHIN-1",
            Quantity::new(3.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let (intent, order) = clamp_risk_reducing_exit_to_venue_position(
            admission.as_ref(),
            BoltV3SubmitIntentKind::KillSwitchForcedReduction,
            intent,
            order,
        )
        .expect("forced reduction within venue position should pass unchanged");
        let mut request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );
        request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
        request.risk_reducing_exit_proof = None;

        assert_eq!(
            request.intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
        assert_eq!(order.quantity(), Quantity::new(3.0, 2));
        assert_eq!(request.order_quantity, Decimal::new(3, 0));
        assert_eq!(
            intent.clamp_outcome,
            Some(OrderIntentClampOutcome::WithinBounds)
        );
    }

    #[test]
    fn kill_switch_flatten_command_routes_forced_reduction_through_clamped_submit() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );
        admission.replace_kill_switch_state(KillSwitchState::Flattening {
            halt_id: "halt-001".to_string(),
        });
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 2, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
        reconcile_no_live_forced_reductions(admission.as_ref(), 2);
        let instrument = binary_option_with_max_price(InstrumentId::from("instrument-yes.VENUE-A"));
        let claim = BoltV3KillSwitchForcedReductionClaim::new(
            "halt-001",
            "flatten-positions",
            "a".repeat(64),
        )
        .expect("forced reduction claim should be valid");
        let candidate = BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
            BoltV3KillSwitchFlattenPositionState {
                evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                account_id: AccountId::from("ACCOUNT-001"),
                instrument_id: InstrumentId::from("instrument-yes.VENUE-A"),
                strategy_id: StrategyId::from("strategy-a"),
                position_id: PositionId::from("POSITION-001"),
                position_side: PositionSide::Long,
                quantity: Quantity::new(5.0, 2),
                source_timestamp_unix_nanos: 1,
            },
        )
        .expect("flatten candidate should be valid");
        let plan =
            BoltV3KillSwitchFlattenSupervisor::plan_flatten(BoltV3KillSwitchFlattenPlanRequest {
                kill_switch_state: KillSwitchState::Flattening {
                    halt_id: "halt-001".to_string(),
                },
                nt_trading_state: TradingState::Reducing,
                action_id: "flatten-positions".to_string(),
                config_sha256: "b".repeat(64),
                policy_sha256: "a".repeat(64),
                source_timestamp_unix_nanos: 2,
                policy: BoltV3KillSwitchFlattenPolicy::new(),
                snapshot: BoltV3KillSwitchFlattenSnapshot::new(vec![candidate])
                    .expect("flatten snapshot should be valid"),
                observed_at_unix_nanos: 2,
                route_proof: BoltV3KillSwitchFlattenRouteProof::new(
                    BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
                ),
                order_template: flatten_market_template(),
                forced_reduction_claim: claim,
            })
            .expect("flatten plan should produce commands");
        let command = plan
            .commands()
            .first()
            .expect("open position should produce a flatten command");

        let mut sink = RecordingVenueMutationSink {
            actor_times_ns: std::collections::VecDeque::from([2, 2]),
            ..RecordingVenueMutationSink::default()
        };
        let mut order_factory = generic_order_factory();
        let outcome = route_kill_switch_flatten_command_with_sink(
            BoltV3OrderExecutionPolicy::live(),
            &mut sink,
            &mut order_factory,
            writer.as_ref(),
            admission.as_ref(),
            super::BoltV3KillSwitchFlattenRoutingContext {
                execution_client_id: "execution_client",
                fallback_price: "1",
                instrument: Some(&instrument),
                order_economics: kill_switch_order_economics(),
            },
            command,
        )
        .expect("flatten command should route through submit policy");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(3.0, 2)]);
        assert_eq!(admission.admitted_order_count(), 1);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(
            writer.order_intents()[0].clamp_outcome,
            Some(OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            })
        );
        assert_eq!(writer.forced_reduction_admissions().len(), 1);
    }

    #[test]
    fn kill_switch_flatten_command_rejects_zero_venue_position_with_clamp_evidence() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::ZERO,
        );
        admission.replace_kill_switch_state(KillSwitchState::Flattening {
            halt_id: "halt-001".to_string(),
        });
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 2, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
        reconcile_no_live_forced_reductions(admission.as_ref(), 2);
        let instrument = binary_option_with_max_price(InstrumentId::from("instrument-yes.VENUE-A"));
        let claim = BoltV3KillSwitchForcedReductionClaim::new(
            "halt-001",
            "flatten-positions",
            "a".repeat(64),
        )
        .expect("forced reduction claim should be valid");
        let candidate = BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
            BoltV3KillSwitchFlattenPositionState {
                evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                account_id: AccountId::from("ACCOUNT-001"),
                instrument_id: InstrumentId::from("instrument-yes.VENUE-A"),
                strategy_id: StrategyId::from("strategy-a"),
                position_id: PositionId::from("POSITION-001"),
                position_side: PositionSide::Long,
                quantity: Quantity::new(5.0, 2),
                source_timestamp_unix_nanos: 1,
            },
        )
        .expect("flatten candidate should be valid");
        let plan =
            BoltV3KillSwitchFlattenSupervisor::plan_flatten(BoltV3KillSwitchFlattenPlanRequest {
                kill_switch_state: KillSwitchState::Flattening {
                    halt_id: "halt-001".to_string(),
                },
                nt_trading_state: TradingState::Reducing,
                action_id: "flatten-positions".to_string(),
                config_sha256: "b".repeat(64),
                policy_sha256: "a".repeat(64),
                source_timestamp_unix_nanos: 2,
                policy: BoltV3KillSwitchFlattenPolicy::new(),
                snapshot: BoltV3KillSwitchFlattenSnapshot::new(vec![candidate])
                    .expect("flatten snapshot should be valid"),
                observed_at_unix_nanos: 2,
                route_proof: BoltV3KillSwitchFlattenRouteProof::new(
                    BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
                ),
                order_template: flatten_market_template(),
                forced_reduction_claim: claim,
            })
            .expect("flatten plan should produce commands");
        let command = plan
            .commands()
            .first()
            .expect("open position should produce a flatten command");

        let mut sink = RecordingVenueMutationSink::default();
        let mut order_factory = generic_order_factory();
        let error = route_kill_switch_flatten_command_with_sink(
            BoltV3OrderExecutionPolicy::live(),
            &mut sink,
            &mut order_factory,
            writer.as_ref(),
            admission.as_ref(),
            super::BoltV3KillSwitchFlattenRoutingContext {
                execution_client_id: "execution_client",
                fallback_price: "1",
                instrument: Some(&instrument),
                order_economics: kill_switch_order_economics(),
            },
            command,
        )
        .expect_err("zero venue position should reject before flatten submit");

        assert!(
            error
                .to_string()
                .contains("no venue-held position to submit"),
            "unexpected flatten clamp rejection: {error:#}"
        );
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(
            writer.order_intents()[0].clamp_outcome,
            Some(OrderIntentClampOutcome::Rejected)
        );
        assert_eq!(
            writer.order_intents()[0].client_order_id,
            "halt-001-flatten-positions-POSITION-001"
        );
    }

    #[test]
    fn two_halt_cycles_require_reconciled_terminal_absence_before_second_submit() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 1, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
        reconcile_no_live_forced_reductions(admission.as_ref(), 2);
        let instrument = binary_option_with_max_price(InstrumentId::from("instrument-yes.VENUE-A"));
        let mut feed = CapitalAdmissionRuntimeFeed::new(
            capital_admission_runtime_feed_config(),
            admission.clone(),
        );
        let mut sink = RecordingVenueMutationSink::default();
        let mut order_factory = generic_order_factory();

        let first = flatten_command_for_halt("halt-001", "POSITION-001");
        route_one_flatten_command(
            admission.as_ref(),
            writer.as_ref(),
            &instrument,
            &mut sink,
            &mut order_factory,
            &first,
        )
        .expect("first halt should submit a clamped forced reduction");
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(3.0, 2)]);

        let first_terminal = OrderEventAny::Canceled(order_canceled_event(
            "halt-001-flatten-positions-POSITION-001",
            1_100,
        ));
        assert!(
            feed.on_order_event(&first_terminal).is_none(),
            "terminal callbacks must not mutate canonical forced-reduction liveness"
        );
        reconcile_no_live_forced_reductions(admission.as_ref(), 1_101);
        admission.replace_kill_switch_state(KillSwitchState::Armed);

        let second = flatten_command_for_halt("halt-002", "POSITION-002");
        route_one_flatten_command(
            admission.as_ref(),
            writer.as_ref(),
            &instrument,
            &mut sink,
            &mut order_factory,
            &second,
        )
        .expect(
            "second halt should submit after NT reconciliation proves the first order is no longer live",
        );

        assert_eq!(
            sink.submitted_order_quantities,
            vec![Quantity::new(3.0, 2), Quantity::new(3.0, 2)]
        );
        let records = writer.order_intents();
        assert_eq!(
            records.last().map(|record| record.clamp_outcome.clone()),
            Some(Some(OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            }))
        );
    }

    #[test]
    fn shadow_submit_records_evidence_without_consuming_capacity_or_calling_nt_submit() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_order("O-19700101-000000-001-SHADOW-1");
        let intent = intent_for_order(&order);
        let request = submit_request_for_order(&order, Decimal::new(50, 0));
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Shadow);

        let outcome = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::for_test(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                    &order,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("shadow submit should still evaluate admission");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::SkippedByPolicy);
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 0);
    }

    #[test]
    fn live_and_shadow_cancel_route_through_the_same_policy_boundary() {
        let mut sink = RecordingVenueMutationSink::default();
        let live_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);
        let shadow_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Shadow);

        let live_outcome = live_policy
            .route_cancel_with_sink(
                &mut sink,
                ClientOrderId::from("O-19700101-000000-001-CANCEL-1"),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("live cancel should call NT");
        let shadow_outcome = shadow_policy
            .route_cancel_with_sink(
                &mut sink,
                ClientOrderId::from("O-19700101-000000-001-CANCEL-2"),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("shadow cancel should be suppressed by policy");

        assert_eq!(live_outcome, BoltV3CancelRoutingOutcome::Canceled);
        assert_eq!(shadow_outcome, BoltV3CancelRoutingOutcome::SkippedByPolicy);
        assert_eq!(sink.cancel_calls, 1);
    }

    #[test]
    fn live_and_shadow_cancel_all_route_through_the_same_policy_boundary() {
        let mut live_sink = RecordingVenueMutationSink::default();
        let mut shadow_sink = RecordingVenueMutationSink::default();
        let instrument_id = InstrumentId::from("instrument-yes.VENUE-A");

        // Hold every request field constant so only execution mode can explain the differential.
        // A counterfeit implementation that routes by side must therefore fail this test.
        let live_outcome = BoltV3OrderExecutionPolicy::live()
            .route_cancel_all_with_sink(
                &mut live_sink,
                instrument_id,
                Some(OrderSide::Buy),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("live cancel-all should call NT");
        let shadow_outcome = BoltV3OrderExecutionPolicy::shadow()
            .route_cancel_all_with_sink(
                &mut shadow_sink,
                instrument_id,
                Some(OrderSide::Buy),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("shadow cancel-all should be suppressed by policy");

        assert_eq!(live_outcome, BoltV3CancelAllRoutingOutcome::CanceledAll);
        assert_eq!(
            shadow_outcome,
            BoltV3CancelAllRoutingOutcome::SkippedByPolicy
        );
        assert_eq!(live_sink.cancel_all_calls, 1);
        assert_eq!(
            live_sink.cancel_all_requests,
            vec![(
                instrument_id,
                Some(OrderSide::Buy),
                Some(ClientId::from("execution_client")),
            )]
        );
        assert_eq!(shadow_sink.cancel_all_calls, 0);
        assert!(shadow_sink.cancel_all_requests.is_empty());
    }

    #[test]
    fn live_modify_is_fail_closed_and_shadow_is_suppressed() {
        // Option A (#835): an in-place modify does NOT pass submit admission, so a
        // Live amend would bypass the risk gate. The Live arm is FAIL-CLOSED — it
        // returns `Err` and never reaches the venue; Shadow stays suppressed
        // (`SkippedByPolicy`, no NT call). Asserts through the recording sink's
        // `modify_calls` side-effect channel (stays 0 for BOTH arms), not just the
        // return value — forcing the Live arm back to a venue call turns this red.
        let mut sink = RecordingVenueMutationSink::default();
        let live_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);
        let shadow_policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Shadow);

        let live_result = live_policy.route_modify_with_sink(
            &mut sink,
            ClientOrderId::from("O-19700101-000000-001-MODIFY-1"),
            Quantity::new(2.0, 2),
            Price::new(0.41, 2),
            Some(ClientId::from("execution_client")),
            None,
        );
        let shadow_outcome = shadow_policy
            .route_modify_with_sink(
                &mut sink,
                ClientOrderId::from("O-19700101-000000-001-MODIFY-2"),
                Quantity::new(3.0, 2),
                Price::new(0.42, 2),
                Some(ClientId::from("execution_client")),
                None,
            )
            .expect("shadow modify should be suppressed by policy");

        assert!(
            live_result.is_err(),
            "live in-place modify must be fail-closed (not admission-gated; #835)"
        );
        assert_eq!(shadow_outcome, BoltV3ModifyRoutingOutcome::SkippedByPolicy);
        // Neither arm reached the venue: Live refused (fail-closed), Shadow suppressed.
        assert_eq!(sink.modify_calls, 0);
        assert!(sink.modify_requests.is_empty());
    }

    #[test]
    fn maker_modify_dispatch_is_fail_closed_in_live_not_admission_gated() {
        // Option A (#835): a compiled `Modify` routed Live is FAIL-CLOSED at the
        // execution seam — the dispatch returns `Err` and the venue modify is never
        // called (`modify_calls` stays 0), because an in-place modify does not pass
        // the submit admission/reservation/fee checks. The maker requotes via the
        // already-admitted cancel+resubmit path (the deployed venue contract has
        // `supports_modify=false`, so the FSM never emits a Modify). No venue mutation
        // occurs, so no intent/admission is recorded. Forcing the Live arm back to a
        // venue call turns this red.
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let command = MakerCompiledOrderCommand::Modify {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.INSTRUMENT"),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
            price: Price::new(0.41, 2),
            quantity: Quantity::new(2.0, 2),
        };

        let result = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        );

        assert!(
            result.is_err(),
            "live maker modify dispatch must be fail-closed (not admission-gated; #835)"
        );
        assert_eq!(runtime.venue_sink.modify_calls, 0);
        // No venue mutation → no order intent / admission recorded.
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
    }

    #[test]
    fn maker_modify_dispatch_in_shadow_suppresses_the_venue_modify() {
        // The Shadow arm of the same dispatch path: the dispatcher still reports the
        // `Modified` command shape, but the execution policy suppresses the venue
        // call, so `modify_calls` stays 0. Pre-fix the path bailed in BOTH modes; a
        // shadow run that leaked a venue modify (counter > 0) also fails here.
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
        let order_economics = crate::bolt_v3_economics_test_support::fixture_order_economics_for(
            "maker_execution_client",
        );
        let command = MakerCompiledOrderCommand::Modify {
            leg: Leg::No,
            instrument_id: InstrumentId::from("NO.INSTRUMENT"),
            client_order_id: ClientOrderId::from("MAKER-NO-1"),
            price: Price::new(0.39, 2),
            quantity: Quantity::new(1.0, 2),
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::shadow(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(&order_economics),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("maker modify should route in shadow without bailing");

        assert_eq!(
            outcome,
            MakerOrderDispatchOutcome::Modified {
                leg: Leg::No,
                instrument_id: InstrumentId::from("NO.INSTRUMENT"),
                client_order_id: ClientOrderId::from("MAKER-NO-1"),
                price: Price::new(0.39, 2),
                quantity: Quantity::new(1.0, 2),
            }
        );
        assert_eq!(
            runtime.venue_sink.modify_calls, 0,
            "shadow mode must not leak a venue modify"
        );
    }

    fn live_submit_cap() -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
        BTreeMap::from([(
            "execution_client".to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count: 1,
                max_order_notional: Decimal::new(100, 0),
            },
        )])
    }

    fn live_submit_cap_for_client(
        client_id: &str,
    ) -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
        BTreeMap::from([(
            client_id.to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count: 1,
                max_order_notional: Decimal::new(100, 0),
            },
        )])
    }

    fn intent_for_order(order: &OrderAny) -> OrderIntentDetails {
        order_intent_details_from_compiled_order(
            "strategy-a".to_string(),
            "0.50".to_string(),
            order,
        )
    }

    fn exit_intent_for_order(order: &OrderAny) -> OrderIntentDetails {
        order_intent_details_from_compiled_order(
            "strategy-a".to_string(),
            "0.50".to_string(),
            order,
        )
    }

    fn submit_request_for_order(
        order: &OrderAny,
        notional: Decimal,
    ) -> BoltV3SubmitAdmissionRequest {
        BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional,
            order_side: OrderSide::Buy,
            order_quantity: Decimal::new(1, 0),
            intent_kind: BoltV3SubmitIntentKind::Entry,
            risk_reducing_exit_proof: None,
            kill_switch_forced_reduction: None,
            admission_evidence: None,
        }
    }

    fn admission_evidence_submit_request_for_order(
        order: &OrderAny,
    ) -> BoltV3SubmitAdmissionRequest {
        let mut request = submit_request_for_order(order, Decimal::new(4, 0));
        request.admission_evidence = Some(BoltV3CompiledOrderAdmissionEvidence {
            venue_id: "VENUE-A".to_string(),
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side: BoltV3CompiledOrderSide::Buy,
            quantity: Decimal::new(1, 0),
            effective_price: Decimal::new(40, 2),
            order_kind: BoltV3CompiledOrderKind::Limit,
            liquidity: BoltV3CompiledOrderLiquidity::Taker,
            quote_set_id: None,
            prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
        });
        request.instrument_id = "instrument-yes.VENUE-A".to_string();
        request.execution_client_id = "execution-client-a".to_string();
        request
    }

    fn risk_reducing_exit_submit_request_for_order(
        order: &OrderAny,
        order_quantity: Decimal,
        position_quantity: Decimal,
    ) -> BoltV3SubmitAdmissionRequest {
        let notional = order
            .price()
            .expect("risk-reducing test order must have a limit price")
            .as_decimal()
            .checked_mul(order_quantity)
            .expect("risk-reducing test notional must not overflow");
        BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional,
            order_side: OrderSide::Sell,
            order_quantity,
            intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
            risk_reducing_exit_proof: Some(BoltV3RiskReducingExitProof {
                position_id: "POSITION-001".to_string(),
                instrument_id: order.instrument_id().to_string(),
                position_side: PositionSide::Long,
                exit_order_side: OrderSide::Sell,
                position_quantity,
                exit_quantity: order_quantity,
            }),
            kill_switch_forced_reduction: None,
            admission_evidence: Some(BoltV3CompiledOrderAdmissionEvidence {
                venue_id: "VENUE-A".to_string(),
                product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
                side: BoltV3CompiledOrderSide::Sell,
                quantity: order_quantity,
                effective_price: Decimal::new(50, 2),
                order_kind: BoltV3CompiledOrderKind::Limit,
                liquidity: BoltV3CompiledOrderLiquidity::Taker,
                quote_set_id: None,
                prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
            }),
        }
    }

    fn generic_order_factory() -> OrderFactory {
        let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
        OrderFactory::new(
            TraderId::new("TRADER-001"),
            StrategyId::new("maker-strategy"),
            None,
            None,
            clock,
            false,
            true,
        )
    }

    fn maker_limit_post_only_template() -> NtOrderTemplate {
        NtOrderTemplate {
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            expire_time: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: true,
            is_reduce_only: false,
            is_quote_quantity: false,
        }
    }

    fn flatten_market_template() -> NtOrderTemplate {
        NtOrderTemplate {
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            expire_time: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: false,
            is_reduce_only: true,
            is_quote_quantity: false,
        }
    }

    fn maker_routing_context(
        order_economics: &super::BoltV3OrderEconomicsHandle,
    ) -> BoltV3MakerOrderRoutingContext<'_> {
        BoltV3MakerOrderRoutingContext {
            strategy_id: "maker-strategy",
            execution_client_id: "maker_execution_client",
            order_economics,
            terminal_value_entry: Some(
                BoltV3TerminalValueEntry::try_new(Decimal::ONE, Decimal::ZERO)
                    .expect("maker terminal value should construct"),
            ),
        }
    }

    fn kill_switch_order_economics() -> &'static super::BoltV3OrderEconomicsHandle {
        static HANDLE: std::sync::OnceLock<super::BoltV3OrderEconomicsHandle> =
            std::sync::OnceLock::new();
        HANDLE.get_or_init(|| {
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client")
        })
    }

    fn capital_admission_config() -> BoltV3SubmitCapitalAdmissionConfig {
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "test-capital-pool".to_string(),
                observed_at_ns: 0,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: u64::MAX,
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
            },
        }
    }

    fn capital_admission_components() -> BoltV3SubmitCapitalAdmissionNtComponents {
        BoltV3SubmitCapitalAdmissionNtComponents {
            source: "nt_capital_admission_state".to_string(),
            observed_at_ns: 0,
            portfolio: PortfolioCapitalAdmissionSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            provider_collateral_allowance: ProviderCollateralAllowanceSnapshot {
                source: "nt_account_free_collateral".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                collateral_allowance: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 0,
                open_order_count: 0,
                all_open_orders_attributed: true,
            },
            product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
                PredictionMarketAdmissionSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns: 0,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::new(100, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            loss_snapshot: None,
        }
    }

    fn provider_collateral_allowance_admission_with_yes_position(
        writer: Arc<DecisionEvidenceRecorder>,
        yes_position: Decimal,
    ) -> Arc<BoltV3SubmitAdmissionState> {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer,
            capital_admission_config(),
        ));
        let mut components = capital_admission_components();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) =
            &mut components.product_state;
        product.source = POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string();
        product.yes_position = yes_position;
        admission.update_capital_admission_nt_components(components);
        let rebuild =
            admission.rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1);
        assert!(rebuild.accepted);
        admission
    }

    fn reconcile_no_live_forced_reductions(
        admission: &BoltV3SubmitAdmissionState,
        observed_at_ns: u64,
    ) {
        let rebuild = admission
            .rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), observed_at_ns);
        assert!(
            rebuild.accepted,
            "canonical NT open-order projection should reconcile forced-reduction liveness"
        );
    }

    fn capital_admission_runtime_feed_config() -> CapitalAdmissionRuntimeFeedConfig {
        CapitalAdmissionRuntimeFeedConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_currency: "USD".to_string(),
            product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
                PredictionMarketAdmissionSnapshot {
                    source: "bolt_configured_binary_product".to_string(),
                    observed_at_ns: 0,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::ZERO,
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
        }
    }

    fn flatten_command_for_halt(
        halt_id: &str,
        position_id: &str,
    ) -> crate::bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand {
        let claim =
            BoltV3KillSwitchForcedReductionClaim::new(halt_id, "flatten-positions", "a".repeat(64))
                .expect("forced reduction claim should be valid");
        let candidate = BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
            BoltV3KillSwitchFlattenPositionState {
                evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                account_id: AccountId::from("ACCOUNT-001"),
                instrument_id: InstrumentId::from("instrument-yes.VENUE-A"),
                strategy_id: StrategyId::from("strategy-a"),
                position_id: PositionId::from(position_id),
                position_side: PositionSide::Long,
                quantity: Quantity::new(5.0, 2),
                source_timestamp_unix_nanos: 1,
            },
        )
        .expect("flatten candidate should be valid");
        let plan =
            BoltV3KillSwitchFlattenSupervisor::plan_flatten(BoltV3KillSwitchFlattenPlanRequest {
                kill_switch_state: KillSwitchState::Flattening {
                    halt_id: halt_id.to_string(),
                },
                nt_trading_state: TradingState::Reducing,
                action_id: "flatten-positions".to_string(),
                config_sha256: "b".repeat(64),
                policy_sha256: "a".repeat(64),
                source_timestamp_unix_nanos: 2,
                policy: BoltV3KillSwitchFlattenPolicy::new(),
                snapshot: BoltV3KillSwitchFlattenSnapshot::new(vec![candidate])
                    .expect("flatten snapshot should be valid"),
                observed_at_unix_nanos: 2,
                route_proof: BoltV3KillSwitchFlattenRouteProof::new(
                    BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
                ),
                order_template: flatten_market_template(),
                forced_reduction_claim: claim,
            })
            .expect("flatten plan should produce commands");
        plan.commands()
            .first()
            .expect("open position should produce a command")
            .clone()
    }

    fn route_one_flatten_command(
        admission: &BoltV3SubmitAdmissionState,
        writer: &DecisionEvidenceRecorder,
        instrument: &InstrumentAny,
        sink: &mut RecordingVenueMutationSink,
        order_factory: &mut OrderFactory,
        command: &crate::bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand,
    ) -> Result<BoltV3SubmitRoutingOutcome> {
        admission.replace_kill_switch_state(KillSwitchState::Flattening {
            halt_id: command.halt_id().to_string(),
        });
        sink.actor_times_ns.extend([2, 2]);
        route_kill_switch_flatten_command_with_sink(
            BoltV3OrderExecutionPolicy::live(),
            sink,
            order_factory,
            writer,
            admission,
            super::BoltV3KillSwitchFlattenRoutingContext {
                execution_client_id: "execution_client",
                fallback_price: "1",
                instrument: Some(instrument),
                order_economics: kill_switch_order_economics(),
            },
            command,
        )
    }

    fn order_canceled_event(client_order_id: &str, ts_event: u64) -> OrderCanceled {
        OrderCanceled::new(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from("instrument-yes.VENUE-A"),
            ClientOrderId::from(client_order_id),
            nautilus_core::UUID4::new(),
            UnixNanos::from(ts_event),
            UnixNanos::from(ts_event),
            false,
            Some(VenueOrderId::from("venue-order-1")),
            Some(AccountId::from("ACCOUNT-001")),
        )
    }

    fn binary_option_with_max_price(instrument_id: InstrumentId) -> InstrumentAny {
        InstrumentAny::BinaryOption(BinaryOption::new(
            instrument_id,
            Symbol::from("instrument-yes"),
            AssetClass::Alternative,
            Currency::USD(),
            UnixNanos::from(1_u64),
            UnixNanos::from(2_u64),
            2,
            2,
            Price::from("0.01"),
            Quantity::from("0.01"),
            Some(Ustr::from("YES")),
            None,
            None,
            Some(Quantity::from("0.01")),
            None,
            None,
            Some(Price::from("1.00")),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            UnixNanos::from(1_u64),
            UnixNanos::from(1_u64),
        ))
    }

    fn limit_order(client_order_id: &str) -> OrderAny {
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
                false,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        )
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("post-only limit order should be valid"),
        )
    }

    #[test]
    fn edge_candidate_and_final_entry_share_terminal_value_scenario() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let terminal_value_entry =
            BoltV3TerminalValueEntry::try_new(Decimal::new(7, 1), Decimal::ZERO)
                .expect("terminal value should construct");
        let candidate_fill_levels = vec![BoltV3PlannedFillLeg {
            price: Decimal::new(5, 1),
            quantity: Decimal::ONE,
        }];
        let sizing = economics
            .quote_taker_sizing(BoltV3TakerEconomicsSizingInput {
                instrument_id: InstrumentId::from("INSTRUMENT.SOURCE"),
                order_side: OrderSide::Buy,
                planned_fill_legs: candidate_fill_levels.clone(),
                terminal_value_entry: terminal_value_entry.clone(),
                requested_at_ns: 1,
                decision_correlation_id: "edge-candidate",
            })
            .expect("candidate sizing should quote from terminal value");
        let order = limit_order("edge-final-entry");
        let intent = intent_for_order(&order);
        let sealed = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                    terminal_value_entry,
                ),
                candidate_fill_levels,
                requested_at_ns: 1,
                decision_correlation_id: "edge-final",
            },
        )
        .expect("final entry should seal from the same terminal value");

        assert_eq!(sizing.net_edge().gross_expected_value, Decimal::new(2, 1));
        assert_eq!(
            sealed.economics().net_edge().gross_expected_value,
            Decimal::new(2, 1)
        );
        assert_eq!(sealed.request().intent_kind, BoltV3SubmitIntentKind::Entry);
        assert_eq!(
            sealed.economics().request().lifecycle_path,
            LifecyclePath::HoldToRedemption
        );
        assert_eq!(
            sealed.economics().request().liquidity_role,
            LiquidityRole::Taker
        );
        assert_eq!(
            sealed.economics().purpose(),
            EconomicsAdmissionPurpose::TradingEdge
        );
        assert_eq!(
            sealed.economics().order_binding(),
            &economics_order_binding(&order).expect("final order should bind")
        );
    }

    #[test]
    fn maker_submit_derives_gross_from_terminal_value_and_final_order() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = post_only_limit_order("maker-terminal-entry");
        let intent = intent_for_order(&order);
        let sealed = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
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
                decision_correlation_id: "maker-terminal-entry",
            },
        )
        .expect("maker entry should seal from terminal value");

        assert_eq!(
            sealed.economics().net_edge().gross_expected_value,
            Decimal::new(2, 1)
        );
        assert_eq!(
            sealed.economics().request().liquidity_role,
            LiquidityRole::GuaranteedMaker
        );
        assert_eq!(sealed.request().intent_kind, BoltV3SubmitIntentKind::Entry);
        assert_eq!(
            sealed.economics().request().lifecycle_path,
            LifecyclePath::HoldToRedemption
        );
        assert_eq!(
            sealed.economics().purpose(),
            EconomicsAdmissionPurpose::TradingEdge
        );
    }

    #[test]
    fn forced_reduction_derives_zero_gross_and_risk_reduction_purpose() {
        let economics =
            crate::bolt_v3_economics_test_support::fixture_order_economics_for("execution_client");
        let order = limit_exit_order("forced-reduction-scenario", Quantity::new(1.0, 2));
        let intent = exit_intent_for_order(&order);
        let position = economics
            .planned_exit_position(
                PositionId::from("POSITION-001"),
                PositionSide::Long,
                Decimal::ONE,
            )
            .expect("forced-reduction position should construct");
        let sealed = build_order_economics_submit_admission(
            &economics,
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: "execution_client",
                intent: &intent,
                order: &order,
                valuation: OrderValuationContext::empty(),
                risk_reducing_exit_position: None,
                scenario: BoltV3FinalOrderEconomicsScenario::forced_reduction(position)
                    .expect("forced-reduction scenario should construct"),
                candidate_fill_levels: vec![BoltV3PlannedFillLeg {
                    price: Decimal::new(5, 1),
                    quantity: Decimal::ONE,
                }],
                requested_at_ns: 1,
                decision_correlation_id: "forced-reduction-scenario",
            },
        )
        .expect("forced reduction should seal without caller-selected gross or lifecycle");

        assert_eq!(
            sealed.economics().net_edge().gross_expected_value,
            Decimal::ZERO
        );
        assert_eq!(
            sealed.request().intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
        assert_eq!(
            sealed.economics().request().lifecycle_path,
            LifecyclePath::PlannedExit
        );
        assert_eq!(
            sealed.economics().purpose(),
            EconomicsAdmissionPurpose::RiskReduction
        );
    }

    fn limit_exit_order(client_order_id: &str, quantity: Quantity) -> OrderAny {
        limit_exit_order_for_instrument(
            client_order_id,
            InstrumentId::from("instrument-yes.VENUE-A"),
            quantity,
        )
    }

    fn limit_exit_order_for_instrument(
        client_order_id: &str,
        instrument_id: InstrumentId,
        quantity: Quantity,
    ) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                instrument_id,
                ClientOrderId::from(client_order_id),
                OrderSide::Sell,
                quantity,
                Price::new(0.50, 2),
                TimeInForce::Gtc,
                None,
                false,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit exit order should be valid"),
        )
    }

    #[test]
    fn economics_order_binding_changes_when_the_final_quantity_changes() {
        let original = limit_order("economics-binding");
        let original_binding = economics_order_binding(&original)
            .expect("the original order should have a canonical binding");
        let mut changed = original.clone();
        changed.set_quantity(Quantity::new(0.5, 2));
        changed.set_leaves_qty(Quantity::new(0.5, 2));

        assert_ne!(
            economics_order_binding(&changed)
                .expect("the changed order should have a canonical binding"),
            original_binding
        );
    }
}
