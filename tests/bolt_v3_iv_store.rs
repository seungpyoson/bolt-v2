use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    audit::{IvAuditHandleId, IvAuditPolicy, IvAuditRetention, IvRawProductKind},
    error::IvRejectReason,
    health::IvSourceHealthState,
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    provenance::{IvPolicyDecision, IvRawRetentionResult, validate_iv_provenance},
    raw_access::{IvRawAccessError, IvRawAccessRole, IvRawAuditRequest, read_raw_event},
    store::{IvStore, IvStoreError},
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

fn audit_policy() -> IvAuditPolicy {
    IvAuditPolicy {
        profile_id: "configured-profile".to_string(),
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

fn greeks_event_for_source(source_id: &str) -> IvIngestEvent {
    let mut event = greeks_event();
    event.source_id = source_id.to_string();
    event.selector_fingerprint = format!("{source_id}-selector");
    event
}

fn raw_audit_request(raw_event_id: impl Into<String>) -> IvRawAuditRequest {
    IvRawAuditRequest {
        raw_event_id: raw_event_id.into(),
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        raw_product_kind: IvRawProductKind::OptionGreeks,
        role: IvRawAccessRole::Audit,
        audit_handle_id: "configured-audit-handle".to_string(),
        access_purpose: "configured-replay-purpose".to_string(),
        as_of_ns: UnixNanos::new(2_200),
    }
}

#[test]
fn raw_payload_access_is_audit_replay_or_test_only() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let audit = read_raw_event(
        &store,
        &audit_policy(),
        &raw_audit_request(raw.raw_event_id.clone()),
    )
    .unwrap();

    assert_eq!(audit.payload, raw.payload);
    assert_eq!(audit.raw_event_id, raw.raw_event_id);
    assert_eq!(audit.access_purpose, "configured-replay-purpose");
    assert!(audit.provenance.has_typed_policy_decision());
    validate_iv_provenance(&audit.provenance).unwrap();
    assert!(audit.provenance.policy_decisions.iter().any(|decision| {
        matches!(
            decision,
            IvPolicyDecision::RawAuditDecision {
                retention_result: IvRawRetentionResult::Retained,
                ..
            }
        )
    }));

    let strategy_result = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            role: IvRawAccessRole::Strategy,
            audit_handle_id: "configured-strategy-handle".to_string(),
            access_purpose: "configured-strategy-purpose".to_string(),
            ..raw_audit_request(raw.raw_event_id)
        },
    );

    assert!(matches!(
        strategy_result,
        Err(IvRawAccessError::StrategyRawAccessDenied)
    ));
}

#[test]
fn raw_payload_access_rejects_blank_audit_request_fields() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let mut blank_source = raw_audit_request(raw.raw_event_id.clone());
    blank_source.source_id = " ".to_string();
    assert_eq!(
        read_raw_event(&store, &audit_policy(), &blank_source),
        Err(IvRawAccessError::AuditPolicyRejected)
    );

    let mut blank_handle = raw_audit_request(raw.raw_event_id.clone());
    blank_handle.audit_handle_id = " ".to_string();
    assert_eq!(
        read_raw_event(&store, &audit_policy(), &blank_handle),
        Err(IvRawAccessError::AuditPolicyRejected)
    );

    let mut blank_purpose = raw_audit_request(raw.raw_event_id);
    blank_purpose.access_purpose = " ".to_string();
    assert_eq!(
        read_raw_event(&store, &audit_policy(), &blank_purpose),
        Err(IvRawAccessError::AuditPolicyRejected)
    );
}

#[test]
fn raw_payload_access_rejects_cross_profile_audit_policy() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();
    let mut policy = audit_policy();
    policy.profile_id = "configured-other-profile".to_string();

    let denied = read_raw_event(
        &store,
        &policy,
        &raw_audit_request(raw.raw_event_id.clone()),
    );

    assert!(matches!(denied, Err(IvRawAccessError::AuditPolicyRejected)));
}

#[test]
fn raw_payload_access_requires_matching_audit_policy() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let denied = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            audit_handle_id: "configured-denied-handle".to_string(),
            ..raw_audit_request(raw.raw_event_id)
        },
    );

    assert!(matches!(denied, Err(IvRawAccessError::AuditPolicyRejected)));
}

#[test]
fn raw_payload_access_hides_event_existence_from_unauthorized_audit_identity() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();
    let policy = audit_policy();

    let existing = read_raw_event(
        &store,
        &policy,
        &IvRawAuditRequest {
            audit_handle_id: "configured-unauthorized-audit-handle".to_string(),
            ..raw_audit_request(raw.raw_event_id)
        },
    );
    let missing = read_raw_event(
        &store,
        &policy,
        &IvRawAuditRequest {
            audit_handle_id: "configured-unauthorized-audit-handle".to_string(),
            ..raw_audit_request("configured-profile:configured-source:999")
        },
    );

    assert!(matches!(
        existing,
        Err(IvRawAccessError::AuditPolicyRejected)
    ));
    assert!(matches!(
        missing,
        Err(IvRawAccessError::AuditPolicyRejected)
    ));
}

#[test]
fn raw_payload_access_hides_ineligible_source_existence_from_authorized_audit_identity() {
    let mut store = IvStore::empty();
    let raw = store
        .ingest_event(greeks_event_for_source("configured-ineligible-source"))
        .unwrap();
    let policy = audit_policy();

    let existing = read_raw_event(
        &store,
        &policy,
        &IvRawAuditRequest {
            source_id: "configured-ineligible-source".to_string(),
            ..raw_audit_request(raw.raw_event_id)
        },
    );
    let missing = read_raw_event(
        &store,
        &policy,
        &IvRawAuditRequest {
            source_id: "configured-ineligible-source".to_string(),
            ..raw_audit_request("configured-profile:configured-ineligible-source:999")
        },
    );

    assert!(matches!(
        existing,
        Err(IvRawAccessError::AuditPolicyRejected)
    ));
    assert!(matches!(
        missing,
        Err(IvRawAccessError::AuditPolicyRejected)
    ));
}

#[test]
fn raw_payload_access_hides_cross_scope_raw_event_id_matches() {
    let mut store = IvStore::empty();
    let raw = store
        .ingest_event(greeks_event_for_source("configured-ineligible-source"))
        .unwrap();
    let policy = audit_policy();

    let existing_cross_scope =
        read_raw_event(&store, &policy, &raw_audit_request(raw.raw_event_id));
    let missing = read_raw_event(
        &store,
        &policy,
        &raw_audit_request("configured-profile:configured-ineligible-source:999"),
    );

    assert!(matches!(
        existing_cross_scope,
        Err(IvRawAccessError::RawEventNotFound { .. })
    ));
    assert!(matches!(
        missing,
        Err(IvRawAccessError::RawEventNotFound { .. })
    ));
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
            as_of_ns: UnixNanos::new(3_200),
            ..raw_audit_request(older_raw.raw_event_id.clone())
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
            as_of_ns: UnixNanos::new(11_101),
            ..raw_audit_request(older_raw.raw_event_id)
        },
    );
    assert!(matches!(age_miss, Err(IvRawAccessError::RetentionMiss)));
}

#[test]
fn store_retention_does_not_front_move_retained_indexed_products() {
    let mut store = IvStore::empty();
    store.ingest_event(greeks_event_at(1_000)).unwrap();
    store.ingest_event(greeks_event_at(2_000)).unwrap();
    let retained_before = store
        .iv_points()
        .iter()
        .find(|point| point.ts_event_ns == UnixNanos::new(2_000))
        .map(std::ptr::from_ref)
        .expect("second point should be indexed before retention");

    store.enforce_retention(&bolt_v2::bolt_v3_iv::store::IvRetentionPolicy {
        max_raw_events: 1,
        max_indexed_points: 1,
        max_smiles: 1,
        max_surfaces: 1,
        max_derived_points: 1,
        max_source_health_events: 1,
    });

    let retained_after = store
        .iv_points()
        .iter()
        .find(|point| point.ts_event_ns == UnixNanos::new(2_000))
        .map(std::ptr::from_ref)
        .expect("second point should remain active after retention");
    assert_eq!(store.iv_points().len(), 1);
    assert_eq!(retained_before, retained_after);
}

#[test]
fn raw_payload_access_rejects_audit_as_of_before_event_receipt() {
    let mut store = IvStore::empty();
    let raw = store.ingest_event(greeks_event()).unwrap();

    let clock_skew = read_raw_event(
        &store,
        &audit_policy(),
        &IvRawAuditRequest {
            as_of_ns: UnixNanos::new(2_000),
            ..raw_audit_request(raw.raw_event_id)
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
        &raw_audit_request(raw.raw_event_id.clone()),
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
