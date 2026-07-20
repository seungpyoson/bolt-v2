//! Batch execution for source-universe single-object operator runs.
//!
//! Source-universe execution packs already materialize one run-spec and
//! execution plan per accepted object. This module adds the missing operator
//! loop: fetch the pinned object, verify bytes/hash, run the existing
//! single-object operator path, and summarize the completed records.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic_artifact_write::atomic_write;
use crate::backfill_accepted_tranche::{
    BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
    BackfillAcceptedTrancheStatus,
};
use crate::backfill_execution_plan::{
    BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION, BackfillExecutionPlan, BackfillExecutionPlanStatus,
    BackfillExecutionRunBinding, BackfillExecutionWorkBudget, evaluate_backfill_execution_plan,
};
use crate::catalog_projection::logical_catalog_hash;
use crate::conversion_boundary::{
    CATALOG_METADATA_FILE, CATALOG_METADATA_VERSION, ConversionCatalogMetadata,
};
use crate::io_safety::{ByteLimit, ensure_within_limit, read_to_vec_with_limit};
use crate::path_resolution::resolve_existing_path;
use crate::{
    operator::{
        CATALOG_DIR, RunSpec, ValidatedRunSpecExecution, run_from_validated_run_spec,
        validate_run_spec_execution,
    },
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackRecord,
        SourceUniverseExecutionPackStatus, validate_source_universe_execution_pack,
    },
};

pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION: &str =
    "source-universe-batch-execution-report.v2";
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
        controls: &SourceUniverseAdmittedControls,
        output_dir: &Path,
    ) -> Result<SourceUniverseBatchExecutionRunOutput>;
}

/// Exact control bytes admitted before any source fetch or output creation.
///
/// The operator runner consumes this owned snapshot rather than reopening the
/// pack's paths, so later filesystem changes cannot alter the admitted run.
#[derive(Debug, Clone)]
pub struct SourceUniverseAdmittedControls {
    run_spec_bytes: Arc<[u8]>,
    accepted_tranche_bytes: Arc<[u8]>,
    execution_plan_bytes: Arc<[u8]>,
    run_spec_execution: ValidatedRunSpecExecution,
}

impl SourceUniverseAdmittedControls {
    #[must_use]
    pub fn run_spec_bytes(&self) -> &[u8] {
        &self.run_spec_bytes
    }

    #[must_use]
    pub fn accepted_tranche_bytes(&self) -> &[u8] {
        &self.accepted_tranche_bytes
    }

    #[must_use]
    pub fn execution_plan_bytes(&self) -> &[u8] {
        &self.execution_plan_bytes
    }

    #[must_use]
    pub fn run_spec(&self) -> &RunSpec {
        self.run_spec_execution.run_spec()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceUniverseControlAdmissionPolicy {
    max_run_spec_bytes: ByteLimit,
    max_accepted_tranche_bytes: ByteLimit,
    max_execution_plan_bytes: ByteLimit,
    max_total_control_bytes: ByteLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseControlAdmissionPolicySpec {
    max_run_spec_bytes: u64,
    max_accepted_tranche_bytes: u64,
    max_execution_plan_bytes: u64,
    max_total_control_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseControlAdmissionPolicyFile {
    control_admission: SourceUniverseControlAdmissionPolicySpec,
}

impl SourceUniverseControlAdmissionPolicy {
    fn from_spec(spec: SourceUniverseControlAdmissionPolicySpec) -> Result<Self> {
        for (name, value) in [
            ("max_run_spec_bytes", spec.max_run_spec_bytes),
            (
                "max_accepted_tranche_bytes",
                spec.max_accepted_tranche_bytes,
            ),
            ("max_execution_plan_bytes", spec.max_execution_plan_bytes),
            ("max_total_control_bytes", spec.max_total_control_bytes),
        ] {
            usize::try_from(value).with_context(|| {
                format!("control admission {name} is not representable as usize")
            })?;
        }

        let policy = Self {
            max_run_spec_bytes: ByteLimit::new(spec.max_run_spec_bytes)
                .context("control admission max_run_spec_bytes")?,
            max_accepted_tranche_bytes: ByteLimit::new(spec.max_accepted_tranche_bytes)
                .context("control admission max_accepted_tranche_bytes")?,
            max_execution_plan_bytes: ByteLimit::new(spec.max_execution_plan_bytes)
                .context("control admission max_execution_plan_bytes")?,
            max_total_control_bytes: ByteLimit::new(spec.max_total_control_bytes)
                .context("control admission max_total_control_bytes")?,
        };
        for (name, individual) in [
            ("max_run_spec_bytes", policy.max_run_spec_bytes),
            (
                "max_accepted_tranche_bytes",
                policy.max_accepted_tranche_bytes,
            ),
            ("max_execution_plan_bytes", policy.max_execution_plan_bytes),
        ] {
            ensure!(
                policy.max_total_control_bytes.max_bytes() >= individual.max_bytes(),
                "control admission max_total_control_bytes {} is less than {name} {}; \
                 the individual ceiling would be unreachable",
                policy.max_total_control_bytes.max_bytes(),
                individual.max_bytes()
            );
        }
        Ok(policy)
    }
}

pub fn load_source_universe_control_admission_policy(
    policy_path: &Path,
) -> Result<SourceUniverseControlAdmissionPolicy> {
    let bytes = fs::read(policy_path).with_context(|| {
        format!(
            "read source-universe control admission policy {}",
            policy_path.display()
        )
    })?;
    let file: SourceUniverseControlAdmissionPolicyFile =
        toml::from_slice(&bytes).with_context(|| {
            format!(
                "parse source-universe control admission policy TOML {}",
                policy_path.display()
            )
        })?;
    SourceUniverseControlAdmissionPolicy::from_spec(file.control_admission).with_context(|| {
        format!(
            "validate source-universe control admission policy {}",
            policy_path.display()
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionRunOutput {
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
}

/// Tuning for a source-universe batch execution run.
///
/// Scheduling fields remain optional, but control admission policy is required
/// so no execution entry point can silently use a hidden byte limit. Object caching is
/// deliberately NOT a config field: the one way to enable it is wrapping the
/// fetcher in [`CachingSourceUniverseObjectFetcher`] (the CLI does this for
/// `--object-cache-dir`), so a cache can never be requested without taking
/// effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionConfig {
    /// Trusted TOML policy governing control reads and retained control bytes.
    pub control_admission: SourceUniverseControlAdmissionPolicy,
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
    pub selected_object_bytes: u64,
    /// Pins the admitted control identity used to produce this output. The
    /// plan transitively binds the admitted run-spec and accepted tranche.
    pub execution_plan_sha256: String,
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
        controls: &SourceUniverseAdmittedControls,
        output_dir: &Path,
    ) -> Result<SourceUniverseBatchExecutionRunOutput> {
        let artifacts =
            run_from_validated_run_spec(&controls.run_spec_execution, object_bytes, output_dir)?;
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
    control_admission: SourceUniverseControlAdmissionPolicy,
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
            control_admission,
            start_sequence: None,
            record_limit,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
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
        let slot = process_work_item(work_item, output_dir, &config, fetcher, runner);
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
    let worker_count = config
        .max_concurrent_records
        .and_then(|workers| usize::try_from(workers).ok())
        .unwrap_or(1)
        .max(1)
        .min(work_item_count.max(1));

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
            handles.push(scope.spawn(move || -> Result<()> {
                let mut fetcher = fetcher_factory().context("construct batch worker fetcher")?;
                let mut runner = runner_factory().context("construct batch worker runner")?;
                loop {
                    if !config.continue_on_error && stop_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    let index = next_index.fetch_add(1, Ordering::SeqCst);
                    if index >= work_items.len() {
                        break;
                    }
                    let work_item = &work_items[index];
                    let slot =
                        process_work_item(work_item, output_dir, config, &mut fetcher, &mut runner);
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

/// A single unit of batch work after selection and resume filtering: either a
/// record carried forward from a prior report (only after its input sha matches
/// and its prior output catalog and metadata re-verify), or a pack record that
/// still needs fetch + verify + run.
enum BatchWorkItem<'pack> {
    Carried {
        record: &'pack SourceUniverseExecutionPackRecord,
        output: VerifiedCarriedOutput,
    },
    NeedsWork {
        record: &'pack SourceUniverseExecutionPackRecord,
        controls: &'pack SourceUniverseAdmittedControls,
    },
}

/// Outcome for one work item, kept in original-sequence slot order so the
/// assembled report is independent of completion order.
enum RecordSlot {
    Completed(SourceUniverseBatchExecutionRecord),
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
    validate_source_universe_execution_pack(&pack)
        .with_context(|| format!("validate execution pack {}", execution_pack_path.display()))?;
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

    let mut admitted_controls = BTreeMap::new();
    let mut total_control_bytes = 0_u64;
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
        let (controls, next_total_control_bytes) = admit_record_controls(
            &pack,
            &pack_base_dir,
            record,
            output_dir,
            &config.control_admission,
            total_control_bytes,
        )
        .with_context(|| {
            format!(
                "admit controls for pack record {} ({})",
                record.sequence, record.operator_run_id
            )
        })?;
        total_control_bytes = next_total_control_bytes;
        ensure!(
            admitted_controls
                .insert(record.sequence, controls)
                .is_none(),
            "execution pack {} has duplicate selected sequence {}",
            pack.pack_id,
            record.sequence
        );
    }

    let resume_records = load_resume_records(config.resume_report.as_deref(), &pack)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create batch output dir {}", output_dir.display()))?;

    Ok(OwnedBatchPlan {
        pack,
        admitted_controls,
        resume_records,
        start_sequence: config.start_sequence,
        record_limit,
    })
}

/// Owns the parsed pack and resume map so the `'pack`-lifetime [`BatchPlan`]
/// work items can borrow from it without an extra clone of every pack record.
struct OwnedBatchPlan {
    pack: SourceUniverseExecutionPack,
    admitted_controls: BTreeMap<u64, SourceUniverseAdmittedControls>,
    resume_records: BTreeMap<u64, SourceUniverseBatchExecutionRecord>,
    start_sequence: Option<u64>,
    record_limit: usize,
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
                // Pack-regeneration guard: carry forward only when the prior
                // record's pinned object AND admitted execution-plan sha still
                // match the current pack record, and the prior output catalog
                // still exists and re-hashes to the carried `catalog_hash`.
                // The plan hash transitively pins the admitted run-spec and
                // accepted tranche, so a changed declared execution can never
                // launder stale output through resume merely because its source
                // object is unchanged. A bare object-sha match only proves the
                // INPUT is unchanged; it never proves the execution controls or
                // prior OUTPUT survived unchanged.
                // Re-verifying through the same `logical_catalog_hash` the
                // operator's completed-output reuse path uses means a deleted or
                // corrupted prior catalog re-executes the record (downgraded to
                // `NeedsWork`) instead of being marked completed off a stale
                // marker — fail safe, never carry a phantom output forward.
                if let Some(prior) = self.resume_records.get(&record.sequence)
                    && prior.selected_object_sha256 == record.selected_object_sha256
                    && prior.execution_plan_sha256 == record.execution_plan_sha256
                    && let Some(output) = verified_carried_output(prior)
                {
                    return BatchWorkItem::Carried { record, output };
                }
                BatchWorkItem::NeedsWork {
                    record,
                    controls: self
                        .admitted_controls
                        .get(&record.sequence)
                        .expect("selected record controls admitted before plan construction"),
                }
            })
            .collect();
        BatchPlan { work_items }
    }
}

fn admit_record_controls(
    pack: &SourceUniverseExecutionPack,
    pack_base_dir: &Path,
    record: &SourceUniverseExecutionPackRecord,
    batch_output_dir: &Path,
    policy: &SourceUniverseControlAdmissionPolicy,
    admitted_control_bytes: u64,
) -> Result<(SourceUniverseAdmittedControls, u64)> {
    let mut next_total_control_bytes = admitted_control_bytes;
    let run_spec_bytes = read_pinned_control(
        pack_base_dir,
        record,
        "run_spec",
        &record.run_spec_path,
        &record.run_spec_sha256,
        policy.max_run_spec_bytes,
        policy.max_total_control_bytes,
        &mut next_total_control_bytes,
    )?;
    let accepted_tranche_bytes = read_pinned_control(
        pack_base_dir,
        record,
        "accepted_tranche",
        &record.accepted_tranche_path,
        &record.accepted_tranche_sha256,
        policy.max_accepted_tranche_bytes,
        policy.max_total_control_bytes,
        &mut next_total_control_bytes,
    )?;
    let execution_plan_bytes = read_pinned_control(
        pack_base_dir,
        record,
        "execution_plan",
        &record.execution_plan_path,
        &record.execution_plan_sha256,
        policy.max_execution_plan_bytes,
        policy.max_total_control_bytes,
        &mut next_total_control_bytes,
    )?;

    let run_spec_text =
        std::str::from_utf8(&run_spec_bytes).context("decode admitted run_spec as UTF-8")?;
    let run_spec: RunSpec =
        toml::from_str(run_spec_text).context("parse admitted run_spec TOML")?;
    let accepted_tranche: BackfillAcceptedTrancheManifest =
        serde_json::from_slice(&accepted_tranche_bytes)
            .context("parse admitted accepted_tranche JSON")?;
    let execution_plan: BackfillExecutionPlan = serde_json::from_slice(&execution_plan_bytes)
        .context("parse admitted execution_plan JSON")?;

    validate_admitted_control_binding(
        pack,
        record,
        &run_spec,
        &accepted_tranche,
        &execution_plan,
        &run_spec_bytes,
        &accepted_tranche_bytes,
    )?;
    let record_output_dir = batch_output_dir.join(&record.operator_run_id);
    let run_spec_execution = validate_run_spec_execution(
        Arc::new(run_spec),
        &record_output_dir,
        &record.selected_object_sha256,
    )
    .context("validate admitted run_spec execution inputs")?;

    Ok((
        SourceUniverseAdmittedControls {
            run_spec_bytes,
            accepted_tranche_bytes,
            execution_plan_bytes,
            run_spec_execution,
        },
        next_total_control_bytes,
    ))
}

fn read_pinned_control(
    pack_base_dir: &Path,
    record: &SourceUniverseExecutionPackRecord,
    role: &str,
    declared_path: &Path,
    expected_sha256: &str,
    individual_limit: ByteLimit,
    total_limit: ByteLimit,
    admitted_control_bytes: &mut u64,
) -> Result<Arc<[u8]>> {
    validate_sha256_hex(expected_sha256)
        .with_context(|| format!("pack record {} has invalid {role}_sha256", record.sequence))?;
    let remaining_total_bytes = total_limit
        .max_bytes()
        .checked_sub(*admitted_control_bytes)
        .context("admitted control bytes already exceed configured max_total_control_bytes")?;
    ensure!(
        remaining_total_bytes > 0,
        "batch admitted control bytes would exceed configured max_total_control_bytes while reading {role}"
    );
    let constrained_by_total = remaining_total_bytes < individual_limit.max_bytes();
    let effective_limit = ByteLimit::new(individual_limit.max_bytes().min(remaining_total_bytes))
        .context("construct effective control byte limit")?;
    let resolved_path = resolve_existing_path(pack_base_dir, declared_path);
    let file = File::open(&resolved_path)
        .with_context(|| format!("open pinned {role} {}", resolved_path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect pinned {role} {}", resolved_path.display()))?;
    ensure!(
        metadata.is_file(),
        "pinned {role} {} is not a regular file",
        resolved_path.display()
    );
    ensure_within_limit(
        format!("read pinned {role} {}", resolved_path.display()),
        metadata.len(),
        effective_limit,
    )?;
    let read_result = read_to_vec_with_limit(
        file,
        effective_limit,
        format!("read pinned {role} {}", resolved_path.display()),
    );
    let bytes = if constrained_by_total {
        read_result.with_context(|| {
            format!(
                "batch admitted control bytes would exceed configured max_total_control_bytes while reading {role}"
            )
        })?
    } else {
        read_result?
    };
    let byte_count = u64::try_from(bytes.len()).context("control byte length exceeds u64")?;
    *admitted_control_bytes = (*admitted_control_bytes)
        .checked_add(byte_count)
        .context("batch admitted control byte total overflow")?;
    ensure!(
        *admitted_control_bytes <= total_limit.max_bytes(),
        "batch admitted control bytes {} exceed configured max_total_control_bytes {} while reading {role}",
        *admitted_control_bytes,
        total_limit.max_bytes()
    );
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    ensure!(
        actual_sha256 == expected_sha256,
        "pack record {} pinned {role} sha256 mismatch: expected {}, got {}",
        record.sequence,
        expected_sha256,
        actual_sha256
    );
    Ok(Arc::from(bytes))
}

fn validate_admitted_control_binding(
    pack: &SourceUniverseExecutionPack,
    record: &SourceUniverseExecutionPackRecord,
    run_spec: &RunSpec,
    accepted_tranche: &BackfillAcceptedTrancheManifest,
    execution_plan: &BackfillExecutionPlan,
    run_spec_bytes: &[u8],
    accepted_tranche_bytes: &[u8],
) -> Result<()> {
    validate_admitted_control_self_validity(
        pack,
        record,
        run_spec,
        accepted_tranche,
        execution_plan,
    )?;

    let run_spec_sha256 = hex::encode(Sha256::digest(run_spec_bytes));
    let accepted_tranche_sha256 = hex::encode(Sha256::digest(accepted_tranche_bytes));
    let expected_plan = evaluate_backfill_execution_plan(
        execution_plan.plan_id.clone(),
        accepted_tranche_sha256.clone(),
        accepted_tranche,
        run_spec_sha256.clone(),
        &BackfillExecutionRunBinding::from_run_spec(run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: execution_plan.max_source_rows,
            max_projected_row_groups: execution_plan.max_projected_row_groups,
            max_wall_seconds: execution_plan.max_wall_seconds,
            require_object_selection_metadata: execution_plan.require_object_selection_metadata,
        },
    );
    ensure!(
        execution_plan == &expected_plan,
        "execution_plan does not match the admitted run_spec and accepted_tranche"
    );

    ensure!(
        !record.operator_run_id.trim().is_empty(),
        "pack record operator_run_id must not be empty"
    );
    ensure!(
        !record.accepted_tranche_id.trim().is_empty(),
        "pack record accepted_tranche_id must not be empty"
    );
    ensure!(
        execution_plan.operator_run_id == record.operator_run_id
            && run_spec.manifest.run_id == record.operator_run_id,
        "operator_run_id mismatch across pack record, run_spec, and execution_plan"
    );
    ensure!(
        accepted_tranche.tranche_id == record.accepted_tranche_id
            && execution_plan.accepted_tranche_id == record.accepted_tranche_id,
        "accepted_tranche_id mismatch across pack record, accepted_tranche, and execution_plan"
    );
    ensure!(
        execution_plan.run_spec_hash == run_spec_sha256,
        "execution_plan run_spec_hash does not bind the admitted run_spec bytes"
    );
    ensure!(
        execution_plan.accepted_tranche_manifest_hash == accepted_tranche_sha256,
        "execution_plan accepted_tranche_manifest_hash does not bind the admitted accepted_tranche bytes"
    );
    ensure!(
        execution_plan.source_proof_id == record.source_proof_id
            && execution_plan.source_proof_version == record.source_proof_version,
        "source proof identity mismatch between pack record and admitted controls"
    );
    ensure!(
        execution_plan.source_binding == record.source_binding,
        "source_binding mismatch between pack record and admitted controls"
    );
    ensure!(
        execution_plan.output_prefix == record.output_prefix,
        "output_prefix mismatch between pack record and admitted controls"
    );
    ensure!(
        run_spec.source_proof.venue == pack.venue,
        "venue mismatch between execution pack and admitted run_spec source proof"
    );
    ensure!(
        execution_plan.table_family == pack.table_family,
        "table_family mismatch between execution pack and admitted controls"
    );
    ensure!(
        run_spec.source_proof.product_category == record.category,
        "category mismatch between pack record and admitted run_spec source proof"
    );
    let identity = run_spec
        .identity
        .single()
        .context("admitted run_spec identity must be single-instrument")?;
    ensure!(
        identity.instrument_id == record.symbol && identity.venue_symbol == record.symbol,
        "symbol mismatch between pack record and admitted run_spec identity"
    );

    let object = execution_plan
        .objects
        .first()
        .context("ready execution_plan must contain exactly one object")?;
    ensure!(
        object.sha256 == record.selected_object_sha256,
        "selected_object_sha256 mismatch between pack record and admitted controls"
    );
    ensure!(
        object.bytes == record.selected_object_bytes,
        "selected_object_bytes mismatch between pack record and admitted controls"
    );
    ensure!(
        object.s3_uri == record.source_uri,
        "source_uri mismatch between pack record and admitted controls"
    );
    ensure!(
        object.source_url == record.source_url,
        "source_url mismatch between pack record and admitted controls"
    );
    ensure!(
        object.archive_date == record.archive_date,
        "archive_date mismatch between pack record and admitted controls"
    );
    Ok(())
}

fn validate_admitted_control_self_validity(
    pack: &SourceUniverseExecutionPack,
    record: &SourceUniverseExecutionPackRecord,
    run_spec: &RunSpec,
    accepted_tranche: &BackfillAcceptedTrancheManifest,
    execution_plan: &BackfillExecutionPlan,
) -> Result<()> {
    for (name, value) in [
        ("venue", pack.venue.as_str()),
        ("table_family", pack.table_family.as_str()),
        ("operator_run_id", record.operator_run_id.as_str()),
        ("accepted_tranche_id", record.accepted_tranche_id.as_str()),
        ("source_binding", record.source_binding.as_str()),
        ("category", record.category.as_str()),
        ("symbol", record.symbol.as_str()),
        ("archive_date", record.archive_date.as_str()),
        ("source_uri", record.source_uri.as_str()),
        ("source_url", record.source_url.as_str()),
        ("output_prefix", record.output_prefix.as_str()),
    ] {
        ensure!(!value.trim().is_empty(), "{name} must not be empty");
    }
    let mut operator_run_id_components = Path::new(&record.operator_run_id).components();
    ensure!(
        matches!(
            operator_run_id_components.next(),
            Some(Component::Normal(_))
        ) && operator_run_id_components.next().is_none(),
        "operator_run_id must be a single normal path component"
    );
    ensure!(
        !record.source_proof_id.trim().is_empty() && record.source_proof_version > 0,
        "source proof identity must be complete"
    );
    ensure!(
        record.selected_object_bytes > 0,
        "selected_object_bytes must be positive"
    );

    ensure!(
        accepted_tranche.schema_version == BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION,
        "accepted_tranche schema_version mismatch: expected {}, got {}",
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION,
        accepted_tranche.schema_version
    );
    ensure!(
        !accepted_tranche.tranche_id.trim().is_empty(),
        "accepted_tranche tranche_id must not be empty"
    );
    ensure!(
        accepted_tranche.status == BackfillAcceptedTrancheStatus::Accepted,
        "accepted_tranche status must be accepted"
    );
    ensure!(
        accepted_tranche.blocking_issues.is_empty(),
        "accepted_tranche must not contain blocking issues"
    );
    ensure!(
        accepted_tranche.object_count == 1 && accepted_tranche.objects.len() == 1,
        "accepted_tranche must bind exactly one object"
    );
    ensure!(
        !accepted_tranche
            .source_proof_scope_report_id
            .trim()
            .is_empty(),
        "accepted_tranche source_proof_scope_report_id must not be empty"
    );
    validate_sha256_hex(&accepted_tranche.source_proof_scope_report_hash)
        .context("accepted_tranche source_proof_scope_report_hash")?;
    ensure!(
        !accepted_tranche.source_proof_id.trim().is_empty()
            && accepted_tranche.source_proof_version > 0,
        "accepted_tranche source proof identity must be complete"
    );
    ensure!(
        !accepted_tranche.source_binding.trim().is_empty(),
        "accepted_tranche source_binding must not be empty"
    );
    ensure!(
        !accepted_tranche.table_family.trim().is_empty(),
        "accepted_tranche table_family must not be empty"
    );
    ensure!(
        !accepted_tranche.parent_manifest_id.trim().is_empty(),
        "accepted_tranche parent_manifest_id must not be empty"
    );
    ensure!(
        accepted_tranche.object_level_tranche_required,
        "accepted_tranche object_level_tranche_required must be true"
    );
    let tranche_object = &accepted_tranche.objects[0];
    for (name, value) in [
        ("s3_uri", tranche_object.s3_uri.as_str()),
        ("source_url", tranche_object.source_url.as_str()),
        ("archive_date", tranche_object.archive_date.as_str()),
    ] {
        ensure!(
            !value.trim().is_empty(),
            "accepted_tranche object {name} must not be empty"
        );
    }
    validate_sha256_hex(&tranche_object.sha256).context("accepted_tranche object sha256")?;
    ensure!(
        tranche_object.bytes > 0,
        "accepted_tranche object bytes must be positive"
    );
    ensure!(
        accepted_tranche.accepted_bytes == tranche_object.bytes,
        "accepted_tranche accepted_bytes must equal its object bytes"
    );
    validate_control_selection_metadata(
        &tranche_object.source_row_groups,
        tranche_object.predicate_ref.as_deref(),
    )?;

    ensure!(
        execution_plan.schema_version == BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
        "execution_plan schema_version mismatch: expected {}, got {}",
        BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
        execution_plan.schema_version
    );
    ensure!(
        !execution_plan.plan_id.trim().is_empty(),
        "execution_plan plan_id must not be empty"
    );
    ensure!(
        execution_plan.status == BackfillExecutionPlanStatus::Ready,
        "execution_plan status must be ready"
    );
    ensure!(
        execution_plan.blocking_issues.is_empty(),
        "execution_plan must not contain blocking issues"
    );
    ensure!(
        execution_plan.object_count == 1 && execution_plan.objects.len() == 1,
        "execution_plan must bind exactly one object"
    );
    ensure!(
        execution_plan.max_source_rows > 0,
        "execution_plan max_source_rows must be positive"
    );
    ensure!(
        execution_plan.max_projected_row_groups > 0,
        "execution_plan max_projected_row_groups must be positive"
    );
    ensure!(
        execution_plan.max_wall_seconds > 0,
        "execution_plan max_wall_seconds must be positive"
    );
    ensure!(
        run_spec.converter.raw_payload.max_object_bytes > 0,
        "run_spec max_object_bytes must be positive"
    );
    ensure!(
        run_spec.converter.raw_payload.max_decoded_bytes > 0,
        "run_spec max_decoded_bytes must be positive"
    );
    ensure!(
        run_spec.manifest.source_proof_id == run_spec.source_proof.source_proof_id
            && run_spec.manifest.source_proof_version == run_spec.source_proof.source_proof_version,
        "run_spec source proof identity mismatch between manifest and source_proof"
    );
    ensure!(
        run_spec.manifest.venue_binding_key == run_spec.source_proof.source_binding,
        "run_spec source_binding mismatch between manifest and source_proof"
    );
    let plan_object = &execution_plan.objects[0];
    validate_control_selection_metadata(
        &plan_object.source_row_groups,
        plan_object.predicate_ref.as_deref(),
    )?;
    ensure!(
        !execution_plan.require_object_selection_metadata
            || !plan_object.source_row_groups.is_empty()
            || plan_object.predicate_ref.is_some(),
        "execution_plan requires object selection metadata"
    );
    Ok(())
}

fn validate_control_selection_metadata(
    source_row_groups: &[u64],
    predicate_ref: Option<&str>,
) -> Result<()> {
    ensure!(
        source_row_groups.windows(2).all(|pair| pair[0] < pair[1]),
        "source_row_groups must be strictly increasing and unique"
    );
    if let Some(predicate_ref) = predicate_ref {
        ensure!(
            !predicate_ref.trim().is_empty(),
            "predicate_ref must not be empty when present"
        );
    }
    Ok(())
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
#[derive(Debug)]
struct VerifiedCarriedOutput {
    output_dir: PathBuf,
    catalog_hash: String,
    canonical_rows: u64,
    nt_catalog_rows: u64,
}

fn verified_carried_output(
    prior: &SourceUniverseBatchExecutionRecord,
) -> Option<VerifiedCarriedOutput> {
    let catalog_root = prior.output_dir.join(CATALOG_DIR);
    // A missing prior output is "not verified" (re-execute), not a panic: NT's
    // catalog open expects an existing path and panics otherwise, so guard the
    // deleted/missing case here before re-hashing. A present-but-corrupt catalog
    // still surfaces as Err below.
    if !catalog_root.is_dir() {
        return None;
    }
    let actual_catalog_hash = logical_catalog_hash(&catalog_root).ok()?;
    if actual_catalog_hash != prior.catalog_hash {
        return None;
    }
    let metadata: ConversionCatalogMetadata =
        serde_json::from_slice(&fs::read(prior.output_dir.join(CATALOG_METADATA_FILE)).ok()?)
            .ok()?;
    if metadata.metadata_version != CATALOG_METADATA_VERSION
        || metadata.catalog_hash != actual_catalog_hash
    {
        return None;
    }
    let canonical_rows = u64::try_from(metadata.canonical_rows).ok()?;
    let nt_catalog_rows = metadata
        .effective_catalog_rows_by_nt_data_type()
        .into_values()
        .try_fold(0_u64, |total, rows| {
            total.checked_add(u64::try_from(rows).ok()?)
        })?;
    Some(VerifiedCarriedOutput {
        output_dir: prior.output_dir.clone(),
        catalog_hash: actual_catalog_hash,
        canonical_rows,
        nt_catalog_rows,
    })
}

#[cfg(test)]
fn carried_output_still_verifies(prior: &SourceUniverseBatchExecutionRecord) -> bool {
    verified_carried_output(prior).is_some()
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
    ensure!(
        prior.schema_version == SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION,
        "resume report {} schema_version mismatch: expected {}, got {}",
        resume_report.display(),
        SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION,
        prior.schema_version
    );
    ensure!(
        prior.pack_id == pack.pack_id,
        "resume report {} pack_id mismatch: expected {}, got {}",
        resume_report.display(),
        pack.pack_id,
        prior.pack_id
    );
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
    output_dir: &Path,
    config: &SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
) -> RecordSlot
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let (record, controls) = match work_item {
        // Current identity always comes from the admitted pack. Only output
        // facts re-derived from verified prior catalog metadata carry forward.
        BatchWorkItem::Carried { record, output } => {
            return RecordSlot::Completed(SourceUniverseBatchExecutionRecord {
                sequence: record.sequence,
                operator_run_id: record.operator_run_id.clone(),
                source_binding: record.source_binding.clone(),
                category: record.category.clone(),
                symbol: record.symbol.clone(),
                archive_date: record.archive_date.clone(),
                selected_object_sha256: record.selected_object_sha256.clone(),
                selected_object_bytes: record.selected_object_bytes,
                execution_plan_sha256: record.execution_plan_sha256.clone(),
                canonical_rows: output.canonical_rows,
                nt_catalog_rows: output.nt_catalog_rows,
                catalog_hash: output.catalog_hash.clone(),
                output_dir: output.output_dir.clone(),
            });
        }
        BatchWorkItem::NeedsWork { record, controls } => (*record, *controls),
    };

    let object_bytes = match fetcher
        .fetch(record)
        .with_context(|| format!("fetch source object for {}", record.operator_run_id))
    {
        Ok(object_bytes) => object_bytes,
        Err(error) => return record_error_slot(record, "fetch", error, config),
    };
    if let Err(error) = verify_object(record, &object_bytes)
        .with_context(|| format!("verify source object for {}", record.operator_run_id))
    {
        return record_error_slot(record, "verify_object", error, config);
    }

    let record_output_dir = output_dir.join(&record.operator_run_id);
    let run_output = match runner
        .run(record, &object_bytes, controls, &record_output_dir)
        .with_context(|| format!("run operator {}", record.operator_run_id))
    {
        Ok(run_output) => run_output,
        Err(error) => return record_error_slot(record, "run_operator", error, config),
    };

    RecordSlot::Completed(SourceUniverseBatchExecutionRecord {
        sequence: record.sequence,
        operator_run_id: record.operator_run_id.clone(),
        source_binding: record.source_binding.clone(),
        category: record.category.clone(),
        symbol: record.symbol.clone(),
        archive_date: record.archive_date.clone(),
        selected_object_sha256: record.selected_object_sha256.clone(),
        selected_object_bytes: record.selected_object_bytes,
        execution_plan_sha256: record.execution_plan_sha256.clone(),
        canonical_rows: run_output.canonical_rows,
        nt_catalog_rows: run_output.nt_catalog_rows,
        catalog_hash: run_output.catalog_hash,
        output_dir: record_output_dir,
    })
}

fn record_error_slot(
    record: &SourceUniverseExecutionPackRecord,
    failure_stage: &str,
    error: anyhow::Error,
    config: &SourceUniverseBatchExecutionConfig,
) -> RecordSlot {
    if config.continue_on_error {
        RecordSlot::Failed(failure_record(record, failure_stage, &error))
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
                total_canonical_rows = total_canonical_rows.saturating_add(record.canonical_rows);
                total_nt_catalog_rows =
                    total_nt_catalog_rows.saturating_add(record.nt_catalog_rows);
                records.push(record);
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

    let pack = &owned_plan.pack;
    Ok(SourceUniverseBatchExecutionReport {
        schema_version: SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION.to_string(),
        batch_id: batch_id.to_string(),
        status,
        pack_id: pack.pack_id.clone(),
        universe_id: pack.universe_id.clone(),
        venue: pack.venue.clone(),
        selected_record_count: records.len().saturating_add(failures.len()) as u64,
        completed_record_count: records.len() as u64,
        failed_record_count: failures.len() as u64,
        total_canonical_rows,
        total_nt_catalog_rows,
        records,
        failures,
    })
}

pub fn write_source_universe_batch_execution_report(
    output_dir: &Path,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
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

    fn committed_run_spec() -> Arc<RunSpec> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root")
            .join(
                "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs/\
                 binance-data-vision-trades-2026-03-01-all-instruments/execution-pack/runs/\
                 00000-source-universe-operator-run-binance-data-vision-trades-2026-03-01-all-instruments-00000/\
                 run-spec.toml",
            );
        let text = fs::read_to_string(path).expect("read committed run spec");
        Arc::new(toml::from_str(&text).expect("parse committed run spec"))
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

    fn copy_reference_output(output_dir: &Path) {
        copy_dir_all(
            &committed_reference_run_dir().join(CATALOG_DIR),
            &output_dir.join(CATALOG_DIR),
        );
        fs::copy(
            committed_reference_run_dir().join(CATALOG_METADATA_FILE),
            output_dir.join(CATALOG_METADATA_FILE),
        )
        .expect("copy committed catalog metadata");
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
            selected_object_bytes: 0,
            execution_plan_sha256: "a".repeat(64),
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
            run_spec_sha256: "run-spec-sha".to_string(),
            accepted_tranche_path: PathBuf::from("tranche.json"),
            accepted_tranche_sha256: "tranche-sha".to_string(),
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
        let mut resume_records = BTreeMap::new();
        resume_records.insert(prior.sequence, prior.clone());
        let run_spec = committed_run_spec();
        let accepted_object_sha256 = run_spec.accepted_object.sha256.clone();
        let run_spec_execution =
            validate_run_spec_execution(run_spec, &prior.output_dir, &accepted_object_sha256)
                .expect("validate committed run spec for owned plan fixture");
        let admitted_controls = BTreeMap::from([(
            prior.sequence,
            SourceUniverseAdmittedControls {
                run_spec_bytes: Arc::from([]),
                accepted_tranche_bytes: Arc::from([]),
                execution_plan_bytes: Arc::from([]),
                run_spec_execution,
            },
        )]);
        OwnedBatchPlan {
            pack,
            admitted_controls,
            resume_records,
            start_sequence: None,
            record_limit: usize::MAX,
        }
    }

    #[test]
    fn carried_output_verifies_against_intact_reference_catalog() {
        // Positive control: a carried record whose prior output catalog is intact
        // and whose carried hash matches the catalog re-verifies, so the gate is
        // not vacuously rejecting every record.
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_reference_output(&output_dir);
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
        copy_reference_output(&output_dir);
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
                plan.work_items.as_slice(),
                [BatchWorkItem::NeedsWork { .. }]
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
        copy_reference_output(&output_dir);
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        let owned_plan = owned_plan_with_carry(&record);
        let plan = owned_plan.plan();
        assert!(
            matches!(plan.work_items.as_slice(), [BatchWorkItem::Carried { .. }]),
            "an intact, hash-matching prior catalog must still carry forward"
        );
    }

    #[test]
    fn plan_reexecutes_carried_record_when_execution_plan_changes() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_reference_output(&output_dir);
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        let mut owned_plan = owned_plan_with_carry(&record);
        owned_plan.pack.records[0].execution_plan_sha256 = "b".repeat(64);

        let plan = owned_plan.plan();

        assert!(
            matches!(
                plan.work_items.as_slice(),
                [BatchWorkItem::NeedsWork { .. }]
            ),
            "a changed admitted execution plan must force re-execution even when the object and prior output still verify"
        );
    }

    #[test]
    fn plan_reexecutes_carried_record_when_selected_object_changes() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let output_dir = temp_dir.path().join("operator-run-carried");
        copy_reference_output(&output_dir);
        let record = carried_record_with_output(output_dir, committed_reference_catalog_hash());
        let mut owned_plan = owned_plan_with_carry(&record);
        owned_plan.pack.records[0].selected_object_sha256 = "b".repeat(64);

        let plan = owned_plan.plan();

        assert!(
            matches!(
                plan.work_items.as_slice(),
                [BatchWorkItem::NeedsWork { .. }]
            ),
            "a changed selected object must force re-execution even when the plan and prior output still verify"
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
