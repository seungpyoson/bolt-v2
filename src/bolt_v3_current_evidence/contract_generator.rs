use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;

const CONTRACT_SCHEMA_VERSION: u32 = 1;
const CONSUMER_MODES: &[&str] = &["offline_projection", "startup_recovery"];
const OBSERVATION_DUTIES: &[&str] = &["diagnostic_observation", "state_observation"];
const MACHINE_DUTIES: &[&str] = &["action", "join", "reconciliation", "recovery"];
const EFFECT_POLICIES: &[&str] = &[
    "must_precede_new_risk",
    "observation_bounded_failure",
    "preserve_result",
    "reconciliation_fail_closed",
    "risk_reducing_continues",
];
/// How an append path repeats, which is what decides whether it can flood.
/// Carried from the retired census, whose whole point was that every path is
/// classified exactly once and by a name from a closed set rather than by prose.
const CLASSIFICATIONS: &[&str] = &[
    "already-deduped",
    "event-keyed",
    "no-named-reader",
    "state-observation",
];
/// The runtime entry points an append path is reachable from. `startup` is the
/// one that cannot flood; the rest are per-tick and are why #1354 exists.
const HANDLER_REACHABILITY: &[&str] = &["book", "index-price", "quote", "startup", "timer"];
/// Suppression is not implemented on this layer, so `prohibited` is the only
/// legal value today. It is a set rather than an equality because every sibling
/// check here is a set, and because the value that replaces it arrives as a
/// vocabulary entry rather than as an edit to a comparison.
const NOVELTY_CAPABILITIES: &[&str] = &["prohibited"];
/// The 19 producer rows of the retired `config/evidence-novelty.toml`.
///
/// Spelled out because that file was deleted with the layer it described, so
/// there is nothing left to resolve a name against. Without the list, provenance
/// is an unvalidated string and a producer could claim a census row that never
/// existed -- which is how the census would quietly stop meaning anything.
/// Three names here have no producer in this contract on purpose:
/// `submit_reservation_metadata`, `venue_truth_capture_failure` and
/// `venue_truth_divergence` are append paths this layer removed.
const CENSUS_PRODUCERS: &[&str] = &[
    "admission_decision",
    "basket_admission_decision",
    "capital_admission_rebuild_audit",
    "entry_skip",
    "exit_decision",
    "exit_evaluation",
    "loss_governor_halt",
    "order_intent",
    "order_lifecycle",
    "order_reject",
    "requote_throttle",
    "settlement",
    "strategy_input_snapshot_blocked_rv",
    "strategy_input_snapshot_submit",
    "submit_reservation_fill",
    "submit_reservation_metadata",
    "terminal_settlement",
    "venue_truth_capture_failure",
    "venue_truth_divergence",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryWire {
    schema_version: u32,
    owners: Vec<OwnerRow>,
    sinks: Vec<SinkRow>,
    consumers: Vec<ConsumerRow>,
    purposes: Vec<PurposeRow>,
    producers: Vec<ProducerRow>,
    identities: Vec<IdentityRow>,
    facts: Vec<FactRow>,
    dispositions: Vec<DispositionRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRow {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SinkRow {
    id: String,
    startup_recovery_reads: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumerRow {
    id: String,
    mode: String,
    owner: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PurposeRow {
    id: String,
    owner: String,
    duties: Vec<String>,
    effect_policy: String,
    sink: String,
    novelty_capability: String,
    current_identity: String,
    current_fact: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProducerRow {
    id: String,
    purpose: String,
    classification: String,
    handler_reachability: Vec<String>,
    call_sites: Vec<String>,
    repeat_semantics: String,
    dedupe_key_evidence: String,
    /// The retired census row this inherited from, empty when the producer
    /// postdates the census. Recorded so a reader can tell an inherited
    /// classification from one derived against this layer directly.
    census_ancestor: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityRow {
    id: String,
    purpose: String,
    kind: String,
    schema_version: u32,
    gate_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactRow {
    id: String,
    identity: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispositionRow {
    fact: String,
    consumer: String,
    action: String,
    event_variant: Option<String>,
    owner_ruling: Option<String>,
}

#[derive(Debug)]
pub struct ContractRegistry {
    wire: RegistryWire,
}

impl ContractRegistry {
    pub fn consumer_count(&self) -> usize {
        self.wire.consumers.len()
    }

    /// Every declared append path, as (producer id, call site).
    ///
    /// Exposed because whether a call site still exists is a question about the
    /// source tree, and this generator is deliberately a pure function of the
    /// registry string -- so the check belongs to a test that can read files.
    pub fn declared_call_sites(&self) -> impl Iterator<Item = (&str, &str)> {
        self.wire.producers.iter().flat_map(|producer| {
            producer
                .call_sites
                .iter()
                .map(|site| (producer.id.as_str(), site.as_str()))
        })
    }

    pub fn producer_count(&self) -> usize {
        self.wire.producers.len()
    }

    pub fn purpose_count(&self) -> usize {
        self.wire.purposes.len()
    }

    pub fn identity_count(&self) -> usize {
        self.wire.identities.len()
    }

    pub fn fact_count(&self) -> usize {
        self.wire.facts.len()
    }
}

pub fn parse_contract_registry(input: &str) -> Result<ContractRegistry> {
    let wire: RegistryWire =
        toml::from_str(input).context("invalid decision-evidence contract TOML")?;
    validate_registry(&wire)?;
    Ok(ContractRegistry { wire })
}

fn validate_registry(wire: &RegistryWire) -> Result<()> {
    ensure!(
        wire.schema_version == CONTRACT_SCHEMA_VERSION,
        "unsupported decision-evidence contract schema_version {}",
        wire.schema_version
    );

    let owners = unique_ids("owner", wire.owners.iter().map(|row| row.id.as_str()))?;
    let sinks = unique_ids("sink", wire.sinks.iter().map(|row| row.id.as_str()))?;
    let consumers = unique_ids("consumer", wire.consumers.iter().map(|row| row.id.as_str()))?;
    let purposes = unique_ids("purpose", wire.purposes.iter().map(|row| row.id.as_str()))?;
    let _producers = unique_ids("producer", wire.producers.iter().map(|row| row.id.as_str()))?;
    let identities = unique_ids(
        "identity",
        wire.identities.iter().map(|row| row.id.as_str()),
    )?;
    let facts = unique_ids("fact", wire.facts.iter().map(|row| row.id.as_str()))?;

    ensure!(sinks.contains("machine"), "missing machine sink");
    ensure!(sinks.contains("observation"), "missing observation sink");
    for sink in &wire.sinks {
        ensure!(
            (sink.id == "machine" && sink.startup_recovery_reads)
                || (sink.id == "observation" && !sink.startup_recovery_reads),
            "sink `{}` has invalid startup recovery classification",
            sink.id
        );
    }

    for consumer in &wire.consumers {
        ensure!(
            owners.contains(consumer.owner.as_str()),
            "consumer `{}` references unknown owner `{}`",
            consumer.id,
            consumer.owner
        );
        ensure!(
            CONSUMER_MODES.contains(&consumer.mode.as_str()),
            "consumer `{}` has unknown mode `{}`",
            consumer.id,
            consumer.mode
        );
    }
    ensure!(
        wire.consumers
            .iter()
            .any(|consumer| consumer.mode == "startup_recovery"),
        "missing startup recovery consumer"
    );

    let identity_by_id: BTreeMap<_, _> = wire
        .identities
        .iter()
        .map(|row| (row.id.as_str(), row))
        .collect();
    let fact_by_id: BTreeMap<_, _> = wire
        .facts
        .iter()
        .map(|row| (row.id.as_str(), row))
        .collect();
    let mut identities_per_purpose: BTreeMap<&str, usize> = BTreeMap::new();
    let mut exact_pairs = BTreeSet::new();
    for identity in &wire.identities {
        ensure!(
            purposes.contains(identity.purpose.as_str()),
            "identity `{}` references unknown purpose `{}`",
            identity.id,
            identity.purpose
        );
        ensure!(
            !identity.kind.trim().is_empty(),
            "identity `{}` has empty kind",
            identity.id
        );
        ensure!(
            identity.schema_version > 0,
            "identity `{}` has zero schema_version",
            identity.id
        );
        ensure!(
            !identity.gate_id.trim().is_empty(),
            "identity `{}` has empty gate_id",
            identity.id
        );
        ensure!(
            exact_pairs.insert((identity.kind.as_str(), identity.schema_version)),
            "duplicate exact identity ({}, {})",
            identity.kind,
            identity.schema_version
        );
        *identities_per_purpose
            .entry(identity.purpose.as_str())
            .or_default() += 1;
    }

    let mut producers_per_purpose: BTreeMap<&str, usize> = BTreeMap::new();
    for producer in &wire.producers {
        ensure!(
            purposes.contains(producer.purpose.as_str()),
            "producer `{}` references unknown purpose `{}`",
            producer.id,
            producer.purpose
        );
        ensure!(
            CLASSIFICATIONS.contains(&producer.classification.as_str()),
            "producer `{}` has unknown classification `{}`",
            producer.id,
            producer.classification
        );
        // Reachability is what makes a classification consequential -- an
        // append path reachable only from startup cannot flood a per-tick
        // stream whatever it is classified as -- so an empty list is a census
        // row that decided nothing.
        ensure!(
            !producer.handler_reachability.is_empty(),
            "producer `{}` names no handler reachability",
            producer.id
        );
        for handler in &producer.handler_reachability {
            ensure!(
                HANDLER_REACHABILITY.contains(&handler.as_str()),
                "producer `{}` names unknown handler `{handler}`",
                producer.id
            );
        }
        ensure!(
            !producer.call_sites.is_empty(),
            "producer `{}` names no call site, so nothing ties its classification to code",
            producer.id
        );
        for site in &producer.call_sites {
            ensure!(
                site.split_once("::")
                    .is_some_and(|(path, function)| path.ends_with(".rs") && !function.is_empty()),
                "producer `{}` call site `{site}` is not `<path>.rs::<function>`",
                producer.id
            );
        }
        ensure!(
            !producer.repeat_semantics.is_empty(),
            "producer `{}` states no repeat semantics",
            producer.id
        );
        ensure!(
            !producer.dedupe_key_evidence.is_empty(),
            "producer `{}` states no dedupe-key evidence",
            producer.id
        );
        // Empty is the legal way to say "this producer postdates the census and
        // its row was derived directly". Anything else must name a row that
        // actually existed, or provenance is just a string.
        ensure!(
            producer.census_ancestor.is_empty()
                || CENSUS_PRODUCERS.contains(&producer.census_ancestor.as_str()),
            "producer `{}` claims census ancestor `{}`, which is not a row of the retired census",
            producer.id,
            producer.census_ancestor
        );
        *producers_per_purpose
            .entry(producer.purpose.as_str())
            .or_default() += 1;
    }

    for purpose in &wire.purposes {
        ensure!(
            owners.contains(purpose.owner.as_str()),
            "purpose `{}` references unknown owner `{}`",
            purpose.id,
            purpose.owner
        );
        ensure!(
            sinks.contains(purpose.sink.as_str()),
            "purpose `{}` references unknown sink `{}`",
            purpose.id,
            purpose.sink
        );
        ensure!(
            EFFECT_POLICIES.contains(&purpose.effect_policy.as_str()),
            "purpose `{}` references unknown effect policy `{}`",
            purpose.id,
            purpose.effect_policy
        );
        ensure!(
            NOVELTY_CAPABILITIES.contains(&purpose.novelty_capability.as_str()),
            "purpose `{}` has unsupported novelty capability `{}`",
            purpose.id,
            purpose.novelty_capability
        );
        ensure!(
            !purpose.duties.is_empty(),
            "purpose `{}` has no duties",
            purpose.id
        );

        let observation = purpose
            .duties
            .iter()
            .all(|duty| OBSERVATION_DUTIES.contains(&duty.as_str()));
        let machine = purpose
            .duties
            .iter()
            .all(|duty| MACHINE_DUTIES.contains(&duty.as_str()));
        ensure!(
            observation || machine,
            "purpose `{}` mixes or uses unknown duty classes",
            purpose.id
        );
        if observation {
            ensure!(
                purpose.sink == "observation",
                "observation purpose `{}` must use observation sink",
                purpose.id
            );
            ensure!(
                purpose.effect_policy == "observation_bounded_failure",
                "observation purpose `{}` must use observation_bounded_failure",
                purpose.id
            );
        } else {
            ensure!(
                purpose.sink == "machine",
                "machine purpose `{}` must use machine sink",
                purpose.id
            );
            ensure!(
                purpose.effect_policy != "observation_bounded_failure",
                "machine purpose `{}` cannot use observation_bounded_failure",
                purpose.id
            );
        }

        ensure!(
            producers_per_purpose
                .get(purpose.id.as_str())
                .copied()
                .unwrap_or_default()
                > 0,
            "purpose `{}` requires at least one producer",
            purpose.id
        );
        ensure!(
            identities_per_purpose.get(purpose.id.as_str()) == Some(&1),
            "purpose `{}` must own exactly one current identity",
            purpose.id
        );
        let identity = identity_by_id
            .get(purpose.current_identity.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "purpose `{}` references unknown current identity `{}`",
                    purpose.id,
                    purpose.current_identity
                )
            })?;
        ensure!(
            identity.purpose == purpose.id,
            "purpose `{}` selects identity owned by `{}`",
            purpose.id,
            identity.purpose
        );
        let fact = fact_by_id
            .get(purpose.current_fact.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "purpose `{}` references unknown current fact `{}`",
                    purpose.id,
                    purpose.current_fact
                )
            })?;
        ensure!(
            fact.identity == purpose.current_identity,
            "purpose `{}` current fact does not bind its current identity",
            purpose.id
        );
    }

    let mut facts_per_identity: BTreeMap<&str, usize> = BTreeMap::new();
    for fact in &wire.facts {
        ensure!(
            identities.contains(fact.identity.as_str()),
            "fact `{}` references unknown identity `{}`",
            fact.id,
            fact.identity
        );
        *facts_per_identity
            .entry(fact.identity.as_str())
            .or_default() += 1;
    }
    for identity in &wire.identities {
        ensure!(
            facts_per_identity.get(identity.id.as_str()) == Some(&1),
            "identity `{}` must bind exactly one current fact",
            identity.id
        );
    }

    let mut cells = BTreeSet::new();
    let mut relevant_per_consumer: BTreeMap<&str, usize> = BTreeMap::new();
    for disposition in &wire.dispositions {
        ensure!(
            facts.contains(disposition.fact.as_str()),
            "disposition references unknown fact `{}`",
            disposition.fact
        );
        ensure!(
            consumers.contains(disposition.consumer.as_str()),
            "disposition references unknown consumer `{}`",
            disposition.consumer
        );
        ensure!(
            cells.insert((disposition.fact.as_str(), disposition.consumer.as_str())),
            "duplicate fact-consumer disposition ({}, {})",
            disposition.fact,
            disposition.consumer
        );
        match disposition.action.as_str() {
            "relevant" => {
                ensure!(
                    disposition
                        .event_variant
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty()),
                    "relevant disposition ({}, {}) requires event_variant",
                    disposition.fact,
                    disposition.consumer
                );
                ensure!(
                    disposition.owner_ruling.is_none(),
                    "relevant disposition ({}, {}) cannot carry owner_ruling",
                    disposition.fact,
                    disposition.consumer
                );
                ensure!(
                    disposition.event_variant.as_deref() == Some(disposition.fact.as_str()),
                    "relevant disposition ({}, {}) must preserve its fact",
                    disposition.fact,
                    disposition.consumer
                );
                *relevant_per_consumer
                    .entry(disposition.consumer.as_str())
                    .or_default() += 1;
            }
            "irrelevant" => {
                ensure!(
                    disposition
                        .owner_ruling
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty()),
                    "irrelevant disposition ({}, {}) requires owner_ruling",
                    disposition.fact,
                    disposition.consumer
                );
                ensure!(
                    disposition.event_variant.is_none(),
                    "irrelevant disposition ({}, {}) cannot carry event_variant",
                    disposition.fact,
                    disposition.consumer
                );
            }
            other => {
                return Err(anyhow!(
                    "disposition ({}, {}) has unknown action `{other}`",
                    disposition.fact,
                    disposition.consumer
                ));
            }
        }
    }
    for fact in &wire.facts {
        for consumer in &wire.consumers {
            ensure!(
                cells.contains(&(fact.id.as_str(), consumer.id.as_str())),
                "missing fact-consumer disposition ({}, {})",
                fact.id,
                consumer.id
            );
        }
    }
    for consumer in &wire.consumers {
        ensure!(
            relevant_per_consumer
                .get(consumer.id.as_str())
                .copied()
                .unwrap_or_default()
                > 0,
            "consumer `{}` has no relevant facts",
            consumer.id
        );
    }
    Ok(())
}

fn unique_ids<'a>(label: &str, ids: impl Iterator<Item = &'a str>) -> Result<BTreeSet<&'a str>> {
    let mut unique = BTreeSet::new();
    for id in ids {
        ensure!(!id.trim().is_empty(), "{label} id must be nonempty");
        ensure!(unique.insert(id), "duplicate {label} id `{id}`");
    }
    ensure!(
        !unique.is_empty(),
        "contract must register at least one {label}"
    );
    Ok(unique)
}

pub fn render_contract(contract: &ContractRegistry) -> String {
    let wire = &contract.wire;
    let mut output =
        String::from("// Generated by generate_decision_evidence_contract; do not edit.\n\n");
    render_enum(
        &mut output,
        "KnownProducer",
        "pub(crate)",
        wire.producers.iter().map(|row| row.id.as_str()),
    );
    render_enum(
        &mut output,
        "KnownPurpose",
        "pub",
        wire.purposes.iter().map(|row| row.id.as_str()),
    );
    render_enum(
        &mut output,
        "KnownIdentity",
        "pub(crate)",
        wire.identities.iter().map(|row| row.id.as_str()),
    );
    output.push_str("#[cfg(test)]\npub(crate) const ALL_IDENTITIES: &[KnownIdentity] = &[\n");
    for identity in &wire.identities {
        output.push_str(&format!(
            "    KnownIdentity::{},\n",
            rust_variant(&identity.id)
        ));
    }
    output.push_str("];\n\n");
    output.push_str("pub(crate) mod identities {\n");
    for identity in &wire.identities {
        output.push_str(&format!(
            "    #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n    pub(crate) struct {};\n",
            rust_variant(&identity.id)
        ));
    }
    output.push_str("}\n\n");
    render_enum(
        &mut output,
        "KnownFact",
        "pub(crate)",
        wire.facts.iter().map(|row| row.id.as_str()),
    );
    render_enum(
        &mut output,
        "KnownConsumer",
        "pub(crate)",
        wire.consumers.iter().map(|row| row.id.as_str()),
    );
    render_enum(
        &mut output,
        "KnownSink",
        "pub(crate)",
        wire.sinks.iter().map(|row| row.id.as_str()),
    );
    render_enum(
        &mut output,
        "EffectPolicy",
        "pub(crate)",
        EFFECT_POLICIES.iter().copied(),
    );

    output.push_str(concat!(
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n",
        "pub(crate) struct IdentityDescriptor {\n",
        "    pub(crate) kind: &'static str,\n",
        "    pub(crate) schema_version: u32,\n",
        "    pub(crate) gate_id: &'static str,\n",
        "}\n\n",
    ));

    output.push_str(concat!(
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n",
        "pub(crate) enum ConsumerDisposition {\n",
        "    Relevant(KnownFact),\n",
        "    Irrelevant(&'static str),\n",
        "}\n\n",
    ));

    render_purpose_match(
        &mut output,
        "current_identity_for_purpose",
        "KnownIdentity",
        wire,
        |row| row.current_identity.as_str(),
    );
    render_purpose_match(&mut output, "sink_for_purpose", "KnownSink", wire, |row| {
        row.sink.as_str()
    });
    render_purpose_match(
        &mut output,
        "effect_policy_for_purpose",
        "EffectPolicy",
        wire,
        |row| row.effect_policy.as_str(),
    );

    output.push_str(concat!(
        "pub(crate) const fn purpose_for_producer(producer: KnownProducer) -> KnownPurpose {\n",
        "    match producer {\n",
    ));
    for row in &wire.producers {
        render_match_arm(
            &mut output,
            "KnownProducer",
            &row.id,
            "KnownPurpose",
            &row.purpose,
        );
    }
    output.push_str("    }\n}\n\n");

    output.push_str(concat!(
        "pub(crate) const fn purpose_for_identity(identity: KnownIdentity) -> KnownPurpose {\n",
        "    match identity {\n",
    ));
    for row in &wire.identities {
        render_match_arm(
            &mut output,
            "KnownIdentity",
            &row.id,
            "KnownPurpose",
            &row.purpose,
        );
    }
    output.push_str("    }\n}\n\n");

    output.push_str(concat!(
        "pub(crate) const fn fact_for_identity(identity: KnownIdentity) -> KnownFact {\n",
        "    match identity {\n",
    ));
    for row in &wire.facts {
        render_match_arm(
            &mut output,
            "KnownIdentity",
            &row.identity,
            "KnownFact",
            &row.id,
        );
    }
    output.push_str("    }\n}\n\n");

    output.push_str(concat!(
        "pub(crate) const fn descriptor_for_identity(identity: KnownIdentity) -> IdentityDescriptor {\n",
        "    match identity {\n",
    ));
    for row in &wire.identities {
        output.push_str(&format!(
            "        KnownIdentity::{} => IdentityDescriptor {{\n            kind: {:?},\n            schema_version: {},\n            gate_id: {:?},\n        }},\n",
            rust_variant(&row.id),
            row.kind,
            row.schema_version,
            row.gate_id
        ));
    }
    output.push_str("    }\n}\n\n");

    output.push_str(concat!(
        "pub(crate) const fn disposition_for(\n",
        "    fact: KnownFact,\n",
        "    consumer: KnownConsumer,\n",
        ") -> ConsumerDisposition {\n",
        "    match (fact, consumer) {\n",
    ));
    for row in &wire.dispositions {
        let fact = rust_variant(&row.fact);
        let consumer = rust_variant(&row.consumer);
        let header = format!("        (KnownFact::{fact}, KnownConsumer::{consumer})");
        let disposition = if row.action == "relevant" {
            format!(
                "ConsumerDisposition::Relevant(KnownFact::{})",
                rust_variant(
                    row.event_variant
                        .as_deref()
                        .expect("validated relevant event")
                )
            )
        } else {
            format!(
                "ConsumerDisposition::Irrelevant({:?})",
                row.owner_ruling
                    .as_deref()
                    .expect("validated irrelevant ruling")
            )
        };
        if header.chars().count() + " => {".chars().count() <= 100 {
            output.push_str(&format!(
                "{header} => {{\n            {disposition}\n        }}\n"
            ));
        } else {
            output.push_str(&format!(
                "        (\n            KnownFact::{fact},\n            KnownConsumer::{consumer},\n        ) => {disposition},\n"
            ));
        }
    }
    output.push_str("    }\n}\n\n");

    output.push_str("pub(crate) fn resolve_identity(kind: &str, schema_version: u32) -> Option<KnownIdentity> {\n");
    for row in &wire.identities {
        output.push_str(&format!(
            "    if kind == {:?} && schema_version == {} {{\n        return Some(KnownIdentity::{});\n    }}\n",
            row.kind,
            row.schema_version,
            rust_variant(&row.id)
        ));
    }
    output.push_str("    None\n}\n");
    output
}

fn render_enum<'a>(
    output: &mut String,
    name: &str,
    visibility: &str,
    ids: impl Iterator<Item = &'a str>,
) {
    output.push_str(&format!(
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\n{visibility} enum {name} {{\n"
    ));
    for id in ids {
        output.push_str(&format!("    {},\n", rust_variant(id)));
    }
    output.push_str("}\n\n");
}

fn render_purpose_match(
    output: &mut String,
    function: &str,
    return_type: &str,
    wire: &RegistryWire,
    value: impl Fn(&PurposeRow) -> &str,
) {
    output.push_str(&format!("pub(crate) const fn {function}(purpose: KnownPurpose) -> {return_type} {{\n    match purpose {{\n"));
    for row in &wire.purposes {
        render_match_arm(output, "KnownPurpose", &row.id, return_type, value(row));
    }
    output.push_str("    }\n}\n\n");
}

fn render_match_arm(
    output: &mut String,
    input_type: &str,
    input_id: &str,
    output_type: &str,
    output_id: &str,
) {
    let input = rust_variant(input_id);
    let target = rust_variant(output_id);
    let arm = format!("        {input_type}::{input} => {output_type}::{target},\n");
    if arm.trim_end().chars().count() <= 100 {
        output.push_str(&arm);
    } else {
        output.push_str(&format!(
            "        {input_type}::{input} => {{\n            {output_type}::{target}\n        }}\n"
        ));
    }
}

fn rust_variant(id: &str) -> String {
    id.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect()
}
