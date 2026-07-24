use super::*;

pub(super) fn validate_nautilus_block(block: &NautilusBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        (
            "nautilus.timeout_connection_secs",
            block.timeout_connection_secs,
        ),
        (
            "nautilus.timeout_reconciliation_secs",
            block.timeout_reconciliation_secs,
        ),
        (
            "nautilus.timeout_portfolio_secs",
            block.timeout_portfolio_secs,
        ),
        (
            "nautilus.timeout_disconnection_secs",
            block.timeout_disconnection_secs,
        ),
        (
            "nautilus.timeout_shutdown_secs",
            block.timeout_shutdown_secs,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }
    errors.extend(validate_data_engine_block(&block.data_engine));
    errors.extend(validate_exec_engine_block(&block.exec_engine));
    errors
}

fn validate_data_engine_block(
    block: &crate::bolt_v3_config::NautilusDataEngineBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(error) = BarIntervalType::from_str(&block.time_bars_interval_type) {
        errors.push(format!(
            "nautilus.data_engine.time_bars_interval_type is not valid ({error}): `{}`",
            block.time_bars_interval_type
        ));
    }
    for aggregation in block.time_bars_origins.keys() {
        if let Err(error) = BarAggregation::from_str(aggregation) {
            errors.push(format!(
                "nautilus.data_engine.time_bars_origins key `{aggregation}` is not a valid Nautilus bar aggregation ({error})"
            ));
        }
    }
    let nt_data_default = nautilus_live::config::LiveDataEngineConfig::default();
    if block.qsize != nt_data_default.qsize {
        errors.push(format!(
            "nautilus.data_engine.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_data_default.qsize
        ));
    }
    errors
}

fn validate_exec_engine_block(
    block: &crate::bolt_v3_config::NautilusExecEngineBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        (
            "nautilus.exec_engine.inflight_check_threshold_ms",
            block.inflight_check_threshold_ms as u64,
        ),
        (
            "nautilus.exec_engine.open_check_threshold_ms",
            block.open_check_threshold_ms as u64,
        ),
        (
            "nautilus.exec_engine.max_single_order_queries_per_cycle",
            block.max_single_order_queries_per_cycle as u64,
        ),
        (
            "nautilus.exec_engine.position_check_threshold_ms",
            block.position_check_threshold_ms as u64,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }

    if block.snapshot_orders {
        errors.push(
            "nautilus.exec_engine.snapshot_orders must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.snapshot_positions {
        errors.push(
            "nautilus.exec_engine.snapshot_positions must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if !block.reconciliation_fail_closed {
        errors.push(
            "nautilus.exec_engine.reconciliation_fail_closed must be true; Bolt requires NT to stop before continuing with incomplete continuous reconciliation"
                .to_string(),
        );
    }
    if block.purge_from_database {
        errors.push(
            "nautilus.exec_engine.purge_from_database must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    let nt_exec_default = nautilus_live::config::LiveExecEngineConfig::default();
    if block.qsize != nt_exec_default.qsize {
        errors.push(format!(
            "nautilus.exec_engine.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_exec_default.qsize
        ));
    }

    for instrument_id in &block.reconciliation_instrument_ids {
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "nautilus.exec_engine.reconciliation_instrument_ids contains invalid instrument ID `{instrument_id}` ({error})"
            ));
        }
    }
    for client_order_id in &block.filtered_client_order_ids {
        if let Err(error) = ClientOrderId::new_checked(client_order_id) {
            errors.push(format!(
                "nautilus.exec_engine.filtered_client_order_ids contains invalid client order ID `{client_order_id}` ({error})"
            ));
        }
    }
    errors
}
