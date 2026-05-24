use clap::Parser;
use std::path::PathBuf;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
    bolt_v3_operator_artifacts::{
        PreRunStateSourceCollectorInputs, WrittenOperatorArtifact,
        assemble_operator_packet_from_static_manifest, compute_operator_approval_envelope_sha256,
        verify_final_operator_packet, write_abort_plan_artifact_from_source_bundle_file,
        write_abort_plan_artifact_from_source_collectors,
        write_entry_decision_evidence_from_source_file,
        write_market_selection_source_artifact_from_decision_evidence_and_instrument_source_file,
        write_pre_run_state_artifact_from_source_bundle_file,
        write_pre_run_state_artifact_from_source_collectors,
        write_static_artifacts_manifest_from_operator_evidence, write_static_operator_artifacts,
        write_strategy_input_evidence_artifact_from_decision_evidence_file,
    },
    bolt_v3_providers::binding_for_provider_key,
    bolt_v3_secrets::{check_no_forbidden_credential_env_vars, resolve_bolt_v3_secrets},
    secrets::SsmResolverSession,
};

#[derive(Parser)]
#[command(name = "bolt-v2")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// Clap keeps parsed argument fields directly in enum variants; these startup-only
// command values are not on a hot path.
#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand)]
enum Command {
    Run {
        #[arg(short, long)]
        config: PathBuf,
    },
    NoSubmitReadiness {
        #[arg(short, long)]
        config: PathBuf,
    },
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
    OperatorArtifacts {
        #[command(subcommand)]
        command: OperatorArtifactsCommand,
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

// The collector commands intentionally expose each required source path as a
// first-class CLI argument so operators can audit the binding explicitly.
#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand)]
enum OperatorArtifactsCommand {
    GenerateStatic {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
    },
    AssembleFinal {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        static_manifest: PathBuf,
        #[arg(long)]
        operator_packet: PathBuf,
    },
    WriteManifestFromOperatorEvidence {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ComputeApprovalEnvelopeSha256 {
        #[arg(short, long)]
        config: PathBuf,
    },
    GeneratePreRunStateFromSourceBundle {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        source_bundle: PathBuf,
        #[arg(long)]
        max_source_bundle_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    GeneratePreRunStateFromSourceCollectors {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        cargo_toml: PathBuf,
        #[arg(long)]
        cargo_lock: PathBuf,
        #[arg(long)]
        clob_signing_source: PathBuf,
        #[arg(long)]
        host_clock_source: PathBuf,
        #[arg(long)]
        venue_account_state_source: PathBuf,
        #[arg(long)]
        funding_margin_source: PathBuf,
        #[arg(long)]
        strategy_input_evidence: PathBuf,
        #[arg(long)]
        strategy_input_evidence_sha256: String,
        #[arg(long)]
        expected_price_to_beat_source: String,
        #[arg(long)]
        single_runner_lock: PathBuf,
        #[arg(long)]
        egress_identity_source: PathBuf,
        #[arg(long)]
        clob_v2_adapter_signing_source: PathBuf,
        #[arg(long)]
        clob_v2_collateral_accounting_source: PathBuf,
        #[arg(long)]
        clob_v2_fee_behavior_source: PathBuf,
        #[arg(long)]
        max_source_bytes: u64,
        #[arg(long)]
        max_host_clock_skew_millis: u64,
        #[arg(long)]
        max_single_runner_lock_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    GenerateAbortPlanFromSourceBundle {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        source_bundle: PathBuf,
        #[arg(long)]
        max_source_bundle_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    GenerateAbortPlanFromSourceCollectors {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        strategy_source: PathBuf,
        #[arg(long)]
        submit_admission_source: PathBuf,
        #[arg(long)]
        max_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    GenerateEntryDecisionEvidenceFromSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        decision_source: PathBuf,
        #[arg(long)]
        max_decision_source_bytes: u64,
        #[arg(long)]
        instrument_source: PathBuf,
        #[arg(long)]
        max_instrument_source_bytes: u64,
        #[arg(long)]
        max_decision_evidence_bytes: u64,
    },
    GenerateStrategyInputFromDecisionEvidence {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        decision_evidence: PathBuf,
        #[arg(long)]
        max_decision_evidence_bytes: u64,
        #[arg(long)]
        market_selection_source: PathBuf,
        #[arg(long)]
        market_selection_source_sha256: String,
        #[arg(long)]
        candidate_market_start_timestamp_ms: Vec<u64>,
        #[arg(long)]
        output: PathBuf,
    },
    GenerateMarketSelectionFromDecisionEvidence {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        decision_evidence: PathBuf,
        #[arg(long)]
        max_decision_evidence_bytes: u64,
        #[arg(long)]
        instrument_source: PathBuf,
        #[arg(long)]
        max_instrument_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    VerifyFinal {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        operator_packet: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Secrets { command } => run_secrets_command(command),
        Command::OperatorArtifacts { command } => run_operator_artifacts_command(command),
        Command::NoSubmitReadiness { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            let report = run_bolt_v3_no_submit_readiness(&loaded)?;
            report.write_configured_redacted_json(&loaded)?;
            println!("bolt-v3 no-submit readiness report written");
            Ok(())
        }
        Command::Run { config } => {
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
    }
}

fn run_operator_artifacts_command(
    command: OperatorArtifactsCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        OperatorArtifactsCommand::GenerateStatic {
            config,
            output_dir,
            strategy_instance_id,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let outcome =
                write_static_operator_artifacts(&loaded, &strategy_instance_id, &output_dir)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&outcome.command_summary)?
            );
            if !outcome.blockers.is_empty() {
                return Err(std::io::Error::other(outcome.blockers.join("; ")).into());
            }
            Ok(())
        }
        OperatorArtifactsCommand::AssembleFinal {
            config,
            static_manifest,
            operator_packet,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let outcome = assemble_operator_packet_from_static_manifest(
                &loaded,
                &static_manifest,
                &operator_packet,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "approval_envelope": written_operator_artifact_json(&outcome.approval_envelope),
                    "operator_packet": written_operator_artifact_json(&outcome.operator_packet),
                    "static_manifest": written_operator_artifact_json(&outcome.static_manifest),
                }))?
            );
            Ok(())
        }
        OperatorArtifactsCommand::WriteManifestFromOperatorEvidence { config, output } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_static_artifacts_manifest_from_operator_evidence(&loaded, &output)?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::ComputeApprovalEnvelopeSha256 { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            let sha256 = compute_operator_approval_envelope_sha256(&loaded)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "sha256": sha256 }))?
            );
            Ok(())
        }
        OperatorArtifactsCommand::GeneratePreRunStateFromSourceBundle {
            config,
            strategy_instance_id,
            source_bundle,
            max_source_bundle_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_pre_run_state_artifact_from_source_bundle_file(
                &loaded,
                &strategy_instance_id,
                &source_bundle,
                max_source_bundle_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::GeneratePreRunStateFromSourceCollectors {
            config,
            strategy_instance_id,
            cargo_toml,
            cargo_lock,
            clob_signing_source,
            host_clock_source,
            venue_account_state_source,
            funding_margin_source,
            strategy_input_evidence,
            strategy_input_evidence_sha256,
            expected_price_to_beat_source,
            single_runner_lock,
            egress_identity_source,
            clob_v2_adapter_signing_source,
            clob_v2_collateral_accounting_source,
            clob_v2_fee_behavior_source,
            max_source_bytes,
            max_host_clock_skew_millis,
            max_single_runner_lock_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let inputs = PreRunStateSourceCollectorInputs {
                cargo_toml_path: &cargo_toml,
                cargo_lock_path: &cargo_lock,
                clob_signing_source_path: &clob_signing_source,
                host_clock_source_path: &host_clock_source,
                venue_account_state_source_path: &venue_account_state_source,
                funding_margin_source_path: &funding_margin_source,
                strategy_input_evidence_path: &strategy_input_evidence,
                strategy_input_evidence_sha256: &strategy_input_evidence_sha256,
                expected_price_to_beat_source: &expected_price_to_beat_source,
                single_runner_lock_path: &single_runner_lock,
                egress_identity_source_path: &egress_identity_source,
                clob_v2_adapter_signing_source_path: &clob_v2_adapter_signing_source,
                clob_v2_collateral_accounting_source_path: &clob_v2_collateral_accounting_source,
                clob_v2_fee_behavior_source_path: &clob_v2_fee_behavior_source,
                max_source_bytes,
                max_host_clock_skew_millis,
                max_single_runner_lock_bytes,
            };
            let written = write_pre_run_state_artifact_from_source_collectors(
                &loaded,
                &strategy_instance_id,
                inputs,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::GenerateAbortPlanFromSourceBundle {
            config,
            strategy_instance_id,
            source_bundle,
            max_source_bundle_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_abort_plan_artifact_from_source_bundle_file(
                &loaded,
                &strategy_instance_id,
                &source_bundle,
                max_source_bundle_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::GenerateStrategyInputFromDecisionEvidence {
            config,
            strategy_instance_id,
            decision_evidence,
            max_decision_evidence_bytes,
            market_selection_source,
            market_selection_source_sha256,
            candidate_market_start_timestamp_ms,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let market_selection_source_ref = WrittenOperatorArtifact {
                path: market_selection_source,
                sha256: market_selection_source_sha256,
            };
            let written = write_strategy_input_evidence_artifact_from_decision_evidence_file(
                &loaded,
                &strategy_instance_id,
                &decision_evidence,
                max_decision_evidence_bytes,
                &market_selection_source_ref,
                &candidate_market_start_timestamp_ms,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::GenerateAbortPlanFromSourceCollectors {
            config,
            strategy_instance_id,
            strategy_source,
            submit_admission_source,
            max_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_abort_plan_artifact_from_source_collectors(
                &loaded,
                &strategy_instance_id,
                &strategy_source,
                &submit_admission_source,
                max_source_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::GenerateEntryDecisionEvidenceFromSource {
            config,
            strategy_instance_id,
            decision_source,
            max_decision_source_bytes,
            instrument_source,
            max_instrument_source_bytes,
            max_decision_evidence_bytes,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_entry_decision_evidence_from_source_file(
                &loaded,
                &strategy_instance_id,
                &decision_source,
                max_decision_source_bytes,
                &instrument_source,
                max_instrument_source_bytes,
                max_decision_evidence_bytes,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::GenerateMarketSelectionFromDecisionEvidence {
            config,
            strategy_instance_id,
            decision_evidence,
            max_decision_evidence_bytes,
            instrument_source,
            max_instrument_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_market_selection_source_artifact_from_decision_evidence_and_instrument_source_file(
                &loaded,
                &strategy_instance_id,
                &decision_evidence,
                max_decision_evidence_bytes,
                &instrument_source,
                max_instrument_source_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::VerifyFinal {
            config,
            operator_packet,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let outcome = verify_final_operator_packet(&loaded, &operator_packet)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&outcome.redacted_summary())?
            );
            Ok(())
        }
    }
}

fn print_written_operator_artifact(
    written: &WrittenOperatorArtifact,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&written_operator_artifact_json(written))?
    );
    Ok(())
}

fn written_operator_artifact_json(written: &WrittenOperatorArtifact) -> serde_json::Value {
    serde_json::json!({
        "path": &written.path,
        "sha256": &written.sha256,
    })
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
