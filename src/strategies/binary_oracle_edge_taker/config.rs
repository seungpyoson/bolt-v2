//! TOML config structs + parse/validate for the binary-oracle-edge-taker strategy.
//!
//! A8 slice (#522): the config field-set macros, the deserialized config structs,
//! the field-type matcher, and the `BinaryOracleEdgeTakerBuilder` parse/validate
//! machinery moved verbatim out of `mod.rs`. The `StrategyBuilder` trait impl
//! (build/register/kind/validate_config) and the order-construction `*_from_config`
//! helpers stay in `mod.rs`/their own slices; this module is pure config parse+validate.

use anyhow::{Context, Result};
use nautilus_model::{
    enums::{OrderType, TimeInForce, TrailingOffsetType, TriggerType},
    identifiers::InstrumentId,
};
use serde::Deserialize;
use toml::Value;

use crate::{
    bolt_v3_market_families, bolt_v3_numeric::is_positive_finite,
    strategies::registry::ValidationError,
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

macro_rules! binary_oracle_edge_taker_config_fields {
    ($macro:ident) => {
        $macro! {
            strategy_id: String => String;
            order_id_tag: String => String;
            oms_type: String => String;
            client_id: String => String;
            configured_target_id: String => String;
            target_kind: String => String;
            rotating_market_family: String => String;
            underlying_asset: String => String;
            cadence_seconds: u64 => Integer;
            cadence_slug_token: String => String;
            market_selection_rule: String => String;
            retry_interval_seconds: u64 => Integer;
            blocked_after_seconds: u64 => Integer;
            use_uuid_client_order_ids: bool => Boolean;
            use_hyphens_in_client_order_ids: bool => Boolean;
            external_order_claims: Vec<String> => Array;
            manage_contingent_orders: bool => Boolean;
            manage_gtd_expiry: bool => Boolean;
            manage_stop: bool => Boolean;
            market_exit_interval_ms: u64 => Integer;
            market_exit_max_attempts: u64 => Integer;
            log_events: bool => Boolean;
            log_commands: bool => Boolean;
            log_rejected_due_post_only_as_warning: bool => Boolean;
            warmup_tick_count: u64 => Integer;
            reentry_cooldown_secs: u64 => Integer;
            order_notional_target: f64 => Float;
            maximum_position_notional: f64 => Float;
            book_impact_cap_bps: u64 => Integer;
            risk_lambda: f64 => Float;
            edge_threshold_basis_points: i64 => Integer;
            exit_hysteresis_bps: i64 => Integer;
            vol_window_secs: Option<u64> => Integer;
            vol_gap_reset_secs: Option<u64> => Integer;
            vol_min_observations: Option<u64> => Integer;
            vol_bridge_valid_secs: Option<u64> => Integer;
            trade_flow_window_secs: u64 => Integer;
            trade_flow_max_samples: u64 => Integer;
            spike_guard_return_threshold: f64 => Float;
            spike_guard_cooldown_secs: u64 => Integer;
            price_to_beat_source: String => String;
            pricing_kurtosis: f64 => Float;
            theta_decay_factor: f64 => Float;
            forced_flat_stale_reference_ms: u64 => Integer;
            forced_flat_thin_book_min_liquidity: f64 => Float;
            lead_agreement_min_corr: f64 => Float;
            lead_jitter_max_ms: u64 => Integer;
        }
    };
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BinaryOracleEdgeTakerOrderConfig {
    pub(super) side: String,
    pub(super) position_side: String,
    pub(super) order_type: OrderType,
    pub(super) time_in_force: TimeInForce,
    pub(super) expire_time_unix_nanos: Option<u64>,
    pub(super) trigger_price: Option<f64>,
    pub(super) activation_price: Option<f64>,
    pub(super) trigger_type: Option<TriggerType>,
    pub(super) trigger_instrument_id: Option<InstrumentId>,
    pub(super) trailing_offset: Option<f64>,
    pub(super) trailing_offset_type: Option<TrailingOffsetType>,
    pub(super) is_post_only: bool,
    pub(super) is_reduce_only: bool,
    pub(super) is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BinaryOracleEdgeTakerFieldType {
    String,
    Boolean,
    Integer,
    Float,
    Array,
    Table,
}

impl BinaryOracleEdgeTakerFieldType {
    fn expected(self) -> &'static str {
        match self {
            Self::String => stringify!(string),
            Self::Boolean => stringify!(boolean),
            Self::Integer => stringify!(integer),
            Self::Float => stringify!(float),
            Self::Array => stringify!(array),
            Self::Table => stringify!(table),
        }
    }

    fn article(self) -> &'static str {
        match self {
            Self::String | Self::Boolean | Self::Float | Self::Table => stringify!(a),
            Self::Integer | Self::Array => stringify!(an),
        }
    }

    fn matches(self, value: &Value) -> bool {
        match self {
            Self::String => value.as_str().is_some(),
            Self::Boolean => value.as_bool().is_some(),
            Self::Integer => value.as_integer().is_some(),
            Self::Float => value.as_float_or_integer().is_some(),
            Self::Array => value.as_array().is_some(),
            Self::Table => value.as_table().is_some(),
        }
    }
}

macro_rules! define_config_struct {
    ($( $field:ident : $ty:ty => $field_type:ident; )+) => {
        #[derive(Debug, Clone, PartialEq, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(super) struct BinaryOracleEdgeTakerConfig {
            $( pub(super) $field: $ty, )+
            pub(super) reference_venue: Option<String>,
            pub(super) reference_instrument_id: Option<String>,
            pub(super) signal_venue: Option<String>,
            pub(super) signal_instrument_id: Option<String>,
            pub(super) resolution_client_id: Option<String>,
            pub(super) resolution_instrument_id: Option<String>,
            pub(super) realized_volatility_surface_id: Option<String>,
            pub(super) entry_order: BinaryOracleEdgeTakerOrderConfig,
            pub(super) exit_order: BinaryOracleEdgeTakerOrderConfig,
            pub(super) forced_exit_order: BinaryOracleEdgeTakerOrderConfig,
        }
    };
}

macro_rules! match_config_field_names {
    ($( $field:ident : $ty:ty => $field_type:ident; )+) => {
        $( stringify!($field) )|+
    };
}

macro_rules! validate_config_fields_impl {
    ($( $field:ident : $ty:ty => $field_type:ident; )+) => {
        |table: &toml::map::Map<String, Value>, field_prefix: &str, errors: &mut Vec<ValidationError>| {
            $(
                let field = format!("{field_prefix}.{}", stringify!($field));
                let field_type = BinaryOracleEdgeTakerFieldType::$field_type;
                match table.get(stringify!($field)) {
                    None => BinaryOracleEdgeTakerBuilder::push_missing(
                        errors,
                        field,
                        concat!(stringify!(missing_), stringify!($field)),
                        field_type,
                    ),
                    Some(value) if !field_type.matches(value) => {
                        BinaryOracleEdgeTakerBuilder::push_wrong_type(
                            errors,
                            field,
                            field_type,
                            value,
                        );
                    }
                    Some(_) => {}
                }
            )+
        }
    };
}

macro_rules! binary_oracle_edge_taker_order_fields {
    ($macro:ident) => {
        $macro! {
            side => String;
            position_side => String;
            order_type => String;
            time_in_force => String;
            is_post_only => Boolean;
            is_reduce_only => Boolean;
            is_quote_quantity => Boolean;
        }
    };
}

const ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD: &str = "expire_time_unix_nanos";
const ORDER_TRIGGER_PRICE_FIELD: &str = "trigger_price";
const ORDER_ACTIVATION_PRICE_FIELD: &str = "activation_price";
const ORDER_TRIGGER_TYPE_FIELD: &str = "trigger_type";
const ORDER_TRIGGER_INSTRUMENT_ID_FIELD: &str = "trigger_instrument_id";
const ORDER_TRAILING_OFFSET_FIELD: &str = "trailing_offset";
const ORDER_TRAILING_OFFSET_TYPE_FIELD: &str = "trailing_offset_type";
const REALIZED_VOLATILITY_SURFACE_ID_FIELD: &str = "realized_volatility_surface_id";
const LEGACY_REALIZED_VOLATILITY_RUNTIME_FIELDS: &[&str] = &[
    "vol_window_secs",
    "vol_gap_reset_secs",
    "vol_min_observations",
    "vol_bridge_valid_secs",
    "signal_venue",
    "signal_instrument_id",
];

macro_rules! match_order_field_names {
    ($( $field:ident => $field_type:ident; )+) => {
        $( stringify!($field) )|+
    };
}

macro_rules! validate_order_fields_impl {
    ($( $field:ident => $field_type:ident; )+) => {
        |table: &toml::map::Map<String, Value>,
         field_prefix: &str,
         errors: &mut Vec<ValidationError>| {
            $(
                BinaryOracleEdgeTakerBuilder::validate_order_field(
                    table,
                    field_prefix,
                    stringify!($field),
                    concat!(stringify!(missing_), stringify!($field)),
                    BinaryOracleEdgeTakerFieldType::$field_type,
                    errors,
                );
            )+
        }
    };
}

binary_oracle_edge_taker_config_fields!(define_config_struct);

#[derive(Debug)]
pub struct BinaryOracleEdgeTakerBuilder;

const ENTRY_ORDER_FIELD: &str = stringify!(entry_order);
const EXIT_ORDER_FIELD: &str = stringify!(exit_order);
const FORCED_EXIT_ORDER_FIELD: &str = stringify!(forced_exit_order);
const WRONG_TYPE_CODE: &str = stringify!(wrong_type);
const UNKNOWN_FIELD_CODE: &str = stringify!(unknown_field);
const INVALID_INSTRUMENT_ID_CODE: &str = stringify!(invalid_instrument_id);

impl BinaryOracleEdgeTakerBuilder {
    pub(super) fn parse_config(raw: &Value) -> Result<BinaryOracleEdgeTakerConfig> {
        let config: BinaryOracleEdgeTakerConfig = raw
            .clone()
            .try_into()
            .context("binary_oracle_edge_taker builder requires a valid config table")?;
        // Fail loud at load: a non-positive spike_guard_return_threshold makes the
        // spike guard's `relative_move >= threshold` test (relative_move is an
        // abs(), always >= 0) always true, arming the cooldown on every reference
        // quote and silently blocking all entry. The TOML type check accepts
        // 0.0/negatives, so the range is validated here, matching the build-path
        // `is_positive_finite` precedent (price_from_config / trailing_offset_from_config).
        anyhow::ensure!(
            is_positive_finite(config.spike_guard_return_threshold),
            "spike_guard_return_threshold must be positive and finite"
        );
        // Fail loud at load for positive-required integer knobs. A zero
        // trade-flow sample cap makes the count-cap evict every observation,
        // permanently emptying the buffer and starving the W3 read seam.
        for (field, value) in [
            (
                stringify!(trade_flow_max_samples),
                Some(config.trade_flow_max_samples),
            ),
            (
                stringify!(trade_flow_window_secs),
                Some(config.trade_flow_window_secs),
            ),
            (
                stringify!(spike_guard_cooldown_secs),
                Some(config.spike_guard_cooldown_secs),
            ),
            (stringify!(vol_window_secs), config.vol_window_secs),
            (stringify!(vol_gap_reset_secs), config.vol_gap_reset_secs),
            (
                stringify!(vol_min_observations),
                config.vol_min_observations,
            ),
            (
                stringify!(vol_bridge_valid_secs),
                config.vol_bridge_valid_secs,
            ),
        ] {
            if let Some(value) = value {
                anyhow::ensure!(value > u64::MIN, "{field} must be positive");
            }
        }
        let legacy_vol_fields = [
            (stringify!(vol_window_secs), config.vol_window_secs),
            (stringify!(vol_gap_reset_secs), config.vol_gap_reset_secs),
            (
                stringify!(vol_min_observations),
                config.vol_min_observations,
            ),
            (
                stringify!(vol_bridge_valid_secs),
                config.vol_bridge_valid_secs,
            ),
        ];
        if config.realized_volatility_surface_id.is_none() {
            for (field, value) in legacy_vol_fields {
                anyhow::ensure!(value.is_some(), "{field} is required");
            }
        } else {
            for (field, value) in legacy_vol_fields {
                anyhow::ensure!(
                    value.is_none(),
                    "{field} is rejected when realized_volatility_surface_id is configured"
                );
            }
        }
        Self::ensure_configured_instrument_id_fields_parse(&config)?;
        Ok(config)
    }

    fn ensure_configured_instrument_id_fields_parse(
        config: &BinaryOracleEdgeTakerConfig,
    ) -> Result<()> {
        for (field_name, instrument_id) in [
            (
                "reference_instrument_id",
                config.reference_instrument_id.as_deref(),
            ),
            (
                "signal_instrument_id",
                config.signal_instrument_id.as_deref(),
            ),
            (
                "resolution_instrument_id",
                config.resolution_instrument_id.as_deref(),
            ),
        ] {
            let Some(instrument_id) = instrument_id else {
                continue;
            };
            anyhow::ensure!(
                instrument_id.parse::<InstrumentId>().is_ok(),
                "{field_name} must be a valid NT instrument id, got `{instrument_id}`"
            );
        }
        Ok(())
    }

    fn push_missing(
        errors: &mut Vec<ValidationError>,
        field: String,
        code: &'static str,
        field_type: BinaryOracleEdgeTakerFieldType,
    ) {
        errors.push(ValidationError {
            field,
            code,
            message: format!("is missing required {} field", field_type.expected()),
        });
    }

    pub(super) fn push_wrong_type(
        errors: &mut Vec<ValidationError>,
        field: String,
        field_type: BinaryOracleEdgeTakerFieldType,
        value: &Value,
    ) {
        errors.push(ValidationError {
            field,
            code: WRONG_TYPE_CODE,
            message: format!(
                "must be {} {}, got {} value",
                field_type.article(),
                field_type.expected(),
                value.type_str()
            ),
        });
    }

    fn push_unknown_field(errors: &mut Vec<ValidationError>, field: String, key: &str) {
        errors.push(ValidationError {
            field,
            code: UNKNOWN_FIELD_CODE,
            message: format!("unknown field `{key}`"),
        });
    }

    pub(super) fn validate_table(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        for key in table.keys() {
            if !matches!(
                key.as_str(),
                ENTRY_ORDER_FIELD
                    | EXIT_ORDER_FIELD
                    | FORCED_EXIT_ORDER_FIELD
                    | "reference_venue"
                    | "reference_instrument_id"
                    | "signal_venue"
                    | "signal_instrument_id"
                    | "resolution_client_id"
                    | "resolution_instrument_id"
                    | REALIZED_VOLATILITY_SURFACE_ID_FIELD
                    | binary_oracle_edge_taker_config_fields!(match_config_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field_prefix}.{key}"), key);
            }
        }

        let surfaced_realized_volatility = table.contains_key(REALIZED_VOLATILITY_SURFACE_ID_FIELD);
        binary_oracle_edge_taker_config_fields!(validate_config_fields_impl)(
            table,
            field_prefix,
            errors,
        );
        if surfaced_realized_volatility {
            Self::suppress_legacy_realized_volatility_missing_errors(field_prefix, errors);
        }
        Self::validate_optional_string_field(table, field_prefix, "reference_venue", errors);
        Self::validate_optional_string_field(
            table,
            field_prefix,
            "reference_instrument_id",
            errors,
        );
        Self::validate_optional_string_field(table, field_prefix, "signal_venue", errors);
        Self::validate_optional_string_field(table, field_prefix, "signal_instrument_id", errors);
        Self::validate_optional_string_field(table, field_prefix, "resolution_client_id", errors);
        Self::validate_optional_string_field(
            table,
            field_prefix,
            "resolution_instrument_id",
            errors,
        );
        Self::validate_optional_string_field(
            table,
            field_prefix,
            REALIZED_VOLATILITY_SURFACE_ID_FIELD,
            errors,
        );
        if surfaced_realized_volatility {
            Self::validate_no_legacy_realized_volatility_fields(table, field_prefix, errors);
        }
        Self::validate_optional_instrument_id_field(
            table,
            field_prefix,
            "reference_instrument_id",
            errors,
        );
        Self::validate_optional_instrument_id_field(
            table,
            field_prefix,
            "signal_instrument_id",
            errors,
        );
        Self::validate_optional_instrument_id_field(
            table,
            field_prefix,
            "resolution_instrument_id",
            errors,
        );
        if table.contains_key("reference_venue") != table.contains_key("reference_instrument_id") {
            let missing = if table.contains_key("reference_venue") {
                "reference_instrument_id"
            } else {
                "reference_venue"
            };
            Self::push_missing(
                errors,
                format!("{field_prefix}.{missing}"),
                "missing_reference_data_pair",
                BinaryOracleEdgeTakerFieldType::String,
            );
        }
        match (
            table.contains_key("signal_venue"),
            table.contains_key("signal_instrument_id"),
        ) {
            (true, true) if surfaced_realized_volatility => {}
            (false, false) if surfaced_realized_volatility => {}
            (true, true) => {}
            (true, false) => Self::push_missing(
                errors,
                format!("{field_prefix}.signal_instrument_id"),
                "missing_signal_data_pair",
                BinaryOracleEdgeTakerFieldType::String,
            ),
            (false, true) | (false, false) => Self::push_missing(
                errors,
                format!("{field_prefix}.signal_venue"),
                "missing_signal_data_pair",
                BinaryOracleEdgeTakerFieldType::String,
            ),
        }
        // Resolution-strike binding is optional, but both-or-neither: a strategy
        // either declares the live Chainlink strike (resolution_client_id +
        // resolution_instrument_id) or neither (entry stays fail-closed). Mirrors
        // the reference_data pair rule.
        if table.contains_key("resolution_client_id")
            != table.contains_key("resolution_instrument_id")
        {
            let missing = if table.contains_key("resolution_client_id") {
                "resolution_instrument_id"
            } else {
                "resolution_client_id"
            };
            Self::push_missing(
                errors,
                format!("{field_prefix}.{missing}"),
                "missing_resolution_data_pair",
                BinaryOracleEdgeTakerFieldType::String,
            );
        }
        Self::validate_order_table(
            table,
            field_prefix,
            ENTRY_ORDER_FIELD,
            concat!(stringify!(missing_), stringify!(entry_order)),
            errors,
        );
        Self::validate_order_table(
            table,
            field_prefix,
            EXIT_ORDER_FIELD,
            concat!(stringify!(missing_), stringify!(exit_order)),
            errors,
        );
        Self::validate_order_table(
            table,
            field_prefix,
            FORCED_EXIT_ORDER_FIELD,
            concat!(stringify!(missing_), stringify!(forced_exit_order)),
            errors,
        );
        Self::validate_rotating_market_family(table, field_prefix, errors);
    }

    fn validate_no_legacy_realized_volatility_fields(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        for field_name in LEGACY_REALIZED_VOLATILITY_RUNTIME_FIELDS {
            if table.contains_key(*field_name) {
                errors.push(ValidationError {
                    field: format!("{field_prefix}.{field_name}"),
                    code: "legacy_realized_volatility_path",
                    message: format!(
                        "`{field_name}` is rejected when `{REALIZED_VOLATILITY_SURFACE_ID_FIELD}` selects surfaced realized-volatility mode"
                    ),
                });
            }
        }
    }

    fn suppress_legacy_realized_volatility_missing_errors(
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        errors.retain(|error| {
            !LEGACY_REALIZED_VOLATILITY_RUNTIME_FIELDS
                .iter()
                .any(|field_name| error.field == format!("{field_prefix}.{field_name}"))
        });
    }

    /// Reject an unknown `rotating_market_family` at config-parse time (P5-10).
    /// Startup market-identity construction already fails loud on an unknown
    /// family, so this is defense-in-depth that converges parse-time validation
    /// with the SINGLE registry source of truth
    /// (`bolt_v3_market_families::validation_bindings`): a family the registry
    /// does not bind can never be selected or traded, so it must be rejected here
    /// rather than accepted and only caught later.
    fn validate_rotating_market_family(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let field_name = stringify!(rotating_market_family);
        let Some(value) = table.get(field_name) else {
            // Presence/type is enforced by the generated field validator; an
            // absent or non-string value is reported there, not here.
            return;
        };
        let Some(family) = value.as_str() else {
            return;
        };
        let is_known = bolt_v3_market_families::validation_bindings()
            .iter()
            .any(|binding| binding.key == family);
        if !is_known {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{field_name}"),
                code: stringify!(unknown_market_family),
                message: format!("unknown market family `{family}`"),
            });
        }
    }

    fn validate_optional_string_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        if let Some(value) = table.get(field_name)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field_prefix}.{field_name}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
    }

    fn validate_optional_instrument_id_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        let Some(value) = table.get(field_name).and_then(Value::as_str) else {
            return;
        };
        if value.parse::<InstrumentId>().is_err() {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{field_name}"),
                code: INVALID_INSTRUMENT_ID_CODE,
                message: format!("must be a valid NT instrument id, got `{value}`"),
            });
        }
    }

    fn validate_order_table(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        missing_code: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        let field = format!("{field_prefix}.{field_name}");
        let Some(value) = table.get(field_name) else {
            Self::push_missing(
                errors,
                field,
                missing_code,
                BinaryOracleEdgeTakerFieldType::Table,
            );
            return;
        };
        let Some(order_table) = value.as_table() else {
            Self::push_wrong_type(errors, field, BinaryOracleEdgeTakerFieldType::Table, value);
            return;
        };

        for key in order_table.keys() {
            if !matches!(
                key.as_str(),
                ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD
                    | ORDER_TRIGGER_PRICE_FIELD
                    | ORDER_ACTIVATION_PRICE_FIELD
                    | ORDER_TRIGGER_TYPE_FIELD
                    | ORDER_TRIGGER_INSTRUMENT_ID_FIELD
                    | ORDER_TRAILING_OFFSET_FIELD
                    | ORDER_TRAILING_OFFSET_TYPE_FIELD
                    | binary_oracle_edge_taker_order_fields!(match_order_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field}.{key}"), key);
            }
        }

        binary_oracle_edge_taker_order_fields!(validate_order_fields_impl)(
            order_table,
            &field,
            errors,
        );
        if let Some(value) = order_table.get(ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Integer.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Integer,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRIGGER_PRICE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Float.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRIGGER_PRICE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Float,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_ACTIVATION_PRICE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Float.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_ACTIVATION_PRICE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Float,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRIGGER_TYPE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRIGGER_TYPE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRIGGER_INSTRUMENT_ID_FIELD)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRIGGER_INSTRUMENT_ID_FIELD}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRAILING_OFFSET_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Float.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRAILING_OFFSET_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Float,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRAILING_OFFSET_TYPE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRAILING_OFFSET_TYPE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
    }

    fn validate_order_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        missing_code: &'static str,
        field_type: BinaryOracleEdgeTakerFieldType,
        errors: &mut Vec<ValidationError>,
    ) {
        let field = format!("{field_prefix}.{field_name}");
        match table.get(field_name) {
            None => Self::push_missing(errors, field, missing_code, field_type),
            Some(value) if !field_type.matches(value) => {
                Self::push_wrong_type(errors, field, field_type, value);
            }
            Some(_) => {}
        }
    }
}
