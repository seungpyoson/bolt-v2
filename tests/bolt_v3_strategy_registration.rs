use crate::support;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_adapters::map_bolt_v3_adapters,
    bolt_v3_archetypes::{binary_oracle_edge_taker, complete_set_arbitrage},
    bolt_v3_client_registration::register_bolt_v3_clients,
    bolt_v3_config::{
        BoltV3RootConfig, ClientBlock, DataInstrumentBlock, RealizedVolatilityAggregationBlock,
        RealizedVolatilityPolicyBlock, RealizedVolatilitySampleKindBlock,
        RealizedVolatilitySourceBlock, RealizedVolatilitySourceClassBlock,
        RealizedVolatilitySurfaceBlock, ReferencePriceBlock, ReferencePriceDriftPolicy,
        ReferencePriceProvider, ReferencePriceSelectionPolicy, ReferencePriceSourceBlock,
        ReferencePriceStalePolicy, load_bolt_v3_config,
    },
    bolt_v3_iv::config::IvRootConfig,
    bolt_v3_live_node::{build_bolt_v3_live_node_with_summary, make_bolt_v3_live_node_builder},
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        BoltV3SubmitLifecyclePolicy,
    },
    strategies::{
        binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder,
        complete_set_arbitrage::CompleteSetArbitrageBuilder,
        production_strategy_registry,
        registry::{FeeProvider, StrategyBuildContext, StrategyBuilder, ValidationError},
    },
};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientId, InstrumentId, StrategyId, Venue},
};
use rust_decimal::Decimal;
use std::{collections::BTreeMap, fs, sync::Arc};

struct NoopFeeProvider;

const RV_DATA_CLIENT_ID: &str = "<DATA_CLIENT_ID>";
const RV_DATA_CLIENT_VENUE: &str = "OKX";

fn reference_price_client_from_toml(value: &str) -> ClientBlock {
    toml::from_str(value).expect("reference price test client should parse")
}

fn add_root_chainlink_feed_binding(root: &mut BoltV3RootConfig, instrument_id: &str) {
    let mut binding = toml::map::Map::new();
    binding.insert(
        "feed_id".to_string(),
        toml::Value::String(
            "0x00057da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439".to_string(),
        ),
    );
    binding.insert(
        "instrument_id".to_string(),
        toml::Value::String(instrument_id.to_string()),
    );
    binding.insert("report_schema_version".to_string(), toml::Value::Integer(3));
    binding.insert("report_decimal_scale".to_string(), toml::Value::Integer(18));
    binding.insert("price_precision".to_string(), toml::Value::Integer(8));

    root.chainlink_data_streams
        .as_mut()
        .expect("fixture root should include chainlink_data_streams")
        .feed_bindings
        .push(toml::Value::Table(binding));
}

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

fn assert_unsupported_executable_entry_order_shape(raw: &toml::Value, label: &str) {
    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.iter().any(|error| {
            error.field == "strategies.configured_updown_main.parameters.runtime.entry_order"
                && error.code == "unsupported_executable_entry_order_shape"
        }),
        "{label} entry runtime table should reject unsupported executable entry shape: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );
    assert!(
        BinaryOracleEdgeTakerBuilder::build(raw, &context).is_err(),
        "{label} entry runtime table must not parse into the strategy config"
    );
}

fn valid_realized_volatility_surface() -> RealizedVolatilitySurfaceBlock {
    RealizedVolatilitySurfaceBlock {
        canonical_base_asset: "CONFIGURED_ASSET".to_string(),
        canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
        policy: RealizedVolatilityPolicyBlock {
            window_ms: 4_000,
            sampling_interval_ms: 1_000,
            min_ready_sources: 1,
            max_source_age_ms: 500,
            max_inter_sample_gap_ms: 2_000,
            min_coverage_ratio: 0.75,
            max_cross_source_dispersion: 0.50,
            seconds_per_annum: 31_536_000.0,
            aggregation: RealizedVolatilityAggregationBlock::UpperQuantile,
            upper_quantile: 1.0,
            trim_fraction: None,
            guard_weight: None,
        },
        estimator: None,
        sources: vec![RealizedVolatilitySourceBlock {
            source_id: "<SOURCE_ID_A>".to_string(),
            data_client_id: ClientId::from(RV_DATA_CLIENT_ID),
            instrument_id: InstrumentId::from("CONFIGURED_ASSET-QUOTE.<DATA_CLIENT_ID>"),
            source_class: RealizedVolatilitySourceClassBlock::SpotQuote,
            sample_kind: RealizedVolatilitySampleKindBlock::Midpoint,
            enabled: true,
            counts_toward_quorum: true,
            canonical_base_asset: "CONFIGURED_ASSET".to_string(),
            canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
        }],
    }
}

fn insert_placeholder_realized_volatility_client(root: &mut BoltV3RootConfig) {
    root.clients.insert(
        RV_DATA_CLIENT_ID.to_string(),
        ClientBlock {
            venue: Venue::from(RV_DATA_CLIENT_VENUE),
            data: Some(toml::Value::Table(toml::map::Map::new())),
            execution: None,
            secrets: None,
            readiness_probe: None,
        },
    );
}

fn insert_realized_volatility_surface(
    root: &mut BoltV3RootConfig,
    surface: RealizedVolatilitySurfaceBlock,
) {
    insert_placeholder_realized_volatility_client(root);
    let _ = root
        .realized_volatility_surfaces
        .get_or_insert_with(BTreeMap::new)
        .insert("<surface_id>".to_string(), surface);
}

fn realized_volatility_validation_errors(
    mutate: impl FnOnce(&mut bolt_v2::bolt_v3_config::LoadedBoltV3Config),
) -> Vec<String> {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    mutate(&mut loaded);
    let mut errors = bolt_v2::bolt_v3_validate::validate_root_only(&loaded.root);
    errors.extend(bolt_v2::bolt_v3_validate::validate_strategies(
        &loaded.root,
        &loaded.strategies,
    ));
    errors
}

fn assert_realized_volatility_validation_error(
    mutate: impl FnOnce(&mut bolt_v2::bolt_v3_config::LoadedBoltV3Config),
    expected: &str,
) {
    let errors = realized_volatility_validation_errors(mutate);
    assert!(
        errors
            .iter()
            .any(|message| message.contains("realized_volatility_surfaces")
                && message.contains(expected)),
        "expected realized_volatility_surfaces validation error containing `{expected}`, got: {errors:?}"
    );
}

#[test]
fn realized_volatility_validation_rejects_duplicate_source_id() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources.push(surface.sources[0].clone());
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "duplicate source_id",
    );
}

#[test]
fn realized_volatility_validation_rejects_unknown_data_client_id() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].data_client_id = ClientId::from("<UNKNOWN_DATA_CLIENT_ID>");
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "data_client_id",
    );
}

#[test]
fn realized_volatility_validation_rejects_non_data_client_id() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let surface = valid_realized_volatility_surface();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded
                .root
                .clients
                .get_mut(RV_DATA_CLIENT_ID)
                .expect("test RV client should exist")
                .data = None;
        },
        "must reference a data-capable client",
    );
}

#[test]
fn realized_volatility_validation_rejects_source_asset_mismatch() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].canonical_base_asset = "OTHER_ASSET".to_string();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_base_asset",
    );
}

#[test]
fn realized_volatility_validation_rejects_padded_surface_base_asset() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.canonical_base_asset = " CONFIGURED_ASSET ".to_string();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_base_asset must not contain surrounding whitespace",
    );
}

#[test]
fn realized_volatility_validation_rejects_padded_surface_quote_asset() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.canonical_quote_asset = " <QUOTE_ASSET> ".to_string();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_quote_asset must not contain surrounding whitespace",
    );
}

#[test]
fn realized_volatility_validation_rejects_empty_source_base_asset() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].canonical_base_asset = String::new();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_base_asset must be non-empty",
    );
}

#[test]
fn realized_volatility_validation_rejects_padded_source_base_asset() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].canonical_base_asset = " CONFIGURED_ASSET ".to_string();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_base_asset must not contain surrounding whitespace",
    );
}

#[test]
fn realized_volatility_validation_rejects_blank_source_quote_asset() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].canonical_quote_asset = "   ".to_string();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_quote_asset must be non-empty",
    );
}

#[test]
fn realized_volatility_validation_rejects_padded_source_quote_asset() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].canonical_quote_asset = " <QUOTE_ASSET> ".to_string();
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "canonical_quote_asset must not contain surrounding whitespace",
    );
}

#[test]
fn realized_volatility_validation_accepts_nt_native_spot_symbols() {
    let errors = realized_volatility_validation_errors(|loaded| {
        let mut surface = valid_realized_volatility_surface();
        surface.canonical_base_asset = "BTC".to_string();
        surface.canonical_quote_asset = "USDT".to_string();
        surface.sources[0].source_id = "okx_btc_usdt_midpoint".to_string();
        surface.sources[0].data_client_id = ClientId::from(RV_DATA_CLIENT_ID);
        surface.sources[0].instrument_id = InstrumentId::from("BTC-USDT.OKX");
        surface.sources[0].canonical_base_asset = "BTC".to_string();
        surface.sources[0].canonical_quote_asset = "USDT".to_string();
        surface.sources.push(RealizedVolatilitySourceBlock {
            source_id: "binance_btc_usdt_midpoint".to_string(),
            data_client_id: ClientId::from(RV_DATA_CLIENT_ID),
            instrument_id: InstrumentId::from("BTCUSDT.BINANCE"),
            source_class: RealizedVolatilitySourceClassBlock::SpotQuote,
            sample_kind: RealizedVolatilitySampleKindBlock::Midpoint,
            enabled: true,
            counts_toward_quorum: true,
            canonical_base_asset: "BTC".to_string(),
            canonical_quote_asset: "USDT".to_string(),
        });
        surface.sources.push(RealizedVolatilitySourceBlock {
            source_id: "bybit_btc_usdt_midpoint".to_string(),
            data_client_id: ClientId::from(RV_DATA_CLIENT_ID),
            instrument_id: InstrumentId::from("BTCUSDT-SPOT.BYBIT"),
            source_class: RealizedVolatilitySourceClassBlock::SpotQuote,
            sample_kind: RealizedVolatilitySampleKindBlock::Midpoint,
            enabled: true,
            counts_toward_quorum: true,
            canonical_base_asset: "BTC".to_string(),
            canonical_quote_asset: "USDT".to_string(),
        });
        insert_realized_volatility_surface(&mut loaded.root, surface);
        add_root_chainlink_feed_binding(&mut loaded.root, "BTC-USD.CHAINLINK");
        loaded.strategies[0]
            .config
            .target
            .as_table_mut()
            .expect("fixture target should be a table")
            .insert(
                "underlying_asset".to_string(),
                toml::Value::String("BTC".to_string()),
            );
        let reference_current_price = loaded.strategies[0]
            .config
            .reference_current_price
            .as_mut()
            .expect("fixture should include reference_current_price");
        reference_current_price.asset = "BTC".to_string();
        reference_current_price
            .sources
            .get_mut("chainlink_primary")
            .expect("fixture should include chainlink_primary reference source")
            .instrument_id = Some("BTC-USD.CHAINLINK".to_string());
        reference_current_price
            .sources
            .get_mut("polyresearch_backup")
            .expect("fixture should include polyresearch_backup reference source")
            .symbol = Some("BTC/USD".to_string());
        loaded.strategies[0].config.realized_volatility_surface_id =
            Some("<surface_id>".to_string());
    });
    assert!(
        errors.is_empty(),
        "NT-native spot symbols should validate without RV errors: {errors:?}"
    );
}

#[test]
fn realized_volatility_validation_rejects_strategy_underlying_surface_asset_mismatch() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.canonical_base_asset = "OTHER_ASSET".to_string();
            surface.sources[0].instrument_id =
                InstrumentId::from("OTHER_ASSET-QUOTE.<DATA_CLIENT_ID>");
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "underlying_asset",
    );
}

#[test]
fn realized_volatility_validation_rejects_same_instrument_distinct_data_clients() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            let mut second_source = surface.sources[0].clone();
            second_source.source_id = "<SOURCE_ID_B>".to_string();
            second_source.data_client_id = ClientId::from("<DATA_CLIENT_ID_B>");
            surface.sources.push(second_source);
            loaded.root.clients.insert(
                "<DATA_CLIENT_ID_B>".to_string(),
                ClientBlock {
                    venue: Venue::from(RV_DATA_CLIENT_VENUE),
                    data: Some(toml::Value::Table(toml::map::Map::new())),
                    execution: None,
                    secrets: None,
                    readiness_probe: None,
                },
            );
            insert_realized_volatility_surface(&mut loaded.root, surface);
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<surface_id>".to_string());
        },
        "distinct data_client_id",
    );
}

#[test]
fn realized_volatility_validation_rejects_empty_source_list() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources.clear();
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "sources",
    );
}

#[test]
fn realized_volatility_validation_rejects_quorum_larger_than_enabled_sources() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.policy.min_ready_sources = 2;
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "min_ready_sources",
    );
}

#[test]
fn realized_volatility_validation_rejects_sampling_interval_larger_than_window() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.policy.window_ms = surface.policy.sampling_interval_ms - 1;
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "window_ms",
    );
}

#[test]
fn realized_volatility_validation_rejects_mark_sources_for_taker_surface() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].source_class = RealizedVolatilitySourceClassBlock::Mark;
            surface.sources[0].sample_kind = RealizedVolatilitySampleKindBlock::Mark;
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "source_class",
    );
}

#[test]
fn realized_volatility_validation_rejects_mismatched_source_sample_pair() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources[0].sample_kind = RealizedVolatilitySampleKindBlock::Trade;
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "sample_kind",
    );
}

#[test]
fn realized_volatility_validation_rejects_mixed_enabled_quorum_source_contracts() {
    assert_realized_volatility_validation_error(
        |loaded| {
            let mut surface = valid_realized_volatility_surface();
            surface.sources.push(RealizedVolatilitySourceBlock {
                source_id: "<SOURCE_ID_B>".to_string(),
                source_class: RealizedVolatilitySourceClassBlock::Trade,
                sample_kind: RealizedVolatilitySampleKindBlock::Trade,
                ..surface.sources[0].clone()
            });
            insert_realized_volatility_surface(&mut loaded.root, surface);
        },
        "source_class",
    );
}

#[test]
fn realized_volatility_validation_rejects_strategy_missing_surface_reference() {
    assert_realized_volatility_validation_error(
        |loaded| {
            loaded.strategies[0].config.realized_volatility_surface_id =
                Some("<missing_surface_id>".to_string());
        },
        "realized_volatility_surface_id",
    );
}

#[test]
fn runtime_mapping_emits_surface_id_and_signal_data_for_surfaced_mode() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    insert_realized_volatility_surface(&mut loaded.root, valid_realized_volatility_surface());
    loaded.strategies[0].config.realized_volatility_surface_id = Some("<surface_id>".to_string());

    let strategy = loaded.strategies.first().expect("fixture strategy");
    let expected_signal_data = strategy
        .config
        .signal_data
        .values()
        .next()
        .expect("fixture strategy should include signal data");
    let expected_signal_venue = expected_signal_data.data_client_id.to_string();
    let expected_signal_instrument = expected_signal_data.instrument_id.to_string();
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("surface id should map into runtime config");
    let table = raw.as_table().expect("runtime config should be a table");

    assert_eq!(
        table
            .get("realized_volatility_surface_id")
            .and_then(toml::Value::as_str),
        Some("<surface_id>")
    );
    assert!(!table.contains_key("vol_window_secs"));
    assert!(!table.contains_key("vol_gap_reset_secs"));
    assert!(!table.contains_key("vol_min_observations"));
    assert!(!table.contains_key("vol_bridge_valid_secs"));
    assert_eq!(
        table.get("signal_venue").and_then(toml::Value::as_str),
        Some(expected_signal_venue.as_str()),
        "surfaced RV mode still needs signal data for fast-spot pricing"
    );
    assert_eq!(
        table
            .get("signal_instrument_id")
            .and_then(toml::Value::as_str),
        Some(expected_signal_instrument.as_str()),
        "surfaced RV mode must not remove the fast-spot instrument binding"
    );
}

#[test]
fn runtime_mapping_omits_strategy_local_submit_orders_switch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    insert_realized_volatility_surface(&mut loaded.root, valid_realized_volatility_surface());
    loaded.strategies[0].config.realized_volatility_surface_id = Some("<surface_id>".to_string());

    let raw = binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[0], &loaded)
        .expect("runtime config should map without stale strategy-local execution policy");

    assert!(
        !raw.as_table()
            .expect("runtime config should be a table")
            .contains_key("submit_orders"),
        "runtime config must not retain stale strategy-local execution policy"
    );
}

#[test]
fn surfaced_runtime_config_builds_without_legacy_realized_volatility_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    insert_realized_volatility_surface(&mut loaded.root, valid_realized_volatility_surface());
    loaded.strategies[0].config.realized_volatility_surface_id = Some("<surface_id>".to_string());

    let raw = binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[0], &loaded)
        .expect("surface id should map into runtime config");
    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
    assert!(
        errors.is_empty(),
        "surfaced runtime config should validate without legacy RV fields: {errors:?}"
    );

    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("surfaced runtime config should build without legacy RV fields");
}

#[test]
fn bolt_v3_registers_configured_strategy_through_runtime_binding_table() {
    fn register_stub(
        node: &mut LiveNode,
        context: bolt_v2::bolt_v3_strategy_registration::StrategyRegistrationContext<'_>,
    ) -> Result<StrategyId, bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError>
    {
        assert_eq!(context.strategy_kind, "stub_runtime_strategy");
        let permit = context
            .submit_admission
            .admit(&submit_request(Decimal::new(1, 0)))
            .map_err(|error| {
                bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError::Binding {
                    strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
                    strategy_archetype: context
                        .strategy
                        .config
                        .strategy_archetype
                        .as_str()
                        .to_string(),
                    message: format!("submit admission admit failed: {error:?}"),
                }
            })?;
        permit.commit_submitted();
        let strategy_id = StrategyId::from("BOLT-V3-PHASE3-BINDING");
        node.add_strategy(support::stub_runtime_strategy::StubRuntimeStrategy::new(
            strategy_id.as_str(),
        ))
        .map_err(|source| {
            bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError::Binding {
                strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
                strategy_archetype: context
                    .strategy
                    .config
                    .strategy_archetype
                    .as_str()
                    .to_string(),
                message: source.to_string(),
            }
        })?;
        Ok(strategy_id)
    }

    fn stub_strategy_kind() -> &'static str {
        "stub_runtime_strategy"
    }

    const TEST_BINDINGS: &[bolt_v2::bolt_v3_strategy_registration::StrategyRuntimeBinding] = &[
        bolt_v2::bolt_v3_strategy_registration::StrategyRuntimeBinding {
            key: "binary_oracle_edge_taker",
            strategy_kind: stub_strategy_kind,
            register: register_stub,
        },
    ];

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-binding-decision-evidence");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let mut empty_loaded = loaded.clone();
    empty_loaded.strategies.clear();
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve");
    let decision_evidence: Arc<
        dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    > = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence.clone()));
    let execution_controls =
        bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyExecutionControls {
            submit_admission: admission.clone(),
            order_execution_policy:
                bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        };
    let adapters =
        map_bolt_v3_adapters(&loaded, &resolved).expect("fixture adapters should map cleanly");
    let builder = make_bolt_v3_live_node_builder(&empty_loaded)
        .expect("v3 LiveNodeBuilder should construct before strategy registration");
    let (builder, _summary) = register_bolt_v3_clients(builder, adapters)
        .expect("fixture data clients should register before strategy registration");
    let mut node = builder
        .build()
        .expect("v3 LiveNode should build before strategy registration");

    let summary =
        bolt_v2::bolt_v3_strategy_registration::register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            &loaded,
            &resolved,
            TEST_BINDINGS,
            execution_controls,
            decision_evidence.clone(),
        )
        .expect("configured strategy should register through matching runtime binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(admission.admitted_order_count(), 1);
    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("BOLT-V3-PHASE3-BINDING")]
    );
}

#[test]
fn non_runtime_strategy_registration_rejects_iv_enabled_config() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve");
    let mut empty_loaded = loaded.clone();
    empty_loaded.strategies.clear();
    let mut node = make_bolt_v3_live_node_builder(&empty_loaded)
        .expect("v3 LiveNodeBuilder should construct before strategy registration")
        .build()
        .expect("v3 LiveNode should build before strategy registration");
    loaded.root.iv = Some(IvRootConfig {
        schema_version: 1,
        profiles: Vec::new(),
    });
    let decision_evidence: Arc<
        dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    > = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence.clone()));
    let execution_controls =
        bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyExecutionControls {
            submit_admission: admission,
            order_execution_policy:
                bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        };

    let error =
        bolt_v2::bolt_v3_strategy_registration::register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            &loaded,
            &resolved,
            &[],
            execution_controls,
            decision_evidence,
        )
        .expect_err("IV configs must use runtime-backed strategy registration");

    assert!(matches!(
        error,
        bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
            message
        } if message.contains("runtime-backed")
    ));
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(1, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
        admission_evidence: None,
    }
}

#[test]
fn complete_set_runtime_binding_and_production_registry_are_active_after_source_integrity() {
    assert!(
        bolt_v2::strategy_bindings::production_validation_bindings()
            .iter()
            .any(|binding| binding.key == complete_set_arbitrage::KEY),
        "complete-set archetype validation binding should be active after OUTCOME_GROUP_KEY"
    );
    let runtime = bolt_v2::strategy_bindings::production_runtime_bindings()
        .iter()
        .find(|binding| binding.key == complete_set_arbitrage::KEY)
        .expect("complete-set runtime binding should be active");
    assert_eq!(
        (runtime.strategy_kind)(),
        CompleteSetArbitrageBuilder::kind()
    );

    let registry = production_strategy_registry().expect("production registry should build");
    assert!(
        registry.get(CompleteSetArbitrageBuilder::kind()).is_some(),
        "complete-set builder should be in the production strategy registry"
    );
}

#[test]
fn complete_set_runtime_mapping_produces_strategy_shell_raw_config() {
    let (_temp, loaded) = complete_set_runtime_fixture();
    let strategy = loaded
        .strategies
        .first()
        .expect("complete-set fixture should include one strategy");

    let raw = complete_set_arbitrage::raw_complete_set_config(strategy, &loaded)
        .expect("complete-set strategy should map into concrete raw config");

    let mut errors: Vec<ValidationError> = Vec::new();
    CompleteSetArbitrageBuilder::validate_config(
        &raw,
        "strategies.complete_set_arb_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "mapped complete-set config should validate: {errors:?}"
    );

    let table = raw
        .as_table()
        .expect("mapped complete-set config should be a table");
    assert_eq!(
        table.get("strategy_id").and_then(|value| value.as_str()),
        Some("complete_set_arbitrage-901")
    );
    assert_eq!(
        table.get("client_id").and_then(|value| value.as_str()),
        Some("polymarket_main")
    );
    assert_eq!(
        table.get("submit_mode").and_then(|value| value.as_str()),
        Some("ioc")
    );
    assert_eq!(
        table
            .get("market_exit_reduce_only")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        table
            .get("max_open_baskets")
            .and_then(|value| value.as_integer()),
        Some(1)
    );
}

#[test]
fn complete_set_live_node_build_registers_strategy_from_strategy_files_after_source_integrity() {
    let (_temp, loaded) = complete_set_runtime_fixture();

    let (node, summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("complete-set LiveNode build should register after source-integrity coverage");

    assert_eq!(summary.clients.len(), 2);
    assert!(
        summary.clients.contains_key("okx_data"),
        "RV source client must be retained in the strategy transport"
    );
    assert!(
        summary.clients.contains_key("polymarket_main"),
        "complete-set strategy client must remain registered"
    );
    assert_eq!(
        node.registered_strategy_ids(),
        vec![StrategyId::from("complete_set_arbitrage-901")]
    );
}

fn complete_set_runtime_fixture() -> (
    support::TempCaseDir,
    bolt_v2::bolt_v3_config::LoadedBoltV3Config,
) {
    let temp = support::TempCaseDir::new("bolt-v3-complete-set-runtime");
    let strategy_dir = temp.path().join("strategies");
    fs::create_dir_all(&strategy_dir).expect("complete-set strategy dir should be created");
    let root_path = temp.path().join("root.toml");
    let strategy_path = strategy_dir.join("complete_set.toml");
    let root = complete_set_root_toml();
    fs::write(&root_path, root).expect("complete-set temp root should be written");
    fs::write(&strategy_path, complete_set_strategy_toml())
        .expect("complete-set strategy file should be written");
    let mut loaded = load_bolt_v3_config(&root_path).expect("complete-set fixture should load");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    (temp, loaded)
}

fn complete_set_root_toml() -> String {
    let mut fixture = support::repo_text("tests/fixtures/bolt_v3/root.toml").replace(
        "order_execution_mode = \"live\"",
        "order_execution_mode = \"shadow\"",
    );
    fixture = fixture.replace(
        "strategy_files = [\n  \"strategies/binary_oracle.toml\",\n]",
        "strategy_files = [\n  \"strategies/complete_set.toml\",\n]",
    );
    let gate_provider_start = fixture
        .find("\n[gate_providers.resolution_oracle_primary]\n")
        .expect("fixture root should include binary-oracle gate provider");
    let gate_provider_end = fixture
        .find("\n[clients.polymarket_main]\n")
        .expect("fixture root should include polymarket client block");
    fixture.replace_range(gate_provider_start..gate_provider_end, "\n");
    format!(
        "{fixture}\n{}\n{}",
        outcome_group_basket_execution_toml(),
        valid_polymarket_event_source_toml()
    )
}

fn outcome_group_basket_execution_toml() -> String {
    r#"
[risk.basket_execution]
enabled = true
state_path = "bolt-v3/baskets/state.json"
schema_version = 1
max_state_file_bytes = 1048576
recovery_policy = "fail_closed_reconcile_before_new_baskets"
max_recovery_age_ms = 300000
max_metadata_age_ms = 7200000

[risk.basket_execution.repair]
max_retries = 2
max_book_age_ms = 250
max_slippage_bps = 50
max_depth_levels = 4

[risk.basket_execution.unwind]
max_retries = 2
max_book_age_ms = 250
max_slippage_bps = 50
max_depth_levels = 4
"#
    .to_string()
}

fn complete_set_strategy_toml() -> String {
    r#"
schema_version = 2
strategy_instance_id = "complete_set_arb_main"
strategy_archetype = "complete_set_arbitrage"
order_id_tag = "901"
oms_type = "netting"
use_uuid_client_order_ids = true
use_hyphens_in_client_order_ids = false
external_order_claims = []
manage_contingent_orders = false
manage_gtd_expiry = false
manage_stop = false
market_exit_interval_ms = 100
market_exit_max_attempts = 100
market_exit_reduce_only = true
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
execution_client_id = "polymarket_main"

[target]
configured_target_id = "complete_set_arb_target"
kind = "static_outcome_group"
rotating_market_family = "outcome_group"
group_sources = ["poly_world_cup"]

[signal_data]

[parameters.runtime]
min_edge_bps = 25
max_basket_notional = "10"
max_open_baskets = 1
submit_mode = "ioc"
vwap_depth_limit_bps = 2000
slippage_buffer_bps = 100
max_repair_attempts = 1
max_unwind_attempts = 1
"#
    .to_string()
}

fn valid_polymarket_event_source_toml() -> String {
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    format!(
        r#"
[[outcome_group_sources]]
source_id = "poly_world_cup"
client_id = "polymarket_main"
kind = "polymarket_gamma_event"
event_slugs = ["world-cup-final"]
sports_market_types = ["moneyline"]
expected_neg_risk_market_id = "neg-risk-123"
terminal_state_labels = ["home", "draw", "away"]
max_markets = 20
enabled = true

[outcome_group_sources.freshness]
max_age_ms = 500
max_clock_skew_ms = 250

[outcome_group_sources.order_constraints]
default_min_quantity = "5"
default_min_notional = "1"

[outcome_group_sources.role_bindings]
kind = "operator_attested_positive_side"
attestation_sha256 = "{digest}"
legs = [
  {{ terminal_state_label = "home", pays_on_terminal_state_native_leg_id = "home-positive", pays_unless_terminal_state_native_leg_id = "home-inverse" }},
  {{ terminal_state_label = "draw", pays_on_terminal_state_native_leg_id = "draw-positive", pays_unless_terminal_state_native_leg_id = "draw-inverse" }},
  {{ terminal_state_label = "away", pays_on_terminal_state_native_leg_id = "away-positive", pays_unless_terminal_state_native_leg_id = "away-inverse" }},
]

[outcome_group_sources.settlement_rules]
settlement_contract_id = "ctf-world-cup-final"
settlement_source_kind = "polymarket_ctf_uma"
terminal_state_convention = "exactly_one_winner"
void_policy = "refund_all_legs"
rounding_policy = "decimal_exact"
timing_policy = "venue_final_resolution"
attestation_sha256 = "{digest}"

[outcome_group_sources.settlement_rules.non_standard_terminal_payouts.void_refund]
convention = "operator_attested_static_payout_per_unit"
terminal_state_label = "void_refund"
legs = [
  {{ outcome_label = "home", side_label = "operator-positive", payout_per_unit = "1" }},
  {{ outcome_label = "home", side_label = "operator-inverse", payout_per_unit = "1" }},
  {{ outcome_label = "draw", side_label = "operator-positive", payout_per_unit = "1" }},
  {{ outcome_label = "draw", side_label = "operator-inverse", payout_per_unit = "1" }},
  {{ outcome_label = "away", side_label = "operator-positive", payout_per_unit = "1" }},
  {{ outcome_label = "away", side_label = "operator-inverse", payout_per_unit = "1" }},
]
attestation_sha256 = "{digest}"
"#
    )
}

#[test]
fn binary_oracle_runtime_mapping_produces_existing_taker_raw_config() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");

    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy should map into existing taker raw config");

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );

    assert!(
        errors.is_empty(),
        "mapped taker config should validate: {errors:?}"
    );
    let table = raw
        .as_table()
        .expect("mapped raw taker config should be a table");
    assert_eq!(
        table.get("strategy_id").and_then(|value| value.as_str()),
        Some("binary_oracle_edge_taker-001")
    );
    assert_eq!(
        table.get("order_id_tag").and_then(|value| value.as_str()),
        Some("001")
    );
    assert_eq!(
        table.get("oms_type").and_then(|value| value.as_str()),
        Some("netting")
    );
    assert_eq!(
        table.get("client_id").and_then(|value| value.as_str()),
        Some("polymarket_main")
    );
    assert!(
        !table.contains_key("reference_venue"),
        "decision_reference is a logical source gate, not an NT quote venue"
    );
    assert!(
        !table.contains_key("reference_instrument_id"),
        "decision_reference is a logical source gate, not an NT quote instrument"
    );
    assert!(
        !table.contains_key("reference_publish_topic"),
        "reference input must come from configured reference_current_price, not a bolt msgbus topic"
    );
    assert_eq!(
        table
            .get("price_to_beat_source")
            .and_then(|value| value.as_str()),
        Some("chainlink_data_streams.configured-reference-price")
    );
    assert_eq!(
        table
            .get("cadence_seconds")
            .and_then(|value| value.as_integer()),
        Some(300)
    );
    assert_eq!(
        table
            .get("configured_target_id")
            .and_then(|value| value.as_str()),
        Some("configured_updown_target")
    );
    assert_eq!(
        table.get("target_kind").and_then(|value| value.as_str()),
        Some("rotating_market")
    );
    assert_eq!(
        table
            .get("rotating_market_family")
            .and_then(|value| value.as_str()),
        Some("updown")
    );
    assert_eq!(
        table
            .get("underlying_asset")
            .and_then(|value| value.as_str()),
        Some("CONFIGURED_ASSET")
    );
    assert_eq!(
        table
            .get("cadence_slug_token")
            .and_then(|value| value.as_str()),
        Some("5m")
    );
    assert_eq!(
        table
            .get("market_selection_rule")
            .and_then(|value| value.as_str()),
        Some("active_or_next")
    );
    assert_eq!(
        table
            .get("retry_interval_seconds")
            .and_then(|value| value.as_integer()),
        Some(5)
    );
    assert_eq!(
        table
            .get("blocked_after_seconds")
            .and_then(|value| value.as_integer()),
        Some(60)
    );
    assert_eq!(
        table
            .get("warmup_tick_count")
            .and_then(|value| value.as_integer()),
        Some(20)
    );
    assert_eq!(
        table
            .get("entry_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("order_type"))
            .and_then(|value| value.as_str()),
        Some("market")
    );
    assert_eq!(
        table
            .get("entry_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("time_in_force"))
            .and_then(|value| value.as_str()),
        Some("fok")
    );
    assert_eq!(
        table
            .get("entry_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("is_quote_quantity"))
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        table
            .get("exit_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("order_type"))
            .and_then(|value| value.as_str()),
        Some("market")
    );
    assert_eq!(
        table
            .get("exit_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("time_in_force"))
            .and_then(|value| value.as_str()),
        Some("ioc")
    );
}

#[test]
fn binary_oracle_runtime_mapping_rejects_missing_signal_data_role() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    loaded.strategies[strategy_index].config.signal_data.clear();

    let error =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect_err("binary oracle strategy should reject missing signal_data role");
    let rendered = error.to_string();
    assert!(
        rendered.contains("signal_data") && rendered.contains("requires exactly one"),
        "rejection should explain that signal_data is required, got: {rendered}"
    );
}

#[test]
fn binary_oracle_runtime_mapping_uses_target_resolution_mapping_without_chainlink_special_case() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let provider = loaded
        .root
        .gate_providers
        .as_mut()
        .and_then(|providers| providers.get_mut("resolution_oracle_primary"))
        .expect("fixture should include a resolution provider");
    provider.provider_kind = Some("pyth".to_string());

    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let mapping = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("resolution"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|resolution| resolution.get_mut("market_mappings"))
        .and_then(toml::Value::as_array_mut)
        .and_then(|mappings| mappings.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include a resolution gate mapping");
    mapping.insert(
        "resolution_kind".to_string(),
        toml::Value::String("pyth".to_string()),
    );
    mapping.insert(
        "resolution_identity".to_string(),
        toml::Value::String("configured-pyth-resolution".to_string()),
    );

    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy should not require Chainlink in the archetype bridge");

    assert_eq!(
        raw.as_table()
            .and_then(|table| table.get("price_to_beat_source"))
            .and_then(|value| value.as_str()),
        Some("pyth.configured-pyth-resolution")
    );
}

#[test]
fn binary_oracle_runtime_mapping_rejects_decision_reference_resolution_identity_that_parses_as_instrument_id()
 {
    // decision_reference is a logical gate identity, not an NT market-data
    // instrument. Keep that boundary explicit so logical oracle admissibility
    // cannot be confused with physical reference-current-price sources.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let mapping = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("decision_reference"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|decision_reference| decision_reference.get_mut("market_mappings"))
        .and_then(toml::Value::as_array_mut)
        .and_then(|mappings| mappings.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include a decision_reference gate mapping");
    mapping.insert(
        "resolution_identity".to_string(),
        toml::Value::String("REFERENCE.SOURCE".to_string()),
    );

    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let error = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded).expect_err(
        "a decision_reference resolution_identity that parses as an NT InstrumentId must be rejected",
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("resolution_identity") && rendered.contains("REFERENCE.SOURCE"),
        "rejection should name the offending resolution_identity, got: {rendered}"
    );
}

#[test]
fn binary_oracle_runtime_mapping_rejects_post_only_gtc_entry_order_runtime_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));
    entry_order.insert("is_quote_quantity".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("post-only GTC entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("limit")
    );
    assert_eq!(
        entry.get("time_in_force").and_then(toml::Value::as_str),
        Some("gtc")
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        entry.get("is_reduce_only").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry
            .get("is_quote_quantity")
            .and_then(toml::Value::as_bool),
        Some(false)
    );

    assert_unsupported_executable_entry_order_shape(&raw, "PostOnlyGtc");
}

#[test]
fn binary_oracle_runtime_mapping_rejects_stop_market_entry_order_runtime_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("stop_market".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("last_price".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("StopMarket entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("stop_market")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );
    assert_eq!(
        entry.get("trigger_type").and_then(toml::Value::as_str),
        Some("last_price")
    );

    assert_unsupported_executable_entry_order_shape(&raw, "StopMarket");
}

#[test]
fn binary_oracle_runtime_mapping_rejects_market_if_touched_entry_order_runtime_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("market_if_touched".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("MarketIfTouched entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("market_if_touched")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );

    assert_unsupported_executable_entry_order_shape(&raw, "MarketIfTouched");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_market_if_touched_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("market_if_touched".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), toml::Value::Float(0.48));
    exit_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("mark_price".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("MarketIfTouched exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("market_if_touched")
    );
    assert_eq!(
        exit.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.48)
    );
    assert_eq!(
        exit.get("trigger_type").and_then(toml::Value::as_str),
        Some("mark_price")
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(false)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "MarketIfTouched exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("MarketIfTouched exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_rejects_trailing_stop_market_entry_order_runtime_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("trailing_stop_market".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("last_price".to_string()),
    );
    entry_order.insert("trailing_offset".to_string(), toml::Value::Float(2.5));
    entry_order.insert(
        "trailing_offset_type".to_string(),
        toml::Value::String("basis_points".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("TrailingStopMarket entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("trailing_stop_market")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );
    assert_eq!(
        entry.get("trigger_type").and_then(toml::Value::as_str),
        Some("last_price")
    );
    assert_eq!(
        entry.get("trailing_offset").and_then(toml::Value::as_float),
        Some(2.5)
    );
    assert_eq!(
        entry
            .get("trailing_offset_type")
            .and_then(toml::Value::as_str),
        Some("basis_points")
    );

    assert_unsupported_executable_entry_order_shape(&raw, "TrailingStopMarket");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_trailing_stop_market_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("trailing_stop_market".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("activation_price".to_string(), toml::Value::Float(0.48));
    exit_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("mark_price".to_string()),
    );
    exit_order.insert("trailing_offset".to_string(), toml::Value::Float(3.0));
    exit_order.insert(
        "trailing_offset_type".to_string(),
        toml::Value::String("ticks".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("TrailingStopMarket exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("trailing_stop_market")
    );
    assert_eq!(
        exit.get("activation_price").and_then(toml::Value::as_float),
        Some(0.48)
    );
    assert_eq!(
        exit.get("trigger_type").and_then(toml::Value::as_str),
        Some("mark_price")
    );
    assert_eq!(
        exit.get("trailing_offset").and_then(toml::Value::as_float),
        Some(3.0)
    );
    assert_eq!(
        exit.get("trailing_offset_type")
            .and_then(toml::Value::as_str),
        Some("ticks")
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "TrailingStopMarket exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("TrailingStopMarket exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_rejects_stop_limit_entry_order_runtime_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("stop_limit".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("StopLimit entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("stop_limit")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    assert_unsupported_executable_entry_order_shape(&raw, "StopLimit");
}

#[test]
fn binary_oracle_runtime_mapping_rejects_limit_if_touched_entry_order_runtime_shape() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit_if_touched".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.39));
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("LimitIfTouched entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("limit_if_touched")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.39)
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    assert_unsupported_executable_entry_order_shape(&raw, "LimitIfTouched");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_post_only_gtc_exit_order() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("post-only GTC exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("limit")
    );
    assert_eq!(
        exit.get("time_in_force").and_then(toml::Value::as_str),
        Some("gtc")
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        exit.get("is_reduce_only").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        exit.get("is_quote_quantity").and_then(toml::Value::as_bool),
        Some(false)
    );
}

#[test]
fn binary_oracle_runtime_mapping_preserves_stop_limit_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("stop_limit".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), toml::Value::Float(0.48));
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("StopLimit exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("stop_limit")
    );
    assert_eq!(
        exit.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.48)
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "StopLimit exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("StopLimit exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_limit_if_touched_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit_if_touched".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), toml::Value::Float(0.46));
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("LimitIfTouched exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("limit_if_touched")
    );
    assert_eq!(
        exit.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.46)
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "LimitIfTouched exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("LimitIfTouched exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_emits_reference_current_price_when_present() {
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture v3 config should load");
    loaded.root.clients.insert(
        "chainlink_reference".to_string(),
        reference_price_client_from_toml(
            r#"
venue = "CHAINLINK_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://streams.chain.link"
websocket_path = "/api/v1/ws"
transport_backend = "sockudo"
heartbeat_secs = 5
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = 0
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
        ),
    );
    loaded.root.clients.insert(
        "polyresearch_reference".to_string(),
        reference_price_client_from_toml(
            r#"
venue = "POLYRESEARCH_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://stream.polyresearch.example/reference"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = "unlimited"
subscribe_ack_timeout_ms = 2000
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/polyresearch/api-key"
"#,
        ),
    );

    let strategy_index = 0;
    loaded.strategies[strategy_index]
        .config
        .reference_current_price = Some(ReferencePriceBlock {
        asset: "BTC".to_string(),
        source_order: vec![
            "chainlink_primary".to_string(),
            "polyresearch_backup".to_string(),
        ],
        min_valid_sources: 1,
        selection_policy: ReferencePriceSelectionPolicy::FirstValidPerInterval,
        max_source_age_ms: 1500,
        max_source_drift_bps: 10,
        drift_policy: ReferencePriceDriftPolicy::Observe,
        stale_policy: ReferencePriceStalePolicy::Block,
        sources: BTreeMap::from([
            (
                "chainlink_primary".to_string(),
                ReferencePriceSourceBlock {
                    provider: ReferencePriceProvider::new("chainlink_ws")
                        .expect("test provider key should be valid"),
                    enabled: true,
                    required: false,
                    client_id: ClientId::from("chainlink_reference"),
                    instrument_id: Some("BTC-USD.CHAINLINK".to_string()),
                    symbol: None,
                },
            ),
            (
                "polyresearch_backup".to_string(),
                ReferencePriceSourceBlock {
                    provider: ReferencePriceProvider::new("polyresearch_ws")
                        .expect("test provider key should be valid"),
                    enabled: true,
                    required: false,
                    client_id: ClientId::from("polyresearch_reference"),
                    instrument_id: None,
                    symbol: Some("BTC".to_string()),
                },
            ),
        ]),
    });

    let strategy = &loaded.strategies[strategy_index];
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded).expect(
        "binary oracle strategy with reference_current_price should map into runtime config",
    );
    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "runtime config with reference_current_price should validate: {errors:?}"
    );

    let table = raw
        .as_table()
        .expect("binary oracle runtime config should be a table");
    let reference_current_price = table
        .get("reference_current_price")
        .and_then(toml::Value::as_table)
        .expect("runtime config should carry reference_current_price table");

    assert_eq!(
        reference_current_price
            .get("asset")
            .and_then(toml::Value::as_str),
        Some("BTC")
    );
    let source_order = reference_current_price
        .get("sources")
        .and_then(toml::Value::as_array)
        .expect("reference_current_price.sources should remain an ordered array");
    assert_eq!(
        source_order
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["chainlink_primary", "polyresearch_backup"]
    );
    assert!(
        table.get("resolution_client_id").is_none(),
        "reference_current_price must not imply resolution_data"
    );
}

#[test]
fn binary_oracle_runtime_mapping_allows_signal_data_with_decision_reference() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.insert(
        "binance_reference".to_string(),
        toml::from_str(&support::repo_text(
            "tests/fixtures/bolt_v3/binance_reference_client.toml",
        ))
        .expect("binance provider fixture client should parse"),
    );
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    loaded.strategies[strategy_index].config.signal_data.insert(
        "primary".to_string(),
        DataInstrumentBlock {
            data_client_id: ClientId::from("binance_reference"),
            instrument_id: InstrumentId::from("BTCUSDT.BINANCE"),
        },
    );

    let strategy = &loaded.strategies[strategy_index];
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("signal_data and decision_reference should be independent roles");
    let table = raw
        .as_table()
        .expect("mapped raw taker config should be a table");

    assert!(
        !table.contains_key("reference_venue"),
        "decision_reference is independent from signal_data and must not synthesize NT quote venue"
    );
    assert!(
        !table.contains_key("reference_instrument_id"),
        "decision_reference is independent from signal_data and must not synthesize NT quote instrument"
    );
    assert_eq!(
        table.get("signal_venue").and_then(|value| value.as_str()),
        Some("binance_reference")
    );
    assert_eq!(
        table
            .get("signal_instrument_id")
            .and_then(|value| value.as_str()),
        Some("BTCUSDT.BINANCE")
    );
}

#[test]
fn binary_oracle_runtime_mapping_emits_resolution_data_when_present() {
    // With a `[resolution_data]` block bound to the shipped `chainlink_strike`
    // client, the archetype emits `resolution_client_id` + `resolution_instrument_id`
    // into the runtime config, and the strategy builder validates them.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    add_root_chainlink_feed_binding(&mut loaded.root, "BTC-USD.CHAINLINK");
    // Align the target's underlying_asset with the BTC-USD.CHAINLINK root
    // feed_binding so the load-time resolution_data binding validation (asset
    // prefix + feed binding) passes for this happy-path emit test.
    loaded.strategies[strategy_index]
        .config
        .target
        .as_table_mut()
        .expect("fixture target should be a table")
        .insert(
            "underlying_asset".to_string(),
            toml::Value::String("BTC".to_string()),
        );
    loaded.strategies[strategy_index].config.resolution_data = Some(DataInstrumentBlock {
        data_client_id: ClientId::from("chainlink_strike"),
        instrument_id: InstrumentId::from("BTC-USD.CHAINLINK"),
    });

    let strategy = &loaded.strategies[strategy_index];
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy with resolution_data should map into runtime config");

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "runtime config with resolution_data should validate: {errors:?}"
    );

    let table = raw
        .as_table()
        .expect("mapped raw taker config should be a table");
    assert_eq!(
        table
            .get("resolution_client_id")
            .and_then(|value| value.as_str()),
        Some("chainlink_strike")
    );
    assert_eq!(
        table
            .get("resolution_instrument_id")
            .and_then(|value| value.as_str()),
        Some("BTC-USD.CHAINLINK")
    );
}

#[test]
fn binary_oracle_runtime_mapping_omits_resolution_data_when_absent() {
    // The shipped fixture strategy declares no `[resolution_data]`, so the
    // archetype emits neither resolution key and the strategy still validates.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    assert!(
        strategy.config.resolution_data.is_none(),
        "fixture strategy should not declare resolution_data"
    );

    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy without resolution_data should map into runtime config");

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "runtime config without resolution_data should validate: {errors:?}"
    );

    let table = raw
        .as_table()
        .expect("mapped raw taker config should be a table");
    assert!(
        !table.contains_key("resolution_client_id"),
        "resolution_client_id must be absent when [resolution_data] is omitted"
    );
    assert!(
        !table.contains_key("resolution_instrument_id"),
        "resolution_instrument_id must be absent when [resolution_data] is omitted"
    );
}

#[test]
fn binary_oracle_runtime_mapping_rejects_resolution_data_with_unknown_client() {
    // A `[resolution_data]` block whose data_client_id is not a loaded client
    // fails closed during runtime mapping (mirrors the signal_data existence check).
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    loaded.strategies[strategy_index].config.resolution_data = Some(DataInstrumentBlock {
        data_client_id: ClientId::from("not_a_configured_client"),
        instrument_id: InstrumentId::from("BTC-USD.CHAINLINK"),
    });

    let strategy = &loaded.strategies[strategy_index];
    let error = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect_err("resolution_data with an unknown data client must fail closed");
    let rendered = error.to_string();
    assert!(
        rendered.contains("resolution_data")
            && rendered.contains("not_a_configured_client")
            && rendered.contains("not present in loaded clients"),
        "unknown resolution data client should fail with a clear message, got: {rendered}"
    );
}

// DF-load-validate (#553): load-time validation of the `[resolution_data]`
// binding. A live-money strategy must NOT load if its resolution-strike binding
// is wrong; the existing runtime asset/subscribe guards only fire much later (at
// the first interval subscribe), which silently leaves `price_to_beat = None`
// instead of failing the operator's deploy. These three tests assert the
// archetype bridge `raw_taker_config` rejects, at load time, a `[resolution_data]`
// block whose:
//   (a) data_client_id resolves to a client whose venue is NOT the Chainlink
//       strike provider (CHAINLINK_DATA_STREAMS),
//   (b) instrument_id asset prefix does not match the target's underlying_asset,
//   (c) instrument_id has no matching feed_binding in that client.
// Today only client-existence is checked in `raw_taker_config`, so all three
// MUST fail until the load-time binding validation is added.

/// (a) The resolution_data client exists, but its venue is not the Chainlink
/// strike provider. Binding the strike source to a non-Chainlink venue must fail
/// closed at load time (the strike index-price subscribe only flows through the
/// Chainlink strike client).
#[test]
fn binary_oracle_runtime_mapping_rejects_resolution_data_with_non_chainlink_client_venue() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    // `okx_data` is a loaded client (venue = OKX), so the existence check passes,
    // but OKX is not the Chainlink strike provider.
    loaded.strategies[strategy_index].config.resolution_data = Some(DataInstrumentBlock {
        data_client_id: ClientId::from("okx_data"),
        instrument_id: InstrumentId::from("BTC-USD.CHAINLINK"),
    });

    let strategy = &loaded.strategies[strategy_index];
    let error = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded).expect_err(
        "resolution_data bound to a non-Chainlink client venue must fail closed at load time",
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("resolution_data")
            && rendered.contains("okx_data")
            && rendered.contains("CHAINLINK_DATA_STREAMS"),
        "non-Chainlink resolution client venue should fail with a clear message naming the client and the required Chainlink venue, got: {rendered}"
    );
}

/// (b) The resolution_data client is the Chainlink strike client and the
/// instrument exists as a feed_binding, but its asset prefix (`BTC`) does not
/// match the target's `underlying_asset` (`CONFIGURED_ASSET`). A wrong-asset
/// strike feed must fail closed at load time, not silently at subscribe time.
#[test]
fn binary_oracle_runtime_mapping_rejects_resolution_data_instrument_asset_mismatch() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    // The fixture target's underlying_asset is "CONFIGURED_ASSET"; the Chainlink
    // strike feed_binding instrument is "BTC-USD.CHAINLINK" (asset prefix "BTC").
    loaded.strategies[strategy_index].config.resolution_data = Some(DataInstrumentBlock {
        data_client_id: ClientId::from("chainlink_strike"),
        instrument_id: InstrumentId::from("BTC-USD.CHAINLINK"),
    });

    let strategy = &loaded.strategies[strategy_index];
    let error = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded).expect_err(
        "resolution_data instrument whose asset prefix does not match underlying_asset must fail closed at load time",
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("resolution_data")
            && rendered.contains("BTC-USD.CHAINLINK")
            && rendered.contains("CONFIGURED_ASSET"),
        "asset-mismatched resolution instrument should fail with a clear message naming the instrument and the underlying_asset, got: {rendered}"
    );
}

/// (c) The resolution_data client is the Chainlink strike client and the
/// underlying_asset matches the instrument's asset prefix, but the instrument has
/// no matching feed_binding in that client. A strike instrument with no feed_id
/// binding can never produce a report, so it must fail closed at load time.
#[test]
fn binary_oracle_runtime_mapping_rejects_resolution_data_instrument_without_feed_binding() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    // Make the target's underlying_asset match the instrument asset prefix (ETH)
    // so the asset-prefix check (b) passes and this test isolates the missing
    // feed_binding gap (c). The chainlink_strike client only binds
    // "BTC-USD.CHAINLINK", so "ETH-USD.CHAINLINK" has no feed_binding.
    loaded.strategies[strategy_index]
        .config
        .target
        .as_table_mut()
        .expect("fixture target should be a table")
        .insert(
            "underlying_asset".to_string(),
            toml::Value::String("ETH".to_string()),
        );
    loaded.strategies[strategy_index].config.resolution_data = Some(DataInstrumentBlock {
        data_client_id: ClientId::from("chainlink_strike"),
        instrument_id: InstrumentId::from("ETH-USD.CHAINLINK"),
    });

    let strategy = &loaded.strategies[strategy_index];
    let error = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded).expect_err(
        "resolution_data instrument with no matching feed_binding in the client must fail closed at load time",
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("resolution_data")
            && rendered.contains("ETH-USD.CHAINLINK")
            && rendered.contains("feed_binding"),
        "resolution instrument with no feed_binding should fail with a clear message naming the instrument and feed_binding, got: {rendered}"
    );
}

#[test]
fn binary_oracle_runtime_mapping_uses_market_family_target_projection() {
    let source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");

    assert!(
        !source.contains("updown::deserialize_target_block"),
        "binary_oracle_edge_taker runtime mapping must not deserialize an updown target directly"
    );
    assert!(
        source.contains("target_runtime_fields_from_target"),
        "binary_oracle_edge_taker runtime mapping should consume the market-family target projection"
    );
}

#[test]
fn bolt_v3_live_node_build_registers_configured_binary_oracle_strategy() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode build should register configured bolt-v3 strategies");

    assert_eq!(
        node.registered_strategy_ids(),
        vec![StrategyId::from("binary_oracle_edge_taker-001")]
    );
}

#[test]
fn binary_oracle_registration_resolves_fee_provider_through_provider_boundary() {
    let source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");
    assert!(
        source.contains("resolve_fee_provider"),
        "binary_oracle_edge_taker registration should call the generic fee-provider resolver"
    );

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-fee-provider-boundary");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("configured Polymarket strategy should register through provider boundary");

    assert_eq!(
        node.registered_strategy_ids(),
        vec![StrategyId::from("binary_oracle_edge_taker-001")]
    );
}

#[test]
fn fee_provider_resolution_does_not_warm_during_registration() {
    let resolver_source = include_str!("../src/bolt_v3_providers/mod.rs");
    let archetype_source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");

    assert!(
        !resolver_source.contains(".warm("),
        "fee-provider resolver must construct only; fee warm remains in strategy runtime readiness"
    );
    assert!(
        !archetype_source.contains(".warm("),
        "runtime registration must not warm fee providers"
    );
}

#[test]
fn binary_oracle_runtime_rejects_execution_client_id_without_execution_block() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence-data-only-exec-client");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let mut polymarket_data_only = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture should include polymarket_main")
        .clone();
    polymarket_data_only.execution = None;
    polymarket_data_only.secrets = None;
    loaded
        .root
        .clients
        .insert("polymarket_data_only".to_string(), polymarket_data_only);
    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    strategy.config.execution_client_id = "polymarket_data_only".into();

    let error =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect_err("data-only client must not be used for execution");

    let message = error.to_string();
    assert!(message.contains("polymarket_data_only"), "{message}");
    assert!(
        message.contains("is required by the existing taker fee-provider boundary"),
        "{message}"
    );
}

#[test]
fn fee_provider_source_fence_blocks_concrete_provider_in_shared_layers() {
    const SOURCE_FENCE_MAX_FILE_BYTES: u64 = 1024 * 1024;

    fn forbidden_fee_provider_reference(line: &str) -> bool {
        line.contains("bolt_v3_providers::polymarket")
            || line.contains("polymarket::")
            || line.contains("build_fee_provider")
    }

    fn source_contains_forbidden_fee_provider_reference(source: &str) -> bool {
        source.lines().any(forbidden_fee_provider_reference)
    }

    fn strip_rust_comments(source: &str) -> String {
        enum State {
            Code,
            LineComment,
            BlockComment,
            String { escaped: bool },
            RawString { hashes: usize },
        }

        fn raw_string_hashes_at(chars: &[char], index: usize) -> Option<usize> {
            if chars.get(index) != Some(&'r') {
                return None;
            }
            let mut cursor = index + 1;
            let mut hashes = 0;
            while chars.get(cursor) == Some(&'#') {
                hashes += 1;
                cursor += 1;
            }
            (chars.get(cursor) == Some(&'"')).then_some(hashes)
        }

        let chars = source.chars().collect::<Vec<_>>();
        let mut output = String::with_capacity(source.len());
        let mut state = State::Code;
        let mut index = 0;
        while let Some(&current) = chars.get(index) {
            match state {
                State::Code => {
                    if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'/') {
                        state = State::LineComment;
                        index += 2;
                    } else if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'*') {
                        state = State::BlockComment;
                        index += 2;
                    } else if let Some(hashes) = raw_string_hashes_at(&chars, index) {
                        output.push('r');
                        index += 1;
                        for _ in 0..hashes {
                            output.push('#');
                            index += 1;
                        }
                        output.push('"');
                        index += 1;
                        state = State::RawString { hashes };
                    } else if current == '"' {
                        output.push(current);
                        state = State::String { escaped: false };
                        index += 1;
                    } else {
                        output.push(current);
                        index += 1;
                    }
                }
                State::LineComment => {
                    if current == '\n' {
                        output.push(current);
                        state = State::Code;
                    }
                    index += 1;
                }
                State::BlockComment => {
                    if current == '\n' {
                        output.push(current);
                        index += 1;
                    } else if current == '*' && chars.get(index + 1) == Some(&'/') {
                        state = State::Code;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                State::String { escaped } => {
                    output.push(current);
                    state = if escaped {
                        State::String { escaped: false }
                    } else if current == '\\' {
                        State::String { escaped: true }
                    } else if current == '"' {
                        State::Code
                    } else {
                        State::String { escaped: false }
                    };
                    index += 1;
                }
                State::RawString { hashes } => {
                    output.push(current);
                    if current == '"' {
                        let closes_raw_string =
                            (1..=hashes).all(|offset| chars.get(index + offset) == Some(&'#'));
                        if closes_raw_string {
                            for offset in 1..=hashes {
                                output.push(chars[index + offset]);
                            }
                            index += hashes + 1;
                            state = State::Code;
                        } else {
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
            }
        }
        output
    }

    fn read_source_fence_target(repo_root: &std::path::Path, relative: &str) -> String {
        let path = repo_root.join(relative);
        let metadata = std::fs::metadata(&path).expect("source-fence target metadata should load");
        assert!(
            metadata.is_file(),
            "source-fence target must be a file: {relative}"
        );
        assert!(
            metadata.len() <= SOURCE_FENCE_MAX_FILE_BYTES,
            "source-fence target {relative} is {} bytes; limit is {SOURCE_FENCE_MAX_FILE_BYTES}",
            metadata.len()
        );
        std::fs::read_to_string(path).expect("source-fence target should be readable")
    }

    assert!(
        source_contains_forbidden_fee_provider_reference("let _ = polymarket::build_fee_provider;"),
        "positive control must catch direct concrete provider construction"
    );
    assert!(
        !source_contains_forbidden_fee_provider_reference(&strip_rust_comments(
            "// let _ = polymarket::build_fee_provider;"
        )),
        "negative control must ignore direct construction in line comments"
    );
    assert!(
        !source_contains_forbidden_fee_provider_reference(&strip_rust_comments(
            "/* let _ = polymarket::build_fee_provider; */"
        )),
        "negative control must ignore direct construction in block comments"
    );
    assert_eq!(
        strip_rust_comments("let text = \"// this is string content\";"),
        "let text = \"// this is string content\";",
        "comment stripping must not treat line-comment markers inside strings as comments"
    );

    fn push_rs_files(repo_root: &std::path::Path, directory: &str, files: &mut Vec<String>) {
        fn push_rs_files_from_path(
            repo_root: &std::path::Path,
            path: &std::path::Path,
            files: &mut Vec<String>,
        ) {
            for entry in std::fs::read_dir(path).expect("source-fence directory should be readable")
            {
                let entry = entry.expect("source-fence directory entry should be readable");
                let file_type = entry
                    .file_type()
                    .expect("source-fence directory entry type should be readable");
                let path = entry.path();
                if file_type.is_dir() {
                    push_rs_files_from_path(repo_root, &path, files);
                } else if file_type.is_file()
                    && path.extension().is_some_and(|extension| extension == "rs")
                {
                    files.push(
                        path.strip_prefix(repo_root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }

        push_rs_files_from_path(repo_root, &repo_root.join(directory), files);
    }

    let recursive_temp = support::TempCaseDir::new("fee-provider-source-fence-recursive");
    let nested_strategy_dir = recursive_temp.path().join("src/strategies/nested");
    std::fs::create_dir_all(&nested_strategy_dir)
        .expect("recursive source-fence control directory should be created");
    std::fs::write(nested_strategy_dir.join("mod.rs"), "")
        .expect("recursive source-fence control Rust file should be created");
    std::fs::write(nested_strategy_dir.join("notes.txt"), "")
        .expect("recursive source-fence control non-Rust file should be created");
    let mut recursive_control_files = Vec::new();
    push_rs_files(
        recursive_temp.path(),
        "src/strategies",
        &mut recursive_control_files,
    );
    recursive_control_files.sort();
    assert_eq!(
        recursive_control_files,
        vec!["src/strategies/nested/mod.rs".to_string()],
        "source-fence collection must recurse into nested strategy modules and ignore non-Rust files"
    );

    let repo_root = support::repo_path("");
    let mut files = Vec::new();
    push_rs_files(&repo_root, "src/bolt_v3_archetypes", &mut files);
    push_rs_files(&repo_root, "src/strategies", &mut files);
    files.extend([
        "src/bolt_v3_strategy_registration.rs".to_string(),
        "src/bolt_v3_submit_admission.rs".to_string(),
        "src/bolt_v3_order_intent.rs".to_string(),
    ]);

    let mut violations = Vec::new();
    files.sort();
    files.dedup();
    for relative in files {
        let source = strip_rust_comments(&read_source_fence_target(&repo_root, &relative));
        for (line_index, line) in source.lines().enumerate() {
            if forbidden_fee_provider_reference(line) {
                violations.push(format!("{}:{}", relative, line_index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "concrete provider construction leaked into shared registration layers: {violations:?}"
    );
}
