mod support;

use std::{
    cell::RefCell,
    fs,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use bolt_v2::{
    bolt_v3_config::{
        CatalogFsProtocol, LoadedBoltV3Config, PersistenceBlock, REFERENCE_STREAM_ID_PARAMETER,
        ReferenceSourceType, ReferenceStreamBlock, RotationKind, StreamingBlock,
        load_bolt_v3_config,
    },
    bolt_v3_decision_event_context::{
        BoltV3DecisionEventCommonContext, bolt_v3_decision_event_common_context,
    },
    bolt_v3_decision_events::{
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
        BOLT_V3_ENTRY_NO_ACTION_ACTIVE_BOOK_NOT_PRICED_REASON,
        BOLT_V3_ENTRY_NO_ACTION_FAIR_PROBABILITY_UNAVAILABLE_REASON,
        BOLT_V3_ENTRY_NO_ACTION_FAST_VENUE_INCOHERENT_REASON,
        BOLT_V3_ENTRY_NO_ACTION_FEE_RATE_UNAVAILABLE_REASON, BOLT_V3_ENTRY_NO_ACTION_FREEZE_REASON,
        BOLT_V3_ENTRY_NO_ACTION_INSUFFICIENT_EDGE_REASON,
        BOLT_V3_ENTRY_NO_ACTION_MISSING_REFERENCE_QUOTE_REASON,
        BOLT_V3_ENTRY_NO_ACTION_POSITION_LIMIT_REACHED_REASON,
        BOLT_V3_ENTRY_NO_ACTION_STALE_REFERENCE_QUOTE_REASON,
        BOLT_V3_ENTRY_NO_ACTION_THIN_BOOK_REASON,
        BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON,
        BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
        BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
        BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INSTRUMENT_MISSING_FROM_CACHE_REASON,
        BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON,
        BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_TRADING_STATE_HALTED_REASON,
        BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_TRADING_STATE_REDUCING_REASON,
        BOLT_V3_EXIT_DECISION_ORDER_MECHANICAL_REJECTION_REASON,
        BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_PRICE_MISSING_REASON,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_QUANTITY_EXCEEDS_SELLABLE_QUANTITY_REASON,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_TRADING_STATE_HALTED_REASON,
        BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE, BOLT_V3_MARKET_SELECTION_FAILURE_REASONS,
        BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON,
        BoltV3EntryEvaluationDecisionEvent, BoltV3EntryOrderSubmissionDecisionEvent,
        BoltV3EntryPreSubmitRejectionDecisionEvent, BoltV3ExitEvaluationDecisionEvent,
        BoltV3ExitOrderSubmissionDecisionEvent, BoltV3ExitPreSubmitRejectionDecisionEvent,
        BoltV3MarketSelectionDecisionEvent,
    },
    bolt_v3_market_families::updown,
    bolt_v3_release_identity::load_bolt_v3_release_identity,
    bolt_v3_strategy_decision_evidence::BoltV3StrategyDecisionEvidence,
    config::Config,
    live_node_setup::{
        DataClientRegistration, ExecClientRegistration, build_live_node, make_live_node_config,
        make_strategy_build_context,
    },
    platform::{
        reference::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind},
        ruleset::{
            CandidateMarket, RuntimeSelectionSnapshot, SELECTION_FREEZE_WINDOW_REASON,
            SelectionDecision, SelectionState,
        },
        runtime::{registry_runtime_strategy_factory, runtime_selection_topic},
    },
    strategies::registry::BoltV3MarketSelectionContext,
    strategies::{
        eth_chainlink_taker::{ETH_CHAINLINK_TAKER_KIND, EthChainlinkTaker},
        production_strategy_registry,
    },
};
use nautilus_common::{
    actor::{DataActor, registry::try_get_actor_unchecked},
    cache::Cache,
    enums::Environment,
    logging::logger::LoggerConfig,
    msgbus::{publish_any, publish_deltas, publish_order_event, switchboard},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, OrderBookDeltas},
    enums::{
        AssetClass, BookAction, LiquiditySide, OmsType, OrderSide, OrderType, PositionSide,
        TradingState,
    },
    events::{OrderAccepted, OrderEventAny, OrderFilled},
    identifiers::{
        AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, TradeId,
        TraderId, VenueOrderId,
    },
    instruments::{InstrumentAny, binary_option::BinaryOption},
    orders::{Order, OrderAny, OrderTestBuilder},
    position::Position,
    types::{Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_trading::Strategy;
use rust_decimal::prelude::ToPrimitive;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_exec_submissions, recorded_mock_exec_submissions,
};
use tempfile::TempDir;
use tokio::time::sleep;
use toml::Value;
#[derive(Debug, Default)]
struct StaticFeeProvider;

#[derive(Debug, Default)]
struct MissingFeeProvider;

static RUNTIME_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
static TEST_NODE_CONFIG: OnceLock<Config> = OnceLock::new();
const RUNTIME_DEFAULT_SELECTED_MARKET: &str = "runtime_default";
const RUNTIME_ROTATION_B_SELECTED_MARKET: &str = "runtime_rotation_b";
const RUNTIME_RECOVERY_A_SELECTED_MARKET: &str = "runtime_recovery_a";
const RUNTIME_RECOVERY_B_SELECTED_MARKET: &str = "runtime_recovery_b";

fn runtime_test_mutex() -> &'static Mutex<()> {
    RUNTIME_TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

fn test_node_config() -> &'static Config {
    TEST_NODE_CONFIG.get_or_init(|| {
        let fixture_path =
            support::repo_path("tests/fixtures/eth_chainlink_taker_runtime/test_node.toml");
        let toml = fs::read_to_string(fixture_path).expect("test node fixture should read");
        toml::from_str(&toml).expect("test node fixture should parse")
    })
}

fn config_str<'a>(config: &'a Value, key: &str) -> &'a str {
    config
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("test node fixture config must include {key}"))
}

fn test_data_client_name() -> &'static str {
    test_node_config()
        .data_clients
        .first()
        .expect("test node fixture should include data client")
        .name
        .as_str()
}

fn test_data_client_venue() -> &'static str {
    let client = test_node_config()
        .data_clients
        .first()
        .expect("test node fixture should include data client");
    config_str(&client.config, "venue")
}

fn test_exec_client_name() -> &'static str {
    test_node_config()
        .exec_clients
        .first()
        .expect("test node fixture should include exec client")
        .name
        .as_str()
}

fn test_exec_client_venue() -> &'static str {
    let client = test_node_config()
        .exec_clients
        .first()
        .expect("test node fixture should include exec client");
    config_str(&client.config, "venue")
}

fn test_exec_account_id_str() -> &'static str {
    let client = test_node_config()
        .exec_clients
        .first()
        .expect("test node fixture should include exec client");
    config_str(&client.config, "account_id")
}

fn test_trader_id() -> TraderId {
    TraderId::from(test_node_config().node.trader_id.as_str())
}

fn test_exec_client_id() -> ClientId {
    ClientId::from(test_exec_client_name())
}

fn test_account_id() -> AccountId {
    AccountId::from(test_exec_account_id_str())
}

impl bolt_v2::clients::polymarket::FeeProvider for StaticFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<rust_decimal::Decimal> {
        Some(rust_decimal::Decimal::ZERO)
    }

    fn warm(&self, _token_id: &str) -> futures_util::future::BoxFuture<'_, anyhow::Result<()>> {
        use futures_util::FutureExt;
        async { Ok(()) }.boxed()
    }
}

impl bolt_v2::clients::polymarket::FeeProvider for MissingFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<rust_decimal::Decimal> {
        None
    }

    fn warm(&self, _token_id: &str) -> futures_util::future::BoxFuture<'_, anyhow::Result<()>> {
        use futures_util::FutureExt;
        async { Ok(()) }.boxed()
    }
}

fn build_test_node() -> LiveNode {
    let cfg = test_node_config();
    let trader_id = test_trader_id();
    let data_config = MockDataClientConfig::new(test_data_client_name(), test_data_client_venue());
    let exec_config = MockExecClientConfig::new(
        test_exec_client_name(),
        test_exec_account_id_str(),
        test_exec_client_venue(),
    );

    let live_node_config =
        make_live_node_config(cfg, trader_id, Environment::Live, LoggerConfig::default());
    let data_clients: Vec<DataClientRegistration> = vec![(
        Some(test_data_client_name().to_string()),
        Box::new(MockDataClientFactory),
        Box::new(data_config),
    )];
    let exec_clients: Vec<ExecClientRegistration> = vec![(
        Some(test_exec_client_name().to_string()),
        Box::new(MockExecutionClientFactory),
        Box::new(exec_config),
    )];

    build_live_node(
        cfg.node.name.clone(),
        live_node_config,
        data_clients,
        exec_clients,
    )
    .unwrap()
}

fn strategy_raw_config() -> Value {
    let mut config: Value = toml::toml! {
        strategy_id = "ETHCHAINLINKTAKER-RT-001"
        warmup_tick_count = 1
        period_duration_secs = 300
        reentry_cooldown_secs = 30
        max_position_usdc = 1000.0
        book_impact_cap_bps = 15
        risk_lambda = 0.0
        worst_case_ev_min_bps = -20
        exit_hysteresis_bps = 5
        vol_window_secs = 60
        vol_gap_reset_secs = 10
        vol_min_observations = 1
        vol_bridge_valid_secs = 10
        pricing_kurtosis = 0.0
        theta_decay_factor = 0.0
        forced_flat_stale_chainlink_ms = 1500
        forced_flat_thin_book_min_liquidity = 100.0
        lead_agreement_min_corr = 0.8
        lead_jitter_max_ms = 250
    }
    .into();
    config.as_table_mut().unwrap().insert(
        "client_id".to_string(),
        Value::String(test_exec_client_name().to_string()),
    );
    config
}

fn strategy_id_from_raw_config(config: &Value) -> StrategyId {
    StrategyId::from(
        config
            .get("strategy_id")
            .and_then(Value::as_str)
            .expect("test strategy config must include strategy_id"),
    )
}

fn strategy_id_from_fixture_config() -> StrategyId {
    strategy_id_from_raw_config(&strategy_raw_config())
}

fn strategy_id_from_multi_fixture(strategy_index: usize) -> StrategyId {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root_multi.toml",
    ))
    .expect("existing-strategy multi root fixture should load");
    StrategyId::from(
        loaded
            .strategies
            .get(strategy_index)
            .expect("multi root fixture should include requested strategy")
            .config
            .strategy_instance_id
            .as_str(),
    )
}

fn existing_strategy_loaded_config() -> LoadedBoltV3Config {
    load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy root fixture should load")
}

fn fixture_reference_stream() -> ReferenceStreamBlock {
    let loaded = existing_strategy_loaded_config();
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy root fixture should include strategy");
    let stream_id = strategy
        .config
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("existing strategy should select reference stream from TOML");
    loaded
        .root
        .reference_streams
        .get(stream_id)
        .cloned()
        .expect("selected reference stream should exist")
}

fn fixture_reference_publish_topic() -> String {
    fixture_reference_stream().publish_topic
}

fn fixture_resolution_basis_key() -> String {
    let loaded = existing_strategy_loaded_config();
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy root fixture should include strategy");
    let stream_id = strategy
        .config
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("existing strategy should select reference stream from TOML");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist");
    let oracle_input = stream
        .inputs
        .iter()
        .find(|input| input.source_type == ReferenceSourceType::Oracle)
        .expect("selected reference stream should include oracle input");
    let oracle_client_id = oracle_input
        .data_client_id
        .as_deref()
        .expect("oracle input should reference data client");
    let oracle_client = loaded
        .root
        .clients
        .get(oracle_client_id)
        .expect("oracle data client should exist");
    let symbol = oracle_input
        .instrument_id
        .split('.')
        .next()
        .expect("oracle instrument should include symbol");
    format!(
        "{}_{}",
        oracle_client.venue.as_str().to_ascii_lowercase(),
        symbol.to_ascii_lowercase()
    )
}

fn active_book_not_priced_no_action_reason() -> &'static str {
    BOLT_V3_ENTRY_NO_ACTION_ACTIVE_BOOK_NOT_PRICED_REASON
}

fn fair_probability_unavailable_no_action_reason() -> &'static str {
    BOLT_V3_ENTRY_NO_ACTION_FAIR_PROBABILITY_UNAVAILABLE_REASON
}

fn fixture_forced_flat_thin_book_min_liquidity() -> f64 {
    strategy_raw_config()
        .get("forced_flat_thin_book_min_liquidity")
        .and_then(Value::as_float)
        .expect("test strategy config must include forced_flat_thin_book_min_liquidity")
}

fn fixture_bolt_v3_default_max_notional_per_order() -> rust_decimal::Decimal {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy root fixture should load");
    loaded
        .root
        .risk
        .default_max_notional_per_order
        .parse()
        .expect("fixture default_max_notional_per_order should be decimal")
}

fn strategy_raw_config_with_min_edge(min_edge_bps: i64) -> Value {
    let mut config = strategy_raw_config();
    config.as_table_mut().unwrap().insert(
        "worst_case_ev_min_bps".to_string(),
        Value::Integer(min_edge_bps),
    );
    config
}

fn strategy_raw_config_with_stale_reference_window(stale_ms: i64) -> Value {
    let mut config = strategy_raw_config();
    config.as_table_mut().unwrap().insert(
        "forced_flat_stale_chainlink_ms".to_string(),
        Value::Integer(stale_ms),
    );
    config
}

fn strategy_raw_config_with_fast_venue_incoherent_entry_gate() -> Value {
    let mut config = strategy_raw_config_with_stale_reference_window(1);
    config
        .as_table_mut()
        .unwrap()
        .insert("lead_agreement_min_corr".to_string(), Value::Float(1.0));
    config
}

fn strategy_raw_config_with_pricing_kurtosis(pricing_kurtosis: f64) -> Value {
    let mut config = strategy_raw_config();
    config.as_table_mut().unwrap().insert(
        "pricing_kurtosis".to_string(),
        Value::Float(pricing_kurtosis),
    );
    config
}

fn strategy_raw_config_with_max_position_usdc(max_position_usdc: f64) -> Value {
    let mut config = strategy_raw_config();
    config.as_table_mut().unwrap().insert(
        "max_position_usdc".to_string(),
        Value::Float(max_position_usdc),
    );
    config
}

fn common_decision_context() -> BoltV3DecisionEventCommonContext {
    common_decision_context_from_strategy_config(&strategy_raw_config())
}

fn common_decision_context_from_strategy_config(
    strategy_config: &Value,
) -> BoltV3DecisionEventCommonContext {
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy root fixture should load");
    let runtime_client_id = strategy_config
        .get("client_id")
        .and_then(Value::as_str)
        .expect("test strategy config should include client_id");
    let runtime_strategy_id = strategy_config
        .get("strategy_id")
        .and_then(Value::as_str)
        .expect("test strategy config should include strategy_id");
    let fixture_client_id = loaded
        .strategies
        .first()
        .expect("existing-strategy root fixture should load one strategy")
        .config
        .execution_client_id
        .clone();
    if runtime_client_id != fixture_client_id {
        let fixture_client = loaded
            .root
            .clients
            .get(&fixture_client_id)
            .cloned()
            .expect("existing-strategy root fixture should define selected client");
        loaded
            .root
            .clients
            .insert(runtime_client_id.to_string(), fixture_client);
        loaded
            .strategies
            .first_mut()
            .expect("existing-strategy root fixture should load one strategy")
            .config
            .execution_client_id = runtime_client_id.to_string();
    }
    loaded
        .strategies
        .first_mut()
        .expect("existing-strategy root fixture should load one strategy")
        .config
        .strategy_instance_id = runtime_strategy_id.to_string();
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let identity = load_bolt_v3_release_identity(&loaded).expect("release identity should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy root fixture should load one strategy");

    bolt_v3_decision_event_common_context(&loaded, strategy, &identity)
        .expect("decision context should derive from v3 TOML and release identity")
}

fn configured_target_id_from_decision_context() -> String {
    common_decision_context().configured_target_id
}

fn selection_ruleset_id_from_fixture_config() -> String {
    configured_target_id_from_decision_context()
}

fn market_selection_context_from_fixture_config() -> BoltV3MarketSelectionContext {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy root fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy root fixture should load one strategy");
    let target = updown::deserialize_target_block(&strategy.config.target)
        .expect("existing-strategy fixture target should deserialize");

    BoltV3MarketSelectionContext {
        market_selection_type: target.market_selection_type.as_str().to_string(),
        rotating_market_family: Some(target.rotating_market_family.as_str().to_string()),
        underlying_asset: Some(target.underlying_asset),
        cadence_seconds: Some(target.cadence_seconds),
        market_selection_rule: Some(target.market_selection_rule.as_str().to_string()),
        retry_interval_seconds: Some(target.retry_interval_seconds),
        blocked_after_seconds: Some(target.blocked_after_seconds),
    }
}

fn thin_book_fixture_quantity() -> Quantity {
    Quantity::new(fixture_forced_flat_thin_book_min_liquidity() / 10.0, 2)
}

fn default_book_quantity() -> Quantity {
    Quantity::new(fixture_forced_flat_thin_book_min_liquidity(), 2)
}

fn decision_persistence_block(path: impl AsRef<std::path::Path>) -> PersistenceBlock {
    PersistenceBlock {
        catalog_directory: path.as_ref().to_string_lossy().into_owned(),
        streaming: StreamingBlock {
            catalog_fs_protocol: CatalogFsProtocol::File,
            flush_interval_milliseconds: 1,
            replace_existing: false,
            rotation_kind: RotationKind::None,
        },
    }
}

fn query_entry_evaluation_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn query_entry_evaluation_events_all_files(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    query_custom_events_all_files(
        path,
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
        configured_target_id,
    )
}

fn entry_decision_enter() -> &'static str {
    "enter"
}

fn entry_decision_no_action() -> &'static str {
    "no_action"
}

fn entry_evaluation_events_with_decision<'a>(
    events: &'a [nautilus_model::data::Data],
    decision: &str,
) -> Vec<&'a BoltV3EntryEvaluationDecisionEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            nautilus_model::data::Data::Custom(custom) => custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>(
            ),
            _ => None,
        })
        .filter(|event| {
            event
                .event_facts
                .get("entry_decision")
                .and_then(serde_json::Value::as_str)
                == Some(decision)
        })
        .collect()
}

fn entry_no_action_events_with_reason<'a>(
    events: &'a [nautilus_model::data::Data],
    reason: &str,
) -> Vec<&'a BoltV3EntryEvaluationDecisionEvent> {
    entry_evaluation_events_with_decision(events, entry_decision_no_action())
        .into_iter()
        .filter(|event| {
            event
                .event_facts
                .get("entry_no_action_reason")
                .and_then(serde_json::Value::as_str)
                == Some(reason)
        })
        .collect()
}

fn has_selected_open_orders_no_action_event(path: &std::path::Path) -> bool {
    query_entry_evaluation_events_all_files(path, &configured_target_id_from_decision_context())
        .iter()
        .any(|event| {
            let nautilus_model::data::Data::Custom(custom) = event else {
                return false;
            };
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>();
            decoded.is_some_and(|event| {
                event.event_facts.get("entry_decision")
                    == Some(&serde_json::Value::String("no_action".to_string()))
                    && event
                        .event_facts
                        .get("updown_market_mechanical_rejection_reason")
                        == Some(&serde_json::Value::String(
                            BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON
                                .to_string(),
                        ))
            })
        })
}

fn query_market_selection_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn query_entry_order_submission_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn query_entry_pre_submit_rejection_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn query_exit_order_submission_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn query_exit_evaluation_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn query_exit_pre_submit_rejection_events(
    path: &std::path::Path,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(path, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap()
}

fn exit_pre_submit_rejection_events_with_reason<'a>(
    events: &'a [nautilus_model::data::Data],
    reason: &str,
) -> Vec<&'a BoltV3ExitPreSubmitRejectionDecisionEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            nautilus_model::data::Data::Custom(custom) => custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitPreSubmitRejectionDecisionEvent>(
            ),
            _ => None,
        })
        .filter(|event| {
            event
                .event_facts
                .get("exit_pre_submit_rejection_reason")
                .and_then(serde_json::Value::as_str)
                == Some(reason)
        })
        .collect()
}

fn query_custom_events_all_files(
    path: &std::path::Path,
    event_type: &str,
    configured_target_id: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    let event_dir = path
        .join("data")
        .join("custom")
        .join(event_type)
        .join(configured_target_id);
    if !event_dir.exists() {
        return Vec::new();
    }

    let mut files = fs::read_dir(event_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "parquet")
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();

    files
        .into_iter()
        .flat_map(|file| {
            ParquetDataCatalog::new(path, None, None, None, None)
                .query_custom_data_dynamic(
                    event_type,
                    Some(&ids),
                    None,
                    None,
                    None,
                    Some(vec![file]),
                    true,
                )
                .unwrap()
        })
        .collect()
}

fn selected_market_fixture(fixture_name: &str) -> support::UpdownSelectedMarketFixture {
    support::bolt_v3_updown_selected_market_fixture(fixture_name)
}

fn selected_market_up_instrument_id(fixture_name: &str) -> InstrumentId {
    InstrumentId::from(
        selected_market_fixture(fixture_name)
            .leg("Up")
            .instrument_id
            .as_str(),
    )
}

fn selected_market_down_instrument_id(fixture_name: &str) -> InstrumentId {
    InstrumentId::from(
        selected_market_fixture(fixture_name)
            .leg("Down")
            .instrument_id
            .as_str(),
    )
}

fn candidate_market_from_fixture(fixture_name: &str, start_ts_ms: u64) -> CandidateMarket {
    let fixture = selected_market_fixture(fixture_name);
    let up_leg = fixture.leg("Up");
    let down_leg = fixture.leg("Down");

    CandidateMarket {
        market_id: fixture.market_slug.clone(),
        market_slug: fixture.market_slug.clone(),
        question_id: fixture.question_id.clone(),
        instrument_id: up_leg.instrument_id.clone(),
        condition_id: fixture.condition_id.clone(),
        up_token_id: up_leg.token_id.clone(),
        down_token_id: down_leg.token_id.clone(),
        selected_market_observed_ts_ms: start_ts_ms,
        price_to_beat: None,
        price_to_beat_source: None,
        price_to_beat_observed_ts_ms: None,
        start_ts_ms,
        end_ts_ms: start_ts_ms + 300_000,
        declared_resolution_basis:
            bolt_v2::platform::resolution_basis::parse_ruleset_resolution_basis(
                &fixture_resolution_basis_key(),
            )
            .unwrap(),
        accepting_orders: true,
        liquidity_num: 1000.0,
        seconds_to_end: 300,
    }
}

fn selection_snapshot(start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    selection_snapshot_for(RUNTIME_DEFAULT_SELECTED_MARKET, start_ts_ms)
}

fn selection_snapshot_with_market_facts(
    start_ts_ms: u64,
    price_to_beat_observed_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let mut market = candidate_market_from_fixture(RUNTIME_DEFAULT_SELECTED_MARKET, start_ts_ms);
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    market.price_to_beat = Some(3_100.0);
    market.price_to_beat_source = Some("polymarket_gamma_market_anchor".to_string());
    market.price_to_beat_observed_ts_ms = Some(price_to_beat_observed_ts_ms);
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms: start_ts_ms,
    }
}

fn idle_selection_snapshot(reason: &str, published_at_ms: u64) -> RuntimeSelectionSnapshot {
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Idle {
                reason: reason.to_string(),
            },
        },
        eligible_candidates: Vec::new(),
        published_at_ms,
    }
}

fn selection_snapshot_for(fixture_name: &str, start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Active {
                market: candidate_market_from_fixture(fixture_name, start_ts_ms),
            },
        },
        eligible_candidates: vec![candidate_market_from_fixture(fixture_name, start_ts_ms)],
        published_at_ms: start_ts_ms,
    }
}

fn future_selection_snapshot(
    published_at_ms: u64,
    market_start_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let mut market =
        candidate_market_from_fixture(RUNTIME_DEFAULT_SELECTED_MARKET, market_start_ts_ms);
    market.selected_market_observed_ts_ms = published_at_ms;
    market.end_ts_ms = market_start_ts_ms + 300_000;
    market.seconds_to_end = (market.end_ts_ms - published_at_ms) / 1_000;
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms,
    }
}

fn short_lived_selection_snapshot(
    published_at_ms: u64,
    market_start_ts_ms: u64,
    market_end_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let mut market =
        candidate_market_from_fixture(RUNTIME_DEFAULT_SELECTED_MARKET, market_start_ts_ms);
    market.selected_market_observed_ts_ms = published_at_ms;
    market.end_ts_ms = market_end_ts_ms;
    market.seconds_to_end = (market.end_ts_ms - published_at_ms) / 1_000;
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms,
    }
}

fn freeze_selection_snapshot(start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    freeze_selection_snapshot_for(RUNTIME_DEFAULT_SELECTED_MARKET, start_ts_ms)
}

fn freeze_selection_snapshot_for(fixture_name: &str, start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Freeze {
                market: candidate_market_from_fixture(fixture_name, start_ts_ms),
                reason: SELECTION_FREEZE_WINDOW_REASON.to_string(),
            },
        },
        eligible_candidates: vec![candidate_market_from_fixture(fixture_name, start_ts_ms)],
        published_at_ms: start_ts_ms,
    }
}

fn reference_snapshot(ts_ms: u64, fair_value: f64, fast_price: f64) -> ReferenceSnapshot {
    let stream = fixture_reference_stream();
    let venues = stream
        .inputs
        .iter()
        .map(|input| {
            let venue_kind = match input.source_type {
                ReferenceSourceType::Oracle => VenueKind::Oracle,
                ReferenceSourceType::Orderbook => VenueKind::Orderbook,
            };
            let observed_price = match venue_kind {
                VenueKind::Oracle => fair_value,
                VenueKind::Orderbook => fast_price,
            };
            EffectiveVenueState {
                venue_name: input.source_id.clone(),
                base_weight: input.base_weight,
                effective_weight: input.base_weight,
                stale: false,
                health: VenueHealth::Healthy,
                observed_ts_ms: Some(ts_ms),
                venue_kind,
                observed_price: Some(observed_price),
                observed_bid: (venue_kind == VenueKind::Orderbook).then_some(fast_price - 0.5),
                observed_ask: (venue_kind == VenueKind::Orderbook).then_some(fast_price + 0.5),
            }
        })
        .collect();

    ReferenceSnapshot {
        ts_ms,
        topic: stream.publish_topic,
        fair_value: Some(fair_value),
        confidence: 1.0,
        venues,
    }
}

#[test]
fn eth_chainlink_taker_runtime_reference_snapshot_uses_fixture_reference_stream_inputs() {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy root fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy root fixture should include strategy");
    let stream_id = strategy
        .config
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("existing strategy should select reference stream from TOML");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist");

    let snapshot = reference_snapshot(1_000, 3_100.0, 3_102.0);

    assert_eq!(snapshot.topic, stream.publish_topic);
    assert_eq!(snapshot.venues.len(), stream.inputs.len());
    for (venue, input) in snapshot.venues.iter().zip(stream.inputs.iter()) {
        let expected_kind = match input.source_type {
            ReferenceSourceType::Oracle => VenueKind::Oracle,
            ReferenceSourceType::Orderbook => VenueKind::Orderbook,
        };
        assert_eq!(venue.venue_name, input.source_id);
        assert_eq!(venue.base_weight, input.base_weight);
        assert_eq!(venue.effective_weight, input.base_weight);
        assert_eq!(venue.venue_kind, expected_kind);
    }
}

fn book_deltas(instrument_id: InstrumentId, bid: f64, ask: f64) -> OrderBookDeltas {
    book_deltas_with_quantity(instrument_id, bid, ask, default_book_quantity())
}

fn book_deltas_with_quantity(
    instrument_id: InstrumentId,
    bid: f64,
    ask: f64,
    quantity: Quantity,
) -> OrderBookDeltas {
    OrderBookDeltas::new(
        instrument_id,
        vec![
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(OrderSide::Buy, Price::new(bid, 3), quantity, 0),
                0,
                1,
                UnixNanos::default(),
                UnixNanos::default(),
            ),
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(OrderSide::Sell, Price::new(ask, 3), quantity, 0),
                0,
                2,
                UnixNanos::default(),
                UnixNanos::default(),
            ),
        ],
    )
}

fn polymarket_binary_option(instrument_id: InstrumentId) -> InstrumentAny {
    polymarket_binary_option_with_size_increment(instrument_id, Quantity::from("0.01"))
}

fn polymarket_binary_option_with_size_increment(
    instrument_id: InstrumentId,
    size_increment: Quantity,
) -> InstrumentAny {
    let price_increment = Price::from("0.001");
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        instrument_id.symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(1_u64),
        UnixNanos::from(2_u64),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn seed_cached_open_position(
    node: &LiveNode,
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
) {
    seed_cached_position_with_entry(
        node,
        strategy_id,
        instrument_id,
        OrderSide::Buy,
        cached_position_entry_price(),
        PositionId::from("P-RECOVERY-001"),
    );
}

fn cached_position_entry_quantity() -> Quantity {
    Quantity::from("5")
}

fn cached_position_entry_price() -> Price {
    Price::from("0.450")
}

fn cached_position_entry_notional_usdc() -> f64 {
    cached_position_entry_quantity().as_f64() * cached_position_entry_price().as_f64()
}

fn cross_strategy_open_entry_order_quantity() -> Quantity {
    Quantity::from("100")
}

fn fixture_notional_usdc(quantity: Quantity, price: Price) -> f64 {
    quantity.as_f64() * price.as_f64()
}

fn seed_cached_position_with_entry(
    node: &LiveNode,
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    entry_order_side: OrderSide,
    entry_price: Price,
    position_id: PositionId,
) {
    let cache_handle = node.kernel().cache();
    seed_cached_position_with_entry_in_cache(
        &cache_handle,
        strategy_id,
        instrument_id,
        entry_order_side,
        entry_price,
        position_id,
    );
}

fn seed_cached_position_with_entry_in_cache(
    cache_handle: &Rc<RefCell<Cache>>,
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    entry_order_side: OrderSide,
    entry_price: Price,
    position_id: PositionId,
) {
    let instrument = polymarket_binary_option(instrument_id);
    let mut fill = OrderFilled::new(
        test_trader_id(),
        strategy_id,
        instrument_id,
        ClientOrderId::from("O-RECOVERY-ENTRY-001"),
        VenueOrderId::from("V-RECOVERY-ENTRY-001"),
        test_account_id(),
        TradeId::from("E-RECOVERY-ENTRY-001"),
        entry_order_side,
        OrderType::Market,
        cached_position_entry_quantity(),
        entry_price,
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::default(),
        UnixNanos::default(),
        false,
        None,
        Some(Money::from("0.01 USDC")),
    );
    fill.position_id = Some(position_id);

    let position = Position::new(&instrument, fill);
    let mut cache = cache_handle.borrow_mut();
    cache.add_position(&position, OmsType::Netting).unwrap();
}

fn seed_cached_open_entry_order(
    node: &LiveNode,
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    quantity: Quantity,
    price: Price,
) {
    seed_cached_open_order(
        node,
        strategy_id,
        instrument_id,
        OrderSide::Buy,
        quantity,
        price,
    );
}

fn seed_cached_open_order(
    node: &LiveNode,
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    side: OrderSide,
    quantity: Quantity,
    price: Price,
) {
    let mut order = OrderTestBuilder::new(OrderType::Limit)
        .trader_id(test_trader_id())
        .strategy_id(strategy_id)
        .instrument_id(instrument_id)
        .client_order_id(ClientOrderId::from("O-RECOVERY-ENTRY-001"))
        .side(side)
        .quantity(quantity)
        .price(price)
        .submit(true)
        .build();
    let accepted = OrderAccepted::new(
        order.trader_id(),
        order.strategy_id(),
        order.instrument_id(),
        order.client_order_id(),
        VenueOrderId::from("V-RECOVERY-ENTRY-001"),
        test_account_id(),
        UUID4::new(),
        UnixNanos::default(),
        UnixNanos::default(),
        false,
    );
    order.apply(OrderEventAny::Accepted(accepted)).unwrap();

    let cache_handle = node.kernel().cache();
    let mut cache = cache_handle.borrow_mut();
    cache
        .add_order(order.clone(), None, Some(test_exec_client_id()), false)
        .unwrap();
    cache.update_order(&order).unwrap();
    assert_eq!(
        cache
            .orders_open(None, Some(&instrument_id), Some(&strategy_id), None, None)
            .len(),
        1
    );
}

fn position_opened_event(
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    avg_px_open: f64,
) -> nautilus_model::events::PositionOpened {
    nautilus_model::events::PositionOpened {
        trader_id: test_trader_id(),
        strategy_id,
        instrument_id,
        position_id,
        account_id: test_account_id(),
        opening_order_id: ClientOrderId::from("ENTRY-RT-001"),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: quantity.as_f64(),
        quantity,
        last_qty: quantity,
        last_px: Price::new(avg_px_open, 3),
        currency: Currency::USDC(),
        avg_px_open,
        event_id: UUID4::new(),
        ts_event: UnixNanos::from(1_u64),
        ts_init: UnixNanos::from(1_u64),
    }
}

fn order_filled_event(
    strategy_id: StrategyId,
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    price: Price,
) -> OrderFilled {
    let mut fill = OrderFilled::new(
        test_trader_id(),
        strategy_id,
        instrument_id,
        client_order_id,
        VenueOrderId::from("V-ENTRY-RT-001"),
        test_account_id(),
        TradeId::from("T-ENTRY-RT-001"),
        OrderSide::Buy,
        OrderType::Limit,
        quantity,
        price,
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::from(1_u64),
        UnixNanos::from(1_u64),
        false,
        None,
        Some(Money::from("0.01 USDC")),
    );
    fill.position_id = Some(position_id);
    fill
}

fn entry_fill_event(
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    client_order_id: ClientOrderId,
    position_id: PositionId,
) -> OrderEventAny {
    let mut fill = OrderFilled::new(
        test_trader_id(),
        strategy_id,
        instrument_id,
        client_order_id,
        VenueOrderId::from("V-RT-ENTRY-001"),
        test_account_id(),
        TradeId::from("E-RT-ENTRY-001"),
        OrderSide::Buy,
        OrderType::Market,
        Quantity::from("5"),
        Price::from("0.450"),
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::from(2_u64),
        UnixNanos::from(2_u64),
        false,
        None,
        Some(Money::from("0.01 USDC")),
    );
    fill.position_id = Some(position_id);
    OrderEventAny::Filled(fill)
}

async fn wait_for_running(handle: &LiveNodeHandle) {
    while !handle.is_running() {
        sleep(Duration::from_millis(10)).await;
    }
}

fn add_eth_entry_instruments(node: &mut LiveNode) {
    add_eth_entry_instruments_with_size_increment(node, Quantity::from("0.01"));
}

fn add_eth_entry_instruments_with_size_increment(node: &mut LiveNode, size_increment: Quantity) {
    let cache_handle = node.kernel().cache();
    let mut cache = cache_handle.borrow_mut();
    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    cache
        .add_instrument(polymarket_binary_option_with_size_increment(
            up,
            size_increment,
        ))
        .unwrap();
    cache
        .add_instrument(polymarket_binary_option_with_size_increment(
            down,
            size_increment,
        ))
        .unwrap();
}

fn eth_up_instrument_id() -> InstrumentId {
    selected_market_up_instrument_id(RUNTIME_DEFAULT_SELECTED_MARKET)
}

fn eth_down_instrument_id() -> InstrumentId {
    selected_market_down_instrument_id(RUNTIME_DEFAULT_SELECTED_MARKET)
}

fn drive_eth_entry_submission(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_selected_market_open_orders_no_action(
    mut node: LiveNode,
    strategy_id: StrategyId,
    evidence_dir: &std::path::Path,
) {
    let handle = node.handle();
    let evidence_dir = evidence_dir.to_path_buf();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 400, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            for _ in 0..100 {
                let evidence_dir_for_check = evidence_dir.clone();
                let found = tokio::task::spawn_blocking(move || {
                    has_selected_open_orders_no_action_event(&evidence_dir_for_check)
                })
                .await
                .unwrap();
                if found {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_no_action(node: LiveNode, strategy_id: StrategyId) {
    drive_eth_entry_no_action_with_book_quantity(node, strategy_id, default_book_quantity());
}

fn drive_eth_entry_no_action_with_book_quantity(
    mut node: LiveNode,
    strategy_id: StrategyId,
    book_quantity: Quantity,
) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas_with_quantity(up, 0.430, 0.450, book_quantity),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas_with_quantity(down, 0.480, 0.490, book_quantity),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_after_cached_position_no_action(node: LiveNode, strategy_id: StrategyId) {
    drive_eth_entry_after_cached_position_for_position_strategy_no_action(
        node,
        strategy_id,
        strategy_id,
    );
}

fn drive_eth_entry_after_cached_position_for_position_strategy_no_action(
    mut node: LiveNode,
    strategy_id: StrategyId,
    position_strategy_id: StrategyId,
) {
    let handle = node.handle();
    let cache_handle = node.kernel().cache();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;

            let up = eth_up_instrument_id();
            seed_cached_position_with_entry_in_cache(
                &cache_handle,
                position_strategy_id,
                up,
                OrderSide::Buy,
                cached_position_entry_price(),
                PositionId::from("P-RT-FILLED-CAPACITY"),
            );

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_missing_reference_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_active_book_not_priced_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            let up = eth_up_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_realized_vol_not_ready_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_stale_reference_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 1, 3_101.0, 3_105.0),
            );
            sleep(Duration::from_millis(20)).await;

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_freeze_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_market_not_started_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let market_start_ts_ms = start_ts_ms + 60_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &future_selection_snapshot(start_ts_ms, market_start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_market_ended_no_action(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let market_end_ts_ms = start_ts_ms + 1_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &short_lived_selection_snapshot(start_ts_ms, start_ts_ms, market_end_ts_ms),
            );
            sleep(Duration::from_millis(1_250)).await;
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(market_end_ts_ms + 250, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(market_end_ts_ms + 450, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_entry_pre_submit_rejection(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_exit_pre_submit_rejection(node: LiveNode, strategy_id: StrategyId) {
    drive_eth_exit_pre_submit_rejection_with_quantity(
        node,
        strategy_id,
        Quantity::new(5.0, 2),
        "P-RT-EXIT-REJECT",
    );
}

fn drive_eth_exit_pre_submit_rejection_with_quantity(
    mut node: LiveNode,
    strategy_id: StrategyId,
    quantity: Quantity,
    position_id: &str,
) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let up = eth_up_instrument_id();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor.on_position_opened(position_opened_event(
                    strategy_id,
                    up,
                    PositionId::from(position_id),
                    quantity,
                    0.450,
                ));
            } else {
                panic!("runtime strategy actor should be registered");
            }
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );
            sleep(Duration::from_millis(50)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

fn drive_eth_exit_sellable_rejection(mut node: LiveNode, strategy_id: StrategyId) {
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            sleep(Duration::from_millis(20)).await;

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor.on_position_opened(position_opened_event(
                    strategy_id,
                    up,
                    PositionId::from("P-RT-EXIT-SELLABLE-REJECT"),
                    Quantity::new(5.0, 2),
                    0.450,
                ));
            } else {
                panic!("runtime strategy actor should be registered");
            }
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });
}

#[test]
fn eth_chainlink_taker_runtime_submits_real_entry_order() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    {
        let cache_handle = node.kernel().cache();
        let mut cache = cache_handle.borrow_mut();
        let up = eth_up_instrument_id();
        let down = eth_down_instrument_id();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = eth_up_instrument_id();
            let down = eth_down_instrument_id();
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, eth_up_instrument_id());
    assert!(submissions[0].client_order_id.to_string().starts_with('O'));
}

#[test]
fn eth_chainlink_taker_runtime_caps_entry_notional_by_bolt_v3_default_max_notional_per_order() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_config = strategy_raw_config();
    let decision_context = common_decision_context();
    let configured_target_id = decision_context.configured_target_id.clone();
    let strategy_archetype = decision_context.strategy_archetype.clone();
    let strategy_id = strategy_id_from_raw_config(&strategy_config);
    let default_max_notional_per_order = fixture_bolt_v3_default_max_notional_per_order();
    let default_max_notional_per_order_f64 = default_max_notional_per_order
        .to_f64()
        .expect("fixture default_max_notional_per_order should fit f64");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        decision_context,
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    build_context.bolt_v3_default_max_notional_per_order = Some(default_max_notional_per_order);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, strategy_archetype.as_str(), &strategy_config).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_submission(node, strategy_id);

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");

    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let accepted_evaluation_events =
        entry_evaluation_events_with_decision(&evaluation_events, entry_decision_enter());
    assert_eq!(accepted_evaluation_events.len(), 1);
    let decoded = accepted_evaluation_events[0];
    let sized_notional = decoded
        .event_facts
        .get("archetype_metrics")
        .and_then(|value| value.get("sized_notional_usdc"))
        .and_then(serde_json::Value::as_f64)
        .expect("entry evaluation should include sized_notional_usdc");
    let effective_cap = decoded
        .event_facts
        .get("archetype_metrics")
        .and_then(|value| value.get("effective_entry_notional_cap_usdc"))
        .and_then(serde_json::Value::as_f64)
        .expect("entry evaluation should include effective_entry_notional_cap_usdc");
    assert_eq!(effective_cap, default_max_notional_per_order_f64);
    assert!(
        sized_notional <= default_max_notional_per_order_f64,
        "sized_notional_usdc {sized_notional} must not exceed bolt-v3 default_max_notional_per_order {default_max_notional_per_order_f64}"
    );

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryOrderSubmissionDecisionEvent>()
                .expect("BoltV3EntryOrderSubmissionDecisionEvent");
            let price = decoded
                .event_facts
                .get("price")
                .and_then(serde_json::Value::as_f64)
                .expect("entry order submission should include price");
            let quantity = decoded
                .event_facts
                .get("quantity")
                .and_then(serde_json::Value::as_f64)
                .expect("entry order submission should include quantity");
            let submitted_notional = price * quantity;
            assert!(
                submitted_notional <= default_max_notional_per_order_f64,
                "submitted notional {submitted_notional} must not exceed bolt-v3 default_max_notional_per_order {default_max_notional_per_order_f64}"
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_restrictive_trading_states_block_entry_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();

    for (trading_state, expected_reason) in [
        (
            TradingState::Halted,
            BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_TRADING_STATE_HALTED_REASON,
        ),
        (
            TradingState::Reducing,
            BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_TRADING_STATE_REDUCING_REASON,
        ),
    ] {
        clear_mock_exec_submissions();

        let temp_dir = TempDir::new().unwrap();
        let mut node = build_test_node();
        add_eth_entry_instruments(&mut node);
        let trader = Rc::clone(node.kernel().trader());
        let strategy_config = strategy_raw_config();
        let decision_context = common_decision_context();
        let configured_target_id = decision_context.configured_target_id.clone();
        let strategy_archetype = decision_context.strategy_archetype.clone();
        let strategy_id = strategy_id_from_raw_config(&strategy_config);
        let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
            decision_context,
            &decision_persistence_block(temp_dir.path()),
        )
        .unwrap();
        let mut build_context = make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(trading_state),
        );
        build_context.bolt_v3_decision_evidence = Some(evidence);
        let strategy_factory = registry_runtime_strategy_factory(
            production_strategy_registry().unwrap(),
            build_context,
        );
        strategy_factory(&trader, strategy_archetype.as_str(), &strategy_config).unwrap();

        drive_eth_entry_submission(node, strategy_id);

        assert!(
            recorded_mock_exec_submissions().is_empty(),
            "{trading_state:?} trading state must block entry submit before NT execution"
        );

        let rejection_events =
            query_entry_pre_submit_rejection_events(temp_dir.path(), &configured_target_id);
        assert_eq!(rejection_events.len(), 1);
        match &rejection_events[0] {
            nautilus_model::data::Data::Custom(custom) => {
                let decoded = custom
                    .data
                    .as_any()
                    .downcast_ref::<BoltV3EntryPreSubmitRejectionDecisionEvent>()
                    .expect("BoltV3EntryPreSubmitRejectionDecisionEvent");
                assert_eq!(
                    decoded.event_facts.get("entry_pre_submit_rejection_reason"),
                    Some(&serde_json::Value::String(expected_reason.to_string()))
                );
            }
            other => panic!("expected Data::Custom, got {other:?}"),
        }

        let submission_events =
            query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
        assert!(
            submission_events.is_empty(),
            "{trading_state:?} trading state must not persist entry order submission"
        );
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_market_selection_result_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    build_context.bolt_v3_market_selection_context =
        Some(market_selection_context_from_fixture_config());
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let handle = node.handle();
    let start_ts_ms = 1_000;
    let price_to_beat_observed_ts_ms = 900;
    let selected_market = selected_market_fixture(RUNTIME_DEFAULT_SELECTED_MARKET);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot_with_market_facts(start_ts_ms, price_to_beat_observed_ts_ms),
            );
            sleep(Duration::from_millis(50)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 0, "{submissions:?}");

    let market_selection_events = query_market_selection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(market_selection_events.len(), 1);
    match &market_selection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3MarketSelectionDecisionEvent>()
                .expect("BoltV3MarketSelectionDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(decoded.decision_event_type, "market_selection_result");
            assert!(!decoded.decision_trace_id.is_empty());
            assert_eq!(
                decoded.event_facts.get("market_selection_type"),
                Some(&serde_json::Value::String("rotating_market".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("market_selection_outcome"),
                Some(&serde_json::Value::String("current".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("market_selection_failure_reason"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("rotating_market_family"),
                Some(&serde_json::Value::String("updown".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("underlying_asset"),
                Some(&serde_json::Value::String("ETH".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("cadence_seconds"),
                Some(&serde_json::Value::from(300))
            );
            assert_eq!(
                decoded.event_facts.get("market_selection_rule"),
                Some(&serde_json::Value::String("active_or_next".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("retry_interval_seconds"),
                Some(&serde_json::Value::from(5))
            );
            assert_eq!(
                decoded.event_facts.get("blocked_after_seconds"),
                Some(&serde_json::Value::from(60))
            );
            assert_eq!(
                decoded.event_facts.get("polymarket_condition_id"),
                Some(&serde_json::Value::String(
                    selected_market.condition_id.clone()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("polymarket_market_slug"),
                Some(&serde_json::Value::String(
                    selected_market.market_slug.clone()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("polymarket_question_id"),
                Some(&serde_json::Value::String(
                    selected_market.question_id.clone()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("up_instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("down_instrument_id"),
                Some(&serde_json::Value::String(
                    eth_down_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("selected_market_observed_timestamp"),
                Some(&serde_json::Value::from(start_ts_ms))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("polymarket_market_start_timestamp_milliseconds"),
                Some(&serde_json::Value::from(start_ts_ms))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("polymarket_market_end_timestamp_milliseconds"),
                Some(&serde_json::Value::from(start_ts_ms + 300_000))
            );
            assert_eq!(
                decoded.event_facts.get("price_to_beat_value"),
                Some(&serde_json::Value::from(3_100.0))
            );
            assert_eq!(
                decoded.event_facts.get("price_to_beat_observed_timestamp"),
                Some(&serde_json::Value::from(price_to_beat_observed_ts_ms))
            );
            assert_eq!(
                decoded.event_facts.get("price_to_beat_source"),
                Some(&serde_json::Value::String(
                    "polymarket_gamma_market_anchor".to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_allowed_failed_market_selection_results_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();

    for reason in BOLT_V3_MARKET_SELECTION_FAILURE_REASONS {
        assert_failed_market_selection_result_without_submit(reason);
    }
}

fn assert_failed_market_selection_result_without_submit(reason: &str) {
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    build_context.bolt_v3_market_selection_context =
        Some(market_selection_context_from_fixture_config());
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let handle = node.handle();
    let published_at_ms = 1_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &idle_selection_snapshot(reason, published_at_ms),
            );
            sleep(Duration::from_millis(50)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 0, "{submissions:?}");

    let market_selection_events = query_market_selection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(market_selection_events.len(), 1);
    match &market_selection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3MarketSelectionDecisionEvent>()
                .expect("BoltV3MarketSelectionDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(decoded.decision_event_type, "market_selection_result");
            assert!(!decoded.decision_trace_id.is_empty());
            assert_eq!(
                decoded.event_facts.get("market_selection_type"),
                Some(&serde_json::Value::String("rotating_market".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("market_selection_timestamp_milliseconds"),
                Some(&serde_json::Value::from(published_at_ms))
            );
            assert_eq!(
                decoded.event_facts.get("market_selection_outcome"),
                Some(&serde_json::Value::String("failed".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("market_selection_failure_reason"),
                Some(&serde_json::Value::String(reason.to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("rotating_market_family"),
                Some(&serde_json::Value::String("updown".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("underlying_asset"),
                Some(&serde_json::Value::String("ETH".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("cadence_seconds"),
                Some(&serde_json::Value::from(300))
            );
            assert_eq!(
                decoded.event_facts.get("market_selection_rule"),
                Some(&serde_json::Value::String("active_or_next".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("retry_interval_seconds"),
                Some(&serde_json::Value::from(5))
            );
            assert_eq!(
                decoded.event_facts.get("blocked_after_seconds"),
                Some(&serde_json::Value::from(60))
            );
            assert_eq!(
                decoded.event_facts.get("polymarket_condition_id"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("price_to_beat_value"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_entry_evaluation_and_order_intent_before_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_submission(node, strategy_id);

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");

    let evaluation_events = query_entry_evaluation_events_all_files(
        temp_dir.path(),
        &configured_target_id_from_decision_context(),
    );
    let accepted_evaluation_events =
        entry_evaluation_events_with_decision(&evaluation_events, entry_decision_enter());
    assert_eq!(accepted_evaluation_events.len(), 1);
    let decoded = accepted_evaluation_events[0];
    assert_eq!(
        decoded.strategy_instance_id,
        strategy_id_from_fixture_config().to_string()
    );
    assert_eq!(decoded.client_id, test_exec_client_name());
    let updown_side = decoded
        .event_facts
        .get("updown_side")
        .and_then(serde_json::Value::as_str)
        .expect("entry evaluation updown_side should be present");
    assert!(
        matches!(updown_side, "up" | "down"),
        "unexpected updown_side {updown_side}"
    );
    let evaluation_trace_id = decoded.decision_trace_id.clone();

    let submission_events = query_entry_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryOrderSubmissionDecisionEvent>()
                .expect("BoltV3EntryOrderSubmissionDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(decoded.decision_trace_id, evaluation_trace_id);
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::String(
                    submissions[0].client_order_id.to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_no_action_entry_evaluation_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_min_edge(2_000),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "no-action entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let insufficient_edge_events = entry_no_action_events_with_reason(
        &evaluation_events,
        BOLT_V3_ENTRY_NO_ACTION_INSUFFICIENT_EDGE_REASON,
    );
    let decoded = *insufficient_edge_events
        .last()
        .expect("expected insufficient-edge no-action event");
    assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
    assert_eq!(decoded.client_id, common_decision_context().client_id);
    assert_eq!(
        decoded.event_facts.get("entry_decision"),
        Some(&serde_json::Value::String("no_action".to_string()))
    );
    assert_eq!(
        decoded.event_facts.get("entry_no_action_reason"),
        Some(&serde_json::Value::String(
            BOLT_V3_ENTRY_NO_ACTION_INSUFFICIENT_EDGE_REASON.to_string()
        ))
    );
    assert_eq!(
        decoded.event_facts.get("updown_side"),
        Some(&serde_json::Value::Null)
    );

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "no-action entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_thin_book_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action_with_book_quantity(node, strategy_id, thin_book_fixture_quantity());

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "thin-book entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let thin_book_events = entry_no_action_events_with_reason(
        &evaluation_events,
        BOLT_V3_ENTRY_NO_ACTION_THIN_BOOK_REASON,
    );
    match thin_book_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_THIN_BOOK_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_market_mechanical_outcome"),
                Some(&serde_json::Value::String("accepted".to_string()))
            );
        }
        None => panic!("expected thin-book no-action event, got {evaluation_events:?}"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "thin-book entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_missing_reference_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_missing_reference_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "missing-reference entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let missing_reference_events = entry_no_action_events_with_reason(
        &evaluation_events,
        BOLT_V3_ENTRY_NO_ACTION_MISSING_REFERENCE_QUOTE_REASON,
    );
    let decoded = *missing_reference_events
        .last()
        .expect("expected missing-reference no-action event");
    assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
    assert_eq!(decoded.client_id, common_decision_context().client_id);
    assert_eq!(
        decoded.event_facts.get("entry_decision"),
        Some(&serde_json::Value::String("no_action".to_string()))
    );
    assert_eq!(
        decoded.event_facts.get("entry_no_action_reason"),
        Some(&serde_json::Value::String(
            BOLT_V3_ENTRY_NO_ACTION_MISSING_REFERENCE_QUOTE_REASON.to_string()
        ))
    );
    assert_eq!(
        decoded.event_facts.get("updown_side"),
        Some(&serde_json::Value::Null)
    );

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "missing-reference entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_fee_rate_unavailable_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(MissingFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "fee-rate-unavailable entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let fee_rate_events = entry_no_action_events_with_reason(
        &evaluation_events,
        BOLT_V3_ENTRY_NO_ACTION_FEE_RATE_UNAVAILABLE_REASON,
    );
    let decoded = *fee_rate_events
        .last()
        .expect("expected fee-rate-unavailable no-action event");
    assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
    assert_eq!(decoded.client_id, common_decision_context().client_id);
    assert_eq!(
        decoded.event_facts.get("entry_decision"),
        Some(&serde_json::Value::String("no_action".to_string()))
    );
    assert_eq!(
        decoded.event_facts.get("entry_no_action_reason"),
        Some(&serde_json::Value::String(
            BOLT_V3_ENTRY_NO_ACTION_FEE_RATE_UNAVAILABLE_REASON.to_string()
        ))
    );
    assert_eq!(
        decoded.event_facts.get("updown_side"),
        Some(&serde_json::Value::Null)
    );

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "fee-rate-unavailable entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_active_book_not_priced_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_active_book_not_priced_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "active-book-not-priced entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    active_book_not_priced_no_action_reason().to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_realized_vol_not_ready_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_realized_vol_not_ready_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "realized-vol-not-ready entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let fair_probability_events = entry_no_action_events_with_reason(
        &evaluation_events,
        fair_probability_unavailable_no_action_reason(),
    );
    match fair_probability_events.last() {
        Some(decoded) => {
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String(
                    entry_decision_no_action().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    fair_probability_unavailable_no_action_reason().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        None => {
            panic!("expected realized-vol-not-ready no-action event, got {evaluation_events:?}")
        }
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "realized-vol-not-ready entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_stale_reference_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_stale_reference_window(1),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_stale_reference_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "stale-reference entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let stale_reference_events = entry_no_action_events_with_reason(
        &evaluation_events,
        BOLT_V3_ENTRY_NO_ACTION_STALE_REFERENCE_QUOTE_REASON,
    );
    match stale_reference_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_STALE_REFERENCE_QUOTE_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        None => panic!("expected stale-reference no-action event, got {evaluation_events:?}"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "stale-reference entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_fast_venue_incoherent_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_fast_venue_incoherent_entry_gate(),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_stale_reference_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "fast-venue-incoherent entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let fast_venue_event = evaluation_events.iter().find_map(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return None;
        };
        let decoded = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()?;
        (decoded.event_facts.get("entry_no_action_reason")
            == Some(&serde_json::Value::String(
                BOLT_V3_ENTRY_NO_ACTION_FAST_VENUE_INCOHERENT_REASON.to_string(),
            )))
        .then_some(decoded)
    });
    match fast_venue_event {
        Some(decoded) => {
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_FAST_VENUE_INCOHERENT_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        None => panic!("expected fast-venue-incoherent no-action event"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "fast-venue-incoherent entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_freeze_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_freeze_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "freeze entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let freeze_event = evaluation_events.iter().find_map(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return None;
        };
        let decoded = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()?;
        (decoded.event_facts.get("entry_no_action_reason")
            == Some(&serde_json::Value::String(
                BOLT_V3_ENTRY_NO_ACTION_FREEZE_REASON.to_string(),
            )))
        .then_some(decoded)
    });
    match freeze_event {
        Some(decoded) => {
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_FREEZE_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        None => panic!("expected freeze no-action event"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "freeze entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_fair_probability_unavailable_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_pricing_kurtosis(-6.0),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "fair-probability-unavailable entry evaluation must not submit order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let fair_probability_events = entry_no_action_events_with_reason(
        &evaluation_events,
        fair_probability_unavailable_no_action_reason(),
    );
    match fair_probability_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    fair_probability_unavailable_no_action_reason().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        None => panic!("expected fair-probability-unavailable no-action event"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "fair-probability-unavailable entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_position_limit_reached_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_max_position_usdc(0.0),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "position-limit-reached entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_POSITION_LIMIT_REACHED_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("strategy_remaining_entry_capacity")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events = query_entry_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "position-limit-reached entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_open_entry_capacity_from_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_max_position_usdc(45.0),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    let up = eth_up_instrument_id();
    seed_cached_open_entry_order(
        &node,
        strategy_id,
        up,
        Quantity::from("100"),
        Price::from("0.450"),
    );
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "open entry capacity from NT cache must not submit another order"
    );

    let evaluation_events = query_entry_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("updown_market_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON
                        .to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("open_entry_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(45.0)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("strategy_remaining_entry_capacity")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_counts_other_strategy_open_entry_capacity_from_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let other_strategy_id = strategy_id_from_multi_fixture(1);
    let open_order_quantity = cross_strategy_open_entry_order_quantity();
    let open_order_price = cached_position_entry_price();
    let open_order_notional = fixture_notional_usdc(open_order_quantity, open_order_price);
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_max_position_usdc(open_order_notional),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    let up = eth_up_instrument_id();
    seed_cached_open_entry_order(
        &node,
        other_strategy_id,
        up,
        open_order_quantity,
        open_order_price,
    );
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "other-strategy open entry capacity must not submit another order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("updown_market_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON
                        .to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("open_entry_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(open_order_notional)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("strategy_remaining_entry_capacity")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_filled_entry_capacity_from_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let filled_entry_notional = cached_position_entry_notional_usdc();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_max_position_usdc(filled_entry_notional),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_after_cached_position_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "filled entry capacity from NT cache must not submit another order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_POSITION_LIMIT_REACHED_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("entry_filled_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(filled_entry_notional)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("open_entry_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("strategy_remaining_entry_capacity")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_counts_other_strategy_filled_entry_capacity_from_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let other_strategy_id = strategy_id_from_multi_fixture(1);
    let filled_entry_notional = cached_position_entry_notional_usdc();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_max_position_usdc(filled_entry_notional),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_after_cached_position_for_position_strategy_no_action(
        node,
        strategy_id,
        other_strategy_id,
    );

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "other-strategy filled entry capacity must not submit another order"
    );

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_POSITION_LIMIT_REACHED_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("entry_filled_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(filled_entry_notional)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("open_entry_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("strategy_remaining_entry_capacity")
                    .and_then(serde_json::Value::as_f64),
                Some(0.0)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_market_not_started_mechanical_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_market_not_started_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "market-not-started mechanical entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_market_mechanical_outcome"),
                Some(&serde_json::Value::String("rejected".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("updown_market_mechanical_rejection_reason"),
                Some(&serde_json::Value::String("market_not_started".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("has_selected_market_open_orders"),
                Some(&serde_json::Value::Bool(false))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events = query_entry_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "market-not-started mechanical entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_market_ended_mechanical_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_market_ended_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "market-ended mechanical entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_market_mechanical_outcome"),
                Some(&serde_json::Value::String("rejected".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("updown_market_mechanical_rejection_reason"),
                Some(&serde_json::Value::String("market_ended".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("has_selected_market_open_orders"),
                Some(&serde_json::Value::Bool(false))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events = query_entry_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "market-ended mechanical entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_selected_open_orders_no_action_without_second_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_selected_market_open_orders_no_action(node, strategy_id, temp_dir.path());

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), &configured_target_id);
    let selected_open_orders_event = evaluation_events.iter().find_map(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return None;
        };
        let decoded = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()?;
        (decoded
            .event_facts
            .get("updown_market_mechanical_rejection_reason")
            == Some(&serde_json::Value::String(
                BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON.to_string(),
            )))
        .then_some(decoded)
    });
    match selected_open_orders_event {
        Some(decoded) => {
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_market_mechanical_outcome"),
                Some(&serde_json::Value::String("rejected".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("updown_market_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON
                        .to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("has_selected_market_open_orders"),
                Some(&serde_json::Value::Bool(true))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        None => panic!("expected selected-market-open-orders no-action event"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert_eq!(submission_events.len(), 1);
}

#[test]
fn eth_chainlink_taker_runtime_writes_entry_pre_submit_rejection_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    drive_eth_entry_pre_submit_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "pre-submit rejection must not submit order"
    );

    let rejection_events = query_entry_pre_submit_rejection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3EntryPreSubmitRejectionDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INSTRUMENT_MISSING_FROM_CACHE_REASON
                        .to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("order_type"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("time_in_force"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("side"),
                Some(&serde_json::Value::String("buy".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("price"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("is_quote_quantity"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("is_post_only"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("is_reduce_only"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events = query_entry_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "pre-submit rejection must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_invalid_quantity_pre_submit_rejection_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        ETH_CHAINLINK_TAKER_KIND,
        &strategy_raw_config_with_max_position_usdc(0.1),
    )
    .unwrap();

    add_eth_entry_instruments_with_size_increment(&mut node, Quantity::from("1"));
    drive_eth_entry_pre_submit_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "invalid quantity rejection must not submit order"
    );

    let rejection_events = query_entry_pre_submit_rejection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3EntryPreSubmitRejectionDecisionEvent");
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("entry_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("side"),
                Some(&serde_json::Value::String("buy".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("price"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events = query_entry_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "invalid quantity rejection must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_exit_pre_submit_rejection_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_exit_pre_submit_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "exit pre-submit rejection must not submit order"
    );

    let rejection_events = query_exit_pre_submit_rejection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    let exit_price_missing_events = exit_pre_submit_rejection_events_with_reason(
        &rejection_events,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_PRICE_MISSING_REASON,
    );
    match exit_price_missing_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_PRICE_MISSING_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("side"),
                Some(&serde_json::Value::String("sell".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("price"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
        }
        None => panic!("expected exit_price_missing rejection event, got {rejection_events:?}"),
    }

    let submission_events = query_exit_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "exit pre-submit rejection must not persist order submission"
    );

    let evaluation_events = query_exit_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    let exit_bid_unavailable_event = evaluation_events.iter().find_map(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return None;
        };
        let decoded = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()?;
        (decoded
            .event_facts
            .get("exit_order_mechanical_rejection_reason")
            .and_then(serde_json::Value::as_str)
            == Some("exit_bid_unavailable"))
        .then_some(decoded)
    });
    match exit_bid_unavailable_event {
        Some(decoded) => {
            assert_eq!(
                decoded.event_facts.get("exit_order_mechanical_outcome"),
                Some(&serde_json::Value::String("rejected".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("exit_order_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    "exit_bid_unavailable".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision"),
                Some(&serde_json::Value::String("hold".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_DECISION_ORDER_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
        }
        None => panic!("expected exit_bid_unavailable evaluation event, got {evaluation_events:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_exit_invalid_quantity_pre_submit_rejection_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_exit_pre_submit_rejection_with_quantity(
        node,
        strategy_id,
        Quantity::new(0.0, 2),
        "P-RT-EXIT-INVALID-QTY",
    );

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "exit invalid quantity rejection must not submit order"
    );

    let rejection_events = query_exit_pre_submit_rejection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    let invalid_quantity_events = exit_pre_submit_rejection_events_with_reason(
        &rejection_events,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON,
    );
    match invalid_quantity_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.strategy_instance_id,
                strategy_id_from_fixture_config().to_string()
            );
            assert_eq!(decoded.client_id, test_exec_client_name());
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("side"),
                Some(&serde_json::Value::String("sell".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("price"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
        }
        None => panic!("expected invalid_quantity rejection event, got {rejection_events:?}"),
    }

    let submission_events = query_exit_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "exit invalid quantity rejection must not persist order submission"
    );

    let evaluation_events = query_exit_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()
                .expect("BoltV3ExitEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("exit_order_mechanical_outcome"),
                Some(&serde_json::Value::String("rejected".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("exit_order_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    "exit_quantity_invalid".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision"),
                Some(&serde_json::Value::String("hold".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_DECISION_ORDER_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_exit_sellable_quantity_pre_submit_rejection_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    let up = eth_up_instrument_id();
    seed_cached_open_order(
        &node,
        strategy_id,
        up,
        OrderSide::Sell,
        Quantity::new(5.0, 2),
        Price::from("0.430"),
    );
    drive_eth_exit_sellable_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "exit sellable quantity rejection must not submit another sell order"
    );

    let rejection_events = query_exit_pre_submit_rejection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    let sellable_quantity_events = exit_pre_submit_rejection_events_with_reason(
        &rejection_events,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_QUANTITY_EXCEEDS_SELLABLE_QUANTITY_REASON,
    );
    match sellable_quantity_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_QUANTITY_EXCEEDS_SELLABLE_QUANTITY_REASON
                        .to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("side"),
                Some(&serde_json::Value::String("sell".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
        }
        None => panic!(
            "expected exit_quantity_exceeds_sellable_quantity rejection event, got {rejection_events:?}"
        ),
    }

    let submission_events = query_exit_order_submission_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    assert!(
        submission_events.is_empty(),
        "exit sellable quantity rejection must not persist order submission"
    );

    let evaluation_events = query_exit_evaluation_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    let open_order_covers_position_event = evaluation_events.iter().find_map(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return None;
        };
        let decoded = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()?;
        (decoded
            .event_facts
            .get("exit_order_mechanical_rejection_reason")
            .and_then(serde_json::Value::as_str)
            == Some("open_exit_order_quantity_covers_position"))
        .then_some(decoded)
    });
    match open_order_covers_position_event {
        Some(decoded) => {
            assert_eq!(
                decoded.event_facts.get("exit_order_mechanical_outcome"),
                Some(&serde_json::Value::String("rejected".to_string()))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("exit_order_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    "open_exit_order_quantity_covers_position".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision"),
                Some(&serde_json::Value::String("hold".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_DECISION_ORDER_MECHANICAL_REJECTION_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(0.0))
            );
        }
        None => panic!(
            "expected open_exit_order_quantity_covers_position evaluation event, got {evaluation_events:?}"
        ),
    }
}

#[test]
fn eth_chainlink_taker_runtime_halted_trading_state_blocks_exit_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_config = strategy_raw_config();
    let decision_context = common_decision_context();
    let configured_target_id = decision_context.configured_target_id.clone();
    let strategy_archetype = decision_context.strategy_archetype.clone();
    let strategy_id = strategy_id_from_raw_config(&strategy_config);
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        decision_context,
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Halted),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, strategy_archetype.as_str(), &strategy_config).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_exit_sellable_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "HALTED trading state must block exit submit before NT execution"
    );

    let rejection_events =
        query_exit_pre_submit_rejection_events(temp_dir.path(), &configured_target_id);
    let trading_state_events = exit_pre_submit_rejection_events_with_reason(
        &rejection_events,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_TRADING_STATE_HALTED_REASON,
    );
    match trading_state_events.last() {
        Some(decoded) => {
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_TRADING_STATE_HALTED_REASON.to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    eth_up_instrument_id().to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("side"),
                Some(&serde_json::Value::String("sell".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::from(5.0))
            );
        }
        None => panic!("expected trading_state_halted rejection event, got {rejection_events:?}"),
    }

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "HALTED trading state must not persist exit order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_submits_uncovered_exit_quantity_when_partial_exit_order_open() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    let up = eth_up_instrument_id();
    seed_cached_open_order(
        &node,
        strategy_id,
        up,
        OrderSide::Sell,
        Quantity::new(2.0, 2),
        Price::from("0.430"),
    );
    drive_eth_exit_sellable_rejection(node, strategy_id);

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);

    let rejection_events = query_exit_pre_submit_rejection_events(
        temp_dir.path(),
        configured_target_id_from_decision_context().as_str(),
    );
    for event in rejection_events {
        match event {
            nautilus_model::data::Data::Custom(custom) => {
                let decoded = custom
                    .data
                    .as_any()
                    .downcast_ref::<BoltV3ExitPreSubmitRejectionDecisionEvent>()
                    .expect("BoltV3ExitPreSubmitRejectionDecisionEvent");
                assert_ne!(
                    decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                    Some(&serde_json::Value::from(
                        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_EXIT_QUANTITY_EXCEEDS_SELLABLE_QUANTITY_REASON
                    )),
                    "partial open exit sell must not reject uncovered quantity"
                );
            }
            other => panic!("expected Data::Custom, got {other:?}"),
        }
    }

    let configured_target_id = configured_target_id_from_decision_context();
    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), &configured_target_id);
    let partial_exit_event = evaluation_events.iter().find_map(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return None;
        };
        let decoded = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()?;
        (decoded.event_facts.get("uncovered_position_quantity")
            == Some(&serde_json::Value::from(3.0)))
        .then_some(decoded)
    });
    match partial_exit_event {
        Some(decoded) => {
            assert_eq!(
                decoded.event_facts.get("authoritative_position_quantity"),
                Some(&serde_json::Value::from(5.0))
            );
            assert_eq!(
                decoded.event_facts.get("authoritative_sellable_quantity"),
                Some(&serde_json::Value::from(3.0))
            );
            assert_eq!(
                decoded.event_facts.get("open_exit_order_quantity"),
                Some(&serde_json::Value::from(2.0))
            );
            assert_eq!(
                decoded.event_facts.get("uncovered_position_quantity"),
                Some(&serde_json::Value::from(3.0))
            );
        }
        None => panic!("expected partial uncovered exit evaluation event"),
    }

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), &configured_target_id);
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitOrderSubmissionDecisionEvent>()
                .expect("BoltV3ExitOrderSubmissionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("quantity"),
                Some(&serde_json::Value::from(3.0))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_blocks_entry_submit_when_decision_evidence_write_fails() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let occupied_path = temp_dir.path().join("not-a-directory");
    std::fs::write(&occupied_path, b"occupied").unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(&occupied_path),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_submission(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "failed decision-evidence handoff must block NT submit"
    );
}

#[test]
fn eth_chainlink_taker_runtime_blocks_exit_submit_when_decision_evidence_write_fails() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let occupied_path = temp_dir.path().join("not-a-directory");
    std::fs::write(&occupied_path, b"occupied").unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(&occupied_path),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_exit_sellable_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "failed exit decision-evidence handoff must block NT submit"
    );
}

#[test]
fn eth_chainlink_taker_runtime_keeps_exit_submit_blocked_after_decision_evidence_failure() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let occupied_path = temp_dir.path().join("not-a-directory");
    std::fs::write(&occupied_path, b"occupied").unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(&occupied_path),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        fixture_reference_publish_topic().to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            sleep(Duration::from_millis(20)).await;

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor.on_position_opened(position_opened_event(
                    strategy_id,
                    up,
                    PositionId::from("P-RT-EXIT-PERSISTENCE-FAILED"),
                    Quantity::new(5.0, 2),
                    0.450,
                ));
            } else {
                panic!("runtime strategy actor should be registered");
            }
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;
            assert!(recorded_mock_exec_submissions().is_empty());

            std::fs::remove_file(&occupied_path).unwrap();
            std::fs::create_dir_all(&occupied_path).unwrap();
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms + 1),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 400, 3_102.0, 3_106.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(100)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "persistence_failed strategy must not submit again before restart"
    );
}

#[test]
fn eth_chainlink_taker_actor_materializes_same_session_entry_fill_by_client_order_id() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    {
        let cache_handle = node.kernel().cache();
        let mut cache = cache_handle.borrow_mut();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            let entry_submission = recorded_mock_exec_submissions()
                .into_iter()
                .next()
                .expect("expected entry submission");
            let entry_client_order_id = entry_submission.client_order_id;
            clear_mock_exec_submissions();

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor
                    .on_order_filled(&order_filled_event(
                        strategy_id,
                        entry_client_order_id,
                        up,
                        PositionId::from("P-ATTR-001"),
                        Quantity::new(5.0, 2),
                        Price::new(0.450, 3),
                    ))
                    .expect(
                        "entry fill should materialize position from submitted client order id",
                    );
            } else {
                panic!("runtime strategy actor should be registered");
            }

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
}

#[test]
fn eth_chainlink_taker_runtime_attributes_same_session_entry_fill_to_strategy() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    let cache_handle = node.kernel().cache();

    {
        let mut cache = cache_handle.borrow_mut();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            let entry_submission = recorded_mock_exec_submissions()
                .into_iter()
                .next()
                .expect("entry submission should be recorded");
            clear_mock_exec_submissions();

            publish_order_event(
                switchboard::get_event_orders_topic(strategy_id),
                &entry_fill_event(
                    strategy_id,
                    entry_submission.instrument_id,
                    entry_submission.client_order_id,
                    PositionId::from("P-RT-ENTRY-001"),
                ),
            );
            sleep(Duration::from_millis(50)).await;

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert!(submissions[0].client_order_id.to_string().starts_with('O'));
}

#[test]
fn eth_chainlink_taker_runtime_submits_down_entry_as_buy_on_down_ask() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    let cache_handle = node.kernel().cache();

    {
        let mut cache = cache_handle.borrow_mut();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_100.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_095.0, 3_094.0),
            );

            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.700, 0.720),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.280, 0.300),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, down);

    let order: OrderAny = cache_handle
        .borrow()
        .order(&submissions[0].client_order_id)
        .cloned()
        .expect("submitted order should be cached");
    assert_eq!(order.order_side(), OrderSide::Buy);
    assert_eq!(order.price(), Some(Price::new(0.300, 3)));
}

#[test]
fn eth_chainlink_taker_runtime_reducing_trading_state_allows_exit_order_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_config = strategy_raw_config();
    let decision_context = common_decision_context();
    let expected_client_id = decision_context.client_id.clone();
    let expected_nautilus_client_id = ClientId::from(expected_client_id.as_str());
    let configured_target_id = decision_context.configured_target_id.clone();
    let strategy_archetype = decision_context.strategy_archetype.clone();
    let strategy_id = strategy_id_from_raw_config(&strategy_config);
    let reference_topic = fixture_reference_publish_topic().to_string();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        decision_context,
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        reference_topic.clone(),
        Some(TradingState::Reducing),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, strategy_archetype.as_str(), &strategy_config).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    add_eth_entry_instruments(&mut node);
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor.on_position_opened(position_opened_event(
                    strategy_id,
                    up,
                    PositionId::from("P-RT-001"),
                    Quantity::new(5.0, 2),
                    0.450,
                ));
            } else {
                panic!("runtime strategy actor should be registered");
            }
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(expected_nautilus_client_id));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert!(submissions[0].client_order_id.to_string().starts_with('O'));

    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    let evaluation_trace_id = match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()
                .expect("BoltV3ExitEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, expected_client_id);
            assert_eq!(
                decoded.event_facts.get("exit_decision"),
                Some(&serde_json::Value::String("exit".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision_reason"),
                Some(&serde_json::Value::String("forced_flat".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_order_mechanical_outcome"),
                Some(&serde_json::Value::String("accepted".to_string()))
            );
            decoded.decision_trace_id.clone()
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    };

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), &configured_target_id);
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitOrderSubmissionDecisionEvent>()
                .expect("BoltV3ExitOrderSubmissionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, expected_client_id);
            assert_eq!(decoded.decision_trace_id, evaluation_trace_id);
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::String(
                    submissions[0].client_order_id.to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_fail_closed_exit_evaluation_before_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_config = strategy_raw_config();
    let decision_context = common_decision_context();
    let expected_client_id = ClientId::from(decision_context.client_id.as_str());
    let configured_target_id = decision_context.configured_target_id.clone();
    let strategy_archetype = decision_context.strategy_archetype.clone();
    let strategy_id = strategy_id_from_raw_config(&strategy_config);
    let reference_topic = fixture_reference_publish_topic().to_string();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        decision_context,
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        reference_topic.clone(),
        Some(TradingState::Reducing),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, strategy_archetype.as_str(), &strategy_config).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    add_eth_entry_instruments(&mut node);
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_100.0),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms + 200, 3_100.0, 3_099.5),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor.on_position_opened(position_opened_event(
                    strategy_id,
                    up,
                    PositionId::from("P-RT-EV-EXIT-001"),
                    Quantity::new(5.0, 2),
                    0.450,
                ));
            } else {
                panic!("runtime strategy actor should be registered");
            }
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms + 400, 3_100.0, 3_099.5),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(expected_client_id));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);

    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    let evaluation_trace_id = match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()
                .expect("BoltV3ExitEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("exit_decision"),
                Some(&serde_json::Value::String("exit".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision_reason"),
                Some(&serde_json::Value::String("fail_closed".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_order_mechanical_outcome"),
                Some(&serde_json::Value::String("accepted".to_string()))
            );
            decoded.decision_trace_id.clone()
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    };

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), &configured_target_id);
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitOrderSubmissionDecisionEvent>()
                .expect("BoltV3ExitOrderSubmissionDecisionEvent");
            assert_eq!(decoded.decision_trace_id, evaluation_trace_id);
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::String(
                    submissions[0].client_order_id.to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_ev_hysteresis_exit_evaluation_before_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_config = strategy_raw_config();
    let decision_context = common_decision_context();
    let expected_client_id = ClientId::from(decision_context.client_id.as_str());
    let configured_target_id = decision_context.configured_target_id.clone();
    let strategy_archetype = decision_context.strategy_archetype.clone();
    let strategy_id = strategy_id_from_raw_config(&strategy_config);
    let reference_topic = fixture_reference_publish_topic().to_string();
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        decision_context,
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        reference_topic.clone(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, strategy_archetype.as_str(), &strategy_config).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();
    add_eth_entry_instruments(&mut node);
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            let entry_submission = recorded_mock_exec_submissions()
                .into_iter()
                .next()
                .expect("entry submission should be recorded");
            clear_mock_exec_submissions();

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor
                    .on_order_filled(&order_filled_event(
                        strategy_id,
                        entry_submission.client_order_id,
                        entry_submission.instrument_id,
                        PositionId::from("P-RT-EV-HYSTERESIS-001"),
                        Quantity::new(5.0, 2),
                        Price::new(0.450, 3),
                    ))
                    .expect(
                        "entry fill should materialize position from submitted client order id",
                    );
            } else {
                panic!("runtime strategy actor should be registered");
            }
            sleep(Duration::from_millis(50)).await;

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(start_ts_ms + 400, 3_100.0, 3_094.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.500, 0.510),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(expected_client_id));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);

    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    let evaluation_trace_id = match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitEvaluationDecisionEvent>()
                .expect("BoltV3ExitEvaluationDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("exit_decision"),
                Some(&serde_json::Value::String("exit".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_decision_reason"),
                Some(&serde_json::Value::String("ev_hysteresis".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("exit_order_mechanical_outcome"),
                Some(&serde_json::Value::String("accepted".to_string()))
            );
            decoded.decision_trace_id.clone()
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    };

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), &configured_target_id);
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitOrderSubmissionDecisionEvent>()
                .expect("BoltV3ExitOrderSubmissionDecisionEvent");
            assert_eq!(decoded.decision_trace_id, evaluation_trace_id);
            assert_eq!(
                decoded.event_facts.get("client_order_id"),
                Some(&serde_json::Value::String(
                    submissions[0].client_order_id.to_string()
                ))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_bootstraps_cached_open_position_for_freeze_exit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();

    {
        let cache_handle = node.kernel().cache();
        let mut cache = cache_handle.borrow_mut();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }
    seed_cached_open_position(&node, strategy_id, up);

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );

            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(recorded_mock_exec_submissions().len(), 1);

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert!(submissions[0].client_order_id.to_string().starts_with('O'));
}

#[test]
fn eth_chainlink_taker_runtime_stays_fail_closed_with_multiple_cached_positions() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();

    {
        let cache_handle = node.kernel().cache();
        let mut cache = cache_handle.borrow_mut();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }
    seed_cached_position_with_entry(
        &node,
        strategy_id,
        up,
        OrderSide::Buy,
        Price::from("0.450"),
        PositionId::from("P-RECOVERY-001"),
    );
    seed_cached_position_with_entry(
        &node,
        strategy_id,
        down,
        OrderSide::Buy,
        Price::from("0.480"),
        PositionId::from("P-RECOVERY-002"),
    );

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.480, 0.490),
            );
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );
            sleep(Duration::from_millis(100)).await;

            assert!(
                recorded_mock_exec_submissions().is_empty(),
                "{:?}",
                recorded_mock_exec_submissions()
            );

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    assert!(recorded_mock_exec_submissions().is_empty());
}

#[test]
fn eth_chainlink_taker_runtime_keeps_exit_path_for_market_a_position_after_rotation_to_market_b() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let market_a_up = eth_up_instrument_id();
    let market_a_down = eth_down_instrument_id();
    let market_b_up = selected_market_up_instrument_id(RUNTIME_ROTATION_B_SELECTED_MARKET);
    let market_b_down = selected_market_down_instrument_id(RUNTIME_ROTATION_B_SELECTED_MARKET);

    {
        let cache_handle = node.kernel().cache();
        let mut cache = cache_handle.borrow_mut();
        for instrument_id in [market_a_up, market_a_down, market_b_up, market_b_down] {
            cache
                .add_instrument(polymarket_binary_option(instrument_id))
                .unwrap();
        }
    }

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let rotation_ts_ms = start_ts_ms + 1_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot_for(RUNTIME_DEFAULT_SELECTED_MARKET, start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_a_up),
                &book_deltas(market_a_up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_a_down),
                &book_deltas(market_a_down, 0.480, 0.490),
            );
            sleep(Duration::from_millis(50)).await;

            if let Some(mut actor) =
                try_get_actor_unchecked::<EthChainlinkTaker>(&strategy_id.inner())
            {
                actor.on_position_opened(position_opened_event(
                    strategy_id,
                    market_a_up,
                    PositionId::from("P-ROTATE-001"),
                    Quantity::new(5.0, 2),
                    0.450,
                ));
            } else {
                panic!("runtime strategy actor should be registered");
            }
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot_for(RUNTIME_ROTATION_B_SELECTED_MARKET, rotation_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(rotation_ts_ms, 3_101.0, 3_104.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_b_up),
                &book_deltas(market_b_up, 0.420, 0.440),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_b_down),
                &book_deltas(market_b_down, 0.500, 0.510),
            );
            sleep(Duration::from_millis(50)).await;
            assert!(
                recorded_mock_exec_submissions().is_empty(),
                "{:?}",
                recorded_mock_exec_submissions()
            );
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot_for(RUNTIME_ROTATION_B_SELECTED_MARKET, rotation_ts_ms),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, market_a_up);
}

#[test]
fn eth_chainlink_taker_runtime_exits_recovered_numeric_down_position_by_selling_held_down_at_best_bid()
 {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let market_a = candidate_market_from_fixture(RUNTIME_RECOVERY_A_SELECTED_MARKET, 0);
    let market_b = candidate_market_from_fixture(RUNTIME_RECOVERY_B_SELECTED_MARKET, 0);
    let market_a_up = selected_market_up_instrument_id(RUNTIME_RECOVERY_A_SELECTED_MARKET);
    let market_a_down = selected_market_down_instrument_id(RUNTIME_RECOVERY_A_SELECTED_MARKET);
    let market_b_up = selected_market_up_instrument_id(RUNTIME_RECOVERY_B_SELECTED_MARKET);
    let market_b_down = selected_market_down_instrument_id(RUNTIME_RECOVERY_B_SELECTED_MARKET);
    let cache_handle = node.kernel().cache();

    {
        let mut cache = cache_handle.borrow_mut();
        for instrument_id in [market_a_up, market_a_down, market_b_up, market_b_down] {
            cache
                .add_instrument(polymarket_binary_option(instrument_id))
                .unwrap();
        }
    }
    seed_cached_position_with_entry(
        &node,
        strategy_id,
        market_a_down,
        OrderSide::Buy,
        Price::from("0.480"),
        PositionId::from("P-RECOVERY-DOWN-001"),
    );

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let rotation_ts_ms = start_ts_ms + 1_000;
    let ruleset_id = selection_ruleset_id_from_fixture_config();
    let market_a_selection = RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id: ruleset_id.clone(),
            state: SelectionState::Active {
                market: CandidateMarket {
                    start_ts_ms,
                    ..market_a.clone()
                },
            },
        },
        eligible_candidates: vec![CandidateMarket {
            start_ts_ms,
            ..market_a.clone()
        }],
        published_at_ms: start_ts_ms,
    };
    let market_b_selection = RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id: ruleset_id.clone(),
            state: SelectionState::Active {
                market: CandidateMarket {
                    start_ts_ms: rotation_ts_ms,
                    ..market_b.clone()
                },
            },
        },
        eligible_candidates: vec![CandidateMarket {
            start_ts_ms: rotation_ts_ms,
            ..market_b.clone()
        }],
        published_at_ms: rotation_ts_ms,
    };
    let market_b_freeze = RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision {
            ruleset_id,
            state: SelectionState::Freeze {
                market: CandidateMarket {
                    start_ts_ms: rotation_ts_ms,
                    ..market_b.clone()
                },
                reason: SELECTION_FREEZE_WINDOW_REASON.to_string(),
            },
        },
        eligible_candidates: vec![CandidateMarket {
            start_ts_ms: rotation_ts_ms,
            ..market_b
        }],
        published_at_ms: rotation_ts_ms,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &market_a_selection,
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &market_b_selection,
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(rotation_ts_ms, 3_101.0, 3_104.0),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_b_up),
                &book_deltas(market_b_up, 0.420, 0.440),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_b_down),
                &book_deltas(market_b_down, 0.500, 0.510),
            );
            assert!(
                recorded_mock_exec_submissions().is_empty(),
                "{:?}",
                recorded_mock_exec_submissions()
            );

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &market_b_freeze,
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(market_a_down),
                &book_deltas(market_a_down, 0.520, 0.530),
            );

            for _ in 0..50 {
                if recorded_mock_exec_submissions().len() == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(test_exec_client_id()));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, market_a_down);

    let order: OrderAny = cache_handle
        .borrow()
        .order(&submissions[0].client_order_id)
        .cloned()
        .expect("submitted order should be cached");
    assert_eq!(order.order_side(), OrderSide::Sell);
    assert_eq!(order.price(), Some(Price::new(0.520, 3)));
}

#[test]
fn eth_chainlink_taker_runtime_does_not_trade_cached_legacy_short_position() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = strategy_id_from_fixture_config();
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            fixture_reference_publish_topic().to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let up = eth_up_instrument_id();
    let down = eth_down_instrument_id();

    {
        let cache_handle = node.kernel().cache();
        let mut cache = cache_handle.borrow_mut();
        cache.add_instrument(polymarket_binary_option(up)).unwrap();
        cache
            .add_instrument(polymarket_binary_option(down))
            .unwrap();
    }
    seed_cached_position_with_entry(
        &node,
        strategy_id,
        down,
        OrderSide::Sell,
        Price::from("0.480"),
        PositionId::from("P-LEGACY-SHORT-001"),
    );

    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let control = async {
            wait_for_running(&handle).await;
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                fixture_reference_publish_topic().to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );
            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &freeze_selection_snapshot(start_ts_ms),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(up, 0.430, 0.450),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(down, 0.520, 0.530),
            );
            sleep(Duration::from_millis(100)).await;

            assert!(
                recorded_mock_exec_submissions().is_empty(),
                "{:?}",
                recorded_mock_exec_submissions()
            );

            handle.stop();
        };

        let runner = async {
            node.run().await.unwrap();
        };

        tokio::join!(control, runner);
    });

    assert!(recorded_mock_exec_submissions().is_empty());
}
