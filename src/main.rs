use clap::Parser;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::{
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
};

use bolt_v2::{
    bolt_v3_atomic_io::{RUNTIME_CONFIG_FILE_MODE, write_atomic_file_with_mode},
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_deploy_target::{
        DeployTargetError, HostFactsSource, Imdsv2HostFactsSource, ObservedHostFacts,
        TargetVerifyOutcome, load_deploy_target, verify_deploy_target,
    },
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_kill_switch_store::{KillSwitchRecoveryState, KillSwitchStore},
    bolt_v3_live_node::{
        BoltV3LiveNodeRuntime, build_bolt_v3_live_node_with_resolved,
        build_bolt_v3_strategy_free_data_client_probe_live_node, current_build_head_sha,
        run_bolt_v3_data_client_census, run_bolt_v3_data_client_probe, run_bolt_v3_live_node,
    },
    bolt_v3_loss_governor_manual_recovery_ops::{
        LossGovernorManualRecoveryCommand, recover_loss_governor_manual_halt,
    },
    bolt_v3_operator_artifacts::{
        LaunchIdentity, WrittenOperatorArtifact, is_lowercase_git_sha, read_launch_identity,
        write_launch_identity,
    },
    bolt_v3_operator_health::{
        BoltV3InputHealth, BoltV3OperatorHealthSurface, BoltV3RejectObserverHealth,
        BoltV3VenueTruthHealth,
    },
    bolt_v3_prod_profile::{
        GENERATOR_FORMAT_VERSION, ProductionInvariants, generate_live_config, live_config_path,
        verify_live_config,
    },
    bolt_v3_providers::{
        ClobV2BalanceAllowanceCacheSync, ClobV2BalanceAllowanceCacheSyncRequest,
        ProviderArtifactReference, ProviderLiveSubmitApprovalContext,
        ProviderProductSubmitProofArtifactRequest, binding_for_provider_key,
        reference_boundary_capture::{
            BoundaryFixtureCaptureProvenance, BoundaryFixtureCaptureRequest,
            capture_reference_boundary_fixture,
        },
        reference_live_probe::run_reference_live_probe,
        sync_clob_v2_balance_allowance_cache_from_configured_account,
    },
    bolt_v3_reference_price_health::{
        ReferenceCurrentPriceHealthReport, ReferenceCurrentPriceHealthRun,
        prepare_reference_current_price_health_run,
        prepare_reference_current_price_health_run_with_resolved,
        run_prepared_reference_current_price_health,
    },
    bolt_v3_secrets::{
        ResolvedBoltV3Secrets, check_no_forbidden_credential_env_vars,
        resolve_bolt_v3_client_secrets, resolve_bolt_v3_secrets,
    },
    secrets::SsmResolverSession,
};

#[cfg(test)]
use bolt_v2::bolt_v3_reference_price_health::ReferenceCurrentPriceSourceUpdateObservation;

const CLOB_V2_CACHE_SYNC_COMPLETED_OUTPUT_FIELD: &str =
    "clob_v2_balance_allowance_cache_sync_completed";
const CLOB_V2_CACHE_SYNC_EXECUTION_CLIENT_OUTPUT_FIELD: &str = "execution_client_id";
const CLOB_V2_CACHE_SYNC_REQUEST_PATH_OUTPUT_FIELD: &str = "request_path";
const CLOB_V2_CACHE_SYNC_BASE_URL_HTTP_SHA256_OUTPUT_FIELD: &str = "base_url_http_sha256";
const KILL_SWITCH_STORE_INIT_COMPLETED_OUTPUT_FIELD: &str = "kill_switch_store_init_completed";
const KILL_SWITCH_STORE_INIT_STATE_PATH_OUTPUT_FIELD: &str = "state_path";
const OPS_STATUS_KILL_SWITCH_STORE_FAIL_CLOSED_HALT_ID: &str =
    "ops-status-kill-switch-store-fail-closed";
const OPS_STATUS_KILL_SWITCH_STORE_UNREADABLE_HALT_ID: &str =
    "ops-status-kill-switch-store-unreadable";
const OPS_STATUS_KILL_SWITCH_STORE_FAIL_CLOSED_REASON_PREFIX: &str =
    "kill-switch store fail-closed";
const LOSS_GOVERNOR_MANUAL_RECOVERY_COMPLETED_OUTPUT_FIELD: &str =
    "loss_governor_manual_recovery_completed";
const LOSS_GOVERNOR_MANUAL_RECOVERY_STATE_PATH_OUTPUT_FIELD: &str = "state_path";
const LOSS_GOVERNOR_MANUAL_RECOVERY_PREVIOUS_STATE_OUTPUT_FIELD: &str = "previous_state";
const LOSS_GOVERNOR_MANUAL_RECOVERY_RECOVERED_STATE_OUTPUT_FIELD: &str = "recovered_state";
const LOSS_GOVERNOR_MANUAL_RECOVERY_COUNT_OUTPUT_FIELD: &str = "manual_recovery_count";
const REFERENCE_CURRENT_PRICE_HEALTH_UNOBSERVED_ERROR: &str =
    "reference_current_price health did not observe every configured source";

#[derive(Parser)]
#[command(name = "bolt-v2")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Run {
        #[arg(short, long)]
        config: PathBuf,
    },
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
    Ops {
        #[command(subcommand)]
        command: Box<OpsCommand>,
    },
    ProviderArtifacts {
        #[command(subcommand)]
        command: Box<ProviderArtifactsCommand>,
    },
}

#[derive(clap::Subcommand)]
enum SecretsCommand {
    Check {
        #[arg(short, long)]
        config: PathBuf,
    },
    Resolve {
        #[arg(short, long)]
        config: PathBuf,
    },
}

#[derive(clap::Subcommand)]
enum OpsCommand {
    Launch {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        config_root: PathBuf,
    },
    PrestartCheck {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        required_catalog_prefix: Option<PathBuf>,
    },
    DataClientProbe {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
    },
    DataClientCensus {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
    },
    GenerateLiveConfig {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        config_root: PathBuf,
    },
    VerifyLiveConfig {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        config_root: PathBuf,
    },
    Status {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        config_root: PathBuf,
        #[arg(long)]
        intended_sha: Option<String>,
    },
    InitKillSwitchStore {
        #[arg(short, long)]
        config: PathBuf,
    },
    #[command(
        about = "Recover only loss-governor halts whose triggering condition has verifiably passed by clock - daily UTC day rolled or rolling window elapsed. Rolling recovery uses the CURRENT config value: more than the full window must have elapsed; exact-equality refuses. every limit is re-checked live at next node start. node must be stopped because state is last-writer-wins; not an operator override. FailedManualIntervention is terminal for this command and needs out-of-band repair. The audit file is unbounded append-only and operators rotate it externally; last audit record per attempt is authoritative. evidence_path/evidence_sha256 are operator-attested audit metadata, not file or hash verification."
    )]
    LossGovernorManualRecovery {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        operator_id: String,
        #[arg(
            long,
            help = "Operator-attested audit metadata path; the file is not opened by this command"
        )]
        evidence_path: String,
        #[arg(
            long,
            help = "Operator-attested audit metadata digest; the file is not hash-verified by this command"
        )]
        evidence_sha256: String,
        #[arg(long)]
        observed_at_ns: u64,
    },
    ReferenceLiveProbe {
        #[arg(short, long)]
        config: PathBuf,
    },
    ReferenceCurrentPriceHealth {
        #[arg(short, long)]
        config: PathBuf,
    },
    CaptureReferenceBoundaryFixture {
        #[command(flatten)]
        args: Box<CaptureReferenceBoundaryFixtureArgs>,
    },
}

#[derive(clap::Args)]
struct CaptureReferenceBoundaryFixtureArgs {
    #[arg(long = "root-config")]
    root_config: PathBuf,
    #[arg(long)]
    client_key: String,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long)]
    wait_timeout_secs: u64,
    #[arg(long)]
    repository: String,
    #[arg(long)]
    workflow_path: String,
    #[arg(long)]
    workflow_digest: String,
    #[arg(long)]
    provenance_config_digest: String,
    #[arg(long)]
    head_sha: String,
    #[arg(long)]
    head_branch: String,
    #[arg(long)]
    run_id: u64,
    #[arg(long)]
    run_attempt: u64,
    #[arg(long)]
    check_suite_id: u64,
    #[arg(long)]
    event: String,
    #[arg(long)]
    created_at: String,
}

#[derive(clap::Subcommand)]
enum ProviderArtifactsCommand {
    GenerateLiveSubmitApproval {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        product_surface: String,
        #[arg(long)]
        expires_at_unix_seconds: u64,
    },
    PreflightLiveSubmitArming {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        product_surface: String,
    },
    GenerateProductSubmitProof(Box<GenerateProductSubmitProofArgs>),
    SyncClobV2BalanceAllowanceCache {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        acknowledge_clob_cache_mutation: bool,
    },
}

#[derive(clap::Args)]
struct GenerateProductSubmitProofArgs {
    #[arg(long)]
    provider_key: String,
    #[arg(long)]
    provider_id: String,
    #[arg(long)]
    product_surface: String,
    #[arg(long)]
    toml_checksum: String,
    #[arg(long)]
    order_proof_artifact_path: String,
    #[arg(long)]
    order_proof_artifact_sha256: String,
    #[arg(long)]
    fill_proof_artifact_path: String,
    #[arg(long)]
    fill_proof_artifact_sha256: String,
    #[arg(long)]
    rounding_proof_artifact_path: String,
    #[arg(long)]
    rounding_proof_artifact_sha256: String,
    #[arg(long)]
    fee_proof_artifact_path: String,
    #[arg(long)]
    fee_proof_artifact_sha256: String,
    #[arg(long)]
    settlement_proof_artifact_path: Option<String>,
    #[arg(long)]
    settlement_proof_artifact_sha256: Option<String>,
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { config } => run_live_node(config),
        Command::Secrets { command } => run_secrets_command(command),
        Command::Ops { command } => run_ops_command(*command),
        Command::ProviderArtifacts { command } => run_provider_artifacts_command(*command),
    }
}

fn run_live_node(config: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    Err(format!(
        "plain `bolt-v2 run --config {}` is disabled for live arming; use \
         `bolt-v2 ops launch --profile <profile-id> --config-root <config-root>`",
        config.display()
    )
    .into())
}

fn start_loaded_node_with_resolved(
    loaded: LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<(), Box<dyn std::error::Error>> {
    let node = build_bolt_v3_live_node_with_resolved(&loaded, resolved)?;
    run_built_node(node, loaded)
}

fn run_built_node(
    mut node: BoltV3LiveNodeRuntime,
    loaded: LoadedBoltV3Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    let app = async move {
        run_bolt_v3_live_node(&mut node, &loaded).await?;
        Ok(())
    };
    runtime.block_on(local.run_until(app))
}

fn run_ops_command(command: OpsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        OpsCommand::Launch {
            profile,
            config_root,
        } => run_ops_launch(profile, config_root),
        OpsCommand::PrestartCheck {
            config,
            required_catalog_prefix,
        } => run_prestart_check(&config, required_catalog_prefix.as_deref()),
        OpsCommand::DataClientProbe { config, client_key } => {
            run_data_client_probe(&config, &client_key)
        }
        OpsCommand::DataClientCensus { config, client_key } => {
            run_data_client_census(&config, &client_key)
        }
        OpsCommand::GenerateLiveConfig {
            profile,
            config_root,
        } => run_generate_live_config(&config_root, &profile),
        OpsCommand::VerifyLiveConfig {
            profile,
            config_root,
        } => run_verify_live_config(&config_root, &profile),
        OpsCommand::Status {
            profile,
            config_root,
            intended_sha,
        } => run_ops_status(&config_root, &profile, intended_sha.as_deref()),
        OpsCommand::InitKillSwitchStore { config } => run_init_kill_switch_store(&config),
        OpsCommand::LossGovernorManualRecovery {
            config,
            operator_id,
            evidence_path,
            evidence_sha256,
            observed_at_ns,
        } => run_loss_governor_manual_recovery(
            &config,
            operator_id,
            evidence_path,
            evidence_sha256,
            observed_at_ns,
        ),
        OpsCommand::ReferenceLiveProbe { config } => run_reference_live_probe_command(&config),
        OpsCommand::ReferenceCurrentPriceHealth { config } => {
            run_reference_current_price_health_command(&config)
        }
        OpsCommand::CaptureReferenceBoundaryFixture { args } => {
            let CaptureReferenceBoundaryFixtureArgs {
                root_config,
                client_key,
                output_dir,
                wait_timeout_secs,
                repository,
                workflow_path,
                workflow_digest,
                provenance_config_digest,
                head_sha,
                head_branch,
                run_id,
                run_attempt,
                check_suite_id,
                event,
                created_at,
            } = *args;
            run_capture_chainlink_reference_fixture_command(
                &root_config,
                BoundaryFixtureCaptureRequest {
                    client_key,
                    output_dir,
                    wait_timeout: std::time::Duration::from_secs(wait_timeout_secs),
                    provenance: BoundaryFixtureCaptureProvenance {
                        repository,
                        workflow_path,
                        workflow_digest,
                        provenance_config_digest,
                        head_sha,
                        head_branch,
                        run_id,
                        run_attempt,
                        check_suite_id,
                        event,
                        created_at,
                    },
                },
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum OpsLaunchStage {
    VerifyConfig,
    TargetVerify,
    SecretsCheck,
    SecretsResolve,
    PrestartCheck,
    ReferenceCurrentPriceHealth,
    Start,
}

const OPS_LAUNCH_STAGE_CHAIN: &[OpsLaunchStage] = &[
    OpsLaunchStage::VerifyConfig,
    OpsLaunchStage::TargetVerify,
    OpsLaunchStage::SecretsCheck,
    OpsLaunchStage::SecretsResolve,
    OpsLaunchStage::PrestartCheck,
    OpsLaunchStage::ReferenceCurrentPriceHealth,
    OpsLaunchStage::Start,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum OpsLaunchStageStatus {
    Completed,
    Entering,
    Failed,
}

#[derive(serde::Serialize)]
struct OpsLaunchStageLog {
    ops_launch_stage: OpsLaunchStage,
    status: OpsLaunchStageStatus,
    last_completed_stage: Option<OpsLaunchStage>,
    last_failed_stage: Option<OpsLaunchStage>,
}

struct OpsLaunchContext {
    profile: String,
    config_root: PathBuf,
    target_host_facts_source: Box<dyn HostFactsSource>,
    loaded: Option<LoadedBoltV3Config>,
    resolved_secrets: Option<ResolvedBoltV3Secrets>,
    /// Host facts observed by the `TargetVerify` stage, threaded forward so the
    /// `Start` stage can record them in the durable launch identity. `None`
    /// until `TargetVerify` runs, and stays `None` when no deploy target is
    /// configured (no host was observed).
    observed_host_facts: Option<ObservedHostFacts>,
}

impl OpsLaunchContext {
    fn new(
        profile: String,
        config_root: PathBuf,
        target_host_facts_source: Box<dyn HostFactsSource>,
    ) -> Self {
        Self {
            profile,
            config_root,
            target_host_facts_source,
            loaded: None,
            resolved_secrets: None,
            observed_host_facts: None,
        }
    }

    fn loaded(&self) -> Result<&LoadedBoltV3Config, Box<dyn std::error::Error>> {
        self.loaded.as_ref().ok_or_else(|| {
            "ops launch config must be loaded by verify-config before this stage".into()
        })
    }
}

fn run_ops_launch(profile: String, config_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut context =
        OpsLaunchContext::new(profile, config_root, Box::new(Imdsv2HostFactsSource::new()));
    run_ops_launch_chain_with(|stage| run_ops_launch_stage(stage, &mut context))
}

fn run_ops_launch_chain_with<F>(mut run_stage: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(OpsLaunchStage) -> Result<(), Box<dyn std::error::Error>>,
{
    let mut last_completed_stage = None;
    for &stage in OPS_LAUNCH_STAGE_CHAIN {
        emit_ops_launch_stage_log(
            stage,
            OpsLaunchStageStatus::Entering,
            last_completed_stage,
            None,
        )?;
        match run_stage(stage) {
            Ok(()) => {
                last_completed_stage = Some(stage);
                if stage != OpsLaunchStage::Start {
                    emit_ops_launch_stage_log(
                        stage,
                        OpsLaunchStageStatus::Completed,
                        last_completed_stage,
                        None,
                    )?;
                }
            }
            Err(error) => {
                emit_ops_launch_stage_log(
                    stage,
                    OpsLaunchStageStatus::Failed,
                    last_completed_stage,
                    Some(stage),
                )?;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn emit_ops_launch_stage_log(
    stage: OpsLaunchStage,
    status: OpsLaunchStageStatus,
    last_completed_stage: Option<OpsLaunchStage>,
    last_failed_stage: Option<OpsLaunchStage>,
) -> Result<(), Box<dyn std::error::Error>> {
    let log = OpsLaunchStageLog {
        ops_launch_stage: stage,
        status,
        last_completed_stage,
        last_failed_stage,
    };
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &log)?;
    writeln!(&mut stdout)?;
    Ok(())
}

fn run_ops_launch_stage(
    stage: OpsLaunchStage,
    context: &mut OpsLaunchContext,
) -> Result<(), Box<dyn std::error::Error>> {
    match stage {
        OpsLaunchStage::VerifyConfig => {
            let verification = verify_live_config(&context.config_root, &context.profile)?;
            context.loaded = Some(verification.loaded);
            Ok(())
        }
        OpsLaunchStage::TargetVerify => {
            context.observed_host_facts = run_loaded_target_verify(
                &context.config_root,
                context.target_host_facts_source.as_ref(),
            )?;
            Ok(())
        }
        OpsLaunchStage::SecretsCheck => run_loaded_secrets_check(context.loaded()?).map(|_| ()),
        OpsLaunchStage::SecretsResolve => {
            context.resolved_secrets = Some(run_loaded_secrets_resolve(context.loaded()?)?);
            Ok(())
        }
        OpsLaunchStage::PrestartCheck => run_loaded_prestart_check(context.loaded()?, None),
        OpsLaunchStage::ReferenceCurrentPriceHealth => {
            let resolved = context.resolved_secrets.as_ref().ok_or(
                "ops launch secrets-resolve stage must run before reference-current-price-health",
            )?;
            run_loaded_reference_current_price_health_with_resolved(context.loaded()?, resolved)
        }
        OpsLaunchStage::Start => {
            let loaded = context
                .loaded
                .take()
                .ok_or("ops launch start stage requires a loaded config from verify-config")?;
            let resolved = context
                .resolved_secrets
                .take()
                .ok_or("ops launch start stage requires resolved secrets from secrets-resolve")?;
            record_launch_identity_best_effort(
                &context.profile,
                &loaded,
                context.observed_host_facts.take(),
            );
            start_loaded_node_with_resolved(loaded, &resolved)
        }
    }
}

/// Build the durable launch identity from primitives. Pure: depends only on
/// its arguments (and the build-time `current_build_head_sha`), so it is
/// unit-testable without a `LoadedBoltV3Config` or any syscalls.
fn build_launch_identity(
    profile: &str,
    config_bundle_checksum: &str,
    pid: u32,
    launched_at_unix_secs: u64,
    target_host_facts: Option<ObservedHostFacts>,
) -> LaunchIdentity {
    LaunchIdentity {
        build_head_sha: current_build_head_sha().map(str::to_owned),
        profile: profile.to_owned(),
        config_bundle_checksum: config_bundle_checksum.to_owned(),
        launched_at_unix_secs,
        pid,
        target_host_facts,
    }
}

fn record_launch_identity_best_effort(
    profile: &str,
    loaded: &LoadedBoltV3Config,
    target_host_facts: Option<ObservedHostFacts>,
) {
    let pid = std::process::id();
    let launched_at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let identity = build_launch_identity(
        profile,
        &loaded.config_bundle_checksum,
        pid,
        launched_at_unix_secs,
        target_host_facts,
    );
    let catalog_directory = Path::new(&loaded.root.persistence.catalog_directory);
    match write_launch_identity(catalog_directory, &identity) {
        Ok(written) => println!(
            "{}",
            serde_json::json!({
                "ops_launch_identity": "written",
                "path": written.path.display().to_string(),
                "sha256": written.sha256,
            })
        ),
        Err(error) => eprintln!(
            "{}",
            serde_json::json!({
                "ops_launch_identity": "write-failed",
                "error": error.to_string(),
            })
        ),
    }
}

fn run_init_kill_switch_store(config: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    let kill_switch = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .ok_or("risk.kill_switch block is required to initialize the kill-switch store")?;
    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
    store.bootstrap_initial_armed_loss_snapshot()?;
    let output = serde_json::json!({
        KILL_SWITCH_STORE_INIT_COMPLETED_OUTPUT_FIELD: true,
        KILL_SWITCH_STORE_INIT_STATE_PATH_OUTPUT_FIELD: store.path().display().to_string(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn run_loss_governor_manual_recovery(
    config: &Path,
    operator_id: String,
    evidence_path: String,
    evidence_sha256: String,
    observed_at_ns: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    let outcome = recover_loss_governor_manual_halt(
        &loaded,
        LossGovernorManualRecoveryCommand {
            operator_id,
            evidence_path,
            evidence_sha256,
            observed_at_ns,
            now_ns: current_unix_nanos_for_cli()?,
        },
    )?;
    let output = serde_json::json!({
        LOSS_GOVERNOR_MANUAL_RECOVERY_COMPLETED_OUTPUT_FIELD: true,
        LOSS_GOVERNOR_MANUAL_RECOVERY_STATE_PATH_OUTPUT_FIELD: outcome.state_path.display().to_string(),
        LOSS_GOVERNOR_MANUAL_RECOVERY_PREVIOUS_STATE_OUTPUT_FIELD: format!("{:?}", outcome.previous_state),
        LOSS_GOVERNOR_MANUAL_RECOVERY_RECOVERED_STATE_OUTPUT_FIELD: format!("{:?}", outcome.recovered_state),
        LOSS_GOVERNOR_MANUAL_RECOVERY_COUNT_OUTPUT_FIELD: outcome.manual_recovery_count,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn run_reference_live_probe_command(config: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let probe = loaded.root.reference_live_probe.as_ref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "reference_live_probe must be configured",
        )
    })?;
    let ssm_resolver_session = SsmResolverSession::new()?;
    let chainlink =
        resolve_bolt_v3_client_secrets(&ssm_resolver_session, &loaded, &probe.chainlink_client_id)?;
    let polyresearch = resolve_bolt_v3_client_secrets(
        &ssm_resolver_session,
        &loaded,
        &probe.polyresearch_client_id,
    )?;
    let mut clients = BTreeMap::new();
    clients.extend(chainlink.clients);
    clients.extend(polyresearch.clients);
    let resolved = ResolvedBoltV3Secrets { clients };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(run_reference_live_probe(&loaded, &resolved))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_reference_current_price_health_command(
    config: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    run_loaded_reference_current_price_health(&loaded)
}

fn run_capture_chainlink_reference_fixture_command(
    config: &Path,
    request: BoundaryFixtureCaptureRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let ssm_resolver_session = SsmResolverSession::new()?;
    let resolved = resolve_bolt_v3_client_secrets(
        &ssm_resolver_session,
        &loaded,
        request.client_key.as_str(),
    )?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(capture_reference_boundary_fixture(
        &loaded, &resolved, request,
    ))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_loaded_reference_current_price_health(
    loaded: &LoadedBoltV3Config,
) -> Result<(), Box<dyn std::error::Error>> {
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let health_run = prepare_reference_current_price_health_run(loaded)?;
    run_reference_current_price_health(health_run)
}

fn run_loaded_reference_current_price_health_with_resolved(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<(), Box<dyn std::error::Error>> {
    let health_run = prepare_reference_current_price_health_run_with_resolved(loaded, resolved)?;
    run_reference_current_price_health(health_run)
}

fn run_reference_current_price_health(
    health_run: ReferenceCurrentPriceHealthRun,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    let report = runtime.block_on(local.run_until(async move {
        let mut health_run = health_run;
        run_prepared_reference_current_price_health(&mut health_run).await
    }))?;
    print_reference_current_price_health_report(&report)
}

#[derive(serde::Serialize)]
struct ReferenceCurrentPriceHealthOperatorReport<'a> {
    #[serde(flatten)]
    report: &'a ReferenceCurrentPriceHealthReport,
    operator_health: BoltV3InputHealth,
}

fn print_reference_current_price_health_report(
    report: &ReferenceCurrentPriceHealthReport,
) -> Result<(), Box<dyn std::error::Error>> {
    let operator_report = ReferenceCurrentPriceHealthOperatorReport {
        report,
        operator_health: BoltV3InputHealth::from_reference_current_price_report(report),
    };
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, &operator_report)?;
    use std::io::Write as _;
    writeln!(&mut stdout)?;
    if !report.all_sources_observed() {
        return Err(std::io::Error::other(REFERENCE_CURRENT_PRICE_HEALTH_UNOBSERVED_ERROR).into());
    }
    Ok(())
}

fn run_data_client_probe(
    config: &Path,
    client_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    let (node_runtime, probe_loaded) =
        build_bolt_v3_strategy_free_data_client_probe_live_node(&loaded, client_key)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    let report = runtime.block_on(local.run_until(async move {
        run_bolt_v3_data_client_probe(node_runtime, &probe_loaded, client_key).await
    }))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_data_client_census(
    config: &Path,
    client_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    let (node_runtime, census_loaded) =
        build_bolt_v3_strategy_free_data_client_probe_live_node(&loaded, client_key)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    let report = runtime.block_on(local.run_until(async move {
        run_bolt_v3_data_client_census(node_runtime, &census_loaded, client_key).await
    }))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// Output reports are typed structs (serde derives the JSON keys from the field
// names) rather than `json!` string keys, so no operator-runtime string literals
// are introduced in this binary's output path.
#[derive(serde::Serialize)]
struct GenerateLiveConfigReport {
    generated_live_config: bool,
    output: String,
    source_profile: String,
    profile_bundle_sha256: String,
    generator_format_version: u32,
    invariants: ProductionInvariants,
}

#[derive(serde::Serialize)]
struct VerifyLiveConfigReport {
    verified_live_config: bool,
    profile: String,
    deployed: String,
    profile_bundle_sha256: String,
    matches_profile: bool,
    loads_against_binary: bool,
    invariants: ProductionInvariants,
}

fn run_generate_live_config(
    config_root: &Path,
    profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = live_config_path(config_root);
    let generated = generate_live_config(config_root, profile)?;
    // Non-secret runtime config (SSM refs + public addresses only) — written
    // group/world-readable so the `bolt` service user can read it (the deploy may
    // tighten ownership/mode to root:bolt 0640). NOT the private 0600 secret mode.
    write_atomic_file_with_mode(&output, generated.text.as_bytes(), RUNTIME_CONFIG_FILE_MODE)
        .map_err(|error| {
            format!(
                "failed to write generated runtime config `{}`: {}",
                error.path.display(),
                error.source
            )
        })?;
    let report = GenerateLiveConfigReport {
        generated_live_config: true,
        output: output.display().to_string(),
        source_profile: generated.source_profile,
        profile_bundle_sha256: generated.profile_bundle_sha256,
        generator_format_version: GENERATOR_FORMAT_VERSION,
        invariants: generated.invariants,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_verify_live_config(
    config_root: &Path,
    profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let verification = verify_live_config(config_root, profile)?;
    let report = VerifyLiveConfigReport {
        verified_live_config: true,
        profile: verification.profile_id,
        deployed: verification.deployed_path.display().to_string(),
        profile_bundle_sha256: verification.profile_bundle_sha256,
        matches_profile: verification.matches_profile,
        loads_against_binary: verification.loads_against_binary,
        invariants: verification.invariants,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// `ops status` is a read-only advisory: it inspects the deployed runtime config,
// the durable launch-identity artifact, and the deploy-target binding, and prints
// a JSON truth-table comparing them against the installed binary. It NEVER gates
// (always exits 0) and is offline-safe (off-instance / IMDS-unreachable host facts
// are reported as advisory text, never as an error). Like the other ops reports,
// every output field is a serde-derived struct/enum key so no operator-runtime
// string literals are introduced in this binary's output path.

/// Advisory comparison of the operator-supplied `--intended-sha` against the SHA
/// the installed binary was built from. Variant names are the kebab-case JSON
/// values, so serde derives them and no literal classification is needed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum IntendedShaStatus {
    NotSpecified,
    Malformed,
    Match,
    Mismatch,
    UnknownInstalled,
}

/// Operator-actionable recommendation derived from installed-vs-intended SHA and
/// launch-identity liveness. Variant names are the kebab-case JSON values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum StateAdvisory {
    /// Installed binary == intended, the recorded launch identity matches the
    /// installed binary / requested profile / current config bundle, AND the
    /// running host is the configured deploy target (or no target is
    /// configured). Reflects the last recorded launch, not live process
    /// liveness (that is systemd's concern).
    NoOp,
    /// Installed binary == intended but the recorded launch identity is absent or
    /// diverges (binary, profile, or config bundle), so a relaunch is needed.
    LaunchNeeded,
    /// Installed binary != intended (a newer/different version must be deployed).
    DeployNeeded,
    /// Cannot recommend: the intended or installed SHA is unknown/malformed,
    /// the deployed config could not be verified, or the running host could not
    /// be confirmed as the configured deploy target (mismatched / unobservable).
    Unknown,
}

/// Deployed-runtime-config row of the advisory.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum ConfigStatus {
    Verified {
        profile_bundle_sha256: String,
        config_bundle_checksum: String,
        matches_profile: bool,
        loads_against_binary: bool,
        deployed_config_path: String,
    },
    Error {
        error: String,
    },
}

/// Durable launch-identity-artifact row of the advisory.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum LaunchIdentityStatus {
    Present {
        build_head_sha: Option<String>,
        profile: String,
        config_bundle_checksum: String,
        launched_at_unix_secs: u64,
        pid: u32,
        target_host_facts: Option<ObservedHostFacts>,
        /// `None` when either the recorded or installed SHA is unknown.
        matches_installed_binary: Option<bool>,
        matches_requested_profile: bool,
        matches_current_config: bool,
    },
    Absent,
    Unreadable {
        error: String,
    },
    SkippedConfigUnavailable,
}

/// One configured deploy-target field that did not match the observed host fact.
#[derive(Debug, serde::Serialize)]
struct DeployTargetFieldMismatch {
    field: String,
    configured: String,
    observed: Option<String>,
}

/// Deploy-target binding row of the advisory.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum DeployTargetStatus {
    NoTargetConfigured,
    Matched,
    Mismatched {
        field_mismatches: Vec<DeployTargetFieldMismatch>,
    },
    /// Off-instance / IMDS unreachable: advisory only, never an error.
    HostFactsUnavailable {
        detail: String,
    },
    ConfigError {
        error: String,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, serde::Serialize)]
struct OpsStatusBody {
    profile: String,
    installed_binary_sha: Option<String>,
    intended_sha: Option<String>,
    intended_sha_status: IntendedShaStatus,
    state_advisory: StateAdvisory,
    config: ConfigStatus,
    launch_identity: LaunchIdentityStatus,
    deploy_target: DeployTargetStatus,
    operator_health: BoltV3OperatorHealthSurface,
}

#[derive(Debug, serde::Serialize)]
struct OpsStatusReport {
    ops_status: OpsStatusBody,
}

/// Classify the operator-supplied intended SHA against the installed binary's
/// build SHA. Pure: depends only on its two arguments so it is unit-testable.
fn intended_sha_status(intended: Option<&str>, installed: Option<&str>) -> IntendedShaStatus {
    let Some(intended) = intended else {
        return IntendedShaStatus::NotSpecified;
    };
    if !is_lowercase_git_sha(intended) {
        return IntendedShaStatus::Malformed;
    }
    match installed {
        Some(installed) if installed == intended => IntendedShaStatus::Match,
        Some(_) => IntendedShaStatus::Mismatch,
        None => IntendedShaStatus::UnknownInstalled,
    }
}

/// Derive the advisory. Pure: depends only on its arguments, so it is unit-testable.
/// Classifies the installed-vs-intended SHA via `intended_sha_status` (the single
/// owner of that truth-table), layers the launch-identity liveness check on top,
/// and downgrades to `Unknown` whenever the deploy target cannot be confirmed
/// (mismatched / unobservable host) or the config is unavailable — so `NoOp` is
/// only ever reported when the running host is the configured (or unconfigured)
/// target.
fn derive_state_advisory(
    intended: Option<&str>,
    installed: Option<&str>,
    launch_identity: &LaunchIdentityStatus,
    deploy_target: &DeployTargetStatus,
) -> StateAdvisory {
    // Every actionable advisory (deploy here / launch here / nothing to do
    // here) is host-specific: it implicitly asserts THIS host is the configured
    // target and that we could assess the config. If the running host is not
    // confirmed as the target, or config could not be assessed, we cannot
    // recommend any action -> `Unknown`. Gating ABOVE the SHA check ensures a
    // SHA mismatch on the wrong / unobservable host does not surface a
    // misleading `deploy-needed` next to `deploy_target: mismatched`.
    let host_confirmed = matches!(
        deploy_target,
        DeployTargetStatus::Matched | DeployTargetStatus::NoTargetConfigured
    );
    if !host_confirmed
        || matches!(
            launch_identity,
            LaunchIdentityStatus::SkippedConfigUnavailable
        )
    {
        return StateAdvisory::Unknown;
    }
    match intended_sha_status(intended, installed) {
        IntendedShaStatus::NotSpecified
        | IntendedShaStatus::Malformed
        | IntendedShaStatus::UnknownInstalled => StateAdvisory::Unknown,
        IntendedShaStatus::Mismatch => StateAdvisory::DeployNeeded,
        IntendedShaStatus::Match => match launch_identity {
            LaunchIdentityStatus::Present {
                matches_installed_binary: Some(true),
                matches_requested_profile: true,
                matches_current_config: true,
                ..
            } => StateAdvisory::NoOp,
            _ => StateAdvisory::LaunchNeeded,
        },
    }
}

/// Read the durable launch-identity artifact under `catalog_directory` and
/// compare it against the installed binary, the requested profile, and the
/// current config bundle. A missing artifact is `Absent`; an unreadable or
/// malformed artifact is `Unreadable` with the error text (advisory, not a gate).
fn launch_identity_status(
    catalog_directory: &Path,
    installed_sha: Option<&str>,
    requested_profile: &str,
    current_config_checksum: &str,
) -> LaunchIdentityStatus {
    match read_launch_identity(catalog_directory) {
        Ok(Some(identity)) => {
            let matches_installed_binary = match (identity.build_head_sha.as_deref(), installed_sha)
            {
                (Some(recorded), Some(installed)) => Some(recorded == installed),
                _ => None,
            };
            LaunchIdentityStatus::Present {
                matches_installed_binary,
                matches_requested_profile: identity.profile == requested_profile,
                matches_current_config: identity.config_bundle_checksum == current_config_checksum,
                build_head_sha: identity.build_head_sha,
                profile: identity.profile,
                config_bundle_checksum: identity.config_bundle_checksum,
                launched_at_unix_secs: identity.launched_at_unix_secs,
                pid: identity.pid,
                target_host_facts: identity.target_host_facts,
            }
        }
        Ok(None) => LaunchIdentityStatus::Absent,
        Err(error) => LaunchIdentityStatus::Unreadable {
            error: error.to_string(),
        },
    }
}

/// Load and verify the deploy-target binding against the supplied host-facts
/// source. Offline-safe: an unobservable host (`DeployTargetError::Observe`) is
/// reported as `HostFactsUnavailable`, never an error. Generic over the source
/// so tests can inject a fake with no network access.
fn deploy_target_status<S: HostFactsSource>(config_root: &Path, source: &S) -> DeployTargetStatus {
    let config = match load_deploy_target(config_root) {
        Ok(config) => config,
        Err(error) => {
            return DeployTargetStatus::ConfigError {
                error: error.to_string(),
            };
        }
    };
    match verify_deploy_target(&config, source).map(|verification| verification.outcome) {
        Ok(TargetVerifyOutcome::NoTargetConfigured) => DeployTargetStatus::NoTargetConfigured,
        Ok(TargetVerifyOutcome::Matched) => DeployTargetStatus::Matched,
        Ok(TargetVerifyOutcome::Mismatched(mismatches)) => DeployTargetStatus::Mismatched {
            field_mismatches: mismatches
                .into_iter()
                .map(|mismatch| DeployTargetFieldMismatch {
                    field: mismatch.field.to_string(),
                    configured: mismatch.configured,
                    observed: mismatch.observed,
                })
                .collect(),
        },
        // Off-instance / IMDS unreachable: advisory, never an error (offline-safe).
        Err(DeployTargetError::Observe(detail)) => {
            DeployTargetStatus::HostFactsUnavailable { detail }
        }
        Err(error) => DeployTargetStatus::Error {
            error: error.to_string(),
        },
    }
}

fn ops_status_operator_health_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> BoltV3OperatorHealthSurface {
    let Some(kill_switch) = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .filter(|kill_switch| kill_switch.enabled)
    else {
        return BoltV3OperatorHealthSurface::not_configured();
    };
    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
    let kill_switch_state = match store.load_recovery_state() {
        Ok(KillSwitchRecoveryState::Recovered(state)) => state,
        Ok(KillSwitchRecoveryState::FailClosed { reason, state: _ }) => {
            KillSwitchState::FailedManualIntervention {
                halt_id: OPS_STATUS_KILL_SWITCH_STORE_FAIL_CLOSED_HALT_ID.to_string(),
                reason: format!(
                    "{OPS_STATUS_KILL_SWITCH_STORE_FAIL_CLOSED_REASON_PREFIX}: {reason}"
                ),
            }
        }
        Err(error) => KillSwitchState::FailedManualIntervention {
            halt_id: OPS_STATUS_KILL_SWITCH_STORE_UNREADABLE_HALT_ID.to_string(),
            reason: error.to_string(),
        },
    };
    let capital_admission_configured =
        bolt_v2::bolt_v3_settlement_runtime::capital_admission_runtime_feed_pool(&loaded.root)
            .is_some();
    let reference_source_count = loaded
        .strategies
        .iter()
        .filter_map(|strategy| strategy.config.reference_current_price.as_ref())
        .map(|reference| reference.source_order.len())
        .sum();
    let reject_observer = if capital_admission_configured {
        BoltV3RejectObserverHealth::unobserved()
    } else {
        BoltV3RejectObserverHealth::not_configured()
    };
    let venue_truth = if capital_admission_configured {
        BoltV3VenueTruthHealth::from_configured_kill_switch_and_capital_state(
            &kill_switch_state,
            None,
        )
    } else {
        BoltV3VenueTruthHealth::not_configured()
    };
    let input_health = if reference_source_count == 0 {
        BoltV3InputHealth::not_configured()
    } else {
        BoltV3InputHealth::unobserved(reference_source_count)
    };
    BoltV3OperatorHealthSurface::from_parts(reject_observer, venue_truth, input_health)
}

fn run_ops_status(
    config_root: &Path,
    profile: &str,
    intended_sha: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let installed_sha = current_build_head_sha();

    // The config row also supplies the catalog directory and current checksum the
    // launch-identity row needs; when the config cannot be verified the identity
    // row is skipped rather than reading an arbitrary location.
    let (config, launch_identity, operator_health) = match verify_live_config(config_root, profile)
    {
        Ok(verification) => {
            let catalog_directory =
                Path::new(&verification.loaded.root.persistence.catalog_directory).to_path_buf();
            let current_checksum = verification.loaded.config_bundle_checksum.clone();
            let config = ConfigStatus::Verified {
                deployed_config_path: verification.deployed_path.display().to_string(),
                profile_bundle_sha256: verification.profile_bundle_sha256,
                config_bundle_checksum: current_checksum.clone(),
                matches_profile: verification.matches_profile,
                loads_against_binary: verification.loads_against_binary,
            };
            let launch_identity = launch_identity_status(
                &catalog_directory,
                installed_sha,
                profile,
                &current_checksum,
            );
            let operator_health = ops_status_operator_health_from_loaded(&verification.loaded);
            (config, launch_identity, operator_health)
        }
        Err(error) => (
            ConfigStatus::Error {
                error: error.to_string(),
            },
            LaunchIdentityStatus::SkippedConfigUnavailable,
            BoltV3OperatorHealthSurface::not_configured(),
        ),
    };

    let deploy_target = deploy_target_status(config_root, &Imdsv2HostFactsSource::new());

    let state_advisory = derive_state_advisory(
        intended_sha,
        installed_sha,
        &launch_identity,
        &deploy_target,
    );

    let report = OpsStatusReport {
        ops_status: OpsStatusBody {
            profile: profile.to_string(),
            installed_binary_sha: installed_sha.map(str::to_string),
            intended_sha: intended_sha.map(str::to_string),
            intended_sha_status: intended_sha_status(intended_sha, installed_sha),
            state_advisory,
            config,
            launch_identity,
            deploy_target,
            operator_health,
        },
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_prestart_check(
    config: &Path,
    required_catalog_prefix_override: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    run_loaded_prestart_check(&loaded, required_catalog_prefix_override)
}

fn run_loaded_prestart_check(
    loaded: &LoadedBoltV3Config,
    required_catalog_prefix_override: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let required_catalog_prefix =
        resolve_required_catalog_prefix(loaded, required_catalog_prefix_override)?;
    if !required_catalog_prefix.is_absolute() {
        return Err(format!(
            "--required-catalog-prefix must be an absolute path: `{}`",
            required_catalog_prefix.display()
        )
        .into());
    }
    let canonical_required_prefix =
        std::fs::canonicalize(&required_catalog_prefix).map_err(|source| {
            std::io::Error::new(
                source.kind(),
                format!(
                    "--required-catalog-prefix `{}` is not readable before service start: {source}",
                    required_catalog_prefix.display()
                ),
            )
        })?;
    let configured_catalog_directory = Path::new(&loaded.root.persistence.catalog_directory);
    let catalog_link_metadata = std::fs::symlink_metadata(configured_catalog_directory).map_err(
        |source| {
            std::io::Error::new(
                source.kind(),
                format!(
                    "persistence.catalog_directory `{}` is not readable before service start: {source}",
                    configured_catalog_directory.display()
                ),
            )
        },
    )?;
    if catalog_link_metadata.file_type().is_symlink() {
        return Err(format!(
            "persistence.catalog_directory `{}` must not be a symlink",
            configured_catalog_directory.display()
        )
        .into());
    }
    if !catalog_link_metadata.is_dir() {
        return Err(format!(
            "persistence.catalog_directory `{}` must be a directory before service start",
            configured_catalog_directory.display()
        )
        .into());
    }
    let catalog_directory =
        std::fs::canonicalize(configured_catalog_directory).map_err(|source| {
            std::io::Error::new(
                source.kind(),
                format!(
                    "persistence.catalog_directory `{}` cannot be canonicalized before service start: {source}",
                    configured_catalog_directory.display()
                ),
            )
        })?;
    if !catalog_directory.starts_with(&canonical_required_prefix) {
        return Err(format!(
            "persistence.catalog_directory `{}` must be under `{}` for this service",
            catalog_directory.display(),
            canonical_required_prefix.display()
        )
        .into());
    }
    let min_free_bytes = loaded.root.persistence.min_free_bytes.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "persistence.min_free_bytes must be configured for ops prestart-check",
        )
    })?;
    verify_catalog_write_probe(&catalog_directory)?;
    let available_bytes = filesystem_available_bytes(&catalog_directory)?;
    if available_bytes < min_free_bytes {
        return Err(format!(
            "persistence.catalog_directory `{}` has {available_bytes} free bytes, below configured persistence.min_free_bytes {min_free_bytes}",
            catalog_directory.display()
        )
        .into());
    }
    println!(
        "ops prestart check ok: catalog_directory={} available_bytes={} min_free_bytes={}",
        catalog_directory.display(),
        available_bytes,
        min_free_bytes
    );
    Ok(())
}

fn resolve_required_catalog_prefix(
    loaded: &LoadedBoltV3Config,
    required_catalog_prefix_override: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(required_catalog_prefix) = required_catalog_prefix_override {
        return Ok(required_catalog_prefix.to_path_buf());
    }
    let configured = loaded
        .root
        .persistence
        .required_catalog_prefix
        .as_deref()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "persistence.required_catalog_prefix must be configured for live storage prestart checks",
            )
        })?;
    Ok(PathBuf::from(configured))
}

fn verify_catalog_write_probe(catalog_directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let probe_path = catalog_directory.join(format!(
        ".bolt-v2-prestart-write-probe-{}",
        std::process::id()
    ));
    match std::fs::remove_file(&probe_path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(std::io::Error::new(
                source.kind(),
                format!(
                    "persistence.catalog_directory `{}` stale write probe cleanup failed at `{}`: {source}",
                    catalog_directory.display(),
                    probe_path.display()
                ),
            )
            .into());
        }
    }
    let mut probe_options = OpenOptions::new();
    probe_options.write(true).create_new(true);
    #[cfg(unix)]
    probe_options.custom_flags(libc::O_NOFOLLOW);
    let mut probe_file = probe_options.open(&probe_path).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "persistence.catalog_directory `{}` write probe failed at `{}`: {source}",
                catalog_directory.display(),
                probe_path.display()
            ),
        )
    })?;
    probe_file.write_all(b"\n").map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "persistence.catalog_directory `{}` write probe byte write failed at `{}`: {source}",
                catalog_directory.display(),
                probe_path.display()
            ),
        )
    })?;
    probe_file.sync_all().map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "persistence.catalog_directory `{}` write probe sync failed at `{}`: {source}",
                catalog_directory.display(),
                probe_path.display()
            ),
        )
    })?;
    drop(probe_file);
    std::fs::remove_file(&probe_path).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "persistence.catalog_directory `{}` write probe cleanup failed at `{}`: {source}",
                catalog_directory.display(),
                probe_path.display()
            ),
        )
    })?;
    Ok(())
}

#[cfg(unix)]
fn filesystem_available_bytes(path: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    let fragment_size = if stat.f_frsize == 0 {
        stat.f_bsize
    } else {
        stat.f_frsize
    };
    let available = u128::from(stat.f_bavail) * u128::from(fragment_size);
    Ok(available.min(u128::from(u64::MAX)) as u64)
}

#[cfg(not(unix))]
fn filesystem_available_bytes(_path: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    Err("ops prestart-check disk free-space validation requires a Unix platform".into())
}

fn run_provider_artifacts_command(
    command: ProviderArtifactsCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        ProviderArtifactsCommand::GenerateLiveSubmitApproval {
            config,
            client_key,
            product_surface,
            expires_at_unix_seconds,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let client = loaded.root.clients.get(&client_key).ok_or_else(|| {
                format!("clients.{client_key} is not configured for live-submit approval")
            })?;
            let binding = binding_for_provider_key(client.venue.as_str()).ok_or_else(|| {
                format!(
                    "clients.{client_key}.venue `{}` is not supported by this build",
                    client.venue.as_str()
                )
            })?;
            let writer = binding.write_live_submit_approval_artifact.ok_or_else(|| {
                format!(
                    "clients.{client_key}.venue `{}` does not support live-submit approval materialization",
                    client.venue.as_str()
                )
            })?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved =
                resolve_bolt_v3_client_secrets(&ssm_resolver_session, &loaded, &client_key)?;
            let build_head_sha = current_build_head_sha()
                .ok_or("bolt-v3 build head_sha is unavailable or invalid")?;
            let now_unix_seconds = current_unix_seconds_for_cli()?;
            let written = writer(
                ProviderLiveSubmitApprovalContext {
                    loaded: &loaded,
                    client_key: &client_key,
                    client,
                    resolved: &resolved,
                    product_surface: Some(&product_surface),
                    now_unix_seconds,
                    build_head_sha,
                },
                expires_at_unix_seconds,
            )?;
            print_written_artifact(&written)
        }
        ProviderArtifactsCommand::PreflightLiveSubmitArming {
            config,
            client_key,
            product_surface,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let client = loaded.root.clients.get(&client_key).ok_or_else(|| {
                format!("clients.{client_key} is not configured for live-submit arming preflight")
            })?;
            let binding = binding_for_provider_key(client.venue.as_str()).ok_or_else(|| {
                format!(
                    "clients.{client_key}.venue `{}` is not supported by this build",
                    client.venue.as_str()
                )
            })?;
            let preflight = binding.preflight_live_submit_arming.ok_or_else(|| {
                format!(
                    "clients.{client_key}.venue `{}` does not support live-submit arming preflight",
                    client.venue.as_str()
                )
            })?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved =
                resolve_bolt_v3_client_secrets(&ssm_resolver_session, &loaded, &client_key)?;
            let build_head_sha = current_build_head_sha()
                .ok_or("bolt-v3 build head_sha is unavailable or invalid")?;
            let now_unix_seconds = current_unix_seconds_for_cli()?;
            let report = preflight(ProviderLiveSubmitApprovalContext {
                loaded: &loaded,
                client_key: &client_key,
                client,
                resolved: &resolved,
                product_surface: Some(&product_surface),
                now_unix_seconds,
                build_head_sha,
            })?
            .ok_or_else(|| {
                format!("clients.{client_key} is not armed for live-submit preflight")
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::to_value(report)?)?
            );
            Ok(())
        }
        ProviderArtifactsCommand::GenerateProductSubmitProof(args) => {
            let GenerateProductSubmitProofArgs {
                provider_key,
                provider_id,
                product_surface,
                toml_checksum,
                order_proof_artifact_path,
                order_proof_artifact_sha256,
                fill_proof_artifact_path,
                fill_proof_artifact_sha256,
                rounding_proof_artifact_path,
                rounding_proof_artifact_sha256,
                fee_proof_artifact_path,
                fee_proof_artifact_sha256,
                settlement_proof_artifact_path,
                settlement_proof_artifact_sha256,
                output,
            } = *args;
            let settlement_proof = match (
                settlement_proof_artifact_path.as_deref(),
                settlement_proof_artifact_sha256.as_deref(),
            ) {
                (Some(artifact_path), Some(artifact_sha256)) => Some(ProviderArtifactReference {
                    artifact_path,
                    artifact_sha256,
                }),
                (None, None) => None,
                _ => {
                    return Err(
                        "settlement proof artifact path and sha256 must be supplied together"
                            .into(),
                    );
                }
            };
            let binding = binding_for_provider_key(&provider_key).ok_or_else(|| {
                format!("provider_key `{provider_key}` is not supported by this build")
            })?;
            let writer = binding.write_product_submit_proof_artifact.ok_or_else(|| {
                format!(
                    "provider_key `{provider_key}` does not support product-submit proof materialization"
                )
            })?;
            let written = writer(ProviderProductSubmitProofArtifactRequest {
                provider_id: &provider_id,
                product_surface: &product_surface,
                toml_checksum: &toml_checksum,
                order_proof: ProviderArtifactReference {
                    artifact_path: &order_proof_artifact_path,
                    artifact_sha256: &order_proof_artifact_sha256,
                },
                fill_proof: ProviderArtifactReference {
                    artifact_path: &fill_proof_artifact_path,
                    artifact_sha256: &fill_proof_artifact_sha256,
                },
                rounding_proof: ProviderArtifactReference {
                    artifact_path: &rounding_proof_artifact_path,
                    artifact_sha256: &rounding_proof_artifact_sha256,
                },
                fee_proof: ProviderArtifactReference {
                    artifact_path: &fee_proof_artifact_path,
                    artifact_sha256: &fee_proof_artifact_sha256,
                },
                settlement_proof,
                output_path: &output,
            })?;
            print_written_artifact(&written)
        }
        ProviderArtifactsCommand::SyncClobV2BalanceAllowanceCache {
            config,
            strategy_instance_id,
            acknowledge_clob_cache_mutation,
        } => {
            if !acknowledge_clob_cache_mutation {
                return Err("sync-clob-v2-balance-allowance-cache requires --acknowledge-clob-cache-mutation".into());
            }
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved = resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let sync = runtime.block_on(
                sync_clob_v2_balance_allowance_cache_from_configured_account(
                    ClobV2BalanceAllowanceCacheSyncRequest {
                        loaded: &loaded,
                        strategy_instance_id: &strategy_instance_id,
                        resolved: &resolved,
                    },
                ),
            )?;
            print_clob_v2_balance_allowance_cache_sync(&sync)
        }
    }
}

fn print_written_artifact(
    written: &WrittenOperatorArtifact,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(&artifact_json(written))?);
    Ok(())
}

fn print_clob_v2_balance_allowance_cache_sync(
    sync: &ClobV2BalanceAllowanceCacheSync,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            CLOB_V2_CACHE_SYNC_COMPLETED_OUTPUT_FIELD: true,
            CLOB_V2_CACHE_SYNC_EXECUTION_CLIENT_OUTPUT_FIELD: &sync.execution_client_id,
            CLOB_V2_CACHE_SYNC_REQUEST_PATH_OUTPUT_FIELD: sync.request_path,
            CLOB_V2_CACHE_SYNC_BASE_URL_HTTP_SHA256_OUTPUT_FIELD: &sync.base_url_http_sha256,
        }))?
    );
    Ok(())
}

fn artifact_json(written: &WrittenOperatorArtifact) -> serde_json::Value {
    serde_json::json!({
        "path": &written.path,
        "sha256": &written.sha256,
    })
}

fn current_unix_seconds_for_cli() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn current_unix_nanos_for_cli() -> Result<u64, Box<dyn std::error::Error>> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(u64::try_from(nanos)?)
}

fn run_secrets_command(command: SecretsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SecretsCommand::Check { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            for binding in run_loaded_secrets_check(&loaded)? {
                println!(
                    "clients.{}: required secret fields present ({})",
                    binding.client_key,
                    binding.secret_field_names.join(", ")
                );
            }
            Ok(())
        }
        SecretsCommand::Resolve { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            let resolved = run_loaded_secrets_resolve(&loaded)?;
            for client_key in resolved.clients.keys() {
                println!("clients.{client_key}: secrets resolved successfully");
            }
            Ok(())
        }
    }
}

struct SecretCheckReport {
    client_key: String,
    secret_field_names: Vec<&'static str>,
}

/// Prove the running host matches the configured deploy target before
/// secrets or runtime side effects. Reads `deploy.toml` from `config_root`
/// (a separate load path from the live-config bundle), observes host facts
/// over IMDSv2 only when a gating binding is configured, and fails closed on
/// a mismatch or an unobservable host. An unconfigured target degrades to a
/// successful no-op so the lane works before any instance is provisioned.
fn run_loaded_target_verify(
    config_root: &Path,
    source: &dyn HostFactsSource,
) -> Result<Option<ObservedHostFacts>, Box<dyn std::error::Error>> {
    let config = load_deploy_target(config_root)?;
    let verification = verify_deploy_target(&config, source)?;
    let log = match &verification.outcome {
        TargetVerifyOutcome::NoTargetConfigured => serde_json::json!({
            "ops_launch_target_verify": "no-target-configured",
        }),
        TargetVerifyOutcome::Matched => serde_json::json!({
            "ops_launch_target_verify": "matched",
        }),
        TargetVerifyOutcome::Mismatched(mismatches) => serde_json::json!({
            "ops_launch_target_verify": "mismatched",
            "field_mismatches": mismatches
                .iter()
                .map(|mismatch| serde_json::json!({
                    "field": mismatch.field,
                    "configured": mismatch.configured,
                    "observed": mismatch.observed,
                }))
                .collect::<Vec<_>>(),
        }),
    };
    println!("{}", serde_json::to_string(&log)?);
    match verification.outcome {
        TargetVerifyOutcome::NoTargetConfigured | TargetVerifyOutcome::Matched => {
            Ok(verification.observed_host_facts)
        }
        TargetVerifyOutcome::Mismatched(mismatches) => Err(format!(
            "deploy target verification failed: {} configured field(s) do not match the running host",
            mismatches.len()
        )
        .into()),
    }
}

fn run_loaded_secrets_check(
    loaded: &LoadedBoltV3Config,
) -> Result<Vec<SecretCheckReport>, Box<dyn std::error::Error>> {
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let mut reports = Vec::new();
    for (client_key, client) in &loaded.root.clients {
        if client.secrets.is_some() {
            let binding = binding_for_provider_key(client.venue.as_str()).ok_or_else(|| {
                format!(
                    "clients.{client_key}.venue `{}` is not supported by this build",
                    client.venue.as_str()
                )
            })?;
            reports.push(SecretCheckReport {
                client_key: client_key.clone(),
                secret_field_names: binding.secret_field_names.to_vec(),
            });
        }
    }
    Ok(reports)
}

fn run_loaded_secrets_resolve(
    loaded: &LoadedBoltV3Config,
) -> Result<ResolvedBoltV3Secrets, Box<dyn std::error::Error>> {
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let ssm_resolver_session = SsmResolverSession::new()?;
    resolve_bolt_v3_secrets(&ssm_resolver_session, loaded).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bolt_v2::bolt_v3_prod_profile::GENERATED_MARKER_PREFIX;
    use clap::{CommandFactory, error::ErrorKind};
    use std::fs;

    fn parsed_ops_command(cli: Cli) -> OpsCommand {
        match cli.command {
            Command::Ops { command } => *command,
            _ => panic!("expected ops command"),
        }
    }

    #[test]
    fn ops_data_client_probe_cli_parses_config_and_client_key() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "data-client-probe",
            "--config",
            "config/root.toml",
            "--client-key",
            "bybit_data",
        ])
        .expect("data-client probe command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::DataClientProbe { config, client_key } => {
                assert_eq!(config, PathBuf::from("config/root.toml"));
                assert_eq!(client_key, "bybit_data");
            }
            _ => panic!("expected ops data-client-probe command"),
        }
    }

    #[test]
    fn ops_loss_governor_manual_recovery_cli_parses_evidence_fields() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "loss-governor-manual-recovery",
            "--config",
            "config/root.toml",
            "--operator-id",
            "operator-primary",
            "--evidence-path",
            "loss-governor/manual-recovery.json",
            "--evidence-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--observed-at-ns",
            "2500",
        ])
        .expect("loss-governor manual recovery command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::LossGovernorManualRecovery {
                config,
                operator_id,
                evidence_path,
                evidence_sha256,
                observed_at_ns,
            } => {
                assert_eq!(config, PathBuf::from("config/root.toml"));
                assert_eq!(operator_id, "operator-primary");
                assert_eq!(evidence_path, "loss-governor/manual-recovery.json");
                assert_eq!(
                    evidence_sha256,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                );
                assert_eq!(observed_at_ns, 2_500);
            }
            _ => panic!("expected ops loss-governor-manual-recovery command"),
        }
    }

    #[test]
    fn ops_loss_governor_manual_recovery_help_does_not_require_config() {
        let error = Cli::command()
            .try_get_matches_from(["bolt-v2", "ops", "loss-governor-manual-recovery", "--help"])
            .expect_err("help should exit through clap without loading config");

        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        let help = error.to_string();
        assert!(
            help.contains("node must be stopped"),
            "help must state the node-stopped operator posture: {help}"
        );
        assert!(
            help.contains("last-writer-wins"),
            "help must state the state race consequence: {help}"
        );
        assert!(
            help.contains("not an operator override"),
            "help must state the command is not an override: {help}"
        );
        assert!(
            help.contains("triggering condition has verifiably passed by clock")
                && help.contains("daily UTC day rolled")
                && help.contains("rolling window elapsed"),
            "help must state the A2 clock-verified recovery semantics: {help}"
        );
        assert!(
            help.contains("CURRENT config value")
                && help.contains("more than the full window must have elapsed")
                && help.contains("exact-equality refuses"),
            "help must state current-config rolling authority and strict window boundary: {help}"
        );
        assert!(
            help.contains("every limit is re-checked live at next node start"),
            "help must state the runtime re-check backstop: {help}"
        );
        assert!(
            help.contains("last audit record per attempt is authoritative"),
            "help must state authoritative audit ordering: {help}"
        );
        assert!(
            help.contains("FailedManualIntervention is terminal")
                && help.contains("out-of-band repair"),
            "help must state terminal failed-intervention posture: {help}"
        );
        assert!(
            help.contains("unbounded append-only") && help.contains("rotate it externally"),
            "help must state audit rotation posture: {help}"
        );
        assert!(
            help.contains("operator-attested audit metadata")
                && help.contains("not file or hash verification"),
            "help must document evidence attestation semantics: {help}"
        );
    }

    #[test]
    fn ops_launch_cli_parses_profile_id_and_config_root() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "launch",
            "--profile",
            "example",
            "--config-root",
            "/opt/bolt-v2/config",
        ])
        .expect("ops launch command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::Launch {
                profile,
                config_root,
            } => {
                assert_eq!(profile, "example");
                assert_eq!(config_root, PathBuf::from("/opt/bolt-v2/config"));
            }
            _ => panic!("expected ops launch command"),
        }
    }

    #[test]
    fn ops_launch_chain_runs_prearm_stages_then_start_in_order() {
        let mut observed = Vec::new();

        run_ops_launch_chain_with(|stage| {
            observed.push(stage);
            Ok(())
        })
        .expect("fake launch stages should pass");

        assert_eq!(
            observed,
            vec![
                OpsLaunchStage::VerifyConfig,
                OpsLaunchStage::TargetVerify,
                OpsLaunchStage::SecretsCheck,
                OpsLaunchStage::SecretsResolve,
                OpsLaunchStage::PrestartCheck,
                OpsLaunchStage::ReferenceCurrentPriceHealth,
                OpsLaunchStage::Start,
            ]
        );
    }

    #[test]
    fn ops_launch_chain_stops_at_failed_stage() {
        let mut observed = Vec::new();

        let error = run_ops_launch_chain_with(|stage| {
            observed.push(stage);
            if stage == OpsLaunchStage::SecretsResolve {
                return Err("secret resolution failed".into());
            }
            Ok(())
        })
        .expect_err("failed launch stage must stop the chain");

        assert_eq!(error.to_string(), "secret resolution failed");
        assert_eq!(
            observed,
            vec![
                OpsLaunchStage::VerifyConfig,
                OpsLaunchStage::TargetVerify,
                OpsLaunchStage::SecretsCheck,
                OpsLaunchStage::SecretsResolve,
            ]
        );
    }

    #[test]
    fn ops_launch_chain_stops_when_target_verify_fails_before_secrets_check() {
        let mut observed = Vec::new();

        let error = run_ops_launch_chain_with(|stage| {
            observed.push(stage);
            if stage == OpsLaunchStage::TargetVerify {
                return Err("deploy target verification failed".into());
            }
            Ok(())
        })
        .expect_err("a failed target-verify stage must stop the chain before secrets");

        assert_eq!(error.to_string(), "deploy target verification failed");
        assert_eq!(
            observed,
            vec![OpsLaunchStage::VerifyConfig, OpsLaunchStage::TargetVerify]
        );
    }

    #[test]
    fn ops_generate_live_config_cli_parses_profile_id_and_config_root() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "generate-live-config",
            "--profile",
            "example",
            "--config-root",
            "config",
        ])
        .expect("generate-live-config command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::GenerateLiveConfig {
                profile,
                config_root,
            } => {
                assert_eq!(profile, "example");
                assert_eq!(config_root, PathBuf::from("config"));
            }
            _ => panic!("expected ops generate-live-config command"),
        }
    }

    #[test]
    fn ops_verify_live_config_cli_parses_profile_id_and_config_root() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "verify-live-config",
            "--profile",
            "example",
            "--config-root",
            "/opt/bolt-v2/config",
        ])
        .expect("verify-live-config command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::VerifyLiveConfig {
                profile,
                config_root,
            } => {
                assert_eq!(profile, "example");
                assert_eq!(config_root, PathBuf::from("/opt/bolt-v2/config"));
            }
            _ => panic!("expected ops verify-live-config command"),
        }
    }

    #[test]
    fn run_live_node_redirects_to_ops_launch_without_arming() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bolt-v2-run-redirect-test-{suffix}"));
        fs::create_dir_all(&dir).expect("test temp dir should create");
        let live = dir.join("live.toml");
        fs::write(
            &live,
            format!("{GENERATED_MARKER_PREFIX}{GENERATOR_FORMAT_VERSION}\n"),
        )
        .expect("generated live.toml marker should write");

        let error =
            run_live_node(live).expect_err("plain run must redirect before any live arming");
        assert!(
            error.to_string().contains("ops launch"),
            "plain run must direct operators to ops launch, got: {error}"
        );
        assert!(
            error.to_string().contains("ops launch --profile")
                && error.to_string().contains("--config-root"),
            "redirect must name the required profile and config-root flags, got: {error}"
        );

        fs::remove_dir_all(&dir).expect("test temp dir should clean up");
    }

    /// Build an `OpsLaunchContext` whose loaded config has every client
    /// `[secrets]` block stripped, so the secrets stages never reach SSM.
    /// `run_loaded_secrets_check` and `run_loaded_secrets_resolve` both skip
    /// clients without secrets, which lets this test exercise the dispatch
    /// wiring hermetically (no AWS, no network).
    fn ops_launch_context_with_secret_free_fixture() -> OpsLaunchContext {
        let mut loaded =
            load_bolt_v3_config(std::path::Path::new("tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root config should load");
        for client in loaded.root.clients.values_mut() {
            client.secrets = None;
        }
        let mut context = OpsLaunchContext::new(
            "fixture-profile".to_string(),
            PathBuf::from("config"),
            Box::new(FakeHostFactsSource::erroring()),
        );
        context.loaded = Some(loaded);
        context
    }

    #[test]
    fn ops_launch_secrets_check_stage_does_not_resolve_secrets() {
        let mut context = ops_launch_context_with_secret_free_fixture();

        run_ops_launch_stage(OpsLaunchStage::SecretsCheck, &mut context)
            .expect("secrets-check stage must pass for a secret-free config");

        assert!(
            context.resolved_secrets.is_none(),
            "secrets-check must validate bindings only and never populate resolved secrets; \
             if resolved_secrets is Some here the SecretsCheck arm is wired to the resolver"
        );
    }

    #[test]
    fn ops_launch_secrets_resolve_stage_populates_resolved_secrets() {
        let mut context = ops_launch_context_with_secret_free_fixture();

        run_ops_launch_stage(OpsLaunchStage::SecretsResolve, &mut context)
            .expect("secrets-resolve stage must pass for a secret-free config");

        assert!(
            context.resolved_secrets.is_some(),
            "secrets-resolve must populate resolved secrets for the start stage to consume; \
             if resolved_secrets is None here the SecretsResolve arm is wired to the checker"
        );
    }

    #[test]
    fn ops_launch_secrets_stages_dispatch_to_distinct_helpers() {
        // Differential guard for the SecretsCheck/SecretsResolve dispatch:
        // only the resolve stage may populate `resolved_secrets`. Swapping the
        // two match arms in `run_ops_launch_stage` flips both observations and
        // fails this test (verified by the swap-proof in the review fix pass).
        let mut check_context = ops_launch_context_with_secret_free_fixture();
        run_ops_launch_stage(OpsLaunchStage::SecretsCheck, &mut check_context)
            .expect("secrets-check stage must pass for a secret-free config");

        let mut resolve_context = ops_launch_context_with_secret_free_fixture();
        run_ops_launch_stage(OpsLaunchStage::SecretsResolve, &mut resolve_context)
            .expect("secrets-resolve stage must pass for a secret-free config");

        assert!(
            check_context.resolved_secrets.is_none() && resolve_context.resolved_secrets.is_some(),
            "SecretsCheck must not resolve and SecretsResolve must resolve; a swapped dispatch \
             inverts these (check resolved={:?}, resolve resolved={:?})",
            check_context.resolved_secrets.is_some(),
            resolve_context.resolved_secrets.is_some(),
        );
    }

    #[test]
    fn ops_launch_target_verify_stage_degrades_when_no_deploy_toml() {
        // Dispatch guard for the TargetVerify arm of `run_ops_launch_stage`:
        // a `config_root` with no `deploy.toml` exercises the dispatch arm and
        // the `NoTargetConfigured => Ok(())` degrade WITHOUT reaching the real
        // IMDS endpoint (no gating binding means the host is never observed),
        // so the test is hermetic — no 169.254.x network call.
        // The `Mismatched => Err` and observe-error fail-closed exits of
        // `run_loaded_target_verify` are pinned directly by
        // `run_loaded_target_verify_errors_when_host_facts_mismatch_configured_target`
        // and `run_loaded_target_verify_errors_when_host_unobservable_for_configured_target`.
        let temp = tempfile::tempdir().expect("tempdir should create");
        let mut context = OpsLaunchContext::new(
            "fixture-profile".to_string(),
            temp.path().to_path_buf(),
            Box::new(FakeHostFactsSource::erroring()),
        );

        run_ops_launch_stage(OpsLaunchStage::TargetVerify, &mut context).expect(
            "target-verify stage must degrade to Ok when no deploy.toml configures a target",
        );
        assert!(
            context.observed_host_facts.is_none(),
            "an unconfigured target must observe no host facts (no host was queried)"
        );
    }

    #[test]
    fn ops_launch_target_verify_stage_uses_injected_host_facts_source() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        fs::write(
            temp.path().join("deploy.toml"),
            "[target]\nregion = \"region-x\"\ninstance_id = \"instance-target\"\n",
        )
        .expect("deploy.toml fixture should write");
        let expected = ObservedHostFacts {
            region: Some("region-x".to_string()),
            availability_zone: Some("region-x-zone-a".to_string()),
            instance_id: Some("instance-target".to_string()),
        };
        let mut context = OpsLaunchContext::new(
            "fixture-profile".to_string(),
            temp.path().to_path_buf(),
            Box::new(FakeHostFactsSource::facts(expected.clone())),
        );

        run_ops_launch_stage(OpsLaunchStage::TargetVerify, &mut context)
            .expect("target-verify stage must use the injected source for configured targets");

        assert_eq!(context.observed_host_facts, Some(expected));
    }

    #[test]
    fn run_loaded_target_verify_returns_observed_facts_for_matching_binding() {
        // A configured gating binding (deploy.toml) plus a source returning the
        // matching facts must thread those facts back to the caller, so the
        // launch identity can record where it launched. Injecting the fake source
        // keeps the test hermetic — no IMDS network call.
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(
            temp.path().join("deploy.toml"),
            "[target]\nregion = \"region-x\"\ninstance_id = \"instance-target\"\n",
        )
        .expect("deploy.toml fixture should write");
        let expected = ObservedHostFacts {
            region: Some("region-x".to_string()),
            availability_zone: Some("region-x-zone-a".to_string()),
            instance_id: Some("instance-target".to_string()),
        };
        let source = FakeHostFactsSource::facts(expected.clone());

        let observed = run_loaded_target_verify(temp.path(), &source)
            .expect("matching facts must verify and thread the observed facts back");

        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn run_loaded_target_verify_returns_none_when_no_deploy_toml() {
        // No deploy.toml means no gating binding, so the host is never observed:
        // the verifier degrades to `Ok(None)` without touching the fake source.
        let temp = tempfile::tempdir().expect("tempdir should create");
        let source = FakeHostFactsSource::erroring();

        let observed = run_loaded_target_verify(temp.path(), &source)
            .expect("an unconfigured target must degrade to Ok(None), never error");

        assert!(
            observed.is_none(),
            "an unconfigured target must observe no host facts"
        );
    }

    #[test]
    fn run_loaded_target_verify_errors_when_host_facts_mismatch_configured_target() {
        // Fail-closed launch gate: a configured gating binding whose value
        // differs from the observed host must abort the launch (Err), never
        // silently proceed. Pins the `Mismatched => Err` arm of
        // `run_loaded_target_verify`; flipping that arm to `Ok(None)` makes this
        // test fail. The injected fake source keeps it hermetic (no IMDS call).
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(
            temp.path().join("deploy.toml"),
            "[target]\nregion = \"region-x\"\ninstance_id = \"instance-target\"\n",
        )
        .expect("deploy.toml fixture should write");
        let source = FakeHostFactsSource::facts(ObservedHostFacts {
            region: Some("region-x".to_string()),
            availability_zone: Some("region-x-zone-a".to_string()),
            instance_id: Some("instance-other".to_string()),
        });

        let error = run_loaded_target_verify(temp.path(), &source)
            .expect_err("a configured target that does not match the host must fail closed");

        assert!(
            error.to_string().contains("do not match the running host"),
            "the abort message must name the host mismatch: {error}"
        );
    }

    #[test]
    fn run_loaded_target_verify_errors_when_host_unobservable_for_configured_target() {
        // Fail-closed on an unobservable host: with a gating binding configured,
        // an erroring host-facts source must abort the launch (Err propagated
        // from `verify_deploy_target`), never degrade to `Ok(None)`. Without a
        // gating binding the source is never consulted, so this fail-closed path
        // is only reachable with a configured target.
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(
            temp.path().join("deploy.toml"),
            "[target]\nregion = \"region-x\"\ninstance_id = \"instance-target\"\n",
        )
        .expect("deploy.toml fixture should write");
        let source = FakeHostFactsSource::erroring();

        run_loaded_target_verify(temp.path(), &source)
            .expect_err("an unobservable host with a configured target must fail closed");
    }

    #[test]
    fn ops_launch_stage_target_verify_serializes_to_kebab_case() {
        // Pin the kebab-case wire contract for the TargetVerify stage so a
        // future serde-attr regression (e.g. dropping `rename_all`) is caught:
        // the stage name is emitted in the ops-launch stage logs that operators
        // and tooling parse.
        assert_eq!(
            serde_json::to_string(&OpsLaunchStage::TargetVerify)
                .expect("OpsLaunchStage must serialize"),
            "\"target-verify\"",
        );
    }

    #[test]
    fn ops_data_client_census_cli_parses_config_and_client_key() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "data-client-census",
            "--config",
            "config/root.toml",
            "--client-key",
            "bybit_data",
        ])
        .expect("data-client census command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::DataClientCensus { config, client_key } => {
                assert_eq!(config, PathBuf::from("config/root.toml"));
                assert_eq!(client_key, "bybit_data");
            }
            _ => panic!("expected ops data-client-census command"),
        }
    }

    #[test]
    fn ops_reference_live_probe_cli_parses_config() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "reference-live-probe",
            "--config",
            "config/root.toml",
        ])
        .expect("reference live probe command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::ReferenceLiveProbe { config } => {
                assert_eq!(config, PathBuf::from("config/root.toml"));
            }
            _ => panic!("expected ops reference-live-probe command"),
        }
    }

    #[test]
    fn ops_reference_current_price_health_cli_parses_config() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "reference-current-price-health",
            "--config",
            "config/root.toml",
        ])
        .expect("reference current price health command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::ReferenceCurrentPriceHealth { config } => {
                assert_eq!(config, PathBuf::from("config/root.toml"));
            }
            _ => panic!("expected ops reference-current-price-health command"),
        }
    }

    #[test]
    fn reference_current_price_health_report_gate_rejects_unobserved_source() {
        let report = ReferenceCurrentPriceHealthReport {
            targets: Vec::new(),
            clients: Vec::new(),
            source_update_observations: vec![ReferenceCurrentPriceSourceUpdateObservation {
                strategy_instance_id: String::new(),
                source_id: String::new(),
                asset: String::new(),
                provider: String::new(),
                provider_instrument: String::new(),
                status: String::new(),
                reason: String::new(),
                observed_ts_ms: None,
                received_ts_ms: None,
            }],
        };

        let error = print_reference_current_price_health_report(&report)
            .expect_err("unobserved reference_current_price sources must fail the command");

        assert!(
            error
                .to_string()
                .contains(REFERENCE_CURRENT_PRICE_HEALTH_UNOBSERVED_ERROR),
            "error should explain the failed health verdict, got: {error}"
        );
    }

    #[test]
    fn prestart_check_rejects_catalog_outside_required_prefix_before_disk_probe() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let outside_catalog_path = temp
            .path()
            .join("outside")
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&required_prefix).expect("required prefix should create");
        fs::create_dir_all(&outside_catalog_path).expect("outside catalog should create");
        let fixture = prestart_fixture_config(&outside_catalog_path, Some(1));

        let error = run_prestart_check(&fixture.config_path, Some(&required_prefix))
            .expect_err("wrong catalog prefix should fail prestart");
        let message = error.to_string();

        assert!(message.contains("persistence.catalog_directory"));
        assert!(message.contains("must be under"));
    }

    #[test]
    fn prestart_check_requires_configured_min_free_bytes() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let fixture = prestart_fixture_config(&catalog_path, None);

        let error = run_prestart_check(&fixture.config_path, Some(&required_prefix))
            .expect_err("missing free-space floor should fail prestart");
        let message = error.to_string();

        assert!(message.contains("persistence.min_free_bytes"));
        assert!(message.contains("ops prestart-check"));
    }

    #[cfg(unix)]
    #[test]
    fn prestart_check_rejects_symlinked_catalog_directory() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_parent = required_prefix.join("var").join("bolt-v3-live");
        let catalog_path = catalog_parent.join("catalog");
        let outside_catalog = temp.path().join("outside-catalog");
        fs::create_dir_all(&catalog_parent).expect("catalog parent should create");
        fs::create_dir_all(&outside_catalog).expect("outside catalog should create");
        std::os::unix::fs::symlink(&outside_catalog, &catalog_path)
            .expect("catalog symlink should create");
        let fixture = prestart_fixture_config(&catalog_path, Some(1));

        let error = run_prestart_check(&fixture.config_path, Some(&required_prefix))
            .expect_err("symlinked catalog directory should fail prestart");
        let message = error.to_string();

        assert!(
            message.contains("must not be a symlink"),
            "expected symlink rejection, got: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn prestart_check_rejects_unwritable_catalog_directory() {
        use std::os::unix::fs::PermissionsExt;

        if running_as_root() {
            return;
        }

        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let fixture = prestart_fixture_config(&catalog_path, Some(1));

        fs::set_permissions(&catalog_path, fs::Permissions::from_mode(0o555))
            .expect("catalog permissions should update");
        let result = run_prestart_check(&fixture.config_path, Some(&required_prefix));
        fs::set_permissions(&catalog_path, fs::Permissions::from_mode(0o755))
            .expect("catalog permissions should restore");
        let error = result.expect_err("unwritable catalog directory should fail prestart");
        let message = error.to_string();

        assert!(
            message.contains("write probe failed"),
            "expected write-probe rejection, got: {message}"
        );
    }

    #[test]
    fn prestart_check_rejects_free_space_below_configured_floor() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let fixture = prestart_fixture_config(&catalog_path, Some(u64::MAX));

        let error = run_prestart_check(&fixture.config_path, Some(&required_prefix))
            .expect_err("impossible free-space floor should fail prestart");
        let message = error.to_string();

        assert!(
            message.contains("below configured persistence.min_free_bytes"),
            "expected low-space rejection, got: {message}"
        );
    }

    #[test]
    fn prestart_check_accepts_writable_catalog_under_required_prefix() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let fixture = prestart_fixture_config(&catalog_path, Some(1));

        run_prestart_check(&fixture.config_path, Some(&required_prefix))
            .expect("writable catalog under required prefix should pass prestart");
    }

    #[test]
    fn prestart_check_removes_stale_write_probe_file() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let stale_probe_path = catalog_path.join(format!(
            ".bolt-v2-prestart-write-probe-{}",
            std::process::id()
        ));
        fs::write(&stale_probe_path, "stale").expect("stale probe file should create");
        let fixture = prestart_fixture_config(&catalog_path, Some(1));

        run_prestart_check(&fixture.config_path, Some(&required_prefix))
            .expect("stale write-probe file should not block prestart");

        assert!(
            !stale_probe_path.exists(),
            "prestart should clean up the stale probe path"
        );
    }

    #[test]
    fn prestart_check_uses_configured_required_catalog_prefix() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let fixture = prestart_fixture_config_with_required_prefix(
            &catalog_path,
            Some(&required_prefix),
            Some(1),
        );

        run_prestart_check(&fixture.config_path, None)
            .expect("configured required catalog prefix should pass prestart");
    }

    #[test]
    fn prestart_check_requires_required_catalog_prefix_without_override() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let required_prefix = temp.path().join("srv").join("bolt-v2");
        let catalog_path = required_prefix
            .join("var")
            .join("bolt-v3-live")
            .join("catalog");
        fs::create_dir_all(&catalog_path).expect("catalog directory should create");
        let fixture = prestart_fixture_config(&catalog_path, Some(1));

        let error = run_prestart_check(&fixture.config_path, None)
            .expect_err("missing required catalog prefix should fail prestart");
        let message = error.to_string();

        assert!(
            message.contains("persistence.required_catalog_prefix"),
            "expected missing required-prefix rejection, got: {message}"
        );
    }

    #[cfg(unix)]
    fn running_as_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    struct PrestartFixture {
        _temp: tempfile::TempDir,
        config_path: PathBuf,
    }

    fn prestart_fixture_config(
        catalog_directory: &Path,
        min_free_bytes: Option<u64>,
    ) -> PrestartFixture {
        prestart_fixture_config_with_required_prefix(catalog_directory, None, min_free_bytes)
    }

    fn prestart_fixture_config_with_required_prefix(
        catalog_directory: &Path,
        required_catalog_prefix: Option<&Path>,
        min_free_bytes: Option<u64>,
    ) -> PrestartFixture {
        let temp = tempfile::tempdir().expect("fixture tempdir should create");
        let strategy_dir = temp.path().join("strategies");
        fs::create_dir_all(&strategy_dir).expect("strategy dir should create");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            strategy_dir.join("binary_oracle.toml"),
        )
        .expect("strategy fixture should copy");

        let mut root = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bolt_v3/root.toml"),
        )
        .expect("root fixture should read");
        let before_catalog_replace = root.clone();
        root = root.replace(
            "catalog_directory = \"/var/lib/bolt/catalog\"",
            &format!(
                "catalog_directory = \"{}\"",
                catalog_directory.to_string_lossy()
            ),
        );
        assert_ne!(
            root, before_catalog_replace,
            "catalog_directory fixture replacement should update root fixture"
        );
        if let Some(required_catalog_prefix) = required_catalog_prefix {
            let before_required_prefix_replace = root.clone();
            root = root.replace(
                "runtime_capture_start_poll_interval_ms = 50",
                &format!(
                    "required_catalog_prefix = \"{}\"\nruntime_capture_start_poll_interval_ms = 50",
                    required_catalog_prefix.to_string_lossy()
                ),
            );
            assert_ne!(
                root, before_required_prefix_replace,
                "required_catalog_prefix fixture replacement should update root fixture"
            );
        }
        if let Some(min_free_bytes) = min_free_bytes {
            let before_min_free_replace = root.clone();
            root = root.replace(
                "runtime_capture_start_poll_interval_ms = 50",
                &format!(
                    "min_free_bytes = {min_free_bytes}\nruntime_capture_start_poll_interval_ms = 50"
                ),
            );
            assert_ne!(
                root, before_min_free_replace,
                "min_free_bytes fixture replacement should update root fixture"
            );
        }
        let config_path = temp.path().join("root.toml");
        fs::write(&config_path, root).expect("root fixture should write");
        PrestartFixture {
            _temp: temp,
            config_path,
        }
    }

    // --- `ops status` advisory truth-table ---------------------------------

    // `DeployTargetError`, `HostFactsSource`, `ObservedHostFacts`, and
    // `write_launch_identity` are already in scope via `use super::*`; only this
    // one is not.
    use bolt_v2::bolt_v3_operator_artifacts::launch_identity_path;

    /// Fake host-facts source for the advisory tests: never touches the network.
    /// Returns either canned facts or a fixed observe error.
    struct FakeHostFactsSource {
        result: Result<ObservedHostFacts, String>,
    }

    impl FakeHostFactsSource {
        fn facts(facts: ObservedHostFacts) -> Self {
            Self { result: Ok(facts) }
        }

        fn erroring() -> Self {
            Self {
                result: Err("fake host facts unavailable".to_string()),
            }
        }
    }

    impl HostFactsSource for FakeHostFactsSource {
        fn observe(&self) -> Result<ObservedHostFacts, DeployTargetError> {
            self.result.clone().map_err(DeployTargetError::Observe)
        }
    }

    fn well_formed_git_sha(seed: char) -> String {
        seed.to_string().repeat(40)
    }

    #[test]
    fn ops_status_cli_parses_profile_config_root_and_optional_intended_sha() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "status",
            "--profile",
            "example",
            "--config-root",
            "/opt/bolt-v2/config",
            "--intended-sha",
            "0123456789abcdef0123456789abcdef01234567",
        ])
        .expect("ops status command should parse");

        match parsed_ops_command(cli) {
            OpsCommand::Status {
                profile,
                config_root,
                intended_sha,
            } => {
                assert_eq!(profile, "example");
                assert_eq!(config_root, PathBuf::from("/opt/bolt-v2/config"));
                assert_eq!(
                    intended_sha.as_deref(),
                    Some("0123456789abcdef0123456789abcdef01234567")
                );
            }
            _ => panic!("expected ops status command"),
        }
    }

    #[test]
    fn ops_status_cli_parses_without_intended_sha() {
        let cli = Cli::try_parse_from([
            "bolt-v2",
            "ops",
            "status",
            "--profile",
            "example",
            "--config-root",
            "/opt/bolt-v2/config",
        ])
        .expect("ops status command should parse without --intended-sha");

        match parsed_ops_command(cli) {
            OpsCommand::Status { intended_sha, .. } => assert!(intended_sha.is_none()),
            _ => panic!("expected ops status command"),
        }
    }

    #[test]
    fn intended_sha_status_reports_not_specified_when_absent() {
        assert_eq!(
            intended_sha_status(None, Some(well_formed_git_sha('a').as_str())),
            IntendedShaStatus::NotSpecified
        );
    }

    #[test]
    fn intended_sha_status_reports_malformed_for_non_git_sha() {
        assert_eq!(
            intended_sha_status(Some("not-a-sha"), Some(well_formed_git_sha('a').as_str())),
            IntendedShaStatus::Malformed
        );
    }

    #[test]
    fn intended_sha_status_reports_match_when_equal() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            intended_sha_status(Some(sha.as_str()), Some(sha.as_str())),
            IntendedShaStatus::Match
        );
    }

    #[test]
    fn intended_sha_status_reports_mismatch_when_different() {
        assert_eq!(
            intended_sha_status(
                Some(well_formed_git_sha('a').as_str()),
                Some(well_formed_git_sha('b').as_str())
            ),
            IntendedShaStatus::Mismatch
        );
    }

    #[test]
    fn intended_sha_status_reports_unknown_installed_when_binary_sha_absent() {
        assert_eq!(
            intended_sha_status(Some(well_formed_git_sha('a').as_str()), None),
            IntendedShaStatus::UnknownInstalled
        );
    }

    #[test]
    fn launch_identity_status_reports_present_with_correct_match_flags() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let installed = well_formed_git_sha('a');
        let identity = LaunchIdentity {
            build_head_sha: Some(installed.clone()),
            profile: "example".to_string(),
            config_bundle_checksum: "checksum-abc".to_string(),
            launched_at_unix_secs: 1,
            pid: 4242,
            target_host_facts: None,
        };
        write_launch_identity(temp.path(), &identity).expect("launch identity should write");

        let status = launch_identity_status(
            temp.path(),
            Some(installed.as_str()),
            "example",
            "checksum-abc",
        );

        match status {
            LaunchIdentityStatus::Present {
                build_head_sha,
                profile,
                config_bundle_checksum,
                launched_at_unix_secs,
                pid,
                target_host_facts,
                matches_installed_binary,
                matches_requested_profile,
                matches_current_config,
            } => {
                assert_eq!(build_head_sha.as_deref(), Some(installed.as_str()));
                assert_eq!(profile, "example");
                assert_eq!(config_bundle_checksum, "checksum-abc");
                assert_eq!(launched_at_unix_secs, 1);
                assert_eq!(pid, 4242);
                assert_eq!(target_host_facts, None);
                assert_eq!(matches_installed_binary, Some(true));
                assert!(matches_requested_profile);
                assert!(matches_current_config);
            }
            other => panic!("expected present launch identity, got {other:?}"),
        }
    }

    #[test]
    fn launch_identity_status_reports_divergence_against_other_profile_and_config() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let identity = LaunchIdentity {
            build_head_sha: Some(well_formed_git_sha('a')),
            profile: "recorded-profile".to_string(),
            config_bundle_checksum: "recorded-checksum".to_string(),
            launched_at_unix_secs: 7,
            pid: 4242,
            target_host_facts: None,
        };
        write_launch_identity(temp.path(), &identity).expect("launch identity should write");

        let status = launch_identity_status(
            temp.path(),
            Some(well_formed_git_sha('b').as_str()),
            "requested-profile",
            "current-checksum",
        );

        match status {
            LaunchIdentityStatus::Present {
                matches_installed_binary,
                matches_requested_profile,
                matches_current_config,
                ..
            } => {
                assert_eq!(matches_installed_binary, Some(false));
                assert!(!matches_requested_profile);
                assert!(!matches_current_config);
            }
            other => panic!("expected present launch identity, got {other:?}"),
        }
    }

    #[test]
    fn launch_identity_status_reports_absent_when_artifact_missing() {
        let temp = tempfile::tempdir().expect("tempdir should create");

        let status = launch_identity_status(temp.path(), None, "example", "checksum-abc");

        assert!(matches!(status, LaunchIdentityStatus::Absent));
    }

    #[test]
    fn build_launch_identity_maps_every_field_from_inputs() {
        // Synthetic host-fact tokens (not real venue/region values) keep this
        // test clear of the runtime-literal fence while still exercising the
        // `Some(facts)` path through the builder.
        let facts = ObservedHostFacts {
            region: Some("test-region".to_string()),
            availability_zone: None,
            instance_id: Some("test-instance".to_string()),
        };

        let identity = build_launch_identity(
            "build-profile",
            "build-checksum",
            7,
            1_700_000_000,
            Some(facts.clone()),
        );

        assert_eq!(identity.profile, "build-profile");
        assert_eq!(identity.config_bundle_checksum, "build-checksum");
        assert_eq!(identity.pid, 7);
        assert_eq!(identity.launched_at_unix_secs, 1_700_000_000);
        assert_eq!(
            identity.build_head_sha,
            current_build_head_sha().map(str::to_owned)
        );
        assert_eq!(identity.target_host_facts, Some(facts));
    }

    #[test]
    fn launch_identity_status_reports_unreadable_for_malformed_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        // An unknown-field document parses against `serde(deny_unknown_fields)`
        // as an error, surfacing as the advisory `unreadable` state.
        fs::write(
            launch_identity_path(temp.path()),
            "{ \"unexpected_field\": true }\n",
        )
        .expect("malformed launch identity should write");

        let status = launch_identity_status(temp.path(), None, "example", "checksum-abc");

        assert!(matches!(status, LaunchIdentityStatus::Unreadable { .. }));
    }

    fn write_deploy_target(config_root: &Path, region: &str, instance_id: &str) {
        fs::write(
            config_root.join("deploy.toml"),
            format!("[target]\nregion = \"{region}\"\ninstance_id = \"{instance_id}\"\n"),
        )
        .expect("deploy.toml fixture should write");
    }

    #[test]
    fn deploy_target_status_reports_no_target_configured_without_deploy_toml() {
        let temp = tempfile::tempdir().expect("tempdir should create");

        let status = deploy_target_status(temp.path(), &FakeHostFactsSource::erroring());

        assert!(matches!(status, DeployTargetStatus::NoTargetConfigured));
    }

    #[test]
    fn deploy_target_status_reports_matched_when_host_facts_agree() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        write_deploy_target(temp.path(), "region-x", "instance-target");
        let source = FakeHostFactsSource::facts(ObservedHostFacts {
            region: Some("region-x".to_string()),
            availability_zone: None,
            instance_id: Some("instance-target".to_string()),
        });

        let status = deploy_target_status(temp.path(), &source);

        assert!(matches!(status, DeployTargetStatus::Matched));
    }

    #[test]
    fn deploy_target_status_reports_mismatched_field_when_host_facts_differ() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        write_deploy_target(temp.path(), "region-x", "instance-target");
        let source = FakeHostFactsSource::facts(ObservedHostFacts {
            region: Some("region-x".to_string()),
            availability_zone: None,
            instance_id: Some("instance-other".to_string()),
        });

        let status = deploy_target_status(temp.path(), &source);

        match status {
            DeployTargetStatus::Mismatched { field_mismatches } => {
                assert_eq!(field_mismatches.len(), 1);
                let mismatch = &field_mismatches[0];
                assert_eq!(mismatch.field, "instance_id");
                assert_eq!(mismatch.configured, "instance-target");
                assert_eq!(mismatch.observed.as_deref(), Some("instance-other"));
            }
            other => panic!("expected mismatched deploy target, got {other:?}"),
        }
    }

    #[test]
    fn deploy_target_status_reports_host_facts_unavailable_on_observe_error() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        write_deploy_target(temp.path(), "region-x", "instance-target");

        let status = deploy_target_status(temp.path(), &FakeHostFactsSource::erroring());

        assert!(matches!(
            status,
            DeployTargetStatus::HostFactsUnavailable { .. }
        ));
    }

    /// Build a `Present` launch identity with every field explicit so the advisory
    /// truth table exercises each match flag independently.
    fn present_launch_identity(
        matches_installed_binary: Option<bool>,
        matches_requested_profile: bool,
        matches_current_config: bool,
    ) -> LaunchIdentityStatus {
        LaunchIdentityStatus::Present {
            build_head_sha: Some(well_formed_git_sha('a')),
            profile: "example".to_string(),
            config_bundle_checksum: "checksum-abc".to_string(),
            launched_at_unix_secs: 1,
            pid: 4242,
            target_host_facts: None,
            matches_installed_binary,
            matches_requested_profile,
            matches_current_config,
        }
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_intended_absent() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                None,
                Some(sha.as_str()),
                &LaunchIdentityStatus::Absent,
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_installed_absent() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                None,
                &LaunchIdentityStatus::Absent,
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_intended_malformed() {
        assert_eq!(
            derive_state_advisory(
                Some("not-a-sha"),
                Some(well_formed_git_sha('a').as_str()),
                &LaunchIdentityStatus::Absent,
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_deploy_needed_when_installed_differs() {
        assert_eq!(
            derive_state_advisory(
                Some(well_formed_git_sha('a').as_str()),
                Some(well_formed_git_sha('b').as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::DeployNeeded
        );
    }

    #[test]
    fn derive_state_advisory_reports_no_op_when_installed_matches_and_identity_live() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::NoOp
        );
    }

    #[test]
    fn derive_state_advisory_reports_launch_needed_when_installed_matches_but_identity_absent() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &LaunchIdentityStatus::Absent,
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::LaunchNeeded
        );
    }

    #[test]
    fn derive_state_advisory_reports_launch_needed_when_identity_diverges() {
        let sha = well_formed_git_sha('a');
        // Right binary installed, but the launch identity disagrees on the binary,
        // the profile, or cannot prove the binary — each forces `launch-needed`.
        for identity in [
            present_launch_identity(Some(false), true, true),
            present_launch_identity(Some(true), false, true),
            present_launch_identity(None, true, true),
        ] {
            assert_eq!(
                derive_state_advisory(
                    Some(sha.as_str()),
                    Some(sha.as_str()),
                    &identity,
                    &DeployTargetStatus::NoTargetConfigured,
                ),
                StateAdvisory::LaunchNeeded
            );
        }
    }

    #[test]
    fn derive_state_advisory_reports_launch_needed_when_config_diverges() {
        // Same binary and profile, but the recorded launch ran against a
        // different config bundle (matches_current_config: false). The advisory
        // must NOT collapse to no-op — a new config bundle needs a relaunch — so
        // the `IntendedShaStatus::Match` arm must inspect config drift, not
        // swallow it under `..`.
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &present_launch_identity(Some(true), true, false),
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::LaunchNeeded
        );
    }

    #[test]
    fn derive_state_advisory_reports_no_op_when_host_matches_configured_target() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::Matched,
            ),
            StateAdvisory::NoOp
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_deploy_target_mismatched() {
        // A live, matching launch identity must NOT collapse to no-op when the
        // running host is not the configured target — otherwise `ops status`
        // would advise "nothing to do" on the wrong host.
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::Mismatched {
                    field_mismatches: Vec::new(),
                },
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_host_facts_unavailable() {
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::HostFactsUnavailable {
                    detail: "imds unreachable".to_string(),
                },
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_config_unavailable() {
        // Config could not be verified, so the launch identity was skipped; the
        // advisory must say `unknown`, not `launch-needed` (a relaunch would
        // fail at config verification anyway).
        let sha = well_formed_git_sha('a');
        assert_eq!(
            derive_state_advisory(
                Some(sha.as_str()),
                Some(sha.as_str()),
                &LaunchIdentityStatus::SkippedConfigUnavailable,
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_sha_mismatch_on_mismatched_host() {
        // SHA mismatch on the WRONG host must NOT advise `deploy-needed`.
        assert_eq!(
            derive_state_advisory(
                Some(well_formed_git_sha('a').as_str()),
                Some(well_formed_git_sha('b').as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::Mismatched {
                    field_mismatches: Vec::new()
                },
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_sha_mismatch_and_host_facts_unavailable() {
        assert_eq!(
            derive_state_advisory(
                Some(well_formed_git_sha('a').as_str()),
                Some(well_formed_git_sha('b').as_str()),
                &present_launch_identity(Some(true), true, true),
                &DeployTargetStatus::HostFactsUnavailable {
                    detail: "imds unreachable".to_string()
                },
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn derive_state_advisory_reports_unknown_when_sha_mismatch_and_config_unavailable() {
        assert_eq!(
            derive_state_advisory(
                Some(well_formed_git_sha('a').as_str()),
                Some(well_formed_git_sha('b').as_str()),
                &LaunchIdentityStatus::SkippedConfigUnavailable,
                &DeployTargetStatus::NoTargetConfigured,
            ),
            StateAdvisory::Unknown
        );
    }

    #[test]
    fn state_advisory_serializes_to_kebab_case_variants() {
        assert_eq!(
            serde_json::to_string(&StateAdvisory::NoOp).expect("serialize no-op"),
            "\"no-op\""
        );
        assert_eq!(
            serde_json::to_string(&StateAdvisory::LaunchNeeded).expect("serialize launch-needed"),
            "\"launch-needed\""
        );
        assert_eq!(
            serde_json::to_string(&StateAdvisory::DeployNeeded).expect("serialize deploy-needed"),
            "\"deploy-needed\""
        );
        assert_eq!(
            serde_json::to_string(&StateAdvisory::Unknown).expect("serialize unknown"),
            "\"unknown\""
        );
    }
}
