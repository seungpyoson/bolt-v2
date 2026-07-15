//! Batch execution for source-universe single-object operator runs.
//!
//! Source-universe execution packs already materialize one run-spec and
//! execution plan per accepted object. This module adds the missing operator
//! loop: fetch the pinned object, verify bytes/hash, run the existing
//! single-object operator path, and summarize the completed records.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::atomic_artifact_write::atomic_write;
use crate::backfill_accepted_tranche::BackfillAcceptedTrancheManifest;
use crate::backfill_execution_plan::{
    BackfillExecutionPlan, ValidatedBackfillExecutionControls,
    validate_backfill_execution_control_bytes,
};
use crate::catalog_projection::logical_catalog_hash;
use crate::path_resolution::{
    claim_contained_output_component, resolve_contained_output_component,
    resolve_pack_control_path, validate_portable_path_component,
};
use crate::{
    operator::{
        CATALOG_DIR, RunSpec, run_from_run_spec_with_registry,
        validate_run_spec_manifest_for_object_hash_with_registry,
    },
    source_proof::SourceBindingRegistry,
    source_universe_execution_pack::{
        SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION, SourceUniverseExecutionPack,
        SourceUniverseExecutionPackRecord, SourceUniverseExecutionPackStatus,
    },
};

pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION: &str =
    "source-universe-batch-execution-report.v3";
pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE: &str =
    "source-universe-batch-execution-report.json";

pub trait SourceUniverseObjectFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>>;
}

pub trait SourceUniverseOperatorRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        object_bytes: &[u8],
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
    ) -> Result<SourceUniverseBatchExecutionRunOutput>;
}

/// Exact control-artifact bytes verified against an execution-pack record.
///
/// Runners consume these bytes instead of reopening mutable source paths after
/// a potentially long object fetch. `Arc` keeps shared tranche/plan content
/// cheap across selected records and parallel workers.
#[derive(Debug, Clone)]
pub struct SourceUniverseVerifiedControlArtifacts {
    pub run_spec_path: PathBuf,
    pub run_spec_bytes: Arc<[u8]>,
    pub run_spec: Arc<RunSpec>,
    pub accepted_tranche_path: PathBuf,
    pub accepted_tranche_bytes: Arc<[u8]>,
    pub accepted_tranche: Arc<BackfillAcceptedTrancheManifest>,
    pub execution_plan_path: PathBuf,
    pub execution_plan_bytes: Arc<[u8]>,
    pub execution_plan: Arc<BackfillExecutionPlan>,
    pub source_bindings_path: PathBuf,
    pub source_bindings_bytes: Arc<[u8]>,
    pub source_bindings_sha256: String,
    pub source_bindings: Arc<SourceBindingRegistry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionRunOutput {
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
}

/// Tuning for a source-universe batch execution run.
///
/// Every field beyond the original three is opt-in: the defaults
/// (`max_concurrent_records: None`, `resume_report: None`) reproduce the
/// original serial, non-resuming behavior byte-for-byte. Object caching is
/// deliberately NOT a config field: the one way to enable it is wrapping the
/// fetcher in [`CachingSourceUniverseObjectFetcher`] (the CLI does this for
/// `--object-cache-dir`), so a cache can never be requested without taking
/// effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionConfig {
    /// Lowest pack sequence to execute (inclusive). `None` starts at the first.
    pub start_sequence: Option<u64>,
    /// Maximum number of selected records to execute. `None` means unbounded.
    pub record_limit: Option<u64>,
    /// Collect per-record failures instead of returning on the first one.
    pub continue_on_error: bool,
    /// Bound on records processed concurrently. `None` or `Some(1)` is serial.
    pub max_concurrent_records: Option<u64>,
    /// Prior batch report to resume from. `None` reprocesses every record.
    ///
    /// Carried records keep their prior run's `output_dir` verbatim, so a
    /// resumed report can span two output roots: consumers must follow
    /// `records[].output_dir` per record, never glob the new run's root.
    /// Resumed runs must write to a fresh output dir; resuming into the dir
    /// that holds the resume report itself is rejected up front because the
    /// clean-write report guard preserves the prior report as evidence.
    pub resume_report: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseBatchExecutionReportStatus {
    Completed,
    CompletedWithFailures,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionRecord {
    pub sequence: u64,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub selected_object_sha256: String,
    pub run_spec_sha256: String,
    pub accepted_tranche_sha256: String,
    pub execution_plan_sha256: String,
    pub execution_record_sha256: String,
    pub source_bindings_sha256: String,
    pub selected_object_bytes: u64,
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionFailureRecord {
    pub sequence: u64,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub selected_object_sha256: String,
    pub execution_record_sha256: String,
    pub selected_object_bytes: u64,
    pub failure_stage: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionReport {
    pub schema_version: String,
    pub batch_id: String,
    pub status: SourceUniverseBatchExecutionReportStatus,
    pub pack_id: String,
    pub universe_id: String,
    pub venue: String,
    pub selected_record_count: u64,
    pub completed_record_count: u64,
    pub failed_record_count: u64,
    pub total_canonical_rows: u64,
    pub total_nt_catalog_rows: u64,
    pub records: Vec<SourceUniverseBatchExecutionRecord>,
    pub failures: Vec<SourceUniverseBatchExecutionFailureRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionReportArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub completed_record_count: u64,
}

/// Validate a batch report as evidence rather than trusting its aggregate
/// counters. This is the single consume boundary used by report publication,
/// resume loading, and conversion-completion reconciliation.
pub fn validate_source_universe_batch_execution_report(
    report: &SourceUniverseBatchExecutionReport,
) -> Result<()> {
    ensure!(
        report.schema_version == SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION,
        "batch execution report schema_version mismatch: expected {}, got {}",
        SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION,
        report.schema_version
    );
    for (name, value) in [
        ("batch_id", report.batch_id.as_str()),
        ("pack_id", report.pack_id.as_str()),
        ("universe_id", report.universe_id.as_str()),
        ("venue", report.venue.as_str()),
    ] {
        ensure!(
            !value.trim().is_empty(),
            "batch execution report {name} must not be empty"
        );
    }

    let completed_record_count = u64::try_from(report.records.len())
        .context("batch execution report completed record count exceeds u64")?;
    let failed_record_count = u64::try_from(report.failures.len())
        .context("batch execution report failed record count exceeds u64")?;
    let selected_record_count = completed_record_count
        .checked_add(failed_record_count)
        .context("batch execution report selected record count overflow")?;
    ensure!(
        report.selected_record_count == selected_record_count,
        "batch execution report selected_record_count mismatch: expected {}, got {}",
        selected_record_count,
        report.selected_record_count
    );
    ensure!(
        report.completed_record_count == completed_record_count,
        "batch execution report completed_record_count mismatch: expected {}, got {}",
        completed_record_count,
        report.completed_record_count
    );
    ensure!(
        report.failed_record_count == failed_record_count,
        "batch execution report failed_record_count mismatch: expected {}, got {}",
        failed_record_count,
        report.failed_record_count
    );

    let expected_status = if report.failures.is_empty() {
        SourceUniverseBatchExecutionReportStatus::Completed
    } else if report.records.is_empty() {
        SourceUniverseBatchExecutionReportStatus::Failed
    } else {
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    };
    ensure!(
        report.status == expected_status,
        "batch execution report status does not match its record and failure sets"
    );

    let total_canonical_rows = report.records.iter().try_fold(0_u64, |total, record| {
        total
            .checked_add(record.canonical_rows)
            .context("batch execution report total_canonical_rows overflow")
    })?;
    let total_nt_catalog_rows = report.records.iter().try_fold(0_u64, |total, record| {
        total
            .checked_add(record.nt_catalog_rows)
            .context("batch execution report total_nt_catalog_rows overflow")
    })?;
    ensure!(
        report.total_canonical_rows == total_canonical_rows,
        "batch execution report total_canonical_rows mismatch: expected {}, got {}",
        total_canonical_rows,
        report.total_canonical_rows
    );
    ensure!(
        report.total_nt_catalog_rows == total_nt_catalog_rows,
        "batch execution report total_nt_catalog_rows mismatch: expected {}, got {}",
        total_nt_catalog_rows,
        report.total_nt_catalog_rows
    );

    let mut sequences = BTreeSet::new();
    let mut operator_run_ids = BTreeSet::new();
    for record in &report.records {
        validate_report_record_identity(
            record.sequence,
            &record.operator_run_id,
            &record.source_binding,
            &record.category,
            &record.symbol,
            &record.archive_date,
        )?;
        ensure!(
            sequences.insert(record.sequence),
            "batch execution report has duplicate sequence {}",
            record.sequence
        );
        ensure!(
            operator_run_ids.insert(record.operator_run_id.as_str()),
            "batch execution report has duplicate operator_run_id {:?}",
            record.operator_run_id
        );
        for (name, digest) in [
            (
                "selected_object_sha256",
                record.selected_object_sha256.as_str(),
            ),
            ("run_spec_sha256", record.run_spec_sha256.as_str()),
            (
                "accepted_tranche_sha256",
                record.accepted_tranche_sha256.as_str(),
            ),
            (
                "execution_plan_sha256",
                record.execution_plan_sha256.as_str(),
            ),
            (
                "execution_record_sha256",
                record.execution_record_sha256.as_str(),
            ),
            (
                "source_bindings_sha256",
                record.source_bindings_sha256.as_str(),
            ),
            ("catalog_hash", record.catalog_hash.as_str()),
        ] {
            validate_sha256_hex(digest)
                .with_context(|| format!("validate completed record {name}"))?;
        }
        ensure!(
            !record.output_dir.as_os_str().is_empty(),
            "batch execution report completed record output_dir must not be empty"
        );
    }
    for failure in &report.failures {
        validate_report_record_identity(
            failure.sequence,
            &failure.operator_run_id,
            &failure.source_binding,
            &failure.category,
            &failure.symbol,
            &failure.archive_date,
        )?;
        ensure!(
            sequences.insert(failure.sequence),
            "batch execution report sequence {} appears in both records and failures or is duplicated",
            failure.sequence
        );
        ensure!(
            operator_run_ids.insert(failure.operator_run_id.as_str()),
            "batch execution report operator_run_id {:?} appears in both records and failures or is duplicated",
            failure.operator_run_id
        );
        validate_sha256_hex(&failure.selected_object_sha256)
            .context("validate failure selected_object_sha256")?;
        validate_sha256_hex(&failure.execution_record_sha256)
            .context("validate failure execution_record_sha256")?;
        ensure!(
            !failure.failure_stage.trim().is_empty(),
            "batch execution report failure_stage must not be empty"
        );
        ensure!(
            !failure.error.trim().is_empty(),
            "batch execution report failure error must not be empty"
        );
    }

    Ok(())
}

fn validate_report_record_identity(
    sequence: u64,
    operator_run_id: &str,
    source_binding: &str,
    category: &str,
    symbol: &str,
    archive_date: &str,
) -> Result<()> {
    for (name, value) in [
        ("operator_run_id", operator_run_id),
        ("source_binding", source_binding),
        ("category", category),
        ("symbol", symbol),
        ("archive_date", archive_date),
    ] {
        ensure!(
            !value.trim().is_empty(),
            "batch execution report sequence {sequence} {name} must not be empty"
        );
    }
    Ok(())
}

pub struct HttpSourceUniverseObjectFetcher {
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

impl HttpSourceUniverseObjectFetcher {
    pub fn new(fetch_timeout_seconds: Option<u64>, http_user_agent: Option<&str>) -> Result<Self> {
        let mut client_builder = reqwest::Client::builder();
        if let Some(fetch_timeout_seconds) = fetch_timeout_seconds {
            ensure!(
                fetch_timeout_seconds > 0,
                "fetch_timeout_seconds must be positive"
            );
            client_builder = client_builder.timeout(Duration::from_secs(fetch_timeout_seconds));
        }
        if let Some(http_user_agent) = http_user_agent {
            ensure!(
                !http_user_agent.trim().is_empty(),
                "http_user_agent must not be empty"
            );
            client_builder = client_builder.user_agent(http_user_agent.to_string());
        }
        let client = client_builder.build().context("build HTTP fetch client")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("create HTTP fetch runtime")?;
        Ok(Self { client, runtime })
    }
}

impl SourceUniverseObjectFetcher for HttpSourceUniverseObjectFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>> {
        let source_url = validated_http_source_url(&record.source_url)?;
        let client = self.client.clone();
        let bytes = self.runtime.block_on(async {
            let response = client
                .get(source_url)
                .send()
                .await
                .with_context(|| format!("GET {}", record.source_url))?;
            response
                .error_for_status()
                .with_context(|| format!("HTTP status for {}", record.source_url))?
                .bytes()
                .await
                .with_context(|| format!("read response body {}", record.source_url))
        })?;
        Ok(bytes.to_vec())
    }
}

/// Content-addressed object cache wrapping an inner fetcher.
///
/// The cache key is the execution-pack-pinned `selected_object_sha256`, so a
/// cached entry is only ever served after re-verifying its byte length and
/// hash against the record. A corrupt entry is deleted and refetched (explicit
/// invalidation + repair); unverified inner bytes never enter the cache.
pub struct CachingSourceUniverseObjectFetcher<F: SourceUniverseObjectFetcher> {
    inner: F,
    cache_dir: PathBuf,
}

impl<F: SourceUniverseObjectFetcher> CachingSourceUniverseObjectFetcher<F> {
    /// Wrap `inner`, caching verified objects under `cache_dir`.
    pub fn new(inner: F, cache_dir: &Path) -> Self {
        Self {
            inner,
            cache_dir: cache_dir.to_path_buf(),
        }
    }

    fn cache_entry_path(&self, record: &SourceUniverseExecutionPackRecord) -> PathBuf {
        self.cache_dir.join(&record.selected_object_sha256)
    }

    /// Read a cached entry and return it only if it still verifies. A corrupt
    /// entry is deleted so it never survives a single cache lookup.
    ///
    /// Workers can share one entry path (records are not deduplicated by sha),
    /// so "already gone" is a cache miss at every step: a read that finds no
    /// file falls through to the inner fetch, and losing the corrupt-entry
    /// delete race to a concurrent worker is a completed repair, not an error.
    fn read_verified_cache_entry(
        &self,
        record: &SourceUniverseExecutionPackRecord,
        cache_path: &Path,
    ) -> Result<Option<Vec<u8>>> {
        let cached = match fs::read(cache_path) {
            Ok(cached) => cached,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read object cache entry {}", cache_path.display()));
            }
        };
        if verify_object(record, &cached).is_ok() {
            return Ok(Some(cached));
        }
        Self::remove_corrupt_cache_entry(cache_path)?;
        Ok(None)
    }

    /// Remove a corrupt cache entry, treating an already-missing file as a
    /// completed removal (a concurrent worker repairing the same shared entry
    /// may win the delete race). Every other failure stays loud: a corrupt
    /// entry that cannot be removed must never be silently retried around.
    fn remove_corrupt_cache_entry(cache_path: &Path) -> Result<()> {
        match fs::remove_file(cache_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!("delete corrupt object cache entry {}", cache_path.display())
            }),
        }
    }

    /// Atomically write verified bytes to the content-addressed cache entry via
    /// the shared `atomic_write` primitive, so concurrent writers of the same
    /// object converge on identical bytes without ever committing a torn file.
    fn store_verified(&self, cache_path: &Path, object_bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("create object cache dir {}", self.cache_dir.display()))?;
        atomic_write(cache_path, object_bytes).with_context(|| {
            format!(
                "atomically install object cache entry {}",
                cache_path.display()
            )
        })?;
        Ok(())
    }
}

impl<F: SourceUniverseObjectFetcher> SourceUniverseObjectFetcher
    for CachingSourceUniverseObjectFetcher<F>
{
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>> {
        let cache_path = self.cache_entry_path(record);
        if let Some(cached) = self.read_verified_cache_entry(record, &cache_path)? {
            return Ok(cached);
        }
        let object_bytes = self.inner.fetch(record)?;
        verify_object(record, &object_bytes).with_context(|| {
            format!(
                "verify fetched object before caching {}",
                record.operator_run_id
            )
        })?;
        self.store_verified(&cache_path, &object_bytes)?;
        Ok(object_bytes)
    }
}

#[derive(Default)]
pub struct LocalSourceUniverseOperatorRunner;

impl SourceUniverseOperatorRunner for LocalSourceUniverseOperatorRunner {
    fn run(
        &mut self,
        _record: &SourceUniverseExecutionPackRecord,
        object_bytes: &[u8],
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
    ) -> Result<SourceUniverseBatchExecutionRunOutput> {
        let artifacts = run_from_run_spec_with_registry(
            &control_artifacts.run_spec,
            object_bytes,
            output_dir,
            &control_artifacts.source_bindings,
        )?;
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: artifacts.output.canonical_table.rows.len() as u64,
            nt_catalog_rows: artifacts.output.read_back_count as u64,
            catalog_hash: artifacts.output.projection.catalog_hash,
        })
    }
}

pub fn execute_source_universe_batch<F, R>(
    batch_id: &str,
    execution_pack_path: &Path,
    output_dir: &Path,
    record_limit: Option<u64>,
    fetcher: &mut F,
    runner: &mut R,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    execute_source_universe_batch_with_config(
        batch_id,
        execution_pack_path,
        output_dir,
        SourceUniverseBatchExecutionConfig {
            record_limit,
            ..SourceUniverseBatchExecutionConfig::default()
        },
        fetcher,
        runner,
    )
}

pub fn execute_source_universe_batch_with_config<F, R>(
    batch_id: &str,
    execution_pack_path: &Path,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let owned_plan = prepare_batch(batch_id, execution_pack_path, output_dir, &config)?;
    let plan = owned_plan.plan();
    // The borrowed-fetcher/runner entry point is inherently serial: it owns a
    // single mutable fetcher and runner, so it processes work items one at a
    // time. Slot assembly is shared with the parallel path, so the resulting
    // report is identical for the same outcomes.
    let mut slots: Vec<Option<RecordSlot>> = (0..plan.work_items.len()).map(|_| None).collect();
    for (slot_index, work_item) in plan.work_items.iter().enumerate() {
        let slot = process_work_item(
            work_item,
            &owned_plan.output_root_lease,
            &config,
            fetcher,
            runner,
        );
        let stop = !config.continue_on_error && matches!(slot, RecordSlot::Stopped(_));
        slots[slot_index] = Some(slot);
        if stop {
            // Serial stop-on-error: the first error reached is also the lowest
            // sequence, matching the parallel lowest-sequence rule.
            return Err(lowest_sequence_error(slots));
        }
    }
    assemble_report(batch_id, &owned_plan, slots, &config)
}

/// Parallel-capable entry point.
///
/// Each worker constructs its own fetcher and runner from the supplied
/// factories (the live fetcher owns a Tokio runtime and is not `Clone`), so
/// bounded record-level parallelism is possible without sharing mutable state.
/// `max_concurrent_records` of `None` or `Some(1)` runs serially.
///
/// For all-success and continue-on-error runs the assembled report is
/// byte-identical to a serial run with the same per-record outcomes (slots are
/// kept in original sequence order). Under stop-on-error the surfaced error is
/// the lowest sequence among records workers actually OBSERVED erroring; a
/// serial run could stop on an even lower sequence that a parallel worker
/// never claimed before the stop flag rose. The run still fails loud either
/// way — only which record's error is reported can differ.
pub fn execute_source_universe_batch_with_factories<F, R>(
    batch_id: &str,
    execution_pack_path: &Path,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    fetcher_factory: impl Fn() -> Result<F> + Sync,
    runner_factory: impl Fn() -> Result<R> + Sync,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let owned_plan = prepare_batch(batch_id, execution_pack_path, output_dir, &config)?;
    let plan = owned_plan.plan();
    let work_item_count = plan.work_items.len();
    if work_item_count == 0 {
        return assemble_report(batch_id, &owned_plan, Vec::new(), &config);
    }
    let worker_count = config
        .max_concurrent_records
        .and_then(|workers| usize::try_from(workers).ok())
        .unwrap_or(1)
        .max(1)
        .min(work_item_count);

    let slots: Vec<Mutex<Option<RecordSlot>>> =
        (0..work_item_count).map(|_| Mutex::new(None)).collect();
    let next_index = AtomicUsize::new(0);
    let stop_flag = AtomicBool::new(false);

    let work_items = plan.work_items.as_slice();
    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next_index = &next_index;
            let stop_flag = &stop_flag;
            let slots = &slots;
            let config = &config;
            let fetcher_factory = &fetcher_factory;
            let runner_factory = &runner_factory;
            let output_root_lease = &owned_plan.output_root_lease;
            handles.push(scope.spawn(move || -> Result<()> {
                let mut dependencies: Option<(F, R)> = None;
                loop {
                    if !config.continue_on_error && stop_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    let index = next_index.fetch_add(1, Ordering::SeqCst);
                    if index >= work_items.len() {
                        break;
                    }
                    let work_item = &work_items[index];
                    let slot = match resolve_work_item(work_item, output_root_lease, config) {
                        ResolvedBatchWorkItem::Terminal(slot) => slot,
                        ResolvedBatchWorkItem::Fresh(fresh) => {
                            if dependencies.is_none() {
                                let constructed = (|| -> Result<(F, R)> {
                                    let fetcher = fetcher_factory()
                                        .context("construct batch worker fetcher")?;
                                    let runner = runner_factory()
                                        .context("construct batch worker runner")?;
                                    Ok((fetcher, runner))
                                })();
                                match constructed {
                                    Ok(values) => dependencies = Some(values),
                                    Err(error) => {
                                        let slot = record_error_slot(
                                            fresh.record,
                                            fresh.execution_record_sha256,
                                            "construct_worker_dependencies",
                                            error,
                                            config,
                                        );
                                        if !config.continue_on_error
                                            && matches!(slot, RecordSlot::Stopped(_))
                                        {
                                            stop_flag.store(true, Ordering::SeqCst);
                                        }
                                        *slots[index].lock().expect("batch slot mutex") =
                                            Some(slot);
                                        continue;
                                    }
                                }
                            }
                            let (fetcher, runner) = dependencies
                                .as_mut()
                                .expect("worker dependencies initialized");
                            process_fresh_work_item(
                                fresh,
                                output_root_lease,
                                config,
                                fetcher,
                                runner,
                            )
                        }
                    };
                    if !config.continue_on_error && matches!(slot, RecordSlot::Stopped(_)) {
                        stop_flag.store(true, Ordering::SeqCst);
                    }
                    *slots[index].lock().expect("batch slot mutex") = Some(slot);
                }
                Ok(())
            }));
        }
        for handle in handles {
            handle.join().expect("batch worker thread panicked")?;
        }
        Ok(())
    })?;

    let slots: Vec<Option<RecordSlot>> = slots
        .into_iter()
        .map(|slot| slot.into_inner().expect("batch slot mutex"))
        .collect();
    if !config.continue_on_error
        && slots
            .iter()
            .any(|slot| matches!(slot, Some(RecordSlot::Stopped(_))))
    {
        return Err(lowest_sequence_error(slots));
    }
    assemble_report(batch_id, &owned_plan, slots, &config)
}

/// A single unit of batch work after selection and resume filtering: a control
/// artifact preflight failure collected under `continue_on_error`, a resume
/// candidate that is revalidated at consumption time, or a pack record that
/// still needs to be fetched, verified, and executed.
enum BatchWorkItem<'pack> {
    PreflightFailed {
        record: &'pack SourceUniverseExecutionPackRecord,
        error: &'pack str,
        execution_record_sha256: &'pack str,
    },
    ResumeCandidate {
        record: &'pack SourceUniverseExecutionPackRecord,
        control_artifacts: &'pack SourceUniverseVerifiedControlArtifacts,
        execution_record_sha256: &'pack str,
        prior: &'pack SourceUniverseBatchExecutionRecord,
    },
    NeedsWork {
        record: &'pack SourceUniverseExecutionPackRecord,
        control_artifacts: &'pack SourceUniverseVerifiedControlArtifacts,
        execution_record_sha256: &'pack str,
    },
}

struct FreshBatchWorkItem<'pack> {
    record: &'pack SourceUniverseExecutionPackRecord,
    control_artifacts: &'pack SourceUniverseVerifiedControlArtifacts,
    execution_record_sha256: &'pack str,
    output_claim: BatchOutputChildClaim,
}

enum ResolvedBatchWorkItem<'pack> {
    Terminal(RecordSlot),
    Fresh(FreshBatchWorkItem<'pack>),
}

/// Outcome for one work item, kept in original-sequence slot order so the
/// assembled report is independent of completion order.
enum RecordSlot {
    Completed(SourceUniverseBatchExecutionRecord),
    Carried {
        report_record: SourceUniverseBatchExecutionRecord,
        pack_record: SourceUniverseExecutionPackRecord,
    },
    Failed(SourceUniverseBatchExecutionFailureRecord),
    Stopped(StoppedRecord),
}

struct StoppedRecord {
    sequence: u64,
    error: anyhow::Error,
}

struct BatchPlan<'pack> {
    work_items: Vec<BatchWorkItem<'pack>>,
}

/// Held identity for the trusted, exclusively controlled local batch output
/// root. On Unix the open directory handle's device/inode is compared with the
/// current pathname before and after long-running work, detecting replacement.
struct BatchOutputRootLease {
    canonical_path: PathBuf,
    handle: fs::File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl BatchOutputRootLease {
    fn acquire(output_dir: &Path) -> Result<Self> {
        let canonical_path = output_dir
            .canonicalize()
            .with_context(|| format!("canonicalize batch output root {}", output_dir.display()))?;
        let handle = fs::File::open(&canonical_path)
            .with_context(|| format!("open batch output root {}", canonical_path.display()))?;
        let metadata = handle
            .metadata()
            .with_context(|| format!("stat batch output root {}", canonical_path.display()))?;
        ensure!(
            metadata.is_dir(),
            "batch output root {} must be a directory",
            canonical_path.display()
        );
        let lease = Self {
            canonical_path,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            handle,
        };
        lease.revalidate()?;
        Ok(lease)
    }

    fn revalidate(&self) -> Result<()> {
        let canonical_now = self.canonical_path.canonicalize().with_context(|| {
            format!(
                "canonicalize leased batch output root {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            canonical_now == self.canonical_path,
            "leased batch output root canonical identity changed from {} to {}",
            self.canonical_path.display(),
            canonical_now.display()
        );
        let path_metadata = fs::metadata(&self.canonical_path).with_context(|| {
            format!(
                "stat leased batch output root path {}",
                self.canonical_path.display()
            )
        })?;
        let handle_metadata = self.handle.metadata().with_context(|| {
            format!(
                "stat held batch output root handle {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            path_metadata.is_dir() && handle_metadata.is_dir(),
            "leased batch output root {} is no longer a directory",
            self.canonical_path.display()
        );
        #[cfg(unix)]
        ensure!(
            path_metadata.dev() == self.device
                && path_metadata.ino() == self.inode
                && handle_metadata.dev() == self.device
                && handle_metadata.ino() == self.inode,
            "leased batch output root {} device/inode identity changed",
            self.canonical_path.display()
        );
        Ok(())
    }

    fn revalidate_child(&self, operator_run_id: &str) -> Result<PathBuf> {
        self.revalidate()?;
        resolve_contained_output_component(&self.canonical_path, operator_run_id)
    }
}

/// Held identity for one atomically claimed selected output directory.
struct BatchOutputChildClaim {
    canonical_path: PathBuf,
    handle: fs::File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl BatchOutputChildClaim {
    fn acquire(root: &BatchOutputRootLease, operator_run_id: &str) -> Result<Self> {
        root.revalidate()?;
        let canonical_path =
            claim_contained_output_component(&root.canonical_path, operator_run_id)?;
        let handle = fs::File::open(&canonical_path).with_context(|| {
            format!("open claimed operator output {}", canonical_path.display())
        })?;
        let metadata = handle.metadata().with_context(|| {
            format!("stat claimed operator output {}", canonical_path.display())
        })?;
        ensure!(
            metadata.is_dir(),
            "claimed operator output {} must be a directory",
            canonical_path.display()
        );
        let claim = Self {
            canonical_path,
            handle,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        };
        claim.revalidate(root, operator_run_id)?;
        Ok(claim)
    }

    fn revalidate(&self, root: &BatchOutputRootLease, operator_run_id: &str) -> Result<PathBuf> {
        let canonical_now = root.revalidate_child(operator_run_id)?;
        ensure!(
            canonical_now == self.canonical_path,
            "claimed operator output canonical identity changed from {} to {}",
            self.canonical_path.display(),
            canonical_now.display()
        );
        let path_metadata = fs::metadata(&self.canonical_path).with_context(|| {
            format!(
                "stat claimed operator output path {}",
                self.canonical_path.display()
            )
        })?;
        let handle_metadata = self.handle.metadata().with_context(|| {
            format!(
                "stat held operator output handle {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            path_metadata.is_dir() && handle_metadata.is_dir(),
            "claimed operator output {} is no longer a directory",
            self.canonical_path.display()
        );
        #[cfg(unix)]
        ensure!(
            path_metadata.dev() == self.device
                && path_metadata.ino() == self.inode
                && handle_metadata.dev() == self.device
                && handle_metadata.ino() == self.inode,
            "claimed operator output {} device/inode identity changed",
            self.canonical_path.display()
        );
        Ok(canonical_now)
    }
}

fn validate_execution_pack_identity(pack: &SourceUniverseExecutionPack) -> Result<()> {
    ensure!(
        pack.schema_version == SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION,
        "execution pack schema_version mismatch: expected {}, got {}",
        SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION,
        pack.schema_version
    );
    ensure!(
        !pack.pack_id.trim().is_empty(),
        "execution pack pack_id must not be empty"
    );
    ensure!(
        !pack.universe_id.trim().is_empty(),
        "execution pack universe_id must not be empty"
    );
    ensure!(
        !pack.venue.trim().is_empty(),
        "execution pack venue must not be empty"
    );
    ensure!(
        !pack.table_family.trim().is_empty(),
        "execution pack table_family must not be empty"
    );

    for records in pack.records.windows(2) {
        ensure!(
            records[0].sequence < records[1].sequence,
            "execution pack {} sequences must be strictly increasing; {} is followed by {}",
            pack.pack_id,
            records[0].sequence,
            records[1].sequence
        );
    }

    let mut operator_run_ids = BTreeSet::new();
    for record in &pack.records {
        validate_portable_path_component("operator_run_id", &record.operator_run_id).with_context(
            || {
                format!(
                    "validate pack record {} operator_run_id {:?}",
                    record.sequence, record.operator_run_id
                )
            },
        )?;
        ensure!(
            operator_run_ids.insert(record.operator_run_id.as_str()),
            "execution pack {} has duplicate operator_run_id {:?}",
            pack.pack_id,
            record.operator_run_id
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct ExecutionRecordFingerprint<'a> {
    pack_context_sha256: &'a str,
    record: &'a SourceUniverseExecutionPackRecord,
}

/// Compute every record fingerprint from one digest of the pack's non-record
/// context. Removing `records` and hashing the remaining context once prevents
/// unbounded pack metadata (for example `artifact_refs`) from being
/// canonicalized again for every record.
fn execution_record_digests(pack: &SourceUniverseExecutionPack) -> Result<BTreeMap<u64, String>> {
    let mut pack_context =
        serde_json::to_value(pack).context("serialize execution pack context")?;
    let pack_object = pack_context
        .as_object_mut()
        .context("serialized execution pack must be a JSON object")?;
    ensure!(
        pack_object.remove("records").is_some(),
        "serialized execution pack context is missing records"
    );
    let pack_context_sha256 = crate::reference_artifact::canonical_json_sha256(&pack_context)
        .context("hash execution-pack non-record context")?;

    let mut digests = BTreeMap::new();
    for record in &pack.records {
        let digest =
            crate::reference_artifact::canonical_json_sha256(&ExecutionRecordFingerprint {
                pack_context_sha256: &pack_context_sha256,
                record,
            })
            .context("hash execution-pack record and pack context")?;
        ensure!(
            digests.insert(record.sequence, digest).is_none(),
            "execution pack {} has duplicate sequence {}",
            pack.pack_id,
            record.sequence
        );
    }
    Ok(digests)
}

fn validate_pack_record_control_alignment(
    pack: &SourceUniverseExecutionPack,
    record: &SourceUniverseExecutionPackRecord,
    controls: &ValidatedBackfillExecutionControls,
) -> Result<()> {
    let run_spec = &controls.run_spec;
    let tranche = &controls.accepted_tranche;
    let plan = &controls.execution_plan;
    let object = plan
        .objects
        .first()
        .context("validated execution plan is missing its accepted object")?;
    let identity = run_spec
        .identity
        .single()
        .context("source-universe execution pack requires one instrument identity")?;

    ensure!(
        pack.venue == run_spec.source_proof.venue,
        "execution pack venue {:?} does not match run_spec source-proof venue {:?}",
        pack.venue,
        run_spec.source_proof.venue
    );
    ensure!(
        pack.universe_id == run_spec.source_proof.instrument_universe_id,
        "execution pack universe_id {:?} does not match run_spec source-proof instrument_universe_id {:?}",
        pack.universe_id,
        run_spec.source_proof.instrument_universe_id
    );
    ensure!(
        pack.table_family == run_spec.source_proof.table_family,
        "execution pack table_family {:?} does not match run_spec source-proof table_family {:?}",
        pack.table_family,
        run_spec.source_proof.table_family
    );
    ensure!(
        record.operator_run_id == run_spec.manifest.run_id
            && record.operator_run_id == plan.operator_run_id,
        "pack record operator_run_id does not match retained controls"
    );
    ensure!(
        record.source_binding == run_spec.source_proof.source_binding
            && record.source_binding == run_spec.manifest.venue_binding_key
            && record.source_binding == tranche.source_binding
            && record.source_binding == plan.source_binding,
        "pack record source_binding does not match retained controls"
    );
    ensure!(
        record.category == run_spec.source_proof.product_category,
        "pack record category does not match retained run_spec source proof"
    );
    ensure!(
        record.symbol == identity.venue_symbol,
        "pack record symbol does not match retained run_spec instrument identity"
    );
    ensure!(
        record.archive_date == run_spec.accepted_object.archive_date
            && record.archive_date == object.archive_date,
        "pack record archive_date does not match retained controls"
    );
    ensure!(
        record.source_uri == run_spec.accepted_object.s3_uri
            && record.source_uri == run_spec.source_proof.raw_sample_uri
            && record.source_uri == object.s3_uri,
        "pack record source_uri does not match retained controls"
    );
    ensure!(
        record.source_url == run_spec.accepted_object.source_url
            && record.source_url == object.source_url,
        "pack record source_url does not match retained controls"
    );
    ensure!(
        record.selected_object_sha256 == run_spec.accepted_object.sha256
            && record.selected_object_sha256 == run_spec.source_proof.raw_sample_hash
            && record.selected_object_sha256 == object.sha256,
        "pack record selected_object_sha256 does not match retained controls"
    );
    ensure!(
        record.selected_object_bytes == run_spec.accepted_object.bytes
            && record.selected_object_bytes == object.bytes,
        "pack record selected_object_bytes does not match retained controls"
    );
    ensure!(
        record.source_proof_id == run_spec.source_proof.source_proof_id
            && record.source_proof_id == plan.source_proof_id,
        "pack record source_proof_id does not match retained controls"
    );
    ensure!(
        record.source_proof_version == run_spec.source_proof.source_proof_version
            && record.source_proof_version == plan.source_proof_version,
        "pack record source_proof_version does not match retained controls"
    );
    ensure!(
        record.accepted_tranche_id == tranche.tranche_id
            && record.accepted_tranche_id == plan.accepted_tranche_id,
        "pack record accepted_tranche_id does not match retained controls"
    );
    ensure!(
        record.output_prefix == run_spec.manifest.output_prefix
            && record.output_prefix == plan.output_prefix,
        "pack record output_prefix does not match retained controls"
    );
    ensure!(
        record.run_spec_sha256 == controls.run_spec_sha256,
        "pack record run_spec_sha256 does not match retained bytes"
    );
    ensure!(
        record.accepted_tranche_sha256 == controls.accepted_tranche_sha256,
        "pack record accepted_tranche_sha256 does not match retained bytes"
    );
    ensure!(
        record.execution_plan_sha256 == controls.execution_plan_sha256,
        "pack record execution_plan_sha256 does not match retained bytes"
    );
    Ok(())
}

fn prepare_batch(
    batch_id: &str,
    execution_pack_path: &Path,
    output_dir: &Path,
    config: &SourceUniverseBatchExecutionConfig,
) -> Result<OwnedBatchPlan> {
    ensure!(!batch_id.trim().is_empty(), "batch_id must not be empty");
    if let Some(limit) = config.record_limit {
        ensure!(limit > 0, "record_limit must be positive when set");
    }
    if let Some(workers) = config.max_concurrent_records {
        ensure!(
            workers > 0,
            "max_concurrent_records must be positive when set"
        );
    }

    let pack_bytes = fs::read(execution_pack_path)
        .with_context(|| format!("read execution pack {}", execution_pack_path.display()))?;
    let pack: SourceUniverseExecutionPack = serde_json::from_slice(&pack_bytes)
        .with_context(|| format!("parse execution pack {}", execution_pack_path.display()))?;
    validate_execution_pack_identity(&pack)?;
    let execution_record_sha256s = execution_record_digests(&pack)?;
    ensure!(
        matches!(
            pack.status,
            SourceUniverseExecutionPackStatus::Ready
                | SourceUniverseExecutionPackStatus::PartiallyReady
        ),
        "execution pack {} is not executable",
        pack.pack_id
    );

    let pack_base_dir = execution_pack_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create batch output dir {}", output_dir.display()))?;
    let output_root_lease = BatchOutputRootLease::acquire(output_dir)?;

    // Resuming into the dir holding the resume report itself would only fail
    // later, at the clean-write report guard (which refuses to overwrite the
    // prior report). Reject the contract violation up front, before any fetch.
    if let Some(resume_report) = config.resume_report.as_deref() {
        let output_report_path = output_dir.join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE);
        let same_target = match (
            resume_report.canonicalize(),
            output_report_path.canonicalize(),
        ) {
            (Ok(resume), Ok(output)) => resume == output,
            _ => resume_report == output_report_path.as_path(),
        };
        ensure!(
            !same_target,
            "resume requires a fresh output dir: resume report {} is the report path of output dir {}; \
             the clean-write report guard preserves the prior report as evidence, so a resumed run \
             must write into a new output dir",
            resume_report.display(),
            output_dir.display()
        );
    }

    // Validate every record's sha256 field before any fetch or cache activity.
    // A pack record whose selected_object_sha256 is not exactly 64 lowercase-hex
    // characters would be used verbatim as a filesystem path component by the
    // caching fetcher, allowing path traversal. Fail loud here so the class of
    // invalid pack is caught at the single consume boundary.
    for record in &pack.records {
        validate_sha256_hex(&record.selected_object_sha256).with_context(|| {
            format!(
                "pack record {} (operator_run_id {}) has an invalid selected_object_sha256: \
                 expected 64 lowercase-hex chars, got {} chars",
                record.sequence,
                record.operator_run_id,
                record.selected_object_sha256.len(),
            )
        })?;
    }

    let record_limit = config
        .record_limit
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);

    // Bind the selected records to the exact control bytes pinned by the pack
    // before resume processing, cache access, source fetches, or worker
    // construction. Selection is intentional: committed campaign packs retain
    // only a bounded golden subset of generated run artifacts, and record_limit
    // is the operator's explicit execution window. Each unique resolved file is
    // read once, while every selected record's expected digest is still checked.
    let mut verified_artifact_cache = BTreeMap::new();
    let mut verified_registry_cache = BTreeMap::new();
    let mut verified_control_artifacts = BTreeMap::new();
    let mut control_artifact_failures = BTreeMap::new();
    for record in pack
        .records
        .iter()
        .filter(|record| {
            config
                .start_sequence
                .is_none_or(|start_sequence| record.sequence >= start_sequence)
        })
        .take(record_limit)
    {
        ensure!(
            !verified_control_artifacts.contains_key(&record.sequence)
                && !control_artifact_failures.contains_key(&record.sequence),
            "execution pack {} has duplicate selected sequence {}",
            pack.pack_id,
            record.sequence,
        );
        let preflight = (|| {
            let prospective_output_dir = output_root_lease
                .canonical_path
                .join(&record.operator_run_id);
            verify_pack_control_artifacts(
                &pack,
                &pack_base_dir,
                record,
                &prospective_output_dir,
                &mut verified_artifact_cache,
                &mut verified_registry_cache,
            )
        })();
        match preflight {
            Ok(verified) => {
                verified_control_artifacts.insert(record.sequence, verified);
            }
            Err(error) if config.continue_on_error => {
                control_artifact_failures.insert(record.sequence, format!("{error:#}"));
            }
            Err(error) => return Err(error),
        }
    }

    let resume_records = load_resume_records(config.resume_report.as_deref(), &pack)?;

    Ok(OwnedBatchPlan {
        pack,
        execution_record_sha256s,
        verified_control_artifacts,
        control_artifact_failures,
        resume_records,
        start_sequence: config.start_sequence,
        record_limit,
        output_root_lease,
    })
}

#[derive(Clone)]
struct VerifiedArtifactContent {
    sha256: String,
    bytes: Arc<[u8]>,
}

#[derive(Clone)]
struct VerifiedSourceBindingRegistry {
    sha256: String,
    bytes: Arc<[u8]>,
    registry: Arc<SourceBindingRegistry>,
}

struct ControlArtifactPin<'record> {
    role: &'static str,
    sha256_field: &'static str,
    declared_path: &'record Path,
    expected_sha256: &'record str,
}

fn verify_pack_control_artifacts(
    pack: &SourceUniverseExecutionPack,
    pack_base_dir: &Path,
    record: &SourceUniverseExecutionPackRecord,
    record_output_dir: &Path,
    verified_artifact_cache: &mut BTreeMap<PathBuf, VerifiedArtifactContent>,
    verified_registry_cache: &mut BTreeMap<PathBuf, VerifiedSourceBindingRegistry>,
) -> Result<SourceUniverseVerifiedControlArtifacts> {
    let (run_spec_path, run_spec_bytes) = verify_pack_control_artifact(
        pack_base_dir,
        record,
        ControlArtifactPin {
            role: "run_spec",
            sha256_field: "run_spec_sha256",
            declared_path: &record.run_spec_path,
            expected_sha256: &record.run_spec_sha256,
        },
        verified_artifact_cache,
    )?;
    let (accepted_tranche_path, accepted_tranche_bytes) = verify_pack_control_artifact(
        pack_base_dir,
        record,
        ControlArtifactPin {
            role: "accepted_tranche",
            sha256_field: "accepted_tranche_sha256",
            declared_path: &record.accepted_tranche_path,
            expected_sha256: &record.accepted_tranche_sha256,
        },
        verified_artifact_cache,
    )?;
    let (execution_plan_path, execution_plan_bytes) = verify_pack_control_artifact(
        pack_base_dir,
        record,
        ControlArtifactPin {
            role: "execution_plan",
            sha256_field: "execution_plan_sha256",
            declared_path: &record.execution_plan_path,
            expected_sha256: &record.execution_plan_sha256,
        },
        verified_artifact_cache,
    )?;

    let validated = validate_backfill_execution_control_bytes(
        run_spec_bytes.as_ref(),
        accepted_tranche_bytes.as_ref(),
        execution_plan_bytes.as_ref(),
    )
    .with_context(|| {
        format!(
            "validate typed control triple for pack record {} ({})",
            record.sequence, record.operator_run_id
        )
    })?;
    validate_pack_record_control_alignment(pack, record, &validated)?;

    let source_bindings_path =
        resolve_pack_control_path(pack_base_dir, &validated.run_spec.source_bindings_path)
            .with_context(|| {
                format!(
                    "resolve source_bindings_path {} for pack record {} ({})",
                    validated.run_spec.source_bindings_path.display(),
                    record.sequence,
                    record.operator_run_id
                )
            })?;
    let verified_registry =
        if let Some(verified) = verified_registry_cache.get(&source_bindings_path) {
            verified.clone()
        } else {
            let bytes = fs::read(&source_bindings_path).with_context(|| {
                format!(
                    "read source-bindings registry snapshot {} for pack record {} ({})",
                    source_bindings_path.display(),
                    record.sequence,
                    record.operator_run_id
                )
            })?;
            let text = std::str::from_utf8(&bytes).with_context(|| {
                format!(
                    "decode source-bindings registry snapshot {} as UTF-8",
                    source_bindings_path.display()
                )
            })?;
            let registry = SourceBindingRegistry::from_toml_str(text).with_context(|| {
                format!(
                    "parse source-bindings registry snapshot {}",
                    source_bindings_path.display()
                )
            })?;
            let verified = VerifiedSourceBindingRegistry {
                sha256: hex::encode(Sha256::digest(&bytes)),
                bytes: Arc::from(bytes),
                registry: Arc::new(registry),
            };
            verified_registry_cache.insert(source_bindings_path.clone(), verified.clone());
            verified
        };

    validate_run_spec_manifest_for_object_hash_with_registry(
        &validated.run_spec,
        record_output_dir,
        &record.selected_object_sha256,
        &verified_registry.registry,
    )
    .with_context(|| {
        format!(
            "validate source proof and run manifest against retained registry {} for pack record {} ({})",
            source_bindings_path.display(),
            record.sequence,
            record.operator_run_id
        )
    })?;

    Ok(SourceUniverseVerifiedControlArtifacts {
        run_spec_path,
        run_spec_bytes,
        run_spec: Arc::new(validated.run_spec),
        accepted_tranche_path,
        accepted_tranche_bytes,
        accepted_tranche: Arc::new(validated.accepted_tranche),
        execution_plan_path,
        execution_plan_bytes,
        execution_plan: Arc::new(validated.execution_plan),
        source_bindings_path,
        source_bindings_bytes: verified_registry.bytes,
        source_bindings_sha256: verified_registry.sha256,
        source_bindings: verified_registry.registry,
    })
}

fn verify_pack_control_artifact(
    pack_base_dir: &Path,
    record: &SourceUniverseExecutionPackRecord,
    pin: ControlArtifactPin<'_>,
    verified_artifact_cache: &mut BTreeMap<PathBuf, VerifiedArtifactContent>,
) -> Result<(PathBuf, Arc<[u8]>)> {
    validate_sha256_hex(pin.expected_sha256).with_context(|| {
        format!(
            "pack record {} (operator_run_id {}) has an invalid {} for pinned artifact {} \
             at {}: expected 64 lowercase-hex chars, got {}",
            record.sequence,
            record.operator_run_id,
            pin.sha256_field,
            pin.role,
            pin.declared_path.display(),
            pin.expected_sha256,
        )
    })?;

    let resolved_path =
        resolve_pack_control_path(pack_base_dir, pin.declared_path).with_context(|| {
            format!(
                "resolve pack record {} ({}) pinned artifact {} declared at {}",
                record.sequence,
                record.operator_run_id,
                pin.role,
                pin.declared_path.display()
            )
        })?;
    let verified = if let Some(verified) = verified_artifact_cache.get(&resolved_path) {
        verified.clone()
    } else {
        let bytes = fs::read(&resolved_path).with_context(|| {
            format!(
                "read pack record {} (operator_run_id {}) pinned artifact {} at {} \
                 (declared {}) with expected SHA-256 {}",
                record.sequence,
                record.operator_run_id,
                pin.role,
                resolved_path.display(),
                pin.declared_path.display(),
                pin.expected_sha256,
            )
        })?;
        let verified = VerifiedArtifactContent {
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: Arc::<[u8]>::from(bytes),
        };
        verified_artifact_cache.insert(resolved_path.clone(), verified.clone());
        verified
    };

    ensure!(
        verified.sha256 == pin.expected_sha256,
        "pack record {} (operator_run_id {}) pinned artifact {} SHA-256 mismatch at {} \
         (declared {}): expected {}, got {}",
        record.sequence,
        record.operator_run_id,
        pin.role,
        resolved_path.display(),
        pin.declared_path.display(),
        pin.expected_sha256,
        verified.sha256,
    );

    Ok((resolved_path, verified.bytes))
}

/// Owns the parsed pack and resume map so the `'pack`-lifetime [`BatchPlan`]
/// work items can borrow from it without an extra clone of every pack record.
struct OwnedBatchPlan {
    pack: SourceUniverseExecutionPack,
    execution_record_sha256s: BTreeMap<u64, String>,
    verified_control_artifacts: BTreeMap<u64, SourceUniverseVerifiedControlArtifacts>,
    control_artifact_failures: BTreeMap<u64, String>,
    resume_records: BTreeMap<u64, SourceUniverseBatchExecutionRecord>,
    start_sequence: Option<u64>,
    record_limit: usize,
    output_root_lease: BatchOutputRootLease,
}

impl OwnedBatchPlan {
    fn plan(&self) -> BatchPlan<'_> {
        let work_items = self
            .pack
            .records
            .iter()
            .filter(|record| {
                self.start_sequence
                    .is_none_or(|start_sequence| record.sequence >= start_sequence)
            })
            .take(self.record_limit)
            .map(|record| {
                let execution_record_sha256 = self
                    .execution_record_sha256s
                    .get(&record.sequence)
                    .expect("execution record digest was precomputed");
                if let Some(error) = self.control_artifact_failures.get(&record.sequence) {
                    return BatchWorkItem::PreflightFailed {
                        record,
                        error,
                        execution_record_sha256,
                    };
                }

                let control_artifacts = self
                    .verified_control_artifacts
                    .get(&record.sequence)
                    .expect("selected record control artifacts were verified");
                match self.resume_records.get(&record.sequence) {
                    Some(prior) => BatchWorkItem::ResumeCandidate {
                        record,
                        control_artifacts,
                        execution_record_sha256,
                        prior,
                    },
                    None => BatchWorkItem::NeedsWork {
                        record,
                        control_artifacts,
                        execution_record_sha256,
                    },
                }
            })
            .collect();
        BatchPlan { work_items }
    }
}

fn carried_record_inputs_match_pack(
    prior: &SourceUniverseBatchExecutionRecord,
    record: &SourceUniverseExecutionPackRecord,
    control_artifacts: &SourceUniverseVerifiedControlArtifacts,
    execution_record_sha256: &str,
) -> bool {
    prior.execution_record_sha256 == execution_record_sha256
        && prior.source_bindings_sha256 == control_artifacts.source_bindings_sha256
        && prior.selected_object_sha256 == record.selected_object_sha256
        && prior.run_spec_sha256 == record.run_spec_sha256
        && prior.accepted_tranche_sha256 == record.accepted_tranche_sha256
        && prior.execution_plan_sha256 == record.execution_plan_sha256
}

fn reconstructed_carried_record(
    prior: &SourceUniverseBatchExecutionRecord,
    record: &SourceUniverseExecutionPackRecord,
    control_artifacts: &SourceUniverseVerifiedControlArtifacts,
    execution_record_sha256: &str,
) -> SourceUniverseBatchExecutionRecord {
    SourceUniverseBatchExecutionRecord {
        sequence: record.sequence,
        operator_run_id: record.operator_run_id.clone(),
        source_binding: record.source_binding.clone(),
        category: record.category.clone(),
        symbol: record.symbol.clone(),
        archive_date: record.archive_date.clone(),
        selected_object_sha256: record.selected_object_sha256.clone(),
        run_spec_sha256: record.run_spec_sha256.clone(),
        accepted_tranche_sha256: record.accepted_tranche_sha256.clone(),
        execution_plan_sha256: record.execution_plan_sha256.clone(),
        execution_record_sha256: execution_record_sha256.to_string(),
        source_bindings_sha256: control_artifacts.source_bindings_sha256.clone(),
        selected_object_bytes: record.selected_object_bytes,
        canonical_rows: prior.canonical_rows,
        nt_catalog_rows: prior.nt_catalog_rows,
        catalog_hash: prior.catalog_hash.clone(),
        output_dir: prior.output_dir.clone(),
    }
}

/// Re-prove a carried record's prior OUTPUT before it is reused on resume.
///
/// The carried record stores its prior run's `output_dir`; the NautilusTrader
/// catalog sits beneath it at [`CATALOG_DIR`]. This re-opens that catalog and
/// re-hashes it through the same [`logical_catalog_hash`] the operator's
/// completed-output reuse path runs, asserting the recomputed hash equals the
/// carried `catalog_hash`. A missing, unreadable, or drifted catalog returns
/// `false` so the caller downgrades the record to fresh work instead of
/// trusting the stale resume marker — output corruption can never survive a
/// resume.
fn carried_output_still_verifies(prior: &SourceUniverseBatchExecutionRecord) -> bool {
    let catalog_root = prior.output_dir.join(CATALOG_DIR);
    // A missing prior output is "not verified" (re-execute), not a panic: NT's
    // catalog open expects an existing path and panics otherwise, so guard the
    // deleted/missing case here before re-hashing. A present-but-corrupt catalog
    // still surfaces as Err below.
    if !catalog_root.is_dir() {
        return false;
    }
    match logical_catalog_hash(&catalog_root) {
        Ok(actual_catalog_hash) => actual_catalog_hash == prior.catalog_hash,
        Err(_) => false,
    }
}

fn load_resume_records(
    resume_report: Option<&Path>,
    pack: &SourceUniverseExecutionPack,
) -> Result<BTreeMap<u64, SourceUniverseBatchExecutionRecord>> {
    let Some(resume_report) = resume_report else {
        return Ok(BTreeMap::new());
    };
    let prior_bytes = fs::read(resume_report)
        .with_context(|| format!("read resume report {}", resume_report.display()))?;
    let prior: SourceUniverseBatchExecutionReport = serde_json::from_slice(&prior_bytes)
        .with_context(|| format!("parse resume report {}", resume_report.display()))?;
    validate_source_universe_batch_execution_report(&prior)
        .with_context(|| format!("validate resume report {}", resume_report.display()))?;
    for (name, expected, actual) in [
        ("pack_id", pack.pack_id.as_str(), prior.pack_id.as_str()),
        (
            "universe_id",
            pack.universe_id.as_str(),
            prior.universe_id.as_str(),
        ),
        ("venue", pack.venue.as_str(), prior.venue.as_str()),
    ] {
        ensure!(
            expected == actual,
            "resume report {} {name} mismatch: expected {:?}, got {:?}",
            resume_report.display(),
            expected,
            actual
        );
    }
    // Only prior successful records (entries of `records`) carry forward; prior
    // failures are reprocessed.
    let mut resume_records = BTreeMap::new();
    for record in prior.records {
        resume_records.insert(record.sequence, record);
    }
    Ok(resume_records)
}

fn process_work_item<F, R>(
    work_item: &BatchWorkItem<'_>,
    output_root_lease: &BatchOutputRootLease,
    config: &SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
) -> RecordSlot
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    match resolve_work_item(work_item, output_root_lease, config) {
        ResolvedBatchWorkItem::Terminal(slot) => slot,
        ResolvedBatchWorkItem::Fresh(fresh) => {
            process_fresh_work_item(fresh, output_root_lease, config, fetcher, runner)
        }
    }
}

fn resolve_work_item<'pack>(
    work_item: &'pack BatchWorkItem<'pack>,
    output_root_lease: &BatchOutputRootLease,
    config: &SourceUniverseBatchExecutionConfig,
) -> ResolvedBatchWorkItem<'pack> {
    let (record, control_artifacts, execution_record_sha256) = match work_item {
        BatchWorkItem::PreflightFailed {
            record,
            error,
            execution_record_sha256,
        } => {
            return ResolvedBatchWorkItem::Terminal(record_error_slot(
                record,
                execution_record_sha256,
                "verify_control_artifacts",
                anyhow::anyhow!(error.to_string()),
                config,
            ));
        }
        BatchWorkItem::ResumeCandidate {
            record,
            control_artifacts,
            execution_record_sha256,
            prior,
        } => {
            if carried_record_inputs_match_pack(
                prior,
                record,
                control_artifacts,
                execution_record_sha256,
            ) && carried_output_still_verifies(prior)
            {
                return ResolvedBatchWorkItem::Terminal(RecordSlot::Carried {
                    report_record: reconstructed_carried_record(
                        prior,
                        record,
                        control_artifacts,
                        execution_record_sha256,
                    ),
                    pack_record: (*record).clone(),
                });
            }
            (*record, *control_artifacts, *execution_record_sha256)
        }
        BatchWorkItem::NeedsWork {
            record,
            control_artifacts,
            execution_record_sha256,
        } => (*record, *control_artifacts, *execution_record_sha256),
    };

    match BatchOutputChildClaim::acquire(output_root_lease, &record.operator_run_id).with_context(
        || {
            format!(
                "claim fresh output for pack record {} ({})",
                record.sequence, record.operator_run_id
            )
        },
    ) {
        Ok(output_claim) => ResolvedBatchWorkItem::Fresh(FreshBatchWorkItem {
            record,
            control_artifacts,
            execution_record_sha256,
            output_claim,
        }),
        Err(error) => ResolvedBatchWorkItem::Terminal(record_error_slot(
            record,
            execution_record_sha256,
            "validate_output",
            error,
            config,
        )),
    }
}

fn process_fresh_work_item<F, R>(
    fresh: FreshBatchWorkItem<'_>,
    output_root_lease: &BatchOutputRootLease,
    config: &SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
) -> RecordSlot
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let FreshBatchWorkItem {
        record,
        control_artifacts,
        execution_record_sha256,
        output_claim,
    } = fresh;

    if let Err(error) = output_claim
        .revalidate(output_root_lease, &record.operator_run_id)
        .with_context(|| {
            format!(
                "revalidate output containment before fetch for {}",
                record.operator_run_id
            )
        })
    {
        return record_error_slot(
            record,
            execution_record_sha256,
            "validate_output",
            error,
            config,
        );
    }

    let object_bytes = match fetcher
        .fetch(record)
        .with_context(|| format!("fetch source object for {}", record.operator_run_id))
    {
        Ok(object_bytes) => object_bytes,
        Err(error) => {
            return record_error_slot(record, execution_record_sha256, "fetch", error, config);
        }
    };
    if let Err(error) = verify_object(record, &object_bytes)
        .with_context(|| format!("verify source object for {}", record.operator_run_id))
    {
        return record_error_slot(
            record,
            execution_record_sha256,
            "verify_object",
            error,
            config,
        );
    }

    // Threat boundary: this detects replacement during the potentially long
    // fetch, but it is not an openat-style capability. `SourceUniverseOperatorRunner::run`
    // and the NT catalog APIs accept `&Path` and reopen descendants by pathname,
    // so an actor able to mutate this trusted workspace after this check cannot
    // be excluded atomically without changing the operator/NT storage API. The
    // preflight atomic child claim, held root/child identities, and this
    // post-fetch check reject every drift observable at the available boundary
    // without pretending otherwise.
    let record_output_dir = match output_claim
        .revalidate(output_root_lease, &record.operator_run_id)
        .with_context(|| {
            format!(
                "revalidate output root and child after fetch for {}",
                record.operator_run_id
            )
        }) {
        Ok(record_output_dir) => record_output_dir,
        Err(error) => {
            return record_error_slot(
                record,
                execution_record_sha256,
                "validate_output",
                error,
                config,
            );
        }
    };

    let run_result = runner
        .run(record, &object_bytes, control_artifacts, &record_output_dir)
        .with_context(|| format!("run operator {}", record.operator_run_id));
    let record_output_dir = match output_claim
        .revalidate(output_root_lease, &record.operator_run_id)
        .with_context(|| {
            format!(
                "revalidate output root and child after runner for {}",
                record.operator_run_id
            )
        }) {
        Ok(record_output_dir) => record_output_dir,
        Err(error) => {
            return record_error_slot(
                record,
                execution_record_sha256,
                "validate_output",
                error,
                config,
            );
        }
    };
    let run_output = match run_result {
        Ok(run_output) => run_output,
        Err(error) => {
            return record_error_slot(
                record,
                execution_record_sha256,
                "run_operator",
                error,
                config,
            );
        }
    };

    RecordSlot::Completed(SourceUniverseBatchExecutionRecord {
        sequence: record.sequence,
        operator_run_id: record.operator_run_id.clone(),
        source_binding: record.source_binding.clone(),
        category: record.category.clone(),
        symbol: record.symbol.clone(),
        archive_date: record.archive_date.clone(),
        selected_object_sha256: record.selected_object_sha256.clone(),
        run_spec_sha256: record.run_spec_sha256.clone(),
        accepted_tranche_sha256: record.accepted_tranche_sha256.clone(),
        execution_plan_sha256: record.execution_plan_sha256.clone(),
        execution_record_sha256: execution_record_sha256.to_string(),
        source_bindings_sha256: control_artifacts.source_bindings_sha256.clone(),
        selected_object_bytes: record.selected_object_bytes,
        canonical_rows: run_output.canonical_rows,
        nt_catalog_rows: run_output.nt_catalog_rows,
        catalog_hash: run_output.catalog_hash,
        output_dir: record_output_dir,
    })
}

fn record_error_slot(
    record: &SourceUniverseExecutionPackRecord,
    execution_record_sha256: &str,
    failure_stage: &str,
    error: anyhow::Error,
    config: &SourceUniverseBatchExecutionConfig,
) -> RecordSlot {
    if config.continue_on_error {
        RecordSlot::Failed(failure_record(
            record,
            execution_record_sha256,
            failure_stage,
            &error,
        ))
    } else {
        RecordSlot::Stopped(StoppedRecord {
            sequence: record.sequence,
            error,
        })
    }
}

fn lowest_sequence_error(slots: Vec<Option<RecordSlot>>) -> anyhow::Error {
    slots
        .into_iter()
        .flatten()
        .filter_map(|slot| match slot {
            RecordSlot::Stopped(stopped) => Some(stopped),
            _ => None,
        })
        .min_by_key(|stopped| stopped.sequence)
        .map(|stopped| stopped.error)
        .unwrap_or_else(|| {
            anyhow::anyhow!("stop-on-error requested but no errored record was recorded")
        })
}

fn assemble_report(
    batch_id: &str,
    owned_plan: &OwnedBatchPlan,
    slots: Vec<Option<RecordSlot>>,
    config: &SourceUniverseBatchExecutionConfig,
) -> Result<SourceUniverseBatchExecutionReport> {
    let mut records = Vec::new();
    let mut failures = Vec::new();
    let mut total_canonical_rows = 0_u64;
    let mut total_nt_catalog_rows = 0_u64;

    for slot in slots {
        match slot {
            Some(RecordSlot::Completed(record)) => {
                total_canonical_rows = total_canonical_rows
                    .checked_add(record.canonical_rows)
                    .context("batch total_canonical_rows overflow")?;
                total_nt_catalog_rows = total_nt_catalog_rows
                    .checked_add(record.nt_catalog_rows)
                    .context("batch total_nt_catalog_rows overflow")?;
                records.push(record);
            }
            Some(RecordSlot::Carried {
                report_record,
                pack_record,
            }) => {
                if carried_output_still_verifies(&report_record) {
                    total_canonical_rows = total_canonical_rows
                        .checked_add(report_record.canonical_rows)
                        .context("batch total_canonical_rows overflow")?;
                    total_nt_catalog_rows = total_nt_catalog_rows
                        .checked_add(report_record.nt_catalog_rows)
                        .context("batch total_nt_catalog_rows overflow")?;
                    records.push(report_record);
                } else {
                    let error = anyhow::anyhow!(
                        "carried catalog for {} drifted before final report assembly",
                        pack_record.operator_run_id
                    );
                    if config.continue_on_error {
                        failures.push(failure_record(
                            &pack_record,
                            &report_record.execution_record_sha256,
                            "verify_carried_catalog",
                            &error,
                        ));
                    } else {
                        return Err(error);
                    }
                }
            }
            Some(RecordSlot::Failed(failure)) => failures.push(failure),
            Some(RecordSlot::Stopped(stopped)) => return Err(stopped.error),
            None => {
                // A `None` slot only happens under stop-on-error when a worker
                // stopped before claiming this index. The caller returns the
                // lowest-sequence error before reaching assembly, so any
                // surviving `None` here is a logic error worth failing loud on.
                ensure!(
                    config.continue_on_error,
                    "batch work item left unprocessed without an error"
                );
            }
        }
    }

    let status = if failures.is_empty() {
        SourceUniverseBatchExecutionReportStatus::Completed
    } else if records.is_empty() {
        SourceUniverseBatchExecutionReportStatus::Failed
    } else {
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    };
    let completed_record_count =
        u64::try_from(records.len()).context("completed record count exceeds u64")?;
    let failed_record_count =
        u64::try_from(failures.len()).context("failed record count exceeds u64")?;
    let selected_record_count = completed_record_count
        .checked_add(failed_record_count)
        .context("selected record count overflow")?;

    let pack = &owned_plan.pack;
    let report = SourceUniverseBatchExecutionReport {
        schema_version: SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION.to_string(),
        batch_id: batch_id.to_string(),
        status,
        pack_id: pack.pack_id.clone(),
        universe_id: pack.universe_id.clone(),
        venue: pack.venue.clone(),
        selected_record_count,
        completed_record_count,
        failed_record_count,
        total_canonical_rows,
        total_nt_catalog_rows,
        records,
        failures,
    };
    validate_source_universe_batch_execution_report(&report)
        .context("validate assembled batch execution report")?;
    Ok(report)
}

pub fn write_source_universe_batch_execution_report(
    output_dir: &Path,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
    validate_source_universe_batch_execution_report(report)
        .context("validate batch execution report before write")?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create batch execution report dir {}", output_dir.display()))?;
    let path = output_dir.join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| format!("write batch execution report {}", path.display()))?;
    Ok(SourceUniverseBatchExecutionReportArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        completed_record_count: report.completed_record_count,
    })
}

/// Validate that `value` is exactly 64 lowercase ASCII hex characters.
///
/// Stricter than the case-insensitive `artifact_index` sha256 check: this
/// requires lowercase hex, so a malformed pack digest can never introduce
/// uppercase — or any non-hex — bytes into a path component. Call at the
/// pack-consume boundary before `value` is used as a filesystem path component.
fn validate_sha256_hex(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')),
        "not a 64-char lowercase-hex sha256 digest"
    );
    Ok(())
}

fn verify_object(record: &SourceUniverseExecutionPackRecord, object_bytes: &[u8]) -> Result<()> {
    ensure!(
        object_bytes.len() as u64 == record.selected_object_bytes,
        "object byte length for {} does not match execution pack: expected {}, got {}",
        record.operator_run_id,
        record.selected_object_bytes,
        object_bytes.len()
    );
    let actual_sha256 = hex::encode(Sha256::digest(object_bytes));
    ensure!(
        actual_sha256 == record.selected_object_sha256,
        "object sha256 for {} does not match execution pack: expected {}, got {}",
        record.operator_run_id,
        record.selected_object_sha256,
        actual_sha256
    );
    Ok(())
}

fn failure_record(
    record: &SourceUniverseExecutionPackRecord,
    execution_record_sha256: &str,
    failure_stage: &str,
    error: &anyhow::Error,
) -> SourceUniverseBatchExecutionFailureRecord {
    SourceUniverseBatchExecutionFailureRecord {
        sequence: record.sequence,
        operator_run_id: record.operator_run_id.clone(),
        source_binding: record.source_binding.clone(),
        category: record.category.clone(),
        symbol: record.symbol.clone(),
        archive_date: record.archive_date.clone(),
        selected_object_sha256: record.selected_object_sha256.clone(),
        execution_record_sha256: execution_record_sha256.to_string(),
        selected_object_bytes: record.selected_object_bytes,
        failure_stage: failure_stage.to_string(),
        error: format!("{error:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_universe_execution_pack::SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION;

    /// Inner fetcher double for cache-repair unit tests; must never be called.
    struct PanicFetcher;

    impl SourceUniverseObjectFetcher for PanicFetcher {
        fn fetch(&mut self, _record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>> {
            panic!("inner fetcher must not be called")
        }
    }

    type TestCachingFetcher = CachingSourceUniverseObjectFetcher<PanicFetcher>;

    #[test]
    fn remove_corrupt_cache_entry_removes_existing_file() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("entry");
        fs::write(&path, b"corrupt").expect("plant entry");
        TestCachingFetcher::remove_corrupt_cache_entry(&path).expect("removes existing entry");
        assert!(!path.exists());
    }

    #[test]
    fn remove_corrupt_cache_entry_tolerates_already_missing_entry() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("already-gone");
        TestCachingFetcher::remove_corrupt_cache_entry(&path)
            .expect("missing entry counts as a completed removal");
    }

    #[test]
    fn remove_corrupt_cache_entry_stays_loud_on_unremovable_entry() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("entry-dir");
        fs::create_dir_all(path.join("child")).expect("create blocking dir");
        let error = TestCachingFetcher::remove_corrupt_cache_entry(&path)
            .expect_err("a directory in the entry path cannot be removed silently");
        assert!(
            format!("{error:#}").contains("delete corrupt object cache entry"),
            "loud removal failure expected, got: {error:#}"
        );
    }

    /// Repo-relative path to the committed PMXT reference NT catalog and the
    /// metadata that records its logical hash. Reusing the same fixture the
    /// `catalog_projection` hash-invariance test pins gives a real catalog whose
    /// `logical_catalog_hash` is known, so the carry-forward gate can be proven
    /// in both directions without hand-building a catalog.
    fn committed_reference_run_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root")
            .join(
                "specs/023-nt-research-analytics-platform/reference/\
                 pmxt-polymarket-selected-source-conversion/backtests/pmxt-run",
            )
    }

    fn committed_reference_catalog_hash() -> String {
        let metadata: serde_json::Value = serde_json::from_slice(
            &fs::read(committed_reference_run_dir().join("catalog-metadata.json"))
                .expect("read committed catalog metadata"),
        )
        .expect("parse committed catalog metadata");
        metadata["catalog_hash"]
            .as_str()
            .expect("catalog_hash present in committed metadata")
            .to_string()
    }

    /// Recursively copy `src` into `dst`, used to plant the committed reference
    /// catalog under a temp record `output_dir` so it can be deleted/corrupted
    /// without touching the committed fixture.
    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("create copy dir");
        for entry in fs::read_dir(src).expect("read source dir") {
            let entry = entry.expect("dir entry");
            let target = dst.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                copy_dir_all(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).expect("copy file");
            }
        }
    }

    fn carried_record_with_output(
        output_dir: PathBuf,
        catalog_hash: String,
    ) -> SourceUniverseBatchExecutionRecord {
        SourceUniverseBatchExecutionRecord {
            sequence: 0,
            operator_run_id: "operator-run-carried".to_string(),
            source_binding: "binding".to_string(),
            category: "spot".to_string(),
            symbol: "SYMBOL".to_string(),
            archive_date: "2026-03-01".to_string(),
            selected_object_sha256: "a".repeat(64),
            run_spec_sha256: "b".repeat(64),
            accepted_tranche_sha256: "c".repeat(64),
            execution_plan_sha256: "d".repeat(64),
            execution_record_sha256: "e".repeat(64),
            source_bindings_sha256: "f".repeat(64),
            selected_object_bytes: 0,
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash,
            output_dir,
        }
    }

    /// Build an [`OwnedBatchPlan`] with a single pack record whose sha matches
    /// the carried `prior`, so the carry-forward decision turns purely on the
    /// new output re-verification gate.
    fn owned_plan_with_carry(prior: &SourceUniverseBatchExecutionRecord) -> OwnedBatchPlan {
        let pack_record = SourceUniverseExecutionPackRecord {
            sequence: prior.sequence,
            work_item_id: "work-item".to_string(),
            operator_run_id: prior.operator_run_id.clone(),
            source_binding: prior.source_binding.clone(),
            category: prior.category.clone(),
            symbol: prior.symbol.clone(),
            archive_date: prior.archive_date.clone(),
            source_uri: "s3://bucket/object.csv.gz".to_string(),
            source_url: "https://example/object.csv.gz".to_string(),
            selected_object_sha256: prior.selected_object_sha256.clone(),
            selected_object_bytes: prior.selected_object_bytes,
            source_proof_id: "proof".to_string(),
            source_proof_version: 1,
            accepted_tranche_id: "tranche".to_string(),
            output_prefix: "s3://bucket/out".to_string(),
            run_spec_path: PathBuf::from("run-spec.toml"),
            run_spec_sha256: prior.run_spec_sha256.clone(),
            accepted_tranche_path: PathBuf::from("tranche.json"),
            accepted_tranche_sha256: prior.accepted_tranche_sha256.clone(),
            execution_plan_path: PathBuf::from("execution-plan.json"),
            execution_plan_sha256: prior.execution_plan_sha256.clone(),
        };
        let pack = SourceUniverseExecutionPack {
            schema_version: SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION.to_string(),
            pack_id: "pack".to_string(),
            status: SourceUniverseExecutionPackStatus::Ready,
            work_order_id: "work-order".to_string(),
            input_id: "input".to_string(),
            gate_id: "gate".to_string(),
            conversion_run_plan_id: "run-plan".to_string(),
            universe_id: "universe".to_string(),
            venue: "venue".to_string(),
            source: "public_archive".to_string(),
            family: "tick_trades".to_string(),
            table_family: "trades".to_string(),
            planned_object_count: 1,
            executable_record_count: 1,
            withheld_record_count: 0,
            selected_record_count: 1,
            materialized_record_count: 1,
            skipped_executable_record_count: 0,
            executable_source_bytes: 0,
            materialized_source_bytes: 0,
            artifact_refs: Vec::new(),
            records: vec![pack_record],
            blocking_reasons: Vec::new(),
        };
        let run_spec_bytes: Arc<[u8]> = Arc::from(
            include_bytes!(
                "../../../specs/023-nt-research-analytics-platform/reference/\
                 source-universe-execution-packs/\
                 bybit-public-archive-tick-trades-2025-06-01-2026-06-01/\
                 execution-pack/runs/\
                 00000-source-universe-operator-run-bybit-public-archive-tick-trades-2025-06-01-2026-06-01-00000/\
                 run-spec.toml"
            )
            .as_slice(),
        );
        let accepted_tranche_bytes: Arc<[u8]> = Arc::from(
            include_bytes!(
                "../../../specs/023-nt-research-analytics-platform/reference/\
                 source-universe-execution-packs/\
                 bybit-public-archive-tick-trades-2025-06-01-2026-06-01/\
                 execution-pack/runs/\
                 00000-source-universe-operator-run-bybit-public-archive-tick-trades-2025-06-01-2026-06-01-00000/\
                 backfill-accepted-tranche-manifest.json"
            )
            .as_slice(),
        );
        let execution_plan_bytes: Arc<[u8]> = Arc::from(
            include_bytes!(
                "../../../specs/023-nt-research-analytics-platform/reference/\
                 source-universe-execution-packs/\
                 bybit-public-archive-tick-trades-2025-06-01-2026-06-01/\
                 execution-pack/runs/\
                 00000-source-universe-operator-run-bybit-public-archive-tick-trades-2025-06-01-2026-06-01-00000/\
                 backfill-execution-plan.json"
            )
            .as_slice(),
        );
        let controls = validate_backfill_execution_control_bytes(
            &run_spec_bytes,
            &accepted_tranche_bytes,
            &execution_plan_bytes,
        )
        .expect("committed controls validate");
        let source_bindings_bytes: Arc<[u8]> = Arc::from(
            include_bytes!(
                "../../../specs/023-nt-research-analytics-platform/reference/\
                 backfill-source-bindings.v1.toml"
            )
            .as_slice(),
        );
        let source_bindings = Arc::new(
            SourceBindingRegistry::from_toml_str(
                std::str::from_utf8(&source_bindings_bytes).expect("registry is UTF-8"),
            )
            .expect("registry parses"),
        );
        let source_bindings_sha256 = hex::encode(Sha256::digest(source_bindings_bytes.as_ref()));
        let mut verified_control_artifacts = BTreeMap::new();
        verified_control_artifacts.insert(
            0,
            SourceUniverseVerifiedControlArtifacts {
                run_spec_path: PathBuf::from("run-spec.toml"),
                run_spec_bytes,
                run_spec: Arc::new(controls.run_spec),
                accepted_tranche_path: PathBuf::from("tranche.json"),
                accepted_tranche_bytes,
                accepted_tranche: Arc::new(controls.accepted_tranche),
                execution_plan_path: PathBuf::from("execution-plan.json"),
                execution_plan_bytes,
                execution_plan: Arc::new(controls.execution_plan),
                source_bindings_path: PathBuf::from("source-bindings.toml"),
                source_bindings_sha256: source_bindings_sha256.clone(),
                source_bindings_bytes,
                source_bindings,
            },
        );
        let execution_record_sha256s = execution_record_digests(&pack).expect("fingerprint pack");
        let mut carried = prior.clone();
        carried.execution_record_sha256 = execution_record_sha256s
            .get(&prior.sequence)
            .expect("record digest")
            .clone();
        carried.source_bindings_sha256 = source_bindings_sha256;
        let mut resume_records = BTreeMap::new();
        resume_records.insert(prior.sequence, carried);
        static NEXT_OUTPUT_ROOT: AtomicUsize = AtomicUsize::new(0);
        let fresh_output_root = prior
            .output_dir
            .parent()
            .expect("prior output has parent")
            .join(format!(
                "fresh-output-{}",
                NEXT_OUTPUT_ROOT.fetch_add(1, Ordering::SeqCst)
            ));
        fs::create_dir_all(&fresh_output_root).expect("create fresh output root");
        let output_root_lease = BatchOutputRootLease::acquire(&fresh_output_root)
            .expect("acquire test output-root lease");
        OwnedBatchPlan {
            pack,
            execution_record_sha256s,
            verified_control_artifacts,
            control_artifact_failures: BTreeMap::new(),
            resume_records,
            start_sequence: None,
            record_limit: usize::MAX,
            output_root_lease,
        }
    }

    #[test]
    fn carried_output_verifies_against_intact_reference_catalog() {
        // Positive control: a carried record whose prior output catalog is intact
        // and whose carried hash matches the catalog re-verifies, so the gate is
        // not vacuously rejecting every record.
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &output_dir.join(CATALOG_DIR),
        );
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        assert!(
            carried_output_still_verifies(&record),
            "intact prior catalog matching the carried hash must verify"
        );
    }

    #[test]
    fn carried_output_does_not_verify_when_catalog_is_deleted() {
        // The finding's scenario: the prior output catalog no longer exists. The
        // bare sha match must NOT carry it forward off the stale marker.
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        fs::create_dir_all(&output_dir).expect("create empty output dir");
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        assert!(
            !carried_output_still_verifies(&record),
            "a deleted prior catalog must fail re-verification"
        );
    }

    #[test]
    fn carried_output_does_not_verify_when_catalog_is_corrupted() {
        // The prior output catalog still exists but its bytes drifted, so its
        // recomputed logical hash no longer matches the carried hash.
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        let catalog_root = output_dir.join(CATALOG_DIR);
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &catalog_root,
        );
        // Carry a hash that does not describe the planted catalog: a drifted
        // output must read as "not a match" rather than be reused verbatim.
        let record = carried_record_with_output(output_dir, "f".repeat(64));
        assert!(
            !carried_output_still_verifies(&record),
            "a prior catalog whose recomputed hash differs from the carried hash must not verify"
        );
    }

    #[test]
    fn plan_reexecutes_carried_record_when_prior_output_is_missing() {
        // End-to-end through the carry-forward decision: a sha-matching prior
        // record whose output catalog is gone is downgraded to NeedsWork (fresh
        // fetch + verify + run), never carried forward as Completed from the
        // stale resume marker.
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        fs::create_dir_all(&output_dir).expect("create empty output dir");
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        let owned_plan = owned_plan_with_carry(&record);
        let plan = owned_plan.plan();
        assert!(
            matches!(
                resolve_work_item(
                    &plan.work_items[0],
                    &owned_plan.output_root_lease,
                    &SourceUniverseBatchExecutionConfig::default(),
                ),
                ResolvedBatchWorkItem::Fresh(_)
            ),
            "a carried record with a missing prior catalog must re-execute, not carry forward"
        );
    }

    #[test]
    fn plan_carries_record_forward_when_prior_output_verifies() {
        // Positive control through the decision: with an intact, hash-matching
        // prior catalog the record is still carried forward (no needless re-run).
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &output_dir.join(CATALOG_DIR),
        );
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        let owned_plan = owned_plan_with_carry(&record);
        let plan = owned_plan.plan();
        assert!(
            matches!(
                resolve_work_item(
                    &plan.work_items[0],
                    &owned_plan.output_root_lease,
                    &SourceUniverseBatchExecutionConfig::default(),
                ),
                ResolvedBatchWorkItem::Terminal(RecordSlot::Carried { .. })
            ),
            "an intact, hash-matching prior catalog must still carry forward"
        );
    }

    #[test]
    fn final_assembly_rejects_a_carried_catalog_that_drifted_after_resolution() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &output_dir.join(CATALOG_DIR),
        );
        let record =
            carried_record_with_output(output_dir.clone(), committed_reference_catalog_hash());
        let owned_plan = owned_plan_with_carry(&record);
        let plan = owned_plan.plan();
        let slot = match resolve_work_item(
            &plan.work_items[0],
            &owned_plan.output_root_lease,
            &SourceUniverseBatchExecutionConfig {
                continue_on_error: true,
                ..SourceUniverseBatchExecutionConfig::default()
            },
        ) {
            ResolvedBatchWorkItem::Terminal(slot @ RecordSlot::Carried { .. }) => slot,
            _ => panic!("intact catalog must initially resolve as carried"),
        };
        fs::remove_dir_all(output_dir.join(CATALOG_DIR)).expect("remove carried catalog");

        let report = assemble_report(
            "batch",
            &owned_plan,
            vec![Some(slot)],
            &SourceUniverseBatchExecutionConfig {
                continue_on_error: true,
                ..SourceUniverseBatchExecutionConfig::default()
            },
        )
        .expect("continue-on-error records final carry drift");

        assert_eq!(
            report.status,
            SourceUniverseBatchExecutionReportStatus::Failed
        );
        assert_eq!(report.records.len(), 0);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].failure_stage, "verify_carried_catalog");
    }

    #[test]
    fn plan_reexecutes_carried_record_when_any_control_artifact_hash_changes() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &output_dir.join(CATALOG_DIR),
        );
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        for field in ["run_spec", "accepted_tranche", "execution_plan"] {
            let mut owned_plan = owned_plan_with_carry(&record);
            match field {
                "run_spec" => owned_plan.pack.records[0].run_spec_sha256 = "e".repeat(64),
                "accepted_tranche" => {
                    owned_plan.pack.records[0].accepted_tranche_sha256 = "e".repeat(64);
                }
                "execution_plan" => {
                    owned_plan.pack.records[0].execution_plan_sha256 = "e".repeat(64);
                }
                _ => unreachable!("test enumerates every control artifact"),
            }

            let plan = owned_plan.plan();

            assert!(
                matches!(
                    resolve_work_item(
                        &plan.work_items[0],
                        &owned_plan.output_root_lease,
                        &SourceUniverseBatchExecutionConfig::default(),
                    ),
                    ResolvedBatchWorkItem::Fresh(_)
                ),
                "a changed {field} hash must re-execute even when source and output still match"
            );
        }
    }

    #[test]
    fn control_preflight_failure_takes_precedence_over_resume_carry() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &output_dir.join(CATALOG_DIR),
        );
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        let mut owned_plan = owned_plan_with_carry(&record);
        owned_plan
            .control_artifact_failures
            .insert(record.sequence, "pinned run spec is missing".to_string());

        let plan = owned_plan.plan();

        assert!(
            matches!(
                plan.work_items.as_slice(),
                [BatchWorkItem::PreflightFailed { .. }]
            ),
            "an invalid current control artifact must fail its record instead of carrying prior output"
        );
    }
}

fn validated_http_source_url(source_url: &str) -> Result<reqwest::Url> {
    let parsed_url = reqwest::Url::parse(source_url)
        .with_context(|| format!("parse source_url for batch execution: {source_url}"))?;
    ensure!(
        parsed_url.scheme() == "https",
        "source_url must be HTTPS for batch execution: {source_url}"
    );
    ensure!(
        parsed_url
            .host_str()
            .map(|host| !host.trim().is_empty())
            .unwrap_or(false),
        "source_url host must not be empty"
    );
    ensure!(
        !parsed_url.path().trim_start_matches('/').trim().is_empty(),
        "source_url missing object path: {source_url}"
    );
    ensure!(
        parsed_url.query().is_none() && parsed_url.fragment().is_none(),
        "source_url query and fragment components are not supported: {source_url}"
    );
    Ok(parsed_url)
}
