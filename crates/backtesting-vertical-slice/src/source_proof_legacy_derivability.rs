//! Legacy source-proof derivability reporting.
//!
//! This report answers which current-contract fields can be derived from the
//! staged source-proof-v3 JSON plus its S3 acceptance manifest. It does not
//! create accepted source proofs, migrate source proofs, or infer venue-specific
//! semantics.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::path_resolution::{resolve_existing_path, resolve_output_dir};
use crate::source_proof::EvidenceState;

pub const SOURCE_PROOF_LEGACY_DERIVABILITY_SCHEMA_VERSION: &str =
    "source-proof-legacy-derivability-report.v1";
pub const SOURCE_PROOF_LEGACY_DERIVABILITY_REPORT_FILE: &str =
    "source-proof-legacy-derivability-report.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofLegacyDerivableField {
    SourceBinding,
    TableFamily,
    FixtureType,
    RequestedTimeRange,
    CoverageTimeRange,
    RawSampleUri,
    RawSampleHash,
    AcceptanceScope,
    ClaimLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofLegacyDerivabilityIssue {
    MissingSourceBindingKey,
    NotExactlyOneTableFamily,
    MissingSourceTimeRange,
    RawPayloadNotFullyS3Bound,
    LicenseNotPassed,
    NtMappingNotPassed,
    FidelityNotPassed,
    ForbiddenClaimsNotPassed,
    SchemaSampleNotPassed,
    MissingVenue,
    MissingProductFamily,
    MissingEvidenceState,
    EvidenceStateNotBackfillable,
    UnknownSourceBinding,
    SourceBindingProductFamilyMismatch,
    SourceBindingEvidenceStateMismatch,
    SourceBindingTableFamilyMismatch,
    MissingRawPayloadFields,
    AcceptedBytesFromS3Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilityRecord {
    pub proof_uri: String,
    pub source_proof_id: Option<String>,
    pub source_proof_version: Option<u32>,
    pub source_binding: Option<String>,
    pub venue: Option<String>,
    pub product_family: Option<String>,
    pub evidence_state: Option<EvidenceState>,
    pub legacy_status: Option<String>,
    pub raw_payload_records: u64,
    pub s3_bound_raw_payload_records: u64,
    pub accepted_bytes_from_s3: u64,
    pub table_families: Vec<String>,
    pub derivable_fields: Vec<SourceProofLegacyDerivableField>,
    pub blocking_issues: Vec<SourceProofLegacyDerivabilityIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilitySummary {
    pub total_records: u64,
    pub s3_bound_records: u64,
    pub single_table_family_records: u64,
    pub acceptance_blocked_records: u64,
    pub blocking_issue_count: u64,
    #[serde(default)]
    pub blocking_issue_counts: Vec<SourceProofLegacyDerivabilityIssueCount>,
    #[serde(default)]
    pub table_family_counts: Vec<SourceProofLegacyDerivabilityTableFamilyCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilityIssueCount {
    pub issue: SourceProofLegacyDerivabilityIssue,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilityTableFamilyCount {
    pub table_family: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilityReport {
    pub schema_version: String,
    pub report_id: String,
    pub records: Vec<SourceProofLegacyDerivabilityRecord>,
    pub summary: SourceProofLegacyDerivabilitySummary,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceProofLegacyDerivabilityJson {
    pub proof_uri: String,
    pub proof: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilityProofFile {
    pub proof_uri: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofLegacyDerivabilitySpec {
    pub report_id: String,
    pub output_dir: PathBuf,
    pub acceptance_manifest_path: PathBuf,
    #[serde(rename = "source_proof", default)]
    pub source_proofs: Vec<SourceProofLegacyDerivabilityProofFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProofLegacyDerivabilityArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofLegacyDerivabilityReportError {
    EmptyReportId,
    EmptyProofUri,
    DuplicateProofUri(String),
    Serialize(String),
}

impl fmt::Display for SourceProofLegacyDerivabilityReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyReportId => write!(f, "source-proof legacy derivability report id is empty"),
            Self::EmptyProofUri => write!(f, "source-proof legacy derivability proof uri is empty"),
            Self::DuplicateProofUri(proof_uri) => write!(
                f,
                "source-proof legacy derivability report has duplicate proof uri {proof_uri:?}"
            ),
            Self::Serialize(error) => {
                write!(
                    f,
                    "serialize source-proof legacy derivability report: {error}"
                )
            }
        }
    }
}

impl Error for SourceProofLegacyDerivabilityReportError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofLegacyDerivabilityWriteError {
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    Serialize(String),
    ExistingArtifactMismatch { path: String },
}

impl fmt::Display for SourceProofLegacyDerivabilityWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create source-proof legacy derivability artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing source-proof legacy derivability artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(
                    f,
                    "write source-proof legacy derivability artifact {path}: {error}"
                )
            }
            Self::Serialize(error) => write!(
                f,
                "serialize source-proof legacy derivability artifact: {error}"
            ),
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty source-proof legacy derivability artifact {path}: existing file content differs"
            ),
        }
    }
}

impl Error for SourceProofLegacyDerivabilityWriteError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofLegacyDerivabilityFileError {
    ReadSpec {
        path: String,
        error: String,
    },
    ParseSpecToml {
        path: String,
        error: String,
    },
    ReadAcceptanceManifest {
        path: String,
        error: String,
    },
    ParseAcceptanceManifest {
        path: String,
        error: String,
    },
    ReadSourceProof {
        proof_uri: String,
        path: String,
        error: String,
    },
    ParseSourceProofJson {
        proof_uri: String,
        path: String,
        error: String,
    },
    BuildReport(SourceProofLegacyDerivabilityReportError),
    WriteArtifact(SourceProofLegacyDerivabilityWriteError),
}

impl fmt::Display for SourceProofLegacyDerivabilityFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(
                    f,
                    "read source-proof legacy derivability spec {path}: {error}"
                )
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source-proof legacy derivability spec TOML {path}: {error}"
            ),
            Self::ReadAcceptanceManifest { path, error } => {
                write!(
                    f,
                    "read source-proof legacy acceptance manifest {path}: {error}"
                )
            }
            Self::ParseAcceptanceManifest { path, error } => write!(
                f,
                "parse source-proof legacy acceptance manifest JSON {path}: {error}"
            ),
            Self::ReadSourceProof {
                proof_uri,
                path,
                error,
            } => write!(f, "read source proof {proof_uri} from {path}: {error}"),
            Self::ParseSourceProofJson {
                proof_uri,
                path,
                error,
            } => write!(
                f,
                "parse source proof JSON {proof_uri} from {path}: {error}"
            ),
            Self::BuildReport(error) => {
                write!(f, "build source-proof legacy derivability report: {error}")
            }
            Self::WriteArtifact(error) => {
                write!(f, "write source-proof legacy derivability report: {error}")
            }
        }
    }
}

impl Error for SourceProofLegacyDerivabilityFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BuildReport(error) => Some(error),
            Self::WriteArtifact(error) => Some(error),
            _ => None,
        }
    }
}

impl SourceProofLegacyDerivabilityReport {
    pub fn from_json_values(
        report_id: impl Into<String>,
        acceptance_manifest: &Value,
        source_proofs: Vec<SourceProofLegacyDerivabilityJson>,
    ) -> Result<Self, SourceProofLegacyDerivabilityReportError> {
        let report_id = report_id.into();
        if report_id.trim().is_empty() {
            return Err(SourceProofLegacyDerivabilityReportError::EmptyReportId);
        }
        let s3_payloads = s3_payloads_by_binding_hash(acceptance_manifest);
        let mut proof_uris = BTreeSet::new();
        let mut records = Vec::with_capacity(source_proofs.len());
        for source_proof in source_proofs {
            if source_proof.proof_uri.trim().is_empty() {
                return Err(SourceProofLegacyDerivabilityReportError::EmptyProofUri);
            }
            if !proof_uris.insert(source_proof.proof_uri.clone()) {
                return Err(SourceProofLegacyDerivabilityReportError::DuplicateProofUri(
                    source_proof.proof_uri,
                ));
            }
            records.push(classify_legacy_source_proof_json(
                source_proof,
                &s3_payloads,
            ));
        }
        let summary = SourceProofLegacyDerivabilitySummary::from_records(&records);
        Ok(Self {
            schema_version: SOURCE_PROOF_LEGACY_DERIVABILITY_SCHEMA_VERSION.to_string(),
            report_id,
            records,
            summary,
        })
    }

    pub fn content_hash(&self) -> Result<String, SourceProofLegacyDerivabilityReportError> {
        crate::reference_artifact::canonical_json_sha256(self)
            .map_err(|error| SourceProofLegacyDerivabilityReportError::Serialize(error.to_string()))
    }
}

impl SourceProofLegacyDerivabilitySummary {
    fn from_records(records: &[SourceProofLegacyDerivabilityRecord]) -> Self {
        let mut summary = Self {
            total_records: records.len() as u64,
            s3_bound_records: 0,
            single_table_family_records: 0,
            acceptance_blocked_records: 0,
            blocking_issue_count: 0,
            blocking_issue_counts: Vec::new(),
            table_family_counts: Vec::new(),
        };
        let mut blocking_issue_counts = BTreeMap::new();
        let mut table_family_counts = BTreeMap::new();
        for record in records {
            if record.raw_payload_records == record.s3_bound_raw_payload_records {
                summary.s3_bound_records += 1;
            }
            if record.table_families.len() == 1 {
                summary.single_table_family_records += 1;
            }
            for table_family in &record.table_families {
                *table_family_counts
                    .entry(table_family.clone())
                    .or_insert(0_u64) += 1;
            }
            if !record.blocking_issues.is_empty() {
                summary.acceptance_blocked_records += 1;
            }
            for issue in &record.blocking_issues {
                *blocking_issue_counts.entry(*issue).or_insert(0_u64) += 1;
            }
            summary.blocking_issue_count = summary
                .blocking_issue_count
                .saturating_add(record.blocking_issues.len() as u64);
        }
        summary.blocking_issue_counts = blocking_issue_counts
            .into_iter()
            .map(|(issue, count)| SourceProofLegacyDerivabilityIssueCount { issue, count })
            .collect();
        summary.table_family_counts = table_family_counts
            .into_iter()
            .map(
                |(table_family, count)| SourceProofLegacyDerivabilityTableFamilyCount {
                    table_family,
                    count,
                },
            )
            .collect();
        summary
    }
}

pub fn write_source_proof_legacy_derivability_report(
    output_dir: &Path,
    report: &SourceProofLegacyDerivabilityReport,
) -> Result<SourceProofLegacyDerivabilityArtifact, SourceProofLegacyDerivabilityWriteError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        SourceProofLegacyDerivabilityWriteError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(SOURCE_PROOF_LEGACY_DERIVABILITY_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_PROOF_LEGACY_DERIVABILITY_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: SourceProofLegacyDerivabilityWriteError::Serialize,
            read_existing_error: |path, error| {
                SourceProofLegacyDerivabilityWriteError::ReadExisting { path, error }
            },
            mismatch_error: |path| {
                SourceProofLegacyDerivabilityWriteError::ExistingArtifactMismatch { path }
            },
            write_error: |path, error| SourceProofLegacyDerivabilityWriteError::Write {
                path,
                error,
            },
        },
    )?;
    Ok(SourceProofLegacyDerivabilityArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        record_count: report.records.len() as u64,
    })
}

pub fn write_source_proof_legacy_derivability_report_from_spec_file(
    spec_path: &Path,
) -> Result<SourceProofLegacyDerivabilityArtifact, SourceProofLegacyDerivabilityFileError> {
    let path_display = spec_path.display().to_string();
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        SourceProofLegacyDerivabilityFileError::ReadSpec {
            path: path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let spec: SourceProofLegacyDerivabilitySpec = toml::from_str(&spec_text).map_err(|error| {
        SourceProofLegacyDerivabilityFileError::ParseSpecToml {
            path: path_display,
            error: error.to_string(),
        }
    })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    let acceptance_manifest_path = resolve_existing_path(base_dir, &spec.acceptance_manifest_path);
    write_source_proof_legacy_derivability_report_from_files_with_base(
        &output_dir,
        spec.report_id,
        &acceptance_manifest_path,
        &spec.acceptance_manifest_path,
        spec.source_proofs,
        base_dir,
    )
}

pub fn write_source_proof_legacy_derivability_report_from_files(
    output_dir: &Path,
    report_id: impl Into<String>,
    acceptance_manifest_path: &Path,
    source_proof_files: Vec<SourceProofLegacyDerivabilityProofFile>,
) -> Result<SourceProofLegacyDerivabilityArtifact, SourceProofLegacyDerivabilityFileError> {
    write_source_proof_legacy_derivability_report_from_files_with_base(
        output_dir,
        report_id,
        acceptance_manifest_path,
        acceptance_manifest_path,
        source_proof_files,
        Path::new("."),
    )
}

fn write_source_proof_legacy_derivability_report_from_files_with_base(
    output_dir: &Path,
    report_id: impl Into<String>,
    acceptance_manifest_path: &Path,
    acceptance_manifest_display_path: &Path,
    source_proof_files: Vec<SourceProofLegacyDerivabilityProofFile>,
    base_dir: &Path,
) -> Result<SourceProofLegacyDerivabilityArtifact, SourceProofLegacyDerivabilityFileError> {
    let acceptance_path_display = acceptance_manifest_display_path.display().to_string();
    let acceptance_bytes = fs::read(acceptance_manifest_path).map_err(|error| {
        SourceProofLegacyDerivabilityFileError::ReadAcceptanceManifest {
            path: acceptance_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let acceptance_manifest: Value =
        serde_json::from_slice(&acceptance_bytes).map_err(|error| {
            SourceProofLegacyDerivabilityFileError::ParseAcceptanceManifest {
                path: acceptance_path_display,
                error: error.to_string(),
            }
        })?;

    let source_proofs = source_proof_files
        .into_iter()
        .map(|source_proof| {
            let SourceProofLegacyDerivabilityProofFile { proof_uri, path } = source_proof;
            let path_display = path.display().to_string();
            let resolved_path = resolve_existing_path(base_dir, &path);
            let bytes = fs::read(&resolved_path).map_err(|error| {
                SourceProofLegacyDerivabilityFileError::ReadSourceProof {
                    proof_uri: proof_uri.clone(),
                    path: path_display.clone(),
                    error: error.to_string(),
                }
            })?;
            let proof: Value = serde_json::from_slice(&bytes).map_err(|error| {
                SourceProofLegacyDerivabilityFileError::ParseSourceProofJson {
                    proof_uri: proof_uri.clone(),
                    path: path_display,
                    error: error.to_string(),
                }
            })?;
            Ok(SourceProofLegacyDerivabilityJson { proof_uri, proof })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let report = SourceProofLegacyDerivabilityReport::from_json_values(
        report_id,
        &acceptance_manifest,
        source_proofs,
    )
    .map_err(SourceProofLegacyDerivabilityFileError::BuildReport)?;
    write_source_proof_legacy_derivability_report(output_dir, &report)
        .map_err(SourceProofLegacyDerivabilityFileError::WriteArtifact)
}

fn classify_legacy_source_proof_json(
    source_proof: SourceProofLegacyDerivabilityJson,
    s3_payloads: &BTreeMap<(String, String), LegacyAcceptedPayload>,
) -> SourceProofLegacyDerivabilityRecord {
    let SourceProofLegacyDerivabilityJson { proof_uri, proof } = source_proof;
    let source_binding = string_field(&proof, "source_binding_key");
    let raw_payloads = raw_payload_records(&proof);
    let mut s3_bound_raw_payload_records = 0_u64;
    let mut accepted_bytes_from_s3 = 0_u64;
    let mut has_malformed_raw_payload = false;
    let mut has_unknown_accepted_bytes = false;
    for payload in &raw_payloads {
        // A malformed entry (missing source_binding or payload_hash) is counted in
        // the denominator above but can never be S3-bound, so it stays out of the
        // numerator and is flagged below.
        let Some(key) = payload.s3_lookup_key() else {
            has_malformed_raw_payload = true;
            continue;
        };
        let Some(record) = s3_payloads.get(&key) else {
            continue;
        };
        s3_bound_raw_payload_records += 1;
        match record.bytes {
            Some(bytes) => {
                accepted_bytes_from_s3 = accepted_bytes_from_s3.saturating_add(bytes);
            }
            None => has_unknown_accepted_bytes = true,
        }
    }

    let table_families = string_array_field(&proof, "table_families");
    let mut blocking_issues = Vec::new();
    if source_binding.is_none() {
        blocking_issues.push(SourceProofLegacyDerivabilityIssue::MissingSourceBindingKey);
    }
    if table_families.len() != 1 {
        blocking_issues.push(SourceProofLegacyDerivabilityIssue::NotExactlyOneTableFamily);
    }
    if !has_source_time_range(&proof) {
        blocking_issues.push(SourceProofLegacyDerivabilityIssue::MissingSourceTimeRange);
    }
    if has_malformed_raw_payload {
        blocking_issues.push(SourceProofLegacyDerivabilityIssue::MissingRawPayloadFields);
    }
    if raw_payloads.len() as u64 != s3_bound_raw_payload_records {
        blocking_issues.push(SourceProofLegacyDerivabilityIssue::RawPayloadNotFullyS3Bound);
    }
    if has_unknown_accepted_bytes {
        // Informational diagnostic: the accepted-bytes counter has no downstream
        // gate, but an absent/ill-typed S3 byte value must not be silently zeroed.
        blocking_issues.push(SourceProofLegacyDerivabilityIssue::AcceptedBytesFromS3Unknown);
    }
    blocking_issues.extend(required_check_blockers(&proof));

    let derivable_fields = derivable_fields(
        &proof,
        source_binding.is_some(),
        table_families.len() == 1,
        raw_payloads.len(),
        s3_bound_raw_payload_records,
    );

    SourceProofLegacyDerivabilityRecord {
        proof_uri,
        source_proof_id: string_field(&proof, "source_proof_id"),
        source_proof_version: u32_field(&proof, "source_proof_version"),
        source_binding,
        venue: string_field(&proof, "venue"),
        product_family: string_field(&proof, "product_family"),
        evidence_state: evidence_state_field(&proof),
        legacy_status: string_field(&proof, "status"),
        raw_payload_records: raw_payloads.len() as u64,
        s3_bound_raw_payload_records,
        accepted_bytes_from_s3,
        table_families,
        derivable_fields,
        blocking_issues,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyAcceptedPayload {
    /// `None` when the matching S3 record carried no `bytes` field, or a value
    /// that was not a non-negative integer. The byte counter it feeds is an
    /// informational diagnostic, so an absent/ill-typed value must surface as a
    /// diagnostic rather than silently zeroing the count.
    bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyRawPayload {
    /// Both fields are `Some` only for a fully-formed entry. A malformed entry
    /// missing `source_binding` or `payload_hash` is retained (never dropped) so
    /// the `raw_payloads.len()` denominator stays honest and its S3-boundedness
    /// is still tested by the gate.
    source_binding: Option<String>,
    payload_hash: Option<String>,
}

impl LegacyRawPayload {
    fn s3_lookup_key(&self) -> Option<(String, String)> {
        match (&self.source_binding, &self.payload_hash) {
            (Some(source_binding), Some(payload_hash)) => {
                Some((source_binding.clone(), payload_hash.clone()))
            }
            _ => None,
        }
    }
}

fn s3_payloads_by_binding_hash(
    manifest: &Value,
) -> BTreeMap<(String, String), LegacyAcceptedPayload> {
    manifest
        .get("s3_payload_records")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|record| {
            let source_binding = string_field(record, "source_binding")?;
            let payload_hash = string_field(record, "payload_hash")?;
            let bytes = u64_field(record, "bytes");
            Some((
                (source_binding, payload_hash),
                LegacyAcceptedPayload { bytes },
            ))
        })
        .collect()
}

fn raw_payload_records(proof: &Value) -> Vec<LegacyRawPayload> {
    proof
        .get("raw_payload_records")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|record| LegacyRawPayload {
            source_binding: string_field(record, "source_binding"),
            payload_hash: string_field(record, "payload_hash"),
        })
        .collect()
}

fn derivable_fields(
    proof: &Value,
    has_source_binding: bool,
    has_single_table_family: bool,
    raw_payload_count: usize,
    s3_bound_raw_payload_records: u64,
) -> Vec<SourceProofLegacyDerivableField> {
    let mut fields = BTreeSet::new();
    if has_source_binding {
        fields.insert(SourceProofLegacyDerivableField::SourceBinding);
    }
    if has_single_table_family {
        fields.insert(SourceProofLegacyDerivableField::TableFamily);
    }
    if proof.get("fixture").and_then(Value::as_str).is_some() {
        fields.insert(SourceProofLegacyDerivableField::FixtureType);
    }
    if has_source_time_range(proof) {
        fields.insert(SourceProofLegacyDerivableField::RequestedTimeRange);
        fields.insert(SourceProofLegacyDerivableField::CoverageTimeRange);
    }
    if raw_payload_count == 1 && s3_bound_raw_payload_records == 1 {
        fields.insert(SourceProofLegacyDerivableField::RawSampleUri);
        fields.insert(SourceProofLegacyDerivableField::RawSampleHash);
    }
    if raw_payload_count as u64 == s3_bound_raw_payload_records && raw_payload_count != 0 {
        fields.insert(SourceProofLegacyDerivableField::AcceptanceScope);
    }
    if proof
        .get("forbidden_claims")
        .and_then(Value::as_array)
        .is_some_and(|claims| !claims.is_empty())
    {
        fields.insert(SourceProofLegacyDerivableField::ClaimLimits);
    }
    fields.into_iter().collect()
}

fn required_check_blockers(proof: &Value) -> Vec<SourceProofLegacyDerivabilityIssue> {
    let mut issues = Vec::new();
    if check_status(proof, "license") != Some("passed") {
        issues.push(SourceProofLegacyDerivabilityIssue::LicenseNotPassed);
    }
    if check_status(proof, "nt_mapping") != Some("passed") {
        issues.push(SourceProofLegacyDerivabilityIssue::NtMappingNotPassed);
    }
    if check_status(proof, "fidelity") != Some("passed") {
        issues.push(SourceProofLegacyDerivabilityIssue::FidelityNotPassed);
    }
    if check_status(proof, "forbidden_claims") != Some("passed") {
        issues.push(SourceProofLegacyDerivabilityIssue::ForbiddenClaimsNotPassed);
    }
    if check_status(proof, "schema_sample") != Some("passed") {
        issues.push(SourceProofLegacyDerivabilityIssue::SchemaSampleNotPassed);
    }
    issues
}

fn check_status<'a>(proof: &'a Value, check: &str) -> Option<&'a str> {
    proof
        .get("required_checks")
        .and_then(Value::as_object)
        .and_then(|checks| checks.get(check))
        .and_then(Value::as_str)
}

fn has_source_time_range(proof: &Value) -> bool {
    proof.get("source_time_range").is_some_and(|range| {
        string_field(range, "start_utc").is_some() && string_field(range, "end_utc").is_some()
    })
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn string_array_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn u32_field(value: &Value, field: &str) -> Option<u32> {
    let raw = value.get(field)?.as_u64()?;
    u32::try_from(raw).ok()
}

fn evidence_state_field(value: &Value) -> Option<EvidenceState> {
    value
        .get("evidence_state")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn u64_field(value: &Value, field: &str) -> Option<u64> {
    value.get(field)?.as_u64()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn classify(proof: Value, manifest: Value) -> SourceProofLegacyDerivabilityRecord {
        let report = SourceProofLegacyDerivabilityReport::from_json_values(
            "legacy-derivability-test",
            &manifest,
            vec![SourceProofLegacyDerivabilityJson {
                proof_uri: "source-proof://legacy/test".to_string(),
                proof,
            }],
        )
        .expect("report builds");
        report
            .records
            .into_iter()
            .next()
            .expect("one classified record")
    }

    #[test]
    fn raw_payload_entry_missing_payload_hash_is_flagged_and_still_counted() {
        let manifest = json!({
            "s3_payload_records": [
                {
                    "source_binding": "venue-binding",
                    "payload_hash": "hash-bound",
                    "bytes": 7,
                }
            ]
        });
        // Two raw payload entries: one fully-formed and S3-bound, one missing
        // payload_hash. The malformed entry must not be dropped — both the
        // denominator and an explicit blocking issue must reflect it.
        let proof = json!({
            "raw_payload_records": [
                { "source_binding": "venue-binding", "payload_hash": "hash-bound" },
                { "source_binding": "venue-binding" }
            ]
        });

        let record = classify(proof, manifest);

        assert_eq!(
            record.raw_payload_records, 2,
            "malformed raw-payload entry must still be counted in the denominator"
        );
        assert_eq!(
            record.s3_bound_raw_payload_records, 1,
            "only the fully-formed entry is S3-bound"
        );
        assert!(
            record
                .blocking_issues
                .contains(&SourceProofLegacyDerivabilityIssue::MissingRawPayloadFields),
            "a raw-payload entry missing payload_hash must raise MissingRawPayloadFields: {:?}",
            record.blocking_issues
        );
        assert!(
            record
                .blocking_issues
                .contains(&SourceProofLegacyDerivabilityIssue::RawPayloadNotFullyS3Bound),
            "the dropped denominator no longer hides an unbound payload: {:?}",
            record.blocking_issues
        );
    }

    #[test]
    fn s3_record_with_absent_bytes_is_diagnosed_not_silently_zeroed() {
        let manifest = json!({
            "s3_payload_records": [
                {
                    "source_binding": "venue-binding",
                    "payload_hash": "hash-bound"
                }
            ]
        });
        let proof = json!({
            "raw_payload_records": [
                { "source_binding": "venue-binding", "payload_hash": "hash-bound" }
            ]
        });

        let record = classify(proof, manifest);

        assert_eq!(
            record.s3_bound_raw_payload_records, 1,
            "the entry is still S3-bound even when its byte count is absent"
        );
        assert_eq!(
            record.accepted_bytes_from_s3, 0,
            "absent bytes contribute nothing to the informational counter"
        );
        assert!(
            record
                .blocking_issues
                .contains(&SourceProofLegacyDerivabilityIssue::AcceptedBytesFromS3Unknown),
            "an absent S3 byte count must surface as a diagnostic rather than a silent zero: {:?}",
            record.blocking_issues
        );
    }
}
