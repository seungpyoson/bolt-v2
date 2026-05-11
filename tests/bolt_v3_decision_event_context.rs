mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_event_context::{
        BoltV3DecisionEventIdentity, bolt_v3_decision_event_common_fields,
    },
};

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

#[test]
fn decision_event_common_fields_come_from_v3_config_and_release_identity() {
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");
    let identity = BoltV3DecisionEventIdentity {
        release_id: "release-sha".to_string(),
        config_hash: "config-hash".to_string(),
        nautilus_trader_revision: "38b912a8b0fe14e4046773973ff46a3b798b1e3e".to_string(),
    };

    let common = bolt_v3_decision_event_common_fields(
        &loaded,
        strategy,
        &identity,
        "123e4567-e89b-12d3-a456-426614174002",
    )
    .expect("common fields should derive from configured strategy");

    assert_eq!(common.schema_version, 1);
    assert_eq!(
        common.decision_trace_id,
        "123e4567-e89b-12d3-a456-426614174002"
    );
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
    assert_eq!(common.runtime_mode, "live");
    assert_eq!(common.release_id, identity.release_id);
    assert_eq!(common.config_hash, identity.config_hash);
    assert_eq!(
        common.nautilus_trader_revision,
        identity.nautilus_trader_revision
    );
    assert_eq!(common.configured_target_id, "eth_updown_5m");
}

#[test]
fn decision_event_context_rejects_missing_client_binding() {
    let mut loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy")
        .clone();
    loaded
        .root
        .clients
        .remove(&strategy.config.execution_client_id);
    let identity = BoltV3DecisionEventIdentity {
        release_id: "release-sha".to_string(),
        config_hash: "config-hash".to_string(),
        nautilus_trader_revision: "38b912a8b0fe14e4046773973ff46a3b798b1e3e".to_string(),
    };

    let error = bolt_v3_decision_event_common_fields(
        &loaded,
        &strategy,
        &identity,
        "123e4567-e89b-12d3-a456-426614174002",
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("references missing execution_client_id")
    );
}

#[test]
fn decision_event_context_rejects_blank_release_identity() {
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");
    let identity = BoltV3DecisionEventIdentity {
        release_id: " ".to_string(),
        config_hash: "config-hash".to_string(),
        nautilus_trader_revision: "38b912a8b0fe14e4046773973ff46a3b798b1e3e".to_string(),
    };

    let error = bolt_v3_decision_event_common_fields(
        &loaded,
        strategy,
        &identity,
        "123e4567-e89b-12d3-a456-426614174002",
    )
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
    let forbidden_client_id = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy")
        .config
        .execution_client_id
        .as_str();

    for forbidden in [
        "eth_chainlink_taker",
        "ETHCHAINLINKTAKER",
        forbidden_client_id,
        "eth_updown_5m",
        "ETH",
        "if strategy",
        "match strategy",
    ] {
        assert!(
            !source.contains(forbidden),
            "decision-event context must not hardcode `{forbidden}`"
        );
    }
}
