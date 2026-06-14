//! Inert `binary_oracle_maker` strategy (Slice 1, #488).
//!
//! Registered-but-**inert**: the strategy compiles, is selectable by the
//! `binary_oracle_maker` archetype key, registers through the shared
//! `production_strategy_registry()`, and validates — but does nothing. Its
//! `impl DataActor` is empty, so every NautilusTrader handler defaults to no-op:
//! it subscribes to nothing and emits no orders. Later slices add quoting,
//! pricing, exposure, and settlement behaviour. The NautilusTrader surface
//! (`core: StrategyCore`, `nautilus_strategy!`, the `StrategyBuilder` impl)
//! mirrors `binary_oracle_edge_taker` *structurally* — it does not copy taker
//! behaviour.

use std::{cell::RefCell, rc::Rc};

use anyhow::Result;
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::{enums::OmsType, identifiers::StrategyId};
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use toml::Value;

use crate::strategies::registry::{
    BoxedStrategy, StrategyBuildContext, StrategyBuilder, ValidationError,
};

pub mod archetype;
mod config;

pub use config::{
    BinaryOracleMakerBuilder, BinaryOracleMakerConfig, parse_config, validate_config,
};

/// The archetype key for the maker — its `StrategyBuilder::kind`,
/// `RUNTIME_BINDING.key`, validation-binding key, and operator TOML
/// `strategy_archetype` value are all this single constant.
pub const KEY: &str = "binary_oracle_maker";

/// Inert binary-oracle market-making strategy. Carries only the NautilusTrader
/// envelope (`core`) plus its parsed config and build context; no active,
/// pricing, or exposure state exists yet.
#[derive(Debug)]
pub struct BinaryOracleMaker {
    core: StrategyCore,
    config: BinaryOracleMakerConfig,
    context: StrategyBuildContext,
}

impl BinaryOracleMaker {
    pub fn new(config: BinaryOracleMakerConfig, context: StrategyBuildContext) -> Self {
        let oms_type = config
            .oms_type
            .parse::<OmsType>()
            .expect("validated binary_oracle_maker oms_type");
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                order_id_tag: Some(config.order_id_tag.clone()),
                oms_type: Some(oms_type),
                ..Default::default()
            }),
            config,
            context,
        }
    }

    /// The parsed maker config (read by later slices once they add behaviour).
    pub fn config(&self) -> &BinaryOracleMakerConfig {
        &self.config
    }

    /// The build context (fee provider / execution venue / submit-admission)
    /// the maker is wired with (read by later slices once they add behaviour).
    pub fn context(&self) -> &StrategyBuildContext {
        &self.context
    }
}

// Empty on purpose: every `DataActor` handler defaults to a no-op, so the maker
// subscribes to nothing and emits no orders. This is the inert guarantee.
impl DataActor for BinaryOracleMaker {}

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

    #[test]
    fn builder_kind_is_archetype_key() {
        assert_eq!(BinaryOracleMakerBuilder::kind(), "binary_oracle_maker");
        assert_eq!(BinaryOracleMakerBuilder::kind(), KEY);
    }
}
