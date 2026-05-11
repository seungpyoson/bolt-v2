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
        CatalogFsProtocol, PersistenceBlock, RotationKind, StreamingBlock, load_bolt_v3_config,
    },
    bolt_v3_decision_event_context::BoltV3DecisionEventCommonContext,
    bolt_v3_decision_events::{
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
        BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
        BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
        BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE, BOLT_V3_MARKET_SELECTION_FAILURE_REASONS,
        BoltV3EntryEvaluationDecisionEvent, BoltV3EntryOrderSubmissionDecisionEvent,
        BoltV3EntryPreSubmitRejectionDecisionEvent, BoltV3ExitEvaluationDecisionEvent,
        BoltV3ExitOrderSubmissionDecisionEvent, BoltV3ExitPreSubmitRejectionDecisionEvent,
        BoltV3MarketSelectionDecisionEvent,
    },
    bolt_v3_strategy_decision_evidence::BoltV3StrategyDecisionEvidence,
    config::Config,
    live_node_setup::{
        DataClientRegistration, ExecClientRegistration, build_live_node, make_live_node_config,
        make_strategy_build_context,
    },
    platform::{
        reference::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind},
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState},
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

fn runtime_test_mutex() -> &'static Mutex<()> {
    RUNTIME_TEST_MUTEX.get_or_init(|| Mutex::new(()))
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
    let trader_id = TraderId::from("BOLT-001");
    let data_config = MockDataClientConfig::new("TESTDATA", "POLYMARKET");
    let exec_config = MockExecClientConfig::new("TEST", "TEST-ACCOUNT", "POLYMARKET");
    let cfg: Config = toml::from_str(
        r#"
        [node]
        name = "ETH-TAKER-RT"
        trader_id = "BOLT-001"
        environment = "Live"
        load_state = false
        save_state = false
        timeout_connection_secs = 1
        timeout_reconciliation_secs = 1
        timeout_portfolio_secs = 1
        timeout_disconnection_secs = 1
        delay_post_stop_secs = 0
        delay_shutdown_secs = 0

        [logging]
        stdout_level = "Info"
        file_level = "Debug"

        [[data_clients]]
        name = "TESTDATA"
        type = "polymarket"
        [data_clients.config]
        subscribe_new_markets = false
        update_instruments_interval_mins = 60
        ws_max_subscriptions = 200
        event_slugs = ["eth-updown-5m"]

        [[exec_clients]]
        name = "TEST"
        type = "polymarket"
        [exec_clients.config]
        account_id = "POLYMARKET-001"
        signature_type = 2
        funder = "0xabc"
        [exec_clients.secrets]
        region = "us-east-1"
        pk = "/pk"
        api_key = "/key"
        api_secret = "/secret"
        passphrase = "/pass"
        "#,
    )
    .unwrap();

    let live_node_config =
        make_live_node_config(&cfg, trader_id, Environment::Live, LoggerConfig::default());
    let data_clients: Vec<DataClientRegistration> = vec![(
        Some("TESTDATA".to_string()),
        Box::new(MockDataClientFactory),
        Box::new(data_config),
    )];
    let exec_clients: Vec<ExecClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockExecutionClientFactory),
        Box::new(exec_config),
    )];

    build_live_node(
        "ETH-TAKER-RT".to_string(),
        live_node_config,
        data_clients,
        exec_clients,
    )
    .unwrap()
}

fn strategy_raw_config() -> Value {
    toml::toml! {
        strategy_id = "ETHCHAINLINKTAKER-RT-001"
        client_id = "TEST"
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
    .into()
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

fn fixture_reference_publish_topic() -> &'static str {
    "platform.reference.test.chainlink"
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
    let strategy_config = strategy_raw_config();
    BoltV3DecisionEventCommonContext {
        schema_version: 1,
        strategy_instance_id: strategy_id_from_raw_config(&strategy_config).to_string(),
        strategy_archetype: ETH_CHAINLINK_TAKER_KIND.to_string(),
        trader_id: "BOLT-001".to_string(),
        client_id: strategy_config
            .get("client_id")
            .and_then(Value::as_str)
            .expect("test strategy config must include client_id")
            .to_string(),
        venue: "POLYMARKET".to_string(),
        runtime_mode: "live".to_string(),
        release_id: "release-sha".to_string(),
        config_hash: "config-hash".to_string(),
        nautilus_trader_revision: "38b912a8b0fe14e4046773973ff46a3b798b1e3e".to_string(),
        configured_target_id: "target-eth-updown".to_string(),
    }
}

fn configured_target_id_from_decision_context() -> String {
    common_decision_context().configured_target_id
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
                            "selected_market_open_orders_present".to_string(),
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

fn candidate_market_named(market_id: &str, start_ts_ms: u64) -> CandidateMarket {
    candidate_market_with_tokens(
        market_id,
        "condition-eth",
        &format!("{market_id}-UP"),
        &format!("{market_id}-DOWN"),
        start_ts_ms,
    )
}

fn candidate_market_with_tokens(
    market_id: &str,
    condition_id: &str,
    up_token_id: &str,
    down_token_id: &str,
    start_ts_ms: u64,
) -> CandidateMarket {
    CandidateMarket {
        market_id: market_id.to_string(),
        market_slug: market_id.to_string(),
        question_id: format!("question-{market_id}"),
        instrument_id: format!("{condition_id}-{up_token_id}.POLYMARKET"),
        condition_id: condition_id.to_string(),
        up_token_id: up_token_id.to_string(),
        down_token_id: down_token_id.to_string(),
        selected_market_observed_ts_ms: start_ts_ms,
        price_to_beat: None,
        price_to_beat_source: None,
        price_to_beat_observed_ts_ms: None,
        start_ts_ms,
        end_ts_ms: start_ts_ms + 300_000,
        declared_resolution_basis:
            bolt_v2::platform::resolution_basis::parse_ruleset_resolution_basis("chainlink_ethusd")
                .unwrap(),
        accepting_orders: true,
        liquidity_num: 1000.0,
        seconds_to_end: 300,
    }
}

fn selection_snapshot(start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    selection_snapshot_for("MKT-ETH-1", start_ts_ms)
}

fn selection_snapshot_with_market_facts(
    start_ts_ms: u64,
    price_to_beat_observed_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let mut market = candidate_market_named("MKT-ETH-1", start_ts_ms);
    market.price_to_beat = Some(3_100.0);
    market.price_to_beat_source = Some("polymarket_gamma_market_anchor".to_string());
    market.price_to_beat_observed_ts_ms = Some(price_to_beat_observed_ts_ms);
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms: start_ts_ms,
    }
}

fn idle_selection_snapshot(reason: &str, published_at_ms: u64) -> RuntimeSelectionSnapshot {
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Idle {
                reason: reason.to_string(),
            },
        },
        eligible_candidates: Vec::new(),
        published_at_ms,
    }
}

fn selection_snapshot_for(market_id: &str, start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Active {
                market: candidate_market_named(market_id, start_ts_ms),
            },
        },
        eligible_candidates: vec![candidate_market_named(market_id, start_ts_ms)],
        published_at_ms: start_ts_ms,
    }
}

fn future_selection_snapshot(
    published_at_ms: u64,
    market_start_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let mut market = candidate_market_named("MKT-ETH-1", market_start_ts_ms);
    market.selected_market_observed_ts_ms = published_at_ms;
    market.end_ts_ms = market_start_ts_ms + 300_000;
    market.seconds_to_end = (market.end_ts_ms - published_at_ms) / 1_000;
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
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
    let mut market = candidate_market_named("MKT-ETH-1", market_start_ts_ms);
    market.selected_market_observed_ts_ms = published_at_ms;
    market.end_ts_ms = market_end_ts_ms;
    market.seconds_to_end = (market.end_ts_ms - published_at_ms) / 1_000;
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms,
    }
}

fn freeze_selection_snapshot(start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    freeze_selection_snapshot_for("MKT-ETH-1", start_ts_ms)
}

fn freeze_selection_snapshot_for(market_id: &str, start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Freeze {
                market: candidate_market_named(market_id, start_ts_ms),
                reason: "freeze window".to_string(),
            },
        },
        eligible_candidates: vec![candidate_market_named(market_id, start_ts_ms)],
        published_at_ms: start_ts_ms,
    }
}

fn reference_snapshot(ts_ms: u64, fair_value: f64, fast_price: f64) -> ReferenceSnapshot {
    ReferenceSnapshot {
        ts_ms,
        topic: fixture_reference_publish_topic().to_string(),
        fair_value: Some(fair_value),
        confidence: 1.0,
        venues: vec![
            EffectiveVenueState {
                venue_name: "chainlink".to_string(),
                base_weight: 1.0,
                effective_weight: 1.0,
                stale: false,
                health: VenueHealth::Healthy,
                observed_ts_ms: Some(ts_ms),
                venue_kind: VenueKind::Oracle,
                observed_price: Some(fair_value),
                observed_bid: None,
                observed_ask: None,
            },
            EffectiveVenueState {
                venue_name: "bybit".to_string(),
                base_weight: 0.9,
                effective_weight: 0.9,
                stale: false,
                health: VenueHealth::Healthy,
                observed_ts_ms: Some(ts_ms),
                venue_kind: VenueKind::Orderbook,
                observed_price: Some(fast_price),
                observed_bid: Some(fast_price - 0.5),
                observed_ask: Some(fast_price + 0.5),
            },
        ],
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
        Price::from("0.450"),
        PositionId::from("P-RECOVERY-001"),
    );
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
        TraderId::from("BOLT-001"),
        strategy_id,
        instrument_id,
        ClientOrderId::from("O-RECOVERY-ENTRY-001"),
        VenueOrderId::from("V-RECOVERY-ENTRY-001"),
        AccountId::from("TEST-ACCOUNT"),
        TradeId::from("E-RECOVERY-ENTRY-001"),
        entry_order_side,
        OrderType::Market,
        Quantity::from("5"),
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
        .trader_id(TraderId::from("BOLT-001"))
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
        AccountId::from("TEST-ACCOUNT"),
        UUID4::new(),
        UnixNanos::default(),
        UnixNanos::default(),
        false,
    );
    order.apply(OrderEventAny::Accepted(accepted)).unwrap();

    let cache_handle = node.kernel().cache();
    let mut cache = cache_handle.borrow_mut();
    cache
        .add_order(order.clone(), None, Some(ClientId::from("TEST")), false)
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
        trader_id: TraderId::from("BOLT-001"),
        strategy_id,
        instrument_id,
        position_id,
        account_id: AccountId::from("TEST-ACCOUNT"),
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
        TraderId::from("BOLT-001"),
        strategy_id,
        instrument_id,
        client_order_id,
        VenueOrderId::from("V-ENTRY-RT-001"),
        AccountId::from("TEST-ACCOUNT"),
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
        TraderId::from("BOLT-001"),
        strategy_id,
        instrument_id,
        client_order_id,
        VenueOrderId::from("V-RT-ENTRY-001"),
        AccountId::from("TEST-ACCOUNT"),
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
    InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET")
}

fn eth_down_instrument_id() -> InstrumentId {
    InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET")
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

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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

fn drive_eth_entry_after_cached_position_no_action(mut node: LiveNode, strategy_id: StrategyId) {
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

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            seed_cached_position_with_entry_in_cache(
                &cache_handle,
                strategy_id,
                up,
                OrderSide::Buy,
                Price::from("0.450"),
                PositionId::from("P-RT-FILLED-CAPACITY"),
            );

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &selection_snapshot(start_ts_ms),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms + 1, 3_101.0, 3_105.0),
            );
            sleep(Duration::from_millis(20)).await;

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(market_end_ts_ms + 250, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(market_end_ts_ms + 450, 3_101.0, 3_105.0),
            );

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
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
    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
        let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
        let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms + 200, 3_101.0, 3_105.0),
            );

            let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
            let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(
        submissions[0].instrument_id,
        InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET")
    );
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

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), &configured_target_id);
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
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
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

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
        (TradingState::Halted, "trading_state_halted"),
        (TradingState::Reducing, "trading_state_reducing"),
    ] {
        clear_mock_exec_submissions();

        let temp_dir = TempDir::new().unwrap();
        let mut node = build_test_node();
        add_eth_entry_instruments(&mut node);
        let trader = Rc::clone(node.kernel().trader());
        let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
        let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
            common_decision_context(),
            &decision_persistence_block(temp_dir.path()),
        )
        .unwrap();
        let mut build_context = make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(trading_state),
        );
        build_context.bolt_v3_decision_evidence = Some(evidence);
        let strategy_factory = registry_runtime_strategy_factory(
            production_strategy_registry().unwrap(),
            build_context,
        );
        strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

        drive_eth_entry_submission(node, strategy_id);

        assert!(
            recorded_mock_exec_submissions().is_empty(),
            "{trading_state:?} trading state must block entry submit before NT execution"
        );

        let rejection_events =
            query_entry_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
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
            query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    build_context.bolt_v3_market_selection_context = Some(BoltV3MarketSelectionContext {
        market_selection_type: "rotating_market".to_string(),
        rotating_market_family: Some("updown".to_string()),
        underlying_asset: Some("ETH".to_string()),
        cadence_seconds: Some(300),
        market_selection_rule: Some("active_or_next".to_string()),
        retry_interval_seconds: Some(5),
        blocked_after_seconds: Some(60),
    });
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, ETH_CHAINLINK_TAKER_KIND, &strategy_raw_config()).unwrap();

    let handle = node.handle();
    let start_ts_ms = 1_000;
    let price_to_beat_observed_ts_ms = 900;
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

    let market_selection_events =
        query_market_selection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(market_selection_events.len(), 1);
    match &market_selection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3MarketSelectionDecisionEvent>()
                .expect("BoltV3MarketSelectionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
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
                Some(&serde_json::Value::String("condition-eth".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("polymarket_market_slug"),
                Some(&serde_json::Value::String("MKT-ETH-1".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("polymarket_question_id"),
                Some(&serde_json::Value::String("question-MKT-ETH-1".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("up_instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("down_instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-DOWN.POLYMARKET".to_string()
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
    build_context.bolt_v3_market_selection_context = Some(BoltV3MarketSelectionContext {
        market_selection_type: "rotating_market".to_string(),
        rotating_market_family: Some("updown".to_string()),
        underlying_asset: Some("ETH".to_string()),
        cadence_seconds: Some(300),
        market_selection_rule: Some("active_or_next".to_string()),
        retry_interval_seconds: Some(5),
        blocked_after_seconds: Some(60),
    });
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

    let market_selection_events =
        query_market_selection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(market_selection_events.len(), 1);
    match &market_selection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3MarketSelectionDecisionEvent>()
                .expect("BoltV3MarketSelectionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
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

    let evaluation_events =
        query_entry_evaluation_events_all_files(temp_dir.path(), "target-eth-updown");
    assert_eq!(evaluation_events.len(), 1);
    let evaluation_trace_id = match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("enter".to_string()))
            );
            let updown_side = decoded
                .event_facts
                .get("updown_side")
                .and_then(serde_json::Value::as_str)
                .expect("entry evaluation updown_side should be present");
            assert!(
                matches!(updown_side, "up" | "down"),
                "unexpected updown_side {updown_side}"
            );
            decoded.decision_trace_id.clone()
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    };

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(submission_events.len(), 1);
    match &submission_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryOrderSubmissionDecisionEvent>()
                .expect("BoltV3EntryOrderSubmissionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
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
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String("insufficient_edge".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

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
                Some(&serde_json::Value::String("thin_book".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("updown_market_mechanical_outcome"),
                Some(&serde_json::Value::String("accepted".to_string()))
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_missing_reference_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "missing-reference entry evaluation must not submit order"
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
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "missing_reference_quote".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(MissingFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "fee-rate-unavailable entry evaluation must not submit order"
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
            assert_eq!(decoded.strategy_instance_id, strategy_id.to_string());
            assert_eq!(decoded.client_id, common_decision_context().client_id);
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "fee_rate_unavailable".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), &configured_target_id);
    assert!(
        submission_events.is_empty(),
        "fee-rate-unavailable entry evaluation must not persist order submission"
    );
}

#[test]
fn eth_chainlink_taker_runtime_writes_stale_reference_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
        &strategy_raw_config_with_stale_reference_window(1),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_stale_reference_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "stale-reference entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "stale_reference_quote".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
                "fast_venue_incoherent".to_string(),
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
                    "fast_venue_incoherent".to_string()
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
fn eth_chainlink_taker_runtime_writes_fair_probability_unavailable_no_action_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
        &strategy_raw_config_with_pricing_kurtosis(-6.0),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "fair-probability-unavailable entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "fair_probability_unavailable".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("updown_side"),
                Some(&serde_json::Value::Null)
            );
        }
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
        &strategy_raw_config_with_max_position_usdc(0.0),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "position-limit-reached entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "position_limit_reached".to_string()
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

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
        &strategy_raw_config_with_max_position_usdc(45.0),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
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

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
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
                    "updown_market_mechanical_rejection".to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("updown_market_mechanical_rejection_reason"),
                Some(&serde_json::Value::String(
                    "selected_market_open_orders_present".to_string()
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
fn eth_chainlink_taker_runtime_writes_filled_entry_capacity_from_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
        &strategy_raw_config_with_max_position_usdc(2.25),
    )
    .unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_after_cached_position_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "filled entry capacity from NT cache must not submit another order"
    );

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
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
                    "position_limit_reached".to_string()
                ))
            );
            assert_eq!(
                decoded
                    .event_facts
                    .get("entry_filled_notional")
                    .and_then(serde_json::Value::as_f64),
                Some(2.25)
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_market_not_started_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "market-not-started mechanical entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "updown_market_mechanical_rejection".to_string()
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

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_entry_market_ended_no_action(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "market-ended mechanical entry evaluation must not submit order"
    );

    let evaluation_events = query_entry_evaluation_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(evaluation_events.len(), 1);
    match &evaluation_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
                .expect("BoltV3EntryEvaluationDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_decision"),
                Some(&serde_json::Value::String("no_action".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("entry_no_action_reason"),
                Some(&serde_json::Value::String(
                    "updown_market_mechanical_rejection".to_string()
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

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
                "selected_market_open_orders_present".to_string(),
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
                    "updown_market_mechanical_rejection".to_string()
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
                    "selected_market_open_orders_present".to_string()
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    drive_eth_entry_pre_submit_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "pre-submit rejection must not submit order"
    );

    let rejection_events =
        query_entry_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3EntryPreSubmitRejectionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    "instrument_missing_from_cache".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
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

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(
        &trader,
        "eth_chainlink_taker",
        &strategy_raw_config_with_max_position_usdc(0.1),
    )
    .unwrap();

    add_eth_entry_instruments_with_size_increment(&mut node, Quantity::from("1"));
    drive_eth_entry_pre_submit_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "invalid quantity rejection must not submit order"
    );

    let rejection_events =
        query_entry_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3EntryPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3EntryPreSubmitRejectionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("entry_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String("invalid_quantity".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
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

    let submission_events =
        query_entry_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_exit_pre_submit_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "exit pre-submit rejection must not submit order"
    );

    let rejection_events =
        query_exit_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3ExitPreSubmitRejectionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String("exit_price_missing".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
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
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), "target-eth-updown");
    assert!(
        submission_events.is_empty(),
        "exit pre-submit rejection must not persist order submission"
    );

    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), "target-eth-updown");
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
                    "exit_order_mechanical_rejection".to_string()
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
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_writes_exit_invalid_quantity_pre_submit_rejection_without_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

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

    let rejection_events =
        query_exit_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3ExitPreSubmitRejectionDecisionEvent");
            assert_eq!(decoded.strategy_instance_id, "ETHCHAINLINKTAKER-RT-001");
            assert_eq!(decoded.client_id, "TEST");
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String("invalid_quantity".to_string()))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
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
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), "target-eth-updown");
    assert!(
        submission_events.is_empty(),
        "exit invalid quantity rejection must not persist order submission"
    );

    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), "target-eth-updown");
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
                    "exit_order_mechanical_rejection".to_string()
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
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

    let rejection_events =
        query_exit_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3ExitPreSubmitRejectionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    "exit_quantity_exceeds_sellable_quantity".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
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
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), "target-eth-updown");
    assert!(
        submission_events.is_empty(),
        "exit sellable quantity rejection must not persist order submission"
    );

    let evaluation_events = query_exit_evaluation_events(temp_dir.path(), "target-eth-updown");
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
                    "exit_order_mechanical_rejection".to_string()
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
        other => panic!("expected Data::Custom, got {other:?}"),
    }
}

#[test]
fn eth_chainlink_taker_runtime_halted_trading_state_blocks_exit_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let temp_dir = TempDir::new().unwrap();
    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Halted),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    drive_eth_exit_sellable_rejection(node, strategy_id);

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "HALTED trading state must block exit submit before NT execution"
    );

    let rejection_events =
        query_exit_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
    assert_eq!(rejection_events.len(), 1);
    match &rejection_events[0] {
        nautilus_model::data::Data::Custom(custom) => {
            let decoded = custom
                .data
                .as_any()
                .downcast_ref::<BoltV3ExitPreSubmitRejectionDecisionEvent>()
                .expect("BoltV3ExitPreSubmitRejectionDecisionEvent");
            assert_eq!(
                decoded.event_facts.get("exit_pre_submit_rejection_reason"),
                Some(&serde_json::Value::String(
                    "trading_state_halted".to_string()
                ))
            );
            assert_eq!(
                decoded.event_facts.get("instrument_id"),
                Some(&serde_json::Value::String(
                    "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string()
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
        other => panic!("expected Data::Custom, got {other:?}"),
    }

    let submission_events =
        query_exit_order_submission_events(temp_dir.path(), "target-eth-updown");
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(temp_dir.path()),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);

    let rejection_events =
        query_exit_pre_submit_rejection_events(temp_dir.path(), "target-eth-updown");
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
                        "exit_quantity_exceeds_sellable_quantity"
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(&occupied_path),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(&occupied_path),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let evidence = BoltV3StrategyDecisionEvidence::from_persistence_block(
        common_decision_context(),
        &decision_persistence_block(&occupied_path),
    )
    .unwrap();
    let mut build_context = make_strategy_build_context(
        Arc::new(StaticFeeProvider),
        "platform.reference.test.chainlink".to_string(),
        Some(TradingState::Active),
    );
    build_context.bolt_v3_decision_evidence = Some(evidence);
    let strategy_factory =
        registry_runtime_strategy_factory(production_strategy_registry().unwrap(), build_context);
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    add_eth_entry_instruments(&mut node);
    let handle = node.handle();
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
                "platform.reference.test.chainlink".to_string().into(),
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
}

#[test]
fn eth_chainlink_taker_runtime_attributes_same_session_entry_fill_to_strategy() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_100.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");

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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");

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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let market_a_up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let market_a_down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");
    let market_b_up = InstrumentId::from("condition-eth-MKT-ETH-2-UP.POLYMARKET");
    let market_b_down = InstrumentId::from("condition-eth-MKT-ETH-2-DOWN.POLYMARKET");

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
                &selection_snapshot_for("MKT-ETH-1", start_ts_ms),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
                &selection_snapshot_for("MKT-ETH-2", rotation_ts_ms),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
                &freeze_selection_snapshot_for("MKT-ETH-2", rotation_ts_ms),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let market_a = candidate_market_with_tokens("MKT-ETH-A", "condition-eth-a", "111", "222", 0);
    let market_b = candidate_market_with_tokens("MKT-ETH-B", "condition-eth-b", "333", "444", 0);
    let market_a_up = InstrumentId::from("condition-eth-a-111.POLYMARKET");
    let market_a_down = InstrumentId::from("condition-eth-a-222.POLYMARKET");
    let market_b_up = InstrumentId::from("condition-eth-b-333.POLYMARKET");
    let market_b_down = InstrumentId::from("condition-eth-b-444.POLYMARKET");
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
    let market_a_selection = RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
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
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
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
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Freeze {
                market: CandidateMarket {
                    start_ts_ms: rotation_ts_ms,
                    ..market_b.clone()
                },
                reason: "freeze window".to_string(),
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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            clear_mock_exec_submissions();

            publish_any(
                runtime_selection_topic(&strategy_id).into(),
                &market_b_selection,
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
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
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
            Some(TradingState::Active),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

    let up = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET");
    let down = InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET");

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
                "platform.reference.test.chainlink".to_string().into(),
                &reference_snapshot(start_ts_ms, 3_100.0, 3_102.0),
            );
            publish_any(
                "platform.reference.test.chainlink".to_string().into(),
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
