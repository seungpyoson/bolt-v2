//! Venue/source-universe conversion acceptance ledger.
//!
//! This ledger is the roll-up above per-day conversion completion ledgers. It
//! records what is converted, what is only source-accepted, and what is blocked.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::backfill_conversion_completion::{
    BACKFILL_CONVERSION_COMPLETION_SCHEMA_VERSION, BackfillConversionCompletionLedger,
    BackfillConversionCompletionStatus,
};
use crate::hashing::{is_lowercase_sha256_hex, sha256_hex};
use crate::path_resolution::{
    portable_artifact_path, resolve_existing_path, resolve_output_dir,
    stable_artifact_identity_path_for_spec,
};
use crate::reference_artifact::ReferenceArtifactPin;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const VENUE_SCALE_CONVERSION_ACCEPTANCE_SCHEMA_VERSION: &str =
    "venue-scale-conversion-acceptance-ledger.v2";
pub const VENUE_SCALE_CONVERSION_ACCEPTANCE_LEDGER_FILE: &str =
    "venue-scale-conversion-acceptance-ledger.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceLedgerSpec {
    pub ledger_id: String,
    pub output_dir: PathBuf,
    #[serde(rename = "venue", default)]
    pub venues: Vec<VenueScaleConversionAcceptanceVenueSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceVenueSpec {
    pub venue_id: String,
    pub venue: String,
    #[serde(rename = "universe", default)]
    pub universes: Vec<VenueScaleConversionAcceptanceUniverseSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceUniverseSpec {
    pub universe_id: String,
    pub scope_label: String,
    pub status: VenueScaleConversionAcceptanceStatus,
    pub completion_ledger_path: Option<PathBuf>,
    pub completion_ledger_artifact_path: Option<PathBuf>,
    pub source_archive_discovery_seed_path: Option<PathBuf>,
    pub source_archive_discovery_seed_artifact_path: Option<PathBuf>,
    pub source_archive_index_manifest_path: Option<PathBuf>,
    pub source_archive_index_manifest_artifact_path: Option<PathBuf>,
    pub source_universe_manifest_path: Option<PathBuf>,
    pub source_universe_manifest_artifact_path: Option<PathBuf>,
    pub source_universe_conversion_queue_path: Option<PathBuf>,
    pub source_universe_conversion_queue_artifact_path: Option<PathBuf>,
    pub source_universe_source_proof_set_path: Option<PathBuf>,
    pub source_universe_source_proof_set_artifact_path: Option<PathBuf>,
    pub source_universe_object_gates_path: Option<PathBuf>,
    pub source_universe_object_gates_artifact_path: Option<PathBuf>,
    pub source_universe_conversion_run_plan_path: Option<PathBuf>,
    pub source_universe_conversion_run_plan_artifact_path: Option<PathBuf>,
    pub selected_source_report_path: Option<PathBuf>,
    pub selected_source_report_artifact_path: Option<PathBuf>,
    #[serde(default)]
    pub blocking_issues: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueScaleConversionAcceptanceStatus {
    Converted,
    PartiallyConverted,
    SourceOnly,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceCategorySummary {
    pub category: String,
    pub source_binding: String,
    pub instrument_count: u64,
    pub object_count: u64,
    pub compressed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceUniverse {
    pub universe_id: String,
    pub scope_label: String,
    pub status: VenueScaleConversionAcceptanceStatus,
    pub completion_ledger_id: Option<String>,
    pub source_archive_discovery_seed_id: Option<String>,
    pub source_archive_discovery_seed_source_binding_count: u64,
    pub source_archive_discovery_seed_representative_object_count: u64,
    pub source_archive_index_manifest_id: Option<String>,
    pub source_archive_index_snapshot_id: Option<String>,
    pub source_archive_index_object_count: u64,
    pub source_archive_index_verified_head_count: u64,
    pub source_archive_index_total_content_length_bytes: u64,
    pub source_manifest_id: Option<String>,
    pub source_conversion_queue_id: Option<String>,
    pub source_conversion_queue_work_item_count: u64,
    pub source_conversion_queue_pending_items: u64,
    pub source_conversion_queue_total_bytes: u64,
    pub source_proof_set_id: Option<String>,
    pub source_proof_count: u64,
    pub source_accepted_proof_count: u64,
    pub source_proof_completed_objects: u64,
    pub source_proof_accepted_bytes: u64,
    pub source_object_gate_id: Option<String>,
    pub source_object_gate_queue_id: Option<String>,
    pub source_conversion_run_plan_id: Option<String>,
    pub source_conversion_run_count: u64,
    pub source_conversion_run_object_count: u64,
    pub source_conversion_run_planned_bytes: u64,
    pub converted_record_count: u64,
    pub converted_canonical_rows: u64,
    pub converted_nt_catalog_rows: u64,
    pub source_object_count: u64,
    pub source_object_gate_count: u64,
    pub source_object_gate_source_binding_count: u64,
    pub source_accepted_bytes: u64,
    pub selected_source_rows: Option<u64>,
    pub selected_source_row_groups: Option<u64>,
    pub selected_projected_row_groups: Option<u64>,
    pub selected_rows: Option<u64>,
    pub selected_asset_count: Option<u64>,
    pub category_summaries: Vec<VenueScaleConversionAcceptanceCategorySummary>,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub blocking_issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceVenue {
    pub venue_id: String,
    pub venue: String,
    pub status: VenueScaleConversionAcceptanceStatus,
    pub universe_count: u64,
    pub converted_universes: u64,
    pub source_only_universes: u64,
    pub blocked_universes: u64,
    pub total_converted_records: u64,
    pub total_converted_canonical_rows: u64,
    pub total_converted_nt_catalog_rows: u64,
    pub total_source_only_objects: u64,
    pub total_source_only_object_gates: u64,
    pub total_source_only_accepted_bytes: u64,
    pub universes: Vec<VenueScaleConversionAcceptanceUniverse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueScaleConversionAcceptanceLedger {
    pub schema_version: String,
    pub ledger_id: String,
    pub status: VenueScaleConversionAcceptanceStatus,
    pub venue_count: u64,
    pub universe_count: u64,
    pub converted_universes: u64,
    pub source_only_universes: u64,
    pub blocked_universes: u64,
    pub total_converted_records: u64,
    pub total_converted_canonical_rows: u64,
    pub total_converted_nt_catalog_rows: u64,
    pub total_source_only_objects: u64,
    pub total_source_only_object_gates: u64,
    pub total_source_only_accepted_bytes: u64,
    pub venues: Vec<VenueScaleConversionAcceptanceVenue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueScaleConversionAcceptanceLedgerArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub venue_count: u64,
    pub universe_count: u64,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseManifestSummary {
    manifest_id: String,
    universe_id: String,
    object_count: u64,
    accepted_bytes: u64,
    #[serde(default)]
    category_summaries: Vec<SourceUniverseCategorySummary>,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseConversionQueueSummary {
    queue_id: String,
    status: String,
    manifest_id: String,
    universe_id: String,
    work_item_count: u64,
    pending_conversion_items: u64,
    total_source_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseSourceProofSetSummary {
    proof_set_id: String,
    proof_count: u64,
    accepted_proof_count: u64,
    total_completed_objects: u64,
    total_accepted_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SourceArchiveDiscoverySeedSummary {
    discovery_id: String,
    status: String,
    source_binding_count: u64,
    representative_object_count: u64,
}

#[derive(Debug, Deserialize)]
struct SourceArchiveIndexManifestSummary {
    manifest_id: String,
    status: String,
    snapshot_id: String,
    object_count: u64,
    verified_head_count: u64,
    total_content_length_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseObjectGateSummary {
    gate_id: String,
    status: String,
    queue_id: String,
    manifest_id: String,
    universe_id: String,
    work_item_count: u64,
    accepted_gate_count: u64,
    source_binding_count: u64,
    total_accepted_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseConversionRunPlanSummary {
    plan_id: String,
    status: String,
    gate_id: String,
    run_count: u64,
    source_binding_count: u64,
    planned_object_count: u64,
    planned_source_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseCategorySummary {
    category: String,
    source_binding: String,
    instrument_count: u64,
    object_count: u64,
    compressed_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SelectedSourceReportSummary {
    source_rows: u64,
    source_row_groups: u64,
    projected_row_groups: u64,
    selected_rows: u64,
    selected_asset_count: u64,
}

pub fn write_venue_scale_conversion_acceptance_ledger_from_spec_file(
    spec_path: &Path,
) -> Result<VenueScaleConversionAcceptanceLedgerArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read venue-scale conversion acceptance spec {}",
            spec_path.display()
        )
    })?;
    let spec: VenueScaleConversionAcceptanceLedgerSpec = toml::from_slice(&spec_bytes)
        .with_context(|| {
            format!(
                "parse venue-scale conversion acceptance spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_venue_scale_conversion_acceptance_ledger(&spec, base_dir)
}

pub fn write_venue_scale_conversion_acceptance_ledger(
    spec: &VenueScaleConversionAcceptanceLedgerSpec,
    base_dir: &Path,
) -> Result<VenueScaleConversionAcceptanceLedgerArtifact> {
    let ledger = evaluate_venue_scale_conversion_acceptance_ledger(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create venue-scale conversion acceptance ledger directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(VENUE_SCALE_CONVERSION_ACCEPTANCE_LEDGER_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        VENUE_SCALE_CONVERSION_ACCEPTANCE_LEDGER_FILE,
        &ledger,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| {
        format!(
            "write venue-scale conversion acceptance ledger {}",
            path.display()
        )
    })?;

    Ok(VenueScaleConversionAcceptanceLedgerArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        venue_count: ledger.venue_count,
        universe_count: ledger.universe_count,
    })
}

pub fn evaluate_venue_scale_conversion_acceptance_ledger(
    spec: &VenueScaleConversionAcceptanceLedgerSpec,
    base_dir: &Path,
) -> Result<VenueScaleConversionAcceptanceLedger> {
    ensure!(
        !spec.ledger_id.trim().is_empty(),
        "ledger_id must not be empty"
    );
    ensure!(!spec.venues.is_empty(), "venue set must not be empty");

    let mut venues = Vec::with_capacity(spec.venues.len());
    for venue_spec in &spec.venues {
        venues.push(evaluate_venue(venue_spec, base_dir)?);
    }

    let status = aggregate_status(venues.iter().map(|venue| venue.status));
    let universe_count = venues.iter().map(|venue| venue.universe_count).sum();
    let converted_universes = venues.iter().map(|venue| venue.converted_universes).sum();
    let source_only_universes = venues.iter().map(|venue| venue.source_only_universes).sum();
    let blocked_universes = venues.iter().map(|venue| venue.blocked_universes).sum();
    let total_converted_records = venues
        .iter()
        .map(|venue| venue.total_converted_records)
        .sum();
    let total_converted_canonical_rows = venues
        .iter()
        .map(|venue| venue.total_converted_canonical_rows)
        .sum();
    let total_converted_nt_catalog_rows = venues
        .iter()
        .map(|venue| venue.total_converted_nt_catalog_rows)
        .sum();
    let total_source_only_objects = venues
        .iter()
        .map(|venue| venue.total_source_only_objects)
        .sum();
    let total_source_only_object_gates = venues
        .iter()
        .map(|venue| venue.total_source_only_object_gates)
        .sum();
    let total_source_only_accepted_bytes = venues
        .iter()
        .map(|venue| venue.total_source_only_accepted_bytes)
        .sum();

    Ok(VenueScaleConversionAcceptanceLedger {
        schema_version: VENUE_SCALE_CONVERSION_ACCEPTANCE_SCHEMA_VERSION.to_string(),
        ledger_id: spec.ledger_id.clone(),
        status,
        venue_count: venues.len() as u64,
        universe_count,
        converted_universes,
        source_only_universes,
        blocked_universes,
        total_converted_records,
        total_converted_canonical_rows,
        total_converted_nt_catalog_rows,
        total_source_only_objects,
        total_source_only_object_gates,
        total_source_only_accepted_bytes,
        venues,
    })
}

fn evaluate_venue(
    spec: &VenueScaleConversionAcceptanceVenueSpec,
    base_dir: &Path,
) -> Result<VenueScaleConversionAcceptanceVenue> {
    ensure!(
        !spec.venue_id.trim().is_empty(),
        "venue_id must not be empty"
    );
    ensure!(!spec.venue.trim().is_empty(), "venue must not be empty");
    ensure!(
        !spec.universes.is_empty(),
        "venue {} must contain at least one universe",
        spec.venue_id
    );

    let mut universes = Vec::with_capacity(spec.universes.len());
    for universe_spec in &spec.universes {
        universes.push(evaluate_universe(universe_spec, base_dir)?);
    }

    let status = aggregate_status(universes.iter().map(|universe| universe.status));
    let converted_universes =
        count_status(&universes, VenueScaleConversionAcceptanceStatus::Converted);
    let source_only_universes =
        count_status(&universes, VenueScaleConversionAcceptanceStatus::SourceOnly);
    let blocked_universes = count_status(&universes, VenueScaleConversionAcceptanceStatus::Blocked);
    let total_converted_records = universes
        .iter()
        .map(|universe| universe.converted_record_count)
        .sum();
    let total_converted_canonical_rows = universes
        .iter()
        .map(|universe| universe.converted_canonical_rows)
        .sum();
    let total_converted_nt_catalog_rows = universes
        .iter()
        .map(|universe| universe.converted_nt_catalog_rows)
        .sum();
    let total_source_only_objects = universes
        .iter()
        .filter(|universe| universe.status == VenueScaleConversionAcceptanceStatus::SourceOnly)
        .map(|universe| universe.source_object_count)
        .sum();
    let total_source_only_object_gates = universes
        .iter()
        .filter(|universe| universe.status == VenueScaleConversionAcceptanceStatus::SourceOnly)
        .map(|universe| universe.source_object_gate_count)
        .sum();
    let total_source_only_accepted_bytes = universes
        .iter()
        .filter(|universe| universe.status == VenueScaleConversionAcceptanceStatus::SourceOnly)
        .map(|universe| universe.source_accepted_bytes)
        .sum();

    Ok(VenueScaleConversionAcceptanceVenue {
        venue_id: spec.venue_id.clone(),
        venue: spec.venue.clone(),
        status,
        universe_count: universes.len() as u64,
        converted_universes,
        source_only_universes,
        blocked_universes,
        total_converted_records,
        total_converted_canonical_rows,
        total_converted_nt_catalog_rows,
        total_source_only_objects,
        total_source_only_object_gates,
        total_source_only_accepted_bytes,
        universes,
    })
}

fn evaluate_universe(
    spec: &VenueScaleConversionAcceptanceUniverseSpec,
    base_dir: &Path,
) -> Result<VenueScaleConversionAcceptanceUniverse> {
    ensure!(
        !spec.universe_id.trim().is_empty(),
        "universe_id must not be empty"
    );
    ensure!(
        !spec.scope_label.trim().is_empty(),
        "scope_label must not be empty for universe {}",
        spec.universe_id
    );

    let mut artifact_refs = Vec::new();
    let mut completion_ledger_id = None;
    let mut source_archive_discovery_seed_id = None;
    let mut source_archive_discovery_seed_source_binding_count = 0;
    let mut source_archive_discovery_seed_representative_object_count = 0;
    let mut source_archive_index_manifest_id = None;
    let mut source_archive_index_snapshot_id = None;
    let mut source_archive_index_object_count = 0;
    let mut source_archive_index_verified_head_count = 0;
    let mut source_archive_index_total_content_length_bytes = 0;
    let mut source_manifest_id = None;
    let mut source_manifest_universe_id = None;
    let mut source_conversion_queue_id = None;
    let mut source_conversion_queue_work_item_count = 0;
    let mut source_conversion_queue_pending_items = 0;
    let mut source_conversion_queue_total_bytes = 0;
    let mut source_proof_set_id = None;
    let mut source_proof_count = 0;
    let mut source_accepted_proof_count = 0;
    let mut source_proof_completed_objects = 0;
    let mut source_proof_accepted_bytes = 0;
    let mut source_object_gate_id = None;
    let mut source_object_gate_queue_id = None;
    let mut source_conversion_run_plan_id = None;
    let mut source_conversion_run_count = 0;
    let mut source_conversion_run_object_count = 0;
    let mut source_conversion_run_planned_bytes = 0;
    let mut converted_record_count = 0;
    let mut converted_canonical_rows = 0;
    let mut converted_nt_catalog_rows = 0;
    let mut converted_completion_proof_seen = false;
    let mut source_object_count = 0;
    let mut source_object_gate_count = 0;
    let mut source_object_gate_source_binding_count = 0;
    let mut source_accepted_bytes = 0;
    let mut selected_source_rows = None;
    let mut selected_source_row_groups = None;
    let mut selected_projected_row_groups = None;
    let mut selected_rows = None;
    let mut selected_asset_count = None;
    let mut category_summaries = Vec::new();

    if let Some(path) = &spec.completion_ledger_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "completion_ledger",
            &path,
            spec.completion_ledger_artifact_path.as_deref(),
        )?);
        let ledger: BackfillConversionCompletionLedger = read_json(&path)?;
        validate_ready_completion_ledger(&ledger, &path)?;
        completion_ledger_id = Some(ledger.ledger_id);
        converted_completion_proof_seen = true;
        converted_record_count += ledger.record_count;
        converted_canonical_rows += ledger.total_canonical_rows;
        converted_nt_catalog_rows += ledger.total_nt_iterations;
    }

    if let Some(path) = &spec.source_archive_discovery_seed_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_archive_discovery_seed",
            &path,
            spec.source_archive_discovery_seed_artifact_path.as_deref(),
        )?);
        let seed: SourceArchiveDiscoverySeedSummary = read_json(&path)?;
        ensure!(
            seed.status == "ready",
            "source archive discovery seed {} is not ready",
            path.display()
        );
        source_archive_discovery_seed_id = Some(seed.discovery_id);
        source_archive_discovery_seed_source_binding_count = seed.source_binding_count;
        source_archive_discovery_seed_representative_object_count =
            seed.representative_object_count;
    }

    if let Some(path) = &spec.source_archive_index_manifest_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_archive_index_manifest",
            &path,
            spec.source_archive_index_manifest_artifact_path.as_deref(),
        )?);
        let manifest: SourceArchiveIndexManifestSummary = read_json(&path)?;
        ensure!(
            manifest.status == "ready",
            "source archive index manifest {} is not ready",
            path.display()
        );
        ensure!(
            manifest.object_count == manifest.verified_head_count,
            "source archive index manifest {} verified head count does not cover every object",
            path.display()
        );
        source_archive_index_manifest_id = Some(manifest.manifest_id);
        source_archive_index_snapshot_id = Some(manifest.snapshot_id);
        source_archive_index_object_count = manifest.object_count;
        source_archive_index_verified_head_count = manifest.verified_head_count;
        source_archive_index_total_content_length_bytes = manifest.total_content_length_bytes;
    }

    if let Some(path) = &spec.source_universe_manifest_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_universe_manifest",
            &path,
            spec.source_universe_manifest_artifact_path.as_deref(),
        )?);
        let manifest: SourceUniverseManifestSummary = read_json(&path)?;
        source_manifest_id = Some(manifest.manifest_id);
        source_manifest_universe_id = Some(manifest.universe_id);
        source_object_count += manifest.object_count;
        source_accepted_bytes += manifest.accepted_bytes;
        category_summaries.extend(manifest.category_summaries.into_iter().map(|summary| {
            VenueScaleConversionAcceptanceCategorySummary {
                category: summary.category,
                source_binding: summary.source_binding,
                instrument_count: summary.instrument_count,
                object_count: summary.object_count,
                compressed_bytes: summary.compressed_bytes,
            }
        }));
    }

    if let Some(path) = &spec.source_universe_conversion_queue_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_universe_conversion_queue",
            &path,
            spec.source_universe_conversion_queue_artifact_path
                .as_deref(),
        )?);
        let queue: SourceUniverseConversionQueueSummary = read_json(&path)?;
        ensure!(
            queue.status == "ready",
            "source-universe conversion queue {} is not ready",
            path.display()
        );
        ensure!(
            queue.work_item_count == queue.pending_conversion_items,
            "source-universe conversion queue {} pending count does not cover every work item",
            path.display()
        );
        if let Some(manifest_id) = &source_manifest_id {
            ensure!(
                manifest_id == &queue.manifest_id,
                "source-universe conversion queue {} manifest_id does not match source manifest",
                path.display()
            );
        } else {
            source_manifest_id = Some(queue.manifest_id.clone());
        }
        if let Some(universe_id) = &source_manifest_universe_id {
            ensure!(
                universe_id == &queue.universe_id,
                "source-universe conversion queue {} universe_id does not match source manifest",
                path.display()
            );
        } else {
            source_manifest_universe_id = Some(queue.universe_id.clone());
        }
        if source_object_count == 0 {
            source_object_count = queue.work_item_count;
        } else {
            ensure!(
                source_object_count == queue.work_item_count,
                "source-universe conversion queue {} work item count does not match source manifest",
                path.display()
            );
        }
        if source_accepted_bytes == 0 {
            source_accepted_bytes = queue.total_source_bytes;
        } else {
            ensure!(
                source_accepted_bytes == queue.total_source_bytes,
                "source-universe conversion queue {} total source bytes do not match source manifest",
                path.display()
            );
        }
        source_conversion_queue_id = Some(queue.queue_id);
        source_conversion_queue_work_item_count = queue.work_item_count;
        source_conversion_queue_pending_items = queue.pending_conversion_items;
        source_conversion_queue_total_bytes = queue.total_source_bytes;
    }

    if let Some(path) = &spec.source_universe_source_proof_set_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_universe_source_proof_set",
            &path,
            spec.source_universe_source_proof_set_artifact_path
                .as_deref(),
        )?);
        let proof_set: SourceUniverseSourceProofSetSummary = read_json(&path)?;
        ensure!(
            proof_set.accepted_proof_count <= proof_set.proof_count,
            "source-universe source proof set {} accepted proof count exceeds proof count",
            path.display()
        );
        if source_object_count > 0 {
            ensure!(
                source_object_count == proof_set.total_completed_objects,
                "source-universe source proof set {} completed object count does not match source manifest",
                path.display()
            );
        } else {
            source_object_count = proof_set.total_completed_objects;
        }
        if source_accepted_bytes > 0 {
            ensure!(
                source_accepted_bytes == proof_set.total_accepted_bytes,
                "source-universe source proof set {} accepted bytes do not match source manifest",
                path.display()
            );
        } else {
            source_accepted_bytes = proof_set.total_accepted_bytes;
        }
        source_proof_set_id = Some(proof_set.proof_set_id);
        source_proof_count = proof_set.proof_count;
        source_accepted_proof_count = proof_set.accepted_proof_count;
        source_proof_completed_objects = proof_set.total_completed_objects;
        source_proof_accepted_bytes = proof_set.total_accepted_bytes;
    }

    if let Some(path) = &spec.source_universe_object_gates_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_universe_object_gates",
            &path,
            spec.source_universe_object_gates_artifact_path.as_deref(),
        )?);
        let gates: SourceUniverseObjectGateSummary = read_json(&path)?;
        ensure!(
            gates.status == "ready",
            "source-universe object gates {} are not ready",
            path.display()
        );
        ensure!(
            gates.work_item_count == gates.accepted_gate_count,
            "source-universe object gates {} accepted count does not cover every work item",
            path.display()
        );

        if let Some(manifest_id) = &source_manifest_id {
            ensure!(
                manifest_id == &gates.manifest_id,
                "source-universe object gates {} manifest_id does not match source manifest",
                path.display()
            );
        } else {
            source_manifest_id = Some(gates.manifest_id.clone());
        }
        if let Some(universe_id) = &source_manifest_universe_id {
            ensure!(
                universe_id == &gates.universe_id,
                "source-universe object gates {} universe_id does not match source manifest",
                path.display()
            );
        }
        if source_object_count == 0 {
            source_object_count = gates.accepted_gate_count;
        } else {
            ensure!(
                source_object_count == gates.accepted_gate_count,
                "source-universe object gates {} object count does not match source manifest",
                path.display()
            );
        }
        if source_accepted_bytes == 0 {
            source_accepted_bytes = gates.total_accepted_bytes;
        } else {
            ensure!(
                source_accepted_bytes == gates.total_accepted_bytes,
                "source-universe object gates {} accepted bytes do not match source manifest",
                path.display()
            );
        }

        source_object_gate_id = Some(gates.gate_id);
        source_object_gate_queue_id = Some(gates.queue_id);
        source_object_gate_count = gates.accepted_gate_count;
        source_object_gate_source_binding_count = gates.source_binding_count;
    }

    if let Some(path) = &spec.source_universe_conversion_run_plan_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "source_universe_conversion_run_plan",
            &path,
            spec.source_universe_conversion_run_plan_artifact_path
                .as_deref(),
        )?);
        let run_plan: SourceUniverseConversionRunPlanSummary = read_json(&path)?;
        ensure!(
            run_plan.status == "ready",
            "source-universe conversion run plan {} is not ready",
            path.display()
        );
        if let Some(gate_id) = &source_object_gate_id {
            ensure!(
                gate_id == &run_plan.gate_id,
                "source-universe conversion run plan {} gate_id does not match object gates",
                path.display()
            );
        }
        if source_object_gate_count > 0 {
            ensure!(
                source_object_gate_count == run_plan.planned_object_count,
                "source-universe conversion run plan {} object count does not match object gates",
                path.display()
            );
        }
        if source_accepted_bytes > 0 {
            ensure!(
                source_accepted_bytes == run_plan.planned_source_bytes,
                "source-universe conversion run plan {} planned bytes do not match source evidence",
                path.display()
            );
        }
        if source_object_gate_source_binding_count > 0 {
            ensure!(
                source_object_gate_source_binding_count == run_plan.source_binding_count,
                "source-universe conversion run plan {} source binding count does not match object gates",
                path.display()
            );
        }
        source_conversion_run_plan_id = Some(run_plan.plan_id);
        source_conversion_run_count = run_plan.run_count;
        source_conversion_run_object_count = run_plan.planned_object_count;
        source_conversion_run_planned_bytes = run_plan.planned_source_bytes;
    }

    if let Some(path) = &spec.selected_source_report_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref(
            "selected_source_report",
            &path,
            spec.selected_source_report_artifact_path.as_deref(),
        )?);
        let report: SelectedSourceReportSummary = read_json(&path)?;
        selected_source_rows = Some(report.source_rows);
        selected_source_row_groups = Some(report.source_row_groups);
        selected_projected_row_groups = Some(report.projected_row_groups);
        selected_rows = Some(report.selected_rows);
        selected_asset_count = Some(report.selected_asset_count);
    }

    validate_status_inputs(
        spec,
        converted_record_count,
        source_object_count,
        source_conversion_run_object_count,
        converted_completion_proof_seen,
        source_proof_count,
        source_accepted_proof_count,
    )?;

    Ok(VenueScaleConversionAcceptanceUniverse {
        universe_id: spec.universe_id.clone(),
        scope_label: spec.scope_label.clone(),
        status: spec.status,
        completion_ledger_id,
        source_archive_discovery_seed_id,
        source_archive_discovery_seed_source_binding_count,
        source_archive_discovery_seed_representative_object_count,
        source_archive_index_manifest_id,
        source_archive_index_snapshot_id,
        source_archive_index_object_count,
        source_archive_index_verified_head_count,
        source_archive_index_total_content_length_bytes,
        source_manifest_id,
        source_conversion_queue_id,
        source_conversion_queue_work_item_count,
        source_conversion_queue_pending_items,
        source_conversion_queue_total_bytes,
        source_proof_set_id,
        source_proof_count,
        source_accepted_proof_count,
        source_proof_completed_objects,
        source_proof_accepted_bytes,
        source_object_gate_id,
        source_object_gate_queue_id,
        source_conversion_run_plan_id,
        source_conversion_run_count,
        source_conversion_run_object_count,
        source_conversion_run_planned_bytes,
        converted_record_count,
        converted_canonical_rows,
        converted_nt_catalog_rows,
        source_object_count,
        source_object_gate_count,
        source_object_gate_source_binding_count,
        source_accepted_bytes,
        selected_source_rows,
        selected_source_row_groups,
        selected_projected_row_groups,
        selected_rows,
        selected_asset_count,
        category_summaries,
        artifact_refs,
        blocking_issues: spec.blocking_issues.clone(),
    })
}

fn validate_ready_completion_ledger(
    ledger: &BackfillConversionCompletionLedger,
    path: &Path,
) -> Result<()> {
    ensure!(
        ledger.schema_version == BACKFILL_CONVERSION_COMPLETION_SCHEMA_VERSION,
        "completion ledger {} uses unsupported schema {:?}; expected {:?}",
        path.display(),
        ledger.schema_version,
        BACKFILL_CONVERSION_COMPLETION_SCHEMA_VERSION
    );
    ensure!(
        !ledger.ledger_id.trim().is_empty(),
        "ready completion ledger {} contains an empty ledger_id",
        path.display()
    );
    ensure!(
        !ledger.batch_id.trim().is_empty(),
        "ready completion ledger {} contains an empty batch_id",
        path.display()
    );
    ensure!(
        ledger.status == BackfillConversionCompletionStatus::Ready,
        "completion ledger {} is not ready",
        path.display()
    );
    ensure!(
        ledger.blocking_issues.is_empty(),
        "ready completion ledger {} contains blocking issues",
        path.display()
    );
    for (field, value) in [
        ("scope_status", ledger.requirements.scope_status.as_str()),
        (
            "current_bte_status",
            ledger.requirements.current_bte_status.as_str(),
        ),
        (
            "parquet_catalog_status",
            ledger.requirements.parquet_catalog_status.as_str(),
        ),
        ("nt_data_type", ledger.requirements.nt_data_type.as_str()),
        (
            "fidelity_class",
            ledger.requirements.fidelity_class.as_str(),
        ),
    ] {
        ensure!(
            !value.trim().is_empty(),
            "ready completion ledger {} contains empty requirement {field}",
            path.display()
        );
    }

    let actual_record_count = u64::try_from(ledger.records.len())
        .context("completion ledger record count does not fit u64")?;
    ensure!(
        actual_record_count > 0,
        "ready completion ledger {} must contain at least one record",
        path.display()
    );
    ensure!(
        ledger.record_count == actual_record_count,
        "completion ledger {} record_count mismatch: declared {}, actual {}",
        path.display(),
        ledger.record_count,
        actual_record_count
    );

    let actual_published_records = u64::try_from(
        ledger
            .records
            .iter()
            .filter(|record| record.published_catalog_direct_s3)
            .count(),
    )
    .context("completion ledger published-record count does not fit u64")?;
    ensure!(
        ledger.published_records == actual_published_records,
        "completion ledger {} published_records mismatch: declared {}, actual {}",
        path.display(),
        ledger.published_records,
        actual_published_records
    );
    if ledger.requirements.require_direct_s3_catalog_access {
        ensure!(
            actual_published_records == actual_record_count,
            "ready completion ledger {} does not prove direct S3 publication for every record: published {}, records {}",
            path.display(),
            actual_published_records,
            actual_record_count
        );
    }

    let actual_mapping_proven_records = u64::try_from(
        ledger
            .records
            .iter()
            .filter(|record| {
                record.mapping_current_bte_status == ledger.requirements.current_bte_status
                    && record.mapping_parquet_catalog_status
                        == ledger.requirements.parquet_catalog_status
            })
            .count(),
    )
    .context("completion ledger mapping-proven record count does not fit u64")?;
    ensure!(
        ledger.mapping_proven_records == actual_mapping_proven_records,
        "completion ledger {} mapping_proven_records mismatch: declared {}, actual {}",
        path.display(),
        ledger.mapping_proven_records,
        actual_mapping_proven_records
    );
    ensure!(
        actual_mapping_proven_records == actual_record_count,
        "ready completion ledger {} does not prove catalog mapping for every record: mapped {}, records {}",
        path.display(),
        actual_mapping_proven_records,
        actual_record_count
    );

    let mut actual_accepted_bytes = 0_u64;
    let mut actual_canonical_rows = 0_u64;
    let mut actual_nt_iterations = 0_u64;
    let mut record_ids = BTreeSet::new();
    for record in &ledger.records {
        ensure!(
            !record.record_id.trim().is_empty(),
            "ready completion ledger {} contains an empty record_id",
            path.display()
        );
        ensure!(
            record_ids.insert(record.record_id.as_str()),
            "ready completion ledger {} contains duplicate record_id {:?}",
            path.display(),
            record.record_id
        );
        for (field, value) in [
            ("archive_date", record.archive_date.as_str()),
            ("source_binding", record.source_binding.as_str()),
            ("table_family", record.table_family.as_str()),
            ("source_proof_id", record.source_proof_id.as_str()),
            ("operator_run_id", record.operator_run_id.as_str()),
            ("output_prefix", record.output_prefix.as_str()),
            ("published_catalog_uri", record.published_catalog_uri.as_str()),
        ] {
            ensure!(
                !value.trim().is_empty(),
                "ready completion ledger {} contains empty record {field}",
                path.display()
            );
        }
        ensure!(
            record.source_proof_version > 0,
            "ready completion ledger {} contains zero record source_proof_version",
            path.display()
        );
        for (field, value) in [
            (
                "accepted_object_sha256",
                record.accepted_object_sha256.as_str(),
            ),
            (
                "publication_evidence_hash",
                record.publication_evidence_hash.as_str(),
            ),
            (
                "catalog_mapping_evaluation_hash",
                record.catalog_mapping_evaluation_hash.as_str(),
            ),
            ("catalog_hash", record.catalog_hash.as_str()),
        ] {
            ensure!(
                is_lowercase_sha256_hex(value),
                "ready completion ledger {} contains invalid record {field}",
                path.display()
            );
        }
        for (field, value) in [
            (
                "publication_evidence_path",
                record.publication_evidence_path.as_path(),
            ),
            (
                "catalog_mapping_evaluation_path",
                record.catalog_mapping_evaluation_path.as_path(),
            ),
        ] {
            ensure!(
                !value.as_os_str().is_empty(),
                "ready completion ledger {} contains empty record {field}",
                path.display()
            );
        }
        ensure!(
            record.accepted_bytes > 0,
            "ready completion ledger {} contains zero record accepted_bytes",
            path.display()
        );
        ensure!(
            record.canonical_rows > 0,
            "ready completion ledger {} contains zero record canonical_rows",
            path.display()
        );
        ensure!(
            record.nt_data_type == ledger.requirements.nt_data_type,
            "completion ledger {} record {:?} nt_data_type does not match requirements",
            path.display(),
            record.record_id
        );
        ensure!(
            record.fidelity_class == ledger.requirements.fidelity_class,
            "completion ledger {} record {:?} fidelity_class does not match requirements",
            path.display(),
            record.record_id
        );
        ensure!(
            record.canonical_rows == record.catalog_read_back_trade_ticks
                && record.canonical_rows == record.published_catalog_expected_iterations
                && record.canonical_rows == record.published_catalog_nt_iterations,
            "completion ledger {} record {:?} row-count lineage is inconsistent",
            path.display(),
            record.record_id
        );
        actual_accepted_bytes = actual_accepted_bytes
            .checked_add(record.accepted_bytes)
            .context("completion ledger accepted-byte total overflow")?;
        actual_canonical_rows = actual_canonical_rows
            .checked_add(record.canonical_rows)
            .context("completion ledger canonical-row total overflow")?;
        actual_nt_iterations = actual_nt_iterations
            .checked_add(record.published_catalog_nt_iterations)
            .context("completion ledger NT-iteration total overflow")?;
    }
    ensure!(
        ledger.total_accepted_bytes == actual_accepted_bytes,
        "completion ledger {} total_accepted_bytes mismatch: declared {}, actual {}",
        path.display(),
        ledger.total_accepted_bytes,
        actual_accepted_bytes
    );
    ensure!(
        ledger.total_canonical_rows == actual_canonical_rows,
        "completion ledger {} total_canonical_rows mismatch: declared {}, actual {}",
        path.display(),
        ledger.total_canonical_rows,
        actual_canonical_rows
    );
    ensure!(
        ledger.total_nt_iterations == actual_nt_iterations,
        "completion ledger {} total_nt_iterations mismatch: declared {}, actual {}",
        path.display(),
        ledger.total_nt_iterations,
        actual_nt_iterations
    );
    Ok(())
}

fn validate_status_inputs(
    spec: &VenueScaleConversionAcceptanceUniverseSpec,
    converted_record_count: u64,
    source_object_count: u64,
    planned_conversion_run_object_count: u64,
    converted_completion_proof_seen: bool,
    source_proof_count: u64,
    source_accepted_proof_count: u64,
) -> Result<()> {
    match spec.status {
        VenueScaleConversionAcceptanceStatus::Converted => {
            ensure!(
                converted_record_count > 0,
                "converted universe {} must reference converted artifact evidence",
                spec.universe_id
            );
            ensure!(
                spec.blocking_issues.is_empty(),
                "converted universe {} must not contain blocking issues",
                spec.universe_id
            );
            // A Converted status must rest on the completion-ledger authority,
            // never on a raw converted_record_count > 0. A completion ledger
            // reaches `status: ready` only after its own internal coverage
            // computation. A conversion manifest is an output record, not a
            // second venue-scale completion authority. Without a ready ledger
            // there is no coverage evidence at all (the run-plan equality check
            // below is skipped whenever no run plan is referenced, planned == 0).
            ensure!(
                converted_completion_proof_seen,
                "converted universe {} must reference a ready completion ledger as coverage proof",
                spec.universe_id
            );
            // When a conversion run plan declares planned objects, every one must
            // have a converted record. This is the same completeness discipline
            // the sibling source-universe execution acceptance evaluator applies.
            ensure!(
                planned_conversion_run_object_count == 0
                    || converted_record_count == planned_conversion_run_object_count,
                "converted universe {} record_count_mismatch: converted {} records but conversion \
                 run plan planned {} objects",
                spec.universe_id,
                converted_record_count,
                planned_conversion_run_object_count
            );
        }
        VenueScaleConversionAcceptanceStatus::SourceOnly => {
            ensure!(
                source_object_count > 0,
                "source-only universe {} must reference source manifest evidence",
                spec.universe_id
            );
            ensure!(
                converted_record_count == 0,
                "source-only universe {} must not reference converted artifact evidence",
                spec.universe_id
            );
            ensure!(
                spec.blocking_issues.is_empty(),
                "source-only universe {} must not contain blocking issues",
                spec.universe_id
            );
            ensure!(
                source_proof_count == 0 || source_accepted_proof_count > 0,
                "source-only universe {} with a source proof set must contain accepted source \
                 proof evidence",
                spec.universe_id
            );
        }
        VenueScaleConversionAcceptanceStatus::Blocked => {
            ensure!(
                !spec.blocking_issues.is_empty(),
                "blocked universe {} must list blocking issues",
                spec.universe_id
            );
        }
        VenueScaleConversionAcceptanceStatus::PartiallyConverted => {
            bail!(
                "partially_converted is an aggregate status and is not valid for universe {}",
                spec.universe_id
            );
        }
    }
    Ok(())
}

fn aggregate_status<I>(statuses: I) -> VenueScaleConversionAcceptanceStatus
where
    I: IntoIterator<Item = VenueScaleConversionAcceptanceStatus>,
{
    let mut total = 0;
    let mut converted = 0;
    let mut source_only = 0;
    let mut blocked = 0;

    for status in statuses {
        total += 1;
        match status {
            VenueScaleConversionAcceptanceStatus::Converted => converted += 1,
            VenueScaleConversionAcceptanceStatus::PartiallyConverted => {
                converted += 1;
                source_only += 1;
            }
            VenueScaleConversionAcceptanceStatus::SourceOnly => source_only += 1,
            VenueScaleConversionAcceptanceStatus::Blocked => blocked += 1,
        }
    }

    if blocked > 0 {
        VenueScaleConversionAcceptanceStatus::Blocked
    } else if converted == total && total > 0 {
        VenueScaleConversionAcceptanceStatus::Converted
    } else if source_only == total && total > 0 {
        VenueScaleConversionAcceptanceStatus::SourceOnly
    } else {
        VenueScaleConversionAcceptanceStatus::PartiallyConverted
    }
}

fn count_status(
    universes: &[VenueScaleConversionAcceptanceUniverse],
    status: VenueScaleConversionAcceptanceStatus,
) -> u64 {
    universes
        .iter()
        .filter(|universe| universe.status == status)
        .count() as u64
}

fn artifact_ref(
    role: &str,
    materialized_path: &Path,
    artifact_identity_path: Option<&Path>,
) -> Result<ReferenceArtifactPin> {
    let path = if artifact_identity_path.is_some() {
        stable_artifact_identity_path_for_spec(
            materialized_path,
            materialized_path,
            artifact_identity_path,
        )?
    } else {
        portable_artifact_path(materialized_path)?
    };
    Ok(ReferenceArtifactPin {
        role: role.to_string(),
        path,
        sha256: sha256_file(materialized_path)?,
    })
}
fn read_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).with_context(|| format!("read JSON artifact {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse JSON artifact {}", path.display()))
}
fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read artifact {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converted_universe_spec() -> VenueScaleConversionAcceptanceUniverseSpec {
        VenueScaleConversionAcceptanceUniverseSpec {
            universe_id: "test-universe".to_string(),
            scope_label: "test-scope".to_string(),
            status: VenueScaleConversionAcceptanceStatus::Converted,
            completion_ledger_path: None,
            completion_ledger_artifact_path: None,
            source_archive_discovery_seed_path: None,
            source_archive_discovery_seed_artifact_path: None,
            source_archive_index_manifest_path: None,
            source_archive_index_manifest_artifact_path: None,
            source_universe_manifest_path: None,
            source_universe_manifest_artifact_path: None,
            source_universe_conversion_queue_path: None,
            source_universe_conversion_queue_artifact_path: None,
            source_universe_source_proof_set_path: None,
            source_universe_source_proof_set_artifact_path: None,
            source_universe_object_gates_path: None,
            source_universe_object_gates_artifact_path: None,
            source_universe_conversion_run_plan_path: None,
            source_universe_conversion_run_plan_artifact_path: None,
            selected_source_report_path: None,
            selected_source_report_artifact_path: None,
            blocking_issues: Vec::new(),
        }
    }

    fn source_only_universe_spec() -> VenueScaleConversionAcceptanceUniverseSpec {
        let mut spec = converted_universe_spec();
        spec.status = VenueScaleConversionAcceptanceStatus::SourceOnly;
        spec
    }

    #[test]
    fn converted_status_requires_full_planned_object_coverage() {
        let spec = converted_universe_spec();
        // converted_record_count (3) < planned conversion-run object count (5):
        // the operator-asserted Converted status must be rejected, not copied.
        let error = validate_status_inputs(&spec, 3, 0, 5, true, 0, 0)
            .expect_err("converted universe short of planned objects must be blocked");
        assert!(
            format!("{error:#}").contains("record_count_mismatch"),
            "expected record_count_mismatch blocker, got: {error:#}"
        );
    }

    #[test]
    fn converted_status_accepts_full_planned_object_coverage() {
        let spec = converted_universe_spec();
        validate_status_inputs(&spec, 5, 0, 5, true, 0, 0)
            .expect("converted universe covering every planned object is accepted");
    }

    #[test]
    fn source_only_status_rejects_blocking_issues() {
        let mut spec = source_only_universe_spec();
        spec.blocking_issues = vec!["missing_object_gates".to_string()];
        let error = validate_status_inputs(&spec, 0, 1, 0, false, 0, 0)
            .expect_err("source-only universe with blocking issues must be rejected");
        assert!(
            format!("{error:#}").contains("blocking issues"),
            "expected blocking-issues rejection, got: {error:#}"
        );
    }

    #[test]
    fn source_only_status_rejects_unaccepted_source_proof_set() {
        let spec = source_only_universe_spec();
        let error = validate_status_inputs(&spec, 0, 1, 0, false, 1, 0)
            .expect_err("source-only universe with unaccepted source proof set must be rejected");
        assert!(
            format!("{error:#}").contains("accepted source proof"),
            "expected accepted-source-proof rejection, got: {error:#}"
        );
    }

    #[test]
    fn converted_status_without_completion_proof_is_rejected_when_planned_zero() {
        let spec = converted_universe_spec();
        // planned == 0 (no run plan) and no completion-proof artifact: the prior
        // bypass accepted this on converted_record_count > 0 alone. It must now
        // be rejected for missing coverage proof.
        let error = validate_status_inputs(&spec, 7, 0, 0, false, 0, 0)
            .expect_err("converted universe without a completion proof must be blocked");
        assert!(
            format!("{error:#}").contains("coverage proof"),
            "expected coverage-proof blocker, got: {error:#}"
        );
    }

    #[test]
    fn converted_status_with_completion_proof_and_no_run_plan_is_accepted() {
        let spec = converted_universe_spec();
        // Control: a ready completion-ledger proof with no run plan
        // (planned == 0) is the legitimate Converted case.
        validate_status_inputs(&spec, 7, 0, 0, true, 0, 0)
            .expect("converted universe with a completion proof and no run plan is accepted");
    }
}
