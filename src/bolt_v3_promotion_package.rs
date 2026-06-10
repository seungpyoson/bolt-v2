use serde::{Deserialize, Serialize};

use crate::bolt_v3_event_market_source_proof::SourceProofAdmissionState;
use crate::bolt_v3_profit_evidence::ProfitEvidenceState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionPackageSeed {
    pub package_id: String,
    pub source_proof_hash: String,
    pub provider_capability_hashes: Vec<String>,
    pub profit_evidence_session_hash: String,
    pub generated_config_path: String,
    pub generated_config_sha256: String,
    pub commit_sha: String,
    pub config_checksum: String,
    pub source_admission_state: SourceProofAdmissionState,
    pub profit_evidence_state: ProfitEvidenceState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionPromotionPackage {
    pub package_id: String,
    pub source_proof_hash: String,
    pub provider_capability_hashes: Vec<String>,
    pub profit_evidence_session_hash: String,
    pub generated_config_path: String,
    pub generated_config_sha256: String,
    pub generated_config: DisabledPromotionConfig,
    pub enabled: bool,
    pub commit_sha: String,
    pub config_checksum: String,
    pub operator_review_status: OperatorReviewStatus,
    pub execution_status: PromotionExecutionStatus,
    pub source_admission_state: SourceProofAdmissionState,
    pub profit_evidence_state: ProfitEvidenceState,
    pub requested_side_effects: PromotionSideEffectRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisabledPromotionConfig {
    pub enabled: bool,
    pub live_execution_enabled: bool,
    pub evidence: PromotionEvidenceBinding,
    pub operator_review_status: OperatorReviewStatus,
    pub execution_status: PromotionExecutionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionEvidenceBinding {
    pub source_proof_hash: String,
    pub provider_capability_hashes: Vec<String>,
    pub profit_evidence_session_hash: String,
    pub commit_sha: String,
    pub config_checksum: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorReviewStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionExecutionStatus {
    OperatorReviewOnly,
    CanaryReady,
    LiveEnabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionSideEffectRequest {
    pub mutates_ssm: bool,
    pub mutates_venue_state: bool,
    pub places_orders: bool,
    pub cancels_orders: bool,
    pub transfers_funds: bool,
}

impl PromotionSideEffectRequest {
    fn none() -> Self {
        Self {
            mutates_ssm: false,
            mutates_venue_state: false,
            places_orders: false,
            cancels_orders: false,
            transfers_funds: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionPackageVerification {
    pub state: PromotionPackageState,
    pub rejections: Vec<PromotionPackageRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionPackageState {
    DisabledConfigGenerated,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionPackageRejection {
    pub reason: PromotionPackageRejectionReason,
    pub field: PromotionPackageField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionPackageRejectionReason {
    PackageEnabled,
    GeneratedConfigEnabled,
    LiveExecutionRequested,
    SourceProofMissing,
    ProviderCapabilityMissing,
    ProfitEvidenceMissing,
    GeneratedConfigPathMissing,
    GeneratedConfigHashMissing,
    CommitShaMissing,
    ConfigChecksumMissing,
    SourceEvidenceNotAccepted,
    ProfitEvidenceNotAccepted,
    GeneratedConfigBindingMismatch,
    SideEffectRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionPackageField {
    PackageEnabled,
    GeneratedConfigEnabled,
    LiveExecutionEnabled,
    SourceProofHash,
    ProviderCapabilityHashes,
    ProfitEvidenceSessionHash,
    GeneratedConfigPath,
    GeneratedConfigSha256,
    CommitSha,
    ConfigChecksum,
    SourceAdmissionState,
    ProfitEvidenceState,
    GeneratedConfigBindings,
    SsmMutation,
    VenueStateMutation,
    OrderPlacement,
    OrderCancellation,
    FundTransfer,
}

pub fn generate_disabled_promotion_package(
    seed: PromotionPackageSeed,
) -> ProductionPromotionPackage {
    let evidence = PromotionEvidenceBinding {
        source_proof_hash: seed.source_proof_hash.clone(),
        provider_capability_hashes: seed.provider_capability_hashes.clone(),
        profit_evidence_session_hash: seed.profit_evidence_session_hash.clone(),
        commit_sha: seed.commit_sha.clone(),
        config_checksum: seed.config_checksum.clone(),
    };
    let generated_config = DisabledPromotionConfig {
        enabled: false,
        live_execution_enabled: false,
        evidence,
        operator_review_status: OperatorReviewStatus::Pending,
        execution_status: PromotionExecutionStatus::OperatorReviewOnly,
    };

    ProductionPromotionPackage {
        package_id: seed.package_id,
        source_proof_hash: seed.source_proof_hash,
        provider_capability_hashes: seed.provider_capability_hashes,
        profit_evidence_session_hash: seed.profit_evidence_session_hash,
        generated_config_path: seed.generated_config_path,
        generated_config_sha256: seed.generated_config_sha256,
        generated_config,
        enabled: false,
        commit_sha: seed.commit_sha,
        config_checksum: seed.config_checksum,
        operator_review_status: OperatorReviewStatus::Pending,
        execution_status: PromotionExecutionStatus::OperatorReviewOnly,
        source_admission_state: seed.source_admission_state,
        profit_evidence_state: seed.profit_evidence_state,
        requested_side_effects: PromotionSideEffectRequest::none(),
    }
}

pub fn render_disabled_promotion_toml(
    config: &DisabledPromotionConfig,
) -> Result<String, toml::ser::Error> {
    toml::to_string(config)
}

pub fn verify_production_promotion_package(
    package: &ProductionPromotionPackage,
) -> PromotionPackageVerification {
    let mut rejections = Vec::new();

    validate_disabled_state(package, &mut rejections);
    validate_required_bindings(package, &mut rejections);
    validate_accepted_evidence_state(package, &mut rejections);
    validate_generated_config_binding(package, &mut rejections);
    validate_side_effect_request(package, &mut rejections);

    let state = if rejections.is_empty() {
        PromotionPackageState::DisabledConfigGenerated
    } else {
        PromotionPackageState::Rejected
    };

    PromotionPackageVerification { state, rejections }
}

fn validate_disabled_state(
    package: &ProductionPromotionPackage,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    if package.enabled {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::PackageEnabled,
            field: PromotionPackageField::PackageEnabled,
        });
    }
    if package.generated_config.enabled {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::GeneratedConfigEnabled,
            field: PromotionPackageField::GeneratedConfigEnabled,
        });
    }
    if package.generated_config.live_execution_enabled {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::LiveExecutionRequested,
            field: PromotionPackageField::LiveExecutionEnabled,
        });
    }
}

fn validate_required_bindings(
    package: &ProductionPromotionPackage,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    reject_blank(
        &package.source_proof_hash,
        PromotionPackageRejectionReason::SourceProofMissing,
        PromotionPackageField::SourceProofHash,
        rejections,
    );
    if package
        .provider_capability_hashes
        .iter()
        .all(|value| is_blank(value))
    {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::ProviderCapabilityMissing,
            field: PromotionPackageField::ProviderCapabilityHashes,
        });
    }
    reject_blank(
        &package.profit_evidence_session_hash,
        PromotionPackageRejectionReason::ProfitEvidenceMissing,
        PromotionPackageField::ProfitEvidenceSessionHash,
        rejections,
    );
    reject_blank(
        &package.generated_config_path,
        PromotionPackageRejectionReason::GeneratedConfigPathMissing,
        PromotionPackageField::GeneratedConfigPath,
        rejections,
    );
    reject_blank(
        &package.generated_config_sha256,
        PromotionPackageRejectionReason::GeneratedConfigHashMissing,
        PromotionPackageField::GeneratedConfigSha256,
        rejections,
    );
    reject_blank(
        &package.commit_sha,
        PromotionPackageRejectionReason::CommitShaMissing,
        PromotionPackageField::CommitSha,
        rejections,
    );
    reject_blank(
        &package.config_checksum,
        PromotionPackageRejectionReason::ConfigChecksumMissing,
        PromotionPackageField::ConfigChecksum,
        rejections,
    );
}

fn validate_accepted_evidence_state(
    package: &ProductionPromotionPackage,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    if package.source_admission_state != SourceProofAdmissionState::CaptureEligible {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::SourceEvidenceNotAccepted,
            field: PromotionPackageField::SourceAdmissionState,
        });
    }
    if package.profit_evidence_state != ProfitEvidenceState::PromotionReady {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::ProfitEvidenceNotAccepted,
            field: PromotionPackageField::ProfitEvidenceState,
        });
    }
}

fn validate_generated_config_binding(
    package: &ProductionPromotionPackage,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    if package.generated_config.evidence != expected_evidence_binding(package) {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::GeneratedConfigBindingMismatch,
            field: PromotionPackageField::GeneratedConfigBindings,
        });
    }
}

fn expected_evidence_binding(package: &ProductionPromotionPackage) -> PromotionEvidenceBinding {
    PromotionEvidenceBinding {
        source_proof_hash: package.source_proof_hash.clone(),
        provider_capability_hashes: package.provider_capability_hashes.clone(),
        profit_evidence_session_hash: package.profit_evidence_session_hash.clone(),
        commit_sha: package.commit_sha.clone(),
        config_checksum: package.config_checksum.clone(),
    }
}

fn validate_side_effect_request(
    package: &ProductionPromotionPackage,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    reject_side_effect(
        package.requested_side_effects.mutates_ssm,
        PromotionPackageField::SsmMutation,
        rejections,
    );
    reject_side_effect(
        package.requested_side_effects.mutates_venue_state,
        PromotionPackageField::VenueStateMutation,
        rejections,
    );
    reject_side_effect(
        package.requested_side_effects.places_orders,
        PromotionPackageField::OrderPlacement,
        rejections,
    );
    reject_side_effect(
        package.requested_side_effects.cancels_orders,
        PromotionPackageField::OrderCancellation,
        rejections,
    );
    reject_side_effect(
        package.requested_side_effects.transfers_funds,
        PromotionPackageField::FundTransfer,
        rejections,
    );
}

fn reject_side_effect(
    requested: bool,
    field: PromotionPackageField,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    if requested {
        rejections.push(PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::SideEffectRequested,
            field,
        });
    }
}

fn reject_blank(
    value: &str,
    reason: PromotionPackageRejectionReason,
    field: PromotionPackageField,
    rejections: &mut Vec<PromotionPackageRejection>,
) {
    if is_blank(value) {
        rejections.push(PromotionPackageRejection { reason, field });
    }
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_seed() -> PromotionPackageSeed {
        PromotionPackageSeed {
            package_id: "pkg-001".to_string(),
            source_proof_hash: "sha256:source".to_string(),
            provider_capability_hashes: vec!["sha256:provider".to_string()],
            profit_evidence_session_hash: "sha256:profit-evidence".to_string(),
            generated_config_path: "artifacts/promotions/pkg-001.toml".to_string(),
            generated_config_sha256: "sha256:generated-config".to_string(),
            commit_sha: "abc123".to_string(),
            config_checksum: "sha256:root-config".to_string(),
            source_admission_state: SourceProofAdmissionState::CaptureEligible,
            profit_evidence_state: ProfitEvidenceState::PromotionReady,
        }
    }

    #[test]
    fn generated_package_is_disabled_toml_bound_to_evidence() {
        let package = generate_disabled_promotion_package(valid_seed());

        assert!(!package.enabled);
        assert!(!package.generated_config.enabled);
        assert!(!package.generated_config.live_execution_enabled);
        assert_eq!(
            package.operator_review_status,
            OperatorReviewStatus::Pending
        );
        assert_eq!(
            package.execution_status,
            PromotionExecutionStatus::OperatorReviewOnly
        );
        assert_eq!(
            package.generated_config.evidence.source_proof_hash,
            package.source_proof_hash
        );
        assert_eq!(
            package.generated_config.evidence.provider_capability_hashes,
            package.provider_capability_hashes
        );
        assert_eq!(
            package
                .generated_config
                .evidence
                .profit_evidence_session_hash,
            package.profit_evidence_session_hash
        );
        assert_eq!(
            package.generated_config.evidence.commit_sha,
            package.commit_sha
        );
        assert_eq!(
            package.generated_config.evidence.config_checksum,
            package.config_checksum
        );

        let rendered =
            render_disabled_promotion_toml(&package.generated_config).expect("typed toml renders");
        let decoded: toml::Value = toml::from_str(&rendered).expect("typed toml parses");

        assert_eq!(
            decoded.get("enabled").and_then(toml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            decoded
                .get("live_execution_enabled")
                .and_then(toml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            verify_production_promotion_package(&package).state,
            PromotionPackageState::DisabledConfigGenerated
        );
    }

    #[test]
    fn enabled_package_or_live_generated_config_rejects() {
        let mut package = generate_disabled_promotion_package(valid_seed());
        package.enabled = true;
        package.generated_config.enabled = true;
        package.generated_config.live_execution_enabled = true;

        let verification = verify_production_promotion_package(&package);

        assert_eq!(verification.state, PromotionPackageState::Rejected);
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::PackageEnabled,
                    field: PromotionPackageField::PackageEnabled,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::GeneratedConfigEnabled,
                    field: PromotionPackageField::GeneratedConfigEnabled,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::LiveExecutionRequested,
                    field: PromotionPackageField::LiveExecutionEnabled,
                })
        );
    }

    #[test]
    fn generated_config_binding_mismatch_rejects() {
        let mut package = generate_disabled_promotion_package(valid_seed());
        package.generated_config.evidence.source_proof_hash = "sha256:other-source".to_string();
        package.generated_config.evidence.provider_capability_hashes =
            vec!["sha256:other-provider".to_string()];
        package
            .generated_config
            .evidence
            .profit_evidence_session_hash = "sha256:other-profit".to_string();
        package.generated_config.evidence.commit_sha = "def456".to_string();
        package.generated_config.evidence.config_checksum = "sha256:other-config".to_string();

        let verification = verify_production_promotion_package(&package);

        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::GeneratedConfigBindingMismatch,
                    field: PromotionPackageField::GeneratedConfigBindings,
                })
        );
    }

    #[test]
    fn missing_required_artifact_hashes_or_path_rejects() {
        let mut package = generate_disabled_promotion_package(valid_seed());
        package.source_proof_hash.clear();
        package.provider_capability_hashes.clear();
        package.profit_evidence_session_hash.clear();
        package.generated_config_path.clear();
        package.generated_config_sha256.clear();
        package.commit_sha.clear();
        package.config_checksum.clear();

        let verification = verify_production_promotion_package(&package);

        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SourceProofMissing,
                    field: PromotionPackageField::SourceProofHash,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::ProviderCapabilityMissing,
                    field: PromotionPackageField::ProviderCapabilityHashes,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::ProfitEvidenceMissing,
                    field: PromotionPackageField::ProfitEvidenceSessionHash,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::GeneratedConfigPathMissing,
                    field: PromotionPackageField::GeneratedConfigPath,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::GeneratedConfigHashMissing,
                    field: PromotionPackageField::GeneratedConfigSha256,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::CommitShaMissing,
                    field: PromotionPackageField::CommitSha,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::ConfigChecksumMissing,
                    field: PromotionPackageField::ConfigChecksum,
                })
        );
    }

    #[test]
    fn rejected_source_or_profit_state_blocks_package() {
        let mut package = generate_disabled_promotion_package(valid_seed());
        package.source_admission_state = SourceProofAdmissionState::SourceProofRejected;
        package.profit_evidence_state = ProfitEvidenceState::Rejected;

        let verification = verify_production_promotion_package(&package);

        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SourceEvidenceNotAccepted,
                    field: PromotionPackageField::SourceAdmissionState,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::ProfitEvidenceNotAccepted,
                    field: PromotionPackageField::ProfitEvidenceState,
                })
        );
    }

    #[test]
    fn promotion_package_rejects_secret_venue_order_and_fund_mutation_requests() {
        let mut package = generate_disabled_promotion_package(valid_seed());
        package.requested_side_effects = PromotionSideEffectRequest {
            mutates_ssm: true,
            mutates_venue_state: true,
            places_orders: true,
            cancels_orders: true,
            transfers_funds: true,
        };

        let verification = verify_production_promotion_package(&package);

        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SideEffectRequested,
                    field: PromotionPackageField::SsmMutation,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SideEffectRequested,
                    field: PromotionPackageField::VenueStateMutation,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SideEffectRequested,
                    field: PromotionPackageField::OrderPlacement,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SideEffectRequested,
                    field: PromotionPackageField::OrderCancellation,
                })
        );
        assert!(
            verification
                .rejections
                .contains(&PromotionPackageRejection {
                    reason: PromotionPackageRejectionReason::SideEffectRequested,
                    field: PromotionPackageField::FundTransfer,
                })
        );
    }

    #[test]
    fn rejection_reason_serializes_to_snake_case() {
        let rejection = PromotionPackageRejection {
            reason: PromotionPackageRejectionReason::LiveExecutionRequested,
            field: PromotionPackageField::LiveExecutionEnabled,
        };

        let encoded = serde_json::to_value(rejection).expect("promotion rejection serializes");

        assert_eq!(encoded["reason"], "live_execution_requested");
        assert_eq!(encoded["field"], "live_execution_enabled");
    }
}
