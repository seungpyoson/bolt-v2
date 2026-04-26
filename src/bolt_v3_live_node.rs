//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or submit paths.
//!
//! This module is a no-trade boundary. It only:
//! - validates the forbidden credential env-var blocklist before
//!   constructing any NautilusTrader client
//! - resolves SSM secrets via the bolt-v3 secret resolver
//! - maps the validated bolt-v3 venue blocks into NT-native adapter
//!   configs
//! - registers the per-venue NT data and execution client factories on a
//!   `nautilus_live::builder::LiveNodeBuilder` via the
//!   [`crate::bolt_v3_client_registration`] boundary
//! - finalizes the builder into a `nautilus_live::node::LiveNode`
//!   without calling `node.run()`
//! - wires the existing `crate::nt_runtime_capture` from the
//!   `[persistence]` / `[persistence.streaming]` blocks
//!
//! The caller owns the `LiveNode`; this module never starts the event
//! loop, opens a network connection, subscribes to market data,
//! constructs orders, or enables any submit path.

use std::time::Duration;

use anyhow::Result;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle},
};
use nautilus_model::identifiers::TraderId;

use crate::{
    bolt_v3_adapters::{BoltV3AdapterConfigs, BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{LoadedBoltV3Config, RuntimeMode},
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, check_no_forbidden_credential_env_vars,
        check_no_forbidden_credential_env_vars_with, resolve_bolt_v3_secrets,
        resolve_bolt_v3_secrets_with,
    },
    nt_runtime_capture::{NtRuntimeCaptureGuards, wire_nt_runtime_capture},
};

#[derive(Debug)]
pub enum BoltV3LiveNodeError {
    ForbiddenEnv(ForbiddenEnvVarError),
    SecretResolution(BoltV3SecretError),
    AdapterMapping(BoltV3AdapterMappingError),
    ClientRegistration(BoltV3ClientRegistrationError),
    Build(anyhow::Error),
}

impl std::fmt::Display for BoltV3LiveNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::SecretResolution(error) => {
                write!(f, "bolt-v3 secret resolution failed: {error}")
            }
            BoltV3LiveNodeError::AdapterMapping(error) => {
                write!(f, "bolt-v3 adapter config mapping failed: {error}")
            }
            BoltV3LiveNodeError::ClientRegistration(error) => {
                write!(f, "bolt-v3 client registration failed: {error}")
            }
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
        }
    }
}

impl std::error::Error for BoltV3LiveNodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => Some(error),
            BoltV3LiveNodeError::SecretResolution(error) => Some(error),
            BoltV3LiveNodeError::AdapterMapping(error) => Some(error),
            BoltV3LiveNodeError::ClientRegistration(error) => Some(error),
            BoltV3LiveNodeError::Build(error) => error.source(),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNode, BoltV3LiveNodeError> {
    check_no_forbidden_credential_env_vars(&loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved =
        resolve_bolt_v3_secrets(loaded).map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let (node, _summary) = build_live_node_with_clients(loaded, &adapters)?;
    Ok(node)
}

/// Test-friendly variant of [`build_bolt_v3_live_node`] which lets the caller
/// inject the environment-variable predicate and the SSM resolver. Production
/// code must use [`build_bolt_v3_live_node`], which queries `std::env` and
/// invokes the real Amazon Web Services Systems Manager resolver.
pub fn build_bolt_v3_live_node_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<LiveNode, BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let (node, _summary) = build_bolt_v3_live_node_with_summary(loaded, env_is_set, resolver)?;
    Ok(node)
}

/// Same as [`build_bolt_v3_live_node_with`] but also returns the
/// [`BoltV3RegistrationSummary`] so tests can assert which NT client
/// kinds the registration boundary added before the builder finalized
/// the node. Not intended for production code paths; production reads
/// the summary by other means if it ever needs to.
pub fn build_bolt_v3_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(LiveNode, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    build_live_node_with_clients(loaded, &adapters)
}

fn build_live_node_with_clients(
    loaded: &LoadedBoltV3Config,
    adapters: &BoltV3AdapterConfigs,
) -> Result<(LiveNode, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    let builder = make_bolt_v3_live_node_builder(loaded).map_err(BoltV3LiveNodeError::Build)?;
    let (builder, summary) = register_bolt_v3_clients(builder, adapters)
        .map_err(BoltV3LiveNodeError::ClientRegistration)?;
    let node = builder.build().map_err(BoltV3LiveNodeError::Build)?;
    Ok((node, summary))
}

/// Translates a validated bolt-v3 config into an NT-native
/// [`LiveNodeBuilder`] with no clients added. Field translation goes
/// through [`make_live_node_config`] so the bolt-v3 → NT field mapping
/// has a single source of truth that the existing per-field tests can
/// keep exercising.
pub fn make_bolt_v3_live_node_builder(
    loaded: &LoadedBoltV3Config,
) -> anyhow::Result<LiveNodeBuilder> {
    let cfg = make_live_node_config(loaded);
    let mut builder = LiveNode::builder(cfg.trader_id, cfg.environment)?
        .with_logging(cfg.logging.clone())
        .with_load_state(cfg.load_state)
        .with_save_state(cfg.save_state)
        .with_timeout_connection(cfg.timeout_connection.as_secs())
        .with_timeout_reconciliation(cfg.timeout_reconciliation.as_secs())
        .with_timeout_portfolio(cfg.timeout_portfolio.as_secs())
        .with_timeout_disconnection_secs(cfg.timeout_disconnection.as_secs())
        .with_delay_post_stop_secs(cfg.delay_post_stop.as_secs())
        .with_delay_shutdown_secs(cfg.timeout_shutdown.as_secs());
    if let Some(mins) = cfg.exec_engine.reconciliation_lookback_mins {
        builder = builder.with_reconciliation_lookback_mins(mins);
    }
    Ok(builder)
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
