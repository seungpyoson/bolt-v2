use clap::Parser;
use std::path::PathBuf;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{
        build_bolt_v3_live_node, build_bolt_v3_no_submit_live_node,
        collect_no_submit_reference_quote_evidence, run_bolt_v3_live_node,
    },
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
    bolt_v3_operator_artifacts::{
        ChainlinkPriceReportSourceMaterializationRequest,
        EntryDecisionProofSourceMaterializationRequest, EntryDecisionSourceCollectionRequest,
        FinalOperatorPacketVerificationScope, OperatorEvidenceJsonBuildInputs,
        PreRunStateSourceCollectorInputs, WrittenOperatorArtifact,
        assemble_operator_packet_from_static_manifest,
        chainlink_data_streams_ssm_credential_parameters,
        collect_entry_decision_source_inputs_from_configured_provider,
        compute_operator_approval_envelope_sha256,
        pre_run_clob_v2_collateral_accounting_source_requires_resolved_secrets,
        update_live_canary_operator_evidence_toml_from_json_file,
        verify_final_operator_packet_with_scope, write_abort_plan_artifact_from_source_bundle_file,
        write_abort_plan_artifact_from_source_collectors, write_base_static_operator_artifacts,
        write_chainlink_price_report_source_from_configured_provider,
        write_entry_decision_evidence_from_source_file, write_entry_decision_proof_source_files,
        write_market_selection_source_artifact_from_decision_evidence_and_instrument_source_file,
        write_operator_evidence_json_from_artifact_paths,
        write_pre_run_clob_v2_adapter_signing_source_artifact_from_nt_signing_source,
        write_pre_run_clob_v2_collateral_accounting_source_artifact_from_configured_balance_allowance,
        write_pre_run_clob_v2_fee_behavior_source_artifact_from_nt_fee_sources,
        write_pre_run_egress_identity_source_artifact_from_configured_probe,
        write_pre_run_funding_margin_source_artifact_from_configured_balance_allowance,
        write_pre_run_host_clock_source_artifact_from_configured_provider_time,
        write_pre_run_state_artifact_from_source_bundle_file,
        write_pre_run_state_artifact_from_source_collectors,
        write_pre_run_venue_account_state_source_artifact_from_configured_account_queries,
        write_reference_quote_observations_source_from_no_submit_evidence,
        write_static_artifacts_manifest_from_operator_evidence, write_static_operator_artifacts,
        write_strategy_input_evidence_artifact_from_decision_evidence_file,
    },
    bolt_v3_providers::{
        ClobV2BalanceAllowanceCacheSync, ClobV2BalanceAllowanceCacheSyncRequest,
        binding_for_provider_key, sync_clob_v2_balance_allowance_cache_from_configured_account,
    },
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
        gate_session: PathBuf,
        #[arg(long)]
        expected_gate_session_sha256: String,
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
    CollectPreRunHostClockSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    CollectPreRunEgressIdentitySource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    CollectPreRunVenueAccountStateSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    CollectPreRunFundingMarginSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        fee_rate_source: PathBuf,
        #[arg(long)]
        fee_rate_source_sha256: String,
        #[arg(long)]
        max_fee_rate_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectPreRunClobV2AdapterSigningSource {
        #[arg(long)]
        cargo_toml: PathBuf,
        #[arg(long)]
        cargo_lock: PathBuf,
        #[arg(long)]
        clob_signing_source: PathBuf,
        #[arg(long)]
        max_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectPreRunClobV2CollateralAccountingSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        fee_rate_source: PathBuf,
        #[arg(long)]
        fee_rate_source_sha256: String,
        #[arg(long)]
        max_fee_rate_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    SyncClobV2BalanceAllowanceCache {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        acknowledge_clob_cache_mutation: bool,
    },
    CollectPreRunClobV2FeeBehaviorSource {
        #[arg(long)]
        nt_execution_parse_source: PathBuf,
        #[arg(long)]
        nt_http_parse_source: PathBuf,
        #[arg(long)]
        max_source_bytes: u64,
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
    CollectChainlinkPriceReportSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        report_timestamp_unix_seconds: u64,
        #[arg(long)]
        max_report_response_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectReferenceQuoteObservationsSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    CollectChainlinkEntryDecisionSourceInputs {
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
        decision_source_output: PathBuf,
        #[arg(long)]
        instrument_source_output: PathBuf,
        #[arg(long)]
        fee_rate_source_output: PathBuf,
    },
    CollectChainlinkEntryDecisionProofSources {
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
        reference_quote_observations_source: PathBuf,
        #[arg(long)]
        max_reference_quote_observations_source_bytes: u64,
        #[arg(long)]
        price_to_beat_source_output: PathBuf,
        #[arg(long)]
        reference_quote_source_output: PathBuf,
        #[arg(long)]
        realized_volatility_source_output: PathBuf,
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
            gate_session,
            expected_gate_session_sha256,
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
                    gate_session_path: &gate_session,
                    expected_gate_session_sha256: expected_gate_session_sha256.as_str(),
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
        OperatorArtifactsCommand::CollectPreRunHostClockSource {
            config,
            strategy_instance_id,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let written = runtime.block_on(
                write_pre_run_host_clock_source_artifact_from_configured_provider_time(
                    &loaded,
                    &strategy_instance_id,
                    &output,
                ),
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectPreRunEgressIdentitySource {
            config,
            strategy_instance_id,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_pre_run_egress_identity_source_artifact_from_configured_probe(
                &loaded,
                &strategy_instance_id,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectPreRunVenueAccountStateSource {
            config,
            strategy_instance_id,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved = resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let written = runtime.block_on(
                write_pre_run_venue_account_state_source_artifact_from_configured_account_queries(
                    &loaded,
                    &strategy_instance_id,
                    &resolved,
                    &output,
                ),
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectPreRunFundingMarginSource {
            config,
            strategy_instance_id,
            fee_rate_source,
            fee_rate_source_sha256,
            max_fee_rate_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let resolved =
                if pre_run_clob_v2_collateral_accounting_source_requires_resolved_secrets(
                    &loaded,
                    &strategy_instance_id,
                )? {
                    let ssm_resolver_session = SsmResolverSession::new()?;
                    Some(resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?)
                } else {
                    None
                };
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let written = runtime.block_on(
                write_pre_run_funding_margin_source_artifact_from_configured_balance_allowance(
                    &loaded,
                    &strategy_instance_id,
                    resolved.as_ref(),
                    &fee_rate_source,
                    &fee_rate_source_sha256,
                    max_fee_rate_source_bytes,
                    &output,
                ),
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectPreRunClobV2AdapterSigningSource {
            cargo_toml,
            cargo_lock,
            clob_signing_source,
            max_source_bytes,
            output,
        } => {
            let written =
                write_pre_run_clob_v2_adapter_signing_source_artifact_from_nt_signing_source(
                    &cargo_toml,
                    &cargo_lock,
                    &clob_signing_source,
                    max_source_bytes,
                    &output,
                )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectPreRunClobV2CollateralAccountingSource {
            config,
            strategy_instance_id,
            fee_rate_source,
            fee_rate_source_sha256,
            max_fee_rate_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let resolved =
                if pre_run_clob_v2_collateral_accounting_source_requires_resolved_secrets(
                    &loaded,
                    &strategy_instance_id,
                )? {
                    let ssm_resolver_session = SsmResolverSession::new()?;
                    Some(resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?)
                } else {
                    None
                };
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let written = runtime.block_on(
                write_pre_run_clob_v2_collateral_accounting_source_artifact_from_configured_balance_allowance(
                    &loaded,
                    &strategy_instance_id,
                    resolved.as_ref(),
                    &fee_rate_source,
                    &fee_rate_source_sha256,
                    max_fee_rate_source_bytes,
                    &output,
                ),
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::SyncClobV2BalanceAllowanceCache {
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
        OperatorArtifactsCommand::CollectPreRunClobV2FeeBehaviorSource {
            nt_execution_parse_source,
            nt_http_parse_source,
            max_source_bytes,
            output,
        } => {
            let written = write_pre_run_clob_v2_fee_behavior_source_artifact_from_nt_fee_sources(
                &nt_execution_parse_source,
                &nt_http_parse_source,
                max_source_bytes,
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
        OperatorArtifactsCommand::CollectChainlinkPriceReportSource {
            config,
            strategy_instance_id,
            report_timestamp_unix_seconds,
            max_report_response_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let credential_parameters =
                chainlink_data_streams_ssm_credential_parameters(&loaded, &strategy_instance_id)?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let credential_api_key = ssm_resolver_session.resolve(
                &loaded.root.aws.region,
                &credential_parameters.api_key_parameter,
            )?;
            let credential_api_secret = ssm_resolver_session.resolve(
                &loaded.root.aws.region,
                &credential_parameters.api_secret_parameter,
            )?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let written = runtime.block_on(
                write_chainlink_price_report_source_from_configured_provider(
                    &loaded,
                    &strategy_instance_id,
                    ChainlinkPriceReportSourceMaterializationRequest {
                        credential_api_key: &credential_api_key,
                        credential_api_secret: &credential_api_secret,
                        report_timestamp_unix_seconds,
                        max_report_response_bytes,
                        output_path: &output,
                    },
                ),
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectReferenceQuoteObservationsSource {
            config,
            strategy_instance_id,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let mut live_node = build_bolt_v3_no_submit_live_node(&loaded)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let local = tokio::task::LocalSet::new();
            let evidence = runtime.block_on(local.run_until(
                collect_no_submit_reference_quote_evidence(&mut live_node, &loaded),
            ))?;
            let written = write_reference_quote_observations_source_from_no_submit_evidence(
                &loaded,
                &strategy_instance_id,
                &evidence,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectChainlinkEntryDecisionSourceInputs {
            config,
            strategy_instance_id,
            price_to_beat_source,
            max_price_to_beat_source_bytes,
            reference_quote_source,
            max_reference_quote_source_bytes,
            realized_volatility_source,
            max_realized_volatility_source_bytes,
            decision_source_output,
            instrument_source_output,
            fee_rate_source_output,
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
                        decision_source_output_path: &decision_source_output,
                        instrument_source_output_path: &instrument_source_output,
                        fee_rate_source_output_path: &fee_rate_source_output,
                    },
                ),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    ENTRY_DECISION_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.decision_source),
                    ENTRY_DECISION_INSTRUMENT_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.instrument_source),
                    ENTRY_DECISION_FEE_RATE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.fee_rate_source),
                }))?
            );
            Ok(())
        }
        OperatorArtifactsCommand::CollectChainlinkEntryDecisionProofSources {
            config,
            strategy_instance_id,
            price_report,
            max_price_report_bytes,
            expected_price_report_sha256,
            market_selection_timestamp_ms,
            decision_timestamp_ms,
            reference_quote_observations_source,
            max_reference_quote_observations_source_bytes,
            price_to_beat_source_output,
            reference_quote_source_output,
            realized_volatility_source_output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_entry_decision_proof_source_files(
                &loaded,
                &strategy_instance_id,
                EntryDecisionProofSourceMaterializationRequest {
                    price_report_path: &price_report,
                    max_price_report_bytes,
                    expected_price_report_sha256: &expected_price_report_sha256,
                    market_selection_timestamp_ms,
                    decision_timestamp_ms,
                    reference_quote_observations_source_path: &reference_quote_observations_source,
                    max_reference_quote_observations_source_bytes,
                    price_to_beat_source_output_path: &price_to_beat_source_output,
                    reference_quote_source_output_path: &reference_quote_source_output,
                    realized_volatility_source_output_path: &realized_volatility_source_output,
                },
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    ENTRY_DECISION_PRICE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.price_to_beat_source),
                    ENTRY_DECISION_REFERENCE_QUOTE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.reference_quote_source),
                    ENTRY_DECISION_REALIZED_VOLATILITY_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.realized_volatility_source),
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

fn print_clob_v2_balance_allowance_cache_sync(
    sync: &ClobV2BalanceAllowanceCacheSync,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = serde_json::Map::new();
    output.insert(
        CLOB_V2_CACHE_SYNC_COMPLETED_OUTPUT_FIELD.to_string(),
        serde_json::json!(true),
    );
    output.insert(
        CLOB_V2_CACHE_SYNC_EXECUTION_CLIENT_OUTPUT_FIELD.to_string(),
        serde_json::json!(&sync.execution_client_id),
    );
    output.insert(
        CLOB_V2_CACHE_SYNC_REQUEST_PATH_OUTPUT_FIELD.to_string(),
        serde_json::json!(sync.request_path),
    );
    output.insert(
        CLOB_V2_CACHE_SYNC_BASE_URL_HTTP_SHA256_OUTPUT_FIELD.to_string(),
        serde_json::json!(&sync.base_url_http_sha256),
    );
    println!("{}", serde_json::to_string_pretty(&output)?);
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
