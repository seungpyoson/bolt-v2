use bolt_v2::bolt_v3_iv::{
    authz::IvAuthorizationMode,
    config::{IvConfigError, IvRootConfig, load_iv_config_from_toml, validate_iv_root_config},
    derive::{
        IvDeriveError, IvDerivedInputField, IvDerivedInputSet, IvDerivedInputSourceKind,
        IvOptionSide, IvTimedInput, resolve_derived_input_policy,
    },
    error::IvRejectReason,
    health::IvSourceHealthState,
    selector::IvSelector,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvProductKind, IvSourceKind},
};

fn valid_iv_toml() -> &'static str {
    r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
strategy_ids = ["configured-strategy"]
enabled_products = ["iv_point", "smile", "surface", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
derived_inputs = []

[profiles.audit_policy]
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10
fallback_policy_ref = "configured-fallback-policy"
interpolation_policy_ref = "configured-interpolation-policy"
quorum_policy_ref = "configured-quorum-policy"

[[profiles.interpolation_policies]]
policy_id = "configured-interpolation-policy"
method = "linear"
strike_axis = "strike"
tenor_axis = "expiry"
minimum_points = 2
eligible_sources = ["configured-greeks-source"]
extrapolation = "reject"

[[profiles.fallback_policies]]
policy_id = "configured-fallback-policy"
candidate_order = ["configured-primary-candidate", "configured-backup-candidate"]
eligible_sources = ["configured-greeks-source"]
maximum_timestamp_skew_ns = 10
required_provenance_fields = ["raw_event_id"]

[[profiles.quorum_policies]]
policy_id = "configured-quorum-policy"
minimum_sources = 2
eligible_sources = ["configured-greeks-source", "configured-backup-source"]
agreement_band = 0.05
tie_break = "mean"

[[profiles.helper_policies]]
helper_policy_id = "configured-helper-policy"
nt_helper_symbol = "imply_vol_and_greeks"
parameter_signature = "configured-helper-signature"
allowed_outputs = ["iv_and_greeks"]
input_policy_ref = "configured-derived-input-policy"
failure_policy = "reject_invalid_helper_output"
max_input_timestamp_skew_ns = 10
max_operator_input_age_ns = 100

[profiles.helper_policies.convention_policy]
allowed_conventions = ["configured-convention"]

[profiles.helper_policies.output_bounds]
finite_required = true
positive_required = true
inclusive_min = 0.0
inclusive_max = 5.0
exclusive_min = 0.0
exclusive_max = 6.0
unit = "unitless"

[profiles.helper_policies.output_bounds.allowed_conventions]
allowed_conventions = []

[[profiles.derived_input_policies]]
input_policy_id = "configured-derived-input-policy"
helper_policy_ref = "configured-helper-policy"
required_fields = ["option_price", "underlying_price", "strike", "option_side", "time_to_expiry_years", "rate", "carry"]
freshness_ns = 100
max_input_skew_ns = 10
operator_value_refresh_policy = "reject_expired_operator_values"

[profiles.derived_input_policies.bounds]
option_price = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 10000.0, unit = "price", allowed_conventions = { allowed_conventions = [] } }
underlying_price = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 99.0, unit = "price", allowed_conventions = { allowed_conventions = [] } }
strike = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 10000.0, unit = "strike", allowed_conventions = { allowed_conventions = [] } }
time_to_expiry_years = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 100.0, unit = "time_to_expiry", allowed_conventions = { allowed_conventions = [] } }
rate = { finite_required = true, positive_required = false, inclusive_min = -1.0, inclusive_max = 1.0, unit = "rate", allowed_conventions = { allowed_conventions = [] } }
carry = { finite_required = true, positive_required = false, inclusive_min = -1.0, inclusive_max = 1.0, unit = "carry", allowed_conventions = { allowed_conventions = [] } }

[profiles.derived_input_policies.convention_policy]
allowed_conventions = ["configured-convention"]

[[profiles.derived_input_policies.field_sources]]
field = "option_price"
allowed_source_kinds = ["query_supplied"]

[[profiles.derived_input_policies.field_sources]]
field = "underlying_price"
allowed_source_kinds = ["query_supplied", "profile_source_ref"]

[[profiles.derived_input_policies.field_sources]]
field = "strike"
allowed_source_kinds = ["query_supplied", "instrument_metadata"]

[[profiles.derived_input_policies.field_sources]]
field = "option_side"
allowed_source_kinds = ["query_supplied", "instrument_metadata"]

[[profiles.derived_input_policies.field_sources]]
field = "time_to_expiry_years"
allowed_source_kinds = ["query_supplied", "instrument_metadata"]

[[profiles.derived_input_policies.field_sources]]
field = "rate"
allowed_source_kinds = ["operator_configured"]

[[profiles.derived_input_policies.field_sources]]
field = "carry"
allowed_source_kinds = ["operator_configured"]

[profiles.selector_authorization]
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"

[[profiles.sources]]
source_id = "configured-chain-source"
selector_fingerprint = "configured-chain-selector"
source_kind = "option_chain"
client_id = "configured-client"
subscription_generation = 8
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionChain"

[profiles.sources.selector]
selector_kind = "source_option_chain"
series_ids = ["configured-series"]
strike_range_policy = "configured-strike-range-policy"

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#
}

#[test]
fn full_profile_toml_maps_to_typed_iv_config_without_defaults() {
    let config = load_iv_config_from_toml(valid_iv_toml()).unwrap();

    assert_eq!(config.schema_version, 1);
    assert_eq!(config.profiles.len(), 1);
    assert_eq!(config.profiles[0].profile_id, "configured-profile");
    assert!(
        config.profiles[0]
            .strategy_ids
            .contains("configured-strategy")
    );
    assert_eq!(
        config.profiles[0].selector_authorization.authorization_mode,
        IvAuthorizationMode::ProfileWide
    );
    assert_eq!(config.profiles[0].max_derived_points, 2);
    assert_eq!(
        config.profiles[0]
            .audit_policy
            .authorized_audit_handles
            .len(),
        1
    );
    assert_eq!(
        config.profiles[0].projection_policies[0].policy_id,
        "configured-projection-policy"
    );
    assert_eq!(
        config.profiles[0].interpolation_policies[0].policy_id,
        "configured-interpolation-policy"
    );
    assert_eq!(
        config.profiles[0].fallback_policies[0].policy_id,
        "configured-fallback-policy"
    );
    assert_eq!(
        config.profiles[0].quorum_policies[0].policy_id,
        "configured-quorum-policy"
    );
    assert_eq!(
        config.profiles[0].helper_policies[0].helper_policy_id,
        "configured-helper-policy"
    );
    assert_eq!(
        config.profiles[0].derived_input_policies[0].input_policy_id,
        "configured-derived-input-policy"
    );
    assert!(config.profiles[0].derived_inputs.is_empty());
    assert_eq!(config.profiles[0].sources[0].subscription_generation, 7);
    assert_eq!(
        config.profiles[0].sources[0].nt_provenance.nt_revision,
        "configured-nt-revision"
    );
    assert_eq!(
        config.profiles[0].sources[0].nt_provenance.nt_symbol,
        "ConfiguredOptionGreeks"
    );
    assert_eq!(config.profiles[0].sources.len(), 2);
    assert_eq!(
        config.profiles[0].sources[0].source_kind,
        IvSourceKind::OptionGreeks
    );
    assert!(matches!(
        config.profiles[0].sources[0].selector,
        IvSelector::SourceOptionGreeks { .. }
    ));
}

fn derived_config_inputs() -> IvDerivedInputSet {
    IvDerivedInputSet {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-greeks-selector".to_string(),
        instrument_id: "configured-option-instrument".to_string(),
        basis: IvBasis::Mark,
        convention: IvConvention::Named("configured-convention".to_string()),
        as_of_ns: UnixNanos::new(1_000),
        received_ts_ns: UnixNanos::new(1_001),
        subscription_generation: 7,
        source_health_state: IvSourceHealthState::Active,
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        input_event_ids: vec!["configured-input-event".to_string()],
        option_price: Some(timed_number(
            1.0,
            995,
            IvDerivedInputSourceKind::QuerySupplied,
        )),
        underlying_price: Some(timed_number(
            100.0,
            996,
            IvDerivedInputSourceKind::QuerySupplied,
        )),
        strike: Some(timed_number(
            100.0,
            997,
            IvDerivedInputSourceKind::QuerySupplied,
        )),
        option_side: Some(IvTimedInput {
            value: IvOptionSide::Call,
            ts_ns: UnixNanos::new(998),
            source_kind: IvDerivedInputSourceKind::QuerySupplied,
            expires_at_ns: None,
        }),
        time_to_expiry_years: Some(timed_number(
            0.5,
            999,
            IvDerivedInputSourceKind::QuerySupplied,
        )),
        rate: Some(timed_number(
            0.01,
            994,
            IvDerivedInputSourceKind::OperatorConfigured,
        )),
        carry: Some(timed_number(
            0.0,
            993,
            IvDerivedInputSourceKind::OperatorConfigured,
        )),
    }
}

fn timed_number(
    value: f64,
    ts_ns: u64,
    source_kind: IvDerivedInputSourceKind,
) -> IvTimedInput<f64> {
    IvTimedInput {
        value,
        ts_ns: UnixNanos::new(ts_ns),
        source_kind,
        expires_at_ns: None,
    }
}

#[test]
fn structured_derived_input_bounds_reject_out_of_bounds_resolved_inputs() {
    let config = load_iv_config_from_toml(valid_iv_toml()).unwrap();
    let policy = &config.profiles[0].derived_input_policies[0];

    assert_eq!(
        resolve_derived_input_policy(policy, derived_config_inputs(), &[]),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: IvDerivedInputField::UnderlyingPrice.as_str().to_string(),
        })
    );
}

#[test]
fn structured_derived_input_convention_policy_rejects_disallowed_conventions() {
    let config = load_iv_config_from_toml(valid_iv_toml()).unwrap();
    let policy = &config.profiles[0].derived_input_policies[0];
    let mut inputs = derived_config_inputs();
    inputs.underlying_price = Some(timed_number(
        98.0,
        996,
        IvDerivedInputSourceKind::QuerySupplied,
    ));
    inputs.convention = IvConvention::Named("configured-other-convention".to_string());

    assert_eq!(
        resolve_derived_input_policy(policy, inputs, &[]),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: "convention_policy".to_string(),
        })
    );
}

#[test]
fn operator_value_refresh_policy_rejects_expired_operator_inputs() {
    let config = load_iv_config_from_toml(valid_iv_toml()).unwrap();
    let policy = &config.profiles[0].derived_input_policies[0];
    let mut inputs = derived_config_inputs();
    inputs.underlying_price = Some(timed_number(
        98.0,
        996,
        IvDerivedInputSourceKind::QuerySupplied,
    ));
    inputs.rate = Some(IvTimedInput {
        value: 0.01,
        ts_ns: UnixNanos::new(994),
        source_kind: IvDerivedInputSourceKind::OperatorConfigured,
        expires_at_ns: Some(UnixNanos::new(999)),
    });

    assert_eq!(
        resolve_derived_input_policy(policy, inputs, &[]),
        Err(IvDeriveError::Rejected {
            reason: IvRejectReason::OperatorInputExpired,
            field: IvDerivedInputField::Rate.as_str().to_string(),
        })
    );
}

#[test]
fn profile_selector_authorization_expands_to_effective_strategy_authorizations() {
    let scoped = valid_iv_toml()
        .replace(
            "authorization_mode = \"profile_wide\"",
            "authorization_mode = \"selector_scoped\"",
        )
        .replace(
            "allowed_selector_fingerprints = []",
            "allowed_selector_fingerprints = [\"configured-greeks-selector\"]",
        )
        .replace(
            "allowed_source_ids = []",
            "allowed_source_ids = [\"configured-greeks-source\"]",
        );
    let config = load_iv_config_from_toml(&scoped).unwrap();

    let authorizations = config.profiles[0].strategy_authorizations();

    assert_eq!(authorizations.len(), 1);
    assert_eq!(authorizations[0].strategy_id, "configured-strategy");
    assert_eq!(
        authorizations[0].allowed_product_kinds,
        [IvProductKind::IvPoint, IvProductKind::SourceHealth].into()
    );
    assert_eq!(
        authorizations[0].allowed_selector_fingerprints,
        ["configured-greeks-selector".to_string()].into()
    );
}

#[test]
fn unknown_schema_version_rejects_before_subscription_planning() {
    let invalid = valid_iv_toml().replacen("schema_version = 1", "schema_version = 999", 1);

    assert!(matches!(
        load_iv_config_from_toml(&invalid),
        Err(IvConfigError::Validation(messages))
            if messages.iter().any(|message| message.contains("iv.schema_version"))
    ));
}

#[test]
fn selector_scoped_source_health_authorization_can_scope_by_source_id() {
    let toml = valid_iv_toml()
        .replace(
            "authorization_mode = \"profile_wide\"",
            "authorization_mode = \"selector_scoped\"",
        )
        .replace(
            "allowed_product_kinds = [\"iv_point\", \"source_health\"]",
            "allowed_product_kinds = [\"source_health\"]",
        )
        .replace(
            "allowed_source_ids = []",
            "allowed_source_ids = [\"configured-greeks-source\"]",
        );

    let config = load_iv_config_from_toml(&toml).unwrap();

    assert_eq!(
        config.profiles[0].selector_authorization.authorization_mode,
        IvAuthorizationMode::SelectorScoped
    );
}

#[test]
fn unknown_nested_audit_policy_fields_reject_at_parse() {
    let invalid = valid_iv_toml().replacen(
        "access_purposes = [\"configured-replay-purpose\"]",
        "access_purposes = [\"configured-replay-purpose\"]\nunknown_policy_field = \"bad\"",
        1,
    );

    assert!(matches!(
        load_iv_config_from_toml(&invalid),
        Err(IvConfigError::Parse(message)) if message.contains("unknown_policy_field")
    ));
}

#[test]
fn unknown_projection_policy_fields_reject_at_parse() {
    let invalid = valid_iv_toml().replacen(
        "max_projection_input_skew_ns = 10",
        "max_projection_input_skew_ns = 10\nunknown_projection_field = \"bad\"",
        1,
    );

    assert!(matches!(
        load_iv_config_from_toml(&invalid),
        Err(IvConfigError::Parse(message)) if message.contains("unknown_projection_field")
    ));
}

#[test]
fn selector_source_product_mismatch_rejects_with_exact_field() {
    let invalid = valid_iv_toml().replacen(
        "source_kind = \"option_greeks\"",
        "source_kind = \"option_chain\"",
        1,
    );
    let config: IvRootConfig = toml::from_str(&invalid).unwrap();

    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("iv.profiles.configured-profile.sources.configured-greeks-source.selector")
    }));
}

#[test]
fn numeric_bounds_and_empty_selectors_reject() {
    let invalid = [
        "max_raw_events",
        "max_indexed_points",
        "max_smiles",
        "max_surfaces",
        "max_derived_points",
        "max_source_health_events",
    ]
    .into_iter()
    .fold(valid_iv_toml().to_string(), |toml, field| {
        toml.replace(&format!("{field} = 2"), &format!("{field} = 0"))
    })
    .replace(
        "instrument_ids = [\"configured-instrument\"]",
        "instrument_ids = []",
    );
    let config: IvRootConfig = toml::from_str(&invalid).unwrap();
    let errors = validate_iv_root_config(&config);

    for field in [
        "max_raw_events",
        "max_indexed_points",
        "max_smiles",
        "max_surfaces",
        "max_derived_points",
        "max_source_health_events",
    ] {
        assert!(errors.iter().any(|message| message.contains(field)));
    }
    assert!(
        errors
            .iter()
            .any(|message| message.contains("instrument_ids"))
    );
}

#[test]
fn duplicate_profile_ids_reject_at_config_validation() {
    let mut config: IvRootConfig = toml::from_str(valid_iv_toml()).unwrap();
    config.profiles.push(config.profiles[0].clone());

    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("iv.profiles.configured-profile")
            && message.contains("profile_id is duplicated")
    }));
}

#[test]
fn duplicate_profile_ids_reject_through_toml_loader() {
    let duplicate_profile = valid_iv_toml()
        .trim_start()
        .strip_prefix("schema_version = 1\n\n")
        .unwrap();
    let invalid = format!("{}\n{}", valid_iv_toml(), duplicate_profile);

    let error =
        load_iv_config_from_toml(&invalid).expect_err("duplicate profile_id must reject at load");

    assert!(matches!(
        error,
        IvConfigError::Validation(errors)
            if errors.iter().any(|message| {
                message.contains("iv.profiles.configured-profile")
                    && message.contains("profile_id is duplicated")
            })
    ));
}

#[test]
fn duplicate_profile_ids_report_every_extra_occurrence() {
    let mut config: IvRootConfig = toml::from_str(valid_iv_toml()).unwrap();
    let duplicate = config.profiles[0].clone();
    config.profiles.push(duplicate.clone());
    config.profiles.push(duplicate);

    let errors = validate_iv_root_config(&config);

    let duplicate_profile_errors = errors
        .iter()
        .filter(|message| message.contains("profile_id is duplicated"))
        .count();
    assert_eq!(duplicate_profile_errors, 2);
}

#[test]
fn duplicate_profile_validation_still_reports_nested_profile_errors() {
    let mut config: IvRootConfig = toml::from_str(valid_iv_toml()).unwrap();
    let mut duplicate = config.profiles[0].clone();
    duplicate.sources.push(duplicate.sources[0].clone());
    config.profiles.push(duplicate);

    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("iv.profiles.configured-profile")
            && message.contains("profile_id is duplicated")
    }));
    assert!(errors.iter().any(|message| {
        message.contains("sources.configured-greeks-source")
            && message.contains("source_id is duplicated")
    }));
}

#[test]
fn profile_ids_reject_surrounding_whitespace() {
    let invalid = valid_iv_toml().replacen(
        "profile_id = \"configured-profile\"",
        "profile_id = \" configured-profile \"",
        1,
    );

    let error = load_iv_config_from_toml(&invalid)
        .expect_err("profile_id with surrounding whitespace must reject");

    assert!(matches!(
        error,
        IvConfigError::Validation(errors)
            if errors.iter().any(|message| {
                message.contains("iv.profiles.profile_id")
                    && message.contains("leading or trailing whitespace")
            })
    ));
}

#[test]
fn derived_input_policy_must_require_every_helper_input_field() {
    let invalid = valid_iv_toml().replacen(
        "required_fields = [\"option_price\", \"underlying_price\", \"strike\", \"option_side\", \"time_to_expiry_years\", \"rate\", \"carry\"]",
        "required_fields = [\"option_price\", \"underlying_price\", \"strike\", \"option_side\", \"time_to_expiry_years\", \"rate\"]",
        1,
    );
    let config: IvRootConfig = toml::from_str(&invalid).unwrap();

    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("derived_input_policies.configured-derived-input-policy.required_fields")
            && message.contains("carry")
    }));
}

#[test]
fn helper_policy_input_policy_ref_requires_configured_derived_input_policy() {
    let mut config: IvRootConfig = toml::from_str(valid_iv_toml()).unwrap();
    config.profiles[0].derived_input_policies.clear();

    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("helper_policies.configured-helper-policy.input_policy_ref")
            && message.contains("configured derived input policy")
    }));
}

#[test]
fn derived_input_policy_field_sources_must_cover_required_fields() {
    let mut config: IvRootConfig = toml::from_str(valid_iv_toml()).unwrap();
    config.profiles[0].derived_input_policies[0]
        .field_sources
        .retain(|field_source| field_source.field != IvDerivedInputField::Carry);

    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("derived_input_policies.configured-derived-input-policy.field_sources")
            && message.contains("carry")
    }));
}

#[test]
fn selector_scoped_authorization_rejects_unknown_selector_fingerprint() {
    let invalid = valid_iv_toml()
        .replace(
            "authorization_mode = \"profile_wide\"",
            "authorization_mode = \"selector_scoped\"",
        )
        .replace(
            "allowed_selector_fingerprints = []",
            "allowed_selector_fingerprints = [\"missing-selector\"]",
        );
    let config: IvRootConfig = toml::from_str(&invalid).unwrap();
    let errors = validate_iv_root_config(&config);

    assert!(errors.iter().any(|message| {
        message.contains("selector_authorization.allowed_selector_fingerprints")
            && message.contains("missing-selector")
    }));
}

#[test]
fn duplicate_option_greeks_nt_topics_reject_before_runtime_binding() {
    let duplicate_source = r#"
[[profiles.sources]]
source_id = "configured-duplicate-greeks-source"
selector_fingerprint = "configured-duplicate-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-backup-client"
subscription_generation = 9
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredDuplicateOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-backup-value"

[profiles.sources.params]
configured_source_param = "configured-backup-value"

"#;
    let invalid = valid_iv_toml().replace(
        "[[profiles.sources]]\nsource_id = \"configured-chain-source\"",
        &format!("{duplicate_source}[[profiles.sources]]\nsource_id = \"configured-chain-source\""),
    );

    let error = load_iv_config_from_toml(&invalid)
        .expect_err("duplicate option-greeks NT topics must reject at config validation");
    let IvConfigError::Validation(errors) = error else {
        panic!("expected validation error for duplicate NT option topic");
    };
    assert!(
        errors.iter().any(|message| {
            message.contains("option_greeks")
                && message.contains("configured-instrument")
                && message.contains("configured-duplicate-greeks-source")
        }),
        "validation errors should identify duplicate option topic source: {errors:?}"
    );
}

#[test]
fn duplicate_custom_data_nt_topics_reject_before_runtime_binding() {
    let duplicate_custom_sources = r#"
[[profiles.sources]]
source_id = "configured-aggregate-source-a"
selector_fingerprint = "configured-aggregate-selector-a"
source_kind = "aggregate_greeks"
client_id = "configured-client"
subscription_generation = 9
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredAggregateGreeks"

[profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "configured-aggregate-topic"
underlying_selectors = ["configured-underlying-selector"]
delta_field = "configured-delta"
gamma_field = "configured-gamma"
vega_field = "configured-vega"
theta_field = "configured-theta"
rho_field = "configured-rho"

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-aggregate-a"

[profiles.sources.params]
configured_source_param = "configured-aggregate-a"

[[profiles.sources]]
source_id = "configured-aggregate-source-b"
selector_fingerprint = "configured-aggregate-selector-b"
source_kind = "aggregate_greeks"
client_id = "configured-client"
subscription_generation = 10
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredAggregateGreeks"

[profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "configured-aggregate-topic"
underlying_selectors = ["configured-underlying-selector"]
delta_field = "configured-delta"
gamma_field = "configured-gamma"
vega_field = "configured-vega"
theta_field = "configured-theta"
rho_field = "configured-rho"

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-aggregate-b"

[profiles.sources.params]
configured_source_param = "configured-aggregate-b"

[[profiles.sources]]
source_id = "configured-custom-source-a"
selector_fingerprint = "configured-custom-selector-a"
source_kind = "custom_implied_volatility"
client_id = "configured-client"
subscription_generation = 11
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredCustomIv"

[profiles.sources.selector]
selector_kind = "source_custom_implied_volatility"
custom_iv_data_type = "configured-custom-topic"
custom_iv_data_fields = ["configured-value"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-custom-a"

[profiles.sources.params]
configured_source_param = "configured-custom-a"

[[profiles.sources]]
source_id = "configured-custom-source-b"
selector_fingerprint = "configured-custom-selector-b"
source_kind = "custom_implied_volatility"
client_id = "configured-client"
subscription_generation = 12
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredCustomIv"

[profiles.sources.selector]
selector_kind = "source_custom_implied_volatility"
custom_iv_data_type = "configured-custom-topic"
custom_iv_data_fields = ["configured-value"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-custom-b"

[profiles.sources.params]
configured_source_param = "configured-custom-b"

"#;
    let invalid = valid_iv_toml().replace(
        "[[profiles.sources]]\nsource_id = \"configured-chain-source\"",
        &format!(
            "{duplicate_custom_sources}[[profiles.sources]]\nsource_id = \"configured-chain-source\""
        ),
    );

    let error = load_iv_config_from_toml(&invalid)
        .expect_err("duplicate custom-data NT topics must reject at config validation");
    let IvConfigError::Validation(errors) = error else {
        panic!("expected validation error for duplicate custom-data NT topics");
    };
    assert!(
        errors
            .iter()
            .any(|message| message.contains("configured-aggregate-topic")),
        "validation errors should identify duplicate aggregate topic: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|message| message.contains("configured-custom-topic")),
        "validation errors should identify duplicate custom-IV topic: {errors:?}"
    );
}
