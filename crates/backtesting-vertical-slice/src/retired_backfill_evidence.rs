//! Typed evidence authority for a retired per-day sample backfill lane.
//!
//! The old lane materialized one executable RunSpec/plan/readiness tree per
//! venue-day. Those controls are no longer runnable. The committed inventory
//! retains exact fingerprints for their historical provenance while binding
//! each day to the independent source-proof, publication, and NT-mapping
//! evidence which remains in the repository.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use anyhow::{Context, Result, ensure};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::{
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    path_resolution::{resolve_existing_path, resolve_planned_write_path},
    source_catalog_mapping_readiness::SourceCatalogMappingStatusEntry,
    source_proof::{SourceProofReport, SourceProofStatus, SourceProofUsageScope},
};

pub const RETIRED_BACKFILL_EVIDENCE_INVENTORY_PATH: &str =
    "specs/023-nt-research-analytics-platform/reference/retired-backfill-evidence.inventory.json";
pub const RETIRED_BACKFILL_EVIDENCE_SCHEMA_VERSION: &str = "retired-backfill-evidence-inventory.v2";

const REFERENCE_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/";
const EXPECTED_TABLE_FAMILY: &str = "trades";
const EXPECTED_NT_DATA_TYPE: &str = "TradeTick";
const ACCEPTED_PUBLICATION_STATUS: &str = "accepted_gate_committed_and_s3_published";
const REFERENCE_COMPONENTS: &[&str] = &["specs", "023-nt-research-analytics-platform", "reference"];
const EMBEDDED_RETIRED_BACKFILL_EVIDENCE_INVENTORY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/retired-backfill-evidence.inventory.json"
));

const RETIRED_GATE_ARTIFACT_SUFFIXES: &[&str] = &[
    "accepted-tranche/backfill-accepted-tranche-manifest.json",
    "backfill-accepted-tranche.toml",
    "backfill-execution-plan.toml",
    "backfill-execution-readiness-artifact-index-required.toml",
    "backfill-execution-readiness.toml",
    "backfill-run-spec-materialization.toml",
    "backfill-source-proof-scope.toml",
    "execution-plan/backfill-execution-plan.json",
    "execution-readiness-artifact-index-required/backfill-execution-readiness-report.json",
    "execution-readiness/backfill-execution-readiness-report.json",
    "materialized-run-spec/backfill-run-spec.toml",
    "object-staging/backfill-object-staging-manifest.json",
    "source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json",
    "source-catalog-mapping-readiness.toml",
    "source-proof-scope/backfill-source-proof-scope-report.json",
];
const RETIRED_AGGREGATE_ROOT_ROLES: &[(&str, &[&str])] = &[
    (
        "backfill-conversion-batches",
        &[
            "backfill-conversion-batch-plan.toml",
            "plan/backfill-conversion-batch-plan.json",
        ],
    ),
    (
        "backfill-conversion-completion-ledgers",
        &[
            "backfill-conversion-completion-ledger.toml",
            "ledger/backfill-conversion-completion-ledger.json",
        ],
    ),
    (
        "backfill-coverage-ledgers",
        &[
            "backfill-coverage-ledger.toml",
            "ledger/backfill-coverage-ledger.json",
        ],
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredBackfillArtifactPin {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetiredBackfillArtifactStorage {
    GitWorkingTree,
    ContentAddressedS3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredBackfillAggregateTombstone {
    pub artifact: RetiredBackfillArtifactPin,
    pub prior_storage: RetiredBackfillArtifactStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredBackfillEvidenceRecord {
    pub record_id: String,
    pub venue: String,
    pub instrument_id: String,
    pub archive_date: String,
    pub source_binding: String,
    pub source_proof: RetiredBackfillArtifactPin,
    pub publication_evidence: RetiredBackfillArtifactPin,
    pub catalog_mapping_evaluation: RetiredBackfillArtifactPin,
    pub gate_artifact_tombstones: Vec<RetiredBackfillArtifactPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_daily_run_spec: Option<RetiredBackfillArtifactPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredBackfillSeriesCoverage {
    pub venue: String,
    pub instrument_id: String,
    pub source_binding: String,
    pub first_archive_date: String,
    pub last_archive_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredBackfillAggregateScope {
    pub root: String,
    pub scope: String,
    pub artifact_roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredBackfillEvidenceInventory {
    pub schema_version: String,
    pub inventory_id: String,
    pub series_coverage: Vec<RetiredBackfillSeriesCoverage>,
    pub records: Vec<RetiredBackfillEvidenceRecord>,
    pub aggregate_scopes: Vec<RetiredBackfillAggregateScope>,
    pub retained_active_daily_run_specs: Vec<String>,
    pub retired_aggregate_artifacts: Vec<RetiredBackfillAggregateTombstone>,
}

#[derive(Debug)]
struct RetiredBackfillRuntimeAuthority {
    directory_roots: BTreeSet<String>,
    exact_files: BTreeSet<String>,
}

static EMBEDDED_RUNTIME_AUTHORITY: LazyLock<Result<RetiredBackfillRuntimeAuthority, String>> =
    LazyLock::new(|| {
        RetiredBackfillEvidenceInventory::parse(EMBEDDED_RETIRED_BACKFILL_EVIDENCE_INVENTORY)
            .and_then(|inventory| inventory.validated_runtime_authority())
            .map_err(|error| format!("{error:#}"))
    });

impl RetiredBackfillEvidenceInventory {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("parse retired backfill evidence inventory JSON")
    }

    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(RETIRED_BACKFILL_EVIDENCE_INVENTORY_PATH);
        let bytes = fs::read(&path)
            .with_context(|| format!("read retired backfill inventory {}", path.display()))?;
        ensure!(
            bytes == EMBEDDED_RETIRED_BACKFILL_EVIDENCE_INVENTORY,
            "retired backfill inventory {} differs from the inventory embedded in this executable",
            path.display()
        );
        let inventory = Self::parse(&bytes)?;
        inventory
            .validate_structure()
            .with_context(|| format!("validate retired backfill inventory {}", path.display()))?;
        Ok(inventory)
    }

    pub fn validate_structure(&self) -> Result<()> {
        self.validated_runtime_authority().map(|_| ())
    }

    fn validated_runtime_authority(&self) -> Result<RetiredBackfillRuntimeAuthority> {
        ensure!(
            self.schema_version == RETIRED_BACKFILL_EVIDENCE_SCHEMA_VERSION,
            "retired backfill inventory schema_version {:?} != supported {:?}",
            self.schema_version,
            RETIRED_BACKFILL_EVIDENCE_SCHEMA_VERSION
        );
        ensure!(
            !self.inventory_id.trim().is_empty(),
            "retired backfill inventory_id must not be empty"
        );
        ensure!(
            !self.records.is_empty(),
            "retired backfill inventory records must not be empty"
        );

        ensure!(
            !self.series_coverage.is_empty(),
            "retired backfill series_coverage must not be empty"
        );
        let mut coverage_by_series = BTreeMap::new();
        let mut previous_coverage_key: Option<(String, String)> = None;
        for coverage in &self.series_coverage {
            validate_lowercase_slug(&coverage.venue, "coverage venue")?;
            validate_instrument_id(&coverage.instrument_id)?;
            validate_lowercase_slug(&coverage.source_binding, "coverage source_binding")?;
            let key = (
                coverage.venue.clone(),
                canonical_instrument_id(&coverage.instrument_id),
            );
            if let Some(previous) = &previous_coverage_key {
                ensure!(
                    previous < &key,
                    "series_coverage must be strictly sorted and unique by venue/instrument"
                );
            }
            previous_coverage_key = Some(key.clone());
            let first = parse_archive_date(&coverage.first_archive_date)?;
            let last = parse_archive_date(&coverage.last_archive_date)?;
            ensure!(
                first <= last,
                "series_coverage {}/{} has an inverted date range",
                coverage.venue,
                coverage.instrument_id
            );
            ensure!(
                coverage_by_series
                    .insert(key, (coverage.source_binding.as_str(), first, last))
                    .is_none(),
                "duplicate series_coverage for {}/{}",
                coverage.venue,
                coverage.instrument_id
            );
        }

        let mut previous_key: Option<(String, String, NaiveDate)> = None;
        let mut record_ids = BTreeSet::new();
        let mut evidence_paths = BTreeSet::new();
        let mut tombstoned_paths = BTreeSet::new();
        let mut expected_active_daily_run_specs = BTreeSet::new();
        let mut series_dates: BTreeMap<(String, String), Vec<NaiveDate>> = BTreeMap::new();
        let mut authority = RetiredBackfillRuntimeAuthority {
            directory_roots: BTreeSet::new(),
            exact_files: BTreeSet::new(),
        };
        for record in &self.records {
            validate_lowercase_slug(&record.venue, "venue")
                .with_context(|| format!("validate {} venue", record.record_id))?;
            validate_instrument_id(&record.instrument_id)
                .with_context(|| format!("validate {} instrument_id", record.record_id))?;
            validate_lowercase_slug(&record.source_binding, "source_binding")
                .with_context(|| format!("validate {} source_binding", record.record_id))?;
            let archive_date = parse_archive_date(&record.archive_date)?;
            let key = (
                record.venue.clone(),
                canonical_instrument_id(&record.instrument_id),
                archive_date,
            );
            if let Some(previous) = &previous_key {
                ensure!(
                    previous < &key,
                    "retired backfill records must be strictly sorted and unique by venue/instrument/date"
                );
            }
            previous_key = Some(key);
            let series_key = (
                record.venue.clone(),
                canonical_instrument_id(&record.instrument_id),
            );
            let Some((expected_binding, first, last)) = coverage_by_series.get(&series_key) else {
                anyhow::bail!(
                    "record {} is outside declared series_coverage",
                    record.record_id
                );
            };
            ensure!(
                record.source_binding.as_str() == *expected_binding,
                "record {} source_binding {:?} != declared {:?}",
                record.record_id,
                record.source_binding,
                expected_binding
            );
            ensure!(
                archive_date >= *first && archive_date <= *last,
                "record {} archive_date is outside declared series coverage",
                record.record_id
            );
            series_dates
                .entry(series_key)
                .or_default()
                .push(archive_date);
            ensure!(
                record_ids.insert(record.record_id.as_str()),
                "duplicate retired backfill record_id {:?}",
                record.record_id
            );
            let expected_record_id = retired_record_id(record);
            ensure!(
                record.record_id == expected_record_id,
                "record_id {:?} != expected {:?}",
                record.record_id,
                expected_record_id
            );

            for (role, pin) in [
                ("source_proof", &record.source_proof),
                ("publication_evidence", &record.publication_evidence),
                (
                    "catalog_mapping_evaluation",
                    &record.catalog_mapping_evaluation,
                ),
            ] {
                validate_pin(pin)
                    .with_context(|| format!("validate {} {role} pin", record.record_id))?;
                ensure!(
                    evidence_paths.insert(pin.path.as_str()),
                    "retained evidence path {:?} is repeated",
                    pin.path
                );
            }

            ensure!(
                record.gate_artifact_tombstones.len() == RETIRED_GATE_ARTIFACT_SUFFIXES.len(),
                "record {} must tombstone exactly {} gate artifacts, got {}",
                record.record_id,
                RETIRED_GATE_ARTIFACT_SUFFIXES.len(),
                record.gate_artifact_tombstones.len()
            );
            let gate_root = retired_gate_root(record);
            let expected_gate_paths = RETIRED_GATE_ARTIFACT_SUFFIXES
                .iter()
                .map(|suffix| format!("{gate_root}{suffix}"))
                .collect::<BTreeSet<_>>();
            let mut actual_gate_paths = BTreeSet::new();
            for pin in &record.gate_artifact_tombstones {
                validate_pin(pin)
                    .with_context(|| format!("validate {} gate tombstone", record.record_id))?;
                ensure!(
                    actual_gate_paths.insert(pin.path.clone()),
                    "record {} repeats gate tombstone {:?}",
                    record.record_id,
                    pin.path
                );
                ensure!(
                    tombstoned_paths.insert(pin.path.as_str()),
                    "gate tombstone path {:?} is owned by more than one record",
                    pin.path
                );
            }
            ensure!(
                actual_gate_paths == expected_gate_paths,
                "record {} gate tombstones do not match the exact retired artifact set",
                record.record_id
            );
            authority
                .directory_roots
                .insert(reference_relative_path(gate_root.trim_end_matches('/'))?.to_string());

            let expected_daily_path = retired_daily_run_spec_path(record);
            if let Some(pin) = &record.retired_daily_run_spec {
                validate_pin(pin).with_context(|| {
                    format!("validate {} retired daily RunSpec", record.record_id)
                })?;
                ensure!(
                    pin.path == expected_daily_path,
                    "daily RunSpec tombstone {:?} != expected {:?}",
                    pin.path,
                    expected_daily_path
                );
                ensure!(
                    tombstoned_paths.insert(pin.path.as_str()),
                    "daily RunSpec tombstone {:?} is repeated",
                    pin.path
                );
                authority
                    .exact_files
                    .insert(reference_relative_path(&pin.path)?.to_string());
            } else {
                expected_active_daily_run_specs.insert(expected_daily_path);
            }
        }

        ensure!(
            series_dates.keys().eq(coverage_by_series.keys()),
            "records must cover exactly every declared series_coverage entry"
        );
        for ((venue, instrument_id), dates) in &series_dates {
            let (_, first, last) = coverage_by_series
                .get(&(venue.clone(), instrument_id.clone()))
                .context("validated record series lost its coverage declaration")?;
            ensure!(
                dates.first() == Some(first) && dates.last() == Some(last),
                "retired series {venue}/{instrument_id} does not cover its declared endpoints {first}..={last}"
            );
            for pair in dates.windows(2) {
                ensure!(
                    pair[1] == pair[0] + Duration::days(1),
                    "retired series {venue}/{instrument_id} has a gap or overlap between {} and {}",
                    pair[0],
                    pair[1]
                );
            }
        }

        let mut active_daily_run_specs = BTreeSet::new();
        let mut previous_active: Option<&str> = None;
        for path in &self.retained_active_daily_run_specs {
            if let Some(previous) = previous_active {
                ensure!(
                    previous < path.as_str(),
                    "retained_active_daily_run_specs must be strictly sorted and unique"
                );
            }
            previous_active = Some(path);
            validate_repo_relative_reference_path(path)
                .context("validate retained active daily RunSpec path")?;
            ensure!(
                active_daily_run_specs.insert(path.clone()),
                "duplicate retained active daily RunSpec path {path:?}"
            );
        }
        ensure!(
            active_daily_run_specs == expected_active_daily_run_specs,
            "retained_active_daily_run_specs must exactly account for records without a retired daily RunSpec"
        );

        ensure!(
            !self.aggregate_scopes.is_empty(),
            "retired aggregate_scopes must not be empty"
        );
        let declared_series_scopes = self
            .series_coverage
            .iter()
            .map(aggregate_scope_for_coverage)
            .collect::<BTreeSet<_>>();
        let mut expected_aggregate_paths = BTreeSet::new();
        let mut scopes_by_root: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut previous_scope_key: Option<(&str, &str)> = None;
        for scope in &self.aggregate_scopes {
            validate_lowercase_slug(&scope.root, "aggregate root")?;
            validate_lowercase_slug(&scope.scope, "aggregate scope")?;
            let expected_roles = aggregate_roles_for_root(&scope.root).with_context(|| {
                format!("aggregate scope declares unknown root {:?}", scope.root)
            })?;
            let key = (scope.root.as_str(), scope.scope.as_str());
            if let Some(previous) = previous_scope_key {
                ensure!(
                    previous < key,
                    "aggregate_scopes must be strictly sorted and unique by root/scope"
                );
            }
            previous_scope_key = Some(key);
            ensure!(
                declared_series_scopes.contains(&scope.scope),
                "aggregate scope {:?} does not match a declared series date range",
                scope.scope
            );
            ensure!(
                !scope.artifact_roles.is_empty(),
                "aggregate scope {}/{} has no artifact_roles",
                scope.root,
                scope.scope
            );
            let mut roles = BTreeSet::new();
            let mut previous_role: Option<&str> = None;
            for role in &scope.artifact_roles {
                validate_portable_relative_path(role, "aggregate artifact role")?;
                if let Some(previous) = previous_role {
                    ensure!(
                        previous < role,
                        "aggregate artifact_roles must be strictly sorted and unique"
                    );
                }
                previous_role = Some(role);
                ensure!(
                    roles.insert(role.as_str()),
                    "duplicate aggregate role {role:?}"
                );
                expected_aggregate_paths.insert(format!(
                    "{REFERENCE_PREFIX}{}/{}/{}",
                    scope.root, scope.scope, role
                ));
            }
            ensure!(
                roles == expected_roles.iter().copied().collect(),
                "aggregate scope {}/{} does not declare the exact structural roles for its root",
                scope.root,
                scope.scope
            );
            scopes_by_root
                .entry(&scope.root)
                .or_default()
                .insert(&scope.scope);
            authority
                .directory_roots
                .insert(format!("{}/{}", scope.root, scope.scope));
        }
        for (root, scopes) in scopes_by_root {
            ensure!(
                scopes == declared_series_scopes.iter().map(String::as_str).collect(),
                "aggregate root {root:?} must cover every declared series"
            );
        }
        ensure!(
            self.aggregate_scopes
                .iter()
                .map(|scope| scope.root.as_str())
                .collect::<BTreeSet<_>>()
                == RETIRED_AGGREGATE_ROOT_ROLES
                    .iter()
                    .map(|(root, _)| *root)
                    .collect(),
            "aggregate_scopes must cover every structural aggregate root"
        );

        ensure!(
            !self.retired_aggregate_artifacts.is_empty(),
            "retired aggregate inventory must not be empty"
        );
        let mut actual_aggregate_paths = BTreeSet::new();
        for tombstone in &self.retired_aggregate_artifacts {
            validate_pin(&tombstone.artifact).context("validate retired aggregate tombstone")?;
            ensure!(
                actual_aggregate_paths.insert(tombstone.artifact.path.as_str()),
                "aggregate tombstone {:?} is repeated",
                tombstone.artifact.path
            );
            ensure!(
                tombstoned_paths.insert(tombstone.artifact.path.as_str()),
                "aggregate tombstone {:?} is repeated",
                tombstone.artifact.path
            );
        }
        ensure!(
            actual_aggregate_paths
                == expected_aggregate_paths
                    .iter()
                    .map(String::as_str)
                    .collect(),
            "retired aggregate tombstones must exactly match declared aggregate scopes and roles"
        );

        for record in &self.records {
            for pin in [
                &record.source_proof,
                &record.publication_evidence,
                &record.catalog_mapping_evaluation,
            ] {
                ensure!(
                    !authority.is_retired(Path::new(&pin.path)),
                    "retained evidence path {:?} must not be a retired runtime control",
                    pin.path
                );
            }
        }
        for path in &tombstoned_paths {
            ensure!(
                authority.is_retired(Path::new(*path)),
                "tombstone path {path:?} is outside the inventory-derived runtime authority"
            );
        }

        Ok(authority)
    }

    pub fn verify_retained_evidence(&self, repo_root: &Path) -> Result<()> {
        self.validate_structure()?;
        let tombstoned_paths = self.tombstoned_paths();
        let retained_active_daily_run_specs = self
            .retained_active_daily_run_specs
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for record in &self.records {
            let source_proof_bytes = verify_retained_pin(repo_root, &record.source_proof)?;
            let publication_bytes = verify_retained_pin(repo_root, &record.publication_evidence)?;
            let mapping_bytes = verify_retained_pin(repo_root, &record.catalog_mapping_evaluation)?;

            let source_proof: SourceProofReport = serde_json::from_slice(&source_proof_bytes)
                .with_context(|| format!("parse source proof {}", record.source_proof.path))?;
            ensure!(
                source_proof.status == SourceProofStatus::Accepted,
                "record {} source proof is not accepted",
                record.record_id
            );
            ensure!(
                source_proof.source_proof_version > 0
                    && source_proof.source_binding == record.source_binding
                    && source_proof.venue == record.venue.as_str()
                    && source_proof.table_family == EXPECTED_TABLE_FAMILY
                    && source_proof.coverage_time_range.start_utc
                        == format!("{}T00:00:00Z", record.archive_date),
                "record {} source proof identity/date does not match inventory",
                record.record_id
            );

            let publication: AcceptedPublicationEvidence =
                serde_json::from_slice(&publication_bytes).with_context(|| {
                    format!(
                        "parse accepted publication evidence {}",
                        record.publication_evidence.path
                    )
                })?;
            ensure!(
                publication.schema_version == "backtesting-accepted-publication-evidence.v1"
                    && publication.scope.status == ACCEPTED_PUBLICATION_STATUS
                    && publication.scope.source_binding == record.source_binding
                    && publication.scope.instrument_id == record.instrument_id
                    && publication.scope.table_family == EXPECTED_TABLE_FAMILY
                    && publication.scope.nt_data_type == EXPECTED_NT_DATA_TYPE
                    && publication.scope.archive_date == record.archive_date,
                "record {} publication scope does not match inventory",
                record.record_id
            );
            ensure!(
                publication.accepted_gate.source_proof_ref
                    == format!("repo://{}", record.source_proof.path),
                "record {} publication source-proof ref does not match retained proof",
                record.record_id
            );
            ensure!(
                publication
                    .accepted_conversion_and_publication
                    .source_proof_id
                    == source_proof.source_proof_id
                    && publication
                        .accepted_conversion_and_publication
                        .source_proof_version
                        == source_proof.source_proof_version
                    && publication
                        .accepted_conversion_and_publication
                        .published_catalog_direct_s3,
                "record {} publication result does not bind the accepted proof",
                record.record_id
            );

            let publication_value: serde_json::Value =
                serde_json::from_slice(&publication_bytes)
                    .context("parse accepted publication evidence for tombstone refs")?;
            let mut retired_refs = Vec::new();
            collect_retired_repo_refs(
                &publication_value,
                &retained_active_daily_run_specs,
                &mut retired_refs,
            );
            for path in retired_refs {
                ensure!(
                    tombstoned_paths.contains(path),
                    "record {} publication retains retired ref {:?} without a tombstone",
                    record.record_id,
                    path
                );
            }

            let mapping: CatalogMappingEvaluation = serde_json::from_slice(&mapping_bytes)
                .with_context(|| {
                    format!(
                        "parse catalog mapping evaluation {}",
                        record.catalog_mapping_evaluation.path
                    )
                })?;
            ensure!(
                mapping.source_sample_mapping_status.len() == 1,
                "record {} mapping evaluation must contain one status entry",
                record.record_id
            );
            let entry = &mapping.source_sample_mapping_status[0];
            ensure!(
                entry.source_proof_id.as_deref() == Some(source_proof.source_proof_id.as_str())
                    && entry.source_proof_version == Some(source_proof.source_proof_version)
                    && entry.source_binding == record.source_binding
                    && entry.usage_scope == Some(SourceProofUsageScope::CanonicalBackfillInput)
                    && entry.table_family == EXPECTED_TABLE_FAMILY
                    && entry
                        .candidate_nt_data_classes
                        .iter()
                        .any(|data_type| data_type == EXPECTED_NT_DATA_TYPE)
                    && entry.current_bte_status == "accepted"
                    && entry.parquet_catalog_status == "proven",
                "record {} catalog mapping does not bind the accepted proof/publication",
                record.record_id
            );
        }
        Ok(())
    }

    pub fn tombstoned_paths(&self) -> BTreeSet<&str> {
        self.records
            .iter()
            .flat_map(|record| {
                record
                    .gate_artifact_tombstones
                    .iter()
                    .map(|pin| pin.path.as_str())
                    .chain(
                        record
                            .retired_daily_run_spec
                            .iter()
                            .map(|pin| pin.path.as_str()),
                    )
            })
            .chain(
                self.retired_aggregate_artifacts
                    .iter()
                    .map(|tombstone| tombstone.artifact.path.as_str()),
            )
            .collect()
    }
}

/// Returns `true` only for the exact gate roots, daily profiles, and aggregate
/// roots authorized by the typed retirement inventory.
///
/// Absolute paths are checked at every normalized repository-reference marker,
/// so a nested marker cannot hide an enclosing retired root. Relative paths
/// must begin at the repository's reference root, preventing an unrelated
/// directory that happens to contain `backfill-gates` from inheriting
/// retirement authority.
pub fn is_retired_backfill_runtime_path(path: &Path) -> bool {
    match &*EMBEDDED_RUNTIME_AUTHORITY {
        Ok(authority) => authority.is_retired(path),
        Err(error) => panic!("embedded retired backfill runtime authority is invalid: {error}"),
    }
}

/// Recognizes repository references shaped like the retired lane's generic
/// control roles without consulting the tombstone-derived runtime authority.
/// This exists only to prove tombstone completeness; runtime admission must
/// use [`is_retired_backfill_runtime_path`].
pub fn is_retired_backfill_evidence_reference(path: &Path) -> bool {
    let Some(components) = lexically_normal_components(path) else {
        return false;
    };
    let Some(relative) = reference_relative_components(path, &components) else {
        return false;
    };
    if relative.len() >= 2 && relative[0] == "backfill-gates" {
        return true;
    }
    if let [file_name] = relative
        && file_name.starts_with("backtesting-vertical-slice-run-spec.")
        && file_name.ends_with(".toml")
    {
        return true;
    }
    relative
        .first()
        .is_some_and(|root| aggregate_roles_for_root(root).is_some())
}

impl RetiredBackfillRuntimeAuthority {
    fn is_retired(&self, path: &Path) -> bool {
        let Some(components) = lexically_normal_components(path) else {
            return false;
        };
        if path.is_absolute() {
            return components
                .windows(REFERENCE_COMPONENTS.len())
                .enumerate()
                .any(|(start, window)| {
                    window == REFERENCE_COMPONENTS
                        && self
                            .is_retired_relative(&components[start + REFERENCE_COMPONENTS.len()..])
                });
        }
        let Some(relative) = reference_relative_components(path, &components) else {
            return false;
        };
        self.is_retired_relative(relative)
    }

    fn is_retired_relative(&self, relative: &[&str]) -> bool {
        let relative = relative.join("/");
        self.exact_files.contains(&relative)
            || self.directory_roots.iter().any(|root| {
                relative == *root
                    || relative
                        .strip_prefix(root)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
    }
}

pub fn ensure_active_backfill_runtime_path(path: &Path) -> Result<()> {
    ensure!(
        !is_retired_backfill_runtime_path(path),
        "retired backfill runtime path is evidence-only and cannot be loaded: {}",
        path.display()
    );
    Ok(())
}

/// Reads one executable backfill control or one of its declared inputs through
/// the retirement boundary. Declared, base-relative, resolved, and canonical
/// identities are checked before bytes are returned, so path fallback or a
/// symlink cannot make a retired lane executable again.
pub(crate) fn read_active_backfill_runtime_input(
    base_dir: Option<&Path>,
    declared_path: &Path,
) -> Result<Vec<u8>> {
    let resolved_path = resolve_active_backfill_runtime_input(base_dir, declared_path)?;
    fs::read(&resolved_path).with_context(|| {
        format!(
            "read active backfill runtime input {}",
            resolved_path.display()
        )
    })
}

/// Resolves a declared backfill input only after every path identity which can
/// select it has passed the retirement boundary. Callers which need metadata
/// before reading use this same boundary instead of resolving independently.
pub(crate) fn resolve_active_backfill_runtime_input(
    base_dir: Option<&Path>,
    declared_path: &Path,
) -> Result<PathBuf> {
    ensure_active_backfill_runtime_path(declared_path)?;

    let resolved_path = if let Some(base_dir) = base_dir {
        if !declared_path.is_absolute() {
            ensure_active_backfill_runtime_path(&base_dir.join(declared_path))?;
            let absolute_base_dir = if base_dir.is_absolute() {
                base_dir.to_path_buf()
            } else {
                std::env::current_dir()
                    .context("resolve current directory for backfill input retirement guard")?
                    .join(base_dir)
            };
            ensure_active_backfill_runtime_path(&absolute_base_dir.join(declared_path))?;
        }
        resolve_existing_path(base_dir, declared_path)
    } else {
        if !declared_path.is_absolute() {
            let current_dir = std::env::current_dir()
                .context("resolve current directory for backfill input retirement guard")?;
            ensure_active_backfill_runtime_path(&current_dir.join(declared_path))?;
        }
        PathBuf::from(declared_path)
    };
    ensure_active_backfill_runtime_path(&resolved_path)?;
    let canonical_path = resolved_path.canonicalize().with_context(|| {
        format!(
            "canonicalize active backfill runtime input {}",
            resolved_path.display()
        )
    })?;
    ensure_active_backfill_runtime_path(&canonical_path)?;
    Ok(canonical_path)
}

/// Selects a final backfill artifact path only after its declared and
/// canonical identities have passed the retirement boundary. This must run
/// before directory creation, existing-artifact reads, or writes.
pub(crate) fn active_backfill_runtime_output_path(
    output_dir: &Path,
    artifact_file_name: &str,
) -> Result<PathBuf> {
    let artifact_file_path = Path::new(artifact_file_name);
    ensure!(
        matches!(
            artifact_file_path
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [Component::Normal(_)]
        ),
        "backfill artifact file name must be one portable path component: {artifact_file_name:?}"
    );
    let output_path = output_dir.join(artifact_file_path);
    ensure_active_backfill_runtime_path(&output_path)?;

    let absolute_output_path = if output_path.is_absolute() {
        output_path.clone()
    } else {
        std::env::current_dir()
            .context("resolve current directory for backfill output retirement guard")?
            .join(&output_path)
    };
    let canonical_output_path = resolve_planned_write_path(&absolute_output_path)?;
    ensure_active_backfill_runtime_path(&canonical_output_path)?;
    Ok(output_path)
}

fn reference_relative_components<'a>(
    path: &Path,
    components: &'a [&'a str],
) -> Option<&'a [&'a str]> {
    let reference_start = if path.is_absolute() {
        components
            .windows(REFERENCE_COMPONENTS.len())
            .rposition(|window| window == REFERENCE_COMPONENTS)?
    } else {
        if !components.starts_with(REFERENCE_COMPONENTS) {
            return None;
        }
        0
    };
    Some(&components[reference_start + REFERENCE_COMPONENTS.len()..])
}

fn lexically_normal_components(path: &Path) -> Option<Vec<&str>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => components.push(value.to_str()?),
            Component::ParentDir => {
                if components
                    .last()
                    .is_some_and(|component| *component != "..")
                {
                    components.pop();
                } else {
                    components.push("..");
                }
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Some(components)
}

fn validate_lowercase_slug(value: &str, field: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "{field} {value:?} must be a non-empty lowercase ASCII slug"
    );
    Ok(())
}

fn validate_instrument_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value == value.to_ascii_uppercase()
            && value
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') }),
        "instrument_id {value:?} must be one non-empty uppercase portable ASCII identifier"
    );
    Ok(())
}

fn canonical_instrument_id(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn validate_portable_relative_path(value: &str, field: &str) -> Result<()> {
    let path = Path::new(value);
    ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "{field} {value:?} must be a normalized portable relative path"
    );
    Ok(())
}

fn retired_scope(record: &RetiredBackfillEvidenceRecord) -> String {
    format!(
        "{}-{}-{}",
        record.venue,
        canonical_instrument_id(&record.instrument_id),
        record.archive_date
    )
}

fn aggregate_scope_for_coverage(coverage: &RetiredBackfillSeriesCoverage) -> String {
    format!(
        "{}-{}-{}-{}",
        coverage.venue,
        canonical_instrument_id(&coverage.instrument_id),
        coverage.first_archive_date,
        coverage.last_archive_date
    )
}

fn aggregate_roles_for_root(root: &str) -> Option<&'static [&'static str]> {
    RETIRED_AGGREGATE_ROOT_ROLES
        .iter()
        .find_map(|(candidate, roles)| (*candidate == root).then_some(*roles))
}

fn retired_record_id(record: &RetiredBackfillEvidenceRecord) -> String {
    format!("retired-backfill-evidence-{}", retired_scope(record))
}

fn retired_gate_root(record: &RetiredBackfillEvidenceRecord) -> String {
    format!(
        "{REFERENCE_PREFIX}backfill-gates/{}/",
        retired_scope(record)
    )
}

fn retired_daily_run_spec_path(record: &RetiredBackfillEvidenceRecord) -> String {
    format!(
        "{REFERENCE_PREFIX}backtesting-vertical-slice-run-spec.{}.toml",
        retired_scope(record)
    )
}

fn validate_repo_relative_reference_path(path: &str) -> Result<()> {
    ensure!(
        path.starts_with(REFERENCE_PREFIX)
            && !path.starts_with('/')
            && !path.contains('\\')
            && !path
                .split('/')
                .any(|component| component.is_empty() || component == "." || component == ".."),
        "path {path:?} must be a normalized repo-relative path under {REFERENCE_PREFIX}"
    );
    Ok(())
}

fn reference_relative_path(path: &str) -> Result<&str> {
    validate_repo_relative_reference_path(path)?;
    path.strip_prefix(REFERENCE_PREFIX)
        .context("reference path lost its validated prefix")
}

fn validate_pin(pin: &RetiredBackfillArtifactPin) -> Result<()> {
    validate_repo_relative_reference_path(&pin.path)
        .with_context(|| format!("validate artifact pin path {:?}", pin.path))?;
    ensure!(
        is_lowercase_sha256_hex(&pin.sha256),
        "artifact pin {:?} has malformed sha256 {:?}",
        pin.path,
        pin.sha256
    );
    ensure!(pin.bytes > 0, "artifact pin {:?} has zero bytes", pin.path);
    Ok(())
}

fn verify_retained_pin(repo_root: &Path, pin: &RetiredBackfillArtifactPin) -> Result<Vec<u8>> {
    let path = repo_root.join(&pin.path);
    let bytes = fs::read(&path)
        .with_context(|| format!("read retained evidence artifact {}", path.display()))?;
    ensure!(
        u64::try_from(bytes.len()).context("retained evidence length does not fit u64")?
            == pin.bytes,
        "retained evidence {} byte length changed",
        pin.path
    );
    ensure!(
        sha256_hex(&bytes) == pin.sha256,
        "retained evidence {} SHA-256 changed",
        pin.path
    );
    Ok(bytes)
}

fn parse_archive_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("parse archive_date {value:?}"))
}

fn collect_retired_repo_refs<'a>(
    value: &'a serde_json::Value,
    retained_active_daily_run_specs: &BTreeSet<&str>,
    paths: &mut Vec<&'a str>,
) {
    match value {
        serde_json::Value::String(value) => {
            if let Some(path) = value.strip_prefix("repo://")
                && is_retired_backfill_evidence_reference(Path::new(path))
                && !retained_active_daily_run_specs
                    .iter()
                    .any(|active| *active == path)
            {
                paths.push(path);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_retired_repo_refs(value, retained_active_daily_run_specs, paths);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_retired_repo_refs(value, retained_active_daily_run_specs, paths);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

#[derive(Debug, Deserialize)]
struct AcceptedPublicationEvidence {
    schema_version: String,
    scope: AcceptedPublicationScope,
    accepted_gate: AcceptedPublicationGate,
    accepted_conversion_and_publication: AcceptedConversionAndPublication,
}

#[derive(Debug, Deserialize)]
struct AcceptedPublicationScope {
    status: String,
    source_binding: String,
    instrument_id: String,
    table_family: String,
    nt_data_type: String,
    archive_date: String,
}

#[derive(Debug, Deserialize)]
struct AcceptedPublicationGate {
    source_proof_ref: String,
}

#[derive(Debug, Deserialize)]
struct AcceptedConversionAndPublication {
    source_proof_id: String,
    source_proof_version: u32,
    published_catalog_direct_s3: bool,
}

#[derive(Debug, Deserialize)]
struct CatalogMappingEvaluation {
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        EMBEDDED_RETIRED_BACKFILL_EVIDENCE_INVENTORY, RetiredBackfillEvidenceInventory,
        RetiredBackfillEvidenceRecord, resolve_active_backfill_runtime_input,
    };

    #[test]
    fn inventory_extension_alone_can_retire_a_third_venue() {
        let mut inventory =
            RetiredBackfillEvidenceInventory::parse(EMBEDDED_RETIRED_BACKFILL_EVIDENCE_INVENTORY)
                .expect("embedded retirement inventory parses");
        let mut third = inventory.records[1].clone();
        remap_record_identity(
            &mut third,
            "binance",
            "bnbusdc",
            "2026-03-02",
            "okx",
            "btcusdt",
            "2027-01-01",
        );
        third.venue = "okx".to_string();
        third.instrument_id = "BTCUSDT".to_string();
        third.source_binding = "okx-spot-native-trades".to_string();
        third.record_id = "retired-backfill-evidence-okx-btcusdt-2027-01-01".to_string();
        inventory.records.push(third);

        let mut coverage = inventory.series_coverage[0].clone();
        coverage.venue = "okx".to_string();
        coverage.instrument_id = "BTCUSDT".to_string();
        coverage.source_binding = "okx-spot-native-trades".to_string();
        coverage.first_archive_date = "2027-01-01".to_string();
        coverage.last_archive_date = "2027-01-01".to_string();
        inventory.series_coverage.push(coverage);
        inventory.series_coverage.sort_by(|left, right| {
            (&left.venue, &left.instrument_id).cmp(&(&right.venue, &right.instrument_id))
        });

        let old_aggregate_scope = "binance-bnbusdc-2026-03-01-2026-05-31";
        let new_aggregate_scope = "okx-btcusdt-2027-01-01-2027-01-01";
        let third_scopes = inventory
            .aggregate_scopes
            .iter()
            .filter(|scope| scope.scope == old_aggregate_scope)
            .cloned()
            .map(|mut scope| {
                scope.scope = new_aggregate_scope.to_string();
                scope
            })
            .collect::<Vec<_>>();
        inventory.aggregate_scopes.extend(third_scopes);
        inventory
            .aggregate_scopes
            .sort_by(|left, right| (&left.root, &left.scope).cmp(&(&right.root, &right.scope)));
        let third_aggregate_artifacts = inventory
            .retired_aggregate_artifacts
            .iter()
            .filter(|tombstone| tombstone.artifact.path.contains(old_aggregate_scope))
            .cloned()
            .map(|mut tombstone| {
                tombstone.artifact.path = tombstone
                    .artifact
                    .path
                    .replace(old_aggregate_scope, new_aggregate_scope);
                tombstone
            })
            .collect::<Vec<_>>();
        inventory
            .retired_aggregate_artifacts
            .extend(third_aggregate_artifacts);

        let authority = inventory
            .validated_runtime_authority()
            .expect("provider-neutral third-venue inventory validates");
        for path in [
            "specs/023-nt-research-analytics-platform/reference/backfill-gates/okx-btcusdt-2027-01-01/arbitrary/new-control.toml",
            "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.okx-btcusdt-2027-01-01.toml",
            "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/okx-btcusdt-2027-01-01-2027-01-01/arbitrary/new-control.toml",
        ] {
            assert!(authority.is_retired(Path::new(path)), "{path}");
        }
    }

    fn remap_record_identity(
        record: &mut RetiredBackfillEvidenceRecord,
        old_venue: &str,
        old_instrument: &str,
        old_date: &str,
        new_venue: &str,
        new_instrument: &str,
        new_date: &str,
    ) {
        for pin in [
            &mut record.source_proof,
            &mut record.publication_evidence,
            &mut record.catalog_mapping_evaluation,
        ] {
            pin.path = pin
                .path
                .replace(old_venue, new_venue)
                .replace(old_instrument, new_instrument)
                .replace(old_date, new_date);
        }
        for pin in &mut record.gate_artifact_tombstones {
            pin.path = pin
                .path
                .replace(old_venue, new_venue)
                .replace(old_instrument, new_instrument)
                .replace(old_date, new_date);
        }
        if let Some(pin) = &mut record.retired_daily_run_spec {
            pin.path = pin
                .path
                .replace(old_venue, new_venue)
                .replace(old_instrument, new_instrument)
                .replace(old_date, new_date);
        }
        record.archive_date = new_date.to_string();
    }

    #[test]
    fn relative_base_and_missing_nested_tombstone_fail_at_the_retirement_wall() {
        let base_dir = Path::new(
            "../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02",
        );
        let nested = Path::new("source-proof-scope/backfill-source-proof-scope-report.json");

        let error = resolve_active_backfill_runtime_input(Some(base_dir), nested)
            .expect_err("missing nested tombstone must reject before filesystem access");

        assert!(error.to_string().contains("retired backfill"), "{error:#}");
        assert!(!error.to_string().contains("No such file"), "{error:#}");
    }

    #[cfg(unix)]
    #[test]
    fn active_alias_to_an_arbitrary_retired_descendant_fails_on_canonical_identity() {
        let temp = tempfile::tempdir().expect("create temporary retirement root");
        let retired = temp.path().join(
            "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/arbitrary/new-runtime-control.toml",
        );
        std::fs::create_dir_all(retired.parent().expect("retired input parent"))
            .expect("create retired input parent");
        std::fs::write(&retired, b"{}\n").expect("write retired input");
        let alias = temp.path().join("active-input-alias.json");
        std::os::unix::fs::symlink(&retired, &alias).expect("create active alias");

        let error = resolve_active_backfill_runtime_input(None, &alias)
            .expect_err("canonical retired target must reject before read");

        assert!(error.to_string().contains("retired backfill"), "{error:#}");
    }
}
