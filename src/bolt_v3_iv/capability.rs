use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const REQUIRED_CANDIDATE_SWEEP_TERMS: [&str; 18] = [
    "option",
    "options",
    "greeks",
    "implied",
    "iv",
    "volatility",
    "smile",
    "surface",
    "chain",
    "custom data",
    "strike",
    "expiry",
    "expiration",
    "tenor",
    "moneyness",
    "skew",
    "premium",
    "vol",
];

const CAPABILITY_ANCHOR_TERMS: [&str; REQUIRED_CANDIDATE_SWEEP_TERMS.len()] =
    REQUIRED_CANDIDATE_SWEEP_TERMS;

const REQUIRED_SEED_FAMILIES: [SeedFamily; 8] = [
    SeedFamily::ModelData,
    SeedFamily::DataActorSubscription,
    SeedFamily::DataEnginePublication,
    SeedFamily::Msgbus,
    SeedFamily::OptionChainManager,
    SeedFamily::GreeksHelper,
    SeedFamily::Adapter,
    SeedFamily::CustomData,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedFamily {
    ModelData,
    DataActorSubscription,
    DataEnginePublication,
    Msgbus,
    OptionChainManager,
    GreeksHelper,
    Adapter,
    CustomData,
}

impl SeedFamily {
    pub fn required() -> &'static [Self] {
        &REQUIRED_SEED_FAMILIES
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityClassification {
    Supported,
    Unreachable,
    NotIvOptions,
    Excluded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvCapabilityCandidate {
    pub surface_id: String,
    pub evidence_path: String,
    pub symbol: String,
    pub matched_terms: BTreeSet<String>,
    pub seed_family: Option<SeedFamily>,
    pub engine_mapping: Option<IvCapabilityEngineMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvCapabilityLedger {
    pub surfaces: Vec<IvCapabilityCandidate>,
    pub classifications: BTreeMap<String, CapabilityClassification>,
    pub classification_rules: Vec<IvCapabilityClassificationRule>,
    pub engine_mapping_rules: Vec<IvCapabilityEngineMappingRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvCapabilityClassificationRule {
    pub surface_id_prefix: String,
    pub classification: CapabilityClassification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvCapabilityEngineMapping {
    pub mapping_kind: IvCapabilityEngineMappingKind,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvCapabilityEngineMappingKind {
    SourceKind,
    ProductKind,
    Helper,
    RuntimeOperation,
    Api,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvCapabilityEngineMappingRule {
    pub surface_id_prefix: String,
    pub engine_mapping: IvCapabilityEngineMapping,
}

impl IvCapabilityLedger {
    pub fn empty() -> Self {
        Self {
            surfaces: Vec::new(),
            classifications: BTreeMap::new(),
            classification_rules: Vec::new(),
            engine_mapping_rules: Vec::new(),
        }
    }

    pub fn classification_for(&self, surface_id: &str) -> Option<CapabilityClassification> {
        self.classifications.get(surface_id).copied().or_else(|| {
            matching_classification_rule(&self.classification_rules, surface_id)
                .map(|rule| rule.classification)
        })
    }

    pub fn engine_mapping_for(&self, surface_id: &str) -> Option<&IvCapabilityEngineMapping> {
        self.surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
            .and_then(|surface| surface.engine_mapping.as_ref())
            .or_else(|| {
                matching_engine_mapping_rule(&self.engine_mapping_rules, surface_id)
                    .map(|rule| &rule.engine_mapping)
            })
    }

    pub fn validate_candidates(
        &self,
        candidates: &[IvCapabilityCandidate],
    ) -> Result<(), IvCapabilityError> {
        for candidate in candidates {
            if self
                .classifications
                .contains_key(candidate.surface_id.as_str())
            {
                if self.classification_for(&candidate.surface_id)
                    == Some(CapabilityClassification::Supported)
                    && self.engine_mapping_for(&candidate.surface_id).is_none()
                {
                    return Err(IvCapabilityError::MissingEngineMapping {
                        surface_id: candidate.surface_id.clone(),
                    });
                }
                continue;
            }

            let classification_rule =
                matching_classification_rule(&self.classification_rules, &candidate.surface_id);
            let rule_classification = classification_rule.map(|rule| rule.classification);

            if matches!(
                rule_classification,
                Some(CapabilityClassification::Unreachable)
                    | Some(CapabilityClassification::NotIvOptions)
                    | Some(CapabilityClassification::Excluded)
            ) {
                if candidate_requires_exact_review(candidate) {
                    return Err(IvCapabilityError::UnclassifiedCandidate {
                        surface_id: candidate.surface_id.clone(),
                    });
                }
                continue;
            }

            return Err(IvCapabilityError::UnclassifiedCandidate {
                surface_id: candidate.surface_id.clone(),
            });
        }

        Ok(())
    }
}

fn candidate_requires_exact_review(candidate: &IvCapabilityCandidate) -> bool {
    candidate.matched_terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "greeks" | "implied" | "iv" | "volatility" | "smile"
        )
    }) || (candidate.matched_terms.contains("surface")
        && (candidate.matched_terms.contains("option")
            || candidate.matched_terms.contains("options")))
        || [
            "option_chain",
            "option_greeks",
            "option_summary",
            "option_surface",
            "imply_vol",
        ]
        .iter()
        .any(|needle| candidate.surface_id.contains(needle))
}

fn matching_classification_rule<'a>(
    rules: &'a [IvCapabilityClassificationRule],
    surface_id: &str,
) -> Option<&'a IvCapabilityClassificationRule> {
    rules
        .iter()
        .filter(|rule| surface_id.starts_with(&rule.surface_id_prefix))
        .max_by_key(|rule| rule.surface_id_prefix.len())
}

fn matching_engine_mapping_rule<'a>(
    rules: &'a [IvCapabilityEngineMappingRule],
    surface_id: &str,
) -> Option<&'a IvCapabilityEngineMappingRule> {
    rules
        .iter()
        .filter(|rule| surface_id.starts_with(&rule.surface_id_prefix))
        .max_by_key(|rule| rule.surface_id_prefix.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtCargoEvidence {
    pub nt_revision: String,
    pub resolved_checkout_path: PathBuf,
    pub lock_revisions: BTreeMap<String, String>,
    pub metadata_packages: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvCapabilityError {
    InvalidCargoMetadata(String),
    InvalidCargoLock(String),
    MissingNtMetadata,
    MissingNtLockSource,
    MissingNtCheckoutPath,
    RevisionMismatch {
        package: String,
        metadata_revision: String,
        lock_revision: String,
    },
    Io(String),
    Toml(String),
    UnclassifiedCandidate {
        surface_id: String,
    },
    MissingEngineMapping {
        surface_id: String,
    },
}

impl From<io::Error> for IvCapabilityError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    source: Option<String>,
    manifest_path: String,
}

#[derive(Debug, Deserialize)]
struct FixtureLedger {
    surfaces: Vec<FixtureSurface>,
    engine_mapping_rules: Vec<IvCapabilityEngineMappingRule>,
    classification_rules: Vec<IvCapabilityClassificationRule>,
}

#[derive(Debug, Deserialize)]
struct FixtureSurface {
    surface_id: String,
    evidence_path: String,
    symbol: String,
    matched_terms: Vec<String>,
    seed_family: Option<SeedFamily>,
    classification: CapabilityClassification,
    engine_mapping: Option<IvCapabilityEngineMapping>,
}

pub fn resolve_nt_cargo_evidence(
    metadata_json: &str,
    lock_text: &str,
) -> Result<NtCargoEvidence, IvCapabilityError> {
    let metadata = serde_json::from_str::<CargoMetadata>(metadata_json)
        .map_err(|error| IvCapabilityError::InvalidCargoMetadata(error.to_string()))?;

    let mut metadata_revisions = BTreeMap::new();
    let mut metadata_packages = BTreeSet::new();
    let mut checkout_path = None;

    for package in metadata.packages {
        let Some(source) = package.source.as_deref() else {
            continue;
        };
        let Some(revision) = nt_source_revision(source) else {
            continue;
        };

        metadata_packages.insert(package.name.clone());
        metadata_revisions.insert(package.name.clone(), revision);

        if checkout_path.is_none() {
            checkout_path = checkout_root_from_manifest(&package.manifest_path);
        }
    }

    if metadata_revisions.is_empty() {
        return Err(IvCapabilityError::MissingNtMetadata);
    }

    let nt_revision = single_revision(&metadata_revisions)?;
    let lock_revisions = nt_lock_revisions(lock_text)?;

    for (package, lock_revision) in &lock_revisions {
        if let Some(metadata_revision) = metadata_revisions.get(package)
            && metadata_revision != lock_revision
        {
            return Err(IvCapabilityError::RevisionMismatch {
                package: package.clone(),
                metadata_revision: metadata_revision.clone(),
                lock_revision: lock_revision.clone(),
            });
        }
    }

    let resolved_checkout_path = checkout_path.ok_or(IvCapabilityError::MissingNtCheckoutPath)?;

    Ok(NtCargoEvidence {
        nt_revision,
        resolved_checkout_path,
        lock_revisions,
        metadata_packages,
    })
}

pub fn scan_seed_families(root: &Path) -> Result<Vec<IvCapabilityCandidate>, IvCapabilityError> {
    let mut candidates = scan_candidates(root)?;
    candidates.retain(|candidate| candidate.seed_family.is_some());
    let mut discovered = candidates
        .iter()
        .filter_map(|candidate| candidate.seed_family)
        .collect::<BTreeSet<_>>();
    let mut files = Vec::new();
    collect_rust_files(root, &mut files)?;
    for file in files {
        let text = fs::read_to_string(&file)?;
        let relative_path = relative_path(root, &file);
        let Some(seed_family) = classify_seed_family(&relative_path, &text) else {
            continue;
        };
        if discovered.contains(&seed_family) {
            continue;
        }
        let matched_terms = matched_terms(&relative_path, &text);
        if matched_terms.is_empty() || !has_capability_anchor(&matched_terms) {
            continue;
        }
        discovered.insert(seed_family);
        let symbol = format!("{seed_family:?}SeedFamily");
        candidates.push(IvCapabilityCandidate {
            surface_id: surface_id(&relative_path, &symbol),
            evidence_path: relative_path,
            symbol,
            matched_terms,
            seed_family: Some(seed_family),
            engine_mapping: None,
        });
    }
    Ok(candidates)
}

pub fn scan_whole_checkout_candidates(
    root: &Path,
) -> Result<Vec<IvCapabilityCandidate>, IvCapabilityError> {
    scan_candidates(root)
}

pub fn load_capability_ledger_fixture(
    path: &Path,
) -> Result<IvCapabilityLedger, IvCapabilityError> {
    let text = fs::read_to_string(path)?;
    let fixture = toml::from_str::<FixtureLedger>(&text)
        .map_err(|error| IvCapabilityError::Toml(error.to_string()))?;

    let mut surfaces = Vec::with_capacity(fixture.surfaces.len());
    let mut classifications = BTreeMap::new();

    for surface in fixture.surfaces {
        classifications.insert(surface.surface_id.clone(), surface.classification);
        surfaces.push(IvCapabilityCandidate {
            surface_id: surface.surface_id,
            evidence_path: surface.evidence_path,
            symbol: surface.symbol,
            matched_terms: surface.matched_terms.into_iter().collect(),
            seed_family: surface.seed_family,
            engine_mapping: surface.engine_mapping,
        });
    }

    Ok(IvCapabilityLedger {
        surfaces,
        classifications,
        classification_rules: fixture.classification_rules,
        engine_mapping_rules: fixture.engine_mapping_rules,
    })
}

fn nt_source_revision(source: &str) -> Option<String> {
    if !source.contains("nautilus_trader") {
        return None;
    }

    if let Some((_, revision)) = source.rsplit_once('#')
        && !revision.is_empty()
    {
        return Some(revision.to_string());
    }

    source.find("rev=").and_then(|start| {
        let revision_start = start + "rev=".len();
        let revision = source[revision_start..]
            .split(['&', '#'])
            .next()
            .unwrap_or("");

        (!revision.is_empty()).then(|| revision.to_string())
    })
}

fn checkout_root_from_manifest(manifest_path: &str) -> Option<PathBuf> {
    let mut cursor = Path::new(manifest_path).parent();

    while let Some(path) = cursor {
        if path.file_name().and_then(|name| name.to_str()) == Some("crates") {
            return path.parent().map(Path::to_path_buf);
        }

        cursor = path.parent();
    }

    Path::new(manifest_path).parent().map(Path::to_path_buf)
}

fn nt_lock_revisions(lock_text: &str) -> Result<BTreeMap<String, String>, IvCapabilityError> {
    let lock = toml::from_str::<toml::Value>(lock_text)
        .map_err(|error| IvCapabilityError::InvalidCargoLock(error.to_string()))?;
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| IvCapabilityError::InvalidCargoLock("missing package array".to_string()))?;
    let mut revisions = BTreeMap::new();

    for package in packages {
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let Some(source) = package.get("source").and_then(toml::Value::as_str) else {
            continue;
        };
        let Some(revision) = nt_source_revision(source) else {
            continue;
        };

        revisions.insert(name.to_string(), revision);
    }

    if revisions.is_empty() {
        return Err(IvCapabilityError::MissingNtLockSource);
    }

    let expected_revision = single_revision(&revisions)?;
    for (package, revision) in &revisions {
        if revision != &expected_revision {
            return Err(IvCapabilityError::RevisionMismatch {
                package: package.clone(),
                metadata_revision: expected_revision.clone(),
                lock_revision: revision.clone(),
            });
        }
    }

    Ok(revisions)
}

fn single_revision(revisions: &BTreeMap<String, String>) -> Result<String, IvCapabilityError> {
    let Some((_, first_revision)) = revisions.iter().next() else {
        return Err(IvCapabilityError::MissingNtMetadata);
    };

    for (package, revision) in revisions {
        if revision != first_revision {
            return Err(IvCapabilityError::RevisionMismatch {
                package: package.clone(),
                metadata_revision: first_revision.clone(),
                lock_revision: revision.clone(),
            });
        }
    }

    Ok(first_revision.clone())
}

fn scan_candidates(root: &Path) -> Result<Vec<IvCapabilityCandidate>, IvCapabilityError> {
    let mut files = Vec::new();
    collect_rust_files(root, &mut files)?;

    let mut candidates = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file)?;
        let relative_path = relative_path(root, &file);
        let seed_family = classify_seed_family(&relative_path, &text);
        for public_symbol in public_symbols(&text) {
            let matched_terms = matched_terms(&relative_path, &public_symbol.evidence);
            if matched_terms.is_empty() || !has_capability_anchor(&matched_terms) {
                continue;
            }

            candidates.push(IvCapabilityCandidate {
                surface_id: surface_id(&relative_path, &public_symbol.symbol),
                evidence_path: relative_path.clone(),
                symbol: public_symbol.symbol,
                matched_terms: matched_terms.clone(),
                seed_family,
                engine_mapping: None,
            });
        }
    }

    Ok(candidates)
}

fn has_capability_anchor(matched_terms: &BTreeSet<String>) -> bool {
    CAPABILITY_ANCHOR_TERMS
        .iter()
        .any(|term| matched_terms.contains(*term))
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), IvCapabilityError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }

    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn matched_terms(relative_path: &str, text: &str) -> BTreeSet<String> {
    let haystack = format!("{} {}", relative_path, text).to_lowercase();

    REQUIRED_CANDIDATE_SWEEP_TERMS
        .iter()
        .filter(|term| term_matches(&haystack, term))
        .map(|term| (*term).to_string())
        .collect()
}

fn term_matches(haystack: &str, term: &str) -> bool {
    if term == "custom data" {
        return haystack.contains("custom data")
            || haystack.contains("custom_data")
            || haystack.contains("custom-data");
    }

    if matches!(term, "option" | "options") {
        return option_identifier_term_matches(haystack, term);
    }

    if matches!(term, "iv" | "vol") {
        return identifier_term_matches(haystack, term);
    }

    haystack.contains(term)
}

fn option_identifier_term_matches(haystack: &str, term: &str) -> bool {
    haystack
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .any(|token| {
            !token.starts_with("optional")
                && ((token != term && token.starts_with(term))
                    || token.ends_with(term)
                    || token.contains(&format!("_{term}_")))
        })
}

fn identifier_term_matches(haystack: &str, term: &str) -> bool {
    haystack
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .any(|token| {
            token == term
                || token
                    .strip_prefix(term)
                    .is_some_and(|suffix| suffix.starts_with('_'))
                || token
                    .strip_suffix(term)
                    .is_some_and(|prefix| prefix.ends_with('_'))
                || token.contains(&format!("_{term}_"))
        })
}

fn classify_seed_family(relative_path: &str, text: &str) -> Option<SeedFamily> {
    let relative_path = relative_path.to_lowercase();
    let text = text.to_lowercase();
    let combined = format!("{relative_path} {text}");

    if combined.contains("custom data") || combined.contains("custom_data") {
        Some(SeedFamily::CustomData)
    } else if relative_path.contains("crates/adapters") {
        Some(SeedFamily::Adapter)
    } else if relative_path.contains("crates/model/src/data/option_chain") {
        Some(SeedFamily::ModelData)
    } else if relative_path.contains("crates/data/src/client") || text.contains("subscribe_option")
    {
        Some(SeedFamily::DataActorSubscription)
    } else if relative_path.contains("crates/data/src/engine") || text.contains("publish_option") {
        Some(SeedFamily::DataEnginePublication)
    } else if relative_path.contains("msgbus")
        || relative_path.contains("topic")
        || text.contains("topic")
    {
        Some(SeedFamily::Msgbus)
    } else if relative_path.contains("option_chains")
        || relative_path.contains("option_chain_manager")
        || text.contains("optionchainaggregator")
    {
        Some(SeedFamily::OptionChainManager)
    } else if relative_path.contains("greeks") || text.contains("blackscholes") {
        Some(SeedFamily::GreeksHelper)
    } else if relative_path.contains("crates/model/src/data") {
        Some(SeedFamily::ModelData)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicSymbol {
    symbol: String,
    evidence: String,
}

fn public_symbols(text: &str) -> Vec<PublicSymbol> {
    let prefixes = [
        "pub struct ",
        "pub enum ",
        "pub trait ",
        "pub const fn ",
        "pub fn ",
        "pub async fn ",
        "pub type ",
        "pub const ",
        "pub mod ",
    ];
    let mut symbols = Vec::new();
    let mut context = Vec::new();

    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with("///")
            || line.starts_with("//!")
            || line.starts_with("#[")
            || line.starts_with("#!")
        {
            context.push(line);
            continue;
        }

        for prefix in prefixes {
            if let Some(rest) = line.strip_prefix(prefix)
                && let Some(symbol) = symbol_token(rest)
            {
                let mut evidence = context.join("\n");
                if !evidence.is_empty() {
                    evidence.push('\n');
                }
                evidence.push_str(&symbol);
                symbols.push(PublicSymbol { symbol, evidence });
                context.clear();
                break;
            }
        }

        if !line.is_empty() {
            context.clear();
        }
    }

    symbols
}

fn symbol_token(rest: &str) -> Option<String> {
    let token = rest
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '<' | '(' | '{' | ';' | ':' | '=')
        })
        .next()
        .unwrap_or("");

    (!token.is_empty()).then(|| token.to_string())
}

fn surface_id(relative_path: &str, symbol: &str) -> String {
    let path = relative_path
        .strip_suffix(".rs")
        .unwrap_or(relative_path)
        .replace('/', ".")
        .replace('-', "_");

    format!("nt.{path}.{}", to_snake_case(symbol))
}

fn to_snake_case(symbol: &str) -> String {
    let mut snake = String::new();

    for (index, character) in symbol.chars().enumerate() {
        if character.is_uppercase() {
            if index != 0 {
                snake.push('_');
            }
            for lowercase in character.to_lowercase() {
                snake.push(lowercase);
            }
        } else {
            snake.push(character);
        }
    }

    snake
}
