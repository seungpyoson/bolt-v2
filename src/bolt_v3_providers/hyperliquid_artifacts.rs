use std::{path::Path, str::FromStr};

use rust_decimal::Decimal;
use serde::Serialize;

use crate::{
    bolt_v3_operator_artifacts::{
        BoltV3OperatorArtifactError, WrittenOperatorArtifact, is_lowercase_sha256,
        write_json_artifact_create_new,
    },
    bolt_v3_submit_admission::{BoltV3ExchangeMutationCounts, validate_no_exchange_mutations},
};

use super::hyperliquid::{self, HyperliquidProductSurface};

const PRODUCT_MATRIX_SCHEMA_VERSION: u32 = 1;
const PRODUCT_MATRIX_RECORD_KIND: &str = "bolt_v3.hyperliquid_product_matrix.v1";
const NO_SUBMIT_READINESS_SCHEMA_VERSION: u32 = 1;
const NO_SUBMIT_READINESS_RECORD_KIND: &str = "bolt_v3.hyperliquid_no_submit_readiness.v1";
const LATENCY_PROFILE_SCHEMA_VERSION: u32 = 1;
const LATENCY_PROFILE_RECORD_KIND: &str = "bolt_v3.hyperliquid_latency_profile.v1";
const LIVE_SUBMIT_APPROVAL_SCHEMA_VERSION: u32 = 1;
const LIVE_SUBMIT_APPROVAL_RECORD_KIND: &str = "bolt_v3.hyperliquid_live_submit_approval.v1";

const LATENCY_PROFILE_ARTIFACT: &str = "hyperliquid_latency_profile";
const NO_SUBMIT_READINESS_ARTIFACT: &str = "hyperliquid_no_submit_readiness";
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidNoSubmitReadinessInput {
    pub base_sha: String,
    pub provider_id: String,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub product_surface: HyperliquidProductSurface,
    pub metadata_evidence: HyperliquidNoSubmitEvidenceRef,
    pub fee_evidence: HyperliquidNoSubmitEvidenceRef,
    pub admission_evidence: HyperliquidNoSubmitEvidenceRef,
    pub exchange_mutations: BoltV3ExchangeMutationCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HyperliquidNoSubmitEvidenceRef {
    pub source_kind: String,
    pub artifact_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct HyperliquidNoSubmitReadinessArtifact {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub provider_key: &'static str,
    pub base_sha: String,
    pub provider_id: String,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub product_surface: HyperliquidProductSurface,
    pub metadata_evidence: HyperliquidNoSubmitEvidenceRef,
    pub fee_evidence: HyperliquidNoSubmitEvidenceRef,
    pub admission_evidence: HyperliquidNoSubmitEvidenceRef,
    pub exchange_mutation_count: u64,
}

pub fn build_hyperliquid_no_submit_readiness_artifact(
    input: HyperliquidNoSubmitReadinessInput,
) -> Result<HyperliquidNoSubmitReadinessArtifact, BoltV3OperatorArtifactError> {
    validate_hyperliquid_no_submit_readiness_input(&input)?;
    let exchange_mutation_count = validate_no_exchange_mutations(input.exchange_mutations)
        .map_err(|_| {
            provider_artifact_invalid(NO_SUBMIT_READINESS_ARTIFACT, "exchange_mutation_count")
        })?;
    Ok(HyperliquidNoSubmitReadinessArtifact {
        schema_version: NO_SUBMIT_READINESS_SCHEMA_VERSION,
        record_kind: NO_SUBMIT_READINESS_RECORD_KIND,
        provider_key: hyperliquid::KEY,
        base_sha: input.base_sha,
        provider_id: input.provider_id,
        toml_checksum: input.toml_checksum,
        signer_fingerprint: input.signer_fingerprint,
        product_surface: input.product_surface,
        metadata_evidence: input.metadata_evidence,
        fee_evidence: input.fee_evidence,
        admission_evidence: input.admission_evidence,
        exchange_mutation_count,
    })
}

pub fn write_hyperliquid_no_submit_readiness_artifact(
    input: HyperliquidNoSubmitReadinessInput,
    output_path: &Path,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let artifact = build_hyperliquid_no_submit_readiness_artifact(input)?;
    write_json_artifact_create_new(output_path, &artifact)
}

fn validate_hyperliquid_no_submit_readiness_input(
    input: &HyperliquidNoSubmitReadinessInput,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_lowercase_hex_field(
        NO_SUBMIT_READINESS_ARTIFACT,
        "base_sha",
        &input.base_sha,
        40,
    )?;
    validate_non_empty_field(
        NO_SUBMIT_READINESS_ARTIFACT,
        "provider_id",
        &input.provider_id,
    )?;
    validate_sha256_field(
        NO_SUBMIT_READINESS_ARTIFACT,
        "toml_checksum",
        &input.toml_checksum,
    )?;
    validate_sha256_field(
        NO_SUBMIT_READINESS_ARTIFACT,
        "signer_fingerprint",
        &input.signer_fingerprint,
    )?;
    if input.product_surface != HyperliquidProductSurface::StandardPerps {
        return Err(provider_artifact_invalid(
            NO_SUBMIT_READINESS_ARTIFACT,
            "product_surface",
        ));
    }
    validate_hyperliquid_no_submit_evidence_ref("metadata_evidence", &input.metadata_evidence)?;
    validate_hyperliquid_no_submit_evidence_ref("fee_evidence", &input.fee_evidence)?;
    validate_hyperliquid_no_submit_evidence_ref("admission_evidence", &input.admission_evidence)?;
    Ok(())
}

fn validate_hyperliquid_no_submit_evidence_ref(
    field: &'static str,
    evidence: &HyperliquidNoSubmitEvidenceRef,
) -> Result<(), BoltV3OperatorArtifactError> {
    validate_non_empty_field(NO_SUBMIT_READINESS_ARTIFACT, field, &evidence.source_kind)?;
    validate_sha256_field(
        NO_SUBMIT_READINESS_ARTIFACT,
        field,
        &evidence.artifact_sha256,
    )
}

fn validate_non_empty_field(
    artifact: &'static str,
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.trim().is_empty() {
        return Err(provider_artifact_invalid(artifact, field));
    }
    Ok(())
}

fn validate_sha256_field(
    artifact: &'static str,
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if is_lowercase_sha256(value) {
        return Ok(());
    }
    Err(provider_artifact_invalid(artifact, field))
}

fn validate_lowercase_hex_field(
    artifact: &'static str,
    field: &'static str,
    value: &str,
    expected_len: usize,
) -> Result<(), BoltV3OperatorArtifactError> {
    if value.len() == expected_len
        && value
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
    {
        return Ok(());
    }
    Err(provider_artifact_invalid(artifact, field))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HyperliquidLiveSubmitOrderLimits {
    pub max_order_count: u32,
    pub max_order_notional: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidLiveSubmitApprovalBinding {
    pub base_sha: String,
    pub provider_id: String,
    pub product_surface: HyperliquidProductSurface,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub order_limits: HyperliquidLiveSubmitOrderLimits,
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
    pub expires_at: u64,
    pub used_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HyperliquidLiveSubmitApprovalArtifact {
    pub schema_version: u32,
    pub record_kind: &'static str,
    pub provider_key: &'static str,
    pub approval_id: String,
    pub base_sha: String,
    pub provider_id: String,
    pub product_surface: HyperliquidProductSurface,
    pub toml_checksum: String,
    pub signer_fingerprint: String,
    pub order_limits: HyperliquidLiveSubmitOrderLimits,
    pub expires_at: u64,
    pub used_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidLiveSubmitApprovalConsumption {
    approval_id: String,
    product_surface: HyperliquidProductSurface,
    used_at: u64,
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
}

pub fn build_hyperliquid_live_submit_approval_artifact(
    input: HyperliquidLiveSubmitApprovalInput,
) -> Result<HyperliquidLiveSubmitApprovalArtifact, BoltV3OperatorArtifactError> {
    validate_hyperliquid_live_submit_approval_input(&input)?;
    Ok(HyperliquidLiveSubmitApprovalArtifact {
        schema_version: LIVE_SUBMIT_APPROVAL_SCHEMA_VERSION,
        record_kind: LIVE_SUBMIT_APPROVAL_RECORD_KIND,
        provider_key: hyperliquid::KEY,
        approval_id: input.approval_id,
        base_sha: input.base_sha,
        provider_id: input.provider_id,
        product_surface: input.product_surface,
        toml_checksum: input.toml_checksum,
        signer_fingerprint: input.signer_fingerprint,
        order_limits: input.order_limits,
        expires_at: input.expires_at,
        used_at: input.used_at,
    })
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
    validate_hyperliquid_live_submit_order_limits(&binding.order_limits)
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

fn provider_artifact_invalid(
    artifact: &'static str,
    field: &'static str,
) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::ProviderArtifactInvalid { artifact, field }
}
