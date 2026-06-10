use serde::{Deserialize, Serialize};

use crate::bolt_v3_event_market_source_proof::SourceArtifactProof;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilityProof {
    pub provider_id: String,
    pub proof_id: String,
    pub provider_terms: SourceArtifactProof,
    pub plan_name: String,
    pub plan_entitlement: SourceArtifactProof,
    pub transport_class: TransportClass,
    pub stream_protocol: String,
    pub update_semantics: UpdateSemantics,
    pub requires_rest_refresh: bool,
    pub supported_leagues: Vec<String>,
    pub supported_markets: Vec<String>,
    pub supported_books: Vec<String>,
    pub historical_tick_support: CapabilityStatus,
    pub order_book_depth_support: CapabilityStatus,
    pub latency_class: LatencyClass,
    pub rate_limit_policy: String,
    pub license_scope: String,
    pub source_classification: SourceClassification,
    pub direct_access_proof: Option<SourceArtifactProof>,
    pub commit_sha: String,
}

impl ProviderCapabilityProof {
    pub fn feed_class(&self) -> ProviderFeedClass {
        match (
            self.transport_class,
            self.update_semantics,
            self.requires_rest_refresh,
        ) {
            (_, UpdateSemantics::SnapshotThenChangedIds, true) => {
                ProviderFeedClass::NotificationPlusRestRefresh
            }
            (TransportClass::Rest, _, _) | (_, UpdateSemantics::RestPolling, _) => {
                ProviderFeedClass::RestPolling
            }
            (_, UpdateSemantics::FullTickStream, false) => ProviderFeedClass::FullTickStream,
            _ => ProviderFeedClass::SnapshotStream,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportClass {
    Rest,
    Sse,
    WebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSemantics {
    FullTickStream,
    SnapshotThenChangedIds,
    RestPolling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFeedClass {
    FullTickStream,
    NotificationPlusRestRefresh,
    RestPolling,
    SnapshotStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    Realtime,
    SubMinute,
    Delayed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceClassification {
    DirectBook,
    AggregatorSourced { aggregator_provider_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilityRequirement {
    pub required_transport_classes: Vec<TransportClass>,
    pub requires_historical_ticks: bool,
    pub requires_order_book_depth: bool,
    pub required_league: Option<String>,
    pub required_market: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilityAdmission {
    pub state: ProviderCapabilityAdmissionState,
    pub rejections: Vec<ProviderCapabilityRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityAdmissionState {
    CapabilityAccepted,
    CapabilityRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilityRejection {
    pub reason: ProviderCapabilityRejectionReason,
    pub field: ProviderCapabilityField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityRejectionReason {
    ProviderPlanMissing,
    ProviderCapabilityStale,
    ProviderCapabilityInsufficient,
    DirectSourceUnproven,
    AggregatorSourceUnlabeled,
    CommitShaMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityField {
    ProviderTerms,
    PlanName,
    PlanEntitlement,
    TransportClass,
    SupportedLeagues,
    SupportedMarkets,
    HistoricalTickSupport,
    OrderBookDepthSupport,
    SourceClassification,
    DirectAccessProof,
    CommitSha,
}

pub fn validate_provider_capability_proof(
    proof: &ProviderCapabilityProof,
    requirement: &ProviderCapabilityRequirement,
    as_of_unix_seconds: u64,
) -> ProviderCapabilityAdmission {
    let mut rejections = Vec::new();

    validate_artifact(
        &proof.provider_terms,
        as_of_unix_seconds,
        ProviderCapabilityField::ProviderTerms,
        &mut rejections,
    );
    if is_blank(&proof.plan_name) || artifact_missing(&proof.plan_entitlement) {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderPlanMissing,
            field: ProviderCapabilityField::PlanEntitlement,
        });
    }
    if artifact_stale(&proof.plan_entitlement, as_of_unix_seconds) {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityStale,
            field: ProviderCapabilityField::PlanEntitlement,
        });
    }
    validate_source_classification(proof, as_of_unix_seconds, &mut rejections);
    validate_role_requirement(proof, requirement, &mut rejections);
    if is_blank(&proof.commit_sha) {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::CommitShaMissing,
            field: ProviderCapabilityField::CommitSha,
        });
    }

    let state = if rejections.is_empty() {
        ProviderCapabilityAdmissionState::CapabilityAccepted
    } else {
        ProviderCapabilityAdmissionState::CapabilityRejected
    };

    ProviderCapabilityAdmission { state, rejections }
}

fn validate_artifact(
    artifact: &SourceArtifactProof,
    as_of_unix_seconds: u64,
    field: ProviderCapabilityField,
    rejections: &mut Vec<ProviderCapabilityRejection>,
) {
    if artifact_missing(artifact) || artifact_stale(artifact, as_of_unix_seconds) {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityStale,
            field,
        });
    }
}

fn validate_source_classification(
    proof: &ProviderCapabilityProof,
    as_of_unix_seconds: u64,
    rejections: &mut Vec<ProviderCapabilityRejection>,
) {
    match &proof.source_classification {
        SourceClassification::DirectBook => {
            let direct_access_is_current =
                proof.direct_access_proof.as_ref().is_some_and(|artifact| {
                    !artifact_missing(artifact) && !artifact_stale(artifact, as_of_unix_seconds)
                });
            if !direct_access_is_current {
                rejections.push(ProviderCapabilityRejection {
                    reason: ProviderCapabilityRejectionReason::DirectSourceUnproven,
                    field: ProviderCapabilityField::DirectAccessProof,
                });
            }
        }
        SourceClassification::AggregatorSourced {
            aggregator_provider_id,
        } => {
            if is_blank(aggregator_provider_id) {
                rejections.push(ProviderCapabilityRejection {
                    reason: ProviderCapabilityRejectionReason::AggregatorSourceUnlabeled,
                    field: ProviderCapabilityField::SourceClassification,
                });
            }
        }
    }
}

fn validate_role_requirement(
    proof: &ProviderCapabilityProof,
    requirement: &ProviderCapabilityRequirement,
    rejections: &mut Vec<ProviderCapabilityRejection>,
) {
    if !requirement.required_transport_classes.is_empty()
        && !requirement
            .required_transport_classes
            .contains(&proof.transport_class)
    {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::TransportClass,
        });
    }
    if requirement.requires_historical_ticks
        && proof.historical_tick_support != CapabilityStatus::Supported
    {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::HistoricalTickSupport,
        });
    }
    if requirement.requires_order_book_depth
        && proof.order_book_depth_support != CapabilityStatus::Supported
    {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::OrderBookDepthSupport,
        });
    }
    if requirement
        .required_league
        .as_ref()
        .is_some_and(|league| !contains_nonblank(&proof.supported_leagues, league))
    {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::SupportedLeagues,
        });
    }
    if requirement
        .required_market
        .as_ref()
        .is_some_and(|market| !contains_nonblank(&proof.supported_markets, market))
    {
        rejections.push(ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::SupportedMarkets,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceQuorumPolicy {
    pub policy_id: String,
    pub market_family: String,
    pub primary_roles: Vec<String>,
    pub backup_roles: Vec<String>,
    pub veto_roles: Vec<String>,
    pub max_provider_staleness_milliseconds: u64,
    pub min_accepted_primary_count: usize,
    pub min_accepted_backup_count: usize,
    pub veto_on_conflict: bool,
    pub quorum_loss_action: QuorumLossAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuorumLossAction {
    BlockNewOrderIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderReferenceObservation {
    pub role_id: String,
    pub provider_id: String,
    pub observed_at_unix_milliseconds: u64,
    pub status: ProviderReferenceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReferenceStatus {
    Available,
    Disconnected,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceQuorumEvaluation {
    pub state: ReferenceQuorumState,
    pub rejections: Vec<ReferenceQuorumRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceQuorumState {
    Satisfied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceQuorumRejection {
    pub reason: ReferenceQuorumRejectionReason,
    pub role_id: String,
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceQuorumRejectionReason {
    ProviderStale,
    ProviderDisconnected,
    PrimaryQuorumLost,
    BackupQuorumLost,
    VetoConflict,
    PolicyInvalid,
}

pub fn evaluate_reference_quorum(
    policy: &ReferenceQuorumPolicy,
    observations: &[ProviderReferenceObservation],
    as_of_unix_milliseconds: u64,
) -> ReferenceQuorumEvaluation {
    let mut rejections = Vec::new();
    if is_zero_u64(policy.max_provider_staleness_milliseconds)
        || policy.primary_roles.is_empty()
        || is_zero_usize(policy.min_accepted_primary_count)
    {
        rejections.push(ReferenceQuorumRejection {
            reason: ReferenceQuorumRejectionReason::PolicyInvalid,
            role_id: first_role(&policy.primary_roles),
            provider_id: None,
        });
    }

    let mut accepted_primary_count = None;
    let mut accepted_backup_count = None;

    for observation in observations {
        if !is_relevant_observation(policy, observation) {
            continue;
        }
        let stale = as_of_unix_milliseconds
            .saturating_sub(observation.observed_at_unix_milliseconds)
            > policy.max_provider_staleness_milliseconds;
        if stale {
            rejections.push(ReferenceQuorumRejection {
                reason: ReferenceQuorumRejectionReason::ProviderStale,
                role_id: observation.role_id.clone(),
                provider_id: Some(observation.provider_id.clone()),
            });
            continue;
        }

        match observation.status {
            ProviderReferenceStatus::Available => {
                if policy.primary_roles.contains(&observation.role_id) {
                    increment_count(&mut accepted_primary_count);
                }
                if policy.backup_roles.contains(&observation.role_id) {
                    increment_count(&mut accepted_backup_count);
                }
            }
            ProviderReferenceStatus::Disconnected => {
                rejections.push(ReferenceQuorumRejection {
                    reason: ReferenceQuorumRejectionReason::ProviderDisconnected,
                    role_id: observation.role_id.clone(),
                    provider_id: Some(observation.provider_id.clone()),
                });
            }
            ProviderReferenceStatus::Conflict => {
                if policy.veto_on_conflict && policy.veto_roles.contains(&observation.role_id) {
                    rejections.push(ReferenceQuorumRejection {
                        reason: ReferenceQuorumRejectionReason::VetoConflict,
                        role_id: observation.role_id.clone(),
                        provider_id: Some(observation.provider_id.clone()),
                    });
                }
            }
        }
    }

    if count_value(accepted_primary_count) < policy.min_accepted_primary_count {
        rejections.push(ReferenceQuorumRejection {
            reason: ReferenceQuorumRejectionReason::PrimaryQuorumLost,
            role_id: first_role(&policy.primary_roles),
            provider_id: None,
        });
    }
    if count_value(accepted_backup_count) < policy.min_accepted_backup_count {
        rejections.push(ReferenceQuorumRejection {
            reason: ReferenceQuorumRejectionReason::BackupQuorumLost,
            role_id: first_role(&policy.backup_roles),
            provider_id: None,
        });
    }

    let state = if rejections.is_empty() {
        ReferenceQuorumState::Satisfied
    } else {
        ReferenceQuorumState::Rejected
    };

    ReferenceQuorumEvaluation { state, rejections }
}

fn is_relevant_observation(
    policy: &ReferenceQuorumPolicy,
    observation: &ProviderReferenceObservation,
) -> bool {
    policy.primary_roles.contains(&observation.role_id)
        || policy.backup_roles.contains(&observation.role_id)
        || policy.veto_roles.contains(&observation.role_id)
}

fn first_role(roles: &[String]) -> String {
    roles.first().cloned().unwrap_or_else(String::new)
}

fn increment_count(count: &mut Option<usize>) {
    *count = Some(
        count
            .map(|value| value.saturating_add(usize::from(true)))
            .unwrap_or_else(|| usize::from(true)),
    );
}

fn count_value(count: Option<usize>) -> usize {
    count.unwrap_or_else(|| usize::from(false))
}

fn is_zero_u64(value: u64) -> bool {
    value.checked_sub(u64::from(true)).is_none()
}

fn is_zero_usize(value: usize) -> bool {
    value.checked_sub(usize::from(true)).is_none()
}

fn artifact_missing(artifact: &SourceArtifactProof) -> bool {
    is_blank(&artifact.url) || is_blank(&artifact.sha256)
}

fn artifact_stale(artifact: &SourceArtifactProof, as_of_unix_seconds: u64) -> bool {
    artifact.retrieved_at_unix_seconds >= artifact.expires_at_unix_seconds
        || as_of_unix_seconds >= artifact.expires_at_unix_seconds
}

fn contains_nonblank(values: &[String], expected: &str) -> bool {
    !is_blank(expected) && values.iter().any(|value| value == expected)
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_event_market_source_proof::SourceArtifactProof;

    fn valid_artifact() -> SourceArtifactProof {
        SourceArtifactProof {
            url: "https://provider.example/terms".to_string(),
            sha256: "sha256:provider".to_string(),
            retrieved_at_unix_seconds: 1_780_000_000,
            expires_at_unix_seconds: 1_780_086_400,
        }
    }

    fn provider() -> ProviderCapabilityProof {
        ProviderCapabilityProof {
            provider_id: "provider-a".to_string(),
            proof_id: "provider-proof-001".to_string(),
            provider_terms: valid_artifact(),
            plan_name: "realtime".to_string(),
            plan_entitlement: valid_artifact(),
            transport_class: TransportClass::WebSocket,
            stream_protocol: "pusher".to_string(),
            update_semantics: UpdateSemantics::SnapshotThenChangedIds,
            requires_rest_refresh: true,
            supported_leagues: vec!["world-cup".to_string()],
            supported_markets: vec!["match-winner".to_string()],
            supported_books: vec!["pinnacle".to_string()],
            historical_tick_support: CapabilityStatus::Supported,
            order_book_depth_support: CapabilityStatus::Supported,
            latency_class: LatencyClass::Realtime,
            rate_limit_policy: "plan-bound".to_string(),
            license_scope: "internal-trading".to_string(),
            source_classification: SourceClassification::AggregatorSourced {
                aggregator_provider_id: "provider-a".to_string(),
            },
            direct_access_proof: None,
            commit_sha: "abcdef0".to_string(),
        }
    }

    fn role_requirement() -> ProviderCapabilityRequirement {
        ProviderCapabilityRequirement {
            required_transport_classes: vec![TransportClass::WebSocket],
            requires_historical_ticks: true,
            requires_order_book_depth: true,
            required_league: Some("world-cup".to_string()),
            required_market: Some("match-winner".to_string()),
        }
    }

    #[test]
    fn notification_plus_refresh_provider_is_classified_without_vendor_branch() {
        let proof = provider();

        assert_eq!(
            proof.feed_class(),
            ProviderFeedClass::NotificationPlusRestRefresh
        );
    }

    #[test]
    fn direct_source_claim_requires_current_direct_access_proof() {
        let mut proof = provider();
        proof.source_classification = SourceClassification::DirectBook;

        let admission =
            validate_provider_capability_proof(&proof, &role_requirement(), 1_780_000_001);

        assert_eq!(
            admission.state,
            ProviderCapabilityAdmissionState::CapabilityRejected
        );
        assert!(admission.rejections.contains(&ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::DirectSourceUnproven,
            field: ProviderCapabilityField::DirectAccessProof,
        }));
    }

    #[test]
    fn aggregator_source_must_keep_non_empty_aggregator_label() {
        let mut proof = provider();
        proof.source_classification = SourceClassification::AggregatorSourced {
            aggregator_provider_id: " ".to_string(),
        };

        let admission =
            validate_provider_capability_proof(&proof, &role_requirement(), 1_780_000_001);

        assert!(admission.rejections.contains(&ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::AggregatorSourceUnlabeled,
            field: ProviderCapabilityField::SourceClassification,
        }));
    }

    #[test]
    fn missing_or_expired_plan_entitlement_rejects_provider_role() {
        let mut missing = provider();
        missing.plan_entitlement.url.clear();
        missing.plan_entitlement.sha256.clear();

        let missing_admission =
            validate_provider_capability_proof(&missing, &role_requirement(), 1_780_000_001);

        assert!(
            missing_admission
                .rejections
                .contains(&ProviderCapabilityRejection {
                    reason: ProviderCapabilityRejectionReason::ProviderPlanMissing,
                    field: ProviderCapabilityField::PlanEntitlement,
                })
        );

        let mut expired = provider();
        expired.plan_entitlement.expires_at_unix_seconds = 1_780_000_001;

        let expired_admission =
            validate_provider_capability_proof(&expired, &role_requirement(), 1_780_000_001);

        assert!(
            expired_admission
                .rejections
                .contains(&ProviderCapabilityRejection {
                    reason: ProviderCapabilityRejectionReason::ProviderCapabilityStale,
                    field: ProviderCapabilityField::PlanEntitlement,
                })
        );
    }

    #[test]
    fn role_requirements_reject_insufficient_history_or_depth() {
        let mut proof = provider();
        proof.historical_tick_support = CapabilityStatus::Unsupported;
        proof.order_book_depth_support = CapabilityStatus::Unsupported;

        let admission =
            validate_provider_capability_proof(&proof, &role_requirement(), 1_780_000_001);

        assert!(admission.rejections.contains(&ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::HistoricalTickSupport,
        }));
        assert!(admission.rejections.contains(&ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::ProviderCapabilityInsufficient,
            field: ProviderCapabilityField::OrderBookDepthSupport,
        }));
    }

    #[test]
    fn quorum_counts_only_fresh_primary_and_backup_roles() {
        let policy = ReferenceQuorumPolicy {
            policy_id: "policy-001".to_string(),
            market_family: "match-winner".to_string(),
            primary_roles: vec!["primary".to_string()],
            backup_roles: vec!["backup".to_string()],
            veto_roles: vec![],
            max_provider_staleness_milliseconds: 1_000,
            min_accepted_primary_count: 1,
            min_accepted_backup_count: 1,
            veto_on_conflict: true,
            quorum_loss_action: QuorumLossAction::BlockNewOrderIntent,
        };
        let observations = vec![
            ProviderReferenceObservation {
                role_id: "primary".to_string(),
                provider_id: "provider-a".to_string(),
                observed_at_unix_milliseconds: 1_000,
                status: ProviderReferenceStatus::Available,
            },
            ProviderReferenceObservation {
                role_id: "backup".to_string(),
                provider_id: "provider-b".to_string(),
                observed_at_unix_milliseconds: 2_000,
                status: ProviderReferenceStatus::Available,
            },
        ];

        let evaluation = evaluate_reference_quorum(&policy, &observations, 2_001);

        assert_eq!(evaluation.state, ReferenceQuorumState::Rejected);
        assert!(evaluation.rejections.contains(&ReferenceQuorumRejection {
            reason: ReferenceQuorumRejectionReason::ProviderStale,
            role_id: "primary".to_string(),
            provider_id: Some("provider-a".to_string()),
        }));
        assert!(evaluation.rejections.contains(&ReferenceQuorumRejection {
            reason: ReferenceQuorumRejectionReason::PrimaryQuorumLost,
            role_id: "primary".to_string(),
            provider_id: None,
        }));
    }

    #[test]
    fn veto_conflict_blocks_even_when_primary_and_backup_quorum_pass() {
        let policy = ReferenceQuorumPolicy {
            policy_id: "policy-001".to_string(),
            market_family: "match-winner".to_string(),
            primary_roles: vec!["primary".to_string()],
            backup_roles: vec!["backup".to_string()],
            veto_roles: vec!["veto".to_string()],
            max_provider_staleness_milliseconds: 1_000,
            min_accepted_primary_count: 1,
            min_accepted_backup_count: 1,
            veto_on_conflict: true,
            quorum_loss_action: QuorumLossAction::BlockNewOrderIntent,
        };
        let observations = vec![
            ProviderReferenceObservation {
                role_id: "primary".to_string(),
                provider_id: "provider-a".to_string(),
                observed_at_unix_milliseconds: 2_000,
                status: ProviderReferenceStatus::Available,
            },
            ProviderReferenceObservation {
                role_id: "backup".to_string(),
                provider_id: "provider-b".to_string(),
                observed_at_unix_milliseconds: 2_000,
                status: ProviderReferenceStatus::Available,
            },
            ProviderReferenceObservation {
                role_id: "veto".to_string(),
                provider_id: "provider-c".to_string(),
                observed_at_unix_milliseconds: 2_000,
                status: ProviderReferenceStatus::Conflict,
            },
        ];

        let evaluation = evaluate_reference_quorum(&policy, &observations, 2_001);

        assert_eq!(evaluation.state, ReferenceQuorumState::Rejected);
        assert!(evaluation.rejections.contains(&ReferenceQuorumRejection {
            reason: ReferenceQuorumRejectionReason::VetoConflict,
            role_id: "veto".to_string(),
            provider_id: Some("provider-c".to_string()),
        }));
    }

    #[test]
    fn satisfied_quorum_uses_policy_owned_counts() {
        let policy = ReferenceQuorumPolicy {
            policy_id: "policy-001".to_string(),
            market_family: "match-winner".to_string(),
            primary_roles: vec!["primary".to_string()],
            backup_roles: vec!["backup".to_string()],
            veto_roles: vec![],
            max_provider_staleness_milliseconds: 1_000,
            min_accepted_primary_count: 1,
            min_accepted_backup_count: 0,
            veto_on_conflict: true,
            quorum_loss_action: QuorumLossAction::BlockNewOrderIntent,
        };
        let observations = vec![ProviderReferenceObservation {
            role_id: "primary".to_string(),
            provider_id: "provider-a".to_string(),
            observed_at_unix_milliseconds: 2_000,
            status: ProviderReferenceStatus::Available,
        }];

        let evaluation = evaluate_reference_quorum(&policy, &observations, 2_001);

        assert_eq!(evaluation.state, ReferenceQuorumState::Satisfied);
        assert!(evaluation.rejections.is_empty());
    }

    #[test]
    fn provider_rejection_reason_serializes_to_snake_case() {
        let rejection = ProviderCapabilityRejection {
            reason: ProviderCapabilityRejectionReason::DirectSourceUnproven,
            field: ProviderCapabilityField::DirectAccessProof,
        };

        let encoded = serde_json::to_value(rejection).expect("provider rejection serializes");

        assert_eq!(encoded["reason"], "direct_source_unproven");
        assert_eq!(encoded["field"], "direct_access_proof");
    }
}
