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
    pub fn has_typed_policy_decision(&self) -> bool {
        !self.policy_decisions.is_empty()
    }
}
