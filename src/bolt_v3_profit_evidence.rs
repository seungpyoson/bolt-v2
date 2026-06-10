use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfitEvidenceSession {
    pub session_id: String,
    pub market_proof_hash: String,
    pub provider_capability_hashes: Vec<String>,
    pub quorum_policy_hash: String,
    pub nt_catalog_path_hash: String,
    pub capture_started_at_unix_seconds: u64,
    pub capture_ended_at_unix_seconds: u64,
    pub fidelity_class: EvidenceFidelityClass,
    pub candidate_count: usize,
    pub no_trade_count: usize,
    pub executable_edge_decision_hash: String,
    pub exact_size_vwap_evidence_hash: String,
    pub order_book_depth_evidence_hash: String,
    pub fee_evidence_hash: String,
    pub submit_admission_evidence_hash: String,
    pub fill_evidence_hash: Option<String>,
    pub no_fill_evidence_hash: Option<String>,
    pub cancel_evidence_hash: Option<String>,
    pub markout_evidence_hash: Option<String>,
    pub settlement_evidence_hash: Option<String>,
    pub profit_summary_hash: String,
    pub threshold_policy_hash: String,
    pub positive_edge_observed: bool,
    pub accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceFidelityClass {
    NtL2Replay,
    ShadowControlledConnect,
    ControlledConnect,
    RestSnapshotBacktest,
    Fixture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfitEvidenceEvaluation {
    pub state: ProfitEvidenceState,
    pub rejections: Vec<ProfitEvidenceRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfitEvidenceState {
    PromotionReady,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfitEvidenceRejection {
    pub reason: ProfitEvidenceRejectionReason,
    pub field: ProfitEvidenceField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfitEvidenceRejectionReason {
    MarketProofMissing,
    ProviderCapabilityMissing,
    QuorumPolicyMissing,
    NtCatalogMissing,
    CandidateEvidenceMissing,
    NoTradeEvidenceMissing,
    ExecutableEdgeEvidenceMissing,
    ExactSizeVwapEvidenceMissing,
    OrderBookDepthEvidenceMissing,
    FeeEvidenceMissing,
    SubmitAdmissionEvidenceMissing,
    PositiveEdgeNeedsFillOrNoFillEvidence,
    PositiveEdgeNeedsMarkoutEvidence,
    PositiveEdgeNeedsSettlementEvidence,
    LowerFidelityCannotPromote,
    ProfitSummaryMissing,
    ThresholdPolicyMissing,
    CaptureWindowInvalid,
    SessionNotAccepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfitEvidenceField {
    MarketProofHash,
    ProviderCapabilityHashes,
    QuorumPolicyHash,
    NtCatalogPathHash,
    CandidateCount,
    NoTradeCount,
    ExecutableEdgeDecision,
    ExactSizeVwapEvidence,
    OrderBookDepthEvidence,
    FeeEvidence,
    SubmitAdmissionEvidence,
    FillOutcomeEvidence,
    MarkoutEvidence,
    SettlementEvidence,
    ProfitSummary,
    ThresholdPolicy,
    FidelityClass,
    CaptureWindow,
    AcceptedFlag,
}

pub fn evaluate_profit_evidence_session(
    session: &ProfitEvidenceSession,
) -> ProfitEvidenceEvaluation {
    let mut rejections = Vec::new();

    reject_blank(
        &session.market_proof_hash,
        ProfitEvidenceRejectionReason::MarketProofMissing,
        ProfitEvidenceField::MarketProofHash,
        &mut rejections,
    );
    if session
        .provider_capability_hashes
        .iter()
        .all(|value| is_blank(value))
    {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::ProviderCapabilityMissing,
            field: ProfitEvidenceField::ProviderCapabilityHashes,
        });
    }
    reject_blank(
        &session.quorum_policy_hash,
        ProfitEvidenceRejectionReason::QuorumPolicyMissing,
        ProfitEvidenceField::QuorumPolicyHash,
        &mut rejections,
    );
    reject_blank(
        &session.nt_catalog_path_hash,
        ProfitEvidenceRejectionReason::NtCatalogMissing,
        ProfitEvidenceField::NtCatalogPathHash,
        &mut rejections,
    );
    if session.capture_ended_at_unix_seconds <= session.capture_started_at_unix_seconds {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::CaptureWindowInvalid,
            field: ProfitEvidenceField::CaptureWindow,
        });
    }
    if low_fidelity(session.fidelity_class) {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::LowerFidelityCannotPromote,
            field: ProfitEvidenceField::FidelityClass,
        });
    }
    if !is_positive(session.candidate_count) {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::CandidateEvidenceMissing,
            field: ProfitEvidenceField::CandidateCount,
        });
    }
    if !is_positive(session.no_trade_count) {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::NoTradeEvidenceMissing,
            field: ProfitEvidenceField::NoTradeCount,
        });
    }
    reject_blank(
        &session.executable_edge_decision_hash,
        ProfitEvidenceRejectionReason::ExecutableEdgeEvidenceMissing,
        ProfitEvidenceField::ExecutableEdgeDecision,
        &mut rejections,
    );
    reject_blank(
        &session.exact_size_vwap_evidence_hash,
        ProfitEvidenceRejectionReason::ExactSizeVwapEvidenceMissing,
        ProfitEvidenceField::ExactSizeVwapEvidence,
        &mut rejections,
    );
    reject_blank(
        &session.order_book_depth_evidence_hash,
        ProfitEvidenceRejectionReason::OrderBookDepthEvidenceMissing,
        ProfitEvidenceField::OrderBookDepthEvidence,
        &mut rejections,
    );
    reject_blank(
        &session.fee_evidence_hash,
        ProfitEvidenceRejectionReason::FeeEvidenceMissing,
        ProfitEvidenceField::FeeEvidence,
        &mut rejections,
    );
    reject_blank(
        &session.submit_admission_evidence_hash,
        ProfitEvidenceRejectionReason::SubmitAdmissionEvidenceMissing,
        ProfitEvidenceField::SubmitAdmissionEvidence,
        &mut rejections,
    );
    if session.positive_edge_observed {
        validate_positive_edge_evidence(session, &mut rejections);
    }
    reject_blank(
        &session.profit_summary_hash,
        ProfitEvidenceRejectionReason::ProfitSummaryMissing,
        ProfitEvidenceField::ProfitSummary,
        &mut rejections,
    );
    reject_blank(
        &session.threshold_policy_hash,
        ProfitEvidenceRejectionReason::ThresholdPolicyMissing,
        ProfitEvidenceField::ThresholdPolicy,
        &mut rejections,
    );
    if !session.accepted {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::SessionNotAccepted,
            field: ProfitEvidenceField::AcceptedFlag,
        });
    }

    let state = if rejections.is_empty() {
        ProfitEvidenceState::PromotionReady
    } else {
        ProfitEvidenceState::Rejected
    };

    ProfitEvidenceEvaluation { state, rejections }
}

fn validate_positive_edge_evidence(
    session: &ProfitEvidenceSession,
    rejections: &mut Vec<ProfitEvidenceRejection>,
) {
    if !has_outcome_evidence(session) {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::PositiveEdgeNeedsFillOrNoFillEvidence,
            field: ProfitEvidenceField::FillOutcomeEvidence,
        });
    }
    if !has_optional_hash(&session.markout_evidence_hash) {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::PositiveEdgeNeedsMarkoutEvidence,
            field: ProfitEvidenceField::MarkoutEvidence,
        });
    }
    if !has_optional_hash(&session.settlement_evidence_hash) {
        rejections.push(ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::PositiveEdgeNeedsSettlementEvidence,
            field: ProfitEvidenceField::SettlementEvidence,
        });
    }
}

fn has_outcome_evidence(session: &ProfitEvidenceSession) -> bool {
    has_optional_hash(&session.fill_evidence_hash)
        || has_optional_hash(&session.no_fill_evidence_hash)
        || has_optional_hash(&session.cancel_evidence_hash)
}

fn reject_blank(
    value: &str,
    reason: ProfitEvidenceRejectionReason,
    field: ProfitEvidenceField,
    rejections: &mut Vec<ProfitEvidenceRejection>,
) {
    if is_blank(value) {
        rejections.push(ProfitEvidenceRejection { reason, field });
    }
}

fn has_optional_hash(value: &Option<String>) -> bool {
    value.as_ref().is_some_and(|hash| !is_blank(hash))
}

fn is_positive(value: usize) -> bool {
    value.checked_sub(usize::from(true)).is_some()
}

fn low_fidelity(fidelity_class: EvidenceFidelityClass) -> bool {
    matches!(
        fidelity_class,
        EvidenceFidelityClass::RestSnapshotBacktest | EvidenceFidelityClass::Fixture
    )
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_session() -> ProfitEvidenceSession {
        ProfitEvidenceSession {
            session_id: "session-001".to_string(),
            market_proof_hash: "sha256:market".to_string(),
            provider_capability_hashes: vec!["sha256:provider".to_string()],
            quorum_policy_hash: "sha256:quorum".to_string(),
            nt_catalog_path_hash: "sha256:catalog".to_string(),
            capture_started_at_unix_seconds: 1_780_000_000,
            capture_ended_at_unix_seconds: 1_780_086_400,
            fidelity_class: EvidenceFidelityClass::NtL2Replay,
            candidate_count: 10,
            no_trade_count: 4,
            executable_edge_decision_hash: "sha256:edge".to_string(),
            exact_size_vwap_evidence_hash: "sha256:vwap".to_string(),
            order_book_depth_evidence_hash: "sha256:book".to_string(),
            fee_evidence_hash: "sha256:fee".to_string(),
            submit_admission_evidence_hash: "sha256:submit".to_string(),
            fill_evidence_hash: Some("sha256:fill".to_string()),
            no_fill_evidence_hash: Some("sha256:no-fill".to_string()),
            cancel_evidence_hash: Some("sha256:cancel".to_string()),
            markout_evidence_hash: Some("sha256:markout".to_string()),
            settlement_evidence_hash: Some("sha256:settlement".to_string()),
            profit_summary_hash: "sha256:profit".to_string(),
            threshold_policy_hash: "sha256:threshold".to_string(),
            positive_edge_observed: true,
            accepted: true,
        }
    }

    #[test]
    fn execution_quality_session_with_complete_evidence_is_promotion_ready() {
        let evaluation = evaluate_profit_evidence_session(&valid_session());

        assert_eq!(evaluation.state, ProfitEvidenceState::PromotionReady);
        assert!(evaluation.rejections.is_empty());
    }

    #[test]
    fn positive_edge_without_fill_markout_or_settlement_evidence_is_rejected() {
        let mut session = valid_session();
        session.fill_evidence_hash = None;
        session.no_fill_evidence_hash = None;
        session.cancel_evidence_hash = None;
        session.markout_evidence_hash = None;
        session.settlement_evidence_hash = None;

        let evaluation = evaluate_profit_evidence_session(&session);

        assert_eq!(evaluation.state, ProfitEvidenceState::Rejected);
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::PositiveEdgeNeedsFillOrNoFillEvidence,
            field: ProfitEvidenceField::FillOutcomeEvidence,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::PositiveEdgeNeedsMarkoutEvidence,
            field: ProfitEvidenceField::MarkoutEvidence,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::PositiveEdgeNeedsSettlementEvidence,
            field: ProfitEvidenceField::SettlementEvidence,
        }));
    }

    #[test]
    fn lower_fidelity_backtest_cannot_promote_capital_scale() {
        let mut session = valid_session();
        session.fidelity_class = EvidenceFidelityClass::RestSnapshotBacktest;

        let evaluation = evaluate_profit_evidence_session(&session);

        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::LowerFidelityCannotPromote,
            field: ProfitEvidenceField::FidelityClass,
        }));
    }

    #[test]
    fn missing_executable_edge_vwap_book_fee_or_submit_evidence_rejects() {
        let mut session = valid_session();
        session.executable_edge_decision_hash.clear();
        session.exact_size_vwap_evidence_hash.clear();
        session.order_book_depth_evidence_hash.clear();
        session.fee_evidence_hash.clear();
        session.submit_admission_evidence_hash.clear();

        let evaluation = evaluate_profit_evidence_session(&session);

        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::ExecutableEdgeEvidenceMissing,
            field: ProfitEvidenceField::ExecutableEdgeDecision,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::ExactSizeVwapEvidenceMissing,
            field: ProfitEvidenceField::ExactSizeVwapEvidence,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::OrderBookDepthEvidenceMissing,
            field: ProfitEvidenceField::OrderBookDepthEvidence,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::FeeEvidenceMissing,
            field: ProfitEvidenceField::FeeEvidence,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::SubmitAdmissionEvidenceMissing,
            field: ProfitEvidenceField::SubmitAdmissionEvidence,
        }));
    }

    #[test]
    fn missing_hash_bindings_or_observation_counts_rejects() {
        let mut session = valid_session();
        session.market_proof_hash.clear();
        session.provider_capability_hashes.clear();
        session.quorum_policy_hash.clear();
        session.nt_catalog_path_hash.clear();
        session.candidate_count = 0;
        session.no_trade_count = 0;

        let evaluation = evaluate_profit_evidence_session(&session);

        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::MarketProofMissing,
            field: ProfitEvidenceField::MarketProofHash,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::ProviderCapabilityMissing,
            field: ProfitEvidenceField::ProviderCapabilityHashes,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::CandidateEvidenceMissing,
            field: ProfitEvidenceField::CandidateCount,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::NoTradeEvidenceMissing,
            field: ProfitEvidenceField::NoTradeCount,
        }));
    }

    #[test]
    fn invalid_capture_window_and_unaccepted_session_reject() {
        let mut session = valid_session();
        session.capture_ended_at_unix_seconds = session.capture_started_at_unix_seconds;
        session.accepted = false;

        let evaluation = evaluate_profit_evidence_session(&session);

        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::CaptureWindowInvalid,
            field: ProfitEvidenceField::CaptureWindow,
        }));
        assert!(evaluation.rejections.contains(&ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::SessionNotAccepted,
            field: ProfitEvidenceField::AcceptedFlag,
        }));
    }

    #[test]
    fn rejection_reason_serializes_to_snake_case() {
        let rejection = ProfitEvidenceRejection {
            reason: ProfitEvidenceRejectionReason::LowerFidelityCannotPromote,
            field: ProfitEvidenceField::FidelityClass,
        };

        let encoded = serde_json::to_value(rejection).expect("profit rejection serializes");

        assert_eq!(encoded["reason"], "lower_fidelity_cannot_promote");
        assert_eq!(encoded["field"], "fidelity_class");
    }
}
