use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

use anyhow::{Context, Result, anyhow};
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::identifiers::StrategyId;
use nautilus_system::trader::Trader;
use nautilus_trading::Strategy;
use toml::Value;

use crate::{
    bolt_v3_strategy_decision_evidence::BoltV3StrategyDecisionEvidence,
    clients::polymarket::FeeProvider, validate::ValidationError,
};

pub trait RuntimeStrategy: Strategy + DataActor + Component + std::fmt::Debug {}

impl<T> RuntimeStrategy for T where T: Strategy + DataActor + Component + std::fmt::Debug {}

pub type BoxedStrategy = Box<dyn RuntimeStrategy>;

#[derive(Clone)]
pub struct BoltV3MarketSelectionContext {
    pub market_selection_type: String,
    pub rotating_market_family: Option<String>,
    pub underlying_asset: Option<String>,
    pub cadence_seconds: Option<i64>,
    pub market_selection_rule: Option<String>,
    pub retry_interval_seconds: Option<u64>,
    pub blocked_after_seconds: Option<u64>,
}

#[derive(Clone)]
pub struct StrategyBuildContext {
    pub fee_provider: Arc<dyn FeeProvider>,
    pub reference_publish_topic: String,
    pub bolt_v3_decision_evidence: Option<BoltV3StrategyDecisionEvidence>,
    pub bolt_v3_market_selection_context: Option<BoltV3MarketSelectionContext>,
}

pub trait StrategyBuilder: Send + Sync + 'static {
    fn kind() -> &'static str;
    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>);
    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy>;
    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId>;
}

#[derive(Clone, Copy)]
pub struct StrategyRegistration {
    kind: &'static str,
    validate_config: fn(&Value, &str, &mut Vec<ValidationError>),
    build: fn(&Value, &StrategyBuildContext) -> Result<BoxedStrategy>,
    register: fn(&Value, &StrategyBuildContext, &Rc<RefCell<Trader>>) -> Result<StrategyId>,
}

impl StrategyRegistration {
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn validate_config(
        &self,
        raw: &Value,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        (self.validate_config)(raw, field_prefix, errors);
    }

    pub fn build(&self, raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        (self.build)(raw, context)
    }

    pub fn register(
        &self,
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        (self.register)(raw, context, trader)
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
            register: B::register,
        };

        if self.registrations.contains_key(registration.kind()) {
            return Err(anyhow!(
                "strategy kind '{}' is already registered",
                registration.kind()
            ));
        }

        self.registrations.insert(registration.kind(), registration);
        Ok(())
    }

    pub fn get(&self, kind: &str) -> Option<&StrategyRegistration> {
        self.registrations.get(kind)
    }

    pub fn validate(
        &self,
        kind: &str,
        raw: &Value,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        if let Some(registration) = self.get(kind) {
            registration.validate_config(raw, field_prefix, errors);
        }
    }

    pub fn build(
        &self,
        kind: &str,
        raw: &Value,
        context: &StrategyBuildContext,
    ) -> Result<BoxedStrategy> {
        let registration = self
            .get(kind)
            .with_context(|| format!("unsupported strategy kind '{kind}'"))?;
        registration.build(raw, context)
    }

    pub fn register_strategy(
        &self,
        kind: &str,
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let registration = self
            .get(kind)
            .with_context(|| format!("unsupported strategy kind '{kind}'"))?;
        registration.register(raw, context, trader)
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

        fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
            if raw.get("strategy_id").and_then(Value::as_str).is_none() {
                errors.push(ValidationError {
                    field: format!("{field_prefix}.strategy_id"),
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

        fn register(
            raw: &Value,
            _context: &StrategyBuildContext,
            trader: &Rc<RefCell<Trader>>,
        ) -> Result<StrategyId> {
            let strategy_id = raw
                .get("strategy_id")
                .and_then(Value::as_str)
                .context("alpha builder requires strategy_id")?;
            let strategy = TestStrategy::new(strategy_id);
            let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
            trader.borrow_mut().add_strategy(strategy)?;
            Ok(strategy_id)
        }
    }

    struct BetaBuilder;

    impl StrategyBuilder for BetaBuilder {
        fn kind() -> &'static str {
            "beta_runtime"
        }

        fn validate_config(_raw: &Value, _field_prefix: &str, _errors: &mut Vec<ValidationError>) {}

        fn build(_raw: &Value, _context: &StrategyBuildContext) -> Result<BoxedStrategy> {
            Err(anyhow!("beta builder is test-only"))
        }

        fn register(
            _raw: &Value,
            _context: &StrategyBuildContext,
            _trader: &Rc<RefCell<Trader>>,
        ) -> Result<StrategyId> {
            Err(anyhow!("beta builder is test-only"))
        }
    }

    struct ContextAwareBuilder;

    impl StrategyBuilder for ContextAwareBuilder {
        fn kind() -> &'static str {
            "context_runtime"
        }

        fn validate_config(_raw: &Value, _field_prefix: &str, _errors: &mut Vec<ValidationError>) {}

        fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
            let strategy_id = raw
                .get("strategy_id")
                .and_then(Value::as_str)
                .context("context builder requires strategy_id")?;
            let expected_topic = raw
                .get("expected_reference_publish_topic")
                .and_then(Value::as_str)
                .context("context builder requires expected_reference_publish_topic")?;
            anyhow::ensure!(
                context.reference_publish_topic == expected_topic,
                "expected reference publish topic {expected_topic}, got {}",
                context.reference_publish_topic
            );
            Ok(Box::new(TestStrategy::new(strategy_id)))
        }

        fn register(
            _raw: &Value,
            _context: &StrategyBuildContext,
            _trader: &Rc<RefCell<Trader>>,
        ) -> Result<StrategyId> {
            Err(anyhow!("context builder register is unused in this test"))
        }
    }

    fn test_context() -> StrategyBuildContext {
        StrategyBuildContext {
            fee_provider: Arc::new(NoopFeeProvider),
            reference_publish_topic: "platform.reference.test".to_string(),
            bolt_v3_decision_evidence: None,
            bolt_v3_market_selection_context: None,
        }
    }

    #[test]
    fn strategy_registry_registers_and_sorts_kinds() {
        let mut registry = StrategyRegistry::new();

        registry.register::<BetaBuilder>().unwrap();
        registry.register::<AlphaBuilder>().unwrap();

        assert_eq!(registry.kinds(), vec!["alpha_runtime", "beta_runtime"]);
        assert_eq!(
            registry.get("alpha_runtime").unwrap().kind(),
            "alpha_runtime"
        );
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

        registration.validate_config(&raw, "strategies[0].config", &mut errors);
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

        registration.validate_config(&raw, "strategies[0].config", &mut errors);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "missing_strategy_id");
    }

    #[test]
    fn strategy_registry_build_passes_reference_publish_topic_through_context() {
        let mut registry = StrategyRegistry::new();
        registry.register::<ContextAwareBuilder>().unwrap();

        let context = test_context();
        let raw = toml::toml! {
            strategy_id = "ALPHA-REFERENCE-001"
            expected_reference_publish_topic = "platform.reference.test"
        }
        .into();

        let strategy = registry.build("context_runtime", &raw, &context).unwrap();

        assert_eq!(
            strategy.component_id().inner().as_str(),
            "ALPHA-REFERENCE-001"
        );
    }
}
