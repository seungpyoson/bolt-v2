#![cfg(test)]

use super::*;

#[test]
fn runtime_redaction_value_buffers_zeroize_on_drop() {
    fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    fn redaction_values_field(runtime: &BoltV3LiveNodeRuntime) -> &Vec<Zeroizing<String>> {
        &runtime.redaction_values
    }

    assert_zeroize_on_drop::<Vec<Zeroizing<String>>>();
    let _ = redaction_values_field as fn(&BoltV3LiveNodeRuntime) -> &Vec<Zeroizing<String>>;
}

#[test]
fn live_node_config_maps_trader_id_and_environment_from_v3_root() {
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);

    assert_eq!(cfg.trader_id, TraderId::from("BOLT-001"));
    assert_eq!(cfg.environment, Environment::Live);
    assert_eq!(cfg.timeout_connection, Duration::from_secs(30));
    assert_eq!(cfg.timeout_reconciliation, Duration::from_secs(60));
    assert_eq!(cfg.timeout_portfolio, Duration::from_secs(10));
    assert_eq!(cfg.timeout_disconnection, Duration::from_secs(10));
    assert_eq!(cfg.delay_post_stop, Duration::from_secs(5));
    assert_eq!(cfg.timeout_shutdown, Duration::from_secs(10));
}

#[test]
fn live_node_builder_rejects_backtest_environment_before_registration() {
    let loaded = fixture_loaded_config();
    let make_error = || {
        let mut cfg = make_live_node_config(&loaded);
        cfg.environment = Environment::Backtest;
        make_bolt_v3_live_node_builder_from_config(cfg)
            .expect_err("NT LiveNodeBuilder must reject Backtest environment")
    };

    let rendered = BoltV3LiveNodeError::BuilderConstruction(make_error()).to_string();
    assert_eq!(
        rendered
            .matches("LiveNodeBuilder construction failed")
            .count(),
        1,
        "builder-construction Display should not duplicate layer prefixes: {rendered}"
    );
    assert!(
        rendered.contains("Backtest environment"),
        "builder-construction failure should identify the invalid environment: {rendered}"
    );

    let BoltV3LiveNodeBuilderError::BuilderConstruction { source } = make_error();
    assert!(
        source.to_string().contains("Backtest environment"),
        "builder-construction failure should identify the invalid environment: {source}"
    );
}

#[test]
fn combined_run_and_runtime_capture_shutdown_failure_preserves_both_error_types() {
    let error = classify_live_node_run_and_capture_shutdown(
        Err(anyhow::anyhow!("runner failed")),
        Err(anyhow::anyhow!("capture shutdown failed")),
    )
    .expect_err("combined failure must surface a bolt-v3 live-node error");

    let source = std::error::Error::source(&error)
        .expect("compound failure should expose the runner error as its source");
    assert_eq!(source.to_string(), "runner failed");

    match error {
        BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
            run_error,
            shutdown_error,
        } => {
            assert_eq!(run_error.to_string(), "runner failed");
            assert_eq!(shutdown_error.to_string(), "capture shutdown failed");
        }
        other => panic!(
            "combined runner/capture-shutdown failure must preserve both \
             error categories, got {other:?}"
        ),
    }
}

#[test]
fn live_node_config_top_level_residuals_are_disabled_or_empty() {
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);

    assert!(cfg.instance_id.is_none());
    assert!(cfg.cache.is_none());
    assert!(cfg.msgbus.is_none());
    assert!(cfg.portfolio.is_none());
    assert!(cfg.emulator.is_none());
    assert!(cfg.streaming.is_none());
    assert!(!cfg.loop_debug);
    assert!(cfg.data_clients.is_empty());
    assert!(cfg.exec_clients.is_empty());
}

#[test]
fn live_node_config_maps_zero_lookback_to_unbounded_reconciliation() {
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);
    assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
}

#[test]
fn strategy_free_timeout_sums_fail_closed_on_overflow() {
    let mut loaded = fixture_loaded_config();
    loaded.root.nautilus.timeout_connection_secs = u64::MAX;
    loaded.root.nautilus.timeout_reconciliation_secs = 1;
    let start_error = strategy_free_start_timeout_secs(&loaded)
        .expect_err("strategy-free start timeout overflow must fail closed");
    assert!(
        matches!(
            start_error,
            BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow
        ),
        "expected start timeout overflow rejection, got {start_error:?}"
    );

    loaded.root.nautilus.timeout_disconnection_secs = u64::MAX;
    loaded.root.nautilus.delay_post_stop_secs = 1;
    let stop_error = strategy_free_stop_timeout_secs(&loaded)
        .expect_err("strategy-free stop timeout overflow must fail closed");
    assert!(
        matches!(
            stop_error,
            BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow
        ),
        "expected stop timeout overflow rejection, got {stop_error:?}"
    );
}

#[test]
fn live_node_config_maps_explicit_nt_runtime_defaults_from_v3_root() {
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);

    assert!(cfg.data_engine.time_bars_build_with_no_updates);
    assert!(cfg.data_engine.time_bars_timestamp_on_close);
    assert!(!cfg.data_engine.time_bars_skip_first_non_full_bar);
    assert_eq!(
        cfg.data_engine.time_bars_interval_type,
        nautilus_model::enums::BarIntervalType::LeftOpen
    );
    assert_eq!(cfg.data_engine.time_bars_build_delay, 0);
    assert!(cfg.data_engine.time_bars_origin_offset.is_empty());
    assert!(!cfg.data_engine.validate_data_sequence);
    assert!(!cfg.data_engine.buffer_deltas);
    assert!(!cfg.data_engine.emit_quotes_from_book);
    assert!(!cfg.data_engine.emit_quotes_from_book_depths);
    assert_eq!(cfg.data_engine.external_clients, None);
    assert!(!cfg.data_engine.debug);
    assert!(!cfg.shutdown_on_error);
    assert_eq!(cfg.data_engine.qsize, 100_000);
    assert!(cfg.exec_engine.load_cache);
    assert!(!cfg.exec_engine.snapshot_orders);
    assert!(!cfg.exec_engine.snapshot_positions);
    assert_eq!(cfg.exec_engine.snapshot_positions_interval_secs, None);
    assert_eq!(cfg.exec_engine.external_clients, None);
    assert!(!cfg.exec_engine.debug);
    assert!(cfg.exec_engine.reconciliation);
    assert_eq!(cfg.exec_engine.reconciliation_startup_delay_secs, 10.0);
    assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
    assert_eq!(cfg.exec_engine.reconciliation_instrument_ids, None);
    assert!(!cfg.exec_engine.filter_unclaimed_external_orders);
    assert!(!cfg.exec_engine.filter_position_reports);
    assert_eq!(cfg.exec_engine.filtered_client_order_ids, None);
    assert!(cfg.exec_engine.generate_missing_orders);
    assert_eq!(cfg.exec_engine.inflight_check_interval_ms, 2_000);
    assert_eq!(cfg.exec_engine.inflight_check_threshold_ms, 5_000);
    assert_eq!(cfg.exec_engine.inflight_check_retries, 5);
    assert_eq!(cfg.exec_engine.open_check_interval_secs, None);
    assert_eq!(cfg.exec_engine.open_check_lookback_mins, Some(60));
    assert_eq!(cfg.exec_engine.open_check_threshold_ms, 5_000);
    assert_eq!(cfg.exec_engine.open_check_missing_retries, 5);
    assert!(cfg.exec_engine.open_check_open_only);
    assert_eq!(cfg.exec_engine.max_single_order_queries_per_cycle, 10);
    assert_eq!(cfg.exec_engine.single_order_query_delay_ms, 100);
    assert_eq!(cfg.exec_engine.position_check_interval_secs, None);
    assert_eq!(cfg.exec_engine.position_check_lookback_mins, 60);
    assert_eq!(cfg.exec_engine.position_check_threshold_ms, 5_000);
    assert_eq!(cfg.exec_engine.position_check_retries, 3);
    assert_eq!(cfg.exec_engine.purge_closed_orders_interval_mins, None);
    assert_eq!(cfg.exec_engine.purge_closed_orders_buffer_mins, None);
    assert_eq!(cfg.exec_engine.purge_closed_positions_interval_mins, None);
    assert_eq!(cfg.exec_engine.purge_closed_positions_buffer_mins, None);
    assert_eq!(cfg.exec_engine.purge_account_events_interval_mins, None);
    assert_eq!(cfg.exec_engine.purge_account_events_lookback_mins, None);
    assert!(!cfg.exec_engine.purge_from_database);
    assert_eq!(cfg.exec_engine.own_books_audit_interval_secs, None);
    assert_eq!(cfg.exec_engine.qsize, 100_000);
    assert!(!cfg.exec_engine.allow_overfills);
    assert!(!cfg.exec_engine.manage_own_order_books);
    assert!(!cfg.risk_engine.bypass);
    assert_eq!(cfg.risk_engine.max_order_submit_rate, "33/00:01:00");
    assert_eq!(cfg.risk_engine.max_order_modify_rate, "33/00:01:00");
    assert!(cfg.risk_engine.max_notional_per_order.is_empty());
    assert!(!cfg.risk_engine.debug);
    assert_eq!(cfg.risk_engine.qsize, 100_000);
}

#[test]
fn live_node_config_maps_explicit_nt_risk_debug_from_v3_root() {
    let mut loaded = fixture_loaded_config();
    loaded.root.risk.nautilus.debug = true;

    let cfg = make_live_node_config(&loaded);

    assert!(cfg.risk_engine.debug);
}

#[test]
fn live_node_config_maps_explicit_nt_data_engine_debug_from_v3_root() {
    let mut loaded = fixture_loaded_config();
    loaded.root.nautilus.data_engine.debug = true;

    let cfg = make_live_node_config(&loaded);

    assert!(cfg.data_engine.debug);
}

#[test]
fn live_node_config_maps_non_empty_nt_max_notional_per_order() {
    let mut loaded = fixture_loaded_config();
    loaded
        .root
        .risk
        .nautilus
        .max_notional_per_order
        .insert("REFERENCE.SOURCE".to_string(), "12345.00".to_string());
    loaded
        .root
        .risk
        .nautilus
        .max_notional_per_order
        .insert("SECONDARY.SOURCE".to_string(), "25000.50".to_string());
    let cfg = make_live_node_config(&loaded);

    assert_eq!(
        cfg.risk_engine
            .max_notional_per_order
            .get("REFERENCE.SOURCE"),
        Some(&"12345.00".to_string())
    );
    assert_eq!(
        cfg.risk_engine
            .max_notional_per_order
            .get("SECONDARY.SOURCE"),
        Some(&"25000.50".to_string())
    );
}

#[test]
fn venue_spendability_source_config_reads_configured_capital_pool_source() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut loaded = fixture_loaded_config();
    write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
    let config = capital_admission_venue_spendability_source_config_from_loaded(&loaded)
        .expect("source config should build")
        .expect("fixture should configure source");

    let snapshot = capital_admission_venue_spendability_snapshot_from_source_config(&config)
        .expect("configured source should be accepted");

    assert_eq!(snapshot.source, "operator_venue_spendability");
    assert_eq!(snapshot.spendable_collateral, Decimal::from(20));
    assert_eq!(snapshot.collateral_allowance, Decimal::from(12));
}

#[test]
fn venue_spendability_source_config_fails_closed_on_sha_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut loaded = fixture_loaded_config();
    write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
    let mut config = capital_admission_venue_spendability_source_config_from_loaded(&loaded)
        .expect("source config should build")
        .expect("fixture should configure source");
    config.expected_sha256 =
        "0000000000000000000000000000000000000000000000000000000000000000".to_string();

    let error = capital_admission_venue_spendability_snapshot_from_source_config(&config)
        .expect_err("hash mismatch must fail closed");
    let rendered = error.to_string();

    assert!(
        rendered.contains("capital admission venue spendability source rejected")
            && rendered.contains("Sha256Mismatch"),
        "startup error should name rejected spendability evidence, got: {rendered}"
    );
}

#[test]
fn live_node_config_maps_log_levels_from_uppercase_strings() {
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);
    assert_eq!(cfg.logging.stdout_level, log::LevelFilter::Info);
    assert_eq!(cfg.logging.fileout_level, log::LevelFilter::Info);
}

#[test]
fn live_node_config_logger_literal_does_not_inherit_nt_defaults() {
    let src = include_str!("../../bolt_v3_live_node/live_node_config.rs");
    let logging_literal = src
        .split("let logging = LoggerConfig {")
        .nth(1)
        .expect("logger config literal must exist")
        .split("let nautilus =")
        .next()
        .expect("logger config literal must precede nautilus config");

    // Field-add drift is caught by Rust struct literal exhaustiveness; this
    // guards against silently re-introducing inherited NT defaults.
    assert!(
        !logging_literal.contains(concat!("..", "Default::default()")),
        "LoggerConfig must set every pinned NT field explicitly"
    );
}

#[test]
fn live_node_config_maps_explicit_logger_residuals_in_builder_path() {
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);

    assert!(cfg.logging.component_level.is_empty());
    assert!(!cfg.logging.log_components_only);
    assert!(cfg.logging.is_colored);
    assert!(!cfg.logging.print_config);
    assert!(!cfg.logging.use_tracing);
    assert!(!cfg.logging.bypass_logging);
    assert!(cfg.logging.file_config.is_none());
    assert!(!cfg.logging.clear_log_file);
    assert!(cfg.logging.fileout_sync_on_flush);
    assert!(!cfg.logging.buffered_stdout);
}

#[test]
fn live_node_config_suppresses_nt_credential_module_logs_to_warn() {
    // Regression for the slice-7 review finding: NT's
    // `nautilus_polymarket::common::credential` and
    // `nautilus_binance::common::credential` modules log credential
    // material at info-level. Bolt-v3 forces those targets to
    // `Warn` even when the root TOML log level is `Info`, so the
    // logger filter must contain both module paths with at most
    // `Warn` regardless of the configured root level.
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);

    for module_path in crate::bolt_v3_providers::credential_log_modules() {
        let key = Ustr::from(module_path);
        let level = cfg
            .logging
            .module_level
            .get(&key)
            .copied()
            .unwrap_or_else(|| panic!("logger module_level missing `{module_path}`"));
        assert!(
            level <= log::LevelFilter::Warn,
            "credential module `{module_path}` filter must be Warn or stricter, got {level:?}"
        );
    }
}

#[test]
fn secret_resolver_setup_variant_renders_clean_message_without_empty_client_path() {
    // Per #255-2: before this fix, session-construction failure was
    // mapped into `BoltV3SecretError` with empty `client_key` and
    // `ssm_path`, rendering as a confusing
    // an empty client key in the secret-path template. The dedicated
    // `BoltV3LiveNodeError::SecretResolverSetup(SecretError)` variant
    // gives operators a clean, accurate message that does not
    // pretend a client or SSM path is involved (none is — the
    // failure happens before any path is read).
    let inner = crate::secrets::SecretError::for_test(
        "failed to build Tokio runtime for SSM resolver session: simulated".to_string(),
    );
    let err = BoltV3LiveNodeError::SecretResolverSetup(inner);
    let rendered = format!("{err}");
    assert!(
        !rendered.contains(".secrets.ssm_resolver_session"),
        "SecretResolverSetup must not render through the client/SSM-path template"
    );
    assert!(
        !rendered.contains("ssm_path"),
        "SecretResolverSetup must not include an empty ssm_path field"
    );
    assert!(
        rendered.contains("SSM resolver session"),
        "SecretResolverSetup message must name the resolver-session setup boundary"
    );
    assert!(
        rendered.contains("simulated"),
        "SecretResolverSetup must surface the wrapped SecretError"
    );
    let source = std::error::Error::source(&err);
    assert!(
        source.is_some(),
        "SecretResolverSetup must report its wrapped SecretError via \
         std::error::Error::source"
    );
}
