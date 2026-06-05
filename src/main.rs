use clap::Parser;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_canary_gate::current_build_head_sha,
    bolt_v3_live_node::{
        BoltV3NoSubmitBookDeltasEvidence, BoltV3NoSubmitDataClientReadinessEvidence,
        BoltV3NoSubmitReferenceQuoteEvidence, BoltV3NoSubmitTradeEvidence,
        build_bolt_v3_all_configured_client_mapping_live_node, build_bolt_v3_live_node,
        build_bolt_v3_no_submit_data_client_probe_live_node, build_bolt_v3_no_submit_live_node,
        collect_no_submit_data_client_metadata_evidence,
        collect_no_submit_data_client_readiness_evidence,
        collect_no_submit_reference_quote_evidence, run_bolt_v3_live_node,
    },
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
    bolt_v3_operator_artifacts::{
        CanaryProofArtifactsCollectionRequest,
        DataClientProductionReadinessMatrixSourceFileRequest, FinalOperatorPacketVerificationScope,
        LiveCanaryPostRunProofInputs, OperatorEvidenceJsonBuildInputs,
        PreRunStateSourceCollectorInputs, WrittenOperatorArtifact,
        assemble_operator_packet_from_static_manifest,
        collect_canary_proof_artifacts_from_configured_provider,
        compute_operator_approval_envelope_sha256,
        pre_run_clob_v2_collateral_accounting_source_requires_resolved_secrets,
        update_live_canary_operator_evidence_toml_from_json_file,
        verify_final_operator_packet_with_scope, write_abort_plan_artifact_from_source_bundle_file,
        write_abort_plan_artifact_from_source_collectors, write_base_static_operator_artifacts,
        write_data_client_behavior_observation_artifact_from_source_file,
        write_data_client_behavior_observation_source_from_probe_events,
        write_data_client_behavior_observation_source_from_probe_events_and_policy_source,
        write_data_client_behavior_probe_events_from_no_submit_readiness_evidence,
        write_data_client_live_node_mapping_source_artifact_from_config,
        write_data_client_nt_source_capability_artifact_from_config,
        write_data_client_policy_behavior_source_artifact_from_nt_sources,
        write_data_client_production_readiness_matrix_artifact_from_source_files,
        write_data_client_readiness_source_artifact_from_config,
        write_data_client_readiness_target_candidates_from_no_submit_readiness_evidence,
        write_entry_decision_evidence_from_source_file,
        write_entry_readiness_gate_session_artifact_from_decision_source_file,
        write_live_canary_post_run_proof_artifacts_from_config,
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

const CANARY_PROOF_GATE_SESSION_OUTPUT_FIELD: &str = "gate_session";
const CANARY_PROOF_CANDIDATE_SOURCE_OUTPUT_FIELD: &str = "canary_proof_candidate_source";
const CANARY_PROOF_ORDER_INTENT_OUTPUT_FIELD: &str = "canary_proof_order_intent";
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
    GenerateProductSubmitProof {
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
        canary_proof_candidate_source: Option<PathBuf>,
        #[arg(long)]
        canary_proof_order_intent: Option<PathBuf>,
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
    CollectDataClientReadinessSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientNtSourceCapability {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        nt_adapter_source: PathBuf,
        #[arg(long)]
        max_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientLiveNodeMappingSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        live_node_source: PathBuf,
        #[arg(long)]
        adapter_mapping_source: PathBuf,
        #[arg(long)]
        provider_registry_source: PathBuf,
        #[arg(long)]
        max_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientBehaviorObservation {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        behavior_source: PathBuf,
        #[arg(long)]
        max_behavior_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientBehaviorObservationSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        probe_events: PathBuf,
        #[arg(long)]
        max_probe_events_bytes: u64,
        #[arg(long)]
        policy_source: Option<PathBuf>,
        #[arg(long)]
        max_policy_source_bytes: Option<u64>,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientPolicyBehaviorSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        nt_policy_source: Vec<PathBuf>,
        #[arg(long)]
        max_source_bytes: u64,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientBehaviorProbeEventsSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientReadinessTargetCandidates {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        client_key: String,
        #[arg(long)]
        output: PathBuf,
    },
    CollectDataClientProductionReadinessMatrix {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        readiness_source: PathBuf,
        #[arg(long)]
        live_node_mapping_source: PathBuf,
        #[arg(long)]
        nt_source_capability: Vec<PathBuf>,
        #[arg(long)]
        target_candidate: Vec<PathBuf>,
        #[arg(long)]
        behavior_observation: Vec<PathBuf>,
        #[arg(long)]
        max_source_bytes: u64,
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
    GenerateEntryReadinessGateSessionFromSource {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        strategy_instance_id: String,
        #[arg(long)]
        decision_source: PathBuf,
        #[arg(long)]
        max_decision_source_bytes: u64,
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
    CollectCanaryProofArtifacts {
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
        signal_quote_source: PathBuf,
        #[arg(long)]
        max_signal_quote_source_bytes: u64,
        #[arg(long)]
        realized_volatility_source: PathBuf,
        #[arg(long)]
        max_realized_volatility_source_bytes: u64,
        #[arg(long)]
        gate_session_output: PathBuf,
        #[arg(long)]
        candidate_source_output: PathBuf,
        #[arg(long)]
        order_intent_output: PathBuf,
    },
    WriteLiveCanaryPostRunProofArtifacts {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        runtime_capture_spool_root: PathBuf,
        #[arg(long)]
        client_order_id: String,
        #[arg(long)]
        venue_order_id: String,
        #[arg(long)]
        venue_order_outcome: String,
        #[arg(long, default_value_t = false)]
        order_remains_open: bool,
        #[arg(long)]
        scanned_artifact: Vec<PathBuf>,
        #[arg(long)]
        retention_purge_path: PathBuf,
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
        OperatorArtifactsCommand::GenerateLiveSubmitApproval {
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
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::PreflightLiveSubmitArming { config, client_key } => {
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
            let output = serde_json::to_value(report)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            Ok(())
        }
        OperatorArtifactsCommand::GenerateProductSubmitProof {
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
        } => {
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
            print_written_operator_artifact(&written)
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
            canary_proof_candidate_source,
            canary_proof_order_intent,
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
                    canary_proof_candidate_source_path: canary_proof_candidate_source.as_deref(),
                    canary_proof_order_intent_path: canary_proof_order_intent.as_deref(),
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
        OperatorArtifactsCommand::CollectDataClientReadinessSource { config, output } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written =
                write_data_client_readiness_source_artifact_from_config(&loaded, &output)?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientNtSourceCapability {
            config,
            client_key,
            nt_adapter_source,
            max_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_data_client_nt_source_capability_artifact_from_config(
                &loaded,
                &client_key,
                &nt_adapter_source,
                max_source_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientLiveNodeMappingSource {
            config,
            live_node_source,
            adapter_mapping_source,
            provider_registry_source,
            max_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let live_node = build_bolt_v3_all_configured_client_mapping_live_node(&loaded)?;
            let written = write_data_client_live_node_mapping_source_artifact_from_config(
                &loaded,
                live_node.registration_summary(),
                &live_node_source,
                &adapter_mapping_source,
                &provider_registry_source,
                max_source_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientBehaviorObservation {
            config,
            client_key,
            behavior_source,
            max_behavior_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_data_client_behavior_observation_artifact_from_source_file(
                &loaded,
                &client_key,
                &behavior_source,
                max_behavior_source_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientBehaviorObservationSource {
            config,
            client_key,
            probe_events,
            max_probe_events_bytes,
            policy_source,
            max_policy_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = if let Some(policy_source) = policy_source.as_ref() {
                let max_policy_source_bytes = max_policy_source_bytes.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--max-policy-source-bytes is required when --policy-source is set"
                    )
                })?;
                write_data_client_behavior_observation_source_from_probe_events_and_policy_source(
                    &loaded,
                    &client_key,
                    &probe_events,
                    max_probe_events_bytes,
                    policy_source,
                    max_policy_source_bytes,
                    &output,
                )?
            } else {
                write_data_client_behavior_observation_source_from_probe_events(
                    &loaded,
                    &client_key,
                    &probe_events,
                    max_probe_events_bytes,
                    &output,
                )?
            };
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientPolicyBehaviorSource {
            config,
            client_key,
            nt_policy_source,
            max_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_data_client_policy_behavior_source_artifact_from_nt_sources(
                &loaded,
                &client_key,
                &nt_policy_source,
                max_source_bytes,
                &output,
            )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientBehaviorProbeEventsSource {
            config,
            client_key,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let (mut live_node, probe_loaded) =
                build_bolt_v3_no_submit_data_client_probe_live_node(&loaded, &client_key)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let local = tokio::task::LocalSet::new();
            let evidence = runtime.block_on(local.run_until(
                collect_no_submit_data_client_readiness_evidence(
                    &mut live_node,
                    &probe_loaded,
                    &client_key,
                ),
            ))?;
            let written =
                write_data_client_behavior_probe_events_from_no_submit_readiness_evidence(
                    &probe_loaded,
                    &client_key,
                    &evidence,
                    &output,
                )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientReadinessTargetCandidates {
            config,
            client_key,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let (mut live_node, probe_loaded) =
                build_bolt_v3_no_submit_data_client_probe_live_node(&loaded, &client_key)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let local = tokio::task::LocalSet::new();
            let metadata = runtime.block_on(local.run_until(
                collect_no_submit_data_client_metadata_evidence(
                    &mut live_node,
                    &probe_loaded,
                    &client_key,
                ),
            ))?;
            let evidence = BoltV3NoSubmitDataClientReadinessEvidence {
                metadata,
                quotes: BoltV3NoSubmitReferenceQuoteEvidence { quotes: Vec::new() },
                books: BoltV3NoSubmitBookDeltasEvidence { deltas: Vec::new() },
                trades: BoltV3NoSubmitTradeEvidence { trades: Vec::new() },
            };
            let written =
                write_data_client_readiness_target_candidates_from_no_submit_readiness_evidence(
                    &probe_loaded,
                    &client_key,
                    &evidence,
                    &output,
                )?;
            print_written_operator_artifact(&written)
        }
        OperatorArtifactsCommand::CollectDataClientProductionReadinessMatrix {
            config,
            readiness_source,
            live_node_mapping_source,
            nt_source_capability,
            target_candidate,
            behavior_observation,
            max_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_data_client_production_readiness_matrix_artifact_from_source_files(
                DataClientProductionReadinessMatrixSourceFileRequest {
                    loaded: &loaded,
                    readiness_source_path: &readiness_source,
                    live_node_mapping_source_path: &live_node_mapping_source,
                    nt_source_capability_paths: &nt_source_capability,
                    target_candidate_paths: &target_candidate,
                    behavior_observation_paths: &behavior_observation,
                    max_source_bytes,
                    output_path: &output,
                },
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
        OperatorArtifactsCommand::GenerateEntryReadinessGateSessionFromSource {
            config,
            strategy_instance_id,
            decision_source,
            max_decision_source_bytes,
            output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let written = write_entry_readiness_gate_session_artifact_from_decision_source_file(
                &loaded,
                &strategy_instance_id,
                &decision_source,
                max_decision_source_bytes,
                &output,
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
        OperatorArtifactsCommand::CollectCanaryProofArtifacts {
            config,
            strategy_instance_id,
            price_to_beat_source,
            max_price_to_beat_source_bytes,
            reference_quote_source,
            max_reference_quote_source_bytes,
            signal_quote_source,
            max_signal_quote_source_bytes,
            realized_volatility_source,
            max_realized_volatility_source_bytes,
            gate_session_output,
            candidate_source_output,
            order_intent_output,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let written =
                runtime.block_on(collect_canary_proof_artifacts_from_configured_provider(
                    &loaded,
                    &strategy_instance_id,
                    CanaryProofArtifactsCollectionRequest {
                        price_to_beat_source_path: &price_to_beat_source,
                        max_price_to_beat_source_bytes,
                        reference_quote_source_path: &reference_quote_source,
                        max_reference_quote_source_bytes,
                        signal_quote_source_path: &signal_quote_source,
                        max_signal_quote_source_bytes,
                        realized_volatility_source_path: &realized_volatility_source,
                        max_realized_volatility_source_bytes,
                        gate_session_output_path: &gate_session_output,
                        candidate_source_output_path: &candidate_source_output,
                        order_intent_output_path: &order_intent_output,
                    },
                ))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    CANARY_PROOF_GATE_SESSION_OUTPUT_FIELD: written_operator_artifact_json(&written.gate_session),
                    CANARY_PROOF_CANDIDATE_SOURCE_OUTPUT_FIELD: written_operator_artifact_json(&written.candidate_source),
                    CANARY_PROOF_ORDER_INTENT_OUTPUT_FIELD: written_operator_artifact_json(&written.order_intent),
                }))?
            );
            Ok(())
        }
        OperatorArtifactsCommand::WriteLiveCanaryPostRunProofArtifacts {
            config,
            run_id,
            runtime_capture_spool_root,
            client_order_id,
            venue_order_id,
            venue_order_outcome,
            order_remains_open,
            scanned_artifact,
            retention_purge_path,
        } => {
            let loaded = load_bolt_v3_config(&config)?;
            // Resolve the run's secrets so the post-run hygiene scan attests
            // `raw_secret_residue_absent` against the exact secret values this
            // run handled (the single secret source of truth), not a hardcoded
            // credential-shape list.
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved = resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?;
            let written = write_live_canary_post_run_proof_artifacts_from_config(
                &loaded,
                &resolved,
                &LiveCanaryPostRunProofInputs {
                    run_id: &run_id,
                    runtime_capture_spool_root: &runtime_capture_spool_root,
                    client_order_id: &client_order_id,
                    venue_order_id: &venue_order_id,
                    venue_order_outcome: &venue_order_outcome,
                    order_remains_open,
                    scanned_artifact_paths: &scanned_artifact,
                    retention_purge_path: &retention_purge_path,
                },
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "canary_evidence": written_operator_artifact_json(&written.canary_evidence),
                    "nt_submit_event": written_operator_artifact_json(&written.nt_submit_event),
                    "venue_order_state": written_operator_artifact_json(&written.venue_order_state),
                    "restart_reconciliation": written_operator_artifact_json(&written.restart_reconciliation),
                    "post_run_hygiene": written_operator_artifact_json(&written.post_run_hygiene),
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
