//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or submit paths.
//!
//! This module is a no-trade boundary. It only:
//! - validates the forbidden credential env-var blocklist before constructing
//!   any NautilusTrader client
//! - builds a `nautilus_live::node::LiveNode` from validated bolt-v3 root
//!   config without calling `node.run()`
//! - wires the existing `crate::nt_runtime_capture` from the
//!   `[persistence]` / `[persistence.streaming]` blocks
//!
//! The caller owns the `LiveNode`; this module never starts the event loop.

use std::time::Duration;

use anyhow::Result;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::{
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle},
};
use nautilus_model::identifiers::TraderId;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, RuntimeMode},
    bolt_v3_secrets::{
        ForbiddenEnvVarError, check_no_forbidden_credential_env_vars,
        check_no_forbidden_credential_env_vars_with,
    },
    nt_runtime_capture::{NtRuntimeCaptureGuards, wire_nt_runtime_capture},
};

#[derive(Debug)]
pub enum BoltV3LiveNodeError {
    ForbiddenEnv(ForbiddenEnvVarError),
    Build(anyhow::Error),
}

impl std::fmt::Display for BoltV3LiveNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
        }
    }
}

impl std::error::Error for BoltV3LiveNodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => Some(error),
            BoltV3LiveNodeError::Build(error) => error.source(),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNode, BoltV3LiveNodeError> {
    check_no_forbidden_credential_env_vars(&loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    build_live_node_after_env_check(loaded)
}

/// Test-friendly variant of [`build_bolt_v3_live_node`] which lets the caller
/// inject the environment-variable predicate. Production code must use
/// [`build_bolt_v3_live_node`], which queries `std::env`.
pub fn build_bolt_v3_live_node_with<F>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
) -> Result<LiveNode, BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
{
    check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    build_live_node_after_env_check(loaded)
}

fn build_live_node_after_env_check(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNode, BoltV3LiveNodeError> {
    let live_config = make_live_node_config(loaded);
    LiveNode::build(loaded.root.trader_id.clone(), Some(live_config))
        .map_err(BoltV3LiveNodeError::Build)
}

pub fn make_live_node_config(loaded: &LoadedBoltV3Config) -> LiveNodeConfig {
    let trader_id = TraderId::from(loaded.root.trader_id.as_str());
    let environment = match loaded.root.runtime.mode {
        RuntimeMode::Live => Environment::Live,
    };
    let logging = LoggerConfig {
        stdout_level: loaded.root.logging.standard_output_level.to_level_filter(),
        fileout_level: loaded.root.logging.file_level.to_level_filter(),
        ..Default::default()
    };
    let nautilus = &loaded.root.nautilus;
    let reconciliation_lookback_mins = if nautilus.reconciliation_lookback_mins == 0 {
        None
    } else {
        Some(nautilus.reconciliation_lookback_mins as u32)
    };
    let exec_engine = nautilus_live::config::LiveExecEngineConfig {
        reconciliation_lookback_mins,
        ..Default::default()
    };

    LiveNodeConfig {
        environment,
        trader_id,
        load_state: nautilus.load_state,
        save_state: nautilus.save_state,
        logging,
        timeout_connection: Duration::from_secs(nautilus.timeout_connection_seconds),
        timeout_reconciliation: Duration::from_secs(nautilus.timeout_reconciliation_seconds),
        timeout_portfolio: Duration::from_secs(nautilus.timeout_portfolio_seconds),
        timeout_disconnection: Duration::from_secs(nautilus.timeout_disconnection_seconds),
        delay_post_stop: Duration::from_secs(nautilus.delay_post_stop_seconds),
        timeout_shutdown: Duration::from_secs(nautilus.timeout_shutdown_seconds),
        exec_engine,
        ..Default::default()
    }
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
        loaded
            .root
            .persistence
            .streaming
            .flush_interval_milliseconds,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_config::BoltV3RootConfig;

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root_text = include_str!("../tests/fixtures/bolt_v3/root.toml");
        let root: BoltV3RootConfig = toml::from_str(root_text).unwrap();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root,
            strategies: Vec::new(),
        }
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
    fn live_node_config_maps_zero_lookback_to_unbounded_reconciliation() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);
        assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
    }

    #[test]
    fn live_node_config_maps_log_levels_from_uppercase_strings() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);
        assert_eq!(cfg.logging.stdout_level, log::LevelFilter::Info);
        assert_eq!(cfg.logging.fileout_level, log::LevelFilter::Info);
    }
}
