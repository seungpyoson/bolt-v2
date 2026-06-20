//! TOML config struct + parse/validate for the `binary_oracle_maker` strategy.
//!
//! This is the **flat NautilusTrader config** the strategy consumes at build:
//! the `StrategyCore` envelope (`strategy_id`, `order_id_tag`, `oms_type`) plus
//! the μ-estimator / health-gate runtime knobs and market-portfolio policy the
//! archetype threads in from the operator `[strategies.<id>.parameters]` block
//! (Slices 2 and 9, #488). It is built by `archetype::raw_maker_config`, never
//! written by an operator directly.
//! `deny_unknown_fields` fails loud on any stray key.
//!
//! This struct validates only the flat table's **structure** (field presence,
//! TOML type, unknown keys, `oms_type` parseability). The operator-facing
//! **bounds** for the μ knobs live solely in `archetype::validate_strategy` (the
//! bolt-v3 go-live gate), so there is one home for each concern. Mirrors the
//! taker config's parse/validate *shape* structurally.

use anyhow::{Context, Result};
use nautilus_model::enums::OmsType;
use serde::Deserialize;
use toml::Value;

use super::binding::MakerMarketDeclaration;
use crate::strategies::registry::ValidationError;

/// Flat NautilusTrader config the maker consumes at build. The `StrategyConfig`
/// envelope fields `BinaryOracleMaker::new` feeds into `StrategyCore::new` plus
/// the μ runtime knobs `MakerMuState::new` projects into its estimator,
/// health-gate, and trade-flow config views, and the generic Slice 9
/// market-portfolio policy later runtime selection consumes. Every other
/// `StrategyConfig` field is left at NT's documented default (see
/// `StrategyConfig::default`).
/// `deny_unknown_fields` fails loud on any stray key so a typo in the operator
/// TOML cannot be silently ignored. `Eq` is intentionally not derived because
/// `mu_min_floor` is an `f64`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleMakerConfig {
    /// The NautilusTrader strategy id (becomes the running strategy's id).
    pub strategy_id: String,
    /// The unique order-id tag for the strategy.
    pub order_id_tag: String,
    /// The order-management-system type, parsed as a NautilusTrader `OmsType`.
    pub oms_type: String,
    /// The configured execution client id used for submit/cancel routing context.
    pub client_id: String,
    /// Signed-trade-flow retention window in seconds (μ estimator input).
    pub trade_flow_window_secs: u64,
    /// Signed-trade-flow retention sample cap (μ estimator input).
    pub trade_flow_max_samples: u64,
    /// Minimum classified (`Buyer`/`Seller`) samples before a μ is produced.
    pub mu_min_classified_samples: u64,
    /// Maximum age (ms) of the most recent trade before μ is considered stale.
    pub mu_stale_window_ms: u64,
    /// Lower bound μ must reach to be healthy (the degenerate-flow floor).
    pub mu_min_floor: f64,
    /// Minimum interval (ms) between requote actions on a leg. Sourced into the
    /// requote budget's same-tick throttle via `build_requote_budget_pair`; the
    /// submit-rate and venue REST caps are NOT config knobs — they come from
    /// `risk.nautilus.max_order_submit_rate` and the venue egress model.
    pub requote_min_interval_ms: u64,
    /// Interval (ms) of the maker's autonomous quote/refresh timer — how often the
    /// runtime re-resolves its active markets and (in later slices) requotes. The
    /// loop cadence; distinct from `requote_min_interval_ms`, which is a per-leg
    /// throttle floor inside a cycle, not the scheduling period.
    pub quote_interval_ms: u64,
    /// Maximum number of markets the portfolio selector may quote concurrently.
    pub market_portfolio_max_active_markets: u64,
    /// Total bankroll notional allocated to the market-selection portfolio.
    pub market_portfolio_total_bankroll_notional: f64,
    /// Minimum per-market slot notional after the bankroll split.
    pub market_portfolio_min_slot_notional: f64,
    /// Deterministic digest of the operator-declared `[[parameters.markets]]` set
    /// (canonical, sorted by `market_key`, all fields). It is threaded into this
    /// flat config by `archetype::raw_maker_config` PRECISELY so the declared
    /// market set is covered by the strategy-config hash the go-live gate binds:
    /// changing any declared market's family/underlying/cadence/slug/static field
    /// or the market count changes this digest, which changes the
    /// `strategy_config_hash`, which invalidates a backtest run captured for a
    /// DIFFERENT market set. Without it the gate would accept stale evidence for
    /// an untested market set.
    pub markets_config_digest: String,
    /// The operator-declared markets the maker quotes, threaded verbatim from the
    /// `[[parameters.markets]]` block by `archetype::raw_maker_config` so the
    /// runtime (`runtime::MakerRuntime`) can resolve them at `on_start` against the
    /// live instrument snapshot. `markets_config_digest` above binds the same set
    /// into the go-live hash; this carries the data the digest summarizes.
    pub markets: Vec<MakerMarketDeclaration>,
}

/// Zero-sized factory the `StrategyBuilder` trait is implemented for (in
/// `mod.rs`). Mirrors `BinaryOracleEdgeTakerBuilder`.
#[derive(Debug)]
pub struct BinaryOracleMakerBuilder;

const WRONG_TYPE_CODE: &str = "wrong_type";
const MISSING_STRATEGY_ID_CODE: &str = "missing_strategy_id";
const MISSING_ORDER_ID_TAG_CODE: &str = "missing_order_id_tag";
const MISSING_OMS_TYPE_CODE: &str = "missing_oms_type";
const MISSING_CLIENT_ID_CODE: &str = "missing_client_id";
const INVALID_OMS_TYPE_CODE: &str = "invalid_oms_type";
const INVALID_ORDER_ID_TAG_CODE: &str = "invalid_order_id_tag";
const UNKNOWN_FIELD_CODE: &str = "unknown_field";

const STRATEGY_ID_FIELD: &str = "strategy_id";
const ORDER_ID_TAG_FIELD: &str = "order_id_tag";
const OMS_TYPE_FIELD: &str = "oms_type";
const CLIENT_ID_FIELD: &str = "client_id";
const TRADE_FLOW_WINDOW_SECS_FIELD: &str = "trade_flow_window_secs";
const TRADE_FLOW_MAX_SAMPLES_FIELD: &str = "trade_flow_max_samples";
const MU_MIN_CLASSIFIED_SAMPLES_FIELD: &str = "mu_min_classified_samples";
const MU_STALE_WINDOW_MS_FIELD: &str = "mu_stale_window_ms";
const MU_MIN_FLOOR_FIELD: &str = "mu_min_floor";
const REQUOTE_MIN_INTERVAL_MS_FIELD: &str = "requote_min_interval_ms";
const QUOTE_INTERVAL_MS_FIELD: &str = "quote_interval_ms";
const MARKETS_FIELD: &str = "markets";
const MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD: &str = "market_portfolio_max_active_markets";
const MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD: &str =
    "market_portfolio_total_bankroll_notional";
const MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD: &str = "market_portfolio_min_slot_notional";
pub(crate) const MARKETS_CONFIG_DIGEST_FIELD: &str = "markets_config_digest";
const MISSING_MARKETS_CONFIG_DIGEST_CODE: &str = "missing_markets_config_digest";

/// Deserialize the maker config from its TOML table. Fails loud if the table is
/// missing required envelope fields or carries unknown keys (via
/// `deny_unknown_fields`).
pub fn parse_config(raw: &Value) -> Result<BinaryOracleMakerConfig> {
    raw.clone()
        .try_into()
        .context("binary_oracle_maker builder requires a valid config table")
}

/// Push **structure** validation errors for the flat maker config into `errors`:
/// unknown keys, plus envelope field presence/type and `oms_type` parseability.
/// The μ runtime knobs are whitelisted here (so the flat table built by
/// `archetype::raw_maker_config` is not flagged as carrying stray keys); their
/// presence and type are guaranteed by that construction and re-enforced by the
/// typed `deny_unknown_fields` deserialization at `parse_config`, and their
/// operator-facing **bounds** are validated upstream in
/// `archetype::validate_strategy` (one home per concern, no dual gate).
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
            STRATEGY_ID_FIELD
                | ORDER_ID_TAG_FIELD
                | OMS_TYPE_FIELD
                | CLIENT_ID_FIELD
                | TRADE_FLOW_WINDOW_SECS_FIELD
                | TRADE_FLOW_MAX_SAMPLES_FIELD
                | MU_MIN_CLASSIFIED_SAMPLES_FIELD
                | MU_STALE_WINDOW_MS_FIELD
                | MU_MIN_FLOOR_FIELD
                | REQUOTE_MIN_INTERVAL_MS_FIELD
                | QUOTE_INTERVAL_MS_FIELD
                | MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD
                | MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD
                | MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD
                | MARKETS_CONFIG_DIGEST_FIELD
                | MARKETS_FIELD
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
    validate_string_field(
        table,
        field_prefix,
        CLIENT_ID_FIELD,
        MISSING_CLIENT_ID_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        MARKETS_CONFIG_DIGEST_FIELD,
        MISSING_MARKETS_CONFIG_DIGEST_CODE,
        errors,
    );
    validate_order_id_tag_delimiter_free(table, field_prefix, errors);
    validate_oms_type_parses(table, field_prefix, errors);
}

/// Fail loud at load when `order_id_tag` contains the `-` delimiter the maker's
/// per-leg client order id is joined on (`runtime::make_leg_identity`). The tag is
/// the leading, free-form id component; constraining it to be delimiter-free keeps
/// the positional id encoding unambiguous (the remaining free-form component,
/// `market_key`, is right-anchored by the decimal window-start/generation and the
/// `yes`/`no` leg), so two distinct markets can never mint the same client order
/// id. A missing or non-string `order_id_tag` is already reported by
/// `validate_string_field`, so it is skipped here.
fn validate_order_id_tag_delimiter_free(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    errors: &mut Vec<ValidationError>,
) {
    let Some(value) = table.get(ORDER_ID_TAG_FIELD).and_then(Value::as_str) else {
        return;
    };
    if value.contains('-') {
        errors.push(ValidationError {
            field: format!("{field_prefix}.{ORDER_ID_TAG_FIELD}"),
            code: INVALID_ORDER_ID_TAG_CODE,
            message: format!("must not contain the `-` order-id delimiter, got `{value}`"),
        });
    }
}

/// Fail loud at load when `oms_type` is a string that does not parse as a
/// NautilusTrader `OmsType`. The string-presence/type check above only proves
/// `oms_type` is a string; `BinaryOracleMaker::new` then parses it with
/// `.expect(...)`, so an unparseable value that passed validation would panic at
/// build instead of surfacing as a clean validation error. Mirrors the taker's
/// `parse_configured_oms_type` (`value.parse::<OmsType>()`) so validation
/// guarantees exactly what build assumes. A missing or non-string `oms_type` is
/// already reported by `validate_string_field`, so it is skipped here.
fn validate_oms_type_parses(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    errors: &mut Vec<ValidationError>,
) {
    let Some(value) = table.get(OMS_TYPE_FIELD).and_then(Value::as_str) else {
        return;
    };
    if value.parse::<OmsType>().is_err() {
        errors.push(ValidationError {
            field: format!("{field_prefix}.{OMS_TYPE_FIELD}"),
            code: INVALID_OMS_TYPE_CODE,
            message: format!("must be a NautilusTrader OmsType, got `{value}`"),
        });
    }
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

    fn valid_raw() -> Value {
        toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            quote_interval_ms = 1000
            market_portfolio_max_active_markets = 3
            market_portfolio_total_bankroll_notional = 1500.0
            market_portfolio_min_slot_notional = 100.0
            markets_config_digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

            [[markets]]
            market_key = "eth-hourly"
            family_key = "updown"
            underlying_asset = "ETH"
            cadence_seconds = 3600
            cadence_slug_token = "1h"
        }
        .into()
    }

    #[test]
    fn parse_config_round_trips_full_config() {
        let config = parse_config(&valid_raw()).expect("valid config parses");
        assert_eq!(config.strategy_id, "BINARY-ORACLE-MAKER-001");
        assert_eq!(config.order_id_tag, "001");
        assert_eq!(config.oms_type, "netting");
        assert_eq!(config.client_id, "maker_execution_client");
        assert_eq!(config.trade_flow_window_secs, 600);
        assert_eq!(config.trade_flow_max_samples, 1000);
        assert_eq!(config.mu_min_classified_samples, 4);
        assert_eq!(config.mu_stale_window_ms, 60_000);
        assert_eq!(config.mu_min_floor, 0.05);
        assert_eq!(config.requote_min_interval_ms, 500);
        assert_eq!(config.quote_interval_ms, 1000);
        assert_eq!(config.market_portfolio_max_active_markets, 3);
        assert_eq!(config.market_portfolio_total_bankroll_notional, 1500.0);
        assert_eq!(config.market_portfolio_min_slot_notional, 100.0);
        assert_eq!(
            config.markets_config_digest,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(config.markets.len(), 1);
        assert_eq!(config.markets[0].market_key, "eth-hourly");
        assert_eq!(config.markets[0].family_key, "updown");
        assert_eq!(config.markets[0].underlying_asset, "ETH");
        assert_eq!(config.markets[0].cadence_seconds, 3600);
        assert_eq!(config.markets[0].cadence_slug_token, "1h");
        assert_eq!(config.markets[0].static_condition_id, None);
    }

    #[test]
    fn validate_config_accepts_valid_config() {
        let mut errors = Vec::new();
        validate_config(&valid_raw(), "strategy", &mut errors);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn validate_config_rejects_order_id_tag_with_delimiter() {
        // The maker joins the per-leg client order id on `-`
        // (`runtime::make_leg_identity`); an `order_id_tag` containing the delimiter
        // would make the positional encoding ambiguous, so it must fail loud at
        // config load rather than risk minting colliding ids. Without the
        // delimiter-free check this passes validation, so the assertion is
        // differential.
        let mut raw = valid_raw();
        raw.as_table_mut()
            .expect("config is a table")
            .insert("order_id_tag".to_string(), Value::String("00-1".to_string()));
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.code == INVALID_ORDER_ID_TAG_CODE),
            "a hyphenated order_id_tag must be rejected: {errors:?}"
        );
    }

    #[test]
    fn parse_config_rejects_missing_mu_knob() {
        // The flat config requires every μ knob (non-Option, deny_unknown_fields);
        // a table missing one fails loud at parse rather than building a maker
        // with an unspecified μ knob.
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            requote_min_interval_ms = 500
            market_portfolio_max_active_markets = 3
            market_portfolio_total_bankroll_notional = 1500.0
            market_portfolio_min_slot_notional = 100.0
        }
        .into();
        assert!(
            parse_config(&raw).is_err(),
            "missing mu_min_floor must fail to parse"
        );
    }

    #[test]
    fn parse_config_rejects_missing_market_portfolio_knob() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            market_portfolio_max_active_markets = 3
            market_portfolio_total_bankroll_notional = 1500.0
        }
        .into();
        assert!(
            parse_config(&raw).is_err(),
            "missing market_portfolio_min_slot_notional must fail to parse"
        );
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
    fn validate_config_flags_unparseable_oms_type() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "not-an-oms-type"
            client_id = "maker_execution_client"
        }
        .into();
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.code == INVALID_OMS_TYPE_CODE),
            "expected invalid_oms_type, got: {errors:?}"
        );
    }

    #[test]
    fn validate_config_accepts_parseable_oms_type() {
        // The minimal envelope's `oms_type = "netting"` must parse as an NT
        // OmsType so build's `.expect(...)` can never fire on validated config.
        assert!(
            "netting".parse::<OmsType>().is_ok(),
            "minimal envelope oms_type must parse as an NT OmsType"
        );
        let mut errors = Vec::new();
        validate_config(&valid_raw(), "strategy", &mut errors);
        assert!(
            !errors
                .iter()
                .any(|error| error.code == INVALID_OMS_TYPE_CODE),
            "unexpected invalid_oms_type: {errors:?}"
        );
    }

    #[test]
    fn validate_config_flags_unknown_field() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
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
