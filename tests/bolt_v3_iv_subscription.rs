use bolt_v2::bolt_v3_iv::{
    config::load_iv_config_from_toml,
    derive::{IvDerivedInputSet, IvDerivedInputSourceKind, IvOptionSide, IvTimedInput},
    error::IvRejectReason,
    health::IvSourceHealthState,
    ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
    query::{IvProductQuery, IvQuery, IvQueryError, IvQueryHandle, IvQueryProduct},
    runtime::{
        IvRuntimeBindingAdapter, IvRuntimeBindingError, IvRuntimeEngine, IvRuntimeEngineError,
        apply_subscription_plans, cargo_pinned_nt_revision,
    },
    selector::IvSelector,
    subscription::{
        IvNtSubscriptionKind, IvProfileSubscriptionConfig, IvRuntimeOperation,
        IvSourceSubscriptionConfig, IvSubscriptionLifecycle, IvSubscriptionPlan,
        plan_profile_reload, plan_profile_start,
    },
    types::IvProductKind,
    types::IvSourceKind,
};
use std::{collections::BTreeMap, str::FromStr};

use bolt_v2::bolt_v3_iv::{time::UnixNanos, types::IvBasis, types::IvConvention};
use nautilus_core::UnixNanos as NtUnixNanos;
use nautilus_model::{
    data::{OptionChainSlice, OptionGreekValues, OptionGreeks, OptionStrikeData, QuoteTick},
    enums::GreeksConvention,
    identifiers::{InstrumentId, OptionSeriesId},
    types::{Price, Quantity},
};

fn profile_id() -> String {
    "iv-profile".to_string()
}

fn profile(sources: Vec<IvSourceSubscriptionConfig>) -> IvProfileSubscriptionConfig {
    IvProfileSubscriptionConfig {
        profile_id: profile_id(),
        sources,
    }
}

fn source(
    source_id: &str,
    source_kind: IvSourceKind,
    client_id: &str,
    selector: IvSelector,
    params: toml::Value,
    subscription_generation: u64,
) -> IvSourceSubscriptionConfig {
    IvSourceSubscriptionConfig {
        source_id: source_id.to_string(),
        source_kind,
        client_id: client_id.to_string(),
        selector,
        params,
        subscription_generation,
    }
}

#[derive(Default)]
struct RecordingRuntimeAdapter {
    applied: Vec<IvSubscriptionPlan>,
}

impl IvRuntimeBindingAdapter for RecordingRuntimeAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), bolt_v2::bolt_v3_iv::runtime::IvRuntimeBindingError> {
        self.applied.push(plan.clone());
        Ok(())
    }
}

fn greeks_ingest_event(source_id: &str, subscription_generation: u64) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "iv-profile".to_string(),
        source_id: source_id.to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "greeks-selector".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(2_000),
        ts_init_ns: Some(UnixNanos::new(1_900)),
        received_ts_ns: UnixNanos::new(2_100),
        subscription_generation,
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

fn timed_input(value: f64, ts: u64) -> IvTimedInput<f64> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: None,
    }
}

fn side_input(value: IvOptionSide, ts: u64) -> IvTimedInput<IvOptionSide> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts),
        source_kind: IvDerivedInputSourceKind::InstrumentMetadata,
        expires_at_ns: None,
    }
}

fn derived_input_set(nt_revision: &str) -> IvDerivedInputSet {
    IvDerivedInputSet {
        profile_id: profile_id(),
        source_id: "greeks-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "greeks-selector".to_string(),
        instrument_id: "BTC-20240101-50000-C.DERIBIT".to_string(),
        basis: IvBasis::Mark,
        convention: IvConvention::Named("BLACK_SCHOLES".to_string()),
        as_of_ns: UnixNanos::new(2_000),
        received_ts_ns: UnixNanos::new(2_005),
        subscription_generation: 7,
        source_health_state: IvSourceHealthState::Active,
        nt_revision: nt_revision.to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        input_event_ids: vec!["configured-input-event".to_string()],
        option_price: Some(timed_input(10.0, 1_995)),
        underlying_price: Some(timed_input(100.0, 1_996)),
        strike: Some(timed_input(100.0, 1_997)),
        option_side: Some(side_input(IvOptionSide::Call, 1_998)),
        time_to_expiry_years: Some(timed_input(0.5, 1_999)),
        rate: Some(timed_input(0.01, 2_000)),
        carry: Some(timed_input(0.0, 2_001)),
        initial_vol: None,
    }
}

fn nt_option_greeks(instrument_id: &str) -> OptionGreeks {
    OptionGreeks {
        instrument_id: InstrumentId::from(instrument_id),
        convention: GreeksConvention::BlackScholes,
        greeks: OptionGreekValues {
            delta: 0.51,
            gamma: 0.02,
            vega: 0.13,
            theta: -0.04,
            rho: 0.01,
        },
        mark_iv: Some(0.44),
        bid_iv: Some(0.43),
        ask_iv: Some(0.45),
        underlying_price: Some(102.0),
        open_interest: Some(2200.0),
        ts_event: NtUnixNanos::from(2_000),
        ts_init: NtUnixNanos::from(1_900),
    }
}

fn configured_runtime_config() -> bolt_v2::bolt_v3_iv::config::IvRootConfig {
    load_iv_config_from_toml(
        r#"
schema_version = 1

[[profiles]]
profile_id = "iv-profile"
enabled_products = ["iv_point", "smile", "source_health"]
max_raw_events = 2
max_indexed_points = 4
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks", "option_chain_slice"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["greeks-source", "chain-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "smile", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "greeks-source"
selector_fingerprint = "greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["BLACK_SCHOLES"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "greeks-value"

[profiles.sources.params]
configured_source_param = "greeks-source-value"

[[profiles.sources]]
source_id = "chain-source"
selector_fingerprint = "chain-selector"
source_kind = "option_chain"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["BLACK_SCHOLES"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionChain"

[profiles.sources.selector]
selector_kind = "source_option_chain"
series_ids = ["DERIBIT:BTC:BTC:2024-01-01T00:00:00Z"]
strike_range_policy = "atm_relative:1:1"

[profiles.sources.selector.nt_params]
configured_nt_param = "chain-value"

[profiles.sources.params]
configured_source_param = "chain-source-value"
"#,
    )
    .unwrap()
}

#[test]
fn runtime_engine_carries_configured_source_nt_provenance() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();

    let provenance = engine
        .source_nt_provenance("iv-profile", "greeks-source")
        .unwrap();

    assert_eq!(provenance.nt_revision, cargo_pinned_nt_revision());
    assert_eq!(
        provenance.nt_evidence_path,
        "configured/nt/evidence/path.rs"
    );
    assert_eq!(provenance.nt_symbol, "ConfiguredOptionGreeks");
}

#[test]
fn runtime_engine_ingests_nt_option_greeks_as_queryable_iv_point() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let nt_greeks = OptionGreeks {
        instrument_id: InstrumentId::from("BTC-20240101-50000-C.DERIBIT"),
        convention: GreeksConvention::BlackScholes,
        greeks: OptionGreekValues {
            delta: 0.51,
            gamma: 0.02,
            vega: 0.13,
            theta: -0.04,
            rho: 0.01,
        },
        mark_iv: Some(0.44),
        bid_iv: Some(0.43),
        ask_iv: Some(0.45),
        underlying_price: Some(102.0),
        open_interest: Some(2200.0),
        ts_event: NtUnixNanos::from(2_000),
        ts_init: NtUnixNanos::from(1_900),
    };

    let raw = engine
        .ingest_nt_option_greeks(
            "iv-profile",
            "greeks-source",
            &nt_greeks,
            UnixNanos::new(2_100),
        )
        .unwrap();

    assert_eq!(raw.provenance.nt_revision, cargo_pinned_nt_revision());
    assert_eq!(raw.provenance.nt_symbol, "ConfiguredOptionGreeks");
    assert_eq!(raw.provenance.ts_event_ns, UnixNanos::new(2_000));
    let IvRawPayload::OptionGreeks(payload) = &raw.payload else {
        panic!("expected option greeks payload");
    };
    assert_eq!(payload.instrument_id, "BTC-20240101-50000-C.DERIBIT");
    assert_eq!(payload.basis_values.len(), 3);
    let handle = IvQueryHandle::from_state(
        "iv-profile",
        config.profiles[0].strategy_authorizations()[0].clone(),
        engine.state_for_profile("iv-profile").unwrap(),
    );
    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
                source_filter: Some("greeks-source".to_string()),
            },
        }))
        .unwrap();
    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected IV point");
    };
    assert_eq!(point.iv, 0.44);
    assert_eq!(point.provenance.nt_symbol, "ConfiguredOptionGreeks");
}

#[test]
fn runtime_engine_ingests_nt_option_chain_slice_as_queryable_smile() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let option_id = InstrumentId::from("BTC-20240101-50000-C.DERIBIT");
    let quote = QuoteTick::new(
        option_id,
        Price::new(4.1, 1),
        Price::new(4.3, 1),
        Quantity::new(12.0, 1),
        Quantity::new(13.0, 1),
        NtUnixNanos::from(2_000),
        NtUnixNanos::from(1_900),
    );
    let greeks = OptionGreeks {
        instrument_id: option_id,
        convention: GreeksConvention::BlackScholes,
        greeks: OptionGreekValues {
            delta: 0.51,
            gamma: 0.02,
            vega: 0.13,
            theta: -0.04,
            rho: 0.01,
        },
        mark_iv: Some(0.55),
        bid_iv: None,
        ask_iv: None,
        underlying_price: Some(102.0),
        open_interest: Some(2200.0),
        ts_event: NtUnixNanos::from(2_000),
        ts_init: NtUnixNanos::from(1_900),
    };
    let mut calls = BTreeMap::new();
    calls.insert(
        Price::new(100.0, 1),
        OptionStrikeData {
            quote,
            greeks: Some(greeks),
        },
    );
    let nt_chain = OptionChainSlice {
        series_id: OptionSeriesId::from_str("DERIBIT:BTC:BTC:2024-01-01").unwrap(),
        atm_strike: Some(Price::new(100.0, 1)),
        calls,
        puts: BTreeMap::new(),
        ts_event: NtUnixNanos::from(2_000),
        ts_init: NtUnixNanos::from(1_900),
    };

    let raw = engine
        .ingest_nt_option_chain_slice(
            "iv-profile",
            "chain-source",
            &nt_chain,
            UnixNanos::new(2_100),
        )
        .unwrap();

    assert_eq!(raw.provenance.nt_symbol, "ConfiguredOptionChain");
    let IvRawPayload::OptionChainSlice(payload) = &raw.payload else {
        panic!("expected option-chain payload");
    };
    assert_eq!(payload.atm_strike, Some(100.0));
    assert_eq!(payload.calls[0].quote.bid_price, Some(4.1));
    assert_eq!(
        payload.calls[0].greeks.as_ref().unwrap().basis_values[0].iv,
        0.55
    );
    let handle = IvQueryHandle::from_state(
        "iv-profile",
        config.profiles[0].strategy_authorizations()[0].clone(),
        engine.state_for_profile("iv-profile").unwrap(),
    );
    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::Smile,
            selector: IvSelector::SmileQuery {
                series_id: "DERIBIT:BTC:BTC:2024-01-01T00:00:00Z".to_string(),
                side: Some("call".to_string()),
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();
    let IvQueryProduct::Smile(smile) = product else {
        panic!("expected smile");
    };
    assert_eq!(smile.atm_strike, Some(100.0));
    assert_eq!(smile.points_by_strike[0].iv, 0.55);
    assert_eq!(smile.provenance.nt_symbol, "ConfiguredOptionChain");
}

#[test]
fn option_greeks_sources_plan_nt_subscribe_operations() {
    let selector = IvSelector::SourceOptionGreeks {
        instrument_ids: vec!["configured-instrument-a".to_string()],
        nt_params: toml::toml! {
            configured_nt_param = "greeks-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "greeks-source-value"
    }
    .into();
    let source = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        selector.clone(),
        params.clone(),
        7,
    );

    let plans = plan_profile_start(&profile(vec![source])).unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "greeks-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionGreeks,
            nt_source_kind: IvNtSubscriptionKind::OptionGreeks,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 7,
        }]
    );

    let mut adapter = RecordingRuntimeAdapter::default();
    let outcomes = apply_subscription_plans(&mut adapter, &plans);

    assert_eq!(adapter.applied, plans);
    assert_eq!(
        outcomes[0].source_health.subscription_state,
        IvSourceHealthState::Subscribing
    );
}

#[test]
fn plan_profile_reload_rejects_mismatched_profile_ids() {
    let source = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["configured-instrument-a".to_string()],
            nt_params: toml::toml! {
                configured_nt_param = "greeks-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "greeks-source-value"
        }
        .into(),
        7,
    );
    let current = profile(vec![source.clone()]);
    let mut next = profile(vec![source]);
    next.profile_id = "other-profile".to_string();

    assert_eq!(
        plan_profile_reload(&current, &next),
        Err(
            bolt_v2::bolt_v3_iv::subscription::IvSubscriptionError::ProfileMismatch {
                current_profile_id: "iv-profile".to_string(),
                next_profile_id: "other-profile".to_string(),
            }
        )
    );
}

#[test]
fn option_chain_sources_plan_nt_subscribe_operations() {
    let selector = IvSelector::SourceOptionChain {
        series_ids: vec![
            "configured-series-a".to_string(),
            "configured-series-b".to_string(),
        ],
        strike_range_policy: "atm_relative:1:1".to_string(),
        nt_params: toml::toml! {
            configured_nt_param = "chain-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "chain-source-value"
    }
    .into();

    let plans = plan_profile_start(&profile(vec![source(
        "chain-source",
        IvSourceKind::OptionChain,
        "configured-client",
        selector.clone(),
        params.clone(),
        8,
    )]))
    .unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "chain-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionChain,
            nt_source_kind: IvNtSubscriptionKind::OptionChain,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 8,
        }]
    );
}

#[test]
fn aggregate_greeks_sources_plan_topic_subscribe_operations() {
    let selector = IvSelector::SourceAggregateGreeks {
        aggregate_key: "configured-aggregate-key".to_string(),
        underlying_selectors: vec!["configured-underlying-selector".to_string()],
        delta_field: "configured-delta-field".to_string(),
        gamma_field: "configured-gamma-field".to_string(),
        vega_field: "configured-vega-field".to_string(),
        theta_field: "configured-theta-field".to_string(),
        rho_field: "configured-rho-field".to_string(),
        iv_field: None,
        iv_basis: None,
        iv_convention: None,
        nt_params: toml::toml! {
            configured_nt_param = "aggregate-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "aggregate-source-value"
    }
    .into();

    let plans = plan_profile_start(&profile(vec![source(
        "aggregate-source",
        IvSourceKind::AggregateGreeks,
        "configured-client",
        selector.clone(),
        params.clone(),
        9,
    )]))
    .unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "aggregate-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeAggregateGreeks,
            nt_source_kind: IvNtSubscriptionKind::AggregateGreeksTopic,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 9,
        }]
    );
}

#[test]
fn custom_implied_volatility_sources_plan_custom_data_subscribe_operations() {
    let selector = IvSelector::SourceCustomImpliedVolatility {
        custom_iv_data_type: "configured-custom-iv-data-type".to_string(),
        custom_iv_data_fields: vec!["configured-custom-iv-field".to_string()],
        nt_params: toml::toml! {
            configured_nt_param = "custom-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "custom-source-value"
    }
    .into();

    let plans = plan_profile_start(&profile(vec![source(
        "custom-source",
        IvSourceKind::CustomImpliedVolatility,
        "configured-client",
        selector.clone(),
        params.clone(),
        10,
    )]))
    .unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "custom-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeCustomData,
            nt_source_kind: IvNtSubscriptionKind::CustomData,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 10,
        }]
    );
}

#[test]
fn runtime_engine_applies_subscription_outcomes_to_queryable_source_health() {
    let config = load_iv_config_from_toml(
        r#"
schema_version = 1

[[profiles]]
profile_id = "iv-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "greeks-source"
selector_fingerprint = "greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument-a"]

[profiles.sources.selector.nt_params]
configured_nt_param = "greeks-value"

[profiles.sources.params]
configured_source_param = "greeks-source-value"
"#,
    )
    .unwrap();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let plans = plan_profile_start(&config.profiles[0].subscription_config()).unwrap();
    let mut adapter = RecordingRuntimeAdapter::default();
    let outcomes = apply_subscription_plans(&mut adapter, &plans);

    engine.apply_plan_outcomes(&outcomes).unwrap();

    let authorization = config.profiles[0].strategy_authorizations().remove(0);
    let handle = IvQueryHandle::from_state(
        "iv-profile",
        authorization,
        engine.state_for_profile("iv-profile").unwrap(),
    );
    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("greeks-source".to_string()),
                state_filter: vec!["subscribing".to_string()],
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source-health product");
    };
    assert_eq!(health.subscription_state, IvSourceHealthState::Subscribing);
    assert_eq!(health.subscription_generation, 7);

    engine
        .ingest_event(greeks_ingest_event("greeks-source", 7))
        .unwrap();
    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("greeks-source".to_string()),
                state_filter: vec!["active".to_string()],
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected active source-health product");
    };
    assert_eq!(health.subscription_state, IvSourceHealthState::Active);
    assert_eq!(health.last_event_ts_ns, Some(UnixNanos::new(2_000)));
    assert_eq!(health.subscription_generation, 7);
}

#[test]
fn runtime_engine_ignores_older_subscription_generation_outcomes() {
    let config = load_iv_config_from_toml(
        r#"
schema_version = 1

[[profiles]]
profile_id = "iv-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "greeks-source"
selector_fingerprint = "greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument-a"]

[profiles.sources.selector.nt_params]
configured_nt_param = "greeks-value"

[profiles.sources.params]
configured_source_param = "greeks-source-value"
"#,
    )
    .unwrap();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let plans = plan_profile_start(&config.profiles[0].subscription_config()).unwrap();
    let mut adapter = RecordingRuntimeAdapter::default();
    let outcomes = apply_subscription_plans(&mut adapter, &plans);
    engine.apply_plan_outcomes(&outcomes).unwrap();

    let mut older = outcomes[0].clone();
    older.plan.subscription_generation = 6;
    older.source_health.subscription_generation = 6;
    older.source_health.subscription_state = IvSourceHealthState::Stale;
    engine.apply_plan_outcomes(&[older]).unwrap();

    let authorization = config.profiles[0].strategy_authorizations().remove(0);
    let handle = IvQueryHandle::from_state(
        "iv-profile",
        authorization,
        engine.state_for_profile("iv-profile").unwrap(),
    );
    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("greeks-source".to_string()),
                state_filter: vec!["subscribing".to_string()],
            },
        }))
        .unwrap();

    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source-health product");
    };
    assert_eq!(health.subscription_state, IvSourceHealthState::Subscribing);
    assert_eq!(health.subscription_generation, 7);
}

#[test]
fn runtime_engine_apply_plan_outcomes_returns_binding_errors_after_recording_health() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let plans = plan_profile_start(&config.profiles[0].subscription_config()).unwrap();
    let mut adapter = RecordingRuntimeAdapter::default();
    let mut outcomes = apply_subscription_plans(&mut adapter, &plans);
    outcomes[0].error = Some(IvRuntimeBindingError::subscription_failed(
        &outcomes[0].plan,
        "configured subscription failure".to_string(),
    ));
    outcomes[0].source_health.subscription_state = IvSourceHealthState::SubscriptionFailed;
    outcomes[0].source_health.last_reject_reason = Some(IvRejectReason::SubscriptionFailed);

    assert_eq!(
        engine.apply_plan_outcomes(&outcomes),
        Err(IvRuntimeEngineError::SubscriptionPlanFailed {
            profile_id: "iv-profile".to_string(),
            source_id: "greeks-source".to_string(),
            reason: IvRejectReason::SubscriptionFailed,
        })
    );
    let health = engine
        .source_health("iv-profile", "greeks-source")
        .expect("failed outcome should still update source health");
    assert_eq!(
        health.subscription_state,
        IvSourceHealthState::SubscriptionFailed
    );
}

#[test]
fn runtime_engine_rejects_unconfigured_ingest_source_before_storage() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();

    let error = engine
        .ingest_event(greeks_ingest_event("missing-source", 7))
        .expect_err("unknown source must reject before storage");

    assert_eq!(
        error,
        IvRuntimeEngineError::IngestRejected {
            profile_id: "iv-profile".to_string(),
            source_id: "missing-source".to_string(),
            reason: IvRejectReason::SourceNotConfigured,
        }
    );
    let health = engine
        .source_health("iv-profile", "missing-source")
        .expect("unknown source rejection should be queryable as source health");
    assert_eq!(health.subscription_state, IvSourceHealthState::Rejected);
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::SourceNotConfigured)
    );
    assert_eq!(
        health
            .reject_counts
            .get(&IvRejectReason::SourceNotConfigured),
        Some(&1)
    );
}

#[test]
fn runtime_engine_rejects_source_clock_ahead_of_receive_time() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let mut event = greeks_ingest_event("greeks-source", 7);
    event.ts_event_ns = UnixNanos::new(2_100);
    event.received_ts_ns = UnixNanos::new(2_000);

    let error = engine
        .ingest_event(event)
        .expect_err("source-clock-ahead events must reject before storage");

    assert_eq!(
        error,
        IvRuntimeEngineError::IngestRejected {
            profile_id: "iv-profile".to_string(),
            source_id: "greeks-source".to_string(),
            reason: IvRejectReason::ClockSkew,
        }
    );
    let state = engine.state_for_profile("iv-profile").unwrap();
    assert_eq!(state.raw_event_count(), 0);
    let health = engine
        .source_health("iv-profile", "greeks-source")
        .expect("clock-skew rejection should be queryable as source health");
    assert_eq!(health.last_reject_reason, Some(IvRejectReason::ClockSkew));
}

#[test]
fn runtime_engine_allows_source_clock_ahead_within_configured_future_skew() {
    let mut config = configured_runtime_config();
    config.profiles[0].max_source_event_future_skew_ns = 125;
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let mut event = greeks_ingest_event("greeks-source", 7);
    event.ts_event_ns = UnixNanos::new(2_100);
    event.received_ts_ns = UnixNanos::new(2_000);

    engine
        .ingest_event(event)
        .expect("source-clock-ahead event within configured skew should index");

    let state = engine.state_for_profile("iv-profile").unwrap();
    assert_eq!(state.raw_event_count(), 1);
    let health = engine
        .source_health("iv-profile", "greeks-source")
        .expect("fresh ingest should publish source health");
    assert_eq!(health.subscription_state, IvSourceHealthState::Active);
    assert_eq!(health.last_reject_reason, None);
}

#[test]
fn runtime_engine_bounds_unconfigured_source_rejection_health() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();

    for index in 0..4_u64 {
        let source_id = format!("missing-source-{index}");
        engine
            .ingest_event(greeks_ingest_event(&source_id, 7 + index))
            .expect_err("unknown source must reject before storage");
    }

    assert!(
        engine
            .source_health("iv-profile", "missing-source-0")
            .is_none()
    );
    assert!(
        engine
            .source_health("iv-profile", "missing-source-1")
            .is_none()
    );
    assert!(
        engine
            .source_health("iv-profile", "missing-source-2")
            .is_some()
    );
    assert!(
        engine
            .source_health("iv-profile", "missing-source-3")
            .is_some()
    );
}

#[test]
fn runtime_engine_bounds_unconfigured_nt_option_greeks_source_rejection_health() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let nt_greeks = nt_option_greeks("BTC-20240101-50000-C.DERIBIT");

    for index in 0..4_u64 {
        let source_id = format!("missing-nt-source-{index}");
        engine
            .ingest_nt_option_greeks(
                "iv-profile",
                &source_id,
                &nt_greeks,
                UnixNanos::new(2_100 + index),
            )
            .expect_err("unknown typed NT source must reject before storage");
    }

    assert!(
        engine
            .source_health("iv-profile", "missing-nt-source-0")
            .is_none()
    );
    assert!(
        engine
            .source_health("iv-profile", "missing-nt-source-1")
            .is_none()
    );
    assert!(
        engine
            .source_health("iv-profile", "missing-nt-source-2")
            .is_some()
    );
    assert!(
        engine
            .source_health("iv-profile", "missing-nt-source-3")
            .is_some()
    );
}

#[test]
fn runtime_engine_preserves_current_nt_option_greeks_selector_rejection_health() {
    let mut config = configured_runtime_config();
    let template = config.profiles[0].sources[0].clone();
    config.profiles[0].sources = (0..4_u64)
        .map(|index| {
            let mut source = template.clone();
            source.source_id = format!("selector-reject-source-{index}");
            source.selector_fingerprint = format!("selector-reject-fingerprint-{index}");
            source
        })
        .collect();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let nt_greeks = nt_option_greeks("ETH-20240101-50000-C.DERIBIT");

    for index in 0..4_u64 {
        let source_id = format!("selector-reject-source-{index}");
        let error = engine
            .ingest_nt_option_greeks(
                "iv-profile",
                &source_id,
                &nt_greeks,
                UnixNanos::new(2_100 + index),
            )
            .expect_err("selector mismatch must reject before storage");
        assert!(matches!(
            error,
            IvRuntimeEngineError::IngestRejected {
                reason: IvRejectReason::SelectorProductMismatch,
                ..
            }
        ));
    }

    for index in 0..4_u64 {
        let source_id = format!("selector-reject-source-{index}");
        let retained = engine
            .source_health("iv-profile", &source_id)
            .expect("current selector rejection health should be queryable");
        assert_eq!(
            retained.last_reject_reason,
            Some(IvRejectReason::SelectorProductMismatch)
        );
    }
}

#[test]
fn runtime_engine_rejects_stale_generation_ingest_without_poisoning_active_health() {
    let config = configured_runtime_config();
    let engine = IvRuntimeEngine::from_iv_root(&config).unwrap();
    let plans = plan_profile_start(&config.profiles[0].subscription_config()).unwrap();
    let mut adapter = RecordingRuntimeAdapter::default();
    let outcomes = apply_subscription_plans(&mut adapter, &plans);
    engine.apply_plan_outcomes(&outcomes).unwrap();

    let error = engine
        .ingest_event(greeks_ingest_event("greeks-source", 6))
        .expect_err("stale source generation must reject before storage");

    assert_eq!(
        error,
        IvRuntimeEngineError::IngestRejected {
            profile_id: "iv-profile".to_string(),
            source_id: "greeks-source".to_string(),
            reason: IvRejectReason::StaleData,
        }
    );
    let health = engine
        .source_health("iv-profile", "greeks-source")
        .expect("configured source health should remain queryable");
    assert_eq!(health.subscription_state, IvSourceHealthState::Subscribing);
    assert_eq!(health.subscription_generation, 7);
    assert_eq!(health.last_reject_reason, Some(IvRejectReason::StaleData));
    assert_eq!(
        health.reject_counts.get(&IvRejectReason::StaleData),
        Some(&1)
    );
}

#[test]
fn runtime_engine_reload_updates_configured_source_generations() {
    let current = configured_runtime_config();
    let mut next = current.clone();
    next.profiles[0].sources[0].subscription_generation = 8;
    let mut engine = IvRuntimeEngine::from_iv_root(&current).unwrap();

    engine.apply_iv_root_reload(&next).unwrap();

    let stale_error = engine
        .ingest_event(greeks_ingest_event("greeks-source", 7))
        .expect_err("old generation must reject after IV root reload");
    assert_eq!(
        stale_error,
        IvRuntimeEngineError::IngestRejected {
            profile_id: "iv-profile".to_string(),
            source_id: "greeks-source".to_string(),
            reason: IvRejectReason::StaleData,
        }
    );

    engine
        .ingest_event(greeks_ingest_event("greeks-source", 8))
        .expect("new generation must ingest after IV root reload");
}

#[test]
fn runtime_engine_reload_stamps_derived_inputs_with_cargo_pinned_nt_revision() {
    let mut current = configured_runtime_config();
    current.profiles[0].derived_inputs = vec![derived_input_set("configured-current-nt-revision")];
    let mut next = current.clone();
    next.profiles[0].derived_inputs = vec![derived_input_set("configured-reload-nt-revision")];
    let mut engine = IvRuntimeEngine::from_iv_root(&current).unwrap();

    engine.apply_iv_root_reload(&next).unwrap();

    let state = engine
        .state_for_profile("iv-profile")
        .expect("reloaded profile state should exist");
    let derived_inputs = state.derived_inputs();
    assert_eq!(derived_inputs.len(), 1);
    assert_eq!(derived_inputs[0].nt_revision, cargo_pinned_nt_revision());
}

#[test]
fn runtime_engine_clones_observe_reloaded_source_generations_for_live_handlers() {
    let current = configured_runtime_config();
    let mut next = current.clone();
    next.profiles[0].sources[0].subscription_generation = 8;
    let mut engine = IvRuntimeEngine::from_iv_root(&current).unwrap();
    let handler_runtime = engine.clone();

    engine.apply_iv_root_reload(&next).unwrap();

    let stale_error = handler_runtime
        .ingest_event(greeks_ingest_event("greeks-source", 7))
        .expect_err("live handler runtime clone must reject the pre-reload generation");
    assert_eq!(
        stale_error,
        IvRuntimeEngineError::IngestRejected {
            profile_id: "iv-profile".to_string(),
            source_id: "greeks-source".to_string(),
            reason: IvRejectReason::StaleData,
        }
    );

    handler_runtime
        .ingest_event(greeks_ingest_event("greeks-source", 8))
        .expect("live handler runtime clone must accept the reloaded generation");
}

#[test]
fn runtime_engine_reload_invalidates_existing_handles_for_old_source_health() {
    let current = configured_runtime_config();
    let mut next = current.clone();
    next.profiles[0].sources[0].subscription_generation = 8;
    let mut engine = IvRuntimeEngine::from_iv_root(&current).unwrap();
    let start_plans = plan_profile_start(&current.profiles[0].subscription_config()).unwrap();
    let mut adapter = RecordingRuntimeAdapter::default();
    let start_outcomes = apply_subscription_plans(&mut adapter, &start_plans);
    engine.apply_plan_outcomes(&start_outcomes).unwrap();
    engine
        .ingest_event(greeks_ingest_event("greeks-source", 7))
        .unwrap();
    let handle = IvQueryHandle::from_state(
        "iv-profile",
        current.profiles[0].strategy_authorizations()[0].clone(),
        engine.state_for_profile("iv-profile").unwrap(),
    );

    engine.apply_iv_root_reload(&next).unwrap();

    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
                source_filter: Some("greeks-source".to_string()),
            },
        })),
        Err(IvQueryError::ProductNotFound)
    );
}

#[test]
fn runtime_engine_reload_invalidates_existing_handles_for_removed_profiles() {
    let mut current = configured_runtime_config();
    let mut retained_profile = current.profiles[0].clone();
    retained_profile.profile_id = "retained-profile".to_string();
    retained_profile.audit_policy.profile_id = "retained-profile".to_string();
    current.profiles.push(retained_profile);
    let mut next = current.clone();
    next.profiles
        .retain(|profile| profile.profile_id == "retained-profile");
    let mut engine = IvRuntimeEngine::from_iv_root(&current).unwrap();
    engine
        .ingest_event(greeks_ingest_event("greeks-source", 7))
        .unwrap();
    let handle = IvQueryHandle::from_state(
        "iv-profile",
        current.profiles[0].strategy_authorizations()[0].clone(),
        engine.state_for_profile("iv-profile").unwrap(),
    );

    engine.apply_iv_root_reload(&next).unwrap();

    assert!(engine.state_for_profile("iv-profile").is_none());
    assert_eq!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "iv-profile".to_string(),
            product_kind: IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
                source_filter: Some("greeks-source".to_string()),
            },
        })),
        Err(IvQueryError::ProductNotFound)
    );
}

#[test]
fn runtime_engine_apply_plan_outcomes_skips_removed_profiles_and_applies_survivors() {
    let mut current = configured_runtime_config();
    let mut retained_profile = current.profiles[0].clone();
    retained_profile.profile_id = "retained-profile".to_string();
    retained_profile.audit_policy.profile_id = "retained-profile".to_string();
    current.profiles.push(retained_profile.clone());
    let mut next = current.clone();
    next.profiles
        .retain(|profile| profile.profile_id == "retained-profile");
    let mut engine = IvRuntimeEngine::from_iv_root(&current).unwrap();

    engine.apply_iv_root_reload(&next).unwrap();

    let removed_plan =
        plan_profile_start(&current.profiles[0].subscription_config()).unwrap()[0].clone();
    let retained_plan =
        plan_profile_start(&retained_profile.subscription_config()).unwrap()[0].clone();
    let mut adapter = RecordingRuntimeAdapter::default();
    let outcomes = apply_subscription_plans(&mut adapter, &[removed_plan, retained_plan]);

    engine.apply_plan_outcomes(&outcomes).unwrap();

    assert!(engine.state_for_profile("iv-profile").is_none());
    let health = engine
        .source_health("retained-profile", "greeks-source")
        .expect("retained profile outcome should still apply");
    assert_eq!(health.subscription_state, IvSourceHealthState::Subscribing);
    assert_eq!(health.subscription_generation, 7);
}

#[test]
fn reload_unsubscribes_old_generation_subscribes_new_generation_and_removes_deleted_sources() {
    let current_greeks = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["configured-instrument-a".to_string()],
            nt_params: toml::toml! {
                configured_nt_param = "old-greeks-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "old-greeks-source-value"
        }
        .into(),
        3,
    );
    let next_greeks = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["configured-instrument-b".to_string()],
            nt_params: toml::toml! {
                configured_nt_param = "new-greeks-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "new-greeks-source-value"
        }
        .into(),
        4,
    );
    let removed_chain = source(
        "removed-chain-source",
        IvSourceKind::OptionChain,
        "configured-client",
        IvSelector::SourceOptionChain {
            series_ids: vec!["configured-series-a".to_string()],
            strike_range_policy: "atm_relative:1:1".to_string(),
            nt_params: toml::toml! {
                configured_nt_param = "removed-chain-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "removed-chain-source-value"
        }
        .into(),
        5,
    );

    let plans = plan_profile_reload(
        &profile(vec![current_greeks.clone(), removed_chain.clone()]),
        &profile(vec![next_greeks.clone()]),
    )
    .unwrap();

    assert_eq!(
        plans,
        vec![
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &current_greeks,
                IvSubscriptionLifecycle::Reload,
                IvRuntimeOperation::UnsubscribeOptionGreeks,
                IvNtSubscriptionKind::OptionGreeks,
            ),
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &next_greeks,
                IvSubscriptionLifecycle::Reload,
                IvRuntimeOperation::SubscribeOptionGreeks,
                IvNtSubscriptionKind::OptionGreeks,
            ),
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &removed_chain,
                IvSubscriptionLifecycle::SourceRemoval,
                IvRuntimeOperation::UnsubscribeOptionChain,
                IvNtSubscriptionKind::OptionChain,
            ),
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &removed_chain,
                IvSubscriptionLifecycle::SourceRemoval,
                IvRuntimeOperation::RemoveSource,
                IvNtSubscriptionKind::OptionChain,
            ),
        ]
    );
}
