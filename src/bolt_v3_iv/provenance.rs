use serde::{Deserialize, Serialize};

use super::{
    error::IvRejectReason, health::IvSourceHealthState, time::UnixNanos, types::IvSourceKind,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvPolicyDecision {
    ProjectionDecision {
        policy_id: String,
        input_product_ids: Vec<String>,
        selector_fingerprints: Vec<String>,
        projection_kind: String,
        basis: String,
        convention: String,
        max_projection_input_skew_ns: u64,
        accepted_input_ids: Vec<String>,
        rejected_input_ids: Vec<String>,
    },
    InterpolationDecision {
        policy_id: String,
        input_point_ids: Vec<String>,
        strike_axis: String,
        tenor_axis: String,
        method: String,
        minimum_points: usize,
        extrapolation: String,
        eligible_sources: Vec<String>,
        accepted_range: Option<String>,
        rejected_range: Option<String>,
    },
    FallbackDecision {
        policy_id: String,
        candidate_order: Vec<String>,
        accepted_candidate: Option<String>,
        rejected_candidates: Vec<IvRejectedCandidate>,
        maximum_timestamp_skew_ns: u64,
        eligible_sources: Vec<String>,
        required_provenance_fields: Vec<String>,
    },
    QuorumDecision {
        policy_id: String,
        participating_sources: Vec<String>,
        rejected_sources: Vec<String>,
        agreement_band: f64,
        tie_break: String,
        quorum_met: bool,
    },
    HelperDecision {
        helper_policy_id: String,
        helper_identity: IvHelperIdentity,
        helper_symbol: String,
        input_set_id: String,
        input_event_ids: Vec<String>,
        output_validated: bool,
        rejection_reason: Option<IvRejectReason>,
    },
    RawAuditDecision {
        audit_handle_id: String,
        raw_event_id: String,
        payload_kind: String,
        access_purpose: String,
        source_eligibility: Vec<String>,
        retention_result: String,
    },
    RejectionDecision {
        reject_reason: IvRejectReason,
        failed_field: Option<String>,
        policy_id: Option<String>,
        source_health_state: IvSourceHealthState,
        subscription_generation: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvRejectedCandidate {
    pub candidate_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvHelperIdentity {
    pub nt_symbol: String,
    pub nt_revision: String,
    pub parameter_signature: String,
    pub helper_policy_id: String,
    pub engine_mapping: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvProvenance {
    pub profile_id: String,
    pub source_id: String,
    pub source_kind: IvSourceKind,
    pub selector_fingerprint: String,
    pub nt_revision: String,
    pub nt_evidence_path: String,
    pub nt_symbol: String,
    pub raw_event_id: Option<String>,
    pub payload_kind: Option<String>,
    pub input_event_ids: Vec<String>,
    pub helper_identity: Option<IvHelperIdentity>,
    pub policy_decisions: Vec<IvPolicyDecision>,
    pub transformation_steps: Vec<String>,
    pub ts_event_ns: UnixNanos,
    pub ts_init_ns: Option<UnixNanos>,
    pub received_ts_ns: UnixNanos,
    pub ingest_sequence: u64,
    pub subscription_generation: u64,
    pub source_health_state: IvSourceHealthState,
    pub reject_reason: Option<IvRejectReason>,
}

impl IvProvenance {
    pub fn from_raw_event(
        seed: IvProvenanceSeed,
        raw_event_id: String,
        payload_kind: String,
    ) -> Self {
        Self {
            profile_id: seed.profile_id,
            source_id: seed.source_id,
            source_kind: seed.source_kind,
            selector_fingerprint: seed.selector_fingerprint,
            nt_revision: seed.nt_revision,
            nt_evidence_path: seed.nt_evidence_path,
            nt_symbol: seed.nt_symbol,
            raw_event_id: Some(raw_event_id),
            payload_kind: Some(payload_kind),
            input_event_ids: Vec::new(),
            helper_identity: None,
            policy_decisions: Vec::new(),
            transformation_steps: Vec::new(),
            ts_event_ns: seed.ts_event_ns,
            ts_init_ns: seed.ts_init_ns,
            received_ts_ns: seed.received_ts_ns,
            ingest_sequence: seed.ingest_sequence,
            subscription_generation: seed.subscription_generation,
            source_health_state: seed.source_health_state,
            reject_reason: None,
        }
    }

    pub fn has_typed_policy_decision(&self) -> bool {
        !self.policy_decisions.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvProvenanceSeed {
    pub profile_id: String,
    pub source_id: String,
    pub source_kind: IvSourceKind,
    pub selector_fingerprint: String,
    pub nt_revision: String,
    pub nt_evidence_path: String,
    pub nt_symbol: String,
    pub ts_event_ns: UnixNanos,
    pub ts_init_ns: Option<UnixNanos>,
    pub received_ts_ns: UnixNanos,
    pub ingest_sequence: u64,
    pub subscription_generation: u64,
    pub source_health_state: IvSourceHealthState,
}

pub fn validate_iv_provenance(provenance: &IvProvenance) -> Result<(), IvRejectReason> {
    let required_text = [
        provenance.profile_id.as_str(),
        provenance.source_id.as_str(),
        provenance.selector_fingerprint.as_str(),
        provenance.nt_revision.as_str(),
        provenance.nt_evidence_path.as_str(),
        provenance.nt_symbol.as_str(),
    ];

    if required_text.iter().any(|value| value.trim().is_empty()) {
        return Err(IvRejectReason::ProvenanceIncomplete);
    }

    match (
        provenance.raw_event_id.as_deref(),
        provenance.payload_kind.as_deref(),
    ) {
        (Some(raw_event_id), Some(payload_kind))
            if !raw_event_id.trim().is_empty() && !payload_kind.trim().is_empty() =>
        {
            Ok(())
        }
        (None, None) => {
            if provenance.input_event_ids.is_empty() || provenance.policy_decisions.is_empty() {
                return Err(IvRejectReason::ProvenanceIncomplete);
            }
            if provenance.helper_identity.is_none() && provenance.transformation_steps.is_empty() {
                return Err(IvRejectReason::ProvenanceIncomplete);
            }
            if provenance.helper_identity.is_some()
                && !provenance
                    .policy_decisions
                    .iter()
                    .any(|decision| matches!(decision, IvPolicyDecision::HelperDecision { .. }))
            {
                return Err(IvRejectReason::ProvenanceIncomplete);
            }
            Ok(())
        }
        _ => Err(IvRejectReason::ProvenanceIncomplete),
    }
}
