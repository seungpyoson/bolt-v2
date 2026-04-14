mod support;

use std::{rc::Rc, sync::Arc, time::Duration};

use bolt_v2::{
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
    strategies::{eth_chainlink_taker::EthChainlinkTaker, production_strategy_registry},
};
use nautilus_common::{
    actor::registry::try_get_actor_unchecked,
    enums::Environment,
    logging::logger::LoggerConfig,
    msgbus::{publish_any, publish_deltas, switchboard},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, OrderBookDeltas},
    enums::{AssetClass, BookAction, LiquiditySide, OmsType, OrderSide, OrderType, PositionSide},
    events::OrderFilled,
    identifiers::{
        AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, TradeId,
        TraderId, VenueOrderId,
    },
    instruments::{InstrumentAny, binary_option::BinaryOption},
    position::Position,
    types::{Currency, Money, Price, Quantity},
};
use nautilus_trading::Strategy;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_exec_submissions, recorded_mock_exec_submissions,
};
use tokio::time::sleep;
use toml::Value;
#[derive(Debug, Default)]
struct StaticFeeProvider;

impl bolt_v2::clients::polymarket::FeeProvider for StaticFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<rust_decimal::Decimal> {
        Some(rust_decimal::Decimal::ZERO)
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

fn candidate_market(start_ts_ms: u64) -> CandidateMarket {
    CandidateMarket {
        market_id: "MKT-ETH-1".to_string(),
        instrument_id: "condition-eth-MKT-ETH-1-UP.POLYMARKET".to_string(),
        condition_id: "condition-eth".to_string(),
        up_token_id: "MKT-ETH-1-UP".to_string(),
        down_token_id: "MKT-ETH-1-DOWN".to_string(),
        start_ts_ms,
        declared_resolution_basis:
            bolt_v2::platform::resolution_basis::parse_ruleset_resolution_basis("chainlink_ethusd")
                .unwrap(),
        accepting_orders: true,
        liquidity_num: 1000.0,
        seconds_to_end: 300,
    }
}

fn selection_snapshot(start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Active {
                market: candidate_market(start_ts_ms),
            },
        },
        eligible_candidates: vec![candidate_market(start_ts_ms)],
        published_at_ms: start_ts_ms,
    }
}

fn freeze_selection_snapshot(start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    RuntimeSelectionSnapshot {
        ruleset_id: "PRIMARY".to_string(),
        decision: SelectionDecision {
            ruleset_id: "PRIMARY".to_string(),
            state: SelectionState::Freeze {
                market: candidate_market(start_ts_ms),
                reason: "freeze window".to_string(),
            },
        },
        eligible_candidates: vec![candidate_market(start_ts_ms)],
        published_at_ms: start_ts_ms,
    }
}

fn reference_snapshot(ts_ms: u64, fair_value: f64, fast_price: f64) -> ReferenceSnapshot {
    ReferenceSnapshot {
        ts_ms,
        topic: "platform.reference.test.chainlink".to_string(),
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
    OrderBookDeltas::new(
        instrument_id,
        vec![
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(
                    OrderSide::Buy,
                    Price::new(bid, 3),
                    Quantity::new(100.0, 2),
                    0,
                ),
                0,
                1,
                UnixNanos::default(),
                UnixNanos::default(),
            ),
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(
                    OrderSide::Sell,
                    Price::new(ask, 3),
                    Quantity::new(100.0, 2),
                    0,
                ),
                0,
                2,
                UnixNanos::default(),
                UnixNanos::default(),
            ),
        ],
    )
}

fn polymarket_binary_option(instrument_id: InstrumentId) -> InstrumentAny {
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
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
    let instrument = polymarket_binary_option(instrument_id);
    let mut fill = OrderFilled::new(
        TraderId::from("BOLT-001"),
        strategy_id,
        instrument_id,
        ClientOrderId::from("O-RECOVERY-ENTRY-001"),
        VenueOrderId::from("V-RECOVERY-ENTRY-001"),
        AccountId::from("TEST-ACCOUNT"),
        TradeId::from("E-RECOVERY-ENTRY-001"),
        OrderSide::Buy,
        OrderType::Market,
        Quantity::from("5"),
        Price::from("0.450"),
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::default(),
        UnixNanos::default(),
        false,
        None,
        Some(Money::from("0.01 USDC")),
    );
    fill.position_id = Some(PositionId::from("P-RECOVERY-001"));

    let position = Position::new(&instrument, fill);
    let cache_handle = node.kernel().cache();
    let mut cache = cache_handle.borrow_mut();
    cache.add_position(&position, OmsType::Netting).unwrap();
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

async fn wait_for_running(handle: &LiveNodeHandle) {
    while !handle.is_running() {
        sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn eth_chainlink_taker_runtime_submits_real_entry_order() {
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
        ),
    );
    strategy_factory(&trader, "eth_chainlink_taker", &strategy_raw_config()).unwrap();

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
fn eth_chainlink_taker_runtime_submits_exit_order_when_open_position_enters_freeze() {
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
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
    assert_eq!(submissions[0].client_id, Some(ClientId::from("TEST")));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert!(submissions[0].client_order_id.to_string().starts_with('O'));
}

#[test]
fn eth_chainlink_taker_runtime_bootstraps_cached_open_position_for_freeze_exit() {
    clear_mock_exec_submissions();

    let mut node = build_test_node();
    let trader = Rc::clone(node.kernel().trader());
    let strategy_id = StrategyId::from("ETHCHAINLINKTAKER-RT-001");
    let strategy_factory = registry_runtime_strategy_factory(
        production_strategy_registry().unwrap(),
        make_strategy_build_context(
            Arc::new(StaticFeeProvider),
            "platform.reference.test.chainlink".to_string(),
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
