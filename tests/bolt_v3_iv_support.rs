use bolt_v2::bolt_v3_iv::{
    error::IvRejectReason,
    types::{IvProductKind, IvSourceKind},
};

pub fn all_source_kinds() -> Vec<IvSourceKind> {
    vec![
        IvSourceKind::OptionGreeks,
        IvSourceKind::OptionChain,
        IvSourceKind::AggregateGreeks,
        IvSourceKind::CustomImpliedVolatility,
    ]
}

pub fn all_product_kinds() -> Vec<IvProductKind> {
    vec![
        IvProductKind::IvPoint,
        IvProductKind::IvGreeksPoint,
        IvProductKind::Smile,
        IvProductKind::Surface,
        IvProductKind::AggregateGreeks,
        IvProductKind::CustomIvEvidence,
        IvProductKind::ProjectedScalarIv,
        IvProductKind::DerivedIv,
        IvProductKind::DerivedInputDiagnostics,
        IvProductKind::SourceHealth,
    ]
}

pub fn required_reject_reason_count() -> usize {
    IvRejectReason::required_reasons().len()
}

pub fn profile_id() -> String {
    "profile-id".to_string()
}

pub fn source_id() -> String {
    "source-id".to_string()
}

pub fn strategy_id() -> String {
    "strategy-id".to_string()
}

pub fn selector_fingerprint() -> String {
    "selector-fingerprint".to_string()
}

pub fn convention_name() -> String {
    "convention-name".to_string()
}

pub fn audit_handle_id() -> String {
    "audit-handle-id".to_string()
}

pub fn access_purpose() -> String {
    "replay".to_string()
}

pub fn instrument_id() -> String {
    "instrument-id".to_string()
}

pub fn nt_revision() -> String {
    "nt-revision".to_string()
}

pub fn nt_evidence_path() -> String {
    "nt-evidence-path".to_string()
}

pub fn nt_symbol() -> String {
    "nt-symbol".to_string()
}
