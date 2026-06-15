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
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::AggressorSide,
        identifiers::{InstrumentId, TradeId},
        types::{Price, Quantity},
    };

    #[test]
    fn builder_kind_is_archetype_key() {
        assert_eq!(BinaryOracleMakerBuilder::kind(), "binary_oracle_maker");
        assert_eq!(BinaryOracleMakerBuilder::kind(), KEY);
    }

    #[test]
    fn build_mu_state_projects_config_knobs_into_runtime_views() {
        // Proves the config-field → runtime-view mapping in `build_mu_state`: a
        // one-sided buy flow of exactly `mu_min_classified_samples` trades must
        // warm the estimator to μ = 1.0 and pass the health gate (above the
        // configured floor). A mismapped warmup/floor knob changes this verdict.
        let config = BinaryOracleMakerConfig {
            strategy_id: "binary_oracle_maker-001".to_string(),
            order_id_tag: "001".to_string(),
            oms_type: "netting".to_string(),
            trade_flow_window_secs: 600,
            trade_flow_max_samples: 1000,
            mu_min_classified_samples: 4,
            mu_stale_window_ms: 60_000,
            mu_min_floor: 0.05,
        };
        let mut state = build_mu_state(&config);
        let instrument = InstrumentId::from("MAKER.SIM");
        for index in 0..config.mu_min_classified_samples {
            let ts_ns = (1_000 + index * 1_000) * NANOS_PER_MILLI_U64;
            let trade = TradeTick::new_checked(
                instrument,
                Price::new(0.5, 2),
                Quantity::new(1.0, 0),
                AggressorSide::Buyer,
                TradeId::from(format!("T{ts_ns}").as_str()),
                UnixNanos::from(ts_ns),
                UnixNanos::from(ts_ns),
            )
            .expect("valid trade tick");
            state.observe(&trade);
        }
        assert_eq!(state.mu_for(&instrument, 50_000), Some(1.0));
        assert_eq!(state.health_for(&instrument, 50_000), None);
    }
}
