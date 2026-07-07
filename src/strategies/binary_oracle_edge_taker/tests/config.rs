#![cfg(test)]

use super::*;

const ZERO_INTEGER_CONFIG_VALUE: i64 = 0;
const POSITIVE_REQUIRED_INTEGER_FIELDS: &[&str] = &[
    stringify!(retry_interval_seconds),
    stringify!(market_exit_max_attempts),
    stringify!(trade_flow_max_samples),
    stringify!(trade_flow_window_secs),
    stringify!(spike_guard_cooldown_secs),
];
const BPS_UPPER_BOUND_EXCESS: i64 = (BPS_DENOMINATOR as i64) + 1;
const BPS_RUNTIME_KNOB_FIELDS: &[&str] = &[
    stringify!(book_impact_cap_bps),
    stringify!(vwap_depth_limit_bps),
    stringify!(slippage_buffer_bps),
    stringify!(sizing_ev_reference_bps),
];
const ZERO_ACCEPTED_BPS_RUNTIME_KNOB_FIELDS: &[&str] = &[
    stringify!(book_impact_cap_bps),
    stringify!(vwap_depth_limit_bps),
    stringify!(slippage_buffer_bps),
];

fn unsupported_executable_entry_order_shape_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            stringify!(side),
            Value::String(stringify!(sell).to_string()),
        ),
        (
            stringify!(position_side),
            Value::String(stringify!(short).to_string()),
        ),
        (
            stringify!(order_type),
            Value::String(stringify!(limit).to_string()),
        ),
        (
            stringify!(time_in_force),
            Value::String(stringify!(ioc).to_string()),
        ),
        (stringify!(is_post_only), Value::Boolean(true)),
        (stringify!(is_reduce_only), Value::Boolean(true)),
        (stringify!(is_quote_quantity), Value::Boolean(false)),
        (stringify!(trigger_price), Value::Float(1.0)),
        (stringify!(activation_price), Value::Float(1.0)),
        (
            stringify!(trigger_type),
            Value::String(stringify!(mark_price).to_string()),
        ),
        (
            stringify!(trigger_instrument_id),
            Value::String("TRIGGER.POLYMARKET".to_string()),
        ),
        (stringify!(trailing_offset), Value::Float(1.0)),
        (
            stringify!(trailing_offset_type),
            Value::String(stringify!(price).to_string()),
        ),
    ]
}

fn raw_with_market_quote_quantity_entry_order() -> Value {
    let mut raw = valid_raw_config();
    let entry_order = raw
        .as_table_mut()
        .expect("valid config must be a table")
        .get_mut(stringify!(entry_order))
        .expect("valid config must include entry_order")
        .as_table_mut()
        .expect("entry_order must be a table");
    entry_order.insert(
        stringify!(order_type).to_string(),
        Value::String(stringify!(market).to_string()),
    );
    entry_order.insert(
        stringify!(time_in_force).to_string(),
        Value::String(stringify!(fok).to_string()),
    );
    entry_order.insert(
        stringify!(is_quote_quantity).to_string(),
        Value::Boolean(true),
    );
    raw
}

fn raw_with_entry_order_field(field: &'static str, value: Value) -> Value {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("valid config must be a table")
        .get_mut(stringify!(entry_order))
        .expect("valid config must include entry_order")
        .as_table_mut()
        .expect("entry_order must be a table")
        .insert(field.to_string(), value);
    raw
}

#[test]
fn parse_config_accepts_market_fok_quote_quantity_entry_order() {
    let parsed =
        BinaryOracleEdgeTakerBuilder::parse_config(&raw_with_market_quote_quantity_entry_order())
            .expect("market/FOK quote-quantity entry order should parse");

    assert_eq!(parsed.entry_order.order_type, OrderType::Market);
    assert_eq!(parsed.entry_order.time_in_force, TimeInForce::Fok);
    assert!(parsed.entry_order.is_quote_quantity);
}

#[test]
fn strategy_core_uses_configured_nt_order_tag_and_oms_type() {
    let strategy = test_strategy();

    assert_eq!(strategy.core.config.order_id_tag.as_deref(), Some("001"));
    assert_eq!(strategy.core.config.oms_type, Some(NtOmsType::Netting));
}

#[test]
fn strategy_core_accepts_nt_hedging_oms_type() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("raw config should be a TOML table")
        .insert("oms_type".to_string(), Value::String("hedging".to_string()));
    let config =
        BinaryOracleEdgeTakerBuilder::parse_config(&raw).expect("Hedging OMS should parse");
    let context = StrategyBuildContext::new(
        RecordingFeeProvider::cold(),
        Arc::new(RecordingDecisionEvidenceWriter),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        ),
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        fixture_execution_venue(),
    );

    let strategy = BinaryOracleEdgeTaker::new(config, context);

    assert_eq!(strategy.core.config.oms_type, Some(NtOmsType::Hedging));
}

#[test]
fn parse_config_rejects_non_positive_spike_guard_return_threshold() {
    // A non-positive spike_guard_return_threshold makes the spike guard's
    // `relative_move >= threshold` test (relative_move is an abs(), always
    // >= 0) always true, arming the cooldown on every reference quote and
    // silently blocking all entry. 0.0 and negatives are valid TOML floats so
    // the type check cannot catch them; they must be rejected fail-loud at
    // config load.
    for bad in [0.0_f64, -0.01_f64] {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert(
                "spike_guard_return_threshold".to_string(),
                Value::Float(bad),
            );
        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("non-positive spike_guard_return_threshold must be rejected");
        assert!(
            err.to_string()
                .contains("spike_guard_return_threshold must be positive and finite"),
            "expected positivity rejection for {bad}, got: {err}"
        );
    }
}

#[test]
fn parse_config_rejects_zero_positive_required_integer_fields() {
    for field in POSITIVE_REQUIRED_INTEGER_FIELDS {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert(
                (*field).to_string(),
                Value::Integer(ZERO_INTEGER_CONFIG_VALUE),
            );
        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("zero positive-required config field must be rejected");
        assert!(
            err.to_string()
                .contains(&format!("{field} must be positive")),
            "expected positivity rejection for {field}, got: {err}"
        );
    }
}

#[test]
fn parse_config_rejects_zero_sizing_ev_reference_bps() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("raw config should be a TOML table")
        .insert(
            stringify!(sizing_ev_reference_bps).to_string(),
            Value::Integer(ZERO_INTEGER_CONFIG_VALUE),
        );

    let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect_err("zero sizing_ev_reference_bps must be rejected");

    assert!(
        err.to_string()
            .contains("sizing_ev_reference_bps must be positive"),
        "expected sizing_ev_reference_bps positivity rejection, got: {err}"
    );
}

#[test]
fn parse_config_rejects_negative_or_non_finite_risk_lambda() {
    for bad in [-0.01_f64, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert(stringify!(risk_lambda).to_string(), Value::Float(bad));

        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("negative or non-finite risk_lambda must be rejected");

        assert!(
            err.to_string()
                .contains("risk_lambda must be finite and >= 0"),
            "expected risk_lambda finite non-negative rejection for {bad}, got: {err}"
        );
    }
}

#[test]
fn parse_config_rejects_bps_runtime_knobs_above_full_scale() {
    for field in BPS_RUNTIME_KNOB_FIELDS {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert((*field).to_string(), Value::Integer(BPS_UPPER_BOUND_EXCESS));

        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("out-of-range bps runtime knob must be rejected");

        assert!(
            err.to_string()
                .contains(&format!("{field} must be at most {BPS_DENOMINATOR}")),
            "expected bps upper-bound rejection for {field}, got: {err}"
        );
    }
}

#[test]
fn parse_config_accepts_bps_runtime_knob_boundaries() {
    for value in [0_i64, BPS_DENOMINATOR as i64] {
        let mut raw = valid_raw_config();
        let table = raw.as_table_mut().expect("raw config should be a table");
        for field in ZERO_ACCEPTED_BPS_RUNTIME_KNOB_FIELDS {
            table.insert((*field).to_string(), Value::Integer(value));
        }

        BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .unwrap_or_else(|error| panic!("bps boundary {value} should parse: {error}"));
    }
}

#[test]
fn parse_config_accepts_sizing_ev_reference_bps_valid_boundaries() {
    for value in [1_i64, BPS_DENOMINATOR as i64] {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a table")
            .insert(
                stringify!(sizing_ev_reference_bps).to_string(),
                Value::Integer(value),
            );

        BinaryOracleEdgeTakerBuilder::parse_config(&raw).unwrap_or_else(|error| {
            panic!("sizing_ev_reference_bps boundary {value} should parse: {error}")
        });
    }
}

#[test]
fn parse_config_rejects_slippage_buffer_below_vwap_depth_limit() {
    let mut raw = valid_raw_config();
    let table = raw.as_table_mut().expect("raw config should be a table");
    table.insert(
        stringify!(vwap_depth_limit_bps).to_string(),
        Value::Integer(50),
    );
    table.insert(
        stringify!(slippage_buffer_bps).to_string(),
        Value::Integer(49),
    );

    let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect_err("slippage buffer must cover the configured VWAP depth limit");

    assert!(
        err.to_string()
            .contains("slippage_buffer_bps must be greater than or equal to vwap_depth_limit_bps"),
        "expected coupling rejection, got: {err}"
    );
}

#[test]
fn validate_config_rejects_bps_runtime_knobs_above_full_scale() {
    for field in BPS_RUNTIME_KNOB_FIELDS {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert((*field).to_string(), Value::Integer(BPS_UPPER_BOUND_EXCESS));
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(
            &errors,
            &format!("strategies[0].config.{field}"),
            "bps_out_of_range",
        );
        assert_eq!(
            error.message,
            format!("must be at most {BPS_DENOMINATOR} bps")
        );
    }
}

#[test]
fn validate_config_rejects_zero_sizing_ev_reference_bps() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("raw config should be a TOML table")
        .insert(
            stringify!(sizing_ev_reference_bps).to_string(),
            Value::Integer(ZERO_INTEGER_CONFIG_VALUE),
        );
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    let error = find_error(
        &errors,
        "strategies[0].config.sizing_ev_reference_bps",
        "positive_required",
    );
    assert_eq!(error.message, "must be positive");
}

#[test]
fn validate_config_rejects_negative_or_non_finite_risk_lambda() {
    for bad in [-0.01_f64, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert(stringify!(risk_lambda).to_string(), Value::Float(bad));
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(
            &errors,
            "strategies[0].config.risk_lambda",
            "value_out_of_range",
        );
        assert_eq!(error.message, "must be finite and >= 0");
    }
}

#[test]
fn validate_config_rejects_negative_bps_runtime_knobs() {
    for field in BPS_RUNTIME_KNOB_FIELDS {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert((*field).to_string(), Value::Integer(-1));
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(
            &errors,
            &format!("strategies[0].config.{field}"),
            "bps_out_of_range",
        );
        assert_eq!(
            error.message,
            format!("must be at most {BPS_DENOMINATOR} bps")
        );
    }
}

#[test]
fn validate_config_rejects_slippage_buffer_below_vwap_depth_limit() {
    let mut raw = valid_raw_config();
    let table = raw.as_table_mut().expect("raw config should be a table");
    table.insert(
        stringify!(vwap_depth_limit_bps).to_string(),
        Value::Integer(50),
    );
    table.insert(
        stringify!(slippage_buffer_bps).to_string(),
        Value::Integer(49),
    );
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    let error = find_error(
        &errors,
        "strategies[0].config.slippage_buffer_bps",
        "slippage_buffer_below_vwap_depth_limit",
    );
    assert_eq!(
        error.message,
        "must be greater than or equal to vwap_depth_limit_bps"
    );
}

#[test]
fn parse_config_rejects_unsupported_executable_entry_order_shapes() {
    for (field, value) in unsupported_executable_entry_order_shape_cases() {
        let raw = raw_with_entry_order_field(field, value);

        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("unsupported executable entry shape must fail at parse-time");
        assert!(
            err.to_string()
                .contains("entry_order must be buy/long market FOK quote-quantity"),
            "parse error for `{field}` should name executable entry shape: {err}"
        );
    }
}

#[test]
fn validate_config_rejects_unsupported_executable_entry_order_shapes() {
    for (field, value) in unsupported_executable_entry_order_shape_cases() {
        let raw = raw_with_entry_order_field(field, value);
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors.iter().any(|error| {
                error.field == "strategies[0].config.entry_order"
                    && error.code == "unsupported_executable_entry_order_shape"
                    && error
                        .message
                        .contains("must be buy/long market FOK quote-quantity without")
            }),
            "`{field}` must reject unsupported executable entry shape: {errors:#?}"
        );
    }
}

#[test]
fn parse_config_rejects_malformed_configured_instrument_ids() {
    for (field, bad_value) in [
        ("signal_instrument_id", "configured-signal-price"),
        ("resolution_instrument_id", "configured-resolution-price"),
    ] {
        let mut raw = valid_raw_config();
        let table = raw
            .as_table_mut()
            .expect("valid raw config should be a TOML table");
        table.insert(field.to_string(), Value::String(bad_value.to_string()));
        if field == "resolution_instrument_id" {
            table.insert(
                "resolution_client_id".to_string(),
                Value::String("resolution_data_client".to_string()),
            );
        }

        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("malformed configured instrument id must fail at parse-time");
        let rendered = err.to_string();
        assert!(
            rendered.contains(field) && rendered.contains(bad_value),
            "expected parse error to name `{field}` and `{bad_value}`, got: {rendered}"
        );
    }
}

#[test]
fn runtime_config_parse_normalizes_order_fields_to_nt_enums() {
    let config = BinaryOracleEdgeTakerBuilder::parse_config(&valid_raw_config())
        .expect("valid raw config should parse into runtime config");

    assert_eq!(config.entry_order.order_type, OrderType::Market);
    assert_eq!(config.entry_order.time_in_force, TimeInForce::Fok);
    assert!(config.entry_order.is_quote_quantity);
    assert_eq!(config.exit_order.order_type, OrderType::Market);
    assert_eq!(config.exit_order.time_in_force, TimeInForce::Ioc);
}

#[test]
fn runtime_config_parse_rejects_stale_submit_orders_switch() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("valid config must be a table")
        .insert("submit_orders".to_string(), Value::Boolean(true));

    let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect_err("strategy-local submit_orders should be rejected as stale policy");
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("submit_orders"),
        "unknown stale field should be named: {rendered}"
    );
}

#[test]
fn strategy_core_uses_explicit_configured_nt_strategy_fields() {
    let raw = valid_raw_config();
    let mut errors = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
    assert!(errors.is_empty(), "{errors:#?}");

    let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw).unwrap();
    let strategy = BinaryOracleEdgeTaker::new(
        config,
        StrategyBuildContext::new(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
            crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
            fixture_execution_venue(),
        ),
    );

    assert!(strategy.core.config.use_uuid_client_order_ids);
    assert!(!strategy.core.config.use_hyphens_in_client_order_ids);
    assert_eq!(
        strategy.core.config.external_order_claims,
        Some(vec![InstrumentId::from("AUXILIARY.SOURCE")])
    );
    assert!(strategy.core.config.manage_contingent_orders);
    assert!(strategy.core.config.manage_gtd_expiry);
    assert!(strategy.core.config.manage_stop);
    assert_eq!(strategy.core.config.market_exit_interval_ms, 250);
    assert_eq!(strategy.core.config.market_exit_max_attempts, 7);
    assert_eq!(
        strategy.core.config.market_exit_time_in_force,
        TimeInForce::Ioc
    );
    assert!(!strategy.core.config.market_exit_reduce_only);
    assert!(!strategy.core.config.log_events);
    assert!(!strategy.core.config.log_commands);
    assert!(!strategy.core.config.log_rejected_due_post_only_as_warning);
}

#[test]
fn validate_config_rejects_missing_signal_data_pair() {
    let mut raw = valid_raw_config();
    let table = raw
        .as_table_mut()
        .expect("valid raw config should be a TOML table");
    table.remove("signal_venue");
    table.remove("signal_instrument_id");

    let mut errors = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors.iter().any(|error| {
            error.field == "strategies[0].config.signal_venue"
                && error.code == "missing_signal_data_pair"
        }),
        "missing signal role should fail raw strategy validation: {errors:#?}"
    );
}

#[test]
fn config_rejects_removed_internal_realized_volatility_fields() {
    let mut raw = valid_raw_config();
    let table = raw
        .as_table_mut()
        .expect("valid raw config should be a TOML table");
    table.insert("vol_window_secs".to_string(), Value::Integer(60));

    let mut errors = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors.iter().any(|error| {
            error.field == "strategies[0].config.vol_window_secs" && error.code == "unknown_field"
        }),
        "removed internal RV fields must be rejected as unknown fields: {errors:#?}"
    );
    assert!(
        !errors.iter().any(|error| {
            error.field == "strategies[0].config.signal_venue"
                || error.field == "strategies[0].config.signal_instrument_id"
        }),
        "surfaced RV mode must keep signal data available for fast-spot pricing: {errors:#?}"
    );
}

#[test]
fn realized_volatility_surface_id_is_required_for_runtime_config() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("valid raw config should be a TOML table")
        .remove("realized_volatility_surface_id");
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors.iter().any(|error| {
            error.field == "strategies[0].config.realized_volatility_surface_id"
                && error.code == "missing_realized_volatility_surface"
        }),
        "taker runtime config must consume the shared realized-volatility surface: {errors:#?}"
    );
}

#[test]
fn validate_config_rejects_resolution_data_with_only_one_field_set() {
    // The live Chainlink strike binding is optional, but both-or-neither: a
    // strategy either declares BOTH `resolution_client_id` +
    // `resolution_instrument_id` (the live strike route) or NEITHER. Setting
    // exactly one is a fail-closed config error — a half-configured resolution
    // route must never load, since it would leave `price_to_beat` permanently
    // unbindable and silently disable the live strike. Mirrors the
    // reference/signal pair guards. Baseline `valid_raw_config()` sets neither.
    for (present, absent) in [
        ("resolution_client_id", "resolution_instrument_id"),
        ("resolution_instrument_id", "resolution_client_id"),
    ] {
        let mut raw = valid_raw_config();
        let table = raw
            .as_table_mut()
            .expect("valid raw config should be a TOML table");
        table.insert(
            present.to_string(),
            Value::String("RESOLUTION.SOURCE".to_string()),
        );

        let mut errors = Vec::new();
        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors.iter().any(|error| {
                error.field == format!("strategies[0].config.{absent}")
                    && error.code == "missing_resolution_data_pair"
            }),
            "setting only `{present}` must fail validation on missing `{absent}`: {errors:#?}"
        );
    }
}

#[test]
fn validate_config_rejects_malformed_configured_instrument_ids() {
    for (field, bad_value) in [
        ("signal_instrument_id", "configured-signal-price"),
        ("resolution_instrument_id", "configured-resolution-price"),
    ] {
        let mut raw = valid_raw_config();
        let table = raw
            .as_table_mut()
            .expect("valid raw config should be a TOML table");
        table.insert(field.to_string(), Value::String(bad_value.to_string()));
        if field == "resolution_instrument_id" {
            table.insert(
                "resolution_client_id".to_string(),
                Value::String("resolution_data_client".to_string()),
            );
        }

        let mut errors = Vec::new();
        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors.iter().any(|error| {
                error.field == format!("strategies[0].config.{field}")
                    && error.code == "invalid_instrument_id"
                    && error.message.contains(bad_value)
            }),
            "`{field}` must reject malformed configured instrument id `{bad_value}`: {errors:#?}"
        );
    }
}

#[test]
fn production_registry_registers_binary_oracle_edge_taker_kind() {
    let registry = production_strategy_registry().expect("registry should build");
    assert!(registry.get("binary_oracle_edge_taker").is_some());
}

#[test]
fn builder_requires_strategy_id_and_client_id() {
    let raw = toml::toml! {
        warmup_tick_count = 20
        reentry_cooldown_secs = 30
        order_notional_target = 1000.0
        maximum_position_notional = 1000.0
        book_impact_cap_bps = 15
        vwap_depth_limit_bps = 15
        slippage_buffer_bps = 15
        risk_lambda = 0.5
        sizing_ev_reference_bps = 500
        edge_threshold_basis_points = -20
        exit_hysteresis_bps = 5
        forced_flat_stale_reference_ms = 1500
        forced_flat_thin_book_min_liquidity = 100.0
        lead_agreement_min_corr = 0.8
        lead_jitter_max_ms = 250
    }
    .into();
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

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

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    let error = find_error(&errors, "strategies[0].config.stray_flag", "unknown_field");
    assert!(error.message.contains("unknown field `stray_flag`"));
}

/// G-field-ssot: the runtime `validate_table` allowlist must be single-sourced
/// from the `BinaryOracleEdgeTakerConfig` serde struct — it cannot drift.
///
/// The optional-string field set is emitted once by the
/// `binary_oracle_edge_taker_extra_string_fields!` macro and expanded into
/// both the struct tail and the `validate_table` allowlist, so the two cannot
/// diverge. This test is the regression lock: it fails if any future change
/// reintroduces a hand-maintained allowlist that drifts from the struct.
#[test]
fn validate_table_allowlist_is_single_sourced_from_config_struct() {
    let serde_ssot = serde_known_top_level_config_fields();

    // Sanity: a known optional-string field is part of the serde SSOT set.
    assert!(
        serde_ssot.contains("resolution_instrument_id"),
        "a known optional field must be part of the serde SSOT field set: {serde_ssot:?}"
    );

    let validate_accepted = validate_table_accepted_top_level_fields(&serde_ssot);

    // The allowlist must accept EXACTLY the serde-known field set: no field the
    // struct accepts may be rejected, and no field outside the struct may be
    // silently allowed. Both directions prove single-sourcing.
    let serde_only: Vec<&String> = serde_ssot.difference(&validate_accepted).collect();
    let validate_only: Vec<&String> = validate_accepted.difference(&serde_ssot).collect();

    assert!(
        serde_only.is_empty(),
        "validate_table rejects serde-known config fields (allowlist drifted from \
         struct SSOT): {serde_only:?}"
    );
    assert!(
        validate_only.is_empty(),
        "validate_table accepts fields the struct SSOT does not define: {validate_only:?}"
    );
}

#[test]
fn builder_rejects_non_table_config() {
    let raw = Value::String("not-a-table".to_string());
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

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

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    let error = find_error(
        &errors,
        "strategies[0].config.warmup_tick_count",
        "wrong_type",
    );
    assert_eq!(error.message, "must be an integer, got string value");
    assert!(!errors.iter().any(|e| {
        e.field == "strategies[0].config.warmup_tick_count" && e.code == "missing_required_integer"
    }));
}

#[test]
fn builder_rejects_unknown_rotating_market_family() {
    // P5-10: a market family not bound by the registry must be rejected at
    // parse, converging with the registry single source of truth.
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("valid config must be a table")
        .insert(
            "rotating_market_family".to_string(),
            Value::String("not-a-real-family".to_string()),
        );
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    let error = find_error(
        &errors,
        "strategies[0].config.rotating_market_family",
        "unknown_market_family",
    );
    assert_eq!(error.message, "unknown market family `not-a-real-family`");
}

#[test]
fn builder_accepts_registry_bound_rotating_market_family() {
    // The valid fixture's family is registry-bound, so no unknown-family
    // error is raised — the check must accept every family the registry
    // binds, not just reject unknown ones.
    let raw = valid_raw_config();
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        !errors
            .iter()
            .any(|error| error.code == "unknown_market_family"),
        "registry-bound family must not raise an unknown-market-family error: {errors:?}"
    );
}

#[test]
fn builder_accepts_static_binary_event_market_family_with_configured_outcomes() {
    let raw = static_binary_event_raw_config();
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
    let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect("static binary event runtime config should parse");

    assert!(
        !errors
            .iter()
            .any(|error| error.code == "unknown_market_family"),
        "static family is registry-bound and must not raise unknown-market-family: {errors:?}"
    );
    assert_eq!(
        config.rotating_market_family,
        crate::bolt_v3_market_families::static_binary_event::KEY
    );
    assert_eq!(
        config.static_condition_id.as_deref(),
        Some("condition-sample-event")
    );
    assert_eq!(config.static_yes_outcome.as_deref(), Some("Yes"));
    assert_eq!(config.static_no_outcome.as_deref(), Some("No"));
    assert_eq!(
        config.static_fair_probability_source.as_deref(),
        Some("reference_current_price")
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn builder_rejects_static_binary_event_without_configured_outcomes() {
    let mut raw = static_binary_event_raw_config();
    let table = raw.as_table_mut().expect("valid config must be a table");
    table.remove("static_yes_outcome");
    table.remove("static_no_outcome");
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    find_error(
        &errors,
        "strategies[0].config.static_yes_outcome",
        "missing_static_yes_outcome",
    );
    find_error(
        &errors,
        "strategies[0].config.static_no_outcome",
        "missing_static_no_outcome",
    );
}

#[test]
fn builder_rejects_static_binary_event_without_fair_probability_source() {
    let mut raw = static_binary_event_raw_config();
    let table = raw.as_table_mut().expect("valid config must be a table");
    table.remove("static_fair_probability_source");
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    find_error(
        &errors,
        "strategies[0].config.static_fair_probability_source",
        "missing_static_fair_probability_source",
    );
}

#[test]
fn builder_rejects_static_binary_event_unsupported_fair_probability_source() {
    let mut raw = static_binary_event_raw_config();
    raw.as_table_mut()
        .expect("valid config must be a table")
        .insert(
            "static_fair_probability_source".to_string(),
            Value::String("manual_probability".to_string()),
        );
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    find_error(
        &errors,
        "strategies[0].config.static_fair_probability_source",
        "unsupported_static_fair_probability_source",
    );
}

#[test]
fn builder_rejects_static_event_fields_for_non_static_family() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("valid config must be a table")
        .insert(
            "static_yes_outcome".to_string(),
            Value::String("Yes".to_string()),
        );
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    find_error(
        &errors,
        "strategies[0].config.static_yes_outcome",
        "static_field_for_non_static_family",
    );
}

fn static_binary_event_raw_config() -> Value {
    let mut raw = valid_raw_config();
    let table = raw.as_table_mut().expect("valid config must be a table");
    table.insert(
        "target_kind".to_string(),
        Value::String("static_market".to_string()),
    );
    table.insert(
        "rotating_market_family".to_string(),
        Value::String(crate::bolt_v3_market_families::static_binary_event::KEY.to_string()),
    );
    table.insert(
        "underlying_asset".to_string(),
        Value::String("sample_event_2026".to_string()),
    );
    table.insert("cadence_seconds".to_string(), Value::Integer(1));
    table.insert(
        "cadence_slug_token".to_string(),
        Value::String("will-sample-event-resolve-yes".to_string()),
    );
    table.insert(
        "market_selection_rule".to_string(),
        Value::String("configured_static".to_string()),
    );
    table.insert(
        "static_condition_id".to_string(),
        Value::String("condition-sample-event".to_string()),
    );
    table.insert(
        "static_yes_outcome".to_string(),
        Value::String("Yes".to_string()),
    );
    table.insert(
        "static_no_outcome".to_string(),
        Value::String("No".to_string()),
    );
    table.insert(
        "static_fair_probability_source".to_string(),
        Value::String("reference_current_price".to_string()),
    );
    raw
}

#[test]
fn builder_accepts_integer_literals_for_f64_fields() {
    let mut raw = valid_raw_config();
    let raw_table = raw.as_table_mut().expect("valid config must be a table");
    for (field, value) in [
        ("order_notional_target", 1_000),
        ("maximum_position_notional", 1_000),
        ("book_impact_cap_bps", 15),
        ("vwap_depth_limit_bps", 15),
        ("slippage_buffer_bps", 15),
        ("risk_lambda", 1),
        ("edge_threshold_basis_points", -20),
        ("exit_hysteresis_bps", 5),
        ("pricing_kurtosis", 0),
        ("theta_decay_factor", 0),
        ("forced_flat_thin_book_min_liquidity", 100),
        ("lead_agreement_min_corr", 1),
    ] {
        raw_table.insert(field.to_string(), Value::Integer(value));
    }
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors.is_empty(),
        "expected integer literals for f64 fields to validate, got: {errors:?}"
    );
}

#[test]
fn builder_requires_executable_edge_cost_runtime_knobs() {
    let mut raw = valid_raw_config();
    let raw_table = raw.as_table_mut().expect("valid config must be a table");
    raw_table.remove("vwap_depth_limit_bps");
    raw_table.remove("slippage_buffer_bps");
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors.iter().any(|error| {
            error.field == "strategies[0].config.vwap_depth_limit_bps"
                && error.code == "missing_vwap_depth_limit_bps"
        }),
        "missing vwap_depth_limit_bps must fail validation: {errors:#?}"
    );
    assert!(
        errors.iter().any(|error| {
            error.field == "strategies[0].config.slippage_buffer_bps"
                && error.code == "missing_slippage_buffer_bps"
        }),
        "missing slippage_buffer_bps must fail validation: {errors:#?}"
    );
}

#[test]
fn builder_accepts_nested_order_shape_without_flat_order_projection() {
    let raw = valid_raw_config();
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors.is_empty(),
        "nested order shape should validate without flat entry_/exit_ projection: {errors:?}"
    );
}

#[test]
fn builder_requires_pricing_model_fields() {
    let raw = toml::toml! {
        strategy_id = "BINARYORACLEEDGETAKER-001"
        client_id = "POLYMARKET"
        warmup_tick_count = 20
        reentry_cooldown_secs = 30
        order_notional_target = 1000.0
        maximum_position_notional = 1000.0
        book_impact_cap_bps = 15
        vwap_depth_limit_bps = 15
        slippage_buffer_bps = 15
        risk_lambda = 0.5
        sizing_ev_reference_bps = 500
        edge_threshold_basis_points = -20
        exit_hysteresis_bps = 5
        forced_flat_stale_reference_ms = 1500
        forced_flat_thin_book_min_liquidity = 100.0
        lead_agreement_min_corr = 0.8
        lead_jitter_max_ms = 250

        [entry_order]
        side = "buy"
        position_side = "long"
        order_type = "market"
        time_in_force = "fok"
        is_post_only = false
        is_reduce_only = false
        is_quote_quantity = true

        [exit_order]
        side = "sell"
        position_side = "long"
        order_type = "market"
        time_in_force = "ioc"
        is_post_only = false
        is_reduce_only = false
        is_quote_quantity = false
    }
    .into();
    let mut errors = Vec::new();

    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.cadence_seconds")
    );
    assert!(errors.iter().any(|e| {
        e.field == "strategies[0].config.realized_volatility_surface_id"
            && e.code == "missing_realized_volatility_surface"
    }));
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.trade_flow_window_secs")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.trade_flow_max_samples")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.spike_guard_return_threshold")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.spike_guard_cooldown_secs")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.pricing_kurtosis")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.theta_decay_factor")
    );
}

#[test]
fn configured_short_position_contract_is_rejected_until_short_economics_exists() {
    let contract = ConfiguredPositionContract {
        entry_order_side: OrderSide::Sell,
        entry_position_side: PositionSide::Short,
        exit_order_side: OrderSide::Buy,
        exit_position_side: PositionSide::Short,
    };

    assert!(!supports_strategy_managed_position(
        OrderSide::Sell,
        PositionSide::Short,
        contract
    ));
}
