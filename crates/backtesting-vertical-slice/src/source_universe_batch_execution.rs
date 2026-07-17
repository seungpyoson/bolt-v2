//! Batch execution for source-universe single-object operator runs.
//!
//! Source-universe execution packs already materialize one run-spec and
//! execution plan per accepted object. This module adds the missing operator
//! loop: fetch the pinned object through one typed verification boundary, run
//! the existing single-object operator path, and summarize completed records.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize, ser::SerializeSeq};
use sha2::{Digest, Sha256};

#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{FileExt, OpenOptionsExt};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "linux")]
use std::{
    io::Read,
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc::{RecvTimeoutError, sync_channel},
    time::Instant,
};

use crate::atomic_artifact_write::{
    OwnedTempDirectory, atomic_file_create_or_verify_guarded,
    compact_owned_temp_directory_to_receipt_bounded, create_owned_temp_directory_guarded,
    initialize_owned_temp_directory_receipt_guarded, open_pinned_regular_file,
    unique_temp_path_guarded,
};
use crate::backfill_execution_plan::{
    BackfillExecutionPlan, ValidatedBackfillExecutionControls,
    validate_backfill_execution_control_bytes,
};
#[cfg(any(target_os = "linux", test))]
use crate::operator_work_budget::OperatorWorkBudgetDeadline;
use crate::operator_work_budget::{
    CooperativeDeadlineWriter, ExactSizedObjectBuffer, OperatorWorkBudgetGuard,
    OperatorWorkBudgetStage, guarded_async_operation_outcome, guarded_operation_outcome,
    read_exact_sized_hashed_pinned_file_guarded, read_exact_sized_pinned_file_guarded,
    sha256_exact_sized_open_file_guarded, sha256_hex_with_budget,
    system_operator_work_budget_clock,
};
use crate::path_resolution::{
    resolve_contained_output_component, resolve_pack_control_path, validate_portable_path_component,
};
use crate::pinned_regular_file::{PinnedRegularFileFingerprint, read_exact_pinned_file};
use crate::reference_artifact::ReferenceArtifactPin;
use crate::retired_backfill_evidence::ensure_active_backfill_runtime_path;
use crate::source_universe_local_storage::{
    SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES, SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE,
    SourceUniverseLifecycleCleanupLimits, SourceUniverseLocalStorageLease,
    SourceUniverseLocalStoragePolicy,
};
use crate::{
    operator::{
        DurableCompletionLocator, DurableExecutionAttestation, DurableExecutionProvenance,
        DurableOutputCandidateSealProbe, DurableRunDispatcher, DurableRunReceipt,
        OperatorRunSummary, RunSpec, VerifiedSourceBindingRegistry,
        probe_durable_output_candidate_seal_summary_capped, validate_durable_run_spec_preflight,
        validate_run_spec_manifest_for_object_hash_with_verified_registry,
    },
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackRecord,
        SourceUniverseExecutionPackStatus, validate_execution_pack_semantics,
    },
};

pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION: &str =
    "source-universe-batch-execution-report.v9";
pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE: &str =
    "source-universe-batch-execution-report.json";

pub const SOURCE_UNIVERSE_OPERATOR_WORKER_MODE: &str = "__source-universe-operator-worker";
pub const SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT: &str =
    ".source-universe-operator-worker-requests";
#[cfg(target_os = "linux")]
const SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_SCHEMA_VERSION: &str =
    "source-universe-operator-worker-request.v6";
#[cfg(target_os = "linux")]
const WORKER_REQUEST_ROLE_ACCEPTED_TRANCHE: &str = "accepted_tranche";
#[cfg(target_os = "linux")]
const WORKER_REQUEST_ROLE_EXECUTION_PLAN: &str = "execution_plan";
#[cfg(target_os = "linux")]
const WORKER_REQUEST_ROLE_RUN_SPEC: &str = "run_spec";
#[cfg(target_os = "linux")]
const WORKER_REQUEST_ROLE_SELECTED_OBJECT: &str = "selected_object";
#[cfg(target_os = "linux")]
const WORKER_REQUEST_ROLE_SOURCE_BINDINGS: &str = "source_bindings";
#[cfg(target_os = "linux")]
const WORKER_REQUEST_ROLES: [&str; 5] = [
    WORKER_REQUEST_ROLE_ACCEPTED_TRANCHE,
    WORKER_REQUEST_ROLE_EXECUTION_PLAN,
    WORKER_REQUEST_ROLE_RUN_SPEC,
    WORKER_REQUEST_ROLE_SELECTED_OBJECT,
    WORKER_REQUEST_ROLE_SOURCE_BINDINGS,
];

fn worker_request_archive_header_bytes() -> Result<u64> {
    u64::try_from(std::mem::size_of::<u64>())
        .context("worker request header width does not fit u64")
}

/// Operator-supplied identity for a launch artifact. The executor opens the
/// path without following symlinks and accepts only this exact length/hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchArtifactPin {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[cfg(test)]
mod source_universe_batch_tests {
    use crate as backtesting_vertical_slice;

    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/source_universe_batch_execution_tests.rs"
    ));
}

impl SourceUniverseBatchArtifactPin {
    pub fn try_new(path: PathBuf, bytes: u64, sha256: String) -> Result<Self> {
        ensure!(bytes > 0, "launch artifact byte length must be positive");
        validate_sha256_hex(&sha256).context("validate launch artifact SHA-256")?;
        Ok(Self {
            path,
            bytes,
            sha256,
        })
    }

    #[cfg(test)]
    fn pin_current_path(path: &Path, max_bytes: u64) -> Result<Self> {
        ensure!(
            max_bytes > 0,
            "launch artifact maximum byte length must be positive"
        );
        let (mut file, identity) = open_pinned_regular_file(path)?;
        ensure!(
            identity.byte_len <= max_bytes,
            "launch artifact {} declared byte length {} exceeds configured maximum {max_bytes}",
            path.display(),
            identity.byte_len
        );
        let bytes = read_exact_pinned_file(&mut file, path, identity.byte_len)?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        identity.revalidate_path(path)?;
        identity.revalidate_handle(path, &file)?;
        Self::try_new(path.to_path_buf(), identity.byte_len, sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchLaunchArtifacts {
    execution_pack: SourceUniverseBatchArtifactPin,
    bootstrap_limits: SourceUniverseBatchBootstrapLimits,
}

/// Trusted pre-parse limits loaded from the batch launch TOML. These limits
/// are independent of the execution pack and therefore apply before any pack
/// or pack-referenced control can choose an allocation size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchBootstrapLimits {
    pub max_launch_artifact_bytes: u64,
    pub max_control_artifact_bytes: u64,
    /// Aggregate encoded-input envelope for all retained raw controls plus the
    /// typed control values derived from them for the selected batch window.
    pub max_retained_control_input_bytes: u64,
}

/// TOML-owned hard resource ceiling for the same-binary worker process.
/// Cooperative decoded-byte accounting remains useful diagnostics, while the
/// kernel address-space limit is the final aggregate peak-memory authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchResourceLimits {
    pub worker_max_virtual_memory_bytes: u64,
    /// TOML-owned allowance for worker executable mappings, allocator state,
    /// runtime stacks, and memory outside the selected object plus the
    /// execution plan's decoded-byte ceiling.
    pub worker_reserved_overhead_bytes: u64,
}

impl SourceUniverseBatchResourceLimits {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            self.worker_max_virtual_memory_bytes > 0,
            "resource_limits.worker_max_virtual_memory_bytes must be positive"
        );
        ensure!(
            self.worker_max_virtual_memory_bytes != u64::MAX,
            "resource_limits.worker_max_virtual_memory_bytes must be finite"
        );
        ensure!(
            self.worker_reserved_overhead_bytes > 0,
            "resource_limits.worker_reserved_overhead_bytes must be positive"
        );
        ensure!(
            self.worker_reserved_overhead_bytes != u64::MAX,
            "resource_limits.worker_reserved_overhead_bytes must be finite"
        );
        Ok(self)
    }
}

impl SourceUniverseBatchBootstrapLimits {
    /// Validate the trusted launch envelope before resolving or opening any
    /// pack-selected path.
    pub fn validate(self) -> Result<Self> {
        ensure!(
            self.max_launch_artifact_bytes > 0,
            "bootstrap_limits.max_launch_artifact_bytes must be positive"
        );
        ensure!(
            self.max_control_artifact_bytes > 0,
            "bootstrap_limits.max_control_artifact_bytes must be positive"
        );
        ensure!(
            self.max_retained_control_input_bytes > 0,
            "bootstrap_limits.max_retained_control_input_bytes must be positive"
        );
        ensure!(
            self.max_control_artifact_bytes <= self.max_retained_control_input_bytes,
            "bootstrap_limits.max_control_artifact_bytes must not exceed max_retained_control_input_bytes"
        );
        Ok(self)
    }
}

impl SourceUniverseBatchLaunchArtifacts {
    /// Construct the production launch boundary from operator-selected exact
    /// execution-pack identity plus an independent ceiling applied before
    /// filesystem access or allocation.
    pub fn try_new(
        execution_pack: SourceUniverseBatchArtifactPin,
        bootstrap_limits: SourceUniverseBatchBootstrapLimits,
    ) -> Result<Self> {
        let bootstrap_limits = bootstrap_limits.validate()?;
        ensure!(
            execution_pack.bytes <= bootstrap_limits.max_launch_artifact_bytes,
            "execution pack {} declared byte length {} exceeds configured maximum {}",
            execution_pack.path.display(),
            execution_pack.bytes,
            bootstrap_limits.max_launch_artifact_bytes
        );
        Ok(Self {
            execution_pack,
            bootstrap_limits,
        })
    }
}

fn read_launch_artifact(
    pin: &SourceUniverseBatchArtifactPin,
    max_artifact_bytes: u64,
) -> Result<Vec<u8>> {
    validate_sha256_hex(&pin.sha256).context("validate launch artifact SHA-256")?;
    ensure!(
        max_artifact_bytes > 0,
        "launch artifact maximum byte length must be positive"
    );
    ensure!(
        pin.bytes > 0,
        "launch artifact {} must declare a positive byte length",
        pin.path.display()
    );
    ensure!(
        pin.bytes <= max_artifact_bytes,
        "launch artifact {} declared byte length {} exceeds configured maximum {max_artifact_bytes}",
        pin.path.display(),
        pin.bytes
    );
    let (mut file, identity) = open_pinned_regular_file(&pin.path)?;
    ensure!(
        identity.byte_len == pin.bytes,
        "launch artifact {} length mismatch: expected {}, got {}",
        pin.path.display(),
        pin.bytes,
        identity.byte_len
    );
    let bytes = read_exact_pinned_file(&mut file, &pin.path, pin.bytes)?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    ensure!(
        actual_sha256 == pin.sha256,
        "launch artifact {} SHA-256 mismatch: expected {}, got {}",
        pin.path.display(),
        pin.sha256,
        actual_sha256
    );
    identity.revalidate_path(&pin.path)?;
    identity.revalidate_handle(&pin.path, &file)?;
    Ok(bytes)
}

/// Selected-object bytes whose execution-pack length and SHA-256 identity have
/// already been proven at the fetch boundary.
///
/// The byte buffer is intentionally opaque: production fetchers can create it
/// only by performing the one guarded verification traversal, while the cache
/// can preserve an equivalent pinned-file read proof without hashing again.
pub struct VerifiedSourceObject {
    bytes: Vec<u8>,
    selected_object_bytes: u64,
    selected_object_sha256: String,
}

impl fmt::Debug for VerifiedSourceObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedSourceObject")
            .field("selected_object_bytes", &self.selected_object_bytes)
            .field("selected_object_sha256", &self.selected_object_sha256)
            .finish_non_exhaustive()
    }
}

impl VerifiedSourceObject {
    /// Bind owned bytes to one execution-pack record after guarded length and
    /// SHA-256 verification. This is the sole constructor available to new
    /// source transports; cache-proof reuse remains private.
    ///
    /// # Errors
    ///
    /// Returns an error when the work budget expires or the bytes do not match
    /// the record's selected-object length and digest.
    pub fn verify(
        record: &SourceUniverseExecutionPackRecord,
        bytes: Vec<u8>,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Self> {
        verify_object_guarded(record, &bytes, work_budget)?;
        Ok(Self {
            bytes,
            selected_object_bytes: record.selected_object_bytes,
            selected_object_sha256: record.selected_object_sha256.clone(),
        })
    }

    fn from_verified_cache_read(
        record: &SourceUniverseExecutionPackRecord,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            bytes,
            selected_object_bytes: record.selected_object_bytes,
            selected_object_sha256: record.selected_object_sha256.clone(),
        }
    }

    /// Borrow the proven bytes without weakening their construction boundary.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn ensure_matches(&self, record: &SourceUniverseExecutionPackRecord) -> Result<()> {
        ensure!(
            self.selected_object_bytes == record.selected_object_bytes
                && self.selected_object_sha256.as_str() == record.selected_object_sha256.as_str(),
            "verified source-object proof does not match execution-pack record {}",
            record.operator_run_id
        );
        ensure!(
            u64::try_from(self.bytes.len())
                .context("verified source-object length does not fit u64")?
                == self.selected_object_bytes,
            "verified source-object bytes no longer match their proof"
        );
        Ok(())
    }

    fn into_bytes_for(self, record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>> {
        self.ensure_matches(record)?;
        Ok(self.bytes)
    }
}

pub trait SourceUniverseObjectFetcher {
    fn fetch(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        run_spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<VerifiedSourceObject>;
}

struct LazySourceUniverseObjectFetcher<'a, F, C> {
    inner: &'a mut Option<F>,
    factory: &'a C,
}

impl<F, C> SourceUniverseObjectFetcher for LazySourceUniverseObjectFetcher<'_, F, C>
where
    F: SourceUniverseObjectFetcher,
    C: Fn() -> Result<F>,
{
    fn fetch(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        run_spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<VerifiedSourceObject> {
        if self.inner.is_none() {
            *self.inner = Some((self.factory)().context("construct batch worker fetcher")?);
        }
        self.inner
            .as_mut()
            .expect("lazy fetcher initialized")
            .fetch(record, run_spec, work_budget)
    }
}

trait SourceUniverseOperatorRunner {
    /// Consume the verified selected-object allocation. Process-isolated
    /// runners must release it after sealing their anonymous request archive
    /// and before spawning a child, so parent and child never retain full
    /// selected-object buffers at the same time.
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        object_bytes: Vec<u8>,
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<SourceUniverseOperatorRunOutcome>;

    /// Probe the deterministic current durable terminal before source fetch.
    /// `Ok(None)` is reserved for a genuine remote NotFound; a returned receipt
    /// was pinned to a non-null current version and fully exact-version
    /// validated by the runner.
    fn discover_current_completion(
        &mut self,
        _record: &SourceUniverseExecutionPackRecord,
        _control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        _output_dir: &Path,
        _work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Option<SourceUniverseDiscoveredRunReceipt>> {
        Ok(None)
    }
}

/// One exact immutable terminal discovered by the worker used for this
/// attempt. The attempt worker may be newer than the immutable publisher
/// attested inside `receipt`.
struct SourceUniverseDiscoveredRunReceipt {
    attempt_worker_sha256: String,
    receipt: DurableRunReceipt,
}

/// Clock-only injection seam for deterministic batch deadline tests.
///
/// The batch executor, not the caller, always constructs the guard from the
/// validated execution plan. Consequently an injected clock cannot substitute
/// the planless compatibility mode or otherwise weaken production limits.
trait SourceUniverseWorkBudgetClockFactory: Sync {
    fn create_clock(&self) -> Arc<dyn crate::operator_work_budget::OperatorWorkBudgetClock>;
}

#[derive(Debug, Default, Clone, Copy)]
struct SystemSourceUniverseWorkBudgetClockFactory;

impl SourceUniverseWorkBudgetClockFactory for SystemSourceUniverseWorkBudgetClockFactory {
    fn create_clock(&self) -> Arc<dyn crate::operator_work_budget::OperatorWorkBudgetClock> {
        system_operator_work_budget_clock()
    }
}

/// Exact control-artifact bytes verified against an execution-pack record.
///
/// Runners consume these bytes instead of reopening mutable source paths after
/// a potentially long object fetch. `Arc` keeps shared tranche/plan content
/// cheap across selected records and parallel workers.
#[derive(Debug, Clone)]
struct SourceUniverseVerifiedControlArtifacts {
    #[cfg(test)]
    pub run_spec_path: PathBuf,
    pub run_spec_bytes: Arc<[u8]>,
    pub run_spec: Arc<RunSpec>,
    #[cfg(test)]
    pub accepted_tranche_path: PathBuf,
    pub accepted_tranche_bytes: Arc<[u8]>,
    #[cfg(test)]
    pub execution_plan_path: PathBuf,
    pub execution_plan_bytes: Arc<[u8]>,
    pub execution_plan: Arc<BackfillExecutionPlan>,
    pub source_bindings_path: PathBuf,
    pub source_bindings_bytes: Arc<[u8]>,
    pub source_bindings_sha256: String,
    pub source_bindings: VerifiedSourceBindingRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceUniverseBatchExecutionRunOutput {
    canonical_rows: u64,
    nt_catalog_rows: u64,
    catalog_hash: String,
}

impl SourceUniverseBatchExecutionRunOutput {
    /// Construct a non-terminal test/injected-runner result. Production's
    /// sealed local runner consumes an operator-owned pre-commit summary.
    #[cfg(test)]
    fn try_new(canonical_rows: u64, nt_catalog_rows: u64, catalog_hash: String) -> Result<Self> {
        validate_sha256_hex(&catalog_hash).context("validate operator run output catalog_hash")?;
        Ok(Self {
            canonical_rows,
            nt_catalog_rows,
            catalog_hash,
        })
    }

    #[cfg(test)]
    #[must_use]
    const fn canonical_rows(&self) -> u64 {
        self.canonical_rows
    }

    fn from_summary(summary: crate::operator::OperatorRunSummary) -> Self {
        Self {
            canonical_rows: summary.canonical_rows,
            nt_catalog_rows: summary.nt_catalog_rows,
            catalog_hash: summary.catalog_hash,
        }
    }
}

/// Opaque committed result. Production constructs it only after a successful
/// quiesced child, canonical local candidate, and exact durable receipt agree.
#[derive(Debug)]
struct SourceUniverseCommittedRunReceipt {
    output: SourceUniverseBatchExecutionRunOutput,
    worker_executable_sha256: String,
    durable_completion: DurableCompletionLocator,
}

/// Runner outcome distinguishes ordinary injected/test results, which remain
/// postchecked, from the process-isolated operator's committed receipt.
#[derive(Debug)]
enum SourceUniverseOperatorRunOutcome {
    #[cfg(test)]
    NonTerminal(SourceUniverseBatchExecutionRunOutput),
    Committed(SourceUniverseCommittedRunReceipt),
}

/// A worker terminated without a trustworthy answer about terminal ownership.
/// Retrying this record could publish a second attempt beside an already
/// committed output, so this error always stops the complete batch even when
/// ordinary per-record failures are configured to continue.
#[derive(Debug)]
struct CommittedIndeterminateWorkerError {
    detail: String,
}

impl fmt::Display for CommittedIndeterminateWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "worker terminal state is committed-indeterminate; automatic retry is forbidden: {}",
            self.detail
        )
    }
}

impl std::error::Error for CommittedIndeterminateWorkerError {}

fn committed_indeterminate_worker_error(detail: impl Into<String>) -> anyhow::Error {
    CommittedIndeterminateWorkerError {
        detail: detail.into(),
    }
    .into()
}

fn is_committed_indeterminate_worker_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<CommittedIndeterminateWorkerError>()
            .is_some()
    })
}

fn validate_worker_durable_receipt(
    bytes: &[u8],
    spec: &RunSpec,
    summary: &OperatorRunSummary,
    expected_worker_executable_sha256: &str,
) -> Result<DurableRunReceipt> {
    ensure!(
        !bytes.is_empty(),
        "source-universe worker did not return a durable completion receipt"
    );
    let response: SourceUniverseOperatorWorkerResponse =
        serde_json::from_slice(bytes).context("parse source-universe worker durable response")?;
    ensure!(
        crate::reference_artifact::canonical_json_bytes(&response)? == bytes,
        "source-universe worker durable response bytes are not canonical"
    );
    let SourceUniverseOperatorWorkerResponse::Completed { receipt } = response else {
        bail!("source-universe execute worker returned no current completion")
    };
    validate_durable_receipt(&receipt, spec, summary)?;
    ensure!(
        receipt.execution_attestation.provenance
            == DurableExecutionProvenance::ExecutedProcessIsolated
            && receipt.execution_attestation.worker_executable_sha256
                == expected_worker_executable_sha256,
        "source-universe worker durable receipt execution attestation disagrees with the sealed process-isolated worker"
    );
    Ok(receipt)
}

fn validate_worker_completion_discovery(
    bytes: &[u8],
    spec: &RunSpec,
) -> Result<Option<(DurableRunReceipt, OperatorRunSummary)>> {
    ensure!(
        !bytes.is_empty(),
        "source-universe discovery worker did not return a response"
    );
    let response: SourceUniverseOperatorWorkerResponse =
        serde_json::from_slice(bytes).context("parse source-universe discovery response")?;
    ensure!(
        crate::reference_artifact::canonical_json_bytes(&response)? == bytes,
        "source-universe discovery response bytes are not canonical"
    );
    match response {
        SourceUniverseOperatorWorkerResponse::NoCurrentCompletion => Ok(None),
        SourceUniverseOperatorWorkerResponse::Completed { receipt } => {
            let summary = durable_receipt_summary(&receipt, spec)?;
            Ok(Some((receipt, summary)))
        }
    }
}

fn durable_receipt_summary(
    receipt: &DurableRunReceipt,
    spec: &RunSpec,
) -> Result<OperatorRunSummary> {
    receipt.completion.validate()?;
    receipt
        .execution_attestation
        .validate()
        .context("validate source-universe worker durable execution attestation")?;
    ensure!(
        receipt.run_id == spec.manifest.run_id
            && receipt.submitted_manifest_hash == spec.manifest.manifest_hash(),
        "source-universe worker durable receipt submitted-run identity mismatch"
    );
    ensure!(
        receipt.canonical_rows > 0 && receipt.nt_catalog_rows == receipt.canonical_rows,
        "source-universe worker durable receipt row summary is invalid"
    );
    validate_sha256_hex(&receipt.catalog_hash)
        .context("validate source-universe worker durable receipt catalog_hash")?;
    Ok(OperatorRunSummary {
        canonical_rows: receipt.canonical_rows,
        nt_catalog_rows: receipt.nt_catalog_rows,
        catalog_hash: receipt.catalog_hash.clone(),
    })
}

fn validate_durable_receipt(
    receipt: &DurableRunReceipt,
    spec: &RunSpec,
    summary: &OperatorRunSummary,
) -> Result<()> {
    let receipt_summary = durable_receipt_summary(receipt, spec)?;
    ensure!(
        receipt_summary == *summary,
        "source-universe worker durable receipt disagrees with the durable local output candidate"
    );
    Ok(())
}

fn accept_quiesced_durable_worker(
    worker_result: Result<WorkerExitEvidence>,
    candidate_probe: Result<DurableOutputCandidateSealProbe>,
    worker_executable_sha256: String,
    validate_receipt: impl FnOnce(&[u8], &OperatorRunSummary) -> Result<DurableRunReceipt>,
    validate_final_local_state: impl FnOnce() -> Result<()>,
) -> Result<SourceUniverseOperatorRunOutcome> {
    let worker_evidence = match worker_result {
        Ok(evidence) if evidence.status.success() => evidence,
        Ok(evidence) => {
            return Err(committed_indeterminate_worker_error(format!(
                "source-universe worker exited unsuccessfully with status {}; a local candidate cannot prove remote terminal publication",
                evidence.status
            )));
        }
        Err(error) => {
            return Err(committed_indeterminate_worker_error(format!(
                "source-universe worker wait failed after start ({error:#}); remote terminal publication cannot be excluded"
            )));
        }
    };
    let summary = match candidate_probe {
        Ok(DurableOutputCandidateSealProbe::Candidate(summary)) => summary,
        Ok(DurableOutputCandidateSealProbe::Absent) => {
            return Err(committed_indeterminate_worker_error(
                "worker exited successfully but its durable local output candidate seal is absent; remote publication side effects cannot be excluded",
            ));
        }
        Err(error) => {
            return Err(committed_indeterminate_worker_error(format!(
                "durable local output candidate-seal probe failed after successful child exit: {error:#}"
            )));
        }
    };
    let durable_receipt = validate_receipt(&worker_evidence.receipt_bytes, &summary).map_err(
        |error| {
            committed_indeterminate_worker_error(format!(
                "durable local output candidate exists but the returned exact durable receipt is invalid: {error:#}"
            ))
        },
    )?;
    validate_final_local_state().map_err(|error| {
        committed_indeterminate_worker_error(format!(
            "output lease changed while accepting durable candidate and receipt: {error:#}"
        ))
    })?;
    durable_receipt
        .execution_attestation
        .validate()
        .map_err(|error| {
            committed_indeterminate_worker_error(format!(
                "durable receipt execution attestation is invalid: {error:#}"
            ))
        })?;
    if durable_receipt
        .execution_attestation
        .worker_executable_sha256
        != worker_executable_sha256
    {
        return Err(committed_indeterminate_worker_error(
            "durable receipt worker executable SHA-256 disagrees with the sealed process-isolated worker",
        ));
    }
    Ok(SourceUniverseOperatorRunOutcome::Committed(
        SourceUniverseCommittedRunReceipt {
            output: SourceUniverseBatchExecutionRunOutput::from_summary(summary),
            worker_executable_sha256: durable_receipt
                .execution_attestation
                .worker_executable_sha256,
            durable_completion: durable_receipt.completion,
        },
    ))
}

fn require_quiesced_worker_lifecycle(
    lifecycle: WorkerLifecycleOutcome,
) -> Result<Result<WorkerExitEvidence>> {
    match lifecycle {
        WorkerLifecycleOutcome::NotStarted(error) => {
            Err(error.context("source-universe operator worker was not started"))
        }
        WorkerLifecycleOutcome::Quiesced(result) => Ok(result),
        WorkerLifecycleOutcome::Indeterminate(error) => Err(committed_indeterminate_worker_error(
            format!("worker process group is not proven quiesced and reaped: {error:#}"),
        )),
    }
}

/// Tuning for a source-universe batch execution run.
///
/// Every field beyond the original three is opt-in: the default
/// `max_concurrent_records: None` reproduces the original serial behavior.
/// Object caching is deliberately NOT a config field: the one way to enable it
/// is wrapping the fetcher in [`CachingSourceUniverseObjectFetcher`] (the CLI
/// does this for the TOML-owned `object_cache_dir`), so a cache can never be
/// requested without taking effect.
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
}

#[cfg(test)]
pub(crate) const TEST_SOURCE_UNIVERSE_LIFECYCLE_CLEANUP_LIMITS:
    SourceUniverseLifecycleCleanupLimits = SourceUniverseLifecycleCleanupLimits {
    max_entries: 100_000,
    max_depth: 64,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseBatchExecutionReportStatus {
    Completed,
    CompletedWithFailures,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseBatchExecutionRecordProvenance {
    ExecutedProcessIsolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseBatchExecutionCompletionResolution {
    Published,
    Discovered,
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
    pub completion_provenance: SourceUniverseBatchExecutionRecordProvenance,
    pub completion_resolution: SourceUniverseBatchExecutionCompletionResolution,
    pub attempt_worker_sha256: String,
    pub terminal_publisher_worker_sha256: String,
    pub durable_completion: Option<DurableCompletionLocator>,
}

/// Exact local identity of an owned output attempt retained after a
/// post-claim failure. This is evidence for an independently governed,
/// fd-relative lifecycle process; runtime execution never deletes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionAttemptIdentity {
    pub output_dir: PathBuf,
    pub device: Option<u64>,
    pub inode: Option<u64>,
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
    pub attempt_output: Option<SourceUniverseBatchExecutionAttemptIdentity>,
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

/// A process-isolated batch report published through the same pinned output
/// root capability that governed execution.
#[derive(Debug)]
pub struct SourceUniversePublishedBatchExecution {
    pub report: SourceUniverseBatchExecutionReport,
    pub artifact: SourceUniverseBatchExecutionReportArtifact,
}

/// Validate a batch report as evidence rather than trusting its aggregate
/// counters. This is the single consume boundary used by report publication
/// and conversion-completion reconciliation.
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
    let mut attempt_outputs = BTreeSet::new();
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
            record.completion_provenance
                == SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated,
            "batch execution report completion provenance must be executed_process_isolated"
        );
        validate_sha256_hex(&record.attempt_worker_sha256)
            .context("validate completed record attempt_worker_sha256")?;
        validate_sha256_hex(&record.terminal_publisher_worker_sha256)
            .context("validate completed record terminal_publisher_worker_sha256")?;
        if record.completion_resolution
            == SourceUniverseBatchExecutionCompletionResolution::Published
        {
            ensure!(
                record.attempt_worker_sha256 == record.terminal_publisher_worker_sha256,
                "batch execution report freshly published completion must bind the attempt worker as terminal publisher"
            );
        }
        record
            .durable_completion
            .as_ref()
            .context("batch execution report completed record is missing durable_completion")?
            .validate()
            .context("validate completed record durable_completion")?;
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
        if let Some(attempt) = &failure.attempt_output {
            ensure!(
                attempt.output_dir.is_absolute(),
                "batch execution report failure attempt output must be absolute: {}",
                attempt.output_dir.display()
            );
            ensure!(
                attempt.device.is_some() == attempt.inode.is_some(),
                "batch execution report failure attempt device/inode must both be present or both be absent"
            );
            ensure!(
                attempt_outputs.insert(attempt.output_dir.clone()),
                "batch execution report has duplicate failure attempt output {}",
                attempt.output_dir.display()
            );
        }
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
    fetch_timeout: Option<Duration>,
}

impl HttpSourceUniverseObjectFetcher {
    pub fn new(fetch_timeout_seconds: Option<u64>, http_user_agent: Option<&str>) -> Result<Self> {
        let mut client_builder =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        let fetch_timeout = fetch_timeout_seconds.map(Duration::from_secs);
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
        Ok(Self {
            client,
            runtime,
            fetch_timeout,
        })
    }
}

fn effective_http_request_timeout(
    configured: Option<Duration>,
    remaining: Option<Duration>,
) -> Option<Duration> {
    match (configured, remaining) {
        (Some(configured), Some(remaining)) => Some(configured.min(remaining)),
        (Some(configured), None) => Some(configured),
        (None, Some(remaining)) => Some(remaining),
        (None, None) => None,
    }
}

fn apply_http_request_timeout(
    request: reqwest::RequestBuilder,
    timeout: Option<Duration>,
) -> reqwest::RequestBuilder {
    match timeout {
        Some(timeout) => request.timeout(timeout),
        None => request,
    }
}

fn source_object_http_error(error: reqwest::Error, operation: &str) -> anyhow::Error {
    anyhow::anyhow!("{operation}: {}", error.without_url())
}

impl SourceUniverseObjectFetcher for HttpSourceUniverseObjectFetcher {
    fn fetch(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        _run_spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<VerifiedSourceObject> {
        let source_url = validated_http_source_url(&record.source_url)?;
        let client = self.client.clone();
        let remaining = work_budget.remaining_wall_time(OperatorWorkBudgetStage::Fetch)?;
        let request_timeout = effective_http_request_timeout(self.fetch_timeout, remaining);
        let request = apply_http_request_timeout(client.get(source_url), request_timeout)
            .build()
            .map_err(|error| source_object_http_error(error, "build source-object GET request"))?;
        let bytes = self.runtime.block_on(guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Fetch,
            async {
                let mut response = client.execute(request).await.map_err(|error| {
                    source_object_http_error(error, "execute source-object GET request")
                })?;
                response = response.error_for_status().map_err(|error| {
                    source_object_http_error(error, "validate source-object HTTP status")
                })?;
                if let Some(content_length) = response.content_length() {
                    ensure!(
                        content_length == record.selected_object_bytes,
                        "HTTP Content-Length {content_length} does not match pinned object size {}",
                        record.selected_object_bytes
                    );
                }
                let mut output = ExactSizedObjectBuffer::new(record.selected_object_bytes)?;
                loop {
                    let chunk = response.chunk().await.map_err(|error| {
                        source_object_http_error(error, "stream source-object response body")
                    })?;
                    let Some(chunk) = chunk else { break };
                    output.push(&chunk, work_budget, OperatorWorkBudgetStage::Fetch)?;
                }
                output.finish(work_budget, OperatorWorkBudgetStage::Fetch)
            },
        ))??;
        VerifiedSourceObject::verify(record, bytes, work_budget)
    }
}

/// Content-addressed object cache wrapping an inner fetcher.
///
/// The cache key is the execution-pack-pinned `selected_object_sha256`, so a
/// cached entry is only ever served after re-verifying its byte length and
/// hash against the record. Any existing invalid entry fails closed and remains
/// available for offline diagnosis or age-based cleanup; runtime never repairs
/// or replaces an occupied content-addressed name. Unverified inner bytes never
/// enter the cache.
pub struct CachingSourceUniverseObjectFetcher<F: SourceUniverseObjectFetcher> {
    inner: F,
    cache_dir: PathBuf,
    run_verification: SourceUniverseCacheRunVerification,
}

type VerifiedCacheRunEntrySlot = Arc<Mutex<Option<VerifiedCacheRunEntry>>>;
type VerifiedCacheRunEntryMap = Arc<Mutex<BTreeMap<String, VerifiedCacheRunEntrySlot>>>;

/// Shared per-batch cache verification state. Parallel workers lock only the
/// same content hash; different objects still verify concurrently.
#[derive(Clone, Default)]
pub struct SourceUniverseCacheRunVerification {
    entries: VerifiedCacheRunEntryMap,
    #[cfg(test)]
    hash_traversals: Arc<AtomicUsize>,
}

impl SourceUniverseCacheRunVerification {
    fn entry(&self, digest: &str) -> Result<VerifiedCacheRunEntrySlot> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("object cache run-verification map lock poisoned"))?;
        Ok(Arc::clone(
            entries
                .entry(digest.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(None))),
        ))
    }

    #[cfg(test)]
    fn hash_traversals_for_test(&self) -> usize {
        self.hash_traversals.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
struct VerifiedCacheRunEntry {
    fingerprint: PinnedRegularFileFingerprint,
    bytes: u64,
    sha256: String,
}

impl<F: SourceUniverseObjectFetcher> CachingSourceUniverseObjectFetcher<F> {
    /// Wrap `inner`, caching verified objects under `cache_dir`.
    pub fn new(inner: F, cache_dir: &Path) -> Self {
        Self::for_run(
            inner,
            cache_dir,
            SourceUniverseCacheRunVerification::default(),
        )
    }

    /// Wrap one worker with verification state shared by every worker in the
    /// same batch run.
    pub fn for_run(
        inner: F,
        cache_dir: &Path,
        run_verification: SourceUniverseCacheRunVerification,
    ) -> Self {
        Self {
            inner,
            cache_dir: cache_dir.to_path_buf(),
            run_verification,
        }
    }

    fn cache_entry_path(&self, record: &SourceUniverseExecutionPackRecord) -> PathBuf {
        self.cache_dir.join(&record.selected_object_sha256)
    }

    /// Read a cached entry and return it only if the same pinned inode, length,
    /// and digest remain valid for this run. Only an absent pathname is a miss.
    /// Any occupied-but-invalid pathname fails closed and remains untouched for
    /// offline diagnosis or an independently governed age-based janitor. The
    /// retained proof is descriptor-free.
    ///
    /// Workers can share one entry path (records are not deduplicated by sha).
    /// The first hit is hashed exactly once per run; later hits must retain the
    /// same inode and length. Absence after a proof, replacement, corruption, or
    /// mutation is an error for the whole run.
    fn read_verified_cache_entry(
        &self,
        record: &SourceUniverseExecutionPackRecord,
        cache_path: &Path,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Option<VerifiedSourceObject>> {
        let verification = self
            .run_verification
            .entry(&record.selected_object_sha256)?;
        let mut verified_this_run = verification
            .lock()
            .map_err(|_| anyhow::anyhow!("object cache entry verification lock poisoned"))?;
        match fs::symlink_metadata(cache_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ensure!(
                    verified_this_run.is_none(),
                    "object cache entry disappeared after this run verified it: {}",
                    cache_path.display()
                );
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("inspect object cache entry {}", cache_path.display())
                });
            }
        }
        let read_result = if verified_this_run.is_some() {
            read_exact_sized_pinned_file_guarded(
                cache_path,
                record.selected_object_bytes,
                work_budget,
                OperatorWorkBudgetStage::ObjectVerification,
            )
        } else {
            #[cfg(test)]
            self.run_verification
                .hash_traversals
                .fetch_add(1, Ordering::SeqCst);
            read_exact_sized_hashed_pinned_file_guarded(
                cache_path,
                record.selected_object_bytes,
                &record.selected_object_sha256,
                work_budget,
                OperatorWorkBudgetStage::ObjectVerification,
            )
        };
        let (cached, identity) = match read_result
            .with_context(|| format!("verify object cache entry {}", cache_path.display()))
        {
            Ok(verified) => verified,
            Err(error) => {
                // A deadline failure is never evidence of corruption.
                work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
                bail!(
                    "occupied object cache entry failed immutable verification (cache path retained; runtime repair prohibited): {error:#}"
                );
            }
        };
        let fingerprint = identity.fingerprint();
        if let Some(verified) = verified_this_run.as_ref() {
            ensure!(
                verified.bytes == record.selected_object_bytes
                    && verified.sha256.as_str() == record.selected_object_sha256.as_str(),
                "object cache pin changed after this run verified it"
            );
            ensure!(
                verified.fingerprint == fingerprint,
                "object cache entry identity changed after this run verified it"
            );
        } else {
            *verified_this_run = Some(VerifiedCacheRunEntry {
                fingerprint,
                bytes: record.selected_object_bytes,
                sha256: record.selected_object_sha256.clone(),
            });
        }
        Ok(Some(VerifiedSourceObject::from_verified_cache_read(
            record, cached,
        )))
    }

    /// Create one immutable content-addressed entry without replacing an
    /// occupied name. Concurrent identical writers converge after exact byte
    /// verification; a conflicting occupant is retained and rejected.
    fn store_verified(
        &self,
        cache_path: &Path,
        object_bytes: &[u8],
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("create object cache dir {}", self.cache_dir.display()))?;
        atomic_file_create_or_verify_guarded(
            cache_path,
            work_budget,
            OperatorWorkBudgetStage::Fetch,
            |file| {
                let mut writer = CooperativeDeadlineWriter::new(
                    file,
                    work_budget,
                    OperatorWorkBudgetStage::Fetch,
                );
                writer.write_all(object_bytes).with_context(|| {
                    format!(
                        "write immutable object cache entry {}",
                        cache_path.display()
                    )
                })?;
                writer.flush().with_context(|| {
                    format!(
                        "flush immutable object cache entry {}",
                        cache_path.display()
                    )
                })?;
                Ok(())
            },
        )
        .with_context(|| {
            format!(
                "create or verify immutable object cache entry {}",
                cache_path.display()
            )
        })?;
        Ok(())
    }
}

impl<F: SourceUniverseObjectFetcher> SourceUniverseObjectFetcher
    for CachingSourceUniverseObjectFetcher<F>
{
    fn fetch(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        run_spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<VerifiedSourceObject> {
        work_budget.check_deadline(OperatorWorkBudgetStage::Fetch)?;
        let cache_path = self.cache_entry_path(record);
        if let Some(cached) = self.read_verified_cache_entry(record, &cache_path, work_budget)? {
            return Ok(cached);
        }
        let object =
            guarded_operation_outcome(work_budget, OperatorWorkBudgetStage::Fetch, || {
                self.inner.fetch(record, run_spec, work_budget)
            })??;
        object.ensure_matches(record)?;
        guarded_operation_outcome(work_budget, OperatorWorkBudgetStage::Fetch, || {
            self.store_verified(&cache_path, object.as_bytes(), work_budget)
        })??;
        Ok(object)
    }
}

#[cfg(test)]
#[derive(Default)]
struct LocalSourceUniverseOperatorRunner;

#[cfg(test)]
impl SourceUniverseOperatorRunner for LocalSourceUniverseOperatorRunner {
    fn run(
        &mut self,
        _record: &SourceUniverseExecutionPackRecord,
        object_bytes: Vec<u8>,
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<SourceUniverseOperatorRunOutcome> {
        let artifacts = crate::operator::run_operator_from_run_spec_with_verified_registry(
            &control_artifacts.run_spec,
            &object_bytes,
            output_dir,
            &control_artifacts.source_bindings,
            work_budget,
        )?;
        let summary = match artifacts {
            crate::operator::OperatorRunArtifacts::Trade(artifacts) => artifacts.batch_summary,
            crate::operator::OperatorRunArtifacts::MultiTable(artifacts) => artifacts.batch_summary,
        };
        crate::operator::convert_test_terminal_output_to_durable_candidate(
            &control_artifacts.run_spec,
            output_dir,
            &control_artifacts.source_bindings,
            &summary,
            work_budget,
        )?;
        Ok(SourceUniverseOperatorRunOutcome::NonTerminal(
            SourceUniverseBatchExecutionRunOutput::from_summary(summary),
        ))
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseOperatorWorkerRequestPayload {
    role: String,
    bytes: u64,
    sha256: String,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SourceUniverseOperatorWorkerRequestKind {
    Execute,
    Discover,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseOperatorWorkerRequestManifest {
    schema_version: String,
    request_kind: SourceUniverseOperatorWorkerRequestKind,
    execution_attestation: DurableExecutionAttestation,
    record: SourceUniverseExecutionPackRecord,
    output_dir: PathBuf,
    source_bindings_path: PathBuf,
    work_budget_deadline: OperatorWorkBudgetDeadline,
    payloads: Vec<SourceUniverseOperatorWorkerRequestPayload>,
}

enum SourceUniverseOperatorWorkerRequest {
    Execute(Vec<u8>),
    Discover,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum SourceUniverseOperatorWorkerResponse {
    NoCurrentCompletion,
    Completed { receipt: DurableRunReceipt },
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnonymousWorkerRequestArchiveIdentity {
    byte_len: u64,
    device: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct CommittedWorkerRequestArchive {
    file: fs::File,
    identity: AnonymousWorkerRequestArchiveIdentity,
    archive_bytes: u64,
    manifest_bytes: u64,
    manifest_sha256: String,
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
struct CommittedWorkerRequestArchive;

/// Process-lifetime evidence is separate from local candidate integrity and
/// remote durable commit authority. Candidate/receipt agreement is considered
/// only after every possible writer in the child process group is quiesced and
/// the leader is reaped.
#[derive(Debug)]
enum WorkerLifecycleOutcome {
    NotStarted(anyhow::Error),
    Quiesced(Result<WorkerExitEvidence>),
    Indeterminate(anyhow::Error),
}

#[derive(Debug)]
struct WorkerExitEvidence {
    status: ExitStatus,
    receipt_bytes: Vec<u8>,
}

#[cfg(target_os = "linux")]
impl CommittedWorkerRequestArchive {
    fn revalidate(&self) -> Result<()> {
        ensure!(
            anonymous_worker_request_archive_identity(&self.file, true)? == self.identity,
            "anonymous worker request archive identity changed"
        );
        Ok(())
    }

    fn stdin_file(&self) -> Result<fs::File> {
        self.revalidate()?;
        let file = self
            .file
            .try_clone()
            .context("clone anonymous worker request archive for child stdin")?;
        ensure!(
            anonymous_worker_request_archive_identity(&file, true)? == self.identity,
            "cloned anonymous worker request archive identity changed"
        );
        Ok(file)
    }
}

#[cfg(target_os = "linux")]
fn anonymous_worker_request_archive_identity(
    file: &fs::File,
    require_read_only: bool,
) -> Result<AnonymousWorkerRequestArchiveIdentity> {
    let metadata = file
        .metadata()
        .context("fstat anonymous worker request archive")?;
    ensure!(
        metadata.file_type().is_file() && metadata.nlink() == 0,
        "worker request archive must be one anonymous regular file with nlink == 0"
    );
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .context("read anonymous worker request archive descriptor flags");
    }
    if require_read_only {
        ensure!(
            flags & libc::O_ACCMODE == libc::O_RDONLY,
            "worker request archive descriptor must be read-only"
        );
        ensure!(
            metadata.mode() & 0o222 == 0,
            "worker request archive inode must have no write permission bits"
        );
    }
    Ok(AnonymousWorkerRequestArchiveIdentity {
        byte_len: metadata.len(),
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(target_os = "linux")]
fn open_worker_request_root(request_root: &Path) -> Result<fs::File> {
    ensure_real_directory(request_root, "worker request root")?;
    let canonical = request_root.canonicalize().with_context(|| {
        format!(
            "canonicalize worker request root {}",
            request_root.display()
        )
    })?;
    ensure!(
        canonical == request_root,
        "worker request root must already be canonical: {}",
        request_root.display()
    );
    let path_metadata = fs::symlink_metadata(request_root)
        .with_context(|| format!("lstat worker request root {}", request_root.display()))?;
    let handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(request_root)
        .with_context(|| format!("open worker request root {}", request_root.display()))?;
    let handle_metadata = handle
        .metadata()
        .with_context(|| format!("fstat worker request root {}", request_root.display()))?;
    ensure!(
        path_metadata.file_type().is_dir()
            && handle_metadata.file_type().is_dir()
            && path_metadata.dev() == handle_metadata.dev()
            && path_metadata.ino() == handle_metadata.ino(),
        "worker request root identity changed while opening: {}",
        request_root.display()
    );
    Ok(handle)
}

#[cfg(target_os = "linux")]
fn create_anonymous_worker_request_file(request_root: &Path) -> Result<fs::File> {
    let root = open_worker_request_root(request_root)?;
    let flags = libc::O_TMPFILE | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC;
    // SAFETY: `root` is a retained directory descriptor, `c"."` is a live
    // NUL-terminated component, and a successful descriptor is immediately
    // transferred to `File`. O_TMPFILE creates no namespace entry.
    let fd = unsafe { libc::openat(root.as_raw_fd(), c".".as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("create anonymous disk-backed worker request archive with O_TMPFILE");
    }
    // SAFETY: successful `openat` returned one new owned descriptor.
    let file = unsafe { fs::File::from_raw_fd(fd) };
    let _ = anonymous_worker_request_archive_identity(&file, false)?;
    Ok(file)
}

#[cfg(target_os = "linux")]
fn reopen_anonymous_worker_request_read_only(
    writable: fs::File,
    expected_bytes: u64,
) -> Result<(fs::File, AnonymousWorkerRequestArchiveIdentity)> {
    let writable_identity = anonymous_worker_request_archive_identity(&writable, false)?;
    ensure!(
        writable_identity.byte_len == expected_bytes,
        "anonymous worker request archive length mismatch before read-only reopen"
    );
    // SAFETY: the descriptor is live and owned by this process. Removing every
    // write bit prevents a fresh writable open through /proc once this handle
    // is closed.
    if unsafe { libc::fchmod(writable.as_raw_fd(), 0o400) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("chmod anonymous worker request archive read-only");
    }
    let proc_path = std::ffi::CString::new(format!("/proc/self/fd/{}", writable.as_raw_fd()))
        .context("construct anonymous worker request /proc descriptor path")?;
    // SAFETY: `proc_path` names the still-live exact writable descriptor. The
    // reopened descriptor is immediately transferred to `File` and then bound
    // back to the original dev/inode identity before the writer is closed.
    let read_fd = unsafe { libc::open(proc_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if read_fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("reopen anonymous worker request archive read-only through /proc/self/fd");
    }
    // SAFETY: successful `open` returned one new owned descriptor.
    let read_only = unsafe { fs::File::from_raw_fd(read_fd) };
    let read_identity = anonymous_worker_request_archive_identity(&read_only, true)?;
    ensure!(
        read_identity.device == writable_identity.device
            && read_identity.inode == writable_identity.inode
            && read_identity.byte_len == writable_identity.byte_len,
        "read-only anonymous worker request reopen changed inode identity"
    );
    drop(writable);
    ensure!(
        anonymous_worker_request_archive_identity(&read_only, true)? == read_identity,
        "anonymous worker request identity changed after closing every writable handle"
    );
    Ok((read_only, read_identity))
}

#[derive(Debug)]
struct ProcessIsolatedSourceUniverseOperatorRunner {
    executable: PinnedWorkerExecutable,
    executable_sha256: Option<String>,
    #[cfg(test)]
    executable_hash_traversals: usize,
    request_root: PathBuf,
    workspace_owner_lock: fs::File,
    worker_termination_grace: Duration,
    resource_limits: SourceUniverseBatchResourceLimits,
}

/// Reject process-level record parallelism until the execution plan carries a
/// real aggregate parent/child memory ceiling. The CLI calls this before any
/// pack, cache, output, fetch, or spawn activity; the constructor repeats the
/// same single source of truth for library callers.
pub fn validate_process_isolated_max_concurrent_records(max_concurrent_records: u64) -> Result<()> {
    ensure!(
        max_concurrent_records == 1,
        "process-isolated execution requires max_concurrent_records=1 until a configured aggregate-memory byte budget exists; got {max_concurrent_records}"
    );
    Ok(())
}

/// Validate the sole production tracer's bounded selection before it may
/// inspect the execution pack, output tree, or object cache.
pub fn validate_process_isolated_batch_selection(
    record_limit: Option<u64>,
    max_concurrent_records: Option<u64>,
) -> Result<()> {
    ensure!(
        max_concurrent_records == Some(1),
        "process-isolated execution requires max_concurrent_records=1; got {max_concurrent_records:?}"
    );
    validate_process_isolated_max_concurrent_records(
        max_concurrent_records.expect("exact process-isolated concurrency was checked"),
    )?;
    ensure!(
        record_limit == Some(1),
        "process-isolated execution requires record_limit=1; got {record_limit:?}"
    );
    Ok(())
}

impl ProcessIsolatedSourceUniverseOperatorRunner {
    fn new(
        request_root: PathBuf,
        workspace_owner_lock: fs::File,
        max_concurrent_records: u64,
        worker_termination_grace: Duration,
        resource_limits: SourceUniverseBatchResourceLimits,
    ) -> Result<Self> {
        validate_process_isolated_max_concurrent_records(max_concurrent_records)?;
        let resource_limits = resource_limits.validate()?;
        ensure!(
            !worker_termination_grace.is_zero(),
            "worker termination grace must be positive"
        );
        ensure!(
            request_root.is_absolute(),
            "worker request root must be absolute: {}",
            request_root.display()
        );
        ensure!(
            request_root.file_name().and_then(|name| name.to_str())
                == Some(SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT),
            "worker request root must use the structural component {}",
            SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT
        );
        let request_parent = request_root
            .parent()
            .context("worker request root has no parent")?
            .canonicalize()
            .with_context(|| {
                format!(
                    "canonicalize worker request parent {}",
                    request_root.display()
                )
            })?;
        let request_root = request_parent.join(SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT);
        Ok(Self {
            executable: PinnedWorkerExecutable::capture_current()?,
            executable_sha256: None,
            #[cfg(test)]
            executable_hash_traversals: 0,
            request_root,
            workspace_owner_lock,
            worker_termination_grace,
            resource_limits,
        })
    }

    fn seal_executable_once(&mut self, work_budget: &OperatorWorkBudgetGuard) -> Result<()> {
        if self.executable_sha256.is_none() {
            let sha256 = self.executable.hash_and_revalidate(None, work_budget)?;
            self.executable_sha256 = Some(sha256);
            #[cfg(test)]
            {
                self.executable_hash_traversals += 1;
            }
        } else {
            self.executable.revalidate_identity()?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn executable_hash_traversals_for_test(&self) -> usize {
        self.executable_hash_traversals
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct PinnedWorkerExecutable {
    file: fs::File,
    byte_len: u64,
    device: u64,
    inode: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
struct PinnedWorkerExecutable;

impl PinnedWorkerExecutable {
    #[cfg(target_os = "linux")]
    fn capture_current() -> Result<Self> {
        // `/proc/self/exe` is a kernel-backed handle to the inode already
        // executing this process. Opening it is not a mutable pathname lookup
        // for the original binary, and the resulting fd is the sole child-exec
        // capability retained by this runner.
        let file = fs::File::open("/proc/self/exe")
            .context("open kernel-backed current executable capability")?;
        let metadata = file
            .metadata()
            .context("fstat current executable capability")?;
        ensure!(
            metadata.file_type().is_file(),
            "current executable capability must refer to a regular file"
        );
        Ok(Self {
            file,
            byte_len: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn capture_current() -> Result<Self> {
        bail!("fd-backed same-binary worker execution is unsupported on this platform")
    }

    #[cfg(target_os = "linux")]
    fn exec_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
    }

    #[cfg(target_os = "linux")]
    fn revalidate_identity(&self) -> Result<()> {
        let metadata = self
            .file
            .metadata()
            .context("re-fstat worker executable capability")?;
        ensure!(
            metadata.file_type().is_file()
                && metadata.len() == self.byte_len
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
                && metadata.mtime() == self.modified_seconds
                && metadata.mtime_nsec() == self.modified_nanoseconds
                && metadata.ctime() == self.changed_seconds
                && metadata.ctime_nsec() == self.changed_nanoseconds,
            "worker executable capability identity changed"
        );
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn revalidate_identity(&self) -> Result<()> {
        bail!("fd-backed same-binary worker execution is unsupported on this platform")
    }

    #[cfg(target_os = "linux")]
    fn hash_and_revalidate(
        &self,
        expected_sha256: Option<&str>,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<String> {
        self.revalidate_identity()?;
        let mut hash_file = self
            .file
            .try_clone()
            .context("clone worker executable capability for hashing")?;
        let sha256 = sha256_exact_sized_open_file_guarded(
            &mut hash_file,
            Path::new("/proc/self/exe"),
            self.byte_len,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )?;
        if let Some(expected_sha256) = expected_sha256 {
            ensure!(
                sha256 == expected_sha256,
                "worker executable capability hash changed"
            );
        }
        self.revalidate_identity()
            .context("worker executable capability identity changed while hashing")?;
        Ok(sha256)
    }

    #[cfg(not(target_os = "linux"))]
    fn hash_and_revalidate(
        &self,
        _expected_sha256: Option<&str>,
        _work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<String> {
        bail!("fd-backed same-binary worker execution is unsupported on this platform")
    }
}

impl SourceUniverseOperatorRunner for ProcessIsolatedSourceUniverseOperatorRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        object_bytes: Vec<u8>,
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<SourceUniverseOperatorRunOutcome> {
        self.seal_executable_once(work_budget)?;
        let worker_executable_sha256 = self
            .executable_sha256
            .clone()
            .context("sealed worker executable is missing its cached SHA-256")?;
        let execution_attestation =
            DurableExecutionAttestation::new_process_isolated(worker_executable_sha256.clone())?;
        let output_lease = PinnedWorkerDirectoryLease::capture(output_dir)?;
        reverify_parent_held_controls(record, control_artifacts, work_budget)?;
        let request_archive = commit_worker_request_archive(
            &self.request_root,
            record,
            SourceUniverseOperatorWorkerRequest::Execute(object_bytes),
            &execution_attestation,
            control_artifacts,
            output_dir,
            work_budget,
        )?;
        // `commit_worker_request_archive` consumes and releases the fetched
        // allocation before returning this archive. The child therefore cannot
        // overlap its selected-object buffer with a full parent-side copy.
        let worker_lifecycle = spawn_and_wait_for_worker(
            &self.executable,
            &self.workspace_owner_lock,
            request_archive,
            work_budget,
            self.worker_termination_grace,
            self.resource_limits,
        );
        let worker_result = require_quiesced_worker_lifecycle(worker_lifecycle)?;

        // Child status, request archive, and local candidate are deliberately
        // not durable commit authorities. Only the `Quiesced` state reaches
        // this point. Acceptance requires all three independent facts: a
        // successful child exit, one canonical local candidate, and an exact
        // durable receipt returned after remote terminal publication. The
        // probe is byte-capped and does not sample the now-expired work budget.
        if let Err(error) = output_lease.revalidate() {
            return Err(committed_indeterminate_worker_error(format!(
                "output lease changed before durable candidate-seal probe: {error:#}"
            )));
        }
        let candidate_probe = probe_durable_output_candidate_seal_summary_capped(
            &control_artifacts.run_spec,
            output_dir,
            &control_artifacts.source_bindings,
            control_artifacts.execution_plan.max_decoded_bytes,
        );
        accept_quiesced_durable_worker(
            worker_result,
            candidate_probe,
            worker_executable_sha256,
            |bytes, summary| {
                validate_worker_durable_receipt(
                    bytes,
                    &control_artifacts.run_spec,
                    summary,
                    &execution_attestation.worker_executable_sha256,
                )
            },
            || output_lease.revalidate(),
        )
    }

    fn discover_current_completion(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Option<SourceUniverseDiscoveredRunReceipt>> {
        self.seal_executable_once(work_budget)?;
        let worker_executable_sha256 = self
            .executable_sha256
            .clone()
            .context("sealed worker executable is missing its cached SHA-256")?;
        let execution_attestation =
            DurableExecutionAttestation::new_process_isolated(worker_executable_sha256.clone())?;
        let output_lease = PinnedWorkerDirectoryLease::capture(output_dir)?;
        reverify_parent_held_controls(record, control_artifacts, work_budget)?;
        let request_archive = commit_worker_request_archive(
            &self.request_root,
            record,
            SourceUniverseOperatorWorkerRequest::Discover,
            &execution_attestation,
            control_artifacts,
            output_dir,
            work_budget,
        )?;
        let worker_lifecycle = spawn_and_wait_for_worker(
            &self.executable,
            &self.workspace_owner_lock,
            request_archive,
            work_budget,
            self.worker_termination_grace,
            self.resource_limits,
        );
        let worker_result = require_quiesced_worker_lifecycle(worker_lifecycle)?;
        let worker_evidence = worker_result.map_err(|error| {
            committed_indeterminate_worker_error(format!(
                "deterministic current-terminal discovery failed after worker start: {error:#}"
            ))
        })?;
        ensure!(
            worker_evidence.status.success(),
            "current-terminal discovery worker exited with status {}",
            worker_evidence.status
        );
        let discovered = validate_worker_completion_discovery(
            &worker_evidence.receipt_bytes,
            &control_artifacts.run_spec,
        )?
        .map(|(receipt, _summary)| SourceUniverseDiscoveredRunReceipt {
            attempt_worker_sha256: worker_executable_sha256,
            receipt,
        });
        output_lease
            .revalidate()
            .context("fresh discovery scratch lease changed during current-terminal validation")?;
        Ok(discovered)
    }
}

struct PinnedWorkerDirectoryLease {
    path: PathBuf,
    handle: fs::File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl PinnedWorkerDirectoryLease {
    fn capture(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("lstat worker output directory {}", path.display()))?;
        ensure!(
            metadata.file_type().is_dir(),
            "worker output must be a real directory: {}",
            path.display()
        );
        let canonical = path
            .canonicalize()
            .with_context(|| format!("canonicalize worker output directory {}", path.display()))?;
        ensure!(
            canonical == path,
            "worker output path must already be canonical: {}",
            path.display()
        );
        let handle = fs::File::open(path)
            .with_context(|| format!("open worker output directory {}", path.display()))?;
        let handle_metadata = handle
            .metadata()
            .with_context(|| format!("fstat worker output directory {}", path.display()))?;
        ensure!(
            handle_metadata.file_type().is_dir(),
            "worker output handle must refer to a directory"
        );
        #[cfg(not(unix))]
        bail!("worker output directory identity is unsupported on this platform");
        let lease = Self {
            path: path.to_path_buf(),
            handle,
            #[cfg(unix)]
            device: handle_metadata.dev(),
            #[cfg(unix)]
            inode: handle_metadata.ino(),
        };
        lease.revalidate()?;
        Ok(lease)
    }

    fn revalidate(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.path)
            .with_context(|| format!("re-lstat worker output {}", self.path.display()))?;
        ensure!(
            metadata.file_type().is_dir(),
            "worker output path is no longer a real directory"
        );
        let handle_metadata = self
            .handle
            .metadata()
            .with_context(|| format!("re-fstat worker output {}", self.path.display()))?;
        #[cfg(unix)]
        ensure!(
            metadata.dev() == self.device
                && metadata.ino() == self.inode
                && handle_metadata.dev() == self.device
                && handle_metadata.ino() == self.inode,
            "worker output directory identity changed: {}",
            self.path.display()
        );
        Ok(())
    }
}

fn reverify_parent_held_controls(
    record: &SourceUniverseExecutionPackRecord,
    controls: &SourceUniverseVerifiedControlArtifacts,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let stage = OperatorWorkBudgetStage::ObjectVerification;
    for (role, bytes, expected_bytes, expected_sha256) in [
        (
            "run_spec",
            controls.run_spec_bytes.as_ref(),
            record.run_spec_bytes,
            record.run_spec_sha256.as_str(),
        ),
        (
            "accepted_tranche",
            controls.accepted_tranche_bytes.as_ref(),
            record.accepted_tranche_bytes,
            record.accepted_tranche_sha256.as_str(),
        ),
        (
            "execution_plan",
            controls.execution_plan_bytes.as_ref(),
            record.execution_plan_bytes,
            record.execution_plan_sha256.as_str(),
        ),
        (
            "source_bindings",
            controls.source_bindings_bytes.as_ref(),
            record.source_bindings_bytes,
            record.source_bindings_sha256.as_str(),
        ),
    ] {
        ensure!(
            u64::try_from(bytes.len()).context("frozen control length does not fit u64")?
                == expected_bytes,
            "frozen parent-held {role} length does not match execution-pack record"
        );
        ensure!(
            sha256_hex_with_budget(bytes, work_budget, stage)? == expected_sha256,
            "frozen parent-held {role} SHA-256 does not match execution-pack record"
        );
    }
    let validated = validate_backfill_execution_control_bytes(
        controls.run_spec_bytes.as_ref(),
        controls.accepted_tranche_bytes.as_ref(),
        controls.execution_plan_bytes.as_ref(),
    )?;
    validate_worker_record_control_alignment(record, &validated)?;
    controls.source_bindings.reassert_for(&validated.run_spec)?;
    ensure!(
        controls.source_bindings.sha256() == record.source_bindings_sha256,
        "frozen parent-held source-bindings registry digest changed"
    );
    Ok(())
}

fn validate_worker_record_control_alignment(
    record: &SourceUniverseExecutionPackRecord,
    controls: &ValidatedBackfillExecutionControls,
) -> Result<()> {
    let run_spec = &controls.run_spec;
    let tranche = &controls.accepted_tranche;
    let plan = &controls.execution_plan;
    let object = plan
        .objects
        .first()
        .context("validated worker execution plan is missing its accepted object")?;
    let identity = run_spec
        .identity
        .single()
        .context("source-universe worker requires one instrument identity")?;
    ensure!(
        record.operator_run_id == run_spec.manifest.run_id
            && record.operator_run_id == plan.operator_run_id,
        "worker record operator_run_id does not match retained controls"
    );
    ensure!(
        record.source_binding == run_spec.source_proof.source_binding
            && record.source_binding == run_spec.manifest.venue_binding_key
            && record.source_binding == tranche.source_binding
            && record.source_binding == plan.source_binding,
        "worker record source_binding does not match retained controls"
    );
    ensure!(
        record.category == run_spec.source_proof.product_category
            && record.symbol == identity.venue_symbol,
        "worker record category/symbol does not match retained run spec"
    );
    ensure!(
        record.archive_date == run_spec.accepted_object.archive_date
            && record.archive_date == object.archive_date,
        "worker record archive_date does not match retained controls"
    );
    ensure!(
        record.source_uri == run_spec.accepted_object.s3_uri
            && record.source_uri == run_spec.source_proof.raw_sample_uri
            && record.source_uri == object.s3_uri,
        "worker record source_uri does not match retained controls"
    );
    ensure!(
        record.source_url == run_spec.accepted_object.source_url
            && record.source_url == object.source_url,
        "worker record source_url does not match retained controls"
    );
    ensure!(
        record.selected_object_sha256 == run_spec.accepted_object.sha256
            && record.selected_object_sha256 == run_spec.source_proof.raw_sample_hash
            && record.selected_object_sha256 == object.sha256
            && record.selected_object_bytes == run_spec.accepted_object.bytes
            && record.selected_object_bytes == object.bytes,
        "worker record selected object identity does not match retained controls"
    );
    ensure!(
        record.source_proof_id == run_spec.source_proof.source_proof_id
            && record.source_proof_id == plan.source_proof_id
            && record.source_proof_version == run_spec.source_proof.source_proof_version
            && record.source_proof_version == plan.source_proof_version,
        "worker record source-proof identity does not match retained controls"
    );
    ensure!(
        record.accepted_tranche_id == tranche.tranche_id
            && record.accepted_tranche_id == plan.accepted_tranche_id,
        "worker record accepted-tranche identity does not match retained controls"
    );
    ensure!(
        record.source_bindings_path == run_spec.source_bindings_path,
        "worker record source-bindings path does not match retained run spec"
    );
    ensure!(
        record.output_prefix == run_spec.manifest.output_prefix
            && record.output_prefix == plan.output_prefix,
        "worker record output_prefix does not match retained controls"
    );
    ensure!(
        record.run_spec_sha256 == controls.run_spec_sha256
            && record.accepted_tranche_sha256 == controls.accepted_tranche_sha256
            && record.execution_plan_sha256 == controls.execution_plan_sha256,
        "worker record control digest does not match retained bytes"
    );
    Ok(())
}

fn commit_worker_request_archive(
    request_root: &Path,
    record: &SourceUniverseExecutionPackRecord,
    request: SourceUniverseOperatorWorkerRequest,
    execution_attestation: &DurableExecutionAttestation,
    controls: &SourceUniverseVerifiedControlArtifacts,
    output_dir: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CommittedWorkerRequestArchive> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            request_root,
            record,
            request,
            execution_attestation,
            controls,
            output_dir,
            work_budget,
        );
        bail!("anonymous worker request archives are unsupported on this platform");
    }

    #[cfg(target_os = "linux")]
    {
        let output_dir = output_dir.canonicalize().with_context(|| {
            format!(
                "canonicalize worker operator output directory {}",
                output_dir.display()
            )
        })?;
        ensure!(
            controls.source_bindings_path.is_absolute(),
            "verified source-bindings provenance path must be absolute"
        );
        // Sample the cross-process Linux monotonic epoch before asking the
        // parent guard for its remaining time. This can only shorten, never
        // refresh, the original wall-time interval.
        let absolute_sample = system_operator_work_budget_clock().now();
        let remaining = work_budget
            .remaining_wall_time(OperatorWorkBudgetStage::ObjectVerification)?
            .context("process-isolated worker request requires a finite wall deadline")?;
        let deadline = absolute_sample
            .checked_add(remaining)
            .context("worker absolute deadline overflow")?;
        let started_at = deadline
            .checked_sub(Duration::from_secs(
                controls.execution_plan.max_wall_seconds,
            ))
            .context("worker absolute deadline precedes configured start")?;
        let work_budget_deadline = OperatorWorkBudgetDeadline {
            started_at_seconds: started_at.as_secs(),
            started_at_nanoseconds: started_at.subsec_nanos(),
            deadline_seconds: deadline.as_secs(),
            deadline_nanoseconds: deadline.subsec_nanos(),
        };
        work_budget_deadline
            .validate_for_max_wall_seconds(controls.execution_plan.max_wall_seconds)?;

        let (request_kind, selected_object) = match request {
            SourceUniverseOperatorWorkerRequest::Execute(object_bytes) => (
                SourceUniverseOperatorWorkerRequestKind::Execute,
                Some(object_bytes),
            ),
            SourceUniverseOperatorWorkerRequest::Discover => {
                (SourceUniverseOperatorWorkerRequestKind::Discover, None)
            }
        };
        let mut payload_bytes: Vec<(&str, &[u8])> = Vec::new();
        payload_bytes
            .try_reserve_exact(WORKER_REQUEST_ROLES.len())
            .context("reserve worker request payload bytes")?;
        payload_bytes.extend([
            (
                WORKER_REQUEST_ROLE_ACCEPTED_TRANCHE,
                controls.accepted_tranche_bytes.as_ref(),
            ),
            (
                WORKER_REQUEST_ROLE_EXECUTION_PLAN,
                controls.execution_plan_bytes.as_ref(),
            ),
            (
                WORKER_REQUEST_ROLE_RUN_SPEC,
                controls.run_spec_bytes.as_ref(),
            ),
        ]);
        if let Some(object_bytes) = selected_object.as_ref() {
            payload_bytes.push((WORKER_REQUEST_ROLE_SELECTED_OBJECT, object_bytes.as_slice()));
        }
        payload_bytes.push((
            WORKER_REQUEST_ROLE_SOURCE_BINDINGS,
            controls.source_bindings_bytes.as_ref(),
        ));
        let mut payloads = Vec::new();
        payloads
            .try_reserve_exact(payload_bytes.len())
            .context("reserve worker request payload inventory")?;
        for (role, bytes) in payload_bytes.iter().copied() {
            payloads.push(SourceUniverseOperatorWorkerRequestPayload {
                role: role.to_string(),
                bytes: u64::try_from(bytes.len())
                    .context("worker request payload length does not fit u64")?,
                sha256: sha256_hex_with_budget(
                    bytes,
                    work_budget,
                    OperatorWorkBudgetStage::ObjectVerification,
                )?,
            });
        }
        let manifest = SourceUniverseOperatorWorkerRequestManifest {
            schema_version: SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_SCHEMA_VERSION.to_string(),
            request_kind,
            execution_attestation: execution_attestation.clone(),
            record: record.clone(),
            output_dir,
            source_bindings_path: controls.source_bindings_path.clone(),
            work_budget_deadline,
            payloads,
        };
        validate_worker_request_manifest(&manifest)?;
        let manifest_body = serde_json::to_vec(&manifest)
            .context("serialize canonical worker request archive manifest")?;
        let manifest_bytes = u64::try_from(manifest_body.len())
            .context("worker request manifest length does not fit u64")?;
        let manifest_sha256 = sha256_hex_with_budget(
            &manifest_body,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )?;
        let archive_bytes = worker_request_archive_expected_bytes(manifest_bytes, &manifest)?;
        let mut writable = create_anonymous_worker_request_file(request_root)?;
        {
            let mut writer = CooperativeDeadlineWriter::new(
                &mut writable,
                work_budget,
                OperatorWorkBudgetStage::ObjectVerification,
            );
            writer
                .write_all(&manifest_bytes.to_be_bytes())
                .context("write worker request manifest-length header")?;
            writer
                .write_all(&manifest_body)
                .context("write worker request manifest")?;
            for (role, bytes) in payload_bytes.iter().copied() {
                writer
                    .write_all(bytes)
                    .with_context(|| format!("write worker request payload {role}"))?;
            }
            writer.flush().context("flush worker request archive")?;
        }
        let (file, identity) = reopen_anonymous_worker_request_read_only(writable, archive_bytes)?;
        Ok(CommittedWorkerRequestArchive {
            file,
            identity,
            archive_bytes,
            manifest_bytes,
            manifest_sha256,
        })
    }
}

fn ensure_real_directory(path: &Path, role: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_real_directory(path, role)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("create {role} {}", path.display()))?;
            validate_real_directory(path, role)?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("lstat {role} {}", path.display()));
        }
    }
    Ok(())
}

fn validate_real_directory(path: &Path, role: &str) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("lstat {role} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_dir(),
        "{role} must be a real directory, not a symlink or special file: {}",
        path.display()
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_worker_request_manifest(
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
) -> Result<()> {
    ensure!(
        manifest.schema_version == SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_SCHEMA_VERSION,
        "worker request schema mismatch"
    );
    manifest
        .execution_attestation
        .validate()
        .context("validate worker request execution attestation")?;
    ensure!(
        manifest.output_dir.is_absolute(),
        "worker request output_dir must be absolute"
    );
    ensure!(
        manifest.source_bindings_path.is_absolute(),
        "worker request source-bindings provenance path must be absolute"
    );
    manifest
        .work_budget_deadline
        .validate()
        .context("validate worker request absolute deadline")?;
    let common_prefix = [
        (
            WORKER_REQUEST_ROLE_ACCEPTED_TRANCHE,
            manifest.record.accepted_tranche_bytes,
            manifest.record.accepted_tranche_sha256.as_str(),
        ),
        (
            WORKER_REQUEST_ROLE_EXECUTION_PLAN,
            manifest.record.execution_plan_bytes,
            manifest.record.execution_plan_sha256.as_str(),
        ),
        (
            WORKER_REQUEST_ROLE_RUN_SPEC,
            manifest.record.run_spec_bytes,
            manifest.record.run_spec_sha256.as_str(),
        ),
    ];
    let selected_object = match manifest.request_kind {
        SourceUniverseOperatorWorkerRequestKind::Execute => Some((
            WORKER_REQUEST_ROLE_SELECTED_OBJECT,
            manifest.record.selected_object_bytes,
            manifest.record.selected_object_sha256.as_str(),
        )),
        SourceUniverseOperatorWorkerRequestKind::Discover => None,
    };
    let source_bindings = [(
        WORKER_REQUEST_ROLE_SOURCE_BINDINGS,
        manifest.record.source_bindings_bytes,
        manifest.record.source_bindings_sha256.as_str(),
    )];
    let expected_count = common_prefix
        .len()
        .checked_add(selected_object.iter().count())
        .and_then(|count| count.checked_add(source_bindings.len()))
        .context("worker request expected payload count overflow")?;
    ensure!(
        manifest.payloads.len() == expected_count,
        "worker request manifest must contain exactly {expected_count} payloads"
    );
    for ((role, bytes, sha256), actual) in common_prefix
        .into_iter()
        .chain(selected_object)
        .chain(source_bindings)
        .zip(manifest.payloads.iter())
    {
        validate_sha256_hex(&actual.sha256)
            .with_context(|| format!("validate worker request payload hash for {role}"))?;
        ensure!(
            actual.role == role
                && actual.bytes > 0
                && actual.bytes == bytes
                && actual.sha256 == sha256,
            "worker request payload pin for role {role} does not match the frozen record"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn worker_request_archive_expected_bytes(
    manifest_bytes: u64,
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
) -> Result<u64> {
    validate_worker_request_manifest(manifest)?;
    manifest.payloads.iter().try_fold(
        worker_request_archive_header_bytes()?
            .checked_add(manifest_bytes)
            .context("worker request manifest range overflow")?,
        |total, payload| {
            total
                .checked_add(payload.bytes)
                .context("worker request payload range overflow")
        },
    )
}

#[cfg(target_os = "linux")]
fn worker_request_payload_offset(
    manifest_bytes: u64,
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
    role: &str,
) -> Result<u64> {
    let mut offset = worker_request_archive_header_bytes()?
        .checked_add(manifest_bytes)
        .context("worker request manifest range overflow")?;
    for payload in &manifest.payloads {
        if payload.role == role {
            return Ok(offset);
        }
        offset = offset
            .checked_add(payload.bytes)
            .context("worker request payload offset overflow")?;
    }
    bail!("worker request manifest is missing role {role}")
}

#[cfg(target_os = "linux")]
fn worker_request_retained_peak_bytes(
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
) -> Result<u64> {
    manifest.payloads.iter().try_fold(0_u64, |total, payload| {
        total
            .checked_add(payload.bytes)
            .context("worker request retained payload bytes overflow")
    })
}

#[cfg(target_os = "linux")]
fn worker_rlimit_from_u64(bytes: u64) -> Result<libc::rlim_t> {
    // `rlim_t` is a platform alias: this is an identity conversion on common
    // 64-bit Linux targets and a checked narrowing conversion where it is
    // smaller. Keep the checked form so the source remains width-correct.
    #[allow(clippy::useless_conversion)]
    let limit =
        libc::rlim_t::try_from(bytes).context("worker address-space limit does not fit rlim_t")?;
    ensure!(
        limit != libc::RLIM_INFINITY,
        "worker address-space limit must not resolve to RLIM_INFINITY"
    );
    Ok(limit)
}

#[cfg(target_os = "linux")]
fn configure_worker_address_space_limit(
    command: &mut Command,
    resource_limits: SourceUniverseBatchResourceLimits,
) -> Result<()> {
    let resource_limits = resource_limits.validate()?;
    let limit = worker_rlimit_from_u64(resource_limits.worker_max_virtual_memory_bytes)?;
    let rlimit = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    // SAFETY: the closure performs only the async-signal-safe `setrlimit`
    // syscall between fork and exec. Every value is copied into the closure;
    // it allocates nothing and touches no shared process state.
    unsafe {
        command.pre_exec(move || {
            if libc::setrlimit(libc::RLIMIT_AS, &rlimit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn configure_worker_workspace_owner_lock_inheritance(
    command: &mut Command,
    workspace_owner_lock: &fs::File,
) -> Result<()> {
    let fd = workspace_owner_lock.as_raw_fd();
    ensure!(
        fd > libc::STDERR_FILENO,
        "workspace owner-lock descriptor must not overlap standard streams"
    );
    // SAFETY: F_GETFD only inspects the live descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .context("inspect workspace owner-lock descriptor flags before worker spawn");
    }
    ensure!(
        flags & libc::FD_CLOEXEC != 0,
        "parent workspace owner-lock descriptor must remain close-on-exec"
    );
    // SAFETY: the closure performs only async-signal-safe fcntl syscalls
    // between fork and exec. It changes the descriptor flag in the child-only
    // descriptor table, so other parent threads never observe an inheritable
    // owner-lock descriptor.
    unsafe {
        command.pre_exec(move || {
            let child_flags = libc::fcntl(fd, libc::F_GETFD);
            if child_flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, child_flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn adopt_inherited_workspace_owner_lock(fd: i32) -> Result<fs::File> {
    ensure!(
        fd > libc::STDERR_FILENO,
        "inherited workspace owner-lock descriptor must not overlap standard streams"
    );
    // SAFETY: F_GETFD only inspects the descriptor supplied by the trusted
    // same-binary parent launch path.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .context("inspect inherited workspace owner-lock descriptor");
    }
    ensure!(
        flags & libc::FD_CLOEXEC == 0,
        "workspace owner-lock descriptor did not survive worker exec"
    );
    // SAFETY: after exec no Rust owner exists for this inherited raw
    // descriptor. This worker adopts the sole process-local owner and keeps it
    // live for the entire worker operation.
    let file = unsafe { fs::File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .context("stat inherited workspace owner-lock descriptor")?;
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == effective_uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0,
        "inherited workspace owner lock must be one owner-private regular file"
    );
    // An inherited descriptor refers to the same open-file description as the
    // parent lease. Reasserting the nonblocking exclusive flock proves the
    // capability still carries that live ownership before any worker write.
    // SAFETY: flock operates on the live inherited descriptor and never waits.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("validate inherited workspace owner lease in worker");
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn validate_current_worker_address_space_limit(expected_bytes: u64) -> Result<()> {
    ensure!(
        expected_bytes > 0 && expected_bytes != u64::MAX,
        "worker address-space limit must be positive and finite"
    );
    let expected = worker_rlimit_from_u64(expected_bytes)?;
    let mut actual = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `actual` is valid writable storage for one `rlimit` value.
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut actual) } != 0 {
        return Err(std::io::Error::last_os_error()).context("read worker RLIMIT_AS after exec");
    }
    ensure!(
        actual.rlim_cur == expected && actual.rlim_max == expected,
        "worker RLIMIT_AS does not match configured hard ceiling: expected {expected_bytes}, got soft={} hard={}",
        actual.rlim_cur,
        actual.rlim_max
    );
    Ok(())
}

fn spawn_and_wait_for_worker(
    executable: &PinnedWorkerExecutable,
    workspace_owner_lock: &fs::File,
    archive: CommittedWorkerRequestArchive,
    work_budget: &OperatorWorkBudgetGuard,
    termination_timeout: Duration,
    resource_limits: SourceUniverseBatchResourceLimits,
) -> WorkerLifecycleOutcome {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            executable,
            workspace_owner_lock,
            archive,
            work_budget,
            termination_timeout,
            resource_limits,
        );
        WorkerLifecycleOutcome::NotStarted(anyhow::anyhow!(
            "fd-backed same-binary worker execution is unsupported on this platform"
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let bootstrap_max_bytes = match (|| -> Result<u64> {
            executable.revalidate_identity()?;
            resource_limits.validate()?;
            let bootstrap_max_bytes = work_budget.decoded_byte_limit().unwrap_or(u64::MAX);
            ensure!(
                bootstrap_max_bytes > 0 && archive.manifest_bytes <= bootstrap_max_bytes,
                "worker request manifest bytes {} exceed configured bootstrap cap {}",
                archive.manifest_bytes,
                bootstrap_max_bytes
            );
            archive.revalidate()?;
            Ok(bootstrap_max_bytes)
        })() {
            Ok(bootstrap_max_bytes) => bootstrap_max_bytes,
            Err(error) => return WorkerLifecycleOutcome::NotStarted(error),
        };
        let stdin = match archive.stdin_file() {
            Ok(stdin) => stdin,
            Err(error) => return WorkerLifecycleOutcome::NotStarted(error),
        };
        let mut command = Command::new(executable.exec_path());
        command
            .arg(SOURCE_UNIVERSE_OPERATOR_WORKER_MODE)
            .arg("--workspace-owner-lock-fd")
            .arg(workspace_owner_lock.as_raw_fd().to_string())
            .arg("--request-archive-bytes")
            .arg(archive.archive_bytes.to_string())
            .arg("--request-manifest-bytes")
            .arg(archive.manifest_bytes.to_string())
            .arg("--request-manifest-sha256")
            .arg(&archive.manifest_sha256)
            .arg("--bootstrap-max-bytes")
            .arg(bootstrap_max_bytes.to_string())
            .arg("--worker-max-virtual-memory-bytes")
            .arg(resource_limits.worker_max_virtual_memory_bytes.to_string())
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Err(error) =
            configure_worker_workspace_owner_lock_inheritance(&mut command, workspace_owner_lock)
        {
            return WorkerLifecycleOutcome::NotStarted(error);
        }
        if let Err(error) = configure_worker_address_space_limit(&mut command, resource_limits) {
            return WorkerLifecycleOutcome::NotStarted(error);
        }
        let lifecycle = spawn_command_with_hard_deadline_observed_with_stdout(
            &mut command,
            work_budget,
            termination_timeout,
            Some(bootstrap_max_bytes),
        );
        let archive_validation = archive.revalidate();
        drop(archive);
        match lifecycle {
            WorkerLifecycleOutcome::NotStarted(error) => WorkerLifecycleOutcome::NotStarted(error),
            WorkerLifecycleOutcome::Indeterminate(error) => {
                WorkerLifecycleOutcome::Indeterminate(error)
            }
            WorkerLifecycleOutcome::Quiesced(result) => {
                WorkerLifecycleOutcome::Quiesced(result.and_then(|status| {
                    archive_validation?;
                    executable.revalidate_identity()?;
                    Ok(status)
                }))
            }
        }
    }
}

#[cfg(all(target_os = "linux", test))]
fn spawn_command_with_hard_deadline(
    command: &mut Command,
    work_budget: &OperatorWorkBudgetGuard,
    termination_timeout: Duration,
) -> Result<ExitStatus> {
    match spawn_command_with_hard_deadline_observed(command, work_budget, termination_timeout) {
        WorkerLifecycleOutcome::NotStarted(error)
        | WorkerLifecycleOutcome::Indeterminate(error) => Err(error),
        WorkerLifecycleOutcome::Quiesced(result) => result.map(|evidence| evidence.status),
    }
}

#[cfg(all(target_os = "linux", test))]
fn spawn_command_with_hard_deadline_observed(
    command: &mut Command,
    work_budget: &OperatorWorkBudgetGuard,
    termination_timeout: Duration,
) -> WorkerLifecycleOutcome {
    spawn_command_with_hard_deadline_observed_with_stdout(
        command,
        work_budget,
        termination_timeout,
        None,
    )
}

#[cfg(target_os = "linux")]
fn spawn_bounded_worker_stdout_reader(
    stdout: ChildStdout,
    max_bytes: u64,
) -> Result<std::thread::JoinHandle<Result<Vec<u8>>>> {
    let read_limit = max_bytes
        .checked_add(1)
        .context("worker receipt byte cap overflow")?;
    std::thread::Builder::new()
        .name("source-universe-worker-receipt".to_string())
        .spawn(move || {
            let mut receipt_bytes = Vec::new();
            stdout
                .take(read_limit)
                .read_to_end(&mut receipt_bytes)
                .context("read source-universe worker durable receipt")?;
            ensure!(
                u64::try_from(receipt_bytes.len())
                    .context("worker receipt length does not fit u64")?
                    <= max_bytes,
                "source-universe worker durable receipt exceeds configured bootstrap cap {max_bytes}"
            );
            Ok(receipt_bytes)
        })
        .context("spawn source-universe worker receipt reader")
}

#[cfg(target_os = "linux")]
fn quiesced_worker_outcome(
    status: Result<ExitStatus>,
    stdout_reader: &mut Option<std::thread::JoinHandle<Result<Vec<u8>>>>,
) -> WorkerLifecycleOutcome {
    WorkerLifecycleOutcome::Quiesced(status.and_then(|status| {
        let receipt_bytes = match stdout_reader.take() {
            Some(reader) => reader
                .join()
                .map_err(|_| anyhow::anyhow!("source-universe worker receipt reader panicked"))??,
            None => Vec::new(),
        };
        Ok(WorkerExitEvidence {
            status,
            receipt_bytes,
        })
    }))
}

#[cfg(target_os = "linux")]
fn spawn_command_with_hard_deadline_observed_with_stdout(
    command: &mut Command,
    work_budget: &OperatorWorkBudgetGuard,
    termination_timeout: Duration,
    captured_stdout_max_bytes: Option<u64>,
) -> WorkerLifecycleOutcome {
    if termination_timeout.is_zero() {
        return WorkerLifecycleOutcome::NotStarted(anyhow::anyhow!(
            "worker termination timeout must be positive"
        ));
    }
    command.process_group(0);
    match work_budget.remaining_wall_time(OperatorWorkBudgetStage::Backtest) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return WorkerLifecycleOutcome::NotStarted(anyhow::anyhow!(
                "process-isolated operator requires a finite wall deadline"
            ));
        }
        Err(error) => return WorkerLifecycleOutcome::NotStarted(error),
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return WorkerLifecycleOutcome::NotStarted(
                anyhow::Error::new(error).context("spawn source-universe operator worker process"),
            );
        }
    };
    let mut stdout_reader = match captured_stdout_max_bytes {
        Some(max_bytes) => match child.stdout.take() {
            Some(stdout) => match spawn_bounded_worker_stdout_reader(stdout, max_bytes) {
                Ok(reader) => Some(reader),
                Err(error) => {
                    return match terminate_and_reap_worker_without_pidfd(child, termination_timeout)
                    {
                        Ok(()) => WorkerLifecycleOutcome::Quiesced(Err(error)),
                        Err(quiescence_error) => {
                            WorkerLifecycleOutcome::Indeterminate(anyhow::anyhow!(
                                "worker receipt reader setup failed ({error:#}) and child quiescence/reap failed ({quiescence_error:#})"
                            ))
                        }
                    };
                }
            },
            None => {
                let error = anyhow::anyhow!("source-universe worker receipt pipe is missing");
                return match terminate_and_reap_worker_without_pidfd(child, termination_timeout) {
                    Ok(()) => WorkerLifecycleOutcome::Quiesced(Err(error)),
                    Err(quiescence_error) => {
                        WorkerLifecycleOutcome::Indeterminate(anyhow::anyhow!(
                            "worker receipt pipe was missing and child quiescence/reap failed ({quiescence_error:#})"
                        ))
                    }
                };
            }
        },
        None => None,
    };
    let pidfd = match open_child_pidfd(&child) {
        Ok(pidfd) => pidfd,
        Err(pidfd_error) => {
            return match terminate_and_reap_worker_without_pidfd(child, termination_timeout) {
                Ok(()) => quiesced_worker_outcome(
                    Err(pidfd_error.context(
                        "establish non-reaping source-universe worker identity before waiting",
                    )),
                    &mut stdout_reader,
                ),
                Err(quiescence_error) => WorkerLifecycleOutcome::Indeterminate(anyhow::anyhow!(
                    "pidfd establishment failed ({pidfd_error:#}) and child quiescence/reap failed ({quiescence_error:#})"
                )),
            };
        }
    };
    let remaining = match work_budget.remaining_wall_time(OperatorWorkBudgetStage::Backtest) {
        Ok(Some(remaining)) => remaining,
        Ok(None) => {
            return match terminate_and_reap_worker_with_pidfd(child, &pidfd, termination_timeout) {
                Ok(()) => quiesced_worker_outcome(
                    Err(anyhow::anyhow!(
                        "process-isolated operator requires a finite wall deadline"
                    )),
                    &mut stdout_reader,
                ),
                Err(error) => WorkerLifecycleOutcome::Indeterminate(error.context(
                    "finite worker deadline disappeared and child quiescence/reap failed",
                )),
            };
        }
        Err(deadline_error) => {
            return match terminate_and_reap_worker_with_pidfd(child, &pidfd, termination_timeout) {
                Ok(()) => quiesced_worker_outcome(Err(deadline_error), &mut stdout_reader),
                Err(error) => WorkerLifecycleOutcome::Indeterminate(anyhow::anyhow!(
                    "worker deadline observation failed ({deadline_error:#}) and child quiescence/reap failed ({error:#})"
                )),
            };
        }
    };
    match wait_for_pidfd(&pidfd, remaining) {
        Ok(true) => {
            // The leader is still an unreaped child, so its pid cannot be
            // recycled. Kill the complete group before reaping even when the
            // leader exited successfully: a worker must not orphan a writer.
            let kill_result = kill_unreaped_worker_process_group(&child);
            let wait_result = child.wait().context("reap source-universe operator worker");
            match (kill_result, wait_result) {
                (Ok(()), Ok(status)) => quiesced_worker_outcome(Ok(status), &mut stdout_reader),
                (kill_result, wait_result) => {
                    WorkerLifecycleOutcome::Indeterminate(anyhow::anyhow!(
                        "worker process-group termination/reap was not fully proven (kill: {}; reap: {})",
                        lifecycle_result_detail(&kill_result),
                        lifecycle_result_detail(&wait_result)
                    ))
                }
            }
        }
        Ok(false) => {
            match terminate_and_reap_worker_with_pidfd(child, &pidfd, termination_timeout) {
                Ok(()) => quiesced_worker_outcome(
                    Err(anyhow::anyhow!(
                        "source-universe operator worker exceeded remaining wall time {:?}",
                        remaining
                    )),
                    &mut stdout_reader,
                ),
                Err(error) => WorkerLifecycleOutcome::Indeterminate(error.context(format!(
                    "worker exceeded remaining wall time {remaining:?} and quiescence/reap failed"
                ))),
            }
        }
        Err(wait_error) => {
            match terminate_and_reap_worker_with_pidfd(child, &pidfd, termination_timeout) {
                Ok(()) => quiesced_worker_outcome(
                    Err(wait_error.context("wait on source-universe operator worker pidfd")),
                    &mut stdout_reader,
                ),
                Err(error) => WorkerLifecycleOutcome::Indeterminate(anyhow::anyhow!(
                    "worker pidfd wait failed ({wait_error:#}) and quiescence/reap failed ({error:#})"
                )),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn lifecycle_result_detail<T>(result: &Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(error) => format!("{error:#}"),
    }
}

#[cfg(target_os = "linux")]
fn open_child_pidfd(child: &Child) -> Result<OwnedFd> {
    let pid = libc::pid_t::try_from(child.id()).context("worker pid does not fit pid_t")?;
    // SAFETY: pidfd_open does not dereference user memory. The numeric pid is
    // owned by `child` and remains unreaped for the lifetime of the returned
    // pidfd, so the kernel binds this descriptor to that exact process.
    let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw_fd == -1 {
        return Err(std::io::Error::last_os_error()).context("pidfd_open worker child");
    }
    let raw_fd = i32::try_from(raw_fd).context("pidfd does not fit RawFd")?;
    // SAFETY: a successful pidfd_open returns a new owned file descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

#[cfg(target_os = "linux")]
fn wait_for_pidfd(pidfd: &OwnedFd, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("pidfd wait timeout overflows monotonic clock")?;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let seconds = libc::time_t::try_from(remaining.as_secs())
            .context("pidfd wait seconds do not fit time_t")?;
        let nanoseconds = libc::c_long::from(remaining.subsec_nanos());
        let timeout_spec = libc::timespec {
            tv_sec: seconds,
            tv_nsec: nanoseconds,
        };
        let mut poll_fd = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll_fd` and `timeout_spec` are valid for the duration of
        // this call; a null signal mask preserves the calling thread's mask.
        let result = unsafe {
            libc::ppoll(
                &mut poll_fd,
                1,
                &timeout_spec,
                std::ptr::null::<libc::sigset_t>(),
            )
        };
        if result > 0 {
            let terminal_events = libc::POLLIN | libc::POLLHUP | libc::POLLERR;
            ensure!(
                poll_fd.revents & terminal_events != 0,
                "worker pidfd returned unexpected poll events {}",
                poll_fd.revents
            );
            return Ok(true);
        }
        if result == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error).context("ppoll worker pidfd");
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct WorkerTerminationDeadline {
    expires_at: Instant,
    configured_grace: Duration,
}

#[cfg(target_os = "linux")]
impl WorkerTerminationDeadline {
    fn new(configured_grace: Duration) -> Result<Self> {
        ensure!(
            !configured_grace.is_zero(),
            "worker termination grace must be positive"
        );
        let expires_at = Instant::now()
            .checked_add(configured_grace)
            .context("worker termination grace overflows monotonic clock")?;
        Ok(Self {
            expires_at,
            configured_grace,
        })
    }

    fn remaining(self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }
}

#[cfg(target_os = "linux")]
fn terminate_and_reap_worker_without_pidfd(
    child: Child,
    termination_timeout: Duration,
) -> Result<()> {
    let termination_deadline = WorkerTerminationDeadline::new(termination_timeout)?;
    if let Err(kill_error) = kill_unreaped_worker_process_group(&child) {
        let detach_result = detach_worker_reaper(child);
        return Err(anyhow::anyhow!(
            "kill worker process group without pidfd failed ({kill_error:#}); detached reaper result: {}",
            lifecycle_result_detail(&detach_result)
        ));
    }
    reap_killed_child_without_pidfd_with_timeout(child, termination_deadline.remaining())
}

#[cfg(target_os = "linux")]
fn terminate_and_reap_worker_with_pidfd(
    mut child: Child,
    pidfd: &OwnedFd,
    termination_timeout: Duration,
) -> Result<()> {
    let termination_deadline = WorkerTerminationDeadline::new(termination_timeout)?;
    if let Err(kill_error) = kill_unreaped_worker_process_group(&child) {
        let detach_result = detach_worker_reaper(child);
        return Err(anyhow::anyhow!(
            "kill worker process group with pidfd failed ({kill_error:#}); detached reaper result: {}",
            lifecycle_result_detail(&detach_result)
        ));
    }
    match wait_for_pidfd(pidfd, termination_deadline.remaining()) {
        Ok(true) => {
            let _ = child.wait().context("reap worker after hard termination")?;
            return Ok(());
        }
        Ok(false) => {}
        Err(wait_error) => {
            reap_killed_child_without_pidfd_with_timeout(
                child,
                termination_deadline.remaining(),
            )
            .with_context(|| {
                format!(
                    "pidfd wait failed ({wait_error:#}) and fallback reap did not complete within the same termination grace"
                )
            })?;
            return Ok(());
        }
    }
    detach_worker_reaper(child)?;
    bail!(
        "source-universe operator worker did not terminate within configured termination timeout {:?}",
        termination_deadline.configured_grace
    )
}

#[cfg(target_os = "linux")]
fn kill_unreaped_worker_process_group(child: &Child) -> Result<()> {
    let process_group = i32::try_from(child.id()).context("worker pid does not fit i32")?;
    // SAFETY: the child was placed in a new process group whose id is its pid;
    // a negative pid targets that complete group. SIGKILL is required because
    // this is the hard wall-time enforcement boundary.
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error).context("kill source-universe operator worker process group")
}

#[cfg(target_os = "linux")]
fn reap_killed_child_without_pidfd_with_timeout(
    child: Child,
    termination_timeout: Duration,
) -> Result<()> {
    let (status_sender, status_receiver) = sync_channel(1);
    std::thread::Builder::new()
        .name("source-universe-worker-fallback-reaper".to_string())
        .spawn(move || {
            let mut child = child;
            let _ = status_sender.send(child.wait());
        })
        .context("spawn fallback worker reaper")?;
    match status_receiver.recv_timeout(termination_timeout) {
        Ok(status) => {
            let _ = status.context("reap killed worker without pidfd")?;
            Ok(())
        }
        Err(RecvTimeoutError::Disconnected) => {
            bail!("fallback source-universe worker reaper disconnected")
        }
        Err(RecvTimeoutError::Timeout) => bail!(
            "source-universe operator worker did not reap within configured termination timeout {:?}",
            termination_timeout
        ),
    }
}

#[cfg(target_os = "linux")]
fn detach_worker_reaper(child: Child) -> Result<()> {
    std::thread::Builder::new()
        .name("source-universe-worker-detached-reaper".to_string())
        .spawn(move || {
            let mut child = child;
            let _ = child.wait();
        })
        .context("spawn detached worker reaper")?;
    Ok(())
}

/// Hidden child-process entry. One read-only anonymous archive on stdin is the
/// only operational input; no request pathname exists or needs cleanup.
pub fn execute_source_universe_operator_worker(
    workspace_owner_lock_fd: i32,
    archive_bytes: u64,
    manifest_bytes: u64,
    manifest_sha256: &str,
    bootstrap_max_bytes: u64,
    worker_max_virtual_memory_bytes: u64,
) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            workspace_owner_lock_fd,
            archive_bytes,
            manifest_bytes,
            manifest_sha256,
            bootstrap_max_bytes,
            worker_max_virtual_memory_bytes,
        );
        bail!("anonymous stdin worker request archives are unsupported on this platform");
    }

    #[cfg(target_os = "linux")]
    {
        let _workspace_owner_lock = adopt_inherited_workspace_owner_lock(workspace_owner_lock_fd)?;
        validate_current_worker_address_space_limit(worker_max_virtual_memory_bytes)?;
        // SAFETY: stdin is live for the process. F_DUPFD_CLOEXEC creates one
        // independently owned descriptor without changing the shared offset.
        let fd = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("duplicate anonymous worker request stdin");
        }
        // SAFETY: successful fcntl returned one new owned descriptor.
        let file = unsafe { fs::File::from_raw_fd(fd) };
        let response = execute_source_universe_operator_worker_from_archive(
            &file,
            archive_bytes,
            manifest_bytes,
            manifest_sha256,
            bootstrap_max_bytes,
        )?;
        let response_bytes = crate::reference_artifact::canonical_json_bytes(&response)
            .context("serialize canonical source-universe durable worker response")?;
        ensure!(
            u64::try_from(response_bytes.len())
                .context("worker response length does not fit u64")?
                <= bootstrap_max_bytes,
            "source-universe durable worker response exceeds configured bootstrap cap"
        );
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        stdout
            .write_all(&response_bytes)
            .context("write source-universe durable worker response")?;
        stdout
            .flush()
            .context("flush source-universe durable worker response")
    }
}

#[cfg(target_os = "linux")]
fn execute_source_universe_operator_worker_from_archive(
    archive: &fs::File,
    archive_bytes: u64,
    manifest_bytes: u64,
    manifest_sha256: &str,
    bootstrap_max_bytes: u64,
) -> Result<SourceUniverseOperatorWorkerResponse> {
    ensure!(
        bootstrap_max_bytes > 0,
        "worker bootstrap byte cap must be positive"
    );
    validate_sha256_hex(manifest_sha256).context("validate worker manifest SHA-256 pin")?;
    let archive_identity = anonymous_worker_request_archive_identity(archive, true)?;
    ensure!(
        archive_identity.byte_len == archive_bytes,
        "worker request archive length {} does not match launch pin {archive_bytes}",
        archive_identity.byte_len
    );
    ensure!(
        manifest_bytes <= bootstrap_max_bytes,
        "worker request manifest bytes {} exceed configured bootstrap cap {}",
        manifest_bytes,
        bootstrap_max_bytes
    );
    let header_bytes = u64::try_from(std::mem::size_of::<u64>())
        .context("worker request header width does not fit u64")?;
    ensure!(
        archive_bytes >= header_bytes,
        "worker request archive is truncated before its manifest-length header"
    );
    let header = read_worker_archive_range(archive, 0, header_bytes, "manifest-length header")?;
    let encoded_manifest_bytes = u64::from_be_bytes(
        header
            .try_into()
            .map_err(|_| anyhow::anyhow!("worker request manifest-length header width changed"))?,
    );
    ensure!(
        encoded_manifest_bytes == manifest_bytes,
        "worker request manifest-length header does not match launch pin"
    );
    let manifest_body =
        read_worker_archive_range(archive, header_bytes, manifest_bytes, "manifest")?;
    ensure!(
        hex::encode(Sha256::digest(&manifest_body)) == manifest_sha256,
        "worker request manifest SHA-256 mismatch"
    );
    let manifest: SourceUniverseOperatorWorkerRequestManifest =
        serde_json::from_slice(&manifest_body).context("parse worker request manifest")?;
    validate_worker_request_manifest(&manifest)?;
    verify_canonical_worker_request_manifest_bytes(&manifest, &manifest_body)?;
    ensure!(
        worker_request_archive_expected_bytes(manifest_bytes, &manifest)? == archive_bytes,
        "worker request archive has truncated or trailing payload bytes"
    );
    drop(manifest_body);
    let output_metadata = fs::symlink_metadata(&manifest.output_dir).with_context(|| {
        format!(
            "lstat worker operator output dir {}",
            manifest.output_dir.display()
        )
    })?;
    ensure!(
        output_metadata.file_type().is_dir(),
        "worker operator output must be a real directory: {}",
        manifest.output_dir.display()
    );
    ensure!(
        manifest.output_dir.canonicalize()? == manifest.output_dir,
        "worker operator output canonical identity changed"
    );
    let output_lease = PinnedWorkerDirectoryLease::capture(&manifest.output_dir)?;

    let plan_payload = worker_request_payload(&manifest, WORKER_REQUEST_ROLE_EXECUTION_PLAN)?;
    let bootstrap_plan_bytes = read_worker_request_payload_unbudgeted(
        archive,
        manifest_bytes,
        &manifest,
        plan_payload,
        bootstrap_max_bytes,
    )?;
    let bootstrap_plan: BackfillExecutionPlan = serde_json::from_slice(&bootstrap_plan_bytes)
        .context("parse worker bootstrap execution plan")?;
    let work_budget = OperatorWorkBudgetGuard::from_execution_plan_with_absolute_deadline(
        &bootstrap_plan,
        manifest.work_budget_deadline,
    )?;
    let worker_executable = PinnedWorkerExecutable::capture_current()
        .context("capture process-isolated worker executable for attestation")?;
    worker_executable
        .hash_and_revalidate(
            Some(&manifest.execution_attestation.worker_executable_sha256),
            &work_budget,
        )
        .context("verify process-isolated worker executable attestation")?;
    work_budget.verify_decoded_bytes(
        worker_request_retained_peak_bytes(&manifest)?,
        OperatorWorkBudgetStage::ObjectVerification,
    )?;
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    let run_spec_bytes = read_worker_request_payload_guarded(
        archive,
        manifest_bytes,
        &manifest,
        worker_request_payload(&manifest, WORKER_REQUEST_ROLE_RUN_SPEC)?,
        &work_budget,
    )?;
    let accepted_tranche_bytes = read_worker_request_payload_guarded(
        archive,
        manifest_bytes,
        &manifest,
        worker_request_payload(&manifest, WORKER_REQUEST_ROLE_ACCEPTED_TRANCHE)?,
        &work_budget,
    )?;
    let execution_plan_bytes = bootstrap_plan_bytes;
    let source_bindings_bytes = read_worker_request_payload_guarded(
        archive,
        manifest_bytes,
        &manifest,
        worker_request_payload(&manifest, WORKER_REQUEST_ROLE_SOURCE_BINDINGS)?,
        &work_budget,
    )?;
    ensure!(
        anonymous_worker_request_archive_identity(archive, true)? == archive_identity,
        "anonymous worker request archive identity changed before operator execution"
    );

    let validated = validate_backfill_execution_control_bytes(
        &run_spec_bytes,
        &accepted_tranche_bytes,
        &execution_plan_bytes,
    )
    .context("validate worker request typed controls")?;
    ensure!(
        validated.execution_plan == bootstrap_plan,
        "worker bootstrap execution plan changed before guarded verification"
    );
    validate_worker_record_control_alignment(&manifest.record, &validated)?;
    drop((
        run_spec_bytes,
        accepted_tranche_bytes,
        execution_plan_bytes,
        bootstrap_plan,
    ));
    let registry = VerifiedSourceBindingRegistry::from_frozen_pack_bytes(
        &validated.run_spec,
        manifest.source_bindings_path.clone(),
        Arc::from(source_bindings_bytes),
        &manifest.record.source_bindings_sha256,
    )?;
    validate_run_spec_manifest_for_object_hash_with_verified_registry(
        &validated.run_spec,
        &manifest.output_dir,
        &manifest.record.selected_object_sha256,
        &registry,
    )?;
    validate_durable_run_spec_preflight(&validated.run_spec, &registry)
        .context("validate durable source-universe worker RunSpec before source bytes")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build source-universe durable worker runtime")?;
    let dispatcher = runtime.block_on(DurableRunDispatcher::prepare_guarded(
        &validated.run_spec,
        &registry,
        &work_budget,
    ))?;
    output_lease.revalidate()?;
    let response = match manifest.request_kind {
        SourceUniverseOperatorWorkerRequestKind::Execute => {
            let object_bytes = read_worker_request_payload_guarded(
                archive,
                manifest_bytes,
                &manifest,
                worker_request_payload(&manifest, WORKER_REQUEST_ROLE_SELECTED_OBJECT)?,
                &work_budget,
            )?;
            let outcome = runtime.block_on(dispatcher.dispatch_guarded(
                &validated.run_spec,
                object_bytes,
                &manifest.output_dir,
                &registry,
                manifest.execution_attestation.clone(),
                &work_budget,
            ))?;
            let receipt = outcome.into_receipt();
            SourceUniverseOperatorWorkerResponse::Completed { receipt }
        }
        SourceUniverseOperatorWorkerRequestKind::Discover => {
            let discovered = runtime.block_on(dispatcher.discover_current_completion_guarded(
                &validated.run_spec,
                &registry,
                &work_budget,
            ))?;
            output_lease.revalidate()?;
            match discovered {
                Some(receipt) => SourceUniverseOperatorWorkerResponse::Completed { receipt },
                None => SourceUniverseOperatorWorkerResponse::NoCurrentCompletion,
            }
        }
    };
    Ok(response)
}

#[cfg(target_os = "linux")]
struct ExactCanonicalBytesWriter<'a> {
    expected: &'a [u8],
    consumed: usize,
}

#[cfg(target_os = "linux")]
impl ExactCanonicalBytesWriter<'_> {
    fn finish(self) -> Result<()> {
        ensure!(
            self.consumed == self.expected.len(),
            "worker request manifest is not canonical"
        );
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Write for ExactCanonicalBytesWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let end = self.consumed.checked_add(bytes.len()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical worker manifest comparison overflow",
            )
        })?;
        if self.expected.get(self.consumed..end) != Some(bytes) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "worker request manifest is not canonical",
            ));
        }
        self.consumed = end;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn verify_canonical_worker_request_manifest_bytes(
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
    expected: &[u8],
) -> Result<()> {
    let mut verifier = ExactCanonicalBytesWriter {
        expected,
        consumed: 0,
    };
    if let Err(error) = serde_json::to_writer(&mut verifier, manifest) {
        bail!("worker request manifest is not canonical: {error}");
    }
    verifier.finish()
}

#[cfg(target_os = "linux")]
fn worker_request_payload<'a>(
    manifest: &'a SourceUniverseOperatorWorkerRequestManifest,
    role: &str,
) -> Result<&'a SourceUniverseOperatorWorkerRequestPayload> {
    manifest
        .payloads
        .iter()
        .find(|payload| payload.role == role)
        .with_context(|| format!("worker request manifest is missing role {role}"))
}

#[cfg(target_os = "linux")]
fn read_worker_archive_range(
    archive: &fs::File,
    offset: u64,
    bytes: u64,
    role: &str,
) -> Result<Vec<u8>> {
    let len = usize::try_from(bytes)
        .with_context(|| format!("worker request {role} length does not fit usize"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(len)
        .with_context(|| format!("reserve worker request {role}"))?;
    output.resize(len, 0);
    let mut consumed = 0_usize;
    while consumed < output.len() {
        let consumed_u64 = u64::try_from(consumed)
            .with_context(|| format!("worker request {role} offset does not fit u64"))?;
        let absolute = offset
            .checked_add(consumed_u64)
            .with_context(|| format!("worker request {role} offset overflow"))?;
        let read = archive
            .read_at(&mut output[consumed..], absolute)
            .with_context(|| format!("pread worker request {role}"))?;
        ensure!(read > 0, "worker request {role} range is truncated");
        consumed = consumed
            .checked_add(read)
            .with_context(|| format!("worker request {role} read length overflow"))?;
    }
    Ok(output)
}

#[cfg(target_os = "linux")]
fn read_worker_request_payload_unbudgeted(
    archive: &fs::File,
    manifest_bytes: u64,
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
    expected: &SourceUniverseOperatorWorkerRequestPayload,
    bootstrap_max_bytes: u64,
) -> Result<Vec<u8>> {
    ensure!(
        expected.bytes <= bootstrap_max_bytes,
        "worker bootstrap payload {} bytes {} exceed configured bootstrap cap {}",
        expected.role,
        expected.bytes,
        bootstrap_max_bytes
    );
    let bytes = read_worker_archive_range(
        archive,
        worker_request_payload_offset(manifest_bytes, manifest, &expected.role)?,
        expected.bytes,
        &expected.role,
    )?;
    ensure!(
        hex::encode(Sha256::digest(&bytes)) == expected.sha256,
        "worker request payload {} SHA-256 mismatch",
        expected.role
    );
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn read_worker_request_payload_guarded(
    archive: &fs::File,
    manifest_bytes: u64,
    manifest: &SourceUniverseOperatorWorkerRequestManifest,
    expected: &SourceUniverseOperatorWorkerRequestPayload,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    work_budget
        .verify_decoded_bytes(expected.bytes, OperatorWorkBudgetStage::ObjectVerification)?;
    let offset = worker_request_payload_offset(manifest_bytes, manifest, &expected.role)?;
    let output_len = usize::try_from(expected.bytes)
        .context("worker request payload length does not fit usize")?;
    ensure!(output_len > 0, "worker request payload must not be empty");
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_len)
        .context("reserve worker request payload")?;
    output.resize(output_len, 0);
    let mut hasher = Sha256::new();
    let mut consumed = 0_usize;
    while consumed < output.len() {
        work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
        let absolute = offset
            .checked_add(
                u64::try_from(consumed).context("worker request consumed bytes do not fit u64")?,
            )
            .context("worker request positional offset overflow")?;
        let read_outcome = archive.read_at(&mut output[consumed..], absolute);
        work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
        let read = read_outcome
            .with_context(|| format!("pread worker request payload {}", expected.role))?;
        ensure!(
            read > 0,
            "worker request payload {} is truncated",
            expected.role
        );
        hasher.update(&output[consumed..consumed + read]);
        consumed = consumed
            .checked_add(read)
            .context("worker request consumed length overflow")?;
    }
    ensure!(
        hex::encode(hasher.finalize()) == expected.sha256,
        "worker request payload {} SHA-256 mismatch",
        expected.role
    );
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    Ok(output)
}

#[cfg(test)]
fn execute_source_universe_batch_with_pinned_artifacts<F, R>(
    batch_id: &str,
    launch_artifacts: &SourceUniverseBatchLaunchArtifacts,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    execute_source_universe_batch_with_clock_factory(
        batch_id,
        launch_artifacts,
        output_dir,
        config,
        fetcher,
        runner,
        &SystemSourceUniverseWorkBudgetClockFactory,
    )
}

#[cfg(test)]
fn execute_source_universe_batch_with_clock_factory<F, R, G>(
    batch_id: &str,
    launch_artifacts: &SourceUniverseBatchLaunchArtifacts,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
    clock_factory: &G,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
    G: SourceUniverseWorkBudgetClockFactory,
{
    let owned_plan = prepare_batch(batch_id, launch_artifacts, output_dir, &config)?;
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
            clock_factory,
            TEST_SOURCE_UNIVERSE_LIFECYCLE_CLEANUP_LIMITS,
            BatchCompletionPolicy::AllowPublish,
        );
        let stop = matches!(slot, RecordSlot::Stopped(_));
        slots[slot_index] = Some(slot);
        if stop {
            // Serial stop-on-error: the first error reached is also the lowest
            // sequence, matching the parallel lowest-sequence rule.
            return Err(lowest_sequence_error(slots));
        }
    }
    assemble_report(batch_id, &owned_plan, slots)
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
/// Parallel production entry: every bootstrap artifact is supplied with its
/// operator-selected length and digest.
#[cfg(test)]
fn execute_source_universe_batch_with_pinned_artifacts_factories<F, R>(
    batch_id: &str,
    launch_artifacts: &SourceUniverseBatchLaunchArtifacts,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    lifecycle_cleanup_limits: SourceUniverseLifecycleCleanupLimits,
    fetcher_factory: impl Fn() -> Result<F> + Sync,
    runner_factory: impl Fn() -> Result<R> + Sync,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let owned_plan = prepare_batch(batch_id, launch_artifacts, output_dir, &config)?;
    execute_prepared_source_universe_batch_with_factories(
        batch_id,
        &owned_plan,
        config,
        lifecycle_cleanup_limits,
        BatchCompletionPolicy::AllowPublish,
        fetcher_factory,
        runner_factory,
    )
}

fn execute_prepared_source_universe_batch_with_factories<F, R>(
    batch_id: &str,
    owned_plan: &OwnedBatchPlan,
    config: SourceUniverseBatchExecutionConfig,
    lifecycle_cleanup_limits: SourceUniverseLifecycleCleanupLimits,
    completion_policy: BatchCompletionPolicy,
    fetcher_factory: impl Fn() -> Result<F> + Sync,
    runner_factory: impl Fn() -> Result<R> + Sync,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let clock_factory = SystemSourceUniverseWorkBudgetClockFactory;
    let plan = owned_plan.plan();
    let work_item_count = plan.work_items.len();
    if work_item_count == 0 {
        return assemble_report(batch_id, owned_plan, Vec::new());
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
                // This isolated workspace does not override Cargo's unwind
                // panic strategy, including in release builds. Convert an
                // otherwise unclassified supervisor panic into the same typed
                // hard-stop used for uncertain durable terminal ownership;
                // the outer join remains a second fail-closed boundary.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<()> {
                        let mut fetcher: Option<F> = None;
                        let mut runner: Option<R> = None;
                        loop {
                            if stop_flag.load(Ordering::SeqCst) {
                                break;
                            }
                            let index = next_index.fetch_add(1, Ordering::SeqCst);
                            if index >= work_items.len() {
                                break;
                            }
                            let work_item = &work_items[index];
                            let slot = match resolve_work_item(work_item, config, &clock_factory) {
                                ResolvedBatchWorkItem::Terminal(slot) => *slot,
                                ResolvedBatchWorkItem::Fresh(fresh) => {
                                    let record = fresh.record;
                                    let execution_record_sha256 = fresh.execution_record_sha256;
                                    match (|| -> Result<RecordSlot> {
                                        if runner.is_none() {
                                            runner = Some(
                                                runner_factory()
                                                    .context("construct batch worker runner")?,
                                            );
                                        }
                                        let mut lazy_fetcher = LazySourceUniverseObjectFetcher {
                                            inner: &mut fetcher,
                                            factory: fetcher_factory,
                                        };
                                        Ok(process_fresh_work_item(
                                            fresh,
                                            output_root_lease,
                                            config,
                                            &mut lazy_fetcher,
                                            runner.as_mut().expect("batch runner initialized"),
                                            lifecycle_cleanup_limits,
                                            completion_policy,
                                        ))
                                    })() {
                                        Ok(slot) => slot,
                                        Err(error) => record_error_slot(
                                            record,
                                            execution_record_sha256,
                                            "construct_worker_dependencies",
                                            error,
                                            config,
                                        ),
                                    }
                                }
                            };
                            if matches!(slot, RecordSlot::Stopped(_)) {
                                stop_flag.store(true, Ordering::SeqCst);
                            }
                            *slots[index].lock().expect("batch slot mutex") = Some(slot);
                        }
                        Ok(())
                    },
                ));
                match outcome {
                    Ok(result) => result,
                    Err(_) => {
                        stop_flag.store(true, Ordering::SeqCst);
                        Err(committed_indeterminate_worker_error(
                            "batch worker thread panicked before durable terminal ownership was proven",
                        ))
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().map_err(|_| {
                committed_indeterminate_worker_error(
                    "batch worker panic escaped the supervised panic boundary",
                )
            })??;
        }
        Ok(())
    })?;

    let slots: Vec<Option<RecordSlot>> = slots
        .into_iter()
        .map(|slot| slot.into_inner().expect("batch slot mutex"))
        .collect();
    if slots
        .iter()
        .any(|slot| matches!(slot, Some(RecordSlot::Stopped(_))))
    {
        return Err(lowest_sequence_error(slots));
    }
    assemble_report(batch_id, owned_plan, slots)
}

/// Sole production batch entry: selected records execute in a pinned
/// same-binary child and can complete only with an exact durable locator.
/// Generic runner injection remains module-private for unit tests.
pub fn execute_source_universe_batch_process_isolated<F>(
    batch_id: &str,
    launch_artifacts: &SourceUniverseBatchLaunchArtifacts,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    fetcher_factory: impl Fn() -> Result<F> + Sync,
    request_root: PathBuf,
    worker_termination_grace_seconds: u64,
    resource_limits: SourceUniverseBatchResourceLimits,
    local_storage_policy: &SourceUniverseLocalStoragePolicy,
    local_storage_lease: &SourceUniverseLocalStorageLease,
    lifecycle_cleanup_limits: SourceUniverseLifecycleCleanupLimits,
) -> Result<SourceUniversePublishedBatchExecution>
where
    F: SourceUniverseObjectFetcher,
{
    validate_process_isolated_batch_selection(config.record_limit, config.max_concurrent_records)?;
    let max_concurrent_records = config
        .max_concurrent_records
        .expect("exact process-isolated concurrency was checked");
    ensure!(
        worker_termination_grace_seconds > 0,
        "worker_termination_grace_seconds must be positive"
    );
    let resource_limits = resource_limits.validate()?;
    let lifecycle_cleanup_limits = lifecycle_cleanup_limits.validate()?;
    let worker_termination_grace = Duration::from_secs(worker_termination_grace_seconds);
    let owned_plan = prepare_batch(batch_id, launch_artifacts, output_dir, &config)?;
    let existing_report = read_existing_batch_report_with_lease(
        &owned_plan.output_root_lease,
        launch_artifacts
            .bootstrap_limits
            .max_retained_control_input_bytes,
    )?;
    if let Some(existing) = existing_report.as_ref() {
        validate_existing_batch_report_selection(existing, batch_id, &owned_plan)?;
    }
    let record_envelope = owned_plan.process_isolated_record_resource_envelope(
        launch_artifacts.bootstrap_limits,
        resource_limits,
    )?;
    local_storage_lease
        .verify_pre_record_admission(
            local_storage_policy,
            record_envelope.local_storage_required_bytes,
        )
        .context("verify selected-record local-storage admission reserve before fetch")?;
    let report = execute_prepared_source_universe_batch_with_factories(
        batch_id,
        &owned_plan,
        config,
        lifecycle_cleanup_limits,
        if existing_report.is_some() {
            BatchCompletionPolicy::RequireExistingRemoteTerminal
        } else {
            BatchCompletionPolicy::AllowPublish
        },
        fetcher_factory,
        move || {
            ProcessIsolatedSourceUniverseOperatorRunner::new(
                request_root.clone(),
                local_storage_lease.duplicate_owner_lock_for_worker()?,
                max_concurrent_records,
                worker_termination_grace,
                resource_limits,
            )
        },
    )?;
    let report = if let Some(existing) = existing_report {
        validate_existing_batch_report_against_remote_discovery(&existing, &report)?;
        existing
    } else {
        report
    };
    local_storage_lease
        .verify_observed_terminal_boundedness(local_storage_policy)
        .context("verify observed local-storage boundedness after record-attempt compaction")?;
    let artifact = write_source_universe_batch_execution_report_with_lease(
        &owned_plan.output_root_lease,
        &report,
    )?;
    local_storage_lease
        .verify_observed_terminal_boundedness(local_storage_policy)
        .context("verify observed local-storage boundedness after batch report publication")?;
    Ok(SourceUniversePublishedBatchExecution { report, artifact })
}

/// A single unit of batch work after selection: either a control-artifact
/// preflight failure collected under `continue_on_error` or a pack record that
/// requires exact-current completion discovery before fetch and execution.
enum BatchWorkItem<'pack> {
    PreflightFailed {
        record: &'pack SourceUniverseExecutionPackRecord,
        error: &'pack str,
        execution_record_sha256: &'pack str,
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
    work_budget: OperatorWorkBudgetGuard,
}

enum ResolvedBatchWorkItem<'pack> {
    Terminal(Box<RecordSlot>),
    Fresh(FreshBatchWorkItem<'pack>),
}

/// Outcome for one work item, kept in original-sequence slot order so the
/// assembled report is independent of completion order.
enum RecordSlot {
    Completed(SourceUniverseBatchExecutionRecord),
    Discovered(SourceUniverseBatchExecutionRecord),
    Failed(SourceUniverseBatchExecutionFailureRecord),
    Stopped(StoppedRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchCompletionPolicy {
    AllowPublish,
    RequireExistingRemoteTerminal,
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
    component: String,
    canonical_path: PathBuf,
    owned: OwnedTempDirectory,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Debug)]
struct BatchOutputClaimFailure {
    attempt: SourceUniverseBatchExecutionAttemptIdentity,
    detail: String,
}

impl fmt::Display for BatchOutputClaimFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "owned output attempt {} could not be fully pinned: {}",
            self.attempt.output_dir.display(),
            self.detail
        )
    }
}

impl std::error::Error for BatchOutputClaimFailure {}

fn claimed_attempt_identity(path: &Path) -> SourceUniverseBatchExecutionAttemptIdentity {
    #[cfg(unix)]
    let (device, inode) = fs::symlink_metadata(path)
        .map(|metadata| (Some(metadata.dev()), Some(metadata.ino())))
        .unwrap_or((None, None));
    SourceUniverseBatchExecutionAttemptIdentity {
        output_dir: path.to_path_buf(),
        #[cfg(unix)]
        device,
        #[cfg(not(unix))]
        device: None,
        #[cfg(unix)]
        inode,
        #[cfg(not(unix))]
        inode: None,
    }
}

fn attempt_identity_from_claim_error(
    error: &anyhow::Error,
) -> Option<SourceUniverseBatchExecutionAttemptIdentity> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<BatchOutputClaimFailure>()
            .map(|failure| failure.attempt.clone())
    })
}

impl BatchOutputChildClaim {
    fn acquire(
        root: &BatchOutputRootLease,
        operator_run_id: &str,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<Self> {
        root.revalidate()?;
        validate_portable_path_component("operator_run_id", operator_run_id)?;
        let (attempt_path, retained_path_bytes) = unique_temp_path_guarded(
            &root.canonical_path.join(operator_run_id),
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .context("derive guarded unique operator output attempt")?;
        let component = attempt_path
            .file_name()
            .and_then(|name| name.to_str())
            .context("unique operator output attempt is not UTF-8")?
            .to_string();
        validate_portable_path_component("operator_output_attempt", &component)?;
        let owned = create_owned_temp_directory_guarded(
            attempt_path.clone(),
            retained_path_bytes,
            work_budget,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .with_context(|| {
            format!(
                "claim unique operator output attempt {}",
                attempt_path.display()
            )
        })?;
        let attempt_identity = claimed_attempt_identity(owned.path());
        (|| -> Result<Self> {
            initialize_owned_temp_directory_receipt_guarded(
                &owned,
                std::ffi::OsStr::new(SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE),
                SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES,
                work_budget,
                OperatorWorkBudgetStage::ObjectVerification,
            )
            .context("initialize record-attempt lifecycle receipt")?;
            let canonical_path =
                resolve_contained_output_component(&root.canonical_path, &component)?;
            ensure!(
                canonical_path == owned.path(),
                "claimed operator output canonical path changed during acquisition"
            );
            let metadata = fs::symlink_metadata(&canonical_path).with_context(|| {
                format!("stat claimed operator output {}", canonical_path.display())
            })?;
            ensure!(
                metadata.is_dir(),
                "claimed operator output {} must be a directory",
                canonical_path.display()
            );
            let claim = Self {
                component,
                canonical_path,
                owned,
                #[cfg(unix)]
                device: metadata.dev(),
                #[cfg(unix)]
                inode: metadata.ino(),
            };
            claim.revalidate(root)?;
            Ok(claim)
        })()
        .map_err(|error| {
            BatchOutputClaimFailure {
                attempt: attempt_identity,
                detail: format!("{error:#}"),
            }
            .into()
        })
    }

    fn revalidate(&self, root: &BatchOutputRootLease) -> Result<PathBuf> {
        let canonical_now = root.revalidate_child(&self.component)?;
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
        self.owned.revalidate_namespace().with_context(|| {
            format!(
                "revalidate owned operator output {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            path_metadata.is_dir(),
            "claimed operator output {} is no longer a directory",
            self.canonical_path.display()
        );
        #[cfg(unix)]
        ensure!(
            path_metadata.dev() == self.device && path_metadata.ino() == self.inode,
            "claimed operator output {} device/inode identity changed",
            self.canonical_path.display()
        );
        Ok(canonical_now)
    }

    fn attempt_identity(&self) -> SourceUniverseBatchExecutionAttemptIdentity {
        SourceUniverseBatchExecutionAttemptIdentity {
            output_dir: self.canonical_path.clone(),
            #[cfg(unix)]
            device: Some(self.device),
            #[cfg(not(unix))]
            device: None,
            #[cfg(unix)]
            inode: Some(self.inode),
            #[cfg(not(unix))]
            inode: None,
        }
    }

    fn compact_to_receipt(
        &self,
        root: &BatchOutputRootLease,
        limits: SourceUniverseLifecycleCleanupLimits,
    ) -> Result<()> {
        self.revalidate(root)?;
        compact_owned_temp_directory_to_receipt_bounded(
            &self.owned,
            std::ffi::OsStr::new(SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE),
            SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES,
            limits.max_entries,
            limits.max_depth,
        )
        .context("compact terminal record attempt to lifecycle receipt")?;
        self.revalidate(root)?;
        Ok(())
    }
}

#[derive(Serialize)]
struct ExecutionRecordFingerprint<'a> {
    pack_context_sha256: &'a str,
    record: &'a SourceUniverseExecutionPackRecord,
}

#[derive(Serialize)]
struct ExecutionPackContextArtifactRef<'a> {
    path: &'a Path,
    role: &'a str,
    sha256: &'a str,
}

struct ExecutionPackContextArtifactRefs<'a>(&'a [ReferenceArtifactPin]);

impl Serialize for ExecutionPackContextArtifactRefs<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for pin in self.0 {
            sequence.serialize_element(&ExecutionPackContextArtifactRef {
                path: &pin.path,
                role: &pin.role,
                sha256: &pin.sha256,
            })?;
        }
        sequence.end()
    }
}

/// Borrowed, record-free execution-pack context. Fields are intentionally in
/// alphabetical order: the predecessor implementation serialized the complete
/// pack through `serde_json::Value`, whose map ordering was alphabetical, then
/// removed `records`. Keeping that byte order preserves existing record
/// fingerprints without cloning the complete pack or its records.
#[derive(Serialize)]
struct ExecutionPackContext<'a> {
    artifact_refs: ExecutionPackContextArtifactRefs<'a>,
    blocking_reasons: &'a [String],
    conversion_run_plan_id: &'a str,
    executable_record_count: u64,
    executable_source_bytes: u64,
    family: &'a str,
    gate_id: &'a str,
    input_id: &'a str,
    materialized_record_count: u64,
    materialized_source_bytes: u64,
    pack_id: &'a str,
    planned_object_count: u64,
    schema_version: &'a str,
    selected_record_count: u64,
    skipped_executable_record_count: u64,
    source: &'a str,
    status: &'a SourceUniverseExecutionPackStatus,
    table_family: &'a str,
    universe_id: &'a str,
    venue: &'a str,
    withheld_record_count: u64,
    work_order_id: &'a str,
}

fn execution_pack_context_sha256(pack: &SourceUniverseExecutionPack) -> Result<String> {
    crate::reference_artifact::canonical_json_sha256(&ExecutionPackContext {
        artifact_refs: ExecutionPackContextArtifactRefs(&pack.artifact_refs),
        blocking_reasons: &pack.blocking_reasons,
        conversion_run_plan_id: &pack.conversion_run_plan_id,
        executable_record_count: pack.executable_record_count,
        executable_source_bytes: pack.executable_source_bytes,
        family: &pack.family,
        gate_id: &pack.gate_id,
        input_id: &pack.input_id,
        materialized_record_count: pack.materialized_record_count,
        materialized_source_bytes: pack.materialized_source_bytes,
        pack_id: &pack.pack_id,
        planned_object_count: pack.planned_object_count,
        schema_version: &pack.schema_version,
        selected_record_count: pack.selected_record_count,
        skipped_executable_record_count: pack.skipped_executable_record_count,
        source: &pack.source,
        status: &pack.status,
        table_family: &pack.table_family,
        universe_id: &pack.universe_id,
        venue: &pack.venue,
        withheld_record_count: pack.withheld_record_count,
        work_order_id: &pack.work_order_id,
    })
    .context("hash execution-pack non-record context")
}

/// Compute every record fingerprint from one digest of the pack's non-record
/// context. Removing `records` and hashing the remaining context once prevents
/// unbounded pack metadata (for example `artifact_refs`) from being
/// canonicalized again for every record.
fn execution_record_digests(pack: &SourceUniverseExecutionPack) -> Result<BTreeMap<u64, String>> {
    let pack_context_sha256 = execution_pack_context_sha256(pack)?;

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

/// Return the canonical digest for one execution-pack record without exposing
/// or duplicating the pack-context fingerprint algorithm.
pub(crate) fn execution_record_digest(
    pack: &SourceUniverseExecutionPack,
    sequence: u64,
) -> Result<String> {
    execution_record_digests(pack)?
        .remove(&sequence)
        .with_context(|| {
            format!(
                "execution pack {} has no record with sequence {sequence}",
                pack.pack_id
            )
        })
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
        record.source_bindings_path == run_spec.source_bindings_path,
        "pack record source-bindings path does not match retained run spec"
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
    launch_artifacts: &SourceUniverseBatchLaunchArtifacts,
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

    let execution_pack_path = &launch_artifacts.execution_pack.path;
    let pack_bytes = read_launch_artifact(
        &launch_artifacts.execution_pack,
        launch_artifacts.bootstrap_limits.max_launch_artifact_bytes,
    )
    .with_context(|| {
        format!(
            "read pinned execution pack {}",
            execution_pack_path.display()
        )
    })?;
    let pack: SourceUniverseExecutionPack = serde_json::from_slice(&pack_bytes)
        .with_context(|| format!("parse execution pack {}", execution_pack_path.display()))?;
    validate_execution_pack_semantics(&pack)?;
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
    let record_limit = config
        .record_limit
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);
    let selected_record_count = pack
        .records
        .iter()
        .filter(|record| {
            config
                .start_sequence
                .is_none_or(|start_sequence| record.sequence >= start_sequence)
        })
        .take(record_limit)
        .count();
    ensure!(
        selected_record_count > 0,
        "batch selection for execution pack {} selects zero records (start_sequence={:?}, record_limit={})",
        pack.pack_id,
        config.start_sequence,
        record_limit
    );
    let execution_record_sha256s = execution_record_digests(&pack)?;
    let control_input_envelope = validate_selected_control_input_envelope(
        &pack,
        &pack_base_dir,
        config.start_sequence,
        record_limit,
        launch_artifacts.bootstrap_limits,
    )?;
    let preflight_output_root = if output_dir.is_absolute() {
        output_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolve current directory for batch output preflight")?
            .join(output_dir)
    };

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

    // Bind the selected records to the exact control bytes pinned by the pack
    // before cache access, source fetches, or worker construction. Selection is
    // intentional: committed campaign packs retain
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
            let prospective_output_dir = resolve_contained_output_component(
                &preflight_output_root,
                &record.operator_run_id,
            )?;
            verify_pack_control_artifacts(PackControlVerificationInput {
                pack: &pack,
                pack_base_dir: &pack_base_dir,
                record,
                record_output_dir: &prospective_output_dir,
                control_input_envelope: &control_input_envelope,
                verified_artifact_cache: &mut verified_artifact_cache,
                verified_registry_cache: &mut verified_registry_cache,
                limits: launch_artifacts.bootstrap_limits,
            })
        })();
        match preflight {
            Ok(verified) => {
                validate_durable_run_spec_preflight(&verified.run_spec, &verified.source_bindings)
                    .with_context(|| {
                        format!(
                            "durable preflight for selected pack record {} ({})",
                            record.sequence, record.operator_run_id
                        )
                    })?;
                verified_control_artifacts.insert(record.sequence, verified);
            }
            Err(error) if config.continue_on_error => {
                control_artifact_failures.insert(record.sequence, format!("{error:#}"));
            }
            Err(error) => return Err(error),
        }
    }

    // No selected source-universe record can create output until every
    // selected RunSpec has proved the sole durable store/SSM/dispatch/capability
    // contract. Ordinary per-record control failures may still be represented
    // in a report when continue_on_error is configured.
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create batch output dir {}", output_dir.display()))?;
    let output_root_lease = BatchOutputRootLease::acquire(output_dir)?;

    Ok(OwnedBatchPlan {
        pack,
        execution_record_sha256s,
        verified_control_artifacts,
        control_artifact_failures,
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

struct ControlArtifactPin<'record> {
    role: &'static str,
    sha256_field: &'static str,
    declared_path: &'record Path,
    expected_bytes: u64,
    expected_sha256: &'record str,
}

fn control_artifact_pins(
    record: &SourceUniverseExecutionPackRecord,
) -> [ControlArtifactPin<'_>; 4] {
    [
        ControlArtifactPin {
            role: "run_spec",
            sha256_field: "run_spec_sha256",
            declared_path: &record.run_spec_path,
            expected_bytes: record.run_spec_bytes,
            expected_sha256: &record.run_spec_sha256,
        },
        ControlArtifactPin {
            role: "accepted_tranche",
            sha256_field: "accepted_tranche_sha256",
            declared_path: &record.accepted_tranche_path,
            expected_bytes: record.accepted_tranche_bytes,
            expected_sha256: &record.accepted_tranche_sha256,
        },
        ControlArtifactPin {
            role: "execution_plan",
            sha256_field: "execution_plan_sha256",
            declared_path: &record.execution_plan_path,
            expected_bytes: record.execution_plan_bytes,
            expected_sha256: &record.execution_plan_sha256,
        },
        ControlArtifactPin {
            role: "source_bindings",
            sha256_field: "source_bindings_sha256",
            declared_path: &record.source_bindings_path,
            expected_bytes: record.source_bindings_bytes,
            expected_sha256: &record.source_bindings_sha256,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ControlEnvelopeIdentity {
    Resolved(PathBuf),
    UnresolvedDeclared(PathBuf),
}

#[derive(Debug)]
enum PlannedControlPath {
    Resolved(PathBuf),
    Rejected(String),
}

#[derive(Debug)]
struct SelectedControlInputEnvelope {
    paths: BTreeMap<(u64, &'static str), PlannedControlPath>,
}

impl SelectedControlInputEnvelope {
    fn resolved_path(
        &self,
        record: &SourceUniverseExecutionPackRecord,
        role: &'static str,
    ) -> Result<&Path> {
        match self.paths.get(&(record.sequence, role)).with_context(|| {
            format!(
                "selected control envelope is missing record {} role {role}",
                record.sequence
            )
        })? {
            PlannedControlPath::Resolved(path) => Ok(path),
            PlannedControlPath::Rejected(error) => bail!(
                "pack record {} (operator_run_id {}) pinned artifact {role} path was rejected during frozen envelope preflight: {error}",
                record.sequence,
                record.operator_run_id
            ),
        }
    }
}

struct PackControlVerificationInput<'a> {
    pack: &'a SourceUniverseExecutionPack,
    pack_base_dir: &'a Path,
    record: &'a SourceUniverseExecutionPackRecord,
    record_output_dir: &'a Path,
    control_input_envelope: &'a SelectedControlInputEnvelope,
    verified_artifact_cache: &'a mut BTreeMap<PathBuf, VerifiedArtifactContent>,
    verified_registry_cache: &'a mut BTreeMap<PathBuf, VerifiedSourceBindingRegistry>,
    limits: SourceUniverseBatchBootstrapLimits,
}

fn validate_selected_control_input_envelope(
    pack: &SourceUniverseExecutionPack,
    pack_base_dir: &Path,
    start_sequence: Option<u64>,
    record_limit: usize,
    limits: SourceUniverseBatchBootstrapLimits,
) -> Result<SelectedControlInputEnvelope> {
    let limits = limits.validate()?;
    let selected_records = || {
        pack.records
            .iter()
            .filter(|record| start_sequence.is_none_or(|start| record.sequence >= start))
            .take(record_limit)
    };

    // Validate every allocation scalar before touching any selected path. This
    // ensures one later oversized pin cannot make an earlier pin trigger
    // filesystem access first. Digest and cross-record identity errors remain
    // in per-record control preflight so `continue_on_error` retains its
    // documented isolation semantics.
    for record in selected_records() {
        for pin in control_artifact_pins(record) {
            ensure!(
                pin.expected_bytes > 0,
                "pack record {} (operator_run_id {}) pinned artifact {} must declare a positive byte length",
                record.sequence,
                record.operator_run_id,
                pin.role
            );
            ensure!(
                pin.expected_bytes <= limits.max_control_artifact_bytes,
                "pack record {} (operator_run_id {}) pinned artifact {} declares {} bytes, exceeding bootstrap_limits.max_control_artifact_bytes {}",
                record.sequence,
                record.operator_run_id,
                pin.role,
                pin.expected_bytes,
                limits.max_control_artifact_bytes
            );
        }
    }

    let mut unique_raw_controls: BTreeMap<ControlEnvelopeIdentity, u64> = BTreeMap::new();
    let mut parsed_source_bindings: BTreeMap<ControlEnvelopeIdentity, u64> = BTreeMap::new();
    let mut planned_paths = BTreeMap::new();
    let mut retained_input_bytes = 0_u64;

    let mut charge = |label: &str, bytes: u64| -> Result<()> {
        retained_input_bytes = retained_input_bytes
            .checked_add(bytes)
            .with_context(|| format!("{label} retained control input byte total overflow"))?;
        ensure!(
            retained_input_bytes <= limits.max_retained_control_input_bytes,
            "{label} raises retained control input bytes to {retained_input_bytes}, exceeding bootstrap_limits.max_retained_control_input_bytes {}",
            limits.max_retained_control_input_bytes
        );
        Ok(())
    };

    for record in selected_records() {
        for pin in control_artifact_pins(record) {
            let retirement_guard = ensure_active_backfill_runtime_path(pin.declared_path);
            let (identity, planned_path) = match retirement_guard
                .and_then(|()| resolve_pack_control_path(pack_base_dir, pin.declared_path))
            {
                Ok(path) => (
                    ControlEnvelopeIdentity::Resolved(path.clone()),
                    PlannedControlPath::Resolved(path),
                ),
                // Missing or otherwise invalid paths are retained as declared
                // identities here so `continue_on_error` can still classify
                // them per record during control preflight. Existing aliases
                // resolve to the same identity and are charged once, matching
                // the retained byte caches.
                Err(error) => (
                    ControlEnvelopeIdentity::UnresolvedDeclared(pin.declared_path.to_path_buf()),
                    PlannedControlPath::Rejected(format!("{error:#}")),
                ),
            };
            ensure!(
                planned_paths
                    .insert((record.sequence, pin.role), planned_path)
                    .is_none(),
                "execution pack {} has duplicate selected sequence {} role {}",
                pack.pack_id,
                record.sequence,
                pin.role
            );
            if let Some(previous_bytes) = unique_raw_controls.get_mut(&identity) {
                if pin.expected_bytes > *previous_bytes {
                    charge(
                        "larger alias of unique raw control",
                        pin.expected_bytes - *previous_bytes,
                    )?;
                    *previous_bytes = pin.expected_bytes;
                }
            } else {
                charge("unique raw control", pin.expected_bytes)?;
                unique_raw_controls.insert(identity.clone(), pin.expected_bytes);
            }

            if pin.role == "source_bindings" {
                if let Some(previous_bytes) = parsed_source_bindings.get_mut(&identity) {
                    if pin.expected_bytes > *previous_bytes {
                        charge(
                            "larger alias of parsed source-bindings control",
                            pin.expected_bytes - *previous_bytes,
                        )?;
                        *previous_bytes = pin.expected_bytes;
                    }
                } else {
                    charge("parsed source-bindings control", pin.expected_bytes)?;
                    parsed_source_bindings.insert(identity, pin.expected_bytes);
                }
            }
        }

        let parsed_control_triple_bytes = record
            .run_spec_bytes
            .checked_add(record.accepted_tranche_bytes)
            .and_then(|bytes| bytes.checked_add(record.execution_plan_bytes))
            .context("parsed control triple input byte total overflow")?;
        charge("parsed control triple", parsed_control_triple_bytes)?;
    }
    Ok(SelectedControlInputEnvelope {
        paths: planned_paths,
    })
}

fn verify_pack_control_artifacts(
    input: PackControlVerificationInput<'_>,
) -> Result<SourceUniverseVerifiedControlArtifacts> {
    let PackControlVerificationInput {
        pack,
        pack_base_dir,
        record,
        record_output_dir,
        control_input_envelope,
        verified_artifact_cache,
        verified_registry_cache,
        limits,
    } = input;
    let verified_run_spec = verify_pack_control_artifact(
        record,
        ControlArtifactPin {
            role: "run_spec",
            sha256_field: "run_spec_sha256",
            declared_path: &record.run_spec_path,
            expected_bytes: record.run_spec_bytes,
            expected_sha256: &record.run_spec_sha256,
        },
        control_input_envelope.resolved_path(record, "run_spec")?,
        pack_base_dir,
        verified_artifact_cache,
        limits,
    )?;
    #[cfg(test)]
    let run_spec_path = verified_run_spec.0.clone();
    let run_spec_bytes = verified_run_spec.1;
    let verified_accepted_tranche = verify_pack_control_artifact(
        record,
        ControlArtifactPin {
            role: "accepted_tranche",
            sha256_field: "accepted_tranche_sha256",
            declared_path: &record.accepted_tranche_path,
            expected_bytes: record.accepted_tranche_bytes,
            expected_sha256: &record.accepted_tranche_sha256,
        },
        control_input_envelope.resolved_path(record, "accepted_tranche")?,
        pack_base_dir,
        verified_artifact_cache,
        limits,
    )?;
    #[cfg(test)]
    let accepted_tranche_path = verified_accepted_tranche.0.clone();
    let accepted_tranche_bytes = verified_accepted_tranche.1;
    let verified_execution_plan = verify_pack_control_artifact(
        record,
        ControlArtifactPin {
            role: "execution_plan",
            sha256_field: "execution_plan_sha256",
            declared_path: &record.execution_plan_path,
            expected_bytes: record.execution_plan_bytes,
            expected_sha256: &record.execution_plan_sha256,
        },
        control_input_envelope.resolved_path(record, "execution_plan")?,
        pack_base_dir,
        verified_artifact_cache,
        limits,
    )?;
    #[cfg(test)]
    let execution_plan_path = verified_execution_plan.0.clone();
    let execution_plan_bytes = verified_execution_plan.1;

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

    let (source_bindings_path, source_bindings_bytes) = verify_pack_control_artifact(
        record,
        ControlArtifactPin {
            role: "source_bindings",
            sha256_field: "source_bindings_sha256",
            declared_path: &record.source_bindings_path,
            expected_bytes: record.source_bindings_bytes,
            expected_sha256: &record.source_bindings_sha256,
        },
        control_input_envelope.resolved_path(record, "source_bindings")?,
        pack_base_dir,
        verified_artifact_cache,
        limits,
    )?;
    let verified_registry =
        if let Some(verified) = verified_registry_cache.get(&source_bindings_path) {
            ensure!(
                verified.resolved_path() == source_bindings_path,
                "cached source-bindings registry resolved path does not match pack record"
            );
            ensure!(
                verified.sha256() == record.source_bindings_sha256,
                "cached source-bindings registry digest does not match pack record"
            );
            verified.reassert_for(&validated.run_spec)?;
            verified.clone()
        } else {
            let verified = VerifiedSourceBindingRegistry::from_frozen_pack_bytes(
                &validated.run_spec,
                source_bindings_path.clone(),
                source_bindings_bytes.clone(),
                &record.source_bindings_sha256,
            )?;
            verified_registry_cache.insert(source_bindings_path.clone(), verified.clone());
            verified
        };

    validate_run_spec_manifest_for_object_hash_with_verified_registry(
        &validated.run_spec,
        record_output_dir,
        &record.selected_object_sha256,
        &verified_registry,
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
        #[cfg(test)]
        run_spec_path,
        run_spec_bytes,
        run_spec: Arc::new(validated.run_spec),
        #[cfg(test)]
        accepted_tranche_path,
        accepted_tranche_bytes,
        #[cfg(test)]
        execution_plan_path,
        execution_plan_bytes,
        execution_plan: Arc::new(validated.execution_plan),
        source_bindings_path,
        source_bindings_bytes,
        source_bindings_sha256: verified_registry.sha256().to_string(),
        source_bindings: verified_registry,
    })
}

fn verify_pack_control_artifact(
    record: &SourceUniverseExecutionPackRecord,
    pin: ControlArtifactPin<'_>,
    resolved_path: &Path,
    pack_base_dir: &Path,
    verified_artifact_cache: &mut BTreeMap<PathBuf, VerifiedArtifactContent>,
    limits: SourceUniverseBatchBootstrapLimits,
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
    ensure!(
        pin.expected_bytes > 0,
        "pack record {} (operator_run_id {}) pinned artifact {} must declare a positive byte length",
        record.sequence,
        record.operator_run_id,
        pin.role
    );
    ensure!(
        pin.expected_bytes <= limits.max_control_artifact_bytes,
        "pack record {} (operator_run_id {}) pinned artifact {} declares {} bytes, exceeding bootstrap_limits.max_control_artifact_bytes {}",
        record.sequence,
        record.operator_run_id,
        pin.role,
        pin.expected_bytes,
        limits.max_control_artifact_bytes
    );

    let current_resolved_path = resolve_pack_control_path(pack_base_dir, pin.declared_path)
        .with_context(|| {
            format!(
                "re-resolve pack record {} ({}) pinned artifact {} declared at {}",
                record.sequence,
                record.operator_run_id,
                pin.role,
                pin.declared_path.display()
            )
        })?;
    ensure!(
        current_resolved_path == resolved_path,
        "pack record {} (operator_run_id {}) pinned artifact {} resolved path changed after envelope accounting",
        record.sequence,
        record.operator_run_id,
        pin.role
    );
    let resolved_path = resolved_path.to_path_buf();
    let cache_miss = !verified_artifact_cache.contains_key(&resolved_path);
    let verified = if let Some(verified) = verified_artifact_cache.get(&resolved_path) {
        verified.clone()
    } else {
        let (mut file, identity) = open_pinned_regular_file(&resolved_path).with_context(|| {
            format!(
                "open pack record {} (operator_run_id {}) pinned artifact {} at {} \
                 (declared {}) without following symlinks",
                record.sequence,
                record.operator_run_id,
                pin.role,
                resolved_path.display(),
                pin.declared_path.display(),
            )
        })?;
        ensure!(
            identity.byte_len == pin.expected_bytes,
            "pack record {} (operator_run_id {}) pinned artifact {} length mismatch at {}: expected {}, got {}",
            record.sequence,
            record.operator_run_id,
            pin.role,
            resolved_path.display(),
            pin.expected_bytes,
            identity.byte_len
        );
        let bytes = read_exact_pinned_file(&mut file, &resolved_path, pin.expected_bytes)?;
        identity.revalidate_path(&resolved_path)?;
        identity.revalidate_handle(&resolved_path, &file)?;
        VerifiedArtifactContent {
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: Arc::<[u8]>::from(bytes),
        }
    };

    ensure!(
        u64::try_from(verified.bytes.len())
            .context("verified control byte length does not fit u64")?
            == pin.expected_bytes,
        "pack record {} (operator_run_id {}) pinned artifact {} byte length does not match retained bytes",
        record.sequence,
        record.operator_run_id,
        pin.role,
    );
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
    if cache_miss {
        verified_artifact_cache.insert(resolved_path.clone(), verified.clone());
    }

    Ok((resolved_path, verified.bytes))
}

/// Owns the parsed pack and verified controls so the `'pack`-lifetime
/// [`BatchPlan`] work items can borrow from them without cloning every record.
struct OwnedBatchPlan {
    pack: SourceUniverseExecutionPack,
    execution_record_sha256s: BTreeMap<u64, String>,
    verified_control_artifacts: BTreeMap<u64, SourceUniverseVerifiedControlArtifacts>,
    control_artifact_failures: BTreeMap<u64, String>,
    start_sequence: Option<u64>,
    record_limit: usize,
    output_root_lease: BatchOutputRootLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessIsolatedRecordResourceEnvelope {
    local_storage_required_bytes: u64,
    worker_virtual_memory_required_bytes: u64,
}

fn calculate_process_isolated_record_resource_envelope(
    selected_object_bytes: u64,
    max_decoded_bytes: u64,
    max_retained_control_input_bytes: u64,
    max_final_object_bytes: u64,
    worker_reserved_overhead_bytes: u64,
) -> Result<ProcessIsolatedRecordResourceEnvelope> {
    let double_selected_object_bytes = selected_object_bytes
        .checked_mul(2)
        .context("selected-record doubled selected-object reserve overflow")?;
    let double_decoded_bytes = max_decoded_bytes
        .checked_mul(2)
        .context("selected-record doubled decoded-byte reserve overflow")?;
    // The selected object exists simultaneously in the content-addressed
    // cache and the anonymous worker-request archive. The archive also carries
    // the retained controls, a length header, and a manifest whose enforced
    // runtime bootstrap cap is max_decoded_bytes. Keep those bytes independent
    // from the two decoded-output reserves so admission remains conservative
    // even when the source object is larger than its decoded output.
    let request_manifest_and_header_bytes = max_decoded_bytes
        .checked_add(worker_request_archive_header_bytes()?)
        .context("selected-record worker-request framing reserve overflow")?;
    let local_storage_required_bytes = double_selected_object_bytes
        .checked_add(double_decoded_bytes)
        .and_then(|value| value.checked_add(max_retained_control_input_bytes))
        .and_then(|value| value.checked_add(max_final_object_bytes))
        .and_then(|value| value.checked_add(request_manifest_and_header_bytes))
        .context("selected-record local-storage requirement overflow")?;
    let worker_virtual_memory_required_bytes = selected_object_bytes
        .checked_add(max_decoded_bytes)
        .and_then(|value| value.checked_add(worker_reserved_overhead_bytes))
        .context("selected-record worker virtual-memory requirement overflow")?;
    Ok(ProcessIsolatedRecordResourceEnvelope {
        local_storage_required_bytes,
        worker_virtual_memory_required_bytes,
    })
}

fn validate_process_isolated_worker_resource_envelope(
    resource_limits: SourceUniverseBatchResourceLimits,
    selected_object_bytes: u64,
    max_decoded_bytes: u64,
    envelope: ProcessIsolatedRecordResourceEnvelope,
) -> Result<()> {
    ensure!(
        resource_limits.worker_max_virtual_memory_bytes
            >= envelope.worker_virtual_memory_required_bytes,
        "resource_limits.worker_max_virtual_memory_bytes {} cannot cover selected_object_bytes {} plus execution_plan.max_decoded_bytes {} plus worker_reserved_overhead_bytes {} (required {})",
        resource_limits.worker_max_virtual_memory_bytes,
        selected_object_bytes,
        max_decoded_bytes,
        resource_limits.worker_reserved_overhead_bytes,
        envelope.worker_virtual_memory_required_bytes
    );
    Ok(())
}

impl OwnedBatchPlan {
    fn process_isolated_record_resource_envelope(
        &self,
        bootstrap_limits: SourceUniverseBatchBootstrapLimits,
        resource_limits: SourceUniverseBatchResourceLimits,
    ) -> Result<ProcessIsolatedRecordResourceEnvelope> {
        let mut selected = self
            .pack
            .records
            .iter()
            .filter(|record| {
                self.start_sequence
                    .is_none_or(|start_sequence| record.sequence >= start_sequence)
            })
            .take(self.record_limit);
        let record = selected
            .next()
            .context("process-isolated selection has no record")?;
        ensure!(
            selected.next().is_none(),
            "process-isolated selected-record envelope is not singular"
        );
        ensure!(
            self.record_limit == 1,
            "process-isolated selected-record envelope requires record_limit=1"
        );
        if let Some(error) = self.control_artifact_failures.get(&record.sequence) {
            bail!(
                "selected process-isolated record {} controls are not verified: {error}",
                record.sequence
            );
        }
        let controls = self
            .verified_control_artifacts
            .get(&record.sequence)
            .context("selected process-isolated record is missing verified controls")?;
        let artifact_store = controls
            .run_spec
            .artifact_store
            .as_ref()
            .context("selected process-isolated record RunSpec is missing artifact_store")?;
        let envelope = calculate_process_isolated_record_resource_envelope(
            record.selected_object_bytes,
            controls.execution_plan.max_decoded_bytes,
            bootstrap_limits.max_retained_control_input_bytes,
            artifact_store.max_final_object_bytes,
            resource_limits.worker_reserved_overhead_bytes,
        )?;
        validate_process_isolated_worker_resource_envelope(
            resource_limits,
            record.selected_object_bytes,
            controls.execution_plan.max_decoded_bytes,
            envelope,
        )?;
        Ok(envelope)
    }

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
                BatchWorkItem::NeedsWork {
                    record,
                    control_artifacts,
                    execution_record_sha256,
                }
            })
            .collect();
        BatchPlan { work_items }
    }
}

fn reconstructed_discovered_record(
    discovered: SourceUniverseDiscoveredRunReceipt,
    sealed_summary: OperatorRunSummary,
    record: &SourceUniverseExecutionPackRecord,
    control_artifacts: &SourceUniverseVerifiedControlArtifacts,
    execution_record_sha256: &str,
) -> SourceUniverseBatchExecutionRecord {
    let SourceUniverseDiscoveredRunReceipt {
        attempt_worker_sha256,
        receipt: durable_receipt,
    } = discovered;
    let DurableRunReceipt {
        completion,
        execution_attestation,
        ..
    } = durable_receipt;
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
        canonical_rows: sealed_summary.canonical_rows,
        nt_catalog_rows: sealed_summary.nt_catalog_rows,
        catalog_hash: sealed_summary.catalog_hash,
        completion_provenance:
            SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated,
        completion_resolution: SourceUniverseBatchExecutionCompletionResolution::Discovered,
        attempt_worker_sha256,
        terminal_publisher_worker_sha256: execution_attestation.worker_executable_sha256,
        durable_completion: Some(completion),
    }
}

#[cfg(test)]
fn process_work_item<F, R, G>(
    work_item: &BatchWorkItem<'_>,
    output_root_lease: &BatchOutputRootLease,
    config: &SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
    clock_factory: &G,
    lifecycle_cleanup_limits: SourceUniverseLifecycleCleanupLimits,
    completion_policy: BatchCompletionPolicy,
) -> RecordSlot
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
    G: SourceUniverseWorkBudgetClockFactory,
{
    match resolve_work_item(work_item, config, clock_factory) {
        ResolvedBatchWorkItem::Terminal(slot) => *slot,
        ResolvedBatchWorkItem::Fresh(fresh) => process_fresh_work_item(
            fresh,
            output_root_lease,
            config,
            fetcher,
            runner,
            lifecycle_cleanup_limits,
            completion_policy,
        ),
    }
}

fn resolve_work_item<'pack, G>(
    work_item: &'pack BatchWorkItem<'pack>,
    config: &SourceUniverseBatchExecutionConfig,
    clock_factory: &G,
) -> ResolvedBatchWorkItem<'pack>
where
    G: SourceUniverseWorkBudgetClockFactory,
{
    let (record, control_artifacts, execution_record_sha256) = match work_item {
        BatchWorkItem::PreflightFailed {
            record,
            error,
            execution_record_sha256,
        } => {
            return ResolvedBatchWorkItem::Terminal(Box::new(record_error_slot(
                record,
                execution_record_sha256,
                "verify_control_artifacts",
                anyhow::anyhow!(error.to_string()),
                config,
            )));
        }
        BatchWorkItem::NeedsWork {
            record,
            control_artifacts,
            execution_record_sha256,
        } => (*record, *control_artifacts, *execution_record_sha256),
    };

    // One plan-derived guard owns exact-current completion discovery plus any
    // subsequent fetch and execution, so the record cannot reset its deadline.
    let work_budget = match OperatorWorkBudgetGuard::from_execution_plan_with_clock(
        &control_artifacts.execution_plan,
        clock_factory.create_clock(),
    ) {
        Ok(work_budget) => work_budget,
        Err(error) => {
            return ResolvedBatchWorkItem::Terminal(Box::new(record_error_slot(
                record,
                execution_record_sha256,
                "create_work_budget",
                error,
                config,
            )));
        }
    };

    ResolvedBatchWorkItem::Fresh(FreshBatchWorkItem {
        record,
        control_artifacts,
        execution_record_sha256,
        work_budget,
    })
}

fn finalize_record_attempt(
    record: &SourceUniverseExecutionPackRecord,
    slot: RecordSlot,
    output_claim: &BatchOutputChildClaim,
    output_root_lease: &BatchOutputRootLease,
    lifecycle_cleanup_limits: SourceUniverseLifecycleCleanupLimits,
) -> RecordSlot {
    match output_claim.compact_to_receipt(output_root_lease, lifecycle_cleanup_limits) {
        Ok(()) => slot,
        Err(error) => RecordSlot::Stopped(StoppedRecord {
            sequence: record.sequence,
            error: error.context(format!(
                "record outcome was captured but owned attempt {} could not be compacted",
                output_claim.canonical_path.display()
            )),
        }),
    }
}

fn process_fresh_work_item<F, R>(
    fresh: FreshBatchWorkItem<'_>,
    output_root_lease: &BatchOutputRootLease,
    config: &SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
    lifecycle_cleanup_limits: SourceUniverseLifecycleCleanupLimits,
    completion_policy: BatchCompletionPolicy,
) -> RecordSlot
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    let FreshBatchWorkItem {
        record,
        control_artifacts,
        execution_record_sha256,
        work_budget,
    } = fresh;

    if let Err(error) = output_root_lease.revalidate().with_context(|| {
        format!(
            "revalidate output root before deterministic terminal discovery for {}",
            record.operator_run_id
        )
    }) {
        return record_error_slot(
            record,
            execution_record_sha256,
            "validate_output",
            error,
            config,
        );
    }

    let output_claim = match guarded_operation_outcome(
        &work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        || BatchOutputChildClaim::acquire(output_root_lease, &record.operator_run_id, &work_budget),
    )
    .and_then(std::convert::identity)
    .with_context(|| {
        format!(
            "claim fresh output for pack record {} ({})",
            record.sequence, record.operator_run_id
        )
    }) {
        Ok(output_claim) => output_claim,
        Err(error) => {
            if let Some(attempt) = attempt_identity_from_claim_error(&error) {
                return record_error_slot_with_attempt(
                    record,
                    execution_record_sha256,
                    "validate_output",
                    error,
                    attempt,
                    config,
                );
            }
            return record_error_slot(
                record,
                execution_record_sha256,
                "validate_output",
                error,
                config,
            );
        }
    };
    let finish = |slot| {
        finalize_record_attempt(
            record,
            slot,
            &output_claim,
            output_root_lease,
            lifecycle_cleanup_limits,
        )
    };

    let record_output_dir = match guarded_operation_outcome(
        &work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        || output_claim.revalidate(output_root_lease),
    )
    .and_then(std::convert::identity)
    .context("revalidate freshly claimed discovery scratch")
    {
        Ok(record_output_dir) => record_output_dir,
        Err(error) => {
            return finish(record_error_slot_with_attempt(
                record,
                execution_record_sha256,
                "validate_output",
                error,
                output_claim.attempt_identity(),
                config,
            ));
        }
    };
    let discovery = runner
        .discover_current_completion(record, control_artifacts, &record_output_dir, &work_budget)
        .and_then(|discovered| {
            discovered
                .map(|discovered| {
                    let summary =
                        durable_receipt_summary(&discovered.receipt, &control_artifacts.run_spec)?;
                    Ok((discovered, summary))
                })
                .transpose()
        });
    match discovery {
        Ok(Some((discovered, summary))) => {
            return finish(RecordSlot::Discovered(reconstructed_discovered_record(
                discovered,
                summary,
                record,
                control_artifacts,
                execution_record_sha256,
            )));
        }
        Ok(None) => {
            if completion_policy == BatchCompletionPolicy::RequireExistingRemoteTerminal {
                return finish(RecordSlot::Stopped(StoppedRecord {
                    sequence: record.sequence,
                    error: anyhow::anyhow!(
                        "immutable batch report exists but its exact remote durable terminal is absent"
                    ),
                }));
            }
        }
        Err(error) => {
            return finish(record_error_slot_with_attempt(
                record,
                execution_record_sha256,
                "discover_durable_completion",
                committed_indeterminate_worker_error(format!(
                    "deterministic current-terminal state is not proven absent: {error:#}"
                )),
                output_claim.attempt_identity(),
                config,
            ));
        }
    }

    let object =
        match guarded_operation_outcome(&work_budget, OperatorWorkBudgetStage::Fetch, || {
            fetcher.fetch(record, control_artifacts.run_spec.as_ref(), &work_budget)
        })
        .and_then(std::convert::identity)
        .with_context(|| format!("fetch source object for {}", record.operator_run_id))
        {
            Ok(object) => object,
            Err(error) => {
                return finish(record_error_slot_with_attempt(
                    record,
                    execution_record_sha256,
                    "fetch",
                    error,
                    output_claim.attempt_identity(),
                    config,
                ));
            }
        };
    let object_bytes = match object.into_bytes_for(record) {
        Ok(bytes) => bytes,
        Err(error) => {
            return finish(record_error_slot_with_attempt(
                record,
                execution_record_sha256,
                "fetch",
                error.context("bind verified source object to execution-pack record"),
                output_claim.attempt_identity(),
                config,
            ));
        }
    };

    // Threat boundary: this detects replacement during the potentially long
    // fetch, but it is not an openat-style capability. `SourceUniverseOperatorRunner::run`
    // and the NT catalog APIs accept `&Path` and reopen descendants by pathname,
    // so an actor able to mutate this trusted workspace after this check cannot
    // be excluded atomically without changing the operator/NT storage API. The
    // post-fetch atomic child claim plus held root/child identities reject
    // every drift observable at the available boundary
    // without pretending otherwise.
    let post_fetch_output_dir = match guarded_operation_outcome(
        &work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
        || output_claim.revalidate(output_root_lease),
    )
    .and_then(std::convert::identity)
    .with_context(|| {
        format!(
            "revalidate output root and child after fetch for {}",
            record.operator_run_id
        )
    }) {
        Ok(record_output_dir) => record_output_dir,
        Err(error) => {
            return finish(record_error_slot_with_attempt(
                record,
                execution_record_sha256,
                "validate_output",
                error,
                output_claim.attempt_identity(),
                config,
            ));
        }
    };
    if post_fetch_output_dir != record_output_dir {
        return finish(record_error_slot_with_attempt(
            record,
            execution_record_sha256,
            "validate_output",
            anyhow::anyhow!("fresh discovery scratch path changed before execution"),
            output_claim.attempt_identity(),
            config,
        ));
    }

    // Allocate and bind every report field before entering the only operation
    // that may consume an operator completion permit. The success tail merely
    // moves the opaque summary's already-allocated hash and assigns scalars.
    let mut completed_record = SourceUniverseBatchExecutionRecord {
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
        canonical_rows: 0,
        nt_catalog_rows: 0,
        catalog_hash: String::new(),
        completion_provenance:
            SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated,
        completion_resolution: SourceUniverseBatchExecutionCompletionResolution::Published,
        attempt_worker_sha256: String::new(),
        terminal_publisher_worker_sha256: String::new(),
        durable_completion: None,
    };

    // The selected operator owns its exact durable completion. Its success
    // path has already passed child/candidate/receipt agreement and must not
    // be postchecked, while an ordinary pre-terminal error still observes
    // deadline expiry before classification.
    let run_result = run_operator_with_terminal_ownership(&work_budget, || {
        runner.run(
            record,
            object_bytes,
            control_artifacts,
            &record_output_dir,
            &work_budget,
        )
    })
    .with_context(|| format!("run operator {}", record.operator_run_id));
    let run_output = match run_result {
        Ok(run_output) => run_output,
        Err(error) => {
            return finish(record_error_slot_with_attempt(
                record,
                execution_record_sha256,
                "run_operator",
                error,
                output_claim.attempt_identity(),
                config,
            ));
        }
    };
    completed_record.canonical_rows = run_output.output.canonical_rows;
    completed_record.nt_catalog_rows = run_output.output.nt_catalog_rows;
    completed_record.catalog_hash = run_output.output.catalog_hash;
    completed_record
        .attempt_worker_sha256
        .clone_from(&run_output.worker_executable_sha256);
    completed_record.terminal_publisher_worker_sha256 = run_output.worker_executable_sha256;
    completed_record.durable_completion = Some(run_output.durable_completion);
    finish(RecordSlot::Completed(completed_record))
}

struct CompletedOperatorRun {
    output: SourceUniverseBatchExecutionRunOutput,
    worker_executable_sha256: String,
    durable_completion: DurableCompletionLocator,
}

#[cfg(test)]
pub(crate) fn synthetic_test_durable_completion() -> DurableCompletionLocator {
    DurableCompletionLocator {
        object: crate::operator::DurableObjectVersionIdentity {
            uri: format!(
                "s3://synthetic-test/{}",
                crate::operator::DURABLE_COMPLETION_MANIFEST_FILE
            ),
            sha256: crate::hashing::sha256_hex(b"synthetic durable completion"),
            byte_len: 1,
            version_id: "synthetic-version".to_string(),
            e_tag: None,
        },
    }
}

#[cfg(test)]
fn synthetic_test_durable_receipt(completion: DurableCompletionLocator) -> DurableRunReceipt {
    DurableRunReceipt {
        completion,
        execution_attestation: DurableExecutionAttestation::new_process_isolated(
            synthetic_test_worker_executable_sha256(),
        )
        .expect("synthetic process-isolated execution attestation"),
        run_id: "synthetic-run".to_string(),
        submitted_manifest_hash: crate::hashing::sha256_hex(b"synthetic manifest"),
        canonical_rows: 1,
        nt_catalog_rows: 1,
        catalog_hash: crate::hashing::sha256_hex(b"synthetic catalog"),
    }
}

#[cfg(test)]
fn synthetic_test_worker_executable_sha256() -> String {
    crate::hashing::sha256_hex(b"synthetic process-isolated worker executable")
}

#[cfg(test)]
pub(crate) fn synthetic_test_current_attempt_worker_sha256() -> String {
    crate::hashing::sha256_hex(b"synthetic current process-isolated worker executable")
}

fn run_operator_with_terminal_ownership(
    work_budget: &OperatorWorkBudgetGuard,
    operation: impl FnOnce() -> Result<SourceUniverseOperatorRunOutcome>,
) -> Result<CompletedOperatorRun> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
    match operation() {
        Ok(SourceUniverseOperatorRunOutcome::Committed(receipt)) => Ok(CompletedOperatorRun {
            output: receipt.output,
            worker_executable_sha256: receipt.worker_executable_sha256,
            durable_completion: receipt.durable_completion,
        }),
        #[cfg(test)]
        Ok(SourceUniverseOperatorRunOutcome::NonTerminal(output)) => {
            work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
            Ok(CompletedOperatorRun {
                output,
                worker_executable_sha256: synthetic_test_worker_executable_sha256(),
                durable_completion: synthetic_test_durable_completion(),
            })
        }
        Err(error) if is_committed_indeterminate_worker_error(&error) => Err(error),
        Err(error) => {
            work_budget.check_deadline(OperatorWorkBudgetStage::Backtest)?;
            Err(error)
        }
    }
}

fn record_error_slot(
    record: &SourceUniverseExecutionPackRecord,
    execution_record_sha256: &str,
    failure_stage: &str,
    error: anyhow::Error,
    config: &SourceUniverseBatchExecutionConfig,
) -> RecordSlot {
    record_error_slot_inner(
        record,
        execution_record_sha256,
        failure_stage,
        error,
        None,
        config,
    )
}

fn record_error_slot_with_attempt(
    record: &SourceUniverseExecutionPackRecord,
    execution_record_sha256: &str,
    failure_stage: &str,
    error: anyhow::Error,
    attempt_output: SourceUniverseBatchExecutionAttemptIdentity,
    config: &SourceUniverseBatchExecutionConfig,
) -> RecordSlot {
    record_error_slot_inner(
        record,
        execution_record_sha256,
        failure_stage,
        error,
        Some(attempt_output),
        config,
    )
}

fn record_error_slot_inner(
    record: &SourceUniverseExecutionPackRecord,
    execution_record_sha256: &str,
    failure_stage: &str,
    error: anyhow::Error,
    attempt_output: Option<SourceUniverseBatchExecutionAttemptIdentity>,
    config: &SourceUniverseBatchExecutionConfig,
) -> RecordSlot {
    let error = if let Some(attempt) = &attempt_output {
        error.context(format!(
            "retained owned output attempt {} (device {:?}, inode {:?})",
            attempt.output_dir.display(),
            attempt.device,
            attempt.inode
        ))
    } else {
        error
    };
    if config.continue_on_error && !is_committed_indeterminate_worker_error(&error) {
        RecordSlot::Failed(failure_record(
            record,
            execution_record_sha256,
            failure_stage,
            &error,
            attempt_output,
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
) -> Result<SourceUniverseBatchExecutionReport> {
    let expected_slot_count = owned_plan
        .pack
        .records
        .iter()
        .filter(|record| {
            owned_plan
                .start_sequence
                .is_none_or(|start_sequence| record.sequence >= start_sequence)
        })
        .take(owned_plan.record_limit)
        .count();
    ensure!(
        slots.len() == expected_slot_count,
        "batch slot cardinality mismatch: expected {expected_slot_count}, got {}",
        slots.len()
    );
    let mut records = Vec::new();
    let mut failures = Vec::new();
    let mut total_canonical_rows = 0_u64;
    let mut total_nt_catalog_rows = 0_u64;

    for (slot_index, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(RecordSlot::Completed(record) | RecordSlot::Discovered(record)) => {
                total_canonical_rows = total_canonical_rows
                    .checked_add(record.canonical_rows)
                    .context("batch total_canonical_rows overflow")?;
                total_nt_catalog_rows = total_nt_catalog_rows
                    .checked_add(record.nt_catalog_rows)
                    .context("batch total_nt_catalog_rows overflow")?;
                records.push(record);
            }
            Some(RecordSlot::Failed(failure)) => failures.push(failure),
            Some(RecordSlot::Stopped(stopped)) => return Err(stopped.error),
            None => bail!("batch work item slot {slot_index} is missing an outcome"),
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
    ensure!(
        selected_record_count
            == u64::try_from(expected_slot_count).context("expected slot count exceeds u64")?,
        "batch report outcome cardinality mismatch: expected {expected_slot_count}, got {selected_record_count}"
    );

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

fn validate_existing_batch_report_selection(
    report: &SourceUniverseBatchExecutionReport,
    batch_id: &str,
    owned_plan: &OwnedBatchPlan,
) -> Result<()> {
    validate_source_universe_batch_execution_report(report)
        .context("validate existing immutable batch execution report")?;
    ensure!(
        report.batch_id == batch_id
            && report.pack_id == owned_plan.pack.pack_id
            && report.universe_id == owned_plan.pack.universe_id
            && report.venue == owned_plan.pack.venue,
        "existing immutable batch report does not match the selected batch and execution pack"
    );
    ensure!(
        report.failures.is_empty(),
        "existing immutable batch report contains failures; retry requires a new batch output identity"
    );
    let expected_records = owned_plan
        .pack
        .records
        .iter()
        .filter(|record| {
            owned_plan
                .start_sequence
                .is_none_or(|start_sequence| record.sequence >= start_sequence)
        })
        .take(owned_plan.record_limit)
        .collect::<Vec<_>>();
    ensure!(
        report.records.len() == expected_records.len(),
        "existing immutable batch report selected record count does not match the current batch selection"
    );
    for (actual, expected) in report.records.iter().zip(expected_records) {
        let execution_record_sha256 = owned_plan
            .execution_record_sha256s
            .get(&expected.sequence)
            .context("selected execution-record digest is missing")?;
        let controls = owned_plan
            .verified_control_artifacts
            .get(&expected.sequence)
            .context("selected verified controls are missing")?;
        ensure!(
            actual.sequence == expected.sequence
                && actual.operator_run_id == expected.operator_run_id
                && actual.source_binding == expected.source_binding
                && actual.category == expected.category
                && actual.symbol == expected.symbol
                && actual.archive_date == expected.archive_date
                && actual.selected_object_sha256 == expected.selected_object_sha256
                && actual.run_spec_sha256 == expected.run_spec_sha256
                && actual.accepted_tranche_sha256 == expected.accepted_tranche_sha256
                && actual.execution_plan_sha256 == expected.execution_plan_sha256
                && actual.execution_record_sha256.as_str() == execution_record_sha256.as_str()
                && actual.source_bindings_sha256.as_str()
                    == controls.source_bindings_sha256.as_str()
                && actual.selected_object_bytes == expected.selected_object_bytes,
            "existing immutable batch report record {} does not match the exact selected controls",
            expected.sequence
        );
    }
    Ok(())
}

fn validate_existing_batch_report_against_remote_discovery(
    existing: &SourceUniverseBatchExecutionReport,
    discovered: &SourceUniverseBatchExecutionReport,
) -> Result<()> {
    ensure!(
        existing.records.len() == discovered.records.len(),
        "existing immutable batch report remote-discovery cardinality mismatch"
    );
    let mut normalized_discovery = discovered.clone();
    for (existing_record, discovered_record) in existing
        .records
        .iter()
        .zip(&mut normalized_discovery.records)
    {
        ensure!(
            discovered_record.completion_resolution
                == SourceUniverseBatchExecutionCompletionResolution::Discovered,
            "existing immutable batch report restart must resolve every exact remote terminal by discovery"
        );
        ensure!(
            existing_record.attempt_worker_sha256 == discovered_record.attempt_worker_sha256,
            "existing immutable batch report attempt worker is not the exact-current worker"
        );
        discovered_record.completion_resolution = existing_record.completion_resolution;
    }
    ensure!(
        &normalized_discovery == existing,
        "existing immutable batch report does not match exact-current remote terminal discovery"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_existing_batch_report_with_lease(
    output_root_lease: &BatchOutputRootLease,
    max_bytes: u64,
) -> Result<Option<SourceUniverseBatchExecutionReport>> {
    ensure!(max_bytes > 0, "batch report read cap must be positive");
    output_root_lease
        .revalidate()
        .context("revalidate leased output root before existing batch report read")?;
    let name = std::ffi::CString::new(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE)
        .context("batch report file name contains an interior NUL")?;
    let Some(mut file) = try_open_batch_report_at(&output_root_lease.handle, &name)? else {
        return Ok(None);
    };
    let initial = file.metadata().context("stat existing batch report")?;
    ensure!(
        initial.len() > 0 && initial.len() <= max_bytes,
        "existing immutable batch report byte length {} exceeds configured bounded range 1..={max_bytes}",
        initial.len()
    );
    let read_cap = initial
        .len()
        .checked_add(1)
        .context("existing batch report read cap overflow")?;
    let reserve = usize::try_from(read_cap).context("existing batch report cap exceeds usize")?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(reserve)
        .context("reserve existing batch report buffer")?;
    (&mut file)
        .take(read_cap)
        .read_to_end(&mut bytes)
        .context("read existing immutable batch report")?;
    ensure!(
        bytes.len()
            == usize::try_from(initial.len()).context("batch report length exceeds usize")?,
        "existing immutable batch report length changed while reading"
    );
    let after_read = file.metadata().context("re-stat existing batch report")?;
    ensure!(
        initial.dev() == after_read.dev()
            && initial.ino() == after_read.ino()
            && initial.len() == after_read.len()
            && initial.mtime() == after_read.mtime()
            && initial.mtime_nsec() == after_read.mtime_nsec()
            && initial.ctime() == after_read.ctime()
            && initial.ctime_nsec() == after_read.ctime_nsec(),
        "existing immutable batch report changed while reading"
    );
    let reopened = open_batch_report_at(&output_root_lease.handle, &name)?;
    let reopened_identity = reopened
        .metadata()
        .context("stat re-opened existing batch report")?;
    ensure!(
        initial.dev() == reopened_identity.dev() && initial.ino() == reopened_identity.ino(),
        "existing immutable batch report namespace changed while reading"
    );
    let report: SourceUniverseBatchExecutionReport =
        serde_json::from_slice(&bytes).context("parse existing immutable batch report")?;
    let canonical = crate::reference_artifact::canonical_json_bytes(&report)
        .context("serialize existing immutable batch report canonically")?;
    ensure!(
        canonical == bytes,
        "existing immutable batch report bytes are not canonical"
    );
    validate_source_universe_batch_execution_report(&report)
        .context("validate existing immutable batch report")?;
    output_root_lease
        .revalidate()
        .context("revalidate leased output root after existing batch report read")?;
    Ok(Some(report))
}

#[cfg(not(target_os = "linux"))]
fn read_existing_batch_report_with_lease(
    _output_root_lease: &BatchOutputRootLease,
    _max_bytes: u64,
) -> Result<Option<SourceUniverseBatchExecutionReport>> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn try_open_batch_report_at(
    output_root: &fs::File,
    name: &std::ffi::CStr,
) -> Result<Option<fs::File>> {
    // SAFETY: the held directory fd and C string are live. O_NOFOLLOW rejects
    // a symlink occupant, and a successful fd is immediately owned by File.
    let fd = unsafe {
        libc::openat(
            output_root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error).context("open batch report at output-root fd");
    }
    // SAFETY: openat returned one new owned descriptor.
    let file = unsafe { fs::File::from_raw_fd(fd) };
    ensure!(
        file.metadata()?.file_type().is_file(),
        "batch report namespace occupant is not a regular file"
    );
    Ok(Some(file))
}

#[cfg(target_os = "linux")]
fn open_batch_report_at(output_root: &fs::File, name: &std::ffi::CStr) -> Result<fs::File> {
    try_open_batch_report_at(output_root, name)?.context("batch report is absent at output-root fd")
}

#[cfg(target_os = "linux")]
fn verify_batch_report_at(
    output_root: &fs::File,
    name: &std::ffi::CStr,
    expected: &[u8],
) -> Result<()> {
    let mut file = open_batch_report_at(output_root, name)?;
    let initial = file.metadata().context("stat opened batch report")?;
    ensure!(
        initial.len()
            == u64::try_from(expected.len()).context("batch report length exceeds u64")?,
        "existing immutable batch report has different bytes"
    );
    let read_cap = initial
        .len()
        .checked_add(1)
        .context("batch report verification read cap overflow")?;
    let reserve =
        usize::try_from(read_cap).context("batch report verification cap exceeds usize")?;
    let mut observed = Vec::new();
    observed
        .try_reserve_exact(reserve)
        .context("reserve batch report verification buffer")?;
    (&mut file)
        .take(read_cap)
        .read_to_end(&mut observed)
        .context("read existing immutable batch report")?;
    ensure!(
        observed == expected,
        "existing immutable batch report has different bytes"
    );
    let after_read = file.metadata().context("re-stat opened batch report")?;
    ensure!(
        initial.dev() == after_read.dev()
            && initial.ino() == after_read.ino()
            && initial.len() == after_read.len()
            && initial.mtime() == after_read.mtime()
            && initial.mtime_nsec() == after_read.mtime_nsec()
            && initial.ctime() == after_read.ctime()
            && initial.ctime_nsec() == after_read.ctime_nsec(),
        "existing immutable batch report changed during verification"
    );
    let reopened = open_batch_report_at(output_root, name)?;
    let current = reopened
        .metadata()
        .context("stat re-opened batch report namespace")?;
    ensure!(
        initial.dev() == current.dev() && initial.ino() == current.ino(),
        "batch report namespace identity changed during verification"
    );
    file.sync_all()
        .context("sync existing immutable batch report")?;
    output_root
        .sync_all()
        .context("sync batch report output-root directory")?;
    let final_open = open_batch_report_at(output_root, name)?;
    let final_identity = final_open
        .metadata()
        .context("stat final batch report namespace")?;
    ensure!(
        initial.dev() == final_identity.dev() && initial.ino() == final_identity.ino(),
        "batch report namespace identity changed while syncing"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn create_or_verify_batch_report_at(output_root: &fs::File, bytes: &[u8]) -> Result<()> {
    let name = std::ffi::CString::new(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE)
        .context("batch report file name contains an interior NUL")?;
    // SAFETY: the held directory fd and static dot component are live. A
    // successful O_TMPFILE descriptor owns an unnamed regular file.
    let fd = unsafe {
        libc::openat(
            output_root.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDWR | libc::O_CLOEXEC | libc::O_TMPFILE,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("create anonymous batch report");
    }
    // SAFETY: openat returned one new owned descriptor.
    let mut candidate = unsafe { fs::File::from_raw_fd(fd) };
    ensure!(
        candidate.metadata()?.file_type().is_file(),
        "anonymous batch report is not a regular file"
    );
    candidate
        .write_all(bytes)
        .context("write anonymous batch report")?;
    candidate.flush().context("flush anonymous batch report")?;
    candidate
        .sync_all()
        .context("sync anonymous batch report")?;
    // SAFETY: the anonymous source fd, held directory fd, and C strings remain
    // live. AT_EMPTY_PATH links the exact anonymous inode create-only.
    let link_result = unsafe {
        libc::linkat(
            candidate.as_raw_fd(),
            c"".as_ptr(),
            output_root.as_raw_fd(),
            name.as_ptr(),
            libc::AT_EMPTY_PATH,
        )
    };
    if link_result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error).context("create-only link anonymous batch report");
        }
    }
    verify_batch_report_at(output_root, &name, bytes)
}

#[cfg(target_os = "linux")]
fn write_source_universe_batch_execution_report_with_lease(
    output_root_lease: &BatchOutputRootLease,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
    validate_source_universe_batch_execution_report(report)
        .context("validate batch execution report before write")?;
    let bytes = crate::reference_artifact::canonical_json_bytes(report)
        .context("serialize canonical batch execution report")?;
    let content_hash = crate::hashing::sha256_hex(&bytes);
    let byte_len = u64::try_from(bytes.len()).context("batch report length exceeds u64")?;
    output_root_lease
        .revalidate()
        .context("revalidate leased batch output root before report publication")?;

    create_or_verify_batch_report_at(&output_root_lease.handle, &bytes).with_context(|| {
        format!(
            "create or verify immutable batch execution report under leased output root {}",
            output_root_lease.canonical_path.display()
        )
    })?;
    output_root_lease
        .revalidate()
        .context("revalidate leased batch output root after report publication")?;

    Ok(SourceUniverseBatchExecutionReportArtifact {
        path: output_root_lease
            .canonical_path
            .join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE),
        content_hash,
        bytes: byte_len,
        completed_record_count: report.completed_record_count,
    })
}

#[cfg(not(target_os = "linux"))]
fn write_source_universe_batch_execution_report_with_lease(
    _output_root_lease: &BatchOutputRootLease,
    _report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
    bail!("fd-relative batch report publication requires Linux O_TMPFILE/linkat")
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn write_source_universe_batch_execution_report(
    output_dir: &Path,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
    let output_root_lease = BatchOutputRootLease::acquire(output_dir)?;
    write_source_universe_batch_execution_report_with_lease(&output_root_lease, report)
}

#[cfg(all(test, not(target_os = "linux")))]
pub(crate) fn write_source_universe_batch_execution_report(
    output_dir: &Path,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
    validate_source_universe_batch_execution_report(report)
        .context("validate batch execution report before test write")?;
    let path = output_dir.join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| format!("write test batch execution report {}", path.display()))?;
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

fn verify_object_guarded(
    record: &SourceUniverseExecutionPackRecord,
    object_bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    ensure!(
        object_bytes.len() as u64 == record.selected_object_bytes,
        "object byte length for {} does not match execution pack: expected {}, got {}",
        record.operator_run_id,
        record.selected_object_bytes,
        object_bytes.len()
    );
    let actual_sha256 = sha256_hex_with_budget(
        object_bytes,
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
    )?;
    ensure!(
        actual_sha256 == record.selected_object_sha256,
        "object sha256 for {} does not match execution pack: expected {}, got {}",
        record.operator_run_id,
        record.selected_object_sha256,
        actual_sha256
    );
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)
}

fn failure_record(
    record: &SourceUniverseExecutionPackRecord,
    execution_record_sha256: &str,
    failure_stage: &str,
    error: &anyhow::Error,
    attempt_output: Option<SourceUniverseBatchExecutionAttemptIdentity>,
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
        attempt_output,
        failure_stage: failure_stage.to_string(),
        error: format!("{error:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_universe_execution_pack::SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION;
    use crate::{
        backfill_execution_plan::BackfillExecutionWorkBudget,
        operator_work_budget::{
            OperatorWorkBudget, OperatorWorkBudgetClock, OperatorWorkBudgetGuard,
        },
    };

    #[derive(Debug, Default)]
    struct ManualWorkBudgetClock {
        now: Mutex<Duration>,
    }

    impl ManualWorkBudgetClock {
        fn expire(&self) {
            *self.now.lock().expect("manual clock lock") = Duration::from_secs(1);
        }

        fn set(&self, now: Duration) {
            *self.now.lock().expect("manual clock lock") = now;
        }
    }

    impl OperatorWorkBudgetClock for ManualWorkBudgetClock {
        fn now(&self) -> Duration {
            *self.now.lock().expect("manual clock lock")
        }
    }

    #[derive(Default)]
    struct FirstRecordExpiresClockFactory {
        created: AtomicUsize,
    }

    struct ExpireAfterConstructionClock {
        observations: AtomicUsize,
    }

    impl OperatorWorkBudgetClock for ExpireAfterConstructionClock {
        fn now(&self) -> Duration {
            if self.observations.fetch_add(1, Ordering::SeqCst) == 0 {
                Duration::ZERO
            } else {
                Duration::from_secs(u64::MAX / 2)
            }
        }
    }

    impl SourceUniverseWorkBudgetClockFactory for FirstRecordExpiresClockFactory {
        fn create_clock(&self) -> Arc<dyn OperatorWorkBudgetClock> {
            if self.created.fetch_add(1, Ordering::SeqCst) == 0 {
                Arc::new(ExpireAfterConstructionClock {
                    observations: AtomicUsize::new(0),
                })
            } else {
                Arc::new(ManualWorkBudgetClock::default())
            }
        }
    }

    #[derive(Default)]
    struct RecordingErrorFetcher {
        calls: Vec<u64>,
    }

    impl SourceUniverseObjectFetcher for RecordingErrorFetcher {
        fn fetch(
            &mut self,
            record: &SourceUniverseExecutionPackRecord,
            _run_spec: &RunSpec,
            _work_budget: &OperatorWorkBudgetGuard,
        ) -> Result<VerifiedSourceObject> {
            self.calls.push(record.sequence);
            anyhow::bail!("synthetic fetch failure after progression")
        }
    }

    struct NeverRunner;

    impl SourceUniverseOperatorRunner for NeverRunner {
        fn run(
            &mut self,
            _record: &SourceUniverseExecutionPackRecord,
            _object_bytes: Vec<u8>,
            _control_artifacts: &SourceUniverseVerifiedControlArtifacts,
            _output_dir: &Path,
            _work_budget: &OperatorWorkBudgetGuard,
        ) -> Result<SourceUniverseOperatorRunOutcome> {
            panic!("runner must not be reached when both records fail during fetch")
        }
    }

    #[cfg(target_os = "linux")]
    fn one_second_process_budget() -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
            max_decoded_bytes: 1024,
            max_source_rows: 1,
            max_projected_row_groups: 1,
            max_wall_seconds: 1,
            require_object_selection_metadata: false,
        }))
        .expect("construct process-isolation work budget")
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fd_backed_worker_executable_survives_origin_path_replacement() {
        let temp = tempfile::tempdir().expect("worker executable tempdir");
        let origin = temp.path().join("worker");
        let displaced = temp.path().join("worker.displaced");
        fs::copy("/usr/bin/true", &origin).expect("copy successful executable");
        let file = fs::File::open(&origin).expect("open executable capability");
        let metadata = file.metadata().expect("fstat executable capability");
        let executable = PinnedWorkerExecutable {
            file,
            byte_len: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        };
        let work_budget = one_second_process_budget();
        let expected_sha256 = executable
            .hash_and_revalidate(None, &work_budget)
            .expect("hash executable before replacement");

        fs::rename(&origin, &displaced).expect("unlink original executable pathname");
        fs::copy("/usr/bin/false", &origin).expect("replace original executable pathname");

        let mut command = Command::new(executable.exec_path());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status =
            spawn_command_with_hard_deadline(&mut command, &work_budget, Duration::from_secs(1))
                .expect("fd-backed executable launches after origin replacement");
        assert!(
            status.success(),
            "worker launch followed the replacement pathname instead of the pinned fd"
        );
        executable
            .hash_and_revalidate(Some(&expected_sha256), &work_budget)
            .expect("pinned executable hash remains stable after replacement");
    }

    #[test]
    fn process_isolated_runner_rejects_parallelism_before_fetch_or_spawn_setup() {
        let error = ProcessIsolatedSourceUniverseOperatorRunner::new(
            PathBuf::from("relative-path-must-not-be-inspected"),
            fs::File::open("/dev/null").expect("open inert test descriptor"),
            2,
            Duration::from_secs(1),
            SourceUniverseBatchResourceLimits {
                worker_max_virtual_memory_bytes: 1,
                worker_reserved_overhead_bytes: 1,
            },
        )
        .expect_err("parallel process workers need an explicit aggregate-memory budget");

        assert!(
            error
                .to_string()
                .contains("requires max_concurrent_records=1"),
            "{error:#}"
        );
    }

    #[test]
    fn process_resource_limits_reject_zero_worker_address_space() {
        let error = SourceUniverseBatchResourceLimits {
            worker_max_virtual_memory_bytes: 0,
            worker_reserved_overhead_bytes: 1,
        }
        .validate()
        .expect_err("worker address-space limit must fail closed at zero");

        assert!(
            error
                .to_string()
                .contains("worker_max_virtual_memory_bytes must be positive"),
            "{error:#}"
        );
    }

    #[test]
    fn process_record_resource_envelope_uses_checked_conservative_formula() {
        let envelope = calculate_process_isolated_record_resource_envelope(10, 20, 30, 40, 50)
            .expect("calculate bounded record resource envelope");

        assert_eq!(envelope.local_storage_required_bytes, 158);
        assert_eq!(envelope.worker_virtual_memory_required_bytes, 80);
        assert!(
            calculate_process_isolated_record_resource_envelope(u64::MAX, 1, 1, 1, 1)
                .expect_err("overflow must fail closed")
                .to_string()
                .contains("overflow")
        );
    }

    #[test]
    fn process_record_resource_envelope_reserves_cache_and_request_archive_copies() {
        let envelope = calculate_process_isolated_record_resource_envelope(100, 1, 1, 1, 1)
            .expect("calculate selected-object-dominant resource envelope");

        assert_eq!(envelope.local_storage_required_bytes, 213);
    }

    #[test]
    fn process_record_resource_envelope_rejects_underdeclared_worker_ceiling() {
        let resource_limits = SourceUniverseBatchResourceLimits {
            worker_max_virtual_memory_bytes: 79,
            worker_reserved_overhead_bytes: 50,
        };
        let envelope = calculate_process_isolated_record_resource_envelope(10, 20, 30, 40, 50)
            .expect("calculate bounded record resource envelope");

        let error =
            validate_process_isolated_worker_resource_envelope(resource_limits, 10, 20, envelope)
                .expect_err("worker ceiling below object + decoded + overhead must fail closed");

        assert!(
            error
                .to_string()
                .contains("worker_max_virtual_memory_bytes"),
            "{error:#}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[allow(clippy::useless_conversion)]
    fn process_resource_limits_reject_kernel_infinity_sentinel() {
        let infinity = u64::try_from(libc::RLIM_INFINITY).expect("rlim_t fits u64");
        let error = worker_rlimit_from_u64(infinity)
            .expect_err("RLIM_INFINITY must not disable the configured hard ceiling");

        assert!(error.to_string().contains("RLIM_INFINITY"), "{error:#}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_runner_hashes_its_pinned_executable_once_per_batch() {
        let temp = tempfile::tempdir().expect("worker request parent");
        let request_root = temp
            .path()
            .canonicalize()
            .expect("canonical request parent")
            .join(SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT);
        let mut runner = ProcessIsolatedSourceUniverseOperatorRunner::new(
            request_root,
            fs::File::open("/dev/null").expect("open inert test descriptor"),
            1,
            Duration::from_secs(1),
            SourceUniverseBatchResourceLimits {
                worker_max_virtual_memory_bytes: 1_073_741_824,
                worker_reserved_overhead_bytes: 1,
            },
        )
        .expect("construct process runner");
        let work_budget = OperatorWorkBudgetGuard::unbounded();

        runner
            .seal_executable_once(&work_budget)
            .expect("seal executable first use");
        runner
            .seal_executable_once(&work_budget)
            .expect("revalidate executable second use");

        assert_eq!(runner.executable_hash_traversals_for_test(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fd_backed_worker_executable_rejects_same_inode_metadata_aba() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("worker executable tempdir");
        let origin = temp.path().join("worker");
        fs::copy("/usr/bin/true", &origin).expect("copy successful executable");
        let file = fs::File::open(&origin).expect("open executable capability");
        let metadata = file.metadata().expect("fstat executable capability");
        let original_mode = metadata.permissions().mode();
        let executable = PinnedWorkerExecutable {
            file,
            byte_len: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        };
        let work_budget = one_second_process_budget();
        let expected_sha256 = executable
            .hash_and_revalidate(None, &work_budget)
            .expect("hash executable before same-inode mutation");

        let mut changed_permissions = metadata.permissions();
        changed_permissions.set_mode(original_mode ^ 0o100);
        fs::set_permissions(&origin, changed_permissions).expect("mutate executable mode");
        std::thread::sleep(Duration::from_millis(2));
        let mut restored_permissions = fs::metadata(&origin)
            .expect("stat mutated executable")
            .permissions();
        restored_permissions.set_mode(original_mode);
        fs::set_permissions(&origin, restored_permissions).expect("restore executable mode");

        let restored_metadata = executable
            .file
            .metadata()
            .expect("fstat executable after metadata ABA");
        assert_eq!(restored_metadata.len(), executable.byte_len);
        assert_eq!(restored_metadata.dev(), executable.device);
        assert_eq!(restored_metadata.ino(), executable.inode);
        assert!(
            restored_metadata.ctime() != executable.changed_seconds
                || restored_metadata.ctime_nsec() != executable.changed_nanoseconds,
            "test mutation must advance the inode change timestamp"
        );
        let error = executable
            .hash_and_revalidate(Some(&expected_sha256), &work_budget)
            .expect_err("same-inode metadata ABA must invalidate executable capability");
        assert!(
            error
                .to_string()
                .contains("executable capability identity changed"),
            "{error:#}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hard_worker_timeout_kills_and_reaps_process_group_descendant() {
        let temp = tempfile::tempdir().expect("worker timeout tempdir");
        let descendant_pid_path = temp.path().join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$DESCENDANT_PID_FILE\"; wait")
            .env("DESCENDANT_PID_FILE", &descendant_pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let lifecycle = spawn_command_with_hard_deadline_observed(
            &mut command,
            &one_second_process_budget(),
            Duration::from_secs(1),
        );
        let WorkerLifecycleOutcome::Quiesced(Err(error)) = lifecycle else {
            panic!("deadline failure may reach the seal probe only after proven quiescence/reap");
        };
        assert!(error.to_string().contains("exceeded remaining wall time"));
        let descendant_pid: i32 = fs::read_to_string(&descendant_pid_path)
            .expect("shell records descendant pid before waiting")
            .trim()
            .parse()
            .expect("parse descendant pid");
        let mut gone = false;
        for _ in 0..100 {
            // SAFETY: signal zero only probes the recorded pid; it does not
            // mutate the process. ESRCH proves the descendant is gone.
            let result = unsafe { libc::kill(descendant_pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            gone,
            "worker descendant {descendant_pid} survived group kill"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hard_worker_wait_accepts_zero_exit_before_deadline() {
        let mut command = Command::new("/usr/bin/true");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = spawn_command_with_hard_deadline(
            &mut command,
            &one_second_process_budget(),
            Duration::from_secs(1),
        )
        .expect("zero worker exits before deadline");
        assert!(status.success());
    }

    #[cfg(target_os = "linux")]
    struct DeadlineAdvancesAfterBoundedWaitClock {
        observations: AtomicUsize,
    }

    #[cfg(target_os = "linux")]
    impl OperatorWorkBudgetClock for DeadlineAdvancesAfterBoundedWaitClock {
        fn now(&self) -> Duration {
            match self.observations.fetch_add(1, Ordering::SeqCst) {
                0..=2 => Duration::ZERO,
                _ => Duration::from_secs(2),
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn completed_worker_status_is_not_reclassified_by_a_post_reap_clock_sample() {
        let clock = Arc::new(DeadlineAdvancesAfterBoundedWaitClock {
            observations: AtomicUsize::new(0),
        });
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
                max_decoded_bytes: 1024,
                max_source_rows: 1,
                max_projected_row_groups: 1,
                max_wall_seconds: 1,
                require_object_selection_metadata: false,
            }),
            clock.clone(),
        )
        .expect("construct terminal-success clock guard");
        let mut command = Command::new("/usr/bin/true");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = spawn_command_with_hard_deadline(&mut command, &guard, Duration::from_secs(1))
            .expect("worker which completed within the pidfd bound remains successful");
        assert!(status.success());
        assert_eq!(
            clock.observations.load(Ordering::SeqCst),
            3,
            "worker wait must not sample the clock after observing terminal status"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_wait_returns_at_its_configured_bound_without_reaping() {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().expect("spawn bounded pidfd wait child");
        let pidfd = open_child_pidfd(&child).expect("open child pidfd");
        let wait_timeout = Duration::from_millis(20);
        let started = std::time::Instant::now();
        assert!(
            !wait_for_pidfd(&pidfd, wait_timeout).expect("bounded pidfd wait"),
            "live child must not report termination"
        );
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "pidfd wait exceeded its configured bound"
        );
        terminate_and_reap_worker_with_pidfd(child, &pidfd, Duration::from_secs(1))
            .expect("terminate and reap bounded pidfd test child");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_termination_grace_is_one_monotonic_deadline() {
        let deadline = WorkerTerminationDeadline::new(Duration::from_millis(200))
            .expect("termination deadline");
        let first_remaining = deadline.remaining();
        std::thread::sleep(Duration::from_millis(10));
        let second_remaining = deadline.remaining();
        assert!(
            second_remaining < first_remaining,
            "termination fallback must consume the original grace instead of resetting it"
        );
        assert!(second_remaining <= deadline.configured_grace);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn successful_worker_leader_cannot_orphan_a_descendant() {
        let temp = tempfile::tempdir().expect("worker orphan tempdir");
        let descendant_pid_path = temp.path().join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$DESCENDANT_PID_FILE\"; exit 0")
            .env("DESCENDANT_PID_FILE", &descendant_pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = spawn_command_with_hard_deadline(
            &mut command,
            &one_second_process_budget(),
            Duration::from_secs(1),
        )
        .expect("successful leader exits before deadline");
        assert!(status.success());
        let descendant_pid: i32 = fs::read_to_string(&descendant_pid_path)
            .expect("shell records orphan candidate pid")
            .trim()
            .parse()
            .expect("parse orphan candidate pid");
        let mut gone = false;
        for _ in 0..100 {
            // SAFETY: signal zero only probes the recorded pid.
            let result = unsafe { libc::kill(descendant_pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            gone,
            "successful worker leader orphaned descendant {descendant_pid}"
        );
    }

    #[cfg(target_os = "linux")]
    struct SpawnConsumesBudgetClock {
        observations: AtomicUsize,
    }

    #[cfg(target_os = "linux")]
    impl OperatorWorkBudgetClock for SpawnConsumesBudgetClock {
        fn now(&self) -> Duration {
            match self.observations.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => Duration::ZERO,
                _ => Duration::from_millis(900),
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_wait_uses_post_spawn_remaining_wall_time() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
                max_decoded_bytes: 1024,
                max_source_rows: 1,
                max_projected_row_groups: 1,
                max_wall_seconds: 1,
                require_object_selection_metadata: false,
            }),
            Arc::new(SpawnConsumesBudgetClock {
                observations: AtomicUsize::new(0),
            }),
        )
        .expect("construct slow-spawn clock guard");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = std::time::Instant::now();
        let error = spawn_command_with_hard_deadline(&mut command, &guard, Duration::from_secs(1))
            .expect_err("post-spawn remaining wall time must expire the worker");
        assert!(
            error.to_string().contains("exceeded remaining wall time"),
            "{error:#}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "wait incorrectly reused the pre-spawn one-second remainder"
        );
    }

    #[cfg(target_os = "linux")]
    struct SyntheticWorkerRequestArchive {
        file: fs::File,
        archive_bytes: u64,
        manifest_bytes: u64,
        manifest_sha256: String,
        raw: Vec<u8>,
    }

    #[cfg(target_os = "linux")]
    fn synthetic_worker_manifest(
        temp: &tempfile::TempDir,
    ) -> (SourceUniverseOperatorWorkerRequestManifest, [Vec<u8>; 5]) {
        let temp_root = temp.path().canonicalize().expect("canonical temp root");
        let output_dir = temp_root.join("output");
        fs::create_dir(&output_dir).expect("create output dir");
        let payloads = [
            b"a".to_vec(),
            b"e".to_vec(),
            b"r".to_vec(),
            b"o".to_vec(),
            b"s".to_vec(),
        ];
        let payload_sha256s = payloads
            .iter()
            .map(|payload| hex::encode(Sha256::digest(payload)))
            .collect::<Vec<_>>();
        let record = SourceUniverseExecutionPackRecord {
            sequence: 0,
            work_item_id: "work-item".to_string(),
            operator_run_id: "operator-run".to_string(),
            source_binding: "source-binding".to_string(),
            category: "spot".to_string(),
            symbol: "BTC-USDT".to_string(),
            archive_date: "2026-07-01".to_string(),
            source_uri: "s3://bucket/object".to_string(),
            source_url: "https://example.invalid/object".to_string(),
            selected_object_sha256: payload_sha256s[3].clone(),
            selected_object_bytes: 1,
            source_proof_id: "proof".to_string(),
            source_proof_version: 1,
            accepted_tranche_id: "tranche".to_string(),
            output_prefix: "s3://bucket/output".to_string(),
            source_bindings_path: PathBuf::from("source-bindings.toml"),
            source_bindings_bytes: 1,
            source_bindings_sha256: payload_sha256s[4].clone(),
            run_spec_path: PathBuf::from("run-spec.toml"),
            run_spec_bytes: 1,
            run_spec_sha256: payload_sha256s[2].clone(),
            accepted_tranche_path: PathBuf::from("accepted-tranche.json"),
            accepted_tranche_bytes: 1,
            accepted_tranche_sha256: payload_sha256s[0].clone(),
            execution_plan_path: PathBuf::from("execution-plan.json"),
            execution_plan_bytes: 1,
            execution_plan_sha256: payload_sha256s[1].clone(),
        };
        let request_payloads = WORKER_REQUEST_ROLES
            .iter()
            .zip(payload_sha256s)
            .map(
                |(role, sha256)| SourceUniverseOperatorWorkerRequestPayload {
                    role: (*role).to_string(),
                    bytes: 1,
                    sha256,
                },
            )
            .collect::<Vec<_>>();
        let manifest = SourceUniverseOperatorWorkerRequestManifest {
            schema_version: SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_SCHEMA_VERSION.to_string(),
            request_kind: SourceUniverseOperatorWorkerRequestKind::Execute,
            execution_attestation: DurableExecutionAttestation::new_process_isolated(
                synthetic_test_worker_executable_sha256(),
            )
            .expect("synthetic process-isolated execution attestation"),
            record,
            output_dir: output_dir.canonicalize().expect("canonical output dir"),
            source_bindings_path: temp_root.join("source-bindings-provenance.toml"),
            work_budget_deadline: OperatorWorkBudgetDeadline {
                started_at_seconds: 1,
                started_at_nanoseconds: 0,
                deadline_seconds: 2,
                deadline_nanoseconds: 0,
            },
            payloads: request_payloads,
        };
        (manifest, payloads)
    }

    #[cfg(target_os = "linux")]
    fn encode_synthetic_worker_archive(
        manifest: &SourceUniverseOperatorWorkerRequestManifest,
        payloads: &[Vec<u8>; 5],
    ) -> (Vec<u8>, u64, String) {
        let manifest_body = serde_json::to_vec(manifest).expect("serialize request manifest");
        let manifest_bytes = u64::try_from(manifest_body.len()).expect("manifest length");
        let mut raw = Vec::new();
        raw.extend_from_slice(&manifest_bytes.to_be_bytes());
        raw.extend_from_slice(&manifest_body);
        for payload in payloads {
            raw.extend_from_slice(payload);
        }
        (
            raw,
            manifest_bytes,
            hex::encode(Sha256::digest(&manifest_body)),
        )
    }

    #[cfg(target_os = "linux")]
    fn anonymous_test_archive(request_root: &Path, raw: &[u8]) -> fs::File {
        let mut writable =
            create_anonymous_worker_request_file(request_root).expect("create anonymous archive");
        writable.write_all(raw).expect("write anonymous archive");
        writable.flush().expect("flush anonymous archive");
        reopen_anonymous_worker_request_read_only(
            writable,
            u64::try_from(raw.len()).expect("archive length"),
        )
        .expect("reopen anonymous archive read-only")
        .0
    }

    #[cfg(target_os = "linux")]
    fn synthetic_worker_archive(temp: &tempfile::TempDir) -> SyntheticWorkerRequestArchive {
        let request_root = temp.path().join("request-root");
        fs::create_dir(&request_root).expect("create request root");
        let request_root = request_root.canonicalize().expect("canonical request root");
        let (manifest, payloads) = synthetic_worker_manifest(temp);
        let (raw, manifest_bytes, manifest_sha256) =
            encode_synthetic_worker_archive(&manifest, &payloads);
        let file = anonymous_test_archive(&request_root, &raw);
        SyntheticWorkerRequestArchive {
            archive_bytes: u64::try_from(raw.len()).expect("archive length"),
            file,
            manifest_bytes,
            manifest_sha256,
            raw,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_request_rejects_tampered_absolute_deadline_fields() {
        let temp = tempfile::tempdir().expect("worker deadline tempdir");
        let (mut manifest, _) = synthetic_worker_manifest(&temp);
        manifest.work_budget_deadline.deadline_seconds =
            manifest.work_budget_deadline.started_at_seconds;
        let ordering_error = validate_worker_request_manifest(&manifest)
            .expect_err("non-increasing worker deadline must fail closed");
        assert!(
            ordering_error.to_string().contains("later than started_at"),
            "{ordering_error:#}"
        );

        manifest.work_budget_deadline.deadline_seconds = 2;
        manifest.work_budget_deadline.deadline_nanoseconds = 1_000_000_000;
        let precision_error = validate_worker_request_manifest(&manifest)
            .expect_err("out-of-range worker deadline nanoseconds must fail closed");
        assert!(
            precision_error.to_string().contains("nanoseconds"),
            "{precision_error:#}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_request_rejects_missing_or_tampered_execution_attestation() {
        let temp = tempfile::tempdir().expect("worker attestation tempdir");
        let (mut manifest, _) = synthetic_worker_manifest(&temp);
        manifest.execution_attestation.worker_executable_sha256 = "not-a-sha256".to_string();
        let tampered = validate_worker_request_manifest(&manifest)
            .expect_err("tampered worker executable identity must fail closed");
        assert!(
            tampered.to_string().contains("worker executable SHA-256"),
            "{tampered:#}"
        );

        let (manifest, _) = synthetic_worker_manifest(&temp);
        let mut missing = serde_json::to_value(manifest).expect("serialize worker request");
        missing
            .as_object_mut()
            .expect("worker request is an object")
            .remove("execution_attestation");
        let missing =
            serde_json::from_value::<SourceUniverseOperatorWorkerRequestManifest>(missing)
                .expect_err("missing execution attestation must fail closed");
        assert!(missing.to_string().contains("execution_attestation"));

        let (manifest, _) = synthetic_worker_manifest(&temp);
        let mut foreign = serde_json::to_value(manifest).expect("serialize worker request");
        foreign["execution_attestation"]["provenance"] =
            serde_json::Value::String("discovered_current_terminal".to_string());
        let foreign =
            serde_json::from_value::<SourceUniverseOperatorWorkerRequestManifest>(foreign)
                .expect_err("foreign execution provenance must fail closed");
        assert!(foreign.to_string().contains("unknown variant"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn durable_discovery_worker_request_contains_no_selected_object_payload() {
        let temp = tempfile::tempdir().expect("discovery worker request tempdir");
        let (mut manifest, _) = synthetic_worker_manifest(&temp);
        manifest.request_kind = SourceUniverseOperatorWorkerRequestKind::Discover;
        manifest
            .payloads
            .retain(|payload| payload.role != WORKER_REQUEST_ROLE_SELECTED_OBJECT);

        validate_worker_request_manifest(&manifest)
            .expect("discovery request with controls and no source payload validates");
        assert!(
            worker_request_payload(&manifest, WORKER_REQUEST_ROLE_SELECTED_OBJECT).is_err(),
            "discovery request must have no selected-object byte range"
        );
        let manifest_bytes = u64::try_from(
            serde_json::to_vec(&manifest)
                .expect("serialize discovery manifest")
                .len(),
        )
        .expect("discovery manifest length");
        let expected_archive_bytes = u64::try_from(std::mem::size_of::<u64>())
            .expect("header width")
            .checked_add(manifest_bytes)
            .and_then(|total| {
                manifest
                    .payloads
                    .iter()
                    .try_fold(total, |total, payload| total.checked_add(payload.bytes))
            })
            .expect("discovery archive byte total");
        assert_eq!(
            worker_request_archive_expected_bytes(manifest_bytes, &manifest)
                .expect("discovery archive size"),
            expected_archive_bytes
        );
    }

    #[test]
    fn delayed_child_reconstructs_the_original_parent_deadline() {
        let plan_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root")
            .join(
                "specs/023-nt-research-analytics-platform/reference/\
                 source-universe-execution-packs/binance-data-vision-trades-2026-03-01-all-instruments/\
                 execution-pack/runs/00000-source-universe-operator-run-binance-data-vision-trades-\
                 2026-03-01-all-instruments-00000/backfill-execution-plan.json",
            );
        let plan: BackfillExecutionPlan =
            serde_json::from_slice(&fs::read(&plan_path).expect("read reference execution plan"))
                .expect("parse reference execution plan");
        let started_at = Duration::from_secs(10);
        let deadline = started_at
            .checked_add(Duration::from_secs(plan.max_wall_seconds))
            .expect("deadline");
        let clock = Arc::new(ManualWorkBudgetClock::default());
        clock.set(deadline.checked_sub(Duration::from_secs(1)).unwrap());
        let interval = OperatorWorkBudgetDeadline {
            started_at_seconds: started_at.as_secs(),
            started_at_nanoseconds: started_at.subsec_nanos(),
            deadline_seconds: deadline.as_secs(),
            deadline_nanoseconds: deadline.subsec_nanos(),
        };

        let guard = OperatorWorkBudgetGuard::from_execution_plan_with_absolute_deadline_and_clock(
            &plan,
            interval,
            clock.clone(),
        )
        .expect("delayed child retains the parent interval");
        assert_eq!(
            guard
                .remaining_wall_time(OperatorWorkBudgetStage::Backtest)
                .expect("remaining deadline"),
            Some(Duration::from_secs(1)),
            "child startup must not receive a fresh max_wall_seconds interval"
        );
        clock.set(deadline);
        assert!(
            guard
                .check_deadline(OperatorWorkBudgetStage::Backtest)
                .expect_err("original parent deadline must expire")
                .to_string()
                .contains("max_wall_seconds")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn anonymous_worker_archive_roundtrips_without_namespace_residue() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let request_root = temp.path().join("request-root");
        fs::create_dir(&request_root).expect("create request root");
        let request_root = request_root.canonicalize().expect("canonical request root");
        let before = fs::read_dir(&request_root)
            .expect("read request root")
            .count();
        let (manifest, payloads) = synthetic_worker_manifest(&temp);
        let (raw, manifest_bytes, _) = encode_synthetic_worker_archive(&manifest, &payloads);
        let archive = anonymous_test_archive(&request_root, &raw);

        assert_eq!(
            fs::read_dir(&request_root)
                .expect("read request root")
                .count(),
            before,
            "O_TMPFILE archive must create no namespace entry"
        );
        let identity = anonymous_worker_request_archive_identity(&archive, true)
            .expect("anonymous read-only identity");
        assert_eq!(identity.byte_len, u64::try_from(raw.len()).unwrap());
        assert!(identity.byte_len > manifest_bytes);
        let manifest_body = read_worker_archive_range(
            &archive,
            u64::try_from(std::mem::size_of::<u64>()).unwrap(),
            manifest_bytes,
            "manifest",
        )
        .expect("read manifest");
        let decoded: SourceUniverseOperatorWorkerRequestManifest =
            serde_json::from_slice(&manifest_body).expect("decode manifest");
        assert_eq!(decoded.source_bindings_path, manifest.source_bindings_path);
        for (expected, payload) in manifest.payloads.iter().zip(payloads.iter()) {
            assert_eq!(
                read_worker_request_payload_unbudgeted(
                    &archive,
                    manifest_bytes,
                    &manifest,
                    expected,
                    manifest_bytes.max(expected.bytes),
                )
                .expect("roundtrip payload"),
                *payload
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_manifest_rejects_missing_duplicate_reordered_and_overflowed_payloads() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let (manifest, _) = synthetic_worker_manifest(&temp);

        let mut missing = manifest.clone();
        missing.payloads.pop();
        assert!(validate_worker_request_manifest(&missing).is_err());

        let mut duplicate = manifest.clone();
        duplicate.payloads[1] = duplicate.payloads[0].clone();
        assert!(validate_worker_request_manifest(&duplicate).is_err());

        let mut reordered = manifest.clone();
        reordered.payloads.swap(0, 1);
        assert!(validate_worker_request_manifest(&reordered).is_err());

        let mut overflowed = manifest;
        overflowed.record.selected_object_bytes = u64::MAX;
        overflowed.payloads[3].bytes = u64::MAX;
        validate_worker_request_manifest(&overflowed)
            .expect("overflow fixture is structurally valid");
        assert!(worker_request_archive_expected_bytes(1, &overflowed).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_archive_rejects_noncanonical_manifest_before_control_parse() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let request_root = temp.path().join("request-root");
        fs::create_dir(&request_root).expect("create request root");
        let request_root = request_root.canonicalize().expect("canonical request root");
        let (manifest, payloads) = synthetic_worker_manifest(&temp);
        let manifest_body = serde_json::to_vec_pretty(&manifest).expect("pretty manifest");
        let manifest_bytes = u64::try_from(manifest_body.len()).unwrap();
        let mut raw = Vec::new();
        raw.extend_from_slice(&manifest_bytes.to_be_bytes());
        raw.extend_from_slice(&manifest_body);
        for payload in payloads {
            raw.extend_from_slice(&payload);
        }
        let archive = anonymous_test_archive(&request_root, &raw);
        let error = execute_source_universe_operator_worker_from_archive(
            &archive,
            u64::try_from(raw.len()).unwrap(),
            manifest_bytes,
            &hex::encode(Sha256::digest(&manifest_body)),
            manifest_bytes,
        )
        .expect_err("noncanonical manifest must fail");
        assert!(error.to_string().contains("not canonical"), "{error:#}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_archive_rejects_wrong_trailing_and_truncated_frames() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let baseline = synthetic_worker_archive(&temp);
        let request_root = temp
            .path()
            .join("request-root")
            .canonicalize()
            .expect("canonical request root");

        let mut wrong_header = baseline.raw.clone();
        wrong_header[..std::mem::size_of::<u64>()]
            .copy_from_slice(&baseline.manifest_bytes.saturating_add(1).to_be_bytes());
        let wrong_header = anonymous_test_archive(&request_root, &wrong_header);
        assert!(
            execute_source_universe_operator_worker_from_archive(
                &wrong_header,
                baseline.archive_bytes,
                baseline.manifest_bytes,
                &baseline.manifest_sha256,
                baseline.manifest_bytes,
            )
            .expect_err("wrong header must fail")
            .to_string()
            .contains("manifest-length header")
        );

        let mut trailing = baseline.raw.clone();
        trailing.push(0);
        let trailing = anonymous_test_archive(&request_root, &trailing);
        assert!(
            execute_source_universe_operator_worker_from_archive(
                &trailing,
                trailing.metadata().unwrap().len(),
                baseline.manifest_bytes,
                &baseline.manifest_sha256,
                baseline.manifest_bytes,
            )
            .expect_err("trailing frame must fail")
            .to_string()
            .contains("truncated or trailing")
        );

        let mut truncated = baseline.raw.clone();
        truncated.pop();
        let truncated = anonymous_test_archive(&request_root, &truncated);
        assert!(
            execute_source_universe_operator_worker_from_archive(
                &truncated,
                truncated.metadata().unwrap().len(),
                baseline.manifest_bytes,
                &baseline.manifest_sha256,
                baseline.manifest_bytes,
            )
            .expect_err("truncated frame must fail")
            .to_string()
            .contains("truncated or trailing")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_archive_rejects_role_order_and_member_hash_drift() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let request_root = temp.path().join("request-root");
        fs::create_dir(&request_root).expect("create request root");
        let request_root = request_root.canonicalize().expect("canonical request root");
        let (mut manifest, payloads) = synthetic_worker_manifest(&temp);
        manifest.payloads.swap(0, 1);
        assert!(validate_worker_request_manifest(&manifest).is_err());

        manifest.payloads.swap(0, 1);
        let (mut raw, manifest_bytes, manifest_sha256) =
            encode_synthetic_worker_archive(&manifest, &payloads);
        let plan_offset: usize = worker_request_payload_offset(
            manifest_bytes,
            &manifest,
            WORKER_REQUEST_ROLE_EXECUTION_PLAN,
        )
        .expect("plan offset")
        .try_into()
        .expect("plan offset fits usize");
        raw[plan_offset] ^= 1;
        let archive = anonymous_test_archive(&request_root, &raw);
        let error = execute_source_universe_operator_worker_from_archive(
            &archive,
            u64::try_from(raw.len()).unwrap(),
            manifest_bytes,
            &manifest_sha256,
            manifest_bytes.max(1),
        )
        .expect_err("member hash mismatch must fail");
        assert!(error.to_string().contains("SHA-256 mismatch"), "{error:#}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_archive_rejects_writable_and_linked_descriptors() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let request_root = temp.path().canonicalize().expect("canonical request root");
        let writable = create_anonymous_worker_request_file(&request_root)
            .expect("create writable anonymous archive");
        assert!(
            anonymous_worker_request_archive_identity(&writable, true)
                .expect_err("writable descriptor must fail")
                .to_string()
                .contains("read-only")
        );

        let linked_path = request_root.join("linked");
        fs::write(&linked_path, b"linked").expect("write linked file");
        let linked = fs::File::open(&linked_path).expect("open linked file");
        assert!(
            anonymous_worker_request_archive_identity(&linked, true)
                .expect_err("linked descriptor must fail")
                .to_string()
                .contains("nlink == 0")
        );
    }

    #[test]
    fn launch_artifact_rejects_length_and_hash_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("artifact");
        fs::write(&path, b"artifact").expect("write launch artifact");
        let pin = SourceUniverseBatchArtifactPin::pin_current_path(&path, u64::MAX)
            .expect("pin artifact");
        assert_eq!(
            read_launch_artifact(&pin, pin.bytes).expect("exact launch cap is inclusive"),
            b"artifact"
        );
        let mut wrong_length = pin.clone();
        wrong_length.bytes += 1;
        assert!(read_launch_artifact(&wrong_length, u64::MAX).is_err());
        let mut wrong_hash = pin;
        wrong_hash.sha256 = "0".repeat(64);
        assert!(read_launch_artifact(&wrong_hash, u64::MAX).is_err());
    }

    #[test]
    fn launch_artifact_cap_is_checked_before_path_access() {
        let pin = SourceUniverseBatchArtifactPin::try_new(
            PathBuf::from("missing-launch-artifact-must-not-be-opened"),
            2,
            "0".repeat(64),
        )
        .expect("synthetic launch pin");

        let error = read_launch_artifact(&pin, 1)
            .expect_err("declared bytes above the configured cap must fail before path access");

        assert!(
            error.to_string().contains("exceeds configured maximum"),
            "{error:#}"
        );
        assert!(
            !format!("{error:#}").contains("open pinned regular file"),
            "byte-cap rejection must precede filesystem access: {error:#}"
        );
    }

    #[test]
    fn launch_artifact_constructor_caps_pack_before_path_access() {
        let oversized = SourceUniverseBatchArtifactPin::try_new(
            PathBuf::from("missing-oversized-launch-artifact"),
            2,
            "1".repeat(64),
        )
        .expect("oversized synthetic launch pin");
        let limits = SourceUniverseBatchBootstrapLimits {
            max_launch_artifact_bytes: 1,
            max_control_artifact_bytes: 1,
            max_retained_control_input_bytes: 1,
        };

        let pack_error = SourceUniverseBatchLaunchArtifacts::try_new(oversized, limits)
            .expect_err("oversized execution pack must fail from its declared pin");
        assert!(
            pack_error.to_string().contains("execution pack")
                && pack_error
                    .to_string()
                    .contains("exceeds configured maximum"),
            "{pack_error:#}"
        );
    }

    #[test]
    fn borrowed_execution_pack_context_preserves_existing_digest() {
        let pack = SourceUniverseExecutionPack {
            schema_version: SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION.to_string(),
            pack_id: "pack".to_string(),
            status: SourceUniverseExecutionPackStatus::Ready,
            work_order_id: "work-order".to_string(),
            input_id: "input".to_string(),
            gate_id: "gate".to_string(),
            conversion_run_plan_id: "conversion-run-plan".to_string(),
            universe_id: "universe".to_string(),
            venue: "venue".to_string(),
            source: "source".to_string(),
            family: "family".to_string(),
            table_family: "trades".to_string(),
            planned_object_count: 1,
            executable_record_count: 1,
            withheld_record_count: 0,
            selected_record_count: 1,
            materialized_record_count: 1,
            skipped_executable_record_count: 0,
            executable_source_bytes: 2,
            materialized_source_bytes: 2,
            artifact_refs: vec![crate::reference_artifact::ReferenceArtifactPin {
                role: "source_bindings".to_string(),
                path: PathBuf::from("source-bindings.toml"),
                sha256: "a".repeat(64),
            }],
            records: Vec::new(),
            blocking_reasons: vec!["synthetic reason".to_string()],
        };
        let mut legacy_context =
            serde_json::to_value(&pack).expect("serialize legacy execution-pack context");
        legacy_context
            .as_object_mut()
            .expect("execution pack serializes as an object")
            .remove("records")
            .expect("legacy context contains records");
        let legacy_digest = crate::reference_artifact::canonical_json_sha256(&legacy_context)
            .expect("hash legacy context");

        assert_eq!(
            execution_pack_context_sha256(&pack).expect("hash borrowed context"),
            legacy_digest,
            "removing the full-pack clone must not invalidate execution-record fingerprints"
        );
    }

    #[test]
    fn selected_control_envelope_is_exact_deduplicated_selected_and_overflow_safe() {
        let controls = tempfile::tempdir().expect("synthetic control root");
        fs::write(controls.path().join("source-bindings.toml"), b"shared")
            .expect("write shared source bindings");
        for sequence in 0..2 {
            fs::write(
                controls.path().join(format!("run-spec-{sequence}.toml")),
                b"run",
            )
            .expect("write run spec");
            fs::write(
                controls.path().join(format!("accepted-{sequence}.json")),
                b"accepted",
            )
            .expect("write accepted tranche");
            fs::write(
                controls.path().join(format!("execution-{sequence}.json")),
                b"execution",
            )
            .expect("write execution plan");
        }
        let record = |sequence: u64| SourceUniverseExecutionPackRecord {
            sequence,
            work_item_id: format!("work-{sequence}"),
            operator_run_id: format!("operator-{sequence}"),
            source_binding: "binding".to_string(),
            category: "category".to_string(),
            symbol: "SYMBOL".to_string(),
            archive_date: "2026-01-01".to_string(),
            source_uri: "s3://bucket/object".to_string(),
            source_url: "https://example.invalid/object".to_string(),
            selected_object_sha256: "a".repeat(64),
            selected_object_bytes: 1,
            source_proof_id: "proof".to_string(),
            source_proof_version: 1,
            accepted_tranche_id: "tranche".to_string(),
            output_prefix: format!("output-{sequence}"),
            source_bindings_path: PathBuf::from("source-bindings.toml"),
            source_bindings_bytes: 6,
            source_bindings_sha256: "b".repeat(64),
            run_spec_path: PathBuf::from(format!("run-spec-{sequence}.toml")),
            run_spec_bytes: 3,
            run_spec_sha256: "c".repeat(64),
            accepted_tranche_path: PathBuf::from(format!("accepted-{sequence}.json")),
            accepted_tranche_bytes: 8,
            accepted_tranche_sha256: "d".repeat(64),
            execution_plan_path: PathBuf::from(format!("execution-{sequence}.json")),
            execution_plan_bytes: 9,
            execution_plan_sha256: "e".repeat(64),
        };
        let mut pack = SourceUniverseExecutionPack {
            schema_version: SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION.to_string(),
            pack_id: "synthetic-control-envelope".to_string(),
            status: SourceUniverseExecutionPackStatus::Ready,
            work_order_id: "work-order".to_string(),
            input_id: "input".to_string(),
            gate_id: "gate".to_string(),
            conversion_run_plan_id: "conversion-plan".to_string(),
            universe_id: "universe".to_string(),
            venue: "venue".to_string(),
            source: "source".to_string(),
            family: "family".to_string(),
            table_family: "trades".to_string(),
            planned_object_count: 2,
            executable_record_count: 2,
            withheld_record_count: 0,
            selected_record_count: 2,
            materialized_record_count: 2,
            skipped_executable_record_count: 0,
            executable_source_bytes: 2,
            materialized_source_bytes: 2,
            artifact_refs: vec![crate::reference_artifact::ReferenceArtifactPin {
                role: "source_bindings".to_string(),
                path: PathBuf::from("source-bindings.toml"),
                sha256: "b".repeat(64),
            }],
            records: vec![record(0), record(1)],
            blocking_reasons: Vec::new(),
        };
        let pack_base_dir = controls.path();
        let first = pack.records.first().expect("synthetic pack has records");
        let second = pack.records.get(1).expect("synthetic pack has two records");
        assert_eq!(
            first.source_bindings_path, second.source_bindings_path,
            "fixture must exercise shared source-bindings deduplication"
        );
        assert_ne!(
            first.run_spec_path, second.run_spec_path,
            "fixture must exercise record-local typed controls"
        );
        let first_triple = first
            .run_spec_bytes
            .checked_add(first.accepted_tranche_bytes)
            .and_then(|bytes| bytes.checked_add(first.execution_plan_bytes))
            .expect("first control triple bytes");
        let second_triple = second
            .run_spec_bytes
            .checked_add(second.accepted_tranche_bytes)
            .and_then(|bytes| bytes.checked_add(second.execution_plan_bytes))
            .expect("second control triple bytes");
        let exact_two_record_envelope = first_triple
            .checked_add(second_triple)
            .and_then(|bytes| bytes.checked_add(first.source_bindings_bytes))
            .and_then(|bytes| bytes.checked_mul(2))
            .expect("two-record retained input envelope");
        let max_control = [
            first.run_spec_bytes,
            first.accepted_tranche_bytes,
            first.execution_plan_bytes,
            first.source_bindings_bytes,
            second.run_spec_bytes,
            second.accepted_tranche_bytes,
            second.execution_plan_bytes,
            second.source_bindings_bytes,
        ]
        .into_iter()
        .max()
        .expect("control byte lengths");
        let exact_limits = SourceUniverseBatchBootstrapLimits {
            max_launch_artifact_bytes: 1,
            max_control_artifact_bytes: max_control,
            max_retained_control_input_bytes: exact_two_record_envelope,
        };
        validate_selected_control_input_envelope(&pack, pack_base_dir, None, 2, exact_limits)
            .expect("exact aggregate boundary must succeed");

        let aggregate_error = validate_selected_control_input_envelope(
            &pack,
            pack_base_dir,
            None,
            2,
            SourceUniverseBatchBootstrapLimits {
                max_retained_control_input_bytes: exact_two_record_envelope - 1,
                ..exact_limits
            },
        )
        .expect_err("aggregate boundary minus one must fail");
        assert!(
            aggregate_error
                .to_string()
                .contains("max_retained_control_input_bytes"),
            "{aggregate_error:#}"
        );

        let second_only_envelope = second_triple
            .checked_add(second.source_bindings_bytes)
            .and_then(|bytes| bytes.checked_mul(2))
            .expect("second-record retained input envelope");
        let first_run_spec_bytes = pack.records[0].run_spec_bytes;
        pack.records[0].run_spec_bytes = u64::MAX;
        validate_selected_control_input_envelope(
            &pack,
            pack_base_dir,
            Some(pack.records[1].sequence),
            1,
            SourceUniverseBatchBootstrapLimits {
                max_retained_control_input_bytes: second_only_envelope,
                ..exact_limits
            },
        )
        .expect("unselected controls must not consume the envelope");
        pack.records[0].run_spec_bytes = first_run_spec_bytes;

        let cap_plus_one = max_control.checked_add(1).expect("control cap increment");
        let original_bytes = [
            pack.records[0].run_spec_bytes,
            pack.records[0].accepted_tranche_bytes,
            pack.records[0].execution_plan_bytes,
            pack.records[0].source_bindings_bytes,
        ];
        for role_index in 0..4 {
            match role_index {
                0 => pack.records[0].run_spec_bytes = cap_plus_one,
                1 => pack.records[0].accepted_tranche_bytes = cap_plus_one,
                2 => pack.records[0].execution_plan_bytes = cap_plus_one,
                3 => pack.records[0].source_bindings_bytes = cap_plus_one,
                _ => unreachable!(),
            }
            let error = validate_selected_control_input_envelope(
                &pack,
                pack_base_dir,
                None,
                1,
                SourceUniverseBatchBootstrapLimits {
                    max_retained_control_input_bytes: u64::MAX,
                    ..exact_limits
                },
            )
            .expect_err("each control role must enforce its independent cap");
            assert!(
                error.to_string().contains("max_control_artifact_bytes"),
                "role index {role_index}: {error:#}"
            );
            pack.records[0].run_spec_bytes = original_bytes[0];
            pack.records[0].accepted_tranche_bytes = original_bytes[1];
            pack.records[0].execution_plan_bytes = original_bytes[2];
            pack.records[0].source_bindings_bytes = original_bytes[3];
        }

        pack.records[0].run_spec_bytes = u64::MAX;
        let overflow_error = validate_selected_control_input_envelope(
            &pack,
            pack_base_dir,
            None,
            1,
            SourceUniverseBatchBootstrapLimits {
                max_launch_artifact_bytes: 1,
                max_control_artifact_bytes: u64::MAX,
                max_retained_control_input_bytes: u64::MAX,
            },
        )
        .expect_err("aggregate checked-add overflow must fail closed");
        assert!(
            overflow_error.to_string().contains("overflow"),
            "{overflow_error:#}"
        );
        pack.records[0].run_spec_bytes = original_bytes[0];

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let alias_root = tempfile::tempdir().expect("control alias root");
            let target = alias_root.path().join("source-bindings.toml");
            fs::write(&target, b"identity-only fixture").expect("write alias target");
            symlink(&target, alias_root.path().join("binding-a.toml"))
                .expect("create first binding alias");
            symlink(&target, alias_root.path().join("binding-b.toml"))
                .expect("create second binding alias");
            pack.records[0].source_bindings_path = PathBuf::from("binding-a.toml");
            pack.records[1].source_bindings_path = PathBuf::from("binding-b.toml");

            let envelope = validate_selected_control_input_envelope(
                &pack,
                alias_root.path(),
                None,
                2,
                exact_limits,
            )
            .expect("resolved aliases must be charged once like the retained control cache");
            let planned_target = envelope
                .resolved_path(&pack.records[0], "source_bindings")
                .expect("frozen first binding path")
                .to_path_buf();
            let replacement_target = alias_root.path().join("replacement-bindings.toml");
            fs::write(&replacement_target, b"replacement identity")
                .expect("write replacement alias target");
            fs::remove_file(alias_root.path().join("binding-a.toml"))
                .expect("remove first binding alias");
            symlink(
                &replacement_target,
                alias_root.path().join("binding-a.toml"),
            )
            .expect("retarget first binding alias");
            assert_eq!(
                envelope
                    .resolved_path(&pack.records[0], "source_bindings")
                    .expect("planned path survives alias retarget"),
                planned_target,
                "verification must consume the path frozen during envelope accounting"
            );
            assert_ne!(
                resolve_pack_control_path(
                    alias_root.path(),
                    &pack.records[0].source_bindings_path,
                )
                .expect("retargeted alias resolves"),
                planned_target,
                "test must prove the ambient alias changed after preflight"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_rejects_manifest_above_bootstrap_cap_before_allocation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = synthetic_worker_archive(&temp);
        let cap = archive
            .manifest_bytes
            .checked_sub(1)
            .expect("manifest is non-empty");

        let error = execute_source_universe_operator_worker_from_archive(
            &archive.file,
            archive.archive_bytes,
            archive.manifest_bytes,
            &archive.manifest_sha256,
            cap,
        )
        .expect_err("oversized bootstrap manifest must fail closed");

        assert!(
            error
                .to_string()
                .contains("exceed configured bootstrap cap"),
            "{error:#}"
        );

        let hash_error = execute_source_universe_operator_worker_from_archive(
            &archive.file,
            archive.archive_bytes,
            archive.manifest_bytes,
            &"0".repeat(64),
            archive.manifest_bytes,
        )
        .expect_err("manifest hash mismatch must fail closed");
        assert!(
            hash_error.to_string().contains("manifest SHA-256 mismatch"),
            "{hash_error:#}"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn per_record_clock_expiry_isolated_and_continue_on_error_advances() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root");
        let committed = crate::source_universe_batch_launch::discover_committed_source_universe_execution_packs(
            repo_root,
        )
        .expect("discover committed execution packs");
        let (seed_pack_path, mut pack) = committed
            .iter()
            .find_map(|committed_pack| {
                let bytes = fs::read(&committed_pack.summary_path).ok()?;
                let pack: SourceUniverseExecutionPack = serde_json::from_slice(&bytes).ok()?;
                (pack.table_family == "trades" && !pack.records.is_empty())
                    .then_some((committed_pack.summary_path.as_path(), pack))
            })
            .expect("committed registry contains a trade execution pack");
        let seed_record = pack.records.first().expect("seed record").clone();
        let seed_base_dir = seed_pack_path.parent().expect("seed pack parent");
        let second_sequence = seed_record
            .sequence
            .checked_add(1)
            .expect("synthetic sequence increment");
        let second_operator_run_id = format!("{}-synthetic-second", seed_record.operator_run_id);
        let synthetic_parent = repo_root.join("target");
        fs::create_dir_all(&synthetic_parent).expect("create repository target directory");
        let synthetic_pack_root =
            tempfile::tempdir_in(&synthetic_parent).expect("synthetic execution-pack root");

        let seed_run_spec_path =
            resolve_pack_control_path(seed_base_dir, &seed_record.run_spec_path)
                .expect("resolve seed run spec");
        let seed_execution_plan_path =
            resolve_pack_control_path(seed_base_dir, &seed_record.execution_plan_path)
                .expect("resolve seed execution plan");
        let synthetic_run_spec_bytes = fs::read_to_string(&seed_run_spec_path)
            .expect("read seed run spec")
            .replace(&seed_record.operator_run_id, &second_operator_run_id)
            .into_bytes();
        let mut synthetic_execution_plan: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&seed_execution_plan_path)
                .expect("read seed execution plan")
                .replace(&seed_record.operator_run_id, &second_operator_run_id),
        )
        .expect("parse synthetic execution plan");
        synthetic_execution_plan["run_spec_hash"] =
            serde_json::Value::String(crate::hashing::sha256_hex(&synthetic_run_spec_bytes));
        let synthetic_execution_plan_bytes = serde_json::to_vec_pretty(&synthetic_execution_plan)
            .expect("serialize synthetic execution plan");
        let synthetic_run_spec_path = synthetic_pack_root.path().join("second-run-spec.toml");
        let synthetic_execution_plan_path = synthetic_pack_root
            .path()
            .join("second-execution-plan.json");
        fs::write(&synthetic_run_spec_path, &synthetic_run_spec_bytes)
            .expect("write synthetic run spec");
        fs::write(
            &synthetic_execution_plan_path,
            &synthetic_execution_plan_bytes,
        )
        .expect("write synthetic execution plan");

        let mut second_record = seed_record.clone();
        second_record.sequence = second_sequence;
        second_record.work_item_id = format!("{}-synthetic-second", seed_record.work_item_id);
        second_record.operator_run_id = second_operator_run_id;
        second_record.run_spec_path = PathBuf::from("second-run-spec.toml");
        second_record.run_spec_bytes =
            u64::try_from(synthetic_run_spec_bytes.len()).expect("run-spec length fits u64");
        second_record.run_spec_sha256 = crate::hashing::sha256_hex(&synthetic_run_spec_bytes);
        second_record.execution_plan_path = PathBuf::from("second-execution-plan.json");
        second_record.execution_plan_bytes = u64::try_from(synthetic_execution_plan_bytes.len())
            .expect("execution-plan length fits u64");
        second_record.execution_plan_sha256 =
            crate::hashing::sha256_hex(&synthetic_execution_plan_bytes);

        pack.pack_id = format!("{}-synthetic-two-record", pack.pack_id);
        pack.status = SourceUniverseExecutionPackStatus::Ready;
        pack.planned_object_count = 2;
        pack.executable_record_count = 2;
        pack.withheld_record_count = 0;
        pack.selected_record_count = 2;
        pack.materialized_record_count = 2;
        pack.skipped_executable_record_count = 0;
        pack.executable_source_bytes = seed_record
            .selected_object_bytes
            .checked_mul(2)
            .expect("synthetic executable bytes");
        pack.materialized_source_bytes = pack.executable_source_bytes;
        pack.records = vec![seed_record.clone(), second_record];
        pack.blocking_reasons.clear();
        let pack_path = synthetic_pack_root
            .path()
            .join("source-universe-execution-pack.json");
        let pack_bytes = serde_json::to_vec_pretty(&pack).expect("serialize synthetic pack");
        fs::write(&pack_path, &pack_bytes).expect("write synthetic pack");

        let output = tempfile::tempdir().expect("batch output");
        let mut fetcher = RecordingErrorFetcher::default();
        let mut runner = NeverRunner;
        let max_artifact_bytes = u64::try_from(pack_bytes.len()).expect("pack length fits u64");
        let control_lengths = pack
            .records
            .iter()
            .flat_map(control_artifact_pins)
            .map(|pin| pin.expected_bytes)
            .collect::<Vec<_>>();
        let max_control_artifact_bytes = *control_lengths
            .iter()
            .max()
            .expect("synthetic pack has controls");
        let max_retained_control_input_bytes = control_lengths
            .iter()
            .try_fold(0_u64, |total, bytes| total.checked_add(*bytes))
            .and_then(|total| total.checked_mul(2))
            .expect("synthetic retained-control cap");
        let launch_artifacts = SourceUniverseBatchLaunchArtifacts::try_new(
            SourceUniverseBatchArtifactPin::pin_current_path(&pack_path, max_artifact_bytes)
                .expect("pin pack"),
            SourceUniverseBatchBootstrapLimits {
                max_launch_artifact_bytes: max_artifact_bytes,
                max_control_artifact_bytes,
                max_retained_control_input_bytes,
            },
        )
        .expect("construct bounded launch artifacts");
        let report = execute_source_universe_batch_with_clock_factory(
            "fake-clock-two-record-regression",
            &launch_artifacts,
            output.path(),
            SourceUniverseBatchExecutionConfig {
                record_limit: Some(2),
                continue_on_error: true,
                ..SourceUniverseBatchExecutionConfig::default()
            },
            &mut fetcher,
            &mut runner,
            &FirstRecordExpiresClockFactory::default(),
        )
        .expect("continue-on-error assembles both failed slots");

        assert_eq!(report.failed_record_count, 2);
        assert!(report.records.is_empty());
        assert_eq!(fetcher.calls, vec![second_sequence]);
        assert_eq!(report.failures[0].sequence, seed_record.sequence);
        assert!(report.failures[0].error.contains("max_wall_seconds"));
        assert_eq!(report.failures[1].sequence, second_sequence);
        assert!(
            report.failures[1]
                .error
                .contains("synthetic fetch failure after progression")
        );
    }

    #[test]
    fn unique_attempt_claims_do_not_block_or_modify_live_and_crashed_attempts() {
        let output = tempfile::tempdir().expect("output root");
        let first_root = BatchOutputRootLease::acquire(output.path()).expect("first root lease");
        let second_root = BatchOutputRootLease::acquire(output.path()).expect("second root lease");
        let run_id = "same-operator-run";
        let work_budget = OperatorWorkBudgetGuard::unbounded();

        let first = BatchOutputChildClaim::acquire(&first_root, run_id, &work_budget)
            .expect("first unique attempt claim");
        fs::write(first.canonical_path.join("partial"), b"crash residue")
            .expect("plant crashed attempt residue");
        let second = BatchOutputChildClaim::acquire(&second_root, run_id, &work_budget)
            .expect("concurrent unique attempt claim");
        fs::write(second.canonical_path.join("live"), b"live attempt")
            .expect("plant live attempt marker");
        let retry = BatchOutputChildClaim::acquire(&first_root, run_id, &work_budget)
            .expect("retry gets another unique attempt");

        assert_ne!(first.canonical_path, second.canonical_path);
        assert_ne!(first.canonical_path, retry.canonical_path);
        assert_ne!(second.canonical_path, retry.canonical_path);
        assert_eq!(
            fs::read(first.canonical_path.join("partial")).expect("crash residue untouched"),
            b"crash residue"
        );
        assert_eq!(
            fs::read(second.canonical_path.join("live")).expect("live attempt untouched"),
            b"live attempt"
        );
        first
            .revalidate(&first_root)
            .expect("first identity retained");
        second
            .revalidate(&second_root)
            .expect("second identity retained");
        retry
            .revalidate(&first_root)
            .expect("retry identity retained");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn leased_report_publication_rejects_output_root_symlink_replacement() {
        let temp = tempfile::tempdir().expect("report publication tempdir");
        let record = discovered_record(crate::hashing::sha256_hex(b"catalog"));
        let owned_plan = owned_plan_for_record(&record, temp.path());
        let report = assemble_report(
            "lease-bound-publication",
            &owned_plan,
            vec![Some(RecordSlot::Discovered(record))],
        )
        .expect("assemble report");
        let original_root = owned_plan.output_root_lease.canonical_path.clone();
        let displaced_root = original_root.with_extension("displaced");
        let replacement_root = original_root.with_extension("replacement");
        fs::create_dir(&replacement_root).expect("create replacement root");
        fs::rename(&original_root, &displaced_root).expect("displace leased root");
        std::os::unix::fs::symlink(&replacement_root, &original_root)
            .expect("replace leased root with symlink");

        let error = write_source_universe_batch_execution_report_with_lease(
            &owned_plan.output_root_lease,
            &report,
        )
        .expect_err("root replacement must fail before report publication");

        assert!(
            format!("{error:#}").contains("leased batch output root"),
            "{error:#}"
        );
        assert!(
            !replacement_root
                .join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE)
                .exists()
        );
        assert!(
            !displaced_root
                .join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE)
                .exists()
        );
    }

    fn one_second_test_guard(clock: Arc<ManualWorkBudgetClock>) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
                max_decoded_bytes: u64::MAX,
                max_source_rows: 1,
                max_projected_row_groups: 1,
                max_wall_seconds: 1,
                require_object_selection_metadata: false,
            }),
            clock,
        )
        .expect("one-second work-budget guard")
    }

    #[test]
    fn terminal_operator_success_is_not_retroactively_rechecked() {
        let clock = Arc::new(ManualWorkBudgetClock::default());
        let guard = one_second_test_guard(Arc::clone(&clock));

        let result = run_operator_with_terminal_ownership(&guard, || {
            clock.expire();
            Ok::<_, anyhow::Error>(SourceUniverseOperatorRunOutcome::Committed(
                SourceUniverseCommittedRunReceipt {
                    output: SourceUniverseBatchExecutionRunOutput::try_new(
                        7,
                        7,
                        crate::hashing::sha256_hex(b"catalog"),
                    )?,
                    worker_executable_sha256: synthetic_test_worker_executable_sha256(),
                    durable_completion: synthetic_test_durable_completion(),
                },
            ))
        })
        .expect("a terminal-owner success cannot be reclassified after its commit");

        assert_eq!(result.output.canonical_rows(), 7);
        assert_eq!(
            result.worker_executable_sha256,
            synthetic_test_worker_executable_sha256()
        );
    }

    #[cfg(unix)]
    #[test]
    fn successful_quiesced_child_candidate_and_exact_receipt_become_committed() {
        use std::os::unix::process::ExitStatusExt;

        let summary = OperatorRunSummary {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: crate::hashing::sha256_hex(b"catalog"),
        };
        let receipt = DurableRunReceipt {
            completion: synthetic_test_durable_completion(),
            execution_attestation: DurableExecutionAttestation::new_process_isolated(
                synthetic_test_worker_executable_sha256(),
            )
            .expect("synthetic process-isolated execution attestation"),
            run_id: "synthetic-run".to_string(),
            submitted_manifest_hash: crate::hashing::sha256_hex(b"synthetic manifest"),
            canonical_rows: summary.canonical_rows,
            nt_catalog_rows: summary.nt_catalog_rows,
            catalog_hash: summary.catalog_hash.clone(),
        };
        let receipt_bytes = crate::reference_artifact::canonical_json_bytes(&receipt)
            .expect("canonical synthetic durable receipt");
        let expected_receipt_bytes = receipt_bytes.clone();

        let outcome = accept_quiesced_durable_worker(
            Ok(WorkerExitEvidence {
                status: ExitStatus::from_raw(0),
                receipt_bytes,
            }),
            Ok(DurableOutputCandidateSealProbe::Candidate(summary.clone())),
            synthetic_test_worker_executable_sha256(),
            move |observed_bytes, observed_summary| {
                ensure!(
                    observed_bytes == expected_receipt_bytes,
                    "returned durable receipt bytes changed"
                );
                ensure!(
                    observed_summary == &summary,
                    "candidate summary changed before exact receipt validation"
                );
                Ok(receipt)
            },
            || Ok(()),
        )
        .expect("all three independent success requirements commit the worker outcome");

        let SourceUniverseOperatorRunOutcome::Committed(committed) = outcome else {
            panic!("production durable acceptance must return a committed outcome");
        };
        assert_eq!(committed.output.canonical_rows(), 7);
        assert_eq!(
            committed.worker_executable_sha256,
            synthetic_test_worker_executable_sha256()
        );
        assert_eq!(
            committed.durable_completion,
            synthetic_test_durable_completion()
        );
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_local_candidate_before_remote_terminal_is_indeterminate() {
        use std::os::unix::process::ExitStatusExt;

        let summary = OperatorRunSummary {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: crate::hashing::sha256_hex(b"catalog"),
        };
        let error = accept_quiesced_durable_worker(
            Ok(WorkerExitEvidence {
                status: ExitStatus::from_raw(1 << 8),
                receipt_bytes: Vec::new(),
            }),
            Ok(DurableOutputCandidateSealProbe::Candidate(summary)),
            synthetic_test_worker_executable_sha256(),
            |_, _| panic!("a crashed child must never reach receipt validation"),
            || panic!("a crashed child must never reach final local validation"),
        )
        .expect_err("a local candidate never proves that remote terminal publication committed");

        assert!(is_committed_indeterminate_worker_error(&error));
        assert!(
            error.to_string().contains("exited unsuccessfully"),
            "{error:#}"
        );
    }

    #[test]
    fn indeterminate_process_lifecycle_cannot_reach_candidate_receipt_acceptance() {
        let error = require_quiesced_worker_lifecycle(WorkerLifecycleOutcome::Indeterminate(
            anyhow::anyhow!("synthetic reap uncertainty"),
        ))
        .expect_err("unproven process quiescence must hard-stop before candidate acceptance");

        assert!(is_committed_indeterminate_worker_error(&error));
        assert!(
            error.to_string().contains("quiesced and reaped"),
            "{error:#}"
        );
    }

    #[test]
    fn prelaunch_failure_remains_an_ordinary_precommit_error() {
        let error = require_quiesced_worker_lifecycle(WorkerLifecycleOutcome::NotStarted(
            anyhow::anyhow!("synthetic spawn refusal"),
        ))
        .expect_err("prelaunch failure must fail without probing a local candidate");

        assert!(!is_committed_indeterminate_worker_error(&error));
        assert!(error.to_string().contains("was not started"), "{error:#}");
    }

    #[cfg(unix)]
    #[test]
    fn zero_exit_without_candidate_is_committed_indeterminate() {
        use std::os::unix::process::ExitStatusExt;

        let error = accept_quiesced_durable_worker(
            Ok(WorkerExitEvidence {
                status: ExitStatus::from_raw(0),
                receipt_bytes: Vec::new(),
            }),
            Ok(DurableOutputCandidateSealProbe::Absent),
            synthetic_test_worker_executable_sha256(),
            |_, _| panic!("an absent candidate must stop before receipt validation"),
            || panic!("an absent candidate must stop before final local validation"),
        )
        .expect_err("zero exit without its local candidate cannot be retried");

        assert!(is_committed_indeterminate_worker_error(&error));
    }

    #[test]
    fn wait_error_before_candidate_acceptance_is_committed_indeterminate() {
        let error = accept_quiesced_durable_worker(
            Err(anyhow::anyhow!("synthetic post-start wait failure")),
            Ok(DurableOutputCandidateSealProbe::Absent),
            synthetic_test_worker_executable_sha256(),
            |_, _| panic!("a wait error must stop before receipt validation"),
            || panic!("a wait error must stop before final local validation"),
        )
        .expect_err("a started worker can have remote side effects before wait failure");

        assert!(is_committed_indeterminate_worker_error(&error));
        assert!(
            error.to_string().contains("post-start wait failure"),
            "{error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn occupied_invalid_candidate_is_committed_indeterminate() {
        use std::os::unix::process::ExitStatusExt;

        let error = accept_quiesced_durable_worker(
            Ok(WorkerExitEvidence {
                status: ExitStatus::from_raw(0),
                receipt_bytes: Vec::new(),
            }),
            Err(anyhow::anyhow!("synthetic occupied invalid candidate")),
            synthetic_test_worker_executable_sha256(),
            |_, _| panic!("an invalid candidate must stop before receipt validation"),
            || panic!("an invalid candidate must stop before final local validation"),
        )
        .expect_err("an occupied invalid candidate must stop automatic retry");

        assert!(is_committed_indeterminate_worker_error(&error));
        assert!(
            error.to_string().contains("occupied invalid candidate"),
            "{error:#}"
        );
    }

    #[test]
    fn late_nonterminal_runner_success_is_rejected() {
        let clock = Arc::new(ManualWorkBudgetClock::default());
        let guard = one_second_test_guard(Arc::clone(&clock));

        let Err(error) = run_operator_with_terminal_ownership(&guard, || {
            clock.expire();
            Ok::<_, anyhow::Error>(SourceUniverseOperatorRunOutcome::NonTerminal(
                SourceUniverseBatchExecutionRunOutput::try_new(
                    7,
                    7,
                    crate::hashing::sha256_hex(b"catalog"),
                )?,
            ))
        }) else {
            panic!("ordinary runners cannot fabricate a committed late success");
        };

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn terminal_operator_error_observes_expiry_before_classification() {
        let clock = Arc::new(ManualWorkBudgetClock::default());
        let guard = one_second_test_guard(Arc::clone(&clock));

        let Err(error) = run_operator_with_terminal_ownership(&guard, || {
            clock.expire();
            anyhow::bail!("ordinary runner error")
        }) else {
            panic!("an uncommitted error path must still observe deadline expiry");
        };

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(
            !error.to_string().contains("ordinary runner error"),
            "deadline expiry must take precedence: {error:#}"
        );
    }

    #[test]
    fn committed_indeterminate_error_is_not_reclassified_by_expired_deadline() {
        let clock = Arc::new(ManualWorkBudgetClock::default());
        let guard = one_second_test_guard(Arc::clone(&clock));

        let Err(error) = run_operator_with_terminal_ownership(&guard, || {
            clock.expire();
            Err(committed_indeterminate_worker_error(
                "synthetic terminal-seal ambiguity",
            ))
        }) else {
            panic!("terminal-seal ambiguity must remain a hard-stop classification");
        };

        assert!(is_committed_indeterminate_worker_error(&error));
        assert!(
            error
                .to_string()
                .contains("synthetic terminal-seal ambiguity"),
            "{error:#}"
        );
        assert!(
            !error.to_string().contains("max_wall_seconds"),
            "an expired deadline must not erase terminal ownership ambiguity: {error:#}"
        );
    }

    #[test]
    fn committed_indeterminate_error_stops_even_when_continue_on_error_is_enabled() {
        let temp = tempfile::tempdir().expect("temporary attempt root");
        let attempt_output = SourceUniverseBatchExecutionAttemptIdentity {
            output_dir: temp.path().join("retained-attempt"),
            device: None,
            inode: None,
        };
        let slot = record_error_slot_with_attempt(
            &synthetic_cache_record(),
            &"0".repeat(64),
            "run_operator",
            committed_indeterminate_worker_error("synthetic occupied invalid seal"),
            attempt_output.clone(),
            &SourceUniverseBatchExecutionConfig {
                continue_on_error: true,
                ..SourceUniverseBatchExecutionConfig::default()
            },
        );

        let RecordSlot::Stopped(stopped) = slot else {
            panic!("committed-indeterminate failures must stop the complete batch");
        };
        assert!(is_committed_indeterminate_worker_error(&stopped.error));
        assert!(
            stopped
                .error
                .to_string()
                .contains(&attempt_output.output_dir.display().to_string()),
            "retained attempt identity must remain attached to the hard stop: {:#}",
            stopped.error
        );
    }

    #[test]
    fn expired_terminal_operator_never_invokes_the_runner() {
        let clock = Arc::new(ManualWorkBudgetClock::default());
        let guard = one_second_test_guard(Arc::clone(&clock));
        clock.expire();
        let invoked = AtomicBool::new(false);

        let Err(error) = run_operator_with_terminal_ownership(&guard, || {
            invoked.store(true, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(SourceUniverseOperatorRunOutcome::NonTerminal(
                SourceUniverseBatchExecutionRunOutput::try_new(
                    0,
                    0,
                    crate::hashing::sha256_hex(b"catalog"),
                )?,
            ))
        }) else {
            panic!("an expired record must stop before runner invocation");
        };

        assert!(!invoked.load(Ordering::SeqCst));
        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn effective_http_timeout_selects_the_tighter_available_bound() {
        let short = Duration::from_secs(3);
        let long = Duration::from_secs(9);
        for (configured, remaining, expected) in [
            (Some(short), Some(long), Some(short)),
            (Some(long), Some(short), Some(short)),
            (Some(short), Some(short), Some(short)),
            (Some(short), None, Some(short)),
            (None, Some(short), Some(short)),
            (None, None, None),
        ] {
            assert_eq!(
                effective_http_request_timeout(configured, remaining),
                expected,
                "configured={configured:?}, remaining={remaining:?}"
            );
        }
    }

    #[test]
    fn request_builder_carries_the_selected_effective_timeout() {
        let selected = Duration::from_secs(7);
        let request = apply_http_request_timeout(
            reqwest::Client::new().get("https://example.test/archive/object"),
            Some(selected),
        )
        .build()
        .expect("build request without sending");
        assert_eq!(request.timeout().copied(), Some(selected));

        let unbounded = apply_http_request_timeout(
            reqwest::Client::new().get("https://example.test/archive/object"),
            None,
        )
        .build()
        .expect("build request without sending");
        assert_eq!(unbounded.timeout(), None);
    }

    /// Inner fetcher double for cache-integrity unit tests; must never be called.
    struct PanicFetcher;

    impl SourceUniverseObjectFetcher for PanicFetcher {
        fn fetch(
            &mut self,
            _record: &SourceUniverseExecutionPackRecord,
            _run_spec: &RunSpec,
            _work_budget: &OperatorWorkBudgetGuard,
        ) -> Result<VerifiedSourceObject> {
            panic!("inner fetcher must not be called")
        }
    }

    type TestCachingFetcher = CachingSourceUniverseObjectFetcher<PanicFetcher>;

    fn synthetic_cache_record() -> SourceUniverseExecutionPackRecord {
        let sha256 = hex::encode(Sha256::digest(b"x"));
        SourceUniverseExecutionPackRecord {
            sequence: 0,
            work_item_id: "work-item".to_string(),
            operator_run_id: "operator-run".to_string(),
            source_binding: "source-binding".to_string(),
            category: "spot".to_string(),
            symbol: "BTC-USDT".to_string(),
            archive_date: "2026-07-01".to_string(),
            source_uri: "s3://bucket/object".to_string(),
            source_url: "https://example.invalid/object".to_string(),
            selected_object_sha256: sha256.clone(),
            selected_object_bytes: 1,
            source_proof_id: "proof".to_string(),
            source_proof_version: 1,
            accepted_tranche_id: "tranche".to_string(),
            output_prefix: "s3://bucket/output".to_string(),
            source_bindings_path: PathBuf::from("source-bindings.toml"),
            source_bindings_bytes: 1,
            source_bindings_sha256: sha256.clone(),
            run_spec_path: PathBuf::from("run-spec.toml"),
            run_spec_bytes: 1,
            run_spec_sha256: sha256.clone(),
            accepted_tranche_path: PathBuf::from("accepted-tranche.json"),
            accepted_tranche_bytes: 1,
            accepted_tranche_sha256: sha256.clone(),
            execution_plan_path: PathBuf::from("execution-plan.json"),
            execution_plan_bytes: 1,
            execution_plan_sha256: sha256,
        }
    }

    #[test]
    fn verified_source_object_rejects_wrong_length_at_fetch_boundary() {
        let record = synthetic_cache_record();

        let error = VerifiedSourceObject::verify(
            &record,
            b"xx".to_vec(),
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect_err("wrong-length bytes must not cross the fetch boundary");

        assert!(
            error.to_string().contains("object byte length"),
            "{error:#}"
        );
    }

    #[test]
    fn verified_source_object_rejects_wrong_hash_at_fetch_boundary() {
        let record = synthetic_cache_record();

        let error = VerifiedSourceObject::verify(
            &record,
            b"y".to_vec(),
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect_err("same-length corrupt bytes must not cross the fetch boundary");

        assert!(error.to_string().contains("object sha256"), "{error:#}");
    }

    #[test]
    fn verified_source_object_cannot_be_rebound_to_another_record() {
        let record = synthetic_cache_record();
        let object = VerifiedSourceObject::verify(
            &record,
            b"x".to_vec(),
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect("construct proof for original record");
        let mut other_record = record.clone();
        other_record.selected_object_sha256 = hex::encode(Sha256::digest(b"y"));

        let error = object
            .into_bytes_for(&other_record)
            .expect_err("a proof must remain bound to its selected-object identity");

        assert!(
            error
                .to_string()
                .contains("proof does not match execution-pack record"),
            "{error:#}"
        );
    }

    #[test]
    fn cache_lookup_treats_only_an_absent_entry_as_a_miss() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);

        let cached = fetcher
            .read_verified_cache_entry(&record, &cache_path, &OperatorWorkBudgetGuard::unbounded())
            .expect("absent entry is a cache miss");

        assert!(cached.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cache_lookup_fails_closed_on_a_first_use_symlink_without_deleting_it() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let target = cache_dir.join("target");
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&target, b"x").expect("write symlink target");
        symlink(&target, &cache_path).expect("plant cache symlink");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);

        let error = fetcher
            .read_verified_cache_entry(&record, &cache_path, &OperatorWorkBudgetGuard::unbounded())
            .expect_err("an occupied symlink must fail closed");

        assert!(
            error
                .to_string()
                .contains("occupied object cache entry failed immutable verification"),
            "{error:#}"
        );
        assert!(
            fs::symlink_metadata(&cache_path)
                .expect("symlink is retained for offline diagnosis")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).expect("read untouched target"), b"x");
    }

    #[test]
    fn cache_lookup_fails_closed_on_first_use_corruption_and_retains_it() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&cache_path, b"y").expect("plant corrupt occupied entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);

        let error = fetcher
            .read_verified_cache_entry(&record, &cache_path, &OperatorWorkBudgetGuard::unbounded())
            .expect_err("first-use corruption must fail closed");

        assert!(
            error
                .to_string()
                .contains("occupied object cache entry failed immutable verification"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(&cache_path).expect("corrupt entry remains for offline diagnosis"),
            b"y"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn identical_cache_store_conflict_converges_without_replacing_the_inode() {
        use std::os::unix::fs::MetadataExt;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&cache_path, b"x").expect("plant identical occupied entry");
        let before = fs::symlink_metadata(&cache_path).expect("stat occupied entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);

        fetcher
            .store_verified(&cache_path, b"x", &OperatorWorkBudgetGuard::unbounded())
            .expect("identical immutable store conflict converges");

        let after = fs::symlink_metadata(&cache_path).expect("re-stat occupied entry");
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(fs::read(&cache_path).expect("read converged entry"), b"x");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn conflicting_cache_store_fails_closed_without_replacing_the_occupant() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&cache_path, b"y").expect("plant conflicting occupied entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);

        let error = fetcher
            .store_verified(&cache_path, b"x", &OperatorWorkBudgetGuard::unbounded())
            .expect_err("conflicting immutable store must fail closed");

        assert!(
            format!("{error:#}").contains("different bytes"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(&cache_path).expect("conflicting occupant remains"),
            b"y"
        );
    }

    #[test]
    fn cache_lookup_fails_closed_on_same_length_inode_replacement_after_first_use() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        let replacement = cache_dir.join("replacement");
        fs::write(&cache_path, b"x").expect("plant verified cache entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);
        let work_budget = OperatorWorkBudgetGuard::unbounded();
        fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect("first lookup verifies")
            .expect("first lookup hits cache");
        fs::write(&replacement, b"x").expect("write same-length replacement");
        fs::rename(&replacement, &cache_path).expect("replace verified inode");

        let error = fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect_err("replacement inode must invalidate the per-run proof");

        assert!(
            error
                .to_string()
                .contains("identity changed after this run verified it"),
            "{error:#}"
        );
    }

    #[test]
    fn two_same_run_cache_hits_traverse_sha_exactly_once() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&cache_path, b"x").expect("plant verified cache entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);
        let work_budget = OperatorWorkBudgetGuard::unbounded();

        for _ in 0..2 {
            fetcher
                .read_verified_cache_entry(&record, &cache_path, &work_budget)
                .expect("same-run cache lookup")
                .expect("same-run cache hit");
        }

        assert_eq!(fetcher.run_verification.hash_traversals_for_test(), 1);
    }

    #[test]
    fn cache_lookup_fails_closed_if_a_verified_entry_disappears() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&cache_path, b"x").expect("plant verified cache entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);
        let work_budget = OperatorWorkBudgetGuard::unbounded();
        fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect("first lookup verifies")
            .expect("first lookup hits cache");
        fs::remove_file(&cache_path).expect("remove verified cache entry");

        let error = fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect_err("verified cache disappearance must fail closed");

        assert!(
            error
                .to_string()
                .contains("disappeared after this run verified it"),
            "{error:#}"
        );
    }

    #[test]
    fn cache_lookup_fails_closed_on_same_length_in_place_mutation_after_first_use() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        fs::write(&cache_path, b"x").expect("plant verified cache entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);
        let work_budget = OperatorWorkBudgetGuard::unbounded();
        fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect("first lookup verifies")
            .expect("first lookup hits cache");
        fs::write(&cache_path, b"y").expect("mutate verified inode in place");

        let error = fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect_err("in-place mutation must invalidate the per-run proof");

        assert!(
            error
                .to_string()
                .contains("changed after this run verified it"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(&cache_path).expect("mutated path is retained for offline diagnosis"),
            b"y"
        );
    }

    #[test]
    fn cache_lookup_retains_a_foreign_replacement_after_first_use() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("object-cache");
        fs::create_dir(&cache_dir).expect("create cache dir");
        let record = synthetic_cache_record();
        let cache_path = cache_dir.join(&record.selected_object_sha256);
        let replacement = cache_dir.join("replacement");
        fs::write(&cache_path, b"x").expect("plant verified cache entry");
        let fetcher = TestCachingFetcher::new(PanicFetcher, &cache_dir);
        let work_budget = OperatorWorkBudgetGuard::unbounded();
        fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect("first lookup verifies")
            .expect("first lookup hits cache");
        fs::write(&replacement, b"y").expect("write corrupt foreign replacement");
        fs::rename(&replacement, &cache_path).expect("replace verified inode");

        let error = fetcher
            .read_verified_cache_entry(&record, &cache_path, &work_budget)
            .expect_err("foreign corrupt replacement must fail closed");

        assert!(
            error
                .to_string()
                .contains("identity changed after this run verified it"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(&cache_path).expect("foreign replacement remains present"),
            b"y"
        );
    }

    fn discovered_record(catalog_hash: String) -> SourceUniverseBatchExecutionRecord {
        SourceUniverseBatchExecutionRecord {
            sequence: 0,
            operator_run_id: "operator-run-discovered".to_string(),
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
            completion_provenance:
                SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated,
            completion_resolution: SourceUniverseBatchExecutionCompletionResolution::Discovered,
            attempt_worker_sha256: synthetic_test_current_attempt_worker_sha256(),
            terminal_publisher_worker_sha256: synthetic_test_worker_executable_sha256(),
            durable_completion: Some(synthetic_test_durable_completion()),
        }
    }

    /// Build an [`OwnedBatchPlan`] with one verified pack record for report and
    /// exact-current completion-discovery tests.
    fn owned_plan_for_record(
        record: &SourceUniverseBatchExecutionRecord,
        output_root_parent: &Path,
    ) -> OwnedBatchPlan {
        let pack_record = SourceUniverseExecutionPackRecord {
            sequence: record.sequence,
            work_item_id: "work-item".to_string(),
            operator_run_id: record.operator_run_id.clone(),
            source_binding: record.source_binding.clone(),
            category: record.category.clone(),
            symbol: record.symbol.clone(),
            archive_date: record.archive_date.clone(),
            source_uri: "s3://bucket/object.csv.gz".to_string(),
            source_url: "https://example/object.csv.gz".to_string(),
            selected_object_sha256: record.selected_object_sha256.clone(),
            selected_object_bytes: record.selected_object_bytes,
            source_proof_id: "proof".to_string(),
            source_proof_version: 1,
            accepted_tranche_id: "tranche".to_string(),
            output_prefix: "s3://bucket/out".to_string(),
            source_bindings_path: PathBuf::from("source-bindings.toml"),
            source_bindings_bytes: 0,
            source_bindings_sha256: record.source_bindings_sha256.clone(),
            run_spec_path: PathBuf::from("run-spec.toml"),
            run_spec_bytes: 0,
            run_spec_sha256: record.run_spec_sha256.clone(),
            accepted_tranche_path: PathBuf::from("tranche.json"),
            accepted_tranche_bytes: 0,
            accepted_tranche_sha256: record.accepted_tranche_sha256.clone(),
            execution_plan_path: PathBuf::from("execution-plan.json"),
            execution_plan_bytes: 0,
            execution_plan_sha256: record.execution_plan_sha256.clone(),
        };
        let mut pack = SourceUniverseExecutionPack {
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
            artifact_refs: vec![crate::reference_artifact::ReferenceArtifactPin {
                role: "source_bindings".to_string(),
                path: PathBuf::from("source-bindings.toml"),
                sha256: record.source_bindings_sha256.clone(),
            }],
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
        let source_bindings_sha256 = hex::encode(Sha256::digest(source_bindings_bytes.as_ref()));
        pack.records[0].source_bindings_sha256 = source_bindings_sha256.clone();
        pack.artifact_refs[0]
            .sha256
            .clone_from(&source_bindings_sha256);
        let mut run_spec = controls.run_spec;
        run_spec.source_bindings_path = pack.records[0].source_bindings_path.clone();
        let source_bindings_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(
                "specs/023-nt-research-analytics-platform/reference/\
                 backfill-source-bindings.v1.toml",
            )
            .canonicalize()
            .expect("resolve committed registry");
        let source_bindings = VerifiedSourceBindingRegistry::from_frozen_pack_bytes(
            &run_spec,
            source_bindings_path,
            source_bindings_bytes.clone(),
            &source_bindings_sha256,
        )
        .expect("verify committed registry");
        let mut verified_control_artifacts = BTreeMap::new();
        verified_control_artifacts.insert(
            0,
            SourceUniverseVerifiedControlArtifacts {
                run_spec_path: PathBuf::from("run-spec.toml"),
                run_spec_bytes,
                run_spec: Arc::new(run_spec),
                accepted_tranche_path: PathBuf::from("tranche.json"),
                accepted_tranche_bytes,
                execution_plan_path: PathBuf::from("execution-plan.json"),
                execution_plan_bytes,
                execution_plan: Arc::new(controls.execution_plan),
                source_bindings_path: pack.records[0].source_bindings_path.clone(),
                source_bindings_bytes,
                source_bindings_sha256: source_bindings_sha256.clone(),
                source_bindings,
            },
        );
        let execution_record_sha256s = execution_record_digests(&pack).expect("fingerprint pack");
        static NEXT_OUTPUT_ROOT: AtomicUsize = AtomicUsize::new(0);
        let fresh_output_root = output_root_parent.join(format!(
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
            start_sequence: None,
            record_limit: usize::MAX,
            output_root_lease,
        }
    }

    #[test]
    fn batch_worker_panic_becomes_a_committed_indeterminate_error() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());

        let error = execute_prepared_source_universe_batch_with_factories(
            "panic-regression",
            &owned_plan,
            SourceUniverseBatchExecutionConfig {
                start_sequence: None,
                record_limit: Some(1),
                continue_on_error: true,
                max_concurrent_records: Some(1),
            },
            TEST_SOURCE_UNIVERSE_LIFECYCLE_CLEANUP_LIMITS,
            BatchCompletionPolicy::AllowPublish,
            || Ok(PanicFetcher),
            || Ok(NeverRunner),
        )
        .expect_err("a batch worker panic must fail closed without unwinding the supervisor");

        assert!(is_committed_indeterminate_worker_error(&error));
        assert!(
            error.to_string().contains("batch worker thread panicked"),
            "{error:#}"
        );
    }

    fn exact_current_discovery_report(
        owned_plan: &OwnedBatchPlan,
    ) -> SourceUniverseBatchExecutionReport {
        let record = &owned_plan.pack.records[0];
        let controls = owned_plan
            .verified_control_artifacts
            .get(&record.sequence)
            .expect("test controls");
        let execution_record_sha256 = owned_plan
            .execution_record_sha256s
            .get(&record.sequence)
            .expect("test execution-record digest");
        let mut discovered = reconstructed_discovered_record(
            SourceUniverseDiscoveredRunReceipt {
                attempt_worker_sha256: synthetic_test_current_attempt_worker_sha256(),
                receipt: synthetic_test_durable_receipt(synthetic_test_durable_completion()),
            },
            OperatorRunSummary {
                canonical_rows: 7,
                nt_catalog_rows: 7,
                catalog_hash: crate::hashing::sha256_hex(b"restart catalog"),
            },
            record,
            controls,
            execution_record_sha256,
        );
        discovered.terminal_publisher_worker_sha256 = discovered.attempt_worker_sha256.clone();
        assemble_report(
            "batch",
            owned_plan,
            vec![Some(RecordSlot::Discovered(discovered))],
        )
        .expect("assemble exact-current discovery report")
    }

    struct RestartSpyFetcher {
        calls: Arc<AtomicUsize>,
    }

    impl SourceUniverseObjectFetcher for RestartSpyFetcher {
        fn fetch(
            &mut self,
            _record: &SourceUniverseExecutionPackRecord,
            _run_spec: &RunSpec,
            _work_budget: &OperatorWorkBudgetGuard,
        ) -> Result<VerifiedSourceObject> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("restart verification must never fetch source bytes")
        }
    }

    struct RestartDiscoveryRunner {
        discovery_calls: Arc<AtomicUsize>,
        run_calls: Arc<AtomicUsize>,
        terminal_present: bool,
    }

    impl SourceUniverseOperatorRunner for RestartDiscoveryRunner {
        fn run(
            &mut self,
            _record: &SourceUniverseExecutionPackRecord,
            _object_bytes: Vec<u8>,
            _control_artifacts: &SourceUniverseVerifiedControlArtifacts,
            _output_dir: &Path,
            _work_budget: &OperatorWorkBudgetGuard,
        ) -> Result<SourceUniverseOperatorRunOutcome> {
            self.run_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("restart verification must never execute or publish")
        }

        fn discover_current_completion(
            &mut self,
            _record: &SourceUniverseExecutionPackRecord,
            control_artifacts: &SourceUniverseVerifiedControlArtifacts,
            _output_dir: &Path,
            _work_budget: &OperatorWorkBudgetGuard,
        ) -> Result<Option<SourceUniverseDiscoveredRunReceipt>> {
            self.discovery_calls.fetch_add(1, Ordering::SeqCst);
            if !self.terminal_present {
                return Ok(None);
            }
            let worker = synthetic_test_current_attempt_worker_sha256();
            Ok(Some(SourceUniverseDiscoveredRunReceipt {
                attempt_worker_sha256: worker.clone(),
                receipt: DurableRunReceipt {
                    completion: synthetic_test_durable_completion(),
                    execution_attestation: DurableExecutionAttestation::new_process_isolated(
                        worker,
                    )?,
                    run_id: control_artifacts.run_spec.manifest.run_id.clone(),
                    submitted_manifest_hash: control_artifacts.run_spec.manifest.manifest_hash(),
                    canonical_rows: 7,
                    nt_catalog_rows: 7,
                    catalog_hash: crate::hashing::sha256_hex(b"restart catalog"),
                },
            }))
        }
    }

    fn run_restart_discovery_policy(
        owned_plan: &OwnedBatchPlan,
        terminal_present: bool,
        fetch_calls: Arc<AtomicUsize>,
        discovery_calls: Arc<AtomicUsize>,
        run_calls: Arc<AtomicUsize>,
    ) -> Result<SourceUniverseBatchExecutionReport> {
        execute_prepared_source_universe_batch_with_factories(
            "batch",
            owned_plan,
            SourceUniverseBatchExecutionConfig {
                start_sequence: None,
                record_limit: Some(1),
                continue_on_error: false,
                max_concurrent_records: Some(1),
            },
            TEST_SOURCE_UNIVERSE_LIFECYCLE_CLEANUP_LIMITS,
            BatchCompletionPolicy::RequireExistingRemoteTerminal,
            move || {
                Ok(RestartSpyFetcher {
                    calls: Arc::clone(&fetch_calls),
                })
            },
            move || {
                Ok(RestartDiscoveryRunner {
                    discovery_calls: Arc::clone(&discovery_calls),
                    run_calls: Arc::clone(&run_calls),
                    terminal_present,
                })
            },
        )
    }

    #[test]
    fn restart_policy_discovers_once_with_zero_fetch_run_or_publish_work() {
        let temp = tempfile::tempdir().expect("restart policy tempdir");
        let owned_plan = owned_plan_for_record(&discovered_record("f".repeat(64)), temp.path());
        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let discovery_calls = Arc::new(AtomicUsize::new(0));
        let run_calls = Arc::new(AtomicUsize::new(0));
        let report = run_restart_discovery_policy(
            &owned_plan,
            true,
            Arc::clone(&fetch_calls),
            Arc::clone(&discovery_calls),
            Arc::clone(&run_calls),
        )
        .expect("exact remote terminal satisfies restart-only policy");

        assert_eq!(discovery_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(run_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            report.records[0].completion_resolution,
            SourceUniverseBatchExecutionCompletionResolution::Discovered
        );
    }

    #[test]
    fn restart_policy_missing_remote_terminal_fails_without_fetch_run_or_publish_work() {
        let temp = tempfile::tempdir().expect("missing restart terminal tempdir");
        let owned_plan = owned_plan_for_record(&discovered_record("f".repeat(64)), temp.path());
        let fetch_calls = Arc::new(AtomicUsize::new(0));
        let discovery_calls = Arc::new(AtomicUsize::new(0));
        let run_calls = Arc::new(AtomicUsize::new(0));
        let error = run_restart_discovery_policy(
            &owned_plan,
            false,
            Arc::clone(&fetch_calls),
            Arc::clone(&discovery_calls),
            Arc::clone(&run_calls),
        )
        .expect_err("missing exact remote terminal must fail closed");

        assert!(
            error
                .to_string()
                .contains("exact remote durable terminal is absent"),
            "{error:#}"
        );
        assert_eq!(discovery_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(run_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fresh_publish_restart_discovers_remote_terminal_and_reuses_immutable_report_bytes() {
        let temp = tempfile::tempdir().expect("restart report tempdir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp.path());
        let discovered = exact_current_discovery_report(&owned_plan);
        let mut published = discovered.clone();
        published.records[0].completion_resolution =
            SourceUniverseBatchExecutionCompletionResolution::Published;
        validate_source_universe_batch_execution_report(&published)
            .expect("fresh published report validates");
        validate_existing_batch_report_selection(&published, "batch", &owned_plan)
            .expect("fresh report binds current selection");
        let first = write_source_universe_batch_execution_report_with_lease(
            &owned_plan.output_root_lease,
            &published,
        )
        .expect("publish immutable fresh report");
        let first_bytes = fs::read(&first.path).expect("read fresh report bytes");

        let retained =
            read_existing_batch_report_with_lease(&owned_plan.output_root_lease, 1024 * 1024)
                .expect("read retained report")
                .expect("retained report exists");
        validate_existing_batch_report_against_remote_discovery(&retained, &discovered)
            .expect("same-worker remote discovery confirms retained report");
        write_source_universe_batch_execution_report_with_lease(
            &owned_plan.output_root_lease,
            &retained,
        )
        .expect("restart verifies original immutable bytes");

        assert_eq!(
            fs::read(&first.path).expect("re-read immutable report"),
            first_bytes
        );
        assert_eq!(
            retained.records[0].completion_resolution,
            SourceUniverseBatchExecutionCompletionResolution::Published,
            "restart returns the original publication fact"
        );
    }

    #[test]
    fn retained_report_from_different_attempt_worker_fails_exact_current_restart() {
        let temp = tempfile::tempdir().expect("restart worker tempdir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp.path());
        let discovered = exact_current_discovery_report(&owned_plan);
        let mut retained = discovered.clone();
        retained.records[0].attempt_worker_sha256 = "a".repeat(64);
        validate_source_universe_batch_execution_report(&retained)
            .expect("older-attempt discovery report remains structurally valid");

        let error = validate_existing_batch_report_against_remote_discovery(&retained, &discovered)
            .expect_err("older attempt worker cannot satisfy exact-current restart proof");
        assert!(
            error.to_string().contains("exact-current worker"),
            "{error:#}"
        );
    }

    #[test]
    fn execution_record_digest_selects_exact_sequence_and_rejects_missing_sequence() {
        let temp = tempfile::tempdir().expect("execution record digest tempdir");
        let record = discovered_record("a".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp.path());
        let expected = owned_plan
            .execution_record_sha256s
            .get(&record.sequence)
            .expect("owned plan record digest");

        assert_eq!(
            execution_record_digest(&owned_plan.pack, record.sequence)
                .expect("select exact record digest"),
            expected.clone()
        );
        let error = execution_record_digest(&owned_plan.pack, record.sequence + 1)
            .expect_err("missing sequence must fail closed");
        assert!(
            error.to_string().contains("has no record with sequence"),
            "{error:#}"
        );
    }

    #[test]
    fn reconstructed_discovered_record_uses_only_the_sealed_summary() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let untrusted_record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&untrusted_record, temp_dir.path());
        let record = &owned_plan.pack.records[0];
        let controls = owned_plan
            .verified_control_artifacts
            .get(&record.sequence)
            .expect("controls");
        let execution_record_sha256 = owned_plan
            .execution_record_sha256s
            .get(&record.sequence)
            .expect("record fingerprint");
        let sealed_hash = crate::hashing::sha256_hex(b"sealed catalog");

        let reconstructed = reconstructed_discovered_record(
            SourceUniverseDiscoveredRunReceipt {
                attempt_worker_sha256: synthetic_test_current_attempt_worker_sha256(),
                receipt: synthetic_test_durable_receipt(
                    untrusted_record
                        .durable_completion
                        .clone()
                        .expect("record has durable completion"),
                ),
            },
            OperatorRunSummary {
                canonical_rows: 3,
                nt_catalog_rows: 3,
                catalog_hash: sealed_hash.clone(),
            },
            record,
            controls,
            execution_record_sha256,
        );

        assert_eq!(reconstructed.canonical_rows, 3);
        assert_eq!(reconstructed.nt_catalog_rows, 3);
        assert_eq!(reconstructed.catalog_hash, sealed_hash);
        assert_eq!(
            reconstructed.completion_provenance,
            SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated
        );
        assert_eq!(
            reconstructed.attempt_worker_sha256,
            synthetic_test_current_attempt_worker_sha256()
        );
        assert_eq!(
            reconstructed.terminal_publisher_worker_sha256,
            synthetic_test_worker_executable_sha256()
        );
        assert_ne!(
            reconstructed.canonical_rows,
            untrusted_record.canonical_rows
        );
        assert_ne!(reconstructed.catalog_hash, untrusted_record.catalog_hash);
    }

    #[test]
    fn final_assembly_consumes_a_sealed_discovery_summary_without_more_io() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());
        let pack_record = &owned_plan.pack.records[0];
        let controls = owned_plan
            .verified_control_artifacts
            .get(&pack_record.sequence)
            .expect("controls");
        let execution_record_sha256 = owned_plan
            .execution_record_sha256s
            .get(&pack_record.sequence)
            .expect("record fingerprint");
        let slot = RecordSlot::Discovered(reconstructed_discovered_record(
            SourceUniverseDiscoveredRunReceipt {
                attempt_worker_sha256: synthetic_test_current_attempt_worker_sha256(),
                receipt: synthetic_test_durable_receipt(
                    record
                        .durable_completion
                        .clone()
                        .expect("discovered record has durable completion"),
                ),
            },
            OperatorRunSummary {
                canonical_rows: 3,
                nt_catalog_rows: 3,
                catalog_hash: crate::hashing::sha256_hex(b"sealed catalog"),
            },
            pack_record,
            controls,
            execution_record_sha256,
        ));
        let report = assemble_report("batch", &owned_plan, vec![Some(slot)])
            .expect("sealed result assembly performs no post-terminal I/O");

        assert_eq!(
            report.status,
            SourceUniverseBatchExecutionReportStatus::Completed
        );
        assert_eq!(report.records.len(), 1);
        assert!(report.failures.is_empty());
        assert_eq!(
            report.records[0].completion_provenance,
            SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated
        );
        assert_eq!(
            report.records[0].attempt_worker_sha256,
            synthetic_test_current_attempt_worker_sha256()
        );
        assert_eq!(
            report.records[0].terminal_publisher_worker_sha256,
            synthetic_test_worker_executable_sha256()
        );
    }

    #[test]
    fn report_validator_rejects_executed_record_without_attempt_worker_hash() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());
        let mut report = assemble_report(
            "batch",
            &owned_plan,
            vec![Some(RecordSlot::Discovered(record))],
        )
        .expect("assemble valid discovered report");
        report.records[0].attempt_worker_sha256.clear();

        let error = validate_source_universe_batch_execution_report(&report)
            .expect_err("executed provenance without a worker executable hash must fail closed");

        assert!(
            error.to_string().contains("attempt_worker_sha256"),
            "{error:#}"
        );
    }

    #[test]
    fn report_validator_rejects_executed_record_with_invalid_worker_executable_hash() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());
        let mut report = assemble_report(
            "batch",
            &owned_plan,
            vec![Some(RecordSlot::Discovered(record))],
        )
        .expect("assemble valid discovered report");
        report.records[0].completion_provenance =
            SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated;
        report.records[0].terminal_publisher_worker_sha256 = "not-a-sha256".to_string();

        let error = validate_source_universe_batch_execution_report(&report)
            .expect_err("executed provenance with an invalid worker hash must fail closed");

        assert!(
            error
                .to_string()
                .contains("terminal_publisher_worker_sha256"),
            "{error:#}"
        );
    }

    #[test]
    fn report_validator_accepts_recovered_original_worker_executable_hash() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());
        let report = assemble_report(
            "batch",
            &owned_plan,
            vec![Some(RecordSlot::Discovered(record))],
        )
        .expect("assemble valid discovered report");
        validate_source_universe_batch_execution_report(&report)
            .expect("recovered report retains the original process-isolated worker hash");
    }

    #[test]
    fn final_assembly_rejects_a_missing_slot_unconditionally() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());
        let error = assemble_report("batch", &owned_plan, vec![None])
            .expect_err("a missing slot is an invariant failure, not a record failure");
        assert!(error.to_string().contains("slot 0 is missing"), "{error:#}");
    }

    #[test]
    fn final_assembly_rejects_slot_vector_cardinality_drift() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let record = discovered_record("f".repeat(64));
        let owned_plan = owned_plan_for_record(&record, temp_dir.path());
        let error = assemble_report("batch", &owned_plan, Vec::new())
            .expect_err("slot cardinality drift must fail before report construction");
        assert!(
            error.to_string().contains("slot cardinality mismatch"),
            "{error:#}"
        );
    }

    #[test]
    fn http_source_url_rejects_credentials_without_disclosing_them() {
        for source_url in [
            "https://operator:credential-value@example.invalid/object.csv.gz",
            "https://operator@example.invalid/object.csv.gz",
            "https://example.invalid/object.csv.gz?token=credential-value",
            "https://operator:credential-value@",
        ] {
            let error = validated_http_source_url(source_url)
                .expect_err("source URLs carrying credentials must fail closed");
            let rendered = format!("{error:#}");
            assert!(
                !rendered.contains("credential-value") && !rendered.contains("operator"),
                "source URL validation errors must not disclose URL credentials: {rendered}"
            );
        }
    }

    #[test]
    fn http_transport_error_formatting_strips_attached_urls_and_credentials() {
        let sensitive_url = reqwest::Url::parse(
            "https://operator:credential-value@example.invalid/object.csv.gz?token=credential-value",
        )
        .expect("parse sensitive test URL");
        let request_error = reqwest::Client::new()
            .get("not-a-valid-absolute-url")
            .build()
            .expect_err("a relative request target must produce a reqwest error")
            .with_url(sensitive_url.clone());
        assert_eq!(request_error.url(), Some(&sensitive_url));

        let rendered = format!(
            "{:#}",
            source_object_http_error(request_error, "execute source-object GET request")
        );
        for sensitive_component in [
            "operator",
            "credential-value",
            "example.invalid",
            "object.csv.gz",
            "token",
        ] {
            assert!(
                !rendered.contains(sensitive_component),
                "HTTP transport error disclosed {sensitive_component:?}: {rendered}"
            );
        }
    }
}

fn validated_http_source_url(source_url: &str) -> Result<reqwest::Url> {
    let parsed_url =
        reqwest::Url::parse(source_url).context("parse source_url for batch execution")?;
    ensure!(
        parsed_url.scheme() == "https",
        "source_url must be HTTPS for batch execution"
    );
    ensure!(
        parsed_url.username().is_empty() && parsed_url.password().is_none(),
        "source_url must not include credentials"
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
        "source_url missing object path"
    );
    ensure!(
        parsed_url.query().is_none() && parsed_url.fragment().is_none(),
        "source_url query and fragment components are not supported"
    );
    Ok(parsed_url)
}
