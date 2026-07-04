use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig},
    strategies::{
        binary_oracle_maker::{archetype, parse_config, validate_config},
        registry::ValidationError,
    },
};
use toml::Value;

const TEST_ARTIFACT_SHA256: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const VALID_ROOT_TOML: &str = include_str!("fixtures/bolt_v3/root.toml");
const TEST_MILLIS_PER_SECOND_U64: u64 = 1_000;
const TEST_NANOS_PER_MILLI_U64: u64 = TEST_MILLIS_PER_SECOND_U64 * TEST_MILLIS_PER_SECOND_U64;

const TRADE_FLOW_WINDOW_SECS_FIELD: &str = "trade_flow_window_secs";
const TRADE_FLOW_MAX_SAMPLES_FIELD: &str = "trade_flow_max_samples";
const MU_MIN_CLASSIFIED_SAMPLES_FIELD: &str = "mu_min_classified_samples";
const MU_STALE_WINDOW_MS_FIELD: &str = "mu_stale_window_ms";
const MU_MIN_FLOOR_FIELD: &str = "mu_min_floor";
const REQUOTE_MIN_INTERVAL_MS_FIELD: &str = "requote_min_interval_ms";
const QUOTE_INTERVAL_MS_FIELD: &str = "quote_interval_ms";
const MARKETS_FIELD: &str = "markets";
const MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD: &str = "market_portfolio_max_active_markets";
const MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD: &str =
    "market_portfolio_total_bankroll_notional";
const MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD: &str = "market_portfolio_min_slot_notional";

const POSITIVE_REQUIRED_CODE: &str = "positive_required";
const VALUE_OUT_OF_RANGE_CODE: &str = "value_out_of_range";
const CONVERSION_OVERFLOW_CODE: &str = "conversion_overflow";
const UNSATISFIABLE_WARMUP_CODE: &str = "unsatisfiable_warmup";
const BANKROLL_BELOW_MIN_SLOT_CODE: &str = "bankroll_below_min_slot";
const MARKETS_ABOVE_ACTIVE_CAP_CODE: &str = "markets_above_active_cap";

fn valid_raw() -> Value {
    toml::toml! {
        strategy_id = "BINARY-ORACLE-MAKER-001"
        order_id_tag = "001"
        oms_type = "netting"
        client_id = "maker_execution_client"
        trade_flow_window_secs = 600
        trade_flow_max_samples = 1000
        mu_min_classified_samples = 4
        mu_stale_window_ms = 60000
        mu_min_floor = 0.05
        requote_min_interval_ms = 500
        quote_interval_ms = 1000
        market_portfolio_max_active_markets = 3
        market_portfolio_total_bankroll_notional = 1500.0
        market_portfolio_min_slot_notional = 100.0
        markets_config_digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

        [[markets]]
        market_key = "eth-hourly"
        family_key = "updown"
        underlying_asset = "ETH"
        cadence_seconds = 3600
        cadence_slug_token = "1h"
    }
    .into()
}

fn valid_strategy_toml() -> String {
    format!(
        r#"
        schema_version = 2
        strategy_instance_id = "maker-001"
        strategy_archetype = "binary_oracle_maker"
        order_id_tag = "001"
        oms_type = "netting"
        use_uuid_client_order_ids = true
        use_hyphens_in_client_order_ids = false
        external_order_claims = []
        manage_contingent_orders = false
        manage_gtd_expiry = false
        manage_stop = false
        market_exit_interval_ms = 100
        market_exit_max_attempts = 100
        log_events = true
        log_commands = true
        log_rejected_due_post_only_as_warning = true
        execution_client_id = "maker_execution"

        [target]
        kind = "test"

        [signal_data.primary]
        data_client_id = "maker_data"
        instrument_id = "GENERIC.TEST"

        [parameters.runtime]
        trade_flow_window_secs = 600
        trade_flow_max_samples = 1000
        mu_min_classified_samples = 4
        mu_stale_window_ms = 60000
        mu_min_floor = 0.05
        requote_min_interval_ms = 500
        quote_interval_ms = 1000

        [parameters.market_portfolio]
        max_active_markets = 3
        total_bankroll_notional = 1500.0
        min_slot_notional = 100.0

        [[parameters.markets]]
        market_key = "eth-hourly"
        family_key = "updown"
        underlying_asset = "ETH"
        cadence_seconds = 3600
        cadence_slug_token = "1h"

        [parameters.backtest]
        verdict = "pass"
        build_head_sha = "{}"
        strategy_config_hash = "{TEST_ARTIFACT_SHA256}"
        run_artifact = "artifact://maker/backtest/run"
        run_artifact_sha256 = "{TEST_ARTIFACT_SHA256}"
        threshold_artifact = "artifact://maker/backtest/thresholds"
        threshold_artifact_sha256 = "{TEST_ARTIFACT_SHA256}"
        execution_model_artifact = "artifact://maker/backtest/execution-model"
        execution_model_artifact_sha256 = "{TEST_ARTIFACT_SHA256}"
        maker_order_count = 3
        passive_fill_count = 2
        min_passive_fill_count = 2
        resolved_market_count = 5
        min_resolved_market_count = 5
        built_maker_replayed = true
        captured_spread_score_micros = 1000
        fees_score_micros = 100
        adverse_selection_score_micros = 200
        settlement_loss_score_micros = 300
        net_score_micros = 400
        thresholds_registered_before_run = true
        balanced_gate_evaluated = true
        strict_gate_evaluated = true
        balanced_gate_passed = true
        historical_full_depth_l2 = true
        full_population_corpus = true
        entry_gated_corpus_used = false
        custom_fill_model_used = false
        custom_fill_model_source_proven = false
        underlying_spot_causal_join = true
        statistical_significance = true
        shared_fair_value_pricing = true
        shared_settlement_primitive = true

        [parameters.backtest.result_contract_replay]
        execution_model = "nt_backtest_node"
        venue_queue_position = true
        catalog_data_types = ["OrderBookDelta", "TradeTick"]
        "#,
        bolt_v2::bolt_v3_operator_artifacts::current_build_head_sha()
            .expect("test binary must carry BOLT_V3_BUILD_HEAD_SHA")
    )
}

fn valid_strategy_config() -> BoltV3StrategyConfig {
    toml::from_str(&valid_strategy_toml()).expect("valid maker strategy config parses")
}

fn valid_root() -> BoltV3RootConfig {
    toml::from_str(VALID_ROOT_TOML).expect("valid root fixture parses")
}

fn table_mut<'a>(value: &'a mut Value, path: &[&str]) -> &'a mut toml::map::Map<String, Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor
            .as_table_mut()
            .and_then(|table| table.get_mut(*key))
            .unwrap_or_else(|| panic!("missing table path component `{key}` in {path:?}"));
    }
    cursor
        .as_table_mut()
        .unwrap_or_else(|| panic!("path {path:?} is not a table"))
}

fn push_market(raw: &mut Value, market_key: &str) {
    let markets = raw
        .as_table_mut()
        .expect("config is a table")
        .get_mut(MARKETS_FIELD)
        .expect("valid raw config has markets")
        .as_array_mut()
        .expect("markets is an array");
    let mut market = markets
        .first()
        .expect("valid raw config has one market")
        .clone();
    market.as_table_mut().expect("market is a table").insert(
        "market_key".to_string(),
        Value::String(market_key.to_string()),
    );
    markets.push(market);
}

fn push_strategy_market(strategy: &mut BoltV3StrategyConfig, market_key: &str) {
    let markets = table_mut(&mut strategy.parameters, &[])
        .get_mut("markets")
        .expect("valid strategy parameters have markets")
        .as_array_mut()
        .expect("parameters.markets is an array");
    let mut market = markets
        .first()
        .expect("valid strategy parameters have one market")
        .clone();
    market.as_table_mut().expect("market is a table").insert(
        "market_key".to_string(),
        Value::String(market_key.to_string()),
    );
    markets.push(market);
}

#[test]
fn maker_config_accepts_execution_client_id_for_order_routing() {
    let raw = valid_raw();
    parse_config(&raw).expect("maker config must retain execution client id for order routing");

    let mut errors: Vec<ValidationError> = Vec::new();
    validate_config(&raw, "strategy", &mut errors);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn maker_config_rejects_missing_execution_client_id() {
    let raw: toml::Value = toml::toml! {
        strategy_id = "BINARY-ORACLE-MAKER-001"
        order_id_tag = "001"
        oms_type = "netting"
        trade_flow_window_secs = 600
        trade_flow_max_samples = 1000
        mu_min_classified_samples = 4
        mu_stale_window_ms = 60000
        mu_min_floor = 0.05
        requote_min_interval_ms = 500
        market_portfolio_max_active_markets = 3
        market_portfolio_total_bankroll_notional = 1500.0
        market_portfolio_min_slot_notional = 100.0
    }
    .into();

    assert!(
        parse_config(&raw).is_err(),
        "missing execution client id must fail to parse"
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    validate_config(&raw, "strategy", &mut errors);
    assert!(
        errors
            .iter()
            .any(|error| error.field == "strategy.client_id" && error.code == "missing_client_id"),
        "expected missing_client_id, got: {errors:?}"
    );
}

#[test]
fn maker_config_and_archetype_reject_the_same_mirrored_scalar_bounds() {
    struct ParityCase {
        name: &'static str,
        mutate_config: fn(&mut Value),
        mutate_strategy: fn(&mut BoltV3StrategyConfig),
        config_field: &'static str,
        config_code: &'static str,
        archetype_error: &'static str,
    }

    fn set_raw(field: &'static str, value: Value) -> impl FnOnce(&mut Value) {
        move |raw| {
            raw.as_table_mut()
                .expect("config is a table")
                .insert(field.to_string(), value);
        }
    }

    fn set_runtime(field: &'static str, value: Value) -> impl FnOnce(&mut BoltV3StrategyConfig) {
        move |strategy| {
            table_mut(&mut strategy.parameters, &["runtime"]).insert(field.to_string(), value);
        }
    }

    fn set_market_portfolio(
        field: &'static str,
        value: Value,
    ) -> impl FnOnce(&mut BoltV3StrategyConfig) {
        move |strategy| {
            table_mut(&mut strategy.parameters, &["market_portfolio"])
                .insert(field.to_string(), value);
        }
    }

    macro_rules! parity_cases {
        ($(($name:literal, $mutate_config:expr, $mutate_strategy:expr, $config_field:expr, $config_code:expr, $archetype_error:literal)),+ $(,)?) => {
            &[
                $(
                    ParityCase {
                        name: $name,
                        mutate_config: |raw| ($mutate_config)(raw),
                        mutate_strategy: |strategy| ($mutate_strategy)(strategy),
                        config_field: $config_field,
                        config_code: $config_code,
                        archetype_error: $archetype_error,
                    },
                )+
            ]
        };
    }

    let cases = parity_cases![
        (
            "zero trade_flow_window_secs",
            set_raw(TRADE_FLOW_WINDOW_SECS_FIELD, Value::Integer(0)),
            set_runtime("trade_flow_window_secs", Value::Integer(0)),
            TRADE_FLOW_WINDOW_SECS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.runtime.trade_flow_window_secs must be > 0"
        ),
        (
            "trade_flow_window_secs seconds-to-millis overflow",
            set_raw(
                TRADE_FLOW_WINDOW_SECS_FIELD,
                Value::Integer((u64::MAX / TEST_MILLIS_PER_SECOND_U64 + 1) as i64),
            ),
            set_runtime(
                "trade_flow_window_secs",
                Value::Integer((u64::MAX / TEST_MILLIS_PER_SECOND_U64 + 1) as i64),
            ),
            TRADE_FLOW_WINDOW_SECS_FIELD,
            CONVERSION_OVERFLOW_CODE,
            "parameters.runtime.trade_flow_window_secs (18446744073709552) must be small enough"
        ),
        (
            "zero trade_flow_max_samples",
            set_raw(TRADE_FLOW_MAX_SAMPLES_FIELD, Value::Integer(0)),
            set_runtime("trade_flow_max_samples", Value::Integer(0)),
            TRADE_FLOW_MAX_SAMPLES_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.runtime.trade_flow_max_samples must be > 0"
        ),
        (
            "zero mu_min_classified_samples",
            set_raw(MU_MIN_CLASSIFIED_SAMPLES_FIELD, Value::Integer(0)),
            set_runtime("mu_min_classified_samples", Value::Integer(0)),
            MU_MIN_CLASSIFIED_SAMPLES_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.runtime.mu_min_classified_samples must be > 0"
        ),
        (
            "mu_min_classified_samples above trade_flow_max_samples",
            set_raw(MU_MIN_CLASSIFIED_SAMPLES_FIELD, Value::Integer(1001)),
            set_runtime("mu_min_classified_samples", Value::Integer(1001)),
            MU_MIN_CLASSIFIED_SAMPLES_FIELD,
            UNSATISFIABLE_WARMUP_CODE,
            "parameters.runtime.mu_min_classified_samples (1001) must be <= parameters.runtime.trade_flow_max_samples (1000)"
        ),
        (
            "zero mu_stale_window_ms",
            set_raw(MU_STALE_WINDOW_MS_FIELD, Value::Integer(0)),
            set_runtime("mu_stale_window_ms", Value::Integer(0)),
            MU_STALE_WINDOW_MS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.runtime.mu_stale_window_ms must be > 0"
        ),
        (
            "mu_min_floor zero",
            set_raw(MU_MIN_FLOOR_FIELD, Value::Float(0.0)),
            set_runtime("mu_min_floor", Value::Float(0.0)),
            MU_MIN_FLOOR_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.runtime.mu_min_floor (0) must be finite and in the open interval (0, 1)"
        ),
        (
            "mu_min_floor one",
            set_raw(MU_MIN_FLOOR_FIELD, Value::Float(1.0)),
            set_runtime("mu_min_floor", Value::Float(1.0)),
            MU_MIN_FLOOR_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.runtime.mu_min_floor (1) must be finite and in the open interval (0, 1)"
        ),
        (
            "mu_min_floor NaN",
            set_raw(MU_MIN_FLOOR_FIELD, Value::Float(f64::NAN)),
            set_runtime("mu_min_floor", Value::Float(f64::NAN)),
            MU_MIN_FLOOR_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.runtime.mu_min_floor (NaN) must be finite and in the open interval (0, 1)"
        ),
        (
            "mu_min_floor infinity",
            set_raw(MU_MIN_FLOOR_FIELD, Value::Float(f64::INFINITY)),
            set_runtime("mu_min_floor", Value::Float(f64::INFINITY)),
            MU_MIN_FLOOR_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.runtime.mu_min_floor (inf) must be finite and in the open interval (0, 1)"
        ),
        (
            "zero requote_min_interval_ms",
            set_raw(REQUOTE_MIN_INTERVAL_MS_FIELD, Value::Integer(0)),
            set_runtime("requote_min_interval_ms", Value::Integer(0)),
            REQUOTE_MIN_INTERVAL_MS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.runtime.requote_min_interval_ms must be > 0"
        ),
        (
            "zero quote_interval_ms",
            set_raw(QUOTE_INTERVAL_MS_FIELD, Value::Integer(0)),
            set_runtime("quote_interval_ms", Value::Integer(0)),
            QUOTE_INTERVAL_MS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.runtime.quote_interval_ms must be > 0"
        ),
        (
            "quote_interval_ms millis-to-nanos overflow",
            set_raw(
                QUOTE_INTERVAL_MS_FIELD,
                Value::Integer((u64::MAX / TEST_NANOS_PER_MILLI_U64 + 1) as i64),
            ),
            set_runtime(
                "quote_interval_ms",
                Value::Integer((u64::MAX / TEST_NANOS_PER_MILLI_U64 + 1) as i64),
            ),
            QUOTE_INTERVAL_MS_FIELD,
            CONVERSION_OVERFLOW_CODE,
            "parameters.runtime.quote_interval_ms (18446744073710) must be small enough"
        ),
        (
            "zero market_portfolio_max_active_markets",
            set_raw(MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD, Value::Integer(0)),
            set_market_portfolio("max_active_markets", Value::Integer(0)),
            MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "parameters.market_portfolio.max_active_markets must be > 0"
        ),
        (
            "market_portfolio_total_bankroll_notional NaN",
            set_raw(
                MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
                Value::Float(f64::NAN),
            ),
            set_market_portfolio("total_bankroll_notional", Value::Float(f64::NAN)),
            MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.market_portfolio.total_bankroll_notional must be a positive finite bankroll notional"
        ),
        (
            "market_portfolio_total_bankroll_notional infinity",
            set_raw(
                MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
                Value::Float(f64::INFINITY),
            ),
            set_market_portfolio("total_bankroll_notional", Value::Float(f64::INFINITY)),
            MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.market_portfolio.total_bankroll_notional must be a positive finite bankroll notional"
        ),
        (
            "market_portfolio_min_slot_notional NaN",
            set_raw(
                MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD,
                Value::Float(f64::NAN),
            ),
            set_market_portfolio("min_slot_notional", Value::Float(f64::NAN)),
            MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.market_portfolio.min_slot_notional must be a positive finite per-market slot notional"
        ),
        (
            "market_portfolio_min_slot_notional infinity",
            set_raw(
                MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD,
                Value::Float(f64::INFINITY),
            ),
            set_market_portfolio("min_slot_notional", Value::Float(f64::INFINITY)),
            MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "parameters.market_portfolio.min_slot_notional must be a positive finite per-market slot notional"
        ),
        (
            "markets over active cap",
            |raw: &mut Value| {
                push_market(raw, "btc-hourly");
                push_market(raw, "sol-hourly");
                push_market(raw, "xrp-hourly");
            },
            |strategy: &mut BoltV3StrategyConfig| {
                push_strategy_market(strategy, "btc-hourly");
                push_strategy_market(strategy, "sol-hourly");
                push_strategy_market(strategy, "xrp-hourly");
            },
            MARKETS_FIELD,
            MARKETS_ABOVE_ACTIVE_CAP_CODE,
            "parameters.markets declares 4 markets but parameters.market_portfolio.max_active_markets is 3"
        ),
        (
            "bankroll below min-slot floor",
            set_raw(
                MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
                Value::Float(50.0),
            ),
            set_market_portfolio("total_bankroll_notional", Value::Float(50.0)),
            MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
            BANKROLL_BELOW_MIN_SLOT_CODE,
            "parameters.market_portfolio.total_bankroll_notional must be >= min(declared markets, max_active_markets) * parameters.market_portfolio.min_slot_notional"
        ),
    ];

    let root = valid_root();
    for case in cases {
        let mut raw = valid_raw();
        (case.mutate_config)(&mut raw);
        let mut config_errors = Vec::new();

        validate_config(&raw, "strategy", &mut config_errors);

        assert!(
            config_errors.iter().any(|error| {
                error.field == format!("strategy.{}", case.config_field)
                    && error.code == case.config_code
            }),
            "{}: config validator did not reject with {} on {}; errors={config_errors:?}",
            case.name,
            case.config_code,
            case.config_field
        );

        let mut strategy = valid_strategy_config();
        (case.mutate_strategy)(&mut strategy);
        let archetype_errors =
            archetype::validate_strategy("strategy `maker-001`", &root, &strategy, None);

        assert!(
            archetype_errors
                .iter()
                .any(|error| error.contains(case.archetype_error)),
            "{}: archetype validator did not reject with `{}`; errors={archetype_errors:?}",
            case.name,
            case.archetype_error
        );
    }
}
