//! `binary_oracle_maker` strategy (Slice 2, #488).
//!
//! The strategy compiles, is selectable by the `binary_oracle_maker` archetype
//! key, registers through the shared `production_strategy_registry()`, and
//! validates. Slice 2 adds the μ (informed-fraction) runtime state ([`mu`]): the
//! maker overrides `on_trade` to feed each instrument's signed trade-flow buffer,
//! from which the shared estimator and fail-closed health gate derive μ. Slice
//! 6c adds the strategy-owned shell method that routes a caller-resolved maker
//! quote tick through the shared quote planner, order compiler, and
//! execution/admission policy; the strategy still has no autonomous subscription
//! or quote loop until later runtime slices. The
//! NautilusTrader surface (`core: StrategyCore`, `nautilus_strategy!`, the
//! `StrategyBuilder` impl) mirrors `binary_oracle_edge_taker` *structurally* —
//! it does not copy taker behaviour.

use std::{cell::RefCell, rc::Rc};

use anyhow::Result;
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::{data::TradeTick, enums::OmsType, identifiers::StrategyId};
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::Decimal;
use toml::Value;

use crate::{
    bolt_v3_loss_governor::LossAdmissionDecision,
    bolt_v3_maker_mu_estimator::{MuEstimatorConfig, MuHealthConfig},
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_order_dispatch::{MakerOrderDispatchInput, MakerOrderDispatchOutcome},
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerMarketActionOrderInput, maker_order_plan_from_market_action,
    },
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_risk::{
        MakerLossRiskPolicy, MakerRiskDecision, apply_maker_risk_mode,
        maker_risk_mode_for_loss_decision,
    },
    bolt_v3_maker_runtime_order::{
        MakerRuntimeLegOrderDispatchOutcome, MakerRuntimeOrderDispatchInput,
        MakerRuntimeOrderDispatchOutcome, dispatch_maker_runtime_order_plan_with_command_router,
    },
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteBlockReason, MakerRuntimeQuoteDecision,
        MakerRuntimeQuoteInput, MakerRuntimeQuoteSetInput,
        MakerRuntimeReferenceFairValueBlockReason, MakerRuntimeReferenceFairValueDecision,
        MakerRuntimeReferenceFairValueInput, maker_reference_current_price_fair_value_decision,
        plan_maker_runtime_quote,
    },
    bolt_v3_order_execution::{
        BoltV3MakerOrderRoutingContext,
        route_maker_order_command as route_maker_order_command_through_policy,
    },
    bolt_v3_order_intent::NtOrderTemplate,
    bolt_v3_quote_lifecycle::{Leg, MarketAction, MarketQuote},
    bolt_v3_quoting::QuoteTargets,
    bolt_v3_reference_price::ReferencePriceSelector,
    bolt_v3_requote_budget::RequoteBudgetPair,
    bolt_v3_submit_admission::BoltV3SubmitLifecyclePolicy,
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::binary_oracle_maker::mu::MakerMuState,
    strategies::registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder, ValidationError},
};

pub mod archetype;
pub mod binding;
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
/// runtime state. Compiled maker order commands route through the shared
/// execution policy using the retained build context; pricing/exposure loops
/// arrive in later slices.
pub struct BinaryOracleMaker {
    core: StrategyCore,
    config: BinaryOracleMakerConfig,
    context: StrategyBuildContext,
    mu: MakerMuState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeQuoteRouteInput<'a> {
    pub quote: MakerRuntimeQuoteInput<'a>,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
    pub max_fee_bps: Decimal,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeQuoteRouteOutcome {
    pub quote: MakerRuntimeQuoteDecision,
    pub orders: Option<MakerRuntimeOrderDispatchOutcome>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeReferenceQuoteRouteInput<'a> {
    pub reference_fair_value: MakerRuntimeReferenceFairValueInput<'a>,
    pub quote_plan: MakerQuotePlanInputs<'a>,
    pub quote_set: MakerRuntimeQuoteSetInput<'a>,
    pub order_plan: MakerRuntimeOrderPlanInput,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
    pub max_fee_bps: Decimal,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome {
    pub fair_value: MakerRuntimeReferenceFairValueDecision,
    pub quote: Option<MakerRuntimeQuoteDecision>,
    pub orders: Option<MakerRuntimeOrderDispatchOutcome>,
    pub blocked_by: Option<BinaryOracleMakerRuntimeReferenceQuoteBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOracleMakerRuntimeReferenceQuoteBlockReason {
    FairValue(MakerRuntimeReferenceFairValueBlockReason),
    Quote(MakerRuntimeQuoteBlockReason),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerMarketActionRouteInput<'a> {
    pub action: MakerMarketActionOrderInput,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
    pub max_fee_bps: Decimal,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerMarketActionRouteOutcome {
    pub order: MakerRuntimeLegOrderDispatchOutcome,
    pub orders: MakerRuntimeOrderDispatchOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRiskRouteInput<'a> {
    pub loss_decision: &'a LossAdmissionDecision,
    pub policy: MakerLossRiskPolicy,
    pub targets: QuoteTargets,
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
    pub max_fee_bps: Decimal,
    pub submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRiskRouteOutcome {
    pub risk: MakerRiskDecision,
    pub orders: Option<MakerRuntimeOrderDispatchOutcome>,
}

impl std::fmt::Debug for BinaryOracleMaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryOracleMaker")
            .field("core", &self.core)
            .field("config", &self.config)
            .field("mu", &self.mu)
            .finish_non_exhaustive()
    }
}

impl BinaryOracleMaker {
    pub fn new(config: BinaryOracleMakerConfig, context: StrategyBuildContext) -> Self {
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
            context,
            mu,
        }
    }

    /// The parsed maker config (read by later slices once they add behaviour).
    pub fn config(&self) -> &BinaryOracleMakerConfig {
        &self.config
    }

    pub fn route_maker_order_command(
        &mut self,
        command: &MakerCompiledOrderCommand,
        submit_order_prefix: &str,
        max_fee_bps: Decimal,
        submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    ) -> Result<MakerOrderDispatchOutcome> {
        let policy = self.context.order_execution_policy();
        let decision_evidence = self.context.decision_evidence_arc();
        let submit_admission = self.context.submit_admission_arc();
        let strategy_id = self.config.strategy_id.clone();
        let execution_client_id = self.config.client_id.clone();
        route_maker_order_command_through_policy(
            policy,
            self,
            decision_evidence.as_ref(),
            submit_admission.as_ref(),
            BoltV3MakerOrderRoutingContext {
                strategy_id: strategy_id.as_str(),
                execution_client_id: execution_client_id.as_str(),
                max_fee_bps,
                submit_lifecycle_policy,
            },
            MakerOrderDispatchInput {
                command,
                submit_order_prefix,
            },
        )
    }

    pub fn route_maker_runtime_quote(
        &mut self,
        market: &mut MarketQuote,
        budget: &mut RequoteBudgetPair,
        input: BinaryOracleMakerRuntimeQuoteRouteInput<'_>,
    ) -> Result<BinaryOracleMakerRuntimeQuoteRouteOutcome> {
        let BinaryOracleMakerRuntimeQuoteRouteInput {
            quote,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
            max_fee_bps,
            submit_lifecycle_policy,
        } = input;

        let quote_decision = plan_maker_runtime_quote(market, budget, quote);
        let orders = if let Some(order_plan) = quote_decision.order_plan.as_ref() {
            let mut route_command =
                |command: &MakerCompiledOrderCommand, submit_order_prefix: &str| {
                    self.route_maker_order_command(
                        command,
                        submit_order_prefix,
                        max_fee_bps,
                        submit_lifecycle_policy,
                    )
                };
            Some(dispatch_maker_runtime_order_plan_with_command_router(
                MakerRuntimeOrderDispatchInput {
                    order_plan,
                    submit_template,
                    price_precision,
                    quantity_precision,
                    submit_order_prefix,
                },
                &mut route_command,
            )?)
        } else {
            None
        };

        Ok(BinaryOracleMakerRuntimeQuoteRouteOutcome {
            quote: quote_decision,
            orders,
        })
    }

    pub fn route_maker_runtime_reference_quote(
        &mut self,
        market: &mut MarketQuote,
        budget: &mut RequoteBudgetPair,
        reference_selector: &mut ReferencePriceSelector,
        input: BinaryOracleMakerRuntimeReferenceQuoteRouteInput<'_>,
    ) -> Result<BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome> {
        let BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
            reference_fair_value,
            quote_plan,
            quote_set,
            order_plan,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
            max_fee_bps,
            submit_lifecycle_policy,
        } = input;

        let fair_value = maker_reference_current_price_fair_value_decision(
            reference_selector,
            reference_fair_value,
        );
        let Some(reference_fair_value_result) = fair_value.fair_value.as_ref() else {
            return Ok(BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome {
                blocked_by: fair_value
                    .blocked_by
                    .map(BinaryOracleMakerRuntimeReferenceQuoteBlockReason::FairValue),
                fair_value,
                quote: None,
                orders: None,
            });
        };
        let oracle_fair_probability_up = reference_fair_value_result.fair_probability_up;
        let quote_route = self.route_maker_runtime_quote(
            market,
            budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote: MakerRuntimeQuoteInput {
                    quote_plan: MakerQuotePlanInputs {
                        family_key: reference_fair_value.family_key,
                        oracle_fair_probability_up,
                        ..quote_plan
                    },
                    quote_set,
                    order_plan,
                },
                submit_template,
                price_precision,
                quantity_precision,
                submit_order_prefix,
                max_fee_bps,
                submit_lifecycle_policy,
            },
        )?;
        let BinaryOracleMakerRuntimeQuoteRouteOutcome { quote, orders } = quote_route;
        let blocked_by = quote
            .blocked_by
            .map(BinaryOracleMakerRuntimeReferenceQuoteBlockReason::Quote);

        Ok(BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome {
            fair_value,
            quote: Some(quote),
            orders,
            blocked_by,
        })
    }

    pub fn route_maker_market_action(
        &mut self,
        input: BinaryOracleMakerMarketActionRouteInput<'_>,
    ) -> Result<BinaryOracleMakerMarketActionRouteOutcome> {
        let BinaryOracleMakerMarketActionRouteInput {
            action,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
            max_fee_bps,
            submit_lifecycle_policy,
        } = input;

        let action_kind = action.action;
        let order_plan = maker_order_plan_from_market_action(action);

        let mut route_command = |command: &MakerCompiledOrderCommand, submit_order_prefix: &str| {
            self.route_maker_order_command(
                command,
                submit_order_prefix,
                max_fee_bps,
                submit_lifecycle_policy,
            )
        };
        let orders = dispatch_maker_runtime_order_plan_with_command_router(
            MakerRuntimeOrderDispatchInput {
                order_plan: &order_plan,
                submit_template,
                price_precision,
                quantity_precision,
                submit_order_prefix,
            },
            &mut route_command,
        )?;
        let order = match action_kind {
            MarketAction::Leg { leg: Leg::No, .. }
            | MarketAction::CancelAllOneSide { leg: Leg::No } => orders.no.clone(),
            MarketAction::Leg { leg: Leg::Yes, .. }
            | MarketAction::CancelAllOneSide { leg: Leg::Yes }
            | MarketAction::CancelAllBothLegs => orders.yes.clone(),
        };

        Ok(BinaryOracleMakerMarketActionRouteOutcome { order, orders })
    }

    pub fn route_maker_loss_risk(
        &mut self,
        market: &mut MarketQuote,
        input: BinaryOracleMakerRiskRouteInput<'_>,
    ) -> Result<BinaryOracleMakerRiskRouteOutcome> {
        let BinaryOracleMakerRiskRouteInput {
            loss_decision,
            policy,
            targets,
            yes_quantity,
            no_quantity,
            yes,
            no,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
            max_fee_bps,
            submit_lifecycle_policy,
        } = input;
        let mode = maker_risk_mode_for_loss_decision(&policy, loss_decision);
        let risk = apply_maker_risk_mode(market, mode);
        let orders = if let Some(action) = risk.action {
            Some(
                self.route_maker_market_action(BinaryOracleMakerMarketActionRouteInput {
                    action: MakerMarketActionOrderInput {
                        action,
                        targets,
                        yes_quantity,
                        no_quantity,
                        yes,
                        no,
                    },
                    submit_template,
                    price_precision,
                    quantity_precision,
                    submit_order_prefix,
                    max_fee_bps,
                    submit_lifecycle_policy,
                })?
                .orders,
            )
        } else {
            None
        };

        Ok(BinaryOracleMakerRiskRouteOutcome { risk, orders })
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
// yet, so this handler is dormant until a later slice subscribes.
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

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(BinaryOracleMaker::new(
            parse_config(raw)?,
            context.clone(),
        )))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = BinaryOracleMaker::new(parse_config(raw)?, context.clone());
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_decision_evidence::{
            BoltV3AdmissionDecisionEvidence, BoltV3BasketAdmissionDecisionEvidence,
            BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
            BoltV3PositionSizerRebuildAuditEvidence, BoltV3StrategyInputEvidenceSnapshot,
            BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        },
        bolt_v3_maker_mu_estimator::MuHealthReason,
        bolt_v3_numeric::NANOS_PER_MILLI_U64,
        bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
        bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
        strategies::registry::FeeProvider,
    };
    use futures_util::{FutureExt, future::BoxFuture};
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::AggressorSide,
        identifiers::{InstrumentId, TradeId, Venue},
        types::{Price, Quantity},
    };
    use std::sync::Arc;

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

    #[derive(Debug)]
    struct NoopDecisionEvidenceWriter;

    impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
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

    fn test_context() -> StrategyBuildContext {
        let writer = Arc::new(NoopDecisionEvidenceWriter);
        StrategyBuildContext::new(
            Arc::new(NoopFeeProvider),
            writer.clone(),
            Arc::new(BoltV3SubmitAdmissionState::new(writer)),
            BoltV3OrderExecutionPolicy::shadow(),
            Venue::from("MAKER.TEST"),
        )
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
            market_portfolio_max_active_markets: 3,
            market_portfolio_total_bankroll_notional: 1500.0,
            market_portfolio_min_slot_notional: 100.0,
            markets_config_digest:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
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
        let mut maker = BinaryOracleMaker::new(maker_config(600, 1000, 4), test_context());
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
}
