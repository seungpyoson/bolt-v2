use bolt_v2::strategies::{
    binary_oracle_maker::{parse_config, validate_config},
    registry::ValidationError,
};

fn valid_raw() -> toml::Value {
    toml::toml! {
        strategy_id = "BINARY-ORACLE-MAKER-001"
        order_id_tag = "001"
        oms_type = "netting"
        client_id = "polymarket_main"
        trade_flow_window_secs = 600
        trade_flow_max_samples = 1000
        mu_min_classified_samples = 4
        mu_stale_window_ms = 60000
        mu_min_floor = 0.05
        requote_min_interval_ms = 500
    }
    .into()
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
