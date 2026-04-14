use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::identifiers::StrategyId;
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use serde::Deserialize;
use toml::Value;

use crate::{
    strategies::registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder},
    validate::ValidationError,
};

trait TomlValueExt {
    fn as_float_or_integer(&self) -> Option<f64>;
}

impl TomlValueExt for Value {
    fn as_float_or_integer(&self) -> Option<f64> {
        self.as_float()
            .or_else(|| self.as_integer().map(|value| value as f64))
    }
}

macro_rules! eth_chainlink_taker_config_fields {
    ($macro:ident) => {
        $macro! {
            strategy_id: String => as_str, "string", "a string", "missing_strategy_id";
            client_id: String => as_str, "string", "a string", "missing_client_id";
            warmup_tick_count: u64 => as_integer, "integer", "an integer", "missing_warmup_tick_count";
            reentry_cooldown_secs: u64 => as_integer, "integer", "an integer", "missing_reentry_cooldown_secs";
            max_position_usdc: f64 => as_float_or_integer, "float", "a float", "missing_max_position_usdc";
            book_impact_cap_bps: u64 => as_integer, "integer", "an integer", "missing_book_impact_cap_bps";
            risk_lambda: f64 => as_float_or_integer, "float", "a float", "missing_risk_lambda";
            worst_case_ev_min_bps: i64 => as_integer, "integer", "an integer", "missing_worst_case_ev_min_bps";
            exit_hysteresis_bps: i64 => as_integer, "integer", "an integer", "missing_exit_hysteresis_bps";
            forced_flat_stale_chainlink_ms: u64 => as_integer, "integer", "an integer", "missing_forced_flat_stale_chainlink_ms";
            forced_flat_thin_book_min_liquidity: f64 => as_float_or_integer, "float", "a float", "missing_forced_flat_thin_book_min_liquidity";
            lead_agreement_min_corr: f64 => as_float_or_integer, "float", "a float", "missing_lead_agreement_min_corr";
            lead_jitter_max_ms: u64 => as_integer, "integer", "an integer", "missing_lead_jitter_max_ms";
        }
    };
}

macro_rules! define_config_struct {
    ($( $field:ident : $ty:ty => $getter:ident, $expected:literal, $expected_with_article:literal, $missing_code:literal; )+) => {
        #[derive(Debug, Clone, PartialEq, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct EthChainlinkTakerConfig {
            $( $field: $ty, )+
        }
    };
}

macro_rules! match_config_field_names {
    ($( $field:ident : $ty:ty => $getter:ident, $expected:literal, $expected_with_article:literal, $missing_code:literal; )+) => {
        $( stringify!($field) )|+
    };
}

macro_rules! validate_config_fields_impl {
    ($( $field:ident : $ty:ty => $getter:ident, $expected:literal, $expected_with_article:literal, $missing_code:literal; )+) => {
        |table: &toml::map::Map<String, Value>, field_prefix: &str, errors: &mut Vec<ValidationError>| {
            $(
                let field = format!("{field_prefix}.{}", stringify!($field));
                match table.get(stringify!($field)) {
                    None => EthChainlinkTakerBuilder::push_missing(errors, field, $missing_code, $expected),
                    Some(value) if value.$getter().is_none() => {
                        EthChainlinkTakerBuilder::push_wrong_type(errors, field, $expected_with_article, value);
                    }
                    Some(_) => {}
                }
            )+
        }
    };
}

eth_chainlink_taker_config_fields!(define_config_struct);

pub struct EthChainlinkTaker {
    core: StrategyCore,
    config: EthChainlinkTakerConfig,
    _context: StrategyBuildContext,
}

impl EthChainlinkTaker {
    fn new(config: EthChainlinkTakerConfig, context: StrategyBuildContext) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                ..Default::default()
            }),
            config,
            _context: context,
        }
    }
}

impl std::fmt::Debug for EthChainlinkTaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthChainlinkTaker")
            .field("config", &self.config)
            .finish()
    }
}

impl DataActor for EthChainlinkTaker {}

nautilus_strategy!(EthChainlinkTaker);

#[derive(Debug)]
pub struct EthChainlinkTakerBuilder;

impl EthChainlinkTakerBuilder {
    fn parse_config(raw: &Value) -> Result<EthChainlinkTakerConfig> {
        raw.clone()
            .try_into()
            .context("eth_chainlink_taker builder requires a valid config table")
    }

    fn push_missing(
        errors: &mut Vec<ValidationError>,
        field: String,
        code: &'static str,
        expected: &'static str,
    ) {
        errors.push(ValidationError {
            field,
            code,
            message: format!("is missing required {expected} field"),
        });
    }

    fn push_wrong_type(
        errors: &mut Vec<ValidationError>,
        field: String,
        expected_with_article: &'static str,
        value: &Value,
    ) {
        errors.push(ValidationError {
            field,
            code: "wrong_type",
            message: format!(
                "must be {expected_with_article}, got {} value",
                value.type_str()
            ),
        });
    }

    fn push_unknown_field(errors: &mut Vec<ValidationError>, field: String, key: &str) {
        errors.push(ValidationError {
            field,
            code: "unknown_field",
            message: format!("unknown field `{key}`"),
        });
    }

    fn validate_table(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        for key in table.keys() {
            if !matches!(
                key.as_str(),
                eth_chainlink_taker_config_fields!(match_config_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field_prefix}.{key}"), key);
            }
        }

        eth_chainlink_taker_config_fields!(validate_config_fields_impl)(
            table,
            field_prefix,
            errors,
        );
    }
}

impl StrategyBuilder for EthChainlinkTakerBuilder {
    fn kind() -> &'static str {
        "eth_chainlink_taker"
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        let Some(table) = raw.as_table() else {
            Self::push_wrong_type(errors, field_prefix.to_string(), "a table", raw);
            return;
        };

        Self::validate_table(table, field_prefix, errors);
    }

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(EthChainlinkTaker::new(
            Self::parse_config(raw)?,
            context.clone(),
        )))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = EthChainlinkTaker::new(Self::parse_config(raw)?, context.clone());
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::{production_strategy_registry, registry::StrategyBuilder};

    fn find_error<'a>(
        errors: &'a [ValidationError],
        field: &str,
        code: &'static str,
    ) -> &'a ValidationError {
        errors
            .iter()
            .find(|e| e.field == field && e.code == code)
            .unwrap_or_else(|| panic!("missing error {field} / {code}: {errors:?}"))
    }

    fn valid_raw_config() -> Value {
        toml::toml! {
            strategy_id = "ETHCHAINLINKTAKER-001"
            client_id = "POLYMARKET"
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            max_position_usdc = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_chainlink_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250
        }
        .into()
    }

    #[test]
    fn production_registry_registers_eth_chainlink_taker_kind() {
        let registry = production_strategy_registry().expect("registry should build");
        assert!(registry.get("eth_chainlink_taker").is_some());
    }

    #[test]
    fn builder_requires_strategy_id_and_client_id() {
        let raw = toml::toml! {
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            max_position_usdc = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_chainlink_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250
        }
        .into();
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.strategy_id")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.client_id")
        );
    }

    #[test]
    fn builder_rejects_unknown_fields() {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("valid config must be a table")
            .insert("stray_flag".to_string(), Value::Boolean(true));
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(&errors, "strategies[0].config.stray_flag", "unknown_field");
        assert!(error.message.contains("unknown field `stray_flag`"));
    }

    #[test]
    fn builder_rejects_non_table_config() {
        let raw = Value::String("not-a-table".to_string());
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(&errors, "strategies[0].config", "wrong_type");
        assert_eq!(error.message, "must be a table, got string value");
        assert!(!errors.iter().any(|e| {
            e.field == "strategies[0].config.strategy_id" && e.code == "missing_required_string"
        }));
    }

    #[test]
    fn builder_rejects_wrong_type_config_at_the_field() {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("valid config must be a table")
            .insert(
                "warmup_tick_count".to_string(),
                Value::String("20".to_string()),
            );
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(
            &errors,
            "strategies[0].config.warmup_tick_count",
            "wrong_type",
        );
        assert_eq!(error.message, "must be an integer, got string value");
        assert!(!errors.iter().any(|e| {
            e.field == "strategies[0].config.warmup_tick_count"
                && e.code == "missing_required_integer"
        }));
    }

    #[test]
    fn builder_accepts_integer_literals_for_f64_fields() {
        let raw = toml::toml! {
            strategy_id = "ETHCHAINLINKTAKER-001"
            client_id = "POLYMARKET"
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            max_position_usdc = 1000
            book_impact_cap_bps = 15
            risk_lambda = 1
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_chainlink_ms = 1500
            forced_flat_thin_book_min_liquidity = 100
            lead_agreement_min_corr = 1
            lead_jitter_max_ms = 250
        }
        .into();
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            !errors
                .iter()
                .any(|e| e.code == "wrong_type" && e.field.starts_with("strategies[0].config")),
            "expected integer literals for f64 fields to validate, got: {errors:?}"
        );
    }
}
