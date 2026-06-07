use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    authz::{IvAuthorizationMode, IvSelectorAuthorization},
    health::IvSourceHealthState,
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    query::{
        IvProductQuery, IvQuery, IvQueryError, IvQueryHandle, IvQueryProduct, IvRawPayloadQuery,
    },
    selector::IvSelector,
    store::IvStore,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvProductKind, IvSourceKind},
};

fn greeks_event(source_id: &str, selector_fingerprint: &str, ts: u64, iv: f64) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: source_id.to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: selector_fingerprint.to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(ts),
        ts_init_ns: Some(UnixNanos::new(ts.saturating_sub(1))),
        received_ts_ns: UnixNanos::new(ts + 1),
        subscription_generation: 1,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::OptionGreeks(IvOptionGreeksPayload {
            instrument_id: "configured-option-instrument".to_string(),
            convention: IvConvention::Named("configured-convention".to_string()),
            basis_values: vec![IvBasisValue {
                basis: IvBasis::Mark,
                iv,
            }],
            greeks: IvGreekValues {
                delta: Some(0.5),
                gamma: None,
                vega: None,
                theta: None,
                rho: None,
            },
            underlying_price: Some(101.0),
            open_interest: None,
        }),
    }
}

fn profile_wide_authorization() -> IvSelectorAuthorization {
    IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::ProfileWide,
        strategy_id: "configured-strategy".to_string(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::IvPoint]),
        allowed_selector_fingerprints: BTreeSet::new(),
        allowed_source_ids: BTreeSet::new(),
    }
}

fn selector_scoped_authorization() -> IvSelectorAuthorization {
    IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::SelectorScoped,
        strategy_id: "configured-strategy".to_string(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::IvPoint]),
        allowed_selector_fingerprints: BTreeSet::from(["configured-allowed-selector".to_string()]),
        allowed_source_ids: BTreeSet::from(["configured-allowed-source".to_string()]),
    }
}

fn point_query(source_filter: Option<&str>, ts: u64) -> IvQuery {
    IvQuery::Product(IvProductQuery {
        strategy_id: "configured-strategy".to_string(),
        profile_id: "configured-profile".to_string(),
        product_kind: IvProductKind::IvPoint,
        selector: IvSelector::PointQuery {
            instrument_ids: vec!["configured-option-instrument".to_string()],
            basis: IvBasis::Mark,
            as_of_ns: UnixNanos::new(ts),
            source_filter: source_filter.map(str::to_string),
        },
    })
}

#[test]
fn profile_wide_strategy_query_returns_strategy_safe_iv_point() {
    let mut store = IvStore::default();
    let raw = store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            0.42,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store);

    let product = handle.query(&point_query(None, 2_000)).unwrap();

    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected IV point product");
    };
    assert_eq!(point.profile_id, "configured-profile");
    assert_eq!(point.source_id, "configured-source");
    assert_eq!(point.iv, 0.42);
    assert_eq!(point.provenance.raw_event_id, Some(raw.raw_event_id));
}

#[test]
fn selector_scoped_strategy_query_requires_matching_source_and_selector() {
    let mut store = IvStore::default();
    store
        .ingest_event(greeks_event(
            "configured-allowed-source",
            "configured-allowed-selector",
            2_000,
            0.41,
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-denied-source",
            "configured-denied-selector",
            2_000,
            0.43,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", selector_scoped_authorization(), store);

    assert!(matches!(
        handle.query(&point_query(Some("configured-allowed-source"), 2_000)),
        Ok(IvQueryProduct::IvPoint(_))
    ));
    assert_eq!(
        handle.query(&point_query(Some("configured-denied-source"), 2_000)),
        Err(IvQueryError::StrategyNotAuthorized)
    );
}

#[test]
fn strategy_query_handle_rejects_raw_payload_requests() {
    let mut store = IvStore::default();
    let raw = store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            0.42,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store);

    assert_eq!(
        handle.query(&IvQuery::RawPayload(IvRawPayloadQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            raw_event_id: raw.raw_event_id,
        })),
        Err(IvQueryError::RawPayloadRejected)
    );
}
