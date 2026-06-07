use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    derive::{
        IvDeriveError, IvDerivedInputField, IvDerivedInputSet, IvDerivedInputSourceKind,
        IvHelperPolicy, IvNtHelperSymbol, IvOptionSide, IvTimedInput, derive_iv,
        select_helper_policy,
    },
    error::IvRejectReason,
    health::IvSourceHealthState,
    provenance::IvPolicyDecision,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

fn convention() -> IvConvention {
    IvConvention::Named("configured-convention".to_string())
}

fn bounds(inclusive_max: f64) -> IvNumericBounds {
    IvNumericBounds {
        finite_required: true,
        positive_required: true,
        inclusive_min: Some(0.0),
        inclusive_max: Some(inclusive_max),
        exclusive_min: None,
        exclusive_max: None,
        unit: IvBoundUnit::Unitless,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
    }
}

fn helper_policy() -> IvHelperPolicy {
    IvHelperPolicy {
        helper_policy_id: "configured-helper-policy".to_string(),
        nt_helper_symbol: IvNtHelperSymbol::ImplyVolAndGreeks,
        parameter_signature: "s,r,b,is_call,k,t,price".to_string(),
        output_bounds: bounds(2.0),
        max_input_timestamp_skew_ns: 20,
        max_operator_input_age_ns: 100,
    }
}

fn timed(value: f64, ts: u64) -> IvTimedInput<f64> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts),
        source_kind: IvDerivedInputSourceKind::QuerySupplied,
        expires_at_ns: None,
    }
}

fn operator(value: f64, ts: u64, expires_at: u64) -> IvTimedInput<f64> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(expires_at)),
    }
}

fn complete_inputs() -> IvDerivedInputSet {
    let option_price =
        nautilus_model::data::black_scholes_greeks(100.0, 0.01, 0.0, 0.25, true, 100.0, 0.5).price;

    IvDerivedInputSet {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        instrument_id: "configured-option-instrument".to_string(),
        basis: IvBasis::Mark,
        convention: convention(),
        as_of_ns: UnixNanos::new(1_000),
        received_ts_ns: UnixNanos::new(1_005),
        subscription_generation: 3,
        source_health_state: IvSourceHealthState::Active,
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "crates/model/src/data/greeks.rs".to_string(),
        input_event_ids: vec!["configured-input-event".to_string()],
        option_price: Some(timed(option_price, 995)),
        underlying_price: Some(timed(100.0, 996)),
        strike: Some(timed(100.0, 997)),
        option_side: Some(IvTimedInput {
            value: IvOptionSide::Call,
            ts_ns: UnixNanos::new(998),
            source_kind: IvDerivedInputSourceKind::QuerySupplied,
            expires_at_ns: None,
        }),
        time_to_expiry_years: Some(timed(0.5, 999)),
        rate: Some(operator(0.01, 994, 1_050)),
        carry: Some(operator(0.0, 993, 1_050)),
    }
}

#[test]
fn helper_policy_selection_uses_configured_nt_symbol() {
    let policies = [
        IvHelperPolicy {
            helper_policy_id: "other-policy".to_string(),
            ..helper_policy()
        },
        helper_policy(),
    ];
    let selected = select_helper_policy(&policies, "configured-helper-policy").unwrap();

    assert_eq!(
        selected.nt_helper_symbol,
        IvNtHelperSymbol::ImplyVolAndGreeks
    );
    assert_eq!(
        selected.nt_helper_symbol.nt_symbol(),
        "nautilus_model::data::imply_vol_and_greeks"
    );
}

#[test]
fn complete_inputs_derive_iv_with_nt_helper_and_helper_provenance() {
    let inputs = complete_inputs();
    let expected = nautilus_model::data::imply_vol_and_greeks(
        100.0,
        0.01,
        0.0,
        true,
        100.0,
        0.5,
        inputs.option_price.as_ref().unwrap().value,
    );

    let output = derive_iv(&helper_policy(), inputs).unwrap();

    assert!((output.point.iv - expected.vol).abs() < 0.001);
    assert!((output.greeks.vega.unwrap() - expected.vega).abs() < 0.001);
    assert_eq!(
        output.helper_identity.nt_symbol,
        "nautilus_model::data::imply_vol_and_greeks"
    );
    assert_eq!(
        output.provenance.helper_identity.as_ref(),
        Some(&output.helper_identity)
    );
    assert!(
        output
            .provenance
            .policy_decisions
            .contains(&IvPolicyDecision::Helper)
    );
}

#[test]
fn missing_required_inputs_reject_with_field_specific_reason() {
    for field in IvDerivedInputField::required_fields() {
        let mut inputs = complete_inputs();
        inputs.clear_field(field);

        assert_eq!(
            derive_iv(&helper_policy(), inputs),
            Err(IvDeriveError::MissingInput { field })
        );
    }
}

#[test]
fn stale_or_skewed_inputs_reject_before_helper_invocation() {
    let mut inputs = complete_inputs();
    inputs.underlying_price.as_mut().unwrap().ts_ns = UnixNanos::new(100);

    assert_eq!(
        derive_iv(&helper_policy(), inputs),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ClockSkew,
            field: "input_timestamp_skew".to_string(),
        })
    );
}

#[test]
fn expired_operator_rate_or_carry_inputs_reject_before_helper_invocation() {
    for field in [IvDerivedInputField::Rate, IvDerivedInputField::Carry] {
        let mut inputs = complete_inputs();
        match field {
            IvDerivedInputField::Rate => {
                inputs.rate = Some(operator(0.01, 994, 999));
            }
            IvDerivedInputField::Carry => {
                inputs.carry = Some(operator(0.0, 993, 999));
            }
            _ => unreachable!(),
        }

        assert_eq!(
            derive_iv(&helper_policy(), inputs),
            Err(IvDeriveError::Rejected {
                reason: IvRejectReason::OperatorInputExpired,
                field: field.as_str().to_string(),
            })
        );
    }
}

#[test]
fn helper_output_outside_configured_bounds_rejects() {
    let policy = IvHelperPolicy {
        output_bounds: bounds(0.10),
        ..helper_policy()
    };

    assert_eq!(
        derive_iv(&policy, complete_inputs()),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidIvValue,
            field: "iv".to_string(),
        })
    );
}
