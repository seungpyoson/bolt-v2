//! Strategy-archetype binding for the `binary_oracle_maker` (Slice 2, #488).
//!
//! This module lives under `src/strategies/` (the NON-scanned strategy layer),
//! so it may freely name both the maker strategy layer
//! (`crate::strategies::binary_oracle_maker::*`, `production_strategy_registry`)
//! and the shared bolt-v3 registration/live-node types. It mirrors the taker
//! archetype binding (`crate::bolt_v3_archetypes::binary_oracle_edge_taker`)
//! *structurally* — it owns:
//!
//! 1. `validate_strategy` — the maker's bolt-v3 startup-validation policy and
//!    **go-live gate**. It deserializes the operator
//!    `[strategies.<id>.parameters]` block into [`ParametersBlock`]
//!    (`deny_unknown_fields`, mirroring the taker's
//!    `try_into::<ParametersBlock>()`) and then bounds-checks the μ-estimator /
//!    health-gate runtime knobs ([`validate_parameter_bounds`]) so a degenerate
//!    or never-warming μ fails closed at load instead of at the first dead
//!    trading session.
//! 2. `register_runtime_strategy` — resolves the configured fee provider and
//!    execution venue, builds the `StrategyBuildContext` + flat raw config table
//!    the maker consumes (the NT envelope plus the μ runtime knobs threaded from
//!    `[parameters.runtime]`), and registers the strategy through the shared
//!    `production_strategy_registry()`.
//! 3. `RUNTIME_BINDING` — the `StrategyRuntimeBinding` the production aggregator
//!    (`crate::strategy_bindings`) lists alongside the taker binding.

use rust_decimal::Decimal;
use serde::Deserialize;
use toml::{Value, map::Map};

use nautilus_model::identifiers::StrategyId;

use crate::bolt_v3_config::{BoltV3StrategyConfig, LoadedStrategy};
use crate::bolt_v3_providers::resolve_fee_provider;
use crate::bolt_v3_strategy_registration::{
    BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
};
use crate::bolt_v3_trade_flow::SignedTradeFlowConfig;
use crate::strategies::binary_oracle_maker::{BinaryOracleMakerBuilder, KEY};
use crate::strategies::production_strategy_registry;
use crate::strategies::registry::StrategyBuilder;

/// The maker runtime binding the production aggregator lists. `key` and
/// `strategy_kind` both resolve to the single archetype constant
/// `binary_oracle_maker`; `register` is this module's `register_runtime_strategy`.
pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: BinaryOracleMakerBuilder::kind,
    register: register_runtime_strategy,
};

/// Operator `[strategies.<id>.parameters]` block for the maker. Mirrors the
/// taker's `ParametersBlock` shape: runtime-tuning knobs live in a nested
/// `[parameters.runtime]` sub-table so the same knob name sits at the same path
/// across strategies. `deny_unknown_fields` fails loud on any stray key.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ParametersBlock {
    runtime: RuntimeParametersBlock,
}

/// Runtime-tuning knobs for the maker's μ (informed-fraction) estimator and its
/// fail-closed health gate. Every value is operator-supplied from TOML; nothing
/// defaults. Unlike the taker's hand-written `Deserialize` (which rejects
/// migrated fields), the maker has no migration history, so the derived
/// `deny_unknown_fields` deserialization is the single source of truth.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RuntimeParametersBlock {
    trade_flow_window_secs: u64,
    trade_flow_max_samples: u64,
    mu_min_classified_samples: u64,
    mu_stale_window_ms: u64,
    mu_min_floor: f64,
    requote_min_interval_ms: u64,
}

/// Bolt-v3 startup validation and **go-live gate** for the maker.
///
/// Confirms the archetype key, then deserializes the operator `[parameters]`
/// block into [`ParametersBlock`] (`deny_unknown_fields`, mirroring the taker's
/// `strategy.parameters.try_into::<ParametersBlock>()`) and bounds-checks the μ
/// runtime knobs ([`validate_parameter_bounds`]). A malformed block or an
/// out-of-bounds knob fails closed at load. `context` and `_default_max_notional`
/// mirror the taker validator's signature so the function is assignable to
/// `ArchetypeValidationBinding::validate_strategy`; the risk-cap parameter is
/// unused until later slices add notional parameters.
pub fn validate_strategy(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    _default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    if strategy.strategy_archetype.as_str() != KEY {
        return vec![format!(
            "{context}: strategy_archetype `{}` is not `{KEY}`",
            strategy.strategy_archetype.as_str()
        )];
    }
    let parameters = match strategy.parameters.clone().try_into::<ParametersBlock>() {
        Ok(value) => value,
        Err(error) => {
            return vec![format!(
                "{context}: parameters block is not a valid `{KEY}` [parameters] block: {error}"
            )];
        }
    };
    validate_parameter_bounds(context, &parameters)
}

/// Fail-closed bounds for the maker's μ runtime knobs (the go-live gate). Each
/// rejected shape would otherwise yield a silently degenerate or never-producible
/// μ at runtime — a fail-soft dead strategy — so it must fail closed at load:
///
/// - a zero retention window, sample cap, or classified-sample minimum means the
///   estimator can never warm or the buffer never retains a trade, so μ is never
///   produced;
/// - a retention window so large its millisecond conversion (`window_secs × 1000`)
///   overflows `u64` would silently saturate to a near-infinite window instead of
///   the value the operator wrote;
/// - a classified-sample minimum above the sample cap is unsatisfiable (the
///   buffer can never hold that many classified samples), so μ is never produced;
/// - a zero staleness window marks every reading stale, blocking μ permanently;
/// - a μ floor outside the open interval `(0, 1)` is degenerate: a floor of `0`
///   admits the constant-0 μ the health gate exists to reject, and a floor `>= 1`
///   blocks every non-degenerate μ.
fn validate_parameter_bounds(context: &str, parameters: &ParametersBlock) -> Vec<String> {
    let runtime = &parameters.runtime;
    let mut errors = Vec::new();
    if runtime.trade_flow_window_secs == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.trade_flow_window_secs must be > 0 (a zero retention window holds no trades, so a μ can never be produced)"
        ));
    }
    let trade_flow = SignedTradeFlowConfig {
        window_secs: runtime.trade_flow_window_secs,
        max_samples: runtime.trade_flow_max_samples,
    };
    if trade_flow.window_ms().is_none() {
        errors.push(format!(
            "{context}: parameters.runtime.trade_flow_window_secs ({}) must be small enough that its second-to-millisecond conversion does not overflow u64 (a larger window silently saturates the retention window instead of meaning the configured value)",
            runtime.trade_flow_window_secs
        ));
    }
    if runtime.trade_flow_max_samples == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.trade_flow_max_samples must be > 0 (a zero sample cap retains no trades, so a μ can never be produced)"
        ));
    }
    if runtime.mu_min_classified_samples == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.mu_min_classified_samples must be > 0 (a zero warmup threshold would admit a μ from an empty window)"
        ));
    }
    if runtime.mu_min_classified_samples > runtime.trade_flow_max_samples {
        errors.push(format!(
            "{context}: parameters.runtime.mu_min_classified_samples ({}) must be <= parameters.runtime.trade_flow_max_samples ({}) (a warmup threshold above the buffer cap is unsatisfiable, so a μ can never be produced)",
            runtime.mu_min_classified_samples, runtime.trade_flow_max_samples
        ));
    }
    if runtime.mu_stale_window_ms == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.mu_stale_window_ms must be > 0 (a zero staleness window marks every reading stale, blocking μ permanently)"
        ));
    }
    if !crate::bolt_v3_numeric::is_positive_finite(runtime.mu_min_floor)
        || runtime.mu_min_floor >= crate::bolt_v3_numeric::UNIT_F64
    {
        errors.push(format!(
            "{context}: parameters.runtime.mu_min_floor ({}) must be finite and in the open interval (0, 1) (a floor of 0 admits the degenerate constant-0 μ the health gate rejects; a floor >= 1 blocks every non-degenerate μ)",
            runtime.mu_min_floor
        ));
    }
    if runtime.requote_min_interval_ms == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.requote_min_interval_ms must be > 0 (a zero requote interval disables the same-tick throttle the requote budget relies on, so the budget rejects construction)"
        ));
    }
    errors
}

/// Register the maker on the live node.
///
/// Mirrors the taker's `register_runtime_strategy` structurally: resolve the fee
/// provider and execution venue from the loaded config, build a
/// `StrategyBuildContext`, then hand the flat raw config table to the shared
/// `production_strategy_registry()`. The raw table carries the NautilusTrader
/// envelope fields (`strategy_id`, `order_id_tag`, `oms_type`) plus the μ runtime
/// knobs `raw_maker_config` threads from the operator `[parameters.runtime]` block.
pub fn register_runtime_strategy(
    node: &mut nautilus_live::node::LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw =
        raw_maker_config(context.strategy).map_err(|message| binding_message(&context, message))?;
    let fee_provider = resolve_fee_provider(
        context.loaded,
        context.strategy.config.execution_client_id.as_str(),
        context.resolved,
    )
    .map_err(|error| binding_message(&context, error.to_string()))?;
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    let execution_venue = context
        .loaded
        .root
        .clients
        .get(execution_client_id)
        .map(|client| client.venue)
        .ok_or_else(|| {
            binding_message(
                &context,
                format!(
                    "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
                ),
            )
        })?;
    let build_context = crate::strategies::registry::StrategyBuildContext::new(
        fee_provider,
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
        context.order_execution_policy,
        execution_venue,
    )
    .with_realized_volatility_runtime(context.realized_volatility_runtime.clone());
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .register_strategy(
            BinaryOracleMakerBuilder::kind(),
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

/// Build the flat raw config table the maker consumes. The NautilusTrader
/// strategy id is `<strategy_archetype>-<order_id_tag>` (validated as an NT
/// `StrategyId`), mirroring the taker's `nt_strategy_id`; `oms_type` is the
/// lowercased NT enum display, matching how the maker config deserializes it.
/// The μ runtime knobs are read from the operator `[parameters.runtime]` block
/// and threaded in flat under the same names `BinaryOracleMakerConfig` consumes.
fn raw_maker_config(strategy: &LoadedStrategy) -> Result<Value, String> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(format!(
            "strategy_archetype `{}` is not `{KEY}`",
            strategy.config.strategy_archetype.as_str()
        ));
    }
    let parameters: ParametersBlock = strategy
        .config
        .parameters
        .clone()
        .try_into()
        .map_err(|error| format!("invalid [parameters] block: {error}"))?;
    let runtime = &parameters.runtime;

    let mut strategy_id = strategy.config.strategy_archetype.as_str().to_string();
    strategy_id.push('-');
    strategy_id.push_str(&strategy.config.order_id_tag);
    StrategyId::new_checked(&strategy_id)
        .map_err(|error| format!("maps to invalid NT StrategyId `{strategy_id}`: {error}"))?;

    let mut table = Map::new();
    table.insert("strategy_id".to_string(), Value::String(strategy_id));
    table.insert(
        "order_id_tag".to_string(),
        Value::String(strategy.config.order_id_tag.clone()),
    );
    table.insert(
        "oms_type".to_string(),
        Value::String(strategy.config.oms_type.to_string().to_ascii_lowercase()),
    );
    insert_runtime_knobs(&mut table, runtime)?;
    Ok(Value::Table(table))
}

/// Thread the μ runtime knobs from the operator `[parameters.runtime]` block into
/// the flat config table under the exact field names `BinaryOracleMakerConfig`
/// consumes. Factored out so the operator → flat-table → consumer-config bridge
/// is unit-testable in isolation (a key-name drift here fails the flat table's
/// `deny_unknown_fields` deserialization at `parse_config`).
fn insert_runtime_knobs(
    table: &mut Map<String, Value>,
    runtime: &RuntimeParametersBlock,
) -> Result<(), String> {
    insert_u64_field(
        table,
        "trade_flow_window_secs",
        runtime.trade_flow_window_secs,
    )?;
    insert_u64_field(
        table,
        "trade_flow_max_samples",
        runtime.trade_flow_max_samples,
    )?;
    insert_u64_field(
        table,
        "mu_min_classified_samples",
        runtime.mu_min_classified_samples,
    )?;
    insert_u64_field(table, "mu_stale_window_ms", runtime.mu_stale_window_ms)?;
    table.insert(
        "mu_min_floor".to_string(),
        Value::Float(runtime.mu_min_floor),
    );
    insert_u64_field(
        table,
        "requote_min_interval_ms",
        runtime.requote_min_interval_ms,
    )?;
    Ok(())
}

/// Insert a `u64` runtime knob into the flat config table as a TOML integer.
/// TOML integers are signed 64-bit, so a value above `i64::MAX` cannot round-trip
/// and fails closed here rather than silently wrapping.
fn insert_u64_field(table: &mut Map<String, Value>, key: &str, value: u64) -> Result<(), String> {
    let integer = i64::try_from(value).map_err(|_| {
        format!("runtime knob `{key}` ({value}) exceeds the supported TOML integer range")
    })?;
    table.insert(key.to_string(), Value::Integer(integer));
    Ok(())
}

/// Wrap a registration failure message in the shared
/// `BoltV3StrategyRegistrationError::Binding` variant, mirroring the taker's
/// `binding_message`.
fn binding_message(
    context: &StrategyRegistrationContext<'_>,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
        strategy_archetype: context
            .strategy
            .config
            .strategy_archetype
            .as_str()
            .to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: &str = "strategy `maker-001`";

    fn valid_runtime() -> RuntimeParametersBlock {
        RuntimeParametersBlock {
            trade_flow_window_secs: 600,
            trade_flow_max_samples: 1000,
            mu_min_classified_samples: 4,
            mu_stale_window_ms: 60_000,
            mu_min_floor: 0.05,
            requote_min_interval_ms: 500,
        }
    }

    fn bounds_errors(runtime: RuntimeParametersBlock) -> Vec<String> {
        validate_parameter_bounds(CONTEXT, &ParametersBlock { runtime })
    }

    #[test]
    fn runtime_binding_key_is_archetype_key() {
        assert_eq!(RUNTIME_BINDING.key, "binary_oracle_maker");
        assert_eq!(RUNTIME_BINDING.key, KEY);
    }

    #[test]
    fn runtime_binding_strategy_kind_matches_key() {
        assert_eq!((RUNTIME_BINDING.strategy_kind)(), KEY);
    }

    #[test]
    fn validate_parameter_bounds_accepts_valid_runtime() {
        assert!(
            bounds_errors(valid_runtime()).is_empty(),
            "valid runtime knobs must pass the go-live gate"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_window() {
        let errors = bounds_errors(RuntimeParametersBlock {
            trade_flow_window_secs: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("trade_flow_window_secs")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_window_secs_that_overflows_millis() {
        // A window_secs whose × 1000 millisecond conversion overflows u64 would
        // silently saturate the retention window instead of meaning the configured
        // value; the go-live gate must reject it loud.
        let errors = bounds_errors(RuntimeParametersBlock {
            trade_flow_window_secs: u64::MAX,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("silently saturates the retention window")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_max_samples() {
        let errors = bounds_errors(RuntimeParametersBlock {
            trade_flow_max_samples: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("trade_flow_max_samples")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_min_classified() {
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_min_classified_samples: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("mu_min_classified_samples")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_min_classified_above_cap() {
        // A warmup threshold above the buffer cap is unsatisfiable: the buffer can
        // never hold that many classified samples, so μ is never produced.
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_min_classified_samples: 1001,
            trade_flow_max_samples: 1000,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must be <= parameters.runtime.trade_flow_max_samples")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_allows_min_classified_equal_to_cap() {
        // The boundary (threshold == cap) is satisfiable, so it must not be rejected.
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_min_classified_samples: 1000,
            trade_flow_max_samples: 1000,
            ..valid_runtime()
        });
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_stale_window() {
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_stale_window_ms: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("mu_stale_window_ms")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_floor_at_or_below_zero() {
        for floor in [0.0, -0.1] {
            let errors = bounds_errors(RuntimeParametersBlock {
                mu_min_floor: floor,
                ..valid_runtime()
            });
            assert!(
                errors.iter().any(|error| error.contains("mu_min_floor")),
                "floor {floor} must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_floor_at_or_above_one() {
        for floor in [1.0, 1.5] {
            let errors = bounds_errors(RuntimeParametersBlock {
                mu_min_floor: floor,
                ..valid_runtime()
            });
            assert!(
                errors.iter().any(|error| error.contains("mu_min_floor")),
                "floor {floor} must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_non_finite_floor() {
        for floor in [f64::NAN, f64::INFINITY] {
            let errors = bounds_errors(RuntimeParametersBlock {
                mu_min_floor: floor,
                ..valid_runtime()
            });
            assert!(
                errors.iter().any(|error| error.contains("mu_min_floor")),
                "non-finite floor must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_requote_interval() {
        // A zero requote interval disables the same-tick throttle the requote
        // budget relies on; `build_requote_budget_pair` rejects it, so the go-live
        // gate must reject it loud at load rather than at first quote.
        let errors = bounds_errors(RuntimeParametersBlock {
            requote_min_interval_ms: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("requote_min_interval_ms")),
            "{errors:?}"
        );
    }

    fn parameters_from_str(toml: &str) -> Result<ParametersBlock, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn parameters_block_deserializes_nested_runtime() {
        let parsed = parameters_from_str(
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
        )
        .expect("valid block deserializes");
        assert_eq!(parsed.runtime, valid_runtime());
    }

    #[test]
    fn parameters_block_rejects_unknown_runtime_key() {
        assert!(
            parameters_from_str(
                r#"
                [runtime]
                trade_flow_window_secs = 600
                trade_flow_max_samples = 1000
                mu_min_classified_samples = 4
                mu_stale_window_ms = 60000
                mu_min_floor = 0.05
                requote_min_interval_ms = 500
                surprise = 1
                "#,
            )
            .is_err(),
            "an unknown [parameters.runtime] key must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_runtime_table() {
        assert!(
            parameters_from_str("decoy = 1").is_err(),
            "an absent [parameters.runtime] table must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_runtime_knob() {
        assert!(
            parameters_from_str(
                r#"
                [runtime]
                trade_flow_window_secs = 600
                trade_flow_max_samples = 1000
                mu_min_classified_samples = 4
                mu_stale_window_ms = 60000
                "#,
            )
            .is_err(),
            "a missing μ knob must fail loud"
        );
    }

    #[test]
    fn runtime_knobs_thread_into_consumer_config() {
        // The load-bearing bridge test: `insert_runtime_knobs` must write exactly
        // the field names `BinaryOracleMakerConfig` deserializes. A key-name drift
        // fails the consumer config's `deny_unknown_fields` parse below; a value
        // drift fails an assertion.
        use crate::strategies::binary_oracle_maker::parse_config;
        let mut table = Map::new();
        table.insert(
            "strategy_id".to_string(),
            Value::String("binary_oracle_maker-001".to_string()),
        );
        table.insert("order_id_tag".to_string(), Value::String("001".to_string()));
        table.insert("oms_type".to_string(), Value::String("netting".to_string()));
        insert_runtime_knobs(&mut table, &valid_runtime()).expect("knobs thread");
        let config =
            parse_config(&Value::Table(table)).expect("flat table parses into the consumer config");
        assert_eq!(config.trade_flow_window_secs, 600);
        assert_eq!(config.trade_flow_max_samples, 1000);
        assert_eq!(config.mu_min_classified_samples, 4);
        assert_eq!(config.mu_stale_window_ms, 60_000);
        assert_eq!(config.mu_min_floor, 0.05);
        assert_eq!(config.requote_min_interval_ms, 500);
    }

    #[test]
    fn insert_u64_field_rejects_value_above_i64_max() {
        let mut table = Map::new();
        assert!(
            insert_u64_field(&mut table, "trade_flow_window_secs", u64::MAX).is_err(),
            "a u64 above i64::MAX cannot round-trip through TOML and must fail closed"
        );
    }
}
