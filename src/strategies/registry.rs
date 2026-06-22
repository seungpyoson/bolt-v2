use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::{
    data::{IndexPriceUpdate, QuoteTick, TradeTick},
    identifiers::{ClientId, InstrumentId, StrategyId, Venue},
    instruments::{Instrument, InstrumentAny},
};
use nautilus_system::trader::Trader;
use nautilus_trading::Strategy;
use rust_decimal::Decimal;
use toml::Value;

use crate::{
    bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
    bolt_v3_realized_volatility::RealizedVolSnapshot,
    bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidationError {
    pub field: String,
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

pub trait RuntimeStrategy: Strategy + DataActor + Component + std::fmt::Debug {}

impl<T> RuntimeStrategy for T where T: Strategy + DataActor + Component + std::fmt::Debug {}

pub type BoxedStrategy = Box<dyn RuntimeStrategy>;

pub trait FeeProvider: Send + Sync {
    fn fee_bps(&self, instrument_id: InstrumentId) -> Option<Decimal>;
    fn entry_fee_bps(&self, instrument: &InstrumentAny, _entry_price: Decimal) -> Option<Decimal> {
        self.fee_bps(instrument.id())
    }
    fn max_entry_fee_bps(
        &self,
        instrument: &InstrumentAny,
        entry_price: Decimal,
    ) -> Option<Decimal> {
        self.entry_fee_bps(instrument, entry_price)
    }
    fn warm(&self, instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>>;
}

#[derive(Clone)]
pub struct StrategyBuildContext {
    fee_provider: Arc<dyn FeeProvider>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    order_execution_policy: BoltV3OrderExecutionPolicy,
    execution_venue: Venue,
    realized_volatility_runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
}

impl StrategyBuildContext {
    /// `execution_venue` is the venue of the strategy's configured execution client
    /// (`root.clients[execution_client_id].venue`). It is a REQUIRED constructor argument — not an
    /// optional builder field — so a production build can never forget to scope market selection to
    /// the venue that orders actually route to. The strategy uses it to fail closed unless the
    /// selected market's venue equals this one (a wrong-venue selection from the shared NT cache
    /// would otherwise be possible once a second venue's instruments coexist in the cache).
    pub fn new(
        fee_provider: Arc<dyn FeeProvider>,
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        order_execution_policy: BoltV3OrderExecutionPolicy,
        execution_venue: Venue,
    ) -> Self {
        Self {
            fee_provider,
            decision_evidence,
            submit_admission,
            order_execution_policy,
            execution_venue,
            realized_volatility_runtime: Arc::new(Mutex::new(RealizedVolSurfaceRuntime::empty())),
        }
    }

    #[cfg(test)]
    pub fn with_realized_volatility_surfaces(
        mut self,
        surfaces: std::collections::BTreeMap<
            String,
            crate::bolt_v3_realized_volatility::RealizedVolEngineConfig,
        >,
    ) -> Self {
        self.realized_volatility_runtime = Arc::new(Mutex::new(
            RealizedVolSurfaceRuntime::from_configs(surfaces)
                .expect("validated realized-volatility surfaces should build runtime"),
        ));
        self
    }

    pub fn with_realized_volatility_runtime(
        mut self,
        runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
    ) -> Self {
        self.realized_volatility_runtime = runtime;
        self
    }

    pub fn fee_provider(&self) -> &dyn FeeProvider {
        self.fee_provider.as_ref()
    }

    pub fn fee_provider_arc(&self) -> Arc<dyn FeeProvider> {
        self.fee_provider.clone()
    }

    pub fn decision_evidence(&self) -> &dyn BoltV3DecisionEvidenceWriter {
        self.decision_evidence.as_ref()
    }

    pub fn decision_evidence_arc(&self) -> Arc<dyn BoltV3DecisionEvidenceWriter> {
        self.decision_evidence.clone()
    }

    pub fn submit_admission(&self) -> &BoltV3SubmitAdmissionState {
        self.submit_admission.as_ref()
    }

    pub fn submit_admission_arc(&self) -> Arc<BoltV3SubmitAdmissionState> {
        self.submit_admission.clone()
    }

    pub fn order_execution_policy(&self) -> BoltV3OrderExecutionPolicy {
        self.order_execution_policy
    }

    #[cfg(test)]
    pub fn with_order_execution_policy(mut self, policy: BoltV3OrderExecutionPolicy) -> Self {
        self.order_execution_policy = policy;
        self
    }

    /// Venue of the configured execution client. Market selection must be scoped to this venue so a
    /// real order can only ever fire against an instrument on the venue it routes to.
    pub fn execution_venue(&self) -> Venue {
        self.execution_venue
    }

    /// Subscription requests scoped to a single configured surface. A strategy must use this
    /// (with its configured `realized_volatility_surface_id`) so it only subscribes the RV
    /// feeds it prices against, even when the root config defines many surfaces.
    pub fn realized_volatility_quote_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .quote_subscription_requests_for_surface(surface_id)
    }

    pub fn realized_volatility_trade_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .trade_subscription_requests_for_surface(surface_id)
    }

    pub fn realized_volatility_index_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .index_subscription_requests_for_surface(surface_id)
    }

    pub fn observe_realized_volatility_quote(&self, quote: &QuoteTick) -> Vec<RealizedVolSnapshot> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .observe_quote(quote)
    }

    pub fn observe_realized_volatility_trade(&self, trade: &TradeTick) -> Vec<RealizedVolSnapshot> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .observe_trade(trade)
    }

    pub fn observe_realized_volatility_index_price(
        &self,
        update: &IndexPriceUpdate,
    ) -> Vec<RealizedVolSnapshot> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .observe_index_price(update)
    }

    pub fn refresh_realized_volatility_snapshot_at(
        &self,
        surface_id: &str,
        now_ms: u64,
    ) -> Option<RealizedVolSnapshot> {
        self.realized_volatility_runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .refresh_surface_at(surface_id, now_ms)
    }
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

#[derive(Clone)]
pub struct StrategyRegistry {
    registrations: BTreeMap<&'static str, StrategyRegistration>,
}

// The bolt-v3 legacy-default fence forbids a `Default` impl on the production
// surface, so the no-argument `new` is sanctioned with an explicit allow rather
// than satisfying `clippy::new_without_default` by adding a forbidden `Default`.
#[allow(clippy::new_without_default)]
impl StrategyRegistry {
    pub fn new() -> Self {
        Self {
            registrations: BTreeMap::new(),
        }
    }

    pub fn register<B: StrategyBuilder>(&mut self) -> Result<()> {
        let registration = StrategyRegistration {
            kind: B::kind(),
            validate_config: B::validate_config,
            build: B::build,
            register: B::register,
        };

        if self.registrations.contains_key(registration.kind()) {
            return Err(anyhow::anyhow!(
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
        fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<rust_decimal::Decimal> {
            None
        }

        fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    #[derive(Debug)]
    struct NoopDecisionEvidenceWriter;

    impl crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &crate::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(
            &self,
            _intent: &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &crate::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_basket_admission_decision(
            &self,
            _decision: &crate::bolt_v3_decision_evidence::BoltV3BasketAdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_position_sizer_rebuild_audit(
            &self,
            _audit: &crate::bolt_v3_decision_evidence::BoltV3PositionSizerRebuildAuditEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_exit_evaluation(
            &self,
            _evidence: &crate::bolt_v3_decision_evidence::BoltV3ExitEvaluationEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_loss_governor_halt(
            &self,
            _evidence: &crate::bolt_v3_decision_evidence::BoltV3LossGovernorHaltEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_reject(
            &self,
            _evidence: &crate::bolt_v3_decision_evidence::BoltV3OrderRejectEvidence,
        ) -> Result<()> {
            Ok(())
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

    fn test_context() -> StrategyBuildContext {
        StrategyBuildContext::new(
            Arc::new(NoopFeeProvider),
            Arc::new(NoopDecisionEvidenceWriter),
            Arc::new(BoltV3SubmitAdmissionState::new(Arc::new(
                NoopDecisionEvidenceWriter,
            ))),
            crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
            // Fixture venue for registry tests. These exercise strategy registration, not
            // venue-scoped market selection, so the value is inert here; production resolves the
            // execution venue from `root.clients[execution_client_id].venue` (venue-agnostic).
            Venue::from("POLYMARKET"),
        )
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
}
