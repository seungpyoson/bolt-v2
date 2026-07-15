use bolt_v2::{
    bolt_v3_complete_set_contract,
    bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig},
    strategies::{
        complete_set_arbitrage::{
            self as complete_set_strategy, archetype as complete_set_arbitrage,
        },
        production_strategy_registry,
    },
    strategy_bindings::{production_runtime_bindings, production_validation_bindings},
};
use nautilus_model::enums::{OrderType, TimeInForce};

#[test]
fn complete_set_runtime_retains_shared_realized_volatility_capability() {
    assert!(
        complete_set_arbitrage::RUNTIME_BINDING
            .capabilities
            .realized_volatility,
        "complete-set runtime binding must retain realized-volatility capability"
    );
    assert!(
        !complete_set_arbitrage::RUNTIME_BINDING
            .capabilities
            .settlement,
        "complete-set runtime binding must not opt into settlement capability"
    );
}

#[test]
fn complete_set_source_files_are_registered_for_runtime_activation() {
    assert_eq!(complete_set_arbitrage::KEY, "complete_set_arbitrage");
    assert_eq!(complete_set_strategy::KEY, "complete_set_arbitrage");

    assert!(
        production_validation_bindings()
            .iter()
            .any(|binding| binding.key == complete_set_arbitrage::KEY),
        "Task 10 must activate complete_set_arbitrage validation binding"
    );
    assert!(
        production_runtime_bindings()
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
fn complete_set_archetype_declares_no_required_reference_roles() {
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
        (
            "invalid order tag",
            complete_set_strategy_toml().replace("order_id_tag = \"901\"", "order_id_tag = \"\""),
            "derived from order_id_tag is not a valid NT StrategyId",
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
        bolt_v3_complete_set_contract::supported_submit_modes(),
        vec![bolt_v3_complete_set_contract::CompleteSetSubmitMode::Ioc]
    );

    let contract = bolt_v3_complete_set_contract::submit_mode_contract(
        bolt_v3_complete_set_contract::CompleteSetSubmitMode::Ioc,
    );
    assert_eq!(
        contract.submit_mode,
        bolt_v3_complete_set_contract::CompleteSetSubmitMode::Ioc
    );
    assert_eq!(contract.order_template.order_type, OrderType::Market);
    assert_eq!(contract.order_template.time_in_force, TimeInForce::Ioc);
    assert!(
        contract.nt_template_errors.is_empty(),
        "ioc mode must be accepted by pinned NT order-template checks: {:?}",
        contract.nt_template_errors
    );
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
