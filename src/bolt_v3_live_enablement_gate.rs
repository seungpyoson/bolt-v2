use serde::{Deserialize, Serialize};

use crate::bolt_v3_promotion_package::PromotionPackageState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEnablementGatePacket {
    pub gate_id: String,
    pub promotion_package_hash: String,
    pub promotion_package_state: PromotionPackageState,
    pub exact_head_commit_sha: String,
    pub scope: LiveGateScope,
    pub ci_status: LiveGateArtifactProof,
    pub source_fence_status: LiveGateArtifactProof,
    pub no_submit_report: LiveGateArtifactProof,
    pub tiny_canary_proof: LiveGateArtifactProof,
    pub operator_approval: OperatorApprovalProof,
    pub legal_geography_proof: LiveGateArtifactProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveGateScope {
    pub venue_account_product_hash: String,
    pub market_family_hash: String,
    pub config_checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveGateArtifactProof {
    pub artifact_hash: String,
    pub package_hash: String,
    pub commit_sha: String,
    pub venue_account_product_hash: String,
    pub market_family_hash: String,
    pub config_checksum: String,
    pub retrieved_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorApprovalProof {
    pub artifact: LiveGateArtifactProof,
    pub decision: OperatorApprovalDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorApprovalDecision {
    Approved,
    Pending,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEnablementGateEvaluation {
    pub state: LiveEnablementGateState,
    pub rejections: Vec<LiveEnablementGateRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEnablementGateState {
    TinyCanaryReady,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEnablementGateRejection {
    pub reason: LiveEnablementGateRejectionReason,
    pub field: LiveEnablementGateField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEnablementGateRejectionReason {
    GateIdMissing,
    PromotionPackageMissing,
    PromotionPackageNotDisabled,
    ExactHeadCommitMissing,
    VenueAccountProductMissing,
    MarketFamilyMissing,
    ConfigChecksumMissing,
    CiStatusMissing,
    CiStatusStale,
    SourceFenceStatusMissing,
    SourceFenceStatusStale,
    NoSubmitReportMissing,
    NoSubmitReportStale,
    TinyCanaryProofMissing,
    TinyCanaryProofStale,
    OperatorApprovalMissing,
    OperatorApprovalStale,
    OperatorApprovalNotApproved,
    LegalGeographyProofMissing,
    LegalGeographyProofStale,
    ArtifactPackageMismatch,
    ArtifactCommitMismatch,
    ArtifactScopeMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEnablementGateField {
    GateId,
    PromotionPackageHash,
    PromotionPackageState,
    ExactHeadCommitSha,
    VenueAccountProductHash,
    MarketFamilyHash,
    ConfigChecksum,
    CiStatus,
    SourceFenceStatus,
    NoSubmitReport,
    TinyCanaryProof,
    OperatorApproval,
    LegalGeographyProof,
}

pub fn evaluate_live_enablement_gate(
    packet: &LiveEnablementGatePacket,
    as_of_unix_seconds: u64,
) -> LiveEnablementGateEvaluation {
    let mut rejections = Vec::new();

    validate_gate_identity(packet, &mut rejections);
    validate_artifact(
        packet,
        &packet.ci_status,
        LiveEnablementGateField::CiStatus,
        LiveEnablementGateRejectionReason::CiStatusMissing,
        LiveEnablementGateRejectionReason::CiStatusStale,
        as_of_unix_seconds,
        &mut rejections,
    );
    validate_artifact(
        packet,
        &packet.source_fence_status,
        LiveEnablementGateField::SourceFenceStatus,
        LiveEnablementGateRejectionReason::SourceFenceStatusMissing,
        LiveEnablementGateRejectionReason::SourceFenceStatusStale,
        as_of_unix_seconds,
        &mut rejections,
    );
    validate_artifact(
        packet,
        &packet.no_submit_report,
        LiveEnablementGateField::NoSubmitReport,
        LiveEnablementGateRejectionReason::NoSubmitReportMissing,
        LiveEnablementGateRejectionReason::NoSubmitReportStale,
        as_of_unix_seconds,
        &mut rejections,
    );
    validate_artifact(
        packet,
        &packet.tiny_canary_proof,
        LiveEnablementGateField::TinyCanaryProof,
        LiveEnablementGateRejectionReason::TinyCanaryProofMissing,
        LiveEnablementGateRejectionReason::TinyCanaryProofStale,
        as_of_unix_seconds,
        &mut rejections,
    );
    validate_operator_approval(packet, as_of_unix_seconds, &mut rejections);
    validate_artifact(
        packet,
        &packet.legal_geography_proof,
        LiveEnablementGateField::LegalGeographyProof,
        LiveEnablementGateRejectionReason::LegalGeographyProofMissing,
        LiveEnablementGateRejectionReason::LegalGeographyProofStale,
        as_of_unix_seconds,
        &mut rejections,
    );

    let state = if rejections.is_empty() {
        LiveEnablementGateState::TinyCanaryReady
    } else {
        LiveEnablementGateState::Rejected
    };

    LiveEnablementGateEvaluation { state, rejections }
}

fn validate_gate_identity(
    packet: &LiveEnablementGatePacket,
    rejections: &mut Vec<LiveEnablementGateRejection>,
) {
    reject_blank(
        &packet.gate_id,
        LiveEnablementGateRejectionReason::GateIdMissing,
        LiveEnablementGateField::GateId,
        rejections,
    );
    reject_blank(
        &packet.promotion_package_hash,
        LiveEnablementGateRejectionReason::PromotionPackageMissing,
        LiveEnablementGateField::PromotionPackageHash,
        rejections,
    );
    if packet.promotion_package_state != PromotionPackageState::DisabledConfigGenerated {
        rejections.push(LiveEnablementGateRejection {
            reason: LiveEnablementGateRejectionReason::PromotionPackageNotDisabled,
            field: LiveEnablementGateField::PromotionPackageState,
        });
    }
    reject_blank(
        &packet.exact_head_commit_sha,
        LiveEnablementGateRejectionReason::ExactHeadCommitMissing,
        LiveEnablementGateField::ExactHeadCommitSha,
        rejections,
    );
    reject_blank(
        &packet.scope.venue_account_product_hash,
        LiveEnablementGateRejectionReason::VenueAccountProductMissing,
        LiveEnablementGateField::VenueAccountProductHash,
        rejections,
    );
    reject_blank(
        &packet.scope.market_family_hash,
        LiveEnablementGateRejectionReason::MarketFamilyMissing,
        LiveEnablementGateField::MarketFamilyHash,
        rejections,
    );
    reject_blank(
        &packet.scope.config_checksum,
        LiveEnablementGateRejectionReason::ConfigChecksumMissing,
        LiveEnablementGateField::ConfigChecksum,
        rejections,
    );
}

fn validate_operator_approval(
    packet: &LiveEnablementGatePacket,
    as_of_unix_seconds: u64,
    rejections: &mut Vec<LiveEnablementGateRejection>,
) {
    validate_artifact(
        packet,
        &packet.operator_approval.artifact,
        LiveEnablementGateField::OperatorApproval,
        LiveEnablementGateRejectionReason::OperatorApprovalMissing,
        LiveEnablementGateRejectionReason::OperatorApprovalStale,
        as_of_unix_seconds,
        rejections,
    );
    if packet.operator_approval.decision != OperatorApprovalDecision::Approved {
        rejections.push(LiveEnablementGateRejection {
            reason: LiveEnablementGateRejectionReason::OperatorApprovalNotApproved,
            field: LiveEnablementGateField::OperatorApproval,
        });
    }
}

fn validate_artifact(
    packet: &LiveEnablementGatePacket,
    artifact: &LiveGateArtifactProof,
    field: LiveEnablementGateField,
    missing_reason: LiveEnablementGateRejectionReason,
    stale_reason: LiveEnablementGateRejectionReason,
    as_of_unix_seconds: u64,
    rejections: &mut Vec<LiveEnablementGateRejection>,
) {
    if is_blank(&artifact.artifact_hash) {
        rejections.push(LiveEnablementGateRejection {
            reason: missing_reason,
            field,
        });
    }
    if artifact.retrieved_at_unix_seconds >= artifact.expires_at_unix_seconds
        || as_of_unix_seconds >= artifact.expires_at_unix_seconds
    {
        rejections.push(LiveEnablementGateRejection {
            reason: stale_reason,
            field,
        });
    }
    if artifact.package_hash != packet.promotion_package_hash {
        rejections.push(LiveEnablementGateRejection {
            reason: LiveEnablementGateRejectionReason::ArtifactPackageMismatch,
            field,
        });
    }
    if artifact.commit_sha != packet.exact_head_commit_sha {
        rejections.push(LiveEnablementGateRejection {
            reason: LiveEnablementGateRejectionReason::ArtifactCommitMismatch,
            field,
        });
    }
    if !artifact_scope_matches(artifact, &packet.scope) {
        rejections.push(LiveEnablementGateRejection {
            reason: LiveEnablementGateRejectionReason::ArtifactScopeMismatch,
            field,
        });
    }
}

fn artifact_scope_matches(artifact: &LiveGateArtifactProof, scope: &LiveGateScope) -> bool {
    artifact.venue_account_product_hash == scope.venue_account_product_hash
        && artifact.market_family_hash == scope.market_family_hash
        && artifact.config_checksum == scope.config_checksum
}

fn reject_blank(
    value: &str,
    reason: LiveEnablementGateRejectionReason,
    field: LiveEnablementGateField,
    rejections: &mut Vec<LiveEnablementGateRejection>,
) {
    if is_blank(value) {
        rejections.push(LiveEnablementGateRejection { reason, field });
    }
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::bolt_v3_promotion_package::PromotionPackageState;

    const AS_OF_UNIX_SECONDS: u64 = 1_780_000_100;

    fn valid_scope() -> LiveGateScope {
        LiveGateScope {
            venue_account_product_hash: "sha256:venue-account-product".to_string(),
            market_family_hash: "sha256:market-family".to_string(),
            config_checksum: "sha256:config".to_string(),
        }
    }

    fn valid_artifact(artifact_hash: &str) -> LiveGateArtifactProof {
        let scope = valid_scope();
        LiveGateArtifactProof {
            artifact_hash: artifact_hash.to_string(),
            package_hash: "sha256:promotion-package".to_string(),
            commit_sha: "abc123".to_string(),
            venue_account_product_hash: scope.venue_account_product_hash,
            market_family_hash: scope.market_family_hash,
            config_checksum: scope.config_checksum,
            retrieved_at_unix_seconds: 1_780_000_000,
            expires_at_unix_seconds: 1_780_086_400,
        }
    }

    fn valid_packet() -> LiveEnablementGatePacket {
        LiveEnablementGatePacket {
            gate_id: "gate-001".to_string(),
            promotion_package_hash: "sha256:promotion-package".to_string(),
            promotion_package_state: PromotionPackageState::DisabledConfigGenerated,
            exact_head_commit_sha: "abc123".to_string(),
            scope: valid_scope(),
            ci_status: valid_artifact("sha256:ci"),
            source_fence_status: valid_artifact("sha256:source-fence"),
            no_submit_report: valid_artifact("sha256:no-submit"),
            tiny_canary_proof: valid_artifact("sha256:canary"),
            operator_approval: OperatorApprovalProof {
                artifact: valid_artifact("sha256:operator"),
                decision: OperatorApprovalDecision::Approved,
            },
            legal_geography_proof: valid_artifact("sha256:legal"),
        }
    }

    #[test]
    fn complete_exact_head_gate_is_canary_ready_for_scope() {
        let evaluation = evaluate_live_enablement_gate(&valid_packet(), AS_OF_UNIX_SECONDS);

        assert_eq!(evaluation.state, LiveEnablementGateState::TinyCanaryReady);
        assert!(evaluation.rejections.is_empty());
    }

    #[test]
    fn missing_exact_head_ci_source_fence_or_no_submit_rejects() {
        let mut packet = valid_packet();
        packet.exact_head_commit_sha.clear();
        packet.ci_status.artifact_hash.clear();
        packet.source_fence_status.artifact_hash.clear();
        packet.no_submit_report.artifact_hash.clear();

        let evaluation = evaluate_live_enablement_gate(&packet, AS_OF_UNIX_SECONDS);

        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::ExactHeadCommitMissing,
                    field: LiveEnablementGateField::ExactHeadCommitSha,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::CiStatusMissing,
                    field: LiveEnablementGateField::CiStatus,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::SourceFenceStatusMissing,
                    field: LiveEnablementGateField::SourceFenceStatus,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::NoSubmitReportMissing,
                    field: LiveEnablementGateField::NoSubmitReport,
                })
        );
    }

    #[test]
    fn stale_ci_source_fence_no_submit_canary_operator_or_legal_rejects() {
        let mut packet = valid_packet();
        packet.ci_status.expires_at_unix_seconds = AS_OF_UNIX_SECONDS;
        packet.source_fence_status.expires_at_unix_seconds = AS_OF_UNIX_SECONDS;
        packet.no_submit_report.expires_at_unix_seconds = AS_OF_UNIX_SECONDS;
        packet.tiny_canary_proof.expires_at_unix_seconds = AS_OF_UNIX_SECONDS;
        packet.operator_approval.artifact.expires_at_unix_seconds = AS_OF_UNIX_SECONDS;
        packet.legal_geography_proof.expires_at_unix_seconds = AS_OF_UNIX_SECONDS;

        let evaluation = evaluate_live_enablement_gate(&packet, AS_OF_UNIX_SECONDS);

        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::CiStatusStale,
                    field: LiveEnablementGateField::CiStatus,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::SourceFenceStatusStale,
                    field: LiveEnablementGateField::SourceFenceStatus,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::NoSubmitReportStale,
                    field: LiveEnablementGateField::NoSubmitReport,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::TinyCanaryProofStale,
                    field: LiveEnablementGateField::TinyCanaryProof,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::OperatorApprovalStale,
                    field: LiveEnablementGateField::OperatorApproval,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::LegalGeographyProofStale,
                    field: LiveEnablementGateField::LegalGeographyProof,
                })
        );
    }

    #[test]
    fn artifact_package_commit_or_scope_mismatch_rejects() {
        let mut packet = valid_packet();
        packet.no_submit_report.package_hash = "sha256:other-package".to_string();
        packet.source_fence_status.commit_sha = "def456".to_string();
        packet.tiny_canary_proof.market_family_hash = "sha256:other-market-family".to_string();
        packet.legal_geography_proof.config_checksum = "sha256:other-config".to_string();

        let evaluation = evaluate_live_enablement_gate(&packet, AS_OF_UNIX_SECONDS);

        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::ArtifactPackageMismatch,
                    field: LiveEnablementGateField::NoSubmitReport,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::ArtifactCommitMismatch,
                    field: LiveEnablementGateField::SourceFenceStatus,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::ArtifactScopeMismatch,
                    field: LiveEnablementGateField::TinyCanaryProof,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::ArtifactScopeMismatch,
                    field: LiveEnablementGateField::LegalGeographyProof,
                })
        );
    }

    #[test]
    fn promotion_package_and_gate_scope_must_be_exact() {
        let mut packet = valid_packet();
        packet.gate_id.clear();
        packet.promotion_package_hash.clear();
        packet.promotion_package_state = PromotionPackageState::Rejected;
        packet.scope.venue_account_product_hash.clear();
        packet.scope.market_family_hash.clear();
        packet.scope.config_checksum.clear();

        let evaluation = evaluate_live_enablement_gate(&packet, AS_OF_UNIX_SECONDS);

        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::GateIdMissing,
                    field: LiveEnablementGateField::GateId,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::PromotionPackageMissing,
                    field: LiveEnablementGateField::PromotionPackageHash,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::PromotionPackageNotDisabled,
                    field: LiveEnablementGateField::PromotionPackageState,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::VenueAccountProductMissing,
                    field: LiveEnablementGateField::VenueAccountProductHash,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::MarketFamilyMissing,
                    field: LiveEnablementGateField::MarketFamilyHash,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::ConfigChecksumMissing,
                    field: LiveEnablementGateField::ConfigChecksum,
                })
        );
    }

    #[test]
    fn operator_approval_and_legal_geography_are_hard_gates() {
        let mut packet = valid_packet();
        packet.operator_approval.artifact.artifact_hash.clear();
        packet.operator_approval.decision = OperatorApprovalDecision::Pending;
        packet.legal_geography_proof.artifact_hash.clear();

        let evaluation = evaluate_live_enablement_gate(&packet, AS_OF_UNIX_SECONDS);

        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::OperatorApprovalMissing,
                    field: LiveEnablementGateField::OperatorApproval,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::OperatorApprovalNotApproved,
                    field: LiveEnablementGateField::OperatorApproval,
                })
        );
        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::LegalGeographyProofMissing,
                    field: LiveEnablementGateField::LegalGeographyProof,
                })
        );
    }

    #[test]
    fn missing_tiny_canary_proof_rejects_canary_ready() {
        let mut packet = valid_packet();
        packet.tiny_canary_proof.artifact_hash.clear();

        let evaluation = evaluate_live_enablement_gate(&packet, AS_OF_UNIX_SECONDS);

        assert!(
            evaluation
                .rejections
                .contains(&LiveEnablementGateRejection {
                    reason: LiveEnablementGateRejectionReason::TinyCanaryProofMissing,
                    field: LiveEnablementGateField::TinyCanaryProof,
                })
        );
    }

    #[test]
    fn rejection_reason_serializes_to_snake_case() {
        let rejection = LiveEnablementGateRejection {
            reason: LiveEnablementGateRejectionReason::ArtifactScopeMismatch,
            field: LiveEnablementGateField::TinyCanaryProof,
        };

        let encoded = serde_json::to_value(rejection).expect("live gate rejection serializes");

        assert_eq!(encoded["reason"], "artifact_scope_mismatch");
        assert_eq!(encoded["field"], "tiny_canary_proof");
    }
}
