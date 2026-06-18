use std::{any::type_name, cell::RefMut};

use anyhow::Result;
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
use nautilus_trading::Strategy;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_decision_evidence::{
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
    },
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        dispatch_maker_order_command,
    },
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionRequestInput,
        BoltV3SubmitAdmissionState, BoltV3SubmitLifecyclePolicy,
        build_submit_admission_request_from_order,
    },
};

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
        S: Strategy + ?Sized,
    {
        let mut sink = NtStrategyVenueMutationSink { strategy };
        self.route_submit_with_sink(routing, &mut sink, order, context)
    }

    fn route_submit_with_sink<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        sink: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> Result<BoltV3SubmitRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        routing
            .decision_evidence
            .record_order_intent(&routing.intent)?;
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                let permit = routing.submit_admission.admit(&routing.request)?;
                sink.submit_order_via_nt(order, context)?;
                permit.commit_submitted();
                Ok(BoltV3SubmitRoutingOutcome::Submitted)
            }
            BoltV3OrderExecutionMode::Shadow => {
                routing
                    .submit_admission
                    .evaluate_and_record_without_consuming_capacity(&routing.request)?;
                log::info!(
                    "bolt-v3 submit skipped by execution policy: mode=shadow strategy_id={} client_order_id={}",
                    routing.request.strategy_id,
                    routing.request.client_order_id,
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
        S: Strategy + ?Sized,
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
        S: Strategy + ?Sized,
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
        // order's notional past the `max_order_notional`/`max_fee_bps` limits a submit
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
        S: Strategy + ?Sized,
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
    client_id: Option<ClientId>,
    position_id: Option<PositionId>,
    params: Option<Params>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3MakerOrderRoutingContext<'a> {
    pub strategy_id: &'a str,
    pub execution_client_id: &'a str,
    pub max_fee_bps: Decimal,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
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
    S: Strategy + ?Sized,
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

trait BoltV3NtVenueMutationSink {
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

    fn modify_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        quantity: Quantity,
        price: Price,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;
}

struct NtStrategyVenueMutationSink<'a, S>
where
    S: Strategy + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyVenueMutationSink<'_, S>
where
    S: Strategy + ?Sized,
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
    S: Strategy + ?Sized,
{
    strategy: &'a mut S,
}

impl<S> BoltV3NtVenueMutationSink for NtStrategyMakerOrderRuntime<'_, S>
where
    S: Strategy + ?Sized,
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
    S: Strategy + ?Sized,
{
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.strategy.core_mut().order_factory()
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
        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.context.strategy_id.to_string(),
            BoltV3OrderIntentKind::Entry,
            fallback_price,
            &order,
        );
        let request = build_submit_admission_request_from_order(
            BoltV3SubmitAdmissionRequestInput {
                execution_client_id: self.context.execution_client_id,
                intent: &intent,
                order: &order,
                instrument: None,
                quote_quantity_last_price: None,
                quote_quantity_reference_price: None,
                lifecycle_policy: self.context.submit_lifecycle_policy,
                risk_reducing_exit_position: None,
            },
            |_| Ok(self.context.max_fee_bps),
        )?;
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
    use nautilus_model::{
        enums::{OrderSide, OrderType, TimeInForce},
        identifiers::{ClientId, ClientOrderId, InstrumentId, StrategyId, TraderId},
        orders::{LimitOrder, Order, OrderAny},
        types::{Price, Quantity},
    };
    use rust_decimal::Decimal;

    use super::{
        BoltV3CancelRoutingOutcome, BoltV3MakerOrderRoutingContext, BoltV3MakerOrderRuntime,
        BoltV3ModifyRoutingOutcome, BoltV3NtVenueMutationSink, BoltV3OrderExecutionMode,
        BoltV3OrderExecutionPolicy, BoltV3SubmitContext, BoltV3SubmitRoutingOutcome,
        BoltV3SubmitRoutingRequest, route_maker_order_command_with_runtime,
    };
    use crate::{
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_decision_evidence::{
            BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
            BoltV3BasketAdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter,
            BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
            BoltV3PositionSizerRebuildAuditEvidence, BoltV3StrategyInputEvidenceSnapshot,
            BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        },
        bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
        bolt_v3_maker_order_dispatch::{MakerOrderDispatchInput, MakerOrderDispatchOutcome},
        bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
        bolt_v3_position_sizer::{
            FeeSlippagePolicy, PredictionMarketSizingSnapshot, ProductKind, ProductSizingSnapshot,
            SizingPolicy,
        },
        bolt_v3_quote_lifecycle::Leg,
        bolt_v3_sizing_state::{
            OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot, VenueSpendabilitySnapshot,
        },
        bolt_v3_submit_admission::{
            BoltV3CompiledOrderKind, BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide,
            BoltV3CompiledOrderSizingEvidence, BoltV3CompiledProductKind,
            BoltV3LiveSubmitApprovalLimits, BoltV3SubmitAdmissionRequest,
            BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
            BoltV3SubmitPositionSizerConfig, BoltV3SubmitPositionSizingNtComponents,
            PredictionMarketOutcomeSide,
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

        fn record_position_sizer_rebuild_audit(
            &self,
            _audit: &BoltV3PositionSizerRebuildAuditEvidence,
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
    }

    #[derive(Debug, Default)]
    struct RecordingVenueMutationSink {
        submit_calls: usize,
        cancel_calls: usize,
        cancel_all_calls: usize,
        cancel_all_requests: Vec<(InstrumentId, Option<OrderSide>, Option<ClientId>)>,
        modify_calls: usize,
        modify_requests: Vec<(ClientOrderId, Quantity, Price, Option<ClientId>)>,
        fail_submits: bool,
    }

    impl BoltV3NtVenueMutationSink for RecordingVenueMutationSink {
        fn submit_order_via_nt(
            &mut self,
            _order: OrderAny,
            _context: BoltV3SubmitContext,
        ) -> Result<()> {
            self.submit_calls += 1;
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
    fn live_submit_failure_rolls_back_position_sizer_reservation() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_with_position_sizer(
            writer.clone(),
            position_sizer_config(),
        ));
        admission.update_position_sizing_nt_components(position_sizing_components());
        let rebuild = admission.rebuild_position_sizing_open_order_reservations(Vec::new(), 1);
        assert!(rebuild.accepted);

        let mut sink = RecordingVenueMutationSink {
            fail_submits: true,
            ..RecordingVenueMutationSink::default()
        };
        let order = limit_order("O-19700101-000000-001-ROLLBACK-1");
        let intent = intent_for_order(&order);
        let request = sized_submit_request_for_order(&order);
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
            admission.position_sizer_live_reserved_liability(),
            Some(Decimal::ZERO)
        );
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(!admission.position_sizer_has_live_reservation("O-19700101-000000-001-ROLLBACK-1"));
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
        assert_eq!(runtime.venue_sink.modify_calls, 0);
        // No venue mutation → no order intent / admission recorded.
        assert!(writer.records().is_empty());
        assert!(writer.admission_decisions().is_empty());
    }

    #[test]
    fn maker_modify_dispatch_in_shadow_suppresses_the_venue_modify() {
        // The Shadow arm of the same dispatch path: the dispatcher still reports the
        // `Modified` command shape, but the execution policy suppresses the venue
        // call, so `modify_calls` stays 0. Pre-fix the path bailed in BOTH modes; a
        // shadow run that leaked a venue modify (counter > 0) also fails here.
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

    fn intent_for_order(order: &OrderAny) -> BoltV3OrderIntentEvidence {
        BoltV3OrderIntentEvidence::from_compiled_order(
            "strategy-a".to_string(),
            BoltV3OrderIntentKind::Entry,
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
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            risk_reducing_exit_proof: None,
            kill_switch_forced_reduction: None,
            position_sizing: None,
        }
    }

    fn sized_submit_request_for_order(order: &OrderAny) -> BoltV3SubmitAdmissionRequest {
        let mut request = submit_request_for_order(order, Decimal::new(4, 0));
        request.position_sizing = Some(BoltV3CompiledOrderSizingEvidence {
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

    fn maker_routing_context() -> BoltV3MakerOrderRoutingContext<'static> {
        BoltV3MakerOrderRoutingContext {
            strategy_id: "maker-strategy",
            execution_client_id: "maker_execution_client",
            max_fee_bps: Decimal::ZERO,
            submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        }
    }

    fn position_sizer_config() -> BoltV3SubmitPositionSizerConfig {
        BoltV3SubmitPositionSizerConfig {
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
            policy: SizingPolicy {
                min_remaining_pool_balance: None,
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
            },
            dedupe_retention_ns: u64::MAX,
        }
    }

    fn position_sizing_components() -> BoltV3SubmitPositionSizingNtComponents {
        BoltV3SubmitPositionSizingNtComponents {
            source: "nt_sizing_state".to_string(),
            observed_at_ns: 0,
            portfolio: PortfolioSizingSnapshot {
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
            order_lifecycle: OrderLifecycleSizingSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 0,
                open_order_count: 0,
                all_open_orders_attributed: true,
            },
            product_state: ProductSizingSnapshot::PredictionMarketBinary(
                PredictionMarketSizingSnapshot {
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
}
