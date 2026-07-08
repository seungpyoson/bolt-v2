use super::*;

/// Translates a validated bolt-v3 config into an NT-native
/// [`LiveNodeBuilder`] with no clients added. Field translation goes
/// through [`make_live_node_config`] so the bolt-v3 → NT field mapping
/// has a single source of truth that the existing per-field tests can
/// keep exercising.
pub fn make_bolt_v3_live_node_builder(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    let cfg = make_live_node_config(loaded);
    make_bolt_v3_live_node_builder_from_config(cfg)
}

pub(super) fn make_bolt_v3_live_node_builder_from_config(
    cfg: LiveNodeConfig,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    LiveNodeBuilder::from_config(cfg)
        .map_err(|source| BoltV3LiveNodeBuilderError::BuilderConstruction { source })
}

pub fn make_live_node_config(loaded: &LoadedBoltV3Config) -> LiveNodeConfig {
    let trader_id = loaded.root.trader_id;
    let environment = loaded.root.runtime.mode;
    let mut module_level: AHashMap<Ustr, LevelFilter> = AHashMap::new();
    for module_path in bolt_v3_providers::credential_log_modules() {
        module_level.insert(Ustr::from(module_path), LevelFilter::Warn);
    }
    let logging = LoggerConfig {
        stdout_level: nautilus_common::logging::map_log_level_to_filter(
            loaded.root.logging.stdout_level,
        ),
        fileout_level: nautilus_common::logging::map_log_level_to_filter(
            loaded.root.logging.fileout_level,
        ),
        component_level: AHashMap::new(),
        module_level,
        log_components_only: false,
        is_colored: true,
        print_config: false,
        use_tracing: false,
        bypass_logging: false,
        file_config: None,
        clear_log_file: false,
        fileout_sync_on_flush: true,
        buffered_stdout: false,
    };
    let nautilus = &loaded.root.nautilus;
    let data = &nautilus.data_engine;
    let data_engine = nautilus_live::config::LiveDataEngineConfig {
        time_bars_build_with_no_updates: data.time_bars_build_with_no_updates,
        time_bars_timestamp_on_close: data.time_bars_timestamp_on_close,
        time_bars_skip_first_non_full_bar: data.time_bars_skip_first_non_full_bar,
        time_bars_interval_type: bar_interval_type_from_str(&data.time_bars_interval_type),
        time_bars_build_delay: data.time_bars_build_delay,
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live data config consumes the same aggregation/nanosecond pairs as a HashMap.
        time_bars_origin_offset: data.time_bars_origins.clone().into_iter().collect(),
        validate_data_sequence: data.validate_data_sequence,
        buffer_deltas: data.buffer_deltas,
        emit_quotes_from_book: data.emit_quotes_from_book,
        emit_quotes_from_book_depths: data.emit_quotes_from_book_depths,
        external_clients: configured_external_clients(&data.external_clients),
        debug: data.debug,
        qsize: data.qsize,
    };
    let exec = &nautilus.exec_engine;
    let reconciliation_lookback_mins = u32_zero_as_none(exec.reconciliation_lookback_mins);
    let exec_engine = nautilus_live::config::LiveExecEngineConfig {
        load_cache: exec.load_cache,
        snapshot_orders: exec.snapshot_orders,
        snapshot_positions: exec.snapshot_positions,
        snapshot_positions_interval_secs: u64_zero_as_none_f64(
            exec.snapshot_positions_interval_secs,
        ),
        external_clients: configured_external_clients(&exec.external_clients),
        debug: exec.debug,
        reconciliation: exec.reconciliation,
        reconciliation_lookback_mins,
        // `f64` is lossless for all practical delay values (< 2^53 seconds).
        reconciliation_startup_delay_secs: exec.reconciliation_startup_delay_secs as f64,
        reconciliation_instrument_ids: non_empty_strings(&exec.reconciliation_instrument_ids),
        filter_unclaimed_external_orders: exec.filter_unclaimed_external_orders,
        filter_position_reports: exec.filter_position_reports,
        filtered_client_order_ids: non_empty_strings(&exec.filtered_client_order_ids),
        generate_missing_orders: exec.generate_missing_orders,
        inflight_check_interval_ms: exec.inflight_check_interval_ms,
        inflight_check_threshold_ms: exec.inflight_check_threshold_ms,
        inflight_check_retries: exec.inflight_check_retries,
        open_check_interval_secs: u64_zero_as_none_f64(exec.open_check_interval_secs),
        open_check_lookback_mins: u32_zero_as_none(exec.open_check_lookback_mins),
        open_check_threshold_ms: exec.open_check_threshold_ms,
        open_check_missing_retries: exec.open_check_missing_retries,
        open_check_open_only: exec.open_check_open_only,
        max_single_order_queries_per_cycle: exec.max_single_order_queries_per_cycle,
        single_order_query_delay_ms: exec.single_order_query_delay_ms,
        position_check_interval_secs: u64_zero_as_none_f64(exec.position_check_interval_secs),
        position_check_lookback_mins: exec.position_check_lookback_mins,
        position_check_threshold_ms: exec.position_check_threshold_ms,
        position_check_retries: exec.position_check_retries,
        purge_closed_orders_interval_mins: u32_zero_as_none(exec.purge_closed_orders_interval_mins),
        purge_closed_orders_buffer_mins: u32_zero_as_none(exec.purge_closed_orders_buffer_mins),
        purge_closed_positions_interval_mins: u32_zero_as_none(
            exec.purge_closed_positions_interval_mins,
        ),
        purge_closed_positions_buffer_mins: u32_zero_as_none(
            exec.purge_closed_positions_buffer_mins,
        ),
        purge_account_events_interval_mins: u32_zero_as_none(
            exec.purge_account_events_interval_mins,
        ),
        purge_account_events_lookback_mins: u32_zero_as_none(
            exec.purge_account_events_lookback_mins,
        ),
        purge_from_database: exec.purge_from_database,
        own_books_audit_interval_secs: u64_zero_as_none_f64(exec.own_books_audit_interval_secs),
        qsize: exec.qsize,
        allow_overfills: exec.allow_overfills,
        manage_own_order_books: exec.manage_own_order_books,
    };
    let risk_engine = nautilus_live::config::LiveRiskEngineConfig {
        // Mandated safety invariant: the NT live risk engine must never be
        // bypassed. This is pinned in code with no config knob so no TOML edit
        // or operator override can disable pre-trade risk checks.
        bypass: false,
        max_order_submit_rate: loaded.root.risk.nautilus.max_order_submit_rate.clone(),
        max_order_modify_rate: loaded.root.risk.nautilus.max_order_modify_rate.clone(),
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live risk config consumes the same string pairs as a HashMap.
        max_notional_per_order: loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .clone()
            .into_iter()
            .collect(),
        debug: loaded.root.risk.nautilus.debug,
        qsize: loaded.root.risk.nautilus.qsize,
    };

    // Explicit struct literal: upstream NT `LiveNodeConfig` field additions must be
    // considered here instead of silently inherited through `Default`.
    LiveNodeConfig {
        environment,
        trader_id,
        load_state: nautilus.load_state,
        save_state: nautilus.save_state,
        shutdown_on_error: nautilus.shutdown_on_error,
        logging,
        instance_id: None,
        timeout_connection: Duration::from_secs(nautilus.timeout_connection_secs),
        timeout_reconciliation: Duration::from_secs(nautilus.timeout_reconciliation_secs),
        timeout_portfolio: Duration::from_secs(nautilus.timeout_portfolio_secs),
        timeout_disconnection: Duration::from_secs(nautilus.timeout_disconnection_secs),
        delay_post_stop: Duration::from_secs(nautilus.delay_post_stop_secs),
        timeout_shutdown: Duration::from_secs(nautilus.timeout_shutdown_secs),
        cache: None,
        msgbus: None,
        portfolio: None,
        emulator: None,
        streaming: None,
        event_store: None,
        loop_debug: false,
        data_engine,
        risk_engine,
        exec_engine,
        data_clients: HashMap::new(),
        exec_clients: HashMap::new(),
        plugins: Vec::new(),
    }
}

fn u32_zero_as_none(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn u64_zero_as_none_f64(value: u64) -> Option<f64> {
    (value != 0).then_some(value as f64)
}

fn non_empty_strings(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

fn configured_external_clients(values: &[ClientId]) -> Option<Vec<ClientId>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// Caller must run root validation first so the string is a valid NT `BarIntervalType`.
fn bar_interval_type_from_str(value: &str) -> BarIntervalType {
    BarIntervalType::from_str(value).expect("root validation must accept data bar interval type")
}

pub fn wire_bolt_v3_runtime_capture(
    node: &LiveNode,
    stop_handle: LiveNodeHandle,
    loaded: &LoadedBoltV3Config,
) -> Result<NtRuntimeCaptureGuards> {
    wire_nt_runtime_capture(
        node,
        stop_handle,
        &loaded.root.persistence.catalog_directory,
        loaded.root.persistence.streaming.flush_interval_ms,
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_ms,
        None,
    )
}

/// Bolt-v3 controlled-connect boundary.
///
/// Drives the pinned NautilusTrader controlled-connect API
/// (`NautilusKernel::connect_data_clients` followed by
/// `NautilusKernel::connect_exec_clients`) on every NT data and
/// execution client that the bolt-v3 client-registration boundary added
/// to `node`, bounded by the bolt-v3
/// `nautilus.timeout_connection_secs` value from `loaded`.
///
/// This boundary is **opt-in**: the bolt-v3 node builders
/// (`build_bolt_v3_live_node_with_resolved` and its `_with` /
/// `_with_summary` siblings) deliberately do not invoke it.
/// A caller must explicitly call this function on a node previously
/// returned by one of those builders. In a bolt-v3-only process, NT's
/// first-wins logger is initialized by the bolt-v3 `LoggerConfig`
/// passed through `LiveNodeBuilder::build`, so the
/// provider-owned credential log module filters remain active during
/// connect.
/// The production bolt-v3 entrypoint preserves that ordering.
///
/// This boundary is **bounded**: the dispatched engine-level connect
/// futures are wrapped in `tokio::time::timeout` driven by
/// `nautilus.timeout_connection_secs`. If the bound elapses before
/// both engines finish dispatching connect to their registered clients
/// the function returns [`BoltV3LiveNodeError::ConnectTimeout`] and
/// the `LiveNode` is left in whatever partially-connected state NT
/// produced; the caller owns subsequent disconnect/teardown via
/// [`disconnect_bolt_v3_clients`].
///
/// This boundary is **dispatch + connected check**, not NT cache or
/// instrument readiness. The pinned NT `DataEngine::connect` and
/// `ExecutionEngine::connect` dispatchers swallow individual client
/// `connect()` errors and only log them, so after the dispatch
/// returns the bolt-v3 boundary consults
/// `NautilusKernel::check_engines_connected()` to ensure every
/// registered client transitioned to `is_connected`. If that check
/// returns false, the boundary returns
/// [`BoltV3LiveNodeError::ConnectIncomplete`] rather than `Ok(())`.
/// The boundary does **not** copy or reimplement NT private drain or
/// flush logic, and it does not gate on NT cache contents or
/// instrument-availability checks; that readiness is owned by a
/// future slice.
///
/// This boundary is **no-trade**: it never enters NT's runner loop
/// and never invokes NT's trader entrypoint, so no strategy actor is
/// activated, no reconciliation runs, and the runner loop is never
/// entered. `NodeState` therefore remains in whatever state the node
/// was in before the call (typically `Idle`). The boundary does not
/// register strategies, select markets, construct orders, submit
/// orders, or invoke any user-level subscription API.
///
/// Errors from individual NT client `connect()` calls are surfaced
/// via NT's logger (the engine-level dispatchers in
/// `nautilus_data::engine::DataEngine::connect` and
/// `nautilus_execution::engine::ExecutionEngine::connect` log
/// individual `Err` values rather than propagating them). The bolt-v3
/// boundary returns `Ok(())` only when both dispatchers have returned
/// within the configured bound **and**
/// `kernel.check_engines_connected()` returns true.
pub async fn connect_bolt_v3_clients(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_secs = loaded.root.nautilus.timeout_connection_secs;
    let bound = Duration::from_secs(timeout_secs);
    let connect = async {
        let kernel = node.kernel_mut();
        kernel.connect_data_clients().await;
        kernel.connect_exec_clients().await;
        kernel.check_engines_connected()
    };
    match tokio::time::timeout(bound, connect).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(BoltV3LiveNodeError::ConnectIncomplete),
        Err(_) => {
            let node_state = format!("{:?}", node.state());
            let kernel = node.kernel();
            Err(BoltV3LiveNodeError::ConnectTimeout {
                timeout_secs,
                node_state,
                not_connected_clients: live_node_not_connected_client_labels_from_statuses(
                    kernel.data_client_connection_status(),
                    kernel.exec_client_connection_status(),
                ),
            })
        }
    }
}

/// Bolt-v3 controlled-disconnect boundary.
///
/// Drives the pinned NautilusTrader controlled-disconnect API
/// (`NautilusKernel::disconnect_clients`) on every NT data and
/// execution client previously added through the bolt-v3
/// client-registration boundary, bounded by the bolt-v3
/// `nautilus.timeout_disconnection_secs` value from `loaded`.
///
/// Recovery counterpart to [`connect_bolt_v3_clients`]: after a
/// `ConnectTimeout` or `ConnectIncomplete` the caller is expected to
/// invoke this function to drain whatever partially-connected NT
/// clients survive, again under a bounded timeout.
///
/// This boundary is **bounded**: NT's
/// `kernel.disconnect_clients()` future is wrapped in
/// `tokio::time::timeout`. On the bound elapsing, the function
/// returns [`BoltV3LiveNodeError::DisconnectTimeout`] with the
/// configured bound. On NT's engine-level disconnect aggregator
/// surfacing an `Err(..)`, the function returns
/// [`BoltV3LiveNodeError::DisconnectFailed`] wrapping the NT
/// `anyhow::Error`. Pinned NT disconnects data clients before
/// execution clients and can short-circuit on a data-client error; a
/// `DisconnectFailed` therefore leaves cleanup state indeterminate and
/// production recovery should rebuild a fresh `LiveNode`.
///
/// This boundary is **no-trade**: it never enters NT's runner loop,
/// never invokes NT's trader entrypoint, never registers strategies,
/// never selects markets, never constructs orders, never submits
/// orders, and never invokes any user-level subscription API. It
/// does not call `LiveNode::stop`; the bolt-v3 LiveNode remains
/// outside NT's runner-driven lifecycle. The boundary does **not**
/// copy or reimplement NT private drain or flush logic.
pub async fn disconnect_bolt_v3_clients(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_secs = loaded.root.nautilus.timeout_disconnection_secs;
    let bound = Duration::from_secs(timeout_secs);
    let disconnect = async { node.kernel_mut().disconnect_clients().await };
    match tokio::time::timeout(bound, disconnect).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(BoltV3LiveNodeError::DisconnectFailed(error)),
        Err(_) => Err(BoltV3LiveNodeError::DisconnectTimeout { timeout_secs }),
    }
}
