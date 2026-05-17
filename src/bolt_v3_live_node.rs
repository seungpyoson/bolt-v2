//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or submit paths.
//!
//! Bolt-v3 LiveNode controlled-build / controlled-connect /
//! controlled-disconnect boundary. This module:
//!
//! - validates the forbidden credential env-var blocklist before
//!   constructing any NautilusTrader client
//! - resolves SSM secrets via the bolt-v3 secret resolver
//! - maps the validated bolt-v3 venue blocks into provider-owned
//!   NT-native adapter configs
//! - registers the per-venue NT data and execution client factories on a
//!   `nautilus_live::builder::LiveNodeBuilder` via the
//!   [`crate::bolt_v3_client_registration`] boundary
//! - calls `LiveNodeBuilder::build`, which is **not** purely passive:
//!   it constructs the NT client objects, lets provider-owned NT
//!   factories parse their credential material, and performs internal
//!   NT engine/message-bus subscriptions for venue instrument topics.
//!   None of these steps open a network connection or run the event
//!   loop.
//! - returns the resulting `nautilus_live::node::LiveNode` to the caller
//!   without entering the NT runner loop from the build path
//! - wires the existing `crate::nt_runtime_capture` from the
//!   `[persistence]` / `[persistence.streaming]` blocks
//! - installs module-level logger filters from provider-owned bindings
//!   that suppress NT credential info logs even when the root TOML log
//!   level is `INFO`
//!
//! The caller owns the `LiveNode`; the build path never opens an
//! external network connection. The opt-in controlled-connect boundary
//! may open adapter sockets. The sole approved NT runner entrypoint in
//! this module is [`run_bolt_v3_live_node`], which first applies the
//! bolt-v3 live canary gate. This module still never calls user-level
//! market-data subscription APIs, registers a strategy actor, constructs
//! an order, or enables any submit path.

use std::{collections::HashMap, future::Future, str::FromStr, sync::Arc, time::Duration};

use ahash::AHashMap;
use anyhow::Result;
use log::LevelFilter;
use nautilus_common::{
    enums::Environment,
    logging::{logger::LoggerConfig, writer::FileWriterConfig},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle, NodeState},
};
use nautilus_model::{
    enums::BarIntervalType,
    identifiers::{ClientId, StrategyId, TraderId},
};
use nautilus_system::config::{RotationConfig, StreamingConfig};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use ustr::Ustr;

use crate::{
    bolt_v3_adapters::{BoltV3AdapterConfigs, BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{LoadedBoltV3Config, LogLevel, NautilusComponentConfig, RuntimeMode},
    bolt_v3_decision_evidence::{BoltV3DecisionEvidenceWriter, JsonlBoltV3DecisionEvidenceWriter},
    bolt_v3_live_canary_gate::{BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_gate},
    bolt_v3_providers,
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars, check_no_forbidden_credential_env_vars_with,
        resolve_bolt_v3_secrets, resolve_bolt_v3_secrets_with,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, register_bolt_v3_strategies_on_node_with_bindings,
    },
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionState},
    nt_runtime_capture::{NtRuntimeCaptureGuards, wire_nt_runtime_capture},
    secrets::SsmResolverSession,
};

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    redaction_values: Vec<String>,
}

impl BoltV3LiveNodeRuntime {
    fn new(
        node: LiveNode,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        redaction_values: Vec<String>,
    ) -> Self {
        Self {
            node,
            submit_admission,
            redaction_values,
        }
    }

    pub fn registered_strategy_ids(&self) -> Vec<StrategyId> {
        self.node.kernel().trader().borrow().strategy_ids()
    }

    pub fn environment(&self) -> Environment {
        self.node.environment()
    }

    pub fn state(&self) -> NodeState {
        self.node.state()
    }

    pub fn registered_data_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().data_engine.borrow().registered_clients()
    }

    pub fn registered_exec_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().exec_engine.borrow().client_ids()
    }

    pub fn cached_instrument_ids(&self) -> Vec<String> {
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        cache
            .instrument_ids(None)
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }

    pub fn redaction_values(&self) -> &[String] {
        &self.redaction_values
    }

    pub fn instance_id(&self) -> String {
        self.node.instance_id().to_string()
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.submit_admission.admitted_order_count()
    }
}

impl std::fmt::Debug for BoltV3LiveNodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoltV3LiveNodeRuntime")
            .field("node", &"[redacted]")
            .field("submit_admission", &self.submit_admission)
            .field("redaction_values", &"[redacted]")
            .finish()
    }
}

#[derive(Debug)]
pub enum BoltV3LiveNodeBuilderError {
    ConfigMapping {
        field: &'static str,
        message: String,
    },
    BuilderConstruction {
        source: anyhow::Error,
    },
}

impl std::fmt::Display for BoltV3LiveNodeBuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeBuilderError::ConfigMapping { field, message } => {
                write!(f, "bolt-v3 NT config mapping failed for {field}: {message}")
            }
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => {
                write!(f, "NT LiveNodeBuilder construction failed: {source}")
            }
        }
    }
}

impl std::error::Error for BoltV3LiveNodeBuilderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeBuilderError::ConfigMapping { .. } => None,
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => Some(source.as_ref()),
        }
    }
}

#[derive(Debug)]
pub enum BoltV3LiveNodeError {
    ForbiddenEnv(ForbiddenEnvVarError),
    /// `SsmResolverSession::new()` failed before any venue secret was
    /// read. The wrapped `SecretError` is the upstream Tokio /
    /// AWS-SDK-config setup failure. Distinct from
    /// [`SecretResolution`] (which carries a per-venue `BoltV3SecretError`
    /// with venue key, secret-config field name, and SSM path) because
    /// session setup happens before any venue path is consulted, so an
    /// operator message that names a venue or SSM path would be wrong.
    SecretResolverSetup(crate::secrets::SecretError),
    SecretResolution(BoltV3SecretError),
    AdapterMapping(BoltV3AdapterMappingError),
    BuilderConstruction(BoltV3LiveNodeBuilderError),
    ClientRegistration(BoltV3ClientRegistrationError),
    StrategyRegistration(BoltV3StrategyRegistrationError),
    Build(anyhow::Error),
    /// The live canary gate rejected entry to NT's runner loop before
    /// `LiveNode::run` was invoked. This variant wraps the specific
    /// fail-closed reason from [`BoltV3LiveCanaryGateError`].
    LiveCanaryGate(BoltV3LiveCanaryGateError),
    /// The validated live canary gate report could not arm the shared
    /// submit-admission state before `LiveNode::run` was invoked.
    SubmitAdmission(BoltV3SubmitAdmissionError),
    /// NT returned an error from `LiveNode::run` after the live canary
    /// gate accepted the loaded config and readiness report.
    Run(anyhow::Error),
    /// NT runtime capture could not be wired from the validated
    /// bolt-v3 `[persistence]` config before the runner loop started.
    RuntimeCaptureWire(anyhow::Error),
    /// NT runtime capture failed during shutdown after the runner loop
    /// exited or after the capture worker asked the LiveNode to stop.
    RuntimeCaptureShutdown(anyhow::Error),
    /// NT's runner loop and runtime-capture shutdown both failed. This
    /// preserves both failure categories instead of reporting the
    /// compound case as only a capture-shutdown error.
    RunAndRuntimeCaptureShutdown {
        run_error: anyhow::Error,
        shutdown_error: anyhow::Error,
    },
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
    /// returns to keep this failure mode honest. The variant stays
    /// generic rather than synthesizing a per-client failure list.
    /// Callers should follow this with a
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
    NoSubmitStartTimeout {
        timeout_seconds: u64,
    },
    NoSubmitStartFailed(anyhow::Error),
    NoSubmitStopTimeout {
        timeout_seconds: u64,
    },
    NoSubmitStopFailed(anyhow::Error),
}

impl std::fmt::Display for BoltV3LiveNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::SecretResolverSetup(error) => write!(
                f,
                "bolt-v3 SSM resolver session setup failed before any venue \
                 secret could be read: {error}"
            ),
            BoltV3LiveNodeError::SecretResolution(error) => {
                write!(f, "bolt-v3 secret resolution failed: {error}")
            }
            BoltV3LiveNodeError::AdapterMapping(error) => {
                write!(f, "bolt-v3 adapter config mapping failed: {error}")
            }
            BoltV3LiveNodeError::BuilderConstruction(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::ClientRegistration(error) => {
                write!(f, "bolt-v3 client registration failed: {error}")
            }
            BoltV3LiveNodeError::StrategyRegistration(error) => {
                write!(f, "bolt-v3 strategy registration failed: {error}")
            }
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
            BoltV3LiveNodeError::LiveCanaryGate(error) => {
                write!(
                    f,
                    "bolt-v3 live canary gate rejected runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::SubmitAdmission(error) => {
                write!(
                    f,
                    "bolt-v3 submit admission rejected runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::Run(error) => write!(f, "LiveNode run failed: {error}"),
            BoltV3LiveNodeError::RuntimeCaptureWire(error) => {
                write!(f, "NT runtime capture wiring failed: {error}")
            }
            BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => {
                write!(f, "NT runtime capture shutdown failed: {error}")
            }
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            } => write!(
                f,
                "LiveNode run failed and NT runtime capture shutdown failed: \
                 run error: {run_error}; shutdown error: {shutdown_error}"
            ),
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
            BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_seconds } => write!(
                f,
                "bolt-v3 no-submit controlled-start exceeded configured \
                 live-node timeout bounds ({timeout_seconds}s)"
            ),
            BoltV3LiveNodeError::NoSubmitStartFailed(error) => {
                write!(f, "bolt-v3 no-submit controlled-start failed: {error}")
            }
            BoltV3LiveNodeError::NoSubmitStopTimeout { timeout_seconds } => write!(
                f,
                "bolt-v3 no-submit controlled-stop exceeded configured \
                 live-node timeout bounds ({timeout_seconds}s)"
            ),
            BoltV3LiveNodeError::NoSubmitStopFailed(error) => {
                write!(f, "bolt-v3 no-submit controlled-stop failed: {error}")
            }
        }
    }
}

impl std::error::Error for BoltV3LiveNodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => Some(error),
            BoltV3LiveNodeError::SecretResolverSetup(error) => Some(error),
            BoltV3LiveNodeError::SecretResolution(error) => Some(error),
            BoltV3LiveNodeError::AdapterMapping(error) => Some(error),
            BoltV3LiveNodeError::BuilderConstruction(error) => Some(error),
            BoltV3LiveNodeError::ClientRegistration(error) => Some(error),
            BoltV3LiveNodeError::StrategyRegistration(error) => Some(error),
            BoltV3LiveNodeError::Build(error) => error.source(),
            BoltV3LiveNodeError::LiveCanaryGate(error) => Some(error),
            BoltV3LiveNodeError::SubmitAdmission(error) => Some(error),
            BoltV3LiveNodeError::Run(error) => error.source(),
            BoltV3LiveNodeError::RuntimeCaptureWire(error)
            | BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => error.source(),
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown { run_error, .. } => {
                Some(run_error.as_ref())
            }
            BoltV3LiveNodeError::ConnectTimeout { .. }
            | BoltV3LiveNodeError::ConnectIncomplete
            | BoltV3LiveNodeError::DisconnectTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStartTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStopTimeout { .. } => None,
            BoltV3LiveNodeError::DisconnectFailed(error)
            | BoltV3LiveNodeError::NoSubmitStartFailed(error)
            | BoltV3LiveNodeError::NoSubmitStopFailed(error) => Some(error.as_ref()),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    check_no_forbidden_credential_env_vars(&loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    // Per #252 design review: own the resolver session at the bolt-v3
    // startup boundary so a single AWS SDK config + SsmClient cache covers
    // every secret resolution in this build, and so the session lifetime is
    // visible to the caller of `resolve_bolt_v3_secrets`. Session-setup
    // failure surfaces as the dedicated `SecretResolverSetup` variant
    // (#255-2) so operator-facing messages don't pretend a venue or SSM
    // path is involved before any path has been read.
    let session = SsmResolverSession::new().map_err(BoltV3LiveNodeError::SecretResolverSetup)?;
    let resolved =
        resolve_bolt_v3_secrets(&session, loaded).map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let (runtime, _summary) = build_live_node_with_clients(loaded, &resolved, adapters)?;
    Ok(runtime)
}

/// Single bolt-v3 entrypoint for entering NT's runner loop.
///
/// The caller builds the `LiveNode` separately, then this function checks
/// the loaded config's `[live_canary]` section and referenced no-submit
/// readiness report before entering the NT runner loop. Production callers
/// must use this wrapper rather than invoking the NT runner method directly.
/// If the gate rejects, NT's runner loop is never entered.
pub async fn run_bolt_v3_live_node(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let gate_report = check_bolt_v3_live_canary_gate(loaded)
        .await
        .map_err(BoltV3LiveNodeError::LiveCanaryGate)?;
    runtime
        .submit_admission
        .arm(gate_report)
        .map_err(BoltV3LiveNodeError::SubmitAdmission)?;
    let node = &mut runtime.node;
    let node_handle = node.handle();
    let mut capture_guards = wire_bolt_v3_runtime_capture(node, node_handle, loaded)
        .map_err(BoltV3LiveNodeError::RuntimeCaptureWire)?;
    let mut capture_failure_receiver = capture_guards.take_failure_receiver();

    let run_result = await_live_node_run_after_capture_failure_notification(
        node.run(),
        capture_failure_receiver.as_mut(),
    )
    .await;
    let shutdown_result = capture_guards.shutdown().await;

    classify_live_node_run_and_capture_shutdown(run_result, shutdown_result)
}

async fn await_live_node_run_after_capture_failure_notification<F>(
    run_future: F,
    capture_failure_receiver: Option<&mut tokio::sync::oneshot::Receiver<()>>,
) -> Result<(), anyhow::Error>
where
    F: Future<Output = Result<(), anyhow::Error>>,
{
    tokio::pin!(run_future);

    if let Some(receiver) = capture_failure_receiver {
        tokio::select! {
            result = &mut run_future => result,
            notification = receiver => {
                if notification.is_ok() {
                    log::error!("NT runtime capture failure detected, awaiting LiveNode shutdown");
                }
                run_future.await
            }
        }
    } else {
        run_future.await
    }
}

pub async fn controlled_no_submit_readiness<F>(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    mut reference_readiness: F,
) -> (
    Result<(), BoltV3LiveNodeError>,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
)
where
    F: FnMut(&BoltV3LiveNodeRuntime) -> Result<(), String>,
{
    let start = start_bolt_v3_no_submit_readiness(&mut runtime.node, loaded).await;
    let reference = if start.is_ok() {
        bounded_no_submit_reference_readiness(runtime, loaded, &mut reference_readiness).await
    } else {
        Err("controlled start failed".to_string())
    };
    let stop = stop_bolt_v3_no_submit_readiness(&mut runtime.node, loaded).await;
    (start, reference, stop)
}

async fn bounded_no_submit_reference_readiness<F>(
    runtime: &BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    reference_readiness: &mut F,
) -> Result<(), String>
where
    F: FnMut(&BoltV3LiveNodeRuntime) -> Result<(), String>,
{
    let first_error = match reference_readiness(runtime) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let timeout_seconds = loaded.root.nautilus.timeout_connection_seconds;
    if timeout_seconds == 0 {
        return Err(first_error);
    }
    tokio::time::sleep(Duration::from_secs(timeout_seconds)).await;
    reference_readiness(runtime).map_err(|final_error| {
        if final_error == first_error {
            final_error
        } else {
            format!("{final_error}; initial reference-readiness error: {first_error}")
        }
    })
}

async fn start_bolt_v3_no_submit_readiness(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_seconds = no_submit_start_timeout_seconds(loaded);
    let start = node.start();
    match tokio::time::timeout(Duration::from_secs(timeout_seconds), start).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(BoltV3LiveNodeError::NoSubmitStartFailed(error)),
        Err(_) => Err(BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_seconds }),
    }
}

async fn stop_bolt_v3_no_submit_readiness(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_seconds = no_submit_stop_timeout_seconds(loaded);
    let stop = node.stop();
    match tokio::time::timeout(Duration::from_secs(timeout_seconds), stop).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
        Err(_) => Err(BoltV3LiveNodeError::NoSubmitStopTimeout { timeout_seconds }),
    }
}

fn no_submit_start_timeout_seconds(loaded: &LoadedBoltV3Config) -> u64 {
    loaded
        .root
        .nautilus
        .timeout_connection_seconds
        .saturating_add(loaded.root.nautilus.timeout_reconciliation_seconds)
        .saturating_add(loaded.root.nautilus.timeout_portfolio_seconds)
}

fn no_submit_stop_timeout_seconds(loaded: &LoadedBoltV3Config) -> u64 {
    loaded
        .root
        .nautilus
        .timeout_disconnection_seconds
        .saturating_add(loaded.root.nautilus.delay_post_stop_seconds)
        .saturating_add(loaded.root.nautilus.timeout_shutdown_seconds)
}

fn classify_live_node_run_and_capture_shutdown(
    run_result: Result<(), anyhow::Error>,
    shutdown_result: Result<(), anyhow::Error>,
) -> Result<(), BoltV3LiveNodeError> {
    match (run_result, shutdown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run_error), Ok(())) => Err(BoltV3LiveNodeError::Run(run_error)),
        (Ok(()), Err(shutdown_error)) => {
            Err(BoltV3LiveNodeError::RuntimeCaptureShutdown(shutdown_error))
        }
        (Err(run_error), Err(shutdown_error)) => {
            log::error!("Live node run error during NT runtime capture shutdown: {run_error}");
            Err(BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            })
        }
    }
}

/// Test-friendly variant of [`build_bolt_v3_live_node`] which lets the caller
/// inject the environment-variable predicate and the SSM resolver. Production
/// code must use [`build_bolt_v3_live_node`], which performs the real
/// forbidden credential environment-variable check and invokes the real Amazon
/// Web Services Systems Manager resolver.
pub fn build_bolt_v3_live_node_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let (runtime, _summary) = build_bolt_v3_live_node_with_summary(loaded, env_is_set, resolver)?;
    Ok(runtime)
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
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
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
    build_live_node_with_clients(loaded, &resolved, adapters)
}

fn build_live_node_with_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> = Arc::new(
        JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded).map_err(|error| {
            BoltV3LiveNodeError::StrategyRegistration(BoltV3StrategyRegistrationError::Evidence {
                message: error.to_string(),
            })
        })?,
    );
    let submit_admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(
        decision_evidence.clone(),
    ));
    let builder =
        make_bolt_v3_live_node_builder(loaded).map_err(BoltV3LiveNodeError::BuilderConstruction)?;
    let (builder, summary) = register_bolt_v3_clients(builder, adapters)
        .map_err(BoltV3LiveNodeError::ClientRegistration)?;
    let mut node = builder.build().map_err(BoltV3LiveNodeError::Build)?;
    let strategy_summary = register_bolt_v3_strategies_on_node_with_bindings(
        &mut node,
        loaded,
        resolved,
        crate::bolt_v3_archetypes::runtime_bindings(),
        submit_admission.clone(),
        decision_evidence.clone(),
    )
    .map_err(BoltV3LiveNodeError::StrategyRegistration)?;
    for strategy in &strategy_summary.registered {
        log::info!(
            "bolt-v3 registered strategy: strategy_instance_id={} strategy_archetype={} nt_strategy_id={}",
            strategy.strategy_instance_id,
            strategy.strategy_archetype.as_str(),
            strategy.registered_strategy_id
        );
    }
    Ok((
        BoltV3LiveNodeRuntime::new(node, submit_admission, resolved.redaction_values()),
        summary,
    ))
}

/// Translates a validated bolt-v3 config into an NT-native
/// [`LiveNodeBuilder`] with no clients added. Field translation goes
/// through [`make_live_node_config`] so the bolt-v3 → NT field mapping
/// has a single source of truth that the existing per-field tests can
/// keep exercising.
pub fn make_bolt_v3_live_node_builder(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    let cfg = make_live_node_config(loaded)?;
    make_bolt_v3_live_node_builder_from_config(cfg)
}

fn make_bolt_v3_live_node_builder_from_config(
    cfg: LiveNodeConfig,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    LiveNodeBuilder::from_config(cfg)
        .map_err(|source| BoltV3LiveNodeBuilderError::BuilderConstruction { source })
}

pub fn make_live_node_config(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNodeConfig, BoltV3LiveNodeBuilderError> {
    let trader_id = TraderId::from(loaded.root.trader_id.as_str());
    let environment = match loaded.root.runtime.mode {
        RuntimeMode::Backtest => Environment::Backtest,
        RuntimeMode::Sandbox => Environment::Sandbox,
        RuntimeMode::Live => Environment::Live,
    };
    let component_level = logger_levels(&loaded.root.logging.component_levels);
    let mut module_level = logger_levels(&loaded.root.logging.module_levels);
    for module_path in bolt_v3_providers::credential_log_modules() {
        module_level.insert(
            Ustr::from(module_path),
            loaded
                .root
                .logging
                .credential_module_level
                .to_level_filter(),
        );
    }
    let file_config =
        optional_file_writer_config("logging.file_config", &loaded.root.logging.file_config)?;
    let logging = LoggerConfig {
        stdout_level: loaded.root.logging.standard_output_level.to_level_filter(),
        fileout_level: loaded.root.logging.file_level.to_level_filter(),
        component_level,
        module_level,
        log_components_only: loaded.root.logging.log_components_only,
        is_colored: loaded.root.logging.is_colored,
        print_config: loaded.root.logging.print_config,
        use_tracing: loaded.root.logging.use_tracing,
        bypass_logging: loaded.root.logging.bypass_logging,
        file_config,
        clear_log_file: loaded.root.logging.clear_log_file,
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
        time_bars_origins: data.time_bars_origins.clone().into_iter().collect(),
        validate_data_sequence: data.validate_data_sequence,
        buffer_deltas: data.buffer_deltas,
        emit_quotes_from_book: data.emit_quotes_from_book,
        emit_quotes_from_book_depths: data.emit_quotes_from_book_depths,
        external_clients: strings_as_client_ids(&data.external_client_ids),
        debug: data.debug,
        graceful_shutdown_on_error: data.graceful_shutdown_on_error,
        qsize: data.qsize,
    };
    let exec = &nautilus.exec_engine;
    let exec_engine = nautilus_live::config::LiveExecEngineConfig {
        load_cache: exec.load_cache,
        snapshot_orders: exec.snapshot_orders,
        snapshot_positions: exec.snapshot_positions,
        snapshot_positions_interval_secs: zero_u64_as_f64_option(
            exec.snapshot_positions_interval_seconds,
        ),
        external_clients: strings_as_client_ids(&exec.external_client_ids),
        debug: exec.debug,
        reconciliation: exec.reconciliation,
        reconciliation_lookback_mins: zero_u32_as_option(exec.reconciliation_lookback_mins),
        // `f64` is lossless for all practical delay values (< 2^53 seconds).
        reconciliation_startup_delay_secs: exec.reconciliation_startup_delay_seconds as f64,
        reconciliation_instrument_ids: non_empty_strings(&exec.reconciliation_instrument_ids),
        filter_unclaimed_external_orders: exec.filter_unclaimed_external_orders,
        filter_position_reports: exec.filter_position_reports,
        filtered_client_order_ids: non_empty_strings(&exec.filtered_client_order_ids),
        generate_missing_orders: exec.generate_missing_orders,
        inflight_check_interval_ms: exec.inflight_check_interval_milliseconds,
        inflight_check_threshold_ms: exec.inflight_check_threshold_milliseconds,
        inflight_check_retries: exec.inflight_check_retries,
        open_check_interval_secs: zero_u64_as_f64_option(exec.open_check_interval_seconds),
        open_check_lookback_mins: zero_u32_as_option(exec.open_check_lookback_mins),
        open_check_threshold_ms: exec.open_check_threshold_milliseconds,
        open_check_missing_retries: exec.open_check_missing_retries,
        open_check_open_only: exec.open_check_open_only,
        max_single_order_queries_per_cycle: exec.max_single_order_queries_per_cycle,
        single_order_query_delay_ms: exec.single_order_query_delay_milliseconds,
        position_check_interval_secs: zero_u64_as_f64_option(exec.position_check_interval_seconds),
        position_check_lookback_mins: exec.position_check_lookback_mins,
        position_check_threshold_ms: exec.position_check_threshold_milliseconds,
        position_check_retries: exec.position_check_retries,
        purge_closed_orders_interval_mins: zero_u32_as_option(
            exec.purge_closed_orders_interval_mins,
        ),
        purge_closed_orders_buffer_mins: zero_u32_as_option(exec.purge_closed_orders_buffer_mins),
        purge_closed_positions_interval_mins: zero_u32_as_option(
            exec.purge_closed_positions_interval_mins,
        ),
        purge_closed_positions_buffer_mins: zero_u32_as_option(
            exec.purge_closed_positions_buffer_mins,
        ),
        purge_account_events_interval_mins: zero_u32_as_option(
            exec.purge_account_events_interval_mins,
        ),
        purge_account_events_lookback_mins: zero_u32_as_option(
            exec.purge_account_events_lookback_mins,
        ),
        purge_from_database: exec.purge_from_database,
        own_books_audit_interval_secs: zero_u64_as_f64_option(
            exec.own_books_audit_interval_seconds,
        ),
        graceful_shutdown_on_error: exec.graceful_shutdown_on_error,
        qsize: exec.qsize,
        allow_overfills: exec.allow_overfills,
        manage_own_order_books: exec.manage_own_order_books,
    };
    let risk_engine = nautilus_live::config::LiveRiskEngineConfig {
        bypass: loaded.root.risk.nt_bypass,
        max_order_submit_rate: loaded.root.risk.nt_max_order_submit_rate.clone(),
        max_order_modify_rate: loaded.root.risk.nt_max_order_modify_rate.clone(),
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live risk config consumes the same string pairs as a HashMap.
        max_notional_per_order: loaded
            .root
            .risk
            .nt_max_notional_per_order
            .clone()
            .into_iter()
            .collect(),
        debug: loaded.root.risk.nt_debug,
        graceful_shutdown_on_error: loaded.root.risk.nt_graceful_shutdown_on_error,
        qsize: loaded.root.risk.nt_qsize,
    };

    // Explicit struct literal: upstream NT `LiveNodeConfig` field additions must be
    // considered here instead of silently inherited through `Default`.
    Ok(LiveNodeConfig {
        environment,
        trader_id,
        load_state: nautilus.load_state,
        save_state: nautilus.save_state,
        logging,
        instance_id: optional_instance_id("nautilus.instance_id", &nautilus.instance_id)?,
        timeout_connection: Duration::from_secs(nautilus.timeout_connection_seconds),
        timeout_reconciliation: Duration::from_secs(nautilus.timeout_reconciliation_seconds),
        timeout_portfolio: Duration::from_secs(nautilus.timeout_portfolio_seconds),
        timeout_disconnection: Duration::from_secs(nautilus.timeout_disconnection_seconds),
        delay_post_stop: Duration::from_secs(nautilus.delay_post_stop_seconds),
        timeout_shutdown: Duration::from_secs(nautilus.timeout_shutdown_seconds),
        cache: optional_toml_config("nautilus.cache", &nautilus.cache)?,
        msgbus: optional_toml_config("nautilus.msgbus", &nautilus.msgbus)?,
        portfolio: optional_toml_config("nautilus.portfolio", &nautilus.portfolio)?,
        emulator: optional_toml_config("nautilus.emulator", &nautilus.emulator)?,
        streaming: optional_streaming_config("nautilus.streaming", &nautilus.streaming)?,
        loop_debug: nautilus.loop_debug,
        data_engine,
        risk_engine,
        exec_engine,
        data_clients: HashMap::new(),
        exec_clients: HashMap::new(),
    })
}

fn optional_toml_config<T>(
    field: &'static str,
    config: &NautilusComponentConfig,
) -> Result<Option<T>, BoltV3LiveNodeBuilderError>
where
    T: DeserializeOwned,
{
    if disabled_component(config) {
        return Ok(None);
    }
    config
        .clone()
        .try_into()
        .map(Some)
        .map_err(|source| config_mapping_error(field, source))
}

fn optional_instance_id(
    field: &'static str,
    config: &NautilusComponentConfig,
) -> Result<Option<UUID4>, BoltV3LiveNodeBuilderError> {
    if disabled_component(config) {
        return Ok(None);
    }
    let Some(value) = config.as_str() else {
        return Err(BoltV3LiveNodeBuilderError::ConfigMapping {
            field,
            message: "expected UUID string or \"disabled\"".to_string(),
        });
    };
    UUID4::from_str(value)
        .map(Some)
        .map_err(|source| BoltV3LiveNodeBuilderError::ConfigMapping {
            field,
            message: source,
        })
}

fn optional_file_writer_config(
    field: &'static str,
    config: &NautilusComponentConfig,
) -> Result<Option<FileWriterConfig>, BoltV3LiveNodeBuilderError> {
    optional_toml_config::<BoltV3FileWriterConfig>(field, config)
        .map(|value| value.map(FileWriterConfig::from))
}

fn optional_streaming_config(
    field: &'static str,
    config: &NautilusComponentConfig,
) -> Result<Option<StreamingConfig>, BoltV3LiveNodeBuilderError> {
    optional_toml_config::<BoltV3StreamingConfig>(field, config)
        .map(|value| value.map(StreamingConfig::from))
}

fn disabled_component(config: &NautilusComponentConfig) -> bool {
    if config.as_str() == Some("disabled") {
        return true;
    }
    false
}

fn config_mapping_error(
    field: &'static str,
    source: toml::de::Error,
) -> BoltV3LiveNodeBuilderError {
    BoltV3LiveNodeBuilderError::ConfigMapping {
        field,
        message: source.to_string(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3FileWriterConfig {
    directory: Option<String>,
    file_name: Option<String>,
    file_format: Option<String>,
    file_rotate: Option<BoltV3FileRotateConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3FileRotateConfig {
    max_file_size: u64,
    max_backup_count: u32,
}

impl From<BoltV3FileWriterConfig> for FileWriterConfig {
    fn from(config: BoltV3FileWriterConfig) -> Self {
        FileWriterConfig::new(
            config.directory,
            config.file_name,
            config.file_format,
            config
                .file_rotate
                .map(|rotate| (rotate.max_file_size, rotate.max_backup_count)),
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3StreamingConfig {
    catalog_path: String,
    fs_protocol: String,
    flush_interval_ms: u64,
    replace_existing: bool,
    rotation_config: BoltV3RotationConfig,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BoltV3RotationConfig {
    Size { max_size: u64 },
    Interval { interval_ns: u64 },
    ScheduledDates { interval_ns: u64, schedule_ns: u64 },
    NoRotation,
}

impl From<BoltV3StreamingConfig> for StreamingConfig {
    fn from(config: BoltV3StreamingConfig) -> Self {
        StreamingConfig::new(
            config.catalog_path,
            config.fs_protocol,
            config.flush_interval_ms,
            config.replace_existing,
            RotationConfig::from(config.rotation_config),
        )
    }
}

impl From<BoltV3RotationConfig> for RotationConfig {
    fn from(config: BoltV3RotationConfig) -> Self {
        match config {
            BoltV3RotationConfig::Size { max_size } => RotationConfig::Size { max_size },
            BoltV3RotationConfig::Interval { interval_ns } => {
                RotationConfig::Interval { interval_ns }
            }
            BoltV3RotationConfig::ScheduledDates {
                interval_ns,
                schedule_ns,
            } => RotationConfig::ScheduledDates {
                interval_ns,
                schedule_ns: UnixNanos::from(schedule_ns),
            },
            BoltV3RotationConfig::NoRotation => RotationConfig::NoRotation,
        }
    }
}

fn logger_levels(
    levels: &std::collections::BTreeMap<String, LogLevel>,
) -> AHashMap<Ustr, LevelFilter> {
    levels
        .iter()
        .map(|(target, level)| (Ustr::from(target.as_str()), level.to_level_filter()))
        .collect()
}

fn non_empty_strings(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

fn zero_u32_as_option(value: u32) -> Option<u32> {
    std::num::NonZeroU32::new(value).map(std::num::NonZeroU32::get)
}

fn zero_u64_as_f64_option(value: u64) -> Option<f64> {
    std::num::NonZeroU64::new(value).map(|non_zero| non_zero.get() as f64)
}

/// Caller must run root validation first so the string is a valid NT `BarIntervalType`.
fn bar_interval_type_from_str(value: &str) -> BarIntervalType {
    BarIntervalType::from_str(value).expect("root validation must accept data bar interval type")
}

/// Caller must run root validation first so every value is a valid NT `ClientId`.
fn strings_as_client_ids(values: &[String]) -> Option<Vec<ClientId>> {
    (!values.is_empty()).then(|| values.iter().map(ClientId::new).collect())
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
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_milliseconds,
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
/// provider-owned credential log module filters remain active during
/// connect.
/// Production callers preserve that ordering.
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
/// instrument-availability checks; that readiness is checked by
/// [`controlled_no_submit_readiness`].
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

    fn toml_table(input: &str) -> toml::Value {
        toml::from_str(input).expect("test TOML component table should parse")
    }

    #[test]
    fn live_node_config_maps_trader_id_and_environment_from_v3_root() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

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
    fn live_node_config_maps_configured_nt_component_blocks() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.instance_id =
            toml::Value::String("2d89666b-1a1e-4a75-b193-4eb3b454c757".to_string());
        loaded.root.nautilus.cache = toml_table(
            r#"
            use_trader_prefix = false
            tick_capacity = 321
            bar_capacity = 654
            "#,
        );
        loaded.root.nautilus.msgbus = toml_table(
            r#"
            streams_prefix = "bolt-stream"
            stream_per_topic = false
            "#,
        );
        loaded.root.nautilus.portfolio = toml_table(
            r#"
            use_mark_prices = true
            bar_updates = false
            "#,
        );
        loaded.root.nautilus.emulator = toml_table(
            r#"
            debug = true
            "#,
        );
        loaded.root.nautilus.streaming = toml_table(
            r#"
            catalog_path = "/tmp/bolt-v3-catalog"
            fs_protocol = "file"
            flush_interval_ms = 250
            replace_existing = true

            [rotation_config]
            kind = "size"
            max_size = 2048
            "#,
        );
        loaded.root.logging.file_config = toml_table(
            r#"
            directory = "logs"
            file_name = "bolt-v3.log"
            file_format = "json"

            [file_rotate]
            max_file_size = 4096
            max_backup_count = 3
            "#,
        );

        let cfg = make_live_node_config(&loaded).expect("configured NT components should map");

        assert_eq!(
            cfg.instance_id
                .expect("instance id should be configured")
                .to_string(),
            "2d89666b-1a1e-4a75-b193-4eb3b454c757"
        );
        let cache = cfg.cache.expect("cache config should map");
        assert!(!cache.use_trader_prefix);
        assert_eq!(cache.tick_capacity, 321);
        assert_eq!(cache.bar_capacity, 654);
        let msgbus = cfg.msgbus.expect("msgbus config should map");
        assert_eq!(msgbus.streams_prefix, "bolt-stream");
        assert!(!msgbus.stream_per_topic);
        let portfolio = cfg.portfolio.expect("portfolio config should map");
        assert!(portfolio.use_mark_prices);
        assert!(!portfolio.bar_updates);
        assert!(cfg.emulator.expect("emulator config should map").debug);
        let streaming = cfg.streaming.expect("streaming config should map");
        assert_eq!(streaming.catalog_path, "/tmp/bolt-v3-catalog");
        match streaming.rotation_config {
            RotationConfig::Size { max_size } => assert_eq!(max_size, 2048),
            other => panic!("expected size rotation, got {other:?}"),
        }
        let file_config = cfg
            .logging
            .file_config
            .expect("logger file config should map");
        assert_eq!(file_config.directory.as_deref(), Some("logs"));
        assert_eq!(file_config.file_name.as_deref(), Some("bolt-v3.log"));
        assert_eq!(file_config.file_format.as_deref(), Some("json"));
        assert_eq!(
            file_config
                .file_rotate
                .expect("file rotation should map")
                .max_file_size,
            4096
        );
    }

    #[test]
    fn live_node_config_reports_field_for_invalid_nt_component_block() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.cache = toml::Value::String("enabled".to_string());

        let error = make_live_node_config(&loaded)
            .expect_err("invalid cache component should produce a mapping error");
        match error {
            BoltV3LiveNodeBuilderError::ConfigMapping { field, message } => {
                assert_eq!(field, "nautilus.cache");
                assert!(
                    message.contains("invalid type") || message.contains("expected"),
                    "mapping error should explain the parse failure: {message}"
                );
            }
            other => panic!("expected config mapping error, got {other:?}"),
        }
    }

    #[test]
    fn live_node_builder_rejects_backtest_environment_before_registration() {
        let loaded = fixture_loaded_config();
        let make_error = || {
            let mut cfg =
                make_live_node_config(&loaded).expect("fixture live-node config should map");
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

        let source = match make_error() {
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => source,
            BoltV3LiveNodeBuilderError::ConfigMapping { field, message } => {
                panic!(
                    "expected builder construction failure, got config mapping failure for {field}: {message}"
                )
            }
        };
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

    #[tokio::test]
    async fn capture_failure_notification_waits_for_active_live_node_run_result() {
        let (notify_failure, mut failure_receiver) = tokio::sync::oneshot::channel();
        let (finish_run, run_finished) = tokio::sync::oneshot::channel();
        let run_future = async move {
            run_finished
                .await
                .expect("test should finish the simulated LiveNode run")
        };
        let mut pending = Box::pin(await_live_node_run_after_capture_failure_notification(
            run_future,
            Some(&mut failure_receiver),
        ));

        notify_failure
            .send(())
            .expect("capture-failure receiver should still be live");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut pending)
                .await
                .is_err(),
            "capture-failure notification must not drop the active LiveNode run future"
        );

        finish_run
            .send(Err(anyhow::anyhow!("runner stopped after capture failure")))
            .expect("simulated LiveNode run should still be awaited");
        let error = pending
            .await
            .expect_err("runner error after capture failure must be preserved");
        assert_eq!(error.to_string(), "runner stopped after capture failure");
    }

    #[test]
    fn live_node_config_top_level_residuals_are_disabled_or_empty() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

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
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");
        assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_runtime_defaults_from_v3_root() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

        assert!(cfg.data_engine.time_bars_build_with_no_updates);
        assert!(cfg.data_engine.time_bars_timestamp_on_close);
        assert!(!cfg.data_engine.time_bars_skip_first_non_full_bar);
        assert_eq!(
            cfg.data_engine.time_bars_interval_type,
            nautilus_model::enums::BarIntervalType::LeftOpen
        );
        assert_eq!(cfg.data_engine.time_bars_build_delay, 0);
        assert!(cfg.data_engine.time_bars_origins.is_empty());
        assert!(!cfg.data_engine.validate_data_sequence);
        assert!(!cfg.data_engine.buffer_deltas);
        assert!(!cfg.data_engine.emit_quotes_from_book);
        assert!(!cfg.data_engine.emit_quotes_from_book_depths);
        assert_eq!(cfg.data_engine.external_clients, None);
        assert!(!cfg.data_engine.debug);
        assert!(!cfg.data_engine.graceful_shutdown_on_error);
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
        assert!(!cfg.exec_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.exec_engine.qsize, 100_000);
        assert!(!cfg.exec_engine.allow_overfills);
        assert!(!cfg.exec_engine.manage_own_order_books);
        assert!(!cfg.risk_engine.bypass);
        assert_eq!(cfg.risk_engine.max_order_submit_rate, "100/00:00:01");
        assert_eq!(cfg.risk_engine.max_order_modify_rate, "100/00:00:01");
        assert!(cfg.risk_engine.max_notional_per_order.is_empty());
        assert!(!cfg.risk_engine.debug);
        assert!(!cfg.risk_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.risk_engine.qsize, 100_000);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_risk_debug_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.risk.nt_debug = true;

        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

        assert!(cfg.risk_engine.debug);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_risk_bypass_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.risk.nt_bypass = true;

        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

        assert!(cfg.risk_engine.bypass);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_data_engine_debug_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.data_engine.debug = true;

        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

        assert!(cfg.data_engine.debug);
    }

    #[test]
    fn live_node_config_maps_non_empty_nt_max_notional_per_order() {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .risk
            .nt_max_notional_per_order
            .insert("ETHUSDT.BINANCE".to_string(), "12345.00".to_string());
        loaded
            .root
            .risk
            .nt_max_notional_per_order
            .insert("BTCUSDT.BINANCE".to_string(), "25000.50".to_string());
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("ETHUSDT.BINANCE"),
            Some(&"12345.00".to_string())
        );
        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("BTCUSDT.BINANCE"),
            Some(&"25000.50".to_string())
        );
    }

    #[test]
    fn live_node_config_maps_log_levels_from_uppercase_strings() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");
        assert_eq!(cfg.logging.stdout_level, log::LevelFilter::Info);
        assert_eq!(cfg.logging.fileout_level, log::LevelFilter::Info);
    }

    #[test]
    fn live_node_config_logger_literal_does_not_inherit_nt_defaults() {
        let src = include_str!("bolt_v3_live_node.rs");
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
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .logging
            .component_levels
            .insert("RiskEngine".to_string(), LogLevel::Error);
        loaded
            .root
            .logging
            .module_levels
            .insert("nautilus_live::node".to_string(), LogLevel::Debug);
        loaded.root.logging.credential_module_level = LogLevel::Error;
        loaded.root.logging.log_components_only = true;
        loaded.root.logging.is_colored = false;
        loaded.root.logging.print_config = true;
        loaded.root.logging.use_tracing = true;
        loaded.root.logging.bypass_logging = true;
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

        assert_eq!(
            cfg.logging
                .component_level
                .get(&Ustr::from("RiskEngine"))
                .copied(),
            Some(log::LevelFilter::Error)
        );
        assert_eq!(
            cfg.logging
                .module_level
                .get(&Ustr::from("nautilus_live::node"))
                .copied(),
            Some(log::LevelFilter::Debug)
        );
        assert!(cfg.logging.log_components_only);
        assert!(!cfg.logging.is_colored);
        assert!(cfg.logging.print_config);
        assert!(cfg.logging.use_tracing);
        assert!(cfg.logging.bypass_logging);
        assert!(cfg.logging.file_config.is_none());
        assert!(!cfg.logging.clear_log_file);
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
        let cfg = make_live_node_config(&loaded).expect("fixture live-node config should map");

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
    fn secret_resolver_setup_variant_renders_clean_message_without_empty_venue_path() {
        // Per #255-2: before this fix, session-construction failure was
        // mapped into `BoltV3SecretError` with empty `venue_key` and
        // `ssm_path`, rendering as a confusing
        // `venues..secrets.ssm_resolver_session ...`. The dedicated
        // `BoltV3LiveNodeError::SecretResolverSetup(SecretError)` variant
        // gives operators a clean, accurate message that does not
        // pretend a venue or SSM path is involved (none is — the
        // failure happens before any path is read).
        let inner = crate::secrets::SecretError::for_test(
            "failed to build Tokio runtime for SSM resolver session: simulated".to_string(),
        );
        let err = BoltV3LiveNodeError::SecretResolverSetup(inner);
        let rendered = format!("{err}");
        assert!(
            !rendered.contains("venues."),
            "SecretResolverSetup must not render through the venue/SSM-path template"
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
}
