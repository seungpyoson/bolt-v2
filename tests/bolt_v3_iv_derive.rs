use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    derive::{
        IvDeriveError, IvDerivedInputBounds, IvDerivedInputField, IvDerivedInputFieldPolicy,
        IvDerivedInputPolicy, IvDerivedInputSet, IvDerivedInputSourceKind,
        IvDerivedProfileSourceRef, IvHelperFailurePolicy, IvHelperOutput, IvHelperPolicy,
        IvNtHelperSymbol, IvOperatorValueRefreshPolicy, IvOptionSide, IvTimedInput, derive_iv,
        resolve_derived_input_policy, select_helper_policy,
    },
    error::IvRejectReason,
    health::IvSourceHealthState,
    provenance::{IvPolicyDecision, validate_iv_provenance},
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
        allowed_outputs: BTreeSet::from([IvHelperOutput::IvAndGreeks]),
        input_policy_ref: "configured-derived-input-policy".to_string(),
        output_bounds: bounds(2.0),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        failure_policy: IvHelperFailurePolicy::RejectInvalidHelperOutput,
        max_input_timestamp_skew_ns: 20,
        max_operator_input_age_ns: 100,
    }
}

fn input_bounds() -> IvDerivedInputBounds {
    IvDerivedInputBounds {
        option_price: Some(input_bound(true, 0.0, 10_000.0, IvBoundUnit::Price)),
        underlying_price: Some(input_bound(true, 0.0, 10_000.0, IvBoundUnit::Price)),
        strike: Some(input_bound(true, 0.0, 10_000.0, IvBoundUnit::Strike)),
        time_to_expiry_years: Some(input_bound(true, 0.0, 100.0, IvBoundUnit::TimeToExpiry)),
        rate: Some(input_bound(false, -1.0, 1.0, IvBoundUnit::Rate)),
        carry: Some(input_bound(false, -1.0, 1.0, IvBoundUnit::Carry)),
    }
}

fn input_bound(
    positive_required: bool,
    inclusive_min: f64,
    inclusive_max: f64,
    unit: IvBoundUnit,
) -> IvNumericBounds {
    IvNumericBounds {
        finite_required: true,
        positive_required,
        inclusive_min: Some(inclusive_min),
        inclusive_max: Some(inclusive_max),
        exclusive_min: None,
        exclusive_max: None,
        unit,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: BTreeSet::new(),
        },
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
    assert_eq!(
        output.provenance.policy_decisions,
        vec![IvPolicyDecision::HelperDecision {
            helper_policy_id: "configured-helper-policy".to_string(),
            helper_identity: output.helper_identity.clone(),
            helper_symbol: "nautilus_model::data::imply_vol_and_greeks".to_string(),
            input_set_id: "configured-profile:configured-source:configured-option-instrument:1000"
                .to_string(),
            input_event_ids: vec!["configured-input-event".to_string()],
            output_validated: true,
            rejection_reason: None,
        }]
    );
    validate_iv_provenance(&output.provenance).unwrap();
}

#[test]
fn derived_input_policy_resolves_profile_source_and_operator_fields_before_helper_call() {
    let mut request_inputs = complete_inputs();
    request_inputs.underlying_price = None;
    request_inputs.rate = None;
    request_inputs.carry = None;

    let mut profile_source_inputs = complete_inputs();
    profile_source_inputs.source_id = "configured-underlying-source".to_string();
    profile_source_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    profile_source_inputs.underlying_price = Some(IvTimedInput {
        value: 100.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });
    profile_source_inputs.input_event_ids = vec!["configured-underlying-event".to_string()];

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: IvDerivedInputField::required_fields().to_vec(),
        field_sources: vec![
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::OptionPrice,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::UnderlyingPrice,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::ProfileSourceRef]),
                profile_source_ref: Some(IvDerivedProfileSourceRef {
                    source_id: "configured-underlying-source".to_string(),
                    selector_fingerprint: "configured-underlying-selector".to_string(),
                }),
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Strike,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::OptionSide,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::TimeToExpiryYears,
                allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::QuerySupplied]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Rate,
                allowed_source_kinds: BTreeSet::from([
                    IvDerivedInputSourceKind::OperatorConfigured,
                ]),
                profile_source_ref: None,
                operator_number: Some(operator(0.01, 994, 1_050)),
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Carry,
                allowed_source_kinds: BTreeSet::from([
                    IvDerivedInputSourceKind::OperatorConfigured,
                ]),
                profile_source_ref: None,
                operator_number: Some(operator(0.0, 993, 1_050)),
                operator_side: None,
            },
        ],
        freshness_ns: 100,
        max_input_skew_ns: 20,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    let resolved =
        resolve_derived_input_policy(&policy, request_inputs, &[profile_source_inputs]).unwrap();
    let output = derive_iv(&helper_policy(), resolved.clone()).unwrap();

    assert_eq!(
        resolved
            .underlying_price
            .expect("underlying must be policy-resolved")
            .source_kind,
        IvDerivedInputSourceKind::ProfileSourceRef
    );
    assert_eq!(
        resolved
            .rate
            .expect("rate must be operator-resolved")
            .source_kind,
        IvDerivedInputSourceKind::OperatorConfigured
    );
    assert!(
        output
            .provenance
            .input_event_ids
            .contains(&"configured-underlying-event".to_string())
    );
}

#[test]
fn derived_input_policy_resolves_fresh_profile_source_from_older_snapshot() {
    let mut request_inputs = complete_inputs();
    request_inputs.underlying_price = None;

    let mut profile_source_inputs = complete_inputs();
    profile_source_inputs.source_id = "configured-underlying-source".to_string();
    profile_source_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    profile_source_inputs.as_of_ns = UnixNanos::new(997);
    profile_source_inputs.underlying_price = Some(IvTimedInput {
        value: 100.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });
    profile_source_inputs.input_event_ids = vec!["configured-underlying-event".to_string()];

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: vec![IvDerivedInputField::UnderlyingPrice],
        field_sources: vec![IvDerivedInputFieldPolicy {
            field: IvDerivedInputField::UnderlyingPrice,
            allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::ProfileSourceRef]),
            profile_source_ref: Some(IvDerivedProfileSourceRef {
                source_id: "configured-underlying-source".to_string(),
                selector_fingerprint: "configured-underlying-selector".to_string(),
            }),
            operator_number: None,
            operator_side: None,
        }],
        freshness_ns: 10,
        max_input_skew_ns: 10,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    let resolved =
        resolve_derived_input_policy(&policy, request_inputs, &[profile_source_inputs]).unwrap();

    assert_eq!(
        resolved.underlying_price.unwrap().ts_ns,
        UnixNanos::new(996)
    );
    assert!(
        resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-underlying-event")
    );
}

#[test]
fn derived_input_policy_uses_latest_fresh_profile_source_snapshot() {
    let mut request_inputs = complete_inputs();
    request_inputs.underlying_price = None;

    let mut stale_profile_source_inputs = complete_inputs();
    stale_profile_source_inputs.source_id = "configured-underlying-source".to_string();
    stale_profile_source_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    stale_profile_source_inputs.as_of_ns = UnixNanos::new(970);
    stale_profile_source_inputs.underlying_price = Some(IvTimedInput {
        value: 90.0,
        ts_ns: UnixNanos::new(970),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });
    stale_profile_source_inputs.input_event_ids = vec!["configured-stale-event".to_string()];

    let mut fresh_profile_source_inputs = complete_inputs();
    fresh_profile_source_inputs.source_id = "configured-underlying-source".to_string();
    fresh_profile_source_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    fresh_profile_source_inputs.as_of_ns = UnixNanos::new(997);
    fresh_profile_source_inputs.underlying_price = Some(IvTimedInput {
        value: 100.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });
    fresh_profile_source_inputs.input_event_ids = vec!["configured-fresh-event".to_string()];

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: vec![IvDerivedInputField::UnderlyingPrice],
        field_sources: vec![IvDerivedInputFieldPolicy {
            field: IvDerivedInputField::UnderlyingPrice,
            allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::ProfileSourceRef]),
            profile_source_ref: Some(IvDerivedProfileSourceRef {
                source_id: "configured-underlying-source".to_string(),
                selector_fingerprint: "configured-underlying-selector".to_string(),
            }),
            operator_number: None,
            operator_side: None,
        }],
        freshness_ns: 10,
        max_input_skew_ns: 10,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    let resolved = resolve_derived_input_policy(
        &policy,
        request_inputs,
        &[stale_profile_source_inputs, fresh_profile_source_inputs],
    )
    .unwrap();

    assert_eq!(resolved.underlying_price.unwrap().value, 100.0);
    assert!(
        resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-fresh-event")
    );
    assert!(
        !resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-stale-event")
    );
}

#[test]
fn profile_source_resolution_skips_mismatched_instrument_candidates() {
    let mut request_inputs = complete_inputs();
    request_inputs.underlying_price = None;

    let mut wrong_instrument_inputs = complete_inputs();
    wrong_instrument_inputs.source_id = "configured-underlying-source".to_string();
    wrong_instrument_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    wrong_instrument_inputs.instrument_id = "configured-other-option-instrument".to_string();
    wrong_instrument_inputs.underlying_price = Some(IvTimedInput {
        value: 125.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });
    wrong_instrument_inputs.input_event_ids = vec!["configured-wrong-instrument-event".to_string()];

    let mut matching_inputs = complete_inputs();
    matching_inputs.source_id = "configured-underlying-source".to_string();
    matching_inputs.selector_fingerprint = "configured-underlying-selector".to_string();
    matching_inputs.underlying_price = Some(IvTimedInput {
        value: 100.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::ProfileSourceRef,
        expires_at_ns: None,
    });
    matching_inputs.input_event_ids = vec!["configured-matching-instrument-event".to_string()];

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: vec![IvDerivedInputField::UnderlyingPrice],
        field_sources: vec![IvDerivedInputFieldPolicy {
            field: IvDerivedInputField::UnderlyingPrice,
            allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::ProfileSourceRef]),
            profile_source_ref: Some(IvDerivedProfileSourceRef {
                source_id: "configured-underlying-source".to_string(),
                selector_fingerprint: "configured-underlying-selector".to_string(),
            }),
            operator_number: None,
            operator_side: None,
        }],
        freshness_ns: 100,
        max_input_skew_ns: 10,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    let resolved = resolve_derived_input_policy(
        &policy,
        request_inputs,
        &[wrong_instrument_inputs, matching_inputs],
    )
    .unwrap();

    assert_eq!(resolved.underlying_price.unwrap().value, 100.0);
    assert!(
        resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-matching-instrument-event")
    );
    assert!(
        !resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-wrong-instrument-event")
    );
}

#[test]
fn derived_input_policy_resolves_instrument_metadata_fields() {
    let mut request_inputs = complete_inputs();
    request_inputs.strike = None;
    request_inputs.option_side = None;

    let mut wrong_instrument_metadata = complete_inputs();
    wrong_instrument_metadata.source_id = "configured-instrument-metadata-source".to_string();
    wrong_instrument_metadata.selector_fingerprint =
        "configured-instrument-metadata-selector".to_string();
    wrong_instrument_metadata.instrument_id = "configured-other-option-instrument".to_string();
    wrong_instrument_metadata.strike = Some(IvTimedInput {
        value: 125.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::InstrumentMetadata,
        expires_at_ns: None,
    });
    wrong_instrument_metadata.option_side = Some(IvTimedInput {
        value: IvOptionSide::Put,
        ts_ns: UnixNanos::new(997),
        source_kind: IvDerivedInputSourceKind::InstrumentMetadata,
        expires_at_ns: None,
    });
    wrong_instrument_metadata.input_event_ids =
        vec!["configured-wrong-instrument-metadata-event".to_string()];

    let mut matching_instrument_metadata = complete_inputs();
    matching_instrument_metadata.source_id = "configured-instrument-metadata-source".to_string();
    matching_instrument_metadata.selector_fingerprint =
        "configured-instrument-metadata-selector".to_string();
    matching_instrument_metadata.as_of_ns = UnixNanos::new(998);
    matching_instrument_metadata.strike = Some(IvTimedInput {
        value: 100.0,
        ts_ns: UnixNanos::new(996),
        source_kind: IvDerivedInputSourceKind::InstrumentMetadata,
        expires_at_ns: None,
    });
    matching_instrument_metadata.option_side = Some(IvTimedInput {
        value: IvOptionSide::Call,
        ts_ns: UnixNanos::new(997),
        source_kind: IvDerivedInputSourceKind::InstrumentMetadata,
        expires_at_ns: None,
    });
    matching_instrument_metadata.input_event_ids =
        vec!["configured-instrument-metadata-event".to_string()];

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: vec![IvDerivedInputField::Strike, IvDerivedInputField::OptionSide],
        field_sources: vec![
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::Strike,
                allowed_source_kinds: BTreeSet::from([
                    IvDerivedInputSourceKind::InstrumentMetadata,
                ]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
            IvDerivedInputFieldPolicy {
                field: IvDerivedInputField::OptionSide,
                allowed_source_kinds: BTreeSet::from([
                    IvDerivedInputSourceKind::InstrumentMetadata,
                ]),
                profile_source_ref: None,
                operator_number: None,
                operator_side: None,
            },
        ],
        freshness_ns: 10,
        max_input_skew_ns: 10,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    let resolved = resolve_derived_input_policy(
        &policy,
        request_inputs,
        &[wrong_instrument_metadata, matching_instrument_metadata],
    )
    .unwrap();

    assert_eq!(resolved.strike.unwrap().value, 100.0);
    assert_eq!(
        resolved.strike.unwrap().source_kind,
        IvDerivedInputSourceKind::InstrumentMetadata
    );
    assert_eq!(resolved.option_side.unwrap().value, IvOptionSide::Call);
    assert!(
        resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-instrument-metadata-event")
    );
    assert!(
        !resolved
            .input_event_ids
            .iter()
            .any(|event_id| event_id == "configured-wrong-instrument-metadata-event")
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
fn expired_operator_option_side_rejects_before_helper_invocation() {
    let mut inputs = complete_inputs();
    inputs.option_side = Some(IvTimedInput {
        value: IvOptionSide::Call,
        ts_ns: UnixNanos::new(998),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(999)),
    });

    assert_eq!(
        derive_iv(&helper_policy(), inputs),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::OperatorInputExpired,
            field: IvDerivedInputField::OptionSide.as_str().to_string(),
        })
    );
}

#[test]
fn derived_input_policy_rejects_expired_operator_option_side() {
    let mut request_inputs = complete_inputs();
    request_inputs.option_side = None;

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: vec![IvDerivedInputField::OptionSide],
        field_sources: vec![IvDerivedInputFieldPolicy {
            field: IvDerivedInputField::OptionSide,
            allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::OperatorConfigured]),
            profile_source_ref: None,
            operator_number: None,
            operator_side: Some(IvTimedInput {
                value: IvOptionSide::Call,
                ts_ns: UnixNanos::new(998),
                source_kind: IvDerivedInputSourceKind::OperatorConfigured,
                expires_at_ns: Some(UnixNanos::new(999)),
            }),
        }],
        freshness_ns: 100,
        max_input_skew_ns: 20,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    assert_eq!(
        resolve_derived_input_policy(&policy, request_inputs, &[]),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::OperatorInputExpired,
            field: IvDerivedInputField::OptionSide.as_str().to_string(),
        })
    );
}

#[test]
fn operator_rate_or_carry_without_expiration_derives_when_within_age() {
    for field in [IvDerivedInputField::Rate, IvDerivedInputField::Carry] {
        let mut inputs = complete_inputs();
        match field {
            IvDerivedInputField::Rate => {
                inputs.rate.as_mut().unwrap().expires_at_ns = None;
            }
            IvDerivedInputField::Carry => {
                inputs.carry.as_mut().unwrap().expires_at_ns = None;
            }
            _ => unreachable!(),
        }

        assert!(derive_iv(&helper_policy(), inputs).is_ok());
    }
}

#[test]
fn operator_rate_or_carry_uses_operator_age_not_market_timestamp_skew() {
    for field in [IvDerivedInputField::Rate, IvDerivedInputField::Carry] {
        let mut inputs = complete_inputs();
        match field {
            IvDerivedInputField::Rate => {
                inputs.rate = Some(operator(0.01, 900, 1_050));
            }
            IvDerivedInputField::Carry => {
                inputs.carry = Some(operator(0.0, 900, 1_050));
            }
            _ => unreachable!(),
        }

        assert!(derive_iv(&helper_policy(), inputs).is_ok());
    }
}

#[test]
fn derived_input_policy_allows_unexpired_operator_input_outside_market_skew() {
    let mut request_inputs = complete_inputs();
    request_inputs.rate = None;

    let policy = IvDerivedInputPolicy {
        input_policy_id: "configured-derived-input-policy".to_string(),
        helper_policy_ref: "configured-helper-policy".to_string(),
        required_fields: vec![IvDerivedInputField::Rate],
        field_sources: vec![IvDerivedInputFieldPolicy {
            field: IvDerivedInputField::Rate,
            allowed_source_kinds: BTreeSet::from([IvDerivedInputSourceKind::OperatorConfigured]),
            profile_source_ref: None,
            operator_number: Some(operator(0.01, 900, 1_050)),
            operator_side: None,
        }],
        freshness_ns: 20,
        max_input_skew_ns: 20,
        bounds: input_bounds(),
        convention_policy: IvConventionBounds {
            allowed_conventions: BTreeSet::from([convention()]),
        },
        operator_value_refresh_policy: IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues,
    };

    let resolved = resolve_derived_input_policy(&policy, request_inputs, &[]).unwrap();

    assert_eq!(resolved.rate.unwrap().ts_ns, UnixNanos::new(900));
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

#[test]
fn helper_floor_output_rejects_as_invalid_iv() {
    let mut inputs = complete_inputs();
    inputs.option_price.as_mut().unwrap().value = 1.0e-12;

    assert_eq!(
        derive_iv(&helper_policy(), inputs),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidIvValue,
            field: "iv".to_string(),
        })
    );
}

#[test]
fn derived_output_with_incomplete_provenance_rejects_before_return() {
    let mut inputs = complete_inputs();
    inputs.input_event_ids.clear();

    assert_eq!(
        derive_iv(&helper_policy(), inputs),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ProvenanceIncomplete,
            field: "provenance".to_string(),
        })
    );
}
