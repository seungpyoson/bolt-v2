use std::collections::BTreeSet;

use bolt_v2::bolt_v3_economics_config::{
    EconomicsConfigError, EconomicsReportingConfig, ExecutionEconomicsConfig,
};

fn reporting() -> EconomicsReportingConfig {
    EconomicsReportingConfig {
        policy_id: "shared-pnl".to_string(),
        pnl_currency: "pUSD".to_string(),
    }
}

fn parse(source: &str) -> Result<ExecutionEconomicsConfig, toml::de::Error> {
    toml::from_str(source)
}

fn valid_config() -> &'static str {
    r#"
economics_slice = "quote_only"
reporting_policy = "shared-pnl"
quote_refresh_secs = 5
quote_max_age_secs = 10
quote_validity_ms = 4000
resting_order_refresh_margin_ms = 500
product_surface_policies = { perp = "default" }

[edge_basis.default]
resolver_id = "perp-notional"
product_metadata_source = "product-snapshot"

[valuation]
routes = {}

[carry]
holding_horizon_secs = 3600
risk_policy_id = "funding-bound"
stress_fixture_id = "funding-stress-v1"
"#
}

#[test]
fn quote_only_config_is_strict_and_validates_freshness_and_policy() {
    let config = parse(valid_config()).unwrap();
    assert!(config.validate(&reporting(), &BTreeSet::new()).is_empty());

    assert!(parse(&valid_config().replace("quote_only", "live")).is_err());
    assert!(parse(&format!("{}\nunknown_key = true", valid_config())).is_err());
}

#[test]
fn zero_or_contradictory_quote_windows_fail_closed() {
    let zero =
        parse(&valid_config().replace("quote_refresh_secs = 5", "quote_refresh_secs = 0")).unwrap();
    assert!(!zero.validate(&reporting(), &BTreeSet::new()).is_empty());

    let invalid_margin = parse(&valid_config().replace(
        "resting_order_refresh_margin_ms = 500",
        "resting_order_refresh_margin_ms = 4000",
    ))
    .unwrap();
    assert!(
        !invalid_margin
            .validate(&reporting(), &BTreeSet::new())
            .is_empty()
    );
}

#[test]
fn missing_edge_resolver_and_reporting_policy_mismatch_fail_closed() {
    let missing = parse(&valid_config().replace(
        "product_surface_policies = { perp = \"default\" }",
        "product_surface_policies = { perp = \"missing\" }",
    ))
    .unwrap();
    assert!(!missing.validate(&reporting(), &BTreeSet::new()).is_empty());

    let mismatch = parse(&valid_config().replace("shared-pnl", "other-policy")).unwrap();
    assert!(!mismatch.validate(&reporting(), &BTreeSet::new()).is_empty());
}

#[test]
fn duplicate_disconnected_or_inactive_valuation_authority_fails_closed() {
    let routes = r#"
[valuation.routes.usdc]
from_unit = "USDC"
to_currency = "pUSD"
valuation_policy = "configured-market"
client_id = "fx-data"
instrument_id = "USDC-pUSD"
orientation = "base_to_quote"
max_age_ms = 1000
legs = []

[valuation.routes.usdc-duplicate]
from_unit = "USDC"
to_currency = "pUSD"
valuation_policy = "configured-market"
client_id = "fx-data"
instrument_id = "USDC-pUSD-2"
orientation = "base_to_quote"
max_age_ms = 1000
legs = []
"#;
    let source = valid_config().replace("[valuation]\nroutes = {}", routes);
    let config = parse(&source).unwrap();
    let errors = config.validate(&reporting(), &BTreeSet::new());
    assert!(errors.iter().any(|error| matches!(
        error,
        EconomicsConfigError::DuplicateValuationAuthority { .. }
    )));
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, EconomicsConfigError::InactiveDataClient { .. }))
    );
}

#[test]
fn non_identity_valuation_route_requires_connected_legs() {
    let route = r#"
[valuation.routes.usdc]
from_unit = "USDC"
to_currency = "pUSD"
valuation_policy = "configured-market"
client_id = "fx-data"
instrument_id = "USDC-pUSD"
orientation = "base_to_quote"
max_age_ms = 1000
legs = []
"#;
    let source = valid_config().replace("[valuation]\nroutes = {}", route);
    let config = parse(&source).unwrap();
    let errors = config.validate(&reporting(), &BTreeSet::from(["fx-data".to_string()]));

    assert!(errors.iter().any(|error| matches!(
        error,
        EconomicsConfigError::DisconnectedValuationRoute { .. }
    )));
}

#[test]
fn every_valuation_leg_requires_an_active_configured_source() {
    let route = r#"
[valuation.routes.token]
from_unit = "TOKEN"
to_currency = "pUSD"
valuation_policy = "configured-market"
client_id = "fx-data"
instrument_id = "TOKEN-pUSD"
orientation = "base_to_quote"
max_age_ms = 1000
legs = [
  { from_unit = "TOKEN", to_unit = "pUSD", client_id = "inactive-leg", instrument_id = "", orientation = "base_to_quote", max_age_ms = 1000 }
]
"#;
    let source = valid_config().replace("[valuation]\nroutes = {}", route);
    let config = parse(&source).unwrap();
    let errors = config.validate(&reporting(), &BTreeSet::from(["fx-data".to_string()]));

    assert!(errors.iter().any(|error| matches!(
        error,
        EconomicsConfigError::InactiveDataClient { client_id, .. } if client_id == "inactive-leg"
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        EconomicsConfigError::InvalidText {
            field: bolt_v2::bolt_v3_economics_config::EconomicsConfigField::ValuationInstrument
        }
    )));
}
