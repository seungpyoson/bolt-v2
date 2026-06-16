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
    use crate::bolt_v3_maker_mu_estimator::MuHealthReason;
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

    const QUERY_NOW_MS: u64 = 50_000;
    const TEST_STALE_WINDOW_MS: u64 = 60_000;
    const TEST_MU_FLOOR: f64 = 0.05;
    const TEST_REQUOTE_MIN_INTERVAL_MS: u64 = 500;

    fn maker_config(
        trade_flow_window_secs: u64,
        trade_flow_max_samples: u64,
        mu_min_classified_samples: u64,
    ) -> BinaryOracleMakerConfig {
        BinaryOracleMakerConfig {
            strategy_id: "binary_oracle_maker-001".to_string(),
            order_id_tag: "001".to_string(),
            oms_type: "netting".to_string(),
            trade_flow_window_secs,
            trade_flow_max_samples,
            mu_min_classified_samples,
            mu_stale_window_ms: TEST_STALE_WINDOW_MS,
            mu_min_floor: TEST_MU_FLOOR,
            requote_min_interval_ms: TEST_REQUOTE_MIN_INTERVAL_MS,
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
}
