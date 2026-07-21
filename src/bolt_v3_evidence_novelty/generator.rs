//! Deterministic Rust generator for the closed evidence-novelty domain registry.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

const SUPPORTED_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceNoveltyRegistry {
    schema_version: u32,
    family: Family,
    #[serde(rename = "allocation")]
    allocations: Vec<Allocation>,
    #[serde(rename = "domain")]
    domains: Vec<Domain>,
    #[serde(rename = "producer")]
    producers: Vec<Producer>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Family {
    name: String,
    capacity: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Allocation {
    name: String,
    id_start: usize,
    id_end_exclusive: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Domain {
    name: String,
    rust_type: String,
    #[serde(default)]
    generated: bool,
    #[serde(rename = "variant")]
    variants: Vec<DomainVariant>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DomainVariant {
    name: String,
    #[serde(default = "registered_by_default")]
    registered: bool,
    #[serde(default)]
    payload_domains: Vec<String>,
    bool_value: Option<bool>,
    optional_bool_value: Option<String>,
}

const fn registered_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Producer {
    name: String,
    rust_marker: String,
    rust_key: String,
    owner: String,
    producer_kind: String,
    allocation: String,
    #[serde(rename = "dimension")]
    dimensions: Vec<Dimension>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DimensionShape {
    Scalar,
    Set,
    Optional,
    Opaque,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Dimension {
    id: usize,
    name: String,
    rust_field_type: String,
    domain: String,
    shape: DimensionShape,
    #[serde(default)]
    component_domains: Vec<String>,
    #[serde(default)]
    optional_component_domains: Vec<String>,
}

pub fn parse_registry(text: &str) -> Result<EvidenceNoveltyRegistry> {
    let registry: EvidenceNoveltyRegistry =
        toml::from_str(text).context("parsing evidence novelty TOML")?;
    registry.validate()?;
    Ok(registry)
}

impl EvidenceNoveltyRegistry {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SUPPORTED_SCHEMA_VERSION,
            "evidence novelty schema_version must be {SUPPORTED_SCHEMA_VERSION}"
        );
        validate_snake_identifier(&self.family.name, "family.name")?;
        ensure!(self.family.capacity > 0, "family capacity must be positive");
        ensure!(
            !self.allocations.is_empty(),
            "registry requires allocations"
        );
        ensure!(!self.domains.is_empty(), "registry requires domains");
        ensure!(!self.producers.is_empty(), "registry requires producers");

        let mut allocations = BTreeMap::new();
        let mut occupied_ids = BTreeSet::new();
        for allocation in &self.allocations {
            validate_snake_identifier(&allocation.name, "allocation.name")?;
            ensure!(
                allocation.id_start < allocation.id_end_exclusive,
                "allocation {} must have a non-empty range",
                allocation.name
            );
            ensure!(
                allocation.id_end_exclusive <= self.family.capacity,
                "allocation {} exceeds family capacity",
                allocation.name
            );
            ensure!(
                allocations
                    .insert(allocation.name.as_str(), allocation)
                    .is_none(),
                "duplicate allocation {}",
                allocation.name
            );
            for id in allocation.id_start..allocation.id_end_exclusive {
                ensure!(
                    occupied_ids.insert(id),
                    "allocation ranges overlap at id {id}"
                );
            }
        }

        let mut domains = BTreeMap::new();
        let mut generated_types = BTreeSet::new();
        for domain in &self.domains {
            validate_snake_identifier(&domain.name, "domain.name")?;
            validate_rust_identifier(&domain.rust_type, "domain.rust_type")?;
            ensure!(
                domains.insert(domain.name.as_str(), domain).is_none(),
                "duplicate domain {}",
                domain.name
            );
            ensure!(
                !domain.variants.is_empty(),
                "domain {} requires variants",
                domain.name
            );
            if domain.generated {
                ensure!(
                    generated_types.insert(domain.rust_type.as_str()),
                    "duplicate generated Rust type {}",
                    domain.rust_type
                );
            }
            let mut variants = BTreeSet::new();
            let bool_mapping = domain
                .variants
                .iter()
                .filter(|variant| variant.bool_value.is_some())
                .count();
            let optional_bool_mapping = domain
                .variants
                .iter()
                .filter(|variant| variant.optional_bool_value.is_some())
                .count();
            ensure!(
                bool_mapping == 0 || bool_mapping == domain.variants.len(),
                "domain {} must map every variant from bool or none",
                domain.name
            );
            ensure!(
                optional_bool_mapping == 0 || optional_bool_mapping == domain.variants.len(),
                "domain {} must map every variant from Option<bool> or none",
                domain.name
            );
            ensure!(
                bool_mapping == 0 || optional_bool_mapping == 0,
                "domain {} cannot have two input mappings",
                domain.name
            );
            if bool_mapping > 0 {
                let values = domain
                    .variants
                    .iter()
                    .map(|variant| variant.bool_value.expect("validated bool mapping"))
                    .collect::<BTreeSet<_>>();
                ensure!(
                    values == BTreeSet::from([false, true]),
                    "domain {} bool mapping must cover false and true exactly once",
                    domain.name
                );
            }
            if optional_bool_mapping > 0 {
                let values = domain
                    .variants
                    .iter()
                    .map(|variant| {
                        variant
                            .optional_bool_value
                            .as_deref()
                            .expect("validated optional bool mapping")
                    })
                    .collect::<BTreeSet<_>>();
                ensure!(
                    values == BTreeSet::from(["false", "none", "true"]),
                    "domain {} Option<bool> mapping must cover none, false, and true exactly once",
                    domain.name
                );
            }
            for variant in &domain.variants {
                validate_rust_identifier(&variant.name, "domain.variant.name")?;
                ensure!(
                    variants.insert(variant.name.as_str()),
                    "domain {} has duplicate variant {}",
                    domain.name,
                    variant.name
                );
                ensure!(
                    variant.registered || variant.payload_domains.is_empty(),
                    "excluded variant {}.{} cannot carry registered payload domains",
                    domain.name,
                    variant.name
                );
            }
        }

        for domain in &self.domains {
            for variant in &domain.variants {
                for payload_domain in &variant.payload_domains {
                    ensure!(
                        domains.contains_key(payload_domain.as_str()),
                        "domain {} variant {} references unknown payload domain {}",
                        domain.name,
                        variant.name,
                        payload_domain
                    );
                }
            }
        }

        let mut producer_names = BTreeSet::new();
        let mut producer_markers = BTreeSet::new();
        let mut producer_keys = BTreeSet::new();
        let mut producer_owners = BTreeSet::new();
        let mut dimension_ids = BTreeSet::new();
        let mut root_domains = BTreeSet::new();
        for producer in &self.producers {
            validate_snake_identifier(&producer.name, "producer.name")?;
            validate_rust_identifier(&producer.rust_marker, "producer.rust_marker")?;
            validate_rust_identifier(&producer.rust_key, "producer.rust_key")?;
            validate_rust_identifier(&producer.owner, "producer.owner")?;
            validate_snake_identifier(&producer.producer_kind, "producer.producer_kind")?;
            ensure!(
                producer_names.insert(producer.name.as_str()),
                "duplicate producer {}",
                producer.name
            );
            ensure!(
                producer_markers.insert(producer.rust_marker.as_str()),
                "duplicate producer marker {}",
                producer.rust_marker
            );
            ensure!(
                producer_keys.insert(producer.rust_key.as_str()),
                "duplicate producer key {}",
                producer.rust_key
            );
            ensure!(
                producer_owners.insert(producer.owner.as_str()),
                "duplicate producer owner {}",
                producer.owner
            );
            let allocation = allocations
                .get(producer.allocation.as_str())
                .with_context(|| {
                    format!(
                        "producer {} references unknown allocation {}",
                        producer.name, producer.allocation
                    )
                })?;
            ensure!(
                !producer.dimensions.is_empty(),
                "producer {} requires dimensions",
                producer.name
            );
            let mut dimension_names = BTreeSet::new();
            for dimension in &producer.dimensions {
                validate_snake_identifier(&dimension.name, "producer.dimension.name")?;
                validate_rust_type(
                    &dimension.rust_field_type,
                    "producer.dimension.rust_field_type",
                )?;
                ensure!(
                    dimension_names.insert(dimension.name.as_str()),
                    "producer {} has duplicate dimension {}",
                    producer.name,
                    dimension.name
                );
                ensure!(
                    dimension_ids.insert(dimension.id),
                    "duplicate dimension id {}",
                    dimension.id
                );
                ensure!(
                    dimension.id >= allocation.id_start
                        && dimension.id < allocation.id_end_exclusive,
                    "producer {} dimension {} id {} escapes allocation {}",
                    producer.name,
                    dimension.name,
                    dimension.id,
                    producer.allocation
                );
                let domain = domains.get(dimension.domain.as_str()).with_context(|| {
                    format!(
                        "producer {} dimension {} references unknown domain {}",
                        producer.name, dimension.name, dimension.domain
                    )
                })?;
                validate_dimension_type(dimension, domain)?;
                root_domains.insert(domain.name.as_str());
                ensure!(
                    matches!(dimension.shape, DimensionShape::Opaque)
                        || (dimension.component_domains.is_empty()
                            && dimension.optional_component_domains.is_empty()),
                    "only opaque dimension {} may declare component domains",
                    dimension.name
                );
                let mut component_names = BTreeSet::new();
                for component_domain in dimension
                    .component_domains
                    .iter()
                    .chain(&dimension.optional_component_domains)
                {
                    ensure!(
                        component_names.insert(component_domain),
                        "producer {} dimension {} repeats component domain {}",
                        producer.name,
                        dimension.name,
                        component_domain
                    );
                    let component = domains.get(component_domain.as_str()).with_context(|| {
                        format!(
                            "producer {} dimension {} references unknown component domain {}",
                            producer.name, dimension.name, component_domain
                        )
                    })?;
                    root_domains.insert(component.name.as_str());
                }
            }
        }

        let mut reachable_domains = BTreeSet::new();
        let mut pending = root_domains.into_iter().collect::<Vec<_>>();
        while let Some(name) = pending.pop() {
            if !reachable_domains.insert(name) {
                continue;
            }
            let domain = domains
                .get(name)
                .expect("root and payload domains validated");
            for variant in &domain.variants {
                pending.extend(variant.payload_domains.iter().map(String::as_str));
            }
        }
        ensure!(
            reachable_domains.len() == domains.len(),
            "every domain must be reachable from a producer dimension"
        );
        Ok(())
    }
}

fn validate_dimension_type(dimension: &Dimension, domain: &Domain) -> Result<()> {
    let expected = match dimension.shape {
        DimensionShape::Scalar => domain.rust_type.clone(),
        DimensionShape::Set => format!("CanonicalSet<{}>", domain.rust_type),
        DimensionShape::Optional => format!("Option<{}>", domain.rust_type),
        DimensionShape::Opaque => return Ok(()),
    };
    ensure!(
        dimension.rust_field_type == expected,
        "dimension {} type must be {}, got {}",
        dimension.name,
        expected,
        dimension.rust_field_type
    );
    Ok(())
}

fn validate_snake_identifier(value: &str, field: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{field} must not be empty");
    ensure!(
        value
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_lowercase()
                || byte == b'_'
                || (index > 0 && byte.is_ascii_digit())),
        "{field} must be a lowercase snake-case identifier"
    );
    Ok(())
}

fn validate_rust_identifier(value: &str, field: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{field} must not be empty");
    ensure!(
        value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        }),
        "{field} must be one Rust identifier"
    );
    Ok(())
}

fn validate_rust_type(value: &str, field: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{field} must not be empty");
    ensure!(
        value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'<' | b'>' | b',' | b' ')
        }),
        "{field} contains unsupported Rust type syntax"
    );
    Ok(())
}

pub fn render_registry(registry: &EvidenceNoveltyRegistry) -> Result<String> {
    registry.validate()?;
    let domains = registry
        .domains
        .iter()
        .map(|domain| (domain.name.as_str(), domain))
        .collect::<BTreeMap<_, _>>();
    let mut output = String::new();
    writeln!(
        output,
        "// @generated by the Rust evidence-novelty generator from"
    )?;
    writeln!(output, "// config/evidence-novelty.toml. Do not edit.\n")?;
    writeln!(
        output,
        "use crate::bolt_v3_decision_evidence::{{BoltV3BinaryOutcomeEdgeBlockReason, BoltV3EntryBlockReason, BoltV3EntryPricingBlockReason, BoltV3EntrySkipReasonCategory, BoltV3ExposureOccupancy, BoltV3ForcedFlatReason, BoltV3OutcomeSide, BoltV3RvGateResult}};"
    )?;
    writeln!(
        output,
        "use crate::bolt_v3_market_families::MarketSelectionOutcome;"
    )?;
    writeln!(
        output,
        "use crate::bolt_v3_realized_volatility::{{RealizedVolBlockReason, RealizedVolSourceRejectReason, RealizedVolSourceStatus}};"
    )?;
    writeln!(
        output,
        "use super::{{CanonicalSet, CanonicalSourceStates, NoveltyEligibleProducer, private}};\n"
    )?;

    render_owner_enum(registry, &mut output)?;
    for domain in registry.domains.iter().filter(|domain| domain.generated) {
        render_generated_domain(domain, &mut output)?;
    }
    for domain in registry.domains.iter().filter(|domain| !domain.generated) {
        render_domain_validator(domain, &domains, &mut output)?;
    }
    for producer in &registry.producers {
        render_producer(producer, &domains, &mut output)?;
    }
    render_cardinality_bounds(registry, &domains, &mut output)?;
    render_registrations(registry, &mut output)?;
    Ok(output)
}

fn domain_cardinality(
    domain_name: &str,
    domains: &BTreeMap<&str, &Domain>,
    visiting: &mut BTreeSet<String>,
) -> Result<u128> {
    ensure!(
        visiting.insert(domain_name.to_string()),
        "domain payload cycle contains {domain_name}"
    );
    let domain = domains
        .get(domain_name)
        .with_context(|| format!("cardinality references unknown domain {domain_name}"))?;
    let mut total = 0_u128;
    for variant in domain.variants.iter().filter(|variant| variant.registered) {
        let mut variant_cardinality = 1_u128;
        for payload_domain in &variant.payload_domains {
            variant_cardinality = variant_cardinality
                .checked_mul(domain_cardinality(payload_domain, domains, visiting)?)
                .context("semantic domain cardinality overflow")?;
        }
        total = total
            .checked_add(variant_cardinality)
            .context("semantic domain cardinality overflow")?;
    }
    visiting.remove(domain_name);
    ensure!(total > 0, "domain {domain_name} has no registered states");
    Ok(total)
}

fn cardinality_constant_name(producer_name: &str) -> String {
    producer_name.to_ascii_uppercase()
}

fn render_cardinality_bounds(
    registry: &EvidenceNoveltyRegistry,
    domains: &BTreeMap<&str, &Domain>,
    output: &mut String,
) -> Result<()> {
    for producer in &registry.producers {
        let mut static_factor = 1_u128;
        let mut opaque_per_registered_source = None;
        for dimension in &producer.dimensions {
            let domain_size = domain_cardinality(&dimension.domain, domains, &mut BTreeSet::new())?;
            let dimension_cardinality = match dimension.shape {
                DimensionShape::Scalar => domain_size,
                DimensionShape::Optional => domain_size
                    .checked_add(1)
                    .context("optional semantic dimension cardinality overflow")?,
                DimensionShape::Set => {
                    let exponent = u32::try_from(domain_size)
                        .context("set semantic domain cardinality exceeds u32")?;
                    2_u128
                        .checked_pow(exponent)
                        .context("set semantic dimension cardinality exceeds u128")?
                }
                DimensionShape::Opaque => {
                    ensure!(
                        opaque_per_registered_source.is_none(),
                        "producer {} may declare at most one source-roster dimension",
                        producer.name
                    );
                    let mut per_source = domain_size;
                    for component_domain in &dimension.component_domains {
                        per_source = per_source
                            .checked_mul(domain_cardinality(
                                component_domain,
                                domains,
                                &mut BTreeSet::new(),
                            )?)
                            .context("source semantic state cardinality overflow")?;
                    }
                    for component_domain in &dimension.optional_component_domains {
                        let optional_component =
                            domain_cardinality(component_domain, domains, &mut BTreeSet::new())?
                                .checked_add(1)
                                .context("optional source component cardinality overflow")?;
                        per_source = per_source
                            .checked_mul(optional_component)
                            .context("source semantic state cardinality overflow")?;
                    }
                    opaque_per_registered_source = Some(per_source);
                    continue;
                }
            };
            static_factor = static_factor
                .checked_mul(dimension_cardinality)
                .context("producer semantic key cardinality overflow")?;
        }
        let constant_name = cardinality_constant_name(&producer.name);
        if let Some(per_source) = opaque_per_registered_source {
            writeln!(
                output,
                "pub const {constant_name}_STATIC_STATE_UPPER_BOUND: u128 = {static_factor};"
            )?;
            writeln!(
                output,
                "pub const {constant_name}_PER_REGISTERED_SOURCE_STATE_UPPER_BOUND: u128 = {per_source};"
            )?;
            writeln!(
                output,
                "pub fn {}_per_episode_state_upper_bound(registered_source_count: u32) -> Option<u128> {{",
                producer.name
            )?;
            writeln!(
                output,
                "    {constant_name}_PER_REGISTERED_SOURCE_STATE_UPPER_BOUND.checked_pow(registered_source_count).and_then(|source_states| {constant_name}_STATIC_STATE_UPPER_BOUND.checked_mul(source_states))"
            )?;
            writeln!(output, "}}\n")?;
        } else {
            writeln!(
                output,
                "pub const {constant_name}_PER_EPISODE_STATE_UPPER_BOUND: u128 = {static_factor};\n"
            )?;
        }
    }
    Ok(())
}

fn render_owner_enum(registry: &EvidenceNoveltyRegistry, output: &mut String) -> Result<()> {
    writeln!(
        output,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]"
    )?;
    writeln!(output, "pub enum EvidenceProducerOwner {{")?;
    for producer in &registry.producers {
        writeln!(output, "    {},", producer.owner)?;
    }
    writeln!(output, "}}\n")?;
    Ok(())
}

fn render_generated_domain(domain: &Domain, output: &mut String) -> Result<()> {
    writeln!(
        output,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]"
    )?;
    writeln!(output, "pub enum {} {{", domain.rust_type)?;
    for variant in &domain.variants {
        ensure!(
            variant.registered,
            "generated domains cannot have excluded variants"
        );
        ensure!(
            variant.payload_domains.is_empty(),
            "generated domains cannot carry payloads"
        );
        writeln!(output, "    {},", variant.name)?;
    }
    writeln!(output, "}}\n")?;

    if domain
        .variants
        .iter()
        .all(|variant| variant.bool_value.is_some())
    {
        writeln!(output, "impl From<bool> for {} {{", domain.rust_type)?;
        writeln!(output, "    fn from(value: bool) -> Self {{")?;
        writeln!(output, "        match value {{")?;
        for variant in &domain.variants {
            writeln!(
                output,
                "            {} => Self::{},",
                variant.bool_value.expect("validated bool mapping"),
                variant.name
            )?;
        }
        writeln!(output, "        }}")?;
        writeln!(output, "    }}")?;
        writeln!(output, "}}\n")?;
    }
    if domain
        .variants
        .iter()
        .all(|variant| variant.optional_bool_value.is_some())
    {
        writeln!(
            output,
            "impl From<Option<bool>> for {} {{",
            domain.rust_type
        )?;
        writeln!(output, "    fn from(value: Option<bool>) -> Self {{")?;
        writeln!(output, "        match value {{")?;
        for variant in &domain.variants {
            let pattern = match variant.optional_bool_value.as_deref() {
                Some("none") => "None",
                Some("false") => "Some(false)",
                Some("true") => "Some(true)",
                _ => bail!("invalid optional bool mapping in {}", domain.name),
            };
            writeln!(output, "            {pattern} => Self::{},", variant.name)?;
        }
        writeln!(output, "        }}")?;
        writeln!(output, "    }}")?;
        writeln!(output, "}}\n")?;
    }
    Ok(())
}

fn render_domain_validator(
    domain: &Domain,
    domains: &BTreeMap<&str, &Domain>,
    output: &mut String,
) -> Result<()> {
    writeln!(
        output,
        "pub(super) fn validate_{}(value: &{}) -> anyhow::Result<()> {{",
        domain.name, domain.rust_type
    )?;
    writeln!(output, "    match value {{")?;
    for variant in &domain.variants {
        if variant.payload_domains.is_empty() {
            if variant.registered {
                writeln!(
                    output,
                    "        {}::{} => Ok(()),",
                    domain.rust_type, variant.name
                )?;
            } else {
                writeln!(
                    output,
                    "        {}::{} => anyhow::bail!(\"unregistered {} state {}\"),",
                    domain.rust_type, variant.name, domain.name, variant.name
                )?;
            }
            continue;
        }
        let bindings = (0..variant.payload_domains.len())
            .map(|index| format!("value_{index}"))
            .collect::<Vec<_>>();
        writeln!(
            output,
            "        {}::{}({}) => {{",
            domain.rust_type,
            variant.name,
            bindings.join(", ")
        )?;
        for (binding, payload_domain_name) in bindings.iter().zip(variant.payload_domains.iter()) {
            let payload_domain = domains
                .get(payload_domain_name.as_str())
                .expect("payload domain validated");
            if !payload_domain.generated {
                writeln!(
                    output,
                    "            validate_{payload_domain_name}({binding})?;"
                )?;
            }
        }
        writeln!(output, "            Ok(())")?;
        writeln!(output, "        }}")?;
    }
    writeln!(output, "    }}")?;
    writeln!(output, "}}\n")?;
    Ok(())
}

fn render_producer(
    producer: &Producer,
    domains: &BTreeMap<&str, &Domain>,
    output: &mut String,
) -> Result<()> {
    writeln!(
        output,
        "#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]"
    )?;
    writeln!(output, "pub struct {} {{", producer.rust_key)?;
    for dimension in &producer.dimensions {
        writeln!(
            output,
            "    {}: {},",
            dimension.name, dimension.rust_field_type
        )?;
    }
    writeln!(output, "}}\n")?;
    writeln!(output, "impl {} {{", producer.rust_key)?;
    writeln!(output, "    #[allow(clippy::too_many_arguments)]")?;
    writeln!(output, "    pub fn try_new(")?;
    for dimension in &producer.dimensions {
        writeln!(
            output,
            "        {}: {},",
            dimension.name, dimension.rust_field_type
        )?;
    }
    writeln!(output, "    ) -> anyhow::Result<Self> {{")?;
    for dimension in &producer.dimensions {
        let domain = domains
            .get(dimension.domain.as_str())
            .expect("dimension domain validated");
        if domain.generated || matches!(dimension.shape, DimensionShape::Opaque) {
            continue;
        }
        match dimension.shape {
            DimensionShape::Scalar => {
                writeln!(
                    output,
                    "        validate_{}(&{})?;",
                    domain.name, dimension.name
                )?;
            }
            DimensionShape::Set => {
                writeln!(output, "        for value in {}.iter() {{", dimension.name)?;
                writeln!(output, "            validate_{}(value)?;", domain.name)?;
                writeln!(output, "        }}")?;
            }
            DimensionShape::Optional => {
                writeln!(
                    output,
                    "        if let Some(value) = &{} {{",
                    dimension.name
                )?;
                writeln!(output, "            validate_{}(value)?;", domain.name)?;
                writeln!(output, "        }}")?;
            }
            DimensionShape::Opaque => unreachable!("handled above"),
        }
    }
    writeln!(output, "        Ok(Self {{")?;
    for dimension in &producer.dimensions {
        writeln!(output, "            {},", dimension.name)?;
    }
    writeln!(output, "        }})")?;
    writeln!(output, "    }}")?;
    writeln!(output, "}}\n")?;
    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub struct {};", producer.rust_marker)?;
    writeln!(
        output,
        "impl private::Sealed for {} {{}}",
        producer.rust_marker
    )?;
    writeln!(
        output,
        "impl NoveltyEligibleProducer for {} {{",
        producer.rust_marker
    )?;
    writeln!(
        output,
        "    type Key = {};
    const OWNER: EvidenceProducerOwner = EvidenceProducerOwner::{};
    const PRODUCER_KIND: &'static str = \"{}\";
}}\n",
        producer.rust_key, producer.owner, producer.producer_kind
    )?;
    Ok(())
}

fn render_registrations(registry: &EvidenceNoveltyRegistry, output: &mut String) -> Result<()> {
    writeln!(output, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(output, "pub struct EvidenceDimensionRegistration {{")?;
    writeln!(output, "    pub producer: &'static str,")?;
    writeln!(output, "    pub owner: EvidenceProducerOwner,")?;
    writeln!(output, "    pub name: &'static str,")?;
    writeln!(output, "    pub domain: &'static str,")?;
    writeln!(output, "    pub id: usize,")?;
    writeln!(output, "}}\n")?;
    writeln!(
        output,
        "pub const EVIDENCE_NOVELTY_FAMILY_CAPACITY: usize = {};",
        registry.family.capacity
    )?;
    writeln!(
        output,
        "pub const EVIDENCE_DIMENSION_REGISTRATIONS: &[EvidenceDimensionRegistration] = &["
    )?;
    for producer in &registry.producers {
        for dimension in &producer.dimensions {
            writeln!(output, "    EvidenceDimensionRegistration {{")?;
            writeln!(output, "        producer: \"{}\",", producer.name)?;
            writeln!(
                output,
                "        owner: EvidenceProducerOwner::{},",
                producer.owner
            )?;
            writeln!(output, "        name: \"{}\",", dimension.name)?;
            writeln!(output, "        domain: \"{}\",", dimension.domain)?;
            writeln!(output, "        id: {},", dimension.id)?;
            writeln!(output, "    }},")?;
        }
    }
    writeln!(output, "];\n")?;
    writeln!(
        output,
        "pub fn evidence_dimension_registration_by_id(id: usize) -> Option<&'static EvidenceDimensionRegistration> {{"
    )?;
    writeln!(
        output,
        "    EVIDENCE_DIMENSION_REGISTRATIONS.iter().find(|registration| registration.id == id)"
    )?;
    writeln!(output, "}}")?;
    Ok(())
}
