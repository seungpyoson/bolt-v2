mod support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_adapters::{BoltV3ClientMappingError, map_bolt_v3_clients},
    bolt_v3_config::{
        LoadedBoltV3Config, REFERENCE_STREAM_ID_PARAMETER, ReferenceSourceType, load_bolt_v3_config,
    },
    bolt_v3_providers::chainlink::{self, ChainlinkSecretsConfig, ResolvedBoltV3ChainlinkSecrets},
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
};

fn existing_strategy_fixture() -> LoadedBoltV3Config {
    load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing strategy root should load")
}

fn selected_chainlink_client_id(loaded: &LoadedBoltV3Config) -> &str {
    let strategy = loaded
        .strategies
        .first()
        .expect("fixture should define one strategy");
    let parameters = strategy
        .config
        .parameters
        .as_table()
        .expect("strategy parameters should be a table");
    let stream_id = parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("strategy should select a reference stream");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist");
    stream
        .inputs
        .iter()
        .find(|input| {
            input.source_type == ReferenceSourceType::Oracle && input.provider_config.is_some()
        })
        .and_then(|input| input.data_client_id.as_deref())
        .expect("selected reference stream should name a Chainlink data client")
}

fn chainlink_secrets_config(
    loaded: &LoadedBoltV3Config,
    client_id: &str,
) -> ChainlinkSecretsConfig {
    let client = loaded
        .root
        .clients
        .get(client_id)
        .expect("selected Chainlink client should exist");
    assert_eq!(client.venue.as_str(), chainlink::KEY);
    client
        .secrets
        .as_ref()
        .expect("selected Chainlink client should declare [secrets]")
        .clone()
        .try_into()
        .expect("Chainlink secrets block should parse")
}

#[test]
fn chainlink_reference_delivery_resolves_configured_ssm_paths_before_adapter_mapping() {
    let loaded = existing_strategy_fixture();
    let client_id = selected_chainlink_client_id(&loaded);
    let secrets = chainlink_secrets_config(&loaded, client_id);
    let chainlink_paths = [
        secrets.api_key_ssm_path.clone(),
        secrets.api_secret_ssm_path.clone(),
    ];
    let mut calls = Vec::new();
    let mut returned_values = BTreeMap::new();

    let resolved = resolve_bolt_v3_secrets_with(&loaded, |region, path| {
        let value = if chainlink_paths.iter().any(|configured| configured == path) {
            format!("resolved-chainlink-value-{}", calls.len())
        } else {
            support::fake_bolt_v3_resolver(region, path).map_err(str::to_string)?
        };
        calls.push((region.to_string(), path.to_string()));
        returned_values.insert(path.to_string(), value.clone());
        Ok::<_, String>(value)
    })
    .expect("fixture secrets should resolve through injected SSM resolver");

    assert!(
        resolved
            .get_as::<ResolvedBoltV3ChainlinkSecrets>(client_id)
            .is_some(),
        "resolved secrets must include selected Chainlink client `{client_id}`"
    );
    for configured_path in [
        secrets.api_key_ssm_path.as_str(),
        secrets.api_secret_ssm_path.as_str(),
    ] {
        assert!(
            calls
                .iter()
                .any(|(region, path)| region == &loaded.root.aws.region && path == configured_path),
            "missing SSM call for configured Chainlink path {configured_path}: {calls:#?}"
        );
    }

    let debug = format!("{resolved:?}");
    assert!(debug.contains(client_id));
    for configured_path in [
        secrets.api_key_ssm_path.as_str(),
        secrets.api_secret_ssm_path.as_str(),
    ] {
        let returned = returned_values
            .get(configured_path)
            .expect("configured path should have returned a synthetic value");
        assert!(
            !debug.contains(returned),
            "resolved Chainlink secret value must not appear in Debug output"
        );
    }
}

#[test]
fn chainlink_adapter_mapping_requires_resolved_ssm_secrets() {
    let loaded = existing_strategy_fixture();
    let client_id = selected_chainlink_client_id(&loaded).to_string();
    let mut resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM resolver");

    resolved.clients.remove(&client_id);
    let error = map_bolt_v3_clients(&loaded, &resolved)
        .expect_err("mapper must not build Chainlink data client without resolved SSM secrets");

    match error {
        BoltV3ClientMappingError::MissingResolvedSecrets {
            client_id_key,
            expected_venue,
        } => {
            assert_eq!(client_id_key, client_id);
            assert_eq!(expected_venue, chainlink::KEY);
        }
        other => panic!("expected MissingResolvedSecrets for Chainlink, got {other}"),
    }
}
