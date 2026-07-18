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
routing_attachment_policy = "forbidden"
reporting_policy = "shared-pnl"
quote_refresh_secs = 5
quote_max_age_secs = 10
quote_validity_ms = 4000
resting_order_refresh_margin_ms = 500
carry_surfaces = ["perp"]
product_surface_policies = { perp = "default" }

[sources]
account_fees = "user_fees"

[formula]
rate_scale = "1"

[quote_components.protocol]
component_id = "protocol-execution"
formula_id = "configured-protocol-formula"
rate_factor_id = "configured-protocol-rate"

[assets.settlement]
native_unit = "pUSD"
identity_kind = "currency"
evidence_fixture_id = "settlement-pusd-v1"

[edge_basis.default]
resolver_id = "perp-notional"
policy_version = 1
product_metadata_source = "product-snapshot"

[valuation]
routes = {}

[carry]
funding_interval_secs = 3600
funding_schedule_phase_secs = 0
funding_venue_rate_cap_bps_per_hour = "400"
funding_standard_price_stress_multiplier = "1.5"
component_id = "funding-carry"
formula_id = "funding-rate-bound"
point_rate_factor_id = "funding-point-rate"
bound_rate_factor_id = "funding-bound-rate"
risk_policy_id = "funding-bound"
stress_fixture_id = "funding-stress-v1"
oracle_price_factor_id = "funding-oracle-price"
next_funding_at_factor_id = "funding-next-event-at"
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
fn non_reporting_native_unit_requires_one_explicit_valuation_route() {
    let source = valid_config().replace("native_unit = \"pUSD\"", "native_unit = \"USDC\"");
    let config = parse(&source).unwrap();
    assert!(
        config
            .validate(&reporting(), &BTreeSet::new())
            .iter()
            .any(|error| {
                matches!(
                    error,
                    EconomicsConfigError::MissingValuationRoute { native_unit, reporting_currency }
                        if native_unit == "USDC" && reporting_currency == "pUSD"
                )
            })
    );
}

#[test]
fn sources_formula_components_and_assets_are_required_policy_not_defaults() {
    for block in [
        "[sources]\naccount_fees = \"user_fees\"\n",
        "[formula]\nrate_scale = \"1\"\n",
        "[quote_components.protocol]\ncomponent_id = \"protocol-execution\"\nformula_id = \"configured-protocol-formula\"\nrate_factor_id = \"configured-protocol-rate\"\n",
        "[assets.settlement]\nnative_unit = \"pUSD\"\nidentity_kind = \"currency\"\nevidence_fixture_id = \"settlement-pusd-v1\"\n",
    ] {
        assert!(parse(&valid_config().replace(block, "")).is_err());
    }
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
valuation_policy = "top_of_book_midpoint"
client_id = "fx-data"
instrument_id = "USDC-pUSD"
orientation = "base_to_quote"
max_age_ms = 1000
legs = []

[valuation.routes.usdc-duplicate]
from_unit = "USDC"
to_currency = "pUSD"
valuation_policy = "top_of_book_midpoint"
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
valuation_policy = "top_of_book_midpoint"
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
valuation_policy = "top_of_book_midpoint"
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
