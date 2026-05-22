use std::{
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use nautilus_model::instruments::InstrumentAny;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::{LiveCanaryOperatorEvidenceBlock, LoadedBoltV3Config},
    bolt_v3_live_canary_gate::{
        APPROVAL_ENVELOPE_RECORD_KIND, APPROVAL_ENVELOPE_SCHEMA_VERSION,
        Phase8OperatorApprovalEnvelopeFile,
    },
    bolt_v3_market_families::{self, MarketSelectionTarget},
    bolt_v3_providers::{ProviderSecretResolveContext, binding_for_provider_key},
    bolt_v3_secrets::BoltV3SecretError,
    bolt_v3_tiny_canary_evidence::{
        Phase8AbortPlanEvidenceFile, Phase8AbortPlanSourceProofs,
        Phase8FinancialEnvelopeEvidenceFile, Phase8MarketSelectionSourceEvidenceFile,
        Phase8PreRunStateEvidenceFile, Phase8PreRunStateSourceProofs,
    },
};

const REDACTED_SSM_MANIFEST_SCHEMA_VERSION: u32 = 1;
const REDACTED_SSM_MANIFEST_RECORD_KIND: &str = "bolt_v3.redacted_ssm_manifest.v1";
const APPROVAL_NONCE_SCHEMA_VERSION: u32 = 1;
const APPROVAL_NONCE_RECORD_KIND: &str = "bolt_v3.operator_approval_nonce.v1";
const APPROVAL_NONCE_BYTES: usize = 32;
const STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION: u32 = 1;
const STATIC_ARTIFACTS_MANIFEST_RECORD_KIND: &str = "bolt_v3.static_operator_artifacts_manifest.v1";
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
    pub ssm_path_sha256: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StaticArtifactsWriteOutcome {
    pub command_summary: BoltV3StaticArtifactsCommandSummary,
    pub blockers: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenOperatorArtifact {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug)]
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
    PreRunStatePrerequisiteUnproven {
        prerequisite: &'static str,
    },
    AbortPrerequisiteUnproven {
        prerequisite: &'static str,
    },
    MissingLiveCanary,
    MissingOperatorEvidence,
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
    OutputPathCollision,
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
            Self::PreRunStatePrerequisiteUnproven { prerequisite } => write!(
                f,
                "refusing to write successful pre-run state evidence because {prerequisite}"
            ),
            Self::AbortPrerequisiteUnproven { prerequisite } => write!(
                f,
                "refusing to write successful abort plan because {prerequisite} is not proven"
            ),
            Self::MissingLiveCanary => write!(
                f,
                "refusing to assemble operator packet because `[live_canary]` is missing"
            ),
            Self::MissingOperatorEvidence => write!(
                f,
                "refusing to assemble operator packet because `[live_canary.operator_evidence]` is missing"
            ),
            Self::StaticManifestRead { path, source } => write!(
                f,
                "failed to read static manifest `{}`: {source}",
                path.display()
            ),
            Self::StaticManifestParse { path, source } => write!(
                f,
                "failed to parse static manifest `{}`: {source}",
                path.display()
            ),
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
            Self::StaticManifestArtifactFileRead { name, path, source } => write!(
                f,
                "failed to read static manifest artifact `{name}` at `{}`: {source}",
                path.display()
            ),
            Self::StaticManifestArtifactFileHashMismatch { name, path } => write!(
                f,
                "static manifest artifact `{name}` file hash mismatch at `{}`",
                path.display()
            ),
            Self::InvalidOperatorEvidenceHash { field } => write!(
                f,
                "`[live_canary.operator_evidence].{field}` must be a lowercase sha256 hex string"
            ),
            Self::InvalidOutputPath { field } => write!(
                f,
                "operator packet output path field `{field}` must not contain parent-directory components"
            ),
            Self::OutputPathCollision => write!(
                f,
                "operator packet output path must differ from approval_envelope_path"
            ),
            Self::Random(error) => write!(f, "failed to generate approval nonce bytes: {error}"),
            Self::Serialize(error) => write!(f, "failed to serialize operator artifact: {error}"),
            Self::Write { path, source } => {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    write!(
                        f,
                        "refusing to overwrite existing operator artifact `{}`: already exists",
                        path.display()
                    )
                } else {
                    write!(
                        f,
                        "failed to write operator artifact `{}`: {source}",
                        path.display()
                    )
                }
            }
        }
    }
}

impl Error for BoltV3OperatorArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SecretInventory(error) => Some(error),
            Self::FinancialEnvelope(error) => Some(error.as_ref()),
            Self::MarketSelection(error) => Some(error.as_ref()),
            Self::StaticManifestRead { source, .. } => Some(source),
            Self::StaticManifestParse { source, .. } => Some(source),
            Self::StaticManifestArtifactFileRead { source, .. } => Some(source),
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
                ssm_path_sha256: sha256_text(path.ssm_path.as_str()),
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

pub fn write_static_operator_artifacts(
    loaded: &LoadedBoltV3Config,
    strategy_instance_id: &str,
    output_dir: &Path,
) -> Result<BoltV3StaticArtifactsWriteOutcome, BoltV3OperatorArtifactError> {
    let mut generated_artifacts = Vec::new();
    let mut blockers = Vec::new();

    let ssm_manifest = build_redacted_ssm_manifest(loaded)?;
    let ssm_manifest_written =
        write_json_artifact_create_new(&output_dir.join(SSM_MANIFEST_FILE_NAME), &ssm_manifest)?;
    generated_artifacts.push(static_artifact_ref(
        SSM_MANIFEST_ARTIFACT_NAME,
        ssm_manifest_written,
    ));

    let financial_envelope = build_phase8_financial_envelope(loaded, strategy_instance_id)
        .map_err(BoltV3OperatorArtifactError::FinancialEnvelope)?;
    let financial_envelope_written = write_json_artifact_create_new(
        &output_dir.join(FINANCIAL_ENVELOPE_FILE_NAME),
        &financial_envelope,
    )?;
    generated_artifacts.push(static_artifact_ref(
        FINANCIAL_ENVELOPE_ARTIFACT_NAME,
        financial_envelope_written,
    ));

    let approval_nonce_written =
        write_approval_nonce_artifact(&output_dir.join(APPROVAL_NONCE_FILE_NAME))?;
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
            generated_artifacts.push(static_artifact_ref(STRATEGY_INPUT_ARTIFACT_NAME, written))
        }
        Err(BoltV3OperatorArtifactError::StrategyInputPrerequisiteUnproven { prerequisite }) => {
            blockers.push(prerequisite);
        }
        Err(error) => return Err(error),
    }

    match write_pre_run_state_artifact(
        loaded,
        strategy_instance_id,
        &output_dir.join(PRE_RUN_STATE_FILE_NAME),
    ) {
        Ok(written) => {
            generated_artifacts.push(static_artifact_ref(PRE_RUN_STATE_ARTIFACT_NAME, written))
        }
        Err(BoltV3OperatorArtifactError::PreRunStatePrerequisiteUnproven { prerequisite }) => {
            blockers.push(prerequisite);
        }
        Err(error) => return Err(error),
    }

    match write_abort_plan_artifact(
        loaded,
        strategy_instance_id,
        &output_dir.join(ABORT_PLAN_FILE_NAME),
    ) {
        Ok(written) => {
            generated_artifacts.push(static_artifact_ref(ABORT_PLAN_ARTIFACT_NAME, written))
        }
        Err(BoltV3OperatorArtifactError::AbortPrerequisiteUnproven { prerequisite }) => {
            blockers.push(prerequisite);
        }
        Err(error) => return Err(error),
    }

    let outcome_blockers = blockers.clone();
    let manifest = BoltV3StaticArtifactsManifest {
        schema_version: STATIC_ARTIFACTS_MANIFEST_SCHEMA_VERSION,
        record_kind: STATIC_ARTIFACTS_MANIFEST_RECORD_KIND,
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        generated_artifacts,
        blockers,
    };
    let manifest_written = write_json_artifact_create_new(
        &output_dir.join(STATIC_ARTIFACTS_MANIFEST_FILE_NAME),
        &manifest,
    )?;

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
    let parsed_static_manifest = read_static_manifest(static_manifest_path)?;
    let static_manifest = &parsed_static_manifest.manifest;

    validate_static_manifest_header(loaded, static_manifest)?;
    if !static_manifest.blockers.is_empty() {
        return Err(BoltV3OperatorArtifactError::StaticManifestBlockers {
            count: static_manifest.blockers.len(),
        });
    }

    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        SSM_MANIFEST_ARTIFACT_NAME,
        &operator_evidence.ssm_manifest_path,
        &operator_evidence.ssm_manifest_sha256,
        "ssm_manifest_sha256",
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        STRATEGY_INPUT_ARTIFACT_NAME,
        &operator_evidence.strategy_input_evidence_path,
        &operator_evidence.strategy_input_evidence_sha256,
        "strategy_input_evidence_sha256",
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        FINANCIAL_ENVELOPE_ARTIFACT_NAME,
        &operator_evidence.financial_envelope_path,
        &operator_evidence.financial_envelope_sha256,
        "financial_envelope_sha256",
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        PRE_RUN_STATE_ARTIFACT_NAME,
        &operator_evidence.pre_run_state_path,
        &operator_evidence.pre_run_state_sha256,
        "pre_run_state_sha256",
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        ABORT_PLAN_ARTIFACT_NAME,
        &operator_evidence.abort_plan_path,
        &operator_evidence.abort_plan_sha256,
        "abort_plan_sha256",
    )?;
    validate_required_static_manifest_artifact(
        loaded,
        static_manifest,
        APPROVAL_NONCE_ARTIFACT_NAME,
        &operator_evidence.approval_nonce_path,
        &operator_evidence.approval_nonce_sha256,
        "approval_nonce_sha256",
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
    if approval_envelope_path == operator_packet_path {
        return Err(BoltV3OperatorArtifactError::OutputPathCollision);
    }
    ensure_output_path_absent(&approval_envelope_path)?;
    ensure_output_path_absent(&operator_packet_path)?;

    let approval_envelope_written =
        write_json_artifact_create_new(&approval_envelope_path, &approval_envelope)?;
    debug_assert_eq!(approval_envelope_written.sha256, approval_envelope_sha256);
    let operator_packet_written =
        write_json_artifact_create_new(&operator_packet_path, &operator_packet)?;
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

fn read_static_manifest(path: &Path) -> Result<ParsedStaticManifest, BoltV3OperatorArtifactError> {
    let bytes =
        fs::read(path).map_err(|source| BoltV3OperatorArtifactError::StaticManifestRead {
            path: path.to_path_buf(),
            source,
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

fn validate_required_static_manifest_artifact(
    loaded: &LoadedBoltV3Config,
    manifest: &BoltV3StaticArtifactsManifestInput,
    name: &'static str,
    configured_path: &str,
    configured_sha256: &str,
    configured_sha256_field: &'static str,
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
    let actual = sha256_file_for_static_manifest(name, &resolved_path)?;
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
    )?;
    Ok(artifact)
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
    let nonce_sha256 = hex::encode(Sha256::digest(nonce));
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
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&bytes)
        .map_err(|source| BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(WrittenOperatorArtifact {
        path: path.to_path_buf(),
        sha256: hex::encode(Sha256::digest(bytes)),
    })
}

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

fn json_artifact_sha256<T: Serialize>(value: &T) -> Result<String, BoltV3OperatorArtifactError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(BoltV3OperatorArtifactError::Serialize)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn sha256_file_for_static_manifest(
    name: &'static str,
    path: &Path,
) -> Result<String, BoltV3OperatorArtifactError> {
    let bytes = fs::read(path).map_err(|source| {
        BoltV3OperatorArtifactError::StaticManifestArtifactFileRead {
            name,
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn validate_operator_evidence_sha256(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.len() == 64
        && value
            .chars()
            .all(|char| matches!(char, '0'..='9' | 'a'..='f'))
    {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::InvalidOperatorEvidenceHash { field })
    }
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
