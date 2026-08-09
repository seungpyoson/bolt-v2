#![cfg(test)]

use super::*;

pub(super) fn loaded_config_with_submit_sizer_recovery(
    temp_path: &std::path::Path,
) -> LoadedBoltV3Config {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded.strategies.clear();
    enable_fixture_kill_switch_for_enforced_provider_collateral_allowance(&mut loaded, temp_path);
    loaded
        .root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should configure capital pools")[0]
        .enforce_submit_admission = true;
    loaded.root.persistence.catalog_directory = std::fs::canonicalize(temp_path)
        .expect("temporary catalog must canonicalize")
        .to_string_lossy()
        .to_string();
    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = 100_000;
    crate::bolt_v3_current_evidence::prepare_test_generation(&loaded);
    loaded
}

pub(super) fn enable_fixture_kill_switch_for_enforced_provider_collateral_allowance(
    loaded: &mut LoadedBoltV3Config,
    temp_path: &std::path::Path,
) {
    loaded.root_path = temp_path.join("root.toml");
    let kill_switch = loaded
        .root
        .risk
        .kill_switch
        .as_mut()
        .expect("fixture should configure kill switch");
    kill_switch.enabled = true;
    std::fs::create_dir_all(temp_path.join("state")).expect("kill switch state dir should create");
    let store = crate::bolt_v3_kill_switch_store::KillSwitchStore::from_root_config_path(
        &loaded.root_path,
        kill_switch,
    );
    store
        .bootstrap_initial_armed_loss_snapshot()
        .expect("fixture kill switch state should bootstrap armed");
}

pub(super) fn fixture_reservation_attribution(
    client_order_id: &str,
    instrument_id: &str,
    side: &str,
    submitted_quantity: &str,
    liability_factor: &str,
    additive_liability: &str,
    reserved_liability: &str,
) -> crate::bolt_v3_current_evidence::ReservationAttribution {
    crate::bolt_v3_current_evidence::ReservationAttribution {
        client_order_id: client_order_id.to_string(),
        submit_reservation_id: format!("{client_order_id}#submit"),
        venue_id: "POLYMARKET".to_string(),
        account_id: "POLYMARKET-001".to_string(),
        product_kind:
            crate::bolt_v3_current_evidence::ReservationProductKind::PredictionMarketBinary,
        collateral_currency: "PUSD".to_string(),
        capital_pool_id: "polymarket-prediction-live".to_string(),
        collateral_group_id: "condition-fixture".to_string(),
        instrument_id: instrument_id.to_string(),
        side: match side {
            "buy" => crate::bolt_v3_current_evidence::EvidenceOrderSide::Buy,
            "sell" => crate::bolt_v3_current_evidence::EvidenceOrderSide::Sell,
            _ => panic!("fixture reservation side must be buy or sell"),
        },
        submitted_quantity: submitted_quantity.to_string(),
        liability_factor: liability_factor.to_string(),
        additive_liability: additive_liability.to_string(),
        reserved_liability: reserved_liability.to_string(),
        observed_at_ns: 1_000,
    }
}

pub(super) fn write_admitted_entry_reservation(
    loaded: &LoadedBoltV3Config,
    reservation: &crate::bolt_v3_current_evidence::ReservationAttribution,
) {
    let _committed = crate::bolt_v3_current_evidence::DecisionEvidenceRuntime::open(loaded)
        .expect("current decision evidence runtime should open")
        .recorder()
        .record_admitted_entry_admission(
            crate::bolt_v3_current_evidence::AdmittedEntryAdmissionFact {
                details: crate::bolt_v3_current_evidence::AdmissionDetails {
                    strategy_id: "strategy-1".to_string(),
                    execution_client_id: "execution-1".to_string(),
                    client_order_id: reservation.client_order_id.clone(),
                    instrument_id: reservation.instrument_id.clone(),
                    notional: reservation.reserved_liability.clone(),
                    loss_halt_reasons: Vec::new(),
                    snapshot_present: false,
                    snapshot_observed_at_ns: None,
                    admission_now_ns: reservation.observed_at_ns,
                    snapshot_age_ns: None,
                    max_snapshot_age_ns: None,
                    snapshot_source: None,
                    per_trade_pnl_present: false,
                    daily_pnl_present: false,
                    rolling_pnl_present: false,
                    current_equity_present: false,
                    peak_equity_present: false,
                    last_account_state_observed_at_ns: None,
                    last_portfolio_snapshot_observed_at_ns: None,
                    last_position_event_observed_at_ns: None,
                    stale_reason: None,
                    loss_snapshot_observed_at_ns: None,
                    loss_eval_now_ns: None,
                },
                reservation: Some(reservation.clone()),
            },
        )
        .expect("admitted reservation attribution should write");
}

pub(super) fn write_admitted_forced_reduction(
    loaded: &LoadedBoltV3Config,
    client_order_id: &str,
    instrument_id: &str,
) {
    let outcome = crate::bolt_v3_current_evidence::DecisionEvidenceRuntime::open(loaded)
        .expect("current decision evidence runtime should open")
        .recorder()
        .record_forced_reduction_admission(
            crate::bolt_v3_current_evidence::ForcedReductionAdmissionFact {
                details: crate::bolt_v3_current_evidence::AdmissionDetails {
                    strategy_id: "strategy-1".to_string(),
                    execution_client_id: "execution-1".to_string(),
                    client_order_id: client_order_id.to_string(),
                    instrument_id: instrument_id.to_string(),
                    notional: "1".to_string(),
                    loss_halt_reasons: Vec::new(),
                    snapshot_present: false,
                    snapshot_observed_at_ns: None,
                    admission_now_ns: 1_000,
                    snapshot_age_ns: None,
                    max_snapshot_age_ns: None,
                    snapshot_source: None,
                    per_trade_pnl_present: false,
                    daily_pnl_present: false,
                    rolling_pnl_present: false,
                    current_equity_present: false,
                    peak_equity_present: false,
                    last_account_state_observed_at_ns: None,
                    last_portfolio_snapshot_observed_at_ns: None,
                    last_position_event_observed_at_ns: None,
                    stale_reason: None,
                    loss_snapshot_observed_at_ns: None,
                    loss_eval_now_ns: None,
                },
                outcome: crate::bolt_v3_current_evidence::AdmissionDecisionOutcome::Admitted,
            },
        );
    assert!(matches!(
        outcome,
        crate::bolt_v3_current_evidence::NonBlockingRecordOutcome::Appended(_)
    ));
}

pub(super) fn fake_bolt_v3_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
    fixture_secret_value(path)
}

pub(super) fn fixture_secret_value(path: &str) -> Result<String, &'static str> {
    if path == "/bolt/binance_reference/api_secret" {
        return Ok("MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".to_string());
    }
    if path == "/bolt/binance_reference/api_key" {
        return Ok("binance-api-key".to_string());
    }
    if path.contains("private-key") || path.contains("private_key") {
        return Ok(
            "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
        );
    }
    if path.contains("api-secret") || path.contains("api_secret") {
        return Ok("YWJj".to_string());
    }
    if path.contains("api-passphrase") || path.contains("passphrase") {
        return Ok("polymarket-passphrase".to_string());
    }
    if path.contains("api-key") || path.contains("api_key") {
        return Ok("fixture-api-key".to_string());
    }
    Err("unexpected SSM path requested by bolt-v3 fake resolver")
}

pub(super) fn poison_mutex<T>(lock: &std::sync::Arc<std::sync::Mutex<T>>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = lock.lock().unwrap();
        panic!("seed poison");
    }));
}

pub(super) fn seed_cached_account_state(
    runtime: &BoltV3LiveNodeRuntime,
    account_id: &str,
    currency_code: &str,
    total: f64,
    free: f64,
) {
    let account_state = account_state_event(account_id, currency_code, total, free, 1);
    runtime
        .node
        .kernel()
        .cache()
        .borrow_mut()
        .update_account_state(&account_state)
        .expect("NT cache should apply account state");
}

pub(super) fn account_state_event(
    account_id: &str,
    currency_code: &str,
    total: f64,
    free: f64,
    timestamp_ns: u64,
) -> AccountState {
    let currency = test_currency(currency_code);
    AccountState::new(
        AccountId::from(account_id),
        AccountType::Cash,
        vec![AccountBalance::new(
            Money::new(total, currency),
            Money::new(total - free, currency),
            Money::new(free, currency),
        )],
        vec![],
        true,
        UUID4::default(),
        UnixNanos::from(timestamp_ns),
        UnixNanos::from(timestamp_ns),
        Some(currency),
    )
}

pub(super) fn test_currency(currency_code: &str) -> Currency {
    if currency_code == "PUSD" {
        return Currency::new("PUSD", 2, 0, "Test PUSD", CurrencyType::Fiat);
    }
    Currency::from(currency_code)
}

pub(super) fn seed_accepted_open_limit_order(
    runtime: &BoltV3LiveNodeRuntime,
    order: OrderAny,
    account_id: &str,
) {
    let cache = runtime.node.kernel().cache();
    let mut cache = cache.borrow_mut();
    cache
        .add_order(
            order.clone(),
            None,
            Some(ClientId::from("polymarket_main")),
            false,
        )
        .expect("NT cache should accept initialized order");
    cache
        .update_order(&OrderEventAny::Submitted(OrderSubmitted::new(
            order.trader_id(),
            order.strategy_id(),
            order.instrument_id(),
            order.client_order_id(),
            AccountId::from(account_id),
            UUID4::default(),
            UnixNanos::from(1),
            UnixNanos::from(1),
        )))
        .expect("NT cache should apply submitted event");
    cache
        .update_order(&OrderEventAny::Accepted(OrderAccepted::new(
            order.trader_id(),
            order.strategy_id(),
            order.instrument_id(),
            order.client_order_id(),
            VenueOrderId::from("venue-order-startup"),
            AccountId::from(account_id),
            UUID4::default(),
            UnixNanos::from(2),
            UnixNanos::from(2),
            false,
        )))
        .expect("NT cache should apply accepted event");
}

pub(super) fn generic_limit_order(
    client_order_id: &str,
    instrument_id: &str,
    order_side: OrderSide,
    quantity: Quantity,
    price: Price,
) -> OrderAny {
    OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from(instrument_id),
            ClientOrderId::from(client_order_id),
            order_side,
            quantity,
            price,
            TimeInForce::Gtc,
            None,
            false,
            false,
            false,
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
            UUID4::default(),
            UnixNanos::from(1),
        )
        .expect("generic limit order should be valid"),
    )
}

pub(super) fn generic_market_order(
    client_order_id: &str,
    instrument_id: &str,
    order_side: OrderSide,
    quantity: Quantity,
) -> OrderAny {
    OrderAny::Market(
        MarketOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from(instrument_id),
            ClientOrderId::from(client_order_id),
            order_side,
            quantity,
            TimeInForce::Ioc,
            UUID4::default(),
            UnixNanos::from(1),
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("generic market order should be valid"),
    )
}

pub(super) fn fixture_loaded_config() -> LoadedBoltV3Config {
    let root_text = include_str!("../../../tests/fixtures/bolt_v3/root.toml");
    let mut root: BoltV3RootConfig = toml::from_str(root_text).unwrap();
    let catalog_id = NEXT_TEST_CATALOG_ID.fetch_add(1, Ordering::Relaxed);
    let catalog_directory = std::env::temp_dir().join(format!(
        "bolt-v3-live-node-test-catalog-{}-{catalog_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&catalog_directory).expect("test catalog should create");
    root.persistence.catalog_directory = std::fs::canonicalize(catalog_directory)
        .expect("test catalog should canonicalize")
        .to_string_lossy()
        .to_string();
    let loaded = LoadedBoltV3Config {
        root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
        config_bundle_checksum: String::new(),
        root,
        strategies: Vec::new(),
    };
    crate::bolt_v3_current_evidence::prepare_test_generation(&loaded);
    loaded
}

pub(super) fn fixture_loaded_config_with_hyperliquid_standard_perps_route() -> LoadedBoltV3Config {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded.strategies.truncate(1);
    let strategy = loaded
        .strategies
        .first_mut()
        .expect("fixture should include one strategy");
    strategy.config.execution_client_id = ClientId::from("hyperliquid_perps");
    strategy.config.target = toml::toml! {
        configured_target_id = "hl-standard-perps-btc"
        kind = "static_instrument"
        rotating_market_family = "hyperliquid_instrument"
        product_surface = "standard_perps"
        instrument_id = "BTC-PERP.HYPERLIQUID"
        quantity_step = "0.001"
    }
    .into();
    loaded
}

pub(super) fn insert_configured_data_client(loaded: &mut LoadedBoltV3Config) {
    loaded.root.clients.insert(
        "configured-client".to_string(),
        toml::from_str(
            r#"
venue = "OKX"

[data]
configured_data_param = "configured-value"
"#,
        )
        .expect("configured data client should parse"),
    );
}

pub(super) fn test_okx_data_client() -> ClientBlock {
    toml::from_str(
        r#"
venue = "OKX"

[data]
book_stale_check_interval_secs = 0
book_stale_threshold_secs = 0
book_snapshot_timeout_secs = 3
"#,
    )
    .expect("test OKX data client should parse")
}

pub(super) fn test_execution_client(venue: &str) -> ClientBlock {
    ClientBlock {
        venue: Venue::from(venue),
        data: None,
        execution: Some(toml::Value::Table(toml::map::Map::new())),
        secrets: None,
        readiness_probe: None,
    }
}

pub(super) fn test_rv_source(
    source_id: &str,
    client_id: &str,
    instrument_id: &str,
    enabled: bool,
) -> RealizedVolatilitySourceBlock {
    RealizedVolatilitySourceBlock {
        source_id: source_id.to_string(),
        data_client_id: ClientId::from(client_id),
        instrument_id: InstrumentId::from(instrument_id),
        source_class: RealizedVolatilitySourceClassBlock::SpotQuote,
        sample_kind: RealizedVolatilitySampleKindBlock::Midpoint,
        enabled,
        counts_toward_quorum: enabled,
        canonical_base_asset: "BTC".to_string(),
        canonical_quote_asset: "USDT".to_string(),
    }
}

pub(super) fn test_rv_surface(
    sources: Vec<RealizedVolatilitySourceBlock>,
) -> RealizedVolatilitySurfaceBlock {
    RealizedVolatilitySurfaceBlock {
        canonical_base_asset: "CONFIGURED_ASSET".to_string(),
        canonical_quote_asset: "USDT".to_string(),
        policy: RealizedVolatilityPolicyBlock {
            window_ms: 600_000,
            sampling_interval_ms: 1_000,
            min_ready_sources: 1,
            max_source_age_ms: 60_000,
            max_inter_sample_gap_ms: 60_000,
            min_coverage_ratio: 0.5,
            max_cross_source_dispersion: 1.0,
            seconds_per_annum: 31_536_000.0,
            aggregation: RealizedVolatilityAggregationBlock::UpperQuantile,
            upper_quantile: 1.0,
            trim_fraction: None,
            guard_weight: None,
        },
        estimator: None,
        sources,
    }
}

pub(super) fn insert_test_rv_surface(
    loaded: &mut LoadedBoltV3Config,
    surface_id: &str,
    sources: Vec<RealizedVolatilitySourceBlock>,
) {
    loaded
        .root
        .realized_volatility_surfaces
        .get_or_insert_with(BTreeMap::new)
        .insert(surface_id.to_string(), test_rv_surface(sources));
}

pub(super) fn loaded_config_with_rv_only_source() -> LoadedBoltV3Config {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded
        .root
        .clients
        .insert("rv_only_data".to_string(), test_okx_data_client());
    insert_test_rv_surface(
        &mut loaded,
        "rv_only_surface",
        vec![test_rv_source(
            "rv_only_midpoint",
            "rv_only_data",
            "CONFIGURED_ASSET-USDT-RVONLY.OKX",
            true,
        )],
    );
    loaded
}

pub(super) fn test_registration_controls(
    writer: Arc<DecisionEvidenceRecorder>,
) -> BoltV3StrategyExecutionControls {
    BoltV3StrategyExecutionControls {
        submit_admission: Arc::new(BoltV3SubmitAdmissionState::new(writer)),
        order_execution_policy: crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        economics_inputs:
            crate::bolt_v3_economics_runtime::AuthoritativeEconomicsInputStore::default(),
        settlement_runtime_sink: None,
        settlement_recovery: None,
        booking_recovery: None,
        settlement_health_transition_emitter: None,
    }
}

pub(super) fn fixture_loaded_config_with_external_option_greeks_iv() -> LoadedBoltV3Config {
    let mut loaded = fixture_loaded_config();
    loaded.root.clients.clear();
    insert_configured_data_client(&mut loaded);
    loaded.root.nautilus.data_engine.external_clients = vec![ClientId::from("configured-client")];
    loaded.root.iv = Some(
        toml::from_str(
            r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
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
eligible_sources = ["configured-greeks-source"]

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
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
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
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
        )
        .expect("configured IV profile should parse"),
    );
    loaded
}
