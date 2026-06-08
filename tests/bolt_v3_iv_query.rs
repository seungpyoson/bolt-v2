use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    authz::{IvAuthorizationMode, IvSelectorAuthorization},
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    derive::{
        IvDerivedInputSet, IvDerivedInputSourceKind, IvHelperPolicy, IvNtHelperSymbol,
        IvOptionSide, IvTimedInput,
    },
    health::IvSourceHealthState,
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    policy::IvProjectionPolicy,
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
        allowed_product_kinds: BTreeSet::from([
            IvProductKind::IvPoint,
            IvProductKind::ProjectedScalarIv,
            IvProductKind::DerivedIv,
        ]),
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

fn convention() -> IvConvention {
    IvConvention::Named("configured-convention".to_string())
}

fn bounds() -> IvNumericBounds {
    IvNumericBounds {
        finite_required: true,
        positive_required: true,
        inclusive_min: Some(0.0),
        inclusive_max: Some(2.0),
        exclusive_min: None,
        exclusive_max: None,
        unit: IvBoundUnit::Unitless,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
    }
}

fn helper_policy() -> IvHelperPolicy {
    IvHelperPolicy {
        helper_policy_id: "configured-helper-policy".to_string(),
        nt_helper_symbol: IvNtHelperSymbol::ImplyVolAndGreeks,
        parameter_signature: "s,r,b,is_call,k,t,price".to_string(),
        output_bounds: bounds(),
        max_input_timestamp_skew_ns: 20,
        max_operator_input_age_ns: 100,
    }
}

fn timed(value: f64, ts: u64) -> IvTimedInput<f64> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts),
        source_kind: IvDerivedInputSourceKind::QuerySupplied,
        expires_at_ns: None,
    }
}

fn complete_inputs() -> IvDerivedInputSet {
    let option_price =
        nautilus_model::data::black_scholes_greeks(100.0, 0.01, 0.0, 0.25, true, 100.0, 0.5).price;

    IvDerivedInputSet {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        instrument_id: "configured-option-instrument".to_string(),
        basis: IvBasis::Mark,
        convention: convention(),
        as_of_ns: UnixNanos::new(2_000),
        received_ts_ns: UnixNanos::new(2_005),
        subscription_generation: 1,
        source_health_state: IvSourceHealthState::Active,
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "crates/model/src/data/greeks.rs".to_string(),
        input_event_ids: vec!["configured-input-event".to_string()],
        option_price: Some(timed(option_price, 1_995)),
        underlying_price: Some(timed(100.0, 1_996)),
        strike: Some(timed(100.0, 1_997)),
        option_side: Some(IvTimedInput {
            value: IvOptionSide::Call,
            ts_ns: UnixNanos::new(1_998),
            source_kind: IvDerivedInputSourceKind::QuerySupplied,
            expires_at_ns: None,
        }),
        time_to_expiry_years: Some(timed(0.5, 1_999)),
        rate: Some(IvTimedInput {
            value: 0.01,
            ts_ns: UnixNanos::new(1_994),
            source_kind: IvDerivedInputSourceKind::OperatorConfigured,
            expires_at_ns: Some(UnixNanos::new(2_050)),
        }),
        carry: Some(IvTimedInput {
            value: 0.0,
            ts_ns: UnixNanos::new(1_993),
            source_kind: IvDerivedInputSourceKind::OperatorConfigured,
            expires_at_ns: Some(UnixNanos::new(2_050)),
        }),
    }
}

#[test]
fn profile_wide_strategy_query_returns_strategy_safe_iv_point() {
    let mut store = IvStore::empty();
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
    let mut store = IvStore::empty();
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
    let mut store = IvStore::empty();
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

#[test]
fn projected_scalar_query_uses_configured_projection_policy() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            0.42,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![IvProjectionPolicy {
            policy_id: "configured-projection-policy".to_string(),
            max_projection_input_skew_ns: 10,
        }]);

    let product = handle
        .query(&IvQuery::Product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(match point_query(None, 2_000) {
                    IvQuery::Product(query) => query.selector,
                    IvQuery::RawPayload(_) => panic!("expected product query"),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert_eq!(projected.source_id, "configured-source");
    assert_eq!(
        projected.selector_fingerprint,
        "configured-selector-fingerprint"
    );
    assert_eq!(projected.value, 0.42);
}

#[test]
fn derived_iv_query_uses_engine_owned_nt_helper_inputs() {
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper_policy()])
    .with_derived_inputs(vec![complete_inputs()]);

    let product = handle
        .query(&IvQuery::Product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();

    let IvQueryProduct::DerivedIv(derived) = product else {
        panic!("expected derived IV product");
    };
    assert_eq!(derived.point.source_id, "configured-source");
    assert_eq!(
        derived.helper_identity.nt_symbol,
        "nautilus_model::data::imply_vol_and_greeks"
    );
    assert!(derived.point.iv > 0.0);
}
