use std::str::FromStr;

use bolt_v2::economics::{
    ActualEconomicEntry, AdmissionTreatment, EconomicQuoteRequest, EstimatedEconomicComponent,
    PlannedFillNotional, RiskBoundAuthority, SignedNativeEffect, VenueEconomicsAdapter,
    VenueQuoteEstimate, currency_from_code,
};
use rust_decimal::Decimal;

fn decimal(value: &str) -> Decimal {
    Decimal::from_str(value).expect("fixture decimal must parse")
}

#[test]
fn signed_currency_effect_preserves_sign_and_native_unit() {
    let effect = SignedNativeEffect::currency(
        decimal("-1.25"),
        currency_from_code("pUSD").expect("native unit must be valid"),
    )
    .expect("non-zero signed effect must be valid");

    assert_eq!(effect.amount(), decimal("-1.25"));
    assert_eq!(effect.currency_id().code.as_str(), "pUSD");
}

#[test]
fn forecast_treatment_cannot_authorize_admission() {
    assert!(!AdmissionTreatment::ForecastOnly.authorizes_admission());
    assert!(AdmissionTreatment::GuaranteedConditionalOnAction.authorizes_admission());
    assert!(
        AdmissionTreatment::RiskBound {
            authority: RiskBoundAuthority::VenueMaximum,
        }
        .authorizes_admission()
    );
}

#[test]
fn invalid_native_units_and_zero_effects_fail_closed() {
    assert!(currency_from_code("").is_err());
    assert!(currency_from_code(" pUSD").is_err());
    assert!(currency_from_code("pUSD\n").is_err());
    assert!(
        SignedNativeEffect::currency(
            Decimal::ZERO,
            currency_from_code("pUSD").expect("native unit must be valid")
        )
        .is_err()
    );
}

#[allow(dead_code)]
fn estimate_and_actual_are_distinct_types(
    actual: ActualEconomicEntry,
    estimate: EstimatedEconomicComponent,
) {
    let _ = (actual, estimate);
}

#[allow(dead_code)]
fn adapter_contract_uses_only_shared_types<T: VenueEconomicsAdapter>(
    adapter: &T,
    request: &EconomicQuoteRequest,
) {
    let planned_fill_notional = PlannedFillNotional::from_legs(&request.planned_fill_legs)
        .expect("contract request must have valid planned legs");
    let _: Result<VenueQuoteEstimate, _> = adapter.quote(request, planned_fill_notional);
}
