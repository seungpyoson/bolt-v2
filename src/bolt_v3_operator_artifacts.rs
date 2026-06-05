use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, btree_map::Entry},
    error::Error,
    ffi::OsStr,
    fmt, fs,
    io::{self, Read, Write},
    ops::Range,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use anyhow::anyhow;
use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use nautilus_network::http::{HttpClient, USER_AGENT};
use rust_decimal::{Decimal, prelude::FromPrimitive};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    bolt_v3_archetypes::{
        ArchetypeGateRequirement, GateRole, GateValueKind,
        binary_oracle_edge_taker::raw_taker_config,
    },
    bolt_v3_canary_proof_policy::{
        CANARY_PROOF_CANDIDATE_SOURCE_RECORD_KIND, CANARY_PROOF_CLAIM,
        CANARY_PROOF_ORDER_INTENT_RECORD_KIND, CanaryProofPolicyInput,
    },
    bolt_v3_client_registration::BoltV3RegistrationSummary,
    bolt_v3_config::{
        BoltV3RootConfig, CHAINLINK_DATA_STREAMS_PROVIDER_KIND, DECISION_REFERENCE_GATE_ROLE,
        DataClientReadinessProbeBookType, DataClientReadinessProbeMarketDataKind,
        DataClientReadinessProbeQuoteTargetSource, LiveCanaryOperatorEvidenceBlock,
        LoadedBoltV3Config, NO_RESOLUTION_KIND, NO_RESOLUTION_VALUE_KIND, PRICE_GATE_VALUE_KIND,
        RESOLUTION_GATE_ROLE,
    },
    bolt_v3_decision_evidence::{
        BoltV3ReadinessGateEvidenceSnapshot, BoltV3StrategyInputEvidenceSnapshot,
        JsonlBoltV3DecisionEvidenceWriter, decision_evidence_path,
        read_latest_entry_decision_evidence_chain, validate_readiness_gate_evidence_snapshot,
        validate_strategy_input_readiness_evidence,
    },
    bolt_v3_live_canary_gate::{
        APPROVAL_ENVELOPE_RECORD_KIND, APPROVAL_ENVELOPE_SCHEMA_VERSION,
        Phase8OperatorApprovalEnvelopeFile, current_build_head_sha,
    },
    bolt_v3_live_node::{
        BoltV3NoSubmitDataClientReadinessEvidence, BoltV3NoSubmitReferenceQuoteEvidence,
        sample_metadata_response_targets, trade_chunk_count_probe_passed,
    },
    bolt_v3_market_families::{self, MarketSelectionTarget, SelectedMarketRequirement},
    bolt_v3_providers::{
        ClobV2AdapterSigningSourceMaterializationRequest,
        ClobV2CollateralAccountingSourceMaterialization,
        ClobV2CollateralAccountingSourceMaterializationRequest,
        ClobV2FeeBehaviorSourceMaterializationRequest, GateProviderEvidenceBinding,
        PriceToBeatReportBinding, ProviderCredentialedBlock, ProviderSecretResolveContext,
        VenueAccountStateSourceMaterializationRequest, binding_for_provider_key,
        confirm_external_snapshot_before_hard_stop, gate_provider_evidence_binding,
        is_lowercase_chainlink_feed_id,
        materialize_clob_v2_adapter_signing_source_from_nt_signing_source,
        materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance,
        materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_once,
        materialize_clob_v2_fee_behavior_source_from_nt_fee_sources,
        materialize_venue_account_state_source_from_configured_account_queries,
    },
    bolt_v3_secrets::{BoltV3SecretError, ResolvedBoltV3Secrets},
    bolt_v3_source_integrity::{
        STRATEGY_KEY, canonical_module_source_set_text, canonical_module_text,
        canonical_source_digest, canonical_source_set_digest, registry_relative_root,
        registry_relative_roots,
    },
    bolt_v3_tiny_canary_evidence::{
        Phase8AbortPlanEvidenceFile, Phase8AbortPlanSourceProofs, Phase8CanaryEvidence,
        Phase8CanaryEvidenceInput, Phase8EvidenceRef, Phase8FinancialEnvelopeEvidenceFile,
        Phase8LiveCanaryResultRefs, Phase8LiveOrderRef, Phase8MarketSelectionRuntimeProvenance,
        Phase8MarketSelectionSourceEvidenceFile, Phase8PreRunStateEvidenceFile,
        Phase8PreRunStateSourceProofs, Phase8RuntimeCaptureRef, Phase8StrategyInputEvidenceFile,
        Phase8StrategyInputSafetyAudit,
    },
    strategies::binary_oracle_edge_taker::{
        BinaryOracleEntryDecisionEvidenceSource, ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND,
        ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION, record_entry_decision_evidence_from_source,
    },
};

const REDACTED_SSM_MANIFEST_SCHEMA_VERSION: u32 = 1;
const REDACTED_SSM_MANIFEST_RECORD_KIND: &str = "bolt_v3.redacted_ssm_manifest.v1";
const DATA_CLIENT_READINESS_SOURCE_SCHEMA_VERSION: u32 = 1;
const DATA_CLIENT_READINESS_SOURCE_RECORD_KIND: &str = "bolt_v3.data_client_readiness_source.v1";
const DATA_CLIENT_READINESS_TARGET_CANDIDATES_RECORD_KIND: &str =
    "bolt_v3.data_client_readiness_target_candidates.v1";
const DATA_CLIENT_READINESS_TARGET_CANDIDATES_STATUS_TARGETS_UNBOUND: &str =
    "target_candidates_only_probe_targets_unbound";
const DATA_CLIENT_READINESS_STATUS_NOT_PRODUCTION_USABLE: &str =
    "not_production_usable_metadata_or_config_only";
const DATA_CLIENT_NT_SOURCE_CAPABILITY_SCHEMA_VERSION: u32 = 1;
const DATA_CLIENT_NT_SOURCE_CAPABILITY_RECORD_KIND: &str =
    "bolt_v3.data_client_nt_source_capability.v1";
const DATA_CLIENT_NT_SOURCE_CAPABILITY_STATUS_NOT_PRODUCTION_USABLE: &str =
    "nt_source_capability_only_behavior_probe_missing";
const DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_SCHEMA_VERSION: u32 = 1;
const DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_RECORD_KIND: &str =
    "bolt_v3.data_client_live_node_mapping_source.v1";
const DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_STATUS_NOT_PRODUCTION_USABLE: &str =
    "live_node_mapping_source_only_behavior_probe_missing";
const DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION: u32 = 1;
const DATA_CLIENT_BEHAVIOR_OBSERVATION_RECORD_KIND: &str =
    "bolt_v3.data_client_behavior_observation.v1";
const DATA_CLIENT_BEHAVIOR_OBSERVATION_SOURCE_RECORD_KIND: &str =
    "bolt_v3.data_client_behavior_observation_source.v1";
const DATA_CLIENT_BEHAVIOR_PROBE_EVENT_RECORD_KIND: &str =
    "bolt_v3.data_client_behavior_probe_event.v1";
const DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_RECORD_KIND: &str =
    "bolt_v3.data_client_policy_behavior_source.v1";
const DATA_CLIENT_BEHAVIOR_OBSERVATION_STATUS_NOT_PRODUCTION_USABLE: &str =
    "behavior_observation_final_matrix_missing";
const DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_STATUS_COMPLETE: &str =
    "data_client_policy_behavior_source_complete";
const DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_STATUS_MISSING_MARKERS: &str =
    "data_client_policy_behavior_source_missing_markers";
const DATA_CLIENT_PRODUCTION_READINESS_MATRIX_SCHEMA_VERSION: u32 = 1;
const DATA_CLIENT_PRODUCTION_READINESS_MATRIX_RECORD_KIND: &str =
    "bolt_v3.data_client_production_readiness_matrix.v1";
const DATA_CLIENT_MISSING_BEHAVIOR_PROOFS: &[&str] = &[
    "metadata_behavior",
    "quote_or_book_behavior",
    "freshness_latency",
    "reconnect_rate_limit_error",
];
const APPROVAL_NONCE_SCHEMA_VERSION: u32 = 1;
const APPROVAL_NONCE_RECORD_KIND: &str = "bolt_v3.operator_approval_nonce.v1";
const APPROVAL_NONCE_BYTES: usize = 32;
const LIVE_CANARY_NT_SUBMIT_EVENT_RECORD_KIND: &str = "nt_submit_event";
const LIVE_CANARY_VENUE_ORDER_STATE_RECORD_KIND: &str = "venue_order_state";
const LIVE_CANARY_RESTART_RECONCILIATION_RECORD_KIND: &str = "restart_reconciliation";
const LIVE_CANARY_POST_RUN_HYGIENE_RECORD_KIND: &str = "post_run_hygiene";
const LIVE_CANARY_TERMINAL_OUTCOME_FILLED: &str = "filled";
const LIVE_CANARY_TERMINAL_OUTCOME_REJECTED: &str = "rejected";
const STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION: u32 = 1;
const STATIC_ARTIFACTS_MANIFEST_RECORD_KIND: &str = "bolt_v3.static_operator_artifacts_manifest.v1";
const PRE_RUN_STATE_SOURCE_PROOF_BUNDLE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_STATE_SOURCE_PROOF_BUNDLE_RECORD_KIND: &str =
    "bolt_v3.pre_run_state_source_proof_bundle.v1";
const ABORT_PLAN_SOURCE_PROOF_BUNDLE_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_SOURCE_PROOF_BUNDLE_RECORD_KIND: &str =
    "bolt_v3.abort_plan_source_proof_bundle.v1";
const SOURCE_BOUND_PRICE_TO_BEAT_SOURCE_SCHEMA_VERSION: u32 = 1;
const SOURCE_BOUND_PRICE_TO_BEAT_SOURCE_RECORD_KIND: &str = "bolt_v3.source_bound_price_to_beat.v1";
const CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD: &str = "feed_bindings";
const CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD: &str = "resolution_identity";
const CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD: &str = "value_kind";
const CHAINLINK_DATA_STREAMS_FEED_ID_FIELD: &str = "feed_id";
const CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD: &str = "report_schema_version";
const CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD: &str = "report_decimal_scale";
const REFERENCE_QUOTE_SOURCE_SCHEMA_VERSION: u32 = 1;
const REFERENCE_QUOTE_SOURCE_RECORD_KIND: &str = "bolt_v3.reference_quote_source.v1";
const SIGNAL_QUOTE_SOURCE_SCHEMA_VERSION: u32 = 1;
const SIGNAL_QUOTE_SOURCE_RECORD_KIND: &str = "bolt_v3.signal_quote_source.v1";
const REFERENCE_QUOTE_OBSERVATIONS_SOURCE_SCHEMA_VERSION: u32 = 1;
const REFERENCE_QUOTE_OBSERVATIONS_SOURCE_RECORD_KIND: &str =
    "bolt_v3.reference_quote_observations_source.v1";
const REALIZED_VOLATILITY_SOURCE_SCHEMA_VERSION: u32 = 1;
const REALIZED_VOLATILITY_SOURCE_RECORD_KIND: &str = "bolt_v3.realized_volatility_source.v1";
pub(crate) const ENTRY_DECISION_FEE_RATE_SOURCE_SCHEMA_VERSION: u32 = 1;
pub(crate) const ENTRY_DECISION_FEE_RATE_SOURCE_RECORD_KIND: &str =
    "bolt_v3.entry_decision_fee_rate_source.v1";
pub(crate) const PRIVATE_ARTIFACT_FILE_MODE: u32 = 0o600;
const ENTRY_DECISION_ZERO_THRESHOLD: f64 = 0.0;
pub(crate) const ENTRY_DECISION_ZERO_TIMESTAMP_MS: u64 = 0;
const SSM_MANIFEST_ARTIFACT_NAME: &str = "ssm-manifest";
const FINANCIAL_ENVELOPE_ARTIFACT_NAME: &str = "financial-envelope";
const STRATEGY_INPUT_ARTIFACT_NAME: &str = "strategy-input";
const GATE_SESSION_ARTIFACT_NAME: &str = "gate-session";
const OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD: &str = "gate_session_path";
const OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD: &str = "expected_gate_session_sha256";
const PRE_RUN_STATE_ARTIFACT_NAME: &str = "pre-run-state";
const ABORT_PLAN_ARTIFACT_NAME: &str = "abort-plan";
const APPROVAL_NONCE_ARTIFACT_NAME: &str = "approval-nonce";
const SSM_MANIFEST_FILE_NAME: &str = "ssm-manifest.json";
const FINANCIAL_ENVELOPE_FILE_NAME: &str = "financial-envelope.json";
const STRATEGY_INPUT_FILE_NAME: &str = "strategy-input.json";
const PRE_RUN_STATE_FILE_NAME: &str = "pre-run-state.json";
const ABORT_PLAN_FILE_NAME: &str = "abort-plan.json";
const APPROVAL_NONCE_FILE_NAME: &str = "approval-nonce.json";
const STATIC_ARTIFACTS_MANIFEST_FILE_NAME: &str = "static-artifacts-manifest.json";
const OPERATOR_EVIDENCE_PACKET_SCHEMA_VERSION: u32 = 1;
const OPERATOR_EVIDENCE_PACKET_RECORD_KIND: &str = "bolt_v3.operator_evidence_packet.v1";
const PRE_RUN_RELEASE_MANIFEST_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_RELEASE_MANIFEST_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_release_manifest_source_proof.v1";
const PRE_RUN_HOST_CLOCK_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_HOST_CLOCK_SOURCE_RECORD_KIND: &str = "bolt_v3.pre_run_host_clock_source.v1";
const PRE_RUN_HOST_CLOCK_REFERENCE_DATE_HEADER: &str = "date";
const PRE_RUN_HOST_CLOCK_REFERENCE_URL_FIELD: &str = "base_url_http";
const PRE_RUN_HOST_CLOCK_REFERENCE_TIMEOUT_FIELD: &str = "http_timeout_secs";
const PRE_RUN_HOST_CLOCK_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
#[rustfmt::skip]
const PRE_RUN_HOST_CLOCK_SOURCE_PROOF_RECORD_KIND: &str = "bolt_v3.pre_run_host_clock_source_proof.v1";
const PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_venue_account_state_source.v1";
const PRE_RUN_VENUE_ACCOUNT_STATE_SNAPSHOT_RECORD_KIND: &str =
    "bolt_v3.pre_run_venue_account_state_snapshot.v1";
const PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_venue_account_state_source_proof.v1";
const PRE_RUN_FUNDING_MARGIN_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_FUNDING_MARGIN_SOURCE_RECORD_KIND: &str = "bolt_v3.pre_run_funding_margin_source.v1";
const PRE_RUN_FUNDING_MARGIN_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_FUNDING_MARGIN_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_funding_margin_source_proof.v1";
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_adapter_signing_source.v1";
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_DOMAIN_REQUIREMENTS_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_adapter_signing_domain_requirements.v1";
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SIGNED_ORDER_FIXTURE_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_adapter_signing_signed_order_fixture.v1";
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SIGNATURE_VERIFICATION_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_adapter_signing_signature_verification.v1";
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_adapter_signing_source_proof.v1";
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_collateral_accounting_source.v1";
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BALANCE_ALLOWANCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_collateral_accounting_balance_allowance.v1";
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ON_CHAIN_BALANCE_ALLOWANCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_collateral_accounting_on_chain_pusd_balance_allowance.v1";
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ASSUMPTIONS_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_collateral_accounting_assumptions.v1";
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_collateral_accounting_source_proof.v1";
const PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BPS_DENOMINATOR: u32 = 10_000;
const PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_fee_behavior_source.v1";
const PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_fee_behavior_source_proof.v1";
const PRE_RUN_MARKET_WINDOW_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_MARKET_WINDOW_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_market_window_source_proof.v1";
const PRE_RUN_SINGLE_RUNNER_LOCK_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_SINGLE_RUNNER_LOCK_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_single_runner_lock_source_proof.v1";
const PRE_RUN_EGRESS_IDENTITY_SOURCE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_EGRESS_IDENTITY_SOURCE_RECORD_KIND: &str =
    "bolt_v3.pre_run_egress_identity_source.v1";
const PRE_RUN_EGRESS_IDENTITY_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_EGRESS_IDENTITY_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_egress_identity_source_proof.v1";
const ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.abort_plan_cancel_if_open_source_proof.v1";
const ABORT_PLAN_CANCEL_IF_OPEN_TARGET_FUNCTION_NAME: &str = "try_submit_exit_order";
const ABORT_PLAN_CANCEL_IF_OPEN_FORCED_FLAT_MARKER: &str =
    "!decision.forced_flat_reasons.is_empty()";
const ABORT_PLAN_CANCEL_IF_OPEN_PENDING_ENTRY_MARKER: &str =
    "managed_position.pending_entry.as_ref()";
const ABORT_PLAN_CANCEL_IF_OPEN_CANCEL_ORDER_MARKER: &str =
    "self.cancel_order(pending_entry.client_order_id, Some(client_id), None)";
const ABORT_PLAN_CANCEL_IF_OPEN_CONTEXT_MARKER: &str =
    "forced-flat exit could not cancel pending entry client_order_id={}";
const ABORT_PLAN_CANCEL_IF_OPEN_EXIT_PENDING_MARKER: &str =
    "self.exposure = ExposureState::ExitPending";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.abort_plan_nt_accepted_venue_pending_source_proof.v1";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_EXIT_PENDING_MARKER: &str =
    "self.exposure = ExposureState::ExitPending(ExitPendingState {";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_PENDING_EXIT_MARKER: &str =
    "pending_exit: PendingExitState {";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_FILL_FALSE_MARKER: &str = "fill_received: false";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_CLOSE_FALSE_MARKER: &str = "close_received: false";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_FALSE_MARKER: &str = "terminal_received: false";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SUBMIT_MARKER: &str =
    "self.submit_order_with_decision_evidence(";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_RESTORE_MANAGED_MARKER: &str =
    "self.exposure = ExposureState::Managed(managed_position);";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_RETURN_ERROR_MARKER: &str = "return Err(error);";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_OK_MARKER: &str = "Ok(Some(client_order_id))";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_FUNCTION_NAME: &str =
    "mark_exit_order_terminal";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_CLIENT_MATCH_MARKER: &str =
    "exit_pending.pending_exit.client_order_id != client_order_id";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_INSTRUMENT_GUARD_MARKER: &str = "if !self.event_instrument_matches_held_exposure(event_instrument_id) {\n            return;\n        }";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_RECEIVED_MARKER: &str =
    "exit_pending.pending_exit.terminal_received = true";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_STATE_UPDATE_MARKER: &str =
    "self.exposure = exit_pending.into_state_after_exit_update();";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_HANDLER_MARKER: &str =
    "self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_CANCELED_HANDLER_NAME: &str = "on_order_canceled";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_REJECTED_HANDLER_NAME: &str = "on_order_rejected";
const ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_EXPIRED_HANDLER_NAME: &str = "on_order_expired";
const ABORT_PLAN_PARTIAL_FILL_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_PARTIAL_FILL_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.abort_plan_partial_fill_source_proof.v1";
const ABORT_PLAN_NETWORK_PARTITION_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_NETWORK_PARTITION_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.abort_plan_network_partition_source_proof.v1";
const ABORT_PLAN_PANIC_GATE_SERVICE_POLICY_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_PANIC_GATE_SERVICE_POLICY_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.abort_plan_panic_gate_service_policy_source_proof.v1";
const ABORT_PLAN_PANIC_BOOTSTRAP_RECOVERY_FUNCTION_NAME: &str = "bootstrap_recovery_from_cache";
const ABORT_PLAN_PANIC_ONE_POSITION_INVARIANT_FUNCTION_NAME: &str =
    "enforce_one_position_invariant";
const ABORT_PLAN_SERVICE_SUBMIT_LIFECYCLE_POLICY_FUNCTION_NAME: &str = "submit_lifecycle_policy";
const ABORT_PLAN_SERVICE_ADMISSION_EVALUATE_FUNCTION_NAME: &str = "evaluate";
const ABORT_PLAN_SERVICE_SUBMIT_INTENT_FUNCTION_NAME: &str = "submit_intent_for";
const ABORT_PLAN_SERVICE_ALLOWS_FUNCTION_NAME: &str = "allows";
const ABORT_PLAN_PANIC_CATCH_UNWIND_MARKER: &str =
    "std::panic::catch_unwind(std::panic::AssertUnwindSafe(||";
const ABORT_PLAN_PANIC_BLIND_RECOVERY_MARKER: &str =
    "self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {";
const ABORT_PLAN_PANIC_CACHE_PROBE_FAILED_MARKER: &str =
    "reason: BlindRecoveryReason::CacheProbeFailed";
const ABORT_PLAN_PANIC_RETURN_MARKER: &str = "return;";
const ABORT_PLAN_PANIC_DEBUG_ASSERTIONS_MARKER: &str = "if cfg!(debug_assertions)";
const ABORT_PLAN_PANIC_DEBUG_PANIC_MARKER: &str = "panic!(";
const ABORT_PLAN_PANIC_REPORT_MARKER: &str =
    "self.report_one_position_invariant_violation(occupancy);";
const ABORT_PLAN_PANIC_RELEASE_BAIL_MARKER: &str = "anyhow::bail!(";
const ABORT_PLAN_SERVICE_POLICY_NEW_MARKER: &str = "BoltV3SubmitLifecyclePolicy::new(";
const ABORT_PLAN_SERVICE_POLICY_CONTINGENT_MARKER: &str = "self.config.manage_contingent_orders";
const ABORT_PLAN_SERVICE_POLICY_GTD_MARKER: &str = "self.config.manage_gtd_expiry";
const ABORT_PLAN_SERVICE_POLICY_STOP_MARKER: &str = "self.config.manage_stop";
const ABORT_PLAN_SERVICE_ADMISSION_UNARMED_MARKER: &str =
    "let Some(report) = inner.gate_report.as_ref() else";
const ABORT_PLAN_SERVICE_ADMISSION_LIFECYCLE_CHECK_MARKER: &str =
    "!request.lifecycle_policy.allows(request.intent_kind)";
const ABORT_PLAN_SERVICE_ADMISSION_LIFECYCLE_REJECT_MARKER: &str =
    "return BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed;";
const ABORT_PLAN_SERVICE_ADMISSION_ADMITTED_MARKER: &str =
    "return BoltV3AdmissionOutcome::Admitted;";
const ABORT_PLAN_SERVICE_REPLACE_ALLOWED_MARKER: &str =
    "BoltV3OrderLifecycleIntent::ReplaceSubmit if self.replace_submit";
const ABORT_PLAN_SERVICE_REPLACE_SUBMIT_MARKER: &str =
    "Ok(Some(BoltV3SubmitIntentKind::ReplaceSubmit))";
const ABORT_PLAN_SERVICE_REPLACE_NONE_MARKER: &str =
    "BoltV3OrderLifecycleIntent::ReplaceSubmit => Ok(None)";
const ABORT_PLAN_SERVICE_CANCEL_NONE_MARKER: &str =
    "BoltV3OrderLifecycleIntent::PlainCancel => Ok(None)";
const ABORT_PLAN_SERVICE_ENTRY_EXIT_ALLOWED_MARKER: &str =
    "BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::RiskReducingExit => true";
const ABORT_PLAN_SERVICE_REPLACE_ALLOWED_FLAG_MARKER: &str =
    "BoltV3SubmitIntentKind::ReplaceSubmit => self.replace_submit";
const ABORT_PLAN_PARTIAL_FILL_ON_ORDER_FILLED_FUNCTION_NAME: &str = "on_order_filled";
const ABORT_PLAN_PARTIAL_FILL_ON_POSITION_CLOSED_FUNCTION_NAME: &str = "on_position_closed";
const ABORT_PLAN_PARTIAL_FILL_MATERIALIZE_FUNCTION_NAME: &str = "materialize_position_from_event";
const ABORT_PLAN_PARTIAL_FILL_TERMINAL_FUNCTION_NAME: &str = "into_state_after_exit_update";
const ABORT_PLAN_PARTIAL_FILL_EXIT_FILL_MARKER: &str =
    "exit.pending_exit.client_order_id == event.client_order_id";
const ABORT_PLAN_PARTIAL_FILL_EXIT_FILL_BRANCH_MARKER: &str = "} else if exit_fill {";
const ABORT_PLAN_PARTIAL_FILL_EXIT_FILL_INSTRUMENT_GUARD_MARKER: &str = "if !self.event_instrument_matches_held_exposure(event.instrument_id) {\n                return Ok(());\n            }";
const ABORT_PLAN_PARTIAL_FILL_FILL_RECEIVED_MARKER: &str =
    "exit_pending.pending_exit.fill_received = true";
const ABORT_PLAN_PARTIAL_FILL_CLOSE_RECEIVED_CHECK_MARKER: &str =
    "if exit_pending.pending_exit.close_received";
const ABORT_PLAN_PARTIAL_FILL_POSITION_MATCH_MARKER: &str =
    "exit_pending.pending_exit.position_id == Some(event.position_id)";
const ABORT_PLAN_PARTIAL_FILL_POSITION_CLOSE_BRANCH_MARKER: &str = "if exit_pending_close {";
const ABORT_PLAN_PARTIAL_FILL_POSITION_CLOSE_INSTRUMENT_GUARD_MARKER: &str = "if !self.event_instrument_matches_held_exposure(event.instrument_id) {\n                return;\n            }";
const ABORT_PLAN_PARTIAL_FILL_CLOSE_RECEIVED_MARKER: &str =
    "exit_pending.pending_exit.close_received = true";
const ABORT_PLAN_PARTIAL_FILL_POSITION_CLEAR_MARKER: &str = "exit_pending.position = None";
const ABORT_PLAN_PARTIAL_FILL_TERMINAL_CHECK_MARKER: &str = "if exit_pending.is_terminal()";
const ABORT_PLAN_PARTIAL_FILL_RESIDUAL_GUARD_MARKER: &str = "if pending_exit.fill_received";
const ABORT_PLAN_PARTIAL_FILL_RESIDUAL_MARKER: &str =
    "pending_exit.residual_position_observed_after_fill = true";
const ABORT_PLAN_PARTIAL_FILL_TERMINAL_RECEIVED_MARKER: &str =
    "self.pending_exit.terminal_received";
const ABORT_PLAN_PARTIAL_FILL_TERMINAL_NOT_FILLED_MARKER: &str = "!self.pending_exit.fill_received";
const ABORT_PLAN_PARTIAL_FILL_TERMINAL_RESIDUAL_MARKER: &str =
    "self.pending_exit.residual_position_observed_after_fill";
const ABORT_PLAN_PARTIAL_FILL_TERMINAL_MANAGED_MARKER: &str =
    "Some(position) => ExposureState::Managed(position)";
const ABORT_PLAN_CANCEL_IF_OPEN_FUNCTION_KEYWORD_WIDTH: usize = [b'f', b'n', b' '].len();
const ABORT_PLAN_CANCEL_IF_OPEN_ATTRIBUTE_MARKER_WIDTH: usize = [b'#', b'['].len();
const ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH: usize = [b'/', b'/'].len();
const ABORT_PLAN_CANCEL_IF_OPEN_PUB_VISIBILITY_PREFIX_WIDTH: usize = [b'p', b'u', b'b', b'('].len();
const ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN: usize = [b' '].len() - [b' '].len();
const ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP: usize = [b' '].len();
const BUILD_CARGO_TOML: &str = include_str!("../Cargo.toml");
const NAUTILUS_TRADER_GIT_URL: &str = "https://github.com/nautechsystems/nautilus_trader.git";
const NAUTILUS_TRADER_CARGO_LOCK_SOURCE_PREFIX: &str =
    "git+https://github.com/nautechsystems/nautilus_trader.git";
const MARKET_SELECTION_SOURCE_BLOCKER: &str = "market-selection remains blocked: T046 missing source-bound price-to-beat strategy decision input";
const ENTRY_READINESS_GATE_SESSION_SCHEMA_VERSION: u32 = 1;
const ENTRY_READINESS_GATE_SESSION_RECORD_KIND: &str = "bolt_v3.entry_readiness_gate_session.v1";
const NORMALIZED_READINESS_GATE_SOURCE_SCHEMA_VERSION: u32 = 1;
const NORMALIZED_READINESS_GATE_SOURCE_RECORD_KIND: &str =
    "bolt_v3.normalized_readiness_gate_source.v1";
const HYPERLIQUID_HIP4_PROVIDER_KIND: &str = "hyperliquid_hip4";
const VENUE_NATIVE_PROVIDER_KIND: &str = "venue_native";
const ENTRY_READINESS_CHAINLINK_REPORT_ARTIFACT_PATH: &str = "entry-decision-price-report";
const ENTRY_READINESS_REFERENCE_REPORT_ARTIFACT_PATH: &str = "entry-decision-reference-report";
const GATE_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const GATE_EVIDENCE_RECORD_KIND: &str = "bolt_v3.gate_evidence.v1";
const GATE_SATISFACTION_KIND_EVIDENCE: &str = "evidence";
const GATE_SATISFACTION_KIND_NO_RESOLUTION: &str = NO_RESOLUTION_KIND;
const GATE_PROVIDER_CAPABILITY_RESOLUTION_VALUE: &str = "resolution_value";
const GATE_PROVIDER_CAPABILITY_REFERENCE_VALUE: &str = "reference_value";
const GATE_FIELD_ARTIFACT_REFS: &str = "artifact_refs";
const GATE_FIELD_ARTIFACT_REFS_PATH: &str = "artifact_refs.path";
const GATE_FIELD_ARTIFACT_REF_PATH: &str = "artifact_ref_path";
const GATE_FIELD_ARTIFACT_SHA256S: &str = "artifact_sha256s";
const GATE_FIELD_CONFIGURED_TARGET_ID: &str = "configured_target_id";
const GATE_FIELD_CREATED_AT_MS: &str = "created_at_ms";
const GATE_FIELD_GATE_SUBSCRIPTIONS: &str = "gate_subscriptions";
const GATE_FIELD_NORMALIZED_VALUE_SHA256: &str = "normalized_value_sha256";
const GATE_FIELD_PROVIDER_ID: &str = "provider_id";
const GATE_FIELD_PROVIDER_KIND: &str = "provider_kind";
const GATE_FIELD_PROVIDER_PROVENANCE_SHA256: &str = "provider_provenance_sha256";
const GATE_FIELD_RECORD_KIND: &str = "record_kind";
const GATE_FIELD_RESOLUTION_IDENTITY: &str = "resolution_identity";
const GATE_FIELD_ROLE: &str = "role";
const GATE_FIELD_ROOT_CONFIG_SHA256: &str = "root_config_sha256";
const GATE_FIELD_SATISFACTION_KIND: &str = "satisfaction_kind";
const GATE_FIELD_SATISFIED_ROLES: &str = "satisfied_roles";
const GATE_FIELD_SCHEMA_VERSION: &str = "schema_version";
const GATE_FIELD_SELECTED_AT_MS: &str = "selected_at_ms";
const GATE_FIELD_SELECTED_MARKET_KEY: &str = "selected_market_key";
const GATE_FIELD_STRATEGY_INSTANCE_ID: &str = "strategy_instance_id";
const GATE_FIELD_VALUE_KIND: &str = "value_kind";
const GATE_VALUE_KIND_INDEX: &str = "index";
const GATE_VALUE_KIND_OUTCOME: &str = "outcome";
const GATE_VALUE_KIND_METADATA: &str = "metadata";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3RedactedSsmManifest {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub config_bundle_checksum: String,
    pub aws_region: String,
    pub entries: Vec<BoltV3RedactedSsmManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3RedactedSsmManifestEntry {
    pub client_key: String,
    pub provider_key: String,
    pub field_name: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3ApprovalNonceArtifact {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub nonce_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StaticArtifactsManifest {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub config_bundle_checksum: String,
    pub generated_artifacts: Vec<BoltV3StaticArtifactRef>,
    pub blockers: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StaticArtifactRef {
    pub name: &'static str,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateArtifactRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEvidenceCollectionStatus {
    Complete,
    Timeout,
    Partial,
    Error,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvidenceInput {
    pub role: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub selected_market_key: String,
    pub collector_observed_at_ms: u64,
    pub source_observed_at_ms: u64,
    pub freshness_max_age_ms: u64,
    pub value_kind: String,
    pub normalized_value: serde_json::Value,
    pub provider_provenance: serde_json::Value,
    pub artifact_refs: Vec<GateArtifactRef>,
    pub collection_status: GateEvidenceCollectionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateEvidence {
    pub schema_version: u32,
    pub record_kind: String,
    pub role: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub selected_market_key: String,
    pub collector_observed_at_ms: u64,
    pub source_observed_at_ms: u64,
    pub fresh_until_ms: u64,
    pub value_kind: String,
    pub normalized_value: serde_json::Value,
    pub normalized_value_sha256: String,
    pub provider_provenance: serde_json::Value,
    pub provider_provenance_sha256: String,
    pub artifact_refs: Vec<GateArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "satisfaction_kind", rename_all = "snake_case")]
pub enum GateSatisfaction {
    Evidence {
        evidence: Box<GateEvidence>,
    },
    NoResolution {
        selected_market_key: String,
        resolution_identity: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryReadinessGateSession {
    pub schema_version: u32,
    pub record_kind: String,
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub selected_market: SelectedMarketRequirement,
    pub created_at_ms: u64,
    pub satisfied_roles: BTreeMap<String, GateSatisfaction>,
    pub session_hash: String,
    pub artifact_refs: Vec<GateArtifactRef>,
}

pub struct EntryReadinessGateSessionRequest<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy_instance_id: &'a str,
    pub selected_market: &'a SelectedMarketRequirement,
    pub requirements: &'a [ArchetypeGateRequirement],
    pub provider_evidence: &'a [GateEvidence],
    pub created_at_ms: u64,
    pub artifact_refs: Vec<GateArtifactRef>,
}

pub struct EntryReadinessGateEvidenceSourceFileRequest<'a> {
    pub role: &'a str,
    pub provider_id: &'a str,
    pub selected_market: &'a SelectedMarketRequirement,
    pub source_path: &'a Path,
    pub max_source_bytes: u64,
    pub expected_source_sha256: &'a str,
    pub artifact_ref_path: &'a str,
    pub collector_observed_at_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalizedReadinessGateSource {
    schema_version: u32,
    record_kind: String,
    provider_kind: String,
    value_kind: String,
    source_observed_at_ms: u64,
    normalized_value: serde_json::Value,
    provider_provenance: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateSessionTargetSubscription {
    required: bool,
    allowed_provider_ids: Option<Vec<String>>,
    allowed_provider_kinds: Option<Vec<String>>,
    allowed_value_kinds: Option<Vec<String>>,
    provider_preference: Option<Vec<String>>,
    allow_no_resolution: bool,
    market_mappings: Option<Vec<GateSessionTargetMarketMapping>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateSessionTargetMarketMapping {
    family_key: String,
    market_class: String,
    resolution_kind: String,
    resolution_identity: String,
    value_kind: String,
    provider_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StaticArtifactsCommandSummary {
    pub generated_artifacts: Vec<BoltV3StaticArtifactSummaryRef>,
    pub manifest_artifact: BoltV3StaticArtifactSummaryRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3BaseStaticArtifactsCommandSummary {
    pub generated_artifacts: Vec<BoltV3StaticArtifactSummaryRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StaticArtifactSummaryRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3OperatorEvidencePacket {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub config_bundle_checksum: String,
    pub static_manifest_path: String,
    pub static_manifest_sha256: String,
    pub live_canary_operator_evidence: BoltV3OperatorEvidencePacketBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3OperatorEvidencePacketBlock {
    pub head_sha: String,
    pub approval_envelope_path: String,
    pub approval_envelope_sha256: String,
    pub ssm_manifest_path: String,
    pub ssm_manifest_sha256: String,
    pub strategy_input_evidence_path: String,
    pub strategy_input_evidence_sha256: String,
    pub gate_session_path: String,
    pub expected_gate_session_sha256: String,
    pub financial_envelope_path: String,
    pub financial_envelope_sha256: String,
    pub pre_run_state_path: String,
    pub pre_run_state_sha256: String,
    pub abort_plan_path: String,
    pub abort_plan_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canary_proof_candidate_source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canary_proof_candidate_source_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canary_proof_order_intent_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canary_proof_order_intent_sha256: Option<String>,
    pub canary_evidence_path: String,
    pub approval_nonce_path: String,
    pub approval_nonce_sha256: String,
    pub approval_consumption_path: String,
    pub decision_evidence_path: String,
    pub nt_submit_event_path: String,
    pub venue_order_state_path: String,
    pub strategy_cancel_path: Option<String>,
    pub restart_reconciliation_path: String,
    pub post_run_hygiene_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3OperatorPacketAssemblyOutcome {
    pub approval_envelope: WrittenOperatorArtifact,
    pub operator_packet: WrittenOperatorArtifact,
    pub static_manifest: WrittenOperatorArtifact,
}

#[derive(Clone, PartialEq, Eq)]
pub struct BoltV3FinalOperatorPacketVerification {
    pub approval_envelope: WrittenOperatorArtifact,
    pub operator_packet: WrittenOperatorArtifact,
    pub static_manifest: WrittenOperatorArtifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalOperatorPacketVerificationScope {
    PreRun,
    PostRun,
}

impl BoltV3FinalOperatorPacketVerification {
    pub fn redacted_summary(&self) -> BoltV3FinalOperatorPacketVerificationSummary {
        BoltV3FinalOperatorPacketVerificationSummary {
            verified_artifacts: vec![
                final_packet_summary_artifact("approval-envelope", &self.approval_envelope.sha256),
                final_packet_summary_artifact(
                    "operator-evidence-packet",
                    &self.operator_packet.sha256,
                ),
                final_packet_summary_artifact(
                    "static-artifacts-manifest",
                    &self.static_manifest.sha256,
                ),
            ],
        }
    }
}

impl fmt::Debug for BoltV3FinalOperatorPacketVerification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.redacted_summary().fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3FinalOperatorPacketVerificationSummary {
    pub verified_artifacts: Vec<BoltV3FinalOperatorPacketVerificationArtifactSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3FinalOperatorPacketVerificationArtifactSummary {
    pub name: &'static str,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StaticArtifactsWriteOutcome {
    pub command_summary: BoltV3StaticArtifactsCommandSummary,
    pub blockers: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BaseStaticArtifactsWriteOutcome {
    pub command_summary: BoltV3BaseStaticArtifactsCommandSummary,
}

#[derive(Clone, PartialEq, Eq)]
pub struct WrittenOperatorArtifact {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientReadinessSourceArtifact {
    schema_version: u32,
    record_kind: &'static str,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    clients: Vec<DataClientReadinessClientSource>,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientReadinessClientSource {
    client_key_hash: String,
    provider_key: String,
    has_data: bool,
    has_execution: bool,
    has_secrets: bool,
    data_only_scope: bool,
    strategy_routed: bool,
    production_usable: bool,
    readiness_status: &'static str,
    supported_market_families: Vec<&'static str>,
    required_secret_blocks: Vec<String>,
    data_config_sha256: Option<String>,
    data_config_field_names: Vec<String>,
    data_config_field_fingerprints: Vec<DataClientReadinessConfigFieldFingerprint>,
    market_coverage_config_values: BTreeMap<String, serde_json::Value>,
    market_coverage_config_field_fingerprints: Vec<DataClientReadinessConfigFieldFingerprint>,
    timeout_policy_field_names: Vec<String>,
    retry_policy_field_names: Vec<String>,
    freshness_policy_field_names: Vec<String>,
    reconnect_policy_field_names: Vec<String>,
    rate_limit_policy_field_names: Vec<String>,
    missing_behavior_proofs: Vec<&'static str>,
    execution_config_sha256: Option<String>,
    execution_config_field_names: Vec<String>,
    execution_config_field_fingerprints: Vec<DataClientReadinessConfigFieldFingerprint>,
    market_identity_targets: Vec<DataClientReadinessTargetSource>,
    readiness_probe_targets: Vec<DataClientReadinessProbeTargetSource>,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientReadinessConfigFieldFingerprint {
    field_name: String,
    value_kind: &'static str,
    value_item_count: Option<usize>,
    value_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientReadinessTargetSource {
    configured_target_id_hash: String,
    family_key: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientReadinessProbeTargetSource {
    quote_target_source: &'static str,
    configured_target_id_hash: Option<String>,
    event_kind: &'static str,
    book_type: Option<&'static str>,
    instrument_id_hash: Option<String>,
    max_metadata_quote_targets: Option<usize>,
    allow_metadata_target_sampling: bool,
    min_observed_targets: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientReadinessTargetCandidatesArtifact {
    schema_version: u32,
    record_kind: &'static str,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    client_key_hash: String,
    provider_key: String,
    observed_at_unix_millis: u64,
    metadata_response_count: usize,
    instrument_count: usize,
    instrument_ids: Vec<String>,
    instrument_ids_sha256: String,
    production_usable: bool,
    readiness_status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientNtSourceCapabilityArtifact {
    schema_version: u32,
    record_kind: &'static str,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    client_key_hash: String,
    provider_key: String,
    nt_source_path_hash: String,
    nt_source_sha256: String,
    nt_source_byte_len: usize,
    metadata_request_instruments_surface_present: bool,
    metadata_request_instrument_surface_present: bool,
    quote_subscription_surface_present: bool,
    book_subscription_surface_present: bool,
    ticker_subscription_surface_present: bool,
    unsupported_dispositions: Vec<&'static str>,
    production_usable: bool,
    readiness_status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientLiveNodeMappingSourceArtifact {
    schema_version: u32,
    record_kind: &'static str,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    live_node_source_path_hash: String,
    live_node_source_sha256: String,
    adapter_mapping_source_path_hash: String,
    adapter_mapping_source_sha256: String,
    provider_registry_source_path_hash: String,
    provider_registry_source_sha256: String,
    live_node_calls_adapter_mapping: bool,
    live_node_registers_mapped_clients: bool,
    adapter_mapping_iterates_loaded_clients: bool,
    adapter_mapping_dispatches_provider_binding: bool,
    adapter_mapping_uses_provider_lookup: bool,
    provider_registry_exposes_binding_lookup: bool,
    unsupported_dispositions: Vec<&'static str>,
    clients: Vec<DataClientLiveNodeMappingClientSource>,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientLiveNodeMappingClientSource {
    client_key_hash: String,
    provider_key: String,
    has_data: bool,
    has_execution: bool,
    provider_binding_registered: bool,
    data_block_flows_through_mapping_source: bool,
    data_client_registered_through_live_node: bool,
    execution_block_flows_through_mapping_source: bool,
    execution_client_registered_through_live_node: bool,
    production_usable: bool,
    readiness_status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientBehaviorObservationArtifact {
    schema_version: u32,
    record_kind: &'static str,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    client_key_hash: String,
    provider_key: String,
    behavior_source_path_hash: String,
    behavior_source_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_source_sha256: Option<String>,
    observed_at_unix_millis: u64,
    observation_window_millis: u64,
    metadata_behavior: DataClientBehaviorSurfaceObservation,
    quote_behavior: DataClientBehaviorSurfaceObservation,
    book_behavior: DataClientBehaviorSurfaceObservation,
    ticker_behavior: DataClientBehaviorSurfaceObservation,
    trade_behavior: DataClientBehaviorSurfaceObservation,
    freshness: DataClientFreshnessObservation,
    reconnect: DataClientPolicyObservation,
    rate_limit: DataClientPolicyObservation,
    parse_error: DataClientPolicyObservation,
    behavior_observation_complete: bool,
    missing_behavior_proofs: Vec<&'static str>,
    production_usable: bool,
    readiness_status: &'static str,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataClientBehaviorObservationSourceFile {
    schema_version: u32,
    record_kind: String,
    client_key_hash: String,
    provider_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_source_sha256: Option<String>,
    observed_at_unix_millis: u64,
    observation_window_millis: u64,
    metadata_behavior: DataClientBehaviorSurfaceObservation,
    quote_behavior: DataClientBehaviorSurfaceObservation,
    book_behavior: DataClientBehaviorSurfaceObservation,
    ticker_behavior: DataClientBehaviorSurfaceObservation,
    trade_behavior: DataClientBehaviorSurfaceObservation,
    freshness: DataClientFreshnessObservation,
    reconnect: DataClientPolicyObservation,
    rate_limit: DataClientPolicyObservation,
    parse_error: DataClientPolicyObservation,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataClientPolicyBehaviorSourceArtifact {
    schema_version: u32,
    record_kind: String,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    client_key_hash: String,
    provider_key: String,
    nt_policy_source_path_hashes: Vec<String>,
    nt_policy_source_sha256s: Vec<String>,
    nt_policy_source_byte_len: usize,
    reconnect: DataClientPolicyObservation,
    rate_limit: DataClientPolicyObservation,
    parse_error: DataClientPolicyObservation,
    source_owned_policy_observation_complete: bool,
    production_usable: bool,
    readiness_status: String,
}

#[derive(Debug, Clone)]
struct DataClientLoadedPolicyBehaviorSource {
    source_sha256: String,
    artifact: DataClientPolicyBehaviorSourceArtifact,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataClientBehaviorProbeEvent {
    schema_version: u32,
    record_kind: String,
    client_key_hash: String,
    provider_key: String,
    observed_at_unix_millis: u64,
    event_kind: String,
    supported_by_nt_source: bool,
    observed_through_live_node: bool,
    age_millis: Option<u64>,
    latency_millis: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_clock_skew_millis: Option<u64>,
    recovered: Option<bool>,
    fail_closed: Option<bool>,
    evidence_sha256: Option<String>,
    unsupported_disposition: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataClientBehaviorSurfaceObservation {
    supported_by_nt_source: bool,
    observed_through_live_node: bool,
    sample_count: u64,
    first_observed_at_unix_millis: Option<u64>,
    last_observed_at_unix_millis: Option<u64>,
    evidence_sha256: Option<String>,
    unsupported_disposition: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataClientFreshnessObservation {
    configured_max_age_millis: u64,
    max_observed_age_millis: u64,
    latency_sample_count: u64,
    latency_p95_millis: u64,
    latency_max_millis: u64,
    within_configured_bound: bool,
    evidence_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataClientPolicyObservation {
    behavior_observed: bool,
    recovered: bool,
    fail_closed: bool,
    evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientProductionReadinessMatrixArtifact {
    schema_version: u32,
    record_kind: &'static str,
    generated_at_unix_seconds: u64,
    config_bundle_checksum: String,
    readiness_source_sha256: String,
    live_node_mapping_source_sha256: String,
    nt_source_capability_sha256s: Vec<String>,
    target_candidate_sha256s: Vec<String>,
    behavior_observation_sha256s: Vec<String>,
    clients: Vec<DataClientProductionReadinessMatrixClient>,
}

#[derive(Debug, Clone, Serialize)]
struct DataClientProductionReadinessMatrixClient {
    client_key_hash: String,
    provider_key: String,
    has_data: bool,
    has_execution: bool,
    readiness_required: bool,
    config_inventory_present: bool,
    live_node_mapping_present: bool,
    nt_source_capability_present: bool,
    source_owned_target_binding_present: bool,
    behavior_observation_complete: bool,
    production_usable: bool,
    readiness_status: &'static str,
    missing_proofs: Vec<&'static str>,
    market_coverage_config_values: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunReleaseManifestSourceProof {
    pub nt_revision: String,
    pub clob_signing_version: String,
    pub nt_revision_matches_compiled_pin: bool,
    pub cargo_toml_sha256: String,
    pub cargo_lock_sha256: String,
    pub clob_signing_source_sha256: String,
    pub evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunHostClockSourceProof {
    pub host_clock_skew_within_bound: bool,
    pub host_clock_skew_millis: u64,
    pub max_host_clock_skew_millis: u64,
    pub host_clock_source_sha256: String,
    pub host_clock_skew_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunVenueAccountStateSourceProof {
    pub conflicting_open_orders_absent: bool,
    pub preexisting_position_absent: bool,
    pub venue_account_state_source_sha256: String,
    pub venue_account_state_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunFundingMarginSourceProof {
    pub funding_margin_covers_max_notional_plus_fees: bool,
    pub funding_margin_source_sha256: String,
    pub funding_margin_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunClobV2AdapterSigningSourceProof {
    pub clob_v2_adapter_signing_verified: bool,
    pub clob_v2_adapter_signing_source_sha256: String,
    pub clob_v2_adapter_signing_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunClobV2CollateralAccountingSourceProof {
    pub clob_v2_collateral_accounting_verified: bool,
    pub clob_v2_collateral_accounting_source_sha256: String,
    pub clob_v2_collateral_accounting_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunClobV2FeeBehaviorSourceProof {
    pub clob_v2_fee_behavior_verified: bool,
    pub clob_v2_fee_behavior_source_sha256: String,
    pub clob_v2_fee_behavior_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunMarketWindowSourceProof {
    pub market_state_approved: bool,
    pub market_window_approved: bool,
    pub market_state_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunSingleRunnerLockSourceProof {
    pub single_runner_lock_acquired: bool,
    pub single_runner_lock_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8PreRunEgressIdentitySourceProof {
    pub egress_identity_approved: bool,
    pub egress_identity_source_sha256: String,
    pub egress_identity_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8AbortPlanCancelIfOpenSourceProof {
    pub cancel_if_open_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8AbortPlanNtAcceptedVenuePendingSourceProof {
    pub nt_accepted_venue_pending_abort_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8AbortPlanPartialFillSourceProof {
    pub partial_fill_abort_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8AbortPlanNetworkPartitionSourceProof {
    pub network_partition_during_submit_abort_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8AbortPlanPanicGateServicePolicySourceProof {
    pub panic_gate_trip_abort_evidence_hash: String,
}

impl fmt::Debug for WrittenOperatorArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrittenOperatorArtifact")
            .field("path", &"[redacted-operator-artifact-path]")
            .field("sha256", &self.sha256)
            .finish()
    }
}

pub enum BoltV3OperatorArtifactError {
    UnsupportedProvider {
        client_key: String,
        provider_key: String,
    },
    SystemTimeBeforeUnixEpoch {
        source: SystemTimeError,
    },
    DataClientNtSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    DataClientNtSourceInvalid {
        field: &'static str,
    },
    DataClientLiveNodeMappingSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    DataClientLiveNodeMappingSourceInvalid {
        field: &'static str,
    },
    DataClientBehaviorObservationSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    DataClientBehaviorObservationSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    DataClientBehaviorObservationSourceInvalid {
        field: &'static str,
    },
    DataClientProductionReadinessMatrixSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    DataClientProductionReadinessMatrixSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    DataClientProductionReadinessMatrixSourceInvalid {
        field: &'static str,
    },
    ProviderArtifactInvalid {
        artifact: &'static str,
        field: &'static str,
    },
    SecretInventory(BoltV3SecretError),
    FinancialEnvelope(anyhow::Error),
    MarketSelection(anyhow::Error),
    MarketSelectionPrerequisiteUnproven {
        prerequisite: &'static str,
    },
    StrategyInputPrerequisiteUnproven {
        prerequisite: &'static str,
    },
    DecisionEvidenceSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    DecisionEvidenceSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    DecisionEvidenceSourceInvalid {
        message: String,
    },
    GateEvidenceInvalid {
        field: &'static str,
    },
    EntryReadinessGateSessionInvalid {
        message: String,
    },
    DecisionEvidenceFileRead {
        path: PathBuf,
        source: std::io::Error,
    },
    MarketSelectionSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    MarketSelectionSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    MarketSelectionInstrumentSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    MarketSelectionInstrumentSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    MarketSelectionInstrumentSourceInvalid {
        field: &'static str,
    },
    PreRunStatePrerequisiteUnproven {
        prerequisite: &'static str,
    },
    PreRunReleaseManifestSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunReleaseManifestSourceInvalid {
        field: &'static str,
    },
    PreRunHostClockSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunHostClockSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    PreRunHostClockSourceMaterialize {
        message: String,
    },
    PreRunHostClockSourceInvalid {
        field: &'static str,
    },
    PreRunVenueAccountStateSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunVenueAccountStateSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    PreRunVenueAccountStateSourceInvalid {
        field: &'static str,
    },
    PreRunFundingMarginSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunFundingMarginSourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    PreRunFundingMarginSourceInvalid {
        field: &'static str,
    },
    PreRunClobV2SourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunClobV2SourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    PreRunClobV2SourceInvalid {
        field: &'static str,
    },
    PreRunMarketWindowSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunMarketWindowSourceInvalid {
        field: &'static str,
    },
    PreRunSingleRunnerLockSourceInvalid {
        field: &'static str,
    },
    PreRunEgressIdentitySourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunEgressIdentitySourceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    PreRunEgressIdentitySourceInvalid {
        field: &'static str,
    },
    PreRunStateSourceBundleRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunStateSourceBundleParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    PreRunStateSourceBundleInvalid {
        field: &'static str,
    },
    AbortPrerequisiteUnproven {
        prerequisite: &'static str,
    },
    AbortPlanCancelIfOpenSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    AbortPlanCancelIfOpenSourceInvalid {
        field: &'static str,
    },
    AbortPlanNtAcceptedVenuePendingSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    AbortPlanNtAcceptedVenuePendingSourceInvalid {
        field: &'static str,
    },
    AbortPlanPartialFillSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    AbortPlanPartialFillSourceInvalid {
        field: &'static str,
    },
    AbortPlanNetworkPartitionSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    AbortPlanNetworkPartitionSourceInvalid {
        field: &'static str,
    },
    AbortPlanPanicGateServicePolicySourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    AbortPlanPanicGateServicePolicySourceInvalid {
        field: &'static str,
    },
    AbortPlanSourceBundleRead {
        path: PathBuf,
        source: std::io::Error,
    },
    AbortPlanSourceBundleParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    AbortPlanSourceBundleInvalid {
        field: &'static str,
    },
    MissingLiveCanary,
    MissingOperatorEvidence,
    OperatorEvidenceJsonRead {
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorEvidenceJsonParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    OperatorEvidenceTomlRead {
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorEvidenceTomlParse {
        source: toml::de::Error,
    },
    OperatorEvidenceTomlSerialize {
        source: toml::ser::Error,
    },
    OperatorEvidenceTomlWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorEvidenceTomlInvalid {
        field: &'static str,
    },
    BuildHeadShaUnavailable,
    OperatorEvidenceHeadShaMismatch,
    StaticManifestRead {
        path: PathBuf,
        source: std::io::Error,
    },
    StaticManifestParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    StaticManifestSchema {
        field: &'static str,
    },
    StaticManifestConfigBundleDrift,
    StaticManifestBlockers {
        count: usize,
    },
    StaticManifestMissingArtifact {
        name: &'static str,
    },
    StaticManifestDuplicateArtifact {
        name: String,
    },
    StaticManifestArtifactPathMismatch {
        name: &'static str,
    },
    StaticManifestArtifactHashMismatch {
        name: &'static str,
    },
    StaticManifestArtifactHashShape {
        field: &'static str,
    },
    StaticManifestArtifactFileRead {
        name: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    StaticManifestArtifactFileHashMismatch {
        name: &'static str,
        path: PathBuf,
    },
    InvalidOperatorEvidenceHash {
        field: &'static str,
    },
    InvalidOutputPath {
        field: &'static str,
    },
    InvalidOutputPathParent {
        field: &'static str,
    },
    OutputPathCollision,
    OperatorPacketRead {
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorPacketParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    OperatorPacketSchema {
        field: &'static str,
    },
    OperatorPacketConfigBundleDrift,
    OperatorPacketStaticManifestHashMismatch,
    OperatorPacketEvidenceMismatch {
        field: &'static str,
    },
    OperatorPacketHashShape {
        field: &'static str,
    },
    StrategyInputReplayInvalid {
        field: &'static str,
    },
    ApprovalEnvelopeRead {
        path: PathBuf,
        source: std::io::Error,
    },
    ApprovalEnvelopeParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    ApprovalEnvelopeSchema {
        field: &'static str,
    },
    ApprovalEnvelopeHashMismatch,
    ApprovalEnvelopeMismatch {
        field: &'static str,
    },
    FinalEvidenceRead {
        field: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    FinalEvidenceParse {
        field: &'static str,
        path: PathBuf,
        source: serde_json::Error,
    },
    FinalEvidenceSchema {
        field: &'static str,
    },
    FinalEvidenceMismatch {
        field: &'static str,
    },
    FinalEvidenceHashMismatch {
        field: &'static str,
    },
    Random(getrandom::Error),
    Serialize(serde_json::Error),
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for BoltV3OperatorArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProvider {
                client_key,
                provider_key,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` is not supported by this build"
            ),
            Self::SystemTimeBeforeUnixEpoch { source } => {
                write!(f, "system time is before Unix epoch: {source}")
            }
            Self::DataClientNtSourceRead { source, .. } => {
                write!(f, "failed to read NT data-client source input: {source}")
            }
            Self::DataClientNtSourceInvalid { field } => write!(
                f,
                "NT data-client source capability field `{field}` is invalid or unproven"
            ),
            Self::DataClientLiveNodeMappingSourceRead { source, .. } => write!(
                f,
                "failed to read data-client LiveNode mapping source input: {source}"
            ),
            Self::DataClientLiveNodeMappingSourceInvalid { field } => write!(
                f,
                "data-client LiveNode mapping source field `{field}` is invalid or unproven"
            ),
            Self::DataClientBehaviorObservationSourceRead { source, .. } => write!(
                f,
                "failed to read data-client behavior observation source input: {source}"
            ),
            Self::DataClientBehaviorObservationSourceParse { source, .. } => write!(
                f,
                "failed to parse data-client behavior observation source input: {source}"
            ),
            Self::DataClientBehaviorObservationSourceInvalid { field } => write!(
                f,
                "data-client behavior observation source field `{field}` is invalid or unproven"
            ),
            Self::DataClientProductionReadinessMatrixSourceRead { source, .. } => write!(
                f,
                "failed to read data-client production-readiness matrix source input: {source}"
            ),
            Self::DataClientProductionReadinessMatrixSourceParse { source, .. } => write!(
                f,
                "failed to parse data-client production-readiness matrix source input: {source}"
            ),
            Self::DataClientProductionReadinessMatrixSourceInvalid { field } => write!(
                f,
                "data-client production-readiness matrix source field `{field}` is invalid or unproven"
            ),
            Self::ProviderArtifactInvalid { artifact, field } => write!(
                f,
                "provider artifact `{artifact}` field `{field}` is invalid or unproven"
            ),
            Self::SecretInventory(error) => write!(f, "{error}"),
            Self::FinancialEnvelope(error) => write!(f, "{error}"),
            Self::MarketSelection(error) => {
                write!(
                    f,
                    "failed to build market selection source evidence: {error}"
                )
            }
            Self::MarketSelectionPrerequisiteUnproven { prerequisite } => write!(
                f,
                "refusing to write market selection source evidence because {prerequisite}"
            ),
            Self::StrategyInputPrerequisiteUnproven { prerequisite } => write!(
                f,
                "refusing to write successful strategy-input evidence because {prerequisite}"
            ),
            Self::DecisionEvidenceSourceRead { source, .. } => {
                write!(f, "failed to read entry decision evidence source: {source}")
            }
            Self::DecisionEvidenceSourceParse { source, .. } => {
                write!(
                    f,
                    "failed to parse entry decision evidence source: {source}"
                )
            }
            Self::DecisionEvidenceSourceInvalid { message } => {
                write!(f, "entry decision evidence source is invalid: {message}")
            }
            Self::GateEvidenceInvalid { field } => {
                write!(f, "gate evidence field `{field}` is invalid or unproven")
            }
            Self::EntryReadinessGateSessionInvalid { message } => {
                write!(f, "entry readiness gate session is invalid: {message}")
            }
            Self::DecisionEvidenceFileRead { source, .. } => {
                write!(f, "failed to read entry decision evidence JSONL: {source}")
            }
            Self::MarketSelectionSourceRead { source, .. } => {
                write!(
                    f,
                    "failed to read market-selection source evidence: {source}"
                )
            }
            Self::MarketSelectionSourceParse { source, .. } => {
                write!(
                    f,
                    "failed to parse market-selection source evidence: {source}"
                )
            }
            Self::MarketSelectionInstrumentSourceRead { source, .. } => {
                write!(
                    f,
                    "failed to read market-selection instrument source: {source}"
                )
            }
            Self::MarketSelectionInstrumentSourceParse { source, .. } => {
                write!(
                    f,
                    "failed to parse market-selection instrument source: {source}"
                )
            }
            Self::MarketSelectionInstrumentSourceInvalid { field } => write!(
                f,
                "market-selection instrument source field `{field}` is invalid or unproven"
            ),
            Self::PreRunStatePrerequisiteUnproven { prerequisite } => write!(
                f,
                "refusing to write successful pre-run state evidence because {prerequisite}"
            ),
            Self::PreRunReleaseManifestSourceRead { source, .. } => {
                write!(f, "failed to read release manifest source input: {source}")
            }
            Self::PreRunReleaseManifestSourceInvalid { field } => write!(
                f,
                "release manifest source field `{field}` is invalid or unproven"
            ),
            Self::PreRunHostClockSourceRead { source, .. } => {
                write!(f, "failed to read host-clock source input: {source}")
            }
            Self::PreRunHostClockSourceParse { source, .. } => {
                write!(f, "failed to parse host-clock source input: {source}")
            }
            Self::PreRunHostClockSourceMaterialize { message } => {
                write!(
                    f,
                    "failed to materialize host-clock source input: {message}"
                )
            }
            Self::PreRunHostClockSourceInvalid { field } => write!(
                f,
                "host-clock source field `{field}` is invalid or unproven"
            ),
            Self::PreRunVenueAccountStateSourceRead { source, .. } => {
                write!(
                    f,
                    "failed to read venue account state source input: {source}"
                )
            }
            Self::PreRunVenueAccountStateSourceParse { source, .. } => {
                write!(
                    f,
                    "failed to parse venue account state source input: {source}"
                )
            }
            Self::PreRunVenueAccountStateSourceInvalid { field } => write!(
                f,
                "venue account state source field `{field}` is invalid or unproven"
            ),
            Self::PreRunFundingMarginSourceRead { source, .. } => {
                write!(f, "failed to read funding margin source input: {source}")
            }
            Self::PreRunFundingMarginSourceParse { source, .. } => {
                write!(f, "failed to parse funding margin source input: {source}")
            }
            Self::PreRunFundingMarginSourceInvalid { field } => write!(
                f,
                "funding margin source field `{field}` is invalid or unproven"
            ),
            Self::PreRunClobV2SourceRead { source, .. } => {
                write!(f, "failed to read CLOB V2 source input: {source}")
            }
            Self::PreRunClobV2SourceParse { source, .. } => {
                write!(f, "failed to parse CLOB V2 source input: {source}")
            }
            Self::PreRunClobV2SourceInvalid { field } => {
                write!(f, "CLOB V2 source field `{field}` is invalid or unproven")
            }
            Self::PreRunMarketWindowSourceRead { source, .. } => {
                write!(f, "failed to read market/window source input: {source}")
            }
            Self::PreRunMarketWindowSourceInvalid { field } => write!(
                f,
                "market/window source field `{field}` is invalid or unproven"
            ),
            Self::PreRunSingleRunnerLockSourceInvalid { field } => write!(
                f,
                "single-runner lock source field `{field}` is invalid or unproven"
            ),
            Self::PreRunEgressIdentitySourceRead { source, .. } => {
                write!(f, "failed to read egress identity source input: {source}")
            }
            Self::PreRunEgressIdentitySourceParse { source, .. } => {
                write!(f, "failed to parse egress identity source input: {source}")
            }
            Self::PreRunEgressIdentitySourceInvalid { field } => write!(
                f,
                "egress identity source field `{field}` is invalid or unproven"
            ),
            Self::PreRunStateSourceBundleRead { source, .. } => {
                write!(f, "failed to read pre-run state source bundle: {source}")
            }
            Self::PreRunStateSourceBundleParse { source, .. } => {
                write!(f, "failed to parse pre-run state source bundle: {source}")
            }
            Self::PreRunStateSourceBundleInvalid { field } => write!(
                f,
                "pre-run state source bundle field `{field}` is invalid or unproven"
            ),
            Self::AbortPrerequisiteUnproven { prerequisite } => write!(
                f,
                "refusing to write successful abort plan because {prerequisite} is not proven"
            ),
            Self::AbortPlanCancelIfOpenSourceRead { source, .. } => {
                write!(
                    f,
                    "failed to read abort plan cancel-if-open source: {source}"
                )
            }
            Self::AbortPlanCancelIfOpenSourceInvalid { field } => write!(
                f,
                "abort plan cancel-if-open source field `{field}` is invalid or unproven"
            ),
            Self::AbortPlanNtAcceptedVenuePendingSourceRead { source, .. } => write!(
                f,
                "failed to read abort plan NT-accepted venue-pending source: {source}"
            ),
            Self::AbortPlanNtAcceptedVenuePendingSourceInvalid { field } => write!(
                f,
                "abort plan NT-accepted venue-pending source field `{field}` is invalid or unproven"
            ),
            Self::AbortPlanPartialFillSourceRead { source, .. } => {
                write!(f, "failed to read abort plan partial-fill source: {source}")
            }
            Self::AbortPlanPartialFillSourceInvalid { field } => write!(
                f,
                "abort plan partial-fill source field `{field}` is invalid or unproven"
            ),
            Self::AbortPlanNetworkPartitionSourceRead { source, .. } => write!(
                f,
                "failed to read abort plan network-partition source: {source}"
            ),
            Self::AbortPlanNetworkPartitionSourceInvalid { field } => write!(
                f,
                "abort plan network-partition source field `{field}` is invalid or unproven"
            ),
            Self::AbortPlanPanicGateServicePolicySourceRead { source, .. } => write!(
                f,
                "failed to read abort plan panic-gate/service-policy source: {source}"
            ),
            Self::AbortPlanPanicGateServicePolicySourceInvalid { field } => write!(
                f,
                "abort plan panic-gate/service-policy source field `{field}` is invalid or unproven"
            ),
            Self::AbortPlanSourceBundleRead { source, .. } => {
                write!(f, "failed to read abort plan source bundle: {source}")
            }
            Self::AbortPlanSourceBundleParse { source, .. } => {
                write!(f, "failed to parse abort plan source bundle: {source}")
            }
            Self::AbortPlanSourceBundleInvalid { field } => write!(
                f,
                "abort plan source bundle field `{field}` is invalid or unproven"
            ),
            Self::MissingLiveCanary => write!(
                f,
                "refusing to assemble operator packet because `[live_canary]` is missing"
            ),
            Self::MissingOperatorEvidence => write!(
                f,
                "refusing to assemble operator packet because `[live_canary.operator_evidence]` is missing"
            ),
            Self::OperatorEvidenceJsonRead { source, .. } => {
                write!(f, "failed to read operator evidence JSON: {source}")
            }
            Self::OperatorEvidenceJsonParse { source, .. } => {
                write!(f, "failed to parse operator evidence JSON: {source}")
            }
            Self::OperatorEvidenceTomlRead { source, .. } => {
                write!(f, "failed to read operator evidence TOML: {source}")
            }
            Self::OperatorEvidenceTomlParse { source } => {
                write!(
                    f,
                    "failed to parse patched operator evidence TOML: {source}"
                )
            }
            Self::OperatorEvidenceTomlSerialize { source } => {
                write!(f, "failed to render operator evidence TOML: {source}")
            }
            Self::OperatorEvidenceTomlWrite { source, .. } => {
                write!(
                    f,
                    "failed to write patched operator evidence TOML: {source}"
                )
            }
            Self::OperatorEvidenceTomlInvalid { field } => write!(
                f,
                "operator evidence TOML field `{field}` is invalid or unproven"
            ),
            Self::BuildHeadShaUnavailable => write!(
                f,
                "bolt-v3 operator packet build head_sha is unavailable or invalid"
            ),
            Self::OperatorEvidenceHeadShaMismatch => write!(
                f,
                "`[live_canary.operator_evidence].head_sha` does not match build head_sha"
            ),
            Self::StaticManifestRead { source, .. } => {
                write!(f, "failed to read static manifest: {source}")
            }
            Self::StaticManifestParse { source, .. } => {
                write!(f, "failed to parse static manifest: {source}")
            }
            Self::StaticManifestSchema { field } => {
                write!(f, "static manifest field `{field}` is invalid")
            }
            Self::StaticManifestConfigBundleDrift => write!(
                f,
                "static manifest config_bundle_checksum does not match loaded config"
            ),
            Self::StaticManifestBlockers { count } => write!(
                f,
                "refusing to assemble operator packet because static manifest blockers are present: {count}"
            ),
            Self::StaticManifestMissingArtifact { name } => {
                write!(f, "static manifest missing required artifact `{name}`")
            }
            Self::StaticManifestDuplicateArtifact { name } => {
                write!(f, "static manifest has duplicate artifact `{name}`")
            }
            Self::StaticManifestArtifactPathMismatch { name } => write!(
                f,
                "static manifest artifact `{name}` path does not match configured operator evidence"
            ),
            Self::StaticManifestArtifactHashMismatch { name } => write!(
                f,
                "static manifest artifact `{name}` sha256 does not match configured operator evidence"
            ),
            Self::StaticManifestArtifactHashShape { field } => write!(
                f,
                "static manifest field `{field}` must be a lowercase sha256 hex string"
            ),
            Self::StaticManifestArtifactFileRead { name, source, .. } => write!(
                f,
                "failed to read static manifest artifact `{name}`: {source}"
            ),
            Self::StaticManifestArtifactFileHashMismatch { name, .. } => {
                write!(f, "static manifest artifact `{name}` file hash mismatch")
            }
            Self::InvalidOperatorEvidenceHash { field } => write!(
                f,
                "`[live_canary.operator_evidence].{field}` must be a lowercase sha256 hex string"
            ),
            Self::InvalidOutputPath { field } => write!(
                f,
                "operator packet output path field `{field}` must not contain parent-directory components"
            ),
            Self::InvalidOutputPathParent { field } => write!(
                f,
                "operator packet output path field `{field}` parent must be a real directory or creatable descendant"
            ),
            Self::OutputPathCollision => write!(
                f,
                "operator packet output path must differ from approval_envelope_path"
            ),
            Self::OperatorPacketRead { source, .. } => {
                write!(f, "failed to read operator packet: {source}")
            }
            Self::OperatorPacketParse { source, .. } => {
                write!(f, "failed to parse operator packet: {source}")
            }
            Self::OperatorPacketSchema { field } => {
                write!(f, "operator packet field `{field}` is invalid")
            }
            Self::OperatorPacketConfigBundleDrift => write!(
                f,
                "operator packet config_bundle_checksum does not match loaded config"
            ),
            Self::OperatorPacketStaticManifestHashMismatch => write!(
                f,
                "operator packet static_manifest_sha256 does not match static manifest file"
            ),
            Self::OperatorPacketEvidenceMismatch { field } => write!(
                f,
                "operator packet live_canary_operator_evidence field `{field}` does not match loaded config"
            ),
            Self::OperatorPacketHashShape { field } => {
                write!(
                    f,
                    "operator packet field `{field}` must be a lowercase sha256 hex string"
                )
            }
            Self::StrategyInputReplayInvalid { field } => write!(
                f,
                "strategy_input_replay field `{field}` is invalid or not bound to decision evidence"
            ),
            Self::ApprovalEnvelopeRead { source, .. } => {
                write!(f, "failed to read approval envelope: {source}")
            }
            Self::ApprovalEnvelopeParse { source, .. } => {
                write!(f, "failed to parse approval envelope: {source}")
            }
            Self::ApprovalEnvelopeSchema { field } => {
                write!(f, "approval envelope field `{field}` is invalid")
            }
            Self::ApprovalEnvelopeHashMismatch => write!(
                f,
                "approval envelope file hash does not match configured operator evidence"
            ),
            Self::ApprovalEnvelopeMismatch { field } => {
                write!(
                    f,
                    "approval envelope field `{field}` does not match configured operator evidence"
                )
            }
            Self::FinalEvidenceRead { field, source, .. } => {
                write!(f, "failed to read final evidence `{field}`: {source}")
            }
            Self::FinalEvidenceParse { field, source, .. } => {
                write!(f, "failed to parse final evidence `{field}`: {source}")
            }
            Self::FinalEvidenceSchema { field } => {
                write!(f, "final evidence field `{field}` is invalid")
            }
            Self::FinalEvidenceMismatch { field } => {
                write!(
                    f,
                    "final evidence field `{field}` does not match configured operator evidence"
                )
            }
            Self::FinalEvidenceHashMismatch { field } => {
                write!(f, "final evidence field `{field}` file hash mismatch")
            }
            Self::Random(error) => write!(f, "failed to generate approval nonce bytes: {error}"),
            Self::Serialize(error) => write!(f, "failed to serialize operator artifact: {error}"),
            Self::Write { source, .. } => {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    write!(
                        f,
                        "refusing to overwrite existing operator artifact: already exists"
                    )
                } else {
                    write!(f, "failed to write operator artifact: {source}")
                }
            }
        }
    }
}

impl fmt::Debug for BoltV3OperatorArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for BoltV3OperatorArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SystemTimeBeforeUnixEpoch { source } => Some(source),
            Self::SecretInventory(error) => Some(error),
            Self::FinancialEnvelope(error) => Some(error.as_ref()),
            Self::MarketSelection(error) => Some(error.as_ref()),
            Self::DataClientNtSourceRead { source, .. } => Some(source),
            Self::DataClientLiveNodeMappingSourceRead { source, .. } => Some(source),
            Self::DataClientBehaviorObservationSourceRead { source, .. } => Some(source),
            Self::DataClientBehaviorObservationSourceParse { source, .. } => Some(source),
            Self::DataClientProductionReadinessMatrixSourceRead { source, .. } => Some(source),
            Self::DataClientProductionReadinessMatrixSourceParse { source, .. } => Some(source),
            Self::DecisionEvidenceSourceRead { source, .. } => Some(source),
            Self::DecisionEvidenceSourceParse { source, .. } => Some(source),
            Self::DecisionEvidenceFileRead { source, .. } => Some(source),
            Self::MarketSelectionSourceRead { source, .. } => Some(source),
            Self::MarketSelectionSourceParse { source, .. } => Some(source),
            Self::MarketSelectionInstrumentSourceRead { source, .. } => Some(source),
            Self::MarketSelectionInstrumentSourceParse { source, .. } => Some(source),
            Self::PreRunReleaseManifestSourceRead { source, .. } => Some(source),
            Self::PreRunHostClockSourceRead { source, .. } => Some(source),
            Self::PreRunHostClockSourceParse { source, .. } => Some(source),
            Self::PreRunVenueAccountStateSourceRead { source, .. } => Some(source),
            Self::PreRunVenueAccountStateSourceParse { source, .. } => Some(source),
            Self::PreRunFundingMarginSourceRead { source, .. } => Some(source),
            Self::PreRunFundingMarginSourceParse { source, .. } => Some(source),
            Self::PreRunClobV2SourceRead { source, .. } => Some(source),
            Self::PreRunClobV2SourceParse { source, .. } => Some(source),
            Self::PreRunMarketWindowSourceRead { source, .. } => Some(source),
            Self::PreRunEgressIdentitySourceRead { source, .. } => Some(source),
            Self::PreRunEgressIdentitySourceParse { source, .. } => Some(source),
            Self::PreRunStateSourceBundleRead { source, .. } => Some(source),
            Self::PreRunStateSourceBundleParse { source, .. } => Some(source),
            Self::AbortPlanCancelIfOpenSourceRead { source, .. } => Some(source),
            Self::AbortPlanNtAcceptedVenuePendingSourceRead { source, .. } => Some(source),
            Self::AbortPlanPartialFillSourceRead { source, .. } => Some(source),
            Self::AbortPlanNetworkPartitionSourceRead { source, .. } => Some(source),
            Self::AbortPlanPanicGateServicePolicySourceRead { source, .. } => Some(source),
            Self::AbortPlanSourceBundleRead { source, .. } => Some(source),
            Self::AbortPlanSourceBundleParse { source, .. } => Some(source),
            Self::OperatorEvidenceJsonRead { source, .. } => Some(source),
            Self::OperatorEvidenceJsonParse { source, .. } => Some(source),
            Self::OperatorEvidenceTomlRead { source, .. } => Some(source),
            Self::OperatorEvidenceTomlParse { source } => Some(source),
            Self::OperatorEvidenceTomlSerialize { source } => Some(source),
            Self::OperatorEvidenceTomlWrite { source, .. } => Some(source),
            Self::StaticManifestRead { source, .. } => Some(source),
            Self::StaticManifestParse { source, .. } => Some(source),
            Self::StaticManifestArtifactFileRead { source, .. } => Some(source),
            Self::OperatorPacketRead { source, .. } => Some(source),
            Self::OperatorPacketParse { source, .. } => Some(source),
            Self::ApprovalEnvelopeRead { source, .. } => Some(source),
            Self::ApprovalEnvelopeParse { source, .. } => Some(source),
            Self::FinalEvidenceRead { source, .. } => Some(source),
            Self::FinalEvidenceParse { source, .. } => Some(source),
            Self::Random(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::Write { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<BoltV3SecretError> for BoltV3OperatorArtifactError {
    fn from(error: BoltV3SecretError) -> Self {
        Self::SecretInventory(error)
    }
}

pub fn normalize_gate_evidence(
    input: GateEvidenceInput,
) -> Result<GateEvidence, BoltV3OperatorArtifactError> {
    if input.collection_status != GateEvidenceCollectionStatus::Complete {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "collection_status",
        });
    }
    ensure_gate_field(GATE_FIELD_ROLE, &input.role)?;
    ensure_gate_field(GATE_FIELD_PROVIDER_ID, &input.provider_id)?;
    ensure_gate_field(GATE_FIELD_PROVIDER_KIND, &input.provider_kind)?;
    ensure_gate_field(GATE_FIELD_SELECTED_MARKET_KEY, &input.selected_market_key)?;
    ensure_gate_field(GATE_FIELD_VALUE_KIND, &input.value_kind)?;
    if !is_lowercase_sha256(&input.selected_market_key) {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "selected_market_key",
        });
    }
    if input.collector_observed_at_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "collector_observed_at_ms",
        });
    }
    if input.source_observed_at_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "source_observed_at_ms",
        });
    }
    if input.freshness_max_age_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "freshness_max_age_ms",
        });
    }
    if input.normalized_value.is_null() {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "normalized_value",
        });
    }
    validate_gate_artifact_refs(&input.artifact_refs)?;
    validate_provider_provenance(&input.provider_kind, &input.provider_provenance)?;
    let fresh_until_ms = input
        .collector_observed_at_ms
        .checked_add(input.freshness_max_age_ms)
        .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "fresh_until_ms",
        })?;
    Ok(GateEvidence {
        schema_version: GATE_EVIDENCE_SCHEMA_VERSION,
        record_kind: GATE_EVIDENCE_RECORD_KIND.to_string(),
        role: input.role,
        provider_id: input.provider_id,
        provider_kind: input.provider_kind,
        selected_market_key: input.selected_market_key,
        collector_observed_at_ms: input.collector_observed_at_ms,
        source_observed_at_ms: input.source_observed_at_ms,
        fresh_until_ms,
        value_kind: input.value_kind,
        normalized_value_sha256: canonical_json_sha256_value(&input.normalized_value)?,
        normalized_value: input.normalized_value,
        provider_provenance_sha256: canonical_json_sha256_value(&input.provider_provenance)?,
        provider_provenance: input.provider_provenance,
        artifact_refs: input.artifact_refs,
    })
}

pub fn collect_entry_readiness_gate_evidence_from_source_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: EntryReadinessGateEvidenceSourceFileRequest<'_>,
) -> Result<GateEvidence, BoltV3OperatorArtifactError> {
    let bytes =
        read_file_bounded(request.source_path, request.max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DecisionEvidenceSourceRead {
                path: request.source_path.to_path_buf(),
                source,
            }
        })?;
    let source_sha256 = hex::encode(Sha256::digest(&bytes));
    if !is_lowercase_sha256(request.expected_source_sha256)
        || source_sha256 != request.expected_source_sha256
    {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source hash does not match expected sha256",
        ));
    }
    let provider = gate_provider_evidence_binding(loaded, request.provider_id)?;
    match provider.provider_kind.as_str() {
        CHAINLINK_DATA_STREAMS_PROVIDER_KIND => {
            collect_chainlink_readiness_gate_evidence_from_source_bytes(
                loaded,
                strategy_instance_id,
                request,
                &provider,
                &bytes,
                source_sha256,
            )
        }
        HYPERLIQUID_HIP4_PROVIDER_KIND | VENUE_NATIVE_PROVIDER_KIND => {
            collect_normalized_metadata_readiness_gate_evidence_from_source_bytes(
                loaded,
                strategy_instance_id,
                request,
                &provider,
                &bytes,
                source_sha256,
            )
        }
        other => Err(entry_decision_source_invalid(format!(
            "entry readiness gate source collection does not support provider_kind `{other}`"
        ))),
    }
}

fn collect_chainlink_readiness_gate_evidence_from_source_bytes(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: EntryReadinessGateEvidenceSourceFileRequest<'_>,
    provider: &GateProviderEvidenceBinding,
    bytes: &[u8],
    source_sha256: String,
) -> Result<GateEvidence, BoltV3OperatorArtifactError> {
    let source: SourceBoundPriceToBeatSource = serde_json::from_slice(bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceParse {
            path: request.source_path.to_path_buf(),
            source,
        }
    })?;
    validate_source_bound_price_to_beat_shape(&source)?;
    validate_price_to_beat_report_provenance(
        loaded,
        strategy_instance_id,
        &source,
        source.market_selection_timestamp_ms,
        source.decision_timestamp_ms,
    )?;
    validate_entry_readiness_evidence_collection_binding(
        loaded,
        strategy_instance_id,
        &request,
        provider,
        PRICE_GATE_VALUE_KIND,
    )?;
    let binding = price_to_beat_report_binding(loaded, strategy_instance_id)?;
    if binding.provider_id != request.provider_id {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source provider_id does not match configured report binding",
        ));
    }
    let report_sha256 = source
        .source_report_full_sha256
        .as_deref()
        .filter(|value| is_lowercase_sha256(value))
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let report_schema_version = source
        .source_report_schema_version
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let report_decimal_scale = source
        .source_report_decimal_scale
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let report_feed_id = source
        .source_report_feed_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let valid_from_timestamp_ms = source
        .source_report_valid_from_timestamp_ms
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let observations_timestamp_ms = source
        .source_report_observations_timestamp_ms
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    normalize_gate_evidence(GateEvidenceInput {
        role: request.role.to_string(),
        provider_id: request.provider_id.to_string(),
        provider_kind: provider.provider_kind.clone(),
        selected_market_key: request.selected_market.selected_market_key.clone(),
        collector_observed_at_ms: request.collector_observed_at_ms,
        source_observed_at_ms: observations_timestamp_ms,
        freshness_max_age_ms: provider.max_age_ms,
        value_kind: PRICE_GATE_VALUE_KIND.to_string(),
        normalized_value: serde_json::json!({
            "price_to_beat_value": source.price_to_beat_value,
        }),
        provider_provenance: serde_json::json!({
            "provider_kind": CHAINLINK_DATA_STREAMS_PROVIDER_KIND,
            "feed_id": report_feed_id,
            "report_schema_version": report_schema_version,
            "report_decimal_scale": report_decimal_scale,
            "source_report_full_sha256": report_sha256,
            "valid_from_timestamp_ms": valid_from_timestamp_ms,
            "observations_timestamp_ms": observations_timestamp_ms,
        }),
        artifact_refs: vec![
            source_artifact_ref(&request, source_sha256)?,
            GateArtifactRef {
                path: ENTRY_READINESS_CHAINLINK_REPORT_ARTIFACT_PATH.to_string(),
                sha256: report_sha256.to_string(),
            },
        ],
        collection_status: GateEvidenceCollectionStatus::Complete,
    })
}

fn collect_normalized_metadata_readiness_gate_evidence_from_source_bytes(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: EntryReadinessGateEvidenceSourceFileRequest<'_>,
    provider: &GateProviderEvidenceBinding,
    bytes: &[u8],
    source_sha256: String,
) -> Result<GateEvidence, BoltV3OperatorArtifactError> {
    let source: NormalizedReadinessGateSource =
        serde_json::from_slice(bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DecisionEvidenceSourceParse {
                path: request.source_path.to_path_buf(),
                source,
            }
        })?;
    if source.schema_version != NORMALIZED_READINESS_GATE_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "normalized readiness gate source schema_version is invalid",
        ));
    }
    if source.record_kind != NORMALIZED_READINESS_GATE_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "normalized readiness gate source record_kind is invalid",
        ));
    }
    if source.provider_kind != provider.provider_kind {
        return Err(entry_decision_source_invalid(
            "normalized readiness gate source provider_kind is invalid",
        ));
    }
    validate_entry_readiness_evidence_collection_binding(
        loaded,
        strategy_instance_id,
        &request,
        provider,
        &source.value_kind,
    )?;
    normalize_gate_evidence(GateEvidenceInput {
        role: request.role.to_string(),
        provider_id: request.provider_id.to_string(),
        provider_kind: provider.provider_kind.clone(),
        selected_market_key: request.selected_market.selected_market_key.clone(),
        collector_observed_at_ms: request.collector_observed_at_ms,
        source_observed_at_ms: source.source_observed_at_ms,
        freshness_max_age_ms: provider.max_age_ms,
        value_kind: source.value_kind,
        normalized_value: source.normalized_value,
        provider_provenance: source.provider_provenance,
        artifact_refs: vec![source_artifact_ref(&request, source_sha256)?],
        collection_status: GateEvidenceCollectionStatus::Complete,
    })
}

fn validate_entry_readiness_evidence_collection_binding(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: &EntryReadinessGateEvidenceSourceFileRequest<'_>,
    provider: &GateProviderEvidenceBinding,
    value_kind: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if request.collector_observed_at_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source collector_observed_at_ms is invalid",
        ));
    }
    ensure_gate_field(GATE_FIELD_ROLE, request.role)?;
    ensure_gate_field(GATE_FIELD_PROVIDER_ID, request.provider_id)?;
    ensure_gate_field(GATE_FIELD_ARTIFACT_REF_PATH, request.artifact_ref_path)?;
    if provider.provider_kind != request.selected_market.resolution_kind {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source provider_kind does not match selected market",
        ));
    }
    if request.selected_market.value_kind != value_kind {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source value_kind does not match selected market",
        ));
    }
    let capability = gate_provider_capability_for_role_name(request.role)?;
    if !provider
        .capabilities
        .iter()
        .any(|provider_capability| provider_capability == capability)
    {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source provider capability does not satisfy role",
        ));
    }
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(|| {
            entry_decision_source_invalid(format!(
                "strategy instance `{strategy_instance_id}` is not loaded"
            ))
        })?;
    let subscription = target_gate_subscription(strategy, request.role)?;
    if !subscription.required {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source role subscription must be required",
        ));
    }
    if !subscription_allows_value_kind(&subscription, value_kind) {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source value_kind is not allowed by target subscription",
        ));
    }
    if !subscription_allows_provider(
        &subscription,
        request.provider_id,
        provider.provider_kind.as_str(),
    ) {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source provider is not allowed by target subscription",
        ));
    }
    if !subscription_mapping_matches_selected_market(
        &subscription,
        request.selected_market,
        request.provider_id,
    ) {
        return Err(entry_decision_source_invalid(
            "entry readiness gate source provider mapping does not match selected market",
        ));
    }
    Ok(())
}

fn gate_provider_capability_for_role_name(
    role: &str,
) -> Result<&'static str, BoltV3OperatorArtifactError> {
    match role {
        RESOLUTION_GATE_ROLE => Ok(GATE_PROVIDER_CAPABILITY_RESOLUTION_VALUE),
        DECISION_REFERENCE_GATE_ROLE => Ok(GATE_PROVIDER_CAPABILITY_REFERENCE_VALUE),
        _ => Err(entry_decision_source_invalid(
            "entry readiness gate source role is unsupported",
        )),
    }
}

fn source_artifact_ref(
    request: &EntryReadinessGateEvidenceSourceFileRequest<'_>,
    sha256: String,
) -> Result<GateArtifactRef, BoltV3OperatorArtifactError> {
    ensure_gate_field(GATE_FIELD_ARTIFACT_REF_PATH, request.artifact_ref_path)?;
    Ok(GateArtifactRef {
        path: request.artifact_ref_path.to_string(),
        sha256,
    })
}

pub fn build_entry_readiness_gate_session(
    request: EntryReadinessGateSessionRequest<'_>,
) -> Result<EntryReadinessGateSession, BoltV3OperatorArtifactError> {
    if request.created_at_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS {
        return Err(entry_readiness_error("created_at_ms must be non-zero"));
    }
    ensure_gate_field(
        GATE_FIELD_STRATEGY_INSTANCE_ID,
        request.strategy_instance_id,
    )?;
    let strategy = request
        .loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == request.strategy_instance_id)
        .ok_or_else(|| {
            entry_readiness_error(format!(
                "strategy instance `{}` is not loaded",
                request.strategy_instance_id
            ))
        })?;
    let Some(target) = strategy.config.target.as_table() else {
        return Err(entry_readiness_error("strategy target must be a table"));
    };
    let configured_target_id = target
        .get(GATE_FIELD_CONFIGURED_TARGET_ID)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| entry_readiness_error("target.configured_target_id is missing"))?;
    if configured_target_id != request.selected_market.configured_target_id {
        return Err(entry_readiness_error(
            "selected market configured_target_id does not match strategy target",
        ));
    }
    if !is_lowercase_sha256(&request.selected_market.selected_market_key) {
        return Err(entry_readiness_error(
            "selected_market_key must be lowercase sha256",
        ));
    }

    let mut satisfied_roles = BTreeMap::new();
    for requirement in request
        .requirements
        .iter()
        .filter(|requirement| requirement.required)
    {
        let role_name = gate_role_name(requirement.role);
        let subscription = target_gate_subscription(strategy, role_name)?;
        if !subscription.required {
            return Err(entry_readiness_error(format!(
                "target.gate_subscriptions.{role_name}.required must be true for required archetype roles"
            )));
        }
        if request.selected_market.resolution_kind == NO_RESOLUTION_KIND {
            if requirement.allow_no_resolution
                && subscription.allow_no_resolution
                && request.selected_market.value_kind == NO_RESOLUTION_VALUE_KIND
            {
                satisfied_roles.insert(
                    role_name.to_string(),
                    GateSatisfaction::NoResolution {
                        selected_market_key: request.selected_market.selected_market_key.clone(),
                        resolution_identity: request.selected_market.resolution_identity.clone(),
                    },
                );
                continue;
            }
            return Err(entry_readiness_error(format!(
                "role `{role_name}` does not allow no_resolution satisfaction"
            )));
        }

        let mut candidates = Vec::new();
        for evidence in request.provider_evidence {
            if evidence.role != role_name
                || evidence.selected_market_key != request.selected_market.selected_market_key
            {
                continue;
            }
            if evidence_satisfies_requirement(
                request.loaded,
                request.selected_market,
                requirement,
                &subscription,
                evidence,
                request.created_at_ms,
            )? {
                candidates.push(evidence);
            }
        }
        if candidates.is_empty() {
            return Err(entry_readiness_error(format!(
                "no provider evidence satisfied role `{role_name}`"
            )));
        }
        let selected = select_gate_evidence_by_preference(role_name, &subscription, &candidates)?;
        satisfied_roles.insert(
            role_name.to_string(),
            GateSatisfaction::Evidence {
                evidence: Box::new(selected.clone()),
            },
        );
    }
    if satisfied_roles.is_empty() {
        return Err(entry_readiness_error(
            "no required gate roles were satisfied",
        ));
    }
    validate_gate_artifact_refs(&request.artifact_refs)?;
    let mut session = EntryReadinessGateSession {
        schema_version: ENTRY_READINESS_GATE_SESSION_SCHEMA_VERSION,
        record_kind: ENTRY_READINESS_GATE_SESSION_RECORD_KIND.to_string(),
        strategy_instance_id: request.strategy_instance_id.to_string(),
        configured_target_id: request.selected_market.configured_target_id.clone(),
        selected_market: request.selected_market.clone(),
        created_at_ms: request.created_at_ms,
        satisfied_roles,
        session_hash: String::new(),
        artifact_refs: request.artifact_refs,
    };
    session.session_hash =
        entry_readiness_session_hash(request.loaded, &session).map_err(|message| {
            entry_readiness_error(format!("session hash canonicalization failed: {message}"))
        })?;
    Ok(session)
}

fn evidence_satisfies_requirement(
    loaded: &LoadedBoltV3Config,
    selected_market: &SelectedMarketRequirement,
    requirement: &ArchetypeGateRequirement,
    subscription: &GateSessionTargetSubscription,
    evidence: &GateEvidence,
    created_at_ms: u64,
) -> Result<bool, BoltV3OperatorArtifactError> {
    if !requirement
        .accepted_value_kinds
        .contains(&gate_value_kind_from_name(&evidence.value_kind)?)
    {
        return Ok(false);
    }
    if !subscription_allows_value_kind(subscription, &evidence.value_kind) {
        return Ok(false);
    }
    if evidence.value_kind != selected_market.value_kind {
        return Ok(false);
    }
    let provider = gate_provider_evidence_binding(loaded, &evidence.provider_id)?;
    if provider.provider_kind != evidence.provider_kind {
        return Ok(false);
    }
    if !provider
        .capabilities
        .iter()
        .any(|capability| capability == gate_provider_capability_for_role(requirement.role))
    {
        return Ok(false);
    }
    if !subscription_allows_provider(subscription, &evidence.provider_id, &evidence.provider_kind) {
        return Ok(false);
    }
    if !subscription_mapping_matches_selected_market(
        subscription,
        selected_market,
        &evidence.provider_id,
    ) {
        return Ok(false);
    }
    validate_gate_evidence_integrity(evidence)?;
    if evidence.collector_observed_at_ms > created_at_ms || created_at_ms > evidence.fresh_until_ms
    {
        return Ok(false);
    }
    if evidence
        .collector_observed_at_ms
        .checked_add(provider.max_age_ms)
        != Some(evidence.fresh_until_ms)
    {
        return Ok(false);
    }
    if evidence
        .collector_observed_at_ms
        .abs_diff(evidence.source_observed_at_ms)
        > provider.max_clock_skew_ms
    {
        return Ok(false);
    }
    Ok(true)
}

fn target_gate_subscription(
    strategy: &crate::bolt_v3_config::LoadedStrategy,
    role_name: &str,
) -> Result<GateSessionTargetSubscription, BoltV3OperatorArtifactError> {
    let subscription_value = strategy
        .config
        .target
        .as_table()
        .and_then(|target| target.get(GATE_FIELD_GATE_SUBSCRIPTIONS))
        .and_then(toml::Value::as_table)
        .and_then(|subscriptions| subscriptions.get(role_name))
        .ok_or_else(|| {
            entry_readiness_error(format!("target.gate_subscriptions.{role_name} is missing"))
        })?;
    subscription_value
        .clone()
        .try_into()
        .map_err(|source: toml::de::Error| {
            entry_readiness_error(format!(
                "target.gate_subscriptions.{role_name} is invalid: {source}"
            ))
        })
}

fn select_gate_evidence_by_preference<'a>(
    role_name: &str,
    subscription: &GateSessionTargetSubscription,
    candidates: &[&'a GateEvidence],
) -> Result<&'a GateEvidence, BoltV3OperatorArtifactError> {
    if let [candidate] = candidates {
        return Ok(*candidate);
    }
    let Some(preference) = subscription.provider_preference.as_deref() else {
        return Err(entry_readiness_error(format!(
            "multiple provider evidence items satisfy role `{role_name}` without provider_preference"
        )));
    };
    for provider_id in preference {
        let mut matching = candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.provider_id == *provider_id);
        let first = matching.next();
        if first.is_some() && matching.next().is_some() {
            return Err(entry_readiness_error(format!(
                "multiple provider evidence items share preferred provider `{provider_id}` for role `{role_name}`"
            )));
        }
        if let Some(candidate) = first {
            return Ok(candidate);
        }
    }
    Err(entry_readiness_error(format!(
        "provider_preference does not deterministically select evidence for role `{role_name}`"
    )))
}

fn subscription_allows_provider(
    subscription: &GateSessionTargetSubscription,
    provider_id: &str,
    provider_kind: &str,
) -> bool {
    let id_allowed = subscription
        .allowed_provider_ids
        .as_deref()
        .map(|ids| ids.iter().any(|allowed| allowed == provider_id))
        .unwrap_or(true);
    let kind_allowed = subscription
        .allowed_provider_kinds
        .as_deref()
        .map(|kinds| kinds.iter().any(|allowed| allowed == provider_kind))
        .unwrap_or(true);
    id_allowed && kind_allowed
}

fn subscription_allows_value_kind(
    subscription: &GateSessionTargetSubscription,
    value_kind: &str,
) -> bool {
    subscription
        .allowed_value_kinds
        .as_deref()
        .map(|kinds| kinds.iter().any(|allowed| allowed == value_kind))
        .unwrap_or(true)
}

fn subscription_mapping_matches_selected_market(
    subscription: &GateSessionTargetSubscription,
    selected_market: &SelectedMarketRequirement,
    provider_id: &str,
) -> bool {
    subscription
        .market_mappings
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .any(|mapping| {
            mapping.family_key == selected_market.family_key
                && mapping.market_class == selected_market.market_class
                && mapping.resolution_kind == selected_market.resolution_kind
                && mapping.resolution_identity == selected_market.resolution_identity
                && mapping.value_kind == selected_market.value_kind
                && mapping
                    .provider_id
                    .as_deref()
                    .map(|mapped_provider| mapped_provider == provider_id)
                    .unwrap_or(true)
        })
}

fn validate_gate_evidence_integrity(
    evidence: &GateEvidence,
) -> Result<(), BoltV3OperatorArtifactError> {
    if evidence.schema_version != GATE_EVIDENCE_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "schema_version",
        });
    }
    if evidence.record_kind != GATE_EVIDENCE_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "record_kind",
        });
    }
    validate_gate_artifact_refs(&evidence.artifact_refs)?;
    validate_provider_provenance(&evidence.provider_kind, &evidence.provider_provenance)?;
    if canonical_json_sha256_value(&evidence.normalized_value)? != evidence.normalized_value_sha256
    {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "normalized_value_sha256",
        });
    }
    if canonical_json_sha256_value(&evidence.provider_provenance)?
        != evidence.provider_provenance_sha256
    {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "provider_provenance_sha256",
        });
    }
    Ok(())
}

fn validate_provider_provenance(
    provider_kind: &str,
    provenance: &serde_json::Value,
) -> Result<(), BoltV3OperatorArtifactError> {
    let provenance_kind = provenance
        .get(GATE_FIELD_PROVIDER_KIND)
        .and_then(serde_json::Value::as_str)
        .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "provider_provenance.provider_kind",
        })?;
    if provenance_kind != provider_kind {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "provider_provenance.provider_kind",
        });
    }
    Ok(())
}

fn validate_gate_artifact_refs(
    artifact_refs: &[GateArtifactRef],
) -> Result<(), BoltV3OperatorArtifactError> {
    if artifact_refs.is_empty() {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "artifact_refs",
        });
    }
    for artifact_ref in artifact_refs {
        ensure_gate_field(GATE_FIELD_ARTIFACT_REFS_PATH, &artifact_ref.path)?;
        if !is_lowercase_sha256(&artifact_ref.sha256) {
            return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
                field: "artifact_refs.sha256",
            });
        }
    }
    Ok(())
}

fn entry_readiness_session_hash(
    loaded: &LoadedBoltV3Config,
    session: &EntryReadinessGateSession,
) -> Result<String, String> {
    let mut satisfied_roles = Vec::new();
    for (role, satisfaction) in &session.satisfied_roles {
        let mut role_input = serde_json::Map::new();
        role_input.insert(GATE_FIELD_ROLE.to_string(), serde_json::json!(role));
        match satisfaction {
            GateSatisfaction::Evidence { evidence } => {
                let mut artifact_refs = evidence.artifact_refs.clone();
                artifact_refs.sort_by(|left, right| left.path.cmp(&right.path));
                let artifact_sha256s: Vec<_> = artifact_refs
                    .iter()
                    .map(|artifact_ref| artifact_ref.sha256.clone())
                    .collect();
                role_input.insert(
                    GATE_FIELD_SATISFACTION_KIND.to_string(),
                    serde_json::json!(GATE_SATISFACTION_KIND_EVIDENCE),
                );
                role_input.insert(
                    GATE_FIELD_PROVIDER_ID.to_string(),
                    serde_json::json!(evidence.provider_id),
                );
                role_input.insert(
                    GATE_FIELD_PROVIDER_KIND.to_string(),
                    serde_json::json!(evidence.provider_kind),
                );
                role_input.insert(
                    GATE_FIELD_VALUE_KIND.to_string(),
                    serde_json::json!(evidence.value_kind),
                );
                role_input.insert(
                    GATE_FIELD_NORMALIZED_VALUE_SHA256.to_string(),
                    serde_json::json!(evidence.normalized_value_sha256),
                );
                role_input.insert(
                    GATE_FIELD_ARTIFACT_SHA256S.to_string(),
                    serde_json::json!(artifact_sha256s),
                );
                role_input.insert(
                    GATE_FIELD_PROVIDER_PROVENANCE_SHA256.to_string(),
                    serde_json::json!(evidence.provider_provenance_sha256),
                );
            }
            GateSatisfaction::NoResolution {
                selected_market_key,
                resolution_identity,
            } => {
                role_input.insert(
                    GATE_FIELD_SATISFACTION_KIND.to_string(),
                    serde_json::json!(GATE_SATISFACTION_KIND_NO_RESOLUTION),
                );
                role_input.insert(
                    GATE_FIELD_SELECTED_MARKET_KEY.to_string(),
                    serde_json::json!(selected_market_key),
                );
                role_input.insert(
                    GATE_FIELD_RESOLUTION_IDENTITY.to_string(),
                    serde_json::json!(resolution_identity),
                );
            }
        }
        satisfied_roles.push(serde_json::Value::Object(role_input));
    }
    let mut session_artifact_refs = session.artifact_refs.clone();
    session_artifact_refs.sort_by(|left, right| left.path.cmp(&right.path));

    let mut hash_input = serde_json::Map::new();
    hash_input.insert(
        GATE_FIELD_SCHEMA_VERSION.to_string(),
        serde_json::json!(ENTRY_READINESS_GATE_SESSION_SCHEMA_VERSION),
    );
    hash_input.insert(
        GATE_FIELD_STRATEGY_INSTANCE_ID.to_string(),
        serde_json::json!(session.strategy_instance_id),
    );
    hash_input.insert(
        GATE_FIELD_CONFIGURED_TARGET_ID.to_string(),
        serde_json::json!(session.configured_target_id),
    );
    hash_input.insert(
        GATE_FIELD_ROOT_CONFIG_SHA256.to_string(),
        serde_json::json!(loaded.config_bundle_checksum),
    );
    hash_input.insert(
        GATE_FIELD_SELECTED_MARKET_KEY.to_string(),
        serde_json::json!(session.selected_market.selected_market_key),
    );
    hash_input.insert(
        GATE_FIELD_SELECTED_AT_MS.to_string(),
        serde_json::json!(session.selected_market.selected_at_ms.to_string()),
    );
    hash_input.insert(
        GATE_FIELD_CREATED_AT_MS.to_string(),
        serde_json::json!(session.created_at_ms.to_string()),
    );
    hash_input.insert(
        GATE_FIELD_SATISFIED_ROLES.to_string(),
        serde_json::Value::Array(satisfied_roles),
    );
    hash_input.insert(
        GATE_FIELD_ARTIFACT_REFS.to_string(),
        serde_json::json!(session_artifact_refs),
    );
    canonical_json_sha256_value(&serde_json::Value::Object(hash_input)).map_err(|error| match error
    {
        BoltV3OperatorArtifactError::Serialize(source) => source.to_string(),
        other => other.to_string(),
    })
}

fn canonical_json_sha256_value(
    value: &serde_json::Value,
) -> Result<String, BoltV3OperatorArtifactError> {
    let canonical = canonical_json_value(value);
    let bytes = serde_json::to_vec(&canonical).map_err(BoltV3OperatorArtifactError::Serialize)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn canonical_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json_value).collect())
        }
        serde_json::Value::Object(object) => {
            let sorted: BTreeMap<_, _> = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json_value(value)))
                .collect();
            serde_json::json!(sorted)
        }
        scalar => scalar.clone(),
    }
}

fn ensure_gate_field(field: &'static str, value: &str) -> Result<(), BoltV3OperatorArtifactError> {
    if value.trim().is_empty() {
        return Err(BoltV3OperatorArtifactError::GateEvidenceInvalid { field });
    }
    Ok(())
}

fn gate_role_name(role: GateRole) -> &'static str {
    match role {
        GateRole::Resolution => RESOLUTION_GATE_ROLE,
        GateRole::DecisionReference => DECISION_REFERENCE_GATE_ROLE,
    }
}

fn gate_value_kind_from_name(value: &str) -> Result<GateValueKind, BoltV3OperatorArtifactError> {
    match value {
        PRICE_GATE_VALUE_KIND => Ok(GateValueKind::Price),
        GATE_VALUE_KIND_INDEX => Ok(GateValueKind::Index),
        GATE_VALUE_KIND_OUTCOME => Ok(GateValueKind::Outcome),
        GATE_VALUE_KIND_METADATA => Ok(GateValueKind::Metadata),
        _ => Err(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "value_kind",
        }),
    }
}

fn gate_provider_capability_for_role(role: GateRole) -> &'static str {
    match role {
        GateRole::Resolution => GATE_PROVIDER_CAPABILITY_RESOLUTION_VALUE,
        GateRole::DecisionReference => GATE_PROVIDER_CAPABILITY_REFERENCE_VALUE,
    }
}

fn entry_readiness_error(message: impl Into<String>) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::EntryReadinessGateSessionInvalid {
        message: message.into(),
    }
}

pub fn build_redacted_ssm_manifest(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3RedactedSsmManifest, BoltV3OperatorArtifactError> {
    let mut entries = Vec::new();
    for (client_key, client) in &loaded.root.clients {
        if client.secrets.is_none() {
            continue;
        }
        let provider_key = client.venue.as_str();
        let binding = binding_for_provider_key(provider_key).ok_or_else(|| {
            BoltV3OperatorArtifactError::UnsupportedProvider {
                client_key: client_key.clone(),
                provider_key: provider_key.to_string(),
            }
        })?;
        let paths = (binding.configured_secret_paths)(ProviderSecretResolveContext {
            client_key,
            region: loaded.root.aws.region.as_str(),
            client,
        })?;
        for path in paths {
            entries.push(BoltV3RedactedSsmManifestEntry {
                client_key: client_key.clone(),
                provider_key: provider_key.to_string(),
                field_name: path.field_name,
            });
        }
    }
    entries.sort_by(|left, right| {
        (
            left.client_key.as_str(),
            left.provider_key.as_str(),
            left.field_name,
        )
            .cmp(&(
                right.client_key.as_str(),
                right.provider_key.as_str(),
                right.field_name,
            ))
    });

    Ok(BoltV3RedactedSsmManifest {
        schema_version: REDACTED_SSM_MANIFEST_SCHEMA_VERSION,
        record_kind: REDACTED_SSM_MANIFEST_RECORD_KIND,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        aws_region: loaded.root.aws.region.clone(),
        entries,
    })
}

pub fn write_data_client_readiness_source_artifact_from_config(
    loaded: &LoadedBoltV3Config,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_readiness_source_artifact(loaded)?;
    write_json_artifact_create_new(output_path, &artifact)
}

pub fn write_data_client_nt_source_capability_artifact_from_config(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    nt_adapter_source_path: &Path,
    max_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_nt_source_capability_artifact(
        loaded,
        client_key,
        nt_adapter_source_path,
        max_source_bytes,
    )?;
    write_json_artifact_create_new(output_path, &artifact)
}

pub fn write_data_client_live_node_mapping_source_artifact_from_config(
    loaded: &LoadedBoltV3Config,
    registration_summary: &BoltV3RegistrationSummary,
    live_node_source_path: &Path,
    adapter_mapping_source_path: &Path,
    provider_registry_source_path: &Path,
    max_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_live_node_mapping_source_artifact(
        loaded,
        registration_summary,
        live_node_source_path,
        adapter_mapping_source_path,
        provider_registry_source_path,
        max_source_bytes,
    )?;
    write_json_artifact_create_new(output_path, &artifact)
}

pub fn write_data_client_behavior_observation_artifact_from_source_file(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    behavior_source_path: &Path,
    max_behavior_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_behavior_observation_artifact_from_source_file(
        loaded,
        client_key,
        behavior_source_path,
        max_behavior_source_bytes,
    )?;
    write_json_artifact_create_new(output_path, &artifact)
}

pub struct DataClientProductionReadinessMatrixSourceFileRequest<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub readiness_source_path: &'a Path,
    pub live_node_mapping_source_path: &'a Path,
    pub nt_source_capability_paths: &'a [PathBuf],
    pub target_candidate_paths: &'a [PathBuf],
    pub behavior_observation_paths: &'a [PathBuf],
    pub max_source_bytes: u64,
    pub output_path: &'a Path,
}

pub fn write_data_client_production_readiness_matrix_artifact_from_source_files(
    request: DataClientProductionReadinessMatrixSourceFileRequest<'_>,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_production_readiness_matrix_artifact_from_source_files(
        request.loaded,
        request.readiness_source_path,
        request.live_node_mapping_source_path,
        request.nt_source_capability_paths,
        request.target_candidate_paths,
        request.behavior_observation_paths,
        request.max_source_bytes,
    )?;
    write_json_artifact_create_new(request.output_path, &artifact)
}

pub fn write_data_client_behavior_observation_source_from_probe_events(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    probe_events_path: &Path,
    max_probe_events_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let source = build_data_client_behavior_observation_source_from_probe_events(
        loaded,
        client_key,
        probe_events_path,
        max_probe_events_bytes,
        None,
    )?;
    write_json_artifact_create_new(output_path, &source)
}

pub fn write_data_client_behavior_observation_source_from_probe_events_and_policy_source(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    probe_events_path: &Path,
    max_probe_events_bytes: u64,
    policy_source_path: &Path,
    max_policy_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let policy_source = read_data_client_policy_behavior_source_artifact(
        loaded,
        client_key,
        policy_source_path,
        max_policy_source_bytes,
    )?;
    let source = build_data_client_behavior_observation_source_from_probe_events(
        loaded,
        client_key,
        probe_events_path,
        max_probe_events_bytes,
        Some(&policy_source),
    )?;
    write_json_artifact_create_new(output_path, &source)
}

pub fn write_data_client_policy_behavior_source_artifact_from_nt_sources(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    nt_policy_source_paths: &[PathBuf],
    max_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_policy_behavior_source_artifact_from_nt_sources(
        loaded,
        client_key,
        nt_policy_source_paths,
        max_source_bytes,
    )?;
    write_json_artifact_create_new(output_path, &artifact)
}

pub fn write_data_client_behavior_probe_events_from_no_submit_evidence(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let events =
        data_client_behavior_probe_events_from_no_submit_evidence(loaded, client_key, evidence)?;
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, &event)
            .map_err(BoltV3OperatorArtifactError::Serialize)?;
        bytes.push(b'\n');
    }
    write_json_artifact_create_new_from_bytes(output_path, &bytes)
}

pub fn write_data_client_behavior_probe_events_from_no_submit_readiness_evidence(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    evidence: &BoltV3NoSubmitDataClientReadinessEvidence,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let events = data_client_behavior_probe_events_from_no_submit_readiness_evidence(
        loaded, client_key, evidence,
    )?;
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, &event)
            .map_err(BoltV3OperatorArtifactError::Serialize)?;
        bytes.push(b'\n');
    }
    write_json_artifact_create_new_from_bytes(output_path, &bytes)
}

pub fn write_data_client_readiness_target_candidates_from_no_submit_readiness_evidence(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    evidence: &BoltV3NoSubmitDataClientReadinessEvidence,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_data_client_readiness_target_candidates_from_no_submit_readiness_evidence(
        loaded, client_key, evidence,
    )?;
    write_json_artifact_create_new(output_path, &artifact)
}

fn data_client_behavior_probe_events_from_no_submit_evidence(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key",
            },
        );
    }
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "client_key",
        },
    )?;
    if client.data.is_none() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key.data",
            },
        );
    }
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let mut events = data_client_quote_probe_events_from_no_submit_evidence(
        client_key,
        provider_key,
        client,
        evidence,
    )?;
    if events.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "no_submit_reference_quote_evidence",
            },
        );
    }
    sort_and_validate_data_client_behavior_probe_events(&mut events, client_key, provider_key)?;
    Ok(events)
}

fn data_client_behavior_probe_events_from_no_submit_readiness_evidence(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    evidence: &BoltV3NoSubmitDataClientReadinessEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key",
            },
        );
    }
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "client_key",
        },
    )?;
    if client.data.is_none() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key.data",
            },
        );
    }
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;

    let mut events = data_client_metadata_probe_events_from_no_submit_evidence(
        client_key,
        provider_key,
        &evidence.metadata,
    )?;
    if let Some(readiness_probe) = &client.readiness_probe {
        match readiness_probe.market_data_kind {
            DataClientReadinessProbeMarketDataKind::Quote => {
                events.extend(
                    data_client_quote_probe_events_from_no_submit_readiness_evidence(
                        client_key,
                        provider_key,
                        client,
                        &evidence.metadata,
                        &evidence.quotes,
                    )?,
                );
            }
            DataClientReadinessProbeMarketDataKind::Book => {
                events.extend(
                    data_client_book_probe_events_from_no_submit_readiness_evidence(
                        client_key,
                        provider_key,
                        client,
                        &evidence.metadata,
                        &evidence.books,
                    )?,
                );
            }
            DataClientReadinessProbeMarketDataKind::Trade => {
                events.extend(
                    data_client_trade_probe_events_from_no_submit_readiness_evidence(
                        client_key,
                        provider_key,
                        client,
                        &evidence.metadata,
                        &evidence.trades,
                    )?,
                );
            }
        }
    } else if !evidence.quotes.quotes.is_empty() {
        events.extend(data_client_quote_probe_events_from_no_submit_evidence(
            client_key,
            provider_key,
            client,
            &evidence.quotes,
        )?);
    }
    if events.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "no_submit_data_client_readiness_evidence",
            },
        );
    }
    sort_and_validate_data_client_behavior_probe_events(&mut events, client_key, provider_key)?;
    Ok(events)
}

fn data_client_metadata_probe_events_from_no_submit_evidence(
    client_key: &str,
    provider_key: &str,
    evidence: &crate::bolt_v3_live_node::BoltV3NoSubmitDataClientMetadataEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let client_key_hash = sha256_text(client_key);
    let mut events = Vec::new();
    for response in &evidence.responses {
        if response.data_client_id != client_key || response.venue != provider_key {
            continue;
        }
        if response.instrument_ids.is_empty() {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.instrument_ids",
                },
            );
        }
        let observed_at_unix_millis = nanos_to_millis_checked(
            response.captured_at_unix_nanos,
            "metadata.captured_at_unix_nanos",
        )?;
        let latency_millis = nanos_delta_to_millis_checked(
            response.captured_at_unix_nanos,
            response.ts_init_unix_nanos,
            "metadata.ts_init_unix_nanos",
        )?;
        events.push(DataClientBehaviorProbeEvent {
            schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
            record_kind: DATA_CLIENT_BEHAVIOR_PROBE_EVENT_RECORD_KIND.to_string(),
            client_key_hash: client_key_hash.clone(),
            provider_key: provider_key.to_string(),
            observed_at_unix_millis,
            event_kind: "metadata".to_string(),
            supported_by_nt_source: true,
            observed_through_live_node: true,
            age_millis: Some(latency_millis),
            latency_millis: Some(latency_millis),
            event_clock_skew_millis: None,
            recovered: None,
            fail_closed: None,
            evidence_sha256: Some(data_client_metadata_probe_evidence_hash(response)?),
            unsupported_disposition: None,
        });
    }
    if events.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "no_submit_data_client_metadata_evidence",
            },
        );
    }
    Ok(events)
}

fn data_client_quote_probe_events_from_no_submit_evidence(
    client_key: &str,
    provider_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let quote_targets = data_client_readiness_quote_target_instruments(client)?;
    data_client_quote_probe_events_for_targets(client_key, provider_key, evidence, &quote_targets)
}

fn data_client_quote_probe_events_from_no_submit_readiness_evidence(
    client_key: &str,
    provider_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    metadata: &crate::bolt_v3_live_node::BoltV3NoSubmitDataClientMetadataEvidence,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let quote_targets = data_client_readiness_quote_target_instruments_for_evidence(
        client_key,
        provider_key,
        client,
        metadata,
    )?;
    let events = data_client_quote_probe_events_for_targets(
        client_key,
        provider_key,
        evidence,
        &quote_targets,
    )?;
    if let Some(readiness_probe) = client.readiness_probe.as_ref()
        && readiness_probe.quote_target_source
            == DataClientReadinessProbeQuoteTargetSource::MetadataResponse
    {
        let observed_targets: BTreeSet<&str> = evidence
            .quotes
            .iter()
            .filter(|quote| quote.data_client_id == client_key)
            .filter(|quote| quote_targets.contains(&quote.instrument_id))
            .map(|quote| quote.instrument_id.as_str())
            .collect();
        // Mirror the live probe's success criterion: every sampled target must
        // stream a quote unless `min_observed_targets` lowers the bar to that
        // many distinct sampled targets (default unset = strict all).
        let required_observations = readiness_probe
            .min_observed_targets
            .map(|min_observed| min_observed.clamp(1, quote_targets.len().max(1)))
            .unwrap_or(quote_targets.len());
        if observed_targets.len() < required_observations {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.instrument_ids.quotes",
                },
            );
        }
    }
    Ok(events)
}

fn data_client_book_probe_events_from_no_submit_readiness_evidence(
    client_key: &str,
    provider_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    metadata: &crate::bolt_v3_live_node::BoltV3NoSubmitDataClientMetadataEvidence,
    evidence: &crate::bolt_v3_live_node::BoltV3NoSubmitBookDeltasEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let book_targets = data_client_readiness_quote_target_instruments_for_evidence(
        client_key,
        provider_key,
        client,
        metadata,
    )?;
    let events = data_client_book_probe_events_for_targets(
        client_key,
        provider_key,
        evidence,
        &book_targets,
    )?;
    if let Some(readiness_probe) = client.readiness_probe.as_ref()
        && readiness_probe.quote_target_source
            == DataClientReadinessProbeQuoteTargetSource::MetadataResponse
    {
        let observed_targets: BTreeSet<&str> = evidence
            .deltas
            .iter()
            .filter(|deltas| deltas.data_client_id == client_key)
            .filter(|deltas| book_targets.contains(&deltas.instrument_id))
            .map(|deltas| deltas.instrument_id.as_str())
            .collect();
        // Mirror the live probe's success criterion: every sampled target must
        // stream a book delta unless `min_observed_targets` lowers the bar, in
        // which case observing at least that many distinct sampled targets is
        // the proof. Keeps the artifact materializer consistent with
        // `BoltV3NoSubmitReferenceQuoteProbeHandle::required_observation_count`.
        let required_observations = readiness_probe
            .min_observed_targets
            .map(|min_observed| min_observed.clamp(1, book_targets.len().max(1)))
            .unwrap_or(book_targets.len());
        if observed_targets.len() < required_observations {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.instrument_ids.books",
                },
            );
        }
    }
    Ok(events)
}

fn data_client_trade_probe_events_from_no_submit_readiness_evidence(
    client_key: &str,
    provider_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    metadata: &crate::bolt_v3_live_node::BoltV3NoSubmitDataClientMetadataEvidence,
    evidence: &crate::bolt_v3_live_node::BoltV3NoSubmitTradeEvidence,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    if let Some(readiness_probe) = client.readiness_probe.as_ref()
        && readiness_probe.market_data_kind == DataClientReadinessProbeMarketDataKind::Trade
        && readiness_probe.quote_target_source
            == DataClientReadinessProbeQuoteTargetSource::MetadataResponse
        && readiness_probe.chunk_size.is_some()
    {
        // Trade chunk-count probe: the certified set is the markets that
        // actually traded during the walk, which is live (not a config-derivable
        // sample), so the materializer cannot re-derive it. Derive the target
        // set from the recorded trades instead and require >= m
        // (min_observed_targets) distinct firing markets via the same pass rule
        // the live probe applies (`trade_chunk_count_probe_passed`).
        let trade_targets: BTreeSet<String> = evidence
            .trades
            .iter()
            .filter(|trade| trade.data_client_id == client_key)
            .map(|trade| trade.instrument_id.clone())
            .collect();
        let events = data_client_trade_probe_events_for_targets(
            client_key,
            provider_key,
            evidence,
            &trade_targets,
        )?;
        let required_live_markets = readiness_probe.min_observed_targets.unwrap_or(0);
        if !trade_chunk_count_probe_passed(trade_targets.len(), required_live_markets) {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.instrument_ids.trades",
                },
            );
        }
        return Ok(events);
    }
    let trade_targets = data_client_readiness_quote_target_instruments_for_evidence(
        client_key,
        provider_key,
        client,
        metadata,
    )?;
    let events = data_client_trade_probe_events_for_targets(
        client_key,
        provider_key,
        evidence,
        &trade_targets,
    )?;
    if let Some(readiness_probe) = client.readiness_probe.as_ref()
        && readiness_probe.quote_target_source
            == DataClientReadinessProbeQuoteTargetSource::MetadataResponse
    {
        let observed_targets: BTreeSet<&str> = evidence
            .trades
            .iter()
            .filter(|trade| trade.data_client_id == client_key)
            .filter(|trade| trade_targets.contains(&trade.instrument_id))
            .map(|trade| trade.instrument_id.as_str())
            .collect();
        // Mirror the live probe's success criterion: every sampled target must
        // stream a trade unless `min_observed_targets` lowers the bar, in which
        // case observing at least that many distinct sampled targets is the
        // proof. Keeps the artifact materializer consistent with
        // `BoltV3NoSubmitReferenceQuoteProbeHandle::required_observation_count`.
        let required_observations = readiness_probe
            .min_observed_targets
            .map(|min_observed| min_observed.clamp(1, trade_targets.len().max(1)))
            .unwrap_or(trade_targets.len());
        if observed_targets.len() < required_observations {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.instrument_ids.trades",
                },
            );
        }
    }
    Ok(events)
}

fn data_client_quote_probe_events_for_targets(
    client_key: &str,
    provider_key: &str,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
    quote_targets: &BTreeSet<String>,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let client_key_hash = sha256_text(client_key);
    let mut events = Vec::new();
    for quote in &evidence.quotes {
        if quote.data_client_id != client_key || !quote_targets.contains(&quote.instrument_id) {
            continue;
        }
        let observed_at_unix_millis =
            nanos_to_millis_checked(quote.captured_at_unix_nanos, "quote.captured_at_unix_nanos")?;
        if observed_at_unix_millis == 0 {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "quote.captured_at_unix_nanos",
                },
            );
        }
        let (age_millis, event_clock_skew_millis) = nanos_age_and_clock_skew_millis(
            quote.captured_at_unix_nanos,
            quote.ts_event_unix_nanos,
        );
        let latency_millis = nanos_delta_to_millis_checked(
            quote.captured_at_unix_nanos,
            quote.ts_init_unix_nanos,
            "quote.ts_init_unix_nanos",
        )?;
        events.push(DataClientBehaviorProbeEvent {
            schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
            record_kind: DATA_CLIENT_BEHAVIOR_PROBE_EVENT_RECORD_KIND.to_string(),
            client_key_hash: client_key_hash.clone(),
            provider_key: provider_key.to_string(),
            observed_at_unix_millis,
            event_kind: "quote".to_string(),
            supported_by_nt_source: true,
            observed_through_live_node: true,
            age_millis: Some(age_millis),
            latency_millis: Some(latency_millis),
            event_clock_skew_millis,
            recovered: None,
            fail_closed: None,
            evidence_sha256: Some(data_client_quote_probe_evidence_hash(quote)?),
            unsupported_disposition: None,
        });
    }
    Ok(events)
}

fn data_client_book_probe_events_for_targets(
    client_key: &str,
    provider_key: &str,
    evidence: &crate::bolt_v3_live_node::BoltV3NoSubmitBookDeltasEvidence,
    book_targets: &BTreeSet<String>,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let client_key_hash = sha256_text(client_key);
    let mut events = Vec::new();
    for deltas in &evidence.deltas {
        if deltas.data_client_id != client_key || !book_targets.contains(&deltas.instrument_id) {
            continue;
        }
        if deltas.delta_count == 0 {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "book.delta_count",
                },
            );
        }
        let observed_at_unix_millis =
            nanos_to_millis_checked(deltas.captured_at_unix_nanos, "book.captured_at_unix_nanos")?;
        if observed_at_unix_millis == 0 {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "book.captured_at_unix_nanos",
                },
            );
        }
        let (age_millis, event_clock_skew_millis) = nanos_age_and_clock_skew_millis(
            deltas.captured_at_unix_nanos,
            deltas.ts_event_unix_nanos,
        );
        let latency_millis = nanos_delta_to_millis_checked(
            deltas.captured_at_unix_nanos,
            deltas.ts_init_unix_nanos,
            "book.ts_init_unix_nanos",
        )?;
        events.push(DataClientBehaviorProbeEvent {
            schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
            record_kind: DATA_CLIENT_BEHAVIOR_PROBE_EVENT_RECORD_KIND.to_string(),
            client_key_hash: client_key_hash.clone(),
            provider_key: provider_key.to_string(),
            observed_at_unix_millis,
            event_kind: "book".to_string(),
            supported_by_nt_source: true,
            observed_through_live_node: true,
            age_millis: Some(age_millis),
            latency_millis: Some(latency_millis),
            event_clock_skew_millis,
            recovered: None,
            fail_closed: None,
            evidence_sha256: Some(data_client_book_probe_evidence_hash(deltas)?),
            unsupported_disposition: None,
        });
    }
    Ok(events)
}

fn data_client_trade_probe_events_for_targets(
    client_key: &str,
    provider_key: &str,
    evidence: &crate::bolt_v3_live_node::BoltV3NoSubmitTradeEvidence,
    trade_targets: &BTreeSet<String>,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let client_key_hash = sha256_text(client_key);
    let mut events = Vec::new();
    for trade in &evidence.trades {
        if trade.data_client_id != client_key || !trade_targets.contains(&trade.instrument_id) {
            continue;
        }
        if trade.size <= 0.0 || !trade.size.is_finite() {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "trade.size",
                },
            );
        }
        let observed_at_unix_millis =
            nanos_to_millis_checked(trade.captured_at_unix_nanos, "trade.captured_at_unix_nanos")?;
        if observed_at_unix_millis == 0 {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "trade.captured_at_unix_nanos",
                },
            );
        }
        let (age_millis, event_clock_skew_millis) = nanos_age_and_clock_skew_millis(
            trade.captured_at_unix_nanos,
            trade.ts_event_unix_nanos,
        );
        let latency_millis = nanos_delta_to_millis_checked(
            trade.captured_at_unix_nanos,
            trade.ts_init_unix_nanos,
            "trade.ts_init_unix_nanos",
        )?;
        events.push(DataClientBehaviorProbeEvent {
            schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
            record_kind: DATA_CLIENT_BEHAVIOR_PROBE_EVENT_RECORD_KIND.to_string(),
            client_key_hash: client_key_hash.clone(),
            provider_key: provider_key.to_string(),
            observed_at_unix_millis,
            event_kind: "trade".to_string(),
            supported_by_nt_source: true,
            observed_through_live_node: true,
            age_millis: Some(age_millis),
            latency_millis: Some(latency_millis),
            event_clock_skew_millis,
            recovered: None,
            fail_closed: None,
            evidence_sha256: Some(data_client_trade_probe_evidence_hash(trade)?),
            unsupported_disposition: None,
        });
    }
    Ok(events)
}

fn sort_and_validate_data_client_behavior_probe_events(
    events: &mut [DataClientBehaviorProbeEvent],
    client_key: &str,
    provider_key: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    events.sort_by(|left, right| {
        (
            left.observed_at_unix_millis,
            left.event_kind.as_str(),
            left.evidence_sha256.as_deref(),
        )
            .cmp(&(
                right.observed_at_unix_millis,
                right.event_kind.as_str(),
                right.evidence_sha256.as_deref(),
            ))
    });
    for event in events {
        validate_data_client_behavior_probe_event(event, client_key, provider_key)?;
    }
    Ok(())
}

fn data_client_readiness_quote_target_instruments_for_evidence(
    client_key: &str,
    provider_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    metadata: &crate::bolt_v3_live_node::BoltV3NoSubmitDataClientMetadataEvidence,
) -> Result<BTreeSet<String>, BoltV3OperatorArtifactError> {
    let Some(readiness_probe) = &client.readiness_probe else {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "clients.<id>.readiness_probe.quote_targets",
            },
        );
    };
    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            data_client_readiness_quote_target_instruments(client)
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            let max_quote_targets = readiness_probe.max_metadata_quote_targets.ok_or(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "clients.<id>.readiness_probe.max_metadata_quote_targets",
                },
            )?;
            if max_quote_targets == 0 {
                return Err(
                    BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                        field: "clients.<id>.readiness_probe.max_metadata_quote_targets",
                    },
                );
            }
            let mut metadata_instruments = BTreeSet::new();
            for response in &metadata.responses {
                if response.data_client_id == client_key && response.venue == provider_key {
                    metadata_instruments.extend(response.instrument_ids.iter().cloned());
                }
            }
            if metadata_instruments.is_empty() {
                return Err(
                    BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                        field: "metadata.instrument_ids",
                    },
                );
            }
            if metadata_instruments.len() > max_quote_targets {
                let allow_target_sampling = readiness_probe.allow_metadata_target_sampling.ok_or(
                    BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                        field: "clients.<id>.readiness_probe.allow_metadata_target_sampling",
                    },
                )?;
                if allow_target_sampling {
                    let metadata_instrument_ids =
                        metadata_instruments.into_iter().collect::<Vec<_>>();
                    metadata_instruments = sample_metadata_response_targets(
                        &metadata_instrument_ids,
                        max_quote_targets,
                    )
                    .into_iter()
                    .collect();
                } else {
                    return Err(
                        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                            field: "metadata.instrument_ids.max_metadata_quote_targets",
                        },
                    );
                }
            }
            Ok(metadata_instruments)
        }
    }
}

fn data_client_readiness_quote_target_instruments(
    client: &crate::bolt_v3_config::ClientBlock,
) -> Result<BTreeSet<String>, BoltV3OperatorArtifactError> {
    let Some(readiness_probe) = &client.readiness_probe else {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "clients.<id>.readiness_probe.quote_targets",
            },
        );
    };
    let Some(quote_targets) = &readiness_probe.quote_targets else {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "clients.<id>.readiness_probe.quote_targets",
            },
        );
    };
    if quote_targets.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "clients.<id>.readiness_probe.quote_targets",
            },
        );
    }
    Ok(quote_targets
        .values()
        .map(|target| target.instrument_id.to_string())
        .collect())
}

fn build_data_client_readiness_target_candidates_from_no_submit_readiness_evidence(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    evidence: &BoltV3NoSubmitDataClientReadinessEvidence,
) -> Result<DataClientReadinessTargetCandidatesArtifact, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key",
            },
        );
    }
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "client_key",
        },
    )?;
    if client.data.is_none() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key.data",
            },
        );
    }
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;

    let mut observed_at_unix_millis = 0_u64;
    let mut metadata_response_count = 0_usize;
    let mut instrument_ids = BTreeSet::new();
    for response in &evidence.metadata.responses {
        if response.data_client_id != client_key {
            continue;
        }
        if response.venue != provider_key {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.responses.venue",
                },
            );
        }
        metadata_response_count += 1;
        let captured_at_unix_millis = nanos_to_millis_checked(
            response.captured_at_unix_nanos,
            "metadata.captured_at_unix_nanos",
        )?;
        observed_at_unix_millis = observed_at_unix_millis.max(captured_at_unix_millis);
        for instrument_id in &response.instrument_ids {
            let parsed = InstrumentId::from_str(instrument_id).map_err(|_| {
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "metadata.instrument_ids",
                }
            })?;
            if parsed.venue.as_str() != provider_key {
                return Err(
                    BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                        field: "metadata.instrument_ids.venue",
                    },
                );
            }
            instrument_ids.insert(instrument_id.to_string());
        }
    }
    if metadata_response_count == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "metadata.responses",
            },
        );
    }
    if instrument_ids.is_empty() || observed_at_unix_millis == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "metadata.instrument_ids",
            },
        );
    }
    let instrument_ids: Vec<String> = instrument_ids.into_iter().collect();
    let instrument_ids_sha256 = data_client_readiness_target_candidates_hash(&instrument_ids);
    Ok(DataClientReadinessTargetCandidatesArtifact {
        schema_version: DATA_CLIENT_READINESS_SOURCE_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_READINESS_TARGET_CANDIDATES_RECORD_KIND,
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        client_key_hash: sha256_text(client_key),
        provider_key: provider_key.to_string(),
        observed_at_unix_millis,
        metadata_response_count,
        instrument_count: instrument_ids.len(),
        instrument_ids,
        instrument_ids_sha256,
        production_usable: false,
        readiness_status: DATA_CLIENT_READINESS_TARGET_CANDIDATES_STATUS_TARGETS_UNBOUND,
    })
}

fn data_client_readiness_target_candidates_hash(instrument_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    for instrument_id in instrument_ids {
        hasher.update((instrument_id.len() as u64).to_le_bytes());
        hasher.update(instrument_id.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn generated_at_unix_seconds() -> Result<u64, BoltV3OperatorArtifactError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| BoltV3OperatorArtifactError::SystemTimeBeforeUnixEpoch { source })
}

fn build_data_client_policy_behavior_source_artifact_from_nt_sources(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    nt_policy_source_paths: &[PathBuf],
    max_source_bytes: u64,
) -> Result<DataClientPolicyBehaviorSourceArtifact, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "client_key",
        });
    }
    if nt_policy_source_paths.is_empty() {
        return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "nt_policy_source_paths",
        });
    }
    if max_source_bytes == 0 {
        return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "max_source_bytes",
        });
    }
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "client_key",
        },
    )?;
    if client.data.is_none() {
        return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "client_key.data",
        });
    }
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;

    let mut source_path_hashes = Vec::new();
    let mut source_sha256s = Vec::new();
    let mut source_byte_len = 0_usize;
    let mut source_texts = Vec::new();
    for path in nt_policy_source_paths {
        let bytes = read_file_bounded(path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientNtSourceRead {
                path: path.to_path_buf(),
                source,
            }
        })?;
        if bytes.is_empty() {
            return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
                field: "nt_policy_source",
            });
        }
        source_byte_len = source_byte_len.saturating_add(bytes.len());
        source_path_hashes.push(source_path_hash(path));
        source_sha256s.push(hex::encode(Sha256::digest(&bytes)));
        source_texts.push(String::from_utf8_lossy(&bytes).to_string());
    }
    source_path_hashes.sort();
    source_sha256s.sort();
    let source_text = source_texts.join("\n");
    let reconnect = data_client_policy_source_observation(
        "reconnect",
        data_client_policy_source_reconnect_markers(&source_text),
        &source_sha256s,
    )?;
    let rate_limit = data_client_policy_source_observation(
        "rate_limit",
        data_client_policy_source_rate_limit_markers(&source_text),
        &source_sha256s,
    )?;
    let parse_error = data_client_policy_source_observation(
        "parse_error",
        data_client_policy_source_parse_error_markers(&source_text),
        &source_sha256s,
    )?;
    let source_owned_policy_observation_complete =
        data_client_policy_observation_proven(&reconnect)
            && data_client_policy_observation_proven(&rate_limit)
            && data_client_policy_observation_proven(&parse_error);
    let readiness_status = if source_owned_policy_observation_complete {
        DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_STATUS_COMPLETE
    } else {
        DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_STATUS_MISSING_MARKERS
    };
    Ok(DataClientPolicyBehaviorSourceArtifact {
        schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_RECORD_KIND.to_string(),
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        client_key_hash: sha256_text(client_key),
        provider_key: provider_key.to_string(),
        nt_policy_source_path_hashes: source_path_hashes,
        nt_policy_source_sha256s: source_sha256s,
        nt_policy_source_byte_len: source_byte_len,
        reconnect,
        rate_limit,
        parse_error,
        source_owned_policy_observation_complete,
        production_usable: false,
        readiness_status: readiness_status.to_string(),
    })
}

fn read_data_client_policy_behavior_source_artifact(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    policy_source_path: &Path,
    max_policy_source_bytes: u64,
) -> Result<DataClientLoadedPolicyBehaviorSource, BoltV3OperatorArtifactError> {
    if max_policy_source_bytes == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "max_policy_source_bytes",
            },
        );
    }
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "client_key",
        },
    )?;
    let provider_key = client.venue.as_str();
    let bytes =
        read_file_bounded(policy_source_path, max_policy_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceRead {
                path: policy_source_path.to_path_buf(),
                source,
            }
        })?;
    let source_sha256 = hex::encode(Sha256::digest(&bytes));
    let artifact: DataClientPolicyBehaviorSourceArtifact =
        serde_json::from_slice(&bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceParse {
                path: policy_source_path.to_path_buf(),
                source,
            }
        })?;
    validate_data_client_policy_behavior_source_artifact(
        &artifact,
        loaded,
        client_key,
        provider_key,
    )?;
    Ok(DataClientLoadedPolicyBehaviorSource {
        source_sha256,
        artifact,
    })
}

fn validate_data_client_policy_behavior_source_artifact(
    source: &DataClientPolicyBehaviorSourceArtifact,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    provider_key: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.schema_version",
            },
        );
    }
    if source.record_kind != DATA_CLIENT_POLICY_BEHAVIOR_SOURCE_RECORD_KIND {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.record_kind",
            },
        );
    }
    if source.config_bundle_checksum.as_str() != loaded.config_bundle_checksum.as_str() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.config_bundle_checksum",
            },
        );
    }
    if source.client_key_hash != sha256_text(client_key) {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.client_key_hash",
            },
        );
    }
    if source.provider_key != provider_key {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.provider_key",
            },
        );
    }
    if source.nt_policy_source_path_hashes.is_empty()
        || source.nt_policy_source_sha256s.is_empty()
        || source
            .nt_policy_source_path_hashes
            .iter()
            .any(|hash| !is_lowercase_sha256(hash))
        || source
            .nt_policy_source_sha256s
            .iter()
            .any(|hash| !is_lowercase_sha256(hash))
        || source.nt_policy_source_byte_len == 0
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.nt_policy_source",
            },
        );
    }
    validate_data_client_policy_observation("policy_source.reconnect", &source.reconnect)?;
    validate_data_client_policy_observation("policy_source.rate_limit", &source.rate_limit)?;
    validate_data_client_policy_observation("policy_source.parse_error", &source.parse_error)?;
    let complete = data_client_policy_observation_proven(&source.reconnect)
        && data_client_policy_observation_proven(&source.rate_limit)
        && data_client_policy_observation_proven(&source.parse_error);
    if source.source_owned_policy_observation_complete != complete {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source.source_owned_policy_observation_complete",
            },
        );
    }
    Ok(())
}

fn data_client_policy_source_observation(
    event_kind: &'static str,
    markers: DataClientPolicySourceMarkers,
    source_sha256s: &[String],
) -> Result<DataClientPolicyObservation, BoltV3OperatorArtifactError> {
    if !markers.behavior_observed {
        return Ok(missing_data_client_policy_observation(event_kind));
    }
    let evidence_inputs: Vec<&str> = source_sha256s.iter().map(String::as_str).collect();
    let evidence_sha256 = data_client_policy_source_evidence_hash(
        event_kind,
        markers.recovered,
        markers.fail_closed,
        &evidence_inputs,
    );
    let observation = DataClientPolicyObservation {
        behavior_observed: true,
        recovered: markers.recovered,
        fail_closed: markers.fail_closed,
        evidence_sha256,
    };
    validate_data_client_policy_observation(event_kind, &observation)?;
    Ok(observation)
}

#[derive(Debug, Clone, Copy)]
struct DataClientPolicySourceMarkers {
    behavior_observed: bool,
    recovered: bool,
    fail_closed: bool,
}

fn data_client_policy_source_reconnect_markers(source: &str) -> DataClientPolicySourceMarkers {
    let normalized = source.to_ascii_lowercase();
    DataClientPolicySourceMarkers {
        behavior_observed: normalized.contains("reconnect"),
        recovered: normalized.contains("resubscribe")
            || normalized.contains("restore_subscriptions")
            || normalized.contains("subscriptions"),
        fail_closed: normalized.contains("disconnect")
            || normalized.contains("map_err")
            || normalized.contains("error"),
    }
}

fn data_client_policy_source_rate_limit_markers(source: &str) -> DataClientPolicySourceMarkers {
    let normalized = source.to_ascii_lowercase();
    DataClientPolicySourceMarkers {
        behavior_observed: normalized.contains("rate_limit")
            || normalized.contains("rate limit")
            || normalized.contains("throttle")
            || normalized.contains("429"),
        recovered: normalized.contains("retry") || normalized.contains("backoff"),
        fail_closed: normalized.contains("is_retryable")
            || normalized.contains("map_err")
            || normalized.contains("error")
            || normalized.contains("429"),
    }
}

fn data_client_policy_source_parse_error_markers(source: &str) -> DataClientPolicySourceMarkers {
    let normalized = source.to_ascii_lowercase();
    let parses_with_error = normalized.contains("parse")
        && (normalized.contains("map_err")
            || normalized.contains("result")
            || normalized.contains("fail_closed"));
    DataClientPolicySourceMarkers {
        behavior_observed: parses_with_error,
        recovered: false,
        fail_closed: parses_with_error,
    }
}

fn data_client_policy_source_evidence_hash(
    event_kind: &'static str,
    recovered: bool,
    fail_closed: bool,
    source_sha256s: &[&str],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event_kind.as_bytes());
    hasher.update([u8::from(recovered)]);
    hasher.update([u8::from(fail_closed)]);
    for source_sha256 in source_sha256s {
        hasher.update(source_sha256.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn data_client_policy_observation_proven(observation: &DataClientPolicyObservation) -> bool {
    observation.behavior_observed && (observation.recovered || observation.fail_closed)
}

fn nanos_to_millis_checked(
    nanos: u64,
    field: &'static str,
) -> Result<u64, BoltV3OperatorArtifactError> {
    let millis = nanos / 1_000_000;
    if millis == 0 {
        Err(BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field })
    } else {
        Ok(millis)
    }
}

fn nanos_delta_to_millis_checked(
    end_nanos: u64,
    start_nanos: u64,
    field: &'static str,
) -> Result<u64, BoltV3OperatorArtifactError> {
    end_nanos
        .checked_sub(start_nanos)
        .map(|nanos| nanos / 1_000_000)
        .ok_or(BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field })
}

fn nanos_age_and_clock_skew_millis(end_nanos: u64, start_nanos: u64) -> (u64, Option<u64>) {
    if end_nanos >= start_nanos {
        ((end_nanos - start_nanos) / 1_000_000, None)
    } else {
        (0, Some((start_nanos - end_nanos) / 1_000_000))
    }
}

fn data_client_quote_probe_evidence_hash(
    quote: &crate::bolt_v3_live_node::BoltV3NoSubmitReferenceQuote,
) -> Result<String, BoltV3OperatorArtifactError> {
    canonical_json_sha256_value(&serde_json::json!({
        "source": "no_submit_reference_quote_probe",
        "data_client_id_hash": sha256_text(&quote.data_client_id),
        "instrument_id_hash": sha256_text(&quote.instrument_id),
        "bid_price": quote.bid_price,
        "ask_price": quote.ask_price,
        "ts_event_unix_nanos": quote.ts_event_unix_nanos,
        "ts_init_unix_nanos": quote.ts_init_unix_nanos,
        "captured_at_unix_nanos": quote.captured_at_unix_nanos,
    }))
}

fn data_client_book_probe_evidence_hash(
    deltas: &crate::bolt_v3_live_node::BoltV3NoSubmitBookDeltas,
) -> Result<String, BoltV3OperatorArtifactError> {
    canonical_json_sha256_value(&serde_json::json!({
        "source": "no_submit_book_deltas_probe",
        "data_client_id_hash": sha256_text(&deltas.data_client_id),
        "instrument_id_hash": sha256_text(&deltas.instrument_id),
        "delta_count": deltas.delta_count,
        "ts_event_unix_nanos": deltas.ts_event_unix_nanos,
        "ts_init_unix_nanos": deltas.ts_init_unix_nanos,
        "captured_at_unix_nanos": deltas.captured_at_unix_nanos,
    }))
}

fn data_client_trade_probe_evidence_hash(
    trade: &crate::bolt_v3_live_node::BoltV3NoSubmitTrade,
) -> Result<String, BoltV3OperatorArtifactError> {
    canonical_json_sha256_value(&serde_json::json!({
        "source": "no_submit_trade_probe",
        "data_client_id_hash": sha256_text(&trade.data_client_id),
        "instrument_id_hash": sha256_text(&trade.instrument_id),
        "price": trade.price,
        "size": trade.size,
        "ts_event_unix_nanos": trade.ts_event_unix_nanos,
        "ts_init_unix_nanos": trade.ts_init_unix_nanos,
        "captured_at_unix_nanos": trade.captured_at_unix_nanos,
    }))
}

fn data_client_metadata_probe_evidence_hash(
    response: &crate::bolt_v3_live_node::BoltV3NoSubmitDataClientMetadata,
) -> Result<String, BoltV3OperatorArtifactError> {
    let instrument_id_hashes: Vec<String> = response
        .instrument_ids
        .iter()
        .map(|instrument_id| sha256_text(instrument_id))
        .collect();
    canonical_json_sha256_value(&serde_json::json!({
        "source": "no_submit_data_client_metadata_probe",
        "data_client_id_hash": sha256_text(&response.data_client_id),
        "venue": response.venue,
        "instrument_count": response.instrument_ids.len(),
        "instrument_id_hashes": instrument_id_hashes,
        "ts_init_unix_nanos": response.ts_init_unix_nanos,
        "captured_at_unix_nanos": response.captured_at_unix_nanos,
    }))
}

fn build_data_client_behavior_observation_source_from_probe_events(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    probe_events_path: &Path,
    max_probe_events_bytes: u64,
    policy_source: Option<&DataClientLoadedPolicyBehaviorSource>,
) -> Result<DataClientBehaviorObservationSourceFile, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key",
            },
        );
    }
    if max_probe_events_bytes == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "max_probe_events_bytes",
            },
        );
    }
    let configured_max_age_millis = data_client_behavior_configured_max_age_millis(loaded)?;
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "client_key",
        },
    )?;
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;

    let bytes = read_file_bounded(probe_events_path, max_probe_events_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceRead {
            path: probe_events_path.to_path_buf(),
            source,
        }
    })?;
    let probe_events = parse_data_client_behavior_probe_events(
        &bytes,
        probe_events_path,
        client_key,
        provider_key,
    )?;
    materialize_data_client_behavior_observation_source_from_probe_events(
        &probe_events,
        client_key,
        provider_key,
        configured_max_age_millis,
        policy_source,
    )
}

fn data_client_behavior_configured_max_age_millis(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3OperatorArtifactError> {
    let live_canary = loaded.root.live_canary.as_ref().ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "live_canary",
        },
    )?;
    if live_canary.reference_quote_max_age_seconds == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "live_canary.reference_quote_max_age_seconds",
            },
        );
    }
    live_canary
        .reference_quote_max_age_seconds
        .checked_mul(1_000)
        .ok_or(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "live_canary.reference_quote_max_age_seconds",
            },
        )
}

fn parse_data_client_behavior_probe_events(
    bytes: &[u8],
    probe_events_path: &Path,
    client_key: &str,
    provider_key: &str,
) -> Result<Vec<DataClientBehaviorProbeEvent>, BoltV3OperatorArtifactError> {
    let source = String::from_utf8_lossy(bytes);
    let mut events = Vec::new();
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let event: DataClientBehaviorProbeEvent = serde_json::from_str(line).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceParse {
                path: probe_events_path.to_path_buf(),
                source,
            }
        })?;
        validate_data_client_behavior_probe_event(&event, client_key, provider_key)?;
        events.push(event);
    }
    if events.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_events",
            },
        );
    }
    Ok(events)
}

fn validate_data_client_behavior_probe_event(
    event: &DataClientBehaviorProbeEvent,
    client_key: &str,
    provider_key: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if event.schema_version != DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.schema_version",
            },
        );
    }
    if event.record_kind != DATA_CLIENT_BEHAVIOR_PROBE_EVENT_RECORD_KIND {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.record_kind",
            },
        );
    }
    if event.client_key_hash != sha256_text(client_key) {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.client_key_hash",
            },
        );
    }
    if event.provider_key != provider_key {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.provider_key",
            },
        );
    }
    if event.observed_at_unix_millis == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.observed_at_unix_millis",
            },
        );
    }
    if data_client_probe_event_surface_kind(event.event_kind.as_str()) {
        validate_data_client_behavior_surface_probe_event(event)
    } else {
        Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.event_kind",
            },
        )
    }
}

fn validate_data_client_behavior_surface_probe_event(
    event: &DataClientBehaviorProbeEvent,
) -> Result<(), BoltV3OperatorArtifactError> {
    if event.supported_by_nt_source {
        if !event.observed_through_live_node
            || event.age_millis.is_none()
            || event.latency_millis.is_none()
            || event.recovered.is_some()
            || event.fail_closed.is_some()
            || !event
                .evidence_sha256
                .as_deref()
                .is_some_and(is_lowercase_sha256)
            || event.unsupported_disposition.is_some()
        {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "probe_event.surface",
                },
            );
        }
    } else if event.observed_through_live_node
        || event.age_millis.is_some()
        || event.latency_millis.is_some()
        || event.event_clock_skew_millis.is_some()
        || event.recovered.is_some()
        || event.fail_closed.is_some()
        || event.evidence_sha256.is_some()
        || match event.unsupported_disposition.as_deref() {
            Some(disposition) => disposition.trim().is_empty(),
            None => true,
        }
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.surface",
            },
        );
    }
    Ok(())
}

fn materialize_data_client_behavior_observation_source_from_probe_events(
    events: &[DataClientBehaviorProbeEvent],
    client_key: &str,
    provider_key: &str,
    configured_max_age_millis: u64,
    policy_source: Option<&DataClientLoadedPolicyBehaviorSource>,
) -> Result<DataClientBehaviorObservationSourceFile, BoltV3OperatorArtifactError> {
    let observed_at_unix_millis = events
        .iter()
        .map(|event| event.observed_at_unix_millis)
        .max()
        .ok_or(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_events",
            },
        )?;
    let first_observed_at_unix_millis = events
        .iter()
        .map(|event| event.observed_at_unix_millis)
        .min()
        .unwrap_or(observed_at_unix_millis);
    let observation_window_millis = observed_at_unix_millis
        .saturating_sub(first_observed_at_unix_millis)
        .max(1);
    let freshness =
        data_client_freshness_observation_from_probe_events(events, configured_max_age_millis)?;
    let policy_source_sha256 = policy_source.map(|source| source.source_sha256.clone());
    let source = DataClientBehaviorObservationSourceFile {
        schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_BEHAVIOR_OBSERVATION_SOURCE_RECORD_KIND.to_string(),
        client_key_hash: sha256_text(client_key),
        provider_key: provider_key.to_string(),
        policy_source_sha256,
        observed_at_unix_millis,
        observation_window_millis,
        metadata_behavior: data_client_surface_observation_from_probe_events(events, "metadata")?,
        quote_behavior: data_client_surface_observation_from_probe_events(events, "quote")?,
        book_behavior: data_client_surface_observation_from_probe_events(events, "book")?,
        ticker_behavior: data_client_surface_observation_from_probe_events(events, "ticker")?,
        trade_behavior: data_client_surface_observation_from_probe_events(events, "trade")?,
        freshness,
        reconnect: policy_source
            .map(|source| source.artifact.reconnect.clone())
            .unwrap_or_else(|| missing_data_client_policy_observation("reconnect")),
        rate_limit: policy_source
            .map(|source| source.artifact.rate_limit.clone())
            .unwrap_or_else(|| missing_data_client_policy_observation("rate_limit")),
        parse_error: policy_source
            .map(|source| source.artifact.parse_error.clone())
            .unwrap_or_else(|| missing_data_client_policy_observation("parse_error")),
    };
    validate_data_client_behavior_observation_source(&source, client_key, provider_key)?;
    Ok(source)
}

fn data_client_surface_observation_from_probe_events(
    events: &[DataClientBehaviorProbeEvent],
    event_kind: &'static str,
) -> Result<DataClientBehaviorSurfaceObservation, BoltV3OperatorArtifactError> {
    let surface_events: Vec<&DataClientBehaviorProbeEvent> = events
        .iter()
        .filter(|event| event.event_kind == event_kind)
        .collect();
    if surface_events.is_empty() {
        return Ok(DataClientBehaviorSurfaceObservation {
            supported_by_nt_source: false,
            observed_through_live_node: false,
            sample_count: 0,
            first_observed_at_unix_millis: None,
            last_observed_at_unix_millis: None,
            evidence_sha256: None,
            unsupported_disposition: Some(format!("{event_kind}_probe_event_missing")),
        });
    }
    if surface_events
        .iter()
        .any(|event| !event.supported_by_nt_source)
    {
        let disposition = surface_events
            .iter()
            .find_map(|event| event.unsupported_disposition.as_deref())
            .filter(|disposition| !disposition.trim().is_empty())
            .ok_or(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "probe_event.unsupported_disposition",
                },
            )?;
        return Ok(DataClientBehaviorSurfaceObservation {
            supported_by_nt_source: false,
            observed_through_live_node: false,
            sample_count: 0,
            first_observed_at_unix_millis: None,
            last_observed_at_unix_millis: None,
            evidence_sha256: None,
            unsupported_disposition: Some(disposition.to_string()),
        });
    }
    let evidence_hashes: Vec<&str> = surface_events
        .iter()
        .map(|event| {
            event.evidence_sha256.as_deref().ok_or(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                    field: "probe_event.evidence_sha256",
                },
            )
        })
        .collect::<Result<_, _>>()?;
    Ok(DataClientBehaviorSurfaceObservation {
        supported_by_nt_source: true,
        observed_through_live_node: true,
        sample_count: surface_events.len() as u64,
        first_observed_at_unix_millis: surface_events
            .iter()
            .map(|event| event.observed_at_unix_millis)
            .min(),
        last_observed_at_unix_millis: surface_events
            .iter()
            .map(|event| event.observed_at_unix_millis)
            .max(),
        evidence_sha256: Some(data_client_aggregate_evidence_hash(&evidence_hashes)),
        unsupported_disposition: None,
    })
}

fn data_client_freshness_observation_from_probe_events(
    events: &[DataClientBehaviorProbeEvent],
    configured_max_age_millis: u64,
) -> Result<DataClientFreshnessObservation, BoltV3OperatorArtifactError> {
    let mut age_millis = Vec::new();
    let mut latency_millis = Vec::new();
    let mut evidence_hashes = Vec::new();
    for event in events
        .iter()
        .filter(|event| data_client_probe_event_surface_kind(event.event_kind.as_str()))
        .filter(|event| event.supported_by_nt_source)
    {
        age_millis.push(event.age_millis.ok_or(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.age_millis",
            },
        )?);
        latency_millis.push(event.latency_millis.ok_or(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.latency_millis",
            },
        )?);
        evidence_hashes.push(event.evidence_sha256.as_deref().ok_or(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "probe_event.evidence_sha256",
            },
        )?);
    }
    if age_millis.is_empty() || latency_millis.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "freshness",
            },
        );
    }
    latency_millis.sort_unstable();
    let latency_p95_index = (latency_millis.len() * 95).div_ceil(100).saturating_sub(1);
    let max_observed_age_millis = age_millis.into_iter().max().ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "freshness.age_millis",
        },
    )?;
    let within_configured_bound = max_observed_age_millis <= configured_max_age_millis;
    Ok(DataClientFreshnessObservation {
        configured_max_age_millis,
        max_observed_age_millis,
        latency_sample_count: latency_millis.len() as u64,
        latency_p95_millis: latency_millis[latency_p95_index],
        latency_max_millis: latency_millis.last().copied().ok_or(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "freshness.latency_millis",
            },
        )?,
        within_configured_bound,
        evidence_sha256: data_client_aggregate_evidence_hash(&evidence_hashes),
    })
}

fn missing_data_client_policy_observation(event_kind: &'static str) -> DataClientPolicyObservation {
    DataClientPolicyObservation {
        behavior_observed: false,
        recovered: false,
        fail_closed: false,
        evidence_sha256: sha256_text(&format!("{event_kind}_policy_source_missing")),
    }
}

fn data_client_aggregate_evidence_hash(evidence_hashes: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for evidence_hash in evidence_hashes {
        hasher.update(evidence_hash.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn data_client_probe_event_surface_kind(event_kind: &str) -> bool {
    matches!(
        event_kind,
        "metadata" | "quote" | "book" | "ticker" | "trade"
    )
}

fn build_data_client_production_readiness_matrix_artifact_from_source_files(
    loaded: &LoadedBoltV3Config,
    readiness_source_path: &Path,
    live_node_mapping_source_path: &Path,
    nt_source_capability_paths: &[PathBuf],
    target_candidate_paths: &[PathBuf],
    behavior_observation_paths: &[PathBuf],
    max_source_bytes: u64,
) -> Result<DataClientProductionReadinessMatrixArtifact, BoltV3OperatorArtifactError> {
    if max_source_bytes == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "max_source_bytes",
            },
        );
    }
    let (readiness_source, readiness_source_sha256) =
        read_data_client_matrix_source_json(readiness_source_path, max_source_bytes)?;
    let (live_node_mapping_source, live_node_mapping_source_sha256) =
        read_data_client_matrix_source_json(live_node_mapping_source_path, max_source_bytes)?;
    validate_data_client_matrix_source_header(
        &readiness_source,
        DATA_CLIENT_READINESS_SOURCE_RECORD_KIND,
        "readiness_source",
        loaded,
    )?;
    validate_data_client_matrix_source_header(
        &live_node_mapping_source,
        DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_RECORD_KIND,
        "live_node_mapping_source",
        loaded,
    )?;

    let config_inventory = data_client_matrix_client_keys(
        &readiness_source,
        "readiness_source.clients",
        "strategy_routed",
    )?;
    let live_node_mapping = data_client_matrix_client_keys(
        &live_node_mapping_source,
        "live_node_mapping_source.clients",
        "data_client_registered_through_live_node",
    )?;

    let mut nt_source_capabilities = BTreeSet::new();
    let mut nt_source_capability_sha256s = Vec::new();
    for path in nt_source_capability_paths {
        let (source, sha256) = read_data_client_matrix_source_json(path, max_source_bytes)?;
        validate_data_client_matrix_source_header(
            &source,
            DATA_CLIENT_NT_SOURCE_CAPABILITY_RECORD_KIND,
            "nt_source_capability",
            loaded,
        )?;
        nt_source_capabilities.insert(data_client_matrix_top_level_client_key(
            &source,
            "nt_source_capability",
        )?);
        nt_source_capability_sha256s.push(sha256);
    }
    nt_source_capability_sha256s.sort();

    let mut target_candidates = BTreeMap::new();
    let mut target_candidate_sha256s = Vec::new();
    for path in target_candidate_paths {
        let (source, sha256) = read_data_client_matrix_source_json(path, max_source_bytes)?;
        validate_data_client_matrix_source_header(
            &source,
            DATA_CLIENT_READINESS_TARGET_CANDIDATES_RECORD_KIND,
            "target_candidate",
            loaded,
        )?;
        let (key, instrument_ids) = data_client_matrix_target_candidate_instrument_ids(&source)?;
        if target_candidates.insert(key, instrument_ids).is_some() {
            return Err(
                BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                    field: "target_candidate.duplicate_client",
                },
            );
        }
        target_candidate_sha256s.push(sha256);
    }
    target_candidate_sha256s.sort();

    let mut behavior_observations = BTreeSet::new();
    let mut behavior_observation_sha256s = Vec::new();
    for path in behavior_observation_paths {
        let (source, sha256) = read_data_client_matrix_source_json(path, max_source_bytes)?;
        validate_data_client_matrix_source_header(
            &source,
            DATA_CLIENT_BEHAVIOR_OBSERVATION_RECORD_KIND,
            "behavior_observation",
            loaded,
        )?;
        if data_client_matrix_json_bool(
            &source,
            "behavior_observation_complete",
            "behavior_observation.behavior_observation_complete",
        )? {
            behavior_observations.insert(data_client_matrix_top_level_client_key(
                &source,
                "behavior_observation",
            )?);
        }
        behavior_observation_sha256s.push(sha256);
    }
    behavior_observation_sha256s.sort();

    let mut clients = Vec::new();
    for (client_key, client) in &loaded.root.clients {
        let provider_key = client.venue.as_str();
        binding_for_provider_key(provider_key).ok_or_else(|| {
            BoltV3OperatorArtifactError::UnsupportedProvider {
                client_key: client_key.clone(),
                provider_key: provider_key.to_string(),
            }
        })?;
        let key = (sha256_text(client_key), provider_key.to_string());
        let has_data = client.data.is_some();
        let has_execution = client.execution.is_some();
        let config_inventory_present = config_inventory.contains(&key);
        let live_node_mapping_present = live_node_mapping.contains(&key);
        let nt_source_capability_present = nt_source_capabilities.contains(&key);
        let behavior_observation_complete = behavior_observations.contains(&key);
        let source_owned_target_binding_present =
            data_client_matrix_source_owned_target_binding_present(
                client_key,
                client,
                &key,
                behavior_observation_complete,
                &target_candidates,
            );
        let readiness_required = has_data;
        let market_coverage_config_values =
            toml_table_selected_values(client.data.as_ref(), data_client_market_coverage_field)?;
        let mut missing_proofs = Vec::new();
        if readiness_required && !config_inventory_present {
            missing_proofs.push("config_inventory");
        }
        if readiness_required && !live_node_mapping_present {
            missing_proofs.push("live_node_mapping");
        }
        if readiness_required && !nt_source_capability_present {
            missing_proofs.push("nt_source_capability");
        }
        if readiness_required && !behavior_observation_complete {
            missing_proofs.push("behavior_observation");
        }
        if readiness_required
            && behavior_observation_complete
            && !source_owned_target_binding_present
        {
            missing_proofs.push("source_owned_target_binding");
        }
        let production_usable = readiness_required && missing_proofs.is_empty();
        let readiness_status = if !readiness_required {
            "not_a_configured_data_client"
        } else if production_usable {
            "data_client_t043a_matrix_complete"
        } else {
            "data_client_t043a_matrix_missing_proofs"
        };
        clients.push(DataClientProductionReadinessMatrixClient {
            client_key_hash: key.0,
            provider_key: key.1,
            has_data,
            has_execution,
            readiness_required,
            config_inventory_present,
            live_node_mapping_present,
            nt_source_capability_present,
            source_owned_target_binding_present,
            behavior_observation_complete,
            production_usable,
            readiness_status,
            missing_proofs,
            market_coverage_config_values,
        });
    }
    clients.sort_by(|left, right| {
        (left.provider_key.as_str(), left.client_key_hash.as_str())
            .cmp(&(right.provider_key.as_str(), right.client_key_hash.as_str()))
    });

    Ok(DataClientProductionReadinessMatrixArtifact {
        schema_version: DATA_CLIENT_PRODUCTION_READINESS_MATRIX_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_PRODUCTION_READINESS_MATRIX_RECORD_KIND,
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        readiness_source_sha256,
        live_node_mapping_source_sha256,
        nt_source_capability_sha256s,
        target_candidate_sha256s,
        behavior_observation_sha256s,
        clients,
    })
}

fn data_client_matrix_target_candidate_instrument_ids(
    source: &serde_json::Value,
) -> Result<((String, String), BTreeSet<String>), BoltV3OperatorArtifactError> {
    let key = data_client_matrix_top_level_client_key(source, "target_candidate")?;
    let provider_key = key.1.as_str();
    let instrument_count =
        data_client_matrix_json_u64(source, "instrument_count", "target_candidate")? as usize;
    let raw_instrument_ids = source
        .get("instrument_ids")
        .and_then(serde_json::Value::as_array)
        .ok_or(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "target_candidate.instrument_ids",
            },
        )?;
    if raw_instrument_ids.is_empty() || raw_instrument_ids.len() != instrument_count {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "target_candidate.instrument_count",
            },
        );
    }
    let mut instrument_ids = BTreeSet::new();
    for value in raw_instrument_ids {
        let instrument_id = value.as_str().ok_or(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "target_candidate.instrument_ids",
            },
        )?;
        let parsed = InstrumentId::from_str(instrument_id).map_err(|_| {
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "target_candidate.instrument_ids",
            }
        })?;
        if parsed.venue.as_str() != provider_key {
            return Err(
                BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                    field: "target_candidate.instrument_ids.venue",
                },
            );
        }
        instrument_ids.insert(instrument_id.to_string());
    }
    if instrument_ids.len() != instrument_count {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "target_candidate.instrument_ids",
            },
        );
    }
    let expected_hash = data_client_readiness_target_candidates_hash(
        &instrument_ids.iter().cloned().collect::<Vec<_>>(),
    );
    if data_client_matrix_json_string(
        source,
        "instrument_ids_sha256",
        "target_candidate.instrument_ids_sha256",
    )? != expected_hash.as_str()
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "target_candidate.instrument_ids_sha256",
            },
        );
    }
    Ok((key, instrument_ids))
}

fn data_client_matrix_source_owned_target_binding_present(
    _client_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    key: &(String, String),
    behavior_observation_complete: bool,
    target_candidates: &BTreeMap<(String, String), BTreeSet<String>>,
) -> bool {
    if !behavior_observation_complete {
        return false;
    }
    let Some(readiness_probe) = &client.readiness_probe else {
        return false;
    };
    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => true,
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            let Some(candidates) = target_candidates.get(key) else {
                return false;
            };
            let Some(quote_targets) = &readiness_probe.quote_targets else {
                return false;
            };
            !quote_targets.is_empty()
                && quote_targets
                    .values()
                    .all(|target| candidates.contains(&target.instrument_id.to_string()))
        }
    }
}

fn read_data_client_matrix_source_json(
    path: &Path,
    max_source_bytes: u64,
) -> Result<(serde_json::Value, String), BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_source_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let value = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceParse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok((value, sha256))
}

fn validate_data_client_matrix_source_header(
    source: &serde_json::Value,
    expected_record_kind: &'static str,
    field: &'static str,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3OperatorArtifactError> {
    if data_client_matrix_json_u64(source, "schema_version", field)? != 1 {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid { field },
        );
    }
    if data_client_matrix_json_string(source, "record_kind", field)? != expected_record_kind {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid { field },
        );
    }
    if data_client_matrix_json_string(source, "config_bundle_checksum", field)?
        != loaded.config_bundle_checksum.as_str()
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid {
                field: "config_bundle_checksum",
            },
        );
    }
    Ok(())
}

fn data_client_matrix_client_keys(
    source: &serde_json::Value,
    field: &'static str,
    proof_field: &'static str,
) -> Result<BTreeSet<(String, String)>, BoltV3OperatorArtifactError> {
    let clients = source
        .get("clients")
        .and_then(serde_json::Value::as_array)
        .ok_or(
            BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid { field },
        )?;
    let mut keys = BTreeSet::new();
    for client in clients {
        let client_key_hash =
            data_client_matrix_json_string(client, "client_key_hash", field)?.to_string();
        let provider_key =
            data_client_matrix_json_string(client, "provider_key", field)?.to_string();
        let proof_present = proof_field == "strategy_routed"
            || data_client_matrix_json_bool(client, proof_field, field)?;
        if proof_present {
            keys.insert((client_key_hash, provider_key));
        }
    }
    Ok(keys)
}

fn data_client_matrix_top_level_client_key(
    source: &serde_json::Value,
    field: &'static str,
) -> Result<(String, String), BoltV3OperatorArtifactError> {
    Ok((
        data_client_matrix_json_string(source, "client_key_hash", field)?.to_string(),
        data_client_matrix_json_string(source, "provider_key", field)?.to_string(),
    ))
}

fn data_client_matrix_json_string<'a>(
    value: &'a serde_json::Value,
    key: &'static str,
    field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    value.get(key).and_then(serde_json::Value::as_str).ok_or(
        BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid { field },
    )
}

fn data_client_matrix_json_bool(
    value: &serde_json::Value,
    key: &'static str,
    field: &'static str,
) -> Result<bool, BoltV3OperatorArtifactError> {
    value.get(key).and_then(serde_json::Value::as_bool).ok_or(
        BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid { field },
    )
}

fn data_client_matrix_json_u64(
    value: &serde_json::Value,
    key: &'static str,
    field: &'static str,
) -> Result<u64, BoltV3OperatorArtifactError> {
    value.get(key).and_then(serde_json::Value::as_u64).ok_or(
        BoltV3OperatorArtifactError::DataClientProductionReadinessMatrixSourceInvalid { field },
    )
}

fn build_data_client_behavior_observation_artifact_from_source_file(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    behavior_source_path: &Path,
    max_behavior_source_bytes: u64,
) -> Result<DataClientBehaviorObservationArtifact, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key",
            },
        );
    }
    if max_behavior_source_bytes == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "max_behavior_source_bytes",
            },
        );
    }
    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
            field: "client_key",
        },
    )?;
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let configured_max_age_millis = data_client_behavior_configured_max_age_millis(loaded)?;

    let source_bytes =
        read_file_bounded(behavior_source_path, max_behavior_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceRead {
                path: behavior_source_path.to_path_buf(),
                source,
            }
        })?;
    let source: DataClientBehaviorObservationSourceFile = serde_json::from_slice(&source_bytes)
        .map_err(|source| {
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceParse {
                path: behavior_source_path.to_path_buf(),
                source,
            }
        })?;
    validate_data_client_behavior_observation_source(&source, client_key, provider_key)?;
    if source.freshness.configured_max_age_millis != configured_max_age_millis {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "freshness.configured_max_age_millis",
            },
        );
    }
    let missing_behavior_proofs = data_client_behavior_observation_missing_proofs(&source);
    let behavior_observation_complete = missing_behavior_proofs.is_empty();

    Ok(DataClientBehaviorObservationArtifact {
        schema_version: DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_BEHAVIOR_OBSERVATION_RECORD_KIND,
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        client_key_hash: sha256_text(client_key),
        provider_key: provider_key.to_string(),
        behavior_source_path_hash: source_path_hash(behavior_source_path),
        behavior_source_sha256: hex::encode(Sha256::digest(&source_bytes)),
        policy_source_sha256: source.policy_source_sha256,
        observed_at_unix_millis: source.observed_at_unix_millis,
        observation_window_millis: source.observation_window_millis,
        metadata_behavior: source.metadata_behavior,
        quote_behavior: source.quote_behavior,
        book_behavior: source.book_behavior,
        ticker_behavior: source.ticker_behavior,
        trade_behavior: source.trade_behavior,
        freshness: source.freshness,
        reconnect: source.reconnect,
        rate_limit: source.rate_limit,
        parse_error: source.parse_error,
        behavior_observation_complete,
        missing_behavior_proofs,
        production_usable: false,
        readiness_status: DATA_CLIENT_BEHAVIOR_OBSERVATION_STATUS_NOT_PRODUCTION_USABLE,
    })
}

fn validate_data_client_behavior_observation_source(
    source: &DataClientBehaviorObservationSourceFile,
    client_key: &str,
    provider_key: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != DATA_CLIENT_BEHAVIOR_OBSERVATION_SCHEMA_VERSION {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "schema_version",
            },
        );
    }
    if source.record_kind != DATA_CLIENT_BEHAVIOR_OBSERVATION_SOURCE_RECORD_KIND {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "record_kind",
            },
        );
    }
    if source.client_key_hash != sha256_text(client_key) {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "client_key_hash",
            },
        );
    }
    if source.provider_key != provider_key {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "provider_key",
            },
        );
    }
    if source
        .policy_source_sha256
        .as_deref()
        .is_some_and(|sha256| !is_lowercase_sha256(sha256))
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "policy_source_sha256",
            },
        );
    }
    if source.observed_at_unix_millis == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "observed_at_unix_millis",
            },
        );
    }
    if source.observation_window_millis == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "observation_window_millis",
            },
        );
    }
    validate_data_client_behavior_surface_observation(
        "metadata_behavior",
        &source.metadata_behavior,
    )?;
    validate_data_client_behavior_surface_observation("quote_behavior", &source.quote_behavior)?;
    validate_data_client_behavior_surface_observation("book_behavior", &source.book_behavior)?;
    validate_data_client_behavior_surface_observation("ticker_behavior", &source.ticker_behavior)?;
    validate_data_client_behavior_surface_observation("trade_behavior", &source.trade_behavior)?;
    validate_data_client_freshness_observation(&source.freshness)?;
    validate_data_client_policy_observation("reconnect", &source.reconnect)?;
    validate_data_client_policy_observation("rate_limit", &source.rate_limit)?;
    validate_data_client_policy_observation("parse_error", &source.parse_error)?;
    Ok(())
}

fn validate_data_client_behavior_surface_observation(
    field: &'static str,
    observation: &DataClientBehaviorSurfaceObservation,
) -> Result<(), BoltV3OperatorArtifactError> {
    if observation.supported_by_nt_source {
        if !observation.observed_through_live_node || observation.sample_count == 0 {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        }
        let Some(first) = observation.first_observed_at_unix_millis else {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        };
        let Some(last) = observation.last_observed_at_unix_millis else {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        };
        if first == 0 || last == 0 || first > last {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        }
        let Some(evidence_sha256) = observation.evidence_sha256.as_deref() else {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        };
        if !is_lowercase_sha256(evidence_sha256) {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        }
        if observation.unsupported_disposition.is_some() {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        }
    } else {
        if observation.observed_through_live_node
            || observation.sample_count != 0
            || observation.first_observed_at_unix_millis.is_some()
            || observation.last_observed_at_unix_millis.is_some()
            || observation.evidence_sha256.is_some()
        {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        }
        if match observation.unsupported_disposition.as_deref() {
            Some(disposition) => disposition.trim().is_empty(),
            None => true,
        } {
            return Err(
                BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
            );
        }
    }
    Ok(())
}

fn validate_data_client_freshness_observation(
    observation: &DataClientFreshnessObservation,
) -> Result<(), BoltV3OperatorArtifactError> {
    if observation.configured_max_age_millis == 0
        || observation.latency_sample_count == 0
        || observation.latency_p95_millis > observation.latency_max_millis
        || observation.max_observed_age_millis > observation.configured_max_age_millis
        || !observation.within_configured_bound
        || !is_lowercase_sha256(&observation.evidence_sha256)
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid {
                field: "freshness",
            },
        );
    }
    Ok(())
}

fn validate_data_client_policy_observation(
    field: &'static str,
    observation: &DataClientPolicyObservation,
) -> Result<(), BoltV3OperatorArtifactError> {
    if !is_lowercase_sha256(&observation.evidence_sha256)
        || (observation.behavior_observed && !observation.recovered && !observation.fail_closed)
        || (!observation.behavior_observed && (observation.recovered || observation.fail_closed))
    {
        return Err(
            BoltV3OperatorArtifactError::DataClientBehaviorObservationSourceInvalid { field },
        );
    }
    Ok(())
}

fn data_client_behavior_observation_missing_proofs(
    source: &DataClientBehaviorObservationSourceFile,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !source.metadata_behavior.supported_by_nt_source
        || !source.metadata_behavior.observed_through_live_node
    {
        missing.push("metadata_behavior");
    }
    let market_data_observed = [
        &source.quote_behavior,
        &source.book_behavior,
        &source.ticker_behavior,
        &source.trade_behavior,
    ]
    .iter()
    .any(|observation| {
        observation.supported_by_nt_source && observation.observed_through_live_node
    });
    if !market_data_observed {
        missing.push("quote_or_book_or_ticker_or_trade_behavior");
    }
    if !source.freshness.within_configured_bound {
        missing.push("freshness_latency");
    }
    let policy_source_present = source
        .policy_source_sha256
        .as_deref()
        .is_some_and(is_lowercase_sha256);
    if !policy_source_present || !data_client_policy_observation_proven(&source.reconnect) {
        missing.push("reconnect_behavior");
    }
    if !policy_source_present || !data_client_policy_observation_proven(&source.rate_limit) {
        missing.push("rate_limit_behavior");
    }
    if !policy_source_present || !data_client_policy_observation_proven(&source.parse_error) {
        missing.push("parse_error_behavior");
    }
    missing
}

fn build_data_client_live_node_mapping_source_artifact(
    loaded: &LoadedBoltV3Config,
    registration_summary: &BoltV3RegistrationSummary,
    live_node_source_path: &Path,
    adapter_mapping_source_path: &Path,
    provider_registry_source_path: &Path,
    max_source_bytes: u64,
) -> Result<DataClientLiveNodeMappingSourceArtifact, BoltV3OperatorArtifactError> {
    if max_source_bytes == 0 {
        return Err(
            BoltV3OperatorArtifactError::DataClientLiveNodeMappingSourceInvalid {
                field: "max_source_bytes",
            },
        );
    }

    let live_node_source_bytes = read_file_bounded(live_node_source_path, max_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::DataClientLiveNodeMappingSourceRead {
                path: live_node_source_path.to_path_buf(),
                source,
            },
        )?;
    let adapter_mapping_source_bytes =
        read_file_bounded(adapter_mapping_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientLiveNodeMappingSourceRead {
                path: adapter_mapping_source_path.to_path_buf(),
                source,
            }
        })?;
    let provider_registry_source_bytes =
        read_file_bounded(provider_registry_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientLiveNodeMappingSourceRead {
                path: provider_registry_source_path.to_path_buf(),
                source,
            }
        })?;
    let live_node_source_text = String::from_utf8_lossy(&live_node_source_bytes);
    let adapter_mapping_source_text = String::from_utf8_lossy(&adapter_mapping_source_bytes);
    let provider_registry_source_text = String::from_utf8_lossy(&provider_registry_source_bytes);

    let live_node_calls_adapter_mapping =
        source_contains_any_symbol(&live_node_source_text, &["map_bolt_v3_adapters"]);
    let live_node_registers_mapped_clients =
        source_contains_any_symbol(&live_node_source_text, &["register_bolt_v3_clients"]);
    let adapter_mapping_iterates_loaded_clients =
        source_contains_any_symbol(&adapter_mapping_source_text, &["loaded.root.clients"]);
    let adapter_mapping_dispatches_provider_binding =
        source_contains_any_symbol(&adapter_mapping_source_text, &["map_adapters"]);
    let adapter_mapping_uses_provider_lookup =
        source_contains_any_symbol(&adapter_mapping_source_text, &["binding_for_provider_key"]);
    let provider_registry_exposes_binding_lookup = source_contains_any_symbol(
        &provider_registry_source_text,
        &["binding_for_provider_key"],
    );

    let mut unsupported_dispositions = Vec::new();
    if !live_node_calls_adapter_mapping {
        unsupported_dispositions.push("live_node_adapter_mapping_marker_missing");
    }
    if !live_node_registers_mapped_clients {
        unsupported_dispositions.push("live_node_client_registration_marker_missing");
    }
    if !adapter_mapping_iterates_loaded_clients {
        unsupported_dispositions.push("adapter_mapping_loaded_clients_marker_missing");
    }
    if !adapter_mapping_dispatches_provider_binding {
        unsupported_dispositions.push("adapter_mapping_provider_dispatch_marker_missing");
    }
    if !adapter_mapping_uses_provider_lookup {
        unsupported_dispositions.push("adapter_mapping_provider_lookup_marker_missing");
    }
    if !provider_registry_exposes_binding_lookup {
        unsupported_dispositions.push("provider_registry_lookup_marker_missing");
    }

    let source_path_proven = unsupported_dispositions.is_empty();
    let mut clients = Vec::new();
    for (client_key, client) in &loaded.root.clients {
        let provider_key = client.venue.as_str();
        binding_for_provider_key(provider_key).ok_or_else(|| {
            BoltV3OperatorArtifactError::UnsupportedProvider {
                client_key: client_key.clone(),
                provider_key: provider_key.to_string(),
            }
        })?;
        let has_data = client.data.is_some();
        let has_execution = client.execution.is_some();
        let registered = registration_summary.clients.get(client_key);
        let data_client_registered_through_live_node =
            registered.is_some_and(|summary| summary.data);
        let execution_client_registered_through_live_node =
            registered.is_some_and(|summary| summary.execution);
        clients.push(DataClientLiveNodeMappingClientSource {
            client_key_hash: sha256_text(client_key),
            provider_key: provider_key.to_string(),
            has_data,
            has_execution,
            provider_binding_registered: true,
            data_block_flows_through_mapping_source: has_data && source_path_proven,
            data_client_registered_through_live_node,
            execution_block_flows_through_mapping_source: has_execution && source_path_proven,
            execution_client_registered_through_live_node,
            production_usable: false,
            readiness_status: DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_STATUS_NOT_PRODUCTION_USABLE,
        });
    }
    clients.sort_by(|left, right| {
        (left.provider_key.as_str(), left.client_key_hash.as_str())
            .cmp(&(right.provider_key.as_str(), right.client_key_hash.as_str()))
    });

    Ok(DataClientLiveNodeMappingSourceArtifact {
        schema_version: DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_LIVE_NODE_MAPPING_SOURCE_RECORD_KIND,
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        live_node_source_path_hash: source_path_hash(live_node_source_path),
        live_node_source_sha256: hex::encode(Sha256::digest(&live_node_source_bytes)),
        adapter_mapping_source_path_hash: source_path_hash(adapter_mapping_source_path),
        adapter_mapping_source_sha256: hex::encode(Sha256::digest(&adapter_mapping_source_bytes)),
        provider_registry_source_path_hash: source_path_hash(provider_registry_source_path),
        provider_registry_source_sha256: hex::encode(Sha256::digest(
            &provider_registry_source_bytes,
        )),
        live_node_calls_adapter_mapping,
        live_node_registers_mapped_clients,
        adapter_mapping_iterates_loaded_clients,
        adapter_mapping_dispatches_provider_binding,
        adapter_mapping_uses_provider_lookup,
        provider_registry_exposes_binding_lookup,
        unsupported_dispositions,
        clients,
    })
}

fn build_data_client_nt_source_capability_artifact(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    nt_adapter_source_path: &Path,
    max_source_bytes: u64,
) -> Result<DataClientNtSourceCapabilityArtifact, BoltV3OperatorArtifactError> {
    if client_key.trim().is_empty() {
        return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "client_key",
        });
    }
    if max_source_bytes == 0 {
        return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "max_source_bytes",
        });
    }

    let client = loaded.root.clients.get(client_key).ok_or(
        BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
            field: "client_key",
        },
    )?;
    let provider_key = client.venue.as_str();
    binding_for_provider_key(provider_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let source_bytes =
        read_file_bounded(nt_adapter_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DataClientNtSourceRead {
                path: nt_adapter_source_path.to_path_buf(),
                source,
            }
        })?;
    let source_text = String::from_utf8_lossy(&source_bytes);
    let metadata_request_instruments_surface_present =
        nt_source_contains_any(&source_text, &["request_instruments"]);
    let metadata_request_instrument_surface_present =
        nt_source_contains_any(&source_text, &["request_instrument"]);
    let quote_subscription_surface_present =
        nt_source_contains_any(&source_text, &["subscribe_quote", "SubscribeQuote"]);
    let book_subscription_surface_present = nt_source_contains_any(
        &source_text,
        &[
            "subscribe_book",
            "subscribe_order_book",
            "SubscribeBook",
            "SubscribeOrderBook",
        ],
    );
    let ticker_subscription_surface_present =
        nt_source_contains_any(&source_text, &["subscribe_ticker", "SubscribeTicker"]);

    let mut unsupported_dispositions = Vec::new();
    if !metadata_request_instruments_surface_present && !metadata_request_instrument_surface_present
    {
        unsupported_dispositions.push("metadata_request_source_marker_missing");
    }
    if !quote_subscription_surface_present {
        unsupported_dispositions.push("quote_subscription_source_marker_missing");
    }
    if !book_subscription_surface_present {
        unsupported_dispositions.push("book_subscription_source_marker_missing");
    }
    if !ticker_subscription_surface_present {
        unsupported_dispositions.push("ticker_subscription_source_marker_missing");
    }

    Ok(DataClientNtSourceCapabilityArtifact {
        schema_version: DATA_CLIENT_NT_SOURCE_CAPABILITY_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_NT_SOURCE_CAPABILITY_RECORD_KIND,
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        client_key_hash: sha256_text(client_key),
        provider_key: provider_key.to_string(),
        nt_source_path_hash: source_path_hash(nt_adapter_source_path),
        nt_source_sha256: hex::encode(Sha256::digest(&source_bytes)),
        nt_source_byte_len: source_bytes.len(),
        metadata_request_instruments_surface_present,
        metadata_request_instrument_surface_present,
        quote_subscription_surface_present,
        book_subscription_surface_present,
        ticker_subscription_surface_present,
        unsupported_dispositions,
        production_usable: false,
        readiness_status: DATA_CLIENT_NT_SOURCE_CAPABILITY_STATUS_NOT_PRODUCTION_USABLE,
    })
}

fn nt_source_contains_any(source_text: &str, markers: &[&str]) -> bool {
    source_contains_any_symbol(source_text, markers)
}

fn source_contains_any_symbol(source_text: &str, markers: &[&str]) -> bool {
    markers
        .iter()
        .any(|marker| source_symbol_present(source_text, marker))
}

fn source_symbol_present(source_text: &str, marker: &str) -> bool {
    let mut offset = 0;
    while let Some(index) = source_text[offset..].find(marker) {
        let start = offset + index;
        let end = start + marker.len();
        let before = source_text[..start].chars().next_back();
        let after = source_text[end..].chars().next();
        if !source_identifier_char(before) && !source_identifier_char(after) {
            return true;
        }
        offset = end;
    }
    false
}

fn source_identifier_char(char: Option<char>) -> bool {
    match char {
        Some(char) => char == '_' || char.is_ascii_alphanumeric(),
        None => false,
    }
}

fn source_path_hash(path: &Path) -> String {
    sha256_text(&normalize_path_components(path).to_string_lossy())
}

fn build_data_client_readiness_source_artifact(
    loaded: &LoadedBoltV3Config,
) -> Result<DataClientReadinessSourceArtifact, BoltV3OperatorArtifactError> {
    let plan = bolt_v3_market_families::market_identity_plan_from_config(loaded)
        .map_err(|error| BoltV3OperatorArtifactError::MarketSelection(anyhow!(error)))?;
    let mut targets_by_client: BTreeMap<String, Vec<DataClientReadinessTargetSource>> =
        BTreeMap::new();
    for target in plan.execution_client_target_refs() {
        let target_source = DataClientReadinessTargetSource {
            configured_target_id_hash: sha256_text(target.configured_target_id),
            family_key: target.family_key,
        };
        match targets_by_client.entry(target.execution_client_id.to_string()) {
            Entry::Occupied(mut entry) => entry.get_mut().push(target_source),
            Entry::Vacant(entry) => {
                entry.insert(vec![target_source]);
            }
        }
    }

    let mut clients = Vec::new();
    for (client_key, client) in &loaded.root.clients {
        let provider_key = client.venue.as_str();
        let binding = binding_for_provider_key(provider_key).ok_or_else(|| {
            BoltV3OperatorArtifactError::UnsupportedProvider {
                client_key: client_key.clone(),
                provider_key: provider_key.to_string(),
            }
        })?;
        let mut market_identity_targets = Vec::new();
        if let Some(targets) = targets_by_client.remove(client_key) {
            market_identity_targets = targets;
        }
        let has_data = client.data.is_some();
        let has_execution = client.execution.is_some();
        let has_secrets = client.secrets.is_some();
        let readiness_probe_targets = data_client_readiness_probe_targets(client)?;
        let data_config_field_names = toml_table_field_names(client.data.as_ref());
        let data_config_field_fingerprints = toml_table_field_fingerprints(client.data.as_ref())?;
        let market_coverage_config_values =
            toml_table_selected_values(client.data.as_ref(), data_client_market_coverage_field)?;
        let market_coverage_config_field_fingerprints = toml_table_selected_field_fingerprints(
            client.data.as_ref(),
            data_client_market_coverage_field,
        )?;
        clients.push(DataClientReadinessClientSource {
            client_key_hash: sha256_text(client_key),
            provider_key: provider_key.to_string(),
            has_data,
            has_execution,
            has_secrets,
            data_only_scope: has_data && !has_execution && !has_secrets,
            strategy_routed: !market_identity_targets.is_empty(),
            production_usable: false,
            readiness_status: DATA_CLIENT_READINESS_STATUS_NOT_PRODUCTION_USABLE,
            supported_market_families: binding.supported_market_families.to_vec(),
            required_secret_blocks: binding
                .required_secret_blocks
                .iter()
                .map(|requirement| {
                    format!(
                        "{}:{}",
                        provider_credentialed_block_name(requirement.block),
                        requirement.consumer
                    )
                })
                .collect(),
            data_config_sha256: client.data.as_ref().map(sha256_toml_value).transpose()?,
            data_config_field_fingerprints,
            market_coverage_config_values,
            market_coverage_config_field_fingerprints,
            timeout_policy_field_names: classified_policy_field_names(
                &data_config_field_names,
                data_client_timeout_policy_field,
            ),
            retry_policy_field_names: classified_policy_field_names(
                &data_config_field_names,
                data_client_retry_policy_field,
            ),
            freshness_policy_field_names: classified_policy_field_names(
                &data_config_field_names,
                data_client_freshness_policy_field,
            ),
            reconnect_policy_field_names: classified_policy_field_names(
                &data_config_field_names,
                data_client_reconnect_policy_field,
            ),
            rate_limit_policy_field_names: classified_policy_field_names(
                &data_config_field_names,
                data_client_rate_limit_policy_field,
            ),
            missing_behavior_proofs: DATA_CLIENT_MISSING_BEHAVIOR_PROOFS.to_vec(),
            data_config_field_names,
            execution_config_sha256: client
                .execution
                .as_ref()
                .map(sha256_toml_value)
                .transpose()?,
            execution_config_field_names: toml_table_field_names(client.execution.as_ref()),
            execution_config_field_fingerprints: toml_table_field_fingerprints(
                client.execution.as_ref(),
            )?,
            market_identity_targets,
            readiness_probe_targets,
        });
    }
    clients.sort_by(|left, right| {
        (left.provider_key.as_str(), left.client_key_hash.as_str())
            .cmp(&(right.provider_key.as_str(), right.client_key_hash.as_str()))
    });

    Ok(DataClientReadinessSourceArtifact {
        schema_version: DATA_CLIENT_READINESS_SOURCE_SCHEMA_VERSION,
        record_kind: DATA_CLIENT_READINESS_SOURCE_RECORD_KIND,
        generated_at_unix_seconds: generated_at_unix_seconds()?,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        clients,
    })
}

fn data_client_readiness_probe_targets(
    client: &crate::bolt_v3_config::ClientBlock,
) -> Result<Vec<DataClientReadinessProbeTargetSource>, BoltV3OperatorArtifactError> {
    let Some(readiness_probe) = &client.readiness_probe else {
        return Ok(Vec::new());
    };
    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            let Some(quote_targets) = &readiness_probe.quote_targets else {
                return Err(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
                    field: "clients.<id>.readiness_probe.quote_targets",
                });
            };
            Ok(quote_targets
                .iter()
                .map(|(target_id, target)| DataClientReadinessProbeTargetSource {
                    quote_target_source: "configured",
                    configured_target_id_hash: Some(sha256_text(target_id)),
                    event_kind: data_client_readiness_probe_event_kind(
                        readiness_probe.market_data_kind,
                    ),
                    book_type: readiness_probe
                        .book_type
                        .map(data_client_readiness_probe_book_type_name),
                    instrument_id_hash: Some(sha256_text(&target.instrument_id.to_string())),
                    max_metadata_quote_targets: None,
                    allow_metadata_target_sampling: false,
                    min_observed_targets: readiness_probe.min_observed_targets,
                })
                .collect())
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            let allow_metadata_target_sampling = readiness_probe
                .allow_metadata_target_sampling
                .ok_or(BoltV3OperatorArtifactError::DataClientNtSourceInvalid {
                    field: "clients.<id>.readiness_probe.allow_metadata_target_sampling",
                })?;
            Ok(vec![DataClientReadinessProbeTargetSource {
                quote_target_source: "metadata_response",
                configured_target_id_hash: None,
                event_kind: data_client_readiness_probe_event_kind(
                    readiness_probe.market_data_kind,
                ),
                book_type: readiness_probe
                    .book_type
                    .map(data_client_readiness_probe_book_type_name),
                instrument_id_hash: None,
                max_metadata_quote_targets: readiness_probe.max_metadata_quote_targets,
                allow_metadata_target_sampling,
                min_observed_targets: readiness_probe.min_observed_targets,
            }])
        }
    }
}

fn data_client_readiness_probe_event_kind(
    market_data_kind: DataClientReadinessProbeMarketDataKind,
) -> &'static str {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => "quote",
        DataClientReadinessProbeMarketDataKind::Book => "book",
        DataClientReadinessProbeMarketDataKind::Trade => "trade",
    }
}

fn data_client_readiness_probe_book_type_name(
    book_type: DataClientReadinessProbeBookType,
) -> &'static str {
    match book_type {
        DataClientReadinessProbeBookType::L1Mbp => "l1_mbp",
        DataClientReadinessProbeBookType::L2Mbp => "l2_mbp",
        DataClientReadinessProbeBookType::L3Mbo => "l3_mbo",
    }
}

fn provider_credentialed_block_name(block: ProviderCredentialedBlock) -> &'static str {
    match block {
        ProviderCredentialedBlock::Data => "data",
        ProviderCredentialedBlock::Execution => "execution",
    }
}

fn toml_table_field_names(value: Option<&toml::Value>) -> Vec<String> {
    let mut names: Vec<String> = match value.and_then(toml::Value::as_table) {
        Some(table) => table.keys().cloned().collect(),
        None => Vec::new(),
    };
    names.sort();
    names
}

fn toml_table_field_fingerprints(
    value: Option<&toml::Value>,
) -> Result<Vec<DataClientReadinessConfigFieldFingerprint>, BoltV3OperatorArtifactError> {
    let mut fingerprints = Vec::new();
    if let Some(table) = value.and_then(toml::Value::as_table) {
        for (field_name, value) in table {
            fingerprints.push(DataClientReadinessConfigFieldFingerprint {
                field_name: field_name.clone(),
                value_kind: toml_value_kind(value),
                value_item_count: toml_value_item_count(value),
                value_sha256: sha256_toml_value(value)?,
            });
        }
    }
    fingerprints.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    Ok(fingerprints)
}

fn toml_table_selected_field_fingerprints(
    value: Option<&toml::Value>,
    predicate: fn(&str) -> bool,
) -> Result<Vec<DataClientReadinessConfigFieldFingerprint>, BoltV3OperatorArtifactError> {
    let mut fingerprints = Vec::new();
    if let Some(table) = value.and_then(toml::Value::as_table) {
        for (field_name, value) in table {
            if predicate(field_name) {
                fingerprints.push(DataClientReadinessConfigFieldFingerprint {
                    field_name: field_name.clone(),
                    value_kind: toml_value_kind(value),
                    value_item_count: toml_value_item_count(value),
                    value_sha256: sha256_toml_value(value)?,
                });
            }
        }
    }
    fingerprints.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    Ok(fingerprints)
}

fn toml_table_selected_values(
    value: Option<&toml::Value>,
    predicate: fn(&str) -> bool,
) -> Result<BTreeMap<String, serde_json::Value>, BoltV3OperatorArtifactError> {
    let mut values = BTreeMap::new();
    if let Some(table) = value.and_then(toml::Value::as_table) {
        for (field_name, value) in table {
            if predicate(field_name) {
                values.insert(
                    field_name.clone(),
                    serde_json::to_value(value).map_err(BoltV3OperatorArtifactError::Serialize)?,
                );
            }
        }
    }
    Ok(values)
}

fn toml_value_kind(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

fn toml_value_item_count(value: &toml::Value) -> Option<usize> {
    match value {
        toml::Value::Array(values) => Some(values.len()),
        toml::Value::Table(values) => Some(values.len()),
        _ => None,
    }
}

fn classified_policy_field_names(
    field_names: &[String],
    predicate: fn(&str) -> bool,
) -> Vec<String> {
    field_names
        .iter()
        .filter(|field| predicate(field))
        .cloned()
        .collect()
}

fn data_client_timeout_policy_field(field: &str) -> bool {
    field.contains("timeout")
}

fn data_client_retry_policy_field(field: &str) -> bool {
    field.contains("retry") || field.contains("retries")
}

fn data_client_freshness_policy_field(field: &str) -> bool {
    field.contains("freshness")
        || field.contains("interval")
        || field.contains("poll")
        || field.contains("debounce")
}

fn data_client_reconnect_policy_field(field: &str) -> bool {
    field.contains("reconnect")
}

fn data_client_rate_limit_policy_field(field: &str) -> bool {
    field.contains("rate_limit") || field.contains("throttle")
}

fn data_client_market_coverage_field(field: &str) -> bool {
    matches!(
        field,
        "product_type"
            | "product_types"
            | "instrument_type"
            | "instrument_types"
            | "contract_type"
            | "contract_types"
            | "instrument_family"
            | "instrument_families"
            | "load_spreads"
    )
}

fn sha256_toml_value(value: &toml::Value) -> Result<String, BoltV3OperatorArtifactError> {
    let bytes = serde_json::to_vec(value).map_err(BoltV3OperatorArtifactError::Serialize)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn build_phase8_financial_envelope(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> anyhow::Result<Phase8FinancialEnvelopeEvidenceFile> {
    Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
}

fn runtime_strategy_id_for_loaded_strategy(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> anyhow::Result<String> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(|| anyhow!("strategy instance `{strategy_instance_id}` is not loaded"))?;
    let raw = raw_taker_config(strategy, loaded).map_err(|error| anyhow!(error.to_string()))?;
    raw.get("strategy_id")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow!("strategy instance `{strategy_instance_id}` has no runtime strategy_id")
        })
}

pub fn write_approval_nonce_artifact(
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_approval_nonce_artifact()?;
    write_json_artifact_create_new(path, &artifact)
}

pub fn build_market_selection_source_artifact(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Result<Phase8MarketSelectionSourceEvidenceFile, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(|| {
            BoltV3OperatorArtifactError::MarketSelection(anyhow!(
                "strategy instance `{strategy_instance_id}` is not loaded"
            ))
        })?;
    let target =
        bolt_v3_market_families::target_runtime_fields_from_target(&strategy.config.target)
            .map_err(|error| BoltV3OperatorArtifactError::MarketSelection(anyhow!(error)))?;
    let selection_target = MarketSelectionTarget {
        family_key: &target.rotating_market_family,
        underlying_asset: &target.underlying_asset,
        cadence_seconds: target.cadence_seconds,
        cadence_slug_token: &target.cadence_slug_token,
    };
    let candidate_windows =
        bolt_v3_market_families::market_selection_candidate_windows_from_target(
            selection_target,
            now_milliseconds,
        )
        .map_err(|error| BoltV3OperatorArtifactError::MarketSelection(anyhow!(error)))?;
    let selected = bolt_v3_market_families::select_binary_option_market_from_target(
        selection_target,
        instruments,
        now_milliseconds,
    )
    .ok_or(
        BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
            prerequisite: "missing source-bound market selection from NT instrument facts",
        },
    )?;

    Phase8MarketSelectionSourceEvidenceFile::from_market_family_selection(
        now_milliseconds,
        &candidate_windows,
        &selected,
    )
    .map_err(BoltV3OperatorArtifactError::MarketSelection)
}

pub fn write_market_selection_source_artifact(
    _loaded: &LoadedBoltV3Config,
    _strategy_instance_id: &str,
    _instruments: &[InstrumentAny],
    _now_milliseconds: u64,
    _path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    Err(
        BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
            prerequisite: MARKET_SELECTION_SOURCE_BLOCKER,
        },
    )
}

pub fn write_market_selection_source_artifact_from_decision_evidence_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    decision_evidence_path: &Path,
    max_decision_evidence_bytes: u64,
    instruments: &[InstrumentAny],
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_market_selection_source_artifact_from_decision_evidence(
        loaded,
        strategy_instance_id,
        decision_evidence_path,
        max_decision_evidence_bytes,
        instruments,
        path,
    )?;
    write_json_artifact_create_new(path, &artifact)
}

fn build_market_selection_source_artifact_from_decision_evidence(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    decision_evidence_path: &Path,
    max_decision_evidence_bytes: u64,
    instruments: &[InstrumentAny],
    path: &Path,
) -> Result<Phase8MarketSelectionSourceEvidenceFile, BoltV3OperatorArtifactError> {
    let chain = read_latest_entry_decision_evidence_chain(
        decision_evidence_path,
        max_decision_evidence_bytes,
    )
    .map_err(|_| BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
        prerequisite: "T046 remains blocked: missing complete source-bound strategy decision input",
    })?;
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let runtime_strategy_id = runtime_strategy_id_for_loaded_strategy(loaded, strategy_instance_id)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    if chain.snapshot.configured_target_id != financial_envelope.configured_target_id() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy decision target does not match config",
            },
        );
    }
    if chain.snapshot.price_to_beat_source.trim() != financial_envelope.price_to_beat_source() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy decision price-to-beat source does not match config",
            },
        );
    }
    if !source_bound_price_to_beat_value_is_usable(&chain.snapshot.price_to_beat_value) {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy decision price-to-beat value is missing or unusable",
            },
        );
    }
    let market_selection_timestamp_ms =
        chain.snapshot.market_selection_timestamp_ms.ok_or(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy decision market-selection timestamp is missing",
            },
        )?;
    let artifact = build_market_selection_source_artifact(
        loaded,
        strategy_instance_id,
        instruments,
        market_selection_timestamp_ms,
    )?;
    let source_sha256 = json_artifact_sha256(&artifact)?;
    let _ =
        Phase8StrategyInputEvidenceFile::from_runtime_snapshot_and_market_selection_source(
            &chain.snapshot,
            financial_envelope.strategy_instance_id(),
            &runtime_strategy_id,
            &artifact,
            path.to_string_lossy(),
            &source_sha256,
            &[],
        )
        .map_err(|_| BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
            prerequisite: "T046 remains blocked: market selection source does not match source-bound strategy decision input",
    })?;
    Ok(artifact)
}

pub fn write_market_selection_source_artifact_from_decision_evidence_and_instrument_source_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    decision_evidence_path: &Path,
    max_decision_evidence_bytes: u64,
    instrument_source_path: &Path,
    max_instrument_source_bytes: u64,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let instrument_source_bytes =
        read_file_bounded(instrument_source_path, max_instrument_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::MarketSelectionInstrumentSourceRead {
                path: instrument_source_path.to_path_buf(),
                source,
            },
        )?;
    let instruments: Vec<InstrumentAny> = serde_json::from_slice(&instrument_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::MarketSelectionInstrumentSourceParse {
                path: instrument_source_path.to_path_buf(),
                source,
            },
        )?;
    if instruments.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionInstrumentSourceInvalid {
                field: "instruments",
            },
        );
    }
    let artifact = build_market_selection_source_artifact_from_decision_evidence(
        loaded,
        strategy_instance_id,
        decision_evidence_path,
        max_decision_evidence_bytes,
        &instruments,
        path,
    )?;
    let decision_evidence_bytes =
        read_file_bounded(decision_evidence_path, max_decision_evidence_bytes).map_err(|_| {
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: missing complete source-bound strategy decision input",
            }
        })?;
    let provenance = Phase8MarketSelectionRuntimeProvenance::new(
        decision_evidence_path.to_string_lossy(),
        hex::encode(Sha256::digest(&decision_evidence_bytes)),
        instrument_source_path.to_string_lossy(),
        hex::encode(Sha256::digest(&instrument_source_bytes)),
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
            prerequisite: "T046 remains blocked: market selection runtime provenance is invalid",
        },
    )?;
    write_json_artifact_create_new(path, &artifact.with_runtime_provenance(provenance))
}

fn source_bound_price_to_beat_value_is_usable(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && Decimal::from_str_exact(trimmed).is_ok_and(|value| value > Decimal::ZERO)
}

pub fn write_abort_plan_artifact(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    _path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let _ =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    Err(BoltV3OperatorArtifactError::AbortPrerequisiteUnproven {
        prerequisite: "panic gate and service policy",
    })
}

pub fn write_abort_plan_artifact_from_source_proofs(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    proofs: Phase8AbortPlanSourceProofs<'_>,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let artifact = Phase8AbortPlanEvidenceFile::from_financial_envelope_and_source_proofs(
        &financial_envelope,
        proofs,
    )
    .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    write_json_artifact_create_new(path, &artifact)
}

#[derive(Debug)]
struct OwnedPhase8AbortPlanSourceProofs {
    cancel_if_open_evidence_hash: String,
    nt_accepted_venue_pending_abort_evidence_hash: String,
    partial_fill_abort_evidence_hash: String,
    network_partition_during_submit_abort_evidence_hash: String,
    panic_gate_trip_abort_evidence_hash: String,
}

impl OwnedPhase8AbortPlanSourceProofs {
    fn as_source_proofs(&self) -> Phase8AbortPlanSourceProofs<'_> {
        Phase8AbortPlanSourceProofs {
            cancel_if_open_defined: true,
            cancel_if_open_evidence_hash: &self.cancel_if_open_evidence_hash,
            nt_accepted_venue_pending_abort_defined: true,
            nt_accepted_venue_pending_abort_evidence_hash: &self
                .nt_accepted_venue_pending_abort_evidence_hash,
            partial_fill_abort_defined: true,
            partial_fill_abort_evidence_hash: &self.partial_fill_abort_evidence_hash,
            network_partition_during_submit_abort_defined: true,
            network_partition_during_submit_abort_evidence_hash: &self
                .network_partition_during_submit_abort_evidence_hash,
            panic_gate_trip_abort_defined: true,
            panic_gate_trip_abort_evidence_hash: &self.panic_gate_trip_abort_evidence_hash,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Phase8AbortPlanSourceProofBundle {
    schema_version: u32,
    record_kind: String,
    cancel_if_open_defined: bool,
    cancel_if_open_evidence: serde_json::Value,
    nt_accepted_venue_pending_abort_defined: bool,
    nt_accepted_venue_pending_abort_evidence: serde_json::Value,
    partial_fill_abort_defined: bool,
    partial_fill_abort_evidence: serde_json::Value,
    network_partition_during_submit_abort_defined: bool,
    network_partition_during_submit_abort_evidence: serde_json::Value,
    panic_gate_trip_abort_defined: bool,
    panic_gate_trip_abort_evidence: serde_json::Value,
}

impl Phase8AbortPlanSourceProofBundle {
    fn into_source_proofs(
        self,
    ) -> Result<OwnedPhase8AbortPlanSourceProofs, BoltV3OperatorArtifactError> {
        if self.schema_version != ABORT_PLAN_SOURCE_PROOF_BUNDLE_SCHEMA_VERSION {
            return Err(BoltV3OperatorArtifactError::AbortPlanSourceBundleInvalid {
                field: "schema_version",
            });
        }
        if self.record_kind != ABORT_PLAN_SOURCE_PROOF_BUNDLE_RECORD_KIND {
            return Err(BoltV3OperatorArtifactError::AbortPlanSourceBundleInvalid {
                field: "record_kind",
            });
        }
        require_abort_plan_source_bundle_bool(
            "cancel_if_open_defined",
            self.cancel_if_open_defined,
        )?;
        require_abort_plan_source_bundle_bool(
            "nt_accepted_venue_pending_abort_defined",
            self.nt_accepted_venue_pending_abort_defined,
        )?;
        require_abort_plan_source_bundle_bool(
            "partial_fill_abort_defined",
            self.partial_fill_abort_defined,
        )?;
        require_abort_plan_source_bundle_bool(
            "network_partition_during_submit_abort_defined",
            self.network_partition_during_submit_abort_defined,
        )?;
        require_abort_plan_source_bundle_bool(
            "panic_gate_trip_abort_defined",
            self.panic_gate_trip_abort_defined,
        )?;
        Ok(OwnedPhase8AbortPlanSourceProofs {
            cancel_if_open_evidence_hash: abort_plan_source_bundle_evidence_hash(
                "cancel_if_open_evidence",
                &self.cancel_if_open_evidence,
            )?,
            nt_accepted_venue_pending_abort_evidence_hash: abort_plan_source_bundle_evidence_hash(
                "nt_accepted_venue_pending_abort_evidence",
                &self.nt_accepted_venue_pending_abort_evidence,
            )?,
            partial_fill_abort_evidence_hash: abort_plan_source_bundle_evidence_hash(
                "partial_fill_abort_evidence",
                &self.partial_fill_abort_evidence,
            )?,
            network_partition_during_submit_abort_evidence_hash:
                abort_plan_source_bundle_evidence_hash(
                    "network_partition_during_submit_abort_evidence",
                    &self.network_partition_during_submit_abort_evidence,
                )?,
            panic_gate_trip_abort_evidence_hash: abort_plan_source_bundle_evidence_hash(
                "panic_gate_trip_abort_evidence",
                &self.panic_gate_trip_abort_evidence,
            )?,
        })
    }
}

fn read_abort_plan_source_bundle_file(
    path: &Path,
    max_bytes: u64,
) -> Result<Phase8AbortPlanSourceProofBundle, BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::AbortPlanSourceBundleRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::AbortPlanSourceBundleParse {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn require_abort_plan_source_bundle_bool(
    field: &'static str,
    value: bool,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::AbortPlanSourceBundleInvalid { field })
    }
}

fn abort_plan_source_bundle_evidence_hash(
    field: &'static str,
    value: &serde_json::Value,
) -> Result<String, BoltV3OperatorArtifactError> {
    if value.is_null() {
        return Err(BoltV3OperatorArtifactError::AbortPlanSourceBundleInvalid { field });
    }
    json_artifact_sha256(value)
}

pub fn write_abort_plan_artifact_from_source_bundle_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    source_bundle_path: &Path,
    max_source_bundle_bytes: u64,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let bundle = read_abort_plan_source_bundle_file(source_bundle_path, max_source_bundle_bytes)?;
    let proofs = bundle.into_source_proofs()?;
    write_abort_plan_artifact_from_source_proofs(
        loaded,
        strategy_instance_id,
        proofs.as_source_proofs(),
        path,
    )
}

pub fn write_abort_plan_artifact_from_source_collectors(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    strategy_source_path: &Path,
    submit_admission_source_path: &Path,
    max_source_bytes: u64,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let cancel_if_open =
        collect_abort_plan_cancel_if_open_source_proof(strategy_source_path, max_source_bytes)?;
    let venue_pending = collect_abort_plan_nt_accepted_venue_pending_source_proof(
        strategy_source_path,
        max_source_bytes,
    )?;
    let partial_fill =
        collect_abort_plan_partial_fill_source_proof(strategy_source_path, max_source_bytes)?;
    let network_partition =
        collect_abort_plan_network_partition_source_proof(strategy_source_path, max_source_bytes)?;
    let panic_gate = collect_abort_plan_panic_gate_service_policy_source_proof(
        strategy_source_path,
        submit_admission_source_path,
        max_source_bytes,
    )?;
    // Resolve the caller-provided strategy root through the source-set registry
    // so collector-derived artifacts bind to the same bytes as live/final gates.
    let strategy_source_sha256 =
        abort_plan_strategy_source_digest(strategy_source_path, max_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceRead {
                path: strategy_source_path.to_path_buf(),
                source,
            },
        )?;
    let submit_admission_source_sha256 =
        canonical_source_digest(submit_admission_source_path, max_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceRead {
                path: submit_admission_source_path.to_path_buf(),
                source,
            },
        )?;
    let proofs = OwnedPhase8AbortPlanSourceProofs {
        cancel_if_open_evidence_hash: cancel_if_open.cancel_if_open_evidence_hash,
        nt_accepted_venue_pending_abort_evidence_hash: venue_pending
            .nt_accepted_venue_pending_abort_evidence_hash,
        partial_fill_abort_evidence_hash: partial_fill.partial_fill_abort_evidence_hash,
        network_partition_during_submit_abort_evidence_hash: network_partition
            .network_partition_during_submit_abort_evidence_hash,
        panic_gate_trip_abort_evidence_hash: panic_gate.panic_gate_trip_abort_evidence_hash,
    };
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let artifact =
        Phase8AbortPlanEvidenceFile::from_financial_envelope_and_collector_source_proofs(
            &financial_envelope,
            proofs.as_source_proofs(),
            &strategy_source_sha256,
            &submit_admission_source_sha256,
        )
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    write_json_artifact_create_new(path, &artifact)
}

pub fn collect_abort_plan_cancel_if_open_source_proof(
    strategy_source_path: &Path,
    max_strategy_source_bytes: u64,
) -> Result<Phase8AbortPlanCancelIfOpenSourceProof, BoltV3OperatorArtifactError> {
    let strategy_source_sha256 =
        abort_plan_strategy_source_digest(strategy_source_path, max_strategy_source_bytes)
            .map_err(
                |source| BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceRead {
                    path: strategy_source_path.to_path_buf(),
                    source,
                },
            )?;
    // Grep the per-file UTF-8 module text (identity OR directory), NOT the framed
    // canonical byte stream. The framed stream interleaves binary frame bytes
    // (relative-path strings, NUL separators, u64-LE length frames) that are valid
    // UTF-8 only by luck of the current file sizes; `canonical_module_text` joins
    // each file's text in the same canonical order WITHOUT those frame bytes, so
    // the contract grep is layout-independent and cannot break as the strategy
    // directory grows. The recorded digest above stays over the framed stream.
    let strategy_source =
        abort_plan_strategy_source_text(strategy_source_path, max_strategy_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceRead {
                path: strategy_source_path.to_path_buf(),
                source,
            },
        )?;
    let contract = require_abort_plan_cancel_if_open_contract(&strategy_source)?;

    let proof_input = Phase8AbortPlanCancelIfOpenSourceProofHashInput {
        schema_version: ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_PROOF_RECORD_KIND,
        strategy_source_sha256: strategy_source_sha256.as_str(),
        forced_flat_cancel_before_exit_pending: contract.forced_flat_cancel_before_exit_pending,
    };
    let cancel_if_open_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8AbortPlanCancelIfOpenSourceProof {
        cancel_if_open_evidence_hash,
    })
}

pub fn collect_abort_plan_nt_accepted_venue_pending_source_proof(
    strategy_source_path: &Path,
    max_strategy_source_bytes: u64,
) -> Result<Phase8AbortPlanNtAcceptedVenuePendingSourceProof, BoltV3OperatorArtifactError> {
    let strategy_source_sha256 =
        abort_plan_strategy_source_digest(strategy_source_path, max_strategy_source_bytes)
            .map_err(|source| {
                BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceRead {
                    path: strategy_source_path.to_path_buf(),
                    source,
                }
            })?;
    // Per-file UTF-8 module text, not the framed canonical stream (see
    // collect_abort_plan_cancel_if_open_source_proof). Digest stays over the frame.
    let strategy_source =
        abort_plan_strategy_source_text(strategy_source_path, max_strategy_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceRead {
                path: strategy_source_path.to_path_buf(),
                source,
            },
        )?;
    let contract = require_abort_plan_nt_accepted_venue_pending_contract(&strategy_source)?;

    let proof_input = Phase8AbortPlanNtAcceptedVenuePendingSourceProofHashInput {
        schema_version: ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SOURCE_PROOF_RECORD_KIND,
        strategy_source_sha256: strategy_source_sha256.as_str(),
        exit_pending_before_submit: contract.exit_pending_before_submit,
        submit_error_restores_managed_position: contract.submit_error_restores_managed_position,
        terminal_handlers_mark_exit_order_terminal: contract
            .terminal_handlers_mark_exit_order_terminal,
    };
    let nt_accepted_venue_pending_abort_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8AbortPlanNtAcceptedVenuePendingSourceProof {
        nt_accepted_venue_pending_abort_evidence_hash,
    })
}

pub fn collect_abort_plan_partial_fill_source_proof(
    strategy_source_path: &Path,
    max_strategy_source_bytes: u64,
) -> Result<Phase8AbortPlanPartialFillSourceProof, BoltV3OperatorArtifactError> {
    let strategy_source_sha256 =
        abort_plan_strategy_source_digest(strategy_source_path, max_strategy_source_bytes)
            .map_err(
                |source| BoltV3OperatorArtifactError::AbortPlanPartialFillSourceRead {
                    path: strategy_source_path.to_path_buf(),
                    source,
                },
            )?;
    // Per-file UTF-8 module text, not the framed canonical stream (see
    // collect_abort_plan_cancel_if_open_source_proof). Digest stays over the frame.
    let strategy_source =
        abort_plan_strategy_source_text(strategy_source_path, max_strategy_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanPartialFillSourceRead {
                path: strategy_source_path.to_path_buf(),
                source,
            },
        )?;
    let contract = require_abort_plan_partial_fill_contract(&strategy_source)?;

    let proof_input = Phase8AbortPlanPartialFillSourceProofHashInput {
        schema_version: ABORT_PLAN_PARTIAL_FILL_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: ABORT_PLAN_PARTIAL_FILL_SOURCE_PROOF_RECORD_KIND,
        strategy_source_sha256: strategy_source_sha256.as_str(),
        partial_fill_waits_for_position_close: contract.partial_fill_waits_for_position_close,
        position_close_completes_exit: contract.position_close_completes_exit,
        residual_after_fill_preserved: contract.residual_after_fill_preserved,
        terminal_without_flat_preserves_managed: contract.terminal_without_flat_preserves_managed,
    };
    let partial_fill_abort_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8AbortPlanPartialFillSourceProof {
        partial_fill_abort_evidence_hash,
    })
}

pub fn collect_abort_plan_network_partition_source_proof(
    strategy_source_path: &Path,
    max_strategy_source_bytes: u64,
) -> Result<Phase8AbortPlanNetworkPartitionSourceProof, BoltV3OperatorArtifactError> {
    let strategy_source_sha256 =
        abort_plan_strategy_source_digest(strategy_source_path, max_strategy_source_bytes)
            .map_err(
                |source| BoltV3OperatorArtifactError::AbortPlanNetworkPartitionSourceRead {
                    path: strategy_source_path.to_path_buf(),
                    source,
                },
            )?;
    // Per-file UTF-8 module text, not the framed canonical stream (see
    // collect_abort_plan_cancel_if_open_source_proof). Digest stays over the frame.
    let strategy_source =
        abort_plan_strategy_source_text(strategy_source_path, max_strategy_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanNetworkPartitionSourceRead {
                path: strategy_source_path.to_path_buf(),
                source,
            },
        )?;
    let contract = require_abort_plan_network_partition_contract(&strategy_source)?;

    let proof_input = Phase8AbortPlanNetworkPartitionSourceProofHashInput {
        schema_version: ABORT_PLAN_NETWORK_PARTITION_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: ABORT_PLAN_NETWORK_PARTITION_SOURCE_PROOF_RECORD_KIND,
        strategy_source_sha256: strategy_source_sha256.as_str(),
        submit_error_restores_managed_position: contract.submit_error_restores_managed_position,
    };
    let network_partition_during_submit_abort_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8AbortPlanNetworkPartitionSourceProof {
        network_partition_during_submit_abort_evidence_hash,
    })
}

pub fn collect_abort_plan_panic_gate_service_policy_source_proof(
    strategy_source_path: &Path,
    submit_admission_source_path: &Path,
    max_source_bytes: u64,
) -> Result<Phase8AbortPlanPanicGateServicePolicySourceProof, BoltV3OperatorArtifactError> {
    let strategy_source_sha256 =
        abort_plan_strategy_source_digest(strategy_source_path, max_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceRead {
                path: strategy_source_path.to_path_buf(),
                source,
            },
        )?;
    let submit_admission_source_sha256 = read_abort_plan_panic_gate_service_policy_source_digest(
        submit_admission_source_path,
        max_source_bytes,
    )?;
    // Grep the per-file UTF-8 module text of each root (identity OR directory),
    // not the framed canonical byte stream (see
    // collect_abort_plan_cancel_if_open_source_proof). For a single-file root the
    // module text is the verbatim file text, so this is byte-identical to the
    // previous from_utf8 path for submit_admission. The digests above stay over
    // the framed canonical stream of each root.
    let strategy_source =
        match abort_plan_strategy_source_text(strategy_source_path, max_source_bytes) {
            Ok(source) => source,
            Err(source) => {
                return Err(
                    BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceRead {
                        path: strategy_source_path.to_path_buf(),
                        source,
                    },
                );
            }
        };
    let submit_admission_source = read_abort_plan_panic_gate_service_policy_source_text(
        submit_admission_source_path,
        max_source_bytes,
    )?;
    let contract = require_abort_plan_panic_gate_service_policy_contract(
        &strategy_source,
        &submit_admission_source,
    )?;

    let proof_input = Phase8AbortPlanPanicGateServicePolicySourceProofHashInput {
        schema_version: ABORT_PLAN_PANIC_GATE_SERVICE_POLICY_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: ABORT_PLAN_PANIC_GATE_SERVICE_POLICY_SOURCE_PROOF_RECORD_KIND,
        strategy_source_sha256: strategy_source_sha256.as_str(),
        submit_admission_source_sha256: submit_admission_source_sha256.as_str(),
        panic_recovery_enters_blind_recovery: contract.panic_recovery_enters_blind_recovery,
        release_invariant_returns_error: contract.release_invariant_returns_error,
        submit_lifecycle_policy_from_config: contract.submit_lifecycle_policy_from_config,
        submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle: contract
            .submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle,
        replace_submit_policy_gates_service_submit: contract
            .replace_submit_policy_gates_service_submit,
    };
    let panic_gate_trip_abort_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8AbortPlanPanicGateServicePolicySourceProof {
        panic_gate_trip_abort_evidence_hash,
    })
}

fn abort_plan_strategy_source_digest(
    strategy_source_path: &Path,
    max_strategy_source_bytes: u64,
) -> io::Result<String> {
    let manifest_dir = abort_plan_strategy_manifest_dir(strategy_source_path)?;
    canonical_source_set_digest(
        &manifest_dir,
        registry_relative_roots(STRATEGY_KEY),
        max_strategy_source_bytes,
    )
}

fn abort_plan_strategy_source_text(
    strategy_source_path: &Path,
    max_strategy_source_bytes: u64,
) -> io::Result<String> {
    let manifest_dir = abort_plan_strategy_manifest_dir(strategy_source_path)?;
    canonical_module_source_set_text(
        &manifest_dir,
        registry_relative_roots(STRATEGY_KEY),
        max_strategy_source_bytes,
    )
}

fn abort_plan_strategy_manifest_dir(strategy_source_path: &Path) -> io::Result<PathBuf> {
    let primary_root = registry_relative_root(STRATEGY_KEY);
    let primary_components = registered_relative_root_components(primary_root)?;
    let canonical_strategy_source_path =
        fs::canonicalize(strategy_source_path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "strategy source root should resolve to the registered primary root {}: {}",
                    primary_root, error
                ),
            )
        })?;

    if !path_has_registered_relative_root_tail(&canonical_strategy_source_path, &primary_components)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "strategy source path {} must end with registered primary root {}",
                canonical_strategy_source_path.display(),
                primary_root
            ),
        ));
    }

    let mut manifest_dir = canonical_strategy_source_path.as_path();
    for _component in &primary_components {
        manifest_dir = manifest_dir.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "strategy source path {} cannot resolve a crate manifest root",
                    canonical_strategy_source_path.display()
                ),
            )
        })?;
    }

    Ok(manifest_dir.to_path_buf())
}

fn registered_relative_root_components(relative_root: &str) -> io::Result<Vec<&str>> {
    let components: Vec<&str> = relative_root.split('/').collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
        || components.iter().any(|component| component.contains('\\'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("registered primary root is not a supported relative path: {relative_root}"),
        ));
    }
    Ok(components)
}

fn path_has_registered_relative_root_tail(path: &Path, relative_components: &[&str]) -> bool {
    let path_components: Vec<&OsStr> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(component) => Some(component),
            _ => None,
        })
        .collect();
    if path_components.len() < relative_components.len() {
        return false;
    }
    path_components[path_components.len() - relative_components.len()..]
        .iter()
        .zip(relative_components)
        .all(|(actual, expected)| *actual == OsStr::new(expected))
}

/// Lowercase-hex SHA-256 of a panic-gate source root's framed canonical byte
/// stream (identity OR directory). This is the digest recorded in the abort-plan
/// artifact; it must stay over the framed canonical stream so it equals the
/// verifier's compile-time embed.
fn read_abort_plan_panic_gate_service_policy_source_digest(
    source_path: &Path,
    max_source_bytes: u64,
) -> Result<String, BoltV3OperatorArtifactError> {
    canonical_source_digest(source_path, max_source_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceRead {
            path: source_path.to_path_buf(),
            source,
        }
    })
}

/// Per-file UTF-8 module text of a panic-gate source root (identity OR
/// directory), in the same canonical order as the digest, WITHOUT the binary
/// frame bytes. This is what the contract grep runs over.
fn read_abort_plan_panic_gate_service_policy_source_text(
    source_path: &Path,
    max_source_bytes: u64,
) -> Result<String, BoltV3OperatorArtifactError> {
    canonical_module_text(source_path, max_source_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceRead {
            path: source_path.to_path_buf(),
            source,
        }
    })
}

pub fn write_strategy_input_evidence_artifact(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    _path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let _ =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    Err(
        BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
            prerequisite: "T046 remains blocked: missing source-bound price-to-beat strategy decision input",
        },
    )
}

pub fn write_strategy_input_evidence_artifact_from_runtime_snapshot(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    market_selection_source_ref: &WrittenOperatorArtifact,
    max_market_selection_source_bytes: u64,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let runtime_strategy_id = runtime_strategy_id_for_loaded_strategy(loaded, strategy_instance_id)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    validate_strategy_input_readiness_evidence(snapshot).map_err(|_| {
        BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
            prerequisite: "T046 remains blocked: strategy input readiness gate identity is missing",
        }
    })?;
    if snapshot.configured_target_id != financial_envelope.configured_target_id() {
        return Err(
            BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy input target does not match config",
            },
        );
    }
    if snapshot.price_to_beat_source.trim() != financial_envelope.price_to_beat_source() {
        return Err(
            BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy input price-to-beat source does not match config",
            },
        );
    }
    let market_selection_source_bytes = read_file_bounded(
        &market_selection_source_ref.path,
        max_market_selection_source_bytes,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::MarketSelectionSourceRead {
            path: market_selection_source_ref.path.clone(),
            source,
        },
    )?;
    if hex::encode(Sha256::digest(&market_selection_source_bytes))
        != market_selection_source_ref.sha256
    {
        return Err(
            BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: market-selection source hash does not match",
            },
        );
    }
    let market_selection_source: Phase8MarketSelectionSourceEvidenceFile =
        serde_json::from_slice(&market_selection_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::MarketSelectionSourceParse {
                path: market_selection_source_ref.path.clone(),
                source,
            }
        })?;
    let artifact =
        Phase8StrategyInputEvidenceFile::from_runtime_snapshot_and_market_selection_source(
            snapshot,
            financial_envelope.strategy_instance_id(),
            &runtime_strategy_id,
            &market_selection_source,
            market_selection_source_ref.path.to_string_lossy(),
            &market_selection_source_ref.sha256,
            market_selection_source.candidate_market_start_timestamps_ms(),
        )
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    write_json_artifact_create_new(path, &artifact)
}

pub fn write_strategy_input_evidence_artifact_from_decision_evidence_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    decision_evidence_path: &Path,
    max_decision_evidence_bytes: u64,
    market_selection_source_ref: &WrittenOperatorArtifact,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let chain = read_latest_entry_decision_evidence_chain(
        decision_evidence_path,
        max_decision_evidence_bytes,
    )
    .map_err(|_| BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
        prerequisite: "T046 remains blocked: missing complete source-bound strategy decision input",
    })?;
    write_strategy_input_evidence_artifact_from_runtime_snapshot(
        loaded,
        strategy_instance_id,
        &chain.snapshot,
        market_selection_source_ref,
        max_decision_evidence_bytes,
        path,
    )
}

#[derive(Debug, Clone, Copy)]
pub struct EntryDecisionSourceBookSideInput {
    pub best_bid: f64,
    pub bid_quantity: f64,
    pub best_ask: f64,
    pub ask_quantity: f64,
    pub liquidity_available: f64,
}

pub struct CanaryProofArtifactsCollectionRequest<'a> {
    pub price_to_beat_source_path: &'a Path,
    pub max_price_to_beat_source_bytes: u64,
    pub reference_quote_source_path: &'a Path,
    pub max_reference_quote_source_bytes: u64,
    pub signal_quote_source_path: &'a Path,
    pub max_signal_quote_source_bytes: u64,
    pub realized_volatility_source_path: &'a Path,
    pub max_realized_volatility_source_bytes: u64,
    pub gate_session_output_path: &'a Path,
    pub candidate_source_output_path: &'a Path,
    pub order_intent_output_path: &'a Path,
}

pub struct CanaryProofArtifactsWritten {
    pub gate_session: WrittenOperatorArtifact,
    pub candidate_source: WrittenOperatorArtifact,
    pub order_intent: WrittenOperatorArtifact,
}

pub struct OperatorEvidenceJsonBuildInputs<'a> {
    pub max_operator_evidence_file_bytes: u64,
    pub approval_consumption_max_age_seconds: u64,
    pub approval_envelope_path: &'a Path,
    pub ssm_manifest_path: &'a Path,
    pub strategy_input_evidence_path: &'a Path,
    pub gate_session_path: &'a Path,
    pub expected_gate_session_sha256: &'a str,
    pub financial_envelope_path: &'a Path,
    pub pre_run_state_path: &'a Path,
    pub abort_plan_path: &'a Path,
    pub canary_proof_candidate_source_path: Option<&'a Path>,
    pub canary_proof_order_intent_path: Option<&'a Path>,
    pub canary_evidence_path: &'a Path,
    pub approval_not_before_unix_seconds: i64,
    pub approval_not_after_unix_seconds: i64,
    pub approval_nonce_path: &'a Path,
    pub approval_consumption_path: &'a Path,
    pub decision_evidence_path: &'a Path,
    pub nt_submit_event_path: &'a Path,
    pub venue_order_state_path: &'a Path,
    pub strategy_cancel_path: Option<&'a Path>,
    pub restart_reconciliation_path: &'a Path,
    pub post_run_hygiene_path: &'a Path,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceBoundPriceToBeatSource {
    schema_version: u32,
    record_kind: String,
    source: String,
    price_to_beat_value: f64,
    source_report_schema_version: Option<u64>,
    source_report_feed_id: Option<String>,
    source_report_decimal_scale: Option<u64>,
    source_report_full_sha256: Option<String>,
    source_report_valid_from_timestamp_ms: Option<u64>,
    source_report_observations_timestamp_ms: Option<u64>,
    source_report_benchmark_price: Option<f64>,
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReferenceQuoteSource {
    schema_version: u32,
    record_kind: String,
    venue: String,
    price: f64,
    observed_ts_ms: u64,
    source_report_schema_version: Option<u64>,
    source_report_feed_id: Option<String>,
    source_report_decimal_scale: Option<u64>,
    source_report_full_sha256: Option<String>,
    source_report_valid_from_timestamp_ms: Option<u64>,
    source_report_observations_timestamp_ms: Option<u64>,
    source_report_benchmark_price: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignalQuoteSource {
    schema_version: u32,
    record_kind: String,
    venue: String,
    price: f64,
    observed_ts_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RealizedVolatilitySource {
    schema_version: u32,
    record_kind: String,
    value: f64,
    ready_ts_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReferenceQuoteObservationsSource {
    schema_version: u32,
    record_kind: String,
    observations: Vec<ReferenceQuoteObservationSource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReferenceQuoteObservationSource {
    data_client_id: String,
    instrument_id: String,
    bid_price: f64,
    ask_price: f64,
    ts_event_unix_nanos: u64,
    ts_init_unix_nanos: u64,
    captured_at_unix_nanos: u64,
    source_report_schema_version: Option<u64>,
    source_report_feed_id: Option<String>,
    source_report_decimal_scale: Option<u64>,
    source_report_full_sha256: Option<String>,
    source_report_valid_from_timestamp_ms: Option<u64>,
    source_report_observations_timestamp_ms: Option<u64>,
    source_report_benchmark_price: Option<f64>,
}

struct EntryDecisionSourceProofs {
    price_source: SourceBoundPriceToBeatSource,
    reference_quote: ReferenceQuoteSource,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntryDecisionFeeRateSourceArtifact {
    pub(crate) schema_version: u32,
    pub(crate) record_kind: String,
    pub(crate) fee_bps_by_instrument_id: BTreeMap<String, f64>,
}

struct PriceToBeatProviderSelection {
    provider_id: String,
    resolution_identity: String,
    value_kind: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntryDecisionSourceProofFileRequest<'a> {
    pub price_to_beat_source_path: &'a Path,
    pub max_price_to_beat_source_bytes: u64,
    pub reference_quote_source_path: &'a Path,
    pub max_reference_quote_source_bytes: u64,
    pub signal_quote_source_path: &'a Path,
    pub max_signal_quote_source_bytes: u64,
    pub realized_volatility_source_path: &'a Path,
    pub max_realized_volatility_source_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntryDecisionSourceProofValidation {
    pub market_selection_timestamp_ms: u64,
    pub decision_timestamp_ms: u64,
    pub reference_quote_price: f64,
}

pub(crate) fn validate_entry_decision_source_proof_files(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: EntryDecisionSourceProofFileRequest<'_>,
) -> Result<EntryDecisionSourceProofValidation, BoltV3OperatorArtifactError> {
    let proofs =
        read_validated_entry_decision_source_proofs(loaded, strategy_instance_id, request)?;
    Ok(EntryDecisionSourceProofValidation {
        market_selection_timestamp_ms: proofs.price_source.market_selection_timestamp_ms,
        decision_timestamp_ms: proofs.price_source.decision_timestamp_ms,
        reference_quote_price: proofs.reference_quote.price,
    })
}

pub(crate) fn validate_canary_proof_source_files(
    request: EntryDecisionSourceProofFileRequest<'_>,
) -> Result<EntryDecisionSourceProofValidation, BoltV3OperatorArtifactError> {
    let price_source: SourceBoundPriceToBeatSource = read_decision_source_json_file(
        request.price_to_beat_source_path,
        request.max_price_to_beat_source_bytes,
    )?;
    let reference_quote: ReferenceQuoteSource = read_decision_source_json_file(
        request.reference_quote_source_path,
        request.max_reference_quote_source_bytes,
    )?;
    let signal_quote: SignalQuoteSource = read_decision_source_json_file(
        request.signal_quote_source_path,
        request.max_signal_quote_source_bytes,
    )?;
    let realized_volatility: RealizedVolatilitySource = read_decision_source_json_file(
        request.realized_volatility_source_path,
        request.max_realized_volatility_source_bytes,
    )?;
    validate_source_bound_price_to_beat_shape(&price_source)?;
    validate_reference_quote_source(
        &reference_quote,
        price_source.market_selection_timestamp_ms,
        price_source.decision_timestamp_ms,
    )?;
    validate_signal_quote_source(
        &signal_quote,
        price_source.market_selection_timestamp_ms,
        price_source.decision_timestamp_ms,
    )?;
    validate_realized_volatility_source(
        &realized_volatility,
        price_source.market_selection_timestamp_ms,
        price_source.decision_timestamp_ms,
    )?;
    Ok(EntryDecisionSourceProofValidation {
        market_selection_timestamp_ms: price_source.market_selection_timestamp_ms,
        decision_timestamp_ms: price_source.decision_timestamp_ms,
        reference_quote_price: reference_quote.price,
    })
}

pub(crate) fn canary_proof_policy_input_from_loaded(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    current_source_ref: &str,
) -> Result<CanaryProofPolicyInput, BoltV3OperatorArtifactError> {
    let live_canary = loaded.root.live_canary.as_ref().ok_or_else(|| {
        entry_decision_source_invalid("live_canary block is required for canary proof artifacts")
    })?;
    let policy = live_canary.proof_policy.as_ref().ok_or_else(|| {
        entry_decision_source_invalid(
            "live_canary.proof_policy block is required for canary proof artifacts",
        )
    })?;
    if !policy.enabled {
        return Err(entry_decision_source_invalid(
            "live_canary.proof_policy must be enabled for canary proof artifacts",
        ));
    }
    if policy.strategy_instance_id != strategy_instance_id {
        return Err(entry_decision_source_invalid(
            "live_canary.proof_policy.strategy_instance_id does not match requested strategy",
        ));
    }
    Ok(CanaryProofPolicyInput {
        strategy_instance_id: policy.strategy_instance_id.clone(),
        execution_client_id: policy.execution_client_id.clone(),
        proof_claim: policy.proof_claim.clone(),
        proof_notional: canary_proof_decimal_from_str(
            policy.proof_notional.as_str(),
            "canary proof notional",
        )?,
        max_notional_per_order: canary_proof_decimal_from_str(
            live_canary.max_notional_per_order.as_str(),
            "canary proof max notional per order",
        )?,
        allow_negative_expected_ev: policy.allow_negative_expected_ev,
        source_ready: true,
        current_source_ref: current_source_ref.to_string(),
        candidates: Vec::new(),
    })
}

fn canary_proof_decimal_from_str(
    value: &str,
    field: &'static str,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    Decimal::from_str(value.trim()).map_err(|source| {
        entry_decision_source_invalid(format!("{field} is not decimal: {source}"))
    })
}

pub(crate) fn build_entry_readiness_gate_session_from_source_proof_files(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    selected: &bolt_v3_market_families::SelectedBinaryOptionMarket,
    request: EntryDecisionSourceProofFileRequest<'_>,
) -> Result<EntryReadinessGateSession, BoltV3OperatorArtifactError> {
    let proofs =
        read_validated_entry_decision_source_proofs(loaded, strategy_instance_id, request)?;
    readiness_session_from_entry_decision_price_source(
        loaded,
        strategy_instance_id,
        selected,
        &proofs.price_source,
        &proofs.reference_quote,
    )
}

pub fn write_reference_quote_observations_source_from_no_submit_evidence(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    if strategy.config.reference_data.is_empty() {
        return Err(entry_decision_source_invalid(
            "reference quote observations source requires configured reference_data",
        ));
    }
    let observations = evidence
        .quotes
        .iter()
        .filter(|quote| {
            strategy.config.reference_data.values().any(|reference| {
                quote.data_client_id == reference.data_client_id.to_string()
                    && quote.instrument_id == reference.instrument_id.to_string()
            })
        })
        .map(|quote| ReferenceQuoteObservationSource {
            data_client_id: quote.data_client_id.clone(),
            instrument_id: quote.instrument_id.clone(),
            bid_price: quote.bid_price,
            ask_price: quote.ask_price,
            ts_event_unix_nanos: quote.ts_event_unix_nanos,
            ts_init_unix_nanos: quote.ts_init_unix_nanos,
            captured_at_unix_nanos: quote.captured_at_unix_nanos,
            source_report_schema_version: None,
            source_report_feed_id: None,
            source_report_decimal_scale: None,
            source_report_full_sha256: None,
            source_report_valid_from_timestamp_ms: None,
            source_report_observations_timestamp_ms: None,
            source_report_benchmark_price: None,
        })
        .collect::<Vec<_>>();
    let source = ReferenceQuoteObservationsSource {
        schema_version: REFERENCE_QUOTE_OBSERVATIONS_SOURCE_SCHEMA_VERSION,
        record_kind: REFERENCE_QUOTE_OBSERVATIONS_SOURCE_RECORD_KIND.to_string(),
        observations,
    };
    validate_reference_quote_observations_source(&source)?;
    write_json_artifact_create_new(output_path, &source)
}

pub async fn collect_canary_proof_artifacts_from_configured_provider(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: CanaryProofArtifactsCollectionRequest<'_>,
) -> Result<CanaryProofArtifactsWritten, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let client_key = strategy.config.execution_client_id.as_str();
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("execution client `{client_key}` is not loaded"),
        }
    })?;
    let binding = binding_for_provider_key(client.venue.as_str()).ok_or_else(|| {
        BoltV3OperatorArtifactError::UnsupportedProvider {
            client_key: client_key.to_string(),
            provider_key: client.venue.to_string(),
        }
    })?;
    let collector = binding.collect_canary_proof_artifacts.ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: "configured provider does not expose canary proof artifact collection"
                .to_string(),
        }
    })?;
    collector(
        crate::bolt_v3_providers::CanaryProofArtifactsProviderContext {
            loaded,
            strategy_instance_id,
            request,
        },
    )
    .await
}

fn read_validated_entry_decision_source_proofs(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    request: EntryDecisionSourceProofFileRequest<'_>,
) -> Result<EntryDecisionSourceProofs, BoltV3OperatorArtifactError> {
    let price_source: SourceBoundPriceToBeatSource = read_decision_source_json_file(
        request.price_to_beat_source_path,
        request.max_price_to_beat_source_bytes,
    )?;
    let reference_quote: ReferenceQuoteSource = read_decision_source_json_file(
        request.reference_quote_source_path,
        request.max_reference_quote_source_bytes,
    )?;
    let signal_quote: SignalQuoteSource = read_decision_source_json_file(
        request.signal_quote_source_path,
        request.max_signal_quote_source_bytes,
    )?;
    let realized_volatility: RealizedVolatilitySource = read_decision_source_json_file(
        request.realized_volatility_source_path,
        request.max_realized_volatility_source_bytes,
    )?;
    validate_price_to_beat_source(loaded, strategy_instance_id, &price_source)?;
    validate_reference_quote_source(
        &reference_quote,
        price_source.market_selection_timestamp_ms,
        price_source.decision_timestamp_ms,
    )?;
    validate_signal_quote_source(
        &signal_quote,
        price_source.market_selection_timestamp_ms,
        price_source.decision_timestamp_ms,
    )?;
    validate_realized_volatility_source(
        &realized_volatility,
        price_source.market_selection_timestamp_ms,
        price_source.decision_timestamp_ms,
    )?;
    Ok(EntryDecisionSourceProofs {
        price_source,
        reference_quote,
    })
}

fn read_decision_source_json_file<T: DeserializeOwned>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceParse {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn validate_price_to_beat_source(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    source: &SourceBoundPriceToBeatSource,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_source_bound_price_to_beat_shape(source)?;
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    if source.source != financial_envelope.price_to_beat_source() {
        return Err(entry_decision_source_invalid(
            "source-bound price_to_beat source does not match the approved strategy source",
        ));
    }
    validate_price_to_beat_report_provenance(
        loaded,
        strategy_instance_id,
        source,
        source.market_selection_timestamp_ms,
        source.decision_timestamp_ms,
    )?;
    Ok(())
}

fn validate_source_bound_price_to_beat_shape(
    source: &SourceBoundPriceToBeatSource,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != SOURCE_BOUND_PRICE_TO_BEAT_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "source-bound price_to_beat schema_version is invalid",
        ));
    }
    if source.record_kind != SOURCE_BOUND_PRICE_TO_BEAT_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "source-bound price_to_beat record_kind is invalid",
        ));
    }
    if !source.price_to_beat_value.is_finite()
        || source.price_to_beat_value <= ENTRY_DECISION_ZERO_THRESHOLD
    {
        return Err(entry_decision_source_invalid(
            "source-bound price_to_beat value is invalid",
        ));
    }
    if source.decision_timestamp_ms < source.market_selection_timestamp_ms {
        return Err(entry_decision_source_invalid(
            "source-bound price_to_beat decision timestamp precedes market selection",
        ));
    }
    Ok(())
}

fn validate_price_to_beat_report_provenance(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    source: &SourceBoundPriceToBeatSource,
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
) -> Result<(), BoltV3OperatorArtifactError> {
    let binding = price_to_beat_report_binding(loaded, strategy_instance_id)?;
    let Some(report_schema_version) = source.source_report_schema_version else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let Some(report_feed_id) = source
        .source_report_feed_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let Some(report_decimal_scale) = source.source_report_decimal_scale else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let Some(report_sha256) = source
        .source_report_full_sha256
        .as_deref()
        .filter(|value| is_lowercase_sha256(value))
    else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let Some(valid_from_timestamp_ms) = source.source_report_valid_from_timestamp_ms else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let Some(observations_timestamp_ms) = source.source_report_observations_timestamp_ms else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let Some(benchmark_price) = source.source_report_benchmark_price else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    if report_schema_version != binding.schema_version
        || report_feed_id != binding.feed_id
        || report_decimal_scale != binding.decimal_scale
        || !benchmark_price.is_finite()
        || (benchmark_price - source.price_to_beat_value).abs() > f64::EPSILON
        || valid_from_timestamp_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || observations_timestamp_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || valid_from_timestamp_ms != market_selection_timestamp_ms
        || valid_from_timestamp_ms > observations_timestamp_ms
        || observations_timestamp_ms < market_selection_timestamp_ms
        || observations_timestamp_ms > decision_timestamp_ms
        || report_sha256.is_empty()
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    Ok(())
}

fn price_to_beat_report_binding(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> Result<PriceToBeatReportBinding, BoltV3OperatorArtifactError> {
    chainlink_report_binding_for_role(loaded, strategy_instance_id, RESOLUTION_GATE_ROLE)
}

fn chainlink_report_binding_for_role(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    gate_role: &str,
) -> Result<PriceToBeatReportBinding, BoltV3OperatorArtifactError> {
    let selection = chainlink_provider_selection_for_role(loaded, strategy_instance_id, gate_role)?;
    let provider = loaded
        .root
        .gate_providers
        .as_ref()
        .and_then(|providers| providers.get(&selection.provider_id))
        .ok_or_else(price_to_beat_report_provenance_config_invalid)?;
    if provider.provider_kind.as_deref() != Some(CHAINLINK_DATA_STREAMS_PROVIDER_KIND) {
        return Err(price_to_beat_report_provenance_config_invalid());
    }
    let provider_config = provider
        .provider_config
        .get(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
        .and_then(toml::Value::as_table)
        .ok_or_else(price_to_beat_report_provenance_config_invalid)?;
    let feed_binding = chainlink_data_streams_feed_binding(
        provider_config,
        &selection.resolution_identity,
        &selection.value_kind,
    )?;
    let feed_id = string_provider_field(
        feed_binding,
        CHAINLINK_DATA_STREAMS_FEED_ID_FIELD,
        price_to_beat_report_provenance_config_invalid,
    )?;
    let schema_version = positive_u64_provider_field(
        feed_binding,
        CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD,
    )?;
    let decimal_scale = positive_u64_provider_field(
        feed_binding,
        CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD,
    )?;
    if !is_lowercase_chainlink_feed_id(&feed_id) {
        return Err(price_to_beat_report_provenance_config_invalid());
    }
    Ok(PriceToBeatReportBinding {
        provider_id: selection.provider_id,
        feed_id,
        schema_version,
        decimal_scale,
    })
}

fn chainlink_provider_selection_for_role(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    gate_role: &str,
) -> Result<PriceToBeatProviderSelection, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let target = strategy
        .config
        .target
        .as_table()
        .ok_or_else(price_to_beat_report_provenance_config_invalid)?;
    let Some(gate_subscription) = target
        .get("gate_subscriptions")
        .and_then(toml::Value::as_table)
        .and_then(|subscriptions| subscriptions.get(gate_role))
        .and_then(toml::Value::as_table)
    else {
        return Err(price_to_beat_report_provenance_config_invalid());
    };
    let mapping = gate_subscription
        .get("market_mappings")
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(toml::Value::as_table)
        .find(|mapping| {
            mapping.get("resolution_kind").and_then(toml::Value::as_str)
                == Some(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
                && mapping.get("value_kind").and_then(toml::Value::as_str)
                    == Some(PRICE_GATE_VALUE_KIND)
        })
        .ok_or_else(price_to_beat_report_provenance_config_invalid)?;
    let resolution_identity = string_provider_field(
        mapping,
        CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD,
        price_to_beat_report_provenance_config_invalid,
    )?;
    let value_kind = string_provider_field(
        mapping,
        CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD,
        price_to_beat_report_provenance_config_invalid,
    )?;
    let provider_id = mapping
        .get("provider_id")
        .and_then(toml::Value::as_str)
        .or_else(|| {
            gate_subscription
                .get("provider_preference")
                .and_then(toml::Value::as_array)
                .and_then(|provider_ids| provider_ids.first())
                .and_then(toml::Value::as_str)
        })
        .or_else(|| {
            let provider_ids = gate_subscription
                .get("allowed_provider_ids")
                .and_then(toml::Value::as_array)?;
            (provider_ids.len() == 1)
                .then(|| provider_ids[0].as_str())
                .flatten()
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(price_to_beat_report_provenance_config_invalid)?;
    Ok(PriceToBeatProviderSelection {
        provider_id,
        resolution_identity,
        value_kind,
    })
}

fn chainlink_data_streams_feed_binding<'a>(
    provider_config: &'a toml::map::Map<String, toml::Value>,
    resolution_identity: &str,
    value_kind: &str,
) -> Result<&'a toml::map::Map<String, toml::Value>, BoltV3OperatorArtifactError> {
    let feed_bindings = provider_config
        .get(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD)
        .and_then(toml::Value::as_array)
        .ok_or_else(price_to_beat_report_provenance_config_invalid)?;
    let mut matches = feed_bindings
        .iter()
        .filter_map(toml::Value::as_table)
        .filter(|binding| {
            binding
                .get(CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD)
                .and_then(toml::Value::as_str)
                .map(str::trim)
                == Some(resolution_identity)
                && binding
                    .get(CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD)
                    .and_then(toml::Value::as_str)
                    .map(str::trim)
                    == Some(value_kind)
        });
    let Some(first) = matches.next() else {
        return Err(price_to_beat_report_provenance_config_invalid());
    };
    if matches.next().is_some() {
        return Err(price_to_beat_report_provenance_config_invalid());
    }
    Ok(first)
}

fn readiness_session_from_entry_decision_price_source(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    selected: &bolt_v3_market_families::SelectedBinaryOptionMarket,
    source: &SourceBoundPriceToBeatSource,
    reference_quote: &ReferenceQuoteSource,
) -> Result<EntryReadinessGateSession, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let selected_market = bolt_v3_market_families::selected_market_requirement_from_target(
        &strategy.config.target,
        selected,
        source.market_selection_timestamp_ms,
    )
    .map_err(|error| BoltV3OperatorArtifactError::MarketSelection(anyhow!(error)))?;
    let binding = price_to_beat_report_binding(loaded, strategy_instance_id)?;
    let provider = gate_provider_evidence_binding(loaded, &binding.provider_id)?;
    let Some(report_sha256) = source
        .source_report_full_sha256
        .as_deref()
        .filter(|value| is_lowercase_sha256(value))
    else {
        return Err(price_to_beat_report_provenance_invalid());
    };
    let report_schema_version = source
        .source_report_schema_version
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let report_decimal_scale = source
        .source_report_decimal_scale
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let report_feed_id = source
        .source_report_feed_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let valid_from_timestamp_ms = source
        .source_report_valid_from_timestamp_ms
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let observations_timestamp_ms = source
        .source_report_observations_timestamp_ms
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let artifact_ref = GateArtifactRef {
        path: ENTRY_READINESS_CHAINLINK_REPORT_ARTIFACT_PATH.to_string(),
        sha256: report_sha256.to_string(),
    };
    let provider_id = binding.provider_id.clone();
    let provider_provenance = serde_json::json!({
        "provider_kind": CHAINLINK_DATA_STREAMS_PROVIDER_KIND,
        "feed_id": report_feed_id,
        "report_schema_version": report_schema_version,
        "report_decimal_scale": report_decimal_scale,
        "source_report_full_sha256": report_sha256,
        "valid_from_timestamp_ms": valid_from_timestamp_ms,
        "observations_timestamp_ms": observations_timestamp_ms,
    });
    let evidence = normalize_gate_evidence(GateEvidenceInput {
        role: RESOLUTION_GATE_ROLE.to_string(),
        provider_id: provider_id.clone(),
        provider_kind: CHAINLINK_DATA_STREAMS_PROVIDER_KIND.to_string(),
        selected_market_key: selected_market.selected_market_key.clone(),
        collector_observed_at_ms: observations_timestamp_ms,
        source_observed_at_ms: observations_timestamp_ms,
        freshness_max_age_ms: provider.max_age_ms,
        value_kind: PRICE_GATE_VALUE_KIND.to_string(),
        normalized_value: serde_json::json!({
            "price_to_beat_value": source.price_to_beat_value,
        }),
        provider_provenance: provider_provenance.clone(),
        artifact_refs: vec![artifact_ref.clone()],
        collection_status: GateEvidenceCollectionStatus::Complete,
    })?;
    let mut requirements = crate::bolt_v3_archetypes::binary_oracle_edge_taker::gate_requirements();
    let mut provider_evidence = vec![evidence];
    let mut session_artifact_refs = vec![artifact_ref.clone()];
    if strategy_has_decision_reference_subscription(strategy) {
        requirements.push(ArchetypeGateRequirement {
            role: GateRole::DecisionReference,
            required: true,
            accepted_value_kinds: BTreeSet::from([GateValueKind::Price, GateValueKind::Outcome]),
            allow_no_resolution: false,
        });
        let reference_binding = chainlink_report_binding_for_role(
            loaded,
            strategy_instance_id,
            DECISION_REFERENCE_GATE_ROLE,
        )?;
        let Some(reference_report_sha256) = reference_quote
            .source_report_full_sha256
            .as_deref()
            .filter(|value| is_lowercase_sha256(value))
        else {
            return Err(decision_reference_report_provenance_invalid());
        };
        let reference_report_schema_version = reference_quote
            .source_report_schema_version
            .ok_or_else(decision_reference_report_provenance_invalid)?;
        let reference_report_decimal_scale = reference_quote
            .source_report_decimal_scale
            .ok_or_else(decision_reference_report_provenance_invalid)?;
        let reference_report_feed_id = reference_quote
            .source_report_feed_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(decision_reference_report_provenance_invalid)?;
        let reference_valid_from_timestamp_ms = reference_quote
            .source_report_valid_from_timestamp_ms
            .ok_or_else(decision_reference_report_provenance_invalid)?;
        let reference_observations_timestamp_ms = reference_quote
            .source_report_observations_timestamp_ms
            .ok_or_else(decision_reference_report_provenance_invalid)?;
        let reference_benchmark_price = reference_quote
            .source_report_benchmark_price
            .ok_or_else(decision_reference_report_provenance_invalid)?;
        if reference_quote.venue != reference_binding.provider_id
            || reference_report_schema_version != reference_binding.schema_version
            || reference_report_feed_id != reference_binding.feed_id
            || reference_report_decimal_scale != reference_binding.decimal_scale
            || reference_valid_from_timestamp_ms > reference_observations_timestamp_ms
            || reference_observations_timestamp_ms != reference_quote.observed_ts_ms
            || (reference_benchmark_price - reference_quote.price).abs() > f64::EPSILON
        {
            return Err(decision_reference_report_provenance_invalid());
        }
        let reference_provider =
            gate_provider_evidence_binding(loaded, &reference_binding.provider_id)?;
        let reference_artifact_ref = GateArtifactRef {
            path: ENTRY_READINESS_REFERENCE_REPORT_ARTIFACT_PATH.to_string(),
            sha256: reference_report_sha256.to_string(),
        };
        let reference_provider_provenance = serde_json::json!({
            "provider_kind": CHAINLINK_DATA_STREAMS_PROVIDER_KIND,
            "feed_id": reference_report_feed_id,
            "report_schema_version": reference_report_schema_version,
            "report_decimal_scale": reference_report_decimal_scale,
            "source_report_full_sha256": reference_report_sha256,
            "valid_from_timestamp_ms": reference_valid_from_timestamp_ms,
            "observations_timestamp_ms": reference_observations_timestamp_ms,
        });
        provider_evidence.push(normalize_gate_evidence(GateEvidenceInput {
            role: DECISION_REFERENCE_GATE_ROLE.to_string(),
            provider_id: reference_binding.provider_id,
            provider_kind: CHAINLINK_DATA_STREAMS_PROVIDER_KIND.to_string(),
            selected_market_key: selected_market.selected_market_key.clone(),
            collector_observed_at_ms: reference_observations_timestamp_ms,
            source_observed_at_ms: reference_observations_timestamp_ms,
            freshness_max_age_ms: reference_provider.max_age_ms,
            value_kind: PRICE_GATE_VALUE_KIND.to_string(),
            normalized_value: serde_json::json!({
                "reference_value": reference_quote.price,
            }),
            provider_provenance: reference_provider_provenance,
            artifact_refs: vec![reference_artifact_ref.clone()],
            collection_status: GateEvidenceCollectionStatus::Complete,
        })?);
        session_artifact_refs.push(reference_artifact_ref);
    }
    build_entry_readiness_gate_session(EntryReadinessGateSessionRequest {
        loaded,
        strategy_instance_id,
        selected_market: &selected_market,
        requirements: &requirements,
        provider_evidence: &provider_evidence,
        created_at_ms: source.decision_timestamp_ms,
        artifact_refs: session_artifact_refs,
    })
}

fn strategy_has_decision_reference_subscription(
    strategy: &crate::bolt_v3_config::LoadedStrategy,
) -> bool {
    strategy
        .config
        .target
        .as_table()
        .and_then(|target| target.get("gate_subscriptions"))
        .and_then(toml::Value::as_table)
        .is_some_and(|subscriptions| subscriptions.contains_key(DECISION_REFERENCE_GATE_ROLE))
}

pub(crate) fn price_to_beat_report_provenance_invalid() -> BoltV3OperatorArtifactError {
    entry_decision_source_invalid("source-bound price_to_beat report provenance is invalid")
}

fn decision_reference_report_provenance_invalid() -> BoltV3OperatorArtifactError {
    entry_decision_source_invalid("source-bound decision_reference report provenance is invalid")
}

pub(crate) fn price_to_beat_report_provenance_config_invalid() -> BoltV3OperatorArtifactError {
    entry_decision_source_invalid("source-bound price_to_beat report provenance config is invalid")
}

fn positive_u64_provider_field(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<u64, BoltV3OperatorArtifactError> {
    table
        .get(field)
        .and_then(toml::Value::as_integer)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value != ENTRY_DECISION_ZERO_TIMESTAMP_MS)
        .ok_or_else(price_to_beat_report_provenance_config_invalid)
}

fn string_provider_field(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
    error: fn() -> BoltV3OperatorArtifactError,
) -> Result<String, BoltV3OperatorArtifactError> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(error)
}

fn validate_reference_quote_source(
    source: &ReferenceQuoteSource,
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != REFERENCE_QUOTE_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "reference quote source schema_version is invalid",
        ));
    }
    if source.record_kind != REFERENCE_QUOTE_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "reference quote source record_kind is invalid",
        ));
    }
    if source.venue.trim().is_empty() || source.venue.trim() != source.venue {
        return Err(entry_decision_source_invalid(
            "reference quote source venue is invalid",
        ));
    }
    if !source.price.is_finite() || source.price <= ENTRY_DECISION_ZERO_THRESHOLD {
        return Err(entry_decision_source_invalid(
            "reference quote source price is invalid",
        ));
    }
    if source.observed_ts_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || source.observed_ts_ms < market_selection_timestamp_ms
        || source.observed_ts_ms > decision_timestamp_ms
    {
        return Err(entry_decision_source_invalid(
            "reference quote source timestamp is invalid",
        ));
    }
    Ok(())
}

fn validate_signal_quote_source(
    source: &SignalQuoteSource,
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != SIGNAL_QUOTE_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "signal quote source schema_version is invalid",
        ));
    }
    if source.record_kind != SIGNAL_QUOTE_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "signal quote source record_kind is invalid",
        ));
    }
    if source.venue.trim().is_empty() || source.venue.trim() != source.venue {
        return Err(entry_decision_source_invalid(
            "signal quote source venue is invalid",
        ));
    }
    if !source.price.is_finite() || source.price <= ENTRY_DECISION_ZERO_THRESHOLD {
        return Err(entry_decision_source_invalid(
            "signal quote source price is invalid",
        ));
    }
    if source.observed_ts_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || source.observed_ts_ms < market_selection_timestamp_ms
        || source.observed_ts_ms > decision_timestamp_ms
    {
        return Err(entry_decision_source_invalid(
            "signal quote source timestamp is invalid",
        ));
    }
    Ok(())
}

fn validate_reference_quote_observations_source(
    source: &ReferenceQuoteObservationsSource,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != REFERENCE_QUOTE_OBSERVATIONS_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "reference quote observations source schema_version is invalid",
        ));
    }
    if source.record_kind != REFERENCE_QUOTE_OBSERVATIONS_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "reference quote observations source record_kind is invalid",
        ));
    }
    if source.observations.is_empty() {
        return Err(entry_decision_source_invalid(
            "reference quote observations source requires observations",
        ));
    }
    for observation in &source.observations {
        if observation.data_client_id.trim().is_empty()
            || observation.data_client_id.trim() != observation.data_client_id
            || observation.instrument_id.trim().is_empty()
            || observation.instrument_id.trim() != observation.instrument_id
            || !observation.bid_price.is_finite()
            || observation.bid_price <= ENTRY_DECISION_ZERO_THRESHOLD
            || !observation.ask_price.is_finite()
            || observation.ask_price <= ENTRY_DECISION_ZERO_THRESHOLD
            || observation.ts_event_unix_nanos == ENTRY_DECISION_ZERO_TIMESTAMP_MS
            || observation.ts_init_unix_nanos == ENTRY_DECISION_ZERO_TIMESTAMP_MS
            || observation.captured_at_unix_nanos == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        {
            return Err(entry_decision_source_invalid(
                "reference quote observations source observation is invalid",
            ));
        }
    }
    Ok(())
}

fn validate_realized_volatility_source(
    source: &RealizedVolatilitySource,
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != REALIZED_VOLATILITY_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "realized volatility source schema_version is invalid",
        ));
    }
    if source.record_kind != REALIZED_VOLATILITY_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "realized volatility source record_kind is invalid",
        ));
    }
    if !source.value.is_finite() || source.value <= ENTRY_DECISION_ZERO_THRESHOLD {
        return Err(entry_decision_source_invalid(
            "realized volatility source value is invalid",
        ));
    }
    if source.ready_ts_ms == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || source.ready_ts_ms < market_selection_timestamp_ms
        || source.ready_ts_ms > decision_timestamp_ms
    {
        return Err(entry_decision_source_invalid(
            "realized volatility source timestamp is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn validate_entry_decision_fee_rate_source_artifact(
    source: &EntryDecisionFeeRateSourceArtifact,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != ENTRY_DECISION_FEE_RATE_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "entry decision fee source schema_version is invalid",
        ));
    }
    if source.record_kind != ENTRY_DECISION_FEE_RATE_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "entry decision fee source record_kind is invalid",
        ));
    }
    if source.fee_bps_by_instrument_id.is_empty() {
        return Err(entry_decision_source_invalid(
            "entry decision fee source requires instrument fee entries",
        ));
    }
    for (instrument_id, fee_bps) in &source.fee_bps_by_instrument_id {
        if instrument_id.trim().is_empty()
            || instrument_id.trim() != instrument_id
            || !fee_bps.is_finite()
            || *fee_bps < ENTRY_DECISION_ZERO_THRESHOLD
        {
            return Err(entry_decision_source_invalid(
                "entry decision fee source entry is invalid",
            ));
        }
    }
    Ok(())
}

pub fn selected_entry_decision_market_attempts(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
    max_attempts: u32,
) -> Result<Vec<bolt_v3_market_families::SelectedBinaryOptionMarket>, BoltV3OperatorArtifactError> {
    if max_attempts == 0 {
        return Err(BoltV3OperatorArtifactError::MarketSelection(anyhow!(
            "entry decision source rotation attempts must be positive"
        )));
    }
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let target =
        bolt_v3_market_families::target_runtime_fields_from_target(&strategy.config.target)
            .map_err(|error| BoltV3OperatorArtifactError::MarketSelection(anyhow!(error)))?;
    let cadence_seconds = u64::try_from(target.cadence_seconds).map_err(|source| {
        BoltV3OperatorArtifactError::MarketSelection(anyhow!(
            "entry decision source cadence_seconds is invalid: {source}"
        ))
    })?;
    let cadence_milliseconds = u64::try_from(Duration::from_secs(cadence_seconds).as_millis())
        .map_err(|source| {
            BoltV3OperatorArtifactError::MarketSelection(anyhow!(
                "entry decision source cadence_milliseconds is invalid: {source}"
            ))
        })?;
    let selection_target = MarketSelectionTarget {
        family_key: &target.rotating_market_family,
        underlying_asset: &target.underlying_asset,
        cadence_seconds: target.cadence_seconds,
        cadence_slug_token: &target.cadence_slug_token,
    };
    let mut attempts = Vec::new();
    let mut seen_market_slugs = BTreeSet::new();
    for attempt_index in 0..max_attempts {
        let offset_milliseconds = cadence_milliseconds
            .checked_mul(u64::from(attempt_index))
            .ok_or_else(|| {
                BoltV3OperatorArtifactError::MarketSelection(anyhow!(
                    "entry decision source rotation attempt offset overflows"
                ))
            })?;
        let attempt_now_milliseconds = now_milliseconds
            .checked_add(offset_milliseconds)
            .ok_or_else(|| {
                BoltV3OperatorArtifactError::MarketSelection(anyhow!(
                    "entry decision source rotation attempt timestamp overflows"
                ))
            })?;
        if let Some(selected) = bolt_v3_market_families::select_binary_option_market_from_target(
            selection_target,
            instruments,
            attempt_now_milliseconds,
        ) && seen_market_slugs.insert(selected.source_identity.market_slug.clone())
        {
            attempts.push(selected);
        }
    }
    if attempts.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "entry decision source requires a selectable two-sided configured market",
            },
        );
    }
    Ok(attempts)
}

pub(crate) fn entry_decision_source_invalid(
    message: impl Into<String>,
) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
        message: message.into(),
    }
}

pub fn write_entry_decision_evidence_from_source_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    decision_source_path: &Path,
    max_decision_source_bytes: u64,
    instrument_source_path: &Path,
    max_instrument_source_bytes: u64,
    max_decision_evidence_bytes: u64,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let decision_source_bytes = read_file_bounded(decision_source_path, max_decision_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceRead {
                path: decision_source_path.to_path_buf(),
                source,
            },
        )?;
    let decision_source: BinaryOracleEntryDecisionEvidenceSource =
        serde_json::from_slice(&decision_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DecisionEvidenceSourceParse {
                path: decision_source_path.to_path_buf(),
                source,
            }
        })?;
    let instrument_source_bytes =
        read_file_bounded(instrument_source_path, max_instrument_source_bytes).map_err(
            |source| BoltV3OperatorArtifactError::MarketSelectionInstrumentSourceRead {
                path: instrument_source_path.to_path_buf(),
                source,
            },
        )?;
    let instruments: Vec<InstrumentAny> = serde_json::from_slice(&instrument_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::MarketSelectionInstrumentSourceParse {
                path: instrument_source_path.to_path_buf(),
                source,
            },
        )?;
    if instruments.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionInstrumentSourceInvalid {
                field: "instruments",
            },
        );
    }
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let raw = raw_taker_config(strategy, loaded).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: source.to_string(),
        }
    })?;
    let writer = Arc::new(
        JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded).map_err(|source| {
            BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("{source:#}"),
            }
        })?,
    );
    record_entry_decision_evidence_from_source(
        &raw,
        writer,
        loaded.root.trader_id,
        &decision_source,
        &instruments,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("{source:#}"),
        },
    )?;
    let path = decision_evidence_path(loaded).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("{source:#}"),
        }
    })?;
    let bytes = read_file_bounded(&path, max_decision_evidence_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceFileRead {
            path: path.clone(),
            source,
        }
    })?;
    Ok(WrittenOperatorArtifact {
        path,
        sha256: hex::encode(Sha256::digest(&bytes)),
    })
}

pub fn write_entry_readiness_gate_session_artifact_from_decision_source_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    decision_source_path: &Path,
    max_decision_source_bytes: u64,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let decision_source_bytes = read_file_bounded(decision_source_path, max_decision_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceRead {
                path: decision_source_path.to_path_buf(),
                source,
            },
        )?;
    let decision_source: BinaryOracleEntryDecisionEvidenceSource =
        serde_json::from_slice(&decision_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::DecisionEvidenceSourceParse {
                path: decision_source_path.to_path_buf(),
                source,
            }
        })?;
    if decision_source.schema_version != ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(GATE_FIELD_SCHEMA_VERSION));
    }
    if decision_source.record_kind != ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(GATE_FIELD_RECORD_KIND));
    }
    let session = &decision_source.readiness_session;
    if session.schema_version != ENTRY_READINESS_GATE_SESSION_SCHEMA_VERSION {
        return Err(entry_readiness_error(GATE_FIELD_SCHEMA_VERSION));
    }
    if session.record_kind != ENTRY_READINESS_GATE_SESSION_RECORD_KIND {
        return Err(entry_readiness_error(GATE_FIELD_RECORD_KIND));
    }
    if session.strategy_instance_id != strategy_instance_id {
        return Err(entry_readiness_error(
            "strategy_instance_id does not match requested strategy",
        ));
    }
    if session.created_at_ms != decision_source.decision_timestamp_ms {
        return Err(entry_readiness_error(
            "created_at_ms does not match source decision_timestamp_ms",
        ));
    }
    let expected_session_hash =
        entry_readiness_session_hash(loaded, session).map_err(entry_readiness_error)?;
    if session.session_hash != expected_session_hash {
        return Err(entry_readiness_error("session_hash does not match session"));
    }
    let snapshot = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(session);
    validate_readiness_gate_evidence_snapshot(&snapshot)
        .map_err(|error| entry_readiness_error(error.to_string()))?;
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(|| entry_readiness_error("strategy_instance_id is not loaded"))?;
    let target = strategy
        .config
        .target
        .as_table()
        .ok_or_else(|| entry_readiness_error("strategy target must be a table"))?;
    let configured_target_id = target
        .get(GATE_FIELD_CONFIGURED_TARGET_ID)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| entry_readiness_error("target.configured_target_id is missing"))?;
    if configured_target_id != session.configured_target_id
        || configured_target_id != session.selected_market.configured_target_id
    {
        return Err(entry_readiness_error(
            "configured_target_id does not match loaded strategy target",
        ));
    }
    write_json_artifact_create_new(path, session)
}

pub fn write_pre_run_state_artifact(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    _path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let _ =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    Err(
        BoltV3OperatorArtifactError::PreRunStatePrerequisiteUnproven {
            prerequisite: "T121 remains blocked: T046 source-bound pre-run state evidence is unproven",
        },
    )
}

pub fn write_pre_run_state_artifact_from_source_proofs(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    proofs: Phase8PreRunStateSourceProofs<'_>,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let artifact = Phase8PreRunStateEvidenceFile::from_financial_envelope_and_source_proofs(
        &financial_envelope,
        proofs,
    )
    .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    write_json_artifact_create_new(path, &artifact)
}

#[derive(Debug, Clone, Copy)]
pub struct PreRunStateSourceCollectorInputs<'a> {
    pub cargo_toml_path: &'a Path,
    pub cargo_lock_path: &'a Path,
    pub clob_signing_source_path: &'a Path,
    pub host_clock_source_path: &'a Path,
    pub venue_account_state_source_path: &'a Path,
    pub funding_margin_source_path: &'a Path,
    pub strategy_input_evidence_path: &'a Path,
    pub strategy_input_evidence_sha256: &'a str,
    pub single_runner_lock_path: &'a Path,
    pub egress_identity_source_path: &'a Path,
    pub clob_v2_adapter_signing_source_path: &'a Path,
    pub clob_v2_collateral_accounting_source_path: &'a Path,
    pub clob_v2_fee_behavior_source_path: &'a Path,
    pub max_source_bytes: u64,
    pub max_host_clock_skew_millis: u64,
    pub max_single_runner_lock_bytes: u64,
}

pub fn write_pre_run_state_artifact_from_source_collectors(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    inputs: PreRunStateSourceCollectorInputs<'_>,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let release_manifest = collect_pre_run_release_manifest_source_proof(
        inputs.cargo_toml_path,
        inputs.cargo_lock_path,
        inputs.clob_signing_source_path,
        inputs.max_source_bytes,
    )?;
    let host_clock = collect_pre_run_host_clock_source_proof(
        inputs.host_clock_source_path,
        inputs.max_source_bytes,
        inputs.max_host_clock_skew_millis,
    )?;
    let venue_account = collect_pre_run_venue_account_state_source_proof(
        inputs.venue_account_state_source_path,
        inputs.max_source_bytes,
        financial_envelope.execution_client_id(),
        financial_envelope.configured_target_id(),
    )?;
    let market_window = collect_pre_run_market_window_source_proof(
        inputs.strategy_input_evidence_path,
        inputs.strategy_input_evidence_sha256,
        financial_envelope.price_to_beat_source(),
        inputs.max_source_bytes,
    )?;
    let funding_margin = collect_pre_run_funding_margin_source_proof(
        inputs.funding_margin_source_path,
        inputs.max_source_bytes,
    )?;
    let egress_identity = collect_pre_run_egress_identity_source_proof(
        inputs.egress_identity_source_path,
        inputs.max_source_bytes,
    )?;
    let clob_signing = collect_pre_run_clob_v2_adapter_signing_source_proof(
        inputs.clob_v2_adapter_signing_source_path,
        inputs.max_source_bytes,
        &release_manifest.clob_signing_version,
    )?;
    let clob_collateral = collect_pre_run_clob_v2_collateral_accounting_source_proof(
        inputs.clob_v2_collateral_accounting_source_path,
        inputs.max_source_bytes,
    )?;
    let clob_fee = collect_pre_run_clob_v2_fee_behavior_source_proof(
        inputs.clob_v2_fee_behavior_source_path,
        inputs.max_source_bytes,
    )?;
    let single_runner = collect_pre_run_single_runner_lock_source_proof(
        loaded,
        strategy_instance_id,
        inputs.single_runner_lock_path,
        inputs.max_single_runner_lock_bytes,
    )?;
    let proofs = OwnedPhase8PreRunStateSourceProofs {
        host_clock_skew_evidence_hash: host_clock.host_clock_skew_evidence_hash,
        venue_account_state_evidence_hash: venue_account.venue_account_state_evidence_hash,
        market_state_evidence_hash: market_window.market_state_evidence_hash,
        funding_margin_evidence_hash: funding_margin.funding_margin_evidence_hash,
        single_runner_lock_evidence_hash: single_runner.single_runner_lock_evidence_hash,
        egress_identity_evidence_hash: egress_identity.egress_identity_evidence_hash,
        clob_v2_adapter_signing_evidence_hash: clob_signing.clob_v2_adapter_signing_evidence_hash,
        clob_v2_collateral_accounting_evidence_hash: clob_collateral
            .clob_v2_collateral_accounting_evidence_hash,
        clob_v2_fee_behavior_evidence_hash: clob_fee.clob_v2_fee_behavior_evidence_hash,
        release_manifest_clob_signing_version: release_manifest.clob_signing_version,
        release_manifest_evidence_hash: release_manifest.evidence_hash,
    };
    write_pre_run_state_artifact_from_source_proofs(
        loaded,
        strategy_instance_id,
        proofs.as_source_proofs(),
        path,
    )
}

#[derive(Debug)]
struct OwnedPhase8PreRunStateSourceProofs {
    host_clock_skew_evidence_hash: String,
    venue_account_state_evidence_hash: String,
    market_state_evidence_hash: String,
    funding_margin_evidence_hash: String,
    single_runner_lock_evidence_hash: String,
    egress_identity_evidence_hash: String,
    clob_v2_adapter_signing_evidence_hash: String,
    clob_v2_collateral_accounting_evidence_hash: String,
    clob_v2_fee_behavior_evidence_hash: String,
    release_manifest_clob_signing_version: String,
    release_manifest_evidence_hash: String,
}

impl OwnedPhase8PreRunStateSourceProofs {
    fn as_source_proofs(&self) -> Phase8PreRunStateSourceProofs<'_> {
        Phase8PreRunStateSourceProofs {
            host_clock_skew_within_bound: true,
            host_clock_skew_evidence_hash: &self.host_clock_skew_evidence_hash,
            conflicting_open_orders_absent: true,
            preexisting_position_absent: true,
            venue_account_state_evidence_hash: &self.venue_account_state_evidence_hash,
            market_state_approved: true,
            market_window_approved: true,
            market_state_evidence_hash: &self.market_state_evidence_hash,
            funding_margin_covers_max_notional_plus_fees: true,
            funding_margin_evidence_hash: &self.funding_margin_evidence_hash,
            single_runner_lock_acquired: true,
            single_runner_lock_evidence_hash: &self.single_runner_lock_evidence_hash,
            egress_identity_approved: true,
            egress_identity_evidence_hash: &self.egress_identity_evidence_hash,
            clob_v2_adapter_signing_verified: true,
            clob_v2_adapter_signing_evidence_hash: &self.clob_v2_adapter_signing_evidence_hash,
            clob_v2_collateral_accounting_verified: true,
            clob_v2_collateral_accounting_evidence_hash: &self
                .clob_v2_collateral_accounting_evidence_hash,
            clob_v2_fee_behavior_verified: true,
            clob_v2_fee_behavior_evidence_hash: &self.clob_v2_fee_behavior_evidence_hash,
            release_manifest_clob_signing_version: &self.release_manifest_clob_signing_version,
            release_manifest_nt_revision_matches_compiled_pin: true,
            release_manifest_evidence_hash: &self.release_manifest_evidence_hash,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunStateSourceProofBundle {
    schema_version: u32,
    record_kind: String,
    host_clock_skew_within_bound: bool,
    host_clock_evidence: serde_json::Value,
    conflicting_open_orders_absent: bool,
    preexisting_position_absent: bool,
    venue_account_state_evidence: serde_json::Value,
    market_state_approved: bool,
    market_window_approved: bool,
    market_state_evidence_hash: String,
    funding_margin_covers_max_notional_plus_fees: bool,
    funding_margin_evidence: serde_json::Value,
    single_runner_lock_acquired: bool,
    single_runner_lock_evidence: serde_json::Value,
    egress_identity_approved: bool,
    egress_identity_evidence: serde_json::Value,
    clob_v2_adapter_signing_verified: bool,
    clob_v2_adapter_signing_evidence: serde_json::Value,
    clob_v2_collateral_accounting_verified: bool,
    clob_v2_collateral_accounting_evidence: serde_json::Value,
    clob_v2_fee_behavior_verified: bool,
    clob_v2_fee_behavior_evidence: serde_json::Value,
    release_manifest_clob_signing_version: String,
    release_manifest_nt_revision_matches_compiled_pin: bool,
    release_manifest_evidence_hash: String,
}

impl Phase8PreRunStateSourceProofBundle {
    fn into_source_proofs(
        self,
    ) -> Result<OwnedPhase8PreRunStateSourceProofs, BoltV3OperatorArtifactError> {
        if self.schema_version != PRE_RUN_STATE_SOURCE_PROOF_BUNDLE_SCHEMA_VERSION {
            return Err(
                BoltV3OperatorArtifactError::PreRunStateSourceBundleInvalid {
                    field: "schema_version",
                },
            );
        }
        if self.record_kind != PRE_RUN_STATE_SOURCE_PROOF_BUNDLE_RECORD_KIND {
            return Err(
                BoltV3OperatorArtifactError::PreRunStateSourceBundleInvalid {
                    field: "record_kind",
                },
            );
        }
        require_pre_run_source_bundle_bool(
            "host_clock_skew_within_bound",
            self.host_clock_skew_within_bound,
        )?;
        require_pre_run_source_bundle_bool(
            "conflicting_open_orders_absent",
            self.conflicting_open_orders_absent,
        )?;
        require_pre_run_source_bundle_bool(
            "preexisting_position_absent",
            self.preexisting_position_absent,
        )?;
        require_pre_run_source_bundle_bool("market_state_approved", self.market_state_approved)?;
        require_pre_run_source_bundle_bool("market_window_approved", self.market_window_approved)?;
        require_pre_run_source_bundle_sha256(
            "market_state_evidence_hash",
            &self.market_state_evidence_hash,
        )?;
        require_pre_run_source_bundle_bool(
            "funding_margin_covers_max_notional_plus_fees",
            self.funding_margin_covers_max_notional_plus_fees,
        )?;
        require_pre_run_source_bundle_bool(
            "single_runner_lock_acquired",
            self.single_runner_lock_acquired,
        )?;
        require_pre_run_source_bundle_bool(
            "egress_identity_approved",
            self.egress_identity_approved,
        )?;
        require_pre_run_source_bundle_bool(
            "clob_v2_adapter_signing_verified",
            self.clob_v2_adapter_signing_verified,
        )?;
        require_pre_run_source_bundle_bool(
            "clob_v2_collateral_accounting_verified",
            self.clob_v2_collateral_accounting_verified,
        )?;
        require_pre_run_source_bundle_bool(
            "clob_v2_fee_behavior_verified",
            self.clob_v2_fee_behavior_verified,
        )?;
        if self.release_manifest_clob_signing_version.trim().is_empty() {
            return Err(
                BoltV3OperatorArtifactError::PreRunStateSourceBundleInvalid {
                    field: "release_manifest_clob_signing_version",
                },
            );
        }
        require_pre_run_source_bundle_bool(
            "release_manifest_nt_revision_matches_compiled_pin",
            self.release_manifest_nt_revision_matches_compiled_pin,
        )?;
        require_pre_run_source_bundle_sha256(
            "release_manifest_evidence_hash",
            &self.release_manifest_evidence_hash,
        )?;

        Ok(OwnedPhase8PreRunStateSourceProofs {
            host_clock_skew_evidence_hash: pre_run_source_bundle_evidence_hash(
                "host_clock_evidence",
                &self.host_clock_evidence,
            )?,
            venue_account_state_evidence_hash: pre_run_source_bundle_evidence_hash(
                "venue_account_state_evidence",
                &self.venue_account_state_evidence,
            )?,
            market_state_evidence_hash: self.market_state_evidence_hash,
            funding_margin_evidence_hash: pre_run_source_bundle_evidence_hash(
                "funding_margin_evidence",
                &self.funding_margin_evidence,
            )?,
            single_runner_lock_evidence_hash: pre_run_source_bundle_evidence_hash(
                "single_runner_lock_evidence",
                &self.single_runner_lock_evidence,
            )?,
            egress_identity_evidence_hash: pre_run_source_bundle_evidence_hash(
                "egress_identity_evidence",
                &self.egress_identity_evidence,
            )?,
            clob_v2_adapter_signing_evidence_hash: pre_run_source_bundle_evidence_hash(
                "clob_v2_adapter_signing_evidence",
                &self.clob_v2_adapter_signing_evidence,
            )?,
            clob_v2_collateral_accounting_evidence_hash: pre_run_source_bundle_evidence_hash(
                "clob_v2_collateral_accounting_evidence",
                &self.clob_v2_collateral_accounting_evidence,
            )?,
            clob_v2_fee_behavior_evidence_hash: pre_run_source_bundle_evidence_hash(
                "clob_v2_fee_behavior_evidence",
                &self.clob_v2_fee_behavior_evidence,
            )?,
            release_manifest_clob_signing_version: self.release_manifest_clob_signing_version,
            release_manifest_evidence_hash: self.release_manifest_evidence_hash,
        })
    }
}

fn read_pre_run_state_source_bundle_file(
    path: &Path,
    max_bytes: u64,
) -> Result<Phase8PreRunStateSourceProofBundle, BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunStateSourceBundleRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunStateSourceBundleParse {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn require_pre_run_source_bundle_bool(
    field: &'static str,
    value: bool,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::PreRunStateSourceBundleInvalid { field })
    }
}

fn require_pre_run_source_bundle_sha256(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if is_lowercase_sha256(value) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::PreRunStateSourceBundleInvalid { field })
    }
}

fn pre_run_source_bundle_evidence_hash(
    field: &'static str,
    value: &serde_json::Value,
) -> Result<String, BoltV3OperatorArtifactError> {
    if value.is_null() {
        return Err(BoltV3OperatorArtifactError::PreRunStateSourceBundleInvalid { field });
    }
    json_artifact_sha256(value)
}

fn read_clob_v2_source<T>(
    path: &Path,
    max_bytes: u64,
) -> Result<(T, String), BoltV3OperatorArtifactError>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let value = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceParse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok((value, sha256))
}

fn require_clob_v2_source_sha256(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if is_lowercase_sha256(value) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field })
    }
}

fn parse_clob_v2_decimal(
    field: &'static str,
    value: &str,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    value
        .parse::<Decimal>()
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field })
}

fn clob_v2_decimal_string_below_required(
    field: &'static str,
    value: &str,
    required: Decimal,
) -> Result<bool, BoltV3OperatorArtifactError> {
    let required = required.normalize().to_string();
    Ok(compare_nonnegative_decimal_strings(
        field,
        value,
        stringify!(required_max_notional_plus_fees),
        &required,
    )? == Ordering::Less)
}

fn clob_v2_min_decimal_string(
    left_field: &'static str,
    left: &str,
    right_field: &'static str,
    right: &str,
) -> Result<String, BoltV3OperatorArtifactError> {
    if compare_nonnegative_decimal_strings(left_field, left, right_field, right)?
        == Ordering::Greater
    {
        Ok(right.to_string())
    } else {
        Ok(left.to_string())
    }
}

fn compare_nonnegative_decimal_strings(
    left_field: &'static str,
    left: &str,
    right_field: &'static str,
    right: &str,
) -> Result<Ordering, BoltV3OperatorArtifactError> {
    let (left_integer, left_fractional) = nonnegative_decimal_parts(left_field, left)?;
    let (right_integer, right_fractional) = nonnegative_decimal_parts(right_field, right)?;
    match left_integer.len().cmp(&right_integer.len()) {
        Ordering::Equal => match left_integer.cmp(&right_integer) {
            Ordering::Equal => {
                let width = left_fractional.len().max(right_fractional.len());
                let mut left_fractional = left_fractional;
                let mut right_fractional = right_fractional;
                left_fractional.extend(std::iter::repeat_n('0', width - left_fractional.len()));
                right_fractional.extend(std::iter::repeat_n('0', width - right_fractional.len()));
                Ok(left_fractional.cmp(&right_fractional))
            }
            ordering => Ok(ordering),
        },
        ordering => Ok(ordering),
    }
}

fn nonnegative_decimal_parts(
    field: &'static str,
    value: &str,
) -> Result<(String, String), BoltV3OperatorArtifactError> {
    let mut parts = value.split('.');
    let integer = parts
        .next()
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field })?;
    let fractional = parts.next().unwrap_or("");
    if parts.next().is_some()
        || integer.is_empty()
        || !integer.chars().all(|ch| ch.is_ascii_digit())
        || !fractional.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field });
    }
    let integer = integer.trim_start_matches('0');
    let fractional = fractional.trim_end_matches('0');
    Ok((
        if integer.is_empty() {
            "0".to_string()
        } else {
            integer.to_string()
        },
        fractional.to_string(),
    ))
}

pub fn write_pre_run_state_artifact_from_source_bundle_file(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    source_bundle_path: &Path,
    max_source_bundle_bytes: u64,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let bundle =
        read_pre_run_state_source_bundle_file(source_bundle_path, max_source_bundle_bytes)?;
    let proofs = bundle.into_source_proofs()?;
    write_pre_run_state_artifact_from_source_proofs(
        loaded,
        strategy_instance_id,
        proofs.as_source_proofs(),
        path,
    )
}

pub fn collect_pre_run_release_manifest_source_proof(
    cargo_toml_path: &Path,
    cargo_lock_path: &Path,
    clob_signing_source_path: &Path,
    max_source_bytes: u64,
) -> Result<Phase8PreRunReleaseManifestSourceProof, BoltV3OperatorArtifactError> {
    let cargo_toml_bytes = read_release_manifest_source_file(cargo_toml_path, max_source_bytes)?;
    let cargo_lock_bytes = read_release_manifest_source_file(cargo_lock_path, max_source_bytes)?;
    let clob_signing_bytes =
        read_release_manifest_source_file(clob_signing_source_path, max_source_bytes)?;
    let cargo_toml_sha256 = hex::encode(Sha256::digest(&cargo_toml_bytes));
    let cargo_lock_sha256 = hex::encode(Sha256::digest(&cargo_lock_bytes));
    let clob_signing_source_sha256 = hex::encode(Sha256::digest(&clob_signing_bytes));
    let cargo_toml_text = release_manifest_utf8(&cargo_toml_bytes, "cargo_toml_utf8")?;
    let cargo_lock_text = release_manifest_utf8(&cargo_lock_bytes, "cargo_lock_utf8")?;
    let clob_signing_text = release_manifest_utf8(&clob_signing_bytes, "clob_signing_source_utf8")?;
    let nt_revision = nautilus_revision_from_cargo_toml(cargo_toml_text)?;
    let compiled_nt_revision = compiled_nautilus_revision_from_build_manifest()?;
    if nt_revision != compiled_nt_revision {
        return Err(
            BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                field: "compiled_nautilus_revision",
            },
        );
    }
    require_cargo_lock_matches_nautilus_revision(cargo_lock_text, nt_revision.as_str())?;
    let clob_signing_version = clob_domain_version_from_source(clob_signing_text)?;
    let proof_input = Phase8PreRunReleaseManifestSourceProofHashInput {
        schema_version: PRE_RUN_RELEASE_MANIFEST_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_RELEASE_MANIFEST_SOURCE_PROOF_RECORD_KIND,
        nt_revision: nt_revision.as_str(),
        clob_signing_version: clob_signing_version.as_str(),
        cargo_toml_sha256: cargo_toml_sha256.as_str(),
        cargo_lock_sha256: cargo_lock_sha256.as_str(),
        clob_signing_source_sha256: clob_signing_source_sha256.as_str(),
    };
    let evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunReleaseManifestSourceProof {
        nt_revision,
        clob_signing_version,
        nt_revision_matches_compiled_pin: true,
        cargo_toml_sha256,
        cargo_lock_sha256,
        clob_signing_source_sha256,
        evidence_hash,
    })
}

pub async fn write_pre_run_venue_account_state_source_artifact_from_configured_account_queries(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    resolved: &ResolvedBoltV3Secrets,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(
                |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                    field: "financial_envelope",
                },
            )?;
    let materialized = materialize_venue_account_state_source_from_configured_account_queries(
        VenueAccountStateSourceMaterializationRequest {
            schema_version: PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_SCHEMA_VERSION,
            account_state_snapshot_record_kind: PRE_RUN_VENUE_ACCOUNT_STATE_SNAPSHOT_RECORD_KIND,
            loaded,
            strategy_instance_id,
            configured_target_id: financial_envelope.configured_target_id(),
            resolved,
        },
    )
    .await?;
    if materialized.open_order_count != 0 {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "conflicting_open_orders_absent",
            },
        );
    }
    if materialized.open_position_count != 0 {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "preexisting_position_absent",
            },
        );
    }
    let source = Phase8PreRunVenueAccountStateSourceEvidence {
        schema_version: PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_RECORD_KIND.to_string(),
        execution_client_id: financial_envelope.execution_client_id().to_string(),
        configured_target_id: financial_envelope.configured_target_id().to_string(),
        open_order_count: materialized.open_order_count,
        open_position_count: materialized.open_position_count,
        account_state_snapshot_sha256: materialized.account_state_snapshot_sha256,
    };

    write_json_artifact_create_new(output_path, &source)
}

pub fn write_pre_run_clob_v2_adapter_signing_source_artifact_from_nt_signing_source(
    cargo_toml_path: &Path,
    cargo_lock_path: &Path,
    clob_signing_source_path: &Path,
    max_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let release_manifest = collect_pre_run_release_manifest_source_proof(
        cargo_toml_path,
        cargo_lock_path,
        clob_signing_source_path,
        max_source_bytes,
    )?;
    let clob_signing_bytes =
        read_release_manifest_source_file(clob_signing_source_path, max_source_bytes)?;
    let clob_signing_source =
        release_manifest_utf8(&clob_signing_bytes, "clob_v2_adapter_signing_source_utf8")?;
    let materialized = materialize_clob_v2_adapter_signing_source_from_nt_signing_source(
        ClobV2AdapterSigningSourceMaterializationRequest {
            schema_version: PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_SCHEMA_VERSION,
            domain_requirements_record_kind:
                PRE_RUN_CLOB_V2_ADAPTER_SIGNING_DOMAIN_REQUIREMENTS_RECORD_KIND,
            signed_order_fixture_record_kind:
                PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SIGNED_ORDER_FIXTURE_RECORD_KIND,
            signature_verification_record_kind:
                PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SIGNATURE_VERIFICATION_RECORD_KIND,
            clob_signing_version: &release_manifest.clob_signing_version,
            clob_signing_source_sha256: &release_manifest.clob_signing_source_sha256,
            clob_signing_source,
        },
    )?;
    let source = Phase8PreRunClobV2AdapterSigningSourceEvidence {
        schema_version: PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_RECORD_KIND.to_string(),
        clob_signing_version: release_manifest.clob_signing_version,
        adapter_signing_source_sha256: release_manifest.clob_signing_source_sha256,
        domain_requirements_sha256: materialized.domain_requirements_sha256,
        signed_order_fixture_sha256: materialized.signed_order_fixture_sha256,
        signature_verification_sha256: materialized.signature_verification_sha256,
        signer_recovered_matches_expected: materialized.signer_recovered_matches_expected,
    };

    write_json_artifact_create_new(output_path, &source)
}

pub fn write_pre_run_clob_v2_fee_behavior_source_artifact_from_nt_fee_sources(
    nt_execution_parse_source_path: &Path,
    nt_http_parse_source_path: &Path,
    max_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let execution_parse_bytes = read_file_bounded(nt_execution_parse_source_path, max_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::PreRunClobV2SourceRead {
                path: nt_execution_parse_source_path.to_path_buf(),
                source,
            },
        )?;
    let execution_parse_source =
        release_manifest_utf8(&execution_parse_bytes, "nt_execution_parse_source_utf8")?;
    let http_parse_bytes =
        read_file_bounded(nt_http_parse_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunClobV2SourceRead {
                path: nt_http_parse_source_path.to_path_buf(),
                source,
            }
        })?;
    let http_parse_source = release_manifest_utf8(&http_parse_bytes, "nt_http_parse_source_utf8")?;
    let materialized = materialize_clob_v2_fee_behavior_source_from_nt_fee_sources(
        ClobV2FeeBehaviorSourceMaterializationRequest {
            schema_version: PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_SCHEMA_VERSION,
            nt_execution_parse_source: execution_parse_source,
            nt_http_parse_source: http_parse_source,
        },
    )?;
    let source = Phase8PreRunClobV2FeeBehaviorSourceEvidence {
        schema_version: PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_RECORD_KIND.to_string(),
        fee_behavior_verified: true,
        maker_zero_fee_verified: materialized.maker_zero_fee_verified,
        taker_fee_schedule_verified: materialized.taker_fee_schedule_verified,
        market_buy_fee_adjustment_verified: materialized.market_buy_fee_adjustment_verified,
        price: materialized.price,
        fee_rate: materialized.fee_rate,
        fee_behavior_source_sha256: materialized.fee_behavior_source_sha256,
        fee_assumptions_sha256: materialized.fee_assumptions_sha256,
    };

    write_json_artifact_create_new(output_path, &source)
}

pub async fn write_pre_run_clob_v2_collateral_accounting_source_artifact_from_configured_balance_allowance(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    resolved: Option<&ResolvedBoltV3Secrets>,
    fee_rate_source_path: &Path,
    fee_rate_source_sha256: &str,
    max_fee_rate_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let collateral_requirement = derive_clob_v2_required_max_notional_plus_fees(
        loaded,
        strategy_instance_id,
        fee_rate_source_path,
        fee_rate_source_sha256,
        max_fee_rate_source_bytes,
    )?;

    let mut materialized =
        materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance(
            ClobV2CollateralAccountingSourceMaterializationRequest {
                schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION,
                balance_allowance_record_kind:
                    PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BALANCE_ALLOWANCE_RECORD_KIND,
                on_chain_balance_allowance_record_kind:
                    PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ON_CHAIN_BALANCE_ALLOWANCE_RECORD_KIND,
                loaded,
                strategy_instance_id,
                resolved,
            },
        )
        .await?;
    let confirmation_policy = materialized.confirmation_policy;
    materialized = confirm_external_snapshot_before_hard_stop(
        materialized,
        confirmation_policy,
        || {
            materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_once(
                ClobV2CollateralAccountingSourceMaterializationRequest {
                    schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION,
                    balance_allowance_record_kind:
                        PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BALANCE_ALLOWANCE_RECORD_KIND,
                    on_chain_balance_allowance_record_kind:
                        PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ON_CHAIN_BALANCE_ALLOWANCE_RECORD_KIND,
                    loaded,
                    strategy_instance_id,
                    resolved,
                },
            )
        },
        |materialized| {
            clob_v2_materialized_balance_allowance_below_required(
                materialized,
                collateral_requirement.required_max_notional_plus_fees,
            )
        },
    )
    .await;
    if clob_v2_decimal_string_below_required(
        stringify!(p_usd_balance),
        &materialized.p_usd_balance,
        collateral_requirement.required_max_notional_plus_fees,
    )? || clob_v2_decimal_string_below_required(
        stringify!(p_usd_allowance),
        &materialized.p_usd_allowance,
        collateral_requirement.required_max_notional_plus_fees,
    )? {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(collateral_accounting_verified),
        });
    }

    let required_max_notional_plus_fees = collateral_requirement
        .required_max_notional_plus_fees
        .normalize()
        .to_string();
    let max_fee_bps = collateral_requirement.max_fee_bps.normalize().to_string();
    let collateral_assumptions = ClobV2CollateralAccountingAssumptionsProof {
        schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ASSUMPTIONS_RECORD_KIND,
        max_notional_per_order: collateral_requirement
            .financial_envelope
            .max_notional_per_order(),
        fee_rate_source_sha256: &collateral_requirement.fee_rate_source_sha256,
        max_fee_bps: &max_fee_bps,
        bps_denominator: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BPS_DENOMINATOR,
        required_max_notional_plus_fees: &required_max_notional_plus_fees,
    };
    let collateral_assumptions_sha256 = json_artifact_sha256(&collateral_assumptions)?;
    let source = Phase8PreRunClobV2CollateralAccountingSourceEvidence {
        schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_RECORD_KIND.to_string(),
        collateral_accounting_verified: true,
        p_usd_balance: materialized.p_usd_balance,
        p_usd_allowance: materialized.p_usd_allowance,
        required_max_notional_plus_fees,
        collateral_accounting_source_sha256: materialized.collateral_accounting_source_sha256,
        collateral_assumptions_sha256,
    };

    write_json_artifact_create_new(output_path, &source)
}

pub fn pre_run_clob_v2_collateral_accounting_source_requires_resolved_secrets(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> Result<bool, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "strategy_instance_id",
        })?;
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let client = loaded.root.clients.get(execution_client_id).ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "execution_client_id",
        },
    )?;
    let execution = client
        .execution
        .as_ref()
        .and_then(toml::Value::as_table)
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "execution" })?;
    Ok(!execution.contains_key("on_chain_collateral"))
}

pub async fn write_pre_run_funding_margin_source_artifact_from_configured_balance_allowance(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    resolved: Option<&ResolvedBoltV3Secrets>,
    fee_rate_source_path: &Path,
    fee_rate_source_sha256: &str,
    max_fee_rate_source_bytes: u64,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let collateral_requirement = derive_clob_v2_required_max_notional_plus_fees(
        loaded,
        strategy_instance_id,
        fee_rate_source_path,
        fee_rate_source_sha256,
        max_fee_rate_source_bytes,
    )?;
    let mut materialized =
        materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance(
            ClobV2CollateralAccountingSourceMaterializationRequest {
                schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION,
                balance_allowance_record_kind:
                    PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BALANCE_ALLOWANCE_RECORD_KIND,
                on_chain_balance_allowance_record_kind:
                    PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ON_CHAIN_BALANCE_ALLOWANCE_RECORD_KIND,
                loaded,
                strategy_instance_id,
                resolved,
            },
        )
        .await?;
    let confirmation_policy = materialized.confirmation_policy;
    materialized = confirm_external_snapshot_before_hard_stop(
        materialized,
        confirmation_policy,
        || {
            materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_once(
                ClobV2CollateralAccountingSourceMaterializationRequest {
                    schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION,
                    balance_allowance_record_kind:
                        PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BALANCE_ALLOWANCE_RECORD_KIND,
                    on_chain_balance_allowance_record_kind:
                        PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_ON_CHAIN_BALANCE_ALLOWANCE_RECORD_KIND,
                    loaded,
                    strategy_instance_id,
                    resolved,
                },
            )
        },
        |materialized| {
            clob_v2_materialized_balance_allowance_below_required(
                materialized,
                collateral_requirement.required_max_notional_plus_fees,
            )
        },
    )
    .await;
    let available_collateral = clob_v2_min_decimal_string(
        stringify!(p_usd_balance),
        &materialized.p_usd_balance,
        stringify!(p_usd_allowance),
        &materialized.p_usd_allowance,
    )?;
    if clob_v2_decimal_string_below_required(
        stringify!(available_collateral),
        &available_collateral,
        collateral_requirement.required_max_notional_plus_fees,
    )? {
        return Err(
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: "funding_margin_covers_max_notional_plus_fees",
            },
        );
    }
    let source = Phase8PreRunFundingMarginSourceEvidence {
        schema_version: PRE_RUN_FUNDING_MARGIN_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_FUNDING_MARGIN_SOURCE_RECORD_KIND.to_string(),
        available_collateral,
        required_max_notional_plus_fees: collateral_requirement
            .required_max_notional_plus_fees
            .normalize()
            .to_string(),
        margin_snapshot_sha256: materialized.collateral_accounting_source_sha256,
    };

    write_json_artifact_create_new(output_path, &source)
}

struct ClobV2CollateralRequirement {
    financial_envelope: Phase8FinancialEnvelopeEvidenceFile,
    fee_rate_source_sha256: String,
    max_fee_bps: Decimal,
    required_max_notional_plus_fees: Decimal,
}

fn clob_v2_materialized_balance_allowance_below_required(
    materialized: &ClobV2CollateralAccountingSourceMaterialization,
    required_max_notional_plus_fees: Decimal,
) -> bool {
    let balance_below = clob_v2_decimal_string_below_required(
        stringify!(p_usd_balance),
        &materialized.p_usd_balance,
        required_max_notional_plus_fees,
    );
    let allowance_below = clob_v2_decimal_string_below_required(
        stringify!(p_usd_allowance),
        &materialized.p_usd_allowance,
        required_max_notional_plus_fees,
    );

    balance_below.unwrap_or(true) || allowance_below.unwrap_or(true)
}

fn derive_clob_v2_required_max_notional_plus_fees(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    fee_rate_source_path: &Path,
    fee_rate_source_sha256: &str,
    max_fee_rate_source_bytes: u64,
) -> Result<ClobV2CollateralRequirement, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "financial_envelope",
            })?;
    let max_notional_per_order = parse_clob_v2_decimal(
        stringify!(max_notional_per_order),
        financial_envelope.max_notional_per_order(),
    )?;
    let (fee_rate_source, actual_fee_rate_source_sha256) =
        read_clob_v2_fee_rate_source(fee_rate_source_path, max_fee_rate_source_bytes)?;
    if actual_fee_rate_source_sha256 != fee_rate_source_sha256 {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "fee_rate_source_sha256",
        });
    }
    validate_entry_decision_fee_rate_source_artifact(&fee_rate_source).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "fee_rate_source",
        }
    })?;
    let max_fee_bps = max_fee_bps_from_fee_rate_source(&fee_rate_source)?;
    let fee_multiplier = Decimal::ONE
        + max_fee_bps / Decimal::from(PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BPS_DENOMINATOR);
    let required_max_notional_plus_fees = max_notional_per_order * fee_multiplier;
    if required_max_notional_plus_fees <= Decimal::ZERO {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(required_max_notional_plus_fees),
        });
    }

    Ok(ClobV2CollateralRequirement {
        financial_envelope,
        fee_rate_source_sha256: actual_fee_rate_source_sha256,
        max_fee_bps,
        required_max_notional_plus_fees,
    })
}

fn read_clob_v2_fee_rate_source(
    path: &Path,
    max_bytes: u64,
) -> Result<(EntryDecisionFeeRateSourceArtifact, String), BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let source = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceParse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok((source, sha256))
}

fn max_fee_bps_from_fee_rate_source(
    source: &EntryDecisionFeeRateSourceArtifact,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    let mut max_fee_bps = Decimal::ZERO;
    for fee_bps in source.fee_bps_by_instrument_id.values() {
        let fee_bps = Decimal::from_f64(*fee_bps).ok_or(
            BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "fee_rate_source",
            },
        )?;
        if fee_bps > max_fee_bps {
            max_fee_bps = fee_bps;
        }
    }
    Ok(max_fee_bps)
}

pub async fn write_pre_run_host_clock_source_artifact_from_configured_provider_time(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let (reference_url, timeout_secs) =
        configured_host_clock_reference(loaded, strategy_instance_id)?;
    let source =
        collect_host_clock_source_from_http_date_header(&reference_url, timeout_secs).await?;
    write_json_artifact_create_new(output_path, &source)
}

fn configured_host_clock_reference(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> Result<(String, u64), BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::PreRunHostClockSourceMaterialize {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let client = loaded
        .root
        .clients
        .get(execution_client_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::PreRunHostClockSourceMaterialize {
                message: format!("execution client `{execution_client_id}` is not configured"),
            },
        )?;
    let execution = client
        .execution
        .as_ref()
        .and_then(toml::Value::as_table)
        .ok_or(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid { field: "execution" })?;
    let reference_url = execution
        .get(PRE_RUN_HOST_CLOCK_REFERENCE_URL_FIELD)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "execution.base_url_http",
        })?;
    let timeout_secs = execution
        .get(PRE_RUN_HOST_CLOCK_REFERENCE_TIMEOUT_FIELD)
        .and_then(toml::Value::as_integer)
        .filter(|value| value.is_positive())
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "execution.http_timeout_secs",
        })?;

    Ok((reference_url.to_string(), timeout_secs))
}

async fn collect_host_clock_source_from_http_date_header(
    reference_url: &str,
    timeout_secs: u64,
) -> Result<Phase8PreRunHostClockSourceEvidence, BoltV3OperatorArtifactError> {
    let client = HttpClient::new(
        HashMap::from([(USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string())]),
        vec![PRE_RUN_HOST_CLOCK_REFERENCE_DATE_HEADER.to_string()],
        Vec::new(),
        None,
        Some(timeout_secs),
        None,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::PreRunHostClockSourceMaterialize {
            message: source.to_string(),
        },
    )?;
    let response = client
        .get(
            reference_url.to_string(),
            None,
            None,
            Some(timeout_secs),
            None,
        )
        .await
        .map_err(
            |source| BoltV3OperatorArtifactError::PreRunHostClockSourceMaterialize {
                message: source.to_string(),
            },
        )?;
    let reference_date = response
        .headers
        .get(PRE_RUN_HOST_CLOCK_REFERENCE_DATE_HEADER)
        .ok_or(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "date_header",
        })?;
    let reference_unix_millis = chrono::DateTime::parse_from_rfc2822(reference_date)
        .map_err(
            |_| BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
                field: "date_header",
            },
        )?
        .timestamp_millis();
    let reference_unix_millis = u64::try_from(reference_unix_millis).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "date_header",
        }
    })?;
    let host_unix_millis = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(
                |_| BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
                    field: "host_unix_millis",
                },
            )?
            .as_millis(),
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "host_unix_millis",
        },
    )?;

    Ok(Phase8PreRunHostClockSourceEvidence {
        schema_version: PRE_RUN_HOST_CLOCK_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_HOST_CLOCK_SOURCE_RECORD_KIND.to_string(),
        host_unix_millis,
        reference_unix_millis,
    })
}

pub fn collect_pre_run_host_clock_source_proof(
    host_clock_source_path: &Path,
    max_source_bytes: u64,
    max_host_clock_skew_millis: u64,
) -> Result<Phase8PreRunHostClockSourceProof, BoltV3OperatorArtifactError> {
    if max_host_clock_skew_millis == 0 {
        return Err(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "max_host_clock_skew_millis",
        });
    }
    let host_clock_source_bytes = read_file_bounded(host_clock_source_path, max_source_bytes)
        .map_err(
            |source| BoltV3OperatorArtifactError::PreRunHostClockSourceRead {
                path: host_clock_source_path.to_path_buf(),
                source,
            },
        )?;
    let host_clock_source_sha256 = hex::encode(Sha256::digest(&host_clock_source_bytes));
    let source: Phase8PreRunHostClockSourceEvidence =
        serde_json::from_slice(&host_clock_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunHostClockSourceParse {
                path: host_clock_source_path.to_path_buf(),
                source,
            }
        })?;
    if source.schema_version != PRE_RUN_HOST_CLOCK_SOURCE_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "schema_version",
        });
    }
    if source.record_kind != PRE_RUN_HOST_CLOCK_SOURCE_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "record_kind",
        });
    }
    let host_clock_skew_millis = source
        .host_unix_millis
        .abs_diff(source.reference_unix_millis);
    if host_clock_skew_millis > max_host_clock_skew_millis {
        return Err(BoltV3OperatorArtifactError::PreRunHostClockSourceInvalid {
            field: "host_clock_skew_millis",
        });
    }
    let proof_input = Phase8PreRunHostClockSourceProofHashInput {
        schema_version: PRE_RUN_HOST_CLOCK_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_HOST_CLOCK_SOURCE_PROOF_RECORD_KIND,
        host_unix_millis: source.host_unix_millis,
        reference_unix_millis: source.reference_unix_millis,
        host_clock_skew_millis,
        max_host_clock_skew_millis,
        host_clock_source_sha256: host_clock_source_sha256.as_str(),
    };
    let host_clock_skew_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunHostClockSourceProof {
        host_clock_skew_within_bound: true,
        host_clock_skew_millis,
        max_host_clock_skew_millis,
        host_clock_source_sha256,
        host_clock_skew_evidence_hash,
    })
}

pub fn collect_pre_run_venue_account_state_source_proof(
    venue_account_state_source_path: &Path,
    max_source_bytes: u64,
    expected_execution_client_id: &str,
    expected_configured_target_id: &str,
) -> Result<Phase8PreRunVenueAccountStateSourceProof, BoltV3OperatorArtifactError> {
    let venue_account_state_source_bytes =
        read_file_bounded(venue_account_state_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceRead {
                path: venue_account_state_source_path.to_path_buf(),
                source,
            }
        })?;
    let venue_account_state_source_sha256 =
        hex::encode(Sha256::digest(&venue_account_state_source_bytes));
    let source: Phase8PreRunVenueAccountStateSourceEvidence =
        serde_json::from_slice(&venue_account_state_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceParse {
                path: venue_account_state_source_path.to_path_buf(),
                source,
            }
        })?;
    if source.schema_version != PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_SCHEMA_VERSION {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "schema_version",
            },
        );
    }
    if source.record_kind != PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_RECORD_KIND {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "record_kind",
            },
        );
    }
    if source.execution_client_id != expected_execution_client_id {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "execution_client_id",
            },
        );
    }
    if source.configured_target_id != expected_configured_target_id {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "configured_target_id",
            },
        );
    }
    if !is_lowercase_sha256(&source.account_state_snapshot_sha256) {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "account_state_snapshot_sha256",
            },
        );
    }
    if source.open_order_count != 0 {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "conflicting_open_orders_absent",
            },
        );
    }
    if source.open_position_count != 0 {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "preexisting_position_absent",
            },
        );
    }
    let proof_input = Phase8PreRunVenueAccountStateSourceProofHashInput {
        schema_version: PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_VENUE_ACCOUNT_STATE_SOURCE_PROOF_RECORD_KIND,
        execution_client_id: &source.execution_client_id,
        configured_target_id: &source.configured_target_id,
        open_order_count: source.open_order_count,
        open_position_count: source.open_position_count,
        account_state_snapshot_sha256: &source.account_state_snapshot_sha256,
        venue_account_state_source_sha256: &venue_account_state_source_sha256,
    };
    let venue_account_state_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunVenueAccountStateSourceProof {
        conflicting_open_orders_absent: true,
        preexisting_position_absent: true,
        venue_account_state_source_sha256,
        venue_account_state_evidence_hash,
    })
}

pub fn collect_pre_run_funding_margin_source_proof(
    funding_margin_source_path: &Path,
    max_source_bytes: u64,
) -> Result<Phase8PreRunFundingMarginSourceProof, BoltV3OperatorArtifactError> {
    let funding_margin_source_bytes =
        read_file_bounded(funding_margin_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceRead {
                path: funding_margin_source_path.to_path_buf(),
                source,
            }
        })?;
    let funding_margin_source_sha256 = hex::encode(Sha256::digest(&funding_margin_source_bytes));
    let source: Phase8PreRunFundingMarginSourceEvidence =
        serde_json::from_slice(&funding_margin_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceParse {
                path: funding_margin_source_path.to_path_buf(),
                source,
            }
        })?;
    if source.schema_version != PRE_RUN_FUNDING_MARGIN_SOURCE_SCHEMA_VERSION {
        return Err(
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: "schema_version",
            },
        );
    }
    if source.record_kind != PRE_RUN_FUNDING_MARGIN_SOURCE_RECORD_KIND {
        return Err(
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: "record_kind",
            },
        );
    }
    if !is_lowercase_sha256(&source.margin_snapshot_sha256) {
        return Err(
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: "margin_snapshot_sha256",
            },
        );
    }
    let required_max_notional_plus_fees = source
        .required_max_notional_plus_fees
        .parse::<Decimal>()
        .map_err(
            |_| BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: stringify!(required_max_notional_plus_fees),
            },
        )?;
    if required_max_notional_plus_fees <= Decimal::ZERO {
        return Err(
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: stringify!(required_max_notional_plus_fees),
            },
        );
    }
    if clob_v2_decimal_string_below_required(
        "available_collateral",
        &source.available_collateral,
        required_max_notional_plus_fees,
    )? {
        return Err(
            BoltV3OperatorArtifactError::PreRunFundingMarginSourceInvalid {
                field: "funding_margin_covers_max_notional_plus_fees",
            },
        );
    }
    let proof_input = Phase8PreRunFundingMarginSourceProofHashInput {
        schema_version: PRE_RUN_FUNDING_MARGIN_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_FUNDING_MARGIN_SOURCE_PROOF_RECORD_KIND,
        available_collateral: &source.available_collateral,
        required_max_notional_plus_fees: &source.required_max_notional_plus_fees,
        margin_snapshot_sha256: &source.margin_snapshot_sha256,
        funding_margin_source_sha256: &funding_margin_source_sha256,
    };
    let funding_margin_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunFundingMarginSourceProof {
        funding_margin_covers_max_notional_plus_fees: true,
        funding_margin_source_sha256,
        funding_margin_evidence_hash,
    })
}

pub fn collect_pre_run_clob_v2_adapter_signing_source_proof(
    clob_v2_adapter_signing_source_path: &Path,
    max_source_bytes: u64,
    expected_clob_signing_version: &str,
) -> Result<Phase8PreRunClobV2AdapterSigningSourceProof, BoltV3OperatorArtifactError> {
    if expected_clob_signing_version.trim().is_empty() {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "expected_clob_signing_version",
        });
    }
    let (source, clob_v2_adapter_signing_source_sha256) =
        read_clob_v2_source::<Phase8PreRunClobV2AdapterSigningSourceEvidence>(
            clob_v2_adapter_signing_source_path,
            max_source_bytes,
        )?;
    if source.schema_version != PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "schema_version",
        });
    }
    if source.record_kind != PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "record_kind",
        });
    }
    if source.clob_signing_version.trim() != expected_clob_signing_version.trim() {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(clob_signing_version),
        });
    }
    if !source.signer_recovered_matches_expected {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(signer_recovered_matches_expected),
        });
    }
    require_clob_v2_source_sha256(
        stringify!(adapter_signing_source_sha256),
        &source.adapter_signing_source_sha256,
    )?;
    require_clob_v2_source_sha256(
        stringify!(domain_requirements_sha256),
        &source.domain_requirements_sha256,
    )?;
    require_clob_v2_source_sha256(
        stringify!(signed_order_fixture_sha256),
        &source.signed_order_fixture_sha256,
    )?;
    require_clob_v2_source_sha256(
        stringify!(signature_verification_sha256),
        &source.signature_verification_sha256,
    )?;
    let proof_input = Phase8PreRunClobV2AdapterSigningSourceProofHashInput {
        schema_version: PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_ADAPTER_SIGNING_SOURCE_PROOF_RECORD_KIND,
        clob_signing_version: &source.clob_signing_version,
        adapter_signing_source_sha256: &source.adapter_signing_source_sha256,
        domain_requirements_sha256: &source.domain_requirements_sha256,
        signed_order_fixture_sha256: &source.signed_order_fixture_sha256,
        signature_verification_sha256: &source.signature_verification_sha256,
        signer_recovered_matches_expected: source.signer_recovered_matches_expected,
        clob_v2_adapter_signing_source_sha256: &clob_v2_adapter_signing_source_sha256,
    };
    let clob_v2_adapter_signing_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunClobV2AdapterSigningSourceProof {
        clob_v2_adapter_signing_verified: true,
        clob_v2_adapter_signing_source_sha256,
        clob_v2_adapter_signing_evidence_hash,
    })
}

pub fn collect_pre_run_clob_v2_collateral_accounting_source_proof(
    clob_v2_collateral_accounting_source_path: &Path,
    max_source_bytes: u64,
) -> Result<Phase8PreRunClobV2CollateralAccountingSourceProof, BoltV3OperatorArtifactError> {
    let (source, clob_v2_collateral_accounting_source_sha256) =
        read_clob_v2_source::<Phase8PreRunClobV2CollateralAccountingSourceEvidence>(
            clob_v2_collateral_accounting_source_path,
            max_source_bytes,
        )?;
    if source.schema_version != PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "schema_version",
        });
    }
    if source.record_kind != PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "record_kind",
        });
    }
    if !source.collateral_accounting_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(clob_v2_collateral_accounting_verified),
        });
    }
    let required_max_notional_plus_fees = parse_clob_v2_decimal(
        stringify!(required_max_notional_plus_fees),
        &source.required_max_notional_plus_fees,
    )?;
    if required_max_notional_plus_fees <= Decimal::ZERO {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(required_max_notional_plus_fees),
        });
    }
    if clob_v2_decimal_string_below_required(
        stringify!(p_usd_balance),
        &source.p_usd_balance,
        required_max_notional_plus_fees,
    )? || clob_v2_decimal_string_below_required(
        stringify!(p_usd_allowance),
        &source.p_usd_allowance,
        required_max_notional_plus_fees,
    )? {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(clob_v2_collateral_accounting_verified),
        });
    }
    require_clob_v2_source_sha256(
        stringify!(collateral_accounting_source_sha256),
        &source.collateral_accounting_source_sha256,
    )?;
    require_clob_v2_source_sha256(
        stringify!(collateral_assumptions_sha256),
        &source.collateral_assumptions_sha256,
    )?;
    let proof_input = Phase8PreRunClobV2CollateralAccountingSourceProofHashInput {
        schema_version: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_SOURCE_PROOF_RECORD_KIND,
        p_usd_balance: &source.p_usd_balance,
        p_usd_allowance: &source.p_usd_allowance,
        required_max_notional_plus_fees: &source.required_max_notional_plus_fees,
        collateral_accounting_source_sha256: &source.collateral_accounting_source_sha256,
        collateral_assumptions_sha256: &source.collateral_assumptions_sha256,
        clob_v2_collateral_accounting_source_sha256: &clob_v2_collateral_accounting_source_sha256,
    };
    let clob_v2_collateral_accounting_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunClobV2CollateralAccountingSourceProof {
        clob_v2_collateral_accounting_verified: true,
        clob_v2_collateral_accounting_source_sha256,
        clob_v2_collateral_accounting_evidence_hash,
    })
}

pub fn collect_pre_run_clob_v2_fee_behavior_source_proof(
    clob_v2_fee_behavior_source_path: &Path,
    max_source_bytes: u64,
) -> Result<Phase8PreRunClobV2FeeBehaviorSourceProof, BoltV3OperatorArtifactError> {
    let (source, clob_v2_fee_behavior_source_sha256) =
        read_clob_v2_source::<Phase8PreRunClobV2FeeBehaviorSourceEvidence>(
            clob_v2_fee_behavior_source_path,
            max_source_bytes,
        )?;
    if source.schema_version != PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "schema_version",
        });
    }
    if source.record_kind != PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "record_kind",
        });
    }
    if !source.fee_behavior_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(clob_v2_fee_behavior_verified),
        });
    }
    if !source.maker_zero_fee_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(maker_zero_fee_verified),
        });
    }
    if !source.taker_fee_schedule_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(taker_fee_schedule_verified),
        });
    }
    if !source.market_buy_fee_adjustment_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(market_buy_fee_adjustment_verified),
        });
    }
    let price = parse_clob_v2_decimal(stringify!(price), &source.price)?;
    let fee_rate = parse_clob_v2_decimal(stringify!(fee_rate), &source.fee_rate)?;
    if price <= Decimal::ZERO || price >= Decimal::ONE {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(price),
        });
    }
    if fee_rate < Decimal::ZERO {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(fee_rate),
        });
    }
    require_clob_v2_source_sha256(
        stringify!(fee_behavior_source_sha256),
        &source.fee_behavior_source_sha256,
    )?;
    require_clob_v2_source_sha256(
        stringify!(fee_assumptions_sha256),
        &source.fee_assumptions_sha256,
    )?;
    let proof_input = Phase8PreRunClobV2FeeBehaviorSourceProofHashInput {
        schema_version: PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_CLOB_V2_FEE_BEHAVIOR_SOURCE_PROOF_RECORD_KIND,
        price: &source.price,
        fee_rate: &source.fee_rate,
        fee_behavior_source_sha256: &source.fee_behavior_source_sha256,
        fee_assumptions_sha256: &source.fee_assumptions_sha256,
        clob_v2_fee_behavior_source_sha256: &clob_v2_fee_behavior_source_sha256,
    };
    let clob_v2_fee_behavior_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunClobV2FeeBehaviorSourceProof {
        clob_v2_fee_behavior_verified: true,
        clob_v2_fee_behavior_source_sha256,
        clob_v2_fee_behavior_evidence_hash,
    })
}

pub fn collect_pre_run_market_window_source_proof(
    strategy_input_evidence_path: &Path,
    strategy_input_evidence_sha256: &str,
    expected_price_to_beat_source: &str,
    max_strategy_input_evidence_bytes: u64,
) -> Result<Phase8PreRunMarketWindowSourceProof, BoltV3OperatorArtifactError> {
    if !is_lowercase_sha256(strategy_input_evidence_sha256) {
        return Err(
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "strategy_input_evidence_sha256",
            },
        );
    }
    let strategy_input_evidence_bytes = read_file_bounded(
        strategy_input_evidence_path,
        max_strategy_input_evidence_bytes,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::PreRunMarketWindowSourceRead {
            path: strategy_input_evidence_path.to_path_buf(),
            source,
        },
    )?;
    let actual_sha256 = hex::encode(Sha256::digest(&strategy_input_evidence_bytes));
    if actual_sha256 != strategy_input_evidence_sha256 {
        return Err(
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "strategy_input_evidence_sha256",
            },
        );
    }
    let market_selection_source_bytes = read_strategy_input_market_selection_source_bytes(
        &strategy_input_evidence_bytes,
        max_strategy_input_evidence_bytes,
    )?;
    let audit = Phase8StrategyInputSafetyAudit::from_evidence_bytes_with_market_selection_source(
        &strategy_input_evidence_bytes,
        strategy_input_evidence_sha256,
        expected_price_to_beat_source,
        &market_selection_source_bytes,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
            field: "strategy_input_evidence",
        },
    )?;
    if !audit.is_approved() {
        return Err(
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "strategy_input_audit",
            },
        );
    }
    let proof_input = Phase8PreRunMarketWindowSourceProofHashInput {
        schema_version: PRE_RUN_MARKET_WINDOW_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_MARKET_WINDOW_SOURCE_PROOF_RECORD_KIND,
        strategy_input_evidence_sha256,
        expected_price_to_beat_source,
    };
    let market_state_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunMarketWindowSourceProof {
        market_state_approved: true,
        market_window_approved: true,
        market_state_evidence_hash,
    })
}

pub fn collect_pre_run_single_runner_lock_source_proof(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    lock_path: &Path,
    max_lock_file_bytes: u64,
) -> Result<Phase8PreRunSingleRunnerLockSourceProof, BoltV3OperatorArtifactError> {
    let _ =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    validate_output_path_components("single_runner_lock_path", lock_path)?;
    let resolved_lock_path = resolve_loaded_config_path_from_path(loaded, lock_path);
    let artifact = Phase8PreRunSingleRunnerLockEvidenceFile {
        schema_version: PRE_RUN_SINGLE_RUNNER_LOCK_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_SINGLE_RUNNER_LOCK_SOURCE_PROOF_RECORD_KIND,
        config_bundle_checksum: loaded.config_bundle_checksum.as_str(),
        strategy_instance_id,
        lock_path_sha256: sha256_text(&resolved_lock_path.to_string_lossy()),
    };
    let bytes =
        serde_json::to_vec_pretty(&artifact).map_err(BoltV3OperatorArtifactError::Serialize)?;
    if bytes.len() as u64 > max_lock_file_bytes {
        return Err(
            BoltV3OperatorArtifactError::PreRunSingleRunnerLockSourceInvalid {
                field: "single_runner_lock_evidence_size",
            },
        );
    }

    let written = write_json_artifact_create_new_from_bytes(&resolved_lock_path, &bytes).map_err(
        |error| match error {
            BoltV3OperatorArtifactError::Write { path, source }
                if path == resolved_lock_path
                    && source.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                BoltV3OperatorArtifactError::PreRunSingleRunnerLockSourceInvalid {
                    field: "single_runner_lock_acquired",
                }
            }
            other => other,
        },
    )?;
    Ok(Phase8PreRunSingleRunnerLockSourceProof {
        single_runner_lock_acquired: true,
        single_runner_lock_evidence_hash: written.sha256,
    })
}

pub fn collect_pre_run_egress_identity_source_proof(
    egress_identity_source_path: &Path,
    max_source_bytes: u64,
) -> Result<Phase8PreRunEgressIdentitySourceProof, BoltV3OperatorArtifactError> {
    let egress_identity_source_bytes =
        read_file_bounded(egress_identity_source_path, max_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceRead {
                path: egress_identity_source_path.to_path_buf(),
                source,
            }
        })?;
    let egress_identity_source_sha256 = hex::encode(Sha256::digest(&egress_identity_source_bytes));
    let source: Phase8PreRunEgressIdentitySourceEvidence =
        serde_json::from_slice(&egress_identity_source_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceParse {
                path: egress_identity_source_path.to_path_buf(),
                source,
            }
        })?;
    if source.schema_version != PRE_RUN_EGRESS_IDENTITY_SOURCE_SCHEMA_VERSION {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: "schema_version",
            },
        );
    }
    if source.record_kind != PRE_RUN_EGRESS_IDENTITY_SOURCE_RECORD_KIND {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: "record_kind",
            },
        );
    }
    if !is_lowercase_sha256(&source.observed_egress_identity_sha256) {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: "observed_egress_identity_sha256",
            },
        );
    }
    if !is_lowercase_sha256(&source.approved_egress_identity_sha256) {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: "approved_egress_identity_sha256",
            },
        );
    }
    if source.observed_egress_identity_sha256 != source.approved_egress_identity_sha256 {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: "approved_egress_identity_sha256",
            },
        );
    }
    let proof_input = Phase8PreRunEgressIdentitySourceProofHashInput {
        schema_version: PRE_RUN_EGRESS_IDENTITY_SOURCE_PROOF_SCHEMA_VERSION,
        record_kind: PRE_RUN_EGRESS_IDENTITY_SOURCE_PROOF_RECORD_KIND,
        observed_egress_identity_sha256: &source.observed_egress_identity_sha256,
        approved_egress_identity_sha256: &source.approved_egress_identity_sha256,
        egress_identity_source_sha256: &egress_identity_source_sha256,
    };
    let egress_identity_evidence_hash = json_artifact_sha256(&proof_input)?;

    Ok(Phase8PreRunEgressIdentitySourceProof {
        egress_identity_approved: true,
        egress_identity_source_sha256,
        egress_identity_evidence_hash,
    })
}

pub fn write_pre_run_egress_identity_source_artifact_from_configured_probe(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    if !loaded
        .strategies
        .iter()
        .any(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
    {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(strategy_instance_id),
            },
        );
    }
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let observed_path = live_canary
        .egress_identity_observed_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(egress_identity_observed_path),
            },
        )?;
    let approved_egress_identity_sha256 = live_canary
        .approved_egress_identity_sha256
        .as_deref()
        .filter(|value| is_lowercase_sha256(value))
        .ok_or(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(approved_egress_identity_sha256),
            },
        )?;
    let max_egress_identity_observed_bytes = live_canary
        .egress_identity_observed_max_bytes
        .filter(|value| std::num::NonZeroU64::new(*value).is_some())
        .ok_or(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(egress_identity_observed_max_bytes),
            },
        )?;
    let resolved_observed_path = resolve_loaded_config_path(loaded, observed_path);
    let observed_identity_bytes =
        read_file_bounded(&resolved_observed_path, max_egress_identity_observed_bytes).map_err(
            |source| BoltV3OperatorArtifactError::PreRunEgressIdentitySourceRead {
                path: resolved_observed_path,
                source,
            },
        )?;
    let observed_identity = std::str::from_utf8(&observed_identity_bytes)
        .map_err(
            |_| BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(observed_egress_identity_sha256),
            },
        )?
        .trim();
    if observed_identity.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(observed_egress_identity_sha256),
            },
        );
    }
    let observed_egress_identity_sha256 = hex::encode(Sha256::digest(observed_identity.as_bytes()));
    if observed_egress_identity_sha256 != approved_egress_identity_sha256 {
        return Err(
            BoltV3OperatorArtifactError::PreRunEgressIdentitySourceInvalid {
                field: stringify!(approved_egress_identity_sha256),
            },
        );
    }
    let source = Phase8PreRunEgressIdentitySourceEvidence {
        schema_version: PRE_RUN_EGRESS_IDENTITY_SOURCE_SCHEMA_VERSION,
        record_kind: PRE_RUN_EGRESS_IDENTITY_SOURCE_RECORD_KIND.to_string(),
        observed_egress_identity_sha256,
        approved_egress_identity_sha256: approved_egress_identity_sha256.to_string(),
    };

    write_json_artifact_create_new(output_path, &source)
}

#[derive(Serialize)]
struct Phase8PreRunSingleRunnerLockEvidenceFile<'a> {
    schema_version: u32,
    record_kind: &'static str,
    config_bundle_checksum: &'a str,
    strategy_instance_id: &'a str,
    lock_path_sha256: String,
}

fn read_strategy_input_market_selection_source_bytes(
    strategy_input_evidence_bytes: &[u8],
    max_market_selection_source_bytes: u64,
) -> Result<Vec<u8>, BoltV3OperatorArtifactError> {
    let json: serde_json::Value =
        serde_json::from_slice(strategy_input_evidence_bytes).map_err(|_| {
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "strategy_input_evidence",
            }
        })?;
    let source_path = json
        .get("market_selection_source_path")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "market_selection_source_path",
            },
        )?;
    let source_sha256 = json
        .get("market_selection_source_sha256")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| is_lowercase_sha256(value))
        .ok_or(
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "market_selection_source_sha256",
            },
        )?;
    let source_path = Path::new(source_path);
    validate_market_window_source_path("market_selection_source_path", source_path)?;
    let source_bytes =
        read_file_bounded(source_path, max_market_selection_source_bytes).map_err(|_| {
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "market_selection_source_path",
            }
        })?;
    if hex::encode(Sha256::digest(&source_bytes)) != source_sha256 {
        return Err(
            BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid {
                field: "market_selection_source_sha256",
            },
        );
    }
    Ok(source_bytes)
}

fn validate_market_window_source_path(
    field: &'static str,
    path: &Path,
) -> Result<(), BoltV3OperatorArtifactError> {
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(BoltV3OperatorArtifactError::PreRunMarketWindowSourceInvalid { field });
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunHostClockSourceEvidence {
    schema_version: u32,
    record_kind: String,
    host_unix_millis: u64,
    reference_unix_millis: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunVenueAccountStateSourceEvidence {
    schema_version: u32,
    record_kind: String,
    execution_client_id: String,
    configured_target_id: String,
    open_order_count: u64,
    open_position_count: u64,
    account_state_snapshot_sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunFundingMarginSourceEvidence {
    schema_version: u32,
    record_kind: String,
    available_collateral: String,
    required_max_notional_plus_fees: String,
    margin_snapshot_sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunClobV2AdapterSigningSourceEvidence {
    schema_version: u32,
    record_kind: String,
    clob_signing_version: String,
    adapter_signing_source_sha256: String,
    domain_requirements_sha256: String,
    signed_order_fixture_sha256: String,
    signature_verification_sha256: String,
    signer_recovered_matches_expected: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunClobV2CollateralAccountingSourceEvidence {
    schema_version: u32,
    record_kind: String,
    collateral_accounting_verified: bool,
    p_usd_balance: String,
    p_usd_allowance: String,
    required_max_notional_plus_fees: String,
    collateral_accounting_source_sha256: String,
    collateral_assumptions_sha256: String,
}

#[derive(Serialize)]
struct ClobV2CollateralAccountingAssumptionsProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    max_notional_per_order: &'a str,
    fee_rate_source_sha256: &'a str,
    max_fee_bps: &'a str,
    bps_denominator: u32,
    required_max_notional_plus_fees: &'a str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunClobV2FeeBehaviorSourceEvidence {
    schema_version: u32,
    record_kind: String,
    fee_behavior_verified: bool,
    maker_zero_fee_verified: bool,
    taker_fee_schedule_verified: bool,
    market_buy_fee_adjustment_verified: bool,
    price: String,
    fee_rate: String,
    fee_behavior_source_sha256: String,
    fee_assumptions_sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Phase8PreRunEgressIdentitySourceEvidence {
    schema_version: u32,
    record_kind: String,
    observed_egress_identity_sha256: String,
    approved_egress_identity_sha256: String,
}

#[derive(Serialize)]
struct Phase8PreRunReleaseManifestSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    nt_revision: &'a str,
    clob_signing_version: &'a str,
    cargo_toml_sha256: &'a str,
    cargo_lock_sha256: &'a str,
    clob_signing_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunHostClockSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    host_unix_millis: u64,
    reference_unix_millis: u64,
    host_clock_skew_millis: u64,
    max_host_clock_skew_millis: u64,
    host_clock_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunVenueAccountStateSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    execution_client_id: &'a str,
    configured_target_id: &'a str,
    open_order_count: u64,
    open_position_count: u64,
    account_state_snapshot_sha256: &'a str,
    venue_account_state_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunFundingMarginSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    available_collateral: &'a str,
    required_max_notional_plus_fees: &'a str,
    margin_snapshot_sha256: &'a str,
    funding_margin_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunClobV2AdapterSigningSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    clob_signing_version: &'a str,
    adapter_signing_source_sha256: &'a str,
    domain_requirements_sha256: &'a str,
    signed_order_fixture_sha256: &'a str,
    signature_verification_sha256: &'a str,
    signer_recovered_matches_expected: bool,
    clob_v2_adapter_signing_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunClobV2CollateralAccountingSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    p_usd_balance: &'a str,
    p_usd_allowance: &'a str,
    required_max_notional_plus_fees: &'a str,
    collateral_accounting_source_sha256: &'a str,
    collateral_assumptions_sha256: &'a str,
    clob_v2_collateral_accounting_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunClobV2FeeBehaviorSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    price: &'a str,
    fee_rate: &'a str,
    fee_behavior_source_sha256: &'a str,
    fee_assumptions_sha256: &'a str,
    clob_v2_fee_behavior_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunMarketWindowSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_input_evidence_sha256: &'a str,
    expected_price_to_beat_source: &'a str,
}

#[derive(Serialize)]
struct Phase8PreRunEgressIdentitySourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    observed_egress_identity_sha256: &'a str,
    approved_egress_identity_sha256: &'a str,
    egress_identity_source_sha256: &'a str,
}

#[derive(Serialize)]
struct Phase8AbortPlanCancelIfOpenSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_source_sha256: &'a str,
    forced_flat_cancel_before_exit_pending: bool,
}

#[derive(Serialize)]
struct Phase8AbortPlanNtAcceptedVenuePendingSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_source_sha256: &'a str,
    exit_pending_before_submit: bool,
    submit_error_restores_managed_position: bool,
    terminal_handlers_mark_exit_order_terminal: bool,
}

#[derive(Serialize)]
struct Phase8AbortPlanPartialFillSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_source_sha256: &'a str,
    partial_fill_waits_for_position_close: bool,
    position_close_completes_exit: bool,
    residual_after_fill_preserved: bool,
    terminal_without_flat_preserves_managed: bool,
}

#[derive(Serialize)]
struct Phase8AbortPlanNetworkPartitionSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_source_sha256: &'a str,
    submit_error_restores_managed_position: bool,
}

#[derive(Serialize)]
struct Phase8AbortPlanPanicGateServicePolicySourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_source_sha256: &'a str,
    submit_admission_source_sha256: &'a str,
    panic_recovery_enters_blind_recovery: bool,
    release_invariant_returns_error: bool,
    submit_lifecycle_policy_from_config: bool,
    submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle: bool,
    replace_submit_policy_gates_service_submit: bool,
}

struct AbortPlanCancelIfOpenContract {
    forced_flat_cancel_before_exit_pending: bool,
}

struct AbortPlanNtAcceptedVenuePendingContract {
    exit_pending_before_submit: bool,
    submit_error_restores_managed_position: bool,
    terminal_handlers_mark_exit_order_terminal: bool,
}

struct AbortPlanPartialFillContract {
    partial_fill_waits_for_position_close: bool,
    position_close_completes_exit: bool,
    residual_after_fill_preserved: bool,
    terminal_without_flat_preserves_managed: bool,
}

struct AbortPlanNetworkPartitionContract {
    submit_error_restores_managed_position: bool,
}

struct AbortPlanPanicGateServicePolicyContract {
    panic_recovery_enters_blind_recovery: bool,
    release_invariant_returns_error: bool,
    submit_lifecycle_policy_from_config: bool,
    submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle: bool,
    replace_submit_policy_gates_service_submit: bool,
}

fn require_abort_plan_cancel_if_open_contract(
    strategy_source: &str,
) -> Result<AbortPlanCancelIfOpenContract, BoltV3OperatorArtifactError> {
    let comment_masked_source = abort_plan_cancel_if_open_comment_masked_source(strategy_source);
    let context_masked_source =
        abort_plan_cancel_if_open_raw_string_masked_source(&comment_masked_source);
    let code_masked_source = abort_plan_cancel_if_open_string_masked_source(&comment_masked_source);
    let mut candidate_indexes = Vec::new();
    let mut first_invalid_candidate_error = None;
    let mut target_function_scope_count = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;
    for function_scope in abort_plan_cancel_if_open_function_scopes(&code_masked_source) {
        let scoped_context_source = &context_masked_source[function_scope.clone()];
        let scoped_code_source = &code_masked_source[function_scope];
        if !abort_plan_cancel_if_open_scope_matches_target_function(scoped_code_source) {
            continue;
        }
        target_function_scope_count += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
        match abort_plan_cancel_if_open_scoped_marker_indexes(
            scoped_context_source,
            scoped_code_source,
        ) {
            Ok(Some(indexes)) => {
                candidate_indexes.push(indexes);
            }
            Ok(None) => {}
            Err(error) => {
                if first_invalid_candidate_error.is_none() {
                    first_invalid_candidate_error = Some(error);
                }
            }
        }
    }

    if target_function_scope_count != ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceInvalid {
                field: "forced_flat_function_scope",
            },
        );
    }

    if candidate_indexes.is_empty()
        && let Some(error) = first_invalid_candidate_error
    {
        return Err(error);
    }

    let [indexes] = candidate_indexes.as_slice() else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceInvalid {
                field: "forced_flat_function_scope",
            },
        );
    };

    if indexes.forced_flat < indexes.pending_entry
        && indexes.pending_entry < indexes.cancel_order
        && indexes.cancel_order < indexes.context
        && indexes.context < indexes.exit_pending
    {
        Ok(AbortPlanCancelIfOpenContract {
            forced_flat_cancel_before_exit_pending: true,
        })
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceInvalid {
                field: "forced_flat_cancel_before_exit_pending",
            },
        )
    }
}

fn require_abort_plan_nt_accepted_venue_pending_contract(
    strategy_source: &str,
) -> Result<AbortPlanNtAcceptedVenuePendingContract, BoltV3OperatorArtifactError> {
    let comment_masked_source = abort_plan_cancel_if_open_comment_masked_source(strategy_source);
    let code_masked_source = abort_plan_cancel_if_open_string_masked_source(&comment_masked_source);
    let mut candidate_indexes = Vec::new();
    let mut target_function_scope_count = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;
    for function_scope in abort_plan_cancel_if_open_function_scopes(&code_masked_source) {
        let scoped_code_source = &code_masked_source[function_scope];
        if !abort_plan_cancel_if_open_scope_matches_target_function(scoped_code_source) {
            continue;
        }
        target_function_scope_count += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
        if let Some(indexes) =
            abort_plan_nt_accepted_venue_pending_scoped_marker_indexes(scoped_code_source)?
        {
            candidate_indexes.push(indexes);
        }
    }

    if target_function_scope_count != ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceInvalid {
                field: "exit_submit_function_scope",
            },
        );
    }

    let [indexes] = candidate_indexes.as_slice() else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceInvalid {
                field: "exit_submit_function_scope",
            },
        );
    };

    if !(indexes.exit_pending < indexes.pending_exit
        && indexes.pending_exit < indexes.fill_received_false
        && indexes.fill_received_false < indexes.close_received_false
        && indexes.close_received_false < indexes.terminal_received_false
        && indexes.terminal_received_false < indexes.submit
        && indexes.submit < indexes.restore_managed
        && indexes.restore_managed < indexes.return_error
        && indexes.return_error < indexes.ok_some)
    {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceInvalid {
                field: "exit_pending_before_submit",
            },
        );
    }

    require_abort_plan_nt_accepted_venue_pending_terminal_contract(&code_masked_source)?;

    Ok(AbortPlanNtAcceptedVenuePendingContract {
        exit_pending_before_submit: true,
        submit_error_restores_managed_position: true,
        terminal_handlers_mark_exit_order_terminal: true,
    })
}

fn require_abort_plan_partial_fill_contract(
    strategy_source: &str,
) -> Result<AbortPlanPartialFillContract, BoltV3OperatorArtifactError> {
    let comment_masked_source = abort_plan_cancel_if_open_comment_masked_source(strategy_source);
    let code_masked_source = abort_plan_cancel_if_open_string_masked_source(&comment_masked_source);

    require_abort_plan_partial_fill_waits_for_position_close(&code_masked_source)?;
    require_abort_plan_partial_fill_position_close_completes_exit(&code_masked_source)?;
    require_abort_plan_partial_fill_residual_after_fill_preserved(&code_masked_source)?;
    require_abort_plan_partial_fill_terminal_without_flat_preserves_managed(&code_masked_source)?;

    Ok(AbortPlanPartialFillContract {
        partial_fill_waits_for_position_close: true,
        position_close_completes_exit: true,
        residual_after_fill_preserved: true,
        terminal_without_flat_preserves_managed: true,
    })
}

fn require_abort_plan_network_partition_contract(
    strategy_source: &str,
) -> Result<AbortPlanNetworkPartitionContract, BoltV3OperatorArtifactError> {
    let comment_masked_source = abort_plan_cancel_if_open_comment_masked_source(strategy_source);
    let code_masked_source = abort_plan_cancel_if_open_string_masked_source(&comment_masked_source);
    let target_function_scopes = abort_plan_cancel_if_open_function_scopes(&code_masked_source)
        .into_iter()
        .filter(|function_scope| {
            let scoped_code_source = &code_masked_source[function_scope.clone()];
            abort_plan_cancel_if_open_scope_matches_target_function(scoped_code_source)
        })
        .collect::<Vec<_>>();

    let [target_function_scope] = target_function_scopes.as_slice() else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanNetworkPartitionSourceInvalid {
                field: "submit_error_function_scope",
            },
        );
    };
    let scoped_code_source = &code_masked_source[target_function_scope.clone()];
    let submit = abort_plan_network_partition_single_marker_index(
        scoped_code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SUBMIT_MARKER,
        "submit_error_restores_managed_position",
    )?;
    let restore_managed = abort_plan_network_partition_single_marker_index(
        scoped_code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_RESTORE_MANAGED_MARKER,
        "submit_error_restores_managed_position",
    )?;
    let return_error = abort_plan_network_partition_single_marker_index(
        scoped_code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_RETURN_ERROR_MARKER,
        "submit_error_restores_managed_position",
    )?;

    if submit < restore_managed && restore_managed < return_error {
        Ok(AbortPlanNetworkPartitionContract {
            submit_error_restores_managed_position: true,
        })
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanNetworkPartitionSourceInvalid {
                field: "submit_error_restores_managed_position",
            },
        )
    }
}

fn require_abort_plan_panic_gate_service_policy_contract(
    strategy_source: &str,
    submit_admission_source: &str,
) -> Result<AbortPlanPanicGateServicePolicyContract, BoltV3OperatorArtifactError> {
    let strategy_comment_masked_source =
        abort_plan_cancel_if_open_comment_masked_source(strategy_source);
    let strategy_code_source =
        abort_plan_cancel_if_open_string_masked_source(&strategy_comment_masked_source);
    let admission_comment_masked_source =
        abort_plan_cancel_if_open_comment_masked_source(submit_admission_source);
    let admission_code_source =
        abort_plan_cancel_if_open_string_masked_source(&admission_comment_masked_source);

    require_abort_plan_panic_recovery_enters_blind_recovery(&strategy_code_source)?;
    require_abort_plan_release_invariant_returns_error(&strategy_code_source)?;
    require_abort_plan_submit_lifecycle_policy_from_config(&strategy_code_source)?;
    require_abort_plan_submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle(
        &admission_code_source,
    )?;
    require_abort_plan_replace_submit_policy_gates_service_submit(&admission_code_source)?;

    Ok(AbortPlanPanicGateServicePolicyContract {
        panic_recovery_enters_blind_recovery: true,
        release_invariant_returns_error: true,
        submit_lifecycle_policy_from_config: true,
        submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle: true,
        replace_submit_policy_gates_service_submit: true,
    })
}

#[derive(Debug)]
struct AbortPlanCancelIfOpenMarkerIndexes {
    forced_flat: usize,
    pending_entry: usize,
    cancel_order: usize,
    context: usize,
    exit_pending: usize,
}

#[derive(Debug)]
struct AbortPlanNtAcceptedVenuePendingMarkerIndexes {
    exit_pending: usize,
    pending_exit: usize,
    fill_received_false: usize,
    close_received_false: usize,
    terminal_received_false: usize,
    submit: usize,
    restore_managed: usize,
    return_error: usize,
    ok_some: usize,
}

fn abort_plan_cancel_if_open_function_scopes(strategy_source: &str) -> Vec<Range<usize>> {
    let mut function_starts = Vec::new();
    let mut line_offset = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;
    for line in strategy_source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if abort_plan_cancel_if_open_function_start_line(trimmed) {
            function_starts.push(line_offset + line.len() - trimmed.len());
        }
        line_offset += line.len();
    }

    let mut scopes = Vec::new();
    let mut starts = function_starts.iter().copied().peekable();
    while let Some(start) = starts.next() {
        let end = starts.peek().copied().unwrap_or(strategy_source.len());
        scopes.push(start..end);
    }
    scopes
}

fn abort_plan_cancel_if_open_scope_matches_target_function(scoped_source: &str) -> bool {
    scoped_source.lines().next().is_some_and(|line| {
        line.as_bytes()
            .windows(ABORT_PLAN_CANCEL_IF_OPEN_FUNCTION_KEYWORD_WIDTH)
            .position(|window| matches!(window, [b'f', b'n', b' ']))
            .is_some_and(|function_keyword_index| {
                let suffix = line
                    [function_keyword_index + ABORT_PLAN_CANCEL_IF_OPEN_FUNCTION_KEYWORD_WIDTH..]
                    .trim_start();
                suffix
                    .strip_prefix(ABORT_PLAN_CANCEL_IF_OPEN_TARGET_FUNCTION_NAME)
                    .and_then(|after_name| after_name.as_bytes().first().copied())
                    .is_some_and(|after_name| matches!(after_name, b'(' | b'<'))
            })
    })
}

fn abort_plan_cancel_if_open_scoped_marker_indexes(
    context_source: &str,
    code_source: &str,
) -> Result<Option<AbortPlanCancelIfOpenMarkerIndexes>, BoltV3OperatorArtifactError> {
    let forced_flat = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_CANCEL_IF_OPEN_FORCED_FLAT_MARKER,
    );
    let pending_entry = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_CANCEL_IF_OPEN_PENDING_ENTRY_MARKER,
    );
    let cancel_order = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_CANCEL_IF_OPEN_CANCEL_ORDER_MARKER,
    );
    let context = abort_plan_cancel_if_open_scoped_marker_occurrences(
        context_source,
        ABORT_PLAN_CANCEL_IF_OPEN_CONTEXT_MARKER,
    );
    let exit_pending = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_CANCEL_IF_OPEN_EXIT_PENDING_MARKER,
    );

    if forced_flat.is_empty()
        || pending_entry.is_empty()
        || cancel_order.is_empty()
        || context.is_empty()
        || exit_pending.is_empty()
    {
        return Ok(None);
    }

    Ok(Some(AbortPlanCancelIfOpenMarkerIndexes {
        forced_flat: abort_plan_cancel_if_open_single_marker_index(
            &forced_flat,
            "forced_flat_reasons",
        )?,
        pending_entry: abort_plan_cancel_if_open_single_marker_index(
            &pending_entry,
            "pending_entry",
        )?,
        cancel_order: abort_plan_cancel_if_open_single_marker_index(&cancel_order, "cancel_order")?,
        context: abort_plan_cancel_if_open_single_marker_index(&context, "cancel_order_context")?,
        exit_pending: abort_plan_cancel_if_open_single_marker_index(&exit_pending, "exit_pending")?,
    }))
}

fn abort_plan_nt_accepted_venue_pending_scoped_marker_indexes(
    code_source: &str,
) -> Result<Option<AbortPlanNtAcceptedVenuePendingMarkerIndexes>, BoltV3OperatorArtifactError> {
    let exit_pending = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_EXIT_PENDING_MARKER,
    );
    let pending_exit = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_PENDING_EXIT_MARKER,
    );
    let fill_received_false = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_FILL_FALSE_MARKER,
    );
    let close_received_false = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_CLOSE_FALSE_MARKER,
    );
    let terminal_received_false = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_FALSE_MARKER,
    );
    let submit = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_SUBMIT_MARKER,
    );
    let restore_managed = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_RESTORE_MANAGED_MARKER,
    );
    let return_error = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_RETURN_ERROR_MARKER,
    );
    let ok_some = abort_plan_cancel_if_open_scoped_marker_occurrences(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_OK_MARKER,
    );

    if exit_pending.is_empty()
        || pending_exit.is_empty()
        || fill_received_false.is_empty()
        || close_received_false.is_empty()
        || terminal_received_false.is_empty()
        || submit.is_empty()
        || restore_managed.is_empty()
        || return_error.is_empty()
        || ok_some.is_empty()
    {
        return Ok(None);
    }

    Ok(Some(AbortPlanNtAcceptedVenuePendingMarkerIndexes {
        exit_pending: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &exit_pending,
            "exit_pending",
        )?,
        pending_exit: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &pending_exit,
            "pending_exit",
        )?,
        fill_received_false: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &fill_received_false,
            "fill_received_false",
        )?,
        close_received_false: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &close_received_false,
            "close_received_false",
        )?,
        terminal_received_false: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &terminal_received_false,
            "terminal_received_false",
        )?,
        submit: abort_plan_nt_accepted_venue_pending_single_marker_index(&submit, "submit")?,
        restore_managed: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &restore_managed,
            "restore_managed",
        )?,
        return_error: abort_plan_nt_accepted_venue_pending_single_marker_index(
            &return_error,
            "return_error",
        )?,
        ok_some: abort_plan_nt_accepted_venue_pending_single_marker_index(&ok_some, "ok_some")?,
    }))
}

fn require_abort_plan_nt_accepted_venue_pending_terminal_contract(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let terminal_scope = abort_plan_nt_accepted_venue_pending_single_function_scope(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_FUNCTION_NAME,
        "mark_exit_order_terminal_function",
    )?;
    let terminal_client_match = abort_plan_cancel_if_open_scoped_marker_occurrences(
        terminal_scope,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_CLIENT_MATCH_MARKER,
    );
    let terminal_instrument_guard = abort_plan_cancel_if_open_scoped_marker_occurrences(
        terminal_scope,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_INSTRUMENT_GUARD_MARKER,
    );
    let terminal_received = abort_plan_cancel_if_open_scoped_marker_occurrences(
        terminal_scope,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_RECEIVED_MARKER,
    );
    let terminal_state_update = abort_plan_cancel_if_open_scoped_marker_occurrences(
        terminal_scope,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_TERMINAL_STATE_UPDATE_MARKER,
    );

    let terminal_client_match = abort_plan_nt_accepted_venue_pending_single_marker_index(
        &terminal_client_match,
        "terminal_client_order_match",
    )?;
    let terminal_instrument_guard = abort_plan_nt_accepted_venue_pending_single_marker_index(
        &terminal_instrument_guard,
        "terminal_event_instrument_guard",
    )?;
    let terminal_received = abort_plan_nt_accepted_venue_pending_single_marker_index(
        &terminal_received,
        "terminal_received",
    )?;
    let terminal_state_update = abort_plan_nt_accepted_venue_pending_single_marker_index(
        &terminal_state_update,
        "terminal_state_update",
    )?;
    require_abort_plan_nt_accepted_venue_pending_terminal_handler(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_CANCELED_HANDLER_NAME,
        "terminal_canceled_handler",
    )?;
    require_abort_plan_nt_accepted_venue_pending_terminal_handler(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_REJECTED_HANDLER_NAME,
        "terminal_rejected_handler",
    )?;
    require_abort_plan_nt_accepted_venue_pending_terminal_handler(
        code_source,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_EXPIRED_HANDLER_NAME,
        "terminal_expired_handler",
    )?;
    if terminal_client_match < terminal_instrument_guard
        && terminal_instrument_guard < terminal_received
        && terminal_received < terminal_state_update
    {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceInvalid {
                field: "terminal_state_update",
            },
        )
    }
}

fn require_abort_plan_nt_accepted_venue_pending_terminal_handler(
    code_source: &str,
    function_name: &'static str,
    field: &'static str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let handler_scope = abort_plan_nt_accepted_venue_pending_single_function_scope(
        code_source,
        function_name,
        field,
    )?;
    let terminal_handler = abort_plan_cancel_if_open_scoped_marker_occurrences(
        handler_scope,
        ABORT_PLAN_NT_ACCEPTED_VENUE_PENDING_HANDLER_MARKER,
    );
    abort_plan_nt_accepted_venue_pending_single_marker_index(&terminal_handler, field)?;
    Ok(())
}

fn abort_plan_nt_accepted_venue_pending_single_function_scope<'a>(
    code_source: &'a str,
    function_name: &'static str,
    field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    let mut scopes = abort_plan_cancel_if_open_function_scopes(code_source)
        .into_iter()
        .filter(|scope| {
            abort_plan_source_scope_matches_function(&code_source[scope.clone()], function_name)
        })
        .collect::<Vec<_>>();
    let [scope] = scopes.as_mut_slice() else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceInvalid { field },
        );
    };
    Ok(&code_source[scope.clone()])
}

fn require_abort_plan_partial_fill_waits_for_position_close(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_partial_fill_single_function_scope(
        code_source,
        ABORT_PLAN_PARTIAL_FILL_ON_ORDER_FILLED_FUNCTION_NAME,
        "partial_fill_order_filled_scope",
    )?;
    let exit_fill = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_EXIT_FILL_MARKER,
        "partial_fill_waits_for_position_close",
    )?;
    let exit_fill_branch = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_EXIT_FILL_BRANCH_MARKER,
        "partial_fill_waits_for_position_close",
    )?;
    let instrument_guards = abort_plan_cancel_if_open_scoped_marker_occurrences(
        scope,
        ABORT_PLAN_PARTIAL_FILL_EXIT_FILL_INSTRUMENT_GUARD_MARKER,
    );
    let fill_received = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_FILL_RECEIVED_MARKER,
        "partial_fill_waits_for_position_close",
    )?;
    let close_received_check = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_CLOSE_RECEIVED_CHECK_MARKER,
        "partial_fill_waits_for_position_close",
    )?;

    if !abort_plan_source_marker_between(&instrument_guards, exit_fill_branch, fill_received) {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid {
                field: "partial_fill_exit_fill_instrument_guard",
            },
        );
    }

    if exit_fill < exit_fill_branch
        && exit_fill_branch < fill_received
        && fill_received < close_received_check
    {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid {
                field: "partial_fill_waits_for_position_close",
            },
        )
    }
}

fn require_abort_plan_partial_fill_position_close_completes_exit(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_partial_fill_single_function_scope(
        code_source,
        ABORT_PLAN_PARTIAL_FILL_ON_POSITION_CLOSED_FUNCTION_NAME,
        "partial_fill_position_closed_scope",
    )?;
    let position_match = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_POSITION_MATCH_MARKER,
        "partial_fill_position_match",
    )?;
    let position_close_branch = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_POSITION_CLOSE_BRANCH_MARKER,
        "position_close_completes_exit",
    )?;
    let instrument_guards = abort_plan_cancel_if_open_scoped_marker_occurrences(
        scope,
        ABORT_PLAN_PARTIAL_FILL_POSITION_CLOSE_INSTRUMENT_GUARD_MARKER,
    );
    let close_received = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_CLOSE_RECEIVED_MARKER,
        "partial_fill_close_received",
    )?;
    let position_clear = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_POSITION_CLEAR_MARKER,
        "partial_fill_position_clear",
    )?;
    let terminal_check = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_TERMINAL_CHECK_MARKER,
        "partial_fill_terminal_check",
    )?;

    if !abort_plan_source_marker_between(&instrument_guards, position_close_branch, close_received)
    {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid {
                field: "partial_fill_position_close_instrument_guard",
            },
        );
    }

    if position_match < position_close_branch
        && position_close_branch < close_received
        && close_received < position_clear
        && position_clear < terminal_check
    {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid {
                field: "position_close_completes_exit",
            },
        )
    }
}

fn require_abort_plan_partial_fill_residual_after_fill_preserved(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_partial_fill_single_function_scope(
        code_source,
        ABORT_PLAN_PARTIAL_FILL_MATERIALIZE_FUNCTION_NAME,
        "partial_fill_materialize_scope",
    )?;
    let residual_guard = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_RESIDUAL_GUARD_MARKER,
        "partial_fill_residual_guard",
    )?;
    let residual_marker = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_RESIDUAL_MARKER,
        "partial_fill_residual_marker",
    )?;

    if residual_guard < residual_marker {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid {
                field: "residual_after_fill_preserved",
            },
        )
    }
}

fn require_abort_plan_partial_fill_terminal_without_flat_preserves_managed(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_partial_fill_single_function_scope(
        code_source,
        ABORT_PLAN_PARTIAL_FILL_TERMINAL_FUNCTION_NAME,
        "partial_fill_terminal_scope",
    )?;
    let terminal_received = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_TERMINAL_RECEIVED_MARKER,
        "partial_fill_terminal_received",
    )?;
    let terminal_not_filled = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_TERMINAL_NOT_FILLED_MARKER,
        "partial_fill_terminal_not_filled",
    )?;
    let terminal_residual = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_TERMINAL_RESIDUAL_MARKER,
        "partial_fill_terminal_residual",
    )?;
    let terminal_managed = abort_plan_partial_fill_single_marker_index(
        scope,
        ABORT_PLAN_PARTIAL_FILL_TERMINAL_MANAGED_MARKER,
        "partial_fill_terminal_managed",
    )?;

    if terminal_received < terminal_not_filled
        && terminal_not_filled < terminal_residual
        && terminal_residual < terminal_managed
    {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid {
                field: "terminal_without_flat_preserves_managed",
            },
        )
    }
}

fn require_abort_plan_panic_recovery_enters_blind_recovery(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_panic_gate_service_policy_single_function_scope(
        code_source,
        ABORT_PLAN_PANIC_BOOTSTRAP_RECOVERY_FUNCTION_NAME,
        "panic_recovery_scope",
    )?;
    let catch_unwind = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_PANIC_CATCH_UNWIND_MARKER,
        "panic_recovery_enters_blind_recovery",
    )?;
    let cache_probe_failed = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_PANIC_CACHE_PROBE_FAILED_MARKER,
        "panic_recovery_enters_blind_recovery",
    )?;
    let blind_recovery = abort_plan_cancel_if_open_scoped_marker_occurrences(
        scope,
        ABORT_PLAN_PANIC_BLIND_RECOVERY_MARKER,
    )
    .into_iter()
    .find(|index| *index < cache_probe_failed)
    .ok_or(
        BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
            field: "panic_recovery_enters_blind_recovery",
        },
    )?;
    let return_after_recovery =
        abort_plan_cancel_if_open_scoped_marker_occurrences(scope, ABORT_PLAN_PANIC_RETURN_MARKER)
            .into_iter()
            .find(|index| *index > cache_probe_failed)
            .ok_or(
                BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
                    field: "panic_recovery_enters_blind_recovery",
                },
            )?;

    if catch_unwind < blind_recovery
        && blind_recovery < cache_probe_failed
        && cache_probe_failed < return_after_recovery
    {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
                field: "panic_recovery_enters_blind_recovery",
            },
        )
    }
}

fn require_abort_plan_release_invariant_returns_error(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_panic_gate_service_policy_single_function_scope(
        code_source,
        ABORT_PLAN_PANIC_ONE_POSITION_INVARIANT_FUNCTION_NAME,
        "release_invariant_scope",
    )?;
    let debug_assertions = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_PANIC_DEBUG_ASSERTIONS_MARKER,
        "release_invariant_returns_error",
    )?;
    let debug_panic = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_PANIC_DEBUG_PANIC_MARKER,
        "release_invariant_returns_error",
    )?;
    let report = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_PANIC_REPORT_MARKER,
        "release_invariant_returns_error",
    )?;
    let bail = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_PANIC_RELEASE_BAIL_MARKER,
        "release_invariant_returns_error",
    )?;

    if debug_assertions < debug_panic && debug_panic < report && report < bail {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
                field: "release_invariant_returns_error",
            },
        )
    }
}

fn require_abort_plan_submit_lifecycle_policy_from_config(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_panic_gate_service_policy_single_function_scope(
        code_source,
        ABORT_PLAN_SERVICE_SUBMIT_LIFECYCLE_POLICY_FUNCTION_NAME,
        "submit_lifecycle_policy_scope",
    )?;
    let policy_new = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_POLICY_NEW_MARKER,
        "submit_lifecycle_policy_from_config",
    )?;
    let contingent = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_POLICY_CONTINGENT_MARKER,
        "submit_lifecycle_policy_from_config",
    )?;
    let gtd = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_POLICY_GTD_MARKER,
        "submit_lifecycle_policy_from_config",
    )?;
    let stop = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_POLICY_STOP_MARKER,
        "submit_lifecycle_policy_from_config",
    )?;

    if policy_new < contingent && contingent < gtd && gtd < stop {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
                field: "submit_lifecycle_policy_from_config",
            },
        )
    }
}

fn require_abort_plan_submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let scope = abort_plan_panic_gate_service_policy_single_function_scope(
        code_source,
        ABORT_PLAN_SERVICE_ADMISSION_EVALUATE_FUNCTION_NAME,
        "submit_admission_evaluate_scope",
    )?;
    let lifecycle_check = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_ADMISSION_LIFECYCLE_CHECK_MARKER,
        "submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle",
    )?;
    let lifecycle_reject = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_ADMISSION_LIFECYCLE_REJECT_MARKER,
        "submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle",
    )?;
    let unarmed = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_ADMISSION_UNARMED_MARKER,
        "submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle",
    )?;
    let admitted = abort_plan_panic_gate_service_policy_single_marker_index(
        scope,
        ABORT_PLAN_SERVICE_ADMISSION_ADMITTED_MARKER,
        "submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle",
    )?;

    if lifecycle_check < lifecycle_reject && lifecycle_reject < unarmed && unarmed < admitted {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
                field: "submit_admission_allows_unarmed_and_rejects_disallowed_lifecycle",
            },
        )
    }
}

fn require_abort_plan_replace_submit_policy_gates_service_submit(
    code_source: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let submit_intent_scope = abort_plan_panic_gate_service_policy_single_function_scope(
        code_source,
        ABORT_PLAN_SERVICE_SUBMIT_INTENT_FUNCTION_NAME,
        "submit_intent_for_scope",
    )?;
    let replace_allowed = abort_plan_panic_gate_service_policy_single_marker_index(
        submit_intent_scope,
        ABORT_PLAN_SERVICE_REPLACE_ALLOWED_MARKER,
        "replace_submit_policy_gates_service_submit",
    )?;
    let replace_submit = abort_plan_panic_gate_service_policy_single_marker_index(
        submit_intent_scope,
        ABORT_PLAN_SERVICE_REPLACE_SUBMIT_MARKER,
        "replace_submit_policy_gates_service_submit",
    )?;
    let replace_none = abort_plan_panic_gate_service_policy_single_marker_index(
        submit_intent_scope,
        ABORT_PLAN_SERVICE_REPLACE_NONE_MARKER,
        "replace_submit_policy_gates_service_submit",
    )?;
    let cancel_none = abort_plan_panic_gate_service_policy_single_marker_index(
        submit_intent_scope,
        ABORT_PLAN_SERVICE_CANCEL_NONE_MARKER,
        "replace_submit_policy_gates_service_submit",
    )?;
    let allows_scope = abort_plan_panic_gate_service_policy_single_function_scope(
        code_source,
        ABORT_PLAN_SERVICE_ALLOWS_FUNCTION_NAME,
        "submit_policy_allows_scope",
    )?;
    let entry_exit_allowed = abort_plan_panic_gate_service_policy_single_marker_index(
        allows_scope,
        ABORT_PLAN_SERVICE_ENTRY_EXIT_ALLOWED_MARKER,
        "replace_submit_policy_gates_service_submit",
    )?;
    let replace_allowed_flag = abort_plan_panic_gate_service_policy_single_marker_index(
        allows_scope,
        ABORT_PLAN_SERVICE_REPLACE_ALLOWED_FLAG_MARKER,
        "replace_submit_policy_gates_service_submit",
    )?;

    if replace_allowed < replace_submit
        && replace_submit < replace_none
        && replace_none < cancel_none
        && entry_exit_allowed < replace_allowed_flag
    {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid {
                field: "replace_submit_policy_gates_service_submit",
            },
        )
    }
}

fn abort_plan_cancel_if_open_scoped_marker_occurrences(
    strategy_source: &str,
    marker: &str,
) -> Vec<usize> {
    let mut indexes = Vec::new();
    let mut search_start = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;
    while let Some(relative_index) = strategy_source[search_start..].find(marker) {
        let index = search_start + relative_index;
        indexes.push(index);
        search_start = index + marker.len();
    }
    indexes
}

fn abort_plan_source_marker_between(
    indexes: &[usize],
    lower_bound: usize,
    upper_bound: usize,
) -> bool {
    indexes
        .iter()
        .any(|index| lower_bound < *index && *index < upper_bound)
}

fn abort_plan_partial_fill_single_function_scope<'a>(
    code_source: &'a str,
    function_name: &'static str,
    field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    let mut scopes = abort_plan_cancel_if_open_function_scopes(code_source)
        .into_iter()
        .filter(|scope| {
            abort_plan_source_scope_matches_function(&code_source[scope.clone()], function_name)
        })
        .collect::<Vec<_>>();
    let [scope] = scopes.as_mut_slice() else {
        return Err(BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid { field });
    };
    Ok(&code_source[scope.clone()])
}

fn abort_plan_source_scope_matches_function(scoped_source: &str, function_name: &str) -> bool {
    scoped_source.lines().next().is_some_and(|line| {
        line.as_bytes()
            .windows(ABORT_PLAN_CANCEL_IF_OPEN_FUNCTION_KEYWORD_WIDTH)
            .position(|window| matches!(window, [b'f', b'n', b' ']))
            .is_some_and(|function_keyword_index| {
                let suffix = line
                    [function_keyword_index + ABORT_PLAN_CANCEL_IF_OPEN_FUNCTION_KEYWORD_WIDTH..]
                    .trim_start();
                suffix
                    .strip_prefix(function_name)
                    .and_then(|after_name| after_name.as_bytes().first().copied())
                    .is_some_and(|after_name| matches!(after_name, b'(' | b'<'))
            })
    })
}

fn abort_plan_cancel_if_open_single_marker_index(
    indexes: &[usize],
    field: &'static str,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let [index] = indexes else {
        return Err(BoltV3OperatorArtifactError::AbortPlanCancelIfOpenSourceInvalid { field });
    };
    Ok(*index)
}

fn abort_plan_partial_fill_single_marker_index(
    source: &str,
    marker: &str,
    field: &'static str,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let indexes = abort_plan_cancel_if_open_scoped_marker_occurrences(source, marker);
    let [index] = indexes.as_slice() else {
        return Err(BoltV3OperatorArtifactError::AbortPlanPartialFillSourceInvalid { field });
    };
    Ok(*index)
}

fn abort_plan_network_partition_single_marker_index(
    source: &str,
    marker: &str,
    field: &'static str,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let indexes = abort_plan_cancel_if_open_scoped_marker_occurrences(source, marker);
    let [index] = indexes.as_slice() else {
        return Err(BoltV3OperatorArtifactError::AbortPlanNetworkPartitionSourceInvalid { field });
    };
    Ok(*index)
}

fn abort_plan_panic_gate_service_policy_single_function_scope<'a>(
    code_source: &'a str,
    function_name: &'static str,
    field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    let mut scopes = abort_plan_cancel_if_open_function_scopes(code_source)
        .into_iter()
        .filter(|scope| {
            abort_plan_source_scope_matches_function(&code_source[scope.clone()], function_name)
        })
        .collect::<Vec<_>>();
    let [scope] = scopes.as_mut_slice() else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid { field },
        );
    };
    Ok(&code_source[scope.clone()])
}

fn abort_plan_panic_gate_service_policy_single_marker_index(
    source: &str,
    marker: &str,
    field: &'static str,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let indexes = abort_plan_cancel_if_open_scoped_marker_occurrences(source, marker);
    let [index] = indexes.as_slice() else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanPanicGateServicePolicySourceInvalid { field },
        );
    };
    Ok(*index)
}

fn abort_plan_nt_accepted_venue_pending_single_marker_index(
    indexes: &[usize],
    field: &'static str,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let [index] = indexes else {
        return Err(
            BoltV3OperatorArtifactError::AbortPlanNtAcceptedVenuePendingSourceInvalid { field },
        );
    };
    Ok(*index)
}

fn abort_plan_cancel_if_open_function_start_line(trimmed_line: &str) -> bool {
    let Some(function_keyword_index) = trimmed_line
        .as_bytes()
        .windows(ABORT_PLAN_CANCEL_IF_OPEN_FUNCTION_KEYWORD_WIDTH)
        .position(|window| matches!(window, [b'f', b'n', b' ']))
    else {
        return false;
    };
    let prefix = trimmed_line[..function_keyword_index].trim();
    let prefix = abort_plan_cancel_if_open_function_prefix_without_same_line_attributes(prefix);
    prefix.is_empty() || abort_plan_cancel_if_open_function_prefix_is_supported(prefix)
}

fn abort_plan_cancel_if_open_function_prefix_without_same_line_attributes(
    mut prefix: &str,
) -> &str {
    loop {
        let trimmed_prefix = prefix.trim_start();
        let Some(stripped_prefix) =
            abort_plan_cancel_if_open_strip_same_line_attribute(trimmed_prefix)
        else {
            return trimmed_prefix;
        };
        prefix = stripped_prefix;
    }
}

fn abort_plan_cancel_if_open_strip_same_line_attribute(prefix: &str) -> Option<&str> {
    let bytes = prefix.as_bytes();
    if !matches!(bytes, [b'#', b'[', ..]) {
        return None;
    }

    let mut index = ABORT_PLAN_CANCEL_IF_OPEN_ATTRIBUTE_MARKER_WIDTH;
    let mut bracket_depth = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => {
                bracket_depth += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
            }
            b']' => {
                bracket_depth -= ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                if bracket_depth == ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN {
                    return Some(prefix[index..].trim_start());
                }
                continue;
            }
            _ => {}
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }
    None
}

fn abort_plan_cancel_if_open_function_prefix_is_supported(prefix: &str) -> bool {
    let prefix = abort_plan_cancel_if_open_function_prefix_without_pub_visibility(prefix);
    prefix
        .split_whitespace()
        .all(abort_plan_cancel_if_open_function_prefix_token_is_supported)
}

fn abort_plan_cancel_if_open_function_prefix_without_pub_visibility(prefix: &str) -> &str {
    let trimmed_prefix = prefix.trim_start();
    let bytes = trimmed_prefix.as_bytes();
    if !matches!(bytes, [b'p', b'u', b'b', b'(', ..]) {
        return trimmed_prefix;
    }

    let mut index = ABORT_PLAN_CANCEL_IF_OPEN_PUB_VISIBILITY_PREFIX_WIDTH;
    let mut paren_depth = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => {
                paren_depth += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
            }
            b')' => {
                paren_depth -= ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                if paren_depth == ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN {
                    return trimmed_prefix[index..].trim_start();
                }
                continue;
            }
            _ => {}
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }
    trimmed_prefix
}

fn abort_plan_cancel_if_open_function_prefix_token_is_supported(token: &str) -> bool {
    let bytes = token.as_bytes();
    matches!(
        bytes,
        [b'p', b'u', b'b']
            | [b'p', b'u', b'b', b'(', .., b')']
            | [b'a', b's', b'y', b'n', b'c']
            | [b'u', b'n', b's', b'a', b'f', b'e']
            | [b'c', b'o', b'n', b's', b't']
            | [b'e', b'x', b't', b'e', b'r', b'n']
    ) || bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"')
}

fn abort_plan_cancel_if_open_comment_masked_source(strategy_source: &str) -> String {
    let source_bytes = strategy_source.as_bytes();
    let mut masked_bytes = source_bytes.to_vec();
    let source_index_origin = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;
    let mut index = source_index_origin;
    let mut block_comment_depth = source_index_origin;

    while index < source_bytes.len() {
        if block_comment_depth > source_index_origin {
            if abort_plan_cancel_if_open_line_separator_byte(source_bytes[index]) {
                index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                continue;
            }
            if abort_plan_cancel_if_open_block_comment_start(source_bytes, index) {
                abort_plan_cancel_if_open_mask_pair(&mut masked_bytes, index);
                block_comment_depth += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                index += ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH;
                continue;
            }
            if abort_plan_cancel_if_open_block_comment_end(source_bytes, index) {
                abort_plan_cancel_if_open_mask_pair(&mut masked_bytes, index);
                block_comment_depth -= ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
                index += ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH;
                continue;
            }
            masked_bytes[index] = b' ';
            index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
            continue;
        }

        if let Some(end) = abort_plan_cancel_if_open_char_literal_end(source_bytes, index) {
            index = end;
            continue;
        }
        if abort_plan_cancel_if_open_line_comment_start(source_bytes, index) {
            abort_plan_cancel_if_open_mask_line_comment(
                source_bytes,
                &mut masked_bytes,
                &mut index,
            );
            continue;
        }
        if abort_plan_cancel_if_open_block_comment_start(source_bytes, index) {
            abort_plan_cancel_if_open_mask_pair(&mut masked_bytes, index);
            block_comment_depth += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
            index += ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH;
            continue;
        }
        if let Some(end) = abort_plan_cancel_if_open_raw_string_end(source_bytes, index) {
            index = end;
            continue;
        }
        if source_bytes[index] == b'"' {
            index = abort_plan_cancel_if_open_quoted_string_end(source_bytes, index);
            continue;
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }

    String::from_utf8(masked_bytes)
        .unwrap_or_else(|source| String::from_utf8_lossy(source.as_bytes()).into_owned())
}

fn abort_plan_cancel_if_open_raw_string_masked_source(strategy_source: &str) -> String {
    let source_bytes = strategy_source.as_bytes();
    let mut masked_bytes = source_bytes.to_vec();
    let mut index = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;

    while index < source_bytes.len() {
        if let Some(end) = abort_plan_cancel_if_open_raw_string_end(source_bytes, index) {
            abort_plan_cancel_if_open_mask_range(source_bytes, &mut masked_bytes, index, end);
            index = end;
            continue;
        }
        if let Some(end) = abort_plan_cancel_if_open_char_literal_end(source_bytes, index) {
            index = end;
            continue;
        }
        if source_bytes[index] == b'"' {
            index = abort_plan_cancel_if_open_quoted_string_end(source_bytes, index);
            continue;
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }

    String::from_utf8(masked_bytes)
        .unwrap_or_else(|source| String::from_utf8_lossy(source.as_bytes()).into_owned())
}

fn abort_plan_cancel_if_open_string_masked_source(strategy_source: &str) -> String {
    let source_bytes = strategy_source.as_bytes();
    let mut masked_bytes = source_bytes.to_vec();
    let mut index = ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_ORIGIN;

    while index < source_bytes.len() {
        if let Some(end) = abort_plan_cancel_if_open_raw_string_end(source_bytes, index) {
            abort_plan_cancel_if_open_mask_range(source_bytes, &mut masked_bytes, index, end);
            index = end;
            continue;
        }
        if let Some(end) = abort_plan_cancel_if_open_char_literal_end(source_bytes, index) {
            index = end;
            continue;
        }
        if source_bytes[index] == b'"' {
            index = abort_plan_cancel_if_open_mask_quoted_string(
                source_bytes,
                &mut masked_bytes,
                index,
            );
            continue;
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }

    String::from_utf8(masked_bytes)
        .unwrap_or_else(|source| String::from_utf8_lossy(source.as_bytes()).into_owned())
}

fn abort_plan_cancel_if_open_raw_string_end(source_bytes: &[u8], start: usize) -> Option<usize> {
    if source_bytes.get(start) != Some(&b'r') {
        return None;
    }
    let mut delimiter_index = start + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    while source_bytes.get(delimiter_index) == Some(&b'#') {
        delimiter_index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }
    if source_bytes.get(delimiter_index) != Some(&b'"') {
        return None;
    }

    let hash_count = delimiter_index - start - ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    let mut index = delimiter_index + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    while index < source_bytes.len() {
        let suffix_start = index + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
        if source_bytes[index] == b'"'
            && abort_plan_cancel_if_open_raw_string_hash_suffix_matches(
                source_bytes,
                suffix_start,
                hash_count,
            )
        {
            return Some(suffix_start + hash_count);
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }
    Some(source_bytes.len())
}

fn abort_plan_cancel_if_open_char_literal_end(source_bytes: &[u8], start: usize) -> Option<usize> {
    if source_bytes.get(start) != Some(&b'\'') {
        return None;
    }

    let content_start = start + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    let content_byte = *source_bytes.get(content_start)?;
    if abort_plan_cancel_if_open_lifetime_start_byte(content_byte)
        && source_bytes.get(content_start + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP)
            != Some(&b'\'')
    {
        return None;
    }

    let mut index = content_start;
    let mut escaped = false;
    while index < source_bytes.len() {
        let byte = source_bytes[index];
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
        if abort_plan_cancel_if_open_line_separator_byte(byte) {
            return None;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'\'' && index > start + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP {
            return Some(index);
        }
    }
    None
}

fn abort_plan_cancel_if_open_lifetime_start_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn abort_plan_cancel_if_open_raw_string_hash_suffix_matches(
    source_bytes: &[u8],
    suffix_start: usize,
    hash_count: usize,
) -> bool {
    let suffix_end = suffix_start + hash_count;
    source_bytes
        .get(suffix_start..suffix_end)
        .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
}

fn abort_plan_cancel_if_open_mask_quoted_string(
    source_bytes: &[u8],
    masked_bytes: &mut [u8],
    start: usize,
) -> usize {
    let end = abort_plan_cancel_if_open_quoted_string_end(source_bytes, start);
    abort_plan_cancel_if_open_mask_range(source_bytes, masked_bytes, start, end);
    end
}

fn abort_plan_cancel_if_open_quoted_string_end(source_bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    let mut escaped = false;
    while index < source_bytes.len() {
        let byte = source_bytes[index];
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' && index > start + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP {
            return index;
        }
    }
    index
}

fn abort_plan_cancel_if_open_line_comment_start(source_bytes: &[u8], index: usize) -> bool {
    matches!(
        source_bytes.get(index..index + ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH),
        Some([b'/', b'/'])
    )
}

fn abort_plan_cancel_if_open_block_comment_start(source_bytes: &[u8], index: usize) -> bool {
    matches!(
        source_bytes.get(index..index + ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH),
        Some([b'/', b'*'])
    )
}

fn abort_plan_cancel_if_open_block_comment_end(source_bytes: &[u8], index: usize) -> bool {
    matches!(
        source_bytes.get(index..index + ABORT_PLAN_CANCEL_IF_OPEN_COMMENT_MARKER_WIDTH),
        Some([b'*', b'/'])
    )
}

fn abort_plan_cancel_if_open_line_separator_byte(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r'
}

fn abort_plan_cancel_if_open_mask_pair(masked_bytes: &mut [u8], index: usize) {
    masked_bytes[index] = b' ';
    masked_bytes[index + ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP] = b' ';
}

fn abort_plan_cancel_if_open_mask_range(
    source_bytes: &[u8],
    masked_bytes: &mut [u8],
    start: usize,
    end: usize,
) {
    let mut index = start;
    while index < end {
        if !abort_plan_cancel_if_open_line_separator_byte(source_bytes[index]) {
            masked_bytes[index] = b' ';
        }
        index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }
}

fn abort_plan_cancel_if_open_mask_line_comment(
    source_bytes: &[u8],
    masked_bytes: &mut [u8],
    index: &mut usize,
) {
    let start = *index;
    while *index < source_bytes.len()
        && !abort_plan_cancel_if_open_line_separator_byte(source_bytes[*index])
    {
        *index += ABORT_PLAN_CANCEL_IF_OPEN_SOURCE_INDEX_STEP;
    }
    abort_plan_cancel_if_open_mask_range(source_bytes, masked_bytes, start, *index);
}

fn read_release_manifest_source_file(
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, BoltV3OperatorArtifactError> {
    read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::PreRunReleaseManifestSourceRead {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn release_manifest_utf8<'a>(
    bytes: &'a [u8],
    field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    std::str::from_utf8(bytes)
        .map_err(|_| BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid { field })
}

fn nautilus_revision_from_cargo_toml(
    cargo_toml_text: &str,
) -> Result<String, BoltV3OperatorArtifactError> {
    let value: toml::Value = toml::from_str(cargo_toml_text).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
            field: "cargo_toml",
        }
    })?;
    let mut revisions = Vec::new();
    collect_nautilus_revisions_from_dependency_table(&value, "dependencies", &mut revisions)?;
    collect_nautilus_revisions_from_dependency_table(&value, "dev-dependencies", &mut revisions)?;
    collect_nautilus_revisions_from_dependency_table(&value, "build-dependencies", &mut revisions)?;
    revisions.sort();
    revisions.dedup();
    match revisions.as_slice() {
        [revision] => Ok(revision.clone()),
        _ => Err(
            BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                field: "nautilus_revision",
            },
        ),
    }
}

fn compiled_nautilus_revision_from_build_manifest() -> Result<String, BoltV3OperatorArtifactError> {
    nautilus_revision_from_cargo_toml(BUILD_CARGO_TOML).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
            field: "compiled_nautilus_revision",
        }
    })
}

fn collect_nautilus_revisions_from_dependency_table(
    value: &toml::Value,
    table_name: &'static str,
    revisions: &mut Vec<String>,
) -> Result<(), BoltV3OperatorArtifactError> {
    let Some(table) = value.get(table_name).and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for (name, dependency) in table {
        if !name.starts_with("nautilus-") {
            continue;
        }
        let dependency_table = dependency.as_table().ok_or(
            BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                field: "nautilus_dependency_source",
            },
        )?;
        let git = dependency_table
            .get("git")
            .and_then(toml::Value::as_str)
            .ok_or(
                BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                    field: "nautilus_dependency_source",
                },
            )?;
        if git != NAUTILUS_TRADER_GIT_URL {
            return Err(
                BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                    field: "nautilus_dependency_source",
                },
            );
        }
        let Some(revision) = dependency_table
            .get("rev")
            .and_then(toml::Value::as_str)
            .filter(|value| is_git_head_sha(value))
        else {
            return Err(
                BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                    field: "nautilus_revision",
                },
            );
        };
        revisions.push(revision.to_string());
    }
    Ok(())
}

fn require_cargo_lock_matches_nautilus_revision(
    cargo_lock_text: &str,
    expected_revision: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let value: toml::Value = toml::from_str(cargo_lock_text).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
            field: "cargo_lock",
        }
    })?;
    let Some(packages) = value.get("package").and_then(toml::Value::as_array) else {
        return Err(
            BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                field: "cargo_lock",
            },
        );
    };
    let mut saw_nautilus_source = false;
    for package in packages {
        let Some(package_table) = package.as_table() else {
            continue;
        };
        let Some(name) = package_table.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        if !name.starts_with("nautilus-") {
            continue;
        }
        let source = package_table
            .get("source")
            .and_then(toml::Value::as_str)
            .ok_or(
                BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                    field: "nautilus_revision",
                },
            )?;
        saw_nautilus_source = true;
        if !cargo_lock_source_matches_revision(source, expected_revision) {
            return Err(
                BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                    field: "nautilus_revision",
                },
            );
        }
    }
    if saw_nautilus_source {
        Ok(())
    } else {
        Err(
            BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
                field: "nautilus_revision",
            },
        )
    }
}

fn cargo_lock_source_matches_revision(source: &str, expected_revision: &str) -> bool {
    let Some((source_before_fragment, fragment)) = source.rsplit_once('#') else {
        return false;
    };
    let Some((source_origin, query)) = source_before_fragment.split_once('?') else {
        return false;
    };
    fragment == expected_revision
        && source_origin == NAUTILUS_TRADER_CARGO_LOCK_SOURCE_PREFIX
        && query
            .split('&')
            .any(|part| part.strip_prefix("rev=") == Some(expected_revision))
}

fn clob_domain_version_from_source(source: &str) -> Result<String, BoltV3OperatorArtifactError> {
    for line in source.lines().map(str::trim) {
        let Some(after_const) = line.strip_prefix("const DOMAIN_VERSION") else {
            continue;
        };
        let Some(after_type_marker) = after_const.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(after_equals) = after_type_marker
            .split_once('=')
            .map(|(_, value)| value.trim())
        else {
            continue;
        };
        let Some(after_open_quote) = after_equals.strip_prefix('"') else {
            continue;
        };
        let Some((version, _)) = after_open_quote.split_once('"') else {
            continue;
        };
        let version = version.trim();
        if version.is_empty() {
            break;
        }
        return Ok(version.to_string());
    }
    Err(
        BoltV3OperatorArtifactError::PreRunReleaseManifestSourceInvalid {
            field: stringify!(clob_signing_version),
        },
    )
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn write_static_operator_artifacts(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    output_dir: &Path,
) -> Result<BoltV3StaticArtifactsWriteOutcome, BoltV3OperatorArtifactError> {
    let mut blockers = Vec::new();
    let (mut generated_artifacts, mut written_artifacts) =
        write_base_static_operator_artifact_refs(loaded, strategy_instance_id, output_dir)?;

    blockers.push(MARKET_SELECTION_SOURCE_BLOCKER);

    match write_strategy_input_evidence_artifact(
        loaded,
        strategy_instance_id,
        &output_dir.join(STRATEGY_INPUT_FILE_NAME),
    ) {
        Ok(written) => {
            written_artifacts.push(written.clone());
            generated_artifacts.push(static_artifact_ref(STRATEGY_INPUT_ARTIFACT_NAME, written))
        }
        Err(BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven { prerequisite }) => {
            blockers.push(prerequisite);
        }
        Err(error) => {
            remove_written_static_artifacts(&written_artifacts);
            return Err(error);
        }
    }

    match write_pre_run_state_artifact(
        loaded,
        strategy_instance_id,
        &output_dir.join(PRE_RUN_STATE_FILE_NAME),
    ) {
        Ok(written) => {
            written_artifacts.push(written.clone());
            generated_artifacts.push(static_artifact_ref(PRE_RUN_STATE_ARTIFACT_NAME, written))
        }
        Err(BoltV3OperatorArtifactError::PreRunStatePrerequisiteUnproven { prerequisite }) => {
            blockers.push(prerequisite);
        }
        Err(error) => {
            remove_written_static_artifacts(&written_artifacts);
            return Err(error);
        }
    }

    match write_abort_plan_artifact(
        loaded,
        strategy_instance_id,
        &output_dir.join(ABORT_PLAN_FILE_NAME),
    ) {
        Ok(written) => {
            written_artifacts.push(written.clone());
            generated_artifacts.push(static_artifact_ref(ABORT_PLAN_ARTIFACT_NAME, written))
        }
        Err(BoltV3OperatorArtifactError::AbortPrerequisiteUnproven { prerequisite }) => {
            blockers.push(prerequisite);
        }
        Err(error) => {
            remove_written_static_artifacts(&written_artifacts);
            return Err(error);
        }
    }

    let outcome_blockers = blockers.clone();
    let manifest = BoltV3StaticArtifactsManifest {
        schema_version: STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION,
        record_kind: STATIC_ARTIFACTS_MANIFEST_RECORD_KIND,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        generated_artifacts,
        blockers,
    };
    let manifest_written = match write_json_artifact_create_new(
        &output_dir.join(STATIC_ARTIFACTS_MANIFEST_FILE_NAME),
        &manifest,
    ) {
        Ok(written) => written,
        Err(error) => {
            remove_written_static_artifacts(&written_artifacts);
            return Err(error);
        }
    };

    Ok(BoltV3StaticArtifactsWriteOutcome {
        command_summary: BoltV3StaticArtifactsCommandSummary {
            generated_artifacts: manifest
                .generated_artifacts
                .iter()
                .map(static_artifact_summary_ref)
                .collect(),
            manifest_artifact: written_artifact_summary_ref(manifest_written),
        },
        blockers: outcome_blockers,
    })
}

pub fn write_static_artifacts_manifest_from_operator_evidence(
    loaded: &LoadedBoltV3Config,
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingOperatorEvidence)?;
    let max_bytes = operator_evidence.max_operator_evidence_file_bytes;
    let generated_artifacts = vec![
        static_artifact_ref_from_operator_evidence(
            loaded,
            SSM_MANIFEST_ARTIFACT_NAME,
            &operator_evidence.ssm_manifest_path,
            &operator_evidence.ssm_manifest_sha256,
            "ssm_manifest_sha256",
            max_bytes,
        )?,
        static_artifact_ref_from_operator_evidence(
            loaded,
            STRATEGY_INPUT_ARTIFACT_NAME,
            &operator_evidence.strategy_input_evidence_path,
            &operator_evidence.strategy_input_evidence_sha256,
            "strategy_input_evidence_sha256",
            max_bytes,
        )?,
        static_artifact_ref_from_operator_evidence(
            loaded,
            FINANCIAL_ENVELOPE_ARTIFACT_NAME,
            &operator_evidence.financial_envelope_path,
            &operator_evidence.financial_envelope_sha256,
            "financial_envelope_sha256",
            max_bytes,
        )?,
        static_artifact_ref_from_operator_evidence(
            loaded,
            PRE_RUN_STATE_ARTIFACT_NAME,
            &operator_evidence.pre_run_state_path,
            &operator_evidence.pre_run_state_sha256,
            "pre_run_state_sha256",
            max_bytes,
        )?,
        static_artifact_ref_from_operator_evidence(
            loaded,
            ABORT_PLAN_ARTIFACT_NAME,
            &operator_evidence.abort_plan_path,
            &operator_evidence.abort_plan_sha256,
            "abort_plan_sha256",
            max_bytes,
        )?,
        static_artifact_ref_from_operator_evidence(
            loaded,
            APPROVAL_NONCE_ARTIFACT_NAME,
            &operator_evidence.approval_nonce_path,
            &operator_evidence.approval_nonce_sha256,
            "approval_nonce_sha256",
            max_bytes,
        )?,
    ];
    let manifest = BoltV3StaticArtifactsManifest {
        schema_version: STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION,
        record_kind: STATIC_ARTIFACTS_MANIFEST_RECORD_KIND,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        generated_artifacts,
        blockers: Vec::new(),
    };
    write_json_artifact_create_new(path, &manifest)
}

pub fn write_operator_evidence_json_from_artifact_paths(
    loaded: &LoadedBoltV3Config,
    inputs: OperatorEvidenceJsonBuildInputs<'_>,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let head_sha = current_build_head_sha()
        .ok_or(BoltV3OperatorArtifactError::BuildHeadShaUnavailable)?
        .to_string();
    let max_bytes = inputs.max_operator_evidence_file_bytes;
    let mut operator_evidence = LiveCanaryOperatorEvidenceBlock {
        head_sha,
        max_operator_evidence_file_bytes: max_bytes,
        approval_consumption_max_age_seconds: inputs.approval_consumption_max_age_seconds,
        approval_envelope_path: operator_evidence_path_string(inputs.approval_envelope_path),
        approval_envelope_sha256: String::new(),
        ssm_manifest_path: operator_evidence_path_string(inputs.ssm_manifest_path),
        ssm_manifest_sha256: operator_evidence_artifact_sha256(
            loaded,
            SSM_MANIFEST_ARTIFACT_NAME,
            "ssm_manifest_path",
            inputs.ssm_manifest_path,
            max_bytes,
        )?,
        strategy_input_evidence_path: operator_evidence_path_string(
            inputs.strategy_input_evidence_path,
        ),
        strategy_input_evidence_sha256: operator_evidence_artifact_sha256(
            loaded,
            STRATEGY_INPUT_ARTIFACT_NAME,
            "strategy_input_evidence_path",
            inputs.strategy_input_evidence_path,
            max_bytes,
        )?,
        gate_session_path: Some(operator_evidence_path_string(inputs.gate_session_path)),
        expected_gate_session_sha256: Some(inputs.expected_gate_session_sha256.to_string()),
        financial_envelope_path: operator_evidence_path_string(inputs.financial_envelope_path),
        financial_envelope_sha256: operator_evidence_artifact_sha256(
            loaded,
            FINANCIAL_ENVELOPE_ARTIFACT_NAME,
            "financial_envelope_path",
            inputs.financial_envelope_path,
            max_bytes,
        )?,
        pre_run_state_path: operator_evidence_path_string(inputs.pre_run_state_path),
        pre_run_state_sha256: operator_evidence_artifact_sha256(
            loaded,
            PRE_RUN_STATE_ARTIFACT_NAME,
            "pre_run_state_path",
            inputs.pre_run_state_path,
            max_bytes,
        )?,
        abort_plan_path: operator_evidence_path_string(inputs.abort_plan_path),
        abort_plan_sha256: operator_evidence_artifact_sha256(
            loaded,
            ABORT_PLAN_ARTIFACT_NAME,
            "abort_plan_path",
            inputs.abort_plan_path,
            max_bytes,
        )?,
        canary_proof_candidate_source_path: inputs
            .canary_proof_candidate_source_path
            .map(operator_evidence_path_string),
        canary_proof_candidate_source_sha256: optional_operator_evidence_file_sha256(
            loaded,
            "canary_proof_candidate_source_path",
            inputs.canary_proof_candidate_source_path,
            max_bytes,
        )?,
        canary_proof_order_intent_path: inputs
            .canary_proof_order_intent_path
            .map(operator_evidence_path_string),
        canary_proof_order_intent_sha256: optional_operator_evidence_file_sha256(
            loaded,
            "canary_proof_order_intent_path",
            inputs.canary_proof_order_intent_path,
            max_bytes,
        )?,
        // Seal the no-submit readiness-report file hash so the live gate can
        // re-check the report content against the operator-approved value. The
        // report path and its size bound live in `[live_canary]`, not in the
        // operator-evidence block, so they are sourced from `live_canary`.
        no_submit_readiness_report_sha256: Some(
            operator_evidence_no_submit_readiness_report_sha256(
                loaded,
                &live_canary.no_submit_readiness_report_path,
                live_canary.max_no_submit_readiness_report_bytes,
            )?,
        ),
        canary_evidence_path: operator_evidence_path_string(inputs.canary_evidence_path),
        approval_not_before_unix_seconds: inputs.approval_not_before_unix_seconds,
        approval_not_after_unix_seconds: inputs.approval_not_after_unix_seconds,
        approval_nonce_path: operator_evidence_path_string(inputs.approval_nonce_path),
        approval_nonce_sha256: operator_evidence_artifact_sha256(
            loaded,
            APPROVAL_NONCE_ARTIFACT_NAME,
            "approval_nonce_path",
            inputs.approval_nonce_path,
            max_bytes,
        )?,
        approval_consumption_path: operator_evidence_path_string(inputs.approval_consumption_path),
        decision_evidence_path: operator_evidence_path_string(inputs.decision_evidence_path),
        nt_submit_event_path: operator_evidence_path_string(inputs.nt_submit_event_path),
        venue_order_state_path: operator_evidence_path_string(inputs.venue_order_state_path),
        strategy_cancel_path: inputs
            .strategy_cancel_path
            .map(operator_evidence_path_string),
        restart_reconciliation_path: operator_evidence_path_string(
            inputs.restart_reconciliation_path,
        ),
        post_run_hygiene_path: operator_evidence_path_string(inputs.post_run_hygiene_path),
    };
    operator_evidence.approval_envelope_sha256 =
        json_artifact_sha256(&approval_envelope_from_operator_evidence(
            &operator_evidence,
            live_canary.approval_id.as_str(),
        ))?;
    validate_live_canary_operator_evidence_toml_patch(&operator_evidence)?;
    validate_operator_evidence_static_artifacts_materialized_for_toml_patch(
        &loaded.root_path,
        &operator_evidence,
    )?;
    write_json_artifact_create_new(output_path, &operator_evidence)
}

fn operator_evidence_artifact_sha256(
    loaded: &LoadedBoltV3Config,
    name: &'static str,
    field: &'static str,
    configured_path: &Path,
    max_bytes: u64,
) -> Result<String, BoltV3OperatorArtifactError> {
    let configured_path_string = operator_evidence_path_string(configured_path);
    validate_operator_evidence_toml_path(field, &configured_path_string)?;
    let resolved_path = resolve_loaded_config_path_from_path(loaded, configured_path);
    sha256_file_for_static_manifest(name, &resolved_path, max_bytes)
}

fn optional_operator_evidence_file_sha256(
    loaded: &LoadedBoltV3Config,
    field: &'static str,
    configured_path: Option<&Path>,
    max_bytes: u64,
) -> Result<Option<String>, BoltV3OperatorArtifactError> {
    let Some(configured_path) = configured_path else {
        return Ok(None);
    };
    let configured_path_string = operator_evidence_path_string(configured_path);
    validate_operator_evidence_toml_path(field, &configured_path_string)?;
    let resolved_path = resolve_loaded_config_path_from_path(loaded, configured_path);
    let bytes = read_file_bounded(&resolved_path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::FinalEvidenceRead {
            field,
            path: resolved_path,
            source,
        }
    })?;
    Ok(Some(hex::encode(Sha256::digest(&bytes))))
}

/// SHA-256 of the no-submit readiness-report file the gate seals into the
/// operator-approval envelope. The report path and its size bound live in
/// `[live_canary]` (not the operator-evidence block), so this helper resolves
/// the configured path against the loaded config root exactly as the gate does
/// and hashes the file bounded by `max_no_submit_readiness_report_bytes`.
fn operator_evidence_no_submit_readiness_report_sha256(
    loaded: &LoadedBoltV3Config,
    configured_path: &str,
    max_bytes: u64,
) -> Result<String, BoltV3OperatorArtifactError> {
    validate_operator_evidence_toml_path("no_submit_readiness_report_path", configured_path)?;
    let resolved_path = resolve_loaded_config_path(loaded, configured_path);
    let bytes = read_file_bounded(&resolved_path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::FinalEvidenceRead {
            field: "no_submit_readiness_report_path",
            path: resolved_path,
            source,
        }
    })?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn operator_evidence_path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub fn update_live_canary_operator_evidence_toml_from_json_file(
    config_path: &Path,
    operator_evidence_json_path: &Path,
    max_operator_evidence_json_bytes: u64,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    if max_operator_evidence_json_bytes == 0 {
        return Err(BoltV3OperatorArtifactError::OperatorEvidenceTomlInvalid {
            field: "max_operator_evidence_json_bytes",
        });
    }
    let operator_evidence_bytes = read_file_bounded(
        operator_evidence_json_path,
        max_operator_evidence_json_bytes,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::OperatorEvidenceJsonRead {
            path: operator_evidence_json_path.to_path_buf(),
            source,
        },
    )?;
    let operator_evidence: LiveCanaryOperatorEvidenceBlock =
        serde_json::from_slice(&operator_evidence_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::OperatorEvidenceJsonParse {
                path: operator_evidence_json_path.to_path_buf(),
                source,
            }
        })?;
    validate_live_canary_operator_evidence_toml_patch(&operator_evidence)?;
    validate_operator_evidence_static_artifacts_materialized_for_toml_patch(
        config_path,
        &operator_evidence,
    )?;

    let root_text = fs::read_to_string(config_path).map_err(|source| {
        BoltV3OperatorArtifactError::OperatorEvidenceTomlRead {
            path: config_path.to_path_buf(),
            source,
        }
    })?;
    let patched_text = patch_live_canary_operator_evidence_toml(&root_text, &operator_evidence)?;
    let parsed: BoltV3RootConfig = toml::from_str(&patched_text)
        .map_err(|source| BoltV3OperatorArtifactError::OperatorEvidenceTomlParse { source })?;
    let patched_operator_evidence = parsed
        .live_canary
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?
        .operator_evidence
        .ok_or(BoltV3OperatorArtifactError::MissingOperatorEvidence)?;
    if patched_operator_evidence != operator_evidence {
        return Err(BoltV3OperatorArtifactError::OperatorEvidenceTomlInvalid {
            field: "live_canary.operator_evidence",
        });
    }

    fs::write(config_path, patched_text.as_bytes()).map_err(|source| {
        BoltV3OperatorArtifactError::OperatorEvidenceTomlWrite {
            path: config_path.to_path_buf(),
            source,
        }
    })?;
    Ok(WrittenOperatorArtifact {
        path: config_path.to_path_buf(),
        sha256: hex::encode(Sha256::digest(patched_text.as_bytes())),
    })
}

fn validate_operator_evidence_static_artifacts_materialized_for_toml_patch(
    config_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    for (name, configured_path, configured_sha256, configured_sha256_field) in [
        (
            SSM_MANIFEST_ARTIFACT_NAME,
            evidence.ssm_manifest_path.as_str(),
            evidence.ssm_manifest_sha256.as_str(),
            "ssm_manifest_sha256",
        ),
        (
            STRATEGY_INPUT_ARTIFACT_NAME,
            evidence.strategy_input_evidence_path.as_str(),
            evidence.strategy_input_evidence_sha256.as_str(),
            "strategy_input_evidence_sha256",
        ),
        (
            FINANCIAL_ENVELOPE_ARTIFACT_NAME,
            evidence.financial_envelope_path.as_str(),
            evidence.financial_envelope_sha256.as_str(),
            "financial_envelope_sha256",
        ),
        (
            PRE_RUN_STATE_ARTIFACT_NAME,
            evidence.pre_run_state_path.as_str(),
            evidence.pre_run_state_sha256.as_str(),
            "pre_run_state_sha256",
        ),
        (
            ABORT_PLAN_ARTIFACT_NAME,
            evidence.abort_plan_path.as_str(),
            evidence.abort_plan_sha256.as_str(),
            "abort_plan_sha256",
        ),
        (
            APPROVAL_NONCE_ARTIFACT_NAME,
            evidence.approval_nonce_path.as_str(),
            evidence.approval_nonce_sha256.as_str(),
            "approval_nonce_sha256",
        ),
    ] {
        validate_operator_evidence_sha256(configured_sha256_field, configured_sha256)?;
        let resolved_path = resolve_config_path_from_config_path(config_path, configured_path);
        let actual_sha256 = sha256_file_for_static_manifest(
            name,
            &resolved_path,
            evidence.max_operator_evidence_file_bytes,
        )?;
        if actual_sha256 != configured_sha256 {
            return Err(
                BoltV3OperatorArtifactError::StaticManifestArtifactFileHashMismatch {
                    name,
                    path: resolved_path,
                },
            );
        }
    }
    validate_operator_evidence_gate_session_file(config_path, evidence)?;
    Ok(())
}

fn validate_operator_evidence_gate_session_file(
    config_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    let gate_session_path = required_operator_evidence_field(
        OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
        evidence.gate_session_path.as_deref(),
    )?;
    let expected_gate_session_sha256 = required_operator_evidence_field(
        OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
        evidence.expected_gate_session_sha256.as_deref(),
    )?;
    validate_operator_evidence_toml_path(
        OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
        gate_session_path,
    )?;
    validate_operator_evidence_sha256(
        OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
        expected_gate_session_sha256,
    )?;
    let resolved_path = resolve_config_path_from_config_path(config_path, gate_session_path);
    let actual_sha256 = sha256_file_for_static_manifest(
        GATE_SESSION_ARTIFACT_NAME,
        &resolved_path,
        evidence.max_operator_evidence_file_bytes,
    )?;
    if actual_sha256 != expected_gate_session_sha256 {
        return Err(
            BoltV3OperatorArtifactError::StaticManifestArtifactFileHashMismatch {
                name: GATE_SESSION_ARTIFACT_NAME,
                path: resolved_path,
            },
        );
    }
    Ok(())
}

fn resolve_config_path_from_config_path(config_path: &Path, configured_path: &str) -> PathBuf {
    let path = Path::new(configured_path.trim());
    if path.is_absolute() {
        return normalize_path_components(path);
    }
    normalize_path_components(
        &config_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path),
    )
}

fn static_artifact_ref_from_operator_evidence(
    loaded: &LoadedBoltV3Config,
    name: &'static str,
    configured_path: &str,
    configured_sha256: &str,
    configured_sha256_field: &'static str,
    max_bytes: u64,
) -> Result<BoltV3StaticArtifactRef, BoltV3OperatorArtifactError> {
    validate_operator_evidence_sha256(configured_sha256_field, configured_sha256)?;
    let resolved_path = resolve_loaded_config_path(loaded, configured_path);
    let actual = sha256_file_for_static_manifest(name, &resolved_path, max_bytes)?;
    if actual != configured_sha256 {
        return Err(
            BoltV3OperatorArtifactError::StaticManifestArtifactFileHashMismatch {
                name,
                path: resolved_path,
            },
        );
    }
    Ok(BoltV3StaticArtifactRef {
        name,
        path: configured_path.to_string(),
        sha256: configured_sha256.to_string(),
    })
}

pub fn write_base_static_operator_artifacts(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    output_dir: &Path,
) -> Result<BoltV3BaseStaticArtifactsWriteOutcome, BoltV3OperatorArtifactError> {
    let (generated_artifacts, _written_artifacts) =
        write_base_static_operator_artifact_refs(loaded, strategy_instance_id, output_dir)?;
    Ok(BoltV3BaseStaticArtifactsWriteOutcome {
        command_summary: BoltV3BaseStaticArtifactsCommandSummary {
            generated_artifacts: generated_artifacts
                .iter()
                .map(static_artifact_summary_ref)
                .collect(),
        },
    })
}

fn write_base_static_operator_artifact_refs(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    output_dir: &Path,
) -> Result<(Vec<BoltV3StaticArtifactRef>, Vec<WrittenOperatorArtifact>), BoltV3OperatorArtifactError>
{
    let mut generated_artifacts = Vec::new();
    let mut written_artifacts = Vec::new();

    let ssm_manifest = build_redacted_ssm_manifest(loaded)?;
    let financial_envelope = build_phase8_financial_envelope(loaded, strategy_instance_id)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let approval_nonce = build_approval_nonce_artifact()?;

    let ssm_manifest_written =
        write_json_artifact_create_new(&output_dir.join(SSM_MANIFEST_FILE_NAME), &ssm_manifest)?;
    written_artifacts.push(ssm_manifest_written.clone());
    generated_artifacts.push(static_artifact_ref(
        SSM_MANIFEST_ARTIFACT_NAME,
        ssm_manifest_written,
    ));

    let financial_envelope_written = match write_json_artifact_create_new(
        &output_dir.join(FINANCIAL_ENVELOPE_FILE_NAME),
        &financial_envelope,
    ) {
        Ok(written) => written,
        Err(error) => {
            remove_written_static_artifacts(&written_artifacts);
            return Err(error);
        }
    };
    written_artifacts.push(financial_envelope_written.clone());
    generated_artifacts.push(static_artifact_ref(
        FINANCIAL_ENVELOPE_ARTIFACT_NAME,
        financial_envelope_written,
    ));

    let approval_nonce_written = match write_json_artifact_create_new(
        &output_dir.join(APPROVAL_NONCE_FILE_NAME),
        &approval_nonce,
    ) {
        Ok(written) => written,
        Err(error) => {
            remove_written_static_artifacts(&written_artifacts);
            return Err(error);
        }
    };
    written_artifacts.push(approval_nonce_written.clone());
    generated_artifacts.push(static_artifact_ref(
        APPROVAL_NONCE_ARTIFACT_NAME,
        approval_nonce_written,
    ));

    Ok((generated_artifacts, written_artifacts))
}

fn remove_written_static_artifacts(written_artifacts: &[WrittenOperatorArtifact]) {
    for artifact in written_artifacts.iter().rev() {
        let _ = fs::remove_file(&artifact.path);
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3StaticArtifactsManifestInput {
    schema_version: u32,
    record_kind: String,
    config_bundle_checksum: String,
    generated_artifacts: Vec<BoltV3StaticArtifactRefInput>,
    blockers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3StaticArtifactRefInput {
    name: String,
    path: String,
    sha256: String,
}

struct ParsedStaticManifest {
    manifest: BoltV3StaticArtifactsManifestInput,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3OperatorEvidencePacketInput {
    schema_version: u32,
    record_kind: String,
    config_bundle_checksum: String,
    static_manifest_path: String,
    static_manifest_sha256: String,
    live_canary_operator_evidence: BoltV3OperatorEvidencePacketBlockInput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3OperatorEvidencePacketBlockInput {
    head_sha: String,
    approval_envelope_path: String,
    approval_envelope_sha256: String,
    ssm_manifest_path: String,
    ssm_manifest_sha256: String,
    strategy_input_evidence_path: String,
    strategy_input_evidence_sha256: String,
    gate_session_path: String,
    expected_gate_session_sha256: String,
    financial_envelope_path: String,
    financial_envelope_sha256: String,
    pre_run_state_path: String,
    pre_run_state_sha256: String,
    abort_plan_path: String,
    abort_plan_sha256: String,
    canary_proof_candidate_source_path: Option<String>,
    canary_proof_candidate_source_sha256: Option<String>,
    canary_proof_order_intent_path: Option<String>,
    canary_proof_order_intent_sha256: Option<String>,
    canary_evidence_path: String,
    approval_nonce_path: String,
    approval_nonce_sha256: String,
    approval_consumption_path: String,
    decision_evidence_path: String,
    nt_submit_event_path: String,
    venue_order_state_path: String,
    strategy_cancel_path: Option<String>,
    restart_reconciliation_path: String,
    post_run_hygiene_path: String,
}

pub fn assemble_operator_packet_from_static_manifest(
    loaded: &LoadedBoltV3Config,
    static_manifest_path: &Path,
    operator_packet_path: &Path,
) -> Result<BoltV3OperatorPacketAssemblyOutcome, BoltV3OperatorArtifactError> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingOperatorEvidence)?;
    let static_manifest_path = resolve_loaded_config_path_from_path(loaded, static_manifest_path);
    let parsed_static_manifest = read_static_manifest(
        &static_manifest_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    let static_manifest = &parsed_static_manifest.manifest;

    validate_static_manifest_header(loaded, static_manifest)?;
    if !static_manifest.blockers.is_empty() {
        return Err(BoltV3OperatorArtifactError::StaticManifestBlockers {
            count: static_manifest.blockers.len(),
        });
    }

    validate_required_operator_evidence_static_artifacts(
        loaded,
        static_manifest,
        operator_evidence,
    )?;
    validate_operator_evidence_gate_session_file(&loaded.root_path, operator_evidence)?;

    let approval_envelope = approval_envelope_from_operator_evidence(
        operator_evidence,
        live_canary.approval_id.as_str(),
    );
    let approval_envelope_sha256 = json_artifact_sha256(&approval_envelope)?;
    let operator_packet = operator_evidence_packet(
        loaded,
        &static_manifest_path,
        parsed_static_manifest.sha256.as_str(),
        operator_evidence,
        approval_envelope_sha256.clone(),
    )?;

    validate_output_path_shape(
        "approval_envelope_path",
        &operator_evidence.approval_envelope_path,
    )?;
    validate_output_path_components("operator_packet_path", operator_packet_path)?;
    let approval_envelope_path =
        resolve_loaded_config_path(loaded, &operator_evidence.approval_envelope_path);
    let operator_packet_path = resolve_loaded_config_path_from_path(loaded, operator_packet_path);
    validate_output_parent("approval_envelope_path", &approval_envelope_path)?;
    validate_output_parent("operator_packet_path", &operator_packet_path)?;
    if output_paths_collide(&approval_envelope_path, &operator_packet_path) {
        return Err(BoltV3OperatorArtifactError::OutputPathCollision);
    }
    ensure_output_path_absent(&approval_envelope_path)?;
    ensure_output_path_absent(&operator_packet_path)?;

    if operator_evidence.approval_envelope_sha256 != approval_envelope_sha256 {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "approval_envelope_sha256",
        });
    }

    let approval_envelope_written =
        write_json_artifact_create_new(&approval_envelope_path, &approval_envelope)?;
    debug_assert_eq!(approval_envelope_written.sha256, approval_envelope_sha256);
    let operator_packet_written =
        match write_json_artifact_create_new(&operator_packet_path, &operator_packet) {
            Ok(written) => written,
            Err(error) => {
                let _ = fs::remove_file(&approval_envelope_path);
                return Err(error);
            }
        };
    let static_manifest_written = WrittenOperatorArtifact {
        path: static_manifest_path,
        sha256: parsed_static_manifest.sha256,
    };

    Ok(BoltV3OperatorPacketAssemblyOutcome {
        approval_envelope: approval_envelope_written,
        operator_packet: operator_packet_written,
        static_manifest: static_manifest_written,
    })
}

pub fn compute_operator_approval_envelope_sha256(
    loaded: &LoadedBoltV3Config,
) -> Result<String, BoltV3OperatorArtifactError> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingOperatorEvidence)?;
    let approval_envelope = approval_envelope_from_operator_evidence(
        operator_evidence,
        live_canary.approval_id.as_str(),
    );

    json_artifact_sha256(&approval_envelope)
}

pub fn verify_source_owned_reference_readiness_from_operator_evidence(
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3OperatorArtifactError> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingOperatorEvidence)?;
    validate_operator_evidence_build_head(operator_evidence)?;
    validate_operator_evidence_gate_session_file(&loaded.root_path, operator_evidence)?;
    verify_source_owned_static_readiness_artifacts(loaded, operator_evidence)?;
    if live_canary_proof_policy_enabled(loaded) {
        verify_canary_proof_operator_evidence(loaded, operator_evidence)
    } else {
        verify_strategy_input_replay_binding(loaded, operator_evidence)
    }
}

pub fn verify_final_operator_packet(
    loaded: &LoadedBoltV3Config,
    operator_packet_path: &Path,
) -> Result<BoltV3FinalOperatorPacketVerification, BoltV3OperatorArtifactError> {
    verify_final_operator_packet_with_scope(
        loaded,
        operator_packet_path,
        FinalOperatorPacketVerificationScope::PostRun,
    )
}

pub fn verify_final_operator_packet_with_scope(
    loaded: &LoadedBoltV3Config,
    operator_packet_path: &Path,
    scope: FinalOperatorPacketVerificationScope,
) -> Result<BoltV3FinalOperatorPacketVerification, BoltV3OperatorArtifactError> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingLiveCanary)?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::MissingOperatorEvidence)?;
    validate_operator_evidence_build_head(operator_evidence)?;
    validate_operator_evidence_gate_session_file(&loaded.root_path, operator_evidence)?;

    let operator_packet_path = resolve_loaded_config_path_from_path(loaded, operator_packet_path);
    let operator_packet_bytes = read_file_bounded(
        &operator_packet_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3OperatorArtifactError::OperatorPacketRead {
        path: operator_packet_path.clone(),
        source,
    })?;
    let operator_packet_sha256 = hex::encode(Sha256::digest(&operator_packet_bytes));
    let operator_packet: BoltV3OperatorEvidencePacketInput =
        serde_json::from_slice(&operator_packet_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::OperatorPacketParse {
                path: operator_packet_path.clone(),
                source,
            }
        })?;

    validate_operator_packet_header(loaded, &operator_packet)?;
    validate_operator_packet_evidence_block(
        operator_evidence,
        &operator_packet.live_canary_operator_evidence,
    )?;

    validate_packet_sha256_field(
        "static_manifest_sha256",
        &operator_packet.static_manifest_sha256,
    )?;
    let static_manifest_path =
        resolve_loaded_config_path(loaded, &operator_packet.static_manifest_path);
    let parsed_static_manifest = read_static_manifest(
        &static_manifest_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    if parsed_static_manifest.sha256 != operator_packet.static_manifest_sha256 {
        return Err(BoltV3OperatorArtifactError::OperatorPacketStaticManifestHashMismatch);
    }
    let static_manifest = &parsed_static_manifest.manifest;

    validate_static_manifest_header(loaded, static_manifest)?;
    if !static_manifest.blockers.is_empty() {
        return Err(BoltV3OperatorArtifactError::StaticManifestBlockers {
            count: static_manifest.blockers.len(),
        });
    }
    validate_required_operator_evidence_static_artifacts(
        loaded,
        static_manifest,
        operator_evidence,
    )?;
    verify_source_owned_static_readiness_artifacts(loaded, operator_evidence)?;
    if !live_canary_proof_policy_enabled(loaded) {
        verify_strategy_input_replay_binding(loaded, operator_evidence)?;
    }
    verify_canary_proof_operator_evidence(loaded, operator_evidence)?;
    let approval_envelope = verify_operator_approval_envelope(
        loaded,
        operator_evidence,
        live_canary.approval_id.as_str(),
    )?;
    if scope == FinalOperatorPacketVerificationScope::PostRun {
        verify_final_live_evidence_files(
            loaded,
            operator_evidence,
            live_canary.approval_id.as_str(),
            approval_envelope.sha256.as_str(),
            live_canary.max_live_order_count,
            live_canary.max_notional_per_order.to_string().as_str(),
        )?;
    }

    Ok(BoltV3FinalOperatorPacketVerification {
        approval_envelope,
        operator_packet: WrittenOperatorArtifact {
            path: operator_packet_path,
            sha256: operator_packet_sha256,
        },
        static_manifest: WrittenOperatorArtifact {
            path: static_manifest_path,
            sha256: parsed_static_manifest.sha256,
        },
    })
}

fn live_canary_proof_policy_enabled(loaded: &LoadedBoltV3Config) -> bool {
    loaded
        .root
        .live_canary
        .as_ref()
        .and_then(|live_canary| live_canary.proof_policy.as_ref())
        .is_some_and(|proof_policy| proof_policy.enabled)
}

fn read_static_manifest(
    path: &Path,
    max_bytes: u64,
) -> Result<ParsedStaticManifest, BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::StaticManifestRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let manifest = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::StaticManifestParse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(ParsedStaticManifest { manifest, sha256 })
}

fn validate_static_manifest_header(
    loaded: &LoadedBoltV3Config,
    manifest: &BoltV3StaticArtifactsManifestInput,
) -> Result<(), BoltV3OperatorArtifactError> {
    if manifest.schema_version != STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::StaticManifestSchema {
            field: "schema_version",
        });
    }
    if manifest.record_kind != STATIC_ARTIFACTS_MANIFEST_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::StaticManifestSchema {
            field: "record_kind",
        });
    }
    if manifest.config_bundle_checksum != loaded.config_bundle_checksum {
        return Err(BoltV3OperatorArtifactError::StaticManifestConfigBundleDrift);
    }
    Ok(())
}

fn validate_required_operator_evidence_static_artifacts(
    loaded: &LoadedBoltV3Config,
    static_manifest: &BoltV3StaticArtifactsManifestInput,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        SSM_MANIFEST_ARTIFACT_NAME,
        &operator_evidence.ssm_manifest_path,
        &operator_evidence.ssm_manifest_sha256,
        "ssm_manifest_sha256",
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        STRATEGY_INPUT_ARTIFACT_NAME,
        &operator_evidence.strategy_input_evidence_path,
        &operator_evidence.strategy_input_evidence_sha256,
        "strategy_input_evidence_sha256",
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        FINANCIAL_ENVELOPE_ARTIFACT_NAME,
        &operator_evidence.financial_envelope_path,
        &operator_evidence.financial_envelope_sha256,
        "financial_envelope_sha256",
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        PRE_RUN_STATE_ARTIFACT_NAME,
        &operator_evidence.pre_run_state_path,
        &operator_evidence.pre_run_state_sha256,
        "pre_run_state_sha256",
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        ABORT_PLAN_ARTIFACT_NAME,
        &operator_evidence.abort_plan_path,
        &operator_evidence.abort_plan_sha256,
        "abort_plan_sha256",
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        APPROVAL_NONCE_ARTIFACT_NAME,
        &operator_evidence.approval_nonce_path,
        &operator_evidence.approval_nonce_sha256,
        "approval_nonce_sha256",
        operator_evidence.max_operator_evidence_file_bytes,
    )
}

fn verify_source_owned_static_readiness_artifacts(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    let financial_envelope: Phase8FinancialEnvelopeEvidenceFile =
        read_operator_evidence_json_artifact(
            loaded,
            operator_evidence,
            "financial_envelope_path",
            "financial_envelope_sha256",
            &operator_evidence.financial_envelope_path,
            &operator_evidence.financial_envelope_sha256,
        )?;
    let expected_financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(
            loaded,
            financial_envelope.strategy_instance_id(),
        )
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    financial_envelope
        .validate_matches(&expected_financial_envelope)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;

    let pre_run_state: Phase8PreRunStateEvidenceFile = read_operator_evidence_json_artifact(
        loaded,
        operator_evidence,
        "pre_run_state_path",
        "pre_run_state_sha256",
        &operator_evidence.pre_run_state_path,
        &operator_evidence.pre_run_state_sha256,
    )?;
    pre_run_state
        .validate_matches_loaded(&expected_financial_envelope)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;

    let abort_plan: Phase8AbortPlanEvidenceFile = read_operator_evidence_json_artifact(
        loaded,
        operator_evidence,
        "abort_plan_path",
        "abort_plan_sha256",
        &operator_evidence.abort_plan_path,
        &operator_evidence.abort_plan_sha256,
    )?;
    abort_plan
        .validate_collector_derived_matches_loaded(&expected_financial_envelope)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)
}

fn verify_canary_proof_operator_evidence(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    let Some(live_canary) = loaded.root.live_canary.as_ref() else {
        return Err(BoltV3OperatorArtifactError::MissingLiveCanary);
    };
    let Some(proof_policy) = live_canary.proof_policy.as_ref() else {
        return Ok(());
    };
    let proof_policy_enabled = proof_policy.enabled;
    if !proof_policy_enabled {
        return Ok(());
    }

    let order_intent_path = required_operator_evidence_field(
        "canary_proof_order_intent_path",
        operator_evidence.canary_proof_order_intent_path.as_deref(),
    )?;
    let order_intent_sha256 = required_operator_evidence_field(
        "canary_proof_order_intent_sha256",
        operator_evidence
            .canary_proof_order_intent_sha256
            .as_deref(),
    )?;
    let candidate_source_path = required_operator_evidence_field(
        "canary_proof_candidate_source_path",
        operator_evidence
            .canary_proof_candidate_source_path
            .as_deref(),
    )?;
    let candidate_source_sha256 = required_operator_evidence_field(
        "canary_proof_candidate_source_sha256",
        operator_evidence
            .canary_proof_candidate_source_sha256
            .as_deref(),
    )?;

    validate_operator_evidence_toml_path("canary_proof_order_intent_path", order_intent_path)?;
    validate_operator_evidence_sha256("canary_proof_order_intent_sha256", order_intent_sha256)?;
    validate_operator_evidence_toml_path(
        "canary_proof_candidate_source_path",
        candidate_source_path,
    )?;
    validate_operator_evidence_sha256(
        "canary_proof_candidate_source_sha256",
        candidate_source_sha256,
    )?;

    let order_intent: serde_json::Value = read_operator_evidence_json_artifact(
        loaded,
        operator_evidence,
        "canary_proof_order_intent_path",
        "canary_proof_order_intent_sha256",
        order_intent_path,
        order_intent_sha256,
    )?;
    let candidate_source: serde_json::Value = read_operator_evidence_json_artifact(
        loaded,
        operator_evidence,
        "canary_proof_candidate_source_path",
        "canary_proof_candidate_source_sha256",
        candidate_source_path,
        candidate_source_sha256,
    )?;

    validate_canary_proof_artifact_content(
        live_canary.max_notional_per_order.as_str(),
        proof_policy.strategy_instance_id.as_str(),
        proof_policy.execution_client_id.as_str(),
        &candidate_source,
        &order_intent,
    )?;
    Ok(())
}

fn validate_canary_proof_artifact_content(
    max_notional_per_order: &str,
    strategy_instance_id: &str,
    execution_client_id: &str,
    candidate_source: &serde_json::Value,
    order_intent: &serde_json::Value,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_canary_proof_string_field(
        candidate_source,
        "record_kind",
        CANARY_PROOF_CANDIDATE_SOURCE_RECORD_KIND,
        "canary_proof_candidate_source.record_kind",
    )?;
    validate_canary_proof_string_field(
        candidate_source,
        "proof_claim",
        CANARY_PROOF_CLAIM,
        "canary_proof_candidate_source.proof_claim",
    )?;
    validate_canary_proof_string_field(
        order_intent,
        "record_kind",
        CANARY_PROOF_ORDER_INTENT_RECORD_KIND,
        "canary_proof_order_intent.record_kind",
    )?;
    validate_canary_proof_string_field(
        order_intent,
        "proof_claim",
        CANARY_PROOF_CLAIM,
        "canary_proof_order_intent.proof_claim",
    )?;
    validate_canary_proof_string_field(
        order_intent,
        "strategy_instance_id",
        strategy_instance_id,
        "canary_proof_order_intent.strategy_instance_id",
    )?;
    validate_canary_proof_string_field(
        order_intent,
        "execution_client_id",
        execution_client_id,
        "canary_proof_order_intent.execution_client_id",
    )?;

    let current_source_ref = canary_proof_required_str(
        candidate_source,
        "current_source_ref",
        "canary_proof_candidate_source.current_source_ref",
    )?;
    let source_refs = order_intent
        .get("source_refs")
        .and_then(serde_json::Value::as_array)
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_proof_order_intent.source_refs",
        })?;
    if !source_refs
        .iter()
        .any(|source_ref| source_ref.as_str() == Some(current_source_ref))
    {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_proof_order_intent.source_refs",
        });
    }

    let notional = canary_proof_required_decimal(
        order_intent,
        "notional",
        "canary_proof_order_intent.notional",
    )?;
    let max_notional = Decimal::from_str(max_notional_per_order).map_err(|_| {
        BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_proof_order_intent.max_notional_per_order",
        }
    })?;
    if notional <= Decimal::ZERO || notional > max_notional {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_proof_order_intent.notional",
        });
    }
    Ok(())
}

fn validate_canary_proof_string_field(
    value: &serde_json::Value,
    json_field: &'static str,
    expected: &str,
    error_field: &'static str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.get(json_field).and_then(serde_json::Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: error_field })
    }
}

fn canary_proof_required_str<'a>(
    value: &'a serde_json::Value,
    json_field: &'static str,
    error_field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    value
        .get(json_field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: error_field })
}

fn canary_proof_required_decimal(
    value: &serde_json::Value,
    json_field: &'static str,
    error_field: &'static str,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    let source = canary_proof_required_str(value, json_field, error_field)?;
    Decimal::from_str(source)
        .map_err(|_| BoltV3OperatorArtifactError::FinalEvidenceSchema { field: error_field })
}

fn read_operator_evidence_json_artifact<T>(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    path_field: &'static str,
    hash_field: &'static str,
    configured_path: &str,
    configured_sha256: &str,
) -> Result<T, BoltV3OperatorArtifactError>
where
    T: DeserializeOwned,
{
    let path = resolve_loaded_config_path(loaded, configured_path);
    let bytes = read_file_bounded(&path, operator_evidence.max_operator_evidence_file_bytes)
        .map_err(|source| BoltV3OperatorArtifactError::FinalEvidenceRead {
            field: path_field,
            path: path.clone(),
            source,
        })?;
    if hex::encode(Sha256::digest(&bytes)) != configured_sha256 {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceHashMismatch { field: hash_field });
    }
    serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::FinalEvidenceParse {
            field: path_field,
            path,
            source,
        }
    })
}

fn verify_strategy_input_replay_binding(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    let strategy_input_path =
        resolve_loaded_config_path(loaded, &operator_evidence.strategy_input_evidence_path);
    let strategy_input_bytes = read_file_bounded(
        &strategy_input_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3OperatorArtifactError::FinalEvidenceRead {
        field: "strategy_input_replay.strategy_input_evidence_path",
        path: strategy_input_path.clone(),
        source,
    })?;
    if hex::encode(Sha256::digest(&strategy_input_bytes))
        != operator_evidence.strategy_input_evidence_sha256
    {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.strategy_input_evidence_sha256",
        });
    }
    let actual_strategy_input: Phase8StrategyInputEvidenceFile =
        serde_json::from_slice(&strategy_input_bytes).map_err(|_| {
            BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                field: "strategy_input_replay.strategy_input_evidence",
            }
        })?;
    let strategy_input_json: serde_json::Value = serde_json::from_slice(&strategy_input_bytes)
        .map_err(
            |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                field: "strategy_input_replay.strategy_input_evidence",
            },
        )?;
    let strategy_instance_id =
        strategy_input_replay_required_str(&strategy_input_json, "strategy_instance_id")?;
    let market_selection_source_path_text =
        strategy_input_replay_required_str(&strategy_input_json, "market_selection_source_path")?;
    let market_selection_source_sha256 =
        strategy_input_replay_required_str(&strategy_input_json, "market_selection_source_sha256")?;
    if !is_lowercase_sha256(market_selection_source_sha256) {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source_sha256",
        });
    }
    let market_selection_source_path = Path::new(market_selection_source_path_text);
    validate_market_window_source_path(
        "strategy_input_replay.market_selection_source_path",
        market_selection_source_path,
    )?;
    let resolved_market_selection_source_path =
        resolve_peer_artifact_path(&strategy_input_path, market_selection_source_path);
    let market_selection_source_bytes = read_file_bounded(
        &resolved_market_selection_source_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3OperatorArtifactError::FinalEvidenceRead {
        field: "strategy_input_replay.market_selection_source_path",
        path: resolved_market_selection_source_path.clone(),
        source,
    })?;
    if hex::encode(Sha256::digest(&market_selection_source_bytes)) != market_selection_source_sha256
    {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source_sha256",
        });
    }
    let market_selection_source: Phase8MarketSelectionSourceEvidenceFile =
        serde_json::from_slice(&market_selection_source_bytes).map_err(|_| {
            BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                field: "strategy_input_replay.market_selection_source",
            }
        })?;
    let resolved_decision_evidence_path =
        resolve_loaded_config_path(loaded, &operator_evidence.decision_evidence_path);
    let configured_decision_evidence_path = decision_evidence_path(loaded)
        .map(|path| resolve_loaded_config_path_from_path(loaded, &path))
        .map_err(
            |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                field: "strategy_input_replay.decision_evidence_path",
            },
        )?;
    if resolved_decision_evidence_path != configured_decision_evidence_path {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.decision_evidence_path",
        });
    }
    let decision_chain = read_latest_entry_decision_evidence_chain(
        &resolved_decision_evidence_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.decision_evidence_path",
        },
    )?;
    let market_selection_runtime_provenance = market_selection_source.runtime_provenance().ok_or(
        BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.runtime_provenance",
        },
    )?;
    let provenance_decision_evidence_path = resolve_peer_artifact_path(
        &resolved_market_selection_source_path,
        Path::new(market_selection_runtime_provenance.decision_evidence_path()),
    );
    if provenance_decision_evidence_path != resolved_decision_evidence_path {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.decision_evidence_path",
        });
    }
    let decision_evidence_bytes = read_file_bounded(
        &resolved_decision_evidence_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.decision_evidence_path",
        },
    )?;
    if hex::encode(Sha256::digest(&decision_evidence_bytes))
        != market_selection_runtime_provenance.decision_evidence_sha256()
    {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.decision_evidence_sha256",
        });
    }
    let instrument_source_path = resolve_peer_artifact_path(
        &resolved_market_selection_source_path,
        Path::new(market_selection_runtime_provenance.instrument_source_path()),
    );
    let instrument_source_bytes = read_file_bounded(
        &instrument_source_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.instrument_source_path",
        },
    )?;
    if hex::encode(Sha256::digest(&instrument_source_bytes))
        != market_selection_runtime_provenance.instrument_source_sha256()
    {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.instrument_source_sha256",
        });
    }
    let instruments: Vec<InstrumentAny> = serde_json::from_slice(&instrument_source_bytes)
        .map_err(
            |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                field: "strategy_input_replay.market_selection_source.instrument_source",
            },
        )?;
    if instruments.is_empty() {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.instrument_source",
        });
    }
    let market_selection_timestamp_ms = decision_chain
        .snapshot
        .market_selection_timestamp_ms
        .ok_or(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source.market_selection_timestamp_ms",
        })?;
    let expected_market_selection_source = build_market_selection_source_artifact(
        loaded,
        strategy_instance_id,
        &instruments,
        market_selection_timestamp_ms,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source",
        },
    )?
    .with_runtime_provenance(
        Phase8MarketSelectionRuntimeProvenance::new(
            market_selection_runtime_provenance.decision_evidence_path(),
            market_selection_runtime_provenance.decision_evidence_sha256(),
            market_selection_runtime_provenance.instrument_source_path(),
            market_selection_runtime_provenance.instrument_source_sha256(),
        )
        .map_err(
            |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                field: "strategy_input_replay.market_selection_source.runtime_provenance",
            },
        )?,
    );
    if expected_market_selection_source != market_selection_source {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.market_selection_source",
        });
    }
    let expected_strategy_input = {
        let runtime_strategy_id =
            runtime_strategy_id_for_loaded_strategy(loaded, strategy_instance_id).map_err(
                |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
                    field: "strategy_input_replay.strategy_instance_id",
                },
            )?;
        Phase8StrategyInputEvidenceFile::from_runtime_snapshot_and_market_selection_source(
            &decision_chain.snapshot,
            strategy_instance_id,
            &runtime_strategy_id,
            &market_selection_source,
            market_selection_source_path_text,
            market_selection_source_sha256,
            market_selection_source.candidate_market_start_timestamps_ms(),
        )
    }
    .map_err(
        |_| BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.expected_strategy_input",
        },
    )?;
    if actual_strategy_input != expected_strategy_input {
        return Err(BoltV3OperatorArtifactError::StrategyInputReplayInvalid {
            field: "strategy_input_replay.strategy_input_evidence",
        });
    }

    Ok(())
}

fn strategy_input_replay_required_str<'a>(
    value: &'a serde_json::Value,
    field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(BoltV3OperatorArtifactError::StrategyInputReplayInvalid { field })
}

fn validate_required_static_manifest_artifact(
    loaded: &LoadedBoltV3Config,
    manifest: &BoltV3StaticArtifactsManifestInput,
    name: &'static str,
    configured_path: &str,
    configured_sha256: &str,
    configured_sha256_field: &'static str,
    max_bytes: u64,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_operator_evidence_sha256(configured_sha256_field, configured_sha256)?;
    let artifact = static_manifest_artifact_by_name(manifest, name)?;
    if artifact.path != configured_path {
        return Err(BoltV3OperatorArtifactError::StaticManifestArtifactPathMismatch { name });
    }
    if artifact.sha256 != configured_sha256 {
        return Err(BoltV3OperatorArtifactError::StaticManifestArtifactHashMismatch { name });
    }
    let resolved_path = resolve_loaded_config_path(loaded, configured_path);
    let actual = sha256_file_for_static_manifest(name, &resolved_path, max_bytes)?;
    if actual != configured_sha256 {
        return Err(
            BoltV3OperatorArtifactError::StaticManifestArtifactFileHashMismatch {
                name,
                path: resolved_path,
            },
        );
    }
    Ok(())
}

fn static_manifest_artifact_by_name<'a>(
    manifest: &'a BoltV3StaticArtifactsManifestInput,
    name: &'static str,
) -> Result<&'a BoltV3StaticArtifactRefInput, BoltV3OperatorArtifactError> {
    let mut matches = manifest
        .generated_artifacts
        .iter()
        .filter(|artifact| artifact.name == name);
    let artifact = matches
        .next()
        .ok_or(BoltV3OperatorArtifactError::StaticManifestMissingArtifact { name })?;
    if matches.next().is_some() {
        return Err(
            BoltV3OperatorArtifactError::StaticManifestDuplicateArtifact {
                name: name.to_string(),
            },
        );
    }
    validate_operator_evidence_sha256(
        "static_manifest.generated_artifacts.sha256",
        &artifact.sha256,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::StaticManifestArtifactHashShape {
            field: "static_manifest.generated_artifacts.sha256",
        },
    )?;
    Ok(artifact)
}

fn validate_operator_evidence_build_head(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    let build_head =
        current_build_head_sha().ok_or(BoltV3OperatorArtifactError::BuildHeadShaUnavailable)?;
    if evidence.head_sha != build_head {
        return Err(BoltV3OperatorArtifactError::OperatorEvidenceHeadShaMismatch);
    }
    Ok(())
}

fn validate_operator_packet_header(
    loaded: &LoadedBoltV3Config,
    packet: &BoltV3OperatorEvidencePacketInput,
) -> Result<(), BoltV3OperatorArtifactError> {
    if packet.schema_version != OPERATOR_EVIDENCE_PACKET_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::OperatorPacketSchema {
            field: "schema_version",
        });
    }
    if packet.record_kind != OPERATOR_EVIDENCE_PACKET_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::OperatorPacketSchema {
            field: "record_kind",
        });
    }
    if packet.config_bundle_checksum != loaded.config_bundle_checksum {
        return Err(BoltV3OperatorArtifactError::OperatorPacketConfigBundleDrift);
    }
    Ok(())
}

fn validate_operator_packet_evidence_block(
    expected: &LiveCanaryOperatorEvidenceBlock,
    actual: &BoltV3OperatorEvidencePacketBlockInput,
) -> Result<(), BoltV3OperatorArtifactError> {
    let expected_gate_session_path = required_operator_evidence_field(
        OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
        expected.gate_session_path.as_deref(),
    )?;
    let expected_gate_session_sha256 = required_operator_evidence_field(
        OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
        expected.expected_gate_session_sha256.as_deref(),
    )?;
    for (field, actual, expected) in [
        (
            "head_sha",
            actual.head_sha.as_str(),
            expected.head_sha.as_str(),
        ),
        (
            "approval_envelope_path",
            actual.approval_envelope_path.as_str(),
            expected.approval_envelope_path.as_str(),
        ),
        (
            "approval_envelope_sha256",
            actual.approval_envelope_sha256.as_str(),
            expected.approval_envelope_sha256.as_str(),
        ),
        (
            "ssm_manifest_path",
            actual.ssm_manifest_path.as_str(),
            expected.ssm_manifest_path.as_str(),
        ),
        (
            "ssm_manifest_sha256",
            actual.ssm_manifest_sha256.as_str(),
            expected.ssm_manifest_sha256.as_str(),
        ),
        (
            "strategy_input_evidence_path",
            actual.strategy_input_evidence_path.as_str(),
            expected.strategy_input_evidence_path.as_str(),
        ),
        (
            "strategy_input_evidence_sha256",
            actual.strategy_input_evidence_sha256.as_str(),
            expected.strategy_input_evidence_sha256.as_str(),
        ),
        (
            OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
            actual.gate_session_path.as_str(),
            expected_gate_session_path,
        ),
        (
            OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
            actual.expected_gate_session_sha256.as_str(),
            expected_gate_session_sha256,
        ),
        (
            "financial_envelope_path",
            actual.financial_envelope_path.as_str(),
            expected.financial_envelope_path.as_str(),
        ),
        (
            "financial_envelope_sha256",
            actual.financial_envelope_sha256.as_str(),
            expected.financial_envelope_sha256.as_str(),
        ),
        (
            "pre_run_state_path",
            actual.pre_run_state_path.as_str(),
            expected.pre_run_state_path.as_str(),
        ),
        (
            "pre_run_state_sha256",
            actual.pre_run_state_sha256.as_str(),
            expected.pre_run_state_sha256.as_str(),
        ),
        (
            "abort_plan_path",
            actual.abort_plan_path.as_str(),
            expected.abort_plan_path.as_str(),
        ),
        (
            "abort_plan_sha256",
            actual.abort_plan_sha256.as_str(),
            expected.abort_plan_sha256.as_str(),
        ),
        (
            "canary_evidence_path",
            actual.canary_evidence_path.as_str(),
            expected.canary_evidence_path.as_str(),
        ),
        (
            "approval_nonce_path",
            actual.approval_nonce_path.as_str(),
            expected.approval_nonce_path.as_str(),
        ),
        (
            "approval_nonce_sha256",
            actual.approval_nonce_sha256.as_str(),
            expected.approval_nonce_sha256.as_str(),
        ),
        (
            "approval_consumption_path",
            actual.approval_consumption_path.as_str(),
            expected.approval_consumption_path.as_str(),
        ),
        (
            "decision_evidence_path",
            actual.decision_evidence_path.as_str(),
            expected.decision_evidence_path.as_str(),
        ),
        (
            "nt_submit_event_path",
            actual.nt_submit_event_path.as_str(),
            expected.nt_submit_event_path.as_str(),
        ),
        (
            "venue_order_state_path",
            actual.venue_order_state_path.as_str(),
            expected.venue_order_state_path.as_str(),
        ),
        (
            "restart_reconciliation_path",
            actual.restart_reconciliation_path.as_str(),
            expected.restart_reconciliation_path.as_str(),
        ),
        (
            "post_run_hygiene_path",
            actual.post_run_hygiene_path.as_str(),
            expected.post_run_hygiene_path.as_str(),
        ),
    ] {
        if actual != expected {
            return Err(BoltV3OperatorArtifactError::OperatorPacketEvidenceMismatch { field });
        }
    }

    for (field, actual, expected) in [
        (
            "canary_proof_candidate_source_path",
            actual.canary_proof_candidate_source_path.as_deref(),
            expected.canary_proof_candidate_source_path.as_deref(),
        ),
        (
            "canary_proof_candidate_source_sha256",
            actual.canary_proof_candidate_source_sha256.as_deref(),
            expected.canary_proof_candidate_source_sha256.as_deref(),
        ),
        (
            "canary_proof_order_intent_path",
            actual.canary_proof_order_intent_path.as_deref(),
            expected.canary_proof_order_intent_path.as_deref(),
        ),
        (
            "canary_proof_order_intent_sha256",
            actual.canary_proof_order_intent_sha256.as_deref(),
            expected.canary_proof_order_intent_sha256.as_deref(),
        ),
    ] {
        if actual != expected {
            return Err(BoltV3OperatorArtifactError::OperatorPacketEvidenceMismatch { field });
        }
    }

    if actual.strategy_cancel_path != expected.strategy_cancel_path {
        return Err(
            BoltV3OperatorArtifactError::OperatorPacketEvidenceMismatch {
                field: "strategy_cancel_path",
            },
        );
    }
    Ok(())
}

fn validate_packet_sha256_field(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if is_lowercase_sha256(value) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::OperatorPacketHashShape { field })
    }
}

fn verify_operator_approval_envelope(
    loaded: &LoadedBoltV3Config,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    validate_operator_evidence_sha256(
        "approval_envelope_sha256",
        &evidence.approval_envelope_sha256,
    )?;
    let path = resolve_loaded_config_path(loaded, &evidence.approval_envelope_path);
    let bytes =
        read_file_bounded(&path, evidence.max_operator_evidence_file_bytes).map_err(|source| {
            BoltV3OperatorArtifactError::ApprovalEnvelopeRead {
                path: path.clone(),
                source,
            }
        })?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    if sha256 != evidence.approval_envelope_sha256 {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeHashMismatch);
    }
    let envelope: Phase8OperatorApprovalEnvelopeFile =
        serde_json::from_slice(&bytes).map_err(|source| {
            BoltV3OperatorArtifactError::ApprovalEnvelopeParse {
                path: path.clone(),
                source,
            }
        })?;
    validate_approval_envelope_fields(evidence, approval_id, &envelope)?;
    Ok(WrittenOperatorArtifact { path, sha256 })
}

fn validate_approval_envelope_fields(
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
    envelope: &Phase8OperatorApprovalEnvelopeFile,
) -> Result<(), BoltV3OperatorArtifactError> {
    if envelope.schema_version != APPROVAL_ENVELOPE_SCHEMA_VERSION {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeSchema {
            field: "schema_version",
        });
    }
    if envelope.record_kind != APPROVAL_ENVELOPE_RECORD_KIND {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeSchema {
            field: "record_kind",
        });
    }
    let approval_id_hash = sha256_text(approval_id);
    let canary_evidence_path_hash = sha256_text(&evidence.canary_evidence_path);
    for (field, actual, expected) in [
        (
            "head_sha",
            envelope.head_sha.as_str(),
            evidence.head_sha.as_str(),
        ),
        (
            "ssm_manifest_sha256",
            envelope.ssm_manifest_sha256.as_str(),
            evidence.ssm_manifest_sha256.as_str(),
        ),
        (
            "strategy_input_evidence_sha256",
            envelope.strategy_input_evidence_sha256.as_str(),
            evidence.strategy_input_evidence_sha256.as_str(),
        ),
        (
            "financial_envelope_sha256",
            envelope.financial_envelope_sha256.as_str(),
            evidence.financial_envelope_sha256.as_str(),
        ),
        (
            "pre_run_state_sha256",
            envelope.pre_run_state_sha256.as_str(),
            evidence.pre_run_state_sha256.as_str(),
        ),
        (
            "abort_plan_sha256",
            envelope.abort_plan_sha256.as_str(),
            evidence.abort_plan_sha256.as_str(),
        ),
        (
            "approval_id_hash",
            envelope.approval_id_hash.as_str(),
            approval_id_hash.as_str(),
        ),
        (
            "approval_nonce_sha256",
            envelope.approval_nonce_sha256.as_str(),
            evidence.approval_nonce_sha256.as_str(),
        ),
        (
            "canary_evidence_path_hash",
            envelope.canary_evidence_path_hash.as_str(),
            canary_evidence_path_hash.as_str(),
        ),
    ] {
        if actual != expected {
            return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch { field });
        }
    }
    if envelope.approval_not_before_unix_secs != evidence.approval_not_before_unix_seconds {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "approval_not_before_unix_secs",
        });
    }
    if envelope.approval_not_after_unix_secs != evidence.approval_not_after_unix_seconds {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "approval_not_after_unix_secs",
        });
    }
    let expected_cancel_hash = evidence.strategy_cancel_path.as_deref().map(sha256_text);
    if envelope.strategy_cancel_path_hash != expected_cancel_hash {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "strategy_cancel_path_hash",
        });
    }
    // Blocker-B binding fields: the sealed gate-session and canary proof
    // order-intent content hashes must match the operator evidence. The runtime
    // gate in `bolt_v3_live_canary_gate.rs` is the authoritative enforcer of
    // these bindings; this is defense-in-depth so both envelope validators
    // enforce the same fields. Both sides are `Option<String>`, so `None ==
    // None`, `Some(a) == Some(a)`, and every other pairing mismatches.
    if envelope.expected_gate_session_sha256 != evidence.expected_gate_session_sha256 {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "expected_gate_session_sha256",
        });
    }
    if envelope.canary_proof_order_intent_sha256 != evidence.canary_proof_order_intent_sha256 {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "canary_proof_order_intent_sha256",
        });
    }
    if envelope.no_submit_readiness_report_sha256 != evidence.no_submit_readiness_report_sha256 {
        return Err(BoltV3OperatorArtifactError::ApprovalEnvelopeMismatch {
            field: "no_submit_readiness_report_sha256",
        });
    }
    Ok(())
}

fn verify_final_live_evidence_files(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
    approval_envelope_sha256: &str,
    max_live_order_count: u32,
    max_notional_per_order: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let (canary, _) = read_final_json_evidence(
        loaded,
        operator_evidence,
        "canary_evidence_path",
        &operator_evidence.canary_evidence_path,
    )?;
    validate_canary_final_evidence(
        &canary,
        operator_evidence,
        approval_id,
        max_live_order_count,
        max_notional_per_order,
    )?;
    verify_canary_static_evidence_ref(
        &canary,
        "ssm_manifest_ref",
        &operator_evidence.ssm_manifest_path,
        &operator_evidence.ssm_manifest_sha256,
    )?;
    verify_canary_static_evidence_ref(
        &canary,
        "strategy_input_evidence_ref",
        &operator_evidence.strategy_input_evidence_path,
        &operator_evidence.strategy_input_evidence_sha256,
    )?;
    verify_canary_final_evidence_ref(
        loaded,
        operator_evidence,
        &canary,
        "decision_evidence_ref",
        "decision_evidence_path",
        &operator_evidence.decision_evidence_path,
    )?;
    verify_canary_final_evidence_ref(
        loaded,
        operator_evidence,
        &canary,
        "nt_submit_event_ref",
        "nt_submit_event_path",
        &operator_evidence.nt_submit_event_path,
    )?;
    verify_canary_final_evidence_ref(
        loaded,
        operator_evidence,
        &canary,
        "venue_order_state_ref",
        "venue_order_state_path",
        &operator_evidence.venue_order_state_path,
    )?;
    verify_optional_strategy_cancel_ref(loaded, operator_evidence, &canary)?;
    verify_canary_final_evidence_ref(
        loaded,
        operator_evidence,
        &canary,
        "restart_reconciliation_ref",
        "restart_reconciliation_path",
        &operator_evidence.restart_reconciliation_path,
    )?;
    verify_canary_final_evidence_ref(
        loaded,
        operator_evidence,
        &canary,
        "post_run_hygiene_ref",
        "post_run_hygiene_path",
        &operator_evidence.post_run_hygiene_path,
    )?;

    let (approval_consumption, _) = read_final_json_evidence(
        loaded,
        operator_evidence,
        "approval_consumption_path",
        &operator_evidence.approval_consumption_path,
    )?;
    let root_toml_sha256 = root_toml_sha256_for_final_evidence(loaded)?;
    validate_approval_consumption_final_evidence(
        &approval_consumption,
        operator_evidence,
        approval_id,
        approval_envelope_sha256,
        &root_toml_sha256,
    )
}

fn validate_canary_final_evidence(
    canary: &serde_json::Value,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
    max_live_order_count: u32,
    max_notional_per_order: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let schema_version = expect_final_u64(canary, "canary_evidence_path.schema_version")?;
    if schema_version != 1 {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_evidence_path.schema_version",
        });
    }
    expect_final_string_equals(
        canary,
        "head_sha",
        "canary_evidence_path.head_sha",
        &operator_evidence.head_sha,
    )?;
    expect_final_string_equals(
        canary,
        "ssm_manifest_sha256",
        "canary_evidence_path.ssm_manifest_sha256",
        &operator_evidence.ssm_manifest_sha256,
    )?;
    expect_final_string_equals(
        canary,
        "approval_id_hash",
        "canary_evidence_path.approval_id_hash",
        &sha256_text(approval_id),
    )?;
    let reported_count = expect_final_u64(canary, "canary_evidence_path.max_live_order_count")?;
    if reported_count != u64::from(max_live_order_count) {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "canary_evidence_path.max_live_order_count",
        });
    }
    expect_final_string_equals(
        canary,
        "max_notional_per_order",
        "canary_evidence_path.max_notional_per_order",
        max_notional_per_order,
    )?;
    expect_final_string_equals(
        canary,
        "outcome",
        "canary_evidence_path.outcome",
        "live_canary_proof",
    )?;
    let block_reasons = canary
        .get("block_reasons")
        .and_then(serde_json::Value::as_array)
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_evidence_path.block_reasons",
        })?;
    if !block_reasons.is_empty() {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "canary_evidence_path.block_reasons",
        });
    }
    let submit_ref = canary.get("submit_admission_ref").ok_or(
        BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "canary_evidence_path.submit_admission_ref",
        },
    )?;
    expect_final_string_equals(
        submit_ref,
        "status",
        "canary_evidence_path.submit_admission_ref.status",
        "accepted",
    )?;
    let admitted_order_count = expect_final_u64(
        submit_ref,
        "canary_evidence_path.submit_admission_ref.admitted_order_count",
    )?;
    let required_admitted_order_count = u64::from(max_live_order_count).saturating_mul(2);
    if admitted_order_count != required_admitted_order_count {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "canary_evidence_path.submit_admission_ref.admitted_order_count",
        });
    }
    Ok(())
}

fn verify_canary_static_evidence_ref(
    canary: &serde_json::Value,
    ref_field: &'static str,
    configured_path: &str,
    configured_sha256: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let evidence_ref = canary
        .get(ref_field)
        .filter(|value| !value.is_null())
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: ref_field })?;
    expect_final_string_equals(
        evidence_ref,
        "path_hash",
        ref_field,
        &sha256_text(configured_path),
    )?;
    expect_final_string_equals(evidence_ref, "record_hash", ref_field, configured_sha256)
}

fn verify_canary_final_evidence_ref(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    canary: &serde_json::Value,
    ref_field: &'static str,
    path_field: &'static str,
    configured_path: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let evidence_ref = canary
        .get(ref_field)
        .filter(|value| !value.is_null())
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: ref_field })?;
    expect_final_string_equals(
        evidence_ref,
        "path_hash",
        ref_field,
        &sha256_text(configured_path),
    )?;
    let record_hash = expect_final_string(evidence_ref, "record_hash", ref_field)?;
    let actual_hash =
        read_final_evidence_sha256(loaded, operator_evidence, path_field, configured_path)?;
    if record_hash != actual_hash {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceHashMismatch { field: ref_field });
    }
    Ok(())
}

fn verify_optional_strategy_cancel_ref(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    canary: &serde_json::Value,
) -> Result<(), BoltV3OperatorArtifactError> {
    match (
        operator_evidence.strategy_cancel_path.as_deref(),
        canary.get("strategy_cancel_ref"),
    ) {
        (None, None) | (None, Some(serde_json::Value::Null)) => Ok(()),
        (Some(_), None) | (Some(_), Some(serde_json::Value::Null)) => {
            verify_strategy_cancel_absent_for_terminal_closed_order(loaded, operator_evidence)
        }
        (Some(configured), Some(value)) if !value.is_null() => {
            expect_final_string_equals(
                value,
                "path_hash",
                "strategy_cancel_ref",
                &sha256_text(configured),
            )?;
            let record_hash = expect_final_string(value, "record_hash", "strategy_cancel_ref")?;
            let actual_hash = read_final_evidence_sha256(
                loaded,
                operator_evidence,
                "strategy_cancel_path",
                configured,
            )?;
            if record_hash != actual_hash {
                return Err(BoltV3OperatorArtifactError::FinalEvidenceHashMismatch {
                    field: "strategy_cancel_ref",
                });
            }
            Ok(())
        }
        (Some(_), Some(_)) | (None, Some(_)) => {
            Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
                field: "strategy_cancel_ref",
            })
        }
    }
}

fn verify_strategy_cancel_absent_for_terminal_closed_order(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    let (venue_order_state, _) = read_final_json_evidence(
        loaded,
        operator_evidence,
        "venue_order_state_path",
        &operator_evidence.venue_order_state_path,
    )?;
    verify_strategy_cancel_absent_for_terminal_closed_order_value(&venue_order_state)
}

fn verify_strategy_cancel_absent_for_terminal_closed_order_value(
    venue_order_state: &serde_json::Value,
) -> Result<(), BoltV3OperatorArtifactError> {
    let outcome = expect_final_string(
        venue_order_state,
        "venue_order_outcome",
        "strategy_cancel_ref.venue_order_outcome",
    )?;
    let order_remains_open = venue_order_state
        .get("order_remains_open")
        .and_then(serde_json::Value::as_bool)
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "strategy_cancel_ref.order_remains_open",
        })?;
    match (outcome, order_remains_open) {
        (LIVE_CANARY_TERMINAL_OUTCOME_FILLED | LIVE_CANARY_TERMINAL_OUTCOME_REJECTED, false) => {
            Ok(())
        }
        _ => Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "strategy_cancel_ref",
        }),
    }
}

fn validate_approval_consumption_final_evidence(
    approval: &serde_json::Value,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
    approval_envelope_sha256: &str,
    root_toml_sha256: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let schema_version = expect_final_i64(approval, "approval_consumption_path.schema_version")?;
    if schema_version != 1 {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceSchema {
            field: "approval_consumption_path.schema_version",
        });
    }
    expect_final_string_equals(
        approval,
        "record_kind",
        "approval_consumption_path.record_kind",
        "phase8_operator_approval_consumption",
    )?;
    expect_final_string_equals(
        approval,
        "head_sha",
        "approval_consumption_path.head_sha",
        &operator_evidence.head_sha,
    )?;
    expect_final_string_equals(
        approval,
        "root_toml_sha256",
        "approval_consumption_path.root_toml_sha256",
        root_toml_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "approval_envelope_sha256",
        "approval_consumption_path.approval_envelope_sha256",
        approval_envelope_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "ssm_manifest_sha256",
        "approval_consumption_path.ssm_manifest_sha256",
        &operator_evidence.ssm_manifest_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "strategy_input_evidence_sha256",
        "approval_consumption_path.strategy_input_evidence_sha256",
        &operator_evidence.strategy_input_evidence_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "financial_envelope_sha256",
        "approval_consumption_path.financial_envelope_sha256",
        &operator_evidence.financial_envelope_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "pre_run_state_sha256",
        "approval_consumption_path.pre_run_state_sha256",
        &operator_evidence.pre_run_state_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "abort_plan_sha256",
        "approval_consumption_path.abort_plan_sha256",
        &operator_evidence.abort_plan_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "approval_id_hash",
        "approval_consumption_path.approval_id_hash",
        &sha256_text(approval_id),
    )?;
    expect_final_string_equals(
        approval,
        "approval_nonce_sha256",
        "approval_consumption_path.approval_nonce_sha256",
        &operator_evidence.approval_nonce_sha256,
    )?;
    expect_final_string_equals(
        approval,
        "canary_evidence_path_hash",
        "approval_consumption_path.canary_evidence_path_hash",
        &sha256_text(&operator_evidence.canary_evidence_path),
    )?;
    let not_before = expect_final_i64(
        approval,
        "approval_consumption_path.approval_not_before_unix_secs",
    )?;
    if not_before != operator_evidence.approval_not_before_unix_seconds {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "approval_consumption_path.approval_not_before_unix_secs",
        });
    }
    let not_after = expect_final_i64(
        approval,
        "approval_consumption_path.approval_not_after_unix_secs",
    )?;
    if not_after != operator_evidence.approval_not_after_unix_seconds {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "approval_consumption_path.approval_not_after_unix_secs",
        });
    }
    let consumed_unix_secs =
        expect_final_i64(approval, "approval_consumption_path.consumed_unix_secs")?;
    if consumed_unix_secs < operator_evidence.approval_not_before_unix_seconds
        || consumed_unix_secs > operator_evidence.approval_not_after_unix_seconds
    {
        return Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
            field: "approval_consumption_path.consumed_unix_secs",
        });
    }
    match (
        operator_evidence.strategy_cancel_path.as_deref(),
        approval.get("strategy_cancel_path_hash"),
    ) {
        (None, None) | (None, Some(serde_json::Value::Null)) => Ok(()),
        (Some(configured), Some(value)) => {
            let actual =
                value
                    .as_str()
                    .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema {
                        field: "approval_consumption_path.strategy_cancel_path_hash",
                    })?;
            if actual == sha256_text(configured) {
                Ok(())
            } else {
                Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
                    field: "approval_consumption_path.strategy_cancel_path_hash",
                })
            }
        }
        (Some(_), None) | (None, Some(_)) => {
            Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch {
                field: "approval_consumption_path.strategy_cancel_path_hash",
            })
        }
    }
}

fn root_toml_sha256_for_final_evidence(
    loaded: &LoadedBoltV3Config,
) -> Result<String, BoltV3OperatorArtifactError> {
    let root_text =
        crate::bounded_config_read::read_to_string(&loaded.root_path).map_err(|source| {
            BoltV3OperatorArtifactError::FinalEvidenceRead {
                field: "approval_consumption_path.root_toml_sha256",
                path: loaded.root_path.clone(),
                source: std::io::Error::other(source),
            }
        })?;
    Ok(hex::encode(Sha256::digest(root_text.as_bytes())))
}

fn read_final_json_evidence(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    field: &'static str,
    configured_path: &str,
) -> Result<(serde_json::Value, String), BoltV3OperatorArtifactError> {
    let resolved_path = resolve_loaded_config_path(loaded, configured_path);
    let bytes = read_file_bounded(
        &resolved_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3OperatorArtifactError::FinalEvidenceRead {
        field,
        path: resolved_path.clone(),
        source,
    })?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let value = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::FinalEvidenceParse {
            field,
            path: resolved_path.clone(),
            source,
        }
    })?;
    Ok((value, sha256))
}

fn read_final_evidence_sha256(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    field: &'static str,
    configured_path: &str,
) -> Result<String, BoltV3OperatorArtifactError> {
    let resolved_path = resolve_loaded_config_path(loaded, configured_path);
    let bytes = read_file_bounded(
        &resolved_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )
    .map_err(|source| BoltV3OperatorArtifactError::FinalEvidenceRead {
        field,
        path: resolved_path,
        source,
    })?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn expect_final_string_equals(
    value: &serde_json::Value,
    json_field: &str,
    error_field: &'static str,
    expected: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let actual = expect_final_string(value, json_field, error_field)?;
    if actual == expected {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::FinalEvidenceMismatch { field: error_field })
    }
}

fn expect_final_string<'a>(
    value: &'a serde_json::Value,
    json_field: &str,
    error_field: &'static str,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    value
        .get(json_field)
        .and_then(serde_json::Value::as_str)
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: error_field })
}

fn expect_final_i64(
    value: &serde_json::Value,
    error_field: &'static str,
) -> Result<i64, BoltV3OperatorArtifactError> {
    let json_field = final_json_field(error_field);
    value
        .get(json_field)
        .and_then(serde_json::Value::as_i64)
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: error_field })
}

fn expect_final_u64(
    value: &serde_json::Value,
    error_field: &'static str,
) -> Result<u64, BoltV3OperatorArtifactError> {
    let json_field = final_json_field(error_field);
    value
        .get(json_field)
        .and_then(serde_json::Value::as_u64)
        .ok_or(BoltV3OperatorArtifactError::FinalEvidenceSchema { field: error_field })
}

fn final_json_field(error_field: &'static str) -> &'static str {
    error_field
        .rsplit_once('.')
        .map(|(_, field)| field)
        .unwrap_or(error_field)
}

fn approval_envelope_from_operator_evidence(
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
) -> Phase8OperatorApprovalEnvelopeFile {
    Phase8OperatorApprovalEnvelopeFile {
        schema_version: APPROVAL_ENVELOPE_SCHEMA_VERSION,
        record_kind: APPROVAL_ENVELOPE_RECORD_KIND.to_string(),
        head_sha: evidence.head_sha.clone(),
        ssm_manifest_sha256: evidence.ssm_manifest_sha256.clone(),
        strategy_input_evidence_sha256: evidence.strategy_input_evidence_sha256.clone(),
        financial_envelope_sha256: evidence.financial_envelope_sha256.clone(),
        pre_run_state_sha256: evidence.pre_run_state_sha256.clone(),
        abort_plan_sha256: evidence.abort_plan_sha256.clone(),
        approval_id_hash: sha256_text(approval_id),
        approval_nonce_sha256: evidence.approval_nonce_sha256.clone(),
        approval_not_before_unix_secs: evidence.approval_not_before_unix_seconds,
        approval_not_after_unix_secs: evidence.approval_not_after_unix_seconds,
        canary_evidence_path_hash: sha256_text(evidence.canary_evidence_path.as_str()),
        // Seal the operator-approved gate-session and canary proof
        // order-intent file-content hashes into the envelope. At
        // materialization time these `evidence` fields are the genuine file
        // hashes (the materializer enforces gate-session file ==
        // expected_gate_session_sha256 and computes
        // canary_proof_order_intent_sha256 directly from the file), so this
        // copy binds the exact order the operator authorized. The gate
        // re-checks the live file content against these sealed envelope
        // values, rejecting any post-approval file swap that updates only the
        // self-declared TOML hash.
        expected_gate_session_sha256: evidence.expected_gate_session_sha256.clone(),
        canary_proof_order_intent_sha256: evidence.canary_proof_order_intent_sha256.clone(),
        // Seal the no-submit readiness-report file hash. At materialization time
        // this `evidence` field is the genuine report file hash (the materializer
        // computes it directly from the file via
        // `operator_evidence_no_submit_readiness_report_sha256`), so this copy
        // binds the exact probe-produced report the operator authorized. The gate
        // re-checks the live report content against this sealed value, rejecting a
        // hand-written all-satisfied report that updates only the self-declared
        // TOML hash.
        no_submit_readiness_report_sha256: evidence.no_submit_readiness_report_sha256.clone(),
        strategy_cancel_path_hash: evidence.strategy_cancel_path.as_deref().map(sha256_text),
    }
}

fn operator_evidence_packet(
    loaded: &LoadedBoltV3Config,
    static_manifest_path: &Path,
    static_manifest_sha256: &str,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_envelope_sha256: String,
) -> Result<BoltV3OperatorEvidencePacket, BoltV3OperatorArtifactError> {
    let gate_session_path = required_operator_evidence_field(
        OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
        evidence.gate_session_path.as_deref(),
    )?;
    let expected_gate_session_sha256 = required_operator_evidence_field(
        OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
        evidence.expected_gate_session_sha256.as_deref(),
    )?;
    Ok(BoltV3OperatorEvidencePacket {
        schema_version: OPERATOR_EVIDENCE_PACKET_SCHEMA_VERSION,
        record_kind: OPERATOR_EVIDENCE_PACKET_RECORD_KIND,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        static_manifest_path: static_manifest_path.to_string_lossy().to_string(),
        static_manifest_sha256: static_manifest_sha256.to_string(),
        live_canary_operator_evidence: BoltV3OperatorEvidencePacketBlock {
            head_sha: evidence.head_sha.clone(),
            approval_envelope_path: evidence.approval_envelope_path.clone(),
            approval_envelope_sha256,
            ssm_manifest_path: evidence.ssm_manifest_path.clone(),
            ssm_manifest_sha256: evidence.ssm_manifest_sha256.clone(),
            strategy_input_evidence_path: evidence.strategy_input_evidence_path.clone(),
            strategy_input_evidence_sha256: evidence.strategy_input_evidence_sha256.clone(),
            gate_session_path: gate_session_path.to_string(),
            expected_gate_session_sha256: expected_gate_session_sha256.to_string(),
            financial_envelope_path: evidence.financial_envelope_path.clone(),
            financial_envelope_sha256: evidence.financial_envelope_sha256.clone(),
            pre_run_state_path: evidence.pre_run_state_path.clone(),
            pre_run_state_sha256: evidence.pre_run_state_sha256.clone(),
            abort_plan_path: evidence.abort_plan_path.clone(),
            abort_plan_sha256: evidence.abort_plan_sha256.clone(),
            canary_proof_candidate_source_path: evidence.canary_proof_candidate_source_path.clone(),
            canary_proof_candidate_source_sha256: evidence
                .canary_proof_candidate_source_sha256
                .clone(),
            canary_proof_order_intent_path: evidence.canary_proof_order_intent_path.clone(),
            canary_proof_order_intent_sha256: evidence.canary_proof_order_intent_sha256.clone(),
            canary_evidence_path: evidence.canary_evidence_path.clone(),
            approval_nonce_path: evidence.approval_nonce_path.clone(),
            approval_nonce_sha256: evidence.approval_nonce_sha256.clone(),
            approval_consumption_path: evidence.approval_consumption_path.clone(),
            decision_evidence_path: evidence.decision_evidence_path.clone(),
            nt_submit_event_path: evidence.nt_submit_event_path.clone(),
            venue_order_state_path: evidence.venue_order_state_path.clone(),
            strategy_cancel_path: evidence.strategy_cancel_path.clone(),
            restart_reconciliation_path: evidence.restart_reconciliation_path.clone(),
            post_run_hygiene_path: evidence.post_run_hygiene_path.clone(),
        },
    })
}

fn build_approval_nonce_artifact()
-> Result<BoltV3ApprovalNonceArtifact, BoltV3OperatorArtifactError> {
    let mut nonce = [0_u8; APPROVAL_NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(BoltV3OperatorArtifactError::Random)?;
    let mut hasher = Sha256::new();
    hasher.update(&nonce[..]);
    let nonce_sha256 = hex::encode(hasher.finalize());
    nonce.zeroize();
    Ok(BoltV3ApprovalNonceArtifact {
        schema_version: APPROVAL_NONCE_SCHEMA_VERSION,
        record_kind: APPROVAL_NONCE_RECORD_KIND,
        nonce_sha256,
    })
}

pub struct LiveCanaryTerminalResultProofInputs<'a> {
    pub run_id: &'a str,
    pub strategy_instance_id_hash: &'a str,
    pub client_order_id: &'a str,
    pub venue_order_id: &'a str,
    pub venue_order_outcome: &'a str,
    pub order_remains_open: bool,
    pub max_operator_evidence_file_bytes: u64,
    pub scanned_artifact_paths: &'a [PathBuf],
    /// The exact set of resolved-secret values this run handled, as produced by
    /// the single secret source of truth
    /// [`ResolvedBoltV3Secrets::redaction_values`]. The post-run hygiene scan
    /// computes `raw_secret_residue_absent` by checking that none of these
    /// values appear verbatim in any scanned artifact's bytes — it is NOT a
    /// hardcoded credential-shape list. The values are held in
    /// [`Zeroizing`] wrappers and are never logged or surfaced; only the boolean
    /// scan verdict escapes. An empty slice means the run resolved no
    /// redactable secret material, so no secret value can possibly leak.
    pub secret_redaction_values: &'a [Zeroizing<String>],
    pub retention_purge_path: &'a Path,
    pub nt_submit_event_path: &'a Path,
    pub venue_order_state_path: &'a Path,
    pub restart_reconciliation_path: &'a Path,
    pub post_run_hygiene_path: &'a Path,
}

pub struct LiveCanaryPostRunProofInputs<'a> {
    pub run_id: &'a str,
    pub runtime_capture_spool_root: &'a Path,
    pub client_order_id: &'a str,
    pub venue_order_id: &'a str,
    pub venue_order_outcome: &'a str,
    pub order_remains_open: bool,
    pub scanned_artifact_paths: &'a [PathBuf],
    pub retention_purge_path: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCanaryPostRunProofArtifacts {
    pub canary_evidence: WrittenOperatorArtifact,
    pub nt_submit_event: WrittenOperatorArtifact,
    pub venue_order_state: WrittenOperatorArtifact,
    pub restart_reconciliation: WrittenOperatorArtifact,
    pub post_run_hygiene: WrittenOperatorArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveCanaryTerminalResultArtifacts {
    nt_submit_event: WrittenOperatorArtifact,
    venue_order_state: WrittenOperatorArtifact,
    restart_reconciliation: WrittenOperatorArtifact,
    post_run_hygiene: WrittenOperatorArtifact,
}

impl LiveCanaryTerminalResultArtifacts {
    fn into_vec(self) -> Vec<WrittenOperatorArtifact> {
        vec![
            self.nt_submit_event,
            self.venue_order_state,
            self.restart_reconciliation,
            self.post_run_hygiene,
        ]
    }
}

#[derive(Debug, Serialize)]
struct LiveCanarySubmitEventProof<'a> {
    record_kind: &'static str,
    run_id: &'a str,
    strategy_instance_id_hash: &'a str,
    client_order_id_hash: String,
    venue_order_id_hash: String,
}

#[derive(Debug, Serialize)]
struct LiveCanaryVenueOrderStateProof<'a> {
    record_kind: &'static str,
    run_id: &'a str,
    strategy_instance_id_hash: &'a str,
    client_order_id_hash: String,
    venue_order_id_hash: String,
    venue_order_outcome: &'a str,
    order_remains_open: bool,
}

#[derive(Debug, Serialize)]
struct LiveCanaryRestartReconciliationProof<'a> {
    record_kind: &'static str,
    source_run_id: &'a str,
    strategy_instance_id_hash: &'a str,
    client_order_id_hash: String,
    venue_order_id_hash: String,
    venue_order_outcome: &'a str,
    order_remains_open: bool,
}

#[derive(Debug, Serialize)]
struct LiveCanaryPostRunHygieneProof<'a> {
    record_kind: &'static str,
    run_id: &'a str,
    strategy_instance_id_hash: &'a str,
    client_order_id_hash: String,
    venue_order_id_hash: String,
    raw_secret_residue_absent: bool,
    scanned_artifact_hashes: Vec<String>,
    retention_purge_path_hash: String,
}

pub fn write_live_canary_terminal_result_artifacts(
    inputs: &LiveCanaryTerminalResultProofInputs<'_>,
) -> anyhow::Result<Vec<WrittenOperatorArtifact>> {
    Ok(write_live_canary_terminal_result_artifact_refs(inputs)?.into_vec())
}

pub fn write_live_canary_post_run_proof_artifacts_from_config(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    inputs: &LiveCanaryPostRunProofInputs<'_>,
) -> anyhow::Result<LiveCanaryPostRunProofArtifacts> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow!("live canary post-run proof requires `[live_canary]`"))?;
    let operator_evidence = live_canary.operator_evidence.as_ref().ok_or_else(|| {
        anyhow!("live canary post-run proof requires `[live_canary.operator_evidence]`")
    })?;
    // Single secret source of truth for the post-run hygiene scan: the exact
    // resolved-secret values this run handled. The scan flags
    // `raw_secret_residue_absent = false` iff any of these values appears
    // verbatim in a scanned artifact's bytes. Held in `Zeroizing` wrappers and
    // never logged.
    let secret_redaction_values = resolved.redaction_values();
    let financial_envelope: Phase8FinancialEnvelopeEvidenceFile =
        read_operator_evidence_json_artifact(
            loaded,
            operator_evidence,
            "financial_envelope_path",
            "financial_envelope_sha256",
            &operator_evidence.financial_envelope_path,
            &operator_evidence.financial_envelope_sha256,
        )?;
    let approved_strategy_instance_id_hash = sha256_text(financial_envelope.strategy_instance_id());
    let nt_submit_event_path =
        resolve_loaded_config_path(loaded, &operator_evidence.nt_submit_event_path);
    let venue_order_state_path =
        resolve_loaded_config_path(loaded, &operator_evidence.venue_order_state_path);
    let restart_reconciliation_path =
        resolve_loaded_config_path(loaded, &operator_evidence.restart_reconciliation_path);
    let post_run_hygiene_path =
        resolve_loaded_config_path(loaded, &operator_evidence.post_run_hygiene_path);
    let terminal_artifacts =
        write_live_canary_terminal_result_artifact_refs(&LiveCanaryTerminalResultProofInputs {
            run_id: inputs.run_id,
            strategy_instance_id_hash: &approved_strategy_instance_id_hash,
            client_order_id: inputs.client_order_id,
            venue_order_id: inputs.venue_order_id,
            venue_order_outcome: inputs.venue_order_outcome,
            order_remains_open: inputs.order_remains_open,
            max_operator_evidence_file_bytes: operator_evidence.max_operator_evidence_file_bytes,
            scanned_artifact_paths: inputs.scanned_artifact_paths,
            secret_redaction_values: &secret_redaction_values,
            retention_purge_path: inputs.retention_purge_path,
            nt_submit_event_path: &nt_submit_event_path,
            venue_order_state_path: &venue_order_state_path,
            restart_reconciliation_path: &restart_reconciliation_path,
            post_run_hygiene_path: &post_run_hygiene_path,
        })?;
    let evidence_input = Phase8CanaryEvidenceInput {
        head_sha: operator_evidence.head_sha.clone(),
        root_config_sha256: root_toml_sha256_for_final_evidence(loaded)?,
        ssm_manifest_sha256: operator_evidence.ssm_manifest_sha256.clone(),
        ssm_manifest_ref: Phase8EvidenceRef {
            path_hash: sha256_text(&operator_evidence.ssm_manifest_path),
            record_hash: operator_evidence.ssm_manifest_sha256.clone(),
        },
        strategy_input_evidence_ref: Phase8EvidenceRef {
            path_hash: sha256_text(&operator_evidence.strategy_input_evidence_path),
            record_hash: operator_evidence.strategy_input_evidence_sha256.clone(),
        },
        approved_strategy_instance_id_hash: approved_strategy_instance_id_hash.clone(),
        approval_id: live_canary.approval_id.clone(),
        max_live_order_count: live_canary.max_live_order_count,
        max_notional_per_order: Decimal::from_str_exact(&live_canary.max_notional_per_order)?,
        runtime_capture_ref: Phase8RuntimeCaptureRef {
            spool_root_hash: sha256_text(&inputs.runtime_capture_spool_root.to_string_lossy()),
            run_id: inputs.run_id.to_string(),
        },
    };
    let decision_evidence_ref = configured_evidence_ref(
        loaded,
        operator_evidence,
        "decision_evidence_path",
        &operator_evidence.decision_evidence_path,
    )?;
    let strategy_cancel_ref = if inputs.order_remains_open {
        operator_evidence
            .strategy_cancel_path
            .as_deref()
            .map(|configured| {
                configured_evidence_ref(
                    loaded,
                    operator_evidence,
                    "strategy_cancel_path",
                    configured,
                )
            })
            .transpose()?
    } else {
        None
    };
    let live_order_ref = Phase8LiveOrderRef {
        strategy_instance_id_hash: approved_strategy_instance_id_hash,
        client_order_id_hash: sha256_text(inputs.client_order_id),
        venue_order_id_hash: sha256_text(inputs.venue_order_id),
    };
    let result_refs = Phase8LiveCanaryResultRefs {
        nt_submit_event_ref: Phase8EvidenceRef {
            path_hash: sha256_text(&operator_evidence.nt_submit_event_path),
            record_hash: terminal_artifacts.nt_submit_event.sha256.clone(),
        },
        venue_order_state_ref: Phase8EvidenceRef {
            path_hash: sha256_text(&operator_evidence.venue_order_state_path),
            record_hash: terminal_artifacts.venue_order_state.sha256.clone(),
        },
        strategy_cancel_ref,
        restart_reconciliation_ref: Phase8EvidenceRef {
            path_hash: sha256_text(&operator_evidence.restart_reconciliation_path),
            record_hash: terminal_artifacts.restart_reconciliation.sha256.clone(),
        },
        post_run_hygiene_ref: Phase8EvidenceRef {
            path_hash: sha256_text(&operator_evidence.post_run_hygiene_path),
            record_hash: terminal_artifacts.post_run_hygiene.sha256.clone(),
        },
    };
    let canary_evidence = Phase8CanaryEvidence::live_canary_proof(
        evidence_input,
        decision_evidence_ref,
        live_order_ref,
        result_refs,
        live_canary.max_live_order_count.saturating_mul(2),
    )?;
    let canary_evidence_path =
        resolve_loaded_config_path(loaded, &operator_evidence.canary_evidence_path);
    canary_evidence.write_json_file(&canary_evidence_path)?;
    let canary_evidence_sha256 = sha256_file_bounded(
        &canary_evidence_path,
        operator_evidence.max_operator_evidence_file_bytes,
    )?;
    Ok(LiveCanaryPostRunProofArtifacts {
        canary_evidence: WrittenOperatorArtifact {
            path: canary_evidence_path,
            sha256: canary_evidence_sha256,
        },
        nt_submit_event: terminal_artifacts.nt_submit_event,
        venue_order_state: terminal_artifacts.venue_order_state,
        restart_reconciliation: terminal_artifacts.restart_reconciliation,
        post_run_hygiene: terminal_artifacts.post_run_hygiene,
    })
}

fn write_live_canary_terminal_result_artifact_refs(
    inputs: &LiveCanaryTerminalResultProofInputs<'_>,
) -> anyhow::Result<LiveCanaryTerminalResultArtifacts> {
    validate_live_canary_terminal_result_inputs(inputs)?;
    let client_order_id_hash = sha256_text(inputs.client_order_id);
    let venue_order_id_hash = sha256_text(inputs.venue_order_id);
    // Single bounded read per scanned artifact: the same bytes are consumed to
    // both hash the artifact and scan it for raw secret residue. The hygiene
    // attestation `raw_secret_residue_absent` is computed from that scan — it is
    // the AND over all scanned artifacts of "none of this run's resolved-secret
    // values (`inputs.secret_redaction_values`) appears verbatim in the bytes".
    let mut scanned_artifact_hashes = Vec::with_capacity(inputs.scanned_artifact_paths.len());
    let mut raw_secret_residue_absent = true;
    for path in inputs.scanned_artifact_paths {
        let scanned = hash_and_scan_scanned_artifact_bounded(
            path,
            inputs.max_operator_evidence_file_bytes,
            inputs.secret_redaction_values,
        )?;
        if scanned.secret_residue_present {
            raw_secret_residue_absent = false;
        }
        scanned_artifact_hashes.push(scanned.sha256);
    }
    let nt_submit_event = write_json_artifact_create_new_or_read_existing(
        inputs.nt_submit_event_path,
        &LiveCanarySubmitEventProof {
            record_kind: LIVE_CANARY_NT_SUBMIT_EVENT_RECORD_KIND,
            run_id: inputs.run_id,
            strategy_instance_id_hash: inputs.strategy_instance_id_hash,
            client_order_id_hash: client_order_id_hash.clone(),
            venue_order_id_hash: venue_order_id_hash.clone(),
        },
        inputs.max_operator_evidence_file_bytes,
    )?;
    let venue_order_state = write_json_artifact_create_new_or_read_existing(
        inputs.venue_order_state_path,
        &LiveCanaryVenueOrderStateProof {
            record_kind: LIVE_CANARY_VENUE_ORDER_STATE_RECORD_KIND,
            run_id: inputs.run_id,
            strategy_instance_id_hash: inputs.strategy_instance_id_hash,
            client_order_id_hash: client_order_id_hash.clone(),
            venue_order_id_hash: venue_order_id_hash.clone(),
            venue_order_outcome: inputs.venue_order_outcome,
            order_remains_open: inputs.order_remains_open,
        },
        inputs.max_operator_evidence_file_bytes,
    )?;
    let restart_reconciliation = write_json_artifact_create_new_or_read_existing(
        inputs.restart_reconciliation_path,
        &LiveCanaryRestartReconciliationProof {
            record_kind: LIVE_CANARY_RESTART_RECONCILIATION_RECORD_KIND,
            source_run_id: inputs.run_id,
            strategy_instance_id_hash: inputs.strategy_instance_id_hash,
            client_order_id_hash: client_order_id_hash.clone(),
            venue_order_id_hash: venue_order_id_hash.clone(),
            venue_order_outcome: inputs.venue_order_outcome,
            order_remains_open: inputs.order_remains_open,
        },
        inputs.max_operator_evidence_file_bytes,
    )?;
    let post_run_hygiene = write_json_artifact_create_new_or_read_existing(
        inputs.post_run_hygiene_path,
        &LiveCanaryPostRunHygieneProof {
            record_kind: LIVE_CANARY_POST_RUN_HYGIENE_RECORD_KIND,
            run_id: inputs.run_id,
            strategy_instance_id_hash: inputs.strategy_instance_id_hash,
            client_order_id_hash,
            venue_order_id_hash,
            raw_secret_residue_absent,
            scanned_artifact_hashes,
            retention_purge_path_hash: sha256_text(&inputs.retention_purge_path.to_string_lossy()),
        },
        inputs.max_operator_evidence_file_bytes,
    )?;
    Ok(LiveCanaryTerminalResultArtifacts {
        nt_submit_event,
        venue_order_state,
        restart_reconciliation,
        post_run_hygiene,
    })
}

fn validate_live_canary_terminal_result_inputs(
    inputs: &LiveCanaryTerminalResultProofInputs<'_>,
) -> anyhow::Result<()> {
    if inputs.run_id.trim().is_empty() {
        return Err(anyhow!("live canary terminal result run_id is empty"));
    }
    if inputs.strategy_instance_id_hash.trim().is_empty() {
        return Err(anyhow!(
            "live canary terminal result strategy_instance_id_hash is empty"
        ));
    }
    if inputs.client_order_id.trim().is_empty() {
        return Err(anyhow!(
            "live canary terminal result client_order_id is empty"
        ));
    }
    if inputs.venue_order_id.trim().is_empty() {
        return Err(anyhow!(
            "live canary terminal result venue_order_id is empty"
        ));
    }
    match inputs.venue_order_outcome {
        LIVE_CANARY_TERMINAL_OUTCOME_FILLED | LIVE_CANARY_TERMINAL_OUTCOME_REJECTED => {}
        _ => {
            return Err(anyhow!(
                "live canary terminal result venue_order_outcome must be terminal"
            ));
        }
    }
    if inputs.order_remains_open {
        return Err(anyhow!(
            "live canary terminal result order_remains_open must be false"
        ));
    }
    if inputs.max_operator_evidence_file_bytes == 0 {
        return Err(anyhow!(
            "live canary terminal result max_operator_evidence_file_bytes must be positive"
        ));
    }
    if inputs.scanned_artifact_paths.is_empty() {
        return Err(anyhow!(
            "live canary terminal result scanned_artifact_paths is empty"
        ));
    }
    Ok(())
}

fn configured_evidence_ref(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
    path_field: &'static str,
    configured_path: &str,
) -> anyhow::Result<Phase8EvidenceRef> {
    Ok(Phase8EvidenceRef {
        path_hash: sha256_text(configured_path),
        record_hash: read_final_evidence_sha256(
            loaded,
            operator_evidence,
            path_field,
            configured_path,
        )?,
    })
}

fn sha256_file_bounded(path: &Path, max_bytes: u64) -> anyhow::Result<String> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        anyhow!(
            "failed to hash bounded live canary evidence file `{}`: {source}",
            path.display()
        )
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// SHA-256 hex digest plus the secret-residue verdict for a single scanned
/// live-canary artifact, both derived from one bounded read of the file bytes.
struct HashedScannedArtifact {
    sha256: String,
    secret_residue_present: bool,
}

/// Read a bounded scanned artifact once and derive both its SHA-256 hash and
/// whether its bytes contain any of this run's resolved-secret values
/// (`secret_redaction_values`, produced by the single secret source of truth
/// [`ResolvedBoltV3Secrets::redaction_values`]). The bytes are read a single
/// time and consumed for both the hash and the scan — never re-read — and the
/// matched bytes are never returned, logged, or surfaced; only the boolean
/// verdict escapes this function.
fn hash_and_scan_scanned_artifact_bounded(
    path: &Path,
    max_bytes: u64,
    secret_redaction_values: &[Zeroizing<String>],
) -> anyhow::Result<HashedScannedArtifact> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        anyhow!(
            "failed to hash bounded live canary evidence file `{}`: {source}",
            path.display()
        )
    })?;
    let secret_residue_present = bytes_contain_any_secret_value(&bytes, secret_redaction_values);
    Ok(HashedScannedArtifact {
        sha256: hex::encode(Sha256::digest(&bytes)),
        secret_residue_present,
    })
}

/// Return `true` if `bytes` contain any of `secret_values` verbatim as a
/// contiguous byte subsequence. This is the single source of truth for the
/// post-run hygiene residue verdict: a scanned artifact leaks raw secret
/// material iff one of the exact resolved-secret values this run handled appears
/// in its bytes. Empty secret values are skipped (they would match everywhere
/// and never represent real residue). Never logs or returns the matched bytes.
fn bytes_contain_any_secret_value(bytes: &[u8], secret_values: &[Zeroizing<String>]) -> bool {
    secret_values
        .iter()
        .map(|value| value.as_bytes())
        .filter(|value| !value.is_empty())
        .any(|value| byte_slice_contains(bytes, value))
}

/// `true` iff `haystack` contains `needle` as a contiguous byte subsequence.
/// Callers filter out empty `needle`s before calling; an empty `needle` here
/// returns `false` (an empty secret value never represents real residue).
fn byte_slice_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn write_json_artifact_create_new_or_read_existing<T: Serialize>(
    path: &Path,
    value: &T,
    max_existing_bytes: u64,
) -> anyhow::Result<WrittenOperatorArtifact> {
    match write_json_artifact_create_new(path, value) {
        Ok(written) => Ok(written),
        Err(BoltV3OperatorArtifactError::Write { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            let expected_bytes = serde_json::to_vec_pretty(value)?;
            let actual_bytes = read_file_bounded(path, max_existing_bytes).map_err(|source| {
                anyhow!(
                    "failed to read existing live canary evidence file `{}`: {source}",
                    path.display()
                )
            })?;
            if actual_bytes != expected_bytes {
                return Err(anyhow!(
                    "existing live canary evidence file `{}` differs from requested proof",
                    path.display()
                ));
            }
            Ok(WrittenOperatorArtifact {
                path: path.to_path_buf(),
                sha256: hex::encode(Sha256::digest(actual_bytes)),
            })
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_json_artifact_create_new<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(BoltV3OperatorArtifactError::Serialize)?;
    write_json_artifact_create_new_from_bytes(path, &bytes)
}

fn write_json_artifact_create_new_from_bytes(
    path: &Path,
    bytes: &[u8],
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    write_json_artifact_create_new_from_bytes_with_file(
        path,
        bytes,
        open_json_artifact_create_new_file,
    )
}

trait ArtifactFile {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn sync_all(&self) -> io::Result<()>;
}

impl ArtifactFile for fs::File {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        Write::write_all(self, bytes)
    }

    fn sync_all(&self) -> io::Result<()> {
        fs::File::sync_all(self)
    }
}

fn open_json_artifact_create_new_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    configure_private_artifact_create_options(&mut options);
    options.open(path)
}

#[cfg(test)]
fn write_json_artifact_create_new_with_file<T, F, File>(
    path: &Path,
    value: &T,
    open_file: F,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError>
where
    T: Serialize,
    F: FnOnce(&Path) -> io::Result<File>,
    File: ArtifactFile,
{
    let bytes = serde_json::to_vec_pretty(value).map_err(BoltV3OperatorArtifactError::Serialize)?;
    write_json_artifact_create_new_from_bytes_with_file(path, &bytes, open_file)
}

fn write_json_artifact_create_new_from_bytes_with_file<F, File>(
    path: &Path,
    bytes: &[u8],
    open_file: F,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError>
where
    F: FnOnce(&Path) -> io::Result<File>,
    File: ArtifactFile,
{
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| BoltV3OperatorArtifactError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut file = open_file(path).map_err(|source| BoltV3OperatorArtifactError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    if let Err(source) = file.write_all(bytes) {
        let _ = fs::remove_file(path);
        return Err(BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    if let Err(source) = file.sync_all() {
        let _ = fs::remove_file(path);
        return Err(BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(WrittenOperatorArtifact {
        path: path.to_path_buf(),
        sha256: hex::encode(Sha256::digest(bytes)),
    })
}

#[cfg(unix)]
fn configure_private_artifact_create_options(options: &mut fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options
        .mode(PRIVATE_ARTIFACT_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn configure_private_artifact_create_options(_options: &mut fs::OpenOptions) {}

fn ensure_output_path_absent(path: &Path) -> Result<(), BoltV3OperatorArtifactError> {
    if path.exists() {
        return Err(BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "operator artifact already exists",
            ),
        });
    }
    Ok(())
}

fn validate_output_parent(
    field: &'static str,
    path: &Path,
) -> Result<(), BoltV3OperatorArtifactError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    for ancestor in parent.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::metadata(ancestor) {
            Ok(metadata) => {
                return if metadata.is_dir() {
                    Ok(())
                } else {
                    Err(BoltV3OperatorArtifactError::InvalidOutputPathParent { field })
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(BoltV3OperatorArtifactError::InvalidOutputPathParent { field }),
        }
    }
    Ok(())
}

fn output_paths_collide(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (
        canonical_existing_parent_path(left),
        canonical_existing_parent_path(right),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn canonical_existing_parent_path(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    let canonical_parent = fs::canonicalize(parent).ok()?;
    Some(normalize_path_components(&canonical_parent.join(file_name)))
}

pub(crate) fn json_artifact_sha256<T: Serialize>(
    value: &T,
) -> Result<String, BoltV3OperatorArtifactError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(BoltV3OperatorArtifactError::Serialize)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn sha256_file_for_static_manifest(
    name: &'static str,
    path: &Path,
    max_bytes: u64,
) -> Result<String, BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::StaticManifestArtifactFileRead {
            name,
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(crate) fn read_file_bounded(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut file = open_regular_artifact_file(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let length = bytes.len() as u64;
    if length > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "operator artifact exceeds max_operator_evidence_file_bytes={max_bytes} bytes (length={length})"
            ),
        ));
    }
    Ok(bytes)
}

fn open_regular_artifact_file(path: &Path) -> std::io::Result<fs::File> {
    let pre_open_metadata = fs::symlink_metadata(path)?;
    validate_operator_artifact_regular_file(&pre_open_metadata)?;
    let file = open_artifact_file_no_follow(path)?;
    let opened_metadata = file.metadata()?;
    validate_operator_artifact_regular_file(&opened_metadata)?;
    validate_same_artifact_file(&pre_open_metadata, &opened_metadata)?;
    let post_open_metadata = fs::symlink_metadata(path)?;
    validate_operator_artifact_regular_file(&post_open_metadata)?;
    validate_same_artifact_file(&opened_metadata, &post_open_metadata)?;
    Ok(file)
}

#[cfg(unix)]
fn open_artifact_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_artifact_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().read(true).open(path)
}

fn validate_operator_artifact_regular_file(metadata: &fs::Metadata) -> std::io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "operator artifact path is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_same_artifact_file(left: &fs::Metadata, right: &fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if left.dev() != right.dev() || left.ino() != right.ino() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "operator artifact path changed during open",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_same_artifact_file(_left: &fs::Metadata, _right: &fs::Metadata) -> std::io::Result<()> {
    Ok(())
}

fn validate_operator_evidence_sha256(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if is_lowercase_sha256(value) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::InvalidOperatorEvidenceHash { field })
    }
}

fn validate_live_canary_operator_evidence_toml_patch(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_operator_evidence_build_head(evidence)?;
    if evidence.max_operator_evidence_file_bytes == 0 {
        return Err(BoltV3OperatorArtifactError::OperatorEvidenceTomlInvalid {
            field: "max_operator_evidence_file_bytes",
        });
    }
    if evidence.approval_consumption_max_age_seconds == 0 {
        return Err(BoltV3OperatorArtifactError::OperatorEvidenceTomlInvalid {
            field: "approval_consumption_max_age_seconds",
        });
    }
    if evidence.approval_not_after_unix_seconds <= evidence.approval_not_before_unix_seconds {
        return Err(BoltV3OperatorArtifactError::OperatorEvidenceTomlInvalid {
            field: "approval_not_after_unix_seconds",
        });
    }
    let gate_session_path = required_operator_evidence_field(
        OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD,
        evidence.gate_session_path.as_deref(),
    )?;
    let expected_gate_session_sha256 = required_operator_evidence_field(
        OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
        evidence.expected_gate_session_sha256.as_deref(),
    )?;
    for (field, value) in [
        (
            "approval_envelope_sha256",
            evidence.approval_envelope_sha256.as_str(),
        ),
        ("ssm_manifest_sha256", evidence.ssm_manifest_sha256.as_str()),
        (
            "strategy_input_evidence_sha256",
            evidence.strategy_input_evidence_sha256.as_str(),
        ),
        (
            "financial_envelope_sha256",
            evidence.financial_envelope_sha256.as_str(),
        ),
        (
            "pre_run_state_sha256",
            evidence.pre_run_state_sha256.as_str(),
        ),
        ("abort_plan_sha256", evidence.abort_plan_sha256.as_str()),
        (
            "approval_nonce_sha256",
            evidence.approval_nonce_sha256.as_str(),
        ),
        (
            OPERATOR_EVIDENCE_EXPECTED_GATE_SESSION_SHA256_FIELD,
            expected_gate_session_sha256,
        ),
    ] {
        validate_operator_evidence_sha256(field, value)?;
    }
    for (field, value) in [
        (
            "canary_proof_candidate_source_sha256",
            evidence.canary_proof_candidate_source_sha256.as_deref(),
        ),
        (
            "canary_proof_order_intent_sha256",
            evidence.canary_proof_order_intent_sha256.as_deref(),
        ),
    ] {
        if let Some(value) = value {
            validate_operator_evidence_sha256(field, value)?;
        }
    }
    for (field, value) in [
        (
            "approval_envelope_path",
            evidence.approval_envelope_path.as_str(),
        ),
        ("ssm_manifest_path", evidence.ssm_manifest_path.as_str()),
        (
            "strategy_input_evidence_path",
            evidence.strategy_input_evidence_path.as_str(),
        ),
        (OPERATOR_EVIDENCE_GATE_SESSION_PATH_FIELD, gate_session_path),
        (
            "financial_envelope_path",
            evidence.financial_envelope_path.as_str(),
        ),
        ("pre_run_state_path", evidence.pre_run_state_path.as_str()),
        ("abort_plan_path", evidence.abort_plan_path.as_str()),
        (
            "canary_evidence_path",
            evidence.canary_evidence_path.as_str(),
        ),
        ("approval_nonce_path", evidence.approval_nonce_path.as_str()),
        (
            "approval_consumption_path",
            evidence.approval_consumption_path.as_str(),
        ),
        (
            "decision_evidence_path",
            evidence.decision_evidence_path.as_str(),
        ),
        (
            "nt_submit_event_path",
            evidence.nt_submit_event_path.as_str(),
        ),
        (
            "venue_order_state_path",
            evidence.venue_order_state_path.as_str(),
        ),
        (
            "restart_reconciliation_path",
            evidence.restart_reconciliation_path.as_str(),
        ),
        (
            "post_run_hygiene_path",
            evidence.post_run_hygiene_path.as_str(),
        ),
    ] {
        validate_operator_evidence_toml_path(field, value)?;
    }
    if let Some(strategy_cancel_path) = evidence.strategy_cancel_path.as_deref() {
        validate_operator_evidence_toml_path("strategy_cancel_path", strategy_cancel_path)?;
    }
    for (field, value) in [
        (
            "canary_proof_candidate_source_path",
            evidence.canary_proof_candidate_source_path.as_deref(),
        ),
        (
            "canary_proof_order_intent_path",
            evidence.canary_proof_order_intent_path.as_deref(),
        ),
    ] {
        if let Some(value) = value {
            validate_operator_evidence_toml_path(field, value)?;
        }
    }
    Ok(())
}

fn required_operator_evidence_field<'a>(
    field: &'static str,
    value: Option<&'a str>,
) -> Result<&'a str, BoltV3OperatorArtifactError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or(BoltV3OperatorArtifactError::OperatorEvidenceTomlInvalid { field })
}

fn validate_operator_evidence_toml_path(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.trim().is_empty() {
        return Err(BoltV3OperatorArtifactError::InvalidOutputPath { field });
    }
    validate_output_path_shape(field, value)
}

pub(crate) fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|char| matches!(char, '0'..='9' | 'a'..='f'))
}

fn validate_output_path_shape(
    field: &'static str,
    configured: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_output_path_components(field, Path::new(configured.trim()))
}

fn validate_output_path_components(
    field: &'static str,
    path: &Path,
) -> Result<(), BoltV3OperatorArtifactError> {
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(BoltV3OperatorArtifactError::InvalidOutputPath { field });
    }
    Ok(())
}

fn patch_live_canary_operator_evidence_toml(
    root_text: &str,
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<String, BoltV3OperatorArtifactError> {
    if !root_text
        .lines()
        .any(|line| toml_table_header(line) == Some("[live_canary]"))
    {
        return Err(BoltV3OperatorArtifactError::MissingLiveCanary);
    }
    let rendered = render_live_canary_operator_evidence_toml(evidence)?;
    Ok(replace_or_append_toml_table(
        root_text,
        "[live_canary.operator_evidence]",
        rendered.as_str(),
    ))
}

fn replace_or_append_toml_table(
    root_text: &str,
    table_header: &'static str,
    rendered_table: &str,
) -> String {
    let mut line_starts = vec![0usize];
    for (index, char) in root_text.char_indices() {
        if char == '\n' {
            line_starts.push(index + 1);
        }
    }
    let table_start = line_starts
        .iter()
        .enumerate()
        .find_map(|(line_index, start)| {
            let end = line_end(root_text, &line_starts, line_index);
            (toml_table_header(&root_text[*start..end]) == Some(table_header)).then_some(line_index)
        });

    let Some(start_line) = table_start else {
        let mut patched = root_text.to_string();
        if !patched.ends_with('\n') {
            patched.push('\n');
        }
        patched.push_str(rendered_table);
        return patched;
    };

    let mut end_line = line_starts.len();
    for line_index in (start_line + 1)..line_starts.len() {
        let start = line_starts[line_index];
        let end = line_end(root_text, &line_starts, line_index);
        if toml_table_header(&root_text[start..end]).is_some() {
            end_line = line_index;
            break;
        }
    }
    let start_byte = line_starts[start_line];
    let end_byte = if end_line < line_starts.len() {
        line_starts[end_line]
    } else {
        root_text.len()
    };
    let mut patched = String::with_capacity(root_text.len() + rendered_table.len());
    patched.push_str(&root_text[..start_byte]);
    patched.push_str(rendered_table);
    patched.push_str(&root_text[end_byte..]);
    patched
}

fn line_end(root_text: &str, line_starts: &[usize], line_index: usize) -> usize {
    line_starts
        .get(line_index + 1)
        .copied()
        .unwrap_or(root_text.len())
}

fn toml_table_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let before_comment = trimmed
        .split_once('#')
        .map(|(value, _)| value.trim())
        .unwrap_or(trimmed);
    (before_comment.starts_with('[') && before_comment.ends_with(']')).then_some(before_comment)
}

fn render_live_canary_operator_evidence_toml(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<String, BoltV3OperatorArtifactError> {
    let body = toml::to_string(evidence)
        .map_err(|source| BoltV3OperatorArtifactError::OperatorEvidenceTomlSerialize { source })?;
    Ok(format!("[live_canary.operator_evidence]\n{body}"))
}

fn resolve_loaded_config_path(loaded: &LoadedBoltV3Config, configured_path: &str) -> PathBuf {
    let path = Path::new(configured_path.trim());
    resolve_loaded_config_path_from_path(loaded, path)
}

fn resolve_loaded_config_path_from_path(loaded: &LoadedBoltV3Config, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize_path_components(path);
    }
    normalize_path_components(
        &loaded
            .root_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path),
    )
}

fn resolve_peer_artifact_path(anchor_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize_path_components(path);
    }
    normalize_path_components(
        &anchor_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path),
    )
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn sha256_text(value: &str) -> String {
    crate::bolt_v3_source_integrity::sha256_hex_lower(value.as_bytes())
}

fn static_artifact_ref(
    name: &'static str,
    written: WrittenOperatorArtifact,
) -> BoltV3StaticArtifactRef {
    BoltV3StaticArtifactRef {
        name,
        path: written.path.to_string_lossy().to_string(),
        sha256: written.sha256,
    }
}

fn static_artifact_summary_ref(
    artifact: &BoltV3StaticArtifactRef,
) -> BoltV3StaticArtifactSummaryRef {
    BoltV3StaticArtifactSummaryRef {
        path: artifact.path.clone(),
        sha256: artifact.sha256.clone(),
    }
}

fn written_artifact_summary_ref(
    written: WrittenOperatorArtifact,
) -> BoltV3StaticArtifactSummaryRef {
    BoltV3StaticArtifactSummaryRef {
        path: written.path.to_string_lossy().to_string(),
        sha256: written.sha256,
    }
}

fn final_packet_summary_artifact(
    name: &'static str,
    sha256: &str,
) -> BoltV3FinalOperatorPacketVerificationArtifactSummary {
    BoltV3FinalOperatorPacketVerificationArtifactSummary {
        name,
        sha256: sha256.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SyncFailingArtifactFile {
        file: fs::File,
    }

    impl ArtifactFile for SyncFailingArtifactFile {
        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            std::io::Write::write_all(&mut self.file, bytes)
        }

        fn sync_all(&self) -> io::Result<()> {
            Err(io::Error::other("forced sync failure"))
        }
    }

    #[test]
    fn abort_plan_strategy_manifest_dir_resolves_caller_checkout_primary_root() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let manifest_dir = temp.path().join("caller-checkout");
        let strategy_source_path = manifest_dir.join(registry_relative_root(STRATEGY_KEY));
        fs::create_dir_all(&strategy_source_path).expect("strategy source dir should create");

        let resolved = abort_plan_strategy_manifest_dir(&strategy_source_path)
            .expect("registered primary root should resolve manifest dir");

        assert_eq!(
            resolved,
            fs::canonicalize(&manifest_dir).expect("manifest dir should canonicalize")
        );
    }

    #[test]
    fn abort_plan_strategy_manifest_dir_rejects_non_primary_tail() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let manifest_dir = temp.path().join("caller-checkout");
        let wrong_source_path = manifest_dir.join("src/strategies/not_the_registered_strategy");
        fs::create_dir_all(&wrong_source_path).expect("wrong strategy dir should create");

        let error = abort_plan_strategy_manifest_dir(&wrong_source_path)
            .expect_err("non-primary root must fail before source-set collection");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            error.to_string().contains("registered primary root"),
            "error should explain the expected root: {error}"
        );
    }

    #[test]
    fn registered_relative_root_tail_uses_components_not_separator_string() {
        let components = registered_relative_root_components(registry_relative_root(STRATEGY_KEY))
            .expect("registered primary root should parse");
        let strategy_source_path = Path::new("/tmp/reviewed-checkout")
            .join("src")
            .join("strategies")
            .join("binary_oracle_edge_taker");

        assert!(path_has_registered_relative_root_tail(
            &strategy_source_path,
            &components
        ));
        assert!(!path_has_registered_relative_root_tail(
            Path::new("/tmp/reviewed-checkout/src/strategies/binary_oracle_edge_taker_extra"),
            &components
        ));
    }

    #[test]
    fn json_artifact_writer_removes_create_new_output_when_sync_fails() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("approval-nonce.json");
        let artifact = BoltV3ApprovalNonceArtifact {
            schema_version: APPROVAL_NONCE_SCHEMA_VERSION,
            record_kind: APPROVAL_NONCE_RECORD_KIND,
            nonce_sha256: "0".repeat(64),
        };

        let error = write_json_artifact_create_new_with_file(&path, &artifact, |path| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(|file| SyncFailingArtifactFile { file })
        })
        .expect_err("sync failure must fail the artifact write");

        assert!(matches!(error, BoltV3OperatorArtifactError::Write { .. }));
        assert!(
            !path.exists(),
            "sync failure must remove the partially-written final artifact path"
        );
    }

    #[test]
    fn live_canary_terminal_result_writer_hashes_ids_and_writes_receipt_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let scanned_artifact = temp.path().join("order-events.jsonl");
        fs::write(&scanned_artifact, b"accepted order event\n").expect("scan input should write");
        let retention_path = temp.path().join("retention-purge.txt");
        // A clean artifact scanned against a real (non-empty) secret-value set
        // whose values do NOT appear in the bytes must attest absent = true.
        // The synthetic value is never a real credential.
        let secret_values = vec![Zeroizing::new(
            "BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_FAKE".to_string(),
        )];

        let written = write_live_canary_terminal_result_artifacts(
            &LiveCanaryTerminalResultProofInputs {
                run_id: "live-run-001",
                strategy_instance_id_hash: &sha256_text("canary-proof-executor-proof"),
                client_order_id: "O-20260529-153130-001-proof-1",
                venue_order_id: "0x31c0fd542faa4a9af561602ee8f302a4aaf838a04ea87068d2cfa048e2be60f5",
                venue_order_outcome: "filled",
                order_remains_open: false,
                max_operator_evidence_file_bytes: 1024,
                scanned_artifact_paths: std::slice::from_ref(&scanned_artifact),
                secret_redaction_values: &secret_values,
                retention_purge_path: &retention_path,
                nt_submit_event_path: &temp.path().join("nt-submit-event.json"),
                venue_order_state_path: &temp.path().join("venue-order-state.json"),
                restart_reconciliation_path: &temp.path().join("restart-reconciliation.json"),
                post_run_hygiene_path: &temp.path().join("post-run-hygiene.json"),
            },
        )
        .expect("terminal result artifacts should write");

        assert_eq!(written.len(), 4);
        let client_hash = sha256_text("O-20260529-153130-001-proof-1");
        let venue_hash =
            sha256_text("0x31c0fd542faa4a9af561602ee8f302a4aaf838a04ea87068d2cfa048e2be60f5");
        let venue_state: serde_json::Value = serde_json::from_slice(
            &fs::read(temp.path().join("venue-order-state.json")).expect("venue state should read"),
        )
        .expect("venue state should parse");
        assert_eq!(venue_state["client_order_id_hash"], client_hash);
        assert_eq!(venue_state["venue_order_id_hash"], venue_hash);
        assert_eq!(venue_state["venue_order_outcome"], "filled");
        assert_eq!(venue_state["order_remains_open"], false);

        let post_hygiene: serde_json::Value = serde_json::from_slice(
            &fs::read(temp.path().join("post-run-hygiene.json")).expect("post hygiene should read"),
        )
        .expect("post hygiene should parse");
        assert_eq!(post_hygiene["raw_secret_residue_absent"], true);
        let expected_scan_hash = hex::encode(Sha256::digest(
            fs::read(&scanned_artifact).expect("scan input should read"),
        ));
        assert_eq!(
            post_hygiene["scanned_artifact_hashes"][0],
            expected_scan_hash
        );
        assert_eq!(
            post_hygiene["retention_purge_path_hash"],
            sha256_text(&retention_path.to_string_lossy())
        );
    }

    #[test]
    fn live_canary_post_run_hygiene_flags_planted_secret_residue_in_scanned_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let scanned_artifact = temp.path().join("order-events.jsonl");
        // Plant an exact resolved-secret value into the scanned artifact bytes,
        // then scan against that same value. The value is a clearly-fake
        // synthetic sentinel, never a real credential.
        let leaked_secret = "BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_FAKE";
        let planted = format!("accepted order event\nleaked={leaked_secret}\n");
        fs::write(&scanned_artifact, planted.as_bytes()).expect("planted scan input should write");
        let retention_path = temp.path().join("retention-purge.txt");
        let secret_values = vec![Zeroizing::new(leaked_secret.to_string())];

        write_live_canary_terminal_result_artifacts(&LiveCanaryTerminalResultProofInputs {
            run_id: "live-run-residue-001",
            strategy_instance_id_hash: &sha256_text("canary-proof-executor-proof"),
            client_order_id: "O-20260529-153130-001-proof-9",
            venue_order_id: "0x31c0fd542faa4a9af561602ee8f302a4aaf838a04ea87068d2cfa048e2be60f5",
            venue_order_outcome: "filled",
            order_remains_open: false,
            max_operator_evidence_file_bytes: 1024,
            scanned_artifact_paths: std::slice::from_ref(&scanned_artifact),
            secret_redaction_values: &secret_values,
            retention_purge_path: &retention_path,
            nt_submit_event_path: &temp.path().join("nt-submit-event.json"),
            venue_order_state_path: &temp.path().join("venue-order-state.json"),
            restart_reconciliation_path: &temp.path().join("restart-reconciliation.json"),
            post_run_hygiene_path: &temp.path().join("post-run-hygiene.json"),
        })
        .expect("terminal result artifacts should write");

        let post_hygiene: serde_json::Value = serde_json::from_slice(
            &fs::read(temp.path().join("post-run-hygiene.json")).expect("post hygiene should read"),
        )
        .expect("post hygiene should parse");
        // The scan must catch the planted secret value: a hardcoded `true` here
        // would fail this assertion, which is exactly what guards against
        // regressing the attestation back to an unconditional value.
        assert_eq!(post_hygiene["raw_secret_residue_absent"], false);
        // The artifact is still hashed from the same single read.
        let expected_scan_hash = hex::encode(Sha256::digest(
            fs::read(&scanned_artifact).expect("scan input should read"),
        ));
        assert_eq!(
            post_hygiene["scanned_artifact_hashes"][0],
            expected_scan_hash
        );
    }

    #[test]
    fn bytes_contain_any_secret_value_discriminates_clean_from_leaked() {
        let leaked = Zeroizing::new("BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_FAKE".to_string());
        let other = Zeroizing::new("BOLTV3_API_SECRET_SENTINEL_DO_NOT_LEAK_FAKE".to_string());
        let secret_values = vec![leaked.clone(), other];

        // Clean operator-evidence shapes — plain text, a bare 64-hex sha256
        // digest, and a `0x`-prefixed on-chain identifier — must NOT be flagged
        // when none of the secret values appear in them.
        assert!(!bytes_contain_any_secret_value(
            b"accepted order event\n",
            &secret_values
        ));
        assert!(!bytes_contain_any_secret_value(
            sha256_text("order-id").as_bytes(),
            &secret_values
        ));
        assert!(!bytes_contain_any_secret_value(
            b"0x31c0fd542faa4a9af561602ee8f302a4aaf838a04ea87068d2cfa048e2be60f5",
            &secret_values
        ));
        // An empty secret-value set can never flag residue.
        assert!(!bytes_contain_any_secret_value(
            b"key=BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_FAKE",
            &[]
        ));
        // A secret value embedded mid-stream must be caught.
        assert!(bytes_contain_any_secret_value(
            b"prefix key=BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_FAKE suffix",
            &secret_values
        ));
        // Matching against a single value works too.
        assert!(bytes_contain_any_secret_value(
            b"key=BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_FAKE",
            std::slice::from_ref(&leaked)
        ));
        // Empty secret values inside the set are skipped and never match.
        assert!(!bytes_contain_any_secret_value(
            b"anything at all",
            std::slice::from_ref(&Zeroizing::new(String::new()))
        ));
    }

    #[test]
    fn strategy_cancel_ref_can_be_absent_when_venue_order_is_terminal_closed() {
        let terminal_closed = serde_json::json!({
            "venue_order_outcome": "filled",
            "order_remains_open": false
        });
        verify_strategy_cancel_absent_for_terminal_closed_order_value(&terminal_closed)
            .expect("closed terminal order should not need a cancel proof");

        let still_open = serde_json::json!({
            "venue_order_outcome": "accepted",
            "order_remains_open": true
        });
        let error = verify_strategy_cancel_absent_for_terminal_closed_order_value(&still_open)
            .expect_err("open order should still require cancel proof");
        assert!(matches!(
            error,
            BoltV3OperatorArtifactError::FinalEvidenceMismatch {
                field: "strategy_cancel_ref"
            }
        ));
    }
}
