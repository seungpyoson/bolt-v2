use clap::Parser;
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::{ffi::CString, os::unix::ffi::OsStrExt};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, current_build_head_sha, run_bolt_v3_live_node},
    bolt_v3_operator_artifacts::WrittenOperatorArtifact,
    bolt_v3_providers::{
        ClobV2BalanceAllowanceCacheSync, ClobV2BalanceAllowanceCacheSyncRequest,
        ProviderArtifactReference, ProviderLiveSubmitApprovalContext,
        ProviderProductSubmitProofArtifactRequest, binding_for_provider_key,
        sync_clob_v2_balance_allowance_cache_from_configured_account,
    },
    bolt_v3_secrets::{
        check_no_forbidden_credential_env_vars, resolve_bolt_v3_client_secrets,
        resolve_bolt_v3_secrets,
    },
    secrets::SsmResolverSession,
};

const CLOB_V2_CACHE_SYNC_COMPLETED_OUTPUT_FIELD: &str =
    "clob_v2_balance_allowance_cache_sync_completed";
const CLOB_V2_CACHE_SYNC_EXECUTION_CLIENT_OUTPUT_FIELD: &str = "execution_client_id";
const CLOB_V2_CACHE_SYNC_REQUEST_PATH_OUTPUT_FIELD: &str = "request_path";
const CLOB_V2_CACHE_SYNC_BASE_URL_HTTP_SHA256_OUTPUT_FIELD: &str = "base_url_http_sha256";

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
    PrestartCheck {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        required_catalog_prefix: PathBuf,
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
        expires_at_unix_seconds: u64,
    },
    PreflightLiveSubmitArming {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
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
    let loaded = load_bolt_v3_config(&config)?;
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

fn run_ops_command(command: OpsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        OpsCommand::PrestartCheck {
            config,
            required_catalog_prefix,
        } => run_prestart_check(&config, &required_catalog_prefix),
    }
}

fn run_prestart_check(
    config: &Path,
    required_catalog_prefix: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if !required_catalog_prefix.is_absolute() {
        return Err(format!(
            "--required-catalog-prefix must be an absolute path: `{}`",
            required_catalog_prefix.display()
        )
        .into());
    }
    let loaded = load_bolt_v3_config(config)?;
    let catalog_directory = Path::new(&loaded.root.persistence.catalog_directory);
    if !catalog_directory.starts_with(required_catalog_prefix) {
        return Err(format!(
            "persistence.catalog_directory `{}` must be under `{}` for this service",
            catalog_directory.display(),
            required_catalog_prefix.display()
        )
        .into());
    }
    let min_free_bytes = loaded.root.persistence.min_free_bytes.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "persistence.min_free_bytes must be configured for ops prestart-check",
        )
    })?;
    let catalog_metadata = std::fs::metadata(catalog_directory).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "persistence.catalog_directory `{}` is not readable before service start: {source}",
                catalog_directory.display()
            ),
        )
    })?;
    if !catalog_metadata.is_dir() {
        return Err(format!(
            "persistence.catalog_directory `{}` must be a directory before service start",
            catalog_directory.display()
        )
        .into());
    }
    let available_bytes = filesystem_available_bytes(catalog_directory)?;
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
                    now_unix_seconds,
                    build_head_sha,
                },
                expires_at_unix_seconds,
            )?;
            print_written_artifact(&written)
        }
        ProviderArtifactsCommand::PreflightLiveSubmitArming { config, client_key } => {
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
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            for (client_key, client) in &loaded.root.clients {
                if client.secrets.is_some() {
                    let binding =
                        binding_for_provider_key(client.venue.as_str()).ok_or_else(|| {
                            format!(
                                "clients.{client_key}.venue `{}` is not supported by this build",
                                client.venue.as_str()
                            )
                        })?;
                    println!(
                        "clients.{client_key}: required secret fields present ({})",
                        binding.secret_field_names.join(", ")
                    );
                }
            }
            Ok(())
        }
        SecretsCommand::Resolve { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved = resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?;
            for client_key in resolved.clients.keys() {
                println!("clients.{client_key}: secrets resolved successfully");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prestart_check_rejects_catalog_outside_required_prefix_before_disk_probe() {
        let config = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/root.toml");
        let error = run_prestart_check(&config, Path::new("/tmp/bolt-v2"))
            .expect_err("wrong catalog prefix should fail prestart");
        let message = error.to_string();

        assert!(message.contains("persistence.catalog_directory"));
        assert!(message.contains("must be under"));
    }

    #[test]
    fn prestart_check_requires_configured_min_free_bytes() {
        let config =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bolt_v3/root.toml");
        let error = run_prestart_check(&config, Path::new("/var/lib/bolt"))
            .expect_err("missing free-space floor should fail prestart");
        let message = error.to_string();

        assert!(message.contains("persistence.min_free_bytes"));
        assert!(message.contains("ops prestart-check"));
    }
}
