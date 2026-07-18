use bolt_v2::economics::{
    AdmissionTreatment, EconomicClass, EconomicsCapabilityHealth, EconomicsUnavailable,
    EdgeBasisEvidence, FormulaId, PointEstimate, RiskBoundAuthority, SignedNativeEffect,
    SnapshotId, SourceId, ValuationEvidence, ValuationRouteId, fold_net_edge,
};

use super::economics_support::{
    canonical_fixture_request, decimal, estimated_component, forecast, guaranteed, native_unit,
    quote_fixture, risk_bound, risk_bound_without_debit_bound,
};

#[test]
fn core_total_uses_guaranteed_point_and_risk_bound_debit() {
    let quote = quote_fixture([
        guaranteed(decimal("-1.00")),
        risk_bound(decimal("-0.25"), decimal("-0.75")),
        forecast(decimal("2.00")),
    ])
    .unwrap();

    assert_eq!(quote.core_total(), decimal("-1.75"));
    assert_eq!(quote.forecast_total(), decimal("0.75"));
    assert!(quote.forecast_complete());
}

#[test]
fn bound_only_component_reserves_core_without_inventing_a_forecast_point() {
    let mut component = risk_bound(decimal("-0.25"), decimal("-0.75"));
    component.point_estimate = PointEstimate::ProvenZero {
        factor_id: FormulaId::new("zero-point-rate").unwrap(),
    };
    component
        .calculation_factors
        .push(bolt_v2::economics::CalculationFactor {
            factor_id: FormulaId::new("zero-point-rate").unwrap(),
            value: decimal("0"),
        });

    let quote = quote_fixture([component]).unwrap();

    assert_eq!(quote.core_total(), decimal("-0.75"));
    assert_eq!(quote.forecast_total(), decimal("0"));
    assert!(quote.forecast_complete());
}

#[test]
fn proven_zero_point_requires_matching_zero_factor_and_charge_shape() {
    let factor_id = FormulaId::new("zero-point-rate").unwrap();
    let mut missing_factor = risk_bound(decimal("-0.25"), decimal("-0.75"));
    missing_factor.point_estimate = PointEstimate::ProvenZero {
        factor_id: factor_id.clone(),
    };
    assert!(matches!(
        quote_fixture([missing_factor]),
        Err(EconomicsUnavailable::InvalidProvenZeroPoint { .. })
    ));

    let mut credit = risk_bound(decimal("-0.25"), decimal("-0.75"));
    credit.point_estimate = PointEstimate::ProvenZero {
        factor_id: factor_id.clone(),
    };
    credit.class = EconomicClass::Credit;
    credit
        .calculation_factors
        .push(bolt_v2::economics::CalculationFactor {
            factor_id,
            value: decimal("0"),
        });
    assert!(matches!(
        quote_fixture([credit]),
        Err(EconomicsUnavailable::InvalidProvenZeroPoint { .. })
    ));
}

#[test]
fn missing_or_positive_risk_bound_rejects_core_quote() {
    assert!(matches!(
        quote_fixture([risk_bound_without_debit_bound()]),
        Err(EconomicsUnavailable::MissingDebitRiskBound { .. })
    ));
    assert!(matches!(
        quote_fixture([risk_bound(decimal("-0.25"), decimal("0.75"))]),
        Err(EconomicsUnavailable::InvalidDebitRiskBound { .. })
    ));
}

#[test]
fn stale_required_component_rejects_admission() {
    let mut component = guaranteed(decimal("-1.00"));
    component.source.valid_until_ns = canonical_fixture_request().requested_at_ns - 1;
    assert!(matches!(
        quote_fixture([component]),
        Err(EconomicsUnavailable::StaleSource { .. })
    ));
}

#[test]
fn stale_forecast_is_discarded_without_authorizing_core() {
    let mut stale_forecast = forecast(decimal("2.00"));
    stale_forecast.source.valid_until_ns = canonical_fixture_request().requested_at_ns - 1;
    let quote = quote_fixture([guaranteed(decimal("-1.00")), stale_forecast]).unwrap();

    assert_eq!(quote.core_total(), decimal("-1.00"));
    assert_eq!(quote.forecast_total(), decimal("-1.00"));
    assert!(!quote.forecast_complete());
}

#[test]
fn duplicate_component_identity_fails_closed() {
    assert!(matches!(
        quote_fixture([guaranteed(decimal("-1.00")), guaranteed(decimal("-2.00")),]),
        Err(EconomicsUnavailable::DuplicateComponent { .. })
    ));
}

#[test]
fn core_edge_uses_positive_fresh_evidence_basis() {
    let request = canonical_fixture_request();
    let quote = quote_fixture([guaranteed(decimal("-1.00"))]).unwrap();
    let basis = EdgeBasisEvidence {
        policy_id: request.edge_basis_policy_id,
        resolver_id: FormulaId::new("fixture-resolver").unwrap(),
        product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
        policy_version: 1,
        normalized_amount: decimal("10.00"),
        scope: estimated_component(
            "scope",
            decimal("-1"),
            AdmissionTreatment::RiskBound {
                authority: RiskBoundAuthority::VenueMaximum,
            },
            Some(decimal("-1")),
        )
        .scope,
        source_snapshot_ids: Vec::new(),
        valid_until_ns: request.requested_at_ns,
    };
    let edge = fold_net_edge(decimal("3.00"), &quote, basis).unwrap();

    assert_eq!(edge.core_net_edge(), decimal("2.00"));
    assert_eq!(edge.forecast_net_edge(), decimal("2.00"));
    assert_eq!(edge.core_edge_ratio(), decimal("0.20"));
}

#[test]
fn health_is_proportional_and_live_accounting_is_disabled() {
    let health = EconomicsCapabilityHealth::quote_only(110, Some(105));
    assert!(health.allows_admission(100).is_ok());
    assert!(health.forecast_available(100));
    assert!(matches!(
        health.allows_live_execution(100),
        Err(EconomicsUnavailable::ActualAccountingUnavailable)
    ));
    assert!(matches!(
        health.allows_admission(111),
        Err(EconomicsUnavailable::RequiredCapabilityStale { .. })
    ));
    assert!(!health.forecast_available(106));
}

#[test]
fn distinct_native_units_require_explicit_valuation() {
    let component = estimated_component(
        "foreign-unit",
        decimal("-1"),
        AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let mut component = component;
    component.point_estimate = PointEstimate::NonZero(
        bolt_v2::economics::SignedNativeEffect::currency(decimal("-1"), native_unit("USDC"))
            .unwrap(),
    );

    assert!(matches!(
        quote_fixture([component]),
        Err(EconomicsUnavailable::MissingValuation { .. })
    ));
}

#[test]
fn missing_forecast_valuation_degrades_forecast_without_blocking_core() {
    let mut supplemental = forecast(decimal("2.00"));
    supplemental.point_estimate = PointEstimate::NonZero(
        SignedNativeEffect::currency(decimal("2.00"), native_unit("USDC")).unwrap(),
    );

    let quote = quote_fixture([guaranteed(decimal("-1.00")), supplemental]).unwrap();

    assert_eq!(quote.core_total(), decimal("-1.00"));
    assert_eq!(quote.forecast_total(), decimal("-1.00"));
    assert!(!quote.forecast_complete());
}

#[test]
fn component_class_must_match_its_signed_effect() {
    let mut component = guaranteed(decimal("-1.00"));
    component.class = EconomicClass::Credit;

    assert_eq!(
        quote_fixture([component]),
        Err(EconomicsUnavailable::EconomicClassSignMismatch)
    );
}

#[test]
fn required_valuation_expiry_limits_quote_validity() {
    let mut component = guaranteed(decimal("-1.00"));
    component.point_estimate = PointEstimate::NonZero(
        SignedNativeEffect::currency(decimal("-1.00"), native_unit("USDC")).unwrap(),
    );
    component.normalized = Some(ValuationEvidence {
        native_effect: component.point_estimate.effect().unwrap().clone(),
        normalized_amount: decimal("-1.00"),
        reporting_unit: native_unit("pUSD"),
        route_id: Some(ValuationRouteId::new("configured-route").unwrap()),
        source_snapshot_ids: vec![SnapshotId::new("valuation-snapshot").unwrap()],
        valued_at_ns: 100,
        valid_until_ns: Some(105),
    });

    assert_eq!(quote_fixture([component]).unwrap().valid_until_ns(), 105);
}
