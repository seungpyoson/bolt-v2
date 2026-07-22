use bolt_v2::bolt_v3_decision_evidence::contract_generator::{
    parse_contract_registry, render_contract_rust,
};

const REGISTRY: &str = include_str!("../config/decision-evidence-contract.toml");
const GENERATED: &str = include_str!("../src/bolt_v3_decision_evidence/generated_contract.rs");

fn replace_once(source: &str, from: &str, to: &str) -> String {
    assert!(source.contains(from), "fixture must contain `{from}`");
    source.replacen(from, to, 1)
}

#[test]
fn current_contract_renders_byte_exact_generated_rust() {
    let registry = parse_contract_registry(REGISTRY).expect("current contract should parse");
    let rendered = render_contract_rust(&registry).expect("current contract should render");
    assert_eq!(rendered, GENERATED);
}

#[test]
fn historical_identity_metadata_is_not_accepted() {
    let mutated = replace_once(
        REGISTRY,
        "schema_version = 1\ngate_id = \"bolt_v3.strategy_input_snapshot\"",
        "schema_version = 1\nstatus = \"historical\"\ngate_id = \"bolt_v3.strategy_input_snapshot\"",
    );
    assert!(parse_contract_registry(&mutated).is_err());
}

#[test]
fn every_fact_consumer_disposition_is_required() {
    let mutated = replace_once(REGISTRY, ", shadow_pnl_v1 = \"irrelevant:#1354\"", "");
    let error = parse_contract_registry(&mutated)
        .expect_err("missing fact-consumer disposition must reject the registry");
    assert!(
        error
            .to_string()
            .contains("must explicitly disposition every consumer")
    );
}

#[test]
fn duplicate_exact_current_identity_pair_is_rejected() {
    let duplicate = r#"
[[identities]]
id = "duplicate_blocked_strategy_input_observation_v1"
kind = "blocked_strategy_input_observation"
schema_version = 1
gate_id = "bolt_v3.blocked_strategy_input_observation"
purpose = "blocked_strategy_input_observation"
fact_ids = ["blocked_strategy_input_observation_v1"]
payload_member = "blocked_strategy_input_observation"
"#;
    let mutated = format!("{REGISTRY}{duplicate}");
    let error = parse_contract_registry(&mutated)
        .expect_err("duplicate exact identity pair must reject the registry");
    assert!(error.to_string().contains("duplicate exact identity pair"));
}

#[test]
fn novelty_capability_is_closed_until_an_observation_design_registers_it() {
    let mutated = replace_once(
        REGISTRY,
        "novelty_capability = \"prohibited\"",
        "novelty_capability = \"allowed\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("unsupported novelty capability must reject the registry");
    assert!(error.to_string().contains("unsupported novelty capability"));
}
