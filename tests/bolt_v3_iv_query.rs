use std::collections::{BTreeMap, BTreeSet};

use bolt_v2::bolt_v3_iv::{
    authz::{IvAuthorizationMode, IvSelectorAuthorization},
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    derive::{
        IvDerivedInputField, IvDerivedInputFieldPolicy, IvDerivedInputPolicy, IvDerivedInputSet,
        IvDerivedInputSourceKind, IvDerivedProfileSourceRef, IvHelperOutput, IvHelperPolicy,
        IvNtHelperSymbol, IvOptionSide, IvTimedInput,
    },
    error::IvRejectReason,
    health::{IvSourceHealth, IvSourceHealthState},
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    policy::{IvProjectionKind, IvProjectionPolicy},
    query::{
        IvProductQuery, IvQuery, IvQueryError, IvQueryHandle, IvQueryProduct, IvRawPayloadQuery,
    },
    selector::IvSelector,
    store::{IvRetentionPolicy, IvStore},
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvProductKind, IvSourceKind},
};

fn test_implied_volatility(percent_points: u32) -> f64 {
    f64::from(percent_points) / f64::from(100_u32)
}

fn greeks_event(
    source_id: &str,
    selector_fingerprint: &str,
    ts: u64,
    implied_volatility: f64,
) -> IvIngestEvent {
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
                iv: implied_volatility,
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

fn greeks_event_with_source_state(
    source_id: &str,
    selector_fingerprint: &str,
    ts: u64,
    implied_volatility: f64,
    source_health_state: IvSourceHealthState,
    subscription_generation: u64,
) -> IvIngestEvent {
    IvIngestEvent {
        source_health_state,
        subscription_generation,
        ..greeks_event(source_id, selector_fingerprint, ts, implied_volatility)
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
            IvProductKind::SourceHealth,
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

fn selector_scoped_source_health_authorization() -> IvSelectorAuthorization {
    IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::SelectorScoped,
        strategy_id: "configured-strategy".to_string(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::SourceHealth]),
        allowed_selector_fingerprints: BTreeSet::new(),
        allowed_source_ids: BTreeSet::from(["configured-active-source".to_string()]),
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
        allowed_outputs: BTreeSet::from([IvHelperOutput::IvAndGreeks]),
        input_policy_ref: "configured-derived-input-policy".to_string(),
        output_bounds: bounds(),
        convention_policy: "configured-convention-policy".to_string(),
        failure_policy: "reject_invalid_helper_output".to_string(),
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

fn operator_timed(value: f64, ts: u64) -> IvTimedInput<f64> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(2_050)),
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

fn profile_resolving_derived_input_policy() -> IvDerivedInputPolicy {
    IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: IvDerivedInputField::required_fields().to_vec(),
        field_sources: vec![
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::UnderlyingPrice,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::ProfileSourceRef]),
                profile_source_ref: Some(IvDerivedProfileSourceRef {
                    source_id: "configured-underlying-source".to_string(),
                    selector_fingerprint: "configured-underlying-selector".to_string(),
                }),
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Rate,
                allowed_source_kinds: BTreeSet::from([
                    IvDerivedInputSourceKind::OperatorConfigured,
                ]),
                profile_source_ref: None,
                operator_number: Some(operator_timed(0.01, 1_994)),
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Carry,
                allowed_source_kinds: BTreeSet::from([
                    IvDerivedInputSourceKind::OperatorConfigured,
                ]),
                profile_source_ref: None,
                operator_number: Some(operator_timed(0.0, 1_993)),
                operator_side: None,
            },
        ],
        freshness_ns: 100,
        max_input_skew_ns: 20,
        bounds: "configured-derived-input-bounds".to_string(),
        convention_policy: "configured-convention-policy".to_string(),
        operator_value_refresh_policy: "reject_expired_operator_values".to_string(),
    }
}

fn source_health(source_id: &str, state: IvSourceHealthState) -> IvSourceHealth {
    IvSourceHealth {
        profile_id: "configured-profile".to_string(),
        source_id: source_id.to_string(),
        subscription_state: state,
        last_event_ts_ns: Some(UnixNanos::new(2_000)),
        last_reject_reason: None,
        reject_counts: BTreeMap::new(),
        stale_state: false,
        retention_state: false,
        subscription_generation: 1,
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
            test_implied_volatility(42),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store);

    let product = handle.query(&point_query(None, 2_000)).unwrap();

    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected IV point product");
    };
    assert_eq!(point.profile_id, "configured-profile");
    assert_eq!(point.source_id, "configured-source");
    assert_eq!(point.iv, test_implied_volatility(42));
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
            test_implied_volatility(41),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-denied-source",
            "configured-denied-selector",
            2_000,
            test_implied_volatility(43),
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
fn strategy_query_rejects_products_from_non_current_source_state() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
            IvSourceHealthState::Stale,
            1,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store);

    assert_eq!(
        handle.query(&point_query(None, 2_000)),
        Err(IvQueryError::ProductNotFound)
    );
}

#[test]
fn strategy_query_rejects_products_when_current_source_health_is_stale() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_source_health(vec![source_health(
            "configured-source",
            IvSourceHealthState::Stale,
        )]);

    assert_eq!(
        handle.query(&point_query(None, 2_000)),
        Err(IvQueryError::ProductNotFound)
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
            test_implied_volatility(42),
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
            test_implied_volatility(42),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![IvProjectionPolicy {
            policy_id: "configured-projection-policy".to_string(),
            projection_kind: IvProjectionKind::Mean,
            basis_selection: "preserve_input_basis".to_string(),
            source_eligibility: vec!["configured-source".to_string()],
            strike_selection: "all_configured_strikes".to_string(),
            tenor_selection: "all_configured_tenors".to_string(),
            evidence_mapping: "preserve_evidence_kind".to_string(),
            minimum_points: 1,
            max_projection_input_skew_ns: 10,
            fallback_policy_ref: None,
            interpolation_policy_ref: None,
            quorum_policy_ref: None,
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
fn source_health_query_applies_configured_state_filter() {
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_source_health(vec![
        source_health("configured-active-source", IvSourceHealthState::Active),
        source_health("configured-stale-source", IvSourceHealthState::Stale),
    ]);

    let product = handle
        .query(&IvQuery::Product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: None,
                state_filter: vec!["stale".to_string()],
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source health product");
    };
    assert_eq!(health.source_id, "configured-stale-source");
    assert_eq!(health.subscription_state, IvSourceHealthState::Stale);
}

#[test]
fn selector_scoped_source_health_query_uses_allowed_source_ids() {
    let handle = IvQueryHandle::new(
        "configured-profile",
        selector_scoped_source_health_authorization(),
        IvStore::empty(),
    )
    .with_source_health(vec![
        source_health("configured-active-source", IvSourceHealthState::Active),
        source_health("configured-denied-source", IvSourceHealthState::Active),
    ]);

    assert!(matches!(
        handle.query(&IvQuery::Product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-active-source".to_string()),
                state_filter: vec!["active".to_string()],
            },
        })),
        Ok(IvQueryProduct::SourceHealth(_))
    ));
    assert_eq!(
        handle.query(&IvQuery::Product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-denied-source".to_string()),
                state_filter: vec!["active".to_string()],
            },
        })),
        Err(IvQueryError::StrategyNotAuthorized)
    );
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

#[test]
fn derived_iv_query_resolves_profile_owned_input_policy_before_helper_call() {
    let mut request_inputs = complete_inputs();
    request_inputs.underlying_price = None;
    request_inputs.rate = None;
    request_inputs.carry = None;

    let mut profile_source_inputs = complete_inputs();
    profile_source_inputs.source_id = "configured-underlying-source".to_string();
    profile_source_inputs.source_kind = IvSourceKind::CustomImpliedVolatility;
    profile_source_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    profile_source_inputs.input_event_ids = vec!["configured-underlying-event".to_string()];
    profile_source_inputs.underlying_price = Some(IvTimedInput {
        value: 100.0,
        ts_ns: UnixNanos::new(1_996),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });

    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![profile_resolving_derived_input_policy()])
    .with_derived_inputs(vec![request_inputs, profile_source_inputs]);

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
    assert!(derived.point.iv > 0.0);
    assert!(
        derived
            .provenance
            .input_event_ids
            .iter()
            .any(|event_id| { event_id == "configured-input-event" })
    );
    assert!(
        derived
            .provenance
            .input_event_ids
            .iter()
            .any(|event_id| { event_id == "configured-underlying-event" })
    );
}

#[test]
fn derived_iv_helper_output_rejection_updates_source_health() {
    let mut helper = helper_policy();
    helper.output_bounds.inclusive_max = Some(0.10);
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper])
    .with_derived_inputs(vec![complete_inputs()]);

    assert_eq!(
        handle.query(&IvQuery::Product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        })),
        Err(IvQueryError::DerivationRejected)
    );

    let health = handle
        .state_handle()
        .source_health_for("configured-profile", "configured-source")
        .unwrap();
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::InvalidIvValue)
    );
    assert_eq!(
        health.reject_counts.get(&IvRejectReason::InvalidIvValue),
        Some(&1)
    );
}

#[test]
fn derived_iv_outputs_are_retained_by_profile_memory_bounds() {
    let mut first_inputs = complete_inputs();
    first_inputs.as_of_ns = UnixNanos::new(2_000);
    let mut second_inputs = complete_inputs();
    second_inputs.as_of_ns = UnixNanos::new(2_010);

    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper_policy()])
    .with_derived_inputs(vec![first_inputs, second_inputs]);

    for as_of_ns in [UnixNanos::new(2_000), UnixNanos::new(2_010)] {
        handle
            .query(&IvQuery::Product(IvProductQuery {
                strategy_id: "configured-strategy".to_string(),
                profile_id: "configured-profile".to_string(),
                product_kind: IvProductKind::DerivedIv,
                selector: IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns,
                },
            }))
            .unwrap();
    }

    handle.enforce_retention(&IvRetentionPolicy {
        max_raw_events: 2,
        max_indexed_points: 2,
        max_smiles: 2,
        max_surfaces: 2,
        max_derived_points: 1,
        max_source_health_events: 2,
    });

    let retained = handle.state_handle().derived_outputs();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].point.ts_event_ns, UnixNanos::new(2_010));
}
