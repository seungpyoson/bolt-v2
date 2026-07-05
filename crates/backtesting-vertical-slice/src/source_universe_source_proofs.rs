//! Source-proof materialization for source-universe category manifests.
//!
//! The materializer turns a config-owned proof policy plus category manifests
//! into [`SourceProofReport`] artifacts. Accepted reports can feed object gates;
//! pending reports preserve the exact remaining proof blockers without
//! overclaiming gate readiness.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::path_resolution::{
    portable_artifact_path_for_spec, resolve_existing_path, resolve_output_dir,
};
use crate::source_proof::{
    AcceptanceMode, AcceptanceScope, CONTRACT_VERSION, CheckOutcome, L2ReplayEvidence,
    LicenseScope, NtMappingStatus, RequiredCheck, RequiredChecks, SOURCE_PROOF_SCHEMA_VERSION,
    SourceCandidateClass, SourceProofClaimLimit, SourceProofFidelityClass, SourceProofReport,
    SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus, TimeRange,
};

pub const SOURCE_UNIVERSE_SOURCE_PROOF_SET_SCHEMA_VERSION: &str =
    "source-universe-source-proof-set.v1";
pub const SOURCE_UNIVERSE_SOURCE_PROOF_SET_FILE: &str = "source-universe-source-proof-set.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofSetSpec {
    pub proof_set_id: String,
    pub output_dir: PathBuf,
    pub source_bindings_path: PathBuf,
    pub venue: String,
    pub table_family: String,
    pub manifest_table_family: String,
    #[serde(default = "default_source_proof_status")]
    pub status: SourceProofStatus,
    pub source_candidate_class: SourceCandidateClass,
    pub source_selection_status: SourceSelectionStatus,
    pub usage_scope: SourceProofUsageScope,
    pub fidelity_class: SourceProofFidelityClass,
    #[serde(default)]
    pub acceptance_mode: Option<AcceptanceMode>,
    #[serde(default)]
    pub accepted_by: Option<String>,
    #[serde(default)]
    pub accepted_at_utc: Option<String>,
    pub requested_start_utc: String,
    pub requested_end_utc: String,
    pub coverage_start_utc: String,
    pub coverage_end_utc: String,
    pub license_ref: String,
    pub license_scope: LicenseScope,
    pub retention_ref: String,
    pub cost_ref: String,
    pub gap_policy_id: String,
    pub raw_sample_selection: String,
    pub schema_sample_policy: String,
    #[serde(default)]
    pub l2_replay_evidence: SourceUniverseSourceProofL2ReplayEvidenceTemplate,
    pub required_checks: SourceUniverseSourceProofRequiredCheckTemplates,
    #[serde(rename = "claim_limit", default)]
    pub claim_limits: Vec<SourceUniverseSourceProofClaimLimitTemplate>,
    #[serde(rename = "source_binding", default)]
    pub source_bindings: Vec<SourceUniverseSourceProofBindingSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofRequiredCheckTemplates {
    pub source_access: SourceUniverseSourceProofRequiredCheckTemplate,
    pub license: SourceUniverseSourceProofRequiredCheckTemplate,
    pub schema: SourceUniverseSourceProofRequiredCheckTemplate,
    pub time_semantics: SourceUniverseSourceProofRequiredCheckTemplate,
    pub instrument_universe: SourceUniverseSourceProofRequiredCheckTemplate,
    pub coverage: SourceUniverseSourceProofRequiredCheckTemplate,
    pub retention_freshness: SourceUniverseSourceProofRequiredCheckTemplate,
    pub granularity: SourceUniverseSourceProofRequiredCheckTemplate,
    pub completeness: SourceUniverseSourceProofRequiredCheckTemplate,
    pub nt_mapping: SourceUniverseSourceProofRequiredCheckTemplate,
    pub cost: SourceUniverseSourceProofRequiredCheckTemplate,
    pub storage: SourceUniverseSourceProofRequiredCheckTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SourceUniverseSourceProofRequiredCheckTemplate {
    PassedEvidenceRef(String),
    Structured(SourceUniverseSourceProofRequiredCheckStructuredTemplate),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofRequiredCheckStructuredTemplate {
    pub outcome: CheckOutcome,
    pub evidence_ref: String,
    #[serde(default)]
    pub expires_at_utc: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofClaimLimitTemplate {
    pub id: String,
    pub severity: String,
    pub claim: String,
    pub reason: String,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofL2ReplayEvidenceTemplate {
    #[serde(default)]
    pub order_book_delta_ref: Option<String>,
    #[serde(default)]
    pub sufficient_snapshot_cadence_ref: Option<String>,
    #[serde(default)]
    pub no_tick_size_change_universe_ref: Option<String>,
    #[serde(default)]
    pub timed_instrument_epoch_replay_ref: Option<String>,
}

impl SourceUniverseSourceProofL2ReplayEvidenceTemplate {
    fn has_any(&self) -> bool {
        self.order_book_delta_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .sufficient_snapshot_cadence_ref
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .no_tick_size_change_universe_ref
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .timed_instrument_epoch_replay_ref
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

const fn default_source_proof_status() -> SourceProofStatus {
    SourceProofStatus::Accepted
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofBindingSpec {
    pub source_binding: String,
    pub source_proof_id: String,
    pub product_category: String,
    pub instrument_universe_id: String,
    pub category_manifest_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofSummary {
    pub source_binding: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub category_manifest_id: String,
    pub category: String,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub first_archive_date: String,
    pub last_archive_date: String,
    pub proof_path: PathBuf,
    pub proof_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseSourceProofSet {
    pub schema_version: String,
    pub proof_set_id: String,
    pub proof_count: u64,
    pub accepted_proof_count: u64,
    pub total_completed_objects: u64,
    pub total_accepted_bytes: u64,
    pub proofs: Vec<SourceUniverseSourceProofSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseSourceProofSetArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub proof_count: u64,
}

#[derive(Debug, Deserialize)]
struct CategoryObjectManifest {
    manifest_id: String,
    source_binding: String,
    category: String,
    table_family: String,
    object_count: u64,
    accepted_bytes: u64,
    first_archive_date: String,
    last_archive_date: String,
    #[serde(default)]
    payload_records: Vec<CategoryObjectManifestRecord>,
}

#[derive(Debug, Deserialize)]
struct CategoryObjectManifestRecord {
    s3_uri: String,
    #[serde(default)]
    source_hash: String,
    #[serde(default)]
    sha256: String,
    bytes: u64,
    symbol: String,
}

struct TemplateContext<'a> {
    source_proof_id: &'a str,
    source_binding: &'a str,
    product_category: &'a str,
    instrument_universe_id: &'a str,
    manifest: &'a CategoryObjectManifest,
    instrument_count: u64,
}

pub fn write_source_universe_source_proof_set_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseSourceProofSetArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe source-proof spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseSourceProofSetSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source-universe source-proof spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_source_proof_set(&spec, base_dir)
}

pub fn write_source_universe_source_proof_set(
    spec: &SourceUniverseSourceProofSetSpec,
    base_dir: &Path,
) -> Result<SourceUniverseSourceProofSetArtifact> {
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe source-proof output directory {}",
            output_dir.display()
        )
    })?;
    let proof_set = evaluate_and_write_source_universe_source_proofs(spec, base_dir, &output_dir)?;
    let path = output_dir.join(SOURCE_UNIVERSE_SOURCE_PROOF_SET_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_UNIVERSE_SOURCE_PROOF_SET_FILE,
        &proof_set,
    )
    .with_context(|| format!("write source-universe source-proof set {}", path.display()))?;
    Ok(SourceUniverseSourceProofSetArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        proof_count: proof_set.proof_count,
    })
}

fn evaluate_and_write_source_universe_source_proofs(
    spec: &SourceUniverseSourceProofSetSpec,
    base_dir: &Path,
    output_dir: &Path,
) -> Result<SourceUniverseSourceProofSet> {
    ensure!(
        !spec.proof_set_id.trim().is_empty(),
        "proof_set_id must not be empty"
    );
    ensure!(
        !spec.source_bindings.is_empty(),
        "source_binding set must not be empty"
    );
    ensure!(
        spec.raw_sample_selection == "first_manifest_record",
        "raw_sample_selection must be first_manifest_record"
    );
    ensure!(
        spec.schema_sample_policy == "raw_sample",
        "schema_sample_policy must be raw_sample"
    );
    ensure!(
        !spec.claim_limits.is_empty(),
        "claim_limit set must not be empty"
    );
    ensure!(
        spec.fidelity_class == SourceProofFidelityClass::L2Replay
            || !spec.l2_replay_evidence.has_any(),
        "l2_replay_evidence is only valid when fidelity_class is L2_REPLAY"
    );
    validate_acceptance_provenance_config(spec)?;

    let registry =
        crate::source_proof::read_source_binding_registry_from_path(&spec.source_bindings_path)
            .with_context(|| {
                format!(
                    "read source-binding registry {}",
                    spec.source_bindings_path.display()
                )
            })?;
    let mut seen_bindings = BTreeSet::new();
    let mut seen_proofs = BTreeSet::new();
    let mut summaries = Vec::with_capacity(spec.source_bindings.len());
    let mut accepted_proof_count = 0;

    for binding in &spec.source_bindings {
        ensure!(
            seen_bindings.insert(binding.source_binding.clone()),
            "duplicate source_binding {}",
            binding.source_binding
        );
        ensure!(
            seen_proofs.insert(binding.source_proof_id.clone()),
            "duplicate source_proof_id {}",
            binding.source_proof_id
        );
        let manifest_path = resolve_existing_path(base_dir, &binding.category_manifest_path);
        let manifest: CategoryObjectManifest = read_json(&manifest_path)?;
        ensure!(
            manifest.source_binding == binding.source_binding,
            "category manifest source_binding {:?} does not match spec {:?}",
            manifest.source_binding,
            binding.source_binding
        );
        ensure!(
            manifest.table_family == spec.manifest_table_family,
            "category manifest table_family {:?} does not match spec {:?}",
            manifest.table_family,
            spec.manifest_table_family
        );
        ensure!(
            manifest.object_count as usize == manifest.payload_records.len(),
            "category manifest object_count does not match payload_records"
        );
        let manifest_bytes = manifest
            .payload_records
            .iter()
            .map(|record| record.bytes)
            .sum::<u64>();
        ensure!(
            manifest_bytes == manifest.accepted_bytes,
            "category manifest accepted_bytes does not match payload records"
        );
        let source_binding = registry
            .source_binding_metadata(&binding.source_binding, &spec.venue)
            .with_context(|| {
                format!(
                    "source_binding {} for venue {} is not configured",
                    binding.source_binding, spec.venue
                )
            })?;
        let fixture_type = source_binding.market_structure_fixture.with_context(|| {
            format!("source_binding {} missing fixture", binding.source_binding)
        })?;
        let raw_sample = manifest
            .payload_records
            .first()
            .with_context(|| format!("category manifest {} is empty", manifest.manifest_id))?;
        let raw_sample_hash = sample_hash(raw_sample)?;
        let instrument_count = manifest
            .payload_records
            .iter()
            .map(|record| record.symbol.as_str())
            .collect::<BTreeSet<_>>()
            .len() as u64;
        let context = TemplateContext {
            source_proof_id: &binding.source_proof_id,
            source_binding: &binding.source_binding,
            product_category: &binding.product_category,
            instrument_universe_id: &binding.instrument_universe_id,
            manifest: &manifest,
            instrument_count,
        };
        let proof = SourceProofReport {
            source_proof_id: binding.source_proof_id.clone(),
            source_proof_version: 1,
            contract_version: CONTRACT_VERSION.to_string(),
            schema_version: SOURCE_PROOF_SCHEMA_VERSION.to_string(),
            status: spec.status,
            source_binding: binding.source_binding.clone(),
            venue: source_binding.venue,
            product_family: source_binding.product_family,
            product_category: binding.product_category.clone(),
            table_family: spec.table_family.clone(),
            evidence_state: source_binding.evidence_state,
            source_candidate_class: spec.source_candidate_class,
            source_selection_status: spec.source_selection_status,
            usage_scope: spec.usage_scope,
            official_free_gap_ref: None,
            paid_vendor_gap_ref: None,
            fixture_type,
            requested_time_range: TimeRange {
                start_utc: spec.requested_start_utc.clone(),
                end_utc: spec.requested_end_utc.clone(),
            },
            coverage_time_range: TimeRange {
                start_utc: spec.coverage_start_utc.clone(),
                end_utc: spec.coverage_end_utc.clone(),
            },
            instrument_universe_id: binding.instrument_universe_id.clone(),
            raw_sample_uri: raw_sample.s3_uri.clone(),
            raw_sample_hash: raw_sample_hash.clone(),
            schema_sample_uri: raw_sample.s3_uri.clone(),
            schema_sample_hash: raw_sample_hash,
            license_ref: spec.license_ref.clone(),
            license_scope: spec.license_scope,
            retention_ref: spec.retention_ref.clone(),
            cost_ref: spec.cost_ref.clone(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: spec.fidelity_class,
            l2_replay_evidence: l2_replay_evidence(spec, &context),
            forbidden_claims: claim_limits(spec, &context)
                .into_iter()
                .map(|limit| limit.claim)
                .collect(),
            claim_limits: claim_limits(spec, &context),
            cross_market_components: Vec::new(),
            acceptance_scope: Some(AcceptanceScope {
                planned_objects: manifest.object_count,
                completed_objects: manifest.object_count,
                failed_objects: 0,
                skipped_objects: 0,
                accepted_bytes: manifest.accepted_bytes,
                selector_scope_violations: 0,
            }),
            gap_policy_id: spec.gap_policy_id.clone(),
            required_checks: required_checks(spec, &context),
            acceptance_mode: spec.acceptance_mode,
            accepted_by: spec.accepted_by.clone(),
            accepted_at: spec.accepted_at_utc.clone(),
            supersedes_source_proof_id: None,
        };
        if proof.status == SourceProofStatus::Accepted {
            proof
                .evaluate_acceptance_with_registry(&registry)
                .with_context(|| {
                    format!(
                        "generated source proof {} is not accepted",
                        proof.source_proof_id
                    )
                })?;
            accepted_proof_count += 1;
        }
        let proof_path = output_dir.join(format!("{}.json", proof.source_proof_id));
        let proof_artifact = crate::reference_artifact::write_reference_artifact_with_len(
            &proof_path,
            SOURCE_PROOF_SCHEMA_VERSION,
            &proof,
        )
        .with_context(|| format!("write source proof {}", proof_path.display()))?;
        let proof_artifact_path = portable_artifact_path_for_spec(&proof_path, &spec.output_dir)?;
        summaries.push(SourceUniverseSourceProofSummary {
            source_binding: binding.source_binding.clone(),
            source_proof_id: proof.source_proof_id,
            source_proof_version: proof.source_proof_version,
            category_manifest_id: manifest.manifest_id,
            category: manifest.category,
            object_count: manifest.object_count,
            accepted_bytes: manifest.accepted_bytes,
            first_archive_date: manifest.first_archive_date,
            last_archive_date: manifest.last_archive_date,
            proof_path: proof_artifact_path,
            proof_hash: proof_artifact.pin.sha256,
        });
    }

    let total_completed_objects = summaries.iter().map(|summary| summary.object_count).sum();
    let total_accepted_bytes = summaries.iter().map(|summary| summary.accepted_bytes).sum();
    Ok(SourceUniverseSourceProofSet {
        schema_version: SOURCE_UNIVERSE_SOURCE_PROOF_SET_SCHEMA_VERSION.to_string(),
        proof_set_id: spec.proof_set_id.clone(),
        proof_count: summaries.len() as u64,
        accepted_proof_count,
        total_completed_objects,
        total_accepted_bytes,
        proofs: summaries,
    })
}

fn validate_acceptance_provenance_config(spec: &SourceUniverseSourceProofSetSpec) -> Result<()> {
    match spec.status {
        SourceProofStatus::Accepted => {
            ensure!(
                spec.acceptance_mode.is_some(),
                "accepted source proofs require acceptance_mode"
            );
            ensure!(
                spec.accepted_by
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty()),
                "accepted source proofs require accepted_by"
            );
            ensure!(
                spec.accepted_at_utc
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty()),
                "accepted source proofs require accepted_at_utc"
            );
        }
        SourceProofStatus::Pending | SourceProofStatus::Rejected => {
            ensure!(
                spec.acceptance_mode.is_none()
                    && spec
                        .accepted_by
                        .as_ref()
                        .is_none_or(|value| value.trim().is_empty())
                    && spec
                        .accepted_at_utc
                        .as_ref()
                        .is_none_or(|value| value.trim().is_empty()),
                "acceptance provenance is only valid for accepted source proofs"
            );
        }
    }
    Ok(())
}

fn sample_hash(record: &CategoryObjectManifestRecord) -> Result<String> {
    if !record.sha256.trim().is_empty() {
        return Ok(record.sha256.clone());
    }
    ensure!(
        !record.source_hash.trim().is_empty(),
        "raw sample record must include sha256 or source_hash"
    );
    Ok(record.source_hash.clone())
}

fn claim_limits(
    spec: &SourceUniverseSourceProofSetSpec,
    context: &TemplateContext<'_>,
) -> Vec<SourceProofClaimLimit> {
    spec.claim_limits
        .iter()
        .map(|limit| SourceProofClaimLimit {
            id: render_template(&limit.id, context),
            severity: render_template(&limit.severity, context),
            claim: render_template(&limit.claim, context),
            reason: render_template(&limit.reason, context),
            evidence_ref: render_template(&limit.evidence_ref, context),
        })
        .collect()
}

fn l2_replay_evidence(
    spec: &SourceUniverseSourceProofSetSpec,
    context: &TemplateContext<'_>,
) -> L2ReplayEvidence {
    L2ReplayEvidence {
        order_book_delta_ref: render_optional_template(
            spec.l2_replay_evidence.order_book_delta_ref.as_ref(),
            context,
        ),
        sufficient_snapshot_cadence_ref: render_optional_template(
            spec.l2_replay_evidence
                .sufficient_snapshot_cadence_ref
                .as_ref(),
            context,
        ),
        no_tick_size_change_universe_ref: render_optional_template(
            spec.l2_replay_evidence
                .no_tick_size_change_universe_ref
                .as_ref(),
            context,
        ),
        timed_instrument_epoch_replay_ref: render_optional_template(
            spec.l2_replay_evidence
                .timed_instrument_epoch_replay_ref
                .as_ref(),
            context,
        ),
    }
}

fn required_checks(
    spec: &SourceUniverseSourceProofSetSpec,
    context: &TemplateContext<'_>,
) -> RequiredChecks {
    RequiredChecks {
        source_access: required_check(&spec.required_checks.source_access, context),
        license: required_check(&spec.required_checks.license, context),
        schema: required_check(&spec.required_checks.schema, context),
        time_semantics: required_check(&spec.required_checks.time_semantics, context),
        instrument_universe: required_check(&spec.required_checks.instrument_universe, context),
        coverage: required_check(&spec.required_checks.coverage, context),
        retention_freshness: required_check(&spec.required_checks.retention_freshness, context),
        granularity: required_check(&spec.required_checks.granularity, context),
        completeness: required_check(&spec.required_checks.completeness, context),
        nt_mapping: required_check(&spec.required_checks.nt_mapping, context),
        cost: required_check(&spec.required_checks.cost, context),
        storage: required_check(&spec.required_checks.storage, context),
    }
}

fn required_check(
    template: &SourceUniverseSourceProofRequiredCheckTemplate,
    context: &TemplateContext<'_>,
) -> RequiredCheck {
    match template {
        SourceUniverseSourceProofRequiredCheckTemplate::PassedEvidenceRef(evidence_ref) => {
            RequiredCheck::passed(render_template(evidence_ref, context))
        }
        SourceUniverseSourceProofRequiredCheckTemplate::Structured(template) => RequiredCheck {
            outcome: template.outcome,
            evidence_ref: render_template(&template.evidence_ref, context),
            expires_at_utc: render_optional_template(template.expires_at_utc.as_ref(), context),
        },
    }
}

fn render_template(template: &str, context: &TemplateContext<'_>) -> String {
    let replacements = BTreeMap::from([
        ("{source_proof_id}", context.source_proof_id.to_string()),
        ("{source_binding}", context.source_binding.to_string()),
        ("{product_category}", context.product_category.to_string()),
        (
            "{instrument_universe_id}",
            context.instrument_universe_id.to_string(),
        ),
        ("{manifest_id}", context.manifest.manifest_id.clone()),
        ("{category}", context.manifest.category.clone()),
        ("{object_count}", context.manifest.object_count.to_string()),
        (
            "{accepted_bytes}",
            context.manifest.accepted_bytes.to_string(),
        ),
        (
            "{first_archive_date}",
            context.manifest.first_archive_date.clone(),
        ),
        (
            "{last_archive_date}",
            context.manifest.last_archive_date.clone(),
        ),
        ("{instrument_count}", context.instrument_count.to_string()),
    ]);
    let mut output = template.to_string();
    for (token, value) in replacements {
        output = output.replace(token, &value);
    }
    output
}

fn render_optional_template(
    template: Option<&String>,
    context: &TemplateContext<'_>,
) -> Option<String> {
    let rendered = render_template(template?, context);
    if rendered.trim().is_empty() {
        None
    } else {
        Some(rendered)
    }
}

fn read_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).with_context(|| format!("read JSON artifact {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse JSON artifact {}", path.display()))
}
