use std::collections::BTreeMap;

use bolt_v2::bolt_v3_economics_config::{
    ValuationConfig, ValuationLegConfig, ValuationOrientation, ValuationRouteConfig,
};
use bolt_v2::bolt_v3_economics_runtime::{
    AuthoritativeValuationObservation, ConfiguredValuationProvider,
};
use bolt_v2::economics::{
    EconomicsUnavailable, ReportingPolicyId, SignedNativeEffect, SnapshotId, ValuationLegEvidence,
    ValuationProvider, ValuationRequest, ValuationRoute, ValuationRouteId, value_with_route,
};

use super::economics_support::{decimal, native_unit};

fn leg(from: &str, to: &str, rate: &str) -> ValuationLegEvidence {
    ValuationLegEvidence {
        from_unit: native_unit(from),
        to_unit: native_unit(to),
        rate: decimal(rate),
        source_snapshot_id: SnapshotId::new(format!("{from}-{to}")).unwrap(),
        observed_at_ns: 90,
        valid_until_ns: 110,
    }
}

#[test]
fn configured_provider_resolves_exact_toml_route_from_fresh_market_observation() {
    let config = ValuationConfig {
        routes: BTreeMap::from([(
            "pusd-usd".to_string(),
            ValuationRouteConfig {
                from_unit: "pUSD".to_string(),
                to_currency: "USD".to_string(),
                valuation_policy:
                    bolt_v2::bolt_v3_economics_config::ValuationPolicy::TopOfBookMidpoint,
                client_id: "coinbase-data".to_string(),
                instrument_id: "USDC-USD.COINBASE".to_string(),
                orientation: ValuationOrientation::BaseToQuote,
                max_age_ms: 1,
                legs: vec![ValuationLegConfig {
                    from_unit: "pUSD".to_string(),
                    to_unit: "USD".to_string(),
                    client_id: "coinbase-data".to_string(),
                    instrument_id: "USDC-USD.COINBASE".to_string(),
                    orientation: ValuationOrientation::BaseToQuote,
                    max_age_ms: 1,
                }],
            },
        )]),
    };
    let provider = ConfiguredValuationProvider::from_config(
        &config,
        &[AuthoritativeValuationObservation {
            client_id: "coinbase-data".to_string(),
            instrument_id: "USDC-USD.COINBASE".to_string(),
            price: decimal("0.99"),
            snapshot_id: SnapshotId::new("coinbase-usdc-usd-100").unwrap(),
            observed_at_ns: 100,
        }],
    )
    .unwrap();
    let evidence = provider
        .value(
            &SignedNativeEffect::currency(decimal("-2"), native_unit("pUSD")).unwrap(),
            &ValuationRequest {
                reporting_unit: native_unit("USD"),
                reporting_policy_id: ReportingPolicyId::new("primary-pnl").unwrap(),
                requested_at_ns: 100,
            },
        )
        .unwrap();

    assert_eq!(evidence.normalized_amount, decimal("-1.98"));
    assert_eq!(
        evidence.source_snapshot_ids,
        vec![SnapshotId::new("coinbase-usdc-usd-100").unwrap()]
    );
}

#[test]
fn configured_provider_rejects_missing_or_duplicate_market_authority() {
    let config = ValuationConfig {
        routes: BTreeMap::from([(
            "usdc-usd".to_string(),
            ValuationRouteConfig {
                from_unit: "USDC".to_string(),
                to_currency: "USD".to_string(),
                valuation_policy:
                    bolt_v2::bolt_v3_economics_config::ValuationPolicy::TopOfBookMidpoint,
                client_id: "coinbase-data".to_string(),
                instrument_id: "USDC-USD.COINBASE".to_string(),
                orientation: ValuationOrientation::BaseToQuote,
                max_age_ms: 1,
                legs: vec![ValuationLegConfig {
                    from_unit: "USDC".to_string(),
                    to_unit: "USD".to_string(),
                    client_id: "coinbase-data".to_string(),
                    instrument_id: "USDC-USD.COINBASE".to_string(),
                    orientation: ValuationOrientation::BaseToQuote,
                    max_age_ms: 1,
                }],
            },
        )]),
    };
    assert!(matches!(
        ConfiguredValuationProvider::from_config(&config, &[]),
        Err(EconomicsUnavailable::MissingQuoteAuthority)
    ));
    let observation = AuthoritativeValuationObservation {
        client_id: "coinbase-data".to_string(),
        instrument_id: "USDC-USD.COINBASE".to_string(),
        price: decimal("1"),
        snapshot_id: SnapshotId::new("coinbase-usdc-usd-100").unwrap(),
        observed_at_ns: 100,
    };
    assert!(matches!(
        ConfiguredValuationProvider::from_config(&config, &[observation.clone(), observation]),
        Err(EconomicsUnavailable::AmbiguousQuoteAuthority)
    ));
}

fn route(from: &str, to: &str, legs: Vec<ValuationLegEvidence>) -> ValuationRoute {
    ValuationRoute {
        route_id: ValuationRouteId::new("configured-route").unwrap(),
        from_unit: native_unit(from),
        to_currency: native_unit(to),
        legs,
        valid_until_ns: 110,
    }
}

#[test]
fn exact_identity_needs_no_route_or_invented_peg() {
    let effect = SignedNativeEffect::currency(decimal("-2.00"), native_unit("pUSD")).unwrap();
    let evidence = value_with_route(&effect, &native_unit("pUSD"), None, 100).unwrap();

    assert_eq!(evidence.normalized_amount, decimal("-2.00"));
    assert_eq!(evidence.route_id, None);
    assert!(evidence.source_snapshot_ids.is_empty());
}

#[test]
fn configured_multi_leg_route_values_native_effect_once() {
    let effect = SignedNativeEffect::currency(decimal("-2.00"), native_unit("TOKEN")).unwrap();
    let route = route(
        "TOKEN",
        "pUSD",
        vec![leg("TOKEN", "USDC", "1.50"), leg("USDC", "pUSD", "0.80")],
    );
    let evidence = value_with_route(&effect, &native_unit("pUSD"), Some(&route), 100).unwrap();

    assert_eq!(evidence.normalized_amount, decimal("-2.40"));
    assert_eq!(evidence.route_id, Some(route.route_id));
}

#[test]
fn disconnected_cyclic_stale_and_implicit_stablecoin_routes_fail_closed() {
    let effect = SignedNativeEffect::currency(decimal("-2.00"), native_unit("USDC")).unwrap();

    assert!(matches!(
        value_with_route(&effect, &native_unit("pUSD"), None, 100),
        Err(EconomicsUnavailable::MissingValuationRoute { .. })
    ));

    let disconnected = route("USDC", "pUSD", vec![leg("TOKEN", "pUSD", "1")]);
    assert!(matches!(
        value_with_route(&effect, &native_unit("pUSD"), Some(&disconnected), 100),
        Err(EconomicsUnavailable::DisconnectedValuationRoute { .. })
    ));

    let cyclic = route(
        "USDC",
        "pUSD",
        vec![
            leg("USDC", "TOKEN", "1"),
            leg("TOKEN", "USDC", "1"),
            leg("USDC", "pUSD", "1"),
        ],
    );
    assert!(matches!(
        value_with_route(&effect, &native_unit("pUSD"), Some(&cyclic), 100),
        Err(EconomicsUnavailable::CyclicValuationRoute { .. })
    ));

    let mut stale = route("USDC", "pUSD", vec![leg("USDC", "pUSD", "1")]);
    stale.legs[0].valid_until_ns = 99;
    assert!(matches!(
        value_with_route(&effect, &native_unit("pUSD"), Some(&stale), 100),
        Err(EconomicsUnavailable::StaleValuation { .. })
    ));
}
