use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow};
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_trading::Strategy;
use toml::Value;

use crate::{clients::polymarket::FeeProvider, validate::ValidationError};

pub trait RuntimeStrategy: Strategy + DataActor + Component + std::fmt::Debug {}

impl<T> RuntimeStrategy for T where T: Strategy + DataActor + Component + std::fmt::Debug {}

pub type BoxedStrategy = Box<dyn RuntimeStrategy>;

#[derive(Clone)]
pub struct StrategyBuildContext {
    pub fee_provider: Arc<dyn FeeProvider>,
}

pub trait StrategyBuilder: Send + Sync + 'static {
    fn kind() -> &'static str;
    fn validate_config(raw: &Value, errors: &mut Vec<ValidationError>);
    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy>;
}

#[derive(Clone, Copy)]
pub struct StrategyRegistration {
    kind: &'static str,
    validate_config: fn(&Value, &mut Vec<ValidationError>),
    build: fn(&Value, &StrategyBuildContext) -> Result<BoxedStrategy>,
}

impl StrategyRegistration {
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn validate_config(&self, raw: &Value, errors: &mut Vec<ValidationError>) {
        (self.validate_config)(raw, errors);
    }

    pub fn build(&self, raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        (self.build)(raw, context)
    }
}

#[derive(Default, Clone)]
pub struct StrategyRegistry {
    registrations: BTreeMap<&'static str, StrategyRegistration>,
}

impl StrategyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<B: StrategyBuilder>(&mut self) -> Result<()> {
        let registration = StrategyRegistration {
            kind: B::kind(),
            validate_config: B::validate_config,
            build: B::build,
        };

        if self.registrations.contains_key(registration.kind()) {
            return Err(anyhow!(
                "strategy kind '{}' is already registered",
                registration.kind()
            ));
        }

        self.registrations
            .insert(registration.kind(), registration);
        Ok(())
    }

    pub fn get(&self, kind: &str) -> Option<&StrategyRegistration> {
        self.registrations.get(kind)
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.registrations.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, anyhow};
    use futures_util::future::{BoxFuture, FutureExt};
    use nautilus_model::identifiers::StrategyId;
    use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};

    use super::*;

    #[derive(Debug, Clone)]
    struct NoopFeeProvider;

    impl FeeProvider for NoopFeeProvider {
        fn fee_bps(&self, _token_id: &str) -> Option<rust_decimal::Decimal> {
            None
        }

        fn warm(&self, _token_id: &str) -> BoxFuture<'_, Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    #[derive(Debug)]
    struct TestStrategy {
        core: StrategyCore,
    }

    impl TestStrategy {
        fn new(strategy_id: &str) -> Self {
            Self {
                core: StrategyCore::new(StrategyConfig {
                    strategy_id: Some(StrategyId::from(strategy_id)),
                    ..Default::default()
                }),
            }
        }
    }

    impl DataActor for TestStrategy {}

    nautilus_strategy!(TestStrategy);

    struct AlphaBuilder;

    impl StrategyBuilder for AlphaBuilder {
        fn kind() -> &'static str {
            "alpha_runtime"
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
                .context("alpha builder requires strategy_id")?;
            Ok(Box::new(TestStrategy::new(strategy_id)))
        }
    }

    struct BetaBuilder;

    impl StrategyBuilder for BetaBuilder {
        fn kind() -> &'static str {
            "beta_runtime"
        }

        fn validate_config(_raw: &Value, _errors: &mut Vec<ValidationError>) {}

        fn build(_raw: &Value, _context: &StrategyBuildContext) -> Result<BoxedStrategy> {
            Err(anyhow!("beta builder is test-only"))
        }
    }

    fn test_context() -> StrategyBuildContext {
        StrategyBuildContext {
            fee_provider: Arc::new(NoopFeeProvider),
        }
    }

    #[test]
    fn strategy_registry_registers_and_sorts_kinds() {
        let mut registry = StrategyRegistry::new();

        registry.register::<BetaBuilder>().unwrap();
        registry.register::<AlphaBuilder>().unwrap();

        assert_eq!(registry.kinds(), vec!["alpha_runtime", "beta_runtime"]);
        assert_eq!(registry.get("alpha_runtime").unwrap().kind(), "alpha_runtime");
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn strategy_registry_rejects_duplicate_registration() {
        let mut registry = StrategyRegistry::new();

        registry.register::<AlphaBuilder>().unwrap();
        let error = registry.register::<AlphaBuilder>().unwrap_err();

        assert!(error.to_string().contains("alpha_runtime"));
    }

    #[test]
    fn strategy_registry_dispatches_validate_and_build() {
        let mut registry = StrategyRegistry::new();
        registry.register::<AlphaBuilder>().unwrap();

        let registration = registry.get("alpha_runtime").unwrap();
        let raw = toml::toml! {
            strategy_id = "ALPHA-001"
        }
        .into();
        let mut errors = Vec::new();

        registration.validate_config(&raw, &mut errors);
        assert!(errors.is_empty());

        let strategy = registration.build(&raw, &test_context()).unwrap();
        assert_eq!(strategy.component_id().inner().as_str(), "ALPHA-001");
    }

    #[test]
    fn strategy_registry_validate_reports_missing_strategy_id() {
        let mut registry = StrategyRegistry::new();
        registry.register::<AlphaBuilder>().unwrap();

        let registration = registry.get("alpha_runtime").unwrap();
        let raw = toml::Value::Table(Default::default());
        let mut errors = Vec::new();

        registration.validate_config(&raw, &mut errors);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "missing_strategy_id");
    }
}
