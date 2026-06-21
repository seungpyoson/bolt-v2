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
    bolt_v3_kill_switch_store::KillSwitchStore,
    bolt_v3_live_node::{
        build_bolt_v3_live_node, build_bolt_v3_strategy_free_data_client_probe_live_node,
        current_build_head_sha, run_bolt_v3_data_client_census, run_bolt_v3_data_client_probe,
        run_bolt_v3_live_node,
    },
    bolt_v3_operator_artifacts::WrittenOperatorArtifact,
    bolt_v3_prod_profile::{
        GENERATED_MARKER_PREFIX, GENERATOR_FORMAT_VERSION, LIVE_CONFIG_FILE_NAME,
        ProductionInvariants, confirm_production_invariants, generate_live_config,
        live_config_path, verify_live_config,
    },
    bolt_v3_providers::{
        ClobV2BalanceAllowanceCacheSync, ClobV2BalanceAllowanceCacheSyncRequest,
        ProviderArtifactReference, ProviderLiveSubmitApprovalContext,
        ProviderProductSubmitProofArtifactRequest, binding_for_provider_key,
        reference_live_probe::run_reference_live_probe,
        sync_clob_v2_balance_allowance_cache_from_configured_account,
    },
    bolt_v3_reference_price_health::{
        ReferenceCurrentPriceHealthReport, prepare_reference_current_price_health_run,
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
const LIVE_LOCAL_CONFIG_FILE_NAME: &str = "live.local.toml";
const BOLT_LIVE_PROFILE_ENV: &str = "BOLT_LIVE_PROFILE";
const CLOB_V2_CACHE_SYNC_EXECUTION_CLIENT_OUTPUT_FIELD: &str = "execution_client_id";
const CLOB_V2_CACHE_SYNC_REQUEST_PATH_OUTPUT_FIELD: &str = "request_path";
const CLOB_V2_CACHE_SYNC_BASE_URL_HTTP_SHA256_OUTPUT_FIELD: &str = "base_url_http_sha256";
const KILL_SWITCH_STORE_INIT_COMPLETED_OUTPUT_FIELD: &str = "kill_switch_store_init_completed";
const KILL_SWITCH_STORE_INIT_STATE_PATH_OUTPUT_FIELD: &str = "state_path";
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
        command: OpsCommand,
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
    InitKillSwitchStore {
        #[arg(short, long)]
        config: PathBuf,
    },
    ReferenceLiveProbe {
        #[arg(short, long)]
        config: PathBuf,
    },
    ReferenceCurrentPriceHealth {
        #[arg(short, long)]
        config: PathBuf,
    },
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
        Command::Ops { command } => run_ops_command(command),
        Command::ProviderArtifacts { command } => run_provider_artifacts_command(*command),
    }
}

fn run_live_node(config: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    verify_runtime_live_config_source(&config)?;
    let loaded = load_bolt_v3_config(&config)?;
    confirm_production_invariants(&loaded)?;
    run_loaded_prestart_check(&loaded, None)?;
    start_loaded_node(loaded)
}

fn start_loaded_node(loaded: LoadedBoltV3Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut node = build_bolt_v3_live_node(&loaded)?;
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

fn verify_runtime_live_config_source(config: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = config.file_name().and_then(|name| name.to_str());
    if file_name == Some(LIVE_LOCAL_CONFIG_FILE_NAME) {
        return Err(format!(
            "runtime config `{}` is the legacy live.local.toml path; run \
             `bolt-v2 ops generate-live-config` from a reviewed profile ID instead",
            config.display()
        )
        .into());
    }
    if file_name != Some(LIVE_CONFIG_FILE_NAME) {
        return Err(format!(
            "runtime config `{}` is not the generated live.toml path; run \
             `bolt-v2 ops generate-live-config` from a reviewed profile ID and start from live.toml",
            config.display()
        )
        .into());
    }
    let text = std::fs::read_to_string(config).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "runtime config `{}` is not readable before live start: {source}",
                config.display()
            ),
        )
    })?;
    if !text.starts_with(GENERATED_MARKER_PREFIX) {
        return Err(format!(
            "runtime config `{}` is named live.toml but is not a generated live config; \
             run `bolt-v2 ops generate-live-config` from a reviewed profile ID instead of \
             hand-editing or using live.local.toml",
            config.display()
        )
        .into());
    }
    let profile = std::env::var(BOLT_LIVE_PROFILE_ENV).map_err(|_| {
        format!(
            "{BOLT_LIVE_PROFILE_ENV} must be set to the reviewed profile ID before running \
             generated live.toml"
        )
    })?;
    if profile.is_empty() {
        return Err(format!(
            "{BOLT_LIVE_PROFILE_ENV} must be non-empty before running generated live.toml"
        )
        .into());
    }
    let config_root = config.parent().ok_or_else(|| {
        format!(
            "runtime config `{}` must be inside the deployed config root",
            config.display()
        )
    })?;
    verify_live_config(config_root, &profile)?;
    Ok(())
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
        OpsCommand::InitKillSwitchStore { config } => run_init_kill_switch_store(&config),
        OpsCommand::ReferenceLiveProbe { config } => run_reference_live_probe_command(&config),
        OpsCommand::ReferenceCurrentPriceHealth { config } => {
            run_reference_current_price_health_command(&config)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum OpsLaunchStage {
    VerifyConfig,
    SecretsCheck,
    SecretsResolve,
    PrestartCheck,
    ReferenceCurrentPriceHealth,
    Start,
}

const OPS_LAUNCH_STAGE_CHAIN: &[OpsLaunchStage] = &[
    OpsLaunchStage::VerifyConfig,
    // Slice 1b target-verifier insertion point: run the target verifier here,
    // after config identity is proven and before secrets or runtime side effects.
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
    live_config: PathBuf,
    loaded: Option<LoadedBoltV3Config>,
}

impl OpsLaunchContext {
    fn new(profile: String, config_root: PathBuf) -> Self {
        let live_config = live_config_path(&config_root);
        Self {
            profile,
            config_root,
            live_config,
            loaded: None,
        }
    }

    fn loaded(&self) -> Result<&LoadedBoltV3Config, Box<dyn std::error::Error>> {
        self.loaded.as_ref().ok_or_else(|| {
            "ops launch config must be loaded by verify-config before this stage".into()
        })
    }
}

fn run_ops_launch(profile: String, config_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut context = OpsLaunchContext::new(profile, config_root);
    run_ops_launch_chain_with(|stage| run_ops_launch_stage(stage, &mut context))
}

fn run_ops_launch_chain_with<F>(mut run_stage: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(OpsLaunchStage) -> Result<(), Box<dyn std::error::Error>>,
{
    let mut last_completed_stage = None;
    for &stage in OPS_LAUNCH_STAGE_CHAIN {
        if stage == OpsLaunchStage::Start {
            emit_ops_launch_stage_log(
                stage,
                OpsLaunchStageStatus::Entering,
                last_completed_stage,
                None,
            )?;
        }
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
            context.live_config = verification.deployed_path;
            context.loaded = Some(load_bolt_v3_config(&context.live_config)?);
            Ok(())
        }
        OpsLaunchStage::SecretsCheck => run_loaded_secrets_check(context.loaded()?).map(|_| ()),
        OpsLaunchStage::SecretsResolve => run_loaded_secrets_resolve(context.loaded()?).map(|_| ()),
        OpsLaunchStage::PrestartCheck => run_loaded_prestart_check(context.loaded()?, None),
        OpsLaunchStage::ReferenceCurrentPriceHealth => {
            run_loaded_reference_current_price_health(context.loaded()?)
        }
        OpsLaunchStage::Start => {
            let loaded = context.loaded.take().ok_or_else(
                || "ops launch start stage requires a loaded config from verify-config",
            )?;
            start_loaded_node(loaded)
        }
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

fn run_loaded_reference_current_price_health(
    loaded: &LoadedBoltV3Config,
) -> Result<(), Box<dyn std::error::Error>> {
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let health_run = prepare_reference_current_price_health_run(loaded)?;
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

fn print_reference_current_price_health_report(
    report: &ReferenceCurrentPriceHealthReport,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, report)?;
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
    use std::fs;

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

        match cli.command {
            Command::Ops {
                command: OpsCommand::DataClientProbe { config, client_key },
            } => {
                assert_eq!(config, PathBuf::from("config/root.toml"));
                assert_eq!(client_key, "bybit_data");
            }
            _ => panic!("expected ops data-client-probe command"),
        }
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

        match cli.command {
            Command::Ops {
                command:
                    OpsCommand::Launch {
                        profile,
                        config_root,
                    },
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
                OpsLaunchStage::SecretsCheck,
                OpsLaunchStage::SecretsResolve,
            ]
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

        match cli.command {
            Command::Ops {
                command:
                    OpsCommand::GenerateLiveConfig {
                        profile,
                        config_root,
                    },
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

        match cli.command {
            Command::Ops {
                command:
                    OpsCommand::VerifyLiveConfig {
                        profile,
                        config_root,
                    },
            } => {
                assert_eq!(profile, "example");
                assert_eq!(config_root, PathBuf::from("/opt/bolt-v2/config"));
            }
            _ => panic!("expected ops verify-live-config command"),
        }
    }

    #[test]
    fn live_config_run_guard_requires_verified_generated_live_toml() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bolt-v2-live-marker-test-{suffix}"));
        fs::create_dir_all(&dir).expect("test temp dir should create");
        let live = dir.join("live.toml");
        fs::write(&live, "[runtime]\n").expect("hand-edited live.toml should write");

        let error = verify_runtime_live_config_source(&live)
            .expect_err("live.toml without generated marker must be rejected");
        assert!(
            error.to_string().contains("is not a generated live config"),
            "error should explain the generated-marker requirement, got: {error}"
        );

        fs::write(
            &live,
            format!("{GENERATED_MARKER_PREFIX}{GENERATOR_FORMAT_VERSION}\n"),
        )
        .expect("generated marker should write");
        if std::env::var_os(BOLT_LIVE_PROFILE_ENV).is_none() {
            let error = verify_runtime_live_config_source(&live)
                .expect_err("generated live.toml without a profile ID must be rejected");
            assert!(
                error.to_string().contains(BOLT_LIVE_PROFILE_ENV),
                "error should require the selected profile ID, got: {error}"
            );
        }

        let root = dir.join("root.toml");
        fs::write(&root, "[runtime]\n").expect("non-live config should write");
        let error = verify_runtime_live_config_source(&root)
            .expect_err("run must reject non-live.toml runtime config paths");
        assert!(
            error
                .to_string()
                .contains("not the generated live.toml path"),
            "error should require generated live.toml, got: {error}"
        );

        let live_local = dir.join("live.local.toml");
        fs::write(&live_local, "[runtime]\n").expect("legacy live.local.toml should write");
        let error = verify_runtime_live_config_source(&live_local)
            .expect_err("live.local.toml must be rejected as a runtime config source");
        assert!(
            error.to_string().contains("legacy live.local.toml"),
            "error should identify live.local.toml as legacy drift, got: {error}"
        );

        fs::remove_dir_all(&dir).expect("test temp dir should clean up");
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

        match cli.command {
            Command::Ops {
                command: OpsCommand::DataClientCensus { config, client_key },
            } => {
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

        match cli.command {
            Command::Ops {
                command: OpsCommand::ReferenceLiveProbe { config },
            } => {
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

        match cli.command {
            Command::Ops {
                command: OpsCommand::ReferenceCurrentPriceHealth { config },
            } => {
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
}
