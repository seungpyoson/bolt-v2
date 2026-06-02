use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    str::FromStr,
};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_operator_artifacts::{
        BoltV3OperatorArtifactError, WrittenOperatorArtifact, is_lowercase_sha256,
        read_file_bounded, write_json_artifact_create_new,
    },
    bolt_v3_submit_admission::{BoltV3ExchangeMutationCounts, validate_no_exchange_mutations},
};

use super::hyperliquid::{self, HyperliquidProductSurface};

const PRODUCT_MATRIX_SCHEMA_VERSION: u32 = 1;
const PRODUCT_MATRIX_RECORD_KIND: &str = "bolt_v3.hyperliquid_product_matrix.v1";
const LATENCY_PROFILE_SCHEMA_VERSION: u32 = 1;
const LATENCY_PROFILE_RECORD_KIND: &str = "bolt_v3.hyperliquid_latency_profile.v1";
const PRODUCT_SUBMIT_PROOF_SCHEMA_VERSION: u32 = 1;
const PRODUCT_SUBMIT_PROOF_RECORD_KIND: &str = "bolt_v3.hyperliquid_product_submit_proof.v1";
const LIVE_SUBMIT_APPROVAL_SCHEMA_VERSION: u32 = 1;
const LIVE_SUBMIT_APPROVAL_RECORD_KIND: &str = "bolt_v3.hyperliquid_live_submit_approval.v1";

const LATENCY_PROFILE_ARTIFACT: &str = "hyperliquid_latency_profile";
const PRODUCT_SUBMIT_PROOF_ARTIFACT: &str = "hyperliquid_product_submit_proof";
const LIVE_SUBMIT_APPROVAL_ARTIFACT: &str = "hyperliquid_live_submit_approval";

#[derive(Debug, Serialize)]
pub struct HyperliquidProductMatrixArtifact {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub provider_key: &'static str,
    pub surfaces: &'static [hyperliquid::HyperliquidProductMatrixEntry],
}

pub fn build_hyperliquid_product_matrix_artifact() -> HyperliquidProductMatrixArtifact {
    HyperliquidProductMatrixArtifact {
        schema_version: PRODUCT_MATRIX_SCHEMA_VERSION,
        record_kind: PRODUCT_MATRIX_RECORD_KIND,
        provider_key: hyperliquid::KEY,
        surfaces: hyperliquid::hyperliquid_product_matrix(),
    }
}

pub fn write_hyperliquid_product_matrix_artifact(
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_hyperliquid_product_matrix_artifact();
    write_json_artifact_create_new(output_path, &artifact)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidLatencyProfileArtifactInput {
    pub provider_id: String,
    pub toml_checksum: String,
    pub latency_profile: hyperliquid::HyperliquidLatencyProfileConfig,
    pub exchange_mutations: BoltV3ExchangeMutationCounts,
}

#[derive(Debug, Serialize)]
pub struct HyperliquidLatencyProfileArtifact {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub provider_key: &'static str,
    pub provider_id: String,
    pub toml_checksum: String,
    pub latency_profile: hyperliquid::HyperliquidLatencyProfileConfig,
    pub exchange_mutation_count: u64,
}

pub fn build_hyperliquid_latency_profile_artifact(
    input: HyperliquidLatencyProfileArtifactInput,
) -> Result<HyperliquidLatencyProfileArtifact, BoltV3OperatorArtifactError> {
    let exchange_mutation_count = validate_hyperliquid_latency_profile_input(&input)?;
    Ok(HyperliquidLatencyProfileArtifact {
        schema_version: LATENCY_PROFILE_SCHEMA_VERSION,
        record_kind: LATENCY_PROFILE_RECORD_KIND,
        provider_key: hyperliquid::KEY,
        provider_id: input.provider_id,
        toml_checksum: input.toml_checksum,
        latency_profile: input.latency_profile,
        exchange_mutation_count,
    })
}

pub fn write_hyperliquid_latency_profile_artifact(
    input: HyperliquidLatencyProfileArtifactInput,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_hyperliquid_latency_profile_artifact(input)?;
    write_json_artifact_create_new(output_path, &artifact)
}

fn validate_hyperliquid_latency_profile_input(
    input: &HyperliquidLatencyProfileArtifactInput,
) -> Result<u64, BoltV3OperatorArtifactError> {
    if input.provider_id.trim().is_empty() {
        return Err(provider_artifact_invalid(
            LATENCY_PROFILE_ARTIFACT,
            "provider_id",
        ));
    }
    if !is_lowercase_sha256(&input.toml_checksum) {
        return Err(provider_artifact_invalid(
            LATENCY_PROFILE_ARTIFACT,
            "toml_checksum",
        ));
    }
    validate_hyperliquid_latency_profile_config(&input.latency_profile)?;
    validate_no_exchange_mutations(input.exchange_mutations)
        .map_err(|_| provider_artifact_invalid(LATENCY_PROFILE_ARTIFACT, "exchange_mutation_count"))
}

fn validate_hyperliquid_latency_profile_config(
    latency_profile: &hyperliquid::HyperliquidLatencyProfileConfig,
) -> Result<(), BoltV3OperatorArtifactError> {
    if latency_profile.local_info_node_url.trim().is_empty()
        || !(latency_profile.local_info_node_url.starts_with("http://")
            || latency_profile.local_info_node_url.starts_with("https://"))
    {
        return Err(provider_artifact_invalid(
            LATENCY_PROFILE_ARTIFACT,
            "latency_profile.local_info_node_url",
        ));
    }
    if latency_profile.placement_profile.trim().is_empty() {
        return Err(provider_artifact_invalid(
            LATENCY_PROFILE_ARTIFACT,
            "latency_profile.placement_profile",
        ));
    }
    if latency_profile.measurement_artifact_path.trim().is_empty() {
        return Err(provider_artifact_invalid(
            LATENCY_PROFILE_ARTIFACT,
            "latency_profile.measurement_artifact_path",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidLiveSubmitOrderLimits {
    pub max_order_count: u32,
    pub max_order_notional: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidProductSubmitProofBinding {
    pub artifact_path: String,
    pub artifact_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HyperliquidProductSubmitProofEvidenceRef {
    pub artifact_path: String,
    pub artifact_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HyperliquidProductSubmitProofArtifact {
    pub schema_version: u32,
    pub record_kind: String,
    pub provider_key: String,
    pub provider_id: String,
    pub product_surface: HyperliquidProductSurface,
    pub toml_checksum: String,
    pub order_proof: HyperliquidProductSubmitProofEvidenceRef,
    pub fill_proof: HyperliquidProductSubmitProofEvidenceRef,
    pub rounding_proof: HyperliquidProductSubmitProofEvidenceRef,
    pub fee_proof: HyperliquidProductSubmitProofEvidenceRef,
    pub settlement_proof: Option<HyperliquidProductSubmitProofEvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidLiveSubmitApprovalBinding {
    pub base_sha: String,
    pub provider_id: String,
    pub product_surface: HyperliquidProductSurface,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub order_limits: HyperliquidLiveSubmitOrderLimits,
    pub product_submit_proof: HyperliquidProductSubmitProofBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidLiveSubmitApprovalInput {
    pub approval_id: String,
    pub base_sha: String,
    pub provider_id: String,
    pub product_surface: HyperliquidProductSurface,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub order_limits: HyperliquidLiveSubmitOrderLimits,
    pub product_submit_proof: HyperliquidProductSubmitProofBinding,
    pub expires_at: u64,
    pub used_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HyperliquidLiveSubmitApprovalArtifact {
    pub schema_version: u32,
    pub record_kind: String,
    pub provider_key: String,
    pub approval_id: String,
    pub base_sha: String,
    pub provider_id: String,
    pub product_surface: HyperliquidProductSurface,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub order_limits: HyperliquidLiveSubmitOrderLimits,
    pub product_submit_proof: Option<HyperliquidProductSubmitProofBinding>,
    pub expires_at: u64,
    pub used_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidLiveSubmitApprovalConsumption {
    approval_id: String,
    product_surface: HyperliquidProductSurface,
    used_at: u64,
    order_limits: HyperliquidLiveSubmitOrderLimits,
    product_submit_proof: HyperliquidProductSubmitProofBinding,
}

impl HyperliquidLiveSubmitApprovalConsumption {
    pub fn approval_id(&self) -> &str {
        self.approval_id.as_str()
    }

    pub fn product_surface(&self) -> HyperliquidProductSurface {
        self.product_surface
    }

    pub fn used_at(&self) -> u64 {
        self.used_at
    }

    pub fn order_limits(&self) -> &HyperliquidLiveSubmitOrderLimits {
        &self.order_limits
    }

    pub fn product_submit_proof(&self) -> &HyperliquidProductSubmitProofBinding {
        &self.product_submit_proof
    }
}

pub fn build_hyperliquid_live_submit_approval_artifact(
    input: HyperliquidLiveSubmitApprovalInput,
) -> Result<HyperliquidLiveSubmitApprovalArtifact, BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_approval_input(&input)?;
    Ok(HyperliquidLiveSubmitApprovalArtifact {
        schema_version: LIVE_SUBMIT_APPROVAL_SCHEMA_VERSION,
        record_kind: LIVE_SUBMIT_APPROVAL_RECORD_KIND.to_string(),
        provider_key: hyperliquid::KEY.to_string(),
        approval_id: input.approval_id,
        base_sha: input.base_sha,
        provider_id: input.provider_id,
        product_surface: input.product_surface,
        toml_checksum: input.toml_checksum,
        signer_fingerprint: input.signer_fingerprint,
        order_limits: input.order_limits,
        product_submit_proof: Some(input.product_submit_proof),
        expires_at: input.expires_at,
        used_at: input.used_at,
    })
}

pub fn read_hyperliquid_live_submit_approval_artifact(
    path: &Path,
    max_bytes: u64,
) -> Result<HyperliquidLiveSubmitApprovalArtifact, BoltV3OperatorArtifactError> {
    let invalid_artifact =
        || provider_artifact_invalid(LIVE_SUBMIT_APPROVAL_ARTIFACT, "approval_artifact");
    let bytes = read_file_bounded(path, max_bytes).map_err(|_| invalid_artifact())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_artifact())
}

pub fn validate_hyperliquid_product_submit_proof_artifact_bytes(
    bytes: &[u8],
    binding: &HyperliquidLiveSubmitApprovalBinding,
) -> Result<(), BoltV3OperatorArtifactError> {
    let artifact: HyperliquidProductSubmitProofArtifact =
        serde_json::from_slice(bytes).map_err(|_| {
            provider_artifact_invalid(PRODUCT_SUBMIT_PROOF_ARTIFACT, "product_submit_proof")
        })?;
    validate_hyperliquid_product_submit_proof_artifact(&artifact, binding)
}

pub fn persist_consumed_hyperliquid_live_submit_approval_artifact(
    path: &Path,
    artifact: &HyperliquidLiveSubmitApprovalArtifact,
) -> Result<(), BoltV3OperatorArtifactError> {
    if artifact.used_at.is_none() {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "used_at",
        ));
    }
    let bytes =
        serde_json::to_vec_pretty(artifact).map_err(BoltV3OperatorArtifactError::Serialize)?;
    let mut file =
        open_hyperliquid_live_submit_approval_file_for_spend(path).map_err(|source| {
            BoltV3OperatorArtifactError::Write {
                path: path.to_path_buf(),
                source,
            }
        })?;
    try_lock_hyperliquid_live_submit_approval_file_for_spend(&file).map_err(|source| {
        BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let existing =
        read_hyperliquid_live_submit_approval_artifact_from_open_file(&file, bytes.len() as u64)?;
    let mut expected_existing = artifact.clone();
    expected_existing.used_at = None;
    if existing != expected_existing {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            if existing.used_at.is_some() {
                "used_at"
            } else {
                "approval_artifact"
            },
        ));
    }

    let result = (|| -> std::io::Result<()> {
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(&bytes)?;
        file.set_len(bytes.len() as u64)?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    result.map_err(|source| BoltV3OperatorArtifactError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn read_hyperliquid_live_submit_approval_artifact_from_open_file(
    file: &fs::File,
    max_bytes: u64,
) -> Result<HyperliquidLiveSubmitApprovalArtifact, BoltV3OperatorArtifactError> {
    let invalid_artifact =
        || provider_artifact_invalid(LIVE_SUBMIT_APPROVAL_ARTIFACT, "approval_artifact");
    let bytes = read_open_file_bounded(file, max_bytes).map_err(|_| invalid_artifact())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_artifact())
}

fn read_open_file_bounded(mut file: &fs::File, max_bytes: u64) -> std::io::Result<Vec<u8>> {
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

#[cfg(unix)]
fn open_hyperliquid_live_submit_approval_file_for_spend(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_hyperliquid_live_submit_approval_file_for_spend(_path: &Path) -> std::io::Result<fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "hyperliquid live-submit approval spend locking requires unix flock",
    ))
}

#[cfg(unix)]
fn try_lock_hyperliquid_live_submit_approval_file_for_spend(
    file: &fs::File,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: `file.as_raw_fd()` is a live descriptor for the duration of the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn try_lock_hyperliquid_live_submit_approval_file_for_spend(
    _file: &fs::File,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "hyperliquid live-submit approval spend locking requires unix flock",
    ))
}

pub fn write_hyperliquid_live_submit_approval_artifact(
    input: HyperliquidLiveSubmitApprovalInput,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_hyperliquid_live_submit_approval_artifact(input)?;
    write_json_artifact_create_new(output_path, &artifact)
}

pub fn validate_hyperliquid_live_submit_approval_artifact(
    artifact: Option<&HyperliquidLiveSubmitApprovalArtifact>,
    binding: &HyperliquidLiveSubmitApprovalBinding,
    now_unix_seconds: u64,
) -> Result<(), BoltV3OperatorArtifactError> {
    let artifact = artifact.ok_or(provider_artifact_invalid(
        LIVE_SUBMIT_APPROVAL_ARTIFACT,
        "approval_artifact",
    ))?;
    if artifact.schema_version != LIVE_SUBMIT_APPROVAL_SCHEMA_VERSION {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "schema_version",
        ));
    }
    if artifact.record_kind != LIVE_SUBMIT_APPROVAL_RECORD_KIND {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "record_kind",
        ));
    }
    if artifact.provider_key != hyperliquid::KEY {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "provider_key",
        ));
    }
    validate_hyperliquid_live_submit_approval_artifact_fields(artifact)?;
    validate_hyperliquid_live_submit_binding(binding)?;
    if artifact.used_at.is_some() {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "used_at",
        ));
    }
    if artifact.expires_at <= now_unix_seconds {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "expires_at",
        ));
    }
    validate_hyperliquid_live_submit_approval_binding_match(artifact, binding)
}

pub fn consume_hyperliquid_live_submit_approval_artifact(
    artifact: &mut HyperliquidLiveSubmitApprovalArtifact,
    binding: &HyperliquidLiveSubmitApprovalBinding,
    expected_approval_id: &str,
    now_unix_seconds: u64,
) -> Result<HyperliquidLiveSubmitApprovalConsumption, BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_approval_artifact(Some(artifact), binding, now_unix_seconds)?;
    if artifact.approval_id != expected_approval_id {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "approval_id",
        ));
    }
    artifact.used_at = Some(now_unix_seconds);
    Ok(HyperliquidLiveSubmitApprovalConsumption {
        approval_id: expected_approval_id.to_string(),
        product_surface: binding.product_surface,
        used_at: now_unix_seconds,
        order_limits: binding.order_limits.clone(),
        product_submit_proof: binding.product_submit_proof.clone(),
    })
}

fn validate_hyperliquid_live_submit_approval_input(
    input: &HyperliquidLiveSubmitApprovalInput,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_non_empty_field("approval_id", &input.approval_id)?;
    validate_hyperliquid_live_submit_base_sha(&input.base_sha)?;
    validate_hyperliquid_live_submit_non_empty_field("provider_id", &input.provider_id)?;
    validate_hyperliquid_live_submit_product_surface(input.product_surface)?;
    validate_hyperliquid_live_submit_sha256_field("toml_checksum", &input.toml_checksum)?;
    validate_hyperliquid_live_submit_sha256_field("signer_fingerprint", &input.signer_fingerprint)?;
    validate_hyperliquid_live_submit_order_limits(&input.order_limits)?;
    validate_hyperliquid_product_submit_proof_binding(&input.product_submit_proof)?;
    if input.expires_at == 0 {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "expires_at",
        ));
    }
    if input.used_at.is_some() {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "used_at",
        ));
    }
    Ok(())
}

fn validate_hyperliquid_live_submit_approval_artifact_fields(
    artifact: &HyperliquidLiveSubmitApprovalArtifact,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_non_empty_field("approval_id", &artifact.approval_id)?;
    validate_hyperliquid_live_submit_base_sha(&artifact.base_sha)?;
    validate_hyperliquid_live_submit_non_empty_field("provider_id", &artifact.provider_id)?;
    validate_hyperliquid_live_submit_product_surface(artifact.product_surface)?;
    validate_hyperliquid_live_submit_sha256_field("toml_checksum", &artifact.toml_checksum)?;
    validate_hyperliquid_live_submit_sha256_field(
        "signer_fingerprint",
        &artifact.signer_fingerprint,
    )?;
    validate_hyperliquid_live_submit_order_limits(&artifact.order_limits)?;
    let product_submit_proof =
        artifact
            .product_submit_proof
            .as_ref()
            .ok_or(provider_artifact_invalid(
                LIVE_SUBMIT_APPROVAL_ARTIFACT,
                "product_submit_proof",
            ))?;
    validate_hyperliquid_product_submit_proof_binding(product_submit_proof)?;
    if artifact.expires_at == 0 {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "expires_at",
        ));
    }
    Ok(())
}

fn validate_hyperliquid_live_submit_binding(
    binding: &HyperliquidLiveSubmitApprovalBinding,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_base_sha(&binding.base_sha)?;
    validate_hyperliquid_live_submit_non_empty_field("provider_id", &binding.provider_id)?;
    validate_hyperliquid_live_submit_product_surface(binding.product_surface)?;
    validate_hyperliquid_live_submit_sha256_field("toml_checksum", &binding.toml_checksum)?;
    validate_hyperliquid_live_submit_sha256_field(
        "signer_fingerprint",
        &binding.signer_fingerprint,
    )?;
    validate_hyperliquid_live_submit_order_limits(&binding.order_limits)?;
    validate_hyperliquid_product_submit_proof_binding(&binding.product_submit_proof)
}

fn validate_hyperliquid_live_submit_approval_binding_match(
    artifact: &HyperliquidLiveSubmitApprovalArtifact,
    binding: &HyperliquidLiveSubmitApprovalBinding,
) -> Result<(), BoltV3OperatorArtifactError> {
    if artifact.base_sha != binding.base_sha {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "base_sha",
        ));
    }
    if artifact.provider_id != binding.provider_id {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "provider_id",
        ));
    }
    if artifact.product_surface != binding.product_surface {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "product_surface",
        ));
    }
    if artifact.toml_checksum != binding.toml_checksum {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "toml_checksum",
        ));
    }
    if artifact.signer_fingerprint != binding.signer_fingerprint {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "signer_fingerprint",
        ));
    }
    if artifact.order_limits != binding.order_limits {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "order_limits",
        ));
    }
    if artifact.product_submit_proof.as_ref() != Some(&binding.product_submit_proof) {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "product_submit_proof",
        ));
    }
    Ok(())
}

fn validate_hyperliquid_live_submit_non_empty_field(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.trim().is_empty() {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            field,
        ));
    }
    Ok(())
}

fn validate_hyperliquid_live_submit_base_sha(
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.len() == 40
        && value
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
    {
        return Ok(());
    }
    Err(provider_artifact_invalid(
        LIVE_SUBMIT_APPROVAL_ARTIFACT,
        "base_sha",
    ))
}

fn validate_hyperliquid_live_submit_sha256_field(
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if is_lowercase_sha256(value) {
        return Ok(());
    }
    Err(provider_artifact_invalid(
        LIVE_SUBMIT_APPROVAL_ARTIFACT,
        field,
    ))
}

fn validate_hyperliquid_live_submit_product_surface(
    product_surface: HyperliquidProductSurface,
) -> Result<(), BoltV3OperatorArtifactError> {
    match product_surface {
        HyperliquidProductSurface::StandardPerps
        | HyperliquidProductSurface::Spot
        | HyperliquidProductSurface::Hip3BuilderPerps
        | HyperliquidProductSurface::Hip4Outcomes => Ok(()),
    }
}

fn validate_hyperliquid_live_submit_order_limits(
    order_limits: &HyperliquidLiveSubmitOrderLimits,
) -> Result<(), BoltV3OperatorArtifactError> {
    if order_limits.max_order_count == 0 {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "order_limits.max_order_count",
        ));
    }
    let max_order_notional =
        Decimal::from_str(order_limits.max_order_notional.trim()).map_err(|_| {
            provider_artifact_invalid(
                LIVE_SUBMIT_APPROVAL_ARTIFACT,
                "order_limits.max_order_notional",
            )
        })?;
    if max_order_notional <= Decimal::ZERO {
        return Err(provider_artifact_invalid(
            LIVE_SUBMIT_APPROVAL_ARTIFACT,
            "order_limits.max_order_notional",
        ));
    }
    Ok(())
}

fn validate_hyperliquid_product_submit_proof_binding(
    product_submit_proof: &HyperliquidProductSubmitProofBinding,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_non_empty_field(
        "product_submit_proof.artifact_path",
        &product_submit_proof.artifact_path,
    )?;
    validate_hyperliquid_live_submit_sha256_field(
        "product_submit_proof.artifact_sha256",
        &product_submit_proof.artifact_sha256,
    )
}

fn validate_hyperliquid_product_submit_proof_artifact(
    artifact: &HyperliquidProductSubmitProofArtifact,
    binding: &HyperliquidLiveSubmitApprovalBinding,
) -> Result<(), BoltV3OperatorArtifactError> {
    if artifact.schema_version != PRODUCT_SUBMIT_PROOF_SCHEMA_VERSION {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "schema_version",
        ));
    }
    if artifact.record_kind != PRODUCT_SUBMIT_PROOF_RECORD_KIND {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "record_kind",
        ));
    }
    if artifact.provider_key != hyperliquid::KEY {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "provider_key",
        ));
    }
    if artifact.provider_id != binding.provider_id {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "provider_id",
        ));
    }
    if artifact.product_surface != binding.product_surface {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "product_surface",
        ));
    }
    if artifact.toml_checksum != binding.toml_checksum {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "toml_checksum",
        ));
    }
    validate_hyperliquid_product_submit_proof_evidence_ref("order_proof", &artifact.order_proof)?;
    validate_hyperliquid_product_submit_proof_evidence_ref("fill_proof", &artifact.fill_proof)?;
    validate_hyperliquid_product_submit_proof_evidence_ref(
        "rounding_proof",
        &artifact.rounding_proof,
    )?;
    validate_hyperliquid_product_submit_proof_evidence_ref("fee_proof", &artifact.fee_proof)?;
    match (binding.product_surface, artifact.settlement_proof.as_ref()) {
        (HyperliquidProductSurface::Hip4Outcomes, Some(settlement_proof)) => {
            validate_hyperliquid_product_submit_proof_evidence_ref(
                "settlement_proof",
                settlement_proof,
            )
        }
        (HyperliquidProductSurface::Hip4Outcomes, None) => Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "settlement_proof",
        )),
        (_, Some(_)) => Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            "settlement_proof",
        )),
        (_, None) => Ok(()),
    }
}

fn validate_hyperliquid_product_submit_proof_evidence_ref(
    field: &'static str,
    evidence_ref: &HyperliquidProductSubmitProofEvidenceRef,
) -> Result<(), BoltV3OperatorArtifactError> {
    let artifact_path_field = match field {
        "order_proof" => "order_proof.artifact_path",
        "fill_proof" => "fill_proof.artifact_path",
        "rounding_proof" => "rounding_proof.artifact_path",
        "fee_proof" => "fee_proof.artifact_path",
        "settlement_proof" => "settlement_proof.artifact_path",
        _ => field,
    };
    if evidence_ref.artifact_path.trim().is_empty() {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            artifact_path_field,
        ));
    }
    let artifact_sha256_field = match field {
        "order_proof" => "order_proof.artifact_sha256",
        "fill_proof" => "fill_proof.artifact_sha256",
        "rounding_proof" => "rounding_proof.artifact_sha256",
        "fee_proof" => "fee_proof.artifact_sha256",
        "settlement_proof" => "settlement_proof.artifact_sha256",
        _ => field,
    };
    if !is_lowercase_sha256(&evidence_ref.artifact_sha256) {
        return Err(provider_artifact_invalid(
            PRODUCT_SUBMIT_PROOF_ARTIFACT,
            artifact_sha256_field,
        ));
    }
    Ok(())
}

fn provider_artifact_invalid(
    artifact: &'static str,
    field: &'static str,
) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::ProviderArtifactInvalid { artifact, field }
}
