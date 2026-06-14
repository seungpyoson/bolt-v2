//! Pure Research Analytics promotion-package contract helpers.
//!
//! This module deliberately does not run backtests, mutate source-proof or BTE
//! artifacts, touch SSM, or write runtime config. It validates that an RA-owned
//! promotion package is only a typed, claim-limited handoff artifact.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{artifact_index::ResearchAnalyticsSubfamily, source_proof::SourceProofFidelityClass};

const RESEARCH_ANALYTICS_KIND_PATH: &str = "research-analytics";
const RESEARCH_ANALYTICS_SCHEMA_VERSION: &str = "v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStatus {
    Draft,
    Blocked,
    ReadyForReview,
    ChangesRequested,
    Rejected,
    ApprovedForConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostApprovalAction {
    AutoMerge,
    AutoEnableStrategy,
    ScheduleLiveTrading,
    TouchSsmCredentials,
    MutateProductionRuntimeConfig,
}

impl PostApprovalAction {
    const fn description(self) -> &'static str {
        match self {
            Self::AutoMerge => "auto-merge",
            Self::AutoEnableStrategy => "auto-enable strategy",
            Self::ScheduleLiveTrading => "schedule live trading",
            Self::TouchSsmCredentials => "touch SSM credentials",
            Self::MutateProductionRuntimeConfig => "mutate production runtime config",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProofEvidenceRef {
    pub source_proof_id: String,
    pub source_proof_version: Option<u64>,
    pub source_proof_report_uri: String,
    pub source_proof_report_hash: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub accepted: bool,
}

impl SourceProofEvidenceRef {
    fn validate(&self) -> Result<(), PromotionPackageError> {
        validate_non_empty("source_proof_id", &self.source_proof_id)?;
        validate_non_empty("source_proof_report_uri", &self.source_proof_report_uri)?;
        validate_sha256("source_proof_report_hash", &self.source_proof_report_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestEvidenceRef {
    pub result_contract_id: String,
    pub result_contract_uri: String,
    pub result_contract_hash: String,
    pub objective: bool,
}

impl BacktestEvidenceRef {
    fn validate(&self) -> Result<(), PromotionPackageError> {
        validate_non_empty("result_contract_id", &self.result_contract_id)?;
        validate_non_empty("result_contract_uri", &self.result_contract_uri)?;
        validate_sha256("result_contract_hash", &self.result_contract_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionPackage {
    pub package_version: u64,
    pub artifact_root: String,
    pub artifact_uri: String,
    pub status: PromotionStatus,
    pub source_proof_refs: Vec<SourceProofEvidenceRef>,
    pub backtest_result_refs: Vec<BacktestEvidenceRef>,
    pub preserved_claim_limits: Vec<String>,
    pub requested_claim_fidelity: SourceProofFidelityClass,
    pub typed_config_uri: Option<String>,
    pub typed_config_hash: Option<String>,
    pub dashboard_field_refs: Vec<String>,
    pub reviewer_policy_refs: Vec<String>,
    pub non_live_boundary: bool,
    pub notebook_runtime_code_refs: Vec<String>,
    pub accepts_source_proofs: bool,
    pub mutates_source_proofs: bool,
    pub mutates_backtest_result_contracts: bool,
    pub weakens_forbidden_claims: bool,
    pub post_approval_actions: Vec<PostApprovalAction>,
}

impl PromotionPackage {
    pub fn validate(&self) -> Result<(), PromotionPackageError> {
        validate_package_identity(self)?;
        validate_promotion_family_uri("artifact_uri", &self.artifact_root, &self.artifact_uri)?;

        for source_ref in &self.source_proof_refs {
            source_ref.validate()?;
        }
        for backtest_ref in &self.backtest_result_refs {
            backtest_ref.validate()?;
        }
        for claim_limit in &self.preserved_claim_limits {
            validate_non_empty("preserved_claim_limits", claim_limit)?;
        }
        for reviewer_ref in &self.reviewer_policy_refs {
            validate_non_empty("reviewer_policy_refs", reviewer_ref)?;
        }
        for dashboard_ref in &self.dashboard_field_refs {
            validate_non_empty("dashboard_field_refs", dashboard_ref)?;
        }
        for notebook_ref in &self.notebook_runtime_code_refs {
            validate_non_empty("notebook_runtime_code_refs", notebook_ref)?;
        }
        if let Some(uri) = &self.typed_config_uri {
            validate_promotion_family_uri("typed_config_uri", &self.artifact_root, uri)?;
        }
        if let Some(hash) = &self.typed_config_hash {
            validate_sha256("typed_config_hash", hash)?;
        }

        let forbidden = self.forbidden_behavior_violations();
        if !forbidden.is_empty() {
            return Err(PromotionPackageError::ForbiddenPromotionPackageBehavior {
                violations: forbidden,
            });
        }

        if self.status == PromotionStatus::ApprovedForConfig {
            let missing = self.approved_for_config_missing_requirements();
            if !missing.is_empty() {
                return Err(PromotionPackageError::ApprovedForConfigMissing { missing });
            }
        }

        Ok(())
    }

    fn approved_for_config_missing_requirements(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self
            .source_proof_refs
            .iter()
            .any(|source_ref| source_ref.accepted)
            || self
                .source_proof_refs
                .iter()
                .any(|source_ref| !source_ref.accepted)
        {
            missing.push("accepted source proof refs");
        }
        if !self
            .backtest_result_refs
            .iter()
            .any(|backtest_ref| backtest_ref.objective)
            || self
                .backtest_result_refs
                .iter()
                .any(|backtest_ref| !backtest_ref.objective)
        {
            missing.push("objective backtest result refs");
        }
        if self.preserved_claim_limits.is_empty() {
            missing.push("preserved claim limits");
        }
        if self
            .typed_config_uri
            .as_deref()
            .is_none_or(|uri| uri.trim().is_empty())
        {
            missing.push("typed config uri");
        }
        if self
            .typed_config_hash
            .as_deref()
            .is_none_or(|hash| hash.trim().is_empty())
        {
            missing.push("typed config hash");
        }
        if self.reviewer_policy_refs.is_empty() {
            missing.push("reviewer/policy refs");
        }
        if !self.non_live_boundary {
            missing.push("explicit non-live boundary");
        }
        missing
    }

    fn forbidden_behavior_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.requested_claim_is_incompatible_with_source_fidelity() {
            violations.push("proof-strength upgrade or fidelity-incompatible claim".to_string());
        }
        if self.accepts_source_proofs {
            violations.push("unauthorized proof acceptance".to_string());
        }
        if self.mutates_source_proofs {
            violations.push("source proof mutation".to_string());
        }
        if self.mutates_backtest_result_contracts {
            violations.push("backtest result contract mutation".to_string());
        }
        if self.weakens_forbidden_claims {
            violations.push("forbidden-claim weakening".to_string());
        }
        if !self.notebook_runtime_code_refs.is_empty() {
            violations.push("notebook runtime code".to_string());
        }
        violations.extend(
            self.post_approval_actions
                .iter()
                .map(|action| action.description().to_string()),
        );
        violations
    }

    fn requested_claim_is_incompatible_with_source_fidelity(&self) -> bool {
        self.source_proof_refs.iter().any(|source_ref| {
            !source_fidelity_supports_claim(
                source_ref.fidelity_class,
                self.requested_claim_fidelity,
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionPackageError {
    EmptyField {
        field: &'static str,
    },
    InvalidPackageVersion,
    InvalidSha256 {
        field: &'static str,
        value: String,
    },
    UnsupportedArtifactRoot {
        artifact_root: String,
    },
    ArtifactOutsidePromotionFamily {
        field: &'static str,
        artifact_root: String,
        uri: String,
        expected_prefix: String,
    },
    ApprovedForConfigMissing {
        missing: Vec<&'static str>,
    },
    ForbiddenPromotionPackageBehavior {
        violations: Vec<String>,
    },
}

impl fmt::Display for PromotionPackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => write!(formatter, "{field} must not be empty"),
            Self::InvalidPackageVersion => {
                write!(formatter, "package_version must be greater than zero")
            }
            Self::InvalidSha256 { field, value } => {
                write!(
                    formatter,
                    "{field} must be lowercase sha256 hex, got {value:?}"
                )
            }
            Self::UnsupportedArtifactRoot { artifact_root } => {
                write!(
                    formatter,
                    "artifact_root must be an s3:// URI, got {artifact_root:?}"
                )
            }
            Self::ArtifactOutsidePromotionFamily {
                field,
                artifact_root,
                uri,
                expected_prefix,
            } => write!(
                formatter,
                "{field} {uri:?} is outside RA promotion package family for artifact_root {artifact_root:?}; expected prefix {expected_prefix:?}"
            ),
            Self::ApprovedForConfigMissing { missing } => write!(
                formatter,
                "approved_for_config missing required evidence/boundaries: {}",
                missing.join(", ")
            ),
            Self::ForbiddenPromotionPackageBehavior { violations } => write!(
                formatter,
                "promotion package contains forbidden behavior: {}",
                violations.join(", ")
            ),
        }
    }
}

impl Error for PromotionPackageError {}

fn validate_package_identity(package: &PromotionPackage) -> Result<(), PromotionPackageError> {
    if package.package_version == 0 {
        return Err(PromotionPackageError::InvalidPackageVersion);
    }
    validate_non_empty("artifact_root", &package.artifact_root)?;
    validate_non_empty("artifact_uri", &package.artifact_uri)?;
    if !package.artifact_root.starts_with("s3://") {
        return Err(PromotionPackageError::UnsupportedArtifactRoot {
            artifact_root: package.artifact_root.clone(),
        });
    }
    Ok(())
}

fn validate_promotion_family_uri(
    field: &'static str,
    artifact_root: &str,
    uri: &str,
) -> Result<(), PromotionPackageError> {
    validate_non_empty(field, uri)?;
    let expected_prefix = promotion_package_prefix(artifact_root);
    if uri.starts_with(&expected_prefix) {
        Ok(())
    } else {
        Err(PromotionPackageError::ArtifactOutsidePromotionFamily {
            field,
            artifact_root: artifact_root.to_string(),
            uri: uri.to_string(),
            expected_prefix,
        })
    }
}

fn promotion_package_prefix(artifact_root: &str) -> String {
    format!(
        "{}/{}/{}/{}/",
        artifact_root.trim_end_matches('/'),
        RESEARCH_ANALYTICS_KIND_PATH,
        RESEARCH_ANALYTICS_SCHEMA_VERSION,
        ResearchAnalyticsSubfamily::PromotionPackages.as_str()
    )
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), PromotionPackageError> {
    if value.trim().is_empty() {
        Err(PromotionPackageError::EmptyField { field })
    } else {
        Ok(())
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), PromotionPackageError> {
    let is_sha256 = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if is_sha256 {
        Ok(())
    } else {
        Err(PromotionPackageError::InvalidSha256 {
            field,
            value: value.to_string(),
        })
    }
}

fn source_fidelity_supports_claim(
    source: SourceProofFidelityClass,
    requested: SourceProofFidelityClass,
) -> bool {
    match source {
        SourceProofFidelityClass::L2Replay => true,
        SourceProofFidelityClass::SnapshotReplay => matches!(
            requested,
            SourceProofFidelityClass::SnapshotReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::TradeReplay => matches!(
            requested,
            SourceProofFidelityClass::TradeReplay
                | SourceProofFidelityClass::TradeBarReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::TradeBarReplay => matches!(
            requested,
            SourceProofFidelityClass::TradeBarReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::QuoteReplay => matches!(
            requested,
            SourceProofFidelityClass::QuoteReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::IndexReplay => matches!(
            requested,
            SourceProofFidelityClass::IndexReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::MarkReplay => matches!(
            requested,
            SourceProofFidelityClass::MarkReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::SignalOnly => matches!(
            requested,
            SourceProofFidelityClass::SignalOnly | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::MetadataOnly => {
            matches!(requested, SourceProofFidelityClass::MetadataOnly)
        }
        SourceProofFidelityClass::ForwardCapturePending => {
            matches!(requested, SourceProofFidelityClass::ForwardCapturePending)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_replay_supports_self_signal_and_metadata_only() {
        // A QuoteReplay source backs QuoteReplay, SignalOnly, and MetadataOnly
        // claims; it must not back any foreign replay class.
        for requested in [
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::QuoteReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::IndexReplay,
            SourceProofFidelityClass::MarkReplay,
            SourceProofFidelityClass::L2Replay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::QuoteReplay,
                requested,
            ));
        }
    }

    #[test]
    fn index_replay_supports_self_signal_and_metadata_only() {
        for requested in [
            SourceProofFidelityClass::IndexReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::IndexReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::MarkReplay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::IndexReplay,
                requested,
            ));
        }
    }

    #[test]
    fn mark_replay_supports_self_signal_and_metadata_only() {
        for requested in [
            SourceProofFidelityClass::MarkReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::MarkReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::IndexReplay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::MarkReplay,
                requested,
            ));
        }
    }
}
