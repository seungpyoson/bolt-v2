mod support;

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy, load_bolt_v3_config},
    bolt_v3_decision_event_context::{
        BoltV3DecisionEventIdentity, bolt_v3_decision_event_common_fields,
    },
    bolt_v3_release_identity::load_bolt_v3_release_identity,
};
use tempfile::TempDir;
use toml::Value;

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

#[test]
fn decision_event_common_fields_come_from_v3_config_and_release_identity() {
    let temp_dir = TempDir::new().unwrap();
    let (loaded, identity) = loaded_with_release_identity(temp_dir.path());
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");
    let decision_trace_id = decision_trace_id(&identity, strategy);

    let common =
        bolt_v3_decision_event_common_fields(&loaded, strategy, &identity, &decision_trace_id)
            .expect("common fields should derive from configured strategy");

    assert_eq!(common.schema_version, 1);
    assert_eq!(common.decision_trace_id, decision_trace_id);
    assert_eq!(
        common.strategy_instance_id,
        strategy.config.strategy_instance_id
    );
    assert_eq!(
        common.strategy_archetype,
        strategy.config.strategy_archetype.as_str()
    );
    assert_eq!(common.trader_id, loaded.root.trader_id);
    assert_eq!(common.client_id, strategy.config.execution_client_id);
    assert_eq!(
        common.venue,
        loaded
            .root
            .clients
            .get(&strategy.config.execution_client_id)
            .expect("client should exist")
            .venue
            .as_str()
    );
    assert_eq!(common.runtime_mode, runtime_mode_from_root_toml(&loaded));
    assert_eq!(common.release_id, identity.release_id);
    assert_eq!(common.config_hash, identity.config_hash);
    assert_eq!(
        common.nautilus_trader_revision,
        identity.nautilus_trader_revision
    );
    assert_eq!(common.configured_target_id, configured_target_id(strategy));
}

#[test]
fn decision_event_context_rejects_missing_client_binding() {
    let temp_dir = TempDir::new().unwrap();
    let (mut loaded, identity) = loaded_with_release_identity(temp_dir.path());
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy")
        .clone();
    loaded
        .root
        .clients
        .remove(&strategy.config.execution_client_id);
    let decision_trace_id = decision_trace_id(&identity, &strategy);

    let error =
        bolt_v3_decision_event_common_fields(&loaded, &strategy, &identity, &decision_trace_id)
            .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("references missing execution_client_id")
    );
}

#[test]
fn decision_event_context_rejects_blank_release_identity() {
    let temp_dir = TempDir::new().unwrap();
    let (loaded, mut identity) = loaded_with_release_identity(temp_dir.path());
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");
    identity.release_id = " ".to_string();
    let decision_trace_id = decision_trace_id(&identity, strategy);

    let error =
        bolt_v3_decision_event_common_fields(&loaded, strategy, &identity, &decision_trace_id)
            .unwrap_err();

    assert!(error.to_string().contains("release_id must be non-empty"));
}

#[test]
fn decision_event_context_wiring_has_no_strategy_or_venue_literal_dispatch() {
    let path = support::repo_path("src/bolt_v3_decision_event_context.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()));
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");
    let forbidden_client_id = strategy.config.execution_client_id.as_str();
    let forbidden_target_id = configured_target_id(strategy);
    let forbidden_underlying_asset = target_string(strategy, "underlying_asset");
    let forbidden_instance_prefix = strategy
        .config
        .strategy_instance_id
        .split('-')
        .next()
        .expect("strategy instance fixture should contain prefix");

    for forbidden in [
        strategy.config.strategy_archetype.as_str(),
        forbidden_instance_prefix,
        forbidden_client_id,
        forbidden_target_id,
        forbidden_underlying_asset,
        "if strategy",
        "match strategy",
    ] {
        assert!(
            !source.contains(forbidden),
            "decision-event context must not hardcode `{forbidden}`"
        );
    }
}

fn loaded_with_release_identity(
    temp_dir: &std::path::Path,
) -> (LoadedBoltV3Config, BoltV3DecisionEventIdentity) {
    let mut loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir);
    let identity = load_bolt_v3_release_identity(&loaded).expect("release identity should load");
    (loaded, identity)
}

fn decision_trace_id(identity: &BoltV3DecisionEventIdentity, strategy: &LoadedStrategy) -> String {
    format!(
        "{}:{}",
        identity.release_id, strategy.config.strategy_instance_id
    )
}

fn runtime_mode_from_root_toml(loaded: &LoadedBoltV3Config) -> String {
    let text = std::fs::read_to_string(&loaded.root_path).expect("root fixture should read");
    let value: Value = toml::from_str(&text).expect("root fixture should parse as TOML value");
    value
        .get("runtime")
        .and_then(Value::as_table)
        .and_then(|runtime| runtime.get("mode"))
        .and_then(Value::as_str)
        .expect("root fixture should define runtime.mode")
        .to_string()
}

fn configured_target_id(strategy: &LoadedStrategy) -> &str {
    target_string(strategy, "configured_target_id")
}

fn target_string<'a>(strategy: &'a LoadedStrategy, field: &str) -> &'a str {
    strategy
        .config
        .target
        .as_table()
        .and_then(|target| target.get(field))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("strategy target should define {field}"))
}
