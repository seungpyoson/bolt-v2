use std::collections::{BTreeMap, BTreeSet};

use bolt_v2::bolt_v3_iv::{
    authz::{IvAuthorizationMode, IvSelectorAuthorization},
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    derive::{
        IvDerivedInputBounds, IvDerivedInputField, IvDerivedInputFieldPolicy, IvDerivedInputPolicy,
        IvDerivedInputSet, IvDerivedInputSourceKind, IvDerivedProfileSourceRef,
        IvHelperFailurePolicy, IvHelperOutput, IvHelperPolicy, IvNtHelperSymbol,
        IvOperatorValueRefreshPolicy, IvOptionSide, IvTimedInput,
    },
    error::IvRejectReason,
    health::{IvSourceHealth, IvSourceHealthState},
    ingest::{
        IvAggregateGreeksPayload, IvAggregateIvValue, IvBasisValue, IvCustomIvPayload,
        IvGreekValues, IvIngestEvent, IvOptionChainQuotePayload, IvOptionChainSlicePayload,
        IvOptionChainStrikePayload, IvOptionGreeksPayload, IvRawPayload,
    },
    policy::{
        IvBasisSelection, IvEvidenceMapping, IvExtrapolationPolicy, IvFallbackPolicy,
        IvInterpolationMethod, IvInterpolationPolicy, IvProjectionKind, IvProjectionPolicy,
        IvQuorumPolicy, IvQuorumTieBreak, IvStrikeSelection, IvTenorSelection,
    },
    provenance::IvPolicyDecision,
    query::{
        IvProductQuery, IvQuery, IvQueryError, IvQueryHandle, IvQueryProduct, IvQueryState,
        IvQueryStateHandle, IvRawPayloadQuery,
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

fn custom_iv_event(
    source_id: &str,
    selector_fingerprint: &str,
    ts: u64,
    value: f64,
) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: source_id.to_string(),
        source_kind: IvSourceKind::CustomImpliedVolatility,
        selector_fingerprint: selector_fingerprint.to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredCustomIvEvent".to_string(),
        ts_event_ns: UnixNanos::new(ts),
        ts_init_ns: Some(UnixNanos::new(ts.saturating_sub(1))),
        received_ts_ns: UnixNanos::new(ts + 1),
        subscription_generation: 1,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::CustomImpliedVolatility(IvCustomIvPayload {
            iv_evidence_kind: "configured-custom-evidence-kind".to_string(),
            value,
            nt_custom_data_json: None,
        }),
    }
}

fn aggregate_greeks_event(
    source_id: &str,
    selector_fingerprint: &str,
    ts: u64,
    aggregate_iv: Option<IvAggregateIvValue>,
) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: source_id.to_string(),
        source_kind: IvSourceKind::AggregateGreeks,
        selector_fingerprint: selector_fingerprint.to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredAggregateGreeksEvent".to_string(),
        ts_event_ns: UnixNanos::new(ts),
        ts_init_ns: Some(UnixNanos::new(ts.saturating_sub(1))),
        received_ts_ns: UnixNanos::new(ts + 1),
        subscription_generation: 1,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::AggregateGreeks(IvAggregateGreeksPayload {
            aggregate_key: "configured-aggregate-key".to_string(),
            underlying_selectors: vec!["configured-underlying-selector".to_string()],
            greeks: IvGreekValues {
                delta: Some(1.25),
                gamma: Some(0.15),
                vega: Some(2.5),
                theta: Some(-0.03),
                rho: Some(0.01),
            },
            aggregate_iv,
            nt_custom_data_json: None,
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

fn chain_quote_payload(instrument_id: &str) -> IvOptionChainQuotePayload {
    IvOptionChainQuotePayload {
        instrument_id: instrument_id.to_string(),
        bid_price: Some(4.1),
        ask_price: Some(4.3),
        bid_size: Some(12.0),
        ask_size: Some(13.0),
        ts_event_ns: UnixNanos::new(1_995),
        ts_init_ns: Some(UnixNanos::new(1_996)),
    }
}

fn chain_greeks_payload(instrument_id: &str, mark_iv: f64) -> IvOptionGreeksPayload {
    IvOptionGreeksPayload {
        instrument_id: instrument_id.to_string(),
        convention: IvConvention::Named("configured-convention".to_string()),
        basis_values: vec![IvBasisValue {
            basis: IvBasis::Mark,
            iv: mark_iv,
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
    }
}

fn option_chain_event(ts: u64) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::OptionChain,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(ts),
        ts_init_ns: Some(UnixNanos::new(ts.saturating_sub(1))),
        received_ts_ns: UnixNanos::new(ts + 1),
        subscription_generation: 1,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
            series_id: "configured-series".to_string(),
            surface_selector: "configured-surface-selector".to_string(),
            atm_strike: Some(100.0),
            calls: vec![
                IvOptionChainStrikePayload {
                    strike: 90.0,
                    quote: chain_quote_payload("configured-call-90"),
                    greeks: Some(chain_greeks_payload("configured-call-90", 0.40)),
                },
                IvOptionChainStrikePayload {
                    strike: 100.0,
                    quote: chain_quote_payload("configured-call-100"),
                    greeks: Some(chain_greeks_payload("configured-call-100", 0.44)),
                },
            ],
            puts: Vec::new(),
        }),
    }
}

fn option_chain_event_with_source(
    source_id: &str,
    selector_fingerprint: &str,
    ts: u64,
    first_call_iv: f64,
    atm_call_iv: f64,
) -> IvIngestEvent {
    let mut event = option_chain_event(ts);
    event.source_id = source_id.to_string();
    event.selector_fingerprint = selector_fingerprint.to_string();
    let IvRawPayload::OptionChainSlice(payload) = &mut event.payload else {
        panic!("expected option-chain fixture");
    };
    payload.calls[0].greeks = Some(chain_greeks_payload("configured-call-90", first_call_iv));
    payload.calls[1].greeks = Some(chain_greeks_payload("configured-call-100", atm_call_iv));
    event
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

fn selector_scoped_projection_authorization() -> IvSelectorAuthorization {
    IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::SelectorScoped,
        strategy_id: "configured-strategy".to_string(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::ProjectedScalarIv]),
        allowed_selector_fingerprints: BTreeSet::from([
            "configured-selector-fingerprint".to_string()
        ]),
        allowed_source_ids: BTreeSet::from(["configured-source".to_string()]),
    }
}

fn selector_scoped_smile_authorization(
    source_id: &str,
    selector_fingerprint: &str,
) -> IvSelectorAuthorization {
    IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::SelectorScoped,
        strategy_id: "configured-strategy".to_string(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::Smile]),
        allowed_selector_fingerprints: BTreeSet::from([selector_fingerprint.to_string()]),
        allowed_source_ids: BTreeSet::from([source_id.to_string()]),
    }
}

fn point_query(source_filter: Option<&str>, ts: u64) -> IvQuery {
    IvQuery::product(IvProductQuery {
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

fn retention_policy(
    max_derived_points: usize,
    max_source_health_events: usize,
) -> IvRetentionPolicy {
    IvRetentionPolicy {
        max_raw_events: 2,
        max_indexed_points: 2,
        max_smiles: 2,
        max_surfaces: 2,
        max_derived_points,
        max_source_health_events,
    }
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
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        failure_policy: IvHelperFailurePolicy::RejectInvalidHelperOutput,
        max_input_timestamp_skew_ns: 20,
        max_operator_input_age_ns: 100,
    }
}

fn input_bounds() -> IvDerivedInputBounds {
    IvDerivedInputBounds {
        option_price: Some(input_bound(true, 0.0, 10_000.0, IvBoundUnit::Price)),
        underlying_price: Some(input_bound(true, 0.0, 10_000.0, IvBoundUnit::Price)),
        strike: Some(input_bound(true, 0.0, 10_000.0, IvBoundUnit::Strike)),
        time_to_expiry_years: Some(input_bound(true, 0.0, 100.0, IvBoundUnit::TimeToExpiry)),
        rate: Some(input_bound(false, -1.0, 1.0, IvBoundUnit::Rate)),
        carry: Some(input_bound(false, -1.0, 1.0, IvBoundUnit::Carry)),
    }
}

fn input_bound(
    positive_required: bool,
    inclusive_min: f64,
    inclusive_max: f64,
    unit: IvBoundUnit,
) -> IvNumericBounds {
    IvNumericBounds {
        finite_required: true,
        positive_required,
        inclusive_min: Some(inclusive_min),
        inclusive_max: Some(inclusive_max),
        exclusive_min: None,
        exclusive_max: None,
        unit,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: BTreeSet::new(),
        },
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
                field: IvDerivedInputField::OptionPrice,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Strike,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::OptionSide,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::TimeToExpiryYears,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
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
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    }
}

fn query_supplied_derived_input_policy(input_policy_id: &str) -> IvDerivedInputPolicy {
    let mut policy = profile_resolving_derived_input_policy();
    policy.input_policy_id = input_policy_id.to_string();
    for field_source in &mut policy.field_sources {
        match field_source.field {
            IvDerivedInputField::Rate | IvDerivedInputField::Carry => {}
            _ => {
                field_source.allowed_source_kinds =
                    BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]);
                field_source.profile_source_ref = None;
            }
        }
    }
    policy
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

fn source_health_with_generation(
    source_id: &str,
    state: IvSourceHealthState,
    subscription_generation: u64,
) -> IvSourceHealth {
    IvSourceHealth {
        subscription_generation,
        ..source_health(source_id, state)
    }
}

fn projection_policy_with_refs(
    minimum_points: usize,
    fallback_policy_ref: Option<&str>,
    interpolation_policy_ref: Option<&str>,
    quorum_policy_ref: Option<&str>,
) -> IvProjectionPolicy {
    IvProjectionPolicy {
        policy_id: "configured-projection-policy".to_string(),
        projection_kind: IvProjectionKind::Mean,
        basis_selection: IvBasisSelection::PreserveInputBasis,
        source_eligibility: vec!["configured-source".to_string()],
        strike_selection: IvStrikeSelection::AllConfiguredStrikes,
        tenor_selection: IvTenorSelection::AllConfiguredTenors,
        evidence_mapping: IvEvidenceMapping::PreserveEvidenceKind,
        minimum_points,
        max_projection_input_skew_ns: 10,
        fallback_policy_ref: fallback_policy_ref.map(str::to_string),
        interpolation_policy_ref: interpolation_policy_ref.map(str::to_string),
        quorum_policy_ref: quorum_policy_ref.map(str::to_string),
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
fn selector_scoped_point_query_records_rejection_decision_for_unauthorized_source() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-denied-source",
            "configured-denied-selector",
            2_000,
            test_implied_volatility(43),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", selector_scoped_authorization(), store);

    assert_eq!(
        handle.query(&point_query(Some("configured-denied-source"), 2_000)),
        Err(IvQueryError::StrategyNotAuthorized)
    );

    assert_eq!(
        handle.query_rejections(),
        vec![IvPolicyDecision::RejectionDecision {
            reject_reason: IvRejectReason::SelectorNotAuthorized,
            failed_field: None,
            policy_id: None,
            source_health_state: IvSourceHealthState::Active,
            subscription_generation: 1,
        }]
    );
}

#[test]
fn selector_scoped_source_health_query_records_rejection_decision_for_unauthorized_source() {
    let handle = IvQueryHandle::new(
        "configured-profile",
        selector_scoped_source_health_authorization(),
        IvStore::empty(),
    )
    .with_source_health(vec![source_health(
        "configured-denied-source",
        IvSourceHealthState::Active,
    )]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-denied-source".to_string()),
                state_filter: vec![],
            },
        })),
        Err(IvQueryError::StrategyNotAuthorized)
    );

    assert_eq!(
        handle.query_rejections(),
        vec![IvPolicyDecision::RejectionDecision {
            reject_reason: IvRejectReason::SelectorNotAuthorized,
            failed_field: None,
            policy_id: None,
            source_health_state: IvSourceHealthState::Active,
            subscription_generation: 1,
        }]
    );
}

#[test]
fn selector_scoped_point_query_skips_unauthorized_matching_source() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-denied-source",
            "configured-denied-selector",
            2_000,
            test_implied_volatility(43),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-allowed-source",
            "configured-allowed-selector",
            2_000,
            test_implied_volatility(41),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", selector_scoped_authorization(), store);

    let product = handle.query(&point_query(None, 2_000)).unwrap();

    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected IV point product");
    };
    assert_eq!(point.source_id, "configured-allowed-source");
    assert_eq!(
        point.provenance.selector_fingerprint,
        "configured-allowed-selector"
    );
    assert_eq!(point.iv, test_implied_volatility(41));
}

#[test]
fn two_strategy_instances_query_shared_engine_state_with_different_selectors() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source-a",
            "configured-selector-a",
            2_000,
            test_implied_volatility(41),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-source-b",
            "configured-selector-b",
            2_001,
            test_implied_volatility(43),
        ))
        .unwrap();
    let shared_state = IvQueryStateHandle::new(IvQueryState::new(store));
    let strategy_a = IvQueryHandle::from_state(
        "configured-profile",
        IvSelectorAuthorization {
            authorization_mode: IvAuthorizationMode::SelectorScoped,
            strategy_id: "configured-strategy-a".to_string(),
            allowed_product_kinds: BTreeSet::from([IvProductKind::IvPoint]),
            allowed_selector_fingerprints: BTreeSet::from(["configured-selector-a".to_string()]),
            allowed_source_ids: BTreeSet::from(["configured-source-a".to_string()]),
        },
        shared_state.clone(),
    );
    let strategy_b = IvQueryHandle::from_state(
        "configured-profile",
        IvSelectorAuthorization {
            authorization_mode: IvAuthorizationMode::SelectorScoped,
            strategy_id: "configured-strategy-b".to_string(),
            allowed_product_kinds: BTreeSet::from([IvProductKind::IvPoint]),
            allowed_selector_fingerprints: BTreeSet::from(["configured-selector-b".to_string()]),
            allowed_source_ids: BTreeSet::from(["configured-source-b".to_string()]),
        },
        shared_state,
    );
    let query = |strategy_id: &str, source_id: &str, as_of_ns: u64| {
        IvQuery::product(IvProductQuery {
            strategy_id: strategy_id.to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(as_of_ns),
                source_filter: Some(source_id.to_string()),
            },
        })
    };

    let IvQueryProduct::IvPoint(point_a) = strategy_a
        .query(&query(
            "configured-strategy-a",
            "configured-source-a",
            2_000,
        ))
        .unwrap()
    else {
        panic!("expected strategy A IV point");
    };
    let IvQueryProduct::IvPoint(point_b) = strategy_b
        .query(&query(
            "configured-strategy-b",
            "configured-source-b",
            2_001,
        ))
        .unwrap()
    else {
        panic!("expected strategy B IV point");
    };

    assert_eq!(
        point_a.provenance.selector_fingerprint,
        "configured-selector-a"
    );
    assert_eq!(
        point_b.provenance.selector_fingerprint,
        "configured-selector-b"
    );
    assert_eq!(
        strategy_a.query(&query(
            "configured-strategy-a",
            "configured-source-b",
            2_001
        )),
        Err(IvQueryError::StrategyNotAuthorized)
    );
    assert_eq!(
        strategy_b.query(&query(
            "configured-strategy-b",
            "configured-source-a",
            2_000
        )),
        Err(IvQueryError::StrategyNotAuthorized)
    );
}

#[test]
fn selector_scoped_smile_query_skips_unauthorized_matching_source() {
    let mut store = IvStore::empty();
    store
        .ingest_event(option_chain_event_with_source(
            "configured-denied-source",
            "configured-denied-selector",
            2_010,
            0.40,
            0.44,
        ))
        .unwrap();
    store
        .ingest_event(option_chain_event_with_source(
            "configured-allowed-source",
            "configured-allowed-selector",
            2_010,
            0.41,
            0.45,
        ))
        .unwrap();
    let handle = IvQueryHandle::new(
        "configured-profile",
        selector_scoped_smile_authorization(
            "configured-allowed-source",
            "configured-allowed-selector",
        ),
        store,
    );

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::Smile,
            selector: IvSelector::SmileQuery {
                series_id: "configured-series".to_string(),
                side: None,
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::Smile(smile) = product else {
        panic!("expected smile product");
    };
    assert_eq!(smile.source_id, "configured-allowed-source");
    assert_eq!(
        smile.provenance.selector_fingerprint,
        "configured-allowed-selector"
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
fn strategy_query_skips_non_current_matching_product_for_current_generation() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(41),
            IvSourceHealthState::Stale,
            1,
        ))
        .unwrap();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(43),
            IvSourceHealthState::Active,
            2,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_current_subscription_generations(BTreeMap::from([(
            "configured-source".to_string(),
            2,
        )]));

    let product = handle.query(&point_query(None, 2_000)).unwrap();

    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected current IV point product");
    };
    assert_eq!(point.iv, test_implied_volatility(43));
    assert_eq!(point.provenance.subscription_generation, 2);
}

#[test]
fn strategy_query_keeps_unmapped_source_products_when_generation_overrides_are_partial() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source-a",
            "configured-selector-a",
            2_000,
            test_implied_volatility(41),
            IvSourceHealthState::Active,
            2,
        ))
        .unwrap();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source-b",
            "configured-selector-b",
            2_000,
            test_implied_volatility(43),
            IvSourceHealthState::Active,
            1,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_current_subscription_generations(BTreeMap::from([(
            "configured-source-a".to_string(),
            2,
        )]));

    let product = handle
        .query(&point_query(Some("configured-source-b"), 2_000))
        .unwrap();

    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected unmapped source IV point");
    };
    assert_eq!(point.source_id, "configured-source-b");
    assert_eq!(point.iv, test_implied_volatility(43));
    assert_eq!(point.provenance.subscription_generation, 1);
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
fn strategy_query_records_stale_source_health_rejection_for_current_product() {
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

    let health = handle
        .source_health_for("configured-profile", "configured-source")
        .expect("stale query rejection must be recorded on source health");
    assert_eq!(health.last_reject_reason, Some(IvRejectReason::StaleData));
    assert_eq!(
        health.reject_counts.get(&IvRejectReason::StaleData),
        Some(&1)
    );
}

#[test]
fn strategy_query_records_observable_rejection_decision_for_stale_product() {
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

    assert_eq!(
        handle.query_rejections(),
        vec![IvPolicyDecision::RejectionDecision {
            reject_reason: IvRejectReason::StaleData,
            failed_field: None,
            policy_id: None,
            source_health_state: IvSourceHealthState::Stale,
            subscription_generation: 1,
        }]
    );
}

#[test]
fn strategy_query_returns_retention_miss_for_evicted_indexed_point() {
    let mut store = IvStore::empty();
    for ts in [2_000, 2_010, 2_020] {
        store
            .ingest_event(greeks_event(
                "configured-source",
                "configured-selector-fingerprint",
                ts,
                test_implied_volatility(42),
            ))
            .unwrap();
    }
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_retention_policy(IvRetentionPolicy {
            max_raw_events: 3,
            max_indexed_points: 1,
            max_smiles: 1,
            max_surfaces: 1,
            max_derived_points: 1,
            max_source_health_events: 2,
        });

    assert_eq!(
        handle.query(&point_query(None, 2_010)),
        Err(IvQueryError::RetentionMiss)
    );

    let health = handle
        .source_health_for("configured-profile", "configured-source")
        .expect("retention miss must be recorded on source health");
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::RetentionMiss)
    );
    assert!(health.retention_state);
}

#[test]
fn retention_miss_tombstones_are_bounded_by_profile_retention() {
    let mut store = IvStore::empty();
    for ts in [2_000, 2_010, 2_020, 2_030, 2_040] {
        store
            .ingest_event(greeks_event(
                "configured-source",
                "configured-selector-fingerprint",
                ts,
                test_implied_volatility(42),
            ))
            .unwrap();
    }
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_retention_policy(IvRetentionPolicy {
            max_raw_events: 5,
            max_indexed_points: 1,
            max_smiles: 1,
            max_surfaces: 1,
            max_derived_points: 1,
            max_source_health_events: 2,
        });

    assert_eq!(
        handle.query(&point_query(None, 2_030)),
        Err(IvQueryError::RetentionMiss)
    );
    assert_eq!(
        handle.query(&point_query(None, 2_000)),
        Err(IvQueryError::ProductNotFound)
    );
    assert!(matches!(
        handle.query(&point_query(None, 2_040)),
        Ok(IvQueryProduct::IvPoint(_))
    ));
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
fn projected_scalar_query_uses_configured_fallback_policy_when_primary_projection_rejects() {
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
        .with_projection_policies(vec![projection_policy_with_refs(
            2,
            Some("configured-fallback-policy"),
            None,
            None,
        )])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-option-instrument".to_string()],
            eligible_sources: vec!["configured-source".to_string()],
            maximum_timestamp_skew_ns: 10,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    assert_eq!(projected.value, test_implied_volatility(42));
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| matches!(
                decision,
                IvPolicyDecision::FallbackDecision { policy_id, .. }
                    if policy_id == "configured-fallback-policy"
            ))
    );
}

#[test]
fn projected_scalar_fallback_skew_ignores_same_source_duplicates() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            1_992,
            test_implied_volatility(40),
        ))
        .unwrap();
    let selected_raw = store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy_with_refs(
            2,
            Some("configured-fallback-policy"),
            None,
            None,
        )])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-option-instrument".to_string()],
            eligible_sources: vec!["configured-source".to_string()],
            maximum_timestamp_skew_ns: 5,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    assert_eq!(projected.value, test_implied_volatility(42));
    assert_eq!(
        projected.provenance.raw_event_id,
        Some(selected_raw.raw_event_id)
    );
}

#[test]
fn projected_scalar_fallback_accepts_nearby_input_inside_configured_timestamp_skew() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            1_995,
            test_implied_volatility(42),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy_with_refs(
            2,
            Some("configured-fallback-policy"),
            None,
            None,
        )])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-option-instrument".to_string()],
            eligible_sources: vec!["configured-source".to_string()],
            maximum_timestamp_skew_ns: 10,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    assert_eq!(projected.value, test_implied_volatility(42));
    assert_eq!(projected.as_of_ns, UnixNanos::new(2_000));
}

#[test]
fn projected_scalar_fallback_rejects_distinct_source_inputs_exceeding_timestamp_skew() {
    let mut store = IvStore::empty();
    for (source_id, selector_fingerprint, ts) in [
        (
            "configured-source",
            "configured-selector-fingerprint",
            1_990,
        ),
        (
            "configured-backup-source",
            "configured-backup-selector-fingerprint",
            2_000,
        ),
    ] {
        store
            .ingest_event(greeks_event(
                source_id,
                selector_fingerprint,
                ts,
                test_implied_volatility(42),
            ))
            .unwrap();
    }
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy_with_refs(
            3,
            Some("configured-fallback-policy"),
            None,
            None,
        )])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-option-instrument".to_string()],
            eligible_sources: vec![
                "configured-source".to_string(),
                "configured-backup-source".to_string(),
            ],
            maximum_timestamp_skew_ns: 5,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
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
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_query_applies_configured_interpolation_policy_for_smile_inputs() {
    let mut store = IvStore::empty();
    store.ingest_event(option_chain_event(2_010)).unwrap();
    let mut projection_policy =
        projection_policy_with_refs(1, None, Some("configured-interpolation-policy"), None);
    projection_policy.strike_selection = IvStrikeSelection::AtmStrike;
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_interpolation_policies(vec![IvInterpolationPolicy {
            policy_id: "configured-interpolation-policy".to_string(),
            method: IvInterpolationMethod::Linear,
            strike_axis: "configured-strike-axis".to_string(),
            tenor_axis: "configured-tenor-axis".to_string(),
            minimum_points: 2,
            eligible_sources: vec!["configured-source".to_string()],
            extrapolation: IvExtrapolationPolicy::Reject,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| matches!(
                decision,
                IvPolicyDecision::InterpolationDecision { policy_id, .. }
                    if policy_id == "configured-interpolation-policy"
            ))
    );
}

#[test]
fn projected_scalar_query_rejects_when_single_strike_interpolation_has_no_eligible_source() {
    let mut store = IvStore::empty();
    store.ingest_event(option_chain_event(2_010)).unwrap();
    let mut projection_policy =
        projection_policy_with_refs(1, None, Some("configured-interpolation-policy"), None);
    projection_policy.strike_selection = IvStrikeSelection::AtmStrike;
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_interpolation_policies(vec![IvInterpolationPolicy {
            policy_id: "configured-interpolation-policy".to_string(),
            method: IvInterpolationMethod::Linear,
            strike_axis: "configured-strike-axis".to_string(),
            tenor_axis: "configured-tenor-axis".to_string(),
            minimum_points: 2,
            eligible_sources: vec!["configured-other-source".to_string()],
            extrapolation: IvExtrapolationPolicy::Reject,
        }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_all_configured_strikes_uses_all_smile_points_when_interpolation_policy_exists()
{
    let mut store = IvStore::empty();
    store.ingest_event(option_chain_event(2_010)).unwrap();
    let projection_policy =
        projection_policy_with_refs(1, None, Some("configured-interpolation-policy"), None);
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_interpolation_policies(vec![IvInterpolationPolicy {
            policy_id: "configured-interpolation-policy".to_string(),
            method: IvInterpolationMethod::Linear,
            strike_axis: "configured-strike-axis".to_string(),
            tenor_axis: "configured-tenor-axis".to_string(),
            minimum_points: 2,
            eligible_sources: vec!["configured-source".to_string()],
            extrapolation: IvExtrapolationPolicy::Reject,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert!((projected.value - 0.42).abs() < f64::EPSILON);
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .all(|decision| !matches!(decision, IvPolicyDecision::InterpolationDecision { .. }))
    );
}

#[test]
fn projected_scalar_smile_query_records_option_convention_not_nt_symbol() {
    let mut store = IvStore::empty();
    store.ingest_event(option_chain_event(2_010)).unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy_with_refs(1, None, None, None)]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    let Some(convention) = projected
        .provenance
        .policy_decisions
        .iter()
        .find_map(|decision| match decision {
            IvPolicyDecision::ProjectionDecision { convention, .. } => Some(convention),
            _ => None,
        })
    else {
        panic!("expected projection policy decision");
    };
    assert!((projected.value - 0.42).abs() < f64::EPSILON);
    assert_eq!(convention, "configured-convention");
}

#[test]
fn projected_scalar_query_interpolates_exact_single_point_smile_when_policy_allows_one_point() {
    let mut event = option_chain_event(2_010);
    let IvRawPayload::OptionChainSlice(payload) = &mut event.payload else {
        panic!("expected option-chain fixture");
    };
    payload.calls.remove(0);

    let mut store = IvStore::empty();
    store.ingest_event(event).unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy_with_refs(
            1,
            None,
            Some("configured-interpolation-policy"),
            None,
        )])
        .with_interpolation_policies(vec![IvInterpolationPolicy {
            policy_id: "configured-interpolation-policy".to_string(),
            method: IvInterpolationMethod::Linear,
            strike_axis: "configured-strike-axis".to_string(),
            tenor_axis: "configured-tenor-axis".to_string(),
            minimum_points: 1,
            eligible_sources: vec!["configured-source".to_string()],
            extrapolation: IvExtrapolationPolicy::Reject,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert_eq!(projected.value, test_implied_volatility(44));
}

#[test]
fn projected_scalar_query_uses_fallback_when_single_smile_interpolation_rejects() {
    let mut event = option_chain_event(2_010);
    let IvRawPayload::OptionChainSlice(payload) = &mut event.payload else {
        panic!("expected option-chain fixture");
    };
    payload.calls.remove(0);

    let mut store = IvStore::empty();
    store.ingest_event(event).unwrap();
    let mut projection_policy = projection_policy_with_refs(
        2,
        Some("configured-fallback-policy"),
        Some("configured-interpolation-policy"),
        None,
    );
    projection_policy.strike_selection = IvStrikeSelection::AtmStrike;
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_interpolation_policies(vec![IvInterpolationPolicy {
            policy_id: "configured-interpolation-policy".to_string(),
            method: IvInterpolationMethod::Linear,
            strike_axis: "configured-strike-axis".to_string(),
            tenor_axis: "configured-tenor-axis".to_string(),
            minimum_points: 2,
            eligible_sources: vec!["configured-source".to_string()],
            extrapolation: IvExtrapolationPolicy::Reject,
        }])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-series".to_string()],
            eligible_sources: vec!["configured-source".to_string()],
            maximum_timestamp_skew_ns: 10,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert_eq!(projected.value, test_implied_volatility(44));
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| matches!(
                decision,
                IvPolicyDecision::FallbackDecision { policy_id, .. }
                    if policy_id == "configured-fallback-policy"
            ))
    );
}

#[test]
fn projected_scalar_smile_quorum_interpolates_each_source_before_quorum() {
    let mut store = IvStore::empty();
    store
        .ingest_event(option_chain_event_with_source(
            "configured-source-a",
            "configured-selector-a",
            2_010,
            0.40,
            0.44,
        ))
        .unwrap();
    store
        .ingest_event(option_chain_event_with_source(
            "configured-source-b",
            "configured-selector-b",
            2_010,
            0.41,
            0.45,
        ))
        .unwrap();

    let mut projection_policy = projection_policy_with_refs(
        2,
        None,
        Some("configured-interpolation-policy"),
        Some("configured-quorum-policy"),
    );
    projection_policy.source_eligibility = vec![
        "configured-source-a".to_string(),
        "configured-source-b".to_string(),
    ];
    projection_policy.strike_selection = IvStrikeSelection::AtmStrike;
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_interpolation_policies(vec![IvInterpolationPolicy {
            policy_id: "configured-interpolation-policy".to_string(),
            method: IvInterpolationMethod::Linear,
            strike_axis: "configured-strike-axis".to_string(),
            tenor_axis: "configured-tenor-axis".to_string(),
            minimum_points: 2,
            eligible_sources: vec![
                "configured-source-a".to_string(),
                "configured-source-b".to_string(),
            ],
            extrapolation: IvExtrapolationPolicy::Reject,
        }])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-source-a".to_string(),
                "configured-source-b".to_string(),
            ],
            agreement_band: 0.02,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::SmileQuery {
                    series_id: "configured-series".to_string(),
                    side: None,
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_010),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_010),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert!((projected.value - 0.445).abs() < f64::EPSILON);
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| matches!(
                decision,
                IvPolicyDecision::QuorumDecision {
                    quorum_met: true,
                    ..
                }
            ))
    );
}

#[test]
fn projected_scalar_query_requires_configured_quorum_before_projecting() {
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
        .with_projection_policies(vec![projection_policy_with_refs(
            1,
            None,
            None,
            Some("configured-quorum-policy"),
        )])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-source".to_string(),
                "configured-backup-source".to_string(),
            ],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
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
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_query_rejects_when_configured_quorum_rejects_even_with_fallback() {
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
        .with_projection_policies(vec![projection_policy_with_refs(
            1,
            Some("configured-fallback-policy"),
            None,
            Some("configured-quorum-policy"),
        )])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-option-instrument".to_string()],
            eligible_sources: vec!["configured-source".to_string()],
            maximum_timestamp_skew_ns: 10,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-source".to_string(),
                "configured-backup-source".to_string(),
            ],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
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
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_fallback_rejects_candidate_missing_required_raw_provenance() {
    let projection_policy =
        projection_policy_with_refs(2, Some("configured-fallback-policy"), None, None);
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_projection_policies(vec![projection_policy])
    .with_fallback_policies(vec![IvFallbackPolicy {
        policy_id: "configured-fallback-policy".to_string(),
        candidate_order: vec!["configured-option-instrument".to_string()],
        eligible_sources: vec!["configured-source".to_string()],
        maximum_timestamp_skew_ns: 10,
        required_provenance_fields: vec!["raw_event_id".to_string()],
    }])
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![complete_inputs()]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns: UnixNanos::new(2_000),
                    inputs: None,
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_fallback_provenance_uses_accepted_candidate_source() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-primary-selector",
            2_000,
            test_implied_volatility(41),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-backup-source",
            "configured-backup-selector",
            2_000,
            test_implied_volatility(43),
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy_with_refs(
            3,
            Some("configured-fallback-policy"),
            None,
            None,
        )])
        .with_fallback_policies(vec![IvFallbackPolicy {
            policy_id: "configured-fallback-policy".to_string(),
            candidate_order: vec!["configured-option-instrument".to_string()],
            eligible_sources: vec!["configured-backup-source".to_string()],
            maximum_timestamp_skew_ns: 10,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    assert_eq!(projected.value, test_implied_volatility(43));
    assert_eq!(projected.source_id, "configured-backup-source");
    assert_eq!(projected.selector_fingerprint, "configured-backup-selector");
    assert_eq!(projected.provenance.source_id, "configured-backup-source");
    assert_eq!(
        projected.provenance.selector_fingerprint,
        "configured-backup-selector"
    );
}

#[test]
fn projected_scalar_point_query_aggregates_matching_sources_for_quorum() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(40),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-backup-source",
            "configured-backup-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-source".to_string(),
        "configured-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-source".to_string(),
                "configured-backup-source".to_string(),
            ],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    assert!((projected.value - test_implied_volatility(41)).abs() < f64::EPSILON);
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| {
                matches!(
                    decision,
                    IvPolicyDecision::QuorumDecision {
                        participating_sources,
                        ..
                    } if participating_sources == &vec![
                        "configured-source".to_string(),
                        "configured-backup-source".to_string(),
                    ]
                )
            })
    );
}

#[test]
fn projected_scalar_point_query_deduplicates_tolerance_inputs_by_source() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            1_995,
            test_implied_volatility(30),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(40),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-backup-source",
            "configured-backup-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-source".to_string(),
        "configured-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-source".to_string(),
                "configured-backup-source".to_string(),
            ],
            agreement_band: 0.20,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    assert!((projected.value - test_implied_volatility(41)).abs() < f64::EPSILON);
}

#[test]
fn projected_scalar_point_query_does_not_count_same_source_duplicates_for_quorum() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            1_995,
            test_implied_volatility(40),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(41),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec!["configured-source".to_string()];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec!["configured-source".to_string()],
            agreement_band: 0.20,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
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
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_point_query_skips_non_current_matching_product_for_current_generation() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(41),
            IvSourceHealthState::Stale,
            1,
        ))
        .unwrap();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(43),
            IvSourceHealthState::Active,
            2,
        ))
        .unwrap();
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_current_subscription_generations(BTreeMap::from([(
            "configured-source".to_string(),
            2,
        )]))
        .with_projection_policies(vec![projection_policy_with_refs(1, None, None, None)]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
        panic!("expected projected scalar product");
    };
    assert_eq!(projected.value, test_implied_volatility(43));
    assert_eq!(projected.provenance.subscription_generation, 2);
}

#[test]
fn selector_scoped_projection_rejects_when_any_input_source_is_not_authorized() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(40),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-backup-source",
            "configured-backup-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-source".to_string(),
        "configured-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new(
        "configured-profile",
        selector_scoped_projection_authorization(),
        store,
    )
    .with_projection_policies(vec![projection_policy])
    .with_quorum_policies(vec![IvQuorumPolicy {
        policy_id: "configured-quorum-policy".to_string(),
        minimum_sources: 2,
        eligible_sources: vec![
            "configured-source".to_string(),
            "configured-backup-source".to_string(),
        ],
        agreement_band: 0.05,
        tie_break: IvQuorumTieBreak::Mean,
    }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
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
        })),
        Err(IvQueryError::StrategyNotAuthorized)
    );
}

#[test]
fn projected_scalar_query_rejects_when_any_input_source_is_stale() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(40),
        ))
        .unwrap();
    store
        .ingest_event(greeks_event(
            "configured-backup-source",
            "configured-backup-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-source".to_string(),
        "configured-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_current_subscription_generations(BTreeMap::from([
            ("configured-source".to_string(), 1),
            ("configured-backup-source".to_string(), 2),
        ]))
        .with_projection_policies(vec![projection_policy])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-source".to_string(),
                "configured-backup-source".to_string(),
            ],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
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
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_custom_evidence_query_aggregates_matching_sources_for_quorum() {
    let mut store = IvStore::empty();
    store
        .ingest_event(custom_iv_event(
            "configured-custom-source",
            "configured-custom-selector",
            2_000,
            test_implied_volatility(40),
        ))
        .unwrap();
    store
        .ingest_event(custom_iv_event(
            "configured-custom-backup-source",
            "configured-custom-backup-selector",
            2_000,
            test_implied_volatility(42),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-custom-source".to_string(),
        "configured-custom-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-custom-source".to_string(),
                "configured-custom-backup-source".to_string(),
            ],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::IvEvidenceQuery {
                    iv_evidence_kind: "configured-custom-evidence-kind".to_string(),
                    source_filter: None,
                    as_of_ns: UnixNanos::new(2_000),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert!((projected.value - test_implied_volatility(41)).abs() < f64::EPSILON);
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| {
                matches!(
                    decision,
                    IvPolicyDecision::QuorumDecision {
                        participating_sources,
                        ..
                    } if participating_sources == &vec![
                        "configured-custom-source".to_string(),
                        "configured-custom-backup-source".to_string(),
                    ]
                )
            })
    );
}

#[test]
fn projected_scalar_aggregate_greeks_query_projects_configured_aggregate_iv() {
    let mut store = IvStore::empty();
    store
        .ingest_event(aggregate_greeks_event(
            "configured-aggregate-source",
            "configured-aggregate-selector",
            2_000,
            Some(IvAggregateIvValue {
                basis: IvBasis::Mark,
                value: test_implied_volatility(39),
                convention: convention(),
            }),
        ))
        .unwrap();
    let mut projection_policy = projection_policy_with_refs(1, None, None, None);
    projection_policy.source_eligibility = vec!["configured-aggregate-source".to_string()];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::AggregateGreeksQuery {
                    aggregate_key: "configured-aggregate-key".to_string(),
                    underlying_selectors: vec!["configured-underlying-selector".to_string()],
                    as_of_ns: UnixNanos::new(2_000),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert_eq!(projected.value, test_implied_volatility(39));
    assert_eq!(
        projected.provenance.nt_symbol,
        "ConfiguredAggregateGreeksEvent"
    );
}

#[test]
fn projected_scalar_aggregate_greeks_query_rejects_without_configured_aggregate_iv() {
    let mut store = IvStore::empty();
    store
        .ingest_event(aggregate_greeks_event(
            "configured-aggregate-source",
            "configured-aggregate-selector",
            2_000,
            None,
        ))
        .unwrap();
    let mut projection_policy = projection_policy_with_refs(1, None, None, None);
    projection_policy.source_eligibility = vec!["configured-aggregate-source".to_string()];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::AggregateGreeksQuery {
                    aggregate_key: "configured-aggregate-key".to_string(),
                    underlying_selectors: vec!["configured-underlying-selector".to_string()],
                    as_of_ns: UnixNanos::new(2_000),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        })),
        Err(IvQueryError::ProjectionRejected)
    );
}

#[test]
fn projected_scalar_aggregate_greeks_query_aggregates_matching_sources_for_quorum() {
    let mut store = IvStore::empty();
    store
        .ingest_event(aggregate_greeks_event(
            "configured-aggregate-source",
            "configured-aggregate-selector",
            2_000,
            Some(IvAggregateIvValue {
                basis: IvBasis::Mark,
                value: test_implied_volatility(39),
                convention: convention(),
            }),
        ))
        .unwrap();
    store
        .ingest_event(aggregate_greeks_event(
            "configured-aggregate-backup-source",
            "configured-aggregate-backup-selector",
            2_000,
            Some(IvAggregateIvValue {
                basis: IvBasis::Mark,
                value: test_implied_volatility(41),
                convention: convention(),
            }),
        ))
        .unwrap();
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-aggregate-source".to_string(),
        "configured-aggregate-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_projection_policies(vec![projection_policy])
        .with_quorum_policies(vec![IvQuorumPolicy {
            policy_id: "configured-quorum-policy".to_string(),
            minimum_sources: 2,
            eligible_sources: vec![
                "configured-aggregate-source".to_string(),
                "configured-aggregate-backup-source".to_string(),
            ],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::AggregateGreeksQuery {
                    aggregate_key: "configured-aggregate-key".to_string(),
                    underlying_selectors: vec!["configured-underlying-selector".to_string()],
                    as_of_ns: UnixNanos::new(2_000),
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert!((projected.value - test_implied_volatility(40)).abs() < f64::EPSILON);
}

#[test]
fn projected_scalar_derived_iv_query_does_not_quorum_over_request_supplied_outputs() {
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-source".to_string(),
        "configured-backup-source".to_string(),
    ];
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_retention_policy(retention_policy(4, 2))
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_projection_policies(vec![projection_policy])
    .with_quorum_policies(vec![IvQuorumPolicy {
        policy_id: "configured-quorum-policy".to_string(),
        minimum_sources: 2,
        eligible_sources: vec![
            "configured-source".to_string(),
            "configured-backup-source".to_string(),
        ],
        agreement_band: 0.05,
        tie_break: IvQuorumTieBreak::Mean,
    }]);

    for source_id in ["configured-source", "configured-backup-source"] {
        let mut request_inputs = complete_inputs();
        request_inputs.source_id = source_id.to_string();
        request_inputs.input_event_ids = vec![format!("{source_id}-event")];
        handle
            .query(&IvQuery::product(IvProductQuery {
                strategy_id: "configured-strategy".to_string(),
                profile_id: "configured-profile".to_string(),
                product_kind: IvProductKind::DerivedIv,
                selector: IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns: UnixNanos::new(2_000),
                    inputs: Some(Box::new(request_inputs)),
                },
            }))
            .unwrap();
    }

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns: UnixNanos::new(2_000),
                    inputs: None,
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        })),
        Err(IvQueryError::ProductNotFound)
    );
}

#[test]
fn projected_scalar_derived_iv_query_derives_profile_inputs_for_quorum_without_warm_cache() {
    let mut projection_policy =
        projection_policy_with_refs(2, None, None, Some("configured-quorum-policy"));
    projection_policy.source_eligibility = vec![
        "configured-source".to_string(),
        "configured-backup-source".to_string(),
    ];

    let mut primary_inputs = complete_inputs();
    primary_inputs.source_id = "configured-source".to_string();
    primary_inputs.input_event_ids = vec!["configured-source-event".to_string()];
    let mut backup_inputs = complete_inputs();
    backup_inputs.source_id = "configured-backup-source".to_string();
    backup_inputs.input_event_ids = vec!["configured-backup-source-event".to_string()];

    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_retention_policy(retention_policy(4, 2))
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![primary_inputs, backup_inputs])
    .with_projection_policies(vec![projection_policy])
    .with_quorum_policies(vec![IvQuorumPolicy {
        policy_id: "configured-quorum-policy".to_string(),
        minimum_sources: 2,
        eligible_sources: vec![
            "configured-source".to_string(),
            "configured-backup-source".to_string(),
        ],
        agreement_band: 0.05,
        tie_break: IvQuorumTieBreak::Mean,
    }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns: UnixNanos::new(2_000),
                    inputs: None,
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();

    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product");
    };
    assert!(
        projected
            .provenance
            .policy_decisions
            .iter()
            .any(|decision| {
                matches!(
                    decision,
                    IvPolicyDecision::QuorumDecision {
                        participating_sources,
                        ..
                    } if participating_sources == &vec![
                        "configured-source".to_string(),
                        "configured-backup-source".to_string(),
                    ]
                )
            })
    );

    assert_eq!(handle.derived_outputs().len(), 2);
}

#[test]
fn current_generation_source_health_lookup_ignores_stale_history() {
    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event_with_source_state(
            "configured-source",
            "configured-selector-fingerprint",
            2_000,
            test_implied_volatility(42),
            IvSourceHealthState::Active,
            2,
        ))
        .unwrap();
    let mut current_generations = BTreeMap::new();
    current_generations.insert("configured-source".to_string(), 2);
    let handle = IvQueryHandle::new("configured-profile", profile_wide_authorization(), store)
        .with_current_subscription_generations(current_generations)
        .with_source_health(vec![
            source_health_with_generation("configured-source", IvSourceHealthState::Stale, 1),
            source_health_with_generation("configured-source", IvSourceHealthState::Active, 2),
        ]);

    assert!(matches!(
        handle.query(&point_query(None, 2_000)),
        Ok(IvQueryProduct::IvPoint(_))
    ));
    assert_eq!(
        handle
            .source_health_for("configured-profile", "configured-source")
            .unwrap()
            .subscription_generation,
        2
    );
}

#[test]
fn source_health_retention_keeps_current_generation_entries() {
    let mut current_generations = BTreeMap::new();
    current_generations.insert("configured-source".to_string(), 2);
    let state = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
    let handle = IvQueryHandle::from_state(
        "configured-profile",
        profile_wide_authorization(),
        state.clone(),
    )
    .with_current_subscription_generations(current_generations)
    .with_source_health(vec![
        source_health_with_generation("configured-source", IvSourceHealthState::Active, 2),
        source_health_with_generation("configured-old-source", IvSourceHealthState::Removed, 1),
        source_health_with_generation("configured-extra-source", IvSourceHealthState::Rejected, 1),
    ]);

    state.enforce_retention(&IvRetentionPolicy {
        max_raw_events: 2,
        max_indexed_points: 2,
        max_smiles: 2,
        max_surfaces: 2,
        max_derived_points: 2,
        max_source_health_events: 2,
    });

    let retained_current = handle
        .source_health_for("configured-profile", "configured-source")
        .expect("current source health must survive retention");
    assert_eq!(retained_current.subscription_generation, 2);
    assert_eq!(
        retained_current.subscription_state,
        IvSourceHealthState::Active
    );
}

#[test]
fn source_health_retention_keeps_all_current_generation_entries_when_current_exceeds_limit() {
    let mut current_generations = BTreeMap::new();
    current_generations.insert("configured-source-a".to_string(), 2);
    current_generations.insert("configured-source-b".to_string(), 2);
    current_generations.insert("configured-source-c".to_string(), 2);
    let state = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
    let handle = IvQueryHandle::from_state(
        "configured-profile",
        profile_wide_authorization(),
        state.clone(),
    )
    .with_current_subscription_generations(current_generations)
    .with_source_health(vec![
        source_health_with_generation("configured-old-source", IvSourceHealthState::Removed, 1),
        source_health_with_generation("configured-source-a", IvSourceHealthState::Active, 2),
        source_health_with_generation("configured-source-b", IvSourceHealthState::Active, 2),
        source_health_with_generation("configured-source-c", IvSourceHealthState::Active, 2),
    ]);

    state.enforce_retention(&IvRetentionPolicy {
        max_raw_events: 2,
        max_indexed_points: 2,
        max_smiles: 2,
        max_surfaces: 2,
        max_derived_points: 2,
        max_source_health_events: 2,
    });

    for source_id in [
        "configured-source-a",
        "configured-source-b",
        "configured-source-c",
    ] {
        let retained_current = handle
            .source_health_for("configured-profile", source_id)
            .expect("current source health must survive retention");
        assert_eq!(retained_current.subscription_generation, 2);
        assert_eq!(
            retained_current.subscription_state,
            IvSourceHealthState::Active
        );
    }
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
            basis_selection: IvBasisSelection::PreserveInputBasis,
            source_eligibility: vec!["configured-source".to_string()],
            strike_selection: IvStrikeSelection::AllConfiguredStrikes,
            tenor_selection: IvTenorSelection::AllConfiguredTenors,
            evidence_mapping: IvEvidenceMapping::PreserveEvidenceKind,
            minimum_points: 1,
            max_projection_input_skew_ns: 10,
            fallback_policy_ref: None,
            interpolation_policy_ref: None,
            quorum_policy_ref: None,
        }]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
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
    let Some(IvPolicyDecision::ProjectionDecision { convention, .. }) =
        projected.provenance.policy_decisions.last()
    else {
        panic!("expected projection policy decision");
    };
    assert_eq!(convention, "configured-convention");
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
        .query(&IvQuery::product(IvProductQuery {
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
fn source_health_query_prefers_current_generation_for_source_filter() {
    let mut current_generations = BTreeMap::new();
    current_generations.insert("configured-source".to_string(), 2);
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_current_subscription_generations(current_generations)
    .with_source_health(vec![
        source_health_with_generation("configured-source", IvSourceHealthState::Stale, 1),
        source_health_with_generation("configured-source", IvSourceHealthState::Active, 2),
    ]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-source".to_string()),
                state_filter: Vec::new(),
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source health product");
    };
    assert_eq!(health.subscription_generation, 2);
    assert_eq!(health.subscription_state, IvSourceHealthState::Active);
}

#[test]
fn source_health_query_prefers_current_generation_without_source_filter() {
    let mut current_generations = BTreeMap::new();
    current_generations.insert("configured-source".to_string(), 2);
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_current_subscription_generations(current_generations)
    .with_source_health(vec![
        source_health_with_generation("configured-source", IvSourceHealthState::Stale, 1),
        source_health_with_generation("configured-source", IvSourceHealthState::Active, 2),
    ]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: None,
                state_filter: Vec::new(),
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source health product");
    };
    assert_eq!(health.subscription_generation, 2);
    assert_eq!(health.subscription_state, IvSourceHealthState::Active);
}

#[test]
fn source_health_current_generation_lookup_does_not_fall_back_to_stale_history() {
    let mut current_generations = BTreeMap::new();
    current_generations.insert("configured-source".to_string(), 2);
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_current_subscription_generations(current_generations)
    .with_source_health(vec![source_health_with_generation(
        "configured-source",
        IvSourceHealthState::Stale,
        1,
    )]);

    assert_eq!(
        handle.source_health_for("configured-profile", "configured-source"),
        None
    );
}

#[test]
fn source_health_rejected_filter_returns_active_rows_with_rejection_diagnostics() {
    let state = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
    let handle = IvQueryHandle::from_state(
        "configured-profile",
        profile_wide_authorization(),
        state.clone(),
    );
    state.record_source_rejection(
        "configured-profile".to_string(),
        "configured-source".to_string(),
        1,
        UnixNanos::new(2_010),
        IvRejectReason::InvalidIvValue,
        false,
    );

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-source".to_string()),
                state_filter: vec!["rejected".to_string()],
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source health product");
    };
    assert_eq!(health.subscription_state, IvSourceHealthState::Active);
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::InvalidIvValue)
    );
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
        handle.query(&IvQuery::product(IvProductQuery {
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
        handle.query(&IvQuery::product(IvProductQuery {
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
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![complete_inputs()]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
                inputs: None,
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
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
                inputs: None,
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
fn derived_iv_query_rejects_stale_profile_owned_input_candidate() {
    let mut request_inputs = complete_inputs();
    request_inputs.underlying_price = None;
    request_inputs.rate = None;
    request_inputs.carry = None;

    let mut profile_source_inputs = complete_inputs();
    profile_source_inputs.source_id = "configured-underlying-source".to_string();
    profile_source_inputs.source_kind = IvSourceKind::CustomImpliedVolatility;
    profile_source_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    profile_source_inputs.source_health_state = IvSourceHealthState::Stale;
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

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
                inputs: None,
            },
        })),
        Err(IvQueryError::DerivationRejected)
    );
}

#[test]
fn derived_iv_query_rejects_old_generation_profile_owned_input_candidate() {
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
    .with_current_subscription_generations(BTreeMap::from([
        ("configured-source".to_string(), 1),
        ("configured-underlying-source".to_string(), 2),
    ]))
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![profile_resolving_derived_input_policy()])
    .with_derived_inputs(vec![request_inputs, profile_source_inputs]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
                inputs: None,
            },
        })),
        Err(IvQueryError::DerivationRejected)
    );
}

#[test]
fn derived_iv_query_keeps_unmapped_inputs_when_generation_overrides_are_partial() {
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_current_subscription_generations(BTreeMap::from([(
        "configured-other-source".to_string(),
        2,
    )]))
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![complete_inputs()]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
                inputs: None,
            },
        }))
        .unwrap();

    let IvQueryProduct::DerivedIv(derived) = product else {
        panic!("expected derived IV from unmapped input source");
    };
    assert_eq!(derived.point.source_id, "configured-source");
    assert_eq!(derived.provenance.subscription_generation, 1);
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
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![complete_inputs()]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
                inputs: None,
            },
        })),
        Err(IvQueryError::DerivationRejected)
    );

    let health = handle
        .source_health_for("configured-profile", "configured-source")
        .unwrap();
    assert_eq!(health.subscription_state, IvSourceHealthState::Active);
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
fn derived_iv_query_rejections_are_retained_by_profile_memory_bounds() {
    let mut helper = helper_policy();
    helper.output_bounds.inclusive_max = Some(0.10);
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_retention_policy(retention_policy(2, 2))
    .with_helper_policies(vec![helper])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )]);

    for index in 0..4_u64 {
        let mut request_inputs = complete_inputs();
        request_inputs.source_id = format!("configured-derived-source-{index}");
        request_inputs.input_event_ids = vec![format!("configured-derived-event-{index}")];
        assert_eq!(
            handle.query(&IvQuery::product(IvProductQuery {
                strategy_id: "configured-strategy".to_string(),
                profile_id: "configured-profile".to_string(),
                product_kind: IvProductKind::DerivedIv,
                selector: IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns: UnixNanos::new(2_000),
                    inputs: Some(Box::new(request_inputs)),
                },
            })),
            Err(IvQueryError::DerivationRejected)
        );
    }

    assert!(
        handle
            .source_health_for("configured-profile", "configured-derived-source-0")
            .is_none()
    );
    assert!(
        handle
            .source_health_for("configured-profile", "configured-derived-source-1")
            .is_none()
    );
    assert!(
        handle
            .source_health_for("configured-profile", "configured-derived-source-2")
            .is_some()
    );
    assert!(
        handle
            .source_health_for("configured-profile", "configured-derived-source-3")
            .is_some()
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
    .with_retention_policy(retention_policy(1, 2))
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![first_inputs, second_inputs]);

    for as_of_ns in [UnixNanos::new(2_000), UnixNanos::new(2_010)] {
        handle
            .query(&IvQuery::product(IvProductQuery {
                strategy_id: "configured-strategy".to_string(),
                profile_id: "configured-profile".to_string(),
                product_kind: IvProductKind::DerivedIv,
                selector: IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns,
                    inputs: None,
                },
            }))
            .unwrap();
    }

    let retained = handle.derived_outputs();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].point.ts_event_ns, UnixNanos::new(2_010));
}

#[test]
fn derived_iv_query_deduplicates_cached_output_for_same_source_helper_and_timestamp() {
    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_retention_policy(retention_policy(4, 2))
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_derived_inputs(vec![complete_inputs()]);

    let query = IvQuery::product(IvProductQuery {
        strategy_id: "configured-strategy".to_string(),
        profile_id: "configured-profile".to_string(),
        product_kind: IvProductKind::DerivedIv,
        selector: IvSelector::DerivedIvQuery {
            instrument_id: "configured-option-instrument".to_string(),
            helper_policy_id: "configured-helper-policy".to_string(),
            as_of_ns: UnixNanos::new(2_000),
            inputs: None,
        },
    });

    handle.query(&query).unwrap();
    handle.query(&query).unwrap();

    let retained = handle.derived_outputs();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].point.source_id, "configured-source");
    assert_eq!(
        retained[0].helper_identity.helper_policy_id,
        "configured-helper-policy"
    );
    assert_eq!(retained[0].point.ts_event_ns, UnixNanos::new(2_000));
}

#[test]
fn derived_iv_query_uses_request_supplied_inputs_without_preseeded_bundle() {
    let mut request_inputs = complete_inputs();
    request_inputs.as_of_ns = UnixNanos::new(2_050);
    request_inputs.received_ts_ns = UnixNanos::new(2_055);
    request_inputs.option_price = Some(timed(request_inputs.option_price.unwrap().value, 2_045));
    request_inputs.underlying_price = Some(timed(100.0, 2_046));
    request_inputs.strike = Some(timed(100.0, 2_047));
    request_inputs.option_side = Some(IvTimedInput {
        value: IvOptionSide::Call,
        ts_ns: UnixNanos::new(2_048),
        source_kind: IvDerivedInputSourceKind::QuerySupplied,
        expires_at_ns: None,
    });
    request_inputs.time_to_expiry_years = Some(timed(0.5, 2_049));
    request_inputs.rate = Some(IvTimedInput {
        value: 0.01,
        ts_ns: UnixNanos::new(2_044),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(2_100)),
    });
    request_inputs.carry = Some(IvTimedInput {
        value: 0.0,
        ts_ns: UnixNanos::new(2_043),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(2_100)),
    });

    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_050),
                inputs: Some(Box::new(request_inputs)),
            },
        }))
        .unwrap();

    let IvQueryProduct::DerivedIv(derived) = product else {
        panic!("expected derived IV product");
    };
    assert_eq!(derived.point.ts_event_ns, UnixNanos::new(2_050));
    assert!(derived.point.iv > 0.0);
}

#[test]
fn request_supplied_derived_iv_does_not_persist_to_projection_cache() {
    let mut request_inputs = complete_inputs();
    request_inputs.as_of_ns = UnixNanos::new(2_050);
    request_inputs.received_ts_ns = UnixNanos::new(2_055);
    request_inputs.option_price = Some(timed(request_inputs.option_price.unwrap().value, 2_045));
    request_inputs.underlying_price = Some(timed(100.0, 2_046));
    request_inputs.strike = Some(timed(100.0, 2_047));
    request_inputs.option_side = Some(IvTimedInput {
        value: IvOptionSide::Call,
        ts_ns: UnixNanos::new(2_048),
        source_kind: IvDerivedInputSourceKind::QuerySupplied,
        expires_at_ns: None,
    });
    request_inputs.time_to_expiry_years = Some(timed(0.5, 2_049));
    request_inputs.rate = Some(IvTimedInput {
        value: 0.01,
        ts_ns: UnixNanos::new(2_044),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(2_100)),
    });
    request_inputs.carry = Some(IvTimedInput {
        value: 0.0,
        ts_ns: UnixNanos::new(2_043),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(2_100)),
    });

    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper_policy()])
    .with_derived_input_policies(vec![query_supplied_derived_input_policy(
        "configured-derived-input-policy",
    )])
    .with_projection_policies(vec![projection_policy_with_refs(1, None, None, None)]);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_050),
                inputs: Some(Box::new(request_inputs)),
            },
        }))
        .unwrap();
    assert!(matches!(product, IvQueryProduct::DerivedIv(_)));

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::DerivedIvQuery {
                    instrument_id: "configured-option-instrument".to_string(),
                    helper_policy_id: "configured-helper-policy".to_string(),
                    as_of_ns: UnixNanos::new(2_050),
                    inputs: None,
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_050),
            },
        })),
        Err(IvQueryError::ProductNotFound)
    );
}

#[test]
fn derived_iv_query_uses_helper_input_policy_ref_not_first_policy_for_helper() {
    let mut helper = helper_policy();
    helper.input_policy_ref = "configured-strict-derived-input-policy".to_string();
    let permissive_policy = query_supplied_derived_input_policy("configured-permissive-policy");
    let mut strict_policy = profile_resolving_derived_input_policy();
    strict_policy.input_policy_id = "configured-strict-derived-input-policy".to_string();

    let mut request_inputs = complete_inputs();
    request_inputs.as_of_ns = UnixNanos::new(2_050);
    request_inputs.received_ts_ns = UnixNanos::new(2_055);

    let handle = IvQueryHandle::new(
        "configured-profile",
        profile_wide_authorization(),
        IvStore::empty(),
    )
    .with_helper_policies(vec![helper])
    .with_derived_input_policies(vec![permissive_policy, strict_policy]);

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "configured-option-instrument".to_string(),
                helper_policy_id: "configured-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(2_050),
                inputs: Some(Box::new(request_inputs)),
            },
        })),
        Err(IvQueryError::DerivationRejected)
    );
}
