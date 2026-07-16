//! Typed evidence authority for the retired per-day BNBUSDC backfill lane.
//!
//! The old lane materialized one executable RunSpec/plan/readiness tree per
//! venue-day. Those controls are no longer runnable. The committed inventory
//! retains exact fingerprints for their historical provenance while binding
//! each day to the independent source-proof, publication, and NT-mapping
//! evidence which remains in the repository.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::{
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    path_resolution::resolve_existing_path,
    source_catalog_mapping_readiness::SourceCatalogMappingStatusEntry,
    source_proof::{SourceProofReport, SourceProofStatus, SourceProofUsageScope},
};

pub const RETIRED_BACKFILL_EVIDENCE_INVENTORY_PATH: &str =
    "specs/023-nt-research-analytics-platform/reference/retired-backfill-evidence.inventory.json";
pub const RETIRED_BACKFILL_EVIDENCE_SCHEMA_VERSION: &str = "retired-backfill-evidence-inventory.v1";

const REFERENCE_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/";
const RETIRED_GATE_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/backfill-gates/";
const EXPECTED_INSTRUMENT_ID: &str = "BNBUSDC";
const EXPECTED_TABLE_FAMILY: &str = "trades";
const EXPECTED_NT_DATA_TYPE: &str = "TradeTick";
const ACCEPTED_PUBLICATION_STATUS: &str = "accepted_gate_committed_and_s3_published";
const RETIRED_BINANCE_FIRST_DATE: &str = "2026-03-01";
const RETIRED_BINANCE_LAST_DATE: &str = "2026-05-31";
const RETIRED_BYBIT_FIRST_DATE: &str = "2026-03-01";
const RETIRED_BYBIT_LAST_DATE: &str = "2026-06-01";
const REFERENCE_COMPONENTS: &[&str] = &["specs", "023-nt-research-analytics-platform", "reference"];

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

const RETIRED_AGGREGATE_ARTIFACTS: &[(&str, &str, &str)] = &[
    (
        "backfill-conversion-batches",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-conversion-batch-plan.toml",
    ),
    (
        "backfill-conversion-batches",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "plan/backfill-conversion-batch-plan.json",
    ),
    (
        "backfill-conversion-batches",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-conversion-batch-plan.toml",
    ),
    (
        "backfill-conversion-batches",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "plan/backfill-conversion-batch-plan.json",
    ),
    (
        "backfill-coverage-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-coverage-ledger.toml",
    ),
    (
        "backfill-coverage-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "ledger/backfill-coverage-ledger.json",
    ),
    (
        "backfill-coverage-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-coverage-ledger.toml",
    ),
    (
        "backfill-coverage-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "ledger/backfill-coverage-ledger.json",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-conversion-completion-ledger.toml",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "ledger/backfill-conversion-completion-ledger.json",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-conversion-completion-ledger.toml",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "ledger/backfill-conversion-completion-ledger.json",
    ),
];

const ACTIVE_GOLDEN_BINANCE_PROFILE: &str =
    "backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetiredBackfillVenue {
    Binance,
    Bybit,
}

impl RetiredBackfillVenue {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
        }
    }

    const fn source_binding(self) -> &'static str {
        match self {
            Self::Binance => "binance-spot-native-trades",
            Self::Bybit => "bybit-spot-tick-trades",
        }
    }
}

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
    pub venue: RetiredBackfillVenue,
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
pub struct RetiredBackfillEvidenceInventory {
    pub schema_version: String,
    pub inventory_id: String,
    pub records: Vec<RetiredBackfillEvidenceRecord>,
    pub retired_aggregate_artifacts: Vec<RetiredBackfillAggregateTombstone>,
}

impl RetiredBackfillEvidenceInventory {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("parse retired backfill evidence inventory JSON")
    }

    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(RETIRED_BACKFILL_EVIDENCE_INVENTORY_PATH);
        let bytes = fs::read(&path)
            .with_context(|| format!("read retired backfill inventory {}", path.display()))?;
        let inventory = Self::parse(&bytes)?;
        inventory
            .validate_structure()
            .with_context(|| format!("validate retired backfill inventory {}", path.display()))?;
        Ok(inventory)
    }

    pub fn validate_structure(&self) -> Result<()> {
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
            self.records.len() == 185,
            "retired backfill inventory must contain exactly 185 records, got {}",
            self.records.len()
        );

        let mut previous_key: Option<(RetiredBackfillVenue, NaiveDate)> = None;
        let mut record_ids = BTreeSet::new();
        let mut evidence_paths = BTreeSet::new();
        let mut tombstoned_paths = BTreeSet::new();
        for record in &self.records {
            let archive_date = parse_archive_date(&record.archive_date)?;
            let key = (record.venue, archive_date);
            if let Some(previous) = previous_key {
                ensure!(
                    previous < key,
                    "retired backfill records must be strictly sorted and unique by venue/date"
                );
            }
            previous_key = Some(key);
            ensure!(
                record_ids.insert(record.record_id.as_str()),
                "duplicate retired backfill record_id {:?}",
                record.record_id
            );
            ensure!(
                record.instrument_id == EXPECTED_INSTRUMENT_ID,
                "record {} instrument_id must be {EXPECTED_INSTRUMENT_ID}",
                record.record_id
            );
            ensure!(
                record.source_binding == record.venue.source_binding(),
                "record {} source_binding does not match venue",
                record.record_id
            );
            let expected_record_id = format!(
                "retired-backfill-evidence-{}-bnbusdc-{}",
                record.venue.as_str(),
                record.archive_date
            );
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
                ensure!(
                    !is_retired_backfill_runtime_path(Path::new(&pin.path)),
                    "retained evidence path {:?} must not be a retired runtime control",
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
            let gate_root = format!(
                "{RETIRED_GATE_PREFIX}{}-bnbusdc-{}/",
                record.venue.as_str(),
                record.archive_date
            );
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

            let daily_profile_should_be_retired = !(record.venue == RetiredBackfillVenue::Binance
                && record.archive_date == "2026-03-01");
            ensure!(
                record.retired_daily_run_spec.is_some() == daily_profile_should_be_retired,
                "record {} retired_daily_run_spec presence is incorrect",
                record.record_id
            );
            if let Some(pin) = &record.retired_daily_run_spec {
                validate_pin(pin).with_context(|| {
                    format!("validate {} retired daily RunSpec", record.record_id)
                })?;
                ensure!(
                    is_retired_daily_run_spec_path(Path::new(&pin.path)),
                    "daily RunSpec tombstone {:?} is not a retired daily profile",
                    pin.path
                );
                ensure!(
                    tombstoned_paths.insert(pin.path.as_str()),
                    "daily RunSpec tombstone {:?} is repeated",
                    pin.path
                );
            }
        }

        validate_exact_date_range(
            &self.records,
            RetiredBackfillVenue::Binance,
            RETIRED_BINANCE_FIRST_DATE,
            RETIRED_BINANCE_LAST_DATE,
            92,
        )?;
        validate_exact_date_range(
            &self.records,
            RetiredBackfillVenue::Bybit,
            RETIRED_BYBIT_FIRST_DATE,
            RETIRED_BYBIT_LAST_DATE,
            93,
        )?;

        ensure!(
            self.retired_aggregate_artifacts.len() == 12,
            "retired aggregate inventory must contain exactly 12 artifacts, got {}",
            self.retired_aggregate_artifacts.len()
        );
        let expected_aggregate_paths = RETIRED_AGGREGATE_ARTIFACTS
            .iter()
            .map(|(root, scope, suffix)| format!("{REFERENCE_PREFIX}{root}/{scope}/{suffix}"))
            .collect::<BTreeSet<_>>();
        let actual_aggregate_paths = self
            .retired_aggregate_artifacts
            .iter()
            .map(|tombstone| tombstone.artifact.path.clone())
            .collect::<BTreeSet<_>>();
        ensure!(
            actual_aggregate_paths == expected_aggregate_paths,
            "retired aggregate tombstones do not match the exact runtime authority"
        );
        for tombstone in &self.retired_aggregate_artifacts {
            validate_pin(&tombstone.artifact).context("validate retired aggregate tombstone")?;
            ensure!(
                is_retired_backfill_runtime_path(Path::new(&tombstone.artifact.path)),
                "aggregate tombstone {:?} is outside the retired runtime scope",
                tombstone.artifact.path
            );
            ensure!(
                tombstoned_paths.insert(tombstone.artifact.path.as_str()),
                "aggregate tombstone {:?} is repeated",
                tombstone.artifact.path
            );
        }
        Ok(())
    }

    pub fn verify_retained_evidence(&self, repo_root: &Path) -> Result<()> {
        self.validate_structure()?;
        let tombstoned_paths = self.tombstoned_paths();
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
            collect_retired_repo_refs(&publication_value, &mut retired_refs);
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

/// Returns `true` only for the exact gate, daily-profile, and aggregate
/// identities authorized by the typed retirement inventory.
///
/// Absolute checkout paths are matched by their normalized repository suffix;
/// relative paths must begin at the repository's reference root, preventing an
/// unrelated directory that happens to contain `backfill-gates` from inheriting
/// retirement authority.
pub fn is_retired_backfill_runtime_path(path: &Path) -> bool {
    let Some(components) = lexically_normal_components(path) else {
        return false;
    };
    let Some(relative) = reference_relative_components(path, &components) else {
        return false;
    };
    is_retired_gate_path(relative)
        || is_retired_daily_run_spec_components(relative)
        || is_retired_aggregate_path(relative)
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
    let canonical_output_path = canonicalize_with_missing_tail(&absolute_output_path)?;
    ensure_active_backfill_runtime_path(&canonical_output_path)?;
    Ok(output_path)
}

fn canonicalize_with_missing_tail(path: &Path) -> Result<PathBuf> {
    let mut cursor = path;
    let mut missing_tail = Vec::new();
    loop {
        match cursor.canonicalize() {
            Ok(mut canonical) => {
                for component in missing_tail.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = cursor.file_name().with_context(|| {
                    format!(
                        "backfill output path {} has no existing canonical ancestor",
                        path.display()
                    )
                })?;
                missing_tail.push(component.to_os_string());
                cursor = cursor.parent().with_context(|| {
                    format!(
                        "backfill output path {} has no parent while resolving canonical identity",
                        path.display()
                    )
                })?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "canonicalize backfill output path identity {}",
                        cursor.display()
                    )
                });
            }
        }
    }
}

fn is_retired_daily_run_spec_path(path: &Path) -> bool {
    let Some(components) = lexically_normal_components(path) else {
        return false;
    };
    let Some(relative) = reference_relative_components(path, &components) else {
        return false;
    };
    is_retired_daily_run_spec_components(relative)
}

fn is_retired_gate_path(relative: &[&str]) -> bool {
    if relative.len() < 3 || relative[0] != "backfill-gates" {
        return false;
    }
    let Some((venue, date)) = retired_gate_venue_and_date(relative[1]) else {
        return false;
    };
    is_retired_venue_date(venue, date)
        && RETIRED_GATE_ARTIFACT_SUFFIXES
            .iter()
            .any(|suffix| components_equal_path(&relative[2..], suffix))
}

fn is_retired_daily_run_spec_components(relative: &[&str]) -> bool {
    let [file_name] = relative else {
        return false;
    };
    if *file_name == ACTIVE_GOLDEN_BINANCE_PROFILE {
        return false;
    }
    let (venue, date_and_suffix) = if let Some(value) =
        file_name.strip_prefix("backtesting-vertical-slice-run-spec.binance-bnbusdc-")
    {
        (RetiredBackfillVenue::Binance, value)
    } else if let Some(value) =
        file_name.strip_prefix("backtesting-vertical-slice-run-spec.bybit-bnbusdc-")
    {
        (RetiredBackfillVenue::Bybit, value)
    } else {
        return false;
    };
    let Some(date) = date_and_suffix.strip_suffix(".toml") else {
        return false;
    };
    is_retired_venue_date(venue, date)
}

fn is_retired_aggregate_path(relative: &[&str]) -> bool {
    if relative.len() < 3 {
        return false;
    }
    RETIRED_AGGREGATE_ARTIFACTS
        .iter()
        .any(|(root, scope, suffix)| {
            relative[0] == *root
                && relative[1] == *scope
                && components_equal_path(&relative[2..], suffix)
        })
}

fn retired_gate_venue_and_date(scope: &str) -> Option<(RetiredBackfillVenue, &str)> {
    if let Some(date) = scope.strip_prefix("binance-bnbusdc-") {
        Some((RetiredBackfillVenue::Binance, date))
    } else {
        scope
            .strip_prefix("bybit-bnbusdc-")
            .map(|date| (RetiredBackfillVenue::Bybit, date))
    }
}

fn is_retired_venue_date(venue: RetiredBackfillVenue, date: &str) -> bool {
    if date.len() != "YYYY-MM-DD".len() || NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return false;
    }
    let (first, last) = match venue {
        RetiredBackfillVenue::Binance => (RETIRED_BINANCE_FIRST_DATE, RETIRED_BINANCE_LAST_DATE),
        RetiredBackfillVenue::Bybit => (RETIRED_BYBIT_FIRST_DATE, RETIRED_BYBIT_LAST_DATE),
    };
    date >= first && date <= last
}

fn components_equal_path(components: &[&str], slash_path: &str) -> bool {
    components.iter().copied().eq(slash_path.split('/'))
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

fn validate_pin(pin: &RetiredBackfillArtifactPin) -> Result<()> {
    ensure!(
        pin.path.starts_with(REFERENCE_PREFIX)
            && !pin.path.starts_with('/')
            && !pin.path.contains(".."),
        "artifact pin path {:?} must be repo-relative under {REFERENCE_PREFIX}",
        pin.path
    );
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

fn validate_exact_date_range(
    records: &[RetiredBackfillEvidenceRecord],
    venue: RetiredBackfillVenue,
    first: &str,
    last: &str,
    expected_count: usize,
) -> Result<()> {
    let dates = records
        .iter()
        .filter(|record| record.venue == venue)
        .map(|record| parse_archive_date(&record.archive_date))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        dates.len() == expected_count,
        "{} inventory must contain exactly {expected_count} records, got {}",
        venue.as_str(),
        dates.len()
    );
    let first = parse_archive_date(first)?;
    let last = parse_archive_date(last)?;
    ensure!(
        dates.first() == Some(&first) && dates.last() == Some(&last),
        "{} inventory date endpoints are not {first}..={last}",
        venue.as_str()
    );
    for (offset, actual) in dates.iter().enumerate() {
        let offset = i64::try_from(offset).context("date offset does not fit i64")?;
        ensure!(
            *actual == first + Duration::days(offset),
            "{} inventory has a gap or overlap at {actual}",
            venue.as_str()
        );
    }
    Ok(())
}

fn collect_retired_repo_refs<'a>(value: &'a serde_json::Value, paths: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(value) => {
            if let Some(path) = value.strip_prefix("repo://") {
                if is_retired_backfill_runtime_path(Path::new(path)) {
                    paths.push(path);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_retired_repo_refs(value, paths);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_retired_repo_refs(value, paths);
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

    use super::resolve_active_backfill_runtime_input;

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
    fn active_alias_to_an_existing_retired_input_fails_on_canonical_identity() {
        let temp = tempfile::tempdir().expect("create temporary retirement root");
        let retired = temp.path().join(
            "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/source-proof-scope/backfill-source-proof-scope-report.json",
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
