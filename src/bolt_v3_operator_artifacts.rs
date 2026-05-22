use std::{
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use nautilus_model::instruments::InstrumentAny;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
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
