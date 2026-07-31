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

/// The whole `[[producers]]` block for one id.
///
/// Spelling the block out as a literal made the anchor break whenever a field
/// was added to every producer, which is a mutation test failing for a reason
/// that has nothing to do with what it tests.
fn producer_block(input: &str, id: &str) -> String {
    let anchor = format!("[[producers]]\nid = \"{id}\"\n");
    let start = input.find(&anchor).expect("producer must exist");
    let rest = &input[start + anchor.len()..];
    let end = rest
        .find("\n[[")
        .map_or(input.len(), |at| start + anchor.len() + at + 1);
    input[start..end].to_string()
}

#[test]
fn current_contract_is_closed_and_deterministic() {
    let contract = parse_contract_registry(REGISTRY).expect("current contract must parse");
    assert_eq!(contract.consumer_count(), 5);
    assert_eq!(contract.producer_count(), 24);
    assert_eq!(contract.purpose_count(), 23);
    assert_eq!(contract.identity_count(), 23);
    assert_eq!(contract.fact_count(), 23);

    let first = render_contract(&contract);
    let second = render_contract(&contract);
    assert_eq!(first, second);
    assert_eq!(first, GENERATED);
}

#[test]
fn every_purpose_requires_at_least_one_structural_producer() {
    let mutated = replace_once(
        REGISTRY,
        &producer_block(REGISTRY, "edge_taker_blocked_strategy_input"),
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

/// A classification outside the closed set is refused.
#[test]
fn an_unknown_producer_classification_is_rejected() {
    let mutated = replace_once(
        REGISTRY,
        "classification = \"state-observation\"",
        "classification = \"probably-fine\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("a classification outside the closed set must fail");
    assert!(
        error.to_string().contains("unknown classification"),
        "unexpected error: {error}"
    );
}

/// A producer that names no append path has classified nothing.
#[test]
fn a_producer_without_a_call_site_is_rejected() {
    let block = producer_block(REGISTRY, "edge_taker_entry_skip");
    let stripped = block
        .lines()
        .map(|line| {
            if line.starts_with("call_sites = ") {
                "call_sites = []".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let error = parse_contract_registry(&replace_once(REGISTRY, &block, &stripped))
        .expect_err("a producer naming no call site must fail");
    assert!(
        error
            .to_string()
            .contains("names no reviewed call-site provenance"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_call_site_without_a_rust_path_and_function_is_rejected() {
    let mutated = replace_once(
        REGISTRY,
        "call_sites = [\"src/strategies/binary_oracle_edge_taker/mod.rs::record_entry_skip_once\"]",
        "call_sites = [\"record_entry_skip_once\"]",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("reviewed call-site provenance without its stable shape must fail");
    assert!(
        error.to_string().contains("is not `<path>.rs::<function>`"),
        "unexpected error: {error}"
    );
}

#[test]
fn empty_repeat_semantics_are_rejected() {
    let mutated = replace_once(
        REGISTRY,
        "repeat_semantics = \"the current last-key mask suppresses the twenty entry-skip reasons only while its full observation key remains adjacent and therefore re-emits A-B-A; the stacked episode-suppression slice replaces that key with a stable episode identity and a monotone twenty-state mask\"",
        "repeat_semantics = \"\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("a producer without reviewed repeat semantics must fail");
    assert!(
        error.to_string().contains("states no repeat semantics"),
        "unexpected error: {error}"
    );
}

#[test]
fn empty_dedupe_key_evidence_is_rejected() {
    let mutated = replace_once(
        REGISTRY,
        "dedupe_key_evidence = \"at this revision the full entry-evaluation key contains volatile observation fields; the required target is stable strategy, target, venue and ordered market identity plus the exact twenty-state entry-skip enumeration\"",
        "dedupe_key_evidence = \"\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("a producer without reviewed dedupe-key evidence must fail");
    assert!(
        error.to_string().contains("states no dedupe-key evidence"),
        "unexpected error: {error}"
    );
}

#[test]
fn an_unknown_census_ancestor_is_rejected() {
    let mutated = replace_once(
        REGISTRY,
        "census_ancestor = \"entry_skip\"",
        "census_ancestor = \"unknown_retired_row\"",
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("an unknown retired-census ancestor must fail");
    assert!(
        error
            .to_string()
            .contains("is not a row of the retired census"),
        "unexpected error: {error}"
    );
}

fn replace_runtime_provenance(block: &str, wiring: &str, triggers: &str) -> String {
    block
        .lines()
        .map(|line| {
            if line.starts_with("runtime_wiring = ") {
                format!("runtime_wiring = \"{wiring}\"")
            } else if line.starts_with("reviewed_runtime_triggers = ") {
                format!("reviewed_runtime_triggers = [{triggers}]")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reviewed runtime provenance uses a closed trigger vocabulary.
#[test]
fn an_unknown_reviewed_runtime_trigger_is_rejected() {
    let block = producer_block(REGISTRY, "edge_taker_entry_skip");
    let mutated = replace_once(
        REGISTRY,
        &block,
        &replace_runtime_provenance(&block, "wired", "\"cron\""),
    );
    let error = parse_contract_registry(&mutated).expect_err("an unknown trigger must fail");
    assert!(
        error
            .to_string()
            .contains("unknown reviewed runtime trigger `cron`"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_wired_producer_requires_a_reviewed_runtime_trigger() {
    let block = producer_block(REGISTRY, "edge_taker_entry_skip");
    let mutated = replace_once(
        REGISTRY,
        &block,
        &replace_runtime_provenance(&block, "wired", ""),
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("wired producer without a reviewed trigger must fail");
    assert!(
        error
            .to_string()
            .contains("is wired but names no reviewed runtime trigger"),
        "unexpected error: {error}"
    );
}

#[test]
fn an_unwired_producer_refuses_reviewed_runtime_triggers() {
    let block = producer_block(REGISTRY, "edge_taker_entry_skip");
    let mutated = replace_once(
        REGISTRY,
        &block,
        &replace_runtime_provenance(&block, "unwired", "\"book\""),
    );
    let error = parse_contract_registry(&mutated)
        .expect_err("unwired producer with reviewed triggers must fail");
    assert!(
        error
            .to_string()
            .contains("is unwired but names reviewed runtime triggers"),
        "unexpected error: {error}"
    );
}

#[test]
fn an_unwired_producer_accepts_no_reviewed_runtime_triggers() {
    parse_contract_registry(REGISTRY)
        .expect("an unwired producer with no reviewed runtime triggers must parse");
}
