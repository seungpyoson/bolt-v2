use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Deserialize;

const CONTRACT_SCHEMA_VERSION: u32 = 1;
const FACT_DUTIES: &[&str] = &["action", "join", "reconciliation", "recovery"];
const OBSERVATION_DUTIES: &[&str] = &["diagnostic_observation", "state_observation"];
const EFFECT_POLICIES: &[&str] = &[
    "must_precede_new_risk",
    "observation_bounded_failure",
    "preserve_result",
    "reconciliation_fail_closed",
    "risk_reducing_continues",
];

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRow {
    id: String,
    issue: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SinkRow {
    id: String,
    config_path: String,
    startup_recovery_reads: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumerRow {
    id: String,
    mode: String,
    owner: String,
    source_anchor: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProducerRow {
    id: String,
    purpose: String,
    source_anchors: Vec<String>,
    handler: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityRow {
    id: String,
    kind: String,
    schema_version: u32,
    gate_id: String,
    purpose: String,
    fact_ids: Vec<String>,
    payload_member: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactRow {
    id: String,
    owner: String,
    dispositions: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ContractRegistry {
    schema_version: u32,
    owners: Vec<OwnerRow>,
    sinks: Vec<SinkRow>,
    consumers: Vec<ConsumerRow>,
    purposes: Vec<PurposeRow>,
    producers: Vec<ProducerRow>,
    identities: Vec<IdentityRow>,
    facts: Vec<FactRow>,
}

impl From<RegistryWire> for ContractRegistry {
    fn from(wire: RegistryWire) -> Self {
        Self {
            schema_version: wire.schema_version,
            owners: wire.owners,
            sinks: wire.sinks,
            consumers: wire.consumers,
            purposes: wire.purposes,
            producers: wire.producers,
            identities: wire.identities,
            facts: wire.facts,
        }
    }
}

pub fn parse_contract_registry(source: &str) -> Result<ContractRegistry> {
    let wire: RegistryWire =
        toml::from_str(source).context("failed to parse decision-evidence contract TOML")?;
    let registry = ContractRegistry::from(wire);
    registry.validate()?;
    Ok(registry)
}

impl ContractRegistry {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CONTRACT_SCHEMA_VERSION,
            "decision-evidence contract schema_version must equal {CONTRACT_SCHEMA_VERSION}"
        );
        ensure!(!self.owners.is_empty(), "contract must register an owner");
        ensure!(!self.sinks.is_empty(), "contract must register a sink");
        ensure!(
            !self.consumers.is_empty(),
            "contract must register a consumer"
        );
        ensure!(
            !self.purposes.is_empty(),
            "contract must register a purpose"
        );
        ensure!(
            !self.producers.is_empty(),
            "contract must register a producer"
        );
        ensure!(
            !self.identities.is_empty(),
            "contract must register an identity"
        );
        ensure!(!self.facts.is_empty(), "contract must register a fact");

        validate_unique_ids("owner", self.owners.iter().map(|row| row.id.as_str()))?;
        validate_unique_ids("sink", self.sinks.iter().map(|row| row.id.as_str()))?;
        validate_unique_ids("consumer", self.consumers.iter().map(|row| row.id.as_str()))?;
        validate_unique_ids("purpose", self.purposes.iter().map(|row| row.id.as_str()))?;
        validate_unique_ids("producer", self.producers.iter().map(|row| row.id.as_str()))?;
        validate_unique_ids(
            "identity",
            self.identities.iter().map(|row| row.id.as_str()),
        )?;
        validate_unique_ids("fact", self.facts.iter().map(|row| row.id.as_str()))?;

        let owners = ids(self.owners.iter().map(|row| row.id.as_str()));
        let sinks = ids(self.sinks.iter().map(|row| row.id.as_str()));
        let consumers = ids(self.consumers.iter().map(|row| row.id.as_str()));
        let purposes = ids(self.purposes.iter().map(|row| row.id.as_str()));
        let identities = ids(self.identities.iter().map(|row| row.id.as_str()));
        let facts = ids(self.facts.iter().map(|row| row.id.as_str()));

        validate_pascal_collisions("consumer", consumers.iter().copied())?;
        validate_pascal_collisions("purpose", purposes.iter().copied())?;
        validate_pascal_collisions("producer", self.producers.iter().map(|row| row.id.as_str()))?;
        validate_pascal_collisions("identity", identities.iter().copied())?;
        validate_pascal_collisions("fact", facts.iter().copied())?;
        validate_pascal_collisions("sink", sinks.iter().copied())?;

        let mut config_paths = BTreeSet::new();
        for row in &self.sinks {
            validate_id("sink", &row.id)?;
            ensure!(
                !row.config_path.trim().is_empty(),
                "sink `{}` has an empty config_path",
                row.id
            );
            ensure!(
                config_paths.insert(row.config_path.as_str()),
                "sink config_path `{}` is duplicated",
                row.config_path
            );
        }

        for row in &self.consumers {
            validate_id("consumer", &row.id)?;
            ensure!(
                owners.contains(row.owner.as_str()),
                "consumer `{}` references unknown owner `{}`",
                row.id,
                row.owner
            );
            ensure!(
                matches!(
                    row.mode.as_str(),
                    "startup_recovery" | "offline_projection" | "query_api"
                ),
                "consumer `{}` has unknown mode `{}`",
                row.id,
                row.mode
            );
            ensure!(
                !row.source_anchor.trim().is_empty(),
                "consumer `{}` has an empty source_anchor",
                row.id
            );
        }

        let identity_by_id = map_by_id(&self.identities, |row| row.id.as_str());
        let fact_by_id = map_by_id(&self.facts, |row| row.id.as_str());
        for row in &self.purposes {
            validate_id("purpose", &row.id)?;
            ensure!(
                owners.contains(row.owner.as_str()),
                "purpose `{}` references unknown owner `{}`",
                row.id,
                row.owner
            );
            ensure!(
                sinks.contains(row.sink.as_str()),
                "purpose `{}` references unknown sink `{}`",
                row.id,
                row.sink
            );
            ensure!(
                EFFECT_POLICIES.contains(&row.effect_policy.as_str()),
                "purpose `{}` has unknown effect_policy `{}`",
                row.id,
                row.effect_policy
            );
            ensure!(
                row.novelty_capability == "prohibited",
                "purpose `{}` grants unsupported novelty capability `{}`",
                row.id,
                row.novelty_capability
            );
            validate_duties(row)?;
            let identity = identity_by_id
                .get(row.current_identity.as_str())
                .ok_or_else(|| {
                    anyhow!(
                        "purpose `{}` references unknown current identity `{}`",
                        row.id,
                        row.current_identity
                    )
                })?;
            ensure!(
                identity.purpose == row.id,
                "purpose `{}` selects identity owned by purpose `{}`",
                row.id,
                identity.purpose
            );
            let fact = fact_by_id.get(row.current_fact.as_str()).ok_or_else(|| {
                anyhow!(
                    "purpose `{}` references unknown current fact `{}`",
                    row.id,
                    row.current_fact
                )
            })?;
            ensure!(
                identity.fact_ids == [fact.id.clone()],
                "current identity `{}` must produce exactly current fact `{}`",
                identity.id,
                fact.id
            );
        }

        for row in &self.producers {
            validate_id("producer", &row.id)?;
            ensure!(
                purposes.contains(row.purpose.as_str()),
                "producer `{}` references unknown purpose `{}`",
                row.id,
                row.purpose
            );
            ensure!(
                !row.source_anchors.is_empty(),
                "producer `{}` has no source anchors",
                row.id
            );
            ensure!(
                row.source_anchors
                    .iter()
                    .all(|anchor| !anchor.trim().is_empty()),
                "producer `{}` contains an empty source anchor",
                row.id
            );
            ensure!(
                !row.handler.trim().is_empty(),
                "producer `{}` has an empty handler",
                row.id
            );
        }

        let mut exact_pairs = BTreeSet::new();
        let mut current_by_purpose = BTreeMap::<&str, usize>::new();
        for row in &self.identities {
            validate_id("identity", &row.id)?;
            ensure!(
                purposes.contains(row.purpose.as_str()),
                "identity `{}` references unknown purpose `{}`",
                row.id,
                row.purpose
            );
            ensure!(
                !row.kind.trim().is_empty(),
                "identity `{}` has empty kind",
                row.id
            );
            ensure!(
                row.schema_version > 0,
                "identity `{}` has non-positive schema_version",
                row.id
            );
            ensure!(
                !row.gate_id.trim().is_empty(),
                "identity `{}` has empty gate_id",
                row.id
            );
            ensure!(
                !row.payload_member.trim().is_empty(),
                "identity `{}` has empty payload_member",
                row.id
            );
            ensure!(
                !row.fact_ids.is_empty(),
                "identity `{}` has no facts",
                row.id
            );
            ensure!(
                row.fact_ids
                    .iter()
                    .all(|fact| facts.contains(fact.as_str())),
                "identity `{}` references an unknown fact",
                row.id
            );
            ensure!(
                exact_pairs.insert((row.kind.as_str(), row.schema_version)),
                "duplicate exact identity pair (`{}`, {})",
                row.kind,
                row.schema_version
            );
            *current_by_purpose.entry(row.purpose.as_str()).or_default() += 1;
        }
        for purpose in &self.purposes {
            ensure!(
                current_by_purpose.get(purpose.id.as_str()) == Some(&1),
                "purpose `{}` must have exactly one current identity",
                purpose.id
            );
        }

        let expected_consumers = consumers;
        for row in &self.facts {
            validate_id("fact", &row.id)?;
            ensure!(
                owners.contains(row.owner.as_str()),
                "fact `{}` references unknown owner `{}`",
                row.id,
                row.owner
            );
            let actual_consumers = ids(row.dispositions.keys().map(String::as_str));
            ensure!(
                actual_consumers == expected_consumers,
                "fact `{}` must explicitly disposition every consumer",
                row.id
            );
            for (consumer, disposition) in &row.dispositions {
                validate_disposition(&row.id, consumer, disposition)?;
            }
        }

        Ok(())
    }
}

fn validate_duties(row: &PurposeRow) -> Result<()> {
    ensure!(!row.duties.is_empty(), "purpose `{}` has no duties", row.id);
    let duties = ids(row.duties.iter().map(String::as_str));
    ensure!(
        duties.len() == row.duties.len(),
        "purpose `{}` has duplicate duties",
        row.id
    );
    for duty in &duties {
        ensure!(
            FACT_DUTIES.contains(duty) || OBSERVATION_DUTIES.contains(duty),
            "purpose `{}` has unknown duty `{duty}`",
            row.id
        );
    }
    let fact = duties.iter().any(|duty| FACT_DUTIES.contains(duty));
    let observation = duties.iter().any(|duty| OBSERVATION_DUTIES.contains(duty));
    ensure!(
        !(fact && observation),
        "purpose `{}` mixes fact and observation duties",
        row.id
    );
    Ok(())
}

fn validate_disposition(fact: &str, consumer: &str, disposition: &str) -> Result<()> {
    if let Some(event) = disposition.strip_prefix("relevant:") {
        ensure!(
            !event.is_empty(),
            "fact `{fact}` has empty relevant event for consumer `{consumer}`"
        );
        validate_rust_variant("event", event)?;
        return Ok(());
    }
    if let Some(ruling) = disposition.strip_prefix("irrelevant:") {
        ensure!(
            !ruling.trim().is_empty(),
            "fact `{fact}` has empty irrelevance ruling for consumer `{consumer}`"
        );
        return Ok(());
    }
    bail!("fact `{fact}` has invalid disposition `{disposition}` for consumer `{consumer}`")
}

fn validate_unique_ids<'a>(label: &str, values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        validate_id(label, value)?;
        ensure!(seen.insert(value), "duplicate {label} id `{value}`");
    }
    Ok(())
}

fn validate_id(label: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} id must not be empty");
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
        "{label} id `{value}` must use lowercase snake_case"
    );
    ensure!(
        !value.as_bytes()[0].is_ascii_digit(),
        "{label} id `{value}` must not start with a digit"
    );
    Ok(())
}

fn validate_rust_variant(label: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} variant must not be empty");
    ensure!(
        value.as_bytes()[0].is_ascii_uppercase(),
        "{label} variant `{value}` must start uppercase"
    );
    ensure!(
        value.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "{label} variant `{value}` must be alphanumeric"
    );
    Ok(())
}

fn validate_pascal_collisions<'a>(
    label: &str,
    values: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let mut rendered = BTreeMap::new();
    for value in values {
        let variant = pascal_case(value);
        if let Some(previous) = rendered.insert(variant.clone(), value) {
            bail!("{label} ids `{previous}` and `{value}` both render as `{variant}`");
        }
    }
    Ok(())
}

fn ids<'a>(values: impl Iterator<Item = &'a str>) -> BTreeSet<&'a str> {
    values.collect()
}

fn map_by_id<'a, T>(rows: &'a [T], id: impl Fn(&'a T) -> &'a str) -> BTreeMap<&'a str, &'a T> {
    rows.iter().map(|row| (id(row), row)).collect()
}

fn pascal_case(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

pub fn render_contract_rust(registry: &ContractRegistry) -> Result<String> {
    registry.validate()?;
    let mut output = String::new();
    writeln!(
        output,
        "// Generated by `generate_decision_evidence_contract`; do not edit by hand."
    )?;
    writeln!(output)?;
    render_enum(
        &mut output,
        "KnownProducer",
        registry.producers.iter().map(|row| row.id.as_str()),
    )?;
    render_enum(
        &mut output,
        "KnownPurpose",
        registry.purposes.iter().map(|row| row.id.as_str()),
    )?;
    render_enum(
        &mut output,
        "KnownIdentity",
        registry.identities.iter().map(|row| row.id.as_str()),
    )?;
    render_enum(
        &mut output,
        "KnownDecodedFact",
        registry.facts.iter().map(|row| row.id.as_str()),
    )?;
    render_enum(
        &mut output,
        "KnownConsumer",
        registry.consumers.iter().map(|row| row.id.as_str()),
    )?;
    render_enum(
        &mut output,
        "KnownSink",
        registry.sinks.iter().map(|row| row.id.as_str()),
    )?;

    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub(crate) enum EffectPolicy {{")?;
    for policy in EFFECT_POLICIES {
        writeln!(output, "    {},", pascal_case(policy))?;
    }
    writeln!(output, "}}")?;
    writeln!(output)?;

    let events = registry
        .facts
        .iter()
        .flat_map(|fact| fact.dispositions.values())
        .filter_map(|value| value.strip_prefix("relevant:"))
        .collect::<BTreeSet<_>>();
    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub(crate) enum KnownEventVariant {{")?;
    for event in events {
        writeln!(output, "    {event},")?;
    }
    writeln!(output, "}}")?;
    writeln!(output)?;
    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub(crate) enum ConsumerDisposition {{")?;
    writeln!(output, "    Relevant(KnownEventVariant),")?;
    writeln!(output, "    Irrelevant,")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    render_marker_module(
        &mut output,
        "producer_markers",
        "ProducerMarker",
        registry.producers.iter().map(|row| row.id.as_str()),
    )?;
    render_marker_module(
        &mut output,
        "purpose_markers",
        "PurposeMarker",
        registry.purposes.iter().map(|row| row.id.as_str()),
    )?;
    render_marker_module(
        &mut output,
        "identity_markers",
        "IdentityMarker",
        registry.identities.iter().map(|row| row.id.as_str()),
    )?;
    render_marker_module(
        &mut output,
        "consumer_markers",
        "ConsumerMarker",
        registry.consumers.iter().map(|row| row.id.as_str()),
    )?;

    writeln!(
        output,
        "pub(crate) fn resolve_identity(kind: &str, schema_version: u32) -> anyhow::Result<KnownIdentity> {{"
    )?;
    writeln!(output, "    match (kind, schema_version) {{")?;
    for row in &registry.identities {
        writeln!(
            output,
            "        ({:?}, {}) => Ok(KnownIdentity::{}),",
            row.kind,
            row.schema_version,
            pascal_case(&row.id)
        )?;
    }
    writeln!(
        output,
        "        _ => Err(anyhow::anyhow!(\"unregistered decision-evidence identity (`{{kind}}`, {{schema_version}})\")),"
    )?;
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn purpose_for_producer(producer: KnownProducer) -> KnownPurpose {{"
    )?;
    writeln!(output, "    match producer {{")?;
    for row in &registry.producers {
        writeln!(
            output,
            "        KnownProducer::{} => KnownPurpose::{},",
            pascal_case(&row.id),
            pascal_case(&row.purpose)
        )?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn current_identity_for_purpose(purpose: KnownPurpose) -> KnownIdentity {{"
    )?;
    writeln!(output, "    match purpose {{")?;
    for row in &registry.purposes {
        writeln!(
            output,
            "        KnownPurpose::{} => KnownIdentity::{},",
            pascal_case(&row.id),
            pascal_case(&row.current_identity)
        )?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn sink_for_purpose(purpose: KnownPurpose) -> KnownSink {{"
    )?;
    writeln!(output, "    match purpose {{")?;
    for row in &registry.purposes {
        writeln!(
            output,
            "        KnownPurpose::{} => KnownSink::{},",
            pascal_case(&row.id),
            pascal_case(&row.sink)
        )?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn effect_policy_for_purpose(purpose: KnownPurpose) -> EffectPolicy {{"
    )?;
    writeln!(output, "    match purpose {{")?;
    for row in &registry.purposes {
        writeln!(
            output,
            "        KnownPurpose::{} => EffectPolicy::{},",
            pascal_case(&row.id),
            pascal_case(&row.effect_policy)
        )?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn facts_for_identity(identity: KnownIdentity) -> &'static [KnownDecodedFact] {{"
    )?;
    writeln!(output, "    match identity {{")?;
    for row in &registry.identities {
        let rendered = row
            .fact_ids
            .iter()
            .map(|fact| format!("KnownDecodedFact::{}", pascal_case(fact)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "        KnownIdentity::{} => &[{}],",
            pascal_case(&row.id),
            rendered
        )?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn identity_metadata(identity: KnownIdentity) -> (&'static str, u32, &'static str, &'static str) {{"
    )?;
    writeln!(output, "    match identity {{")?;
    for row in &registry.identities {
        writeln!(
            output,
            "        KnownIdentity::{} => ({:?}, {}, {:?}, {:?}),",
            pascal_case(&row.id),
            row.kind,
            row.schema_version,
            row.gate_id,
            row.payload_member
        )?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    writeln!(
        output,
        "pub(crate) const fn disposition(fact: KnownDecodedFact, consumer: KnownConsumer) -> ConsumerDisposition {{"
    )?;
    writeln!(output, "    match (fact, consumer) {{")?;
    for fact in &registry.facts {
        for consumer in &registry.consumers {
            let disposition = fact
                .dispositions
                .get(&consumer.id)
                .ok_or_else(|| anyhow!("missing disposition while rendering"))?;
            let rendered = if let Some(event) = disposition.strip_prefix("relevant:") {
                format!("ConsumerDisposition::Relevant(KnownEventVariant::{event})")
            } else {
                "ConsumerDisposition::Irrelevant".to_string()
            };
            writeln!(
                output,
                "        (KnownDecodedFact::{}, KnownConsumer::{}) => {},",
                pascal_case(&fact.id),
                pascal_case(&consumer.id),
                rendered
            )?;
        }
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    format_generated_rust(output)
}

fn format_generated_rust(source: String) -> Result<String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start rustfmt for generated decision-evidence contract")?;
    child
        .stdin
        .take()
        .context("rustfmt stdin was unavailable")?
        .write_all(source.as_bytes())
        .context("failed to write generated decision-evidence contract to rustfmt")?;
    let formatted = child
        .wait_with_output()
        .context("failed to wait for rustfmt")?;
    ensure!(
        formatted.status.success(),
        "rustfmt rejected generated decision-evidence contract: {}",
        String::from_utf8_lossy(&formatted.stderr)
    );
    String::from_utf8(formatted.stdout)
        .context("rustfmt returned non-UTF-8 decision-evidence contract bytes")
}

fn render_enum<'a>(
    output: &mut String,
    name: &str,
    values: impl Iterator<Item = &'a str>,
) -> Result<()> {
    writeln!(
        output,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]"
    )?;
    writeln!(output, "pub(crate) enum {name} {{")?;
    for value in values {
        writeln!(output, "    {},", pascal_case(value))?;
    }
    writeln!(output, "}}")?;
    writeln!(output)?;
    Ok(())
}

fn render_marker_module<'a>(
    output: &mut String,
    module: &str,
    trait_name: &str,
    values: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let values = values.collect::<Vec<_>>();
    writeln!(output, "mod {module}_sealed {{")?;
    writeln!(output, "    pub trait Sealed {{}}")?;
    writeln!(output, "}}")?;
    writeln!(
        output,
        "pub(crate) trait {trait_name}: {module}_sealed::Sealed {{}}"
    )?;
    writeln!(output, "pub(crate) mod {module} {{")?;
    for value in &values {
        writeln!(output, "    pub(crate) struct {};", pascal_case(value))?;
    }
    writeln!(output, "}}")?;
    for value in values {
        let variant = pascal_case(value);
        writeln!(
            output,
            "impl {module}_sealed::Sealed for {module}::{variant} {{}}"
        )?;
        writeln!(output, "impl {trait_name} for {module}::{variant} {{}}")?;
    }
    writeln!(output)?;
    Ok(())
}
