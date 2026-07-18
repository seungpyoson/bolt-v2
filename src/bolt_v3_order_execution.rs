use std::{any::type_name, cell::RefMut, collections::BTreeMap, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use nautilus_common::{
    factories::OrderFactory,
    messages::execution::{
        BatchCancelOrders, CancelAllOrders, CancelOrder, ModifyOrder, SubmitOrderList,
    },
};
use nautilus_core::Params;
use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId},
    orders::{Order, OrderAny, OrderList},
    types::{Price, Quantity},
};
use nautilus_trading::{Strategy, StrategyNative};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_capital_admission::ProductAdmissionSnapshot,
    bolt_v3_capital_admission_state::capital_admission_source_is_accepted_venue_truth,
    bolt_v3_decision_evidence::{
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentClampNotEvaluatedReason,
        BoltV3OrderIntentClampOutcome, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
        BoltV3OrderIntentOrderFields,
    },
    bolt_v3_economics_config::EconomicsRoutingAttachmentPolicy,
    bolt_v3_economics_runtime::{
        EconomicsAdmission, EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
    },
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        dispatch_maker_order_command,
    },
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionRequestInput,
        BoltV3SubmitAdmissionState, BoltV3SubmitLifecyclePolicy,
        build_submit_admission_request_from_order, order_economics_facts,
    },
    economics::{
        AccountId as EconomicsAccountId, EdgeBasisPolicyId,
        ExecutionClientId as EconomicsExecutionClientId, InstrumentId as EconomicsInstrumentId,
        LifecyclePath, LiquidityRoleAssumption, NativeUnitId, PositionContext, ProductSurfaceId,
        ReportingPolicyId,
    },
    integrations::nautilus::economics::{
        NtEconomicsIntent, canonical_quote_request_from_nt, economics_order_binding,
    },
};

#[cfg(test)]
use nautilus_model::instruments::InstrumentAny;

#[cfg(test)]
use crate::{
    bolt_v3_order_intent::{NtOrderBuildInputs, build_nt_order},
    bolt_v3_submit_admission::BoltV3SubmitIntentKind,
};

#[derive(Clone)]
pub struct BoltV3OrderRoutingHandle {
    source: Arc<dyn EconomicsAdmissionSource>,
    execution_client_id: EconomicsExecutionClientId,
    account_id: EconomicsAccountId,
    product_surface_routes: BTreeMap<ProductSurfaceId, (EdgeBasisPolicyId, BoltV3CarryPlan)>,
    reporting_policy_id: ReportingPolicyId,
    reporting_unit: NativeUnitId,
    routing_attachment_policy: EconomicsRoutingAttachmentPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3CarryPlan {
    NoCarry,
    Required,
}

pub struct BoltV3OrderEconomicsIntent<'a> {
    pub request: &'a BoltV3SubmitAdmissionRequestInput<'a>,
    pub planned_fill_legs: Vec<BoltV3PlannedFillLeg>,
    pub liquidity_role: LiquidityRoleAssumption,
    pub position: Option<PositionContext>,
    pub lifecycle_path: LifecyclePath,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
    pub gross_expected_value: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3PlannedFillLeg {
    pub price: Decimal,
    pub quantity: Decimal,
}

pub struct BoltV3OrderRoutingConfig<'a> {
    pub execution_client_id: &'a str,
    pub account_id: &'a str,
    pub product_surface_id: &'a str,
    pub reporting_policy_id: &'a str,
    pub reporting_unit: &'a str,
    pub edge_basis_policy_id: &'a str,
    pub carry_plan: BoltV3CarryPlan,
    pub routing_attachment_policy: EconomicsRoutingAttachmentPolicy,
}

pub struct BoltV3ProductSurfaceRoute<'a> {
    pub product_surface_id: &'a str,
    pub edge_basis_policy_id: &'a str,
    pub carry_plan: BoltV3CarryPlan,
}

pub struct BoltV3MultiSurfaceOrderRoutingConfig<'a> {
    pub execution_client_id: &'a str,
    pub account_id: &'a str,
    pub product_surface_routes: Vec<BoltV3ProductSurfaceRoute<'a>>,
    pub reporting_policy_id: &'a str,
    pub reporting_unit: &'a str,
    pub routing_attachment_policy: EconomicsRoutingAttachmentPolicy,
}

impl BoltV3OrderRoutingHandle {
    pub fn new(
        source: Arc<dyn EconomicsAdmissionSource>,
        config: BoltV3OrderRoutingConfig<'_>,
    ) -> anyhow::Result<Self> {
        Self::new_with_product_surfaces(
            source,
            BoltV3MultiSurfaceOrderRoutingConfig {
                execution_client_id: config.execution_client_id,
                account_id: config.account_id,
                product_surface_routes: vec![BoltV3ProductSurfaceRoute {
                    product_surface_id: config.product_surface_id,
                    edge_basis_policy_id: config.edge_basis_policy_id,
                    carry_plan: config.carry_plan,
                }],
                reporting_policy_id: config.reporting_policy_id,
                reporting_unit: config.reporting_unit,
                routing_attachment_policy: config.routing_attachment_policy,
            },
        )
    }

    pub fn new_with_product_surfaces(
        source: Arc<dyn EconomicsAdmissionSource>,
        config: BoltV3MultiSurfaceOrderRoutingConfig<'_>,
    ) -> anyhow::Result<Self> {
        let mut product_surface_routes = BTreeMap::new();
        for route in config.product_surface_routes {
            let product_surface_id = ProductSurfaceId::new(route.product_surface_id)?;
            let edge_basis_policy_id = EdgeBasisPolicyId::new(route.edge_basis_policy_id)?;
            anyhow::ensure!(
                product_surface_routes
                    .insert(product_surface_id, (edge_basis_policy_id, route.carry_plan),)
                    .is_none(),
                "economics product surface route is duplicated"
            );
        }
        anyhow::ensure!(
            !product_surface_routes.is_empty(),
            "economics requires at least one product surface route"
        );
        Ok(Self {
            source,
            execution_client_id: EconomicsExecutionClientId::new(config.execution_client_id)?,
            account_id: EconomicsAccountId::new(config.account_id)?,
            product_surface_routes,
            reporting_policy_id: ReportingPolicyId::new(config.reporting_policy_id)?,
            reporting_unit: NativeUnitId::new(config.reporting_unit)?,
            routing_attachment_policy: config.routing_attachment_policy,
        })
    }

    pub fn quote_admission(
        &self,
        intent: BoltV3OrderEconomicsIntent<'_>,
    ) -> anyhow::Result<EconomicsAdmission> {
        let facts = order_economics_facts(intent.request)?;
        anyhow::ensure!(
            self.execution_client_id.as_str() == intent.request.execution_client_id,
            "economics routing execution client does not match the final order route"
        );
        match intent.liquidity_role {
            LiquidityRoleAssumption::GuaranteedMaker => anyhow::ensure!(
                intent.request.order.is_post_only(),
                "guaranteed-maker economics requires a final post-only order"
            ),
            LiquidityRoleAssumption::Taker => anyhow::ensure!(
                !intent.request.order.is_post_only(),
                "economics liquidity-role assumption does not match final order"
            ),
        }
        let planned_fill_legs =
            normalize_planned_fill_legs(intent.request.order, facts, intent.planned_fill_legs)?;
        let instrument_id =
            EconomicsInstrumentId::new(intent.request.order.instrument_id().to_string())?;
        let candidate_surfaces = self
            .product_surface_routes
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let product_surface_id = self.source.resolve_product_surface(
            &self.execution_client_id,
            &instrument_id,
            &candidate_surfaces,
        )?;
        let (edge_basis_policy_id, carry_plan) = self
            .product_surface_routes
            .get(&product_surface_id)
            .ok_or_else(|| anyhow::anyhow!("economics source selected an unconfigured surface"))?;
        let position = match carry_plan {
            BoltV3CarryPlan::NoCarry => {
                anyhow::ensure!(
                    intent.position.is_none(),
                    "non-carry product surface rejects position carry context"
                );
                None
            }
            BoltV3CarryPlan::Required => Some(intent.position.ok_or_else(|| {
                anyhow::anyhow!(
                    "carry product surface requires strategy-declared position and horizon"
                )
            })?),
        };
        let nt_planned_fill_legs = planned_fill_legs
            .iter()
            .map(|leg| (leg.price, leg.quantity))
            .collect::<Vec<_>>();
        let request = canonical_quote_request_from_nt(NtEconomicsIntent {
            execution_client_id: self.execution_client_id.as_str(),
            account_id: nautilus_model::identifiers::AccountId::from(self.account_id.as_str()),
            instrument_id: intent.request.order.instrument_id(),
            product_surface_id: product_surface_id.as_str(),
            order_side: intent.request.order.order_side(),
            liquidity_role: intent.liquidity_role,
            planned_fill_legs: &nt_planned_fill_legs,
            routing_attachment_id: match self.routing_attachment_policy {
                EconomicsRoutingAttachmentPolicy::Forbidden => None,
            },
            position,
            lifecycle_path: intent.lifecycle_path,
            reporting_policy_id: self.reporting_policy_id.as_str(),
            reporting_unit: self.reporting_unit.as_str(),
            edge_basis_policy_id: edge_basis_policy_id.as_str(),
            requested_at_ns: intent.requested_at_ns,
            decision_correlation_id: intent.decision_correlation_id,
        })
        .map_err(|error| anyhow::anyhow!("economics NT mapping failed: {error:?}"))?;
        self.source
            .quote_admission(EconomicsAdmissionQuoteIntent {
                request,
                order_binding: economics_order_binding(intent.request.order).map_err(|error| {
                    anyhow::anyhow!("economics order binding failed: {error:?}")
                })?,
                gross_expected_value: intent.gross_expected_value,
                base_reservation_notional: facts.base_reservation_notional,
            })
            .map_err(Into::into)
    }
}

fn normalize_planned_fill_legs(
    order: &OrderAny,
    facts: crate::bolt_v3_submit_admission::BoltV3OrderEconomicsFacts,
    legs: Vec<BoltV3PlannedFillLeg>,
) -> anyhow::Result<Vec<BoltV3PlannedFillLeg>> {
    anyhow::ensure!(!legs.is_empty(), "economics requires planned fill levels");
    let mut remaining = if order.is_quote_quantity() {
        facts.base_reservation_notional
    } else {
        facts.planned_fill_quantity
    };
    let mut normalized = Vec::new();
    for leg in legs {
        anyhow::ensure!(
            leg.price > Decimal::ZERO && leg.quantity > Decimal::ZERO,
            "economics planned fill level must have positive price and quantity"
        );
        let available = if order.is_quote_quantity() {
            leg.price
                .checked_mul(leg.quantity)
                .context("economics planned fill notional overflow")?
        } else {
            leg.quantity
        };
        let consumed = available.min(remaining);
        let quantity = if order.is_quote_quantity() {
            consumed
                .checked_div(leg.price)
                .context("economics planned fill quantity division failed")?
        } else {
            consumed
        };
        normalized.push(BoltV3PlannedFillLeg {
            price: leg.price,
            quantity,
        });
        remaining = remaining
            .checked_sub(consumed)
            .context("economics planned fill subtraction failed")?;
        if remaining.is_zero() {
            break;
        }
    }
    anyhow::ensure!(
        remaining.is_zero(),
        "economics planned fill levels do not cover the final order"
    );
    if order.price().is_some() {
        let within_limit = normalized.iter().all(|leg| match order.order_side() {
            OrderSide::Buy => leg.price <= facts.price,
            OrderSide::Sell => leg.price >= facts.price,
            _ => false,
        });
        anyhow::ensure!(
            within_limit,
            "economics planned fill level exceeds the final order limit"
        );
    }
    Ok(normalized)
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
        S: Strategy + StrategyNative + ?Sized,
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
        } = routing;
        let (intent, request, order) = match clamp_risk_reducing_exit_to_venue_position(
            submit_admission,
            intent,
            request,
            order,
        ) {
            Ok(clamped) => clamped,
            Err(error) => {
                decision_evidence.record_order_intent(error.intent())?;
                return Err(error.into_error());
            }
        };
        decision_evidence.record_order_intent(&intent)?;
        if economics_order_binding(&order)
            .map_err(|error| anyhow::anyhow!("economics order binding failed: {error:?}"))?
            != *request.economics_admission.order_binding()
        {
            return Err(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionError::EconomicsOrderMismatch
                    .into(),
            );
        }
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                let permit = submit_admission.admit(&request)?;
                sink.submit_order_via_nt(order, context)?;
                permit.commit_submitted();
                Ok(BoltV3SubmitRoutingOutcome::Submitted)
            }
            BoltV3OrderExecutionMode::Shadow => {
                submit_admission.evaluate_and_record_without_consuming_capacity(&request)?;
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
        S: Strategy + StrategyNative + ?Sized,
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
        S: Strategy + StrategyNative + ?Sized,
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
        // order.s notional past the configured submit limits a submit
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
        S: Strategy + StrategyNative + ?Sized,
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

fn clamp_risk_reducing_exit_to_venue_position(
    submit_admission: &BoltV3SubmitAdmissionState,
    mut intent: BoltV3OrderIntentEvidence,
    mut request: BoltV3SubmitAdmissionRequest,
    mut order: OrderAny,
) -> std::result::Result<
    (
        BoltV3OrderIntentEvidence,
        BoltV3SubmitAdmissionRequest,
        OrderAny,
    ),
    BoltV3ExitClampError,
> {
    if !request.intent_kind.is_venue_position_exit_clamp_eligible()
        || request.order_quantity <= Decimal::ZERO
    {
        return Ok((intent, request, order));
    }
    if request.order_side != OrderSide::Sell {
        intent.clamp_outcome = Some(BoltV3OrderIntentClampOutcome::NotEvaluated {
            reason: BoltV3OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
        });
        return Ok((intent, request, order));
    }
    let venue_position = match venue_truth_exit_position(submit_admission, &request) {
        VenueTruthExitPosition::Position(position) => position,
        VenueTruthExitPosition::NoVenueTruth => {
            intent.clamp_outcome = Some(BoltV3OrderIntentClampOutcome::NotEvaluated {
                reason: BoltV3OrderIntentClampNotEvaluatedReason::NoVenueTruth,
            });
            return Ok((intent, request, order));
        }
        VenueTruthExitPosition::ForeignInstrument => {
            intent.clamp_outcome = Some(BoltV3OrderIntentClampOutcome::NotEvaluated {
                reason: BoltV3OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            });
            return Ok((intent, request, order));
        }
    };
    if request.order_quantity <= venue_position {
        intent.clamp_outcome = Some(BoltV3OrderIntentClampOutcome::WithinBounds);
        return Ok((intent, request, order));
    }
    if venue_position <= Decimal::ZERO {
        return Err(rejected_exit_clamp(
            intent,
            anyhow::anyhow!(
                "risk-reducing exit rejected: no venue-held position to submit: instrument_id={}",
                request.instrument_id
            ),
        ));
    }

    let original_order_quantity = request.order_quantity;
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
                request.instrument_id
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
                request.instrument_id
            ),
        ));
    }

    order.set_quantity(clamped_quantity);
    order.set_leaves_qty(clamped_quantity);
    request.order_quantity = submitted_quantity;
    request.notional = match request
        .notional
        .checked_mul(submitted_quantity)
        .and_then(|notional| notional.checked_div(original_order_quantity))
    {
        Some(notional) => notional,
        None => {
            return Err(rejected_exit_clamp(
                intent,
                anyhow::anyhow!(
                    "risk-reducing exit clamped notional could not be derived: instrument_id={}",
                    request.instrument_id
                ),
            ));
        }
    };
    if let Some(proof) = request.risk_reducing_exit_proof.as_mut() {
        proof.position_quantity = venue_position;
        proof.exit_quantity = submitted_quantity;
    }
    if let Some(admission_evidence) = request.admission_evidence.as_mut() {
        admission_evidence.quantity = submitted_quantity;
    }
    intent.quantity = order.quantity().to_string();
    intent.clamp_outcome = Some(BoltV3OrderIntentClampOutcome::Clamped {
        original_quantity: original_order_quantity.to_string(),
    });
    intent.order_fields = BoltV3OrderIntentOrderFields::from_order(&order);

    Ok((intent, request, order))
}

#[derive(Debug)]
struct BoltV3ExitClampError {
    intent: Box<BoltV3OrderIntentEvidence>,
    error: anyhow::Error,
}

impl BoltV3ExitClampError {
    fn intent(&self) -> &BoltV3OrderIntentEvidence {
        self.intent.as_ref()
    }

    fn into_error(self) -> anyhow::Error {
        self.error
    }
}

fn rejected_exit_clamp(
    mut intent: BoltV3OrderIntentEvidence,
    error: anyhow::Error,
) -> BoltV3ExitClampError {
    intent.clamp_outcome = Some(BoltV3OrderIntentClampOutcome::Rejected);
    BoltV3ExitClampError {
        intent: Box::new(intent),
        error,
    }
}

enum VenueTruthExitPosition {
    Position(Decimal),
    NoVenueTruth,
    ForeignInstrument,
}

fn venue_truth_exit_position(
    submit_admission: &BoltV3SubmitAdmissionState,
    request: &BoltV3SubmitAdmissionRequest,
) -> VenueTruthExitPosition {
    let Some(state) = submit_admission.capital_admission_state_snapshot() else {
        return VenueTruthExitPosition::NoVenueTruth;
    };
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    if !capital_admission_source_is_accepted_venue_truth(&product.source) {
        return VenueTruthExitPosition::NoVenueTruth;
    }
    if request.instrument_id == product.yes_instrument_id {
        VenueTruthExitPosition::Position(product.yes_position)
    } else if request.instrument_id == product.no_instrument_id {
        VenueTruthExitPosition::Position(product.no_position)
    } else {
        VenueTruthExitPosition::ForeignInstrument
    }
}

fn floor_decimal_to_quantity_precision(value: Decimal, precision: u8) -> Result<Decimal> {
    Ok(value.round_dp_with_strategy(u32::from(precision), RoundingStrategy::ToZero))
}

pub struct BoltV3SubmitRoutingRequest<'a> {
    decision_evidence: &'a dyn BoltV3DecisionEvidenceWriter,
    submit_admission: &'a BoltV3SubmitAdmissionState,
    intent: BoltV3OrderIntentEvidence,
    request: BoltV3SubmitAdmissionRequest,
}

impl<'a> BoltV3SubmitRoutingRequest<'a> {
    pub fn new(
        decision_evidence: &'a dyn BoltV3DecisionEvidenceWriter,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: BoltV3OrderIntentEvidence,
        request: BoltV3SubmitAdmissionRequest,
    ) -> Self {
        Self {
            decision_evidence,
            submit_admission,
            intent,
            request,
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

#[derive(Clone, Copy)]
pub struct BoltV3MakerOrderRoutingContext<'a> {
    pub strategy_id: &'a str,
    pub execution_client_id: &'a str,
    pub order_routing: &'a BoltV3OrderRoutingHandle,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub struct BoltV3KillSwitchFlattenRoutingContext<'a> {
    pub execution_client_id: &'a str,
    pub fallback_price: &'a str,
    pub instrument: Option<&'a InstrumentAny>,
    pub order_routing: &'a BoltV3OrderRoutingHandle,
    pub gross_expected_value: Decimal,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[cfg(test)]
pub(crate) fn route_kill_switch_flatten_command_with_sink<S>(
    policy: BoltV3OrderExecutionPolicy,
    sink: &mut S,
    order_factory: &mut OrderFactory,
    decision_evidence: &dyn BoltV3DecisionEvidenceWriter,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3KillSwitchFlattenRoutingContext<'_>,
    command: &crate::bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand,
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
    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        command.strategy_id().to_string(),
        BoltV3OrderIntentKind::Exit,
        context.fallback_price.to_string(),
        &order,
    );
    let admission_input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: context.execution_client_id,
        intent: &intent,
        order: &order,
        valuation: crate::bolt_v3_submit_admission::OrderValuationContext {
            instrument: context.instrument,
            ..crate::bolt_v3_submit_admission::OrderValuationContext::empty()
        },
        lifecycle_policy: context.submit_lifecycle_policy,
        risk_reducing_exit_position: None,
    };
    let economics_admission =
        context
            .order_routing
            .quote_admission(BoltV3OrderEconomicsIntent {
                request: &admission_input,
                planned_fill_legs: {
                    let facts = order_economics_facts(&admission_input)?;
                    vec![BoltV3PlannedFillLeg {
                        price: facts.price,
                        quantity: facts.planned_fill_quantity,
                    }]
                },
                liquidity_role: LiquidityRoleAssumption::Taker,
                position: None,
                lifecycle_path: LifecyclePath::PlannedExit,
                requested_at_ns: order.ts_init().as_u64(),
                decision_correlation_id: order.client_order_id().as_str(),
                gross_expected_value: context.gross_expected_value,
            })?;
    let mut request =
        build_submit_admission_request_from_order(admission_input, economics_admission)?;
    request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
    request.risk_reducing_exit_proof = None;
    request.kill_switch_forced_reduction = Some(command.forced_reduction_claim().clone());

    policy.route_submit_with_sink(
        BoltV3SubmitRoutingRequest::new(decision_evidence, submit_admission, intent, request),
        sink,
        order,
        BoltV3SubmitContext::with_client_id_and_position_id(
            ClientId::from(context.execution_client_id),
            command.position_id(),
        ),
    )
}

#[cfg(test)]
fn flatten_client_order_id(
    command: &crate::bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand,
) -> ClientOrderId {
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
    decision_evidence: &dyn BoltV3DecisionEvidenceWriter,
    submit_admission: &BoltV3SubmitAdmissionState,
    context: BoltV3MakerOrderRoutingContext<'_>,
    input: MakerOrderDispatchInput<'_>,
) -> Result<MakerOrderDispatchOutcome>
where
    S: Strategy + StrategyNative + ?Sized,
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
}

#[cfg(test)]
pub(crate) struct BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    dispatch: F,
}

#[cfg(test)]
impl<F> BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    pub(crate) fn new(dispatch: F) -> Self {
        Self { dispatch }
    }
}

#[cfg(test)]
impl<F> BoltV3NtVenueMutationSink for BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        (self.dispatch)(order, context)
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
}

struct NtStrategyVenueMutationSink<'a, S>
where
    S: Strategy + StrategyNative + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyVenueMutationSink<'_, S>
where
    S: Strategy + StrategyNative + ?Sized,
{
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
}

trait BoltV3MakerOrderRuntime: BoltV3NtVenueMutationSink {
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory>;
}

struct NtStrategyMakerOrderRuntime<'a, S>
where
    S: Strategy + StrategyNative + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + StrategyNative + ?Sized,
{
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
}

impl<S> BoltV3MakerOrderRuntime for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + StrategyNative + ?Sized,
{
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.strategy.order_factory()
    }
}

fn route_maker_order_command_with_runtime<R>(
    policy: BoltV3OrderExecutionPolicy,
    runtime: &mut R,
    decision_evidence: &dyn BoltV3DecisionEvidenceWriter,
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
    decision_evidence: &'a dyn BoltV3DecisionEvidenceWriter,
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

    fn submit_maker_order(&mut self, order: OrderAny, gross_expected_value: f64) -> Result<()> {
        let fallback_price = order
            .price()
            .map(|price| price.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bolt-v3 maker submit requires a limit price for client_order_id={}",
                    order.client_order_id()
                )
            })?;
        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.context.strategy_id.to_string(),
            BoltV3OrderIntentKind::Entry,
            fallback_price,
            &order,
        );
        let admission_input = BoltV3SubmitAdmissionRequestInput {
            execution_client_id: self.context.execution_client_id,
            intent: &intent,
            order: &order,
            valuation: crate::bolt_v3_submit_admission::OrderValuationContext::empty(),
            lifecycle_policy: self.context.submit_lifecycle_policy,
            risk_reducing_exit_position: None,
        };
        let economics_admission =
            self.context
                .order_routing
                .quote_admission(BoltV3OrderEconomicsIntent {
                    request: &admission_input,
                    planned_fill_legs: {
                        let facts = order_economics_facts(&admission_input)?;
                        vec![BoltV3PlannedFillLeg {
                            price: facts.price,
                            quantity: facts.planned_fill_quantity,
                        }]
                    },
                    liquidity_role: LiquidityRoleAssumption::GuaranteedMaker,
                    position: None,
                    lifecycle_path: LifecyclePath::PlannedExit,
                    requested_at_ns: order.ts_init().as_u64(),
                    decision_correlation_id: order.client_order_id().as_str(),
                    gross_expected_value: Decimal::from_str(&gross_expected_value.to_string())?,
                })?;
        let request =
            build_submit_admission_request_from_order(admission_input, economics_admission)?;
        let submit_context =
            BoltV3SubmitContext::with_client_id(ClientId::from(self.context.execution_client_id));
        self.policy.route_submit_with_sink(
            BoltV3SubmitRoutingRequest::new(
                self.decision_evidence,
                self.submit_admission,
                intent,
                request,
            ),
            self.runtime,
            order,
            submit_context,
        )?;
        Ok(())
    }

    fn cancel_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    ) -> Result<()> {
        self.policy.route_cancel_with_sink(
            self.runtime,
            client_order_id,
            Some(ClientId::from(self.context.execution_client_id)),
            None,
        )?;
        Ok(())
    }

    fn cancel_all_maker_orders(
        &mut self,
        _leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<()> {
        self.policy.route_cancel_all_with_sink(
            self.runtime,
            instrument_id,
            order_side,
            Some(ClientId::from(self.context.execution_client_id)),
            None,
        )?;
        Ok(())
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
        collections::BTreeMap,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    use anyhow::Result;
    use nautilus_common::{
        clock::{Clock, TestClock},
        factories::OrderFactory,
    };
    use nautilus_core::Params;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::{AssetClass, OrderSide, OrderType, PositionSide, TimeInForce, TradingState},
        events::{OrderCanceled, OrderEventAny},
        identifiers::{
            AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, Symbol,
            TraderId, VenueOrderId,
        },
        instruments::{BinaryOption, InstrumentAny},
        orders::{LimitOrder, Order, OrderAny},
        types::{Currency, Price, Quantity},
    };
    use rust_decimal::Decimal;
    use ustr::Ustr;

    use super::{
        BoltV3CancelRoutingOutcome, BoltV3MakerOrderRoutingContext, BoltV3MakerOrderRuntime,
        BoltV3ModifyRoutingOutcome, BoltV3NtVenueMutationSink, BoltV3OrderExecutionMode,
        BoltV3OrderExecutionPolicy, BoltV3SubmitContext, BoltV3SubmitRoutingOutcome,
        BoltV3SubmitRoutingRequest, clamp_risk_reducing_exit_to_venue_position,
        route_kill_switch_flatten_command_with_sink, route_maker_order_command_with_runtime,
    };
    use crate::{
        bolt_v3_capital_admission::{
            CapitalAdmissionPolicy, PredictionMarketAdmissionSnapshot, ProductAdmissionSnapshot,
            ProductKind,
        },
        bolt_v3_capital_admission_runtime_feed::{
            CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
            POLYMARKET_VENUE_TRUTH_REST_SOURCE,
        },
        bolt_v3_capital_admission_state::{
            OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
            VenueSpendabilitySnapshot,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_decision_evidence::{
            BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
            BoltV3BasketAdmissionDecisionEvidence, BoltV3CapitalAdmissionRebuildAuditEvidence,
            BoltV3DecisionEvidenceWriter, BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence,
            BoltV3ExitEvaluationEvidence, BoltV3LossGovernorHaltEvidence,
            BoltV3OrderIntentClampNotEvaluatedReason, BoltV3OrderIntentClampOutcome,
            BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OrderRejectEvidence,
            BoltV3RequoteThrottleEvidence, BoltV3SettlementBookingErrorEvidence,
            BoltV3SettlementEvidence, BoltV3StrategyInputEvidenceSnapshot,
            BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
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
            BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy, PredictionMarketOutcomeSide,
        },
    };

    #[derive(Debug, Default)]
    struct RecordingDecisionEvidenceWriter {
        records: Mutex<Vec<BoltV3OrderIntentEvidence>>,
        admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
    }

    impl RecordingDecisionEvidenceWriter {
        fn records(&self) -> Vec<BoltV3OrderIntentEvidence> {
            self.records
                .lock()
                .expect("recording evidence mutex should not be poisoned")
                .clone()
        }

        fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
            self.admission_decisions
                .lock()
                .expect("recording admission mutex should not be poisoned")
                .clone()
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
    }

    impl BoltV3MakerOrderRuntime for RecordingMakerRuntime {
        fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
            self.order_factory.borrow_mut()
        }
    }

    #[test]
    fn maker_submit_routes_through_shared_execution_policy_and_admission() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap_for_client("maker_execution_client"),
        ));
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
            gross_expected_value: 0.02,
        };

        let outcome = route_maker_order_command_with_runtime(
            BoltV3OrderExecutionPolicy::live(),
            &mut runtime,
            writer.as_ref(),
            admission.as_ref(),
            maker_routing_context(),
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
        assert_eq!(writer.records().len(), 1);
        assert_eq!(writer.records()[0].strategy_id, "maker-strategy");
        assert_eq!(
            writer.records()[0].instrument_id,
            InstrumentId::from("YES.INSTRUMENT").to_string()
        );
        assert_eq!(writer.admission_decisions().len(), 1);
        assert_eq!(
            writer.admission_decisions()[0].outcome,
            BoltV3AdmissionOutcome::Admitted
        );
        assert_eq!(admission.admitted_order_count(), 1);
    }

    #[test]
    fn maker_cancel_routes_through_shared_execution_policy_with_configured_client() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
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
            maker_routing_context(),
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
        assert!(writer.records().is_empty());
        assert!(writer.admission_decisions().is_empty());
    }

    #[test]
    fn maker_cancel_all_routes_through_shared_execution_policy_with_configured_client() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
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
            maker_routing_context(),
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
        assert!(writer.records().is_empty());
        assert!(writer.admission_decisions().is_empty());
    }

    impl BoltV3DecisionEvidenceWriter for RecordingDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            self.records
                .lock()
                .expect("recording evidence mutex should not be poisoned")
                .push(intent.clone());
            Ok(())
        }

        fn record_admission_decision(
            &self,
            decision: &BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            self.admission_decisions
                .lock()
                .expect("recording admission mutex should not be poisoned")
                .push(decision.clone());
            Ok(())
        }

        fn record_basket_admission_decision(
            &self,
            _decision: &BoltV3BasketAdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_capital_admission_rebuild_audit(
            &self,
            _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> Result<()> {
            anyhow::bail!("recording order-execution writer received entry-skip evidence")
        }

        fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
            anyhow::bail!("recording order-execution writer received exit-decision evidence")
        }

        fn record_exit_evaluation(&self, _evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
            Ok(())
        }

        fn record_loss_governor_halt(
            &self,
            _evidence: &BoltV3LossGovernorHaltEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
            Ok(())
        }

        fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
            anyhow::bail!("recording order-execution writer received requote-throttle evidence")
        }

        fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> Result<()> {
            anyhow::bail!("recording order-execution writer received settlement evidence")
        }

        fn record_settlement_booking_error(
            &self,
            _evidence: &BoltV3SettlementBookingErrorEvidence,
        ) -> Result<()> {
            anyhow::bail!(
                "recording order-execution writer received settlement booking-error evidence"
            )
        }

        fn drain_shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct RecordingVenueMutationSink {
        submit_calls: usize,
        submitted_order_quantities: Vec<Quantity>,
        cancel_calls: usize,
        cancel_all_calls: usize,
        cancel_all_requests: Vec<(InstrumentId, Option<OrderSide>, Option<ClientId>)>,
        fail_submits: bool,
    }

    impl BoltV3NtVenueMutationSink for RecordingVenueMutationSink {
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
            Ok(())
        }

        fn cancel_order_via_nt(
            &mut self,
            _client_order_id: ClientOrderId,
            _client_id: Option<ClientId>,
            _params: Option<Params>,
        ) -> Result<()> {
            self.cancel_calls += 1;
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
    }

    #[test]
    fn live_submit_records_evidence_consumes_capacity_and_calls_nt_submit_once() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
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
                BoltV3SubmitRoutingRequest::new(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("live submit should route through NT");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submit_calls, 1);
        assert_eq!(writer.records().len(), 1);
        assert_eq!(writer.admission_decisions().len(), 1);
        assert_eq!(
            writer.admission_decisions()[0].outcome,
            BoltV3AdmissionOutcome::Admitted
        );
        assert_eq!(admission.admitted_order_count(), 1);
    }

    #[test]
    fn live_submit_failure_rolls_back_capital_admission_reservation() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer.clone(),
            capital_admission_config(),
        ));
        admission.update_capital_admission_nt_components(capital_admission_components());
        let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1);
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
            BoltV3SubmitRoutingRequest::new(writer.as_ref(), admission.as_ref(), intent, request),
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
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
    fn live_submit_rejects_price_change_after_economics_quote() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            writer.clone(),
            live_submit_cap(),
        ));
        let quoted_order = limit_order("O-19700101-000000-001-PRICE-BINDING-1");
        let final_order =
            limit_order_at("O-19700101-000000-001-PRICE-BINDING-1", Price::new(0.60, 2));
        let intent = intent_for_order(&quoted_order);
        let request = submit_request_for_order(&quoted_order, Decimal::new(50, 0));
        let mut sink = RecordingVenueMutationSink::default();

        let error = BoltV3OrderExecutionPolicy::live()
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::new(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                ),
                &mut sink,
                final_order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("a changed final price requires a fresh economics admission");

        assert_eq!(
            error.to_string(),
            "bolt-v3 submit admission final order no longer matches its sealed economics quote"
        );
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(admission.admitted_order_count(), 0);
    }

    #[test]
    fn live_risk_reducing_exit_rejects_when_clamp_invalidates_sealed_economics() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer.clone(), Decimal::new(3, 0));

        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order("O-19700101-000000-001-EXIT-CLAMP-1", Quantity::new(5.0, 2));
        let intent = exit_intent_for_order(&order);
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let error = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::new(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("a changed final quantity requires a fresh economics admission");

        assert_eq!(
            error.to_string(),
            "bolt-v3 submit admission final order no longer matches its sealed economics quote"
        );
        assert_eq!(sink.submit_calls, 0);
        assert!(sink.submitted_order_quantities.is_empty());
        assert_eq!(admission.admitted_order_count(), 0);
        let records = writer.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].quantity, Quantity::new(3.0, 2).to_string());
        assert_eq!(
            records[0].clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::Clamped {
                original_quantity: Decimal::new(5, 0).to_string(),
            })
        );
    }

    #[test]
    fn risk_reducing_exit_without_venue_truth_records_not_evaluated_reason() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
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
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let outcome = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::new(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("missing venue truth should pass through with explicit evidence");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::Submitted);
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(5.0, 2)]);
        let records = writer.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::NotEvaluated {
                reason: BoltV3OrderIntentClampNotEvaluatedReason::NoVenueTruth,
            })
        );
    }

    #[test]
    fn risk_reducing_exit_for_foreign_instrument_records_not_evaluated_reason() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer, Decimal::new(3, 0));
        let order = limit_exit_order_for_instrument(
            "O-19700101-000000-001-EXIT-FOREIGN-1",
            InstrumentId::from("instrument-foreign.VENUE-A"),
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );

        let (intent, request, order) =
            clamp_risk_reducing_exit_to_venue_position(admission.as_ref(), intent, request, order)
                .expect("foreign instrument should pass through with explicit evidence");

        assert_eq!(order.quantity(), Quantity::new(5.0, 2));
        assert_eq!(request.order_quantity, Decimal::new(5, 0));
        assert_eq!(
            intent.clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::NotEvaluated {
                reason: BoltV3OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            })
        );
    }

    #[test]
    fn clamp_eligible_non_sell_order_records_not_evaluated_reason() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer, Decimal::new(3, 0));
        let order = limit_order("O-19700101-000000-001-FLAT-BUY-1");
        let intent = exit_intent_for_order(&order);
        let mut request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );
        request.order_side = OrderSide::Buy;
        request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
        request.risk_reducing_exit_proof = None;
        if let Some(admission_evidence) = request.admission_evidence.as_mut() {
            admission_evidence.side = BoltV3CompiledOrderSide::Buy;
        }

        let (intent, request, order) =
            clamp_risk_reducing_exit_to_venue_position(admission.as_ref(), intent, request, order)
                .expect("non-Sell forced reduction should pass through with explicit evidence");

        assert_eq!(order.order_side(), OrderSide::Buy);
        assert_eq!(request.order_side, OrderSide::Buy);
        assert_eq!(
            intent.clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::NotEvaluated {
                reason: BoltV3OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
            })
        );
    }

    #[test]
    fn zero_venue_position_rejects_with_rejected_clamp_evidence() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer.clone(), Decimal::ZERO);
        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order(
            "O-19700101-000000-001-EXIT-REJECTED-1",
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );
        let policy = BoltV3OrderExecutionPolicy::from_mode(BoltV3OrderExecutionMode::Live);

        let error = policy
            .route_submit_with_sink(
                BoltV3SubmitRoutingRequest::new(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect_err("zero venue position should reject before venue submission");

        assert!(
            error
                .to_string()
                .contains("no venue-held position to submit"),
            "unexpected clamp rejection: {error:#}"
        );
        assert_eq!(sink.submit_calls, 0);
        let records = writer.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::Rejected)
        );
    }

    #[test]
    fn kill_switch_forced_reduction_clamps_to_venue_position() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer, Decimal::new(3, 0));
        let order = limit_exit_order(
            "O-19700101-000000-001-FORCED-CLAMP-1",
            Quantity::new(5.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let mut request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(5, 0),
            Decimal::new(5, 0),
        );
        request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
        request.risk_reducing_exit_proof = None;

        let (intent, request, order) =
            clamp_risk_reducing_exit_to_venue_position(admission.as_ref(), intent, request, order)
                .expect("forced reduction should share the venue-position clamp");

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
            Some(BoltV3OrderIntentClampOutcome::Clamped {
                original_quantity: Decimal::new(5, 0).to_string(),
            })
        );
    }

    #[test]
    fn kill_switch_forced_reduction_within_venue_position_records_within_bounds() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer, Decimal::new(8, 0));
        let order = limit_exit_order(
            "O-19700101-000000-001-FORCED-WITHIN-1",
            Quantity::new(3.0, 2),
        );
        let intent = exit_intent_for_order(&order);
        let mut request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );
        request.intent_kind = BoltV3SubmitIntentKind::KillSwitchForcedReduction;
        request.risk_reducing_exit_proof = None;

        let (intent, request, order) =
            clamp_risk_reducing_exit_to_venue_position(admission.as_ref(), intent, request, order)
                .expect("forced reduction within venue position should pass unchanged");

        assert_eq!(
            request.intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
        assert_eq!(order.quantity(), Quantity::new(3.0, 2));
        assert_eq!(request.order_quantity, Decimal::new(3, 0));
        assert_eq!(
            intent.clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::WithinBounds)
        );
    }

    #[test]
    fn kill_switch_flatten_rejects_when_clamp_invalidates_sealed_economics() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer.clone(), Decimal::new(3, 0));
        admission.replace_kill_switch_state(KillSwitchState::Flattening {
            halt_id: "halt-001".to_string(),
        });
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 2, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
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
                order_routing: Box::leak(Box::new(
                    crate::bolt_v3_economics_runtime::test_order_routing_handle("execution_client"),
                )),
                gross_expected_value: Decimal::ONE,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
            },
            command,
        )
        .expect_err("a clamped flatten requires a fresh economics admission");

        assert_eq!(
            error.to_string(),
            "bolt-v3 submit admission final order no longer matches its sealed economics quote"
        );
        assert_eq!(sink.submit_calls, 0);
        assert!(sink.submitted_order_quantities.is_empty());
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(writer.records().len(), 1);
        assert_eq!(
            writer.records()[0].clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::Clamped {
                original_quantity: Quantity::new(5.0, 2).as_decimal().to_string(),
            })
        );
        assert_eq!(writer.admission_decisions().len(), 1);
        assert_eq!(
            writer.admission_decisions()[0].intent_kind,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction
        );
    }

    #[test]
    fn kill_switch_flatten_command_rejects_zero_venue_position_with_clamp_evidence() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer.clone(), Decimal::ZERO);
        admission.replace_kill_switch_state(KillSwitchState::Flattening {
            halt_id: "halt-001".to_string(),
        });
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 2, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
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
                order_routing: Box::leak(Box::new(
                    crate::bolt_v3_economics_runtime::test_order_routing_handle("execution_client"),
                )),
                gross_expected_value: Decimal::ONE,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
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
        assert_eq!(writer.records().len(), 1);
        assert_eq!(
            writer.records()[0].clamp_outcome,
            Some(BoltV3OrderIntentClampOutcome::Rejected)
        );
        assert_eq!(
            writer.records()[0].client_order_id,
            "halt-001-flatten-positions-POSITION-001"
        );
    }

    #[test]
    fn two_halt_cycles_release_terminal_forced_reduction_and_second_submits() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = venue_truth_admission_with_yes_position(writer.clone(), Decimal::new(5, 0));
        admission.configure_kill_switch_forced_reduction_policy(
            BoltV3KillSwitchForcedReductionPolicy::new("a".repeat(64), 1, Decimal::new(10, 0))
                .expect("forced reduction policy should be valid"),
        );
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
        .expect("first halt should submit a forced reduction");
        assert_eq!(sink.submitted_order_quantities, vec![Quantity::new(5.0, 2)]);

        let first_terminal = OrderEventAny::Canceled(order_canceled_event(
            "halt-001-flatten-positions-POSITION-001",
            1_100,
        ));
        assert!(
            feed.on_order_event(&first_terminal).is_none(),
            "forced-reduction terminal release should not require capital-reservation ownership"
        );
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
            "second halt should submit after the first terminal releases the forced-reduction cap",
        );

        assert_eq!(
            sink.submitted_order_quantities,
            vec![Quantity::new(5.0, 2), Quantity::new(5.0, 2)]
        );
        let records = writer.records();
        assert_eq!(
            records.last().map(|record| record.clamp_outcome.clone()),
            Some(Some(BoltV3OrderIntentClampOutcome::WithinBounds))
        );
    }

    #[test]
    fn shadow_submit_records_evidence_without_consuming_capacity_or_calling_nt_submit() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
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
                BoltV3SubmitRoutingRequest::new(
                    writer.as_ref(),
                    admission.as_ref(),
                    intent,
                    request,
                ),
                &mut sink,
                order,
                BoltV3SubmitContext::with_client_id(ClientId::from("execution_client")),
            )
            .expect("shadow submit should still evaluate admission");

        assert_eq!(outcome, BoltV3SubmitRoutingOutcome::SkippedByPolicy);
        assert_eq!(sink.submit_calls, 0);
        assert_eq!(writer.records().len(), 1);
        assert_eq!(writer.admission_decisions().len(), 1);
        assert_eq!(
            writer.admission_decisions()[0].outcome,
            BoltV3AdmissionOutcome::Admitted
        );
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
    fn live_modify_is_fail_closed_and_shadow_is_suppressed() {
        // Option A (#835): an in-place modify does NOT pass submit admission, so a
        // Live amend would bypass the risk gate. The Live arm is FAIL-CLOSED — it
        // returns `Err` and never reaches the venue; Shadow stays suppressed
        // (`SkippedByPolicy`, no NT call). The venue sink exposes no modify operation,
        // so neither arm can acquire a venue-mutation path.
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
    }

    #[test]
    fn maker_modify_dispatch_is_fail_closed_in_live_not_admission_gated() {
        // Option A (#835): a compiled `Modify` routed Live is FAIL-CLOSED at the
        // execution seam — the dispatch returns `Err` because an in-place modify does not pass
        // the submit admission/reservation/fee checks. The maker requotes via the
        // already-admitted cancel+resubmit path (the deployed venue contract has
        // `supports_modify=false`, so the FSM never emits a Modify). No venue mutation
        // occurs, so no intent/admission is recorded. Forcing the Live arm back to a
        // venue call turns this red.
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
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
            maker_routing_context(),
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
            },
        );

        assert!(
            result.is_err(),
            "live maker modify dispatch must be fail-closed (not admission-gated; #835)"
        );
        // No venue mutation → no order intent / admission recorded.
        assert!(writer.records().is_empty());
        assert!(writer.admission_decisions().is_empty());
    }

    #[test]
    fn maker_modify_dispatch_in_shadow_suppresses_the_venue_modify() {
        // The Shadow arm of the same dispatch path: the dispatcher still reports the
        // `Modified` command shape, but the execution policy suppresses the venue
        // call. The venue sink exposes no modify operation.
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut runtime = RecordingMakerRuntime::new();
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
            maker_routing_context(),
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

    fn intent_for_order(order: &OrderAny) -> BoltV3OrderIntentEvidence {
        BoltV3OrderIntentEvidence::from_compiled_order(
            "strategy-a".to_string(),
            BoltV3OrderIntentKind::Entry,
            "0.50".to_string(),
            order,
        )
    }

    fn exit_intent_for_order(order: &OrderAny) -> BoltV3OrderIntentEvidence {
        BoltV3OrderIntentEvidence::from_compiled_order(
            "strategy-a".to_string(),
            BoltV3OrderIntentKind::Exit,
            "0.50".to_string(),
            order,
        )
    }

    fn submit_request_for_order(
        order: &OrderAny,
        notional: Decimal,
    ) -> BoltV3SubmitAdmissionRequest {
        BoltV3SubmitAdmissionRequest {
            economics_admission:
                crate::bolt_v3_economics_runtime::test_economics_admission_with_binding(
                    notional,
                    economics_order_binding(order).expect("test order binding should serialize"),
                ),
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional,
            order_side: OrderSide::Buy,
            order_quantity: Decimal::new(1, 0),
            intent_kind: BoltV3SubmitIntentKind::Entry,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
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
        BoltV3SubmitAdmissionRequest {
            economics_admission:
                crate::bolt_v3_economics_runtime::test_economics_admission_with_binding(
                    Decimal::new(25, 1),
                    economics_order_binding(order).expect("test order binding should serialize"),
                ),
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional: Decimal::new(25, 1),
            order_side: OrderSide::Sell,
            order_quantity,
            intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
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

    fn maker_routing_context() -> BoltV3MakerOrderRoutingContext<'static> {
        BoltV3MakerOrderRoutingContext {
            strategy_id: "maker-strategy",
            execution_client_id: "maker_execution_client",
            order_routing: Box::leak(Box::new(
                crate::bolt_v3_economics_runtime::test_order_routing_handle(
                    "maker_execution_client",
                ),
            )),
            submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        }
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
            },
            dedupe_retention_ns: u64::MAX,
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
            venue_spendability: VenueSpendabilitySnapshot {
                source: "nt_account_free_collateral".to_string(),
                observed_at_ns: 0,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-001".to_string(),
                collateral_currency: "USD".to_string(),
                spendable_collateral: Decimal::new(100, 0),
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
                    conditional_token_allowance: Decimal::new(100, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            loss_snapshot: None,
        }
    }

    fn venue_truth_admission_with_yes_position(
        writer: Arc<RecordingDecisionEvidenceWriter>,
        yes_position: Decimal,
    ) -> Arc<BoltV3SubmitAdmissionState> {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_capital_admission(
            writer,
            capital_admission_config(),
        ));
        let mut components = capital_admission_components();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) =
            &mut components.product_state;
        product.source = POLYMARKET_VENUE_TRUTH_REST_SOURCE.to_string();
        product.yes_position = yes_position;
        admission.update_capital_admission_nt_components(components);
        let rebuild = admission.rebuild_capital_admission_open_order_reservations(Vec::new(), 1);
        assert!(rebuild.accepted);
        admission
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
                    conditional_token_allowance: Decimal::ZERO,
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            startup_observed_at_ns: 0,
            dedupe_retention_ns: u64::MAX,
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
        writer: &RecordingDecisionEvidenceWriter,
        instrument: &InstrumentAny,
        sink: &mut RecordingVenueMutationSink,
        order_factory: &mut OrderFactory,
        command: &crate::bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand,
    ) -> Result<BoltV3SubmitRoutingOutcome> {
        admission.replace_kill_switch_state(KillSwitchState::Flattening {
            halt_id: command.halt_id().to_string(),
        });
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
                order_routing: Box::leak(Box::new(
                    crate::bolt_v3_economics_runtime::test_order_routing_handle("execution_client"),
                )),
                gross_expected_value: Decimal::ONE,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
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
        limit_order_at(client_order_id, Price::new(0.50, 2))
    }

    fn limit_order_at(client_order_id: &str, price: Price) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("INSTRUMENT.SOURCE"),
                ClientOrderId::from(client_order_id),
                OrderSide::Buy,
                Quantity::new(1.0, 2),
                price,
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
}
