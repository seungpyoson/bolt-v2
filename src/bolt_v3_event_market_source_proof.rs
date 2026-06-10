use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventMarketSourceProof {
    pub proof_id: String,
    pub event_family: String,
    pub competition_id: String,
    pub market_family: String,
    pub venue_id: String,
    pub account_scope: String,
    pub product_surface: String,
    pub official_event_source: SourceArtifactProof,
    pub venue_market_terms: SourceArtifactProof,
    pub official_resolution_rule: MarketResolutionRule,
    pub venue_resolution_rule: MarketResolutionRule,
    pub jurisdiction_availability: JurisdictionAvailabilityProof,
    pub provider_capability_hashes: Vec<String>,
    pub config_checksum: String,
    pub commit_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArtifactProof {
    pub url: String,
    pub sha256: String,
    pub retrieved_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketResolutionRule {
    pub result_scope: Option<ResultScope>,
    pub void_rule: Option<EventDisposition>,
    pub postponement_rule: Option<EventDisposition>,
    pub abandonment_rule: Option<EventDisposition>,
    pub settlement_rule: Option<SettlementRule>,
}

impl MarketResolutionRule {
    fn is_complete(&self) -> bool {
        self.result_scope.is_some()
            && self.void_rule.is_some()
            && self.postponement_rule.is_some()
            && self.abandonment_rule.is_some()
            && self.settlement_rule.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultScope {
    RegulationOnly,
    IncludesExtraTime,
    IncludesPenalties,
    Outright,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventDisposition {
    Void,
    Reschedule,
    SettleAsPlayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementRule {
    OfficialResult,
    VenueDeclaredResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JurisdictionAvailabilityProof {
    pub status: JurisdictionAvailabilityStatus,
    pub source: SourceArtifactProof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JurisdictionAvailabilityStatus {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofAdmission {
    pub state: SourceProofAdmissionState,
    pub rejections: Vec<SourceProofRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofAdmissionState {
    CaptureEligible,
    SourceProofRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofRejection {
    pub reason: SourceProofRejectionReason,
    pub field: SourceProofField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofRejectionReason {
    OfficialEventSourceMissing,
    OfficialEventSourceStale,
    VenueTermsMissing,
    VenueTermsStale,
    ResolutionRuleMissing,
    ResolutionRuleConflict,
    JurisdictionUnavailable,
    JurisdictionProofMissing,
    JurisdictionProofStale,
    ProviderCapabilityHashMissing,
    ConfigChecksumMissing,
    CommitShaMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofField {
    OfficialEventSource,
    VenueMarketTerms,
    OfficialResolutionRule,
    VenueResolutionRule,
    JurisdictionAvailability,
    JurisdictionAvailabilitySource,
    ProviderCapabilityHashes,
    ConfigChecksum,
    CommitSha,
}

pub fn validate_event_market_source_proof(
    proof: &EventMarketSourceProof,
    as_of_unix_seconds: u64,
) -> SourceProofAdmission {
    let mut rejections = Vec::new();

    validate_artifact(
        &proof.official_event_source,
        as_of_unix_seconds,
        SourceProofRejectionReason::OfficialEventSourceMissing,
        SourceProofRejectionReason::OfficialEventSourceStale,
        SourceProofField::OfficialEventSource,
        &mut rejections,
    );
    validate_artifact(
        &proof.venue_market_terms,
        as_of_unix_seconds,
        SourceProofRejectionReason::VenueTermsMissing,
        SourceProofRejectionReason::VenueTermsStale,
        SourceProofField::VenueMarketTerms,
        &mut rejections,
    );
    validate_resolution_rule(
        &proof.official_resolution_rule,
        SourceProofField::OfficialResolutionRule,
        &mut rejections,
    );
    validate_resolution_rule(
        &proof.venue_resolution_rule,
        SourceProofField::VenueResolutionRule,
        &mut rejections,
    );
    validate_resolution_rule_consistency(proof, &mut rejections);
    validate_jurisdiction(proof, as_of_unix_seconds, &mut rejections);

    if proof
        .provider_capability_hashes
        .iter()
        .all(|value| is_blank(value))
    {
        rejections.push(SourceProofRejection {
            reason: SourceProofRejectionReason::ProviderCapabilityHashMissing,
            field: SourceProofField::ProviderCapabilityHashes,
        });
    }
    if is_blank(&proof.config_checksum) {
        rejections.push(SourceProofRejection {
            reason: SourceProofRejectionReason::ConfigChecksumMissing,
            field: SourceProofField::ConfigChecksum,
        });
    }
    if is_blank(&proof.commit_sha) {
        rejections.push(SourceProofRejection {
            reason: SourceProofRejectionReason::CommitShaMissing,
            field: SourceProofField::CommitSha,
        });
    }

    let state = if rejections.is_empty() {
        SourceProofAdmissionState::CaptureEligible
    } else {
        SourceProofAdmissionState::SourceProofRejected
    };

    SourceProofAdmission { state, rejections }
}

fn validate_artifact(
    artifact: &SourceArtifactProof,
    as_of_unix_seconds: u64,
    missing_reason: SourceProofRejectionReason,
    stale_reason: SourceProofRejectionReason,
    field: SourceProofField,
    rejections: &mut Vec<SourceProofRejection>,
) {
    if is_blank(&artifact.url) || is_blank(&artifact.sha256) {
        rejections.push(SourceProofRejection {
            reason: missing_reason,
            field,
        });
    }
    if artifact.retrieved_at_unix_seconds >= artifact.expires_at_unix_seconds
        || as_of_unix_seconds >= artifact.expires_at_unix_seconds
    {
        rejections.push(SourceProofRejection {
            reason: stale_reason,
            field,
        });
    }
}

fn validate_resolution_rule(
    rule: &MarketResolutionRule,
    field: SourceProofField,
    rejections: &mut Vec<SourceProofRejection>,
) {
    if !rule.is_complete() {
        rejections.push(SourceProofRejection {
            reason: SourceProofRejectionReason::ResolutionRuleMissing,
            field,
        });
    }
}

fn validate_resolution_rule_consistency(
    proof: &EventMarketSourceProof,
    rejections: &mut Vec<SourceProofRejection>,
) {
    if proof.official_resolution_rule.is_complete()
        && proof.venue_resolution_rule.is_complete()
        && proof.official_resolution_rule != proof.venue_resolution_rule
    {
        rejections.push(SourceProofRejection {
            reason: SourceProofRejectionReason::ResolutionRuleConflict,
            field: SourceProofField::VenueResolutionRule,
        });
    }
}

fn validate_jurisdiction(
    proof: &EventMarketSourceProof,
    as_of_unix_seconds: u64,
    rejections: &mut Vec<SourceProofRejection>,
) {
    if proof.jurisdiction_availability.status != JurisdictionAvailabilityStatus::Available {
        rejections.push(SourceProofRejection {
            reason: SourceProofRejectionReason::JurisdictionUnavailable,
            field: SourceProofField::JurisdictionAvailability,
        });
    }
    validate_artifact(
        &proof.jurisdiction_availability.source,
        as_of_unix_seconds,
        SourceProofRejectionReason::JurisdictionProofMissing,
        SourceProofRejectionReason::JurisdictionProofStale,
        SourceProofField::JurisdictionAvailabilitySource,
        rejections,
    );
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_artifact() -> SourceArtifactProof {
        SourceArtifactProof {
            url: "https://source.example/world-cup.pdf".to_string(),
            sha256: "sha256:source".to_string(),
            retrieved_at_unix_seconds: 1_780_000_000,
            expires_at_unix_seconds: 1_780_086_400,
        }
    }

    fn valid_resolution_rule() -> MarketResolutionRule {
        MarketResolutionRule {
            result_scope: Some(ResultScope::IncludesExtraTime),
            void_rule: Some(EventDisposition::Void),
            postponement_rule: Some(EventDisposition::Reschedule),
            abandonment_rule: Some(EventDisposition::Void),
            settlement_rule: Some(SettlementRule::OfficialResult),
        }
    }

    fn valid_proof() -> EventMarketSourceProof {
        EventMarketSourceProof {
            proof_id: "proof-001".to_string(),
            event_family: "soccer".to_string(),
            competition_id: "world-cup".to_string(),
            market_family: "match-winner".to_string(),
            venue_id: "prediction-venue".to_string(),
            account_scope: "operator-account".to_string(),
            product_surface: "clob".to_string(),
            official_event_source: valid_artifact(),
            venue_market_terms: valid_artifact(),
            official_resolution_rule: valid_resolution_rule(),
            venue_resolution_rule: valid_resolution_rule(),
            jurisdiction_availability: JurisdictionAvailabilityProof {
                status: JurisdictionAvailabilityStatus::Available,
                source: valid_artifact(),
            },
            provider_capability_hashes: vec!["sha256:provider".to_string()],
            config_checksum: "sha256:config".to_string(),
            commit_sha: "abcdef0".to_string(),
        }
    }

    #[test]
    fn complete_source_proof_is_accepted_for_capture() {
        let admission = validate_event_market_source_proof(&valid_proof(), 1_780_000_001);

        assert_eq!(admission.state, SourceProofAdmissionState::CaptureEligible);
        assert!(admission.rejections.is_empty());
    }

    #[test]
    fn missing_official_source_url_or_hash_rejects_before_strategy_evaluation() {
        let mut proof = valid_proof();
        proof.official_event_source.url.clear();
        proof.official_event_source.sha256.clear();

        let admission = validate_event_market_source_proof(&proof, 1_780_000_001);

        assert_eq!(
            admission.state,
            SourceProofAdmissionState::SourceProofRejected
        );
        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::OfficialEventSourceMissing,
            field: SourceProofField::OfficialEventSource,
        }));
    }

    #[test]
    fn stale_official_or_venue_terms_reject_before_capture() {
        let mut proof = valid_proof();
        proof.official_event_source.expires_at_unix_seconds = 1_780_000_001;
        proof.venue_market_terms.expires_at_unix_seconds = 1_780_000_001;

        let admission = validate_event_market_source_proof(&proof, 1_780_000_001);

        assert_eq!(
            admission.state,
            SourceProofAdmissionState::SourceProofRejected
        );
        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::OfficialEventSourceStale,
            field: SourceProofField::OfficialEventSource,
        }));
        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::VenueTermsStale,
            field: SourceProofField::VenueMarketTerms,
        }));
    }

    #[test]
    fn missing_or_conflicting_resolution_rules_reject() {
        let mut missing = valid_proof();
        missing.venue_resolution_rule.settlement_rule = None;

        let missing_admission = validate_event_market_source_proof(&missing, 1_780_000_001);

        assert!(
            missing_admission
                .rejections
                .contains(&SourceProofRejection {
                    reason: SourceProofRejectionReason::ResolutionRuleMissing,
                    field: SourceProofField::VenueResolutionRule,
                })
        );

        let mut conflicting = valid_proof();
        conflicting.venue_resolution_rule.result_scope = Some(ResultScope::RegulationOnly);

        let conflicting_admission = validate_event_market_source_proof(&conflicting, 1_780_000_001);

        assert!(
            conflicting_admission
                .rejections
                .contains(&SourceProofRejection {
                    reason: SourceProofRejectionReason::ResolutionRuleConflict,
                    field: SourceProofField::VenueResolutionRule,
                })
        );
    }

    #[test]
    fn jurisdiction_unavailability_rejects_live_or_capture_eligibility() {
        let mut proof = valid_proof();
        proof.jurisdiction_availability.status = JurisdictionAvailabilityStatus::Unavailable;

        let admission = validate_event_market_source_proof(&proof, 1_780_000_001);

        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::JurisdictionUnavailable,
            field: SourceProofField::JurisdictionAvailability,
        }));
    }

    #[test]
    fn missing_provider_hash_config_checksum_or_commit_rejects() {
        let mut proof = valid_proof();
        proof.provider_capability_hashes.clear();
        proof.config_checksum.clear();
        proof.commit_sha.clear();

        let admission = validate_event_market_source_proof(&proof, 1_780_000_001);

        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::ProviderCapabilityHashMissing,
            field: SourceProofField::ProviderCapabilityHashes,
        }));
        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::ConfigChecksumMissing,
            field: SourceProofField::ConfigChecksum,
        }));
        assert!(admission.rejections.contains(&SourceProofRejection {
            reason: SourceProofRejectionReason::CommitShaMissing,
            field: SourceProofField::CommitSha,
        }));
    }

    #[test]
    fn source_proof_rejection_reason_serializes_to_snake_case() {
        let rejection = SourceProofRejection {
            reason: SourceProofRejectionReason::VenueTermsStale,
            field: SourceProofField::VenueMarketTerms,
        };

        let encoded = serde_json::to_value(rejection).expect("rejection serializes");

        assert_eq!(encoded["reason"], "venue_terms_stale");
        assert_eq!(encoded["field"], "venue_market_terms");
    }
}
