//! TOML config struct + parse/validate for the inert `binary_oracle_maker` strategy.
//!
//! Slice 1 (#488): the maker is registered-but-**inert** — it carries only the
//! minimal NautilusTrader `StrategyConfig` envelope (`strategy_id`,
//! `order_id_tag`, `oms_type`) that `StrategyCore::new` consumes. NO trading
//! parameters live here yet; later slices add quoting/pricing/exposure config.
//! Mirrors the taker config's parse/validate *shape* structurally — it does not
//! copy the taker's parameter rows.

use anyhow::{Context, Result};
use serde::Deserialize;
use toml::Value;

use crate::strategies::registry::ValidationError;

/// Minimal NautilusTrader config envelope for the inert maker. Only the fields
/// `BinaryOracleMaker::new` feeds into `StrategyCore::new(StrategyConfig { .. })`
/// are present; every other `StrategyConfig` field is left at NT's documented
/// default (see `StrategyConfig::default`). `deny_unknown_fields` fails loud on
/// any stray key so a typo in the operator TOML cannot be silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleMakerConfig {
    /// The NautilusTrader strategy id (becomes the running strategy's id).
    pub strategy_id: String,
    /// The unique order-id tag for the strategy.
    pub order_id_tag: String,
    /// The order-management-system type, parsed as a NautilusTrader `OmsType`.
    pub oms_type: String,
}

/// Zero-sized factory the `StrategyBuilder` trait is implemented for (in
/// `mod.rs`). Mirrors `BinaryOracleEdgeTakerBuilder`.
#[derive(Debug)]
pub struct BinaryOracleMakerBuilder;

const WRONG_TYPE_CODE: &str = "wrong_type";
const MISSING_STRATEGY_ID_CODE: &str = "missing_strategy_id";
const MISSING_ORDER_ID_TAG_CODE: &str = "missing_order_id_tag";
const MISSING_OMS_TYPE_CODE: &str = "missing_oms_type";
const UNKNOWN_FIELD_CODE: &str = "unknown_field";

const STRATEGY_ID_FIELD: &str = "strategy_id";
const ORDER_ID_TAG_FIELD: &str = "order_id_tag";
const OMS_TYPE_FIELD: &str = "oms_type";

/// Deserialize the maker config from its TOML table. Fails loud if the table is
/// missing required envelope fields or carries unknown keys (via
/// `deny_unknown_fields`).
pub fn parse_config(raw: &Value) -> Result<BinaryOracleMakerConfig> {
    raw.clone()
        .try_into()
        .context("binary_oracle_maker builder requires a valid config table")
}

/// Push envelope validation errors (missing field / wrong type / unknown key)
/// for the inert maker into `errors`. No parameter-row rules yet — the maker has
/// no trading parameters in Slice 1.
pub fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
    let Some(table) = raw.as_table() else {
        errors.push(ValidationError {
            field: field_prefix.to_string(),
            code: WRONG_TYPE_CODE,
            message: format!("must be a table, got {} value", raw.type_str()),
        });
        return;
    };

    for key in table.keys() {
        if !matches!(
            key.as_str(),
            STRATEGY_ID_FIELD | ORDER_ID_TAG_FIELD | OMS_TYPE_FIELD
        ) {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{key}"),
                code: UNKNOWN_FIELD_CODE,
                message: format!("unknown field `{key}`"),
            });
        }
    }

    validate_string_field(
        table,
        field_prefix,
        STRATEGY_ID_FIELD,
        MISSING_STRATEGY_ID_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        ORDER_ID_TAG_FIELD,
        MISSING_ORDER_ID_TAG_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        OMS_TYPE_FIELD,
        MISSING_OMS_TYPE_CODE,
        errors,
    );
}

fn validate_string_field(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    field_name: &str,
    missing_code: &'static str,
    errors: &mut Vec<ValidationError>,
) {
    let field = format!("{field_prefix}.{field_name}");
    match table.get(field_name) {
        None => errors.push(ValidationError {
            field,
            code: missing_code,
            message: format!("missing required field `{field_name}`"),
        }),
        Some(value) if value.as_str().is_none() => errors.push(ValidationError {
            field,
            code: WRONG_TYPE_CODE,
            message: format!("must be a string, got {} value", value.type_str()),
        }),
        Some(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_raw() -> Value {
        toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
        }
        .into()
    }

    #[test]
    fn parse_config_round_trips_minimal_envelope() {
        let config = parse_config(&minimal_raw()).expect("minimal envelope parses");
        assert_eq!(config.strategy_id, "BINARY-ORACLE-MAKER-001");
        assert_eq!(config.order_id_tag, "001");
        assert_eq!(config.oms_type, "netting");
    }

    #[test]
    fn validate_config_accepts_minimal_envelope() {
        let mut errors = Vec::new();
        validate_config(&minimal_raw(), "strategy", &mut errors);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn validate_config_flags_missing_field() {
        let raw: Value = toml::toml! {
            order_id_tag = "001"
            oms_type = "netting"
        }
        .into();
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.code == MISSING_STRATEGY_ID_CODE),
            "expected missing_strategy_id, got: {errors:?}"
        );
    }

    #[test]
    fn validate_config_flags_unknown_field() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            unexpected = "value"
        }
        .into();
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors.iter().any(|error| error.code == UNKNOWN_FIELD_CODE),
            "expected unknown_field, got: {errors:?}"
        );
    }
}
