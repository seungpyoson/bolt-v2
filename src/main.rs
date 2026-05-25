use clap::Parser;
use std::{collections::BTreeMap, path::PathBuf};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
    bolt_v3_operator_artifacts::{
        EntryDecisionProofSourceMaterializationRequest, EntryDecisionRealizedVolatilityProofInput,
        EntryDecisionReferenceQuoteProofInput, EntryDecisionSourceCollectionRequest,
        FinalOperatorPacketVerificationScope, OperatorEvidenceJsonBuildInputs,
        PreRunStateSourceCollectorInputs, WrittenOperatorArtifact,
        assemble_operator_packet_from_static_manifest,
        collect_entry_decision_source_inputs_from_configured_provider,
        compute_operator_approval_envelope_sha256,
        update_live_canary_operator_evidence_toml_from_json_file,
        verify_final_operator_packet_with_scope, write_abort_plan_artifact_from_source_bundle_file,
        write_abort_plan_artifact_from_source_collectors, write_base_static_operator_artifacts,
        write_entry_decision_evidence_from_source_file, write_entry_decision_proof_source_files,
        write_market_selection_source_artifact_from_decision_evidence_and_instrument_source_file,
        write_operator_evidence_json_from_artifact_paths,
        write_pre_run_state_artifact_from_source_bundle_file,
        write_pre_run_state_artifact_from_source_collectors,
        write_static_artifacts_manifest_from_operator_evidence, write_static_operator_artifacts,
        write_strategy_input_evidence_artifact_from_decision_evidence_file,
    },
    bolt_v3_providers::binding_for_provider_key,
    bolt_v3_secrets::{check_no_forbidden_credential_env_vars, resolve_bolt_v3_secrets},
    secrets::SsmResolverSession,
};

const ENTRY_DECISION_SOURCE_OUTPUT_FIELD: &str = "decision_source";
const ENTRY_DECISION_INSTRUMENT_SOURCE_OUTPUT_FIELD: &str = "instrument_source";
const ENTRY_DECISION_PRICE_SOURCE_OUTPUT_FIELD: &str = "price_to_beat_source";
const ENTRY_DECISION_REFERENCE_QUOTE_SOURCE_OUTPUT_FIELD: &str = "reference_quote_source";
const ENTRY_DECISION_REALIZED_VOLATILITY_SOURCE_OUTPUT_FIELD: &str = "realized_volatility_source";
const ENTRY_DECISION_FEE_RATE_SOURCE_OUTPUT_FIELD: &str = "fee_rate_source";
const OPERATOR_EVIDENCE_JSON_SHA256_OUTPUT_FIELD: &str = "operator_evidence_json_sha256";

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
    GenerateBaseStatic {
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
    UpdateOperatorEvidenceToml {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        operator_evidence_json: PathBuf,
        #[arg(long)]
        max_operator_evidence_json_bytes: u64,
    },
    GenerateOperatorEvidenceJson {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        max_operator_evidence_file_bytes: u64,
        #[arg(long)]
        approval_consumption_max_age_seconds: u64,
        #[arg(long)]
        approval_envelope: PathBuf,
        #[arg(long)]
        ssm_manifest: PathBuf,
        #[arg(long)]
        strategy_input_evidence: PathBuf,
        #[arg(long)]
        financial_envelope: PathBuf,
        #[arg(long)]
        pre_run_state: PathBuf,
        #[arg(long)]
        abort_plan: PathBuf,
        #[arg(long)]
        canary_evidence: PathBuf,
        #[arg(long)]
        approval_not_before_unix_seconds: i64,
        #[arg(long)]
        approval_not_after_unix_seconds: i64,
        #[arg(long)]
        approval_nonce: PathBuf,
        #[arg(long)]
        approval_consumption: PathBuf,
        #[arg(long)]
        decision_evidence: PathBuf,
        #[arg(long)]
        nt_submit_event: PathBuf,
        #[arg(long)]
        venue_order_state: PathBuf,
        #[arg(long)]
        strategy_cancel: Option<PathBuf>,
        #[arg(long)]
        restart_reconciliation: PathBuf,
        #[arg(long)]
        post_run_hygiene: PathBuf,
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
    CollectEntryDecisionSourceInputs {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        price_to_beat_source: PathBuf,
        #[arg(long)]
        max_price_to_beat_source_bytes: u64,
        #[arg(long)]
        reference_quote_source: PathBuf,
        #[arg(long)]
        max_reference_quote_source_bytes: u64,
        #[arg(long)]
        realized_volatility_source: PathBuf,
        #[arg(long)]
        max_realized_volatility_source_bytes: u64,
        #[arg(long)]
        fee_rate_source: PathBuf,
        #[arg(long)]
        max_fee_rate_source_bytes: u64,
        #[arg(long)]
        decision_source_output: PathBuf,
        #[arg(long)]
        instrument_source_output: PathBuf,
    },
    CollectEntryDecisionProofSources {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        price_report: PathBuf,
        #[arg(long)]
        max_price_report_bytes: u64,
        #[arg(long)]
        expected_price_report_sha256: String,
        #[arg(long)]
        market_selection_timestamp_ms: u64,
        #[arg(long)]
        decision_timestamp_ms: u64,
        #[arg(long)]
        reference_quote_venue: String,
        #[arg(long)]
        reference_quote_price: f64,
        #[arg(long)]
        reference_quote_observed_ts_ms: u64,
        #[arg(long)]
        realized_volatility_value: f64,
        #[arg(long)]
        realized_volatility_ready_ts_ms: u64,
        #[arg(long)]
        fee_bps_by_instrument_id: Vec<String>,
        #[arg(long)]
        price_to_beat_source_output: PathBuf,
        #[arg(long)]
        reference_quote_source_output: PathBuf,
        #[arg(long)]
        realized_volatility_source_output: PathBuf,
        #[arg(long)]
        fee_rate_source_output: PathBuf,
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
        #[arg(long, value_enum, default_value_t = FinalVerificationStage::PostRun)]
        verification_stage: FinalVerificationStage,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum FinalVerificationStage {
    PreRun,
    PostRun,
}

impl From<FinalVerificationStage> for FinalOperatorPacketVerificationScope {
    fn from(stage: FinalVerificationStage) -> Self {
        match stage {
            FinalVerificationStage::PreRun => Self::PreRun,
            FinalVerificationStage::PostRun => Self::PostRun,
        }
    }
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
        OperatorArtifactsCommand::GenerateBaseStatic {
            config,
            output_dir,
            strategy_instance_id,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let outcome =
                write_base_static_operator_artifacts(&loaded, &strategy_instance_id, &output_dir)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&outcome.command_summary)?
            );
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
        OperatorArtifactsCommand::UpdateOperatorEvidenceToml {
            config,
            operator_evidence_json,
            max_operator_evidence_json_bytes,
        } => {
            let written = update_live_canary_operator_evidence_toml_from_json_file(
                &config,
                &operator_evidence_json,
                max_operator_evidence_json_bytes,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({ "root_toml_sha256": written.sha256 })
                )?
            );
            Ok(())
        }
        OperatorArtifactsCommand::GenerateOperatorEvidenceJson {
            config,
            output,
            max_operator_evidence_file_bytes,
            approval_consumption_max_age_seconds,
            approval_envelope,
            ssm_manifest,
            strategy_input_evidence,
            financial_envelope,
            pre_run_state,
            abort_plan,
            canary_evidence,
            approval_not_before_unix_seconds,
            approval_not_after_unix_seconds,
            approval_nonce,
            approval_consumption,
            decision_evidence,
            nt_submit_event,
            venue_order_state,
            strategy_cancel,
            restart_reconciliation,
            post_run_hygiene,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_operator_evidence_json_from_artifact_paths(
                &loaded,
                OperatorEvidenceJsonBuildInputs {
                    max_operator_evidence_file_bytes,
                    approval_consumption_max_age_seconds,
                    approval_envelope_path: &approval_envelope,
                    ssm_manifest_path: &ssm_manifest,
                    strategy_input_evidence_path: &strategy_input_evidence,
                    financial_envelope_path: &financial_envelope,
                    pre_run_state_path: &pre_run_state,
                    abort_plan_path: &abort_plan,
                    canary_evidence_path: &canary_evidence,
                    approval_not_before_unix_seconds,
                    approval_not_after_unix_seconds,
                    approval_nonce_path: &approval_nonce,
                    approval_consumption_path: &approval_consumption,
                    decision_evidence_path: &decision_evidence,
                    nt_submit_event_path: &nt_submit_event,
                    venue_order_state_path: &venue_order_state,
                    strategy_cancel_path: strategy_cancel.as_deref(),
                    restart_reconciliation_path: &restart_reconciliation,
                    post_run_hygiene_path: &post_run_hygiene,
                },
                &output,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    OPERATOR_EVIDENCE_JSON_SHA256_OUTPUT_FIELD: written.sha256,
                }))?
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
        OperatorArtifactsCommand::CollectEntryDecisionSourceInputs {
            config,
            strategy_instance_id,
            price_to_beat_source,
            max_price_to_beat_source_bytes,
            reference_quote_source,
            max_reference_quote_source_bytes,
            realized_volatility_source,
            max_realized_volatility_source_bytes,
            fee_rate_source,
            max_fee_rate_source_bytes,
            decision_source_output,
            instrument_source_output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let written = runtime.block_on(
                collect_entry_decision_source_inputs_from_configured_provider(
                    &loaded,
                    &strategy_instance_id,
                    EntryDecisionSourceCollectionRequest {
                        price_to_beat_source_path: &price_to_beat_source,
                        max_price_to_beat_source_bytes,
                        reference_quote_source_path: &reference_quote_source,
                        max_reference_quote_source_bytes,
                        realized_volatility_source_path: &realized_volatility_source,
                        max_realized_volatility_source_bytes,
                        fee_rate_source_path: &fee_rate_source,
                        max_fee_rate_source_bytes,
                        decision_source_output_path: &decision_source_output,
                        instrument_source_output_path: &instrument_source_output,
                    },
                ),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    ENTRY_DECISION_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.decision_source),
                    ENTRY_DECISION_INSTRUMENT_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.instrument_source),
                }))?
            );
            Ok(())
        }
        OperatorArtifactsCommand::CollectEntryDecisionProofSources {
            config,
            strategy_instance_id,
            price_report,
            max_price_report_bytes,
            expected_price_report_sha256,
            market_selection_timestamp_ms,
            decision_timestamp_ms,
            reference_quote_venue,
            reference_quote_price,
            reference_quote_observed_ts_ms,
            realized_volatility_value,
            realized_volatility_ready_ts_ms,
            fee_bps_by_instrument_id,
            price_to_beat_source_output,
            reference_quote_source_output,
            realized_volatility_source_output,
            fee_rate_source_output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let fee_bps_by_instrument_id =
                parse_fee_bps_by_instrument_id(&fee_bps_by_instrument_id)?;
            let written = write_entry_decision_proof_source_files(
                &loaded,
                &strategy_instance_id,
                EntryDecisionProofSourceMaterializationRequest {
                    price_report_path: &price_report,
                    max_price_report_bytes,
                    expected_price_report_sha256: &expected_price_report_sha256,
                    market_selection_timestamp_ms,
                    decision_timestamp_ms,
                    reference_quote: EntryDecisionReferenceQuoteProofInput {
                        venue: reference_quote_venue,
                        price: reference_quote_price,
                        observed_ts_ms: reference_quote_observed_ts_ms,
                    },
                    realized_volatility: EntryDecisionRealizedVolatilityProofInput {
                        value: realized_volatility_value,
                        ready_ts_ms: realized_volatility_ready_ts_ms,
                    },
                    fee_bps_by_instrument_id,
                    price_to_beat_source_output_path: &price_to_beat_source_output,
                    reference_quote_source_output_path: &reference_quote_source_output,
                    realized_volatility_source_output_path: &realized_volatility_source_output,
                    fee_rate_source_output_path: &fee_rate_source_output,
                },
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    ENTRY_DECISION_PRICE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.price_to_beat_source),
                    ENTRY_DECISION_REFERENCE_QUOTE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.reference_quote_source),
                    ENTRY_DECISION_REALIZED_VOLATILITY_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.realized_volatility_source),
                    ENTRY_DECISION_FEE_RATE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.fee_rate_source),
                }))?
            );
            Ok(())
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
            verification_stage,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let outcome = verify_final_operator_packet_with_scope(
                &loaded,
                &operator_packet,
                verification_stage.into(),
            )?;
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

fn parse_fee_bps_by_instrument_id(values: &[String]) -> Result<BTreeMap<String, f64>, String> {
    let mut fees = BTreeMap::new();
    for value in values {
        let Some((instrument_id, fee_bps)) = value.split_once('=') else {
            return Err(
                "fee-bps-by-instrument-id entries must use instrument_id=fee_bps".to_string(),
            );
        };
        let instrument_id = instrument_id.trim();
        if instrument_id.is_empty() {
            return Err("fee-bps-by-instrument-id instrument id is empty".to_string());
        }
        let fee_bps = fee_bps
            .parse::<f64>()
            .map_err(|_| "fee-bps-by-instrument-id fee bps is invalid".to_string())?;
        if fees.insert(instrument_id.to_string(), fee_bps).is_some() {
            return Err("fee-bps-by-instrument-id contains duplicate instrument id".to_string());
        }
    }
    Ok(fees)
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
