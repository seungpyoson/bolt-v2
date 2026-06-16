use bolt_v2::{
    bolt_v3_archetypes::{complete_set_arbitrage, validation_bindings},
    bolt_v3_basket_execution::{
        BoltV3BasketExecutionConfig, BoltV3BasketExecutionEvent, BoltV3BasketExecutionLegIntent,
        BoltV3BasketExecutionState, BoltV3BasketExecutionStatus,
        BoltV3BasketExecutionSubmitDisposition, BoltV3BasketFillSource, BoltV3BasketRepairPolicy,
        BoltV3BasketUnwindPolicy,
    },
    bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig},
    strategies::{complete_set_arbitrage as complete_set_strategy, production_strategy_registry},
    strategy_runtime_bindings::runtime_bindings,
};
use nautilus_model::enums::{OrderSide, OrderType, TimeInForce};
use rust_decimal::Decimal;

const SUBMIT_NOW_UNIX_MS: u64 = 2_000;
const SUBMIT_MAX_OBSERVATION_AGE_MS: u64 = 500;
const SUBMIT_OBSERVED_UNIX_MS: u64 = 1_750;

#[test]
fn complete_set_source_files_are_registered_for_runtime_activation() {
    assert_eq!(complete_set_arbitrage::KEY, "complete_set_arbitrage");
    assert_eq!(complete_set_strategy::KEY, "complete_set_arbitrage");

    assert!(
        validation_bindings()
            .iter()
            .any(|binding| binding.key == complete_set_arbitrage::KEY),
        "Task 10 must activate complete_set_arbitrage validation binding"
    );
    assert!(
        runtime_bindings()
            .iter()
            .any(|binding| binding.key == complete_set_arbitrage::KEY),
        "Task 10 must activate complete_set_arbitrage runtime binding"
    );
    assert!(
        production_strategy_registry()
            .expect("production registry should build")
            .get(complete_set_arbitrage::KEY)
            .is_some(),
        "Task 10 must register complete_set_arbitrage in production strategy registry"
    );
}

#[test]
fn complete_set_archetype_declares_no_rv_or_required_reference_roles() {
    assert!(!complete_set_arbitrage::requires_realized_volatility_surface());
    assert!(complete_set_arbitrage::gate_requirements().is_empty());
    assert!(complete_set_arbitrage::required_reference_data_roles().is_empty());
    assert!(
        complete_set_arbitrage::optional_signal_gate_keys(&complete_set_parameters())
            .expect("valid parameters should parse")
            .is_empty()
    );
}

#[test]
fn complete_set_archetype_validates_runtime_schema_scanner_bounds_and_submit_mode() {
    let root = root_config();
    let valid = complete_set_strategy_config(&complete_set_strategy_toml());
    let errors =
        complete_set_arbitrage::validate_strategy("strategy `complete-set`", &root, &valid, None);
    assert!(
        errors.is_empty(),
        "valid complete-set config failed: {errors:#?}"
    );

    for (case, strategy_toml, expected) in [
        (
            "missing depth",
            complete_set_strategy_toml().replace("vwap_depth_limit_bps = 2000\n", ""),
            "parameters.runtime.vwap_depth_limit_bps is required",
        ),
        (
            "zero slippage",
            complete_set_strategy_toml()
                .replace("slippage_buffer_bps = 100", "slippage_buffer_bps = 0"),
            "parameters.runtime.slippage_buffer_bps must be positive",
        ),
        (
            "unsupported mode",
            complete_set_strategy_toml()
                .replace("submit_mode = \"ioc\"", "submit_mode = \"scan_all\""),
            "parameters.runtime.submit_mode `scan_all` is not supported",
        ),
    ] {
        let strategy = complete_set_strategy_config(&strategy_toml);
        let errors = complete_set_arbitrage::validate_strategy(
            "strategy `complete-set`",
            &root,
            &strategy,
            None,
        );
        assert!(
            errors.iter().any(|message| message.contains(expected)),
            "{case} should contain `{expected}`, got {errors:#?}"
        );
    }
}

#[test]
fn complete_set_submit_modes_map_to_nt_order_templates() {
    assert_eq!(
        complete_set_arbitrage::supported_submit_modes(),
        vec![complete_set_arbitrage::CompleteSetSubmitMode::Ioc]
    );

    let contract = complete_set_arbitrage::submit_mode_contract(
        complete_set_arbitrage::CompleteSetSubmitMode::Ioc,
    );
    assert_eq!(
        contract.submit_mode,
        complete_set_arbitrage::CompleteSetSubmitMode::Ioc
    );
    assert_eq!(contract.order_template.order_type, OrderType::Market);
    assert_eq!(contract.order_template.time_in_force, TimeInForce::Ioc);
    assert!(
        contract.nt_template_errors.is_empty(),
        "ioc mode must be accepted by pinned NT order-template checks: {:?}",
        contract.nt_template_errors
    );
}

#[test]
fn strategy_shell_forwards_events_into_shared_executor_without_submit_mechanics() {
    let mut shell = complete_set_strategy::CompleteSetArbitrageShell::new("complete-set-main");
    let policy = shell.mechanics_policy();
    assert!(policy.shared_basket_execution_owns_admission);
    assert!(policy.shared_basket_execution_owns_venue_mutation);
    assert!(policy.shared_basket_execution_owns_fillability);
    assert!(policy.shared_basket_execution_owns_repair_unwind);

    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-SHELL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("submit command should persist client order ids before fills");
    basket
        .apply_event(BoltV3BasketExecutionEvent::VenueOrderId {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: "VOID-YES".to_string(),
        })
        .expect("venue id should apply before shell forwards fill");
    shell
        .forward_executor_event(
            &mut basket,
            BoltV3BasketExecutionEvent::LegFill {
                client_order_id: "COID-YES".to_string(),
                venue_order_id: Some("VOID-YES".to_string()),
                quantity: dec("1.0"),
                cost: dec("0.44"),
                source: BoltV3BasketFillSource::Strategy,
            },
        )
        .expect("shell should forward event to shared executor");

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Partial);
    assert_eq!(shell.forwarded_event_count(), 1);
}

#[test]
fn strategy_shell_keeps_live_settlement_disabled_until_task0_signal_is_reachable() {
    assert_eq!(
        complete_set_strategy::live_settlement_policy(),
        complete_set_strategy::CompleteSetSettlementPolicy::RejectUntilReachableNtSignal
    );

    let mut basket = reserved_basket();
    let rejected = complete_set_strategy::forward_settlement_signal(&mut basket)
        .expect_err("settlement signal remains gated by Task 0 disposition");
    assert_eq!(
        rejected.to_string(),
        "live settlement signal is not reachable"
    );
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
}

fn reserved_basket() -> BoltV3BasketExecutionState {
    let config = BoltV3BasketExecutionConfig {
        repair: BoltV3BasketRepairPolicy {
            max_retries: 2,
            max_book_age_ms: 250,
            max_slippage_bps: 50,
            max_depth_levels: 4,
            allow_unwind_when_repair_denied: true,
        },
        unwind: BoltV3BasketUnwindPolicy {
            max_retries: 2,
            max_book_age_ms: 250,
            max_slippage_bps: 50,
            max_depth_levels: 4,
        },
    };
    let mut basket = BoltV3BasketExecutionState::candidate(
        "basket-1",
        "complete-set-main",
        "polymarket-main",
        vec![
            ("YES", "YES.POLYMARKET", dec("1.0")),
            ("NO", "NO.POLYMARKET", dec("1.0")),
        ],
        vec![vec![dec("1.0"), dec("0.0")], vec![dec("0.0"), dec("1.0")]],
        dec("0.10"),
        dec("1000"),
        config,
    )
    .expect("candidate should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::ReservationPersisted)
        .expect("reservation should persist");
    basket
}

fn leg_intents() -> Vec<BoltV3BasketExecutionLegIntent> {
    vec![
        BoltV3BasketExecutionLegIntent {
            leg_id: "YES".to_string(),
            instrument_id: "YES.POLYMARKET".to_string(),
            venue: "POLYMARKET".to_string(),
            client_order_id: "COID-YES".to_string(),
            side: OrderSide::Buy,
            quantity: dec("1.0"),
            notional: dec("0.44"),
            observed_unix_ms: SUBMIT_OBSERVED_UNIX_MS,
        },
        BoltV3BasketExecutionLegIntent {
            leg_id: "NO".to_string(),
            instrument_id: "NO.POLYMARKET".to_string(),
            venue: "POLYMARKET".to_string(),
            client_order_id: "COID-NO".to_string(),
            side: OrderSide::Buy,
            quantity: dec("1.0"),
            notional: dec("0.46"),
            observed_unix_ms: SUBMIT_OBSERVED_UNIX_MS,
        },
    ]
}

fn complete_set_parameters() -> toml::Value {
    complete_set_strategy_config(&complete_set_strategy_toml()).parameters
}

fn complete_set_strategy_config(toml_source: &str) -> BoltV3StrategyConfig {
    toml::from_str(toml_source).expect("complete-set strategy config should parse")
}

fn root_config() -> BoltV3RootConfig {
    toml::from_str(include_str!("fixtures/bolt_v3/root.toml"))
        .expect("fixture root config should parse")
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

[reference_data]

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

fn dec(value: &str) -> Decimal {
    value.parse().expect("decimal fixture should parse")
}
