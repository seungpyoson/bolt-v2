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
max_source_health_events = 2

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
accepted_conventions = ["configured-convention"]

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
accepted_conventions = ["configured-convention"]

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
    let invalid = valid_iv_toml()
        .replace("max_raw_events = 2", "max_raw_events = 0")
        .replace(
            "instrument_ids = [\"configured-instrument\"]",
            "instrument_ids = []",
        );
    let config: IvRootConfig = toml::from_str(&invalid).unwrap();
    let errors = validate_iv_root_config(&config);

    assert!(
        errors
            .iter()
            .any(|message| message.contains("max_raw_events"))
    );
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
