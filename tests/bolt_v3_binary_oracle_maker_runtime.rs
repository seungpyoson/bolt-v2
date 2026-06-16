mod support;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_decision_evidence::BoltV3AdmissionOutcome,
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_order_dispatch::MakerOrderDispatchOutcome,
    bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionState, BoltV3SubmitLifecyclePolicy},
    strategies::{
        binary_oracle_maker::{BinaryOracleMaker, BinaryOracleMakerConfig},
        registry::{FeeProvider, StrategyBuildContext},
    },
};
use futures_util::{FutureExt, future::BoxFuture};
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, Venue},
    types::{Price, Quantity},
};
use rust_decimal::Decimal;
use std::sync::Arc;

#[test]
fn maker_runtime_submit_routes_through_shared_context_in_shadow() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    let command = MakerCompiledOrderCommand::Submit {
        leg: Leg::Yes,
        template: Box::new(maker_limit_post_only_template()),
        inputs: NtOrderBuildInputs {
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: OrderSide::Buy,
            quantity: Quantity::new(2.0, 2),
            price: Some(Price::new(0.40, 2)),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
        },
        fallback_price: Price::new(0.40, 2),
    };

    let outcome = maker
        .route_maker_order_command(
            &command,
            "maker_submit",
            Decimal::ZERO,
            BoltV3SubmitLifecyclePolicy::new(true),
        )
        .expect("maker submit should route through shared execution context");

    assert_eq!(
        outcome,
        MakerOrderDispatchOutcome::Submitted {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
            price: Price::new(0.40, 2),
            quantity: Quantity::new(2.0, 2),
        }
    );
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(writer.records().len(), 1);
    assert_eq!(writer.records()[0].strategy_id, "maker-strategy");
    assert_eq!(writer.admission_decisions().len(), 1);
    assert_eq!(
        writer.admission_decisions()[0].outcome,
        BoltV3AdmissionOutcome::Admitted
    );
}

#[derive(Debug)]
struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

fn maker_context(
    writer: Arc<support::RecordingDecisionEvidenceWriter>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer,
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("MAKER.TEST"),
    )
}

fn maker_config() -> BinaryOracleMakerConfig {
    BinaryOracleMakerConfig {
        strategy_id: "maker-strategy".to_string(),
        order_id_tag: "001".to_string(),
        oms_type: "netting".to_string(),
        client_id: "maker_execution_client".to_string(),
        trade_flow_window_secs: 600,
        trade_flow_max_samples: 1000,
        mu_min_classified_samples: 4,
        mu_stale_window_ms: 60_000,
        mu_min_floor: 0.05,
        requote_min_interval_ms: 500,
    }
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
