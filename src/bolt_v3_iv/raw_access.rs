use serde::{Deserialize, Serialize};

use super::{
    ingest::IvRawPayload,
    provenance::{IvPolicyDecision, IvProvenance, validate_iv_provenance},
    store::IvStore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvRawAccessRole {
    Audit,
    Replay,
    Test,
    Strategy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvRawAuditRequest {
    pub raw_event_id: String,
    pub role: IvRawAccessRole,
    pub audit_handle_id: String,
    pub access_purpose: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvRawAuditAccess {
    pub raw_event_id: String,
    pub profile_id: String,
    pub source_id: String,
    pub payload_kind: String,
    pub payload: IvRawPayload,
    pub provenance: IvProvenance,
    pub access_purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvRawAccessError {
    StrategyRawAccessDenied,
    RawEventNotFound { raw_event_id: String },
    ProvenanceIncomplete,
}

pub fn read_raw_event(
    store: &IvStore,
    request: &IvRawAuditRequest,
) -> Result<IvRawAuditAccess, IvRawAccessError> {
    if request.role == IvRawAccessRole::Strategy {
        return Err(IvRawAccessError::StrategyRawAccessDenied);
    }

    let raw_event = store.raw_event(&request.raw_event_id).ok_or_else(|| {
        IvRawAccessError::RawEventNotFound {
            raw_event_id: request.raw_event_id.clone(),
        }
    })?;
    let mut provenance = raw_event.provenance.clone();
    provenance.policy_decisions.push(IvPolicyDecision::RawAudit);

    validate_iv_provenance(&provenance).map_err(|_| IvRawAccessError::ProvenanceIncomplete)?;

    Ok(IvRawAuditAccess {
        raw_event_id: raw_event.raw_event_id.clone(),
        profile_id: raw_event.profile_id.clone(),
        source_id: raw_event.source_id.clone(),
        payload_kind: raw_event.payload_kind.clone(),
        payload: raw_event.payload.clone(),
        provenance,
        access_purpose: request.access_purpose.clone(),
    })
}
