mod support;

#[test]
fn parses_minimal_bolt_v3_root_and_strategy_config() {
    use bolt_v2::bolt_v3_archetypes::binary_oracle_edge_taker::{
        ArchetypeOrderSide, ArchetypeOrderType, ArchetypePositionSide, ArchetypeTimeInForce,
        ParametersBlock,
    };
    use bolt_v2::bolt_v3_config::{RuntimeMode, load_bolt_v3_config};
    use bolt_v2::bolt_v3_market_families::updown::{TargetBlock, TargetKind};

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("minimal v3 config should load");

    assert_eq!(loaded.root.schema_version, 1);
    assert_eq!(loaded.root.trader_id, "BOLT-001");
    assert_eq!(loaded.root.runtime.mode, RuntimeMode::Live);
    assert_eq!(
        loaded.root.venues["polymarket_main"].kind.as_str(),
        "polymarket"
    );
    assert_eq!(
        loaded.root.venues["binance_reference"].kind.as_str(),
        "binance"
    );
    assert!(loaded.root.venues["polymarket_main"].execution.is_some());
    assert!(loaded.root.venues["binance_reference"].execution.is_none());

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
    assert_eq!(target.cadence_seconds, 300);
    assert_eq!(target.cadence_slug_token, "5m");
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .expect("fixture parameters block should deserialize as binary_oracle_edge_taker");
    assert_eq!(parameters.entry_order.side, ArchetypeOrderSide::Buy);
    assert_eq!(
        parameters.entry_order.position_side,
        ArchetypePositionSide::Long
    );
    assert_eq!(parameters.entry_order.order_type, ArchetypeOrderType::Limit);
    assert_eq!(
        parameters.entry_order.time_in_force,
        ArchetypeTimeInForce::Fok
    );
    assert_eq!(parameters.exit_order.side, ArchetypeOrderSide::Sell);
    assert_eq!(
        parameters.exit_order.position_side,
        ArchetypePositionSide::Long
    );
    assert_eq!(parameters.exit_order.order_type, ArchetypeOrderType::Market);
    assert_eq!(
        parameters.exit_order.time_in_force,
        ArchetypeTimeInForce::Ioc
    );
    assert!(strategy.reference_data.contains_key("spot"));
    assert_eq!(strategy.reference_data["spot"].venue, "binance_reference");
    assert_eq!(
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_milliseconds,
        50
    );
}

#[test]
fn root_example_declares_live_canary_gate_contract() {
    use bolt_v2::bolt_v3_config::{BoltV3RootConfig, RuntimeMode};

    let root_text = std::fs::read_to_string(support::repo_path("config/root.example.toml"))
        .expect("root example should be readable");
    let root: BoltV3RootConfig =
        toml::from_str(&root_text).expect("root example should parse as root config");

    assert_eq!(root.runtime.mode, RuntimeMode::Live);
    let live_canary = root
        .live_canary
        .as_ref()
        .expect("operator root example must declare [live_canary]");
    assert!(
        !live_canary.approval_id.trim().is_empty(),
        "root example must declare live_canary.approval_id"
    );
    assert!(
        !live_canary
            .no_submit_readiness_report_path
            .trim()
            .is_empty(),
        "root example must declare live_canary.no_submit_readiness_report_path"
    );
    assert!(
        live_canary.max_no_submit_readiness_report_bytes > 0,
        "root example must cap no-submit readiness report bytes"
    );
    assert!(
        live_canary.max_live_order_count > 0,
        "root example must cap live order count"
    );
    assert!(
        !live_canary.max_notional_per_order.trim().is_empty(),
        "root example must cap live canary notional"
    );
    assert!(
        root.persistence
            .runtime_capture_start_poll_interval_milliseconds
            > 0,
        "root example must declare persistence.runtime_capture_start_poll_interval_milliseconds"
    );
}

#[test]
fn parses_nt_environment_runtime_modes_in_bolt_v3_root() {
    use bolt_v2::bolt_v3_config::{BoltV3RootConfig, RuntimeMode};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    for (raw_mode, expected) in [
        ("backtest", RuntimeMode::Backtest),
        ("sandbox", RuntimeMode::Sandbox),
        ("live", RuntimeMode::Live),
    ] {
        let mutated = fixture.replace("mode = \"live\"", &format!("mode = \"{raw_mode}\""));
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("NT environment runtime mode should parse");
        assert_eq!(root.runtime.mode, expected);
    }
}

#[test]
fn accepts_binary_oracle_entry_order_shape_from_toml_contract() {
    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated_strategy = strategy_toml.replace(
        "[parameters.entry_order]\nside = \"buy\"\nposition_side = \"long\"\norder_type = \"limit\"\ntime_in_force = \"fok\"",
        "[parameters.entry_order]\nside = \"buy\"\nposition_side = \"long\"\norder_type = \"market\"\ntime_in_force = \"ioc\"",
    );

    let messages = binary_oracle_strategy_validation_messages(&mutated_strategy);

    assert!(
        !messages
            .iter()
            .any(|message| message.contains("parameters.entry_order combination")),
        "entry order shape is TOML-owned and must not be rejected by hardcoded combo policy: {messages:#?}"
    );
}

#[test]
fn accepts_binary_oracle_exit_order_shape_from_toml_contract() {
    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated_strategy = strategy_toml.replace(
        "[parameters.exit_order]\nside = \"sell\"\nposition_side = \"long\"\norder_type = \"market\"\ntime_in_force = \"ioc\"",
        "[parameters.exit_order]\nside = \"sell\"\nposition_side = \"long\"\norder_type = \"limit\"\ntime_in_force = \"gtc\"",
    );

    let messages = binary_oracle_strategy_validation_messages(&mutated_strategy);

    assert!(
        !messages
            .iter()
            .any(|message| message.contains("parameters.exit_order combination")),
        "exit order shape is TOML-owned and must not be rejected by hardcoded combo policy: {messages:#?}"
    );
}

#[test]
fn rejects_binary_oracle_market_exit_time_in_force_before_strategy_build() {
    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated_strategy = strategy_toml.replace(
        "market_exit_time_in_force = \"gtc\"",
        "market_exit_time_in_force = \"day\"",
    );

    let messages = binary_oracle_strategy_validation_messages(&mutated_strategy);

    assert!(
        messages.iter().any(|message| {
            message.contains("market_exit_time_in_force") && message.contains("day")
        }),
        "unsupported market_exit_time_in_force must fail during strategy validation, got: {messages:#?}"
    );
}

#[test]
fn rejects_binary_oracle_zero_market_exit_runtime_fields_before_strategy_build() {
    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    for (field, replacement) in [
        ("market_exit_interval_ms", "market_exit_interval_ms = 0"),
        ("market_exit_max_attempts", "market_exit_max_attempts = 0"),
    ] {
        let mutated_strategy = strategy_toml.replace(&format!("{field} = 100"), replacement);
        let messages = binary_oracle_strategy_validation_messages(&mutated_strategy);

        assert!(
            messages
                .iter()
                .any(|message| message.contains(field) && message.contains("positive integer")),
            "{field}=0 must fail during strategy validation, got: {messages:#?}"
        );
    }
}

fn binary_oracle_strategy_validation_messages(strategy_toml: &str) -> Vec<String> {
    use bolt_v2::bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy};
    use bolt_v2::bolt_v3_validate::validate_strategies;

    let root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("root fixture should parse");
    let strategy: BoltV3StrategyConfig =
        toml::from_str(strategy_toml).expect("mutated strategy fixture should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    validate_strategies(&root, &loaded)
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
                assert_eq!(report.findings[0].venue_key, "polymarket_main");
                assert_eq!(report.findings[0].env_var, forbidden);
            }
            other => panic!("expected ForbiddenEnv error, got {other:?}"),
        }
    }
}

#[test]
fn rejects_polymarket_execution_venue_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
instance_id = "disabled"
cache = "disabled"
msgbus = "disabled"
portfolio = "disabled"
emulator = "disabled"
streaming = "disabled"
loop_debug = false
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

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
external_client_ids = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
external_client_ids = []
debug = false
reconciliation = true
reconciliation_startup_delay_seconds = 10
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_milliseconds = 2000
inflight_check_threshold_milliseconds = 5000
inflight_check_retries = 5
open_check_lookback_mins = 60
open_check_threshold_milliseconds = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_milliseconds = 100
position_check_lookback_mins = 60
position_check_threshold_milliseconds = 5000
position_check_retries = 3
purge_from_database = false
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"
nt_bypass = false
nt_max_order_submit_rate = "100/00:00:01"
nt_max_order_modify_rate = "100/00:00:01"
nt_max_notional_per_order = {}
nt_debug = false
nt_graceful_shutdown_on_error = false
nt_qsize = 100000

[logging]
standard_output_level = "INFO"
file_level = "INFO"
component_levels = {}
module_levels = {}
credential_module_level = "WARN"
log_components_only = false
is_colored = true
print_config = false
use_tracing = false
bypass_logging = false
file_config = "disabled"
clear_log_file = false
stale_log_source_directory = "/var/lib/bolt"
stale_log_archive_directory = "/var/log/bolt/nautilus-archive"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_milliseconds = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
max_retries = 3
retry_delay_initial_milliseconds = 250
retry_delay_max_milliseconds = 2000
ack_timeout_seconds = 5
fee_cache_ttl_seconds = 300
transport_backend = "tungstenite"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket-execution-only TOML should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[execution]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for polymarket execution venue, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_reference_data_venue_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
instance_id = "disabled"
cache = "disabled"
msgbus = "disabled"
portfolio = "disabled"
emulator = "disabled"
streaming = "disabled"
loop_debug = false
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

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
external_client_ids = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
external_client_ids = []
debug = false
reconciliation = true
reconciliation_startup_delay_seconds = 10
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_milliseconds = 2000
inflight_check_threshold_milliseconds = 5000
inflight_check_retries = 5
open_check_lookback_mins = 60
open_check_threshold_milliseconds = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_milliseconds = 100
position_check_lookback_mins = 60
position_check_threshold_milliseconds = 5000
position_check_retries = 3
purge_from_database = false
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"
nt_bypass = false
nt_max_order_submit_rate = "100/00:00:01"
nt_max_order_modify_rate = "100/00:00:01"
nt_max_notional_per_order = {}
nt_debug = false
nt_graceful_shutdown_on_error = false
nt_qsize = 100000

[logging]
standard_output_level = "INFO"
file_level = "INFO"
component_levels = {}
module_levels = {}
credential_module_level = "WARN"
log_components_only = false
is_colored = true
print_config = false
use_tracing = false
bypass_logging = false
file_config = "disabled"
clear_log_file = false
stale_log_source_directory = "/var/lib/bolt"
stale_log_archive_directory = "/var/log/bolt/nautilus-archive"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_milliseconds = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.binance_reference]
kind = "binance"

[venues.binance_reference.data]
product_types = ["spot"]
environment = "mainnet"
base_url_http = "https://binance.test.invalid/http"
base_url_ws = "wss://binance.test.invalid/ws"
instrument_status_poll_seconds = 3600
transport_backend = "tungstenite"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("binance-data-only TOML should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[data]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for binance reference-data venue, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_venue_numeric_fields_at_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
instance_id = "disabled"
cache = "disabled"
msgbus = "disabled"
portfolio = "disabled"
emulator = "disabled"
streaming = "disabled"
loop_debug = false
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

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
external_client_ids = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
external_client_ids = []
debug = false
reconciliation = true
reconciliation_startup_delay_seconds = 10
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_milliseconds = 2000
inflight_check_threshold_milliseconds = 5000
inflight_check_retries = 5
open_check_lookback_mins = 60
open_check_threshold_milliseconds = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_milliseconds = 100
position_check_lookback_mins = 60
position_check_threshold_milliseconds = 5000
position_check_retries = 3
purge_from_database = false
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"
nt_bypass = false
nt_max_order_submit_rate = "100/00:00:01"
nt_max_order_modify_rate = "100/00:00:01"
nt_max_notional_per_order = {}
nt_debug = false
nt_graceful_shutdown_on_error = false
nt_qsize = 100000

[logging]
standard_output_level = "INFO"
file_level = "INFO"
component_levels = {}
module_levels = {}
credential_module_level = "WARN"
log_components_only = false
is_colored = true
print_config = false
use_tracing = false
bypass_logging = false
file_config = "disabled"
clear_log_file = false
stale_log_source_directory = "/var/lib/bolt"
stale_log_archive_directory = "/var/log/bolt/nautilus-archive"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_milliseconds = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 0
ws_timeout_seconds = 0
subscribe_new_markets = false
auto_load_missing_instruments = false
update_instruments_interval_minutes = 0
websocket_max_subscriptions_per_connection = 0
auto_load_debounce_milliseconds = 0
transport_backend = "tungstenite"

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 0
max_retries = 0
retry_delay_initial_milliseconds = 0
retry_delay_max_milliseconds = 0
ack_timeout_seconds = 0
fee_cache_ttl_seconds = 0
transport_backend = "tungstenite"

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket bounds TOML should parse");
    let messages = validate_root_only(&root);
    let expected = [
        "venues.polymarket_main.data.http_timeout_seconds must be a positive integer",
        "venues.polymarket_main.data.ws_timeout_seconds must be a positive integer",
        "venues.polymarket_main.data.update_instruments_interval_minutes must be a positive integer",
        "venues.polymarket_main.data.websocket_max_subscriptions_per_connection must be a positive integer",
        "venues.polymarket_main.data.auto_load_debounce_milliseconds must be a positive integer",
        "venues.polymarket_main.execution.http_timeout_seconds must be a positive integer",
        "venues.polymarket_main.execution.max_retries must be a positive integer",
        "venues.polymarket_main.execution.retry_delay_initial_milliseconds must be a positive integer",
        "venues.polymarket_main.execution.retry_delay_max_milliseconds must be a positive integer",
        "venues.polymarket_main.execution.ack_timeout_seconds must be a positive integer",
        "venues.polymarket_main.execution.fee_cache_ttl_seconds must be a positive integer",
    ];
    for needle in expected {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_polymarket_execution_max_retries_above_nt_u32_at_startup_validation() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "max_retries = 3",
        &format!("max_retries = {}", u64::from(u32::MAX) + 1),
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("max_retries range fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains(
            "venues.polymarket_main.execution.max_retries must fit in u32 expected by NT"
        )),
        "expected max_retries NT u32 range validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_empty_polymarket_data_and_execution_urls() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let data_urls = r#"base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com""#;
    let empty_data_urls = r#"base_url_http = " "
base_url_ws = " "
base_url_gamma = " "
base_url_data_api = " ""#;
    let execution_urls = r#"base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com""#;
    let empty_execution_urls = r#"base_url_http = " "
base_url_ws = " "
base_url_data_api = " ""#;
    let mutated = replace_in_fixture_root(data_urls, empty_data_urls)
        .replace(execution_urls, empty_execution_urls);
    let root: BoltV3RootConfig = toml::from_str(&mutated).expect("mutated root should still parse");
    let messages = validate_root_only(&root);
    let expected = [
        "venues.polymarket_main.data.base_url_http must be a non-empty URL",
        "venues.polymarket_main.data.base_url_ws must be a non-empty URL",
        "venues.polymarket_main.data.base_url_gamma must be a non-empty URL",
        "venues.polymarket_main.data.base_url_data_api must be a non-empty URL",
        "venues.polymarket_main.execution.base_url_http must be a non-empty URL",
        "venues.polymarket_main.execution.base_url_ws must be a non-empty URL",
        "venues.polymarket_main.execution.base_url_data_api must be a non-empty URL",
    ];
    for needle in expected {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_polymarket_execution_retry_delay_initial_above_max() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "retry_delay_initial_milliseconds = 250",
        "retry_delay_initial_milliseconds = 3000",
    )
    .replace(
        "retry_delay_max_milliseconds = 2000",
        "retry_delay_max_milliseconds = 1000",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated).expect("mutated root should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| {
            m.contains(
                "venues.polymarket_main.execution.retry_delay_initial_milliseconds (3000) must be <= retry_delay_max_milliseconds (1000)",
            )
        }),
        "expected retry-delay ordering validation error, got: {messages:#?}"
    );
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
fn rejects_zero_runtime_capture_start_poll_interval() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "runtime_capture_start_poll_interval_milliseconds = 50",
        "runtime_capture_start_poll_interval_milliseconds = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero capture poll interval fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| message.contains(
            "persistence.runtime_capture_start_poll_interval_milliseconds must be a positive integer"
        )),
        "expected runtime-capture start poll interval validation error, got: {messages:#?}"
    );
}

fn binance_execution_block(account_id: &str) -> String {
    format!(
        r#"

[venues.binance_reference.execution]
account_id = "{account_id}"
product_types = ["spot"]
environment = "mainnet"
base_url_http = "https://api.binance.com"
base_url_ws = "wss://stream.binance.com:9443/ws"
base_url_ws_trading = "wss://ws-api.binance.com/ws-api/v3"
use_ws_trading = true
use_position_ids = true
default_taker_fee = "0.0004"
futures_leverages = {{}}
futures_margin_types = {{}}
treat_expired_as_canceled = false
use_trade_lite = false
transport_backend = "sockudo"
"#
    )
}

fn fixture_without_binance_data_block() -> String {
    replace_in_fixture_root(
        "[venues.binance_reference.data]\nproduct_types = [\"spot\"]\nenvironment = \"mainnet\"\nbase_url_http = \"https://api.binance.com\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_http\nbase_url_ws = \"wss://stream.binance.com:9443/ws\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_ws\ninstrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs\ntransport_backend = \"sockudo\"\n\n",
        "",
    )
}

#[test]
fn rejects_zero_explicit_nt_exec_runtime_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "inflight_check_threshold_milliseconds = 5000\ninflight_check_retries = 5",
        "inflight_check_threshold_milliseconds = 0\ninflight_check_retries = 5",
    )
    .replace(
        "open_check_threshold_milliseconds = 5000\nopen_check_missing_retries = 5",
        "open_check_threshold_milliseconds = 0\nopen_check_missing_retries = 5",
    )
    .replace(
        "max_single_order_queries_per_cycle = 10\nsingle_order_query_delay_milliseconds = 100",
        "max_single_order_queries_per_cycle = 0\nsingle_order_query_delay_milliseconds = 100",
    )
    .replace(
        "position_check_threshold_milliseconds = 5000\nposition_check_retries = 3",
        "position_check_threshold_milliseconds = 0\nposition_check_retries = 3",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero NT exec defaults fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.inflight_check_threshold_milliseconds must be a positive integer",
        "nautilus.exec_engine.open_check_threshold_milliseconds must be a positive integer",
        "nautilus.exec_engine.max_single_order_queries_per_cycle must be a positive integer",
        "nautilus.exec_engine.position_check_threshold_milliseconds must be a positive integer",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_zero_optional_nt_exec_runtime_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    const OPTIONAL_NT_EXEC_FIELDS: &[&str] = &[
        "snapshot_positions_interval_seconds",
        "reconciliation_lookback_mins",
        "open_check_interval_seconds",
        "open_check_lookback_mins",
        "position_check_interval_seconds",
        "purge_closed_orders_interval_mins",
        "purge_closed_orders_buffer_mins",
        "purge_closed_positions_interval_mins",
        "purge_closed_positions_buffer_mins",
        "purge_account_events_interval_mins",
        "purge_account_events_lookback_mins",
        "own_books_audit_interval_seconds",
    ];

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture root should load");
    let mut without_optional_fields = String::new();
    for line in fixture.lines() {
        if OPTIONAL_NT_EXEC_FIELDS
            .iter()
            .any(|field| line.trim_start().starts_with(&format!("{field} =")))
        {
            continue;
        }
        without_optional_fields.push_str(line);
        without_optional_fields.push('\n');
    }

    let zero_optional_fields = OPTIONAL_NT_EXEC_FIELDS
        .iter()
        .map(|field| format!("{field} = 0\n"))
        .collect::<String>();
    let mutated = without_optional_fields.replace(
        "snapshot_positions = false\n",
        &format!("snapshot_positions = false\n{zero_optional_fields}"),
    );

    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero optional NT exec fixture should parse");
    let messages = validate_root_only(&root);
    for field in OPTIONAL_NT_EXEC_FIELDS {
        let needle = format!("nautilus.exec_engine.{field} must be a positive integer when set");
        assert!(
            messages.iter().any(|message| message == &needle),
            "expected validation error `{needle}`, got: {messages:#?}"
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
    .replace(
        "time_bars_origins = {}",
        "time_bars_origins = { INVALID = 1 }",
    )
    .replace(
        "emit_quotes_from_book_depths = false\nexternal_client_ids = []\ndebug = false",
        "emit_quotes_from_book_depths = false\nexternal_client_ids = [\"\"]\ndebug = false",
    );
    assert!(
        mutated.contains("time_bars_interval_type = \"SIDEWAYS\"")
            && mutated.contains("time_bars_origins = { INVALID = 1 }")
            && mutated.contains("external_client_ids = [\"\"]"),
        "test fixture mutation must exercise every invalid data-engine branch"
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT data-engine fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.data_engine.time_bars_interval_type is not valid",
        "nautilus.data_engine.time_bars_origins key `INVALID` is not a valid Nautilus bar aggregation",
        "nautilus.data_engine.external_client_ids contains invalid client ID",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn accepts_configured_nt_exec_runtime_values() {
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
        toml::from_str(&mutated).expect("configured NT exec values fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.snapshot_orders",
        "nautilus.exec_engine.snapshot_positions",
        "nautilus.exec_engine.purge_from_database",
        "nautilus.exec_engine.graceful_shutdown_on_error",
        "nautilus.exec_engine.qsize",
    ] {
        assert!(
            !messages.iter().any(|m| m.contains(needle)),
            "`{needle}` is TOML-owned NT config and must not be rejected by bolt-v3 policy: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_nt_exec_filter_identifiers() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated =
        replace_in_fixture_root("external_client_ids = []", "external_client_ids = [\"\"]")
            .replace(
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
        "nautilus.exec_engine.external_client_ids contains invalid client ID",
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
fn accepts_nt_risk_bypass_true_as_configured_nt_field() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("nt_bypass = false", "nt_bypass = true");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("nt_bypass=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages.iter().any(|m| m.contains("risk.nt_bypass")),
        "risk.nt_bypass is an NT config field and must not be rejected by bolt-v3 policy: {messages:#?}"
    );
}

#[test]
fn accepts_configured_nt_risk_runtime_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "nt_graceful_shutdown_on_error = false",
        "nt_graceful_shutdown_on_error = true",
    )
    .replace("nt_qsize = 100000", "nt_qsize = 1000");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("configured NT risk values fixture should parse");
    let messages = validate_root_only(&root);
    for needle in ["risk.nt_graceful_shutdown_on_error", "risk.nt_qsize"] {
        assert!(
            !messages.iter().any(|m| m.contains(needle)),
            "`{needle}` is TOML-owned NT config and must not be rejected by bolt-v3 policy: {messages:#?}"
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
            "nt_max_order_submit_rate = \"100/00:00:01\"\nnt_max_order_modify_rate = \"100/00:00:01\"",
            &format!(
                "nt_max_order_submit_rate = \"{submit_rate}\"\nnt_max_order_modify_rate = \"{modify_rate}\""
            ),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("invalid NT rate limit fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages
                .iter()
                .any(|m| m
                    .contains("risk.nt_max_order_submit_rate is not a valid Nautilus rate limit")),
            "expected submit-rate validation message for `{submit_rate}`, got: {messages:#?}"
        );
        // Only the first case mutates modify_rate; the remaining cases keep it
        // valid so submit-rate parsing branches are isolated.
        if modify_rate == "100/00:00:00" {
            assert!(
                messages.iter().any(|m| m
                    .contains("risk.nt_max_order_modify_rate is not a valid Nautilus rate limit")),
                "expected modify-rate validation message for `{modify_rate}`, got: {messages:#?}"
            );
        } else {
            assert!(
                !messages.iter().any(|m| m
                    .contains("risk.nt_max_order_modify_rate is not a valid Nautilus rate limit")),
                "valid modify_rate `{modify_rate}` must not produce a modify-rate error: {messages:#?}"
            );
        }
    }
}

#[test]
fn rejects_invalid_nt_risk_max_notional_map_entries() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "nt_max_notional_per_order = {}",
        "nt_max_notional_per_order = { \"BAD\" = \"not-a-decimal\" }",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT max-notional map fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains(
            "risk.nt_max_notional_per_order key `BAD` is not a valid Nautilus instrument ID"
        )),
        "expected invalid instrument-id validation error, got: {messages:#?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m
                .contains("risk.nt_max_notional_per_order[`BAD`] is not a valid decimal string")),
        "expected invalid notional validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_non_positive_nt_risk_max_notional_map_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for notional in ["0", "-1.00"] {
        let mutated = replace_in_fixture_root(
            "nt_max_notional_per_order = {}",
            &format!("nt_max_notional_per_order = {{ \"ETHUSDT.BINANCE\" = \"{notional}\" }}"),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("non-positive NT max-notional fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains(
                "risk.nt_max_notional_per_order[`ETHUSDT.BINANCE`] must be a positive decimal string"
            )),
            "expected positive notional validation error for `{notional}`, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_non_positive_default_max_notional_per_order() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for notional in ["0", "-1.00"] {
        let mutated = replace_in_fixture_root(
            "default_max_notional_per_order = \"10.00\"",
            &format!("default_max_notional_per_order = \"{notional}\""),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("non-positive default notional fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m
                .contains("risk.default_max_notional_per_order must be a positive decimal string")),
            "expected positive default notional validation error for `{notional}`, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_orphan_secrets_block_without_data_or_execution() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "[venues.binance_reference.data]\nproduct_types = [\"spot\"]\nenvironment = \"mainnet\"\nbase_url_http = \"https://api.binance.com\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_http\nbase_url_ws = \"wss://stream.binance.com:9443/ws\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_ws\ninstrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs\ntransport_backend = \"sockudo\"\n\n",
        "",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("orphan-secrets fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[secrets]")
            && m.contains("no [data] or [execution] block is configured")),
        "expected orphan-secrets validation error, got: {messages:#?}"
    );
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
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("api_key_ssm_path")
            && m.contains("absolute-style SSM parameter path starting with `/`")),
        "expected SSM-path leading-slash validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_funder_address_with_invalid_evm_syntax() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"",
        "funder_address = \"0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder_address")
            && m.contains("not a valid EVM public address")),
        "expected EVM-syntax validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_funder_address_zero_address() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"",
        "funder_address = \"0x0000000000000000000000000000000000000000\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder_address")
            && m.contains("zero address")),
        "expected zero-address validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_missing_funder_address_for_poly_proxy_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"\n",
        "",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder_address")
            && m.contains("required when signature_type is `poly_proxy` or `poly_gnosis_safe`")),
        "expected required-funder validation error, got: {messages:#?}"
    );
}

#[test]
fn allows_missing_funder_address_for_eoa_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let without_funder = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"\n",
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
        !messages.iter().any(|m| m.contains("funder_address")),
        "EOA signature must allow absent funder_address, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_data_zero_instrument_status_poll_seconds() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "instrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
        "instrument_status_poll_seconds = 0 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-poll-interval fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("instrument_status_poll_seconds")
            && m.contains("must be a positive integer")),
        "expected positive-integer poll-interval validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_data_empty_product_types() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("product_types = [\"spot\"]", "product_types = []");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("empty Binance data product_types fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("binance_reference") && m.contains("data.product_types")),
        "expected Binance data product_types validation error, got: {messages:#?}"
    );
}

#[test]
fn accepts_binance_execution_block_when_configured_from_toml() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mut fixture =
        std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture root.toml should be readable");
    fixture.push_str(&binance_execution_block("BINANCE-001"));
    let root: BoltV3RootConfig =
        toml::from_str(&fixture).expect("binance execution fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.is_empty(),
        "configured Binance execution venue must validate without a current-scope rejection: {messages:#?}"
    );
}

#[test]
fn accepts_binance_execution_only_block_when_configured_from_toml() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mut fixture = fixture_without_binance_data_block();
    fixture.push_str(&binance_execution_block("BINANCE-001"));
    let root: BoltV3RootConfig =
        toml::from_str(&fixture).expect("binance execution-only fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.is_empty(),
        "configured Binance execution-only venue must validate when [secrets] are present: {messages:#?}"
    );
}

#[test]
fn rejects_binance_execution_venue_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let secrets_block = "[venues.binance_reference.secrets]\napi_key_ssm_path = \"/bolt/binance_reference/api_key\"\napi_secret_ssm_path = \"/bolt/binance_reference/api_secret\"\n";
    let mut fixture = fixture_without_binance_data_block();
    assert!(
        fixture.contains(secrets_block),
        "fixture must contain Binance secrets block for this validation test to mutate"
    );
    fixture = fixture.replace(secrets_block, "");
    fixture.push_str(&binance_execution_block("BINANCE-001"));
    let root: BoltV3RootConfig =
        toml::from_str(&fixture).expect("binance execution-only fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[execution]")
            && m.contains("required [secrets] block")
            && m.contains("Binance execution venue")),
        "expected missing-secrets failure for Binance execution venue, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_execution_empty_product_types() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mut fixture =
        std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture root.toml should be readable");
    fixture.push_str(
        &binance_execution_block("BINANCE-001")
            .replace("product_types = [\"spot\"]", "product_types = []"),
    );
    let root: BoltV3RootConfig = toml::from_str(&fixture)
        .expect("empty Binance execution product_types fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("binance_reference") && m.contains("execution.product_types")),
        "expected Binance execution product_types validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_execution_blank_account_id() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mut fixture =
        std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture root.toml should be readable");
    fixture.push_str(&binance_execution_block("   "));
    let root: BoltV3RootConfig =
        toml::from_str(&fixture).expect("blank account-id fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("binance_reference") && m.contains("execution.account_id")),
        "expected Binance execution account-id validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_empty_binance_execution_urls() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (field, original) in [
        (
            "base_url_http",
            "base_url_http = \"https://api.binance.com\"",
        ),
        (
            "base_url_ws",
            "base_url_ws = \"wss://stream.binance.com:9443/ws\"",
        ),
        (
            "base_url_ws_trading",
            "base_url_ws_trading = \"wss://ws-api.binance.com/ws-api/v3\"",
        ),
    ] {
        let mut fixture =
            std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root.toml should be readable");
        let execution_block =
            binance_execution_block("BINANCE-001").replace(original, &format!("{field} = \"   \""));
        fixture.push_str(&execution_block);
        let root: BoltV3RootConfig =
            toml::from_str(&fixture).expect("empty Binance execution URL fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains("binance_reference")
                && m.contains(&format!("execution.{field}"))
                && m.contains("non-empty URL")),
            "expected Binance execution URL validation error for {field}, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_polymarket_data_only_venue_with_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let execution_block = "[venues.polymarket_main.execution]\naccount_id = \"POLYMARKET-001\"\nsignature_type = \"poly_proxy\"\nfunder_address = \"0x1111111111111111111111111111111111111111\"\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/user\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_seconds = 60\nmax_retries = 3\nretry_delay_initial_milliseconds = 250\nretry_delay_max_milliseconds = 2000\nack_timeout_seconds = 5\nfee_cache_ttl_seconds = 300\ntransport_backend = \"sockudo\"\n\n";
    let mutated = replace_in_fixture_root(execution_block, "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("polymarket data-only secrets fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[secrets]")
            && m.contains("[execution]")),
        "expected Polymarket data-only secrets validation error, got: {messages:#?}"
    );
}

#[test]
fn accepts_configured_polymarket_data_subscribe_new_markets() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "subscribe_new_markets = false",
        "subscribe_new_markets = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("subscribe_new_markets=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages
            .iter()
            .any(|m| m.contains("polymarket_main") && m.contains("subscribe_new_markets")),
        "configured subscribe_new_markets must validate, got: {messages:#?}"
    );
}

#[test]
fn accepts_polymarket_data_new_market_filter_keyword() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "transport_backend = \"sockudo\"\n\n[venues.polymarket_main.execution]",
        "transport_backend = \"sockudo\"\nnew_market_filter = { kind = \"keyword\", keyword = \"bitcoin\" }\n\n[venues.polymarket_main.execution]",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("new-market-filter fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages
            .iter()
            .any(|m| m.contains("polymarket_main") && m.contains("new_market_filter")),
        "configured new_market_filter must validate, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_empty_new_market_filter_keyword() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "transport_backend = \"sockudo\"\n\n[venues.polymarket_main.execution]",
        "transport_backend = \"sockudo\"\nnew_market_filter = { kind = \"keyword\", keyword = \" \" }\n\n[venues.polymarket_main.execution]",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("empty new-market-filter fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("new_market_filter")
            && m.contains("must be non-empty")),
        "expected empty new_market_filter keyword validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_new_market_filter_unknown_fields() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "transport_backend = \"sockudo\"\n\n[venues.polymarket_main.execution]",
        "transport_backend = \"sockudo\"\nnew_market_filter = { kind = \"keyword\", keyword = \"bitcoin\", typo = true }\n\n[venues.polymarket_main.execution]",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("unknown new-market-filter fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("new_market_filter") && m.contains("typo")),
        "validation error should name the unknown new_market_filter field, got: {messages:#?}"
    );
}

#[test]
fn rejects_nautilus_loop_debug_true_for_rust_live_runtime() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("loop_debug = false", "loop_debug = true");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("nautilus loop-debug fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("nautilus.loop_debug") && m.contains("must be false")),
        "expected loop_debug validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_logging_credential_module_level_below_warn() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "credential_module_level = \"WARN\"",
        "credential_module_level = \"INFO\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("logging credential module level fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(
            |m| m.contains("logging.credential_module_level") && m.contains("WARN or stricter")
        ),
        "expected credential module log-level validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_logging_clear_log_file_true_for_rust_live_runtime() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("clear_log_file = false", "clear_log_file = true");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("logging clear-log-file fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("logging.clear_log_file") && m.contains("must be false")),
        "expected clear_log_file validation error, got: {messages:#?}"
    );
}

#[test]
fn accepts_configured_venue_keys_without_global_provider_kind_cap() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig =
        toml::from_str(&fixture).expect("configured venue-key fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.is_empty(),
        "configured venue ids must validate without a global provider-kind count cap; got: {messages:#?}"
    );
}

#[test]
fn accepts_more_than_one_polymarket_venue_when_keys_are_distinct() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let extra_venue = "\n\n[venues.polymarket_secondary]\nkind = \"polymarket\"\n\n[venues.polymarket_secondary.data]\nbase_url_http = \"https://test.invalid/clob\"\nbase_url_ws = \"wss://test.invalid/ws/market\"\nbase_url_gamma = \"https://test.invalid/gamma\"\nbase_url_data_api = \"https://test.invalid/data\"\nhttp_timeout_seconds = 60\nws_timeout_seconds = 30\nsubscribe_new_markets = false\nauto_load_missing_instruments = false\nupdate_instruments_interval_minutes = 60\nwebsocket_max_subscriptions_per_connection = 200\nauto_load_debounce_milliseconds = 250\ntransport_backend = \"tungstenite\"\n";
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = format!("{fixture}{extra_venue}");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("two-polymarket-venues fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.is_empty(),
        "configured venue ids must own routing; duplicate provider kind alone must not be rejected: {messages:#?}"
    );
}
