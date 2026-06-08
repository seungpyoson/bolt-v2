use bolt_v2::bolt_v3_iv::{
    authz::IvAuthorizationMode,
    config::{IvConfigError, IvRootConfig, load_iv_config_from_toml, validate_iv_root_config},
    selector::IvSelector,
    types::{IvProductKind, IvSourceKind},
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
convention_policy = "configured-convention-policy"
failure_policy = "reject_invalid_helper_output"
max_input_timestamp_skew_ns = 10
max_operator_input_age_ns = 100

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
bounds = "configured-derived-input-bounds"
convention_policy = "configured-convention-policy"
operator_value_refresh_policy = "reject_expired_operator_values"

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
