use std::{
    error::Error,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use nautilus_model::instruments::InstrumentAny;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{
    bolt_v3_config::{LiveCanaryOperatorEvidenceBlock, LoadedBoltV3Config},
    bolt_v3_decision_evidence::{
        BoltV3StrategyInputEvidenceSnapshot, read_latest_entry_decision_evidence_chain,
    },
    bolt_v3_live_canary_gate::{
        APPROVAL_ENVELOPE_RECORD_KIND, APPROVAL_ENVELOPE_SCHEMA_VERSION,
        Phase8OperatorApprovalEnvelopeFile, current_build_head_sha,
    },
    bolt_v3_market_families::{self, MarketSelectionTarget},
    bolt_v3_providers::{ProviderSecretResolveContext, binding_for_provider_key},
    bolt_v3_secrets::BoltV3SecretError,
    bolt_v3_tiny_canary_evidence::{
        Phase8AbortPlanEvidenceFile, Phase8AbortPlanSourceProofs,
        Phase8FinancialEnvelopeEvidenceFile, Phase8MarketSelectionSourceEvidenceFile,
        Phase8PreRunStateEvidenceFile, Phase8PreRunStateSourceProofs,
        Phase8StrategyInputEvidenceFile, Phase8StrategyInputSafetyAudit,
    },
};

const REDACTED_SSM_MANIFEST_SCHEMA_VERSION: u32 = 1;
const REDACTED_SSM_MANIFEST_RECORD_KIND: &str = "bolt_v3.redacted_ssm_manifest.v1";
const APPROVAL_NONCE_SCHEMA_VERSION: u32 = 1;
const APPROVAL_NONCE_RECORD_KIND: &str = "bolt_v3.operator_approval_nonce.v1";
const APPROVAL_NONCE_BYTES: usize = 32;
const STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION: u32 = 1;
const STATIC_ARTIFACTS_MANIFEST_RECORD_KIND: &str = "bolt_v3.static_operator_artifacts_manifest.v1";
const PRE_RUN_STATE_SOURCE_PROOF_BUNDLE_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_STATE_SOURCE_PROOF_BUNDLE_RECORD_KIND: &str =
    "bolt_v3.pre_run_state_source_proof_bundle.v1";
const ABORT_PLAN_SOURCE_PROOF_BUNDLE_SCHEMA_VERSION: u32 = 1;
const ABORT_PLAN_SOURCE_PROOF_BUNDLE_RECORD_KIND: &str =
    "bolt_v3.abort_plan_source_proof_bundle.v1";
const SSM_MANIFEST_ARTIFACT_NAME: &str = "ssm-manifest";
const FINANCIAL_ENVELOPE_ARTIFACT_NAME: &str = "financial-envelope";
const STRATEGY_INPUT_ARTIFACT_NAME: &str = "strategy-input";
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
const PRE_RUN_MARKET_WINDOW_SOURCE_PROOF_SCHEMA_VERSION: u32 = 1;
const PRE_RUN_MARKET_WINDOW_SOURCE_PROOF_RECORD_KIND: &str =
    "bolt_v3.pre_run_market_window_source_proof.v1";
const BUILD_CARGO_TOML: &str = include_str!("../Cargo.toml");
const NAUTILUS_TRADER_GIT_URL: &str = "https://github.com/nautechsystems/nautilus_trader.git";
const NAUTILUS_TRADER_CARGO_LOCK_SOURCE_PREFIX: &str =
    "git+https://github.com/nautechsystems/nautilus_trader.git";
const MARKET_SELECTION_SOURCE_BLOCKER: &str = "market-selection remains blocked: T046 missing source-bound price-to-beat strategy decision input";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StaticArtifactsCommandSummary {
    pub generated_artifacts: Vec<BoltV3StaticArtifactSummaryRef>,
    pub manifest_artifact: BoltV3StaticArtifactSummaryRef,
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
    pub financial_envelope_path: String,
    pub financial_envelope_sha256: String,
    pub pre_run_state_path: String,
    pub pre_run_state_sha256: String,
    pub abort_plan_path: String,
    pub abort_plan_sha256: String,
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

#[derive(Clone, PartialEq, Eq)]
pub struct WrittenOperatorArtifact {
    pub path: PathBuf,
    pub sha256: String,
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
pub struct Phase8PreRunMarketWindowSourceProof {
    pub market_state_approved: bool,
    pub market_window_approved: bool,
    pub market_state_evidence_hash: String,
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
    SecretInventory(BoltV3SecretError),
    FinancialEnvelope(anyhow::Error),
    MarketSelection(anyhow::Error),
    MarketSelectionPrerequisiteUnproven {
        prerequisite: &'static str,
    },
    StrategyInputPrerequisiteUnproven {
        prerequisite: &'static str,
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
    PreRunMarketWindowSourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    PreRunMarketWindowSourceInvalid {
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
            Self::PreRunMarketWindowSourceRead { source, .. } => {
                write!(f, "failed to read market/window source input: {source}")
            }
            Self::PreRunMarketWindowSourceInvalid { field } => write!(
                f,
                "market/window source field `{field}` is invalid or unproven"
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
            Self::SecretInventory(error) => Some(error),
            Self::FinancialEnvelope(error) => Some(error.as_ref()),
            Self::MarketSelection(error) => Some(error.as_ref()),
            Self::MarketSelectionSourceRead { source, .. } => Some(source),
            Self::MarketSelectionSourceParse { source, .. } => Some(source),
            Self::MarketSelectionInstrumentSourceRead { source, .. } => Some(source),
            Self::MarketSelectionInstrumentSourceParse { source, .. } => Some(source),
            Self::PreRunReleaseManifestSourceRead { source, .. } => Some(source),
            Self::PreRunMarketWindowSourceRead { source, .. } => Some(source),
            Self::PreRunStateSourceBundleRead { source, .. } => Some(source),
            Self::PreRunStateSourceBundleParse { source, .. } => Some(source),
            Self::AbortPlanSourceBundleRead { source, .. } => Some(source),
            Self::AbortPlanSourceBundleParse { source, .. } => Some(source),
            Self::StaticManifestRead { source, .. } => Some(source),
            Self::StaticManifestParse { source, .. } => Some(source),
            Self::StaticManifestArtifactFileRead { source, .. } => Some(source),
            Self::OperatorPacketRead { source, .. } => Some(source),
            Self::OperatorPacketParse { source, .. } => Some(source),
            Self::ApprovalEnvelopeRead { source, .. } => Some(source),
            Self::ApprovalEnvelopeParse { source, .. } => Some(source),
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

pub fn build_phase8_financial_envelope(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> anyhow::Result<Phase8FinancialEnvelopeEvidenceFile> {
    Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
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
    if chain.snapshot.configured_target_id != financial_envelope.configured_target_id() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy decision target does not match config",
            },
        );
    }
    if chain.snapshot.price_to_beat_source != financial_envelope.price_to_beat_source() {
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
            &artifact,
            path.to_string_lossy(),
            &source_sha256,
            &[],
        )
        .map_err(|_| BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
            prerequisite: "T046 remains blocked: market selection source does not match source-bound strategy decision input",
    })?;
    write_json_artifact_create_new(path, &artifact)
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
    write_market_selection_source_artifact_from_decision_evidence_file(
        loaded,
        strategy_instance_id,
        decision_evidence_path,
        max_decision_evidence_bytes,
        &instruments,
        path,
    )
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
    candidate_market_start_timestamps_ms: &[u64],
    path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let financial_envelope =
        Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(loaded, strategy_instance_id)
            .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    if snapshot.configured_target_id != financial_envelope.configured_target_id() {
        return Err(
            BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven {
                prerequisite: "T046 remains blocked: strategy input target does not match config",
            },
        );
    }
    if snapshot.price_to_beat_source != financial_envelope.price_to_beat_source() {
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
            &market_selection_source,
            market_selection_source_ref.path.to_string_lossy(),
            &market_selection_source_ref.sha256,
            candidate_market_start_timestamps_ms,
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
    candidate_market_start_timestamps_ms: &[u64],
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
        candidate_market_start_timestamps_ms,
        path,
    )
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
struct Phase8PreRunMarketWindowSourceProofHashInput<'a> {
    schema_version: u32,
    record_kind: &'static str,
    strategy_input_evidence_sha256: &'a str,
    expected_price_to_beat_source: &'a str,
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
            field: "clob_signing_version",
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
    let mut generated_artifacts = Vec::new();
    let mut written_artifacts = Vec::new();
    let mut blockers = Vec::new();

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
    financial_envelope_path: String,
    financial_envelope_sha256: String,
    pre_run_state_path: String,
    pre_run_state_sha256: String,
    abort_plan_path: String,
    abort_plan_sha256: String,
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
    let parsed_static_manifest = read_static_manifest(
        static_manifest_path,
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

    let approval_envelope = approval_envelope_from_operator_evidence(
        operator_evidence,
        live_canary.approval_id.as_str(),
    );
    let approval_envelope_sha256 = json_artifact_sha256(&approval_envelope)?;
    let operator_packet = operator_evidence_packet(
        loaded,
        static_manifest_path,
        parsed_static_manifest.sha256.as_str(),
        operator_evidence,
        approval_envelope_sha256.clone(),
    );

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
        path: static_manifest_path.to_path_buf(),
        sha256: parsed_static_manifest.sha256,
    };

    Ok(BoltV3OperatorPacketAssemblyOutcome {
        approval_envelope: approval_envelope_written,
        operator_packet: operator_packet_written,
        static_manifest: static_manifest_written,
    })
}

pub fn verify_final_operator_packet(
    loaded: &LoadedBoltV3Config,
    operator_packet_path: &Path,
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
    let approval_envelope = verify_operator_approval_envelope(
        loaded,
        operator_evidence,
        live_canary.approval_id.as_str(),
    )?;

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
    Ok(())
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
        strategy_cancel_path_hash: evidence.strategy_cancel_path.as_deref().map(sha256_text),
    }
}

fn operator_evidence_packet(
    loaded: &LoadedBoltV3Config,
    static_manifest_path: &Path,
    static_manifest_sha256: &str,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_envelope_sha256: String,
) -> BoltV3OperatorEvidencePacket {
    BoltV3OperatorEvidencePacket {
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
            financial_envelope_path: evidence.financial_envelope_path.clone(),
            financial_envelope_sha256: evidence.financial_envelope_sha256.clone(),
            pre_run_state_path: evidence.pre_run_state_path.clone(),
            pre_run_state_sha256: evidence.pre_run_state_sha256.clone(),
            abort_plan_path: evidence.abort_plan_path.clone(),
            abort_plan_sha256: evidence.abort_plan_sha256.clone(),
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
    }
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

fn write_json_artifact_create_new<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    write_json_artifact_create_new_with_file(path, value, open_json_artifact_create_new_file)
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
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| BoltV3OperatorArtifactError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(BoltV3OperatorArtifactError::Serialize)?;
    let mut file = open_file(path).map_err(|source| BoltV3OperatorArtifactError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    if let Err(source) = file.write_all(&bytes) {
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

    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
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

fn json_artifact_sha256<T: Serialize>(value: &T) -> Result<String, BoltV3OperatorArtifactError> {
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

fn read_file_bounded(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
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

fn is_lowercase_sha256(value: &str) -> bool {
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
    hex::encode(Sha256::digest(value.as_bytes()))
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
