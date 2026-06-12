use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    str::FromStr,
    sync::Arc,
};

use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config},
    bolt_v3_iv::{
        derive::IvDerivedInputSet,
        error::IvRejectReason,
        health::IvSourceHealthState,
        ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
        query::{IvProductQuery, IvQuery, IvQueryProduct},
        runtime::{IvRuntimeEngine, IvRuntimeEngineError, cargo_pinned_nt_revision},
        selector::IvSelector,
        store::{IvRetentionPolicy, IvStore},
        subscription::IvRuntimeOperation,
        time::UnixNanos,
        types::{IvBasis, IvConvention, IvSourceKind},
    },
    bolt_v3_live_node::{
        plan_iv_engine_lifecycle, plan_iv_engine_reload_lifecycle,
        wire_bolt_v3_iv_runtime_event_bindings,
    },
    bolt_v3_strategy_registration::{
        build_iv_query_handle_registry_for_root, build_iv_query_handle_registry_for_runtime,
        validate_iv_strategy_references,
    },
};
use nautilus_common::msgbus::{self, switchboard};
use nautilus_core::{Params, UnixNanos as NtUnixNanos};
use nautilus_model::{
    data::{
        CustomData, CustomDataTrait, DataType, HasTsInit, OptionChainSlice, OptionGreekValues,
        OptionGreeks, OptionStrikeData, QuoteTick,
    },
    enums::GreeksConvention,
    identifiers::{InstrumentId, OptionSeriesId},
    types::{Price, Quantity},
};
use serde::Serialize;

fn repo_path(relative: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(relative)
        .to_string_lossy()
        .to_string()
}

fn greeks_event(ts: u64) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-greeks-selector".to_string(),
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
                iv: 0.40,
            }],
            greeks: IvGreekValues {
                delta: None,
                gamma: None,
                vega: None,
                theta: None,
                rho: None,
            },
            underlying_price: None,
            open_interest: None,
        }),
    }
}

fn greeks_event_with_generation(ts: u64, subscription_generation: u64) -> IvIngestEvent {
    IvIngestEvent {
        subscription_generation,
        ..greeks_event(ts)
    }
}

fn live_event_router_root_config() -> BoltV3RootConfig {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "smile", "source_health"]
max_raw_events = 4
max_indexed_points = 4
max_smiles = 4
max_surfaces = 4
max_derived_points = 4
max_source_health_events = 4
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks", "option_chain_slice"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source", "configured-chain-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 4
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "smile", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["BLACK_SCHOLES"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-greeks-value"

[iv.profiles.sources.params]
configured_source_param = "configured-greeks-value"

[[iv.profiles.sources]]
source_id = "configured-chain-source"
selector_fingerprint = "configured-chain-selector"
source_kind = "option_chain"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["BLACK_SCHOLES"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionChain"

[iv.profiles.sources.selector]
selector_kind = "source_option_chain"
series_ids = ["DERIBIT:BTC:BTC:2024-01-01T00:00:00Z"]
strike_range_policy = "atm_relative:1:1"

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-chain-value"

[iv.profiles.sources.params]
configured_source_param = "configured-chain-value"
"#
    );
    toml::from_str(&with_iv).unwrap()
}

fn configured_nt_option_greeks() -> OptionGreeks {
    OptionGreeks {
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
    }
}

fn configured_nt_option_chain_slice() -> OptionChainSlice {
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
    let mut calls = BTreeMap::new();
    calls.insert(
        Price::new(100.0, 1),
        OptionStrikeData {
            quote,
            greeks: Some(configured_nt_option_greeks()),
        },
    );
    OptionChainSlice {
        series_id: OptionSeriesId::from_str("DERIBIT:BTC:BTC:2024-01-01").unwrap(),
        atm_strike: Some(Price::new(100.0, 1)),
        calls,
        puts: BTreeMap::new(),
        ts_event: NtUnixNanos::from(2_000),
        ts_init: NtUnixNanos::from(1_900),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ConfiguredAggregateGreeksEvent {
    configured_delta: f64,
    configured_gamma: f64,
    configured_vega: f64,
    configured_theta: f64,
    configured_rho: f64,
    configured_iv: f64,
    configured_adapter_payload: String,
    ts_event: NtUnixNanos,
    ts_init: NtUnixNanos,
}

impl HasTsInit for ConfiguredAggregateGreeksEvent {
    fn ts_init(&self) -> NtUnixNanos {
        self.ts_init
    }
}

impl CustomDataTrait for ConfiguredAggregateGreeksEvent {
    fn type_name(&self) -> &'static str {
        "ConfiguredAggregateGreeksEvent"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn ts_event(&self) -> NtUnixNanos {
        self.ts_event
    }

    fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    fn clone_arc(&self) -> Arc<dyn CustomDataTrait> {
        Arc::new(self.clone())
    }

    fn eq_arc(&self, other: &dyn CustomDataTrait) -> bool {
        other.as_any().downcast_ref::<Self>() == Some(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ConfiguredCustomIvEvent {
    configured_iv: f64,
    configured_adapter_payload: String,
    ts_event: NtUnixNanos,
    ts_init: NtUnixNanos,
}

impl HasTsInit for ConfiguredCustomIvEvent {
    fn ts_init(&self) -> NtUnixNanos {
        self.ts_init
    }
}

impl CustomDataTrait for ConfiguredCustomIvEvent {
    fn type_name(&self) -> &'static str {
        "ConfiguredCustomIvEvent"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn ts_event(&self) -> NtUnixNanos {
        self.ts_event
    }

    fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    fn clone_arc(&self) -> Arc<dyn CustomDataTrait> {
        Arc::new(self.clone())
    }

    fn eq_arc(&self, other: &dyn CustomDataTrait) -> bool {
        other.as_any().downcast_ref::<Self>() == Some(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct JsonCustomDataEvent {
    type_name: &'static str,
    payload: serde_json::Value,
    ts_event: NtUnixNanos,
    ts_init: NtUnixNanos,
}

impl HasTsInit for JsonCustomDataEvent {
    fn ts_init(&self) -> NtUnixNanos {
        self.ts_init
    }
}

impl CustomDataTrait for JsonCustomDataEvent {
    fn type_name(&self) -> &'static str {
        self.type_name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn ts_event(&self) -> NtUnixNanos {
        self.ts_event
    }

    fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string(&self.payload).map_err(Into::into)
    }

    fn clone_arc(&self) -> Arc<dyn CustomDataTrait> {
        Arc::new(self.clone())
    }

    fn eq_arc(&self, other: &dyn CustomDataTrait) -> bool {
        other.as_any().downcast_ref::<Self>() == Some(self)
    }
}

fn live_custom_data_router_root_config() -> BoltV3RootConfig {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["aggregate_greeks", "custom_iv_evidence", "source_health"]
max_raw_events = 4
max_indexed_points = 4
max_smiles = 4
max_surfaces = 4
max_derived_points = 4
max_source_health_events = 4
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["aggregate_greeks", "custom_implied_volatility"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-aggregate-source", "configured-custom-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 4
max_age_ns = 10000

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["aggregate_greeks", "custom_iv_evidence", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-aggregate-source"
selector_fingerprint = "configured-aggregate-selector"
source_kind = "aggregate_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredAggregateGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "ConfiguredAggregateGreeksEvent"
underlying_selectors = ["configured-underlying-selector"]
delta_field = "configured_delta"
gamma_field = "configured_gamma"
vega_field = "configured_vega"
theta_field = "configured_theta"
rho_field = "configured_rho"
iv_field = "configured_iv"
iv_basis = "mark"
iv_convention = "configured-convention"

[iv.profiles.sources.selector.nt_params]

[iv.profiles.sources.params]

[[iv.profiles.sources]]
source_id = "configured-custom-source"
selector_fingerprint = "configured-custom-selector"
source_kind = "custom_implied_volatility"
client_id = "configured-client"
subscription_generation = 8
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredCustomIv"

[iv.profiles.sources.selector]
selector_kind = "source_custom_implied_volatility"
custom_iv_data_type = "ConfiguredCustomIvEvent"
custom_iv_data_fields = ["configured_iv"]

[iv.profiles.sources.selector.nt_params]

[iv.profiles.sources.params]
"#
    );
    toml::from_str(&with_iv).unwrap()
}

fn configured_aggregate_custom_data() -> (DataType, CustomData) {
    let mut params = Params::new();
    params.insert(
        "underlying_selectors".to_string(),
        serde_json::json!(["configured-underlying-selector"]),
    );
    let data_type = DataType::new(
        "ConfiguredAggregateGreeksEvent",
        Some(params),
        Some("configured-aggregate-source".to_string()),
    );
    let data = CustomData::new(
        Arc::new(ConfiguredAggregateGreeksEvent {
            configured_delta: 1.25,
            configured_gamma: 0.15,
            configured_vega: 2.5,
            configured_theta: -0.25,
            configured_rho: 0.05,
            configured_iv: 0.39,
            configured_adapter_payload: "configured-aggregate-extra".to_string(),
            ts_event: NtUnixNanos::from(2_000),
            ts_init: NtUnixNanos::from(1_900),
        }),
        data_type.clone(),
    );
    (data_type, data)
}

fn configured_malformed_aggregate_custom_data() -> (DataType, CustomData) {
    let mut params = Params::new();
    params.insert(
        "underlying_selectors".to_string(),
        serde_json::json!(["configured-underlying-selector"]),
    );
    let data_type = DataType::new(
        "ConfiguredAggregateGreeksEvent",
        Some(params),
        Some("configured-aggregate-source".to_string()),
    );
    let data = CustomData::new(
        Arc::new(JsonCustomDataEvent {
            type_name: "ConfiguredAggregateGreeksEvent",
            payload: serde_json::json!({
                "configured_delta": 1.25,
                "configured_gamma": 0.15,
                "configured_theta": -0.25,
                "configured_rho": 0.05
            }),
            ts_event: NtUnixNanos::from(2_100),
            ts_init: NtUnixNanos::from(2_000),
        }),
        data_type.clone(),
    );
    (data_type, data)
}

fn configured_custom_iv_data() -> (DataType, CustomData) {
    let data_type = DataType::new(
        "ConfiguredCustomIvEvent",
        None,
        Some("configured-custom-source".to_string()),
    );
    let data = CustomData::new(
        Arc::new(ConfiguredCustomIvEvent {
            configured_iv: 0.37,
            configured_adapter_payload: "configured-custom-extra".to_string(),
            ts_event: NtUnixNanos::from(2_000),
            ts_init: NtUnixNanos::from(1_900),
        }),
        data_type.clone(),
    );
    (data_type, data)
}

fn configured_malformed_custom_iv_data() -> (DataType, CustomData) {
    let data_type = DataType::new(
        "ConfiguredCustomIvEvent",
        None,
        Some("configured-custom-source".to_string()),
    );
    let data = CustomData::new(
        Arc::new(JsonCustomDataEvent {
            type_name: "ConfiguredCustomIvEvent",
            payload: serde_json::json!({
                "configured_other_value": 0.37
            }),
            ts_event: NtUnixNanos::from(2_200),
            ts_init: NtUnixNanos::from(2_100),
        }),
        data_type.clone(),
    );
    (data_type, data)
}

#[test]
fn live_iv_event_bindings_route_nt_option_greeks_into_strategy_handle() {
    let parsed = live_event_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    let _bindings =
        wire_bolt_v3_iv_runtime_event_bindings(parsed.iv.as_ref().unwrap(), &engine).unwrap();
    let greeks = configured_nt_option_greeks();

    msgbus::publish_option_greeks(
        switchboard::get_option_greeks_topic(greeks.instrument_id),
        &greeks,
    );

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
                source_filter: Some("configured-greeks-source".to_string()),
            },
        }))
        .unwrap();
    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected IV point");
    };
    assert_eq!(point.iv, 0.44);
    assert_eq!(
        point.convention,
        IvConvention::Named(GreeksConvention::BlackScholes.to_string())
    );
    assert_eq!(point.provenance.nt_symbol, "ConfiguredOptionGreeks");
}

#[test]
fn runtime_nt_option_greeks_rejects_unaccepted_convention() {
    let mut parsed = live_event_router_root_config();
    parsed.iv.as_mut().unwrap().profiles[0].sources[0].accepted_conventions =
        BTreeSet::from(["PRICE_ADJUSTED".to_string()]);
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let error = engine
        .ingest_nt_option_greeks(
            "configured-profile",
            "configured-greeks-source",
            &configured_nt_option_greeks(),
            UnixNanos::new(2_100),
        )
        .expect_err("unaccepted NT greeks convention must reject before indexing");

    assert!(matches!(
        error,
        IvRuntimeEngineError::IngestRejected {
            reason: IvRejectReason::UnsupportedConvention,
            ..
        }
    ));
    let health = engine
        .source_health("configured-profile", "configured-greeks-source")
        .expect("convention rejection should be recorded in source health");
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::UnsupportedConvention)
    );
}

#[test]
fn runtime_nt_option_greeks_rejects_missing_iv_basis() {
    let mut parsed = live_event_router_root_config();
    parsed.iv.as_mut().unwrap().profiles[0].sources[0].accepted_conventions =
        BTreeSet::from(["BLACK_SCHOLES".to_string()]);
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let mut greeks = configured_nt_option_greeks();
    greeks.mark_iv = None;
    greeks.bid_iv = None;
    greeks.ask_iv = None;

    let error = engine
        .ingest_nt_option_greeks(
            "configured-profile",
            "configured-greeks-source",
            &greeks,
            UnixNanos::new(2_100),
        )
        .expect_err("greeks events without mark/bid/ask IV must reject typed indexing");

    assert!(matches!(
        error,
        IvRuntimeEngineError::IngestRejected {
            reason: IvRejectReason::MissingIvBasis,
            ..
        }
    ));
    let health = engine
        .source_health("configured-profile", "configured-greeks-source")
        .expect("missing-basis rejection should be recorded in source health");
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::MissingIvBasis)
    );
}

#[test]
fn runtime_nt_option_greeks_rejects_zero_iv_in_source_health() {
    let mut parsed = live_event_router_root_config();
    parsed.iv.as_mut().unwrap().profiles[0].sources[0].accepted_conventions =
        BTreeSet::from(["BLACK_SCHOLES".to_string()]);
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let mut greeks = configured_nt_option_greeks();
    greeks.mark_iv = Some(0.0);
    greeks.bid_iv = None;
    greeks.ask_iv = None;

    let error = engine
        .ingest_nt_option_greeks(
            "configured-profile",
            "configured-greeks-source",
            &greeks,
            UnixNanos::new(2_100),
        )
        .expect_err("zero IV must reject typed indexing and update health");

    assert!(matches!(
        error,
        IvRuntimeEngineError::IngestRejected {
            reason: IvRejectReason::InvalidIvValue,
            ..
        }
    ));
    let health = engine
        .source_health("configured-profile", "configured-greeks-source")
        .expect("invalid-IV rejection should be recorded in source health");
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
fn runtime_nt_option_chain_rejects_missing_iv_basis() {
    let mut parsed = live_event_router_root_config();
    parsed.iv.as_mut().unwrap().profiles[0].sources[1].accepted_conventions =
        BTreeSet::from(["BLACK_SCHOLES".to_string()]);
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let mut chain = configured_nt_option_chain_slice();
    for strike in chain.calls.values_mut() {
        let greeks = strike
            .greeks
            .as_mut()
            .expect("configured option-chain strike should include greeks");
        greeks.mark_iv = None;
        greeks.bid_iv = None;
        greeks.ask_iv = None;
    }

    let error = engine
        .ingest_nt_option_chain_slice(
            "configured-profile",
            "configured-chain-source",
            &chain,
            UnixNanos::new(2_100),
        )
        .expect_err("option-chain events without mark/bid/ask IV must reject typed indexing");

    assert!(matches!(
        error,
        IvRuntimeEngineError::IngestRejected {
            reason: IvRejectReason::MissingIvBasis,
            ..
        }
    ));
    let health = engine
        .source_health("configured-profile", "configured-chain-source")
        .expect("missing-basis rejection should be recorded in source health");
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::MissingIvBasis)
    );
}

#[test]
fn live_iv_event_bindings_route_nt_option_chain_into_strategy_handle() {
    let parsed = live_event_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    let _bindings =
        wire_bolt_v3_iv_runtime_event_bindings(parsed.iv.as_ref().unwrap(), &engine).unwrap();
    let chain = configured_nt_option_chain_slice();

    msgbus::publish_option_chain(switchboard::get_option_chain_topic(chain.series_id), &chain);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::Smile,
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
    assert_eq!(smile.points_by_strike[0].iv, 0.44);
    assert_eq!(smile.provenance.nt_symbol, "ConfiguredOptionChain");
}

#[test]
fn runtime_custom_data_ingest_preserves_original_json_in_raw_payloads() {
    let parsed = live_custom_data_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let (_, aggregate_data) = configured_aggregate_custom_data();
    let aggregate_raw = engine
        .ingest_nt_aggregate_greeks_custom_data(
            "configured-profile",
            "configured-aggregate-source",
            &aggregate_data,
            UnixNanos::new(2_100),
        )
        .unwrap();
    let IvRawPayload::AggregateGreeks(aggregate_payload) = aggregate_raw.payload else {
        panic!("expected aggregate greeks raw payload");
    };
    assert_eq!(
        aggregate_payload
            .nt_custom_data_json
            .as_ref()
            .and_then(|payload| payload.get("configured_adapter_payload"))
            .and_then(serde_json::Value::as_str),
        Some("configured-aggregate-extra")
    );

    let (_, custom_data) = configured_custom_iv_data();
    let custom_raw = engine
        .ingest_nt_custom_iv_data(
            "configured-profile",
            "configured-custom-source",
            &custom_data,
            UnixNanos::new(2_100),
        )
        .unwrap();
    let IvRawPayload::CustomImpliedVolatility(custom_payload) = custom_raw.payload else {
        panic!("expected custom IV raw payload");
    };
    assert_eq!(
        custom_payload
            .nt_custom_data_json
            .as_ref()
            .and_then(|payload| payload.get("configured_adapter_payload"))
            .and_then(serde_json::Value::as_str),
        Some("configured-custom-extra")
    );
}

#[test]
fn live_iv_event_bindings_route_nt_custom_data_into_aggregate_greeks_handle() {
    let parsed = live_custom_data_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    let _bindings =
        wire_bolt_v3_iv_runtime_event_bindings(parsed.iv.as_ref().unwrap(), &engine).unwrap();
    let (data_type, data) = configured_aggregate_custom_data();

    msgbus::publish_any(switchboard::get_custom_topic(&data_type), &data);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::AggregateGreeks,
            selector: IvSelector::AggregateGreeksQuery {
                aggregate_key: "ConfiguredAggregateGreeksEvent".to_string(),
                underlying_selectors: vec!["configured-underlying-selector".to_string()],
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();
    let IvQueryProduct::AggregateGreeks(aggregate) = product else {
        panic!("expected aggregate greeks product");
    };
    assert_eq!(aggregate.greeks.delta, Some(1.25));
    assert_eq!(aggregate.greeks.vega, Some(2.5));
    let aggregate_iv = aggregate.aggregate_iv.expect("configured aggregate IV");
    assert_eq!(aggregate_iv.basis, IvBasis::Mark);
    assert_eq!(aggregate_iv.value, 0.39);
    assert_eq!(
        aggregate_iv.convention,
        IvConvention::Named("configured-convention".to_string())
    );
    assert_eq!(aggregate.provenance.nt_symbol, "ConfiguredAggregateGreeks");
}

#[test]
fn live_iv_event_bindings_route_nt_custom_data_into_custom_iv_evidence_handle() {
    let parsed = live_custom_data_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    let _bindings =
        wire_bolt_v3_iv_runtime_event_bindings(parsed.iv.as_ref().unwrap(), &engine).unwrap();
    let (data_type, data) = configured_custom_iv_data();

    msgbus::publish_any(switchboard::get_custom_topic(&data_type), &data);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::CustomIvEvidence,
            selector: IvSelector::IvEvidenceQuery {
                iv_evidence_kind: "ConfiguredCustomIvEvent".to_string(),
                source_filter: Some("configured-custom-source".to_string()),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();
    let IvQueryProduct::CustomIvEvidence(evidence) = product else {
        panic!("expected custom IV evidence product");
    };
    assert_eq!(evidence.value, 0.37);
    assert_eq!(evidence.provenance.nt_symbol, "ConfiguredCustomIv");
}

#[test]
fn live_iv_event_bindings_record_malformed_aggregate_custom_data_rejection_in_source_health() {
    let parsed = live_custom_data_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    let _bindings =
        wire_bolt_v3_iv_runtime_event_bindings(parsed.iv.as_ref().unwrap(), &engine).unwrap();
    let (data_type, data) = configured_malformed_aggregate_custom_data();

    msgbus::publish_any(switchboard::get_custom_topic(&data_type), &data);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-aggregate-source".to_string()),
                state_filter: vec!["rejected".to_string()],
            },
        }))
        .unwrap();
    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source health product");
    };
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::InvalidIvValue)
    );
    assert_eq!(
        health.reject_counts.get(&IvRejectReason::InvalidIvValue),
        Some(&1)
    );
    assert_eq!(health.last_event_ts_ns, Some(UnixNanos::new(2_100)));
    assert_eq!(health.subscription_generation, 7);
}

#[test]
fn live_iv_event_bindings_record_malformed_custom_iv_data_rejection_in_source_health() {
    let parsed = live_custom_data_router_root_config();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    let _bindings =
        wire_bolt_v3_iv_runtime_event_bindings(parsed.iv.as_ref().unwrap(), &engine).unwrap();
    let (data_type, data) = configured_malformed_custom_iv_data();

    msgbus::publish_any(switchboard::get_custom_topic(&data_type), &data);

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::SourceHealth,
            selector: IvSelector::SourceHealthQuery {
                source_filter: Some("configured-custom-source".to_string()),
                state_filter: vec!["rejected".to_string()],
            },
        }))
        .unwrap();
    let IvQueryProduct::SourceHealth(health) = product else {
        panic!("expected source health product");
    };
    assert_eq!(
        health.last_reject_reason,
        Some(IvRejectReason::InvalidIvValue)
    );
    assert_eq!(
        health.reject_counts.get(&IvRejectReason::InvalidIvValue),
        Some(&1)
    );
    assert_eq!(health.last_event_ts_ns, Some(UnixNanos::new(2_200)));
    assert_eq!(health.subscription_generation, 8);
}

#[test]
fn source_health_and_retention_eviction_keep_current_views_bounded() {
    assert!(IvSourceHealthState::Configured.can_transition_to(IvSourceHealthState::Subscribing));
    assert!(IvSourceHealthState::Subscribing.can_transition_to(IvSourceHealthState::Active));
    assert!(IvSourceHealthState::Active.can_transition_to(IvSourceHealthState::Stale));
    assert!(!IvSourceHealthState::Removed.can_satisfy_current_query());

    let mut store = IvStore::empty();
    store.ingest_event(greeks_event(1_000)).unwrap();
    store
        .ingest_event(greeks_event_with_generation(2_000, 7))
        .unwrap();
    store.ingest_event(greeks_event(3_000)).unwrap();

    store.enforce_retention(&IvRetentionPolicy {
        max_raw_events: 2,
        max_indexed_points: 2,
        max_smiles: 2,
        max_surfaces: 2,
        max_derived_points: 2,
        max_source_health_events: 2,
    });

    assert_eq!(store.raw_events().len(), 2);
    assert_eq!(store.iv_points().len(), 2);
    assert_eq!(
        store.raw_events()[0].provenance.ts_event_ns,
        UnixNanos::new(2_000)
    );
}

#[test]
fn root_config_accepts_iv_profile_block() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );

    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();

    assert!(parsed.iv.is_some());
    assert_eq!(
        parsed.iv.unwrap().profiles[0].profile_id,
        "configured-profile"
    );
}

#[test]
fn live_node_strategy_handle_registration_and_iv_lifecycle_are_config_driven() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "projected_scalar_iv", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();

    let mut store = IvStore::empty();
    store
        .ingest_event(greeks_event_with_generation(2_000, 7))
        .unwrap();
    let registry = build_iv_query_handle_registry_for_root(&parsed, store).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");
    assert!(handle.authorization().is_profile_wide());
    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(IvSelector::PointQuery {
                    instrument_ids: vec!["configured-option-instrument".to_string()],
                    basis: IvBasis::Mark,
                    as_of_ns: UnixNanos::new(2_000),
                    source_filter: None,
                }),
                projection_policy_id: "configured-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(2_000),
            },
        }))
        .unwrap();
    let IvQueryProduct::ProjectedScalarIv(projected) = product else {
        panic!("expected projected scalar IV product from configured registry handle");
    };
    assert_eq!(projected.value, 0.40);

    let lifecycle = plan_iv_engine_lifecycle(&parsed).unwrap();
    assert_eq!(lifecycle.start_plans.len(), 1);
    assert_eq!(lifecycle.stop_plans.len(), 1);
    assert_eq!(lifecycle.start_plans[0].subscription_generation, 7);
}

#[test]
fn live_root_registry_stamps_derived_inputs_with_cargo_pinned_nt_revision() {
    let mut root = live_event_router_root_config();
    let profile = root.iv.as_mut().unwrap().profiles.first_mut().unwrap();
    profile.derived_inputs = vec![IvDerivedInputSet {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-greeks-selector".to_string(),
        instrument_id: "BTC-20240101-50000-C.DERIBIT".to_string(),
        basis: IvBasis::Mark,
        convention: IvConvention::Named("BLACK_SCHOLES".to_string()),
        as_of_ns: UnixNanos::new(2_000),
        received_ts_ns: UnixNanos::new(2_005),
        subscription_generation: 7,
        source_health_state: IvSourceHealthState::Active,
        nt_revision: "configured-config-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        input_event_ids: vec!["configured-input-event".to_string()],
        option_price: None,
        underlying_price: None,
        strike: None,
        option_side: None,
        time_to_expiry_years: None,
        rate: None,
        carry: None,
    }];

    let registry = build_iv_query_handle_registry_for_root(&root, IvStore::empty()).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");

    let derived_inputs = handle.derived_inputs();
    assert_eq!(derived_inputs.len(), 1);
    assert_eq!(derived_inputs[0].nt_revision, cargo_pinned_nt_revision());
}

#[test]
fn live_iv_root_reload_plans_reloaded_and_removed_sources() {
    let current = live_event_router_root_config();
    let mut next = current.clone();
    let profile = next.iv.as_mut().unwrap().profiles.first_mut().unwrap();
    profile.sources[0].subscription_generation = 8;
    profile.sources[0].selector_fingerprint = "configured-greeks-selector-reloaded".to_string();
    let removed_source = profile.sources.pop().unwrap();

    let lifecycle = plan_iv_engine_reload_lifecycle(&current, &next).unwrap();
    let operations = lifecycle
        .reload_plans
        .iter()
        .map(|plan| plan.operation)
        .collect::<Vec<_>>();

    assert!(lifecycle.start_plans.is_empty());
    assert!(lifecycle.stop_plans.is_empty());
    assert_eq!(
        operations,
        vec![
            IvRuntimeOperation::UnsubscribeOptionGreeks,
            IvRuntimeOperation::SubscribeOptionGreeks,
            IvRuntimeOperation::UnsubscribeOptionChain,
            IvRuntimeOperation::RemoveSource,
        ]
    );
    assert_eq!(lifecycle.reload_plans[1].subscription_generation, 8);
    assert_eq!(
        lifecycle.reload_plans[3].source_id,
        removed_source.source_id
    );
}

#[test]
fn registered_strategy_handle_reads_runtime_engine_state_after_registration() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");

    engine
        .ingest_event(greeks_event_with_generation(2_000, 7))
        .unwrap();

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
                source_filter: None,
            },
        }))
        .unwrap();

    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected runtime-ingested IV point");
    };
    assert_eq!(point.iv, 0.40);
}

#[test]
fn runtime_engine_enforces_configured_retention_after_ingest() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");

    for ts in [1_000, 2_000, 3_000] {
        engine
            .ingest_event(greeks_event_with_generation(ts, 7))
            .unwrap();
    }

    assert!(matches!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(1_000),
                source_filter: None,
            },
        })),
        Err(bolt_v2::bolt_v3_iv::query::IvQueryError::RetentionMiss)
    ));

    let product = handle
        .query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(3_000),
                source_filter: None,
            },
        }))
        .unwrap();
    let IvQueryProduct::IvPoint(point) = product else {
        panic!("expected latest retained runtime-ingested IV point");
    };
    assert_eq!(point.provenance.ts_event_ns, UnixNanos::new(3_000));
}

#[test]
fn runtime_engine_enforces_retention_after_failed_indexing_ingest() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();

    for ts in [1_000, 2_000, 3_000] {
        let mut event = greeks_event_with_generation(ts, 7);
        let IvRawPayload::OptionGreeks(payload) = &mut event.payload else {
            panic!("fixture must be option greeks");
        };
        payload.basis_values[0].iv = f64::NAN;
        assert!(matches!(
            engine.ingest_event(event),
            Err(IvRuntimeEngineError::IngestRejected {
                reason: IvRejectReason::InvalidIvValue,
                ..
            })
        ));
    }

    assert_eq!(
        engine
            .state_for_profile("configured-profile")
            .unwrap()
            .raw_event_count(),
        2
    );
}

#[test]
fn iv_strategy_reference_validation_rejects_missing_runtime_strategy_ids() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "missing-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();
    let loaded = LoadedBoltV3Config {
        root_path: Path::new("tests/fixtures/bolt_v3/root.toml").to_path_buf(),
        config_bundle_checksum: "configured-config-bundle-checksum".to_string(),
        root: parsed,
        strategies: Vec::new(),
    };

    let error = validate_iv_strategy_references(&loaded)
        .expect_err("IV profile must not reference an unloaded strategy");
    let rendered = error.to_string();
    assert!(
        rendered.contains("configured-profile") && rendered.contains("missing-strategy"),
        "validation error should identify the profile and missing strategy: {rendered}"
    );
}

#[test]
fn runtime_registry_supports_two_configured_strategies_with_different_selectors() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point"]
max_raw_events = 4
max_indexed_points = 4
max_smiles = 4
max_surfaces = 4
max_derived_points = 4
max_source_health_events = 4
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-source-a", "configured-source-b"]

[iv.profiles.audit_policy.audit_retention]
max_events = 4
max_age_ns = 10000

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy-a"
authorization_mode = "selector_scoped"
allowed_product_kinds = ["iv_point"]
allowed_selector_fingerprints = ["configured-selector-a"]
allowed_source_ids = ["configured-source-a"]

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy-b"
authorization_mode = "selector_scoped"
allowed_product_kinds = ["iv_point"]
allowed_selector_fingerprints = ["configured-selector-b"]
allowed_source_ids = ["configured-source-b"]

[[iv.profiles.sources]]
source_id = "configured-source-a"
selector_fingerprint = "configured-selector-a"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-option-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value-a"

[iv.profiles.sources.params]
configured_source_param = "configured-value-a"

[[iv.profiles.sources]]
source_id = "configured-source-b"
selector_fingerprint = "configured-selector-b"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-option-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value-b"

[iv.profiles.sources.params]
configured_source_param = "configured-value-b"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let mut event_a = greeks_event_with_generation(2_000, 7);
    event_a.source_id = "configured-source-a".to_string();
    event_a.selector_fingerprint = "configured-selector-a".to_string();
    let mut event_b = greeks_event_with_generation(2_001, 7);
    event_b.source_id = "configured-source-b".to_string();
    event_b.selector_fingerprint = "configured-selector-b".to_string();
    if let IvRawPayload::OptionGreeks(payload) = &mut event_b.payload {
        payload.basis_values[0].iv = 0.43;
    }
    engine.ingest_event(event_a).unwrap();
    engine.ingest_event(event_b).unwrap();

    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle_a = registry
        .handle("configured-strategy-a", "configured-profile")
        .expect("strategy A should receive configured IV profile handle");
    let handle_b = registry
        .handle("configured-strategy-b", "configured-profile")
        .expect("strategy B should receive configured IV profile handle");
    let query = |strategy_id: &str, source_id: &str, as_of_ns: u64| {
        IvQuery::product(IvProductQuery {
            strategy_id: strategy_id.to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(as_of_ns),
                source_filter: Some(source_id.to_string()),
            },
        })
    };

    let IvQueryProduct::IvPoint(point_a) = handle_a
        .query(&query(
            "configured-strategy-a",
            "configured-source-a",
            2_000,
        ))
        .unwrap()
    else {
        panic!("expected strategy A IV point");
    };
    let IvQueryProduct::IvPoint(point_b) = handle_b
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
    assert!(
        handle_a
            .query(&query(
                "configured-strategy-a",
                "configured-source-b",
                2_001
            ))
            .is_err()
    );
    assert!(
        handle_b
            .query(&query(
                "configured-strategy-b",
                "configured-source-a",
                2_000
            ))
            .is_err()
    );
}

#[test]
fn runtime_engine_rejects_stale_subscription_generation_products() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[iv.profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[iv.profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10

[[iv.profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );
    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let registry = build_iv_query_handle_registry_for_runtime(&parsed, &engine).unwrap();
    let handle = registry
        .handle("configured-strategy", "configured-profile")
        .expect("configured strategy should receive configured IV profile handle");

    let stale_rejection = engine
        .ingest_event(greeks_event_with_generation(2_000, 6))
        .expect_err("stale subscription generations must reject before indexing");
    assert!(matches!(
        stale_rejection,
        bolt_v2::bolt_v3_iv::runtime::IvRuntimeEngineError::IngestRejected {
            reason: bolt_v2::bolt_v3_iv::error::IvRejectReason::StaleData,
            ..
        }
    ));

    assert!(matches!(
        handle.query(&IvQuery::product(IvProductQuery {
            strategy_id: "configured-strategy".to_string(),
            profile_id: "configured-profile".to_string(),
            product_kind: bolt_v2::bolt_v3_iv::types::IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["configured-option-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(2_000),
                source_filter: None,
            },
        })),
        Err(bolt_v2::bolt_v3_iv::query::IvQueryError::ProductNotFound)
    ));
}

#[test]
fn runtime_engine_rejects_events_older_than_profile_freshness_bound() {
    let mut parsed = live_event_router_root_config();
    let profile = parsed
        .iv
        .as_mut()
        .unwrap()
        .profiles
        .iter_mut()
        .find(|profile| profile.profile_id == "configured-profile")
        .unwrap();
    profile.max_source_event_age_ns = Some(5);
    let engine = IvRuntimeEngine::from_iv_root(parsed.iv.as_ref().unwrap()).unwrap();
    let mut event = greeks_event_with_generation(2_000, 7);
    event.received_ts_ns = UnixNanos::new(2_006);

    let stale_rejection = engine
        .ingest_event(event)
        .expect_err("events older than the profile freshness bound must reject before indexing");
    assert!(matches!(
        stale_rejection,
        bolt_v2::bolt_v3_iv::runtime::IvRuntimeEngineError::IngestRejected {
            reason: bolt_v2::bolt_v3_iv::error::IvRejectReason::StaleData,
            ..
        }
    ));

    let health = engine
        .source_health("configured-profile", "configured-greeks-source")
        .expect("stale event must be reflected in source health");
    assert_eq!(health.subscription_state, IvSourceHealthState::Stale);
    assert!(health.stale_state);
    assert_eq!(health.last_reject_reason, Some(IvRejectReason::StaleData));
}
