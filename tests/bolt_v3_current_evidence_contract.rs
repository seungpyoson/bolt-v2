use bolt_v2::bolt_v3_current_evidence::contract_generator::{
    parse_contract_registry, render_contract,
};

const REGISTRY: &str = include_str!("../config/decision-evidence-contract.toml");
const GENERATED: &str = include_str!("../src/bolt_v3_current_evidence/generated_contract.rs");

fn replace_once(input: &str, from: &str, to: &str) -> String {
    assert_eq!(
        input.matches(from).count(),
        1,
        "mutation anchor must be unique"
    );
    input.replacen(from, to, 1)
}

#[test]
fn current_contract_is_closed_and_deterministic() {
    let contract = parse_contract_registry(REGISTRY).expect("current contract must parse");
    assert_eq!(contract.consumer_count(), 5);
    assert_eq!(contract.producer_count(), 26);
    assert_eq!(contract.purpose_count(), 25);
    assert_eq!(contract.identity_count(), 25);
    assert_eq!(contract.fact_count(), 25);

    let first = render_contract(&contract);
    let second = render_contract(&contract);
    assert_eq!(first, second);
    assert_eq!(first, GENERATED);
}

#[test]
fn every_purpose_requires_at_least_one_structural_producer() {
    let mutated = replace_once(
        REGISTRY,
        "[[producers]]\nid = \"edge_taker_blocked_strategy_input\"\npurpose = \"blocked_strategy_input_observation\"\n",
        "",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("a purpose without a structural producer must fail");
    assert!(error.to_string().contains("at least one producer"));
}

#[test]
fn adding_a_consumer_invalidates_every_unadjudicated_fact() {
    let mutated = format!(
        "{REGISTRY}\n[[consumers]]\nid = \"new_recovery_consumer_v1\"\nmode = \"startup_recovery\"\nowner = \"bolt_v3_decision_evidence\"\n"
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("consumer-universe growth must require explicit dispositions");
    assert!(
        error
            .to_string()
            .contains("missing fact-consumer disposition")
    );
}

#[test]
fn every_fact_consumer_cell_requires_an_explicit_disposition() {
    let mutated = replace_once(
        REGISTRY,
        "[[dispositions]]\nfact = \"blocked_strategy_input_observation_v1\"\nconsumer = \"reservation_recovery_v1\"\naction = \"irrelevant\"\nowner_ruling = \"current_contract_owner_ruling_2026_07_22\"\n",
        "",
    );
    let error =
        parse_contract_registry(&mutated).expect_err("missing fact-consumer disposition must fail");
    assert!(
        error
            .to_string()
            .contains("missing fact-consumer disposition")
    );
}

#[test]
fn relevant_dispositions_cannot_relabel_a_fact() {
    let mutated = replace_once(
        REGISTRY,
        "[[dispositions]]\nfact = \"submit_linked_strategy_input_snapshot_v1\"\nconsumer = \"shadow_pnl_v1\"\naction = \"relevant\"\nevent_variant = \"submit_linked_strategy_input_snapshot_v1\"\n",
        "[[dispositions]]\nfact = \"submit_linked_strategy_input_snapshot_v1\"\nconsumer = \"shadow_pnl_v1\"\naction = \"relevant\"\nevent_variant = \"entry_order_intent_v1\"\n",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("a relevant disposition cannot relabel one fact as another");
    assert!(error.to_string().contains("must preserve its fact"));
}

#[test]
fn duplicate_exact_identity_pairs_are_rejected() {
    let mutated = replace_once(
        REGISTRY,
        "id = \"submit_linked_strategy_input_snapshot_v1\"\npurpose = \"submit_linked_strategy_input_snapshot\"\nkind = \"strategy_input_snapshot\"\nschema_version = 16",
        "id = \"submit_linked_strategy_input_snapshot_v1\"\npurpose = \"submit_linked_strategy_input_snapshot\"\nkind = \"blocked_strategy_input_observation\"\nschema_version = 1",
    );
    let error =
        parse_contract_registry(&mutated).expect_err("duplicate exact identity pair must fail");
    assert!(error.to_string().contains("duplicate exact identity"));
}

#[test]
fn observation_purposes_cannot_route_to_machine_or_use_a_fact_policy() {
    let mutated_sink = replace_once(
        REGISTRY,
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"observation_bounded_failure\"\nsink = \"observation\"",
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"observation_bounded_failure\"\nsink = \"machine\"",
    );
    assert!(
        parse_contract_registry(&mutated_sink)
            .expect_err("observation machine routing must fail")
            .to_string()
            .contains("observation purpose")
    );

    let mutated_policy = replace_once(
        REGISTRY,
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"observation_bounded_failure\"\nsink = \"observation\"",
        "id = \"blocked_strategy_input_observation\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"state_observation\"]\neffect_policy = \"preserve_result\"\nsink = \"observation\"",
    );
    assert!(
        parse_contract_registry(&mutated_policy)
            .expect_err("observation fact policy must fail")
            .to_string()
            .contains("observation purpose")
    );
}

#[test]
fn unknown_owner_sink_consumer_mode_and_effect_policy_are_rejected() {
    let unknown_owner = replace_once(
        REGISTRY,
        "[[owners]]\nid = \"bolt_v3_decision_evidence\"",
        "[[owners]]\nid = \"unregistered_owner\"",
    );
    assert!(
        parse_contract_registry(&unknown_owner)
            .expect_err("unknown owner must fail")
            .to_string()
            .contains("unknown owner")
    );

    let unknown_sink = replace_once(
        REGISTRY,
        "id = \"submit_linked_strategy_input_snapshot\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"join\"]\neffect_policy = \"must_precede_new_risk\"\nsink = \"machine\"",
        "id = \"submit_linked_strategy_input_snapshot\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"join\"]\neffect_policy = \"must_precede_new_risk\"\nsink = \"archive\"",
    );
    assert!(
        parse_contract_registry(&unknown_sink)
            .expect_err("unknown sink must fail")
            .to_string()
            .contains("unknown sink")
    );

    let unknown_consumer_mode = replace_once(
        REGISTRY,
        "id = \"shadow_pnl_v1\"\nmode = \"offline_projection\"",
        "id = \"shadow_pnl_v1\"\nmode = \"best_effort_query\"",
    );
    assert!(
        parse_contract_registry(&unknown_consumer_mode)
            .expect_err("unknown consumer mode must fail")
            .to_string()
            .contains("unknown mode")
    );

    let unknown_policy = replace_once(
        REGISTRY,
        "id = \"submit_linked_strategy_input_snapshot\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"join\"]\neffect_policy = \"must_precede_new_risk\"",
        "id = \"submit_linked_strategy_input_snapshot\"\nowner = \"bolt_v3_decision_evidence\"\nduties = [\"join\"]\neffect_policy = \"best_effort\"",
    );
    assert!(
        parse_contract_registry(&unknown_policy)
            .expect_err("unknown effect policy must fail")
            .to_string()
            .contains("unknown effect policy")
    );
}

#[test]
fn at_least_one_startup_recovery_consumer_is_required() {
    let without_startup_recovery = REGISTRY.replace(
        "mode = \"startup_recovery\"",
        "mode = \"offline_projection\"",
    );
    assert!(
        parse_contract_registry(&without_startup_recovery)
            .expect_err("a contract without a startup recovery consumer must fail")
            .to_string()
            .contains("missing startup recovery consumer")
    );
}
