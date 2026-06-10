use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    audit::{IvAuditHandleId, IvAuditPolicy, IvAuditRetention, IvRawProductKind},
    error::IvRejectReason,
    health::IvSourceHealthState,
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    provenance::validate_iv_provenance,
    raw_access::{IvRawAccessError, IvRawAccessRole, IvRawAuditRequest, read_raw_event},
    store::{IvStore, IvStoreError},
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

fn audit_policy() -> IvAuditPolicy {
    IvAuditPolicy {
        enabled_raw_products: BTreeSet::from([IvRawProductKind::OptionGreeks]),
        authorized_audit_handles: BTreeSet::from([IvAuditHandleId(
            "configured-audit-handle".to_string(),
        )]),
        access_purposes: BTreeSet::from(["configured-replay-purpose".to_string()]),
        eligible_sources: BTreeSet::from(["configured-source".to_string()]),
        audit_retention: IvAuditRetention {
            max_events: Some(2),
            max_age_ns: Some(10_000),
        },
    }
}

fn greeks_event_at(ts: u64) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(ts),
        ts_init_ns: Some(UnixNanos::new(ts.saturating_sub(100))),
        received_ts_ns: UnixNanos::new(ts + 100),
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

fn greeks_event() -> IvIngestEvent {
    greeks_event_at(2_000)
}

#[test]
fn raw_payload_access_is_audit_replay_or_test_only() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let audit = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id.clone(),
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-audit-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
            as_of_ns: UnixNanos::new(2_200),
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
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id,
            role: IvRawAccessRole::Strategy,
            audit_handle_id: "configured-strategy-handle".to_string(),
            access_purpose: "configured-strategy-purpose".to_string(),
            as_of_ns: UnixNanos::new(2_200),
        },
    );

    assert!(matches!(
        strategy_result,
        Err(IvRawAccessError::StrategyRawAccessDenied)
    ));
}

#[test]
fn raw_payload_access_requires_matching_audit_policy() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let denied = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id,
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-denied-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
            as_of_ns: UnixNanos::new(2_200),
        },
    );

    assert!(matches!(denied, Err(IvRawAccessError::AuditPolicyRejected)));
}

#[test]
fn raw_payload_access_enforces_audit_retention_window() {
    let mut store = IvStore::empty();
    let older_raw = store.ingest_event(greeks_event_at(1_000)).unwrap();
    store.ingest_event(greeks_event_at(2_000)).unwrap();
    store.ingest_event(greeks_event_at(3_000)).unwrap();

    let event_count_miss = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: older_raw.raw_event_id.clone(),
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-audit-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
            as_of_ns: UnixNanos::new(3_200),
        },
    );
    assert!(matches!(
        event_count_miss,
        Err(IvRawAccessError::RetentionMiss)
    ));

    let age_miss = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: older_raw.raw_event_id,
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-audit-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
            as_of_ns: UnixNanos::new(11_101),
        },
    );
    assert!(matches!(age_miss, Err(IvRawAccessError::RetentionMiss)));
}

#[test]
fn raw_payload_access_rejects_audit_as_of_before_event_receipt() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let clock_skew = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id,
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-audit-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
            as_of_ns: UnixNanos::new(2_000),
        },
    );

    assert!(matches!(clock_skew, Err(IvRawAccessError::RetentionMiss)));
}

#[test]
fn invalid_iv_payload_preserves_raw_event_without_indexing_products() {
    let mut event = greeks_event();
    let IvRawPayload::OptionGreeks(payload) = &mut event.payload else {
        panic!("fixture must be option greeks");
    };
    payload.basis_values = vec![IvBasisValue {
        basis: IvBasis::Mark,
        iv: f64::NAN,
    }];

    let mut store = IvStore::empty();
    let result = store.ingest_event(event);

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());

    let raw = &store.raw_events()[0];
    let audit = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            raw_event_id: raw.raw_event_id.clone(),
            role: IvRawAccessRole::Audit,
            audit_handle_id: "configured-audit-handle".to_string(),
            access_purpose: "configured-replay-purpose".to_string(),
            as_of_ns: UnixNanos::new(2_200),
        },
    )
    .unwrap();
    assert_eq!(audit.raw_event_id, raw.raw_event_id);
}

#[test]
fn payload_kind_mismatch_preserves_raw_event_without_indexing_products() {
    let mut event = greeks_event();
    event.source_kind = IvSourceKind::CustomImpliedVolatility;

    let mut store = IvStore::empty();
    let result = store.ingest_event(event);

    assert_eq!(result, Err(IvStoreError::PayloadKindMismatch));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());
    assert!(store.iv_evidence().is_empty());
}

#[test]
fn zero_iv_payload_preserves_raw_event_without_indexing_products() {
    let mut event = greeks_event();
    let IvRawPayload::OptionGreeks(payload) = &mut event.payload else {
        panic!("fixture must be option greeks");
    };
    let zero_implied_volatility = 1.0 - 1.0;
    payload.basis_values = vec![IvBasisValue {
        basis: IvBasis::Mark,
        iv: zero_implied_volatility,
    }];

    let mut store = IvStore::empty();
    let result = store.ingest_event(event);

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());
}

#[test]
fn non_finite_greek_payload_preserves_raw_event_without_indexing_products() {
    let mut event = greeks_event();
    let IvRawPayload::OptionGreeks(payload) = &mut event.payload else {
        panic!("fixture must be option greeks");
    };
    payload.greeks.delta = Some(f64::NAN);

    let mut store = IvStore::empty();
    let result = store.ingest_event(event);

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());
}

#[test]
fn non_finite_underlying_price_preserves_raw_event_without_indexing_products() {
    let mut event = greeks_event();
    let IvRawPayload::OptionGreeks(payload) = &mut event.payload else {
        panic!("fixture must be option greeks");
    };
    payload.underlying_price = Some(f64::NAN);

    let mut store = IvStore::empty();
    let result = store.ingest_event(event);

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());
}

#[test]
fn non_finite_open_interest_preserves_raw_event_without_indexing_products() {
    let mut event = greeks_event();
    let IvRawPayload::OptionGreeks(payload) = &mut event.payload else {
        panic!("fixture must be option greeks");
    };
    payload.open_interest = Some(f64::INFINITY);

    let mut store = IvStore::empty();
    let result = store.ingest_event(event);

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());
}

#[test]
fn non_finite_aggregate_greek_preserves_raw_event_without_indexing_products() {
    let mut store = IvStore::empty();
    let result = store.ingest_event(IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::AggregateGreeks,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(2_000),
        ts_init_ns: Some(UnixNanos::new(1_900)),
        received_ts_ns: UnixNanos::new(2_100),
        subscription_generation: 14,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::AggregateGreeks(
            bolt_v2::bolt_v3_iv::ingest::IvAggregateGreeksPayload {
                aggregate_key: "configured-aggregate-key".to_string(),
                underlying_selectors: vec!["configured-underlying-selector".to_string()],
                greeks: IvGreekValues {
                    delta: Some(0.25),
                    gamma: Some(f64::INFINITY),
                    vega: Some(1.5),
                    theta: None,
                    rho: None,
                },
                aggregate_iv: None,
                nt_custom_data_json: None,
            },
        ),
    });

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.aggregate_greeks().is_empty());
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
