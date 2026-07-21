//! Deterministic generator for immutable decision-evidence record identities.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

const SUPPORTED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceIdentityRegistry {
    schema_version: u32,
    consumers: Vec<String>,
    decode_actions: Vec<String>,
    #[serde(rename = "identity")]
    identities: Vec<IdentityRegistration>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct IdentityRegistration {
    name: String,
    kind: String,
    schema_versions: Vec<u32>,
    rust_variant_prefix: String,
    rust_kind_constant: String,
    record_family: String,
    gate_id: String,
    decode_action: String,
    current_encoder_version: Option<u32>,
    consumers: Vec<String>,
}

pub fn parse_registry(text: &str) -> Result<EvidenceIdentityRegistry> {
    let registry = toml::from_str::<EvidenceIdentityRegistry>(text)
        .context("parsing decision-evidence identity TOML")?;
    registry.validate()?;
    Ok(registry)
}

pub fn validate_append_only_compatibility(
    frozen: &EvidenceIdentityRegistry,
    current: &EvidenceIdentityRegistry,
) -> Result<()> {
    frozen.validate()?;
    current.validate()?;

    let current_consumers = current.consumers.iter().collect::<BTreeSet<_>>();
    for consumer in &frozen.consumers {
        ensure!(
            current_consumers.contains(consumer),
            "registered recovery consumer {consumer} was removed"
        );
    }
    let current_actions = current.decode_actions.iter().collect::<BTreeSet<_>>();
    for action in &frozen.decode_actions {
        ensure!(
            current_actions.contains(action),
            "registered decode action {action} was removed"
        );
    }

    let current_by_name = current
        .identities
        .iter()
        .map(|identity| (identity.name.as_str(), identity))
        .collect::<BTreeMap<_, _>>();
    for frozen_identity in &frozen.identities {
        let current_identity = current_by_name
            .get(frozen_identity.name.as_str())
            .with_context(|| format!("frozen identity {} was removed", frozen_identity.name))?;
        ensure!(
            frozen_identity.kind == current_identity.kind
                && frozen_identity.rust_variant_prefix == current_identity.rust_variant_prefix
                && frozen_identity.rust_kind_constant == current_identity.rust_kind_constant
                && frozen_identity.record_family == current_identity.record_family
                && frozen_identity.gate_id == current_identity.gate_id
                && frozen_identity.decode_action == current_identity.decode_action
                && frozen_identity.consumers == current_identity.consumers,
            "frozen identity {} changed immutable metadata",
            frozen_identity.name
        );
        let current_versions = current_identity
            .schema_versions
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        for version in &frozen_identity.schema_versions {
            ensure!(
                current_versions.contains(version),
                "frozen identity pair ({}, {}) was removed",
                frozen_identity.kind,
                version
            );
        }
    }

    let frozen_pairs = frozen
        .identities
        .iter()
        .flat_map(|identity| {
            identity
                .schema_versions
                .iter()
                .map(move |version| (identity.kind.as_str(), *version))
        })
        .collect::<BTreeSet<_>>();
    let frozen_encoders = current_encoders_by_family(frozen)?;
    let current_encoders = current_encoders_by_family(current)?;
    for (family, frozen_pair) in frozen_encoders {
        let current_pair = current_encoders
            .get(family)
            .with_context(|| format!("record family {family} lost its current encoder"))?;
        ensure!(
            *current_pair == frozen_pair || !frozen_pairs.contains(current_pair),
            "record family {family} moved its current encoder to historical identity ({}, {})",
            current_pair.0,
            current_pair.1
        );
    }
    Ok(())
}

fn current_encoders_by_family(
    registry: &EvidenceIdentityRegistry,
) -> Result<BTreeMap<&str, (&str, u32)>> {
    let mut encoders = BTreeMap::new();
    for identity in &registry.identities {
        if let Some(version) = identity.current_encoder_version {
            ensure!(
                encoders
                    .insert(
                        identity.record_family.as_str(),
                        (identity.kind.as_str(), version),
                    )
                    .is_none(),
                "record family {} has multiple current encoders",
                identity.record_family
            );
        }
    }
    Ok(encoders)
}

impl EvidenceIdentityRegistry {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SUPPORTED_SCHEMA_VERSION,
            "decision-evidence identity schema_version must be {SUPPORTED_SCHEMA_VERSION}"
        );
        ensure!(
            !self.consumers.is_empty(),
            "consumer domain must not be empty"
        );
        ensure!(
            !self.decode_actions.is_empty(),
            "decode-action domain must not be empty"
        );
        ensure!(
            !self.identities.is_empty(),
            "identity registry must not be empty"
        );

        let consumers = validate_unique_snake_values(&self.consumers, "consumer")?;
        let actions = validate_unique_snake_values(&self.decode_actions, "decode_action")?;
        let mut names = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        let mut variants = BTreeSet::new();
        let mut current_families = BTreeSet::new();
        let mut constants = BTreeMap::<&str, &str>::new();

        for identity in &self.identities {
            validate_snake(&identity.name, "identity.name")?;
            validate_wire_token(&identity.kind, "identity.kind")?;
            validate_rust_identifier(
                &identity.rust_variant_prefix,
                "identity.rust_variant_prefix",
            )?;
            validate_rust_identifier(&identity.rust_kind_constant, "identity.rust_kind_constant")?;
            validate_snake(&identity.record_family, "identity.record_family")?;
            validate_wire_token(&identity.gate_id, "identity.gate_id")?;
            ensure!(
                names.insert(identity.name.as_str()),
                "duplicate identity name {}",
                identity.name
            );
            ensure!(
                !identity.schema_versions.is_empty(),
                "identity {} has no schema versions",
                identity.name
            );
            ensure!(
                actions.contains(identity.decode_action.as_str()),
                "identity {} references unknown decode action {}",
                identity.name,
                identity.decode_action
            );
            for consumer in &identity.consumers {
                ensure!(
                    consumers.contains(consumer.as_str()),
                    "identity {} references unknown consumer {}",
                    identity.name,
                    consumer
                );
            }
            if let Some(existing) =
                constants.insert(identity.rust_kind_constant.as_str(), identity.kind.as_str())
            {
                ensure!(
                    existing == identity.kind,
                    "kind constant {} has conflicting values",
                    identity.rust_kind_constant
                );
            }

            let mut row_versions = BTreeSet::new();
            for version in &identity.schema_versions {
                ensure!(
                    *version > 0,
                    "identity {} has zero schema version",
                    identity.name
                );
                ensure!(
                    row_versions.insert(*version),
                    "identity {} repeats schema version {}",
                    identity.name,
                    version
                );
                ensure!(
                    pairs.insert((identity.kind.as_str(), *version)),
                    "duplicate identity pair ({}, {})",
                    identity.kind,
                    version
                );
                let variant = format!("{}V{}", identity.rust_variant_prefix, version);
                ensure!(
                    variants.insert(variant),
                    "duplicate generated identity variant"
                );
            }
            if let Some(current) = identity.current_encoder_version {
                ensure!(
                    row_versions.contains(&current),
                    "identity {} current encoder version is not registered",
                    identity.name
                );
                ensure!(
                    current_families.insert(identity.record_family.as_str()),
                    "record family {} has multiple current encoders",
                    identity.record_family
                );
            }
        }
        Ok(())
    }
}

fn validate_unique_snake_values<'a>(
    values: &'a [String],
    label: &str,
) -> Result<BTreeSet<&'a str>> {
    let mut unique = BTreeSet::new();
    for value in values {
        validate_snake(value, label)?;
        ensure!(unique.insert(value.as_str()), "duplicate {label} {value}");
    }
    Ok(unique)
}

fn validate_snake(value: &str, label: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    ensure!(
        value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        }),
        "{label} must be lowercase snake case"
    );
    Ok(())
}

fn validate_rust_identifier(value: &str, label: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    ensure!(
        value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        }),
        "{label} must be one Rust identifier"
    );
    Ok(())
}

fn validate_wire_token(value: &str, label: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')),
        "{label} contains unsupported bytes"
    );
    Ok(())
}

fn pascal(value: &str) -> String {
    value
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

pub fn render_registry(registry: &EvidenceIdentityRegistry) -> Result<String> {
    registry.validate()?;
    let mut output = String::new();
    writeln!(
        output,
        "// @generated from config/decision-evidence-identities.toml. Do not edit.\n"
    )?;
    writeln!(output, "use anyhow::{{Result, anyhow}};\n")?;

    let constants = registry
        .identities
        .iter()
        .map(|identity| (identity.rust_kind_constant.as_str(), identity.kind.as_str()))
        .collect::<BTreeMap<_, _>>();
    for (name, value) in constants {
        writeln!(output, "pub const {name}: &str = {value:?};")?;
    }
    writeln!(output)?;

    writeln!(
        output,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]"
    )?;
    writeln!(output, "pub enum EvidenceRecordIdentity {{")?;
    for identity in &registry.identities {
        for version in &identity.schema_versions {
            writeln!(output, "    {}V{},", identity.rust_variant_prefix, version)?;
        }
    }
    writeln!(output, "}}\n")?;

    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub enum EvidenceConsumer {{")?;
    for consumer in &registry.consumers {
        writeln!(output, "    {},", pascal(consumer))?;
    }
    writeln!(output, "}}\n")?;

    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub enum EvidenceDecodeAction {{")?;
    for action in &registry.decode_actions {
        writeln!(output, "    {},", pascal(action))?;
    }
    writeln!(output, "}}\n")?;

    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub struct EvidenceIdentityMetadata {{")?;
    writeln!(output, "    pub kind: &'static str,")?;
    writeln!(output, "    pub schema_version: u32,")?;
    writeln!(output, "    pub gate_id: &'static str,")?;
    writeln!(output, "    pub decode_action: EvidenceDecodeAction,")?;
    writeln!(output, "}}\n")?;

    writeln!(
        output,
        "pub fn resolve_evidence_record_identity(kind: &str, schema_version: u32) -> Result<EvidenceRecordIdentity> {{"
    )?;
    writeln!(output, "    match (kind, schema_version) {{")?;
    for identity in &registry.identities {
        for version in &identity.schema_versions {
            writeln!(
                output,
                "        ({:?}, {}) => Ok(EvidenceRecordIdentity::{}V{}),",
                identity.kind, version, identity.rust_variant_prefix, version
            )?;
        }
    }
    writeln!(
        output,
        "        _ => Err(anyhow!(\"unregistered decision-evidence identity kind={{kind:?}} schema_version={{schema_version}}\")),"
    )?;
    writeln!(output, "    }}")?;
    writeln!(output, "}}\n")?;

    writeln!(output, "impl EvidenceRecordIdentity {{")?;
    writeln!(output, "    #[must_use]")?;
    writeln!(
        output,
        "    pub const fn metadata(self) -> EvidenceIdentityMetadata {{"
    )?;
    writeln!(output, "        match self {{")?;
    for identity in &registry.identities {
        for version in &identity.schema_versions {
            writeln!(
                output,
                "            Self::{}V{} => EvidenceIdentityMetadata {{ kind: {:?}, schema_version: {}, gate_id: {:?}, decode_action: EvidenceDecodeAction::{} }},",
                identity.rust_variant_prefix,
                version,
                identity.kind,
                version,
                identity.gate_id,
                pascal(&identity.decode_action)
            )?;
        }
    }
    writeln!(output, "        }}")?;
    writeln!(output, "    }}")?;
    writeln!(output)?;
    writeln!(output, "    #[must_use]")?;
    writeln!(
        output,
        "    pub const fn decode_action_for(self, consumer: EvidenceConsumer) -> Option<EvidenceDecodeAction> {{"
    )?;
    writeln!(output, "        match (self, consumer) {{")?;
    for identity in &registry.identities {
        for version in &identity.schema_versions {
            for consumer in &identity.consumers {
                writeln!(
                    output,
                    "            (Self::{}V{}, EvidenceConsumer::{}) => Some(EvidenceDecodeAction::{}),",
                    identity.rust_variant_prefix,
                    version,
                    pascal(consumer),
                    pascal(&identity.decode_action)
                )?;
            }
        }
    }
    writeln!(output, "            _ => None,")?;
    writeln!(output, "        }}")?;
    writeln!(output, "    }}")?;
    for identity in registry
        .identities
        .iter()
        .filter(|identity| identity.current_encoder_version.is_some())
    {
        let version = identity
            .current_encoder_version
            .expect("filtered current encoder");
        writeln!(output)?;
        writeln!(output, "    #[must_use]")?;
        writeln!(
            output,
            "    pub const fn current_{}() -> Self {{",
            identity.record_family
        )?;
        writeln!(
            output,
            "        Self::{}V{}",
            identity.rust_variant_prefix, version
        )?;
        writeln!(output, "    }}")?;
    }
    writeln!(output, "}}")?;
    Ok(output)
}
