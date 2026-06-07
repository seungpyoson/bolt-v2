use serde::{Deserialize, Serialize};

use super::{
    error::IvRejectReason, health::IvSourceHealthState, time::UnixNanos, types::IvSourceKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvPolicyDecision {
    Projection,
    Interpolation,
    Fallback,
    Quorum,
    Helper,
    RawAudit,
    Rejection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvHelperIdentity {
    pub nt_symbol: String,
    pub nt_revision: String,
    pub parameter_signature: String,
    pub helper_policy_id: String,
    pub engine_mapping: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    if provenance.raw_event_id.as_deref().is_none_or(str::is_empty)
        || provenance.payload_kind.as_deref().is_none_or(str::is_empty)
    {
        return Err(IvRejectReason::ProvenanceIncomplete);
    }

    Ok(())
}
