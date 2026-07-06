//! Venue/source-universe conversion acceptance ledger.
//!
//! This ledger is the roll-up above per-day conversion completion ledgers. It
//! records what is converted, what is only source-accepted, and what is blocked.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::hashing::sha256_hex;
use crate::path_resolution::{portable_artifact_path, resolve_existing_path, resolve_output_dir};
use crate::reference_artifact::ReferenceArtifactPin;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const VENUE_SCALE_CONVERSION_ACCEPTANCE_SCHEMA_VERSION: &str =
    "venue-scale-conversion-acceptance-ledger.v1";
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
    pub source_archive_discovery_seed_path: Option<PathBuf>,
    pub source_archive_index_manifest_path: Option<PathBuf>,
    pub source_universe_manifest_path: Option<PathBuf>,
    pub source_universe_conversion_queue_path: Option<PathBuf>,
    pub source_universe_source_proof_set_path: Option<PathBuf>,
    pub source_universe_object_gates_path: Option<PathBuf>,
    pub source_universe_conversion_run_plan_path: Option<PathBuf>,
    pub selected_conversion_manifest_path: Option<PathBuf>,
    pub selected_source_report_path: Option<PathBuf>,
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
    pub catalog_rows_by_nt_data_type: BTreeMap<String, u64>,
    pub catalog_hash: Option<String>,
    pub output_catalog_uri: Option<String>,
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
struct CompletionLedgerSummary {
    ledger_id: String,
    status: String,
    record_count: u64,
    total_canonical_rows: u64,
    total_nt_iterations: u64,
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
struct SelectedConversionManifestSummary {
    canonical_rows: u64,
    #[serde(default)]
    catalog_rows_by_nt_data_type: BTreeMap<String, u64>,
    // `catalog_hash` and `completed_at` are required, not optional: a manifest is
    // a completion proof only when it attests a finalized conversion that actually
    // wrote a catalog. A stub manifest that omits them must fail to parse here
    // rather than be accepted as coverage evidence.
    catalog_hash: String,
    output_catalog_uri: Option<String>,
    completed_at: String,
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
    let mut catalog_rows_by_nt_data_type = BTreeMap::new();
    let mut catalog_hash = None;
    let mut output_catalog_uri = None;
    let mut selected_source_rows = None;
    let mut selected_source_row_groups = None;
    let mut selected_projected_row_groups = None;
    let mut selected_rows = None;
    let mut selected_asset_count = None;
    let mut category_summaries = Vec::new();

    if let Some(path) = &spec.completion_ledger_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref("completion_ledger", &path)?);
        let ledger: CompletionLedgerSummary = read_json(&path)?;
        ensure!(
            ledger.status == "ready",
            "completion ledger {} is not ready",
            path.display()
        );
        completion_ledger_id = Some(ledger.ledger_id);
        converted_completion_proof_seen = true;
        converted_record_count += ledger.record_count;
        converted_canonical_rows += ledger.total_canonical_rows;
        converted_nt_catalog_rows += ledger.total_nt_iterations;
    }

    if let Some(path) = &spec.source_archive_discovery_seed_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref("source_archive_discovery_seed", &path)?);
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
        artifact_refs.push(artifact_ref("source_archive_index_manifest", &path)?);
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
        artifact_refs.push(artifact_ref("source_universe_manifest", &path)?);
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
        artifact_refs.push(artifact_ref("source_universe_conversion_queue", &path)?);
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
        artifact_refs.push(artifact_ref("source_universe_source_proof_set", &path)?);
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
        artifact_refs.push(artifact_ref("source_universe_object_gates", &path)?);
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
        artifact_refs.push(artifact_ref("source_universe_conversion_run_plan", &path)?);
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

    if let Some(path) = &spec.selected_conversion_manifest_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref("selected_conversion_manifest", &path)?);
        let conversion: SelectedConversionManifestSummary = read_json(&path)?;
        // A selected conversion manifest is accepted as a completion proof only
        // when it attests a *finalized* conversion that actually produced catalog
        // output, mirroring the completion-ledger `status == "ready"` gate above.
        // Parseability alone is not proof: a stub manifest with zero canonical
        // rows, no catalog hash, or no completion timestamp must be rejected so it
        // cannot satisfy the Converted coverage requirement through the
        // planned == 0 path.
        ensure!(
            conversion.canonical_rows > 0,
            "selected conversion manifest {} reports zero canonical rows and is not a finalized conversion",
            path.display()
        );
        ensure!(
            !conversion.catalog_hash.trim().is_empty(),
            "selected conversion manifest {} is missing a catalog hash and is not a finalized conversion",
            path.display()
        );
        ensure!(
            !conversion.completed_at.trim().is_empty(),
            "selected conversion manifest {} is missing a completion timestamp and is not a finalized conversion",
            path.display()
        );
        converted_completion_proof_seen = true;
        converted_record_count += 1;
        converted_canonical_rows += conversion.canonical_rows;
        converted_nt_catalog_rows += conversion
            .catalog_rows_by_nt_data_type
            .values()
            .copied()
            .sum::<u64>();
        for (data_type, rows) in conversion.catalog_rows_by_nt_data_type {
            *catalog_rows_by_nt_data_type.entry(data_type).or_insert(0) += rows;
        }
        catalog_hash = Some(conversion.catalog_hash);
        output_catalog_uri = conversion.output_catalog_uri;
    }

    if let Some(path) = &spec.selected_source_report_path {
        let path = resolve_existing_path(base_dir, path);
        artifact_refs.push(artifact_ref("selected_source_report", &path)?);
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
        catalog_rows_by_nt_data_type,
        catalog_hash,
        output_catalog_uri,
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
            // A Converted status must rest on a self-attesting completion-proof
            // artifact, never on a raw converted_record_count > 0: a completion
            // ledger reaches `status: ready` only after its own internal coverage
            // computation, and a selected conversion manifest is a finalized
            // conversion output. Without such a proof there is no coverage
            // evidence at all (the run-plan equality check below is skipped
            // whenever no run plan is referenced, planned == 0).
            ensure!(
                converted_completion_proof_seen,
                "converted universe {} must reference a completion ledger or selected conversion \
                 manifest as coverage proof",
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

fn artifact_ref(role: &str, path: &Path) -> Result<ReferenceArtifactPin> {
    Ok(ReferenceArtifactPin {
        role: role.to_string(),
        path: portable_artifact_path(path),
        sha256: sha256_file(path)?,
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
            source_archive_discovery_seed_path: None,
            source_archive_index_manifest_path: None,
            source_universe_manifest_path: None,
            source_universe_conversion_queue_path: None,
            source_universe_source_proof_set_path: None,
            source_universe_object_gates_path: None,
            source_universe_conversion_run_plan_path: None,
            selected_conversion_manifest_path: None,
            selected_source_report_path: None,
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
        // Control: a self-attesting completion proof with no run plan
        // (planned == 0) is the legitimate Converted-via-completion-ledger or
        // selected-manifest case.
        validate_status_inputs(&spec, 7, 0, 0, true, 0, 0)
            .expect("converted universe with a completion proof and no run plan is accepted");
    }
}
