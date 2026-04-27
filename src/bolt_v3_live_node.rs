//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or submit paths.
//!
//! Bolt-v3 LiveNode controlled-build / controlled-connect /
//! controlled-disconnect boundary. This module:
//!
//! - validates the forbidden credential env-var blocklist before
//!   constructing any NautilusTrader client
//! - resolves SSM secrets via the bolt-v3 secret resolver
//! - maps the validated bolt-v3 venue blocks into NT-native adapter
//!   configs (`PolymarketDataClientConfig`, `PolymarketExecClientConfig`,
//!   `BinanceDataClientConfig`)
//! - registers the per-venue NT data and execution client factories on a
//!   `nautilus_live::builder::LiveNodeBuilder` via the
//!   [`crate::bolt_v3_client_registration`] boundary
//! - calls `LiveNodeBuilder::build`, which is **not** purely passive:
//!   it constructs the NT client objects, parses the Polymarket private
//!   key into an NT secp256k1 signer (deriving the EVM address), and
//!   performs internal NT engine/message-bus subscriptions for venue
//!   instrument topics. None of these steps open a network connection
//!   or run the event loop.
//! - returns the resulting `nautilus_live::node::LiveNode` to the caller
//!   without entering the NT runner loop
//! - wires the existing `crate::nt_runtime_capture` from the
//!   `[persistence]` / `[persistence.streaming]` blocks
//! - installs module-level logger filters that suppress NT credential
//!   info logs from `nautilus_polymarket::common::credential` and
//!   `nautilus_binance::common::credential` even when the root TOML log
//!   level is `INFO`
//!
//! The caller owns the `LiveNode`; the build path never opens an
//! external network connection. The opt-in controlled-connect boundary
//! may open adapter sockets, but this module never starts the event
//! loop, subscribes to market data through any user-level `subscribe_*`
//! API, registers a strategy actor, constructs an order, or enables
//! any submit path.

use std::time::Duration;

use ahash::AHashMap;
use anyhow::Result;
use log::LevelFilter;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle},
};
use nautilus_model::identifiers::TraderId;
use ustr::Ustr;

/// NT adapter modules whose `log::info!` calls embed credential
/// material (Polymarket address/funder/api-key prefixes; Binance
/// auto-detected key type). Bolt-v3 forces these targets to `WARN` even
/// when the root TOML log level is `INFO`, so credential prefixes never
/// land in stdout or the file writer. NT's logger does prefix matching
/// on the `component` field, which defaults to the source module path
/// when no `component=` key-value pair is supplied — the credential log
/// sites use the bare `log::info!` macro, so the module path applies.
pub const NT_CREDENTIAL_LOG_MODULES: &[&str] = &[
    "nautilus_polymarket::common::credential",
    "nautilus_binance::common::credential",
];

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
    /// The bolt-v3 controlled-connect boundary
    /// ([`connect_bolt_v3_clients`]) bounds the dispatched
    /// `NautilusKernel::connect_data_clients` and
    /// `NautilusKernel::connect_exec_clients` calls by the
    /// `nautilus.timeout_connection_seconds` value from the loaded
    /// bolt-v3 config. A `ConnectTimeout` is surfaced when that bound
    /// elapses before NT's engine-level connect dispatchers return,
    /// instead of the controlled-connect call hanging indefinitely.
    /// The wrapped value is the configured timeout the boundary
    /// applied (in seconds), captured so log/audit consumers can
    /// distinguish a 1-second test timeout from a 30-second
    /// production timeout without re-reading the source config.
    ConnectTimeout {
        timeout_seconds: u64,
    },
    /// The bolt-v3 controlled-connect boundary dispatched both NT
    /// engine-level connect futures within the configured bound, but
    /// at least one registered NT data or execution client did not
    /// transition to `is_connected` afterwards. The pinned NT
    /// `DataEngine::connect` and `ExecutionEngine::connect`
    /// dispatchers swallow individual client `connect()` errors and
    /// only log them, so bolt-v3 consults
    /// `NautilusKernel::check_engines_connected()` after dispatch
    /// returns to keep this failure mode honest. This slice keeps the
    /// variant generic rather than synthesizing a per-client failure
    /// list. Callers should follow this with a
    /// [`disconnect_bolt_v3_clients`] call to drain any partially
    /// connected clients under the bounded controlled-disconnect
    /// boundary.
    ConnectIncomplete,
    /// The bolt-v3 controlled-disconnect boundary
    /// ([`disconnect_bolt_v3_clients`]) bounds the
    /// `NautilusKernel::disconnect_clients` future by the
    /// `nautilus.timeout_disconnection_seconds` value from the loaded
    /// bolt-v3 config. A `DisconnectTimeout` is surfaced when that
    /// bound elapses before NT finishes disconnecting all data and
    /// execution clients, instead of the controlled-disconnect call
    /// hanging indefinitely. The wrapped value is the configured
    /// timeout the boundary applied (in seconds).
    DisconnectTimeout {
        timeout_seconds: u64,
    },
    /// The bolt-v3 controlled-disconnect boundary dispatched
    /// `NautilusKernel::disconnect_clients` and NT returned an
    /// `Err(..)` from at least one registered client's `disconnect()`
    /// call. The wrapped `anyhow::Error` is the value NT bubbled up
    /// from its engine-level disconnect aggregator.
    DisconnectFailed(anyhow::Error),
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
            BoltV3LiveNodeError::ConnectTimeout { timeout_seconds } => write!(
                f,
                "bolt-v3 controlled-connect exceeded the configured \
                 nautilus.timeout_connection_seconds bound ({timeout_seconds}s)"
            ),
            BoltV3LiveNodeError::ConnectIncomplete => write!(
                f,
                "bolt-v3 controlled-connect dispatched both NT engine-level connect \
                 futures within the configured bound but `kernel.check_engines_connected()` \
                 returned false; at least one registered NT data or execution client did \
                 not transition to is_connected after NT swallowed/logged its connect error"
            ),
            BoltV3LiveNodeError::DisconnectTimeout { timeout_seconds } => write!(
                f,
                "bolt-v3 controlled-disconnect exceeded the configured \
                 nautilus.timeout_disconnection_seconds bound ({timeout_seconds}s)"
            ),
            BoltV3LiveNodeError::DisconnectFailed(error) => write!(
                f,
                "bolt-v3 controlled-disconnect surfaced an NT engine-level disconnect \
                 aggregator error: {error}"
            ),
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
            BoltV3LiveNodeError::ConnectTimeout { .. }
            | BoltV3LiveNodeError::ConnectIncomplete
            | BoltV3LiveNodeError::DisconnectTimeout { .. } => None,
            BoltV3LiveNodeError::DisconnectFailed(error) => error.source(),
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
    let mut module_level: AHashMap<Ustr, LevelFilter> = AHashMap::new();
    for module_path in NT_CREDENTIAL_LOG_MODULES {
        module_level.insert(Ustr::from(module_path), LevelFilter::Warn);
    }
    let logging = LoggerConfig {
        stdout_level: loaded.root.logging.standard_output_level.to_level_filter(),
        fileout_level: loaded.root.logging.file_level.to_level_filter(),
        module_level,
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

/// Bolt-v3 controlled-connect boundary.
///
/// Drives the pinned NautilusTrader controlled-connect API
/// (`NautilusKernel::connect_data_clients` followed by
/// `NautilusKernel::connect_exec_clients`) on every NT data and
/// execution client that the bolt-v3 client-registration boundary added
/// to `node`, bounded by the bolt-v3
/// `nautilus.timeout_connection_seconds` value from `loaded`.
///
/// This boundary is **opt-in**: `build_bolt_v3_live_node` and its
/// `_with` / `_with_summary` siblings deliberately do not invoke it.
/// A caller must explicitly call this function on a node previously
/// returned by one of those builders. In a bolt-v3-only process, NT's
/// first-wins logger is initialized by the bolt-v3 `LoggerConfig`
/// passed through `LiveNodeBuilder::build`, so the
/// `NT_CREDENTIAL_LOG_MODULES` filter remains active during connect.
/// A future production v3 entrypoint must preserve that ordering.
///
/// This boundary is **bounded**: the dispatched engine-level connect
/// futures are wrapped in `tokio::time::timeout` driven by
/// `nautilus.timeout_connection_seconds`. If the bound elapses before
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
    let timeout_seconds = loaded.root.nautilus.timeout_connection_seconds;
    let bound = Duration::from_secs(timeout_seconds);
    let connect = async {
        let kernel = node.kernel_mut();
        kernel.connect_data_clients().await;
        kernel.connect_exec_clients().await;
        kernel.check_engines_connected()
    };
    match tokio::time::timeout(bound, connect).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(BoltV3LiveNodeError::ConnectIncomplete),
        Err(_) => Err(BoltV3LiveNodeError::ConnectTimeout { timeout_seconds }),
    }
}

/// Bolt-v3 controlled-disconnect boundary.
///
/// Drives the pinned NautilusTrader controlled-disconnect API
/// (`NautilusKernel::disconnect_clients`) on every NT data and
/// execution client previously added through the bolt-v3
/// client-registration boundary, bounded by the bolt-v3
/// `nautilus.timeout_disconnection_seconds` value from `loaded`.
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
    let timeout_seconds = loaded.root.nautilus.timeout_disconnection_seconds;
    let bound = Duration::from_secs(timeout_seconds);
    let disconnect = async { node.kernel_mut().disconnect_clients().await };
    match tokio::time::timeout(bound, disconnect).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(BoltV3LiveNodeError::DisconnectFailed(error)),
        Err(_) => Err(BoltV3LiveNodeError::DisconnectTimeout { timeout_seconds }),
    }
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

        for module_path in NT_CREDENTIAL_LOG_MODULES {
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
}
