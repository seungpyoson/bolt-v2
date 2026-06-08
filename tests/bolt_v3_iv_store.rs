use bolt_v2::bolt_v3_iv::{
    error::IvRejectReason,
    health::IvSourceHealthState,
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    provenance::validate_iv_provenance,
    raw_access::{IvRawAccessError, IvRawAccessRole, IvRawAuditRequest, read_raw_event},
    store::IvStore,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

fn greeks_event() -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(2_000),
        ts_init_ns: Some(UnixNanos::new(1_900)),
        received_ts_ns: UnixNanos::new(2_100),
        subscription_generation: 14,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::OptionGreeks(IvOptionGreeksPayload {
            instrument_id: "configured-option-instrument".to_string(),
            convention: IvConvention::Named("configured-convention".to_string()),
            basis_values: vec![IvBasisValue {
                basis: IvBasis::Mark,
                iv: 0.44,
            }],
            greeks: IvGreekValues {
                delta: Some(0.5),
                gamma: Some(0.03),
                vega: Some(0.14),
                theta: None,
                rho: None,
            },
            underlying_price: Some(102.0),
            open_interest: Some(2200.0),
        }),
    }
}

#[test]
fn raw_payload_access_is_audit_replay_or_test_only() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let audit = read_raw_event(
        &store,
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id.clone(),
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-audit-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
        },
    )
    .unwrap();

    assert_eq!(audit.payload, raw.payload);
    assert_eq!(audit.raw_event_id, raw.raw_event_id);
    assert_eq!(audit.access_purpose, "configured-replay-purpose");
    assert!(audit.provenance.has_typed_policy_decision());
    validate_iv_provenance(&audit.provenance).unwrap();

    let strategy_result = read_raw_event(
        &store,
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id,
            role: IvRawAccessRole::Strategy,
            audit_handle_id: "configured-strategy-handle".to_string(),
            access_purpose: "configured-strategy-purpose".to_string(),
        },
    );

    assert!(matches!(
        strategy_result,
        Err(IvRawAccessError::StrategyRawAccessDenied)
    ));
}

#[test]
fn indexed_products_reject_incomplete_provenance() {
    let mut store = IvStore::empty();
    store.ingest_event(greeks_event()).unwrap();

    for provenance in store.all_product_provenance() {
        validate_iv_provenance(provenance).unwrap();
        assert!(provenance.raw_event_id.is_some());
        assert!(provenance.payload_kind.is_some());
    }

    let mut incomplete = store.iv_points()[0].provenance.clone();
    incomplete.raw_event_id = None;

    assert_eq!(
        validate_iv_provenance(&incomplete),
        Err(IvRejectReason::ProvenanceIncomplete)
    );
}
