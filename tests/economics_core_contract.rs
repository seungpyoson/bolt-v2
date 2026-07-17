use std::str::FromStr;

use bolt_v2::economics::{
    AdmissionTreatment, NativeUnitId, SignedNativeEffect,
};
use rust_decimal::Decimal;

fn decimal(value: &str) -> Decimal {
    Decimal::from_str(value).expect("fixture decimal must parse")
}

#[test]
fn signed_currency_effect_preserves_sign_and_native_unit() {
    let effect = SignedNativeEffect::currency(
        decimal("-1.25"),
        NativeUnitId::new("pUSD").expect("native unit must be valid"),
    )
    .expect("non-zero signed effect must be valid");

    assert_eq!(effect.amount(), decimal("-1.25"));
    assert_eq!(effect.unit().as_str(), "pUSD");
}

#[test]
fn forecast_treatment_cannot_authorize_admission() {
    assert!(!AdmissionTreatment::ForecastOnly.authorizes_admission());
    assert!(AdmissionTreatment::GuaranteedConditionalOnAction.authorizes_admission());
}
