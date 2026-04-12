use anyhow::{Context, Result};
use bolt_v2::{
    strategies::registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder},
    validate::ValidationError,
};
use nautilus_common::actor::DataActor;
use nautilus_model::identifiers::StrategyId;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use toml::Value;

#[derive(Debug)]
pub(crate) struct StubRuntimeStrategy {
    core: StrategyCore,
}

impl StubRuntimeStrategy {
    pub(crate) fn new(strategy_id: &str) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(strategy_id)),
                ..Default::default()
            }),
        }
    }
}

impl DataActor for StubRuntimeStrategy {}

nautilus_strategy!(StubRuntimeStrategy);

#[derive(Debug)]
pub(crate) struct StubRuntimeStrategyBuilder;

impl StrategyBuilder for StubRuntimeStrategyBuilder {
    fn kind() -> &'static str {
        "stub_runtime_strategy"
    }

    fn validate_config(raw: &Value, errors: &mut Vec<ValidationError>) {
        if raw.get("strategy_id").and_then(Value::as_str).is_none() {
            errors.push(ValidationError {
                field: "strategies[0].config.strategy_id".to_string(),
                code: "missing_strategy_id",
                message: "is missing required string field".to_string(),
            });
        }
    }

    fn build(raw: &Value, _context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        let strategy_id = raw
            .get("strategy_id")
            .and_then(Value::as_str)
            .context("stub runtime strategy requires strategy_id")?;
        Ok(Box::new(StubRuntimeStrategy::new(strategy_id)))
    }
}
