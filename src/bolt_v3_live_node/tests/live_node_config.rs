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

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_capture_failure_after_running_still_awaits_runner_result() {
    let (capture_failure_sender, capture_failure_receiver) = tokio::sync::oneshot::channel();
    capture_failure_sender
        .send(())
        .expect("capture failure signal should send");
    let run_future = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Err(anyhow::anyhow!("capture stopped runner"))
    };
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = Some(capture_failure_receiver);

    let outcome = live_node_run_startup_watchdog(
        run_future.as_mut(),
        &mut capture_failure_receiver,
        || NodeState::Running,
        || true,
        || stop_called.set(true),
        LiveNodeStartupWatchdogBounds {
            startup_timeout: Duration::from_secs(1),
            shutdown_grace: Duration::from_millis(25),
            trader_invariant_poll: Duration::from_millis(50),
        },
        vec!["data:chainlink_reference".to_string()],
    )
    .await;

    assert!(
        !stop_called.get(),
        "capture failure after Running should wait for the runner/capture shutdown path"
    );
    match outcome {
        LiveNodeRunStartupOutcome::Finished(Err(error)) => {
            assert_eq!(error.to_string(), "capture stopped runner");
        }
        other => {
            panic!("expected post-Running capture failure to surface runner result, got {other:?}")
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_capture_failure_during_shutdown_preserves_runner_result() {
    let (capture_failure_sender, capture_failure_receiver) = tokio::sync::oneshot::channel();
    capture_failure_sender
        .send(())
        .expect("capture failure signal should send");
    let run_future = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(())
    };
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = Some(capture_failure_receiver);

    let outcome = live_node_run_startup_watchdog(
        run_future.as_mut(),
        &mut capture_failure_receiver,
        || NodeState::ShuttingDown,
        || true,
        || stop_called.set(true),
        LiveNodeStartupWatchdogBounds {
            startup_timeout: Duration::from_secs(1),
            shutdown_grace: Duration::from_millis(25),
            trader_invariant_poll: Duration::from_millis(50),
        },
        vec!["data:chainlink_reference".to_string()],
    )
    .await;

    assert!(
        !stop_called.get(),
        "capture failure during shutdown should stay on the normal runner/capture shutdown path"
    );
    match outcome {
        LiveNodeRunStartupOutcome::Finished(Ok(())) => {}
        other => panic!(
            "capture failure during shutdown should preserve runner result instead of timeout, got {other:?}"
        ),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_capture_failure_during_hung_startup_returns_within_shutdown_grace() {
    let (capture_failure_sender, capture_failure_receiver) = tokio::sync::oneshot::channel();
    capture_failure_sender
        .send(())
        .expect("capture failure signal should send");
    let run_future = std::future::pending::<Result<(), anyhow::Error>>();
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = Some(capture_failure_receiver);
    let shutdown_grace = Duration::from_millis(25);
    let started = std::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(150),
        live_node_run_startup_watchdog(
            run_future.as_mut(),
            &mut capture_failure_receiver,
            || NodeState::Starting,
            || true,
            || stop_called.set(true),
            LiveNodeStartupWatchdogBounds {
                startup_timeout: Duration::from_secs(1),
                shutdown_grace,
                trader_invariant_poll: Duration::from_millis(50),
            },
            vec!["data:chainlink_reference".to_string()],
        ),
    )
    .await
    .expect("pre-Running capture failure must not await the hung runner forever");
    let elapsed = started.elapsed();

    assert!(
        stop_called.get(),
        "pre-Running capture failure must request stop"
    );
    assert!(
        elapsed < Duration::from_millis(150),
        "pre-Running capture failure should return after shutdown grace {shutdown_grace:?}, elapsed {elapsed:?}"
    );
    match outcome {
        LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
            trigger,
            shutdown_grace: observed_shutdown_grace,
            node_state,
            registered_client_labels,
        } => {
            assert_eq!(
                trigger,
                LiveNodeStartupShutdownGraceTrigger::RuntimeCaptureFailure
            );
            assert_eq!(observed_shutdown_grace, shutdown_grace);
            assert_eq!(node_state, "Starting");
            assert_eq!(
                registered_client_labels,
                vec!["data:chainlink_reference".to_string()]
            );
        }
        other => panic!("expected bounded startup capture-failure shutdown timeout, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_deadline_during_shutdown_preserves_runner_result() {
    let run_future = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(())
    };
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = None;

    let outcome = live_node_run_startup_watchdog(
        run_future.as_mut(),
        &mut capture_failure_receiver,
        || NodeState::ShuttingDown,
        || true,
        || stop_called.set(true),
        LiveNodeStartupWatchdogBounds {
            startup_timeout: Duration::from_millis(1),
            shutdown_grace: Duration::from_millis(100),
            trader_invariant_poll: Duration::from_millis(50),
        },
        vec!["data:chainlink_reference".to_string()],
    )
    .await;

    assert!(
        stop_called.get(),
        "deadline during existing shutdown should still request stop idempotently"
    );
    match outcome {
        LiveNodeRunStartupOutcome::Finished(Ok(())) => {}
        other => panic!(
            "deadline during shutdown should preserve runner result instead of timeout, got {other:?}"
        ),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_deadline_during_hung_startup_timeout_names_shutdown_grace() {
    let run_future = std::future::pending::<Result<(), anyhow::Error>>();
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = None;
    let shutdown_grace = Duration::from_millis(25);
    let started = std::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(150),
        live_node_run_startup_watchdog(
            run_future.as_mut(),
            &mut capture_failure_receiver,
            || NodeState::Starting,
            || true,
            || stop_called.set(true),
            LiveNodeStartupWatchdogBounds {
                startup_timeout: Duration::from_millis(1),
                shutdown_grace,
                trader_invariant_poll: Duration::from_millis(50),
            },
            vec!["data:chainlink_reference".to_string()],
        ),
    )
    .await
    .expect("deadline during hung startup must return after shutdown grace");
    let elapsed = started.elapsed();

    assert!(
        stop_called.get(),
        "deadline during startup must request stop"
    );
    assert!(
        elapsed < Duration::from_millis(150),
        "startup shutdown timeout should return after grace {shutdown_grace:?}, elapsed {elapsed:?}"
    );
    match outcome {
        LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
            trigger,
            shutdown_grace: observed_shutdown_grace,
            node_state,
            registered_client_labels,
        } => {
            assert_eq!(
                trigger,
                LiveNodeStartupShutdownGraceTrigger::StartupDeadline
            );
            assert_eq!(observed_shutdown_grace, shutdown_grace);
            assert_eq!(node_state, "Starting");
            assert_eq!(
                registered_client_labels,
                vec!["data:chainlink_reference".to_string()]
            );
        }
        other => {
            panic!("deadline during hung startup should name shutdown-grace timeout, got {other:?}")
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_deadline_during_shutdown_timeout_names_shutdown_grace() {
    let run_future = std::future::pending::<Result<(), anyhow::Error>>();
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = None;
    let shutdown_grace = Duration::from_millis(25);
    let started = std::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(150),
        live_node_run_startup_watchdog(
            run_future.as_mut(),
            &mut capture_failure_receiver,
            || NodeState::ShuttingDown,
            || true,
            || stop_called.set(true),
            LiveNodeStartupWatchdogBounds {
                startup_timeout: Duration::from_millis(1),
                shutdown_grace,
                trader_invariant_poll: Duration::from_millis(50),
            },
            vec!["data:chainlink_reference".to_string()],
        ),
    )
    .await
    .expect("deadline during shutdown must return after shutdown grace");
    let elapsed = started.elapsed();

    assert!(
        stop_called.get(),
        "deadline during shutdown should request stop idempotently"
    );
    assert!(
        elapsed < Duration::from_millis(150),
        "shutdown timeout should return after grace {shutdown_grace:?}, elapsed {elapsed:?}"
    );
    match outcome {
        LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
            trigger,
            shutdown_grace: observed_shutdown_grace,
            node_state,
            registered_client_labels,
        } => {
            assert_eq!(
                trigger,
                LiveNodeStartupShutdownGraceTrigger::StartupDeadline
            );
            assert_eq!(observed_shutdown_grace, shutdown_grace);
            assert_eq!(node_state, "ShuttingDown");
            assert_eq!(
                registered_client_labels,
                vec!["data:chainlink_reference".to_string()]
            );
        }
        other => {
            panic!("deadline during shutdown should name shutdown-grace timeout, got {other:?}")
        }
    }
}

#[test]
fn live_node_trader_running_invariant_rejects_running_without_trader() {
    // Red against the NT engines-not-connected fail-open shape: NodeState is
    // Running while the trader was never started (07-10 journal repro class).
    assert!(
        !live_node_trader_running_invariant(NodeState::Running, false),
        "Running without trader must violate the launch invariant"
    );
    assert!(
        live_node_trader_running_invariant(NodeState::Running, true),
        "Running with trader started is the successful launch signature"
    );
    assert!(
        live_node_trader_running_invariant(NodeState::Starting, false),
        "Starting without trader is still in progress, not a violation"
    );
    assert!(
        live_node_trader_running_invariant(NodeState::Idle, false),
        "Idle without trader is not a Running fail-open"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_aborts_when_node_running_without_trader() {
    // Green: watchdog detects the fail-open and aborts. When the runner returns
    // after stop, the outcome is TraderNotStarted (named launch failure).
    let stop_called = Cell::new(false);
    let run_future = async {
        loop {
            if stop_called.get() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::pin!(run_future);
    let mut capture_failure_receiver = None;
    let shutdown_grace = Duration::from_millis(200);
    let started = std::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(500),
        live_node_run_startup_watchdog(
            run_future.as_mut(),
            &mut capture_failure_receiver,
            || NodeState::Running,
            || false,
            || stop_called.set(true),
            LiveNodeStartupWatchdogBounds {
                startup_timeout: Duration::from_secs(10),
                shutdown_grace,
                trader_invariant_poll: Duration::from_millis(50),
            },
            vec![
                "data:binance_reference".to_string(),
                "data:polymarket_main".to_string(),
            ],
        ),
    )
    .await
    .expect("trader-not-started fail-open must abort promptly, not idle past the smoke guard");
    let elapsed = started.elapsed();

    assert!(
        stop_called.get(),
        "trader-not-started abort must request stop"
    );
    assert!(
        elapsed < Duration::from_millis(400),
        "trader-not-started abort should fire on the poll path well before the startup bound, elapsed {elapsed:?}"
    );
    match outcome {
        LiveNodeRunStartupOutcome::TraderNotStarted {
            node_state,
            registered_client_labels,
        } => {
            assert_eq!(node_state, "Running");
            assert_eq!(
                registered_client_labels,
                vec![
                    "data:binance_reference".to_string(),
                    "data:polymarket_main".to_string(),
                ]
            );
        }
        other => panic!("expected TraderNotStarted for Running-without-trader, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_watchdog_grace_timeout_still_names_trader_not_started() {
    // When the runner never returns after stop, the outcome is a grace timeout
    // but the trigger must remain TraderNotStartedInvariant — not StartupDeadline.
    let run_future = std::future::pending::<Result<(), anyhow::Error>>();
    tokio::pin!(run_future);
    let stop_called = Cell::new(false);
    let mut capture_failure_receiver = None;
    let shutdown_grace = Duration::from_millis(25);

    let outcome = tokio::time::timeout(
        Duration::from_millis(400),
        live_node_run_startup_watchdog(
            run_future.as_mut(),
            &mut capture_failure_receiver,
            || NodeState::Running,
            || false,
            || stop_called.set(true),
            LiveNodeStartupWatchdogBounds {
                startup_timeout: Duration::from_secs(10),
                shutdown_grace,
                trader_invariant_poll: Duration::from_millis(50),
            },
            vec!["data:binance_reference".to_string()],
        ),
    )
    .await
    .expect("hung runner after trader-not-started must still return after grace");

    assert!(stop_called.get());
    match outcome {
        LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
            trigger,
            shutdown_grace: observed_grace,
            node_state,
            registered_client_labels,
        } => {
            assert_eq!(
                trigger,
                LiveNodeStartupShutdownGraceTrigger::TraderNotStartedInvariant,
                "grace-timeout after trader-not-started must not be attributed to StartupDeadline"
            );
            assert_eq!(observed_grace, shutdown_grace);
            assert_eq!(node_state, "Running");
            assert_eq!(
                registered_client_labels,
                vec!["data:binance_reference".to_string()]
            );
            // Public error surface also preserves the named cause.
            let public_error = BoltV3LiveNodeError::LiveNodeStartupShutdownGraceTimeout {
                trigger,
                shutdown_grace: observed_grace,
                node_state: node_state.clone(),
                registered_client_labels: registered_client_labels.clone(),
            };
            let display = public_error.to_string();
            assert!(
                display.contains("trader was never started"),
                "grace-timeout Display must still name trader-not-started: {display}"
            );
            assert!(
                display.contains("trader-not-started launch invariant"),
                "grace-timeout Display must name the invariant trigger: {display}"
            );
        }
        other => panic!(
            "expected StartupShutdownGraceTimeout with TraderNotStartedInvariant, got {other:?}"
        ),
    }
}

#[test]
fn live_node_trader_not_started_error_display_is_named() {
    let error = BoltV3LiveNodeError::LiveNodeTraderNotStarted {
        node_state: "Running".to_string(),
        registered_client_labels: vec!["data:binance_reference".to_string()],
    };
    let display = error.to_string();
    assert!(
        display.contains("trader was never started"),
        "named error must identify trader-not-started: {display}"
    );
    assert!(
        display.contains("engines-not-connected fail-open"),
        "named error must name the fail-open class: {display}"
    );
    assert!(
        display.contains("data:binance_reference"),
        "named error must list registered clients: {display}"
    );
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
fn live_node_startup_shutdown_grace_exceeds_nt_stop_budget_by_derived_slack() {
    let loaded = fixture_loaded_config();
    let stop_budget =
        nautilus_stop_budget_secs(&loaded).expect("fixture NT stop budget should derive");
    let slack = live_node_startup_shutdown_grace_slack_secs(&loaded);
    let startup_shutdown_grace = live_node_startup_shutdown_grace_secs(&loaded)
        .expect("fixture startup shutdown grace should derive");

    assert_eq!(slack, loaded.root.nautilus.timeout_connection_secs);
    assert_eq!(startup_shutdown_grace, stop_budget + slack);
    assert!(
        stop_budget < startup_shutdown_grace,
        "startup shutdown grace must exceed NT stop budget: stop={stop_budget}s grace={startup_shutdown_grace}s"
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
#[should_panic(expected = "capital admission venue spendability feed lock poisoned")]
fn venue_spendability_refresh_panics_on_poisoned_capital_admission_feed_lock() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut loaded = fixture_loaded_config();
    write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
    let config = capital_admission_venue_spendability_source_config_from_loaded(&loaded)
        .expect("source config should build")
        .expect("fixture should configure source");
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    let feed = runtime
        .capital_admission_runtime_feed
        .as_ref()
        .expect("fixture should configure capital-admission runtime feed");
    poison_mutex(feed);

    let _ = refresh_capital_admission_venue_spendability_from_source(feed, &config);
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
fn live_node_config_suppresses_nt_orderbook_out_of_order_warns_to_error() {
    // NT `nautilus_model::orderbook::book` emits per-tick WARN on sequence/ts
    // high-water regressions. Force Error so they do not dominate operator logs.
    let loaded = fixture_loaded_config();
    let cfg = make_live_node_config(&loaded);
    let key = Ustr::from("nautilus_model::orderbook::book");
    let level = cfg
        .logging
        .module_level
        .get(&key)
        .copied()
        .expect("logger module_level must include nautilus_model::orderbook::book");
    assert_eq!(
        level,
        log::LevelFilter::Error,
        "orderbook module filter must be Error, got {level:?}"
    );
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
