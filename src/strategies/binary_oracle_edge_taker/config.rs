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
    bolt_v3_config::ExposureObligationLimits, bolt_v3_strategy_context::StrategyBuildContext,
};

use crate::{
    bolt_v3_config::ReferencePriceBlock,
    bolt_v3_market_families,
    bolt_v3_numeric::{BPS_DENOMINATOR, is_non_negative_finite, is_positive_finite},
    strategies::registry::ValidationError,
};

use super::BinaryOracleEdgeTaker;

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
            vwap_depth_limit_bps: u64 => Integer;
            slippage_buffer_bps: u64 => Integer;
            risk_lambda: f64 => Float;
            sizing_ev_reference_bps: u64 => Integer;
            edge_threshold_basis_points: i64 => Integer;
            exit_hysteresis_bps: i64 => Integer;
            trade_flow_window_secs: u64 => Integer;
            trade_flow_max_samples: u64 => Integer;
            spike_guard_return_threshold: f64 => Float;
            spike_guard_cooldown_secs: u64 => Integer;
            price_to_beat_source: String => String;
            realized_volatility_max_source_age_ms: u64 => Integer;
            pricing_kurtosis: f64 => Float;
            theta_decay_factor: f64 => Float;
            forced_flat_stale_reference_ms: u64 => Integer;
            forced_flat_thin_book_min_liquidity: f64 => Float;
            lead_agreement_min_corr: f64 => Float;
            lead_jitter_max_ms: u64 => Integer;
            exposure_obligations: ExposureObligationLimits => Table;
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
            pub(super) signal_venue: Option<String>,
            pub(super) signal_instrument_id: Option<String>,
            pub(super) resolution_client_id: Option<String>,
            pub(super) resolution_instrument_id: Option<String>,
            pub(super) reference_current_price: Option<ReferencePriceBlock>,
            pub(super) realized_volatility_surface_id: String,
            pub(super) static_condition_id: Option<String>,
            pub(super) static_yes_outcome: Option<String>,
            pub(super) static_no_outcome: Option<String>,
            pub(super) static_fair_probability_source: Option<String>,
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
const UNSUPPORTED_EXECUTABLE_ENTRY_ORDER_SHAPE_CODE: &str =
    stringify!(unsupported_executable_entry_order_shape);
const UNSUPPORTED_EXECUTABLE_EXIT_ORDER_SHAPE_CODE: &str =
    stringify!(unsupported_executable_exit_order_shape);
const ORDER_SIDE_BUY_VALUE: &str = stringify!(buy);
const POSITION_SIDE_LONG_VALUE: &str = stringify!(long);

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
        anyhow::ensure!(
            is_non_negative_finite(config.risk_lambda),
            "risk_lambda must be finite and >= 0"
        );
        anyhow::ensure!(
            is_non_negative_finite(config.forced_flat_thin_book_min_liquidity),
            "forced_flat_thin_book_min_liquidity must be finite and >= 0"
        );
        anyhow::ensure!(
            config.edge_threshold_basis_points >= 0,
            "edge_threshold_basis_points must be >= 0"
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
            (
                stringify!(retry_interval_seconds),
                Some(config.retry_interval_seconds),
            ),
            (
                stringify!(market_exit_max_attempts),
                Some(config.market_exit_max_attempts),
            ),
            (
                stringify!(sizing_ev_reference_bps),
                Some(config.sizing_ev_reference_bps),
            ),
            (
                stringify!(realized_volatility_max_source_age_ms),
                Some(config.realized_volatility_max_source_age_ms),
            ),
        ] {
            if let Some(value) = value {
                anyhow::ensure!(value > u64::MIN, "{field} must be positive");
            }
        }
        Self::ensure_bps_runtime_knobs_within_full_scale(&config)?;
        Self::ensure_executable_entry_order_shape(&config)?;
        Self::ensure_executable_exit_order_shapes(&config)?;
        Self::ensure_configured_instrument_id_fields_parse(&config)?;
        Ok(config)
    }

    pub fn build_strategy(
        raw: &Value,
        context: &StrategyBuildContext,
    ) -> Result<BinaryOracleEdgeTaker> {
        Ok(BinaryOracleEdgeTaker::new(
            Self::parse_config(raw)?,
            context.clone(),
        ))
    }

    fn ensure_bps_runtime_knobs_within_full_scale(
        config: &BinaryOracleEdgeTakerConfig,
    ) -> Result<()> {
        for (field, value) in [
            (stringify!(book_impact_cap_bps), config.book_impact_cap_bps),
            (
                stringify!(vwap_depth_limit_bps),
                config.vwap_depth_limit_bps,
            ),
            (stringify!(slippage_buffer_bps), config.slippage_buffer_bps),
            (
                stringify!(sizing_ev_reference_bps),
                config.sizing_ev_reference_bps,
            ),
        ] {
            anyhow::ensure!(
                (value as f64) <= BPS_DENOMINATOR,
                "{field} must be at most {BPS_DENOMINATOR}"
            );
        }
        anyhow::ensure!(
            config.slippage_buffer_bps >= config.vwap_depth_limit_bps,
            "slippage_buffer_bps must be greater than or equal to vwap_depth_limit_bps"
        );
        Ok(())
    }

    fn ensure_executable_entry_order_shape(config: &BinaryOracleEdgeTakerConfig) -> Result<()> {
        anyhow::ensure!(
            Self::entry_order_shape_supported(&config.entry_order),
            "{ENTRY_ORDER_FIELD} must be buy/long market FOK quote-quantity without post-only, reduce-only, trigger, or trailing fields"
        );
        Ok(())
    }

    pub(super) fn entry_order_shape_supported(order: &BinaryOracleEdgeTakerOrderConfig) -> bool {
        order.side == ORDER_SIDE_BUY_VALUE
            && order.position_side == POSITION_SIDE_LONG_VALUE
            && order.order_type == OrderType::Market
            && order.time_in_force == TimeInForce::Fok
            && !order.is_post_only
            && !order.is_reduce_only
            && order.is_quote_quantity
            && order.trigger_price.is_none()
            && order.activation_price.is_none()
            && order.trigger_type.is_none()
            && order.trigger_instrument_id.is_none()
            && order.trailing_offset.is_none()
            && order.trailing_offset_type.is_none()
    }

    fn ensure_executable_exit_order_shapes(config: &BinaryOracleEdgeTakerConfig) -> Result<()> {
        for (field, order) in [
            (EXIT_ORDER_FIELD, &config.exit_order),
            (FORCED_EXIT_ORDER_FIELD, &config.forced_exit_order),
        ] {
            anyhow::ensure!(
                Self::exit_order_shape_supported(order),
                "{field} must be market IOC base-quantity without post-only, trigger, or trailing fields"
            );
        }
        Ok(())
    }

    pub(super) fn exit_order_shape_supported(order: &BinaryOracleEdgeTakerOrderConfig) -> bool {
        order.order_type == OrderType::Market
            && order.time_in_force == TimeInForce::Ioc
            && !order.is_post_only
            && !order.is_quote_quantity
            && order.trigger_price.is_none()
            && order.activation_price.is_none()
            && order.trigger_type.is_none()
            && order.trigger_instrument_id.is_none()
            && order.trailing_offset.is_none()
            && order.trailing_offset_type.is_none()
    }

    fn ensure_configured_instrument_id_fields_parse(
        config: &BinaryOracleEdgeTakerConfig,
    ) -> Result<()> {
        for (field_name, instrument_id) in [
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
                    | "signal_venue"
                    | "signal_instrument_id"
                    | "resolution_client_id"
                    | "resolution_instrument_id"
                    | "reference_current_price"
                    | REALIZED_VOLATILITY_SURFACE_ID_FIELD
                    | "static_condition_id"
                    | "static_yes_outcome"
                    | "static_no_outcome"
                    | "static_fair_probability_source"
                    | binary_oracle_edge_taker_config_fields!(match_config_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field_prefix}.{key}"), key);
            }
        }

        binary_oracle_edge_taker_config_fields!(validate_config_fields_impl)(
            table,
            field_prefix,
            errors,
        );
        for field_name in [
            stringify!(book_impact_cap_bps),
            stringify!(vwap_depth_limit_bps),
            stringify!(slippage_buffer_bps),
            stringify!(sizing_ev_reference_bps),
        ] {
            Self::validate_bps_runtime_knob_upper_bound(table, field_prefix, field_name, errors);
        }
        Self::validate_positive_u64_field(
            table,
            field_prefix,
            stringify!(retry_interval_seconds),
            errors,
        );
        Self::validate_positive_u64_field(
            table,
            field_prefix,
            stringify!(market_exit_max_attempts),
            errors,
        );
        Self::validate_positive_u64_field(
            table,
            field_prefix,
            stringify!(sizing_ev_reference_bps),
            errors,
        );
        Self::validate_positive_u64_field(
            table,
            field_prefix,
            stringify!(realized_volatility_max_source_age_ms),
            errors,
        );
        Self::validate_non_negative_finite_float_field(
            table,
            field_prefix,
            stringify!(risk_lambda),
            errors,
        );
        Self::validate_non_negative_finite_float_field(
            table,
            field_prefix,
            stringify!(forced_flat_thin_book_min_liquidity),
            errors,
        );
        if table
            .get(stringify!(edge_threshold_basis_points))
            .and_then(Value::as_integer)
            .is_some_and(|value| value < 0)
        {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{}", stringify!(edge_threshold_basis_points)),
                code: "out_of_range",
                message: "must be >= 0".to_string(),
            });
        }
        Self::validate_optional_string_field(table, field_prefix, "signal_venue", errors);
        Self::validate_optional_string_field(table, field_prefix, "signal_instrument_id", errors);
        Self::validate_optional_string_field(table, field_prefix, "resolution_client_id", errors);
        Self::validate_optional_string_field(
            table,
            field_prefix,
            "resolution_instrument_id",
            errors,
        );
        Self::validate_optional_table_field(table, field_prefix, "reference_current_price", errors);
        Self::validate_optional_string_field(
            table,
            field_prefix,
            REALIZED_VOLATILITY_SURFACE_ID_FIELD,
            errors,
        );
        Self::validate_optional_string_field(table, field_prefix, "static_condition_id", errors);
        Self::validate_optional_string_field(table, field_prefix, "static_yes_outcome", errors);
        Self::validate_optional_string_field(table, field_prefix, "static_no_outcome", errors);
        Self::validate_optional_string_field(
            table,
            field_prefix,
            "static_fair_probability_source",
            errors,
        );
        Self::validate_required_realized_volatility_surface_id(table, field_prefix, errors);
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
        match (
            table.contains_key("signal_venue"),
            table.contains_key("signal_instrument_id"),
        ) {
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
        // the source-bound data pair rule.
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
        Self::validate_executable_entry_order_shape(table, field_prefix, errors);
        Self::validate_executable_exit_order_shapes(table, field_prefix, errors);
        Self::validate_slippage_buffer_covers_vwap_depth(table, field_prefix, errors);
        Self::validate_rotating_market_family(table, field_prefix, errors);
        Self::validate_static_binary_event_runtime_fields(table, field_prefix, errors);
    }

    fn validate_executable_entry_order_shape(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let Some(order_table) = table.get(ENTRY_ORDER_FIELD).and_then(Value::as_table) else {
            return;
        };
        let supported = order_table
            .get(stringify!(side))
            .and_then(Value::as_str)
            .is_some_and(|value| value == ORDER_SIDE_BUY_VALUE)
            && order_table
                .get(stringify!(position_side))
                .and_then(Value::as_str)
                .is_some_and(|value| value == POSITION_SIDE_LONG_VALUE)
            && order_table
                .get(stringify!(order_type))
                .and_then(Value::as_str)
                .is_some_and(|value| value == stringify!(market))
            && order_table
                .get(stringify!(time_in_force))
                .and_then(Value::as_str)
                .is_some_and(|value| value == stringify!(fok))
            && order_table
                .get(stringify!(is_post_only))
                .and_then(Value::as_bool)
                .is_some_and(|value| !value)
            && order_table
                .get(stringify!(is_reduce_only))
                .and_then(Value::as_bool)
                .is_some_and(|value| !value)
            && order_table
                .get(stringify!(is_quote_quantity))
                .and_then(Value::as_bool)
                .is_some_and(|value| value)
            && !order_table.contains_key(ORDER_TRIGGER_PRICE_FIELD)
            && !order_table.contains_key(ORDER_ACTIVATION_PRICE_FIELD)
            && !order_table.contains_key(ORDER_TRIGGER_TYPE_FIELD)
            && !order_table.contains_key(ORDER_TRIGGER_INSTRUMENT_ID_FIELD)
            && !order_table.contains_key(ORDER_TRAILING_OFFSET_FIELD)
            && !order_table.contains_key(ORDER_TRAILING_OFFSET_TYPE_FIELD);
        if !supported {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{ENTRY_ORDER_FIELD}"),
                code: UNSUPPORTED_EXECUTABLE_ENTRY_ORDER_SHAPE_CODE,
                message: "must be buy/long market FOK quote-quantity without post-only, reduce-only, trigger, or trailing fields".to_string(),
            });
        }
    }

    fn validate_executable_exit_order_shapes(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        for field in [EXIT_ORDER_FIELD, FORCED_EXIT_ORDER_FIELD] {
            let Some(order) = table.get(field).and_then(Value::as_table) else {
                continue;
            };
            let supported = Value::Table(order.clone())
                .try_into::<BinaryOracleEdgeTakerOrderConfig>()
                .is_ok_and(|order| Self::exit_order_shape_supported(&order));
            if !supported {
                errors.push(ValidationError {
                    field: format!("{field_prefix}.{field}"),
                    code: UNSUPPORTED_EXECUTABLE_EXIT_ORDER_SHAPE_CODE,
                    message: "must be market IOC base-quantity without post-only, trigger, or trailing fields".to_string(),
                });
            }
        }
    }

    fn validate_bps_runtime_knob_upper_bound(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        let Some(value) = table.get(field_name).and_then(Value::as_integer) else {
            return;
        };
        if value.is_negative() || (value as f64) > BPS_DENOMINATOR {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{field_name}"),
                code: stringify!(bps_out_of_range),
                message: format!("must be at most {BPS_DENOMINATOR} bps"),
            });
        }
    }

    fn validate_positive_u64_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        let Some(value) = table.get(field_name).and_then(Value::as_integer) else {
            return;
        };
        if value <= 0 {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{field_name}"),
                code: stringify!(positive_required),
                message: "must be positive".to_string(),
            });
        }
    }

    fn validate_non_negative_finite_float_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        let Some(value) = table.get(field_name).and_then(Value::as_float_or_integer) else {
            return;
        };
        if !is_non_negative_finite(value) {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{field_name}"),
                code: stringify!(value_out_of_range),
                message: "must be finite and >= 0".to_string(),
            });
        }
    }

    fn validate_slippage_buffer_covers_vwap_depth(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let Some(vwap_depth_limit_bps) = table
            .get(stringify!(vwap_depth_limit_bps))
            .and_then(Value::as_integer)
        else {
            return;
        };
        let Some(slippage_buffer_bps) = table
            .get(stringify!(slippage_buffer_bps))
            .and_then(Value::as_integer)
        else {
            return;
        };
        if vwap_depth_limit_bps.is_negative() || slippage_buffer_bps.is_negative() {
            return;
        }
        if slippage_buffer_bps < vwap_depth_limit_bps {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{}", stringify!(slippage_buffer_bps)),
                code: stringify!(slippage_buffer_below_vwap_depth_limit),
                message: "must be greater than or equal to vwap_depth_limit_bps".to_string(),
            });
        }
    }

    fn validate_required_realized_volatility_surface_id(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        if !table.contains_key(REALIZED_VOLATILITY_SURFACE_ID_FIELD) {
            Self::push_missing(
                errors,
                format!("{field_prefix}.{REALIZED_VOLATILITY_SURFACE_ID_FIELD}"),
                "missing_realized_volatility_surface",
                BinaryOracleEdgeTakerFieldType::String,
            );
        }
    }

    /// Reject an unknown `rotating_market_family` at config-parse time.
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

    fn validate_static_binary_event_runtime_fields(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let family = table
            .get(stringify!(rotating_market_family))
            .and_then(Value::as_str);
        let static_fields = [
            "static_condition_id",
            "static_yes_outcome",
            "static_no_outcome",
            "static_fair_probability_source",
        ];
        if family != Some(bolt_v3_market_families::static_binary_event_family_key()) {
            for field_name in static_fields {
                if table.contains_key(field_name) {
                    errors.push(ValidationError {
                        field: format!("{field_prefix}.{field_name}"),
                        code: stringify!(static_field_for_non_static_family),
                        message: format!(
                            "is only valid when rotating_market_family is `{}`",
                            bolt_v3_market_families::static_binary_event_family_key()
                        ),
                    });
                }
            }
            return;
        }

        for (field_name, code) in [
            ("static_yes_outcome", "missing_static_yes_outcome"),
            ("static_no_outcome", "missing_static_no_outcome"),
            (
                "static_fair_probability_source",
                "missing_static_fair_probability_source",
            ),
        ] {
            if !table.contains_key(field_name) {
                Self::push_missing(
                    errors,
                    format!("{field_prefix}.{field_name}"),
                    code,
                    BinaryOracleEdgeTakerFieldType::String,
                );
            }
        }

        let yes = table.get("static_yes_outcome").and_then(Value::as_str);
        let no = table.get("static_no_outcome").and_then(Value::as_str);
        if yes.is_some() && yes == no {
            errors.push(ValidationError {
                field: format!("{field_prefix}.static_no_outcome"),
                code: stringify!(static_outcomes_not_distinct),
                message: "must be distinct from static_yes_outcome".to_string(),
            });
        }

        if let Some(source) = table.get("static_fair_probability_source").and_then(Value::as_str)
            && source
                != bolt_v3_market_families::static_binary_event_reference_current_price_fair_probability_source()
        {
            errors.push(ValidationError {
                field: format!("{field_prefix}.static_fair_probability_source"),
                code: stringify!(unsupported_static_fair_probability_source),
                message: format!(
                    "must be `{}` until another static-event fair-probability source is implemented",
                    bolt_v3_market_families::static_binary_event_reference_current_price_fair_probability_source()
                ),
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

    fn validate_optional_table_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        if let Some(value) = table.get(field_name)
            && !BinaryOracleEdgeTakerFieldType::Table.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field_prefix}.{field_name}"),
                BinaryOracleEdgeTakerFieldType::Table,
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
