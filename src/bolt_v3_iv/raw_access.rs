use serde::{Deserialize, Serialize};

use super::{
    audit::{IvAuditPolicy, IvRawProductKind},
    ingest::IvRawPayload,
    provenance::{IvPolicyDecision, IvProvenance, IvRawRetentionResult, validate_iv_provenance},
    store::IvStore,
    time::UnixNanos,
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
    pub profile_id: String,
    pub source_id: String,
    pub raw_product_kind: IvRawProductKind,
    pub role: IvRawAccessRole,
    pub audit_handle_id: String,
    pub access_purpose: String,
    pub as_of_ns: UnixNanos,
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
    AuditPolicyRejected,
    RetentionMiss,
    RawEventNotFound { raw_event_id: String },
    ProvenanceIncomplete,
}

pub fn read_raw_event(
    store: &IvStore,
    audit_policy: &IvAuditPolicy,
    request: &IvRawAuditRequest,
) -> Result<IvRawAuditAccess, IvRawAccessError> {
    if request.role == IvRawAccessRole::Strategy {
        return Err(IvRawAccessError::StrategyRawAccessDenied);
    }
    if request.source_id.trim().is_empty()
        || request.audit_handle_id.trim().is_empty()
        || request.access_purpose.trim().is_empty()
    {
        return Err(IvRawAccessError::AuditPolicyRejected);
    }
    if audit_policy.profile_id != request.profile_id {
        return Err(IvRawAccessError::AuditPolicyRejected);
    }
    if !audit_policy.authorizes(
        request.raw_product_kind,
        &request.source_id,
        &request.audit_handle_id,
        &request.access_purpose,
    ) {
        return Err(IvRawAccessError::AuditPolicyRejected);
    }

    let raw_event = store.raw_event(&request.raw_event_id).ok_or_else(|| {
        IvRawAccessError::RawEventNotFound {
            raw_event_id: request.raw_event_id.clone(),
        }
    })?;
    let raw_product_kind = IvRawProductKind::from(raw_event.payload.payload_kind());
    if raw_event.profile_id != request.profile_id
        || raw_event.source_id != request.source_id
        || raw_product_kind != request.raw_product_kind
    {
        return Err(IvRawAccessError::RawEventNotFound {
            raw_event_id: request.raw_event_id.clone(),
        });
    }
    if let Some(max_events) = audit_policy.audit_retention.max_events {
        let retained_start = store.raw_events().len().saturating_sub(max_events);
        let Some(position) = store
            .raw_events()
            .iter()
            .position(|event| event.raw_event_id == request.raw_event_id)
        else {
            return Err(IvRawAccessError::RawEventNotFound {
                raw_event_id: request.raw_event_id.clone(),
            });
        };
        if position < retained_start {
            return Err(IvRawAccessError::RetentionMiss);
        }
    }
    if request.as_of_ns.get() < raw_event.received_ts_ns.get() {
        return Err(IvRawAccessError::RetentionMiss);
    }
    if audit_policy
        .audit_retention
        .max_age_ns
        .is_some_and(|max_age_ns| {
            request
                .as_of_ns
                .get()
                .saturating_sub(raw_event.received_ts_ns.get())
                > max_age_ns
        })
    {
        return Err(IvRawAccessError::RetentionMiss);
    }

    let mut provenance = raw_event.provenance.clone();
    provenance
        .policy_decisions
        .push(IvPolicyDecision::RawAuditDecision {
            audit_handle_id: request.audit_handle_id.clone(),
            raw_event_id: request.raw_event_id.clone(),
            payload_kind: raw_event.payload_kind.clone(),
            access_purpose: request.access_purpose.clone(),
            source_eligibility: audit_policy.eligible_sources.iter().cloned().collect(),
            retention_result: IvRawRetentionResult::Retained,
        });

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
