//! `binary_oracle_maker` strategy (Slice 2, #488).
//!
//! The strategy compiles, is selectable by the `binary_oracle_maker` archetype
//! key, registers through the shared `production_strategy_registry()`, and
//! validates. Slice 2 adds the μ (informed-fraction) runtime state ([`mu`]): the
//! maker overrides `on_trade` to feed each instrument's signed trade-flow buffer,
//! from which the shared estimator and fail-closed health gate derive μ. It
//! subscribes to nothing yet (so the handler is dormant) and **submits no
//! orders** — the no-orders guarantee is preserved until later slices add
//! subscription, quoting, pricing, exposure, and settlement. The NautilusTrader
//! surface (`core: StrategyCore`, `nautilus_strategy!`, the `StrategyBuilder`
//! impl) mirrors `binary_oracle_edge_taker` *structurally* — it does not copy
//! taker behaviour.

use std::{cell::RefCell, rc::Rc};

use anyhow::Result;
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::{data::TradeTick, enums::OmsType, identifiers::StrategyId};
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use toml::Value;

use crate::bolt_v3_maker_mu_estimator::{MuEstimatorConfig, MuHealthConfig};
use crate::bolt_v3_trade_flow::SignedTradeFlowConfig;
use crate::strategies::binary_oracle_maker::mu::MakerMuState;
use crate::strategies::registry::{
    BoxedStrategy, StrategyBuildContext, StrategyBuilder, ValidationError,
};

pub mod archetype;
mod config;
pub mod mu;

pub use config::{
    BinaryOracleMakerBuilder, BinaryOracleMakerConfig, parse_config, validate_config,
};

/// The archetype key for the maker — its `StrategyBuilder::kind`,
/// `RUNTIME_BINDING.key`, validation-binding key, and operator TOML
/// `strategy_archetype` value are all this single constant.
pub const KEY: &str = "binary_oracle_maker";

/// Binary-oracle market-making strategy. Carries the NautilusTrader envelope
/// (`core`), its parsed config, and the per-instrument μ (informed-fraction)
/// runtime state. It emits no orders yet: Slice 2 wires trade observation into
/// the μ buffer; quoting, pricing, and exposure arrive in later slices.
#[derive(Debug)]
pub struct BinaryOracleMaker {
    core: StrategyCore,
    config: BinaryOracleMakerConfig,
    mu: MakerMuState,
}

impl BinaryOracleMaker {
    pub fn new(config: BinaryOracleMakerConfig) -> Self {
        let oms_type = config
            .oms_type
            .parse::<OmsType>()
            .expect("validated binary_oracle_maker oms_type");
        let mu = build_mu_state(&config);
        Self {
            core: StrategyCore::new(
                StrategyConfig::builder()
                    .strategy_id(StrategyId::from(config.strategy_id.as_str()))
                    .order_id_tag(config.order_id_tag.clone())
                    .oms_type(oms_type)
                    .build(),
            ),
            config,
            mu,
        }
    }

    /// The parsed maker config (read by later slices once they add behaviour).
    pub fn config(&self) -> &BinaryOracleMakerConfig {
        &self.config
    }
}

/// Project the maker's flat μ runtime knobs into the three config views
/// [`MakerMuState`] holds — the estimator warmup threshold, the health-gate
/// bounds, and the shared trade-flow retention. The single place that maps a μ
/// config field to its runtime view, so each knob is wired in exactly one home.
fn build_mu_state(config: &BinaryOracleMakerConfig) -> MakerMuState {
    MakerMuState::new(
        MuEstimatorConfig {
            min_classified_samples: config.mu_min_classified_samples,
        },
        MuHealthConfig {
            stale_window_ms: config.mu_stale_window_ms,
            mu_min_floor: config.mu_min_floor,
        },
        SignedTradeFlowConfig {
            window_secs: config.trade_flow_window_secs,
            max_samples: config.trade_flow_max_samples,
        },
    )
}

// The maker overrides only `on_trade`, to feed the per-instrument μ buffer; every
// other `DataActor` handler defaults to a no-op. The maker subscribes to nothing
// yet, so this handler is dormant until a later slice subscribes — and it submits
// no orders, so the no-orders inert guarantee still holds.
impl DataActor for BinaryOracleMaker {
    fn on_trade(&mut self, trade: &TradeTick) -> anyhow::Result<()> {
        self.mu.observe(trade);
        Ok(())
    }
}

nautilus_strategy!(BinaryOracleMaker);

impl StrategyBuilder for BinaryOracleMakerBuilder {
    fn kind() -> &'static str {
        KEY
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        validate_config(raw, field_prefix, errors);
    }

    fn build(raw: &Value, _context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(BinaryOracleMaker::new(parse_config(raw)?)))
    }

    fn register(
        raw: &Value,
        _context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = BinaryOracleMaker::new(parse_config(raw)?);
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_numeric::NANOS_PER_MILLI_U64;
    use crate::{
        bolt_v3_decision_evidence::{
            BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
            BoltV3OrderIntentEvidence, BoltV3PositionSizerRebuildAuditEvidence,
            BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitReservationFillEvidence,
            BoltV3SubmitReservationMetadataEvidence,
        },
        bolt_v3_maker_mu_estimator::MuHealthReason,
        bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
        bolt_v3_maker_order_dispatch::MakerOrderDispatchOutcome,
        bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
        bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
        bolt_v3_quote_lifecycle::Leg,
        bolt_v3_submit_admission::{BoltV3SubmitAdmissionState, BoltV3SubmitLifecyclePolicy},
        strategies::registry::{FeeProvider, StrategyBuildContext},
    };
    use futures_util::{FutureExt, future::BoxFuture};
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::{AggressorSide, OrderSide, OrderType, TimeInForce},
        identifiers::{ClientOrderId, InstrumentId, TradeId, Venue},
        types::{Price, Quantity},
    };
    use rust_decimal::Decimal;
    use std::sync::{Arc, Mutex};

    #[test]
    fn builder_kind_is_archetype_key() {
        assert_eq!(BinaryOracleMakerBuilder::kind(), "binary_oracle_maker");
        assert_eq!(BinaryOracleMakerBuilder::kind(), KEY);
    }

    const QUERY_NOW_MS: u64 = 50_000;
    const TEST_STALE_WINDOW_MS: u64 = 60_000;
    const TEST_MU_FLOOR: f64 = 0.05;
    const TEST_REQUOTE_MIN_INTERVAL_MS: u64 = 500;

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

    #[derive(Debug, Default)]
    struct RecordingDecisionEvidenceWriter {
        order_intents: Mutex<Vec<BoltV3OrderIntentEvidence>>,
        admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
    }

    impl RecordingDecisionEvidenceWriter {
        fn order_intents(&self) -> Vec<BoltV3OrderIntentEvidence> {
            self.order_intents
                .lock()
                .expect("recording order-intent mutex should not be poisoned")
                .clone()
        }

        fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
            self.admission_decisions
                .lock()
                .expect("recording admission mutex should not be poisoned")
                .clone()
        }
    }

    impl BoltV3DecisionEvidenceWriter for RecordingDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            self.order_intents
                .lock()
                .expect("recording order-intent mutex should not be poisoned")
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

    fn maker_config(
        trade_flow_window_secs: u64,
        trade_flow_max_samples: u64,
        mu_min_classified_samples: u64,
    ) -> BinaryOracleMakerConfig {
        BinaryOracleMakerConfig {
            strategy_id: "binary_oracle_maker-001".to_string(),
            order_id_tag: "001".to_string(),
            oms_type: "netting".to_string(),
            client_id: "maker_execution_client".to_string(),
            trade_flow_window_secs,
            trade_flow_max_samples,
            mu_min_classified_samples,
            mu_stale_window_ms: TEST_STALE_WINDOW_MS,
            mu_min_floor: TEST_MU_FLOOR,
            requote_min_interval_ms: TEST_REQUOTE_MIN_INTERVAL_MS,
        }
    }

    fn maker_context(
        writer: Arc<RecordingDecisionEvidenceWriter>,
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

    // Observe one trade per `side` at ts 1000, 2000, ... ms (so the newest is ~46s
    // before QUERY_NOW_MS), routing through the maker's own `MakerMuState`.
    fn observe_sides(state: &mut MakerMuState, instrument: InstrumentId, sides: &[AggressorSide]) {
        for (index, side) in sides.iter().enumerate() {
            let ts_ns = (1_000 + index as u64 * 1_000) * NANOS_PER_MILLI_U64;
            let trade = TradeTick::new_checked(
                instrument,
                Price::new(0.5, 2),
                Quantity::new(1.0, 0),
                *side,
                TradeId::from(format!("T{ts_ns}").as_str()),
                UnixNanos::from(ts_ns),
                UnixNanos::from(ts_ns),
            )
            .expect("valid trade tick");
            state.observe(&trade);
        }
    }

    #[test]
    fn build_mu_state_projects_warmup_and_stale_knobs() {
        // Pins the warmup (`mu_min_classified_samples`) and stale-window mappings
        // through the public gate read: a one-sided buy flow of exactly the warmup
        // count (newest ~46s before `now`) warms μ to 1.0 and clears the gate, so
        // `usable_mu_for` is `Ok(1.0)`. A mismapped warmup would starve μ to
        // `Err(Absent)`; a stale window mismapped below ~46s would flip it to
        // `Err(Stale)`. `window_secs`/`max_samples` are non-binding here — they are
        // pinned by the two tests below. The Stale and BelowFloor branches
        // themselves are exercised directly in `mu`'s own tests.
        let mut state = build_mu_state(&maker_config(600, 1000, 4));
        let instrument = InstrumentId::from("MAKER.SIM");
        observe_sides(&mut state, instrument, &[AggressorSide::Buyer; 4]);
        assert_eq!(state.usable_mu_for(&instrument, QUERY_NOW_MS), Ok(1.0));
    }

    #[test]
    fn build_mu_state_maps_trade_flow_window_secs() {
        // Pins `window_secs`: a 5s retention window ages out trades observed ~46s
        // ago, so `samples_within` is empty and μ is None. Were `build_mu_state`
        // to read the window from `trade_flow_max_samples` (a field swap), the
        // window would be 1000s, the trades would be retained, and the gate would
        // return `Ok(1.0)` — flipping this assertion. An empty in-window view makes
        // both μ and the staleness anchor absent, so the gate fails closed Absent.
        let mut state = build_mu_state(&maker_config(5, 1000, 1));
        let instrument = InstrumentId::from("MAKER.SIM");
        observe_sides(&mut state, instrument, &[AggressorSide::Buyer; 4]);
        assert_eq!(
            state.usable_mu_for(&instrument, QUERY_NOW_MS),
            Err(MuHealthReason::Absent)
        );
    }

    #[test]
    fn build_mu_state_maps_trade_flow_max_samples() {
        // Pins `max_samples`: a cap of 2 retains only the last two (buy) trades of
        // a sell,sell,buy,buy flow, so μ = 1.0. Were `build_mu_state` to read the
        // cap from `trade_flow_window_secs` (a field swap → cap 600), the buffer
        // would keep all four trades, balancing to μ = 0.0 and the gate to
        // `Err(BelowFloor)` — flipping this assertion. The wide 600s window keeps
        // all retained trades in-window so only the cap, not staleness, is exercised.
        let mut state = build_mu_state(&maker_config(600, 2, 2));
        let instrument = InstrumentId::from("MAKER.SIM");
        observe_sides(
            &mut state,
            instrument,
            &[
                AggressorSide::Seller,
                AggressorSide::Seller,
                AggressorSide::Buyer,
                AggressorSide::Buyer,
            ],
        );
        assert_eq!(state.usable_mu_for(&instrument, QUERY_NOW_MS), Ok(1.0));
    }

    #[test]
    fn on_trade_feeds_the_mu_buffer() {
        // Differential guard for the real `DataActor::on_trade` handler (not just
        // `MakerMuState::observe`): a no-op `on_trade` would leave the buffer empty
        // and the gate `Err(Absent)`, so the post-flow `Ok(1.0)` assertion fails on
        // that buggy variant. Asserts through the μ side-effect channel the handler
        // is supposed to drive.
        let mut maker = BinaryOracleMaker::new(maker_config(600, 1000, 4));
        let instrument = InstrumentId::from("MAKER.SIM");
        assert_eq!(
            maker.mu.usable_mu_for(&instrument, QUERY_NOW_MS),
            Err(MuHealthReason::Absent),
            "no trade observed yet must fail closed"
        );
        for index in 0..4u64 {
            let ts_ns = (1_000 + index * 1_000) * NANOS_PER_MILLI_U64;
            let tick = TradeTick::new_checked(
                instrument,
                Price::new(0.5, 2),
                Quantity::new(1.0, 0),
                AggressorSide::Buyer,
                TradeId::from(format!("T{ts_ns}").as_str()),
                UnixNanos::from(ts_ns),
                UnixNanos::from(ts_ns),
            )
            .expect("valid trade tick");
            maker
                .on_trade(&tick)
                .expect("maker on_trade should process");
        }
        assert_eq!(
            maker.mu.usable_mu_for(&instrument, QUERY_NOW_MS),
            Ok(1.0),
            "on_trade must route each tick into the per-instrument μ buffer"
        );
    }

    #[test]
    fn maker_runtime_submit_routes_through_shared_context_in_shadow() {
        let writer = Arc::new(RecordingDecisionEvidenceWriter::default());
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
        let mut maker = BinaryOracleMaker::new(
            maker_config(600, 1000, 4),
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
        assert_eq!(writer.order_intents().len(), 1);
        assert_eq!(
            writer.order_intents()[0].strategy_id,
            "binary_oracle_maker-001"
        );
        assert_eq!(writer.admission_decisions().len(), 1);
        assert_eq!(
            writer.admission_decisions()[0].outcome,
            BoltV3AdmissionOutcome::Admitted
        );
    }
}
