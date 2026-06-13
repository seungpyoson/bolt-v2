mod support;

use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3AdmissionOutcome, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
};
use bolt_v2::bolt_v3_order_execution::{
    BoltV3CancelRoutingOutcome, BoltV3NtVenueMutationSink, BoltV3OrderExecutionMode,
    BoltV3OrderExecutionPolicy, BoltV3SubmitContext, BoltV3SubmitRoutingOutcome,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3LiveSubmitApprovalLimits, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
};
use nautilus_model::enums::{OrderSide, TimeInForce};
use nautilus_model::identifiers::{ClientId, ClientOrderId, InstrumentId, StrategyId, TraderId};
use nautilus_model::orders::{LimitOrder, Order, OrderAny};
use nautilus_model::types::{Params, Price, Quantity};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Default)]
struct RecordingVenueMutationSink {
    submit_calls: usize,
    cancel_calls: usize,
}

impl BoltV3NtVenueMutationSink for RecordingVenueMutationSink {
    fn submit_order_via_nt(
        &mut self,
        _order: OrderAny,
        _context: BoltV3SubmitContext,
    ) -> anyhow::Result<()> {
        self.submit_calls += 1;
        Ok(())
    }

    fn cancel_order_via_nt(
        &mut self,
        _client_order_id: ClientOrderId,
        _client_id: Option<ClientId>,
        _params: Option<Params>,
    ) -> anyhow::Result<()> {
        self.cancel_calls += 1;
        Ok(())
    }
}

#[test]
fn live_submit_records_evidence_consumes_capacity_and_calls_nt_submit_once() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
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
        .route_submit(
            writer.as_ref(),
            admission.as_ref(),
            intent,
            request,
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("polymarket_main")),
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
fn shadow_submit_records_evidence_without_consuming_capacity_or_calling_nt_submit() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
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
        .route_submit(
            writer.as_ref(),
            admission.as_ref(),
            intent,
            request,
            &mut sink,
            order,
            BoltV3SubmitContext::with_client_id(ClientId::from("polymarket_main")),
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
        .route_cancel(
            &mut sink,
            ClientOrderId::from("O-19700101-000000-001-CANCEL-1"),
            Some(ClientId::from("polymarket_main")),
            None,
        )
        .expect("live cancel should call NT");
    let shadow_outcome = shadow_policy
        .route_cancel(
            &mut sink,
            ClientOrderId::from("O-19700101-000000-001-CANCEL-2"),
            Some(ClientId::from("polymarket_main")),
            None,
        )
        .expect("shadow cancel should be suppressed by policy");

    assert_eq!(live_outcome, BoltV3CancelRoutingOutcome::Canceled);
    assert_eq!(shadow_outcome, BoltV3CancelRoutingOutcome::SkippedByPolicy);
    assert_eq!(sink.cancel_calls, 1);
}

fn live_submit_cap() -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
    BTreeMap::from([(
        "polymarket_main".to_string(),
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
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: order.client_order_id().to_string(),
        instrument_id: order.instrument_id().to_string(),
        notional,
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(1, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
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
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("limit order should be valid"),
    )
}
