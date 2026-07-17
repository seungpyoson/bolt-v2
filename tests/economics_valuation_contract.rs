use bolt_v2::economics::{
    EconomicsUnavailable, SignedNativeEffect, SnapshotId, ValuationLegEvidence, ValuationRoute,
    ValuationRouteId, value_with_route,
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
