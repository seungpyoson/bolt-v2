#![cfg(test)]

use super::*;

const ZERO_INTEGER_CONFIG_VALUE: i64 = 0;
const POSITIVE_REQUIRED_INTEGER_FIELDS: &[&str] = &[
    stringify!(trade_flow_max_samples),
    stringify!(trade_flow_window_secs),
    stringify!(spike_guard_cooldown_secs),
    stringify!(vol_window_secs),
    stringify!(vol_gap_reset_secs),
    stringify!(vol_min_observations),
    stringify!(vol_bridge_valid_secs),
];

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
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        ),
        fixture_execution_venue(),
    )
    .with_readiness_evidence(test_readiness_gate_evidence());

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
fn parse_config_rejects_malformed_configured_instrument_ids() {
    for (field, bad_value) in [
        ("reference_instrument_id", "configured-reference-price"),
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

    assert_eq!(config.entry_order.order_type, OrderType::Limit);
    assert_eq!(config.entry_order.time_in_force, TimeInForce::Fok);
    assert_eq!(config.exit_order.order_type, OrderType::Market);
    assert_eq!(config.exit_order.time_in_force, TimeInForce::Ioc);
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
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
            fixture_execution_venue(),
        )
        .with_readiness_evidence(test_readiness_gate_evidence()),
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
        ("reference_instrument_id", "configured-reference-price"),
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
        risk_lambda = 0.5
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
fn builder_accepts_integer_literals_for_f64_fields() {
    let mut raw = valid_raw_config();
    let raw_table = raw.as_table_mut().expect("valid config must be a table");
    for (field, value) in [
        ("order_notional_target", 1_000),
        ("maximum_position_notional", 1_000),
        ("book_impact_cap_bps", 15),
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
        risk_lambda = 0.5
        edge_threshold_basis_points = -20
        exit_hysteresis_bps = 5
        forced_flat_stale_reference_ms = 1500
        forced_flat_thin_book_min_liquidity = 100.0
        lead_agreement_min_corr = 0.8
        lead_jitter_max_ms = 250

        [entry_order]
        side = "buy"
        position_side = "long"
        order_type = "limit"
        time_in_force = "fok"
        is_post_only = false
        is_reduce_only = false
        is_quote_quantity = false

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
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.vol_window_secs")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.vol_gap_reset_secs")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.vol_min_observations")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.field == "strategies[0].config.vol_bridge_valid_secs")
    );
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
