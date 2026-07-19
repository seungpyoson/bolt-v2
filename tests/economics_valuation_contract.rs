use std::collections::BTreeMap;

use bolt_v2::bolt_v3_economics_config::{
    ValuationConfig, ValuationLegConfig, ValuationOrientation, ValuationPolicy,
    ValuationRouteConfig,
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
        fetched_at_ns: 95,
        valid_until_ns: 110,
    }
}

#[test]
fn configured_provider_values_usdc_e_from_the_nt_usdc_market_at_configured_parity() {
    let config = ValuationConfig {
        routes: BTreeMap::from([(
            "pusd-usd".to_string(),
            ValuationRouteConfig {
                from_unit: "pUSD".to_string(),
                to_currency: "USD".to_string(),
                legs: vec![
                    ValuationLegConfig::ProviderConversion {
                        from_unit: "pUSD".to_string(),
                        to_unit: "USDC.e".to_string(),
                        source_id: "pusd-usdc-e-redemption".to_string(),
                        max_age_ms: 1,
                    },
                    ValuationLegConfig::MarketQuote {
                        from_unit: "USDC.e".to_string(),
                        source_currency: "USDC".to_string(),
                        source_currency_per_from_unit: "1".to_string(),
                        to_unit: "USD".to_string(),
                        valuation_policy: ValuationPolicy::TopOfBookMidpoint,
                        client_id: "coinbase-data".to_string(),
                        instrument_id: "USDC-USD.COINBASE".to_string(),
                        orientation: ValuationOrientation::BaseToQuote,
                        max_age_ms: 1,
                    },
                ],
            },
        )]),
    };
    let provider = ConfiguredValuationProvider::from_config(
        &config,
        &[
            AuthoritativeValuationObservation::ProviderConversion {
                source_id: "pusd-usdc-e-redemption".to_string(),
                from_unit: native_unit("pUSD"),
                to_unit: native_unit("USDC.e"),
                rate: decimal("1"),
                snapshot_id: SnapshotId::new("pusd-usdc-e-contract-100").unwrap(),
                observed_at_ns: 100,
                fetched_at_ns: 100,
                valid_until_ns: 1_000_100,
            },
            AuthoritativeValuationObservation::MarketQuote {
                client_id: "coinbase-data".to_string(),
                instrument_id: "USDC-USD.COINBASE".to_string(),
                price: decimal("0.99"),
                snapshot_id: SnapshotId::new("coinbase-usdc-usd-100").unwrap(),
                observed_at_ns: 100,
                fetched_at_ns: 100,
                valid_until_ns: 1_000_100,
            },
        ],
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
    assert_eq!(evidence.valid_until_ns, Some(1_000_100));
    assert_eq!(
        evidence.source_snapshot_ids,
        vec![
            SnapshotId::new("pusd-usdc-e-contract-100").unwrap(),
            SnapshotId::new("coinbase-usdc-usd-100").unwrap(),
        ]
    );
    assert!(matches!(
        provider.value(
            &SignedNativeEffect::currency(decimal("-2"), native_unit("pUSD")).unwrap(),
            &ValuationRequest {
                reporting_unit: native_unit("USD"),
                reporting_policy_id: ReportingPolicyId::new("primary-pnl").unwrap(),
                requested_at_ns: 1_000_101,
            },
        ),
        Err(EconomicsUnavailable::StaleValuation { .. })
    ));
}

#[test]
fn configured_provider_rejects_missing_or_duplicate_market_authority() {
    let config = ValuationConfig {
        routes: BTreeMap::from([(
            "usdc-usd".to_string(),
            ValuationRouteConfig {
                from_unit: "USDC".to_string(),
                to_currency: "USD".to_string(),
                legs: vec![ValuationLegConfig::MarketQuote {
                    from_unit: "USDC".to_string(),
                    source_currency: "USDC".to_string(),
                    source_currency_per_from_unit: "1".to_string(),
                    to_unit: "USD".to_string(),
                    valuation_policy: ValuationPolicy::TopOfBookMidpoint,
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
    let observation = AuthoritativeValuationObservation::MarketQuote {
        client_id: "coinbase-data".to_string(),
        instrument_id: "USDC-USD.COINBASE".to_string(),
        price: decimal("1"),
        snapshot_id: SnapshotId::new("coinbase-usdc-usd-100").unwrap(),
        observed_at_ns: 100,
        fetched_at_ns: 100,
        valid_until_ns: 1_000_100,
    };
    assert!(matches!(
        ConfiguredValuationProvider::from_config(&config, &[observation.clone(), observation]),
        Err(EconomicsUnavailable::AmbiguousQuoteAuthority)
    ));
}

#[test]
fn configured_provider_rejects_contradictory_observation_timeline() {
    let config = ValuationConfig {
        routes: BTreeMap::from([(
            "usdc-usd".to_string(),
            ValuationRouteConfig {
                from_unit: "USDC".to_string(),
                to_currency: "USD".to_string(),
                legs: vec![ValuationLegConfig::MarketQuote {
                    from_unit: "USDC".to_string(),
                    source_currency: "USDC".to_string(),
                    source_currency_per_from_unit: "1".to_string(),
                    to_unit: "USD".to_string(),
                    valuation_policy: ValuationPolicy::TopOfBookMidpoint,
                    client_id: "coinbase-data".to_string(),
                    instrument_id: "USDC-USD.COINBASE".to_string(),
                    orientation: ValuationOrientation::BaseToQuote,
                    max_age_ms: 1,
                }],
            },
        )]),
    };
    let observation = AuthoritativeValuationObservation::MarketQuote {
        client_id: "coinbase-data".to_string(),
        instrument_id: "USDC-USD.COINBASE".to_string(),
        price: decimal("1"),
        snapshot_id: SnapshotId::new("coinbase-usdc-usd-100").unwrap(),
        observed_at_ns: 101,
        fetched_at_ns: 100,
        valid_until_ns: 1_000_100,
    };

    assert!(matches!(
        ConfiguredValuationProvider::from_config(&config, &[observation]),
        Err(EconomicsUnavailable::InvalidQuoteValidityPolicy)
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
    let effect = SignedNativeEffect::currency(decimal("-2.00"), native_unit("HYPE")).unwrap();
    let route = route(
        "HYPE",
        "pUSD",
        vec![leg("HYPE", "USDC", "1.50"), leg("USDC", "pUSD", "0.80")],
    );
    let evidence = value_with_route(&effect, &native_unit("pUSD"), Some(&route), 100).unwrap();

    assert_eq!(evidence.normalized_amount, decimal("-2.40"));
    assert_eq!(evidence.route_id, Some(route.route_id));
}

#[test]
fn valuation_rate_and_normalized_amount_overflow_fail_closed() {
    let effect = SignedNativeEffect::currency(decimal("2"), native_unit("HYPE")).unwrap();
    let overflowing_route = route(
        "HYPE",
        "pUSD",
        vec![
            leg("HYPE", "USDC", &rust_decimal::Decimal::MAX.to_string()),
            leg("USDC", "pUSD", "2"),
        ],
    );

    assert_eq!(
        value_with_route(&effect, &native_unit("pUSD"), Some(&overflowing_route), 100,),
        Err(EconomicsUnavailable::InvalidDecimal)
    );
}

#[test]
fn disconnected_cyclic_stale_and_implicit_stablecoin_routes_fail_closed() {
    let effect = SignedNativeEffect::currency(decimal("-2.00"), native_unit("USDC")).unwrap();

    assert!(matches!(
        value_with_route(&effect, &native_unit("pUSD"), None, 100),
        Err(EconomicsUnavailable::MissingValuationRoute { .. })
    ));

    let disconnected = route("USDC", "pUSD", vec![leg("HYPE", "pUSD", "1")]);
    assert!(matches!(
        value_with_route(&effect, &native_unit("pUSD"), Some(&disconnected), 100),
        Err(EconomicsUnavailable::DisconnectedValuationRoute { .. })
    ));

    let cyclic = route(
        "USDC",
        "pUSD",
        vec![
            leg("USDC", "HYPE", "1"),
            leg("HYPE", "USDC", "1"),
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
