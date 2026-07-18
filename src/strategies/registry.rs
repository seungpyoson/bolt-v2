use std::collections::BTreeMap;

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActorNative, component::Component};
use nautilus_trading::{Strategy, StrategyNative};
use toml::Value;

use crate::bolt_v3_strategy_context::StrategyBuildContext;
use crate::bolt_v3_strategy_registration::PreparedStrategyRegistration;

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

pub trait StrategyBuilder: Send + Sync + 'static {
    type Strategy: Strategy
        + StrategyNative
        + DataActorNative
        + Component
        + std::fmt::Debug
        + 'static;

    fn kind() -> &'static str;
    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>);
    fn build_typed(raw: &Value, context: &StrategyBuildContext) -> Result<Self::Strategy>;
}

fn prepare_builder<B: StrategyBuilder>(
    raw: &Value,
    context: &StrategyBuildContext,
) -> Result<PreparedStrategyRegistration> {
    Ok(PreparedStrategyRegistration::from_strategy(B::build_typed(
        raw, context,
    )?))
}

#[derive(Clone, Copy)]
pub struct StrategyRegistration {
    kind: &'static str,
    validate_config: fn(&Value, &str, &mut Vec<ValidationError>),
    prepare: fn(&Value, &StrategyBuildContext) -> Result<PreparedStrategyRegistration>,
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

    pub fn prepare(
        &self,
        raw: &Value,
        context: &StrategyBuildContext,
    ) -> Result<PreparedStrategyRegistration> {
        (self.prepare)(raw, context)
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
            prepare: prepare_builder::<B>,
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

    pub(crate) fn register_guarded<B: StrategyBuilder>(&mut self) -> Result<()>
    where
        B::Strategy: super::FillVoidPolicyGuard,
    {
        self.register::<B>()
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

    pub fn prepare_strategy(
        &self,
        kind: &str,
        raw: &Value,
        context: &StrategyBuildContext,
    ) -> Result<PreparedStrategyRegistration> {
        let registration = self
            .get(kind)
            .with_context(|| format!("unsupported strategy kind '{kind}'"))?;
        registration.prepare(raw, context)
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.registrations.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::{Context, anyhow};
    use futures_util::future::{BoxFuture, FutureExt};
    use nautilus_common::actor::DataActor;
    use nautilus_model::identifiers::{InstrumentId, StrategyId, Venue};
    use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};

    use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;

    use super::*;

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

        fn record_capital_admission_rebuild_audit(
            &self,
            _audit: &crate::bolt_v3_decision_evidence::BoltV3CapitalAdmissionRebuildAuditEvidence,
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

        fn record_entry_skip(
            &self,
            _skip: &crate::bolt_v3_decision_evidence::BoltV3EntrySkipEvidence,
        ) -> Result<()> {
            anyhow::bail!("registry noop writer received entry-skip evidence")
        }

        fn record_exit_decision(
            &self,
            _decision: &crate::bolt_v3_decision_evidence::BoltV3ExitDecisionEvidence,
        ) -> Result<()> {
            anyhow::bail!("registry noop writer received exit-decision evidence")
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

        fn record_requote_throttle(
            &self,
            _throttle: &crate::bolt_v3_decision_evidence::BoltV3RequoteThrottleEvidence,
        ) -> Result<()> {
            anyhow::bail!("registry noop writer received requote-throttle evidence")
        }

        fn record_settlement(
            &self,
            _evidence: &crate::bolt_v3_decision_evidence::BoltV3SettlementEvidence,
        ) -> Result<()> {
            anyhow::bail!("registry noop writer received settlement evidence")
        }

        fn record_settlement_booking_error(
            &self,
            _evidence: &crate::bolt_v3_decision_evidence::BoltV3SettlementBookingErrorEvidence,
        ) -> Result<()> {
            anyhow::bail!("registry noop writer received settlement booking-error evidence")
        }

        fn drain_shutdown(&self) -> Result<()> {
            // Deliberate no-op: this registry fixture never owns durable evidence.
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

    crate::strategies::nautilus_strategy_with_fill_void_guard!(TestStrategy, {});

    struct AlphaBuilder;

    impl StrategyBuilder for AlphaBuilder {
        type Strategy = TestStrategy;

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

        fn build_typed(raw: &Value, _context: &StrategyBuildContext) -> Result<Self::Strategy> {
            let strategy_id = raw
                .get("strategy_id")
                .and_then(Value::as_str)
                .context("alpha builder requires strategy_id")?;
            Ok(TestStrategy::new(strategy_id))
        }
    }

    struct BetaBuilder;

    impl StrategyBuilder for BetaBuilder {
        type Strategy = TestStrategy;

        fn kind() -> &'static str {
            "beta_runtime"
        }

        fn validate_config(_raw: &Value, _field_prefix: &str, _errors: &mut Vec<ValidationError>) {}

        fn build_typed(_raw: &Value, _context: &StrategyBuildContext) -> Result<Self::Strategy> {
            Err(anyhow!("beta builder is test-only"))
        }
    }

    fn test_context() -> StrategyBuildContext {
        StrategyBuildContext::new(
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
    fn strategy_context_starts_without_optional_capabilities() {
        let context = test_context();

        assert!(context.realized_volatility_capability().is_none());
        assert!(context.settlement_capability().is_none());
        assert!(
            context
                .realized_volatility_quote_subscription_requests_for_surface("unused")
                .is_empty()
        );
        assert!(
            context
                .realized_volatility_trade_subscription_requests_for_surface("unused")
                .is_empty()
        );
        assert!(
            context
                .realized_volatility_index_subscription_requests_for_surface("unused")
                .is_empty()
        );
        assert_eq!(
            context.refresh_realized_volatility_snapshot_at("unused", 1),
            None
        );
        assert!(context.settlement_runtime_sink().is_none());
        assert!(context.settlement_recovery().is_none());
        assert!(context.settlement_account_id().is_none());
        assert!(context.settlement_currency().is_none());
        assert!(context.settlement_health_transition_emitter().is_none());
    }

    #[test]
    fn settlement_none_builders_install_and_preserve_requested_capability() {
        let context = test_context()
            .with_settlement_runtime_sink(None)
            .with_settlement_recovery(None)
            .with_settlement_account_id(None)
            .with_settlement_currency(None)
            .with_settlement_health_transition_emitter(None);

        assert!(context.settlement_capability().is_some());
        assert!(context.settlement_runtime_sink().is_none());
        assert!(context.settlement_recovery().is_none());
        assert!(context.settlement_account_id().is_none());
        assert!(context.settlement_currency().is_none());
        assert!(context.settlement_health_transition_emitter().is_none());
    }

    #[test]
    fn strategy_registry_registers_and_sorts_kinds() {
        let mut registry = StrategyRegistry::new();

        registry.register_guarded::<BetaBuilder>().unwrap();
        registry.register_guarded::<AlphaBuilder>().unwrap();

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

        registry.register_guarded::<AlphaBuilder>().unwrap();
        let error = registry.register_guarded::<AlphaBuilder>().unwrap_err();

        assert!(error.to_string().contains("alpha_runtime"));
    }

    #[test]
    fn strategy_registry_dispatches_validate_and_prepare() {
        let mut registry = StrategyRegistry::new();
        registry.register_guarded::<AlphaBuilder>().unwrap();

        let registration = registry.get("alpha_runtime").unwrap();
        let raw = toml::toml! {
            strategy_id = "ALPHA-001"
        }
        .into();
        let mut errors = Vec::new();

        registration.validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(errors.is_empty());

        registration.prepare(&raw, &test_context()).unwrap();
    }

    #[test]
    fn strategy_registry_validate_reports_missing_strategy_id() {
        let mut registry = StrategyRegistry::new();
        registry.register_guarded::<AlphaBuilder>().unwrap();

        let registration = registry.get("alpha_runtime").unwrap();
        let raw = toml::Value::Table(Default::default());
        let mut errors = Vec::new();

        registration.validate_config(&raw, "strategies[0].config", &mut errors);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "missing_strategy_id");
    }
}
