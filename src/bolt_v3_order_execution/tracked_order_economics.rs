use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use anyhow::Result;
use nautilus_common::actor::DataActorNative;
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
    BoltV3OrderExecutionPolicy, BoltV3SubmitRoutingOutcome, BoltV3TakerEconomicsSizingInput,
    NtStrategyVenueMutationSink, economics_basis::seal_final_order_economics_basis,
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
    cancellation: TrackedOrderCancellation,
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
    ) -> Result<()>
    where
        F: FnOnce() -> Result<BoltV3SubmitRoutingOutcome>,
    {
        if !policy.allows_venue_mutation() {
            return route().map(|_| ());
        }
        let client_order_id = order.client_order_id();
        let [leg] = admission.request().planned_fill_legs.as_slice() else {
            anyhow::bail!("resting economics registration requires exactly one planned fill leg");
        };
        anyhow::ensure!(
            leg.quantity > Decimal::ZERO,
            "resting economics registration requires positive quantity"
        );
        let authorized_quantity_ceiling = leg.quantity;
        {
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
                    cancellation: TrackedOrderCancellation::new(order),
                },
            );
        }

        match route() {
            Ok(BoltV3SubmitRoutingOutcome::Submitted) => Ok(()),
            Ok(_) => {
                self.remove_provisional_registration(client_order_id)?;
                anyhow::bail!("resting economics registration state mismatch")
            }
            Err(error) => {
                self.remove_provisional_registration(client_order_id)?;
                Err(error)
            }
        }
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

    fn remove_provisional_registration(&self, client_order_id: ClientOrderId) -> Result<()> {
        self.tracked_orders
            .write()
            .map_err(|_| anyhow::anyhow!("resting economics state lock poisoned"))?
            .remove(&client_order_id);
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
        BoltV3OrderExecutionPolicy, BoltV3SubmitRoutingOutcome,
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
        let intent = order_intent_details_from_compiled_order(
            "strategy-a".to_string(),
            "0.50".to_string(),
            &order,
        );
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
                decision_correlation_id: "maker-reentrant-submit",
            },
        )
        .expect("maker economics should seal");
        let callback_order = order.clone();

        economics
            .route_resting_submit(
                BoltV3OrderExecutionPolicy::live(),
                order,
                sealed.economics().clone(),
                || {
                    economics.reconcile_tracked_order_at(
                        client_order_id,
                        Some(callback_order),
                        1,
                    )?;
                    Ok(BoltV3SubmitRoutingOutcome::Submitted)
                },
            )
            .expect("a synchronous callback must not deadlock the submit transaction");

        assert_eq!(
            economics.resting_order_ids().unwrap(),
            vec![client_order_id]
        );
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
