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

#[test]
fn observation_purpose_cannot_route_to_the_recovery_sink() {
    let mutated = replace_once(
        REGISTRY,
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"observation_bounded_failure\"\nsink = \"observation\"",
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"observation_bounded_failure\"\nsink = \"machine\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("observation purpose must not route to the recovery sink");
    assert!(
        error
            .to_string()
            .contains("duty class is incompatible with sink")
    );
}

#[test]
fn observation_purpose_requires_the_observation_failure_policy() {
    let mutated = replace_once(
        REGISTRY,
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"observation_bounded_failure\"",
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"preserve_result\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("observation purpose must use its registered failure policy");
    assert!(
        error
            .to_string()
            .contains("duty class is incompatible with effect_policy")
    );
}

#[test]
fn adding_a_consumer_requires_every_fact_to_be_readjudicated() {
    let mutated = format!(
        "{REGISTRY}\n[[consumers]]\nid = \"new_recovery_consumer_v1\"\nmode = \"startup_recovery\"\nowner = \"bolt_v3_decision_evidence\"\nsource_anchor = \"src/new_consumer.rs:recover\"\n"
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("consumer-universe growth must invalidate every incomplete fact matrix");
    assert!(
        error
            .to_string()
            .contains("must explicitly disposition every consumer")
    );
}

#[test]
fn every_purpose_has_exactly_one_current_identity() {
    let duplicate = r#"
[[identities]]
id = "blocked_strategy_input_observation_v2"
kind = "blocked_strategy_input_observation_v2"
schema_version = 1
gate_id = "bolt_v3.strategy_input_snapshot"
purpose = "blocked_strategy_input_observation"
fact_ids = ["blocked_strategy_input_observation_v1"]
payload_member = "blocked_strategy_input_observation"
"#;
    let mutated = format!("{REGISTRY}{duplicate}");
    let error = parse_contract_registry(&mutated)
        .expect_err("a purpose with multiple current identities must be rejected");
    assert!(error.to_string().contains("exactly one current identity"));
}

#[test]
fn every_purpose_has_a_registered_structural_producer() {
    let mutated = replace_once(
        REGISTRY,
        "id = \"edge_taker_blocked_strategy_input\"\npurpose = \"blocked_strategy_input_observation\"",
        "id = \"edge_taker_blocked_strategy_input\"\npurpose = \"submit_linked_strategy_input_snapshot\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("a purpose without a structural producer must be rejected");
    assert!(error.to_string().contains("has no registered producer"));
}

#[test]
fn orphan_current_fact_is_rejected() {
    let orphan = r##"
[[facts]]
id = "orphan_fact_v1"
owner = "bolt_v3_decision_evidence"
dispositions = { submit_reservation_recovery_v1 = "irrelevant:#1354", settlement_recovery_v1 = "irrelevant:#1354", booking_recovery_v1 = "irrelevant:#1354", shadow_pnl_v1 = "irrelevant:#1354" }
"##;
    let mutated = format!("{REGISTRY}{orphan}");
    let error = parse_contract_registry(&mutated)
        .expect_err("a fact without a current identity must be rejected");
    assert!(
        error
            .to_string()
            .contains("must belong to exactly one current identity")
    );
}

#[test]
fn consumer_without_any_relevant_fact_is_rejected() {
    let mutated = REGISTRY
        .replace(
            "shadow_pnl_v1 = \"relevant\"",
            "shadow_pnl_v1 = \"irrelevant:#1354\"",
        )
        .replace(
            "shadow_pnl_v1 = \"relevant\"",
            "shadow_pnl_v1 = \"irrelevant:#1354\"",
        )
        .replace(
            "shadow_pnl_v1 = \"relevant\"",
            "shadow_pnl_v1 = \"irrelevant:#1354\"",
        );
    let error = parse_contract_registry(&mutated)
        .expect_err("a consumer with no relevant fact must be rejected");
    assert!(error.to_string().contains("has no relevant fact"));
}
