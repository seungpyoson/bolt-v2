mod support;

use std::fs;

#[test]
fn bolt_v3_config_uses_nautilus_vocabulary_field_names() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::ClientId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let strategy = &loaded.strategies[0].config;
    assert_eq!(
        strategy.execution_client_id,
        nautilus_model::identifiers::ClientId::from("polymarket_main")
    );
    assert_eq!(
        strategy.reference_data["primary"].data_client_id,
        nautilus_model::identifiers::ClientId::from("binance_reference")
    );

    let data_engine = &loaded.root.nautilus.data_engine;
    let exec_engine = &loaded.root.nautilus.exec_engine;
    assert!(data_engine.external_clients.is_empty());
    assert!(exec_engine.external_clients.is_empty());

    let _typed_check: &Vec<ClientId> = &data_engine.external_clients;
}

#[test]
fn bolt_v3_config_uses_clients_section_with_nt_venue_identifier() {
    // FINDING-3: TOML `[clients.<id>]` holds an NT Venue identifier in
    // `venue`. The NT Venue type wraps a Ustr and the serde macro enforces
    // correctness at parse time, so the `venue = "POLYMARKET"` value is
    // checked structurally as well as semantically.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::Venue;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let clients = &loaded.root.clients;
    assert!(clients.contains_key("polymarket_main"));
    assert!(clients.contains_key("binance_reference"));

    let polymarket = &clients["polymarket_main"];
    assert_eq!(polymarket.venue, Venue::from("POLYMARKET"));
    assert!(polymarket.execution.is_some());

    let binance = &clients["binance_reference"];
    assert_eq!(binance.venue, Venue::from("BINANCE"));
    assert!(binance.execution.is_none());
}

#[test]
fn bolt_v3_root_trader_id_uses_nt_typed_identifier() {
    // `BoltV3RootConfig.trader_id` is typed as `nautilus_model::identifiers::TraderId`
    // so the NT identifier macro rejects empty strings at parse time instead of
    // leaving that as a bolt-side runtime check.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::TraderId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let trader_id: TraderId = loaded.root.trader_id;
    assert_eq!(trader_id, TraderId::from("BOLT-001"));
}

#[test]
fn bolt_v3_root_trader_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let mutated = replace_in_fixture_root("trader_id = \"BOLT-001\"", "trader_id = \"\"");
    let err = toml::from_str::<BoltV3RootConfig>(&mutated)
        .expect_err("empty trader_id should be rejected by NT TraderId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty trader_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_polymarket_account_id_uses_nt_typed_identifier() {
    // `PolymarketExecutionConfig.account_id` is typed as
    // `nautilus_model::identifiers::AccountId` so NT's identifier macro
    // rejects empty / invalid strings at parse time and the bolt
    // execution-config binding holds the same typed value the NT
    // PolymarketExecClientConfig expects, eliminating the
    // `AccountId::from(_.as_str())` round-trip.
    use bolt_v2::bolt_v3_providers::polymarket::PolymarketExecutionConfig;
    use nautilus_model::identifiers::AccountId;

    let exec_toml = r#"
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
ack_timeout_secs = 5
fee_cache_ttl_secs = 300
transport_backend = "sockudo"
"#;
    let parsed: PolymarketExecutionConfig =
        toml::from_str(exec_toml).expect("polymarket execution block should parse");
    let account_id: AccountId = parsed.account_id;
    assert_eq!(account_id, AccountId::from("POLYMARKET-001"));
}

#[test]
fn bolt_v3_polymarket_account_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_providers::polymarket::PolymarketExecutionConfig;

    let exec_toml = r#"
account_id = ""
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
ack_timeout_secs = 5
fee_cache_ttl_secs = 300
transport_backend = "sockudo"
"#;
    let err = toml::from_str::<PolymarketExecutionConfig>(exec_toml)
        .expect_err("empty account_id should be rejected by NT AccountId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty account_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_polymarket_and_nautilus_config_rejects_nt_field_aliases() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig,
        bolt_v3_providers::polymarket::{PolymarketDataConfig, PolymarketExecutionConfig},
    };

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let nt_named = fixture;

    let parsed: BoltV3RootConfig =
        toml::from_str(&nt_named).expect("NT-owned field names should parse");
    let polymarket = parsed
        .clients
        .get("polymarket_main")
        .expect("polymarket fixture client should exist");
    let data: PolymarketDataConfig = polymarket
        .data
        .clone()
        .expect("polymarket data block should exist")
        .try_into()
        .expect("polymarket data block should parse with NT names");
    assert_eq!(data.update_instruments_interval_mins, 60);
    assert_eq!(data.ws_max_subscriptions, 200);
    assert!(!data.auto_load_missing_instruments);
    assert_eq!(data.auto_load_debounce_ms, 250);
    let execution: PolymarketExecutionConfig = polymarket
        .execution
        .clone()
        .expect("polymarket execution block should exist")
        .try_into()
        .expect("polymarket execution block should parse with NT names");
    assert_eq!(
        execution.funder.as_deref(),
        Some("0x1111111111111111111111111111111111111111")
    );
    assert_eq!(parsed.nautilus.timeout_shutdown_secs, 10);

    let old_update = ["update_instruments", "_interval", "_minutes"].concat();
    let old_ws = ["websocket", "_max_subscriptions", "_per_connection"].concat();
    let old_funder = ["funder", "_address = "].concat();
    let old_shutdown = ["timeout", "_shutdown = "].concat();
    let aliases = nt_named
        .replace("update_instruments_interval_mins", &old_update)
        .replace("ws_max_subscriptions", &old_ws)
        .replace("funder = ", &old_funder)
        .replace("timeout_shutdown_secs = ", &old_shutdown);
    let error = toml::from_str::<BoltV3RootConfig>(&aliases)
        .expect_err("NT-owned alias field names should fail parse");
    let rendered = error.to_string();
    assert!(
        rendered.contains("unknown field"),
        "alias rejection should come from deny_unknown_fields, got: {rendered}"
    );
}

#[test]
fn bolt_v3_reference_data_instrument_id_uses_nt_typed_identifier() {
    // `ReferenceDataBlock.instrument_id` is typed as
    // `nautilus_model::identifiers::InstrumentId`. The strategy block is
    // parsed via `toml::from_str(&content)` directly (borrowed source),
    // so NT's `impl_serialization_for_identifier!` macro runs and routes
    // through `InstrumentId::new_checked`, eliminating the bolt-side
    // runtime empty / non-empty guard.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::InstrumentId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let reference = loaded.strategies[0]
        .config
        .reference_data
        .get("primary")
        .expect("binary_oracle strategy fixture should have reference_data.primary");
    let instrument_id: InstrumentId = reference.instrument_id;
    assert_eq!(instrument_id.to_string(), "BTCUSDT.BINANCE");
}

#[test]
fn bolt_v3_reference_data_instrument_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3StrategyConfig;

    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated = strategy_toml.replace(
        "instrument_id = \"BTCUSDT.BINANCE\"",
        "instrument_id = \"\"",
    );
    let err = toml::from_str::<BoltV3StrategyConfig>(&mutated)
        .expect_err("empty instrument_id should be rejected by NT InstrumentId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("InstrumentId"),
        "rejection should cite the NT InstrumentId parser, got: {rendered}"
    );
    assert!(
        rendered.contains("empty")
            || rendered.contains("invalid")
            || rendered.contains("missing")
            || rendered.contains("separator"),
        "rejection should explain the empty instrument_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_strategy_execution_client_id_uses_nt_typed_identifier() {
    // `BoltV3StrategyConfig.execution_client_id` is typed as
    // `nautilus_model::identifiers::ClientId`. The strategy block is
    // parsed via `toml::from_str(&content)` directly (borrowed source),
    // so NT's `impl_serialization_for_identifier!` macro routes the
    // value through `ClientId::new_checked` and rejects empty / non-ascii
    // strings at parse time without a bolt-side runtime guard.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::ClientId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let strategy = &loaded.strategies[0].config;
    let execution_client_id: ClientId = strategy.execution_client_id;
    assert_eq!(execution_client_id, ClientId::from("polymarket_main"));

    let primary = strategy
        .reference_data
        .get("primary")
        .expect("binary_oracle fixture should have reference_data.primary");
    let data_client_id: ClientId = primary.data_client_id;
    assert_eq!(data_client_id, ClientId::from("binance_reference"));
}

#[test]
fn bolt_v3_strategy_execution_client_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3StrategyConfig;

    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated = strategy_toml.replace(
        "execution_client_id = \"polymarket_main\"",
        "execution_client_id = \"\"",
    );
    let err = toml::from_str::<BoltV3StrategyConfig>(&mutated)
        .expect_err("empty execution_client_id should be rejected by NT ClientId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty execution_client_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_reference_data_data_client_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3StrategyConfig;

    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated = strategy_toml.replace(
        "data_client_id = \"binance_reference\"",
        "data_client_id = \"\"",
    );
    let err = toml::from_str::<BoltV3StrategyConfig>(&mutated)
        .expect_err("empty data_client_id should be rejected by NT ClientId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty data_client_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_strategy_oms_type_uses_nt_canonical_enum() {
    // FINDING-1: `strategy.oms_type` is typed as `nautilus_model::enums::OmsType`
    // (not a bolt shadow enum). NT's enum_strum_serde! macro makes deserialize
    // case-insensitive, so the fixture's `oms_type = "netting"` (lowercase)
    // continues to parse.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::enums::OmsType;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let oms_type: OmsType = loaded.strategies[0].config.oms_type;
    assert_eq!(oms_type, OmsType::Netting);
}

#[test]
fn bolt_v3_strategy_oms_type_accepts_nt_variants() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    for supported_oms_type in ["hedging", "unspecified"] {
        let mutated_strategy = std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable")
        .replace(
            "oms_type = \"netting\"",
            &format!("oms_type = \"{supported_oms_type}\""),
        );
        let strategy: BoltV3StrategyConfig =
            toml::from_str(&mutated_strategy).expect("oms_type should parse via NT enum");
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.iter().all(|message| !message.contains("oms_type")),
            "NT oms_type variant {supported_oms_type} should not be narrowed by bolt validation: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_strategy_execution_client_id_rejects_data_only_client_with_client_vocabulary() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let execution_block = "[clients.polymarket_main.execution]\naccount_id = \"POLYMARKET-001\"\nsignature_type = \"poly_proxy\"\nfunder = \"0x1111111111111111111111111111111111111111\"\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/user\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_secs = 60\nmax_retries = 3\nretry_delay_initial_ms = 250\nretry_delay_max_ms = 2000\nack_timeout_secs = 5\nfee_cache_ttl_secs = 300\ntransport_backend = \"sockudo\"\n\n";
    let root: BoltV3RootConfig = toml::from_str(&replace_in_fixture_root(execution_block, ""))
        .expect("data-only polymarket fixture should parse");
    let strategy: BoltV3StrategyConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&root, &loaded);
    let rendered = messages.join("\n");
    assert!(rendered.contains("strategy execution_client_id `polymarket_main`"));
    assert!(rendered.contains("execution-capable client"));
    assert!(rendered.contains("referenced client has no [execution] block"));
    assert!(!rendered.contains("execution-capable venue"));
    assert!(!rendered.contains("referenced venue"));
}

#[test]
fn bolt_v3_polymarket_client_rejects_execution_without_data_block_with_client_vocabulary() {
    // Bug class: a config can split a Polymarket venue across two
    // `clients.<id>` blocks (one execution-only, one data-only). The
    // existing execution-capable strategy check still passes, but
    // `bolt_v3_providers::polymarket::build_market_slug_filters_for_client`
    // binds per-target market-slug filters to a single `client_key` and
    // skips every target whose `execution_client_id != client_key`.
    // Splitting the data adapter off the execution client_key therefore
    // silently strips the configured target market restriction during
    // data-client mapping. Fail closed inside the Polymarket provider
    // binding: every Polymarket client that declares `[execution]` must
    // also declare a co-located `[data]` block under the same
    // `clients.<id>`. The rule lives in the polymarket binding because
    // it is a Polymarket-internal invariant; core validation stays
    // provider-neutral per the source-fence.
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let polymarket_main_data_block = "[clients.polymarket_main.data]\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/market\"\nbase_url_gamma = \"https://gamma-api.polymarket.com\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_secs = 60\nws_timeout_secs = 30\nsubscribe_new_markets = false\nauto_load_missing_instruments = false\nauto_load_debounce_ms = 250\nupdate_instruments_interval_mins = 60\nws_max_subscriptions = 200\ntransport_backend = \"sockudo\"\n\n";
    let polymarket_data_only_client = "\n[clients.polymarket_data]\nvenue = \"POLYMARKET\"\n\n[clients.polymarket_data.data]\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/market\"\nbase_url_gamma = \"https://gamma-api.polymarket.com\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_secs = 60\nws_timeout_secs = 30\nsubscribe_new_markets = false\nauto_load_missing_instruments = false\nauto_load_debounce_ms = 250\nupdate_instruments_interval_mins = 60\nws_max_subscriptions = 200\ntransport_backend = \"sockudo\"\n";
    let split_fixture = format!(
        "{}{}",
        replace_in_fixture_root(polymarket_main_data_block, ""),
        polymarket_data_only_client
    );
    let root: BoltV3RootConfig =
        toml::from_str(&split_fixture).expect("split polymarket clients fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        rendered.contains("clients.polymarket_main"),
        "expected rejection citing `clients.polymarket_main`, got: {rendered}"
    );
    assert!(
        rendered.contains("provider=POLYMARKET"),
        "expected rejection to tag `provider=POLYMARKET`, got: {rendered}"
    );
    assert!(
        rendered.contains("declares [execution] but no [data] block"),
        "expected rejection citing `declares [execution] but no [data] block`, got: {rendered}"
    );
    assert!(
        rendered.contains("client_key"),
        "expected rejection to use NT client vocabulary `client_key`, got: {rendered}"
    );
    assert!(
        !rendered.contains("venues."),
        "rejection must not regress to stale `venues.` vocabulary, got: {rendered}"
    );
    assert!(
        !rendered.contains("data-capable venue"),
        "rejection must not regress to stale `data-capable venue` vocabulary, got: {rendered}"
    );
}

#[test]
fn bolt_v3_reference_data_client_id_rejects_execution_only_client_with_client_vocabulary() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let data_block = "[clients.binance_reference.data]\nproduct_types = [\"spot\"]\nenvironment = \"mainnet\"\nbase_url_http = \"https://api.binance.com\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_http\nbase_url_ws = \"wss://stream.binance.com:9443/ws\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_ws\ninstrument_status_poll_secs = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs\ntransport_backend = \"sockudo\"\n\n";
    let root: BoltV3RootConfig = toml::from_str(&replace_in_fixture_root(data_block, ""))
        .expect("execution-only binance fixture should parse");
    let strategy: BoltV3StrategyConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&root, &loaded);
    let rendered = messages.join("\n");
    assert!(rendered.contains("reference_data.primary.data_client_id `binance_reference`"));
    assert!(rendered.contains("data-capable client"));
    assert!(rendered.contains("referenced client has no [data] block"));
    assert!(!rendered.contains("data-capable venue"));
    assert!(!rendered.contains("referenced venue"));
}

#[test]
fn bolt_v3_runtime_mode_uses_nt_environment_enum() {
    // FINDING-1: `runtime.mode` is typed as `nautilus_common::enums::Environment`
    // (not a bolt shadow `RuntimeMode`). NT's `Environment` derives serde
    // directly without `#[serde(rename_all)]`, so the on-disk fixture value
    // is PascalCase (`mode = "Live"`). Bolt's validator rejects Backtest and
    // Sandbox explicitly because the binary is a live-trading node only.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_common::enums::Environment;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let mode: Environment = loaded.root.runtime.mode;
    assert_eq!(mode, Environment::Live);
}

#[test]
fn bolt_v3_runtime_mode_rejects_backtest_and_sandbox_variants() {
    // FINDING-1: bolt-v3 only supports `Environment::Live`. The binary is a
    // live LiveNode wrapper, so the validator must reject `Backtest` and
    // `Sandbox` explicitly rather than letting them silently flow into NT's
    // kernel boot path.
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for variant in ["Backtest", "Sandbox"] {
        let mutated = replace_in_fixture_root("mode = \"Live\"", &format!("mode = \"{variant}\""));
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("non-Live Environment variant should parse via NT");
        let messages = validate_root_only(&root);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("runtime.mode") && m.contains("Live")),
            "expected runtime.mode rejection citing Live variant for `{variant}`, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_logging_levels_use_nt_canonical_enum() {
    // FINDING-1: `logging.stdout_level` and `logging.fileout_level` are typed
    // as `nautilus_common::enums::LogLevel` (not a bolt shadow). NT's
    // LogLevel uses explicit `#[serde(rename = "UPPERCASE")]` per variant
    // (and notably uses `Warning` rather than the bolt shadow's `Warn`), so
    // the fixture's `stdout_level = "INFO"` / `fileout_level = "INFO"`
    // continue to parse unchanged.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_common::enums::LogLevel;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let stdout_level: LogLevel = loaded.root.logging.stdout_level;
    let fileout_level: LogLevel = loaded.root.logging.fileout_level;
    assert_eq!(stdout_level, LogLevel::Info);
    assert_eq!(fileout_level, LogLevel::Info);
}

#[test]
fn bolt_v3_logging_levels_accept_nt_warning_uppercase_spelling() {
    // FINDING-1: NT spells warning-level as `"WARNING"` (its shadow had
    // `Warn`). Switching to the NT canonical enum means `stdout_level = "WARNING"`
    // is the supported spelling; `"WARN"` is no longer accepted. Lock this so
    // a future regression to `Warn` immediately fails.
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;
    use nautilus_common::enums::LogLevel;

    let warning_root =
        replace_in_fixture_root("stdout_level = \"INFO\"", "stdout_level = \"WARNING\"");
    let root: BoltV3RootConfig =
        toml::from_str(&warning_root).expect("NT WARNING level should parse");
    assert_eq!(root.logging.stdout_level, LogLevel::Warning);

    let warn_root = replace_in_fixture_root("stdout_level = \"INFO\"", "stdout_level = \"WARN\"");
    let err = toml::from_str::<BoltV3RootConfig>(&warn_root)
        .expect_err("legacy WARN spelling should be rejected by NT LogLevel");
    let rendered = err.to_string();
    assert!(
        rendered.contains("WARN") && (rendered.contains("variant") || rendered.contains("unknown")),
        "rejection should reference the WARN spelling and explain it is unknown, got: {rendered}"
    );
}

#[test]
fn bolt_v3_archetype_order_params_use_nt_canonical_enums() {
    // FINDING-1: archetype `[parameters.*_order]` rows are typed with NT's
    // canonical `OrderType` and `TimeInForce` (not bolt shadow enums).
    // NT serde is case-insensitive, so the fixture's `order_type = "limit"`
    // and `time_in_force = "fok"` continue to parse unchanged.
    use bolt_v2::bolt_v3_archetypes::binary_oracle_edge_taker::ParametersBlock;
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::enums::{OrderType, TimeInForce};

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");
    let strategy = &loaded.strategies[0].config;
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .expect("fixture parameters block should deserialize as binary_oracle_edge_taker");

    let entry_order_type: OrderType = parameters.entry_order.order_type;
    let entry_tif: TimeInForce = parameters.entry_order.time_in_force;
    assert_eq!(entry_order_type, OrderType::Limit);
    assert_eq!(entry_tif, TimeInForce::Fok);

    let exit_order_type: OrderType = parameters.exit_order.order_type;
    let exit_tif: TimeInForce = parameters.exit_order.time_in_force;
    assert_eq!(exit_order_type, OrderType::Market);
    assert_eq!(exit_tif, TimeInForce::Ioc);
}

#[test]
fn bolt_v3_archetype_runtime_parameters_reject_unknown_fields() {
    use bolt_v2::{
        bolt_v3_archetypes::binary_oracle_edge_taker::ParametersBlock,
        bolt_v3_config::BoltV3StrategyConfig,
    };

    let strategy_toml = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated = strategy_toml.replace(
        "lead_jitter_max_ms = 250",
        "lead_jitter_max_ms = 250\nrisk_lambdaa = 0.5",
    );
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&mutated).expect("strategy envelope should parse");
    let parsed: Result<ParametersBlock, _> = strategy.parameters.try_into();
    let err = parsed.expect_err("unknown fields inside [parameters.runtime] should fail parse");
    let rendered = err.to_string();
    assert!(
        rendered.contains("unknown field") && rendered.contains("risk_lambdaa"),
        "runtime parameter rejection should come from deny_unknown_fields, got: {rendered}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_post_only_gtc_entry_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    let maker_strategy = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace("time_in_force = \"fok\"", "time_in_force = \"gtc\"")
    .replacen("is_post_only = false", "is_post_only = true", 1);
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_strategy)
        .expect("post-only GTC entry order should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "post-only GTC entry order should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_post_only_gtc_exit_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    let taker_strategy = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let maker_exit_strategy = taker_strategy
        .replace("order_type = \"market\"", "order_type = \"limit\"")
        .replace("time_in_force = \"ioc\"", "time_in_force = \"gtc\"");
    let (before_exit, exit_block) = maker_exit_strategy
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit order block");
    let maker_exit_strategy = format!(
        "{before_exit}[parameters.exit_order]{}",
        exit_block.replacen("is_post_only = false", "is_post_only = true", 1)
    );
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_exit_strategy)
        .expect("post-only GTC exit order should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "post-only GTC exit order should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_mixed_maker_taker_order_configs() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let validate_strategy = |strategy_source: String, case_name: &str| {
        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
            .unwrap_or_else(|error| panic!("{case_name} should parse via NT order enums: {error}"));
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.is_empty(),
            "{case_name} should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
        );
    };

    let maker_entry_taker_exit = fixture
        .replace("time_in_force = \"fok\"", "time_in_force = \"gtc\"")
        .replacen("is_post_only = false", "is_post_only = true", 1);
    validate_strategy(maker_entry_taker_exit, "maker entry with taker exit");

    let maker_exit_taker_entry = fixture
        .replace("order_type = \"market\"", "order_type = \"limit\"")
        .replace("time_in_force = \"ioc\"", "time_in_force = \"gtc\"");
    let (before_exit, exit_block) = maker_exit_taker_entry
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit order block");
    let maker_exit_taker_entry = format!(
        "{before_exit}[parameters.exit_order]{}",
        exit_block.replacen("is_post_only = false", "is_post_only = true", 1)
    );
    validate_strategy(maker_exit_taker_entry, "taker entry with maker exit");
}

#[test]
fn bolt_v3_archetype_accepts_coherent_short_side_order_contract() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let short_strategy = fixture
        .replacen("side = \"buy\"", "side = \"sell\"", 1)
        .replacen("position_side = \"long\"", "position_side = \"short\"", 1);
    let (before_exit, exit_block) = short_strategy
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit order block");
    let short_strategy = format!(
        "{before_exit}[parameters.exit_order]{}",
        exit_block
            .replacen("side = \"sell\"", "side = \"buy\"", 1)
            .replacen("position_side = \"long\"", "position_side = \"short\"", 1)
    );

    let strategy: BoltV3StrategyConfig =
        toml::from_str(&short_strategy).expect("coherent short-side order contract should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "coherent short-side order contract should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_incoherent_order_position_contract() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let (before_exit, exit_block) = fixture
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit order block");
    let incoherent_strategy = format!(
        "{before_exit}[parameters.exit_order]{}",
        exit_block.replacen("side = \"sell\"", "side = \"buy\"", 1)
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&incoherent_strategy)
        .expect("incoherent order position contract should still parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("position contract is not supported")
                && message.contains("long requires entry side=buy, exit side=sell")
                && message.contains("short requires entry side=sell, exit side=buy")
        }),
        "incoherent order position contract should be rejected with contract guidance: {messages:#?}"
    );
}

#[test]
fn polymarket_post_order_params_declares_camel_case_is_post_only_flag() {
    let query_source = include_str!("fixtures/nt_polymarket_query_post_order_params_7c2aafb.txt");
    let nt_field = ["post", "only"].join("_");

    assert!(query_source.contains("Revision: 7c2aafb30fb143069c915a3f2057bb12174405f6"));
    assert!(query_source.contains(
        "Full source SHA-256: c81bc63f9bfabff4c1dc7a3fcff33ee7c9f8c119e80e629a94afc59590238ed0"
    ));
    assert!(query_source.contains("pub struct PostOrderParams"));
    assert!(query_source.contains(r#"#[serde(rename_all = "camelCase")]"#));
    assert!(query_source.contains(&format!("pub {nt_field}: bool")));
    assert!(query_source.contains(r#"json.contains("postOnly")"#));
    assert!(query_source.contains(&format!(r#"json.contains("{nt_field}")"#)));
}

#[test]
fn bolt_v3_archetype_rejects_unsupported_nt_order_type_variants() {
    // FINDING-1: NT's OrderType has 9 variants; binary_oracle_edge_taker only
    // permits (Limit, Fok) on entry and (Market, Ioc) on exit. A
    // `[parameters.entry_order]` row with `order_type = "stop_market"` must
    // parse via NT serde and then be rejected by the archetype validator.
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    let mutated_strategy = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace("order_type = \"limit\"", "order_type = \"stop_market\"");
    let strategy: BoltV3StrategyConfig = toml::from_str(&mutated_strategy)
        .expect("stop_market order_type should parse via NT OrderType");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("binary_oracle_edge_taker")),
        "expected entry_order rejection citing binary_oracle_edge_taker, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_gtd_time_in_force_until_expiry_policy_exists() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let entry_gtd_strategy: BoltV3StrategyConfig =
        toml::from_str(&fixture.replace("time_in_force = \"fok\"", "time_in_force = \"gtd\""))
            .expect("gtd should parse via NT TimeInForce");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: entry_gtd_strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|m| {
            m.contains("entry_order")
                && m.contains("time_in_force=fok")
                && m.contains("time_in_force=gtc")
        }),
        "expected entry_order GTD rejection until an expiry policy exists, got: {messages:#?}"
    );

    let exit_gtd_strategy: BoltV3StrategyConfig =
        toml::from_str(&fixture.replace("time_in_force = \"ioc\"", "time_in_force = \"gtd\""))
            .expect("gtd should parse via NT TimeInForce");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: exit_gtd_strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|m| {
            m.contains("exit_order")
                && m.contains("time_in_force=ioc")
                && m.contains("time_in_force=gtc")
        }),
        "expected exit_order GTD rejection until an expiry policy exists, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_gtd_limit_order_with_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let gtd_strategy_source = fixture.replace(
        "time_in_force = \"fok\"\nis_post_only = false",
        "time_in_force = \"gtd\"\nexpire_time_unix_nanos = 4102444800000000000\nis_post_only = false",
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&gtd_strategy_source)
        .expect("GTD expiry should parse through the typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "GTD limit order with explicit expiry should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_non_triggered_entry_order_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture.replace(
        "time_in_force = \"fok\"\nis_post_only = false",
        "time_in_force = \"fok\"\ntrigger_price = 0.52\nis_post_only = false",
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("non-triggered entry trigger_price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected non-triggered entry_order trigger_price rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_non_triggered_exit_order_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let (before_exit, exit_block) = fixture
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit order block");
    let strategy_source = format!(
        "{before_exit}[parameters.exit_order]{}",
        exit_block.replacen(
            "time_in_force = \"ioc\"\nis_post_only = false",
            "time_in_force = \"ioc\"\ntrigger_price = 0.48\nis_post_only = false",
            1,
        )
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("non-triggered exit trigger_price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("exit_order") && m.contains("trigger_price")),
        "expected non-triggered exit_order trigger_price rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_stop_market_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_market_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_market\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_market_strategy_source)
        .expect("StopMarket trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "StopMarket entry order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_market_if_touched_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let market_if_touched_strategy_source = fixture
        .replace(
            "order_type = \"limit\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&market_if_touched_strategy_source)
        .expect("MarketIfTouched trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "MarketIfTouched entry order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_without_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"limit\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched entry without trigger price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected MarketIfTouched entry_order rejection requiring trigger_price, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_with_non_positive_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    for trigger_price in ["0.0", "-0.01"] {
        let strategy_source = fixture
            .replace(
                "order_type = \"limit\"",
                "order_type = \"market_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                &format!(
                    "time_in_force = \"gtc\"\ntrigger_price = {trigger_price}\nis_post_only = false"
                ),
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source).expect(
            "MarketIfTouched entry with non-positive trigger price should parse typed order config",
        );
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
            "expected MarketIfTouched entry_order rejection for trigger_price={trigger_price}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_gtd_entry_without_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"limit\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtd\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched GTD entry without expiry should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("expire_time_unix_nanos")),
        "expected MarketIfTouched entry_order GTD rejection requiring expiry, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_disallowed_flags() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    for (field, replacement) in [
        ("is_post_only", "is_post_only = true"),
        ("is_reduce_only", "is_reduce_only = true"),
        ("is_quote_quantity", "is_quote_quantity = true"),
    ] {
        let strategy_source = fixture
            .replace(
                "order_type = \"limit\"",
                "order_type = \"market_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = false",
            )
            .replacen(&format!("{field} = false"), replacement, 1);

        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
            .expect("MarketIfTouched disallowed flag case should parse typed order config");
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains(field)),
            "expected MarketIfTouched entry_order rejection for {field}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_exit_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let (before_exit, exit_and_after) = fixture
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit_order section");
    let exit_source = exit_and_after
        .replace(
            "order_type = \"market\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"ioc\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.48\nis_post_only = false",
        );
    let strategy_source = format!("{before_exit}[parameters.exit_order]{exit_source}");

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched exit order should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("exit_order") && m.contains("combination")),
        "expected MarketIfTouched exit_order rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_stop_limit_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = true",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "StopLimit entry order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_stop_limit_exit_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let (before_exit, exit_and_after) = fixture
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit_order table");
    let stop_limit_exit = exit_and_after
        .replace("order_type = \"market\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"ioc\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.48\nis_post_only = true",
        );
    let stop_limit_strategy_source =
        format!("{before_exit}[parameters.exit_order]{stop_limit_exit}");

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit exit trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "StopLimit exit order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_without_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit entry without trigger price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected StopLimit entry_order rejection requiring trigger_price, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_with_non_positive_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    for trigger_price in ["0.0", "-0.01"] {
        let stop_limit_strategy_source = fixture
            .replace("order_type = \"limit\"", "order_type = \"stop_limit\"")
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                &format!(
                    "time_in_force = \"gtc\"\ntrigger_price = {trigger_price}\nis_post_only = false"
                ),
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source).expect(
            "StopLimit entry with non-positive trigger price should parse typed order config",
        );
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
            "expected StopLimit entry_order rejection for trigger_price={trigger_price}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_gtd_entry_without_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtd\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit GTD entry without expiry should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("expire_time_unix_nanos")),
        "expected StopLimit entry_order GTD rejection requiring expiry, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_disallowed_flags() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    for (field, replacement) in [
        ("is_reduce_only", "is_reduce_only = true"),
        ("is_quote_quantity", "is_quote_quantity = true"),
    ] {
        let stop_limit_strategy_source = fixture
            .replace("order_type = \"limit\"", "order_type = \"stop_limit\"")
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = false",
            )
            .replacen(&format!("{field} = false"), replacement, 1);

        let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
            .expect("StopLimit disallowed flag case should parse typed order config");
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains(field)),
            "expected StopLimit entry_order rejection for {field}, got: {messages:#?}"
        );
    }
}

#[test]
fn parses_minimal_bolt_v3_root_and_strategy_config() {
    use bolt_v2::bolt_v3_archetypes::binary_oracle_edge_taker::ParametersBlock;
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use bolt_v2::bolt_v3_market_families::updown::{TargetBlock, TargetKind};
    use nautilus_common::enums::Environment;
    use nautilus_model::enums::{OrderType, TimeInForce};

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("minimal v3 config should load");

    assert_eq!(loaded.root.schema_version, 1);
    assert_eq!(
        loaded.root.trader_id,
        nautilus_model::identifiers::TraderId::from("BOLT-001")
    );
    assert_eq!(loaded.root.runtime.mode, Environment::Live);
    assert_eq!(
        loaded.root.clients["polymarket_main"].venue.as_str(),
        "POLYMARKET"
    );
    assert_eq!(
        loaded.root.clients["binance_reference"].venue.as_str(),
        "BINANCE"
    );
    assert!(loaded.root.clients["polymarket_main"].execution.is_some());
    assert!(loaded.root.clients["binance_reference"].execution.is_none());

    assert_eq!(loaded.strategies.len(), 1);
    let strategy = &loaded.strategies[0].config;
    assert_eq!(
        strategy.strategy_archetype.as_str(),
        "binary_oracle_edge_taker"
    );
    let target: TargetBlock = strategy
        .target
        .clone()
        .try_into()
        .expect("fixture target block should deserialize as updown TargetBlock");
    assert_eq!(target.kind, TargetKind::RotatingMarket);
    assert_eq!(target.cadence_secs, 300);
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .expect("fixture parameters block should deserialize as binary_oracle_edge_taker");
    assert_eq!(parameters.entry_order.order_type, OrderType::Limit);
    assert_eq!(parameters.entry_order.time_in_force, TimeInForce::Fok);
    assert_eq!(parameters.exit_order.order_type, OrderType::Market);
    assert_eq!(parameters.exit_order.time_in_force, TimeInForce::Ioc);
    assert!(strategy.reference_data.contains_key("primary"));
    assert_eq!(
        strategy.reference_data["primary"].data_client_id,
        nautilus_model::identifiers::ClientId::from("binance_reference")
    );
}

#[test]
fn rejects_unknown_bolt_v3_config_fields() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = fixture.replace(
        "schema_version = 1",
        "schema_version = 1\nunexpected_root_field = \"nope\"",
    );

    let error = toml::from_str::<BoltV3RootConfig>(&mutated)
        .expect_err("unknown root field should fail to parse")
        .to_string();
    assert!(
        error.contains("unexpected_root_field"),
        "error should name the unknown field, got: {error}"
    );

    let mutated_strategy = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace(
        "[parameters]\nedge_threshold_basis_points = 100",
        "[parameters]\nedge_threshold_basis_points = 100\nbogus_parameter = 7",
    );

    // The strategy envelope's `parameters` field is now archetype-
    // neutral raw TOML (`toml::Value`); unknown-field rejection inside
    // `[parameters]` moves from envelope-parse time to archetype typed
    // deserialization time. The first parse therefore succeeds, but
    // `try_into::<ParametersBlock>` (the per-archetype deserializer)
    // still rejects the unknown field by name.
    let strategy: bolt_v2::bolt_v3_config::BoltV3StrategyConfig = toml::from_str(&mutated_strategy)
        .expect(
            "strategy envelope parse should succeed when parameters is archetype-neutral raw TOML",
        );
    let parameters_error = strategy
        .parameters
        .try_into::<bolt_v2::bolt_v3_archetypes::binary_oracle_edge_taker::ParametersBlock>()
        .expect_err("unknown field inside [parameters] should fail archetype typed deserialization")
        .to_string();
    assert!(
        parameters_error.contains("bogus_parameter"),
        "archetype deserialization error should name the unknown strategy field, got: {parameters_error}"
    );
}

#[test]
fn rejects_forbidden_polymarket_env_vars_before_client_build() {
    use bolt_v2::{
        bolt_v3_config::load_bolt_v3_config,
        bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
    };

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    for forbidden in [
        "POLYMARKET_PK",
        "POLYMARKET_FUNDER",
        "POLYMARKET_API_KEY",
        "POLYMARKET_API_SECRET",
        "POLYMARKET_PASSPHRASE",
    ] {
        let result = build_bolt_v3_live_node_with(
            &loaded,
            |var| var == forbidden,
            support::fake_bolt_v3_resolver,
        );
        let error = result.expect_err("forbidden env var must block LiveNode build");
        match error {
            BoltV3LiveNodeError::ForbiddenEnv(report) => {
                assert_eq!(report.findings.len(), 1, "{report}");
                assert_eq!(report.findings[0].client_key, "polymarket_main");
                assert_eq!(report.findings[0].env_var, forbidden);
            }
            other => panic!("expected ForbiddenEnv error, got {other:?}"),
        }
    }
}

#[test]
fn rejects_polymarket_execution_client_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "Live"

[nautilus]
load_state = true
save_state = true
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 0
open_check_lookback_mins = 60
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 0
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
bypass = false
max_order_submit_rate = "100/00:00:01"
max_order_modify_rate = "100/00:00:01"
max_notional_per_order = {}
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
ack_timeout_secs = 5
fee_cache_ttl_secs = 300
transport_backend = "sockudo"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket-execution-only TOML should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[execution]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for polymarket execution client, got: {messages:#?}"
    );
    assert!(rendered.contains("Polymarket execution client"));
    assert!(!rendered.contains("Polymarket execution venue"));
    assert!(rendered.contains("(provider=POLYMARKET)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_binance_reference_data_client_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "Live"

[nautilus]
load_state = true
save_state = true
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 0
open_check_lookback_mins = 60
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 0
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
bypass = false
max_order_submit_rate = "100/00:00:01"
max_order_modify_rate = "100/00:00:01"
max_notional_per_order = {}
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[clients.binance_reference]
venue = "BINANCE"

[clients.binance_reference.data]
product_types = ["spot"]
environment = "mainnet"
base_url_http = "https://binance.test.invalid/http"
base_url_ws = "wss://binance.test.invalid/ws"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("binance-data-only TOML should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[data]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for binance reference-data client, got: {messages:#?}"
    );
    assert!(rendered.contains("Binance reference-data client"));
    assert!(!rendered.contains("Binance reference-data venue"));
    assert!(rendered.contains("(provider=BINANCE)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_binance_execution_block_with_provider_vocabulary() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated =
        format!("{fixture}\n\n[clients.binance_reference.execution]\nnot_allowed = true\n");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("binance execution mutation should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[execution]")
            && m.contains("not allowed")),
        "expected Binance execution-block rejection, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=BINANCE)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_polymarket_client_numeric_fields_at_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "Live"

[nautilus]
load_state = true
save_state = true
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 0
open_check_lookback_mins = 60
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 0
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
bypass = false
max_order_submit_rate = "100/00:00:01"
max_order_modify_rate = "100/00:00:01"
max_notional_per_order = {}
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 0
ws_timeout_secs = 0
subscribe_new_markets = false
auto_load_missing_instruments = false
auto_load_debounce_ms = 250
update_instruments_interval_mins = 0
ws_max_subscriptions = 0
transport_backend = "sockudo"

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 0
max_retries = 0
retry_delay_initial_ms = 0
retry_delay_max_ms = 0
ack_timeout_secs = 0
fee_cache_ttl_secs = 0
transport_backend = "sockudo"

[clients.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket bounds TOML should parse");
    let messages = validate_root_only(&root);
    let expected = [
        "clients.polymarket_main.data.http_timeout_secs must be a positive integer",
        "clients.polymarket_main.data.ws_timeout_secs must be a positive integer",
        "clients.polymarket_main.data.update_instruments_interval_mins must be a positive integer",
        "clients.polymarket_main.data.ws_max_subscriptions must be a positive integer",
        "clients.polymarket_main.execution.http_timeout_secs must be a positive integer",
        "clients.polymarket_main.execution.max_retries must be a positive integer",
        "clients.polymarket_main.execution.retry_delay_initial_ms must be a positive integer",
        "clients.polymarket_main.execution.retry_delay_max_ms must be a positive integer",
        "clients.polymarket_main.execution.ack_timeout_secs must be a positive integer",
        "clients.polymarket_main.execution.fee_cache_ttl_secs must be a positive integer",
    ];
    for needle in expected {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_unsupported_root_and_strategy_schema_versions() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::{validate_root_only, validate_strategies},
    };

    let mutated_root =
        std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture should be readable")
            .replace("schema_version = 1", "schema_version = 2");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated_root).expect("mutated root should parse with raw u32");
    let root_messages = validate_root_only(&root);
    assert!(
        root_messages
            .iter()
            .any(|m| m.contains("root schema_version=2 is unsupported")),
        "expected unsupported root schema version, got: {root_messages:#?}"
    );

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture should be readable"),
    )
    .expect("stable root should parse");

    let mutated_strategy = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace("schema_version = 1", "schema_version = 7");
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&mutated_strategy).expect("mutated strategy should parse with raw u32");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let strategy_messages = validate_strategies(&stable_root, &loaded);
    assert!(
        strategy_messages
            .iter()
            .any(|m| m.contains("schema_version=7 is unsupported")),
        "expected unsupported strategy schema version, got: {strategy_messages:#?}"
    );
}

fn replace_in_fixture_root(needle: &str, replacement: &str) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    assert!(
        fixture.contains(needle),
        "fixture must contain `{needle}` for this validation test to mutate"
    );
    fixture.replace(needle, replacement)
}

#[test]
fn rejects_zero_explicit_nt_exec_runtime_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "inflight_check_threshold_ms = 5000\ninflight_check_retries = 5",
        "inflight_check_threshold_ms = 0\ninflight_check_retries = 5",
    )
    .replace(
        "open_check_threshold_ms = 5000\nopen_check_missing_retries = 5",
        "open_check_threshold_ms = 0\nopen_check_missing_retries = 5",
    )
    .replace(
        "max_single_order_queries_per_cycle = 10\nsingle_order_query_delay_ms = 100",
        "max_single_order_queries_per_cycle = 0\nsingle_order_query_delay_ms = 100",
    )
    .replace(
        "position_check_threshold_ms = 5000\nposition_check_retries = 3",
        "position_check_threshold_ms = 0\nposition_check_retries = 3",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero NT exec defaults fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.inflight_check_threshold_ms must be a positive integer",
        "nautilus.exec_engine.open_check_threshold_ms must be a positive integer",
        "nautilus.exec_engine.max_single_order_queries_per_cycle must be a positive integer",
        "nautilus.exec_engine.position_check_threshold_ms must be a positive integer",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_nt_data_engine_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "time_bars_interval_type = \"LEFT_OPEN\"",
        "time_bars_interval_type = \"SIDEWAYS\"",
    )
    .replace("time_bars_origins = {}", "time_bars_origins = { INVALID = 1 }")
    .replace(
        "debug = false\ngraceful_shutdown_on_error = false\nqsize = 100000\n\n[nautilus.exec_engine]",
        "debug = false\ngraceful_shutdown_on_error = true\nqsize = 1000\n\n[nautilus.exec_engine]",
    );
    assert!(
        mutated.contains("time_bars_interval_type = \"SIDEWAYS\"")
            && mutated.contains("time_bars_origins = { INVALID = 1 }")
            && mutated.contains("graceful_shutdown_on_error = true")
            && mutated.contains("qsize = 1000"),
        "test fixture mutation must exercise every invalid data-engine branch"
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT data-engine fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.data_engine.time_bars_interval_type is not valid",
        "nautilus.data_engine.time_bars_origins key `INVALID` is not a valid Nautilus bar aggregation",
        "nautilus.data_engine.graceful_shutdown_on_error must be false",
        "nautilus.data_engine.qsize must match NT default",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_nt_exec_values_unsupported_by_rust_live_runtime() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("snapshot_orders = false", "snapshot_orders = true")
        .replace("snapshot_positions = false", "snapshot_positions = true")
        .replace("purge_from_database = false", "purge_from_database = true")
        .replace(
            "graceful_shutdown_on_error = false",
            "graceful_shutdown_on_error = true",
        )
        .replace("qsize = 100000", "qsize = 1000");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("unsupported NT exec values fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.snapshot_orders must be false",
        "nautilus.exec_engine.snapshot_positions must be false",
        "nautilus.exec_engine.purge_from_database must be false",
        "nautilus.exec_engine.graceful_shutdown_on_error must be false",
        "nautilus.exec_engine.qsize must match NT default",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_nt_exec_filter_identifiers() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "reconciliation_instrument_ids = []",
        "reconciliation_instrument_ids = [\"INVALID\"]",
    )
    .replace(
        "filtered_client_order_ids = []",
        "filtered_client_order_ids = [\"\"]",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT exec filter identifiers fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.reconciliation_instrument_ids contains invalid instrument ID",
        "nautilus.exec_engine.filtered_client_order_ids contains invalid client order ID",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_external_client_id_at_parse_time() {
    // FINDING-8: external_clients is `Vec<ClientId>`, so an empty string is
    // rejected by ClientId's deserializer before the validator ever runs.
    // This keeps invalid identifiers from reaching runtime code through any
    // code path that bypasses validate_root_only.
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let mutated = replace_in_fixture_root(
        "emit_quotes_from_book_depths = false\nexternal_clients = []",
        "emit_quotes_from_book_depths = false\nexternal_clients = [\"\"]",
    );
    let parse_result = toml::from_str::<BoltV3RootConfig>(&mutated);
    let error = parse_result.expect_err("empty ClientId must be rejected at TOML parse time");
    let message = error.to_string();
    assert!(
        message.contains("invalid string for 'value'"),
        "expected ClientId rejection from serde, got: {message}"
    );
    assert!(
        message.contains("external_clients = [\"\"]"),
        "expected error to point at the offending external_clients entry, got: {message}"
    );
}

#[test]
fn rejects_nt_risk_bypass_true() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("bypass = false", "bypass = true");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("risk.nautilus.bypass=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("risk.nautilus.bypass must be false")),
        "expected risk.nautilus.bypass=false validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_nt_risk_values_unsupported_by_rust_live_runtime() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Anchor on the bypass=false line which is unique to [risk.nautilus]
    // so neither replace bleeds into the [nautilus.data_engine] /
    // [nautilus.exec_engine] qsize and graceful_shutdown_on_error fields.
    let mutated = replace_in_fixture_root(
        "bypass = false\nmax_order_submit_rate",
        "bypass = false\ngraceful_shutdown_on_error_marker_anchor\nmax_order_submit_rate",
    )
    .replace(
        "debug = false\ngraceful_shutdown_on_error = false\nqsize = 100000",
        "debug = false\ngraceful_shutdown_on_error = true\nqsize = 1000",
    )
    .replace("\ngraceful_shutdown_on_error_marker_anchor", "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("unsupported NT risk values fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "risk.nautilus.graceful_shutdown_on_error must be false",
        "risk.nautilus.qsize must match NT default",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_nt_risk_rate_limit_strings() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (submit_rate, modify_rate) in [
        ("0/00:00:01", "100/00:00:00"),
        ("100", "100/00:00:01"),
        ("abc/00:00:01", "100/00:00:01"),
        ("100/00:01", "100/00:00:01"),
        ("100/00:00:01:00", "100/00:00:01"),
        ("100/00:60:00", "100/00:00:01"),
        ("100/00:00:60", "100/00:00:01"),
    ] {
        let mutated = replace_in_fixture_root(
            "max_order_submit_rate = \"100/00:00:01\"\nmax_order_modify_rate = \"100/00:00:01\"",
            &format!(
                "max_order_submit_rate = \"{submit_rate}\"\nmax_order_modify_rate = \"{modify_rate}\""
            ),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("invalid NT rate limit fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains(
                "risk.nautilus.max_order_submit_rate is not a valid Nautilus rate limit"
            )),
            "expected submit-rate validation message for `{submit_rate}`, got: {messages:#?}"
        );
        // Only the first case mutates modify_rate; the remaining cases keep it
        // valid so submit-rate parsing branches are isolated.
        if modify_rate == "100/00:00:00" {
            assert!(
                messages.iter().any(|m| m.contains(
                    "risk.nautilus.max_order_modify_rate is not a valid Nautilus rate limit"
                )),
                "expected modify-rate validation message for `{modify_rate}`, got: {messages:#?}"
            );
        } else {
            assert!(
                !messages.iter().any(|m| m.contains(
                    "risk.nautilus.max_order_modify_rate is not a valid Nautilus rate limit"
                )),
                "valid modify_rate `{modify_rate}` must not produce a modify-rate error: {messages:#?}"
            );
        }
    }
}

#[test]
fn rejects_invalid_nt_risk_max_notional_map_entries() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "max_notional_per_order = {}",
        "max_notional_per_order = { \"BAD\" = \"not-a-decimal\" }",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT max-notional map fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains(
            "risk.nautilus.max_notional_per_order key `BAD` is not a valid Nautilus instrument ID"
        )),
        "expected invalid instrument-id validation error, got: {messages:#?}"
    );
    assert!(
        messages.iter().any(|m| m
            .contains("risk.nautilus.max_notional_per_order[`BAD`] is not a valid decimal string")),
        "expected invalid notional validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_non_positive_nt_risk_max_notional_map_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for notional in ["0", "-1.00"] {
        let mutated = replace_in_fixture_root(
            "max_notional_per_order = {}",
            &format!("max_notional_per_order = {{ \"ETHUSDT.BINANCE\" = \"{notional}\" }}"),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("non-positive NT max-notional fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains(
                "risk.nautilus.max_notional_per_order[`ETHUSDT.BINANCE`] must be a positive decimal string"
            )),
            "expected positive notional validation error for `{notional}`, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_orphan_secrets_block_without_data_or_execution() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "[clients.binance_reference.data]\nproduct_types = [\"spot\"]\nenvironment = \"mainnet\"\nbase_url_http = \"https://api.binance.com\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_http\nbase_url_ws = \"wss://stream.binance.com:9443/ws\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_ws\ninstrument_status_poll_secs = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs\ntransport_backend = \"sockudo\"\n\n",
        "",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("orphan-secrets fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[secrets]")
            && m.contains("no [data] block is configured")),
        "expected orphan-secrets validation error, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=BINANCE)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_ssm_paths_missing_leading_slash() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "api_key_ssm_path = \"/bolt/binance_reference/api_key\"",
        "api_key_ssm_path = \"bolt/binance_reference/api_key\"",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated).expect("ssm-path mutation should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("api_key_ssm_path")
            && m.contains("absolute-style SSM parameter path starting with `/`")),
        "expected SSM-path leading-slash validation error, got: {messages:#?}"
    );
    assert!(rendered.contains("clients.binance_reference.secrets.api_key_ssm_path"));
    let legacy_path = ["venues", "binance_reference"].join(".");
    assert!(!rendered.contains(&legacy_path));
}

#[test]
fn rejects_polymarket_funder_with_invalid_evm_syntax() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder = \"0x1111111111111111111111111111111111111111\"",
        "funder = \"0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("not a valid EVM public address")),
        "expected EVM-syntax validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_funder_zero_address() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder = \"0x1111111111111111111111111111111111111111\"",
        "funder = \"0x0000000000000000000000000000000000000000\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("zero address")),
        "expected zero-address validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_missing_funder_for_poly_proxy_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder = \"0x1111111111111111111111111111111111111111\"\n",
        "",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("required when signature_type is `poly_proxy` or `poly_gnosis_safe`")),
        "expected required-funder validation error, got: {messages:#?}"
    );
}

#[test]
fn allows_missing_funder_for_eoa_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let without_funder = replace_in_fixture_root(
        "funder = \"0x1111111111111111111111111111111111111111\"\n",
        "",
    );
    let with_eoa = without_funder.replace(
        "signature_type = \"poly_proxy\"",
        "signature_type = \"eoa\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&with_eoa).expect("eoa-without-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages.iter().any(|m| m.contains("funder")),
        "EOA signature must allow absent funder, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_data_zero_instrument_status_poll_secs() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "instrument_status_poll_secs = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
        "instrument_status_poll_secs = 0 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-poll-interval fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("instrument_status_poll_secs")
            && m.contains("must be a positive integer")),
        "expected positive-integer poll-interval validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_only_client_with_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let execution_block = "[clients.polymarket_main.execution]\naccount_id = \"POLYMARKET-001\"\nsignature_type = \"poly_proxy\"\nfunder = \"0x1111111111111111111111111111111111111111\"\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/user\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_secs = 60\nmax_retries = 3\nretry_delay_initial_ms = 250\nretry_delay_max_ms = 2000\nack_timeout_secs = 5\nfee_cache_ttl_secs = 300\ntransport_backend = \"sockudo\"\n\n";
    let mutated = replace_in_fixture_root(execution_block, "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("polymarket data-only secrets fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[secrets]")
            && m.contains("[execution]")),
        "expected Polymarket data-only secrets validation error, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=POLYMARKET)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_polymarket_data_subscribe_new_markets_true_in_current_slice() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "subscribe_new_markets = false",
        "subscribe_new_markets = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("subscribe_new_markets=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("subscribe_new_markets")
            && m.contains("must be false")),
        "expected subscribe_new_markets=true validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_missing_instruments_true_in_current_slice() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "auto_load_missing_instruments = false",
        "auto_load_missing_instruments = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("auto_load_missing_instruments=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_missing_instruments")
            && m.contains("must be false")),
        "expected auto_load_missing_instruments=true validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_debounce_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated =
        replace_in_fixture_root("auto_load_debounce_ms = 250", "auto_load_debounce_ms = 0");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("auto_load_debounce_ms=0 fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_debounce_ms")
            && m.contains("positive integer")),
        "expected auto_load_debounce_ms=0 validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_more_than_one_polymarket_client_in_current_slice() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let extra_client = "\n\n[clients.polymarket_secondary]\nvenue = \"POLYMARKET\"\n\n[clients.polymarket_secondary.data]\nbase_url_http = \"https://test.invalid/clob\"\nbase_url_ws = \"wss://test.invalid/ws/market\"\nbase_url_gamma = \"https://test.invalid/gamma\"\nbase_url_data_api = \"https://test.invalid/data\"\nhttp_timeout_secs = 60\nws_timeout_secs = 30\nsubscribe_new_markets = false\nauto_load_missing_instruments = false\nauto_load_debounce_ms = 250\nupdate_instruments_interval_mins = 60\nws_max_subscriptions = 200\ntransport_backend = \"sockudo\"\n\n[clients.polymarket_secondary.secrets]\nprivate_key_ssm_path = \"/bolt/polymarket_secondary/private_key\"\napi_key_ssm_path = \"/bolt/polymarket_secondary/api_key\"\napi_secret_ssm_path = \"/bolt/polymarket_secondary/api_secret\"\npassphrase_ssm_path = \"/bolt/polymarket_secondary/passphrase\"\n";
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = format!("{fixture}{extra_client}");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("two-polymarket-venues fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("at most one [clients.<id>] block per venue")
                && m.contains("polymarket")),
        "expected one-venue-per-kind validation error, got: {messages:#?}"
    );
}
