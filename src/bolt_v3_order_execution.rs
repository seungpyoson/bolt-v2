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
    enums::{OrderSide, OrderType, TimeInForce, TrailingOffsetType, TriggerType},
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId},
    instruments::InstrumentAny,
    orders::{Order, OrderAny, OrderList},
    types::{Price, Quantity},
};
use nautilus_trading::{Strategy, StrategyNative};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_capital_admission::ProductAdmissionSnapshot,
    bolt_v3_current_evidence::{
        EntryOrderIntentFact, EvidenceOrderSide, EvidenceOrderType, EvidenceTimeInForce,
        EvidenceTrailingOffsetType, EvidenceTriggerType, NonBlockingRecordOutcome,
        OrderExecutionEvidence, OrderIntentClampNotEvaluatedReason, OrderIntentClampOutcome,
        OrderIntentDetails, OrderIntentOrderFields, RecordFailure, RiskReducingExitOrderIntentFact,
    },
    bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenCommand,
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        dispatch_maker_order_command,
    },
    bolt_v3_order_intent::{NtOrderBuildInputs, build_nt_order},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionRequestInput,
        BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        build_submit_admission_request_from_order,
    },
};

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
        let intent_kind = request.intent_kind;
        let (intent, request, order) = match clamp_risk_reducing_exit_to_venue_position(
            submit_admission,
            intent,
            request,
            order,
        ) {
            Ok(clamped) => clamped,
            Err(error) => {
                record_order_intent(decision_evidence, intent_kind, error.intent().clone())?;
                return Err(error.into_error());
            }
        };
        record_order_intent(decision_evidence, intent_kind, intent.clone())?;
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

fn record_order_intent(
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

fn clamp_risk_reducing_exit_to_venue_position(
    submit_admission: &BoltV3SubmitAdmissionState,
    mut intent: OrderIntentDetails,
    mut request: BoltV3SubmitAdmissionRequest,
    mut order: OrderAny,
) -> std::result::Result<
    (OrderIntentDetails, BoltV3SubmitAdmissionRequest, OrderAny),
    BoltV3ExitClampError,
> {
    if !request.intent_kind.is_venue_position_exit_clamp_eligible()
        || request.order_quantity <= Decimal::ZERO
    {
        return Ok((intent, request, order));
    }
    if request.order_side != OrderSide::Sell {
        intent.clamp_outcome = Some(OrderIntentClampOutcome::NotEvaluated {
            reason: OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
        });
        return Ok((intent, request, order));
    }
    let venue_position = match canonical_nt_exit_position(submit_admission, &request) {
        CanonicalNtExitPosition::Position(position) => position,
        CanonicalNtExitPosition::Missing => {
            intent.clamp_outcome = Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::NoCanonicalNtPosition,
            });
            return Ok((intent, request, order));
        }
        CanonicalNtExitPosition::ForeignInstrument => {
            intent.clamp_outcome = Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            });
            return Ok((intent, request, order));
        }
    };
    if request.order_quantity <= venue_position {
        intent.clamp_outcome = Some(OrderIntentClampOutcome::WithinBounds);
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
    intent.clamp_outcome = Some(OrderIntentClampOutcome::Clamped {
        original_quantity: original_order_quantity.to_string(),
    });
    intent.order_fields = order_intent_order_fields(&order);

    Ok((intent, request, order))
}

#[derive(Debug)]
struct BoltV3ExitClampError {
    intent: Box<OrderIntentDetails>,
    error: anyhow::Error,
}

impl BoltV3ExitClampError {
    fn intent(&self) -> &OrderIntentDetails {
        self.intent.as_ref()
    }

    fn into_error(self) -> anyhow::Error {
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
    request: &BoltV3SubmitAdmissionRequest,
) -> CanonicalNtExitPosition {
    let Some(state) = submit_admission.capital_admission_state_snapshot() else {
        return CanonicalNtExitPosition::Missing;
    };
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = state.product_state;
    if request.instrument_id == product.yes_instrument_id {
        CanonicalNtExitPosition::Position(product.yes_position)
    } else if request.instrument_id == product.no_instrument_id {
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
}

impl<'a> BoltV3SubmitRoutingRequest<'a> {
    pub fn new(
        decision_evidence: &'a dyn OrderIntentEvidence,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: OrderIntentDetails,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3MakerOrderRoutingContext<'a> {
    pub strategy_id: &'a str,
    pub execution_client_id: &'a str,
    pub max_fee_bps: Decimal,
}

#[derive(Debug, Clone, Copy)]
pub struct BoltV3KillSwitchFlattenRoutingContext<'a> {
    pub execution_client_id: &'a str,
    pub fallback_price: &'a str,
    pub instrument: Option<&'a InstrumentAny>,
    pub max_fee_bps: Decimal,
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
    let mut request = build_submit_admission_request_from_order(
        BoltV3SubmitAdmissionRequestInput {
            execution_client_id: context.execution_client_id,
            intent: &intent,
            intent_kind: BoltV3SubmitIntentKind::KillSwitchForcedReduction,
            order: &order,
            valuation: crate::bolt_v3_submit_admission::OrderValuationContext {
                instrument: context.instrument,
                ..crate::bolt_v3_submit_admission::OrderValuationContext::empty()
            },
            risk_reducing_exit_position: None,
        },
        |_| Ok(context.max_fee_bps),
    )?;
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
    dispatch: F,
}

impl<F> BoltV3NtSubmitOnlySink<F>
where
    F: FnMut(OrderAny, BoltV3SubmitContext) -> Result<()>,
{
    pub(crate) fn new(dispatch: F) -> Self {
        Self { dispatch }
    }
}

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
    S: Strategy + StrategyNative + ?Sized,
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
        let request = build_submit_admission_request_from_order(
            BoltV3SubmitAdmissionRequestInput {
                execution_client_id: self.context.execution_client_id,
                intent: &intent,
                intent_kind: BoltV3SubmitIntentKind::Entry,
                order: &order,
                valuation: crate::bolt_v3_submit_admission::OrderValuationContext::empty(),
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
        BoltV3CancelAllRoutingOutcome, BoltV3CancelRoutingOutcome, BoltV3MakerOrderRoutingContext,
        BoltV3MakerOrderRuntime, BoltV3ModifyRoutingOutcome, BoltV3NtVenueMutationSink,
        BoltV3OrderExecutionMode, BoltV3OrderExecutionPolicy, BoltV3SubmitContext,
        BoltV3SubmitRoutingOutcome, BoltV3SubmitRoutingRequest,
        clamp_risk_reducing_exit_to_venue_position, order_intent_details_from_compiled_order,
        route_kill_switch_flatten_command_with_sink, route_maker_order_command_with_runtime,
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
            BoltV3SubmitIntentKind, PredictionMarketOutcomeSide,
        },
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
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.order_intents()[0].strategy_id, "maker-strategy");
        assert_eq!(
            writer.order_intents()[0].instrument_id,
            InstrumentId::from("YES.INSTRUMENT").to_string()
        );
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 1);
    }

    #[test]
    fn maker_cancel_routes_through_shared_execution_policy_with_configured_client() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
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
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
    }

    #[test]
    fn maker_cancel_all_routes_through_shared_execution_policy_with_configured_client() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
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
        assert!(writer.order_intents().is_empty());
        assert_eq!(writer.admission_count(), 0);
    }

    #[derive(Debug, Default)]
    struct RecordingVenueMutationSink {
        submit_calls: usize,
        submitted_order_quantities: Vec<Quantity>,
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
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(writer.admitted_entry_admissions().len(), 1);
        assert_eq!(admission.admitted_order_count(), 1);
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
    fn live_risk_reducing_exit_clamps_submitted_quantity_to_venue_position() {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = provider_collateral_allowance_admission_with_yes_position(
            writer.clone(),
            Decimal::new(3, 0),
        );

        let mut sink = RecordingVenueMutationSink::default();
        let order = limit_exit_order("O-19700101-000000-001-EXIT-CLAMP-1", Quantity::new(5.0, 2));
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
            .expect("live risk-reducing exit should clamp to venue position and submit");

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
                original_quantity: Decimal::new(5, 0).to_string(),
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
        let request = risk_reducing_exit_submit_request_for_order(
            &order,
            Decimal::new(3, 0),
            Decimal::new(3, 0),
        );

        let outcome = BoltV3OrderExecutionPolicy::live()
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
            Some(OrderIntentClampOutcome::Clamped {
                original_quantity: Decimal::new(5, 0).to_string(),
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

        let mut sink = RecordingVenueMutationSink::default();
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
                max_fee_bps: Decimal::ZERO,
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
                max_fee_bps: Decimal::ZERO,
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
        BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-a".to_string(),
            execution_client_id: "execution_client".to_string(),
            client_order_id: order.client_order_id().to_string(),
            instrument_id: order.instrument_id().to_string(),
            notional: Decimal::new(25, 1),
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

    fn maker_routing_context() -> BoltV3MakerOrderRoutingContext<'static> {
        BoltV3MakerOrderRoutingContext {
            strategy_id: "maker-strategy",
            execution_client_id: "maker_execution_client",
            max_fee_bps: Decimal::ZERO,
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
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: Decimal::new(10, 2),
                    max_slippage_liability: Decimal::new(20, 2),
                }),
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
                max_fee_bps: Decimal::ZERO,
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
