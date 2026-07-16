//! Shared cooperative work-budget enforcement for operator execution.

use std::{
    cmp::Ordering as CmpOrdering,
    fs,
    future::Future,
    io::{BufReader, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    backfill_execution_plan::{BackfillExecutionPlan, BackfillExecutionWorkBudget},
    pinned_regular_file::{PinnedRegularFileIdentity, open_pinned_regular_file},
};

/// Auditable cooperative checkpoint vocabulary shared by every operator path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorWorkBudgetStage {
    Fetch,
    ObjectVerification,
    Decode,
    Normalize,
    CatalogProjection,
    CanonicalWrite,
    Backtest,
    Finalize,
    Publish,
}

/// Opaque one-use authorization for one completion-boundary commit.
pub struct OperatorWorkBudgetCommitPermit {
    stage: OperatorWorkBudgetStage,
    clock: Arc<dyn OperatorWorkBudgetClock>,
    started_at: Duration,
    deadline: Option<Duration>,
}

impl std::fmt::Debug for OperatorWorkBudgetCommitPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperatorWorkBudgetCommitPermit")
            .field("stage", &self.stage)
            .field("started_at", &self.started_at)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl OperatorWorkBudgetCommitPermit {
    /// Stage authorized by the guard. The permit itself is intentionally not
    /// cloneable and must be consumed by the commit operation.
    pub const fn stage(&self) -> OperatorWorkBudgetStage {
        self.stage
    }

    /// Consume the permit and derive the remaining duration from its original
    /// absolute deadline. Delaying consumption can only shorten this boundary.
    pub(crate) fn remaining_wall_time_at_consumption(self) -> Result<Option<Duration>> {
        let Some(deadline) = self.deadline else {
            return Ok(None);
        };
        let now = self.clock.now();
        let actual = now.checked_sub(self.started_at).ok_or_else(|| {
            anyhow!(
                "monotonic work-budget clock regressed while consuming commit permit at stage {}: start {:?}, now {now:?}",
                self.stage,
                self.started_at
            )
        })?;
        ensure!(
            now < deadline,
            "work-budget commit permit reached or exceeded its original wall deadline after {actual:?} at stage {}",
            self.stage
        );
        Ok(Some(deadline.checked_sub(now).expect(
            "deadline check guarantees positive permit duration",
        )))
    }
}

impl std::fmt::Display for OperatorWorkBudgetStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Fetch => "fetch",
            Self::ObjectVerification => "object_verification",
            Self::Decode => "decode",
            Self::Normalize => "normalize",
            Self::CatalogProjection => "catalog_projection",
            Self::CanonicalWrite => "canonical_write",
            Self::Backtest => "backtest",
            Self::Finalize => "finalize",
            Self::Publish => "publish",
        };
        formatter.write_str(label)
    }
}

/// Explicit guard mode for every operator entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorWorkBudget {
    /// Planless research and low-level callers traverse the same guarded core
    /// without a validated backfill plan imposing limits.
    Unbounded,
    /// Validated backfill callers enforce every configured dimension.
    Backfill(BackfillExecutionWorkBudget),
}

impl OperatorWorkBudget {
    /// Copy the limits from an already validated execution plan.
    pub fn from_execution_plan(plan: &BackfillExecutionPlan) -> Self {
        Self::Backfill(BackfillExecutionWorkBudget {
            max_decoded_bytes: plan.max_decoded_bytes,
            max_source_rows: plan.max_source_rows,
            max_projected_row_groups: plan.max_projected_row_groups,
            max_wall_seconds: plan.max_wall_seconds,
            require_object_selection_metadata: plan.require_object_selection_metadata,
        })
    }
}

/// Monotonic time source used by cooperative wall-budget checkpoints.
pub trait OperatorWorkBudgetClock: Send + Sync {
    /// Duration from an arbitrary stable monotonic epoch.
    fn now(&self) -> Duration;
}

#[derive(Debug, Default)]
struct SystemOperatorWorkBudgetClock;

impl OperatorWorkBudgetClock for SystemOperatorWorkBudgetClock {
    fn now(&self) -> Duration {
        let mut timestamp = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `timestamp` is valid writable storage and CLOCK_MONOTONIC is
        // a process-independent epoch supplied by the kernel. This exact epoch
        // is therefore safe to seal into a child request.
        let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timestamp) };
        if result != 0 || timestamp.tv_sec < 0 || !(0..1_000_000_000).contains(&timestamp.tv_nsec) {
            // The trait is intentionally infallible for injected test clocks.
            // A production clock failure returns the maximal instant so every
            // finite guard fails closed instead of resetting or extending time.
            return Duration::MAX;
        }
        Duration::new(
            u64::try_from(timestamp.tv_sec).unwrap_or(u64::MAX),
            u32::try_from(timestamp.tv_nsec).unwrap_or(u32::MAX),
        )
    }
}

/// Return the single production monotonic clock implementation shared by all
/// guarded operator entry points and clock-only test seams.
pub(crate) fn system_operator_work_budget_clock() -> Arc<dyn OperatorWorkBudgetClock> {
    Arc::new(SystemOperatorWorkBudgetClock)
}

/// Cross-process identity of one finite cooperative wall-time interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorWorkBudgetDeadline {
    pub started_at_seconds: u64,
    pub started_at_nanoseconds: u32,
    pub deadline_seconds: u64,
    pub deadline_nanoseconds: u32,
}

impl OperatorWorkBudgetDeadline {
    fn started_at(self) -> Result<Duration> {
        ensure!(
            self.started_at_nanoseconds < 1_000_000_000,
            "work-budget started_at nanoseconds must be below one second"
        );
        Ok(Duration::new(
            self.started_at_seconds,
            self.started_at_nanoseconds,
        ))
    }

    fn deadline(self) -> Result<Duration> {
        ensure!(
            self.deadline_nanoseconds < 1_000_000_000,
            "work-budget deadline nanoseconds must be below one second"
        );
        Ok(Duration::new(
            self.deadline_seconds,
            self.deadline_nanoseconds,
        ))
    }

    /// Validate the intrinsic monotonic interval shape.
    pub fn validate(self) -> Result<()> {
        let started_at = self.started_at()?;
        let deadline = self.deadline()?;
        ensure!(
            deadline > started_at,
            "sealed work-budget deadline must be later than started_at"
        );
        Ok(())
    }

    /// Validate the sealed interval against the execution plan which owns it.
    pub fn validate_for_max_wall_seconds(self, max_wall_seconds: u64) -> Result<()> {
        self.validate()?;
        let started_at = self.started_at()?;
        let deadline = self.deadline()?;
        let expected_deadline = started_at
            .checked_add(Duration::from_secs(max_wall_seconds))
            .context("sealed work-budget deadline overflows monotonic duration")?;
        ensure!(
            deadline == expected_deadline,
            "sealed work-budget deadline does not equal started_at + max_wall_seconds"
        );
        Ok(())
    }
}

struct OperatorWorkBudgetGuardInner {
    budget: OperatorWorkBudget,
    clock: Arc<dyn OperatorWorkBudgetClock>,
    started_at: Duration,
    deadline: Option<Duration>,
    source_rows_consumed: AtomicU64,
}

/// Cloneable guard shared by fetch, normalization, projection, execution, and
/// publication. Clones share one source-row counter and one deadline.
#[derive(Clone)]
pub struct OperatorWorkBudgetGuard {
    inner: Arc<OperatorWorkBudgetGuardInner>,
}

impl std::fmt::Debug for OperatorWorkBudgetGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperatorWorkBudgetGuard")
            .field("budget", &self.inner.budget)
            .field("deadline", &self.inner.deadline)
            .field("source_rows_consumed", &self.source_rows_consumed())
            .finish_non_exhaustive()
    }
}

impl OperatorWorkBudgetGuard {
    /// Build the explicit guard used by planless research and low-level wrappers.
    pub fn unbounded() -> Self {
        Self::new(OperatorWorkBudget::Unbounded)
            .expect("an unbounded work budget cannot overflow its deadline")
    }

    /// Build a guard using the production monotonic clock.
    pub fn new(budget: OperatorWorkBudget) -> Result<Self> {
        Self::with_clock(budget, system_operator_work_budget_clock())
    }

    /// Build the guard for a validated batch record. The input type and this
    /// constructor deliberately exclude [`OperatorWorkBudget::Unbounded`]; a
    /// caller may inject only the monotonic clock, never the budget mode.
    pub fn from_execution_plan_with_clock(
        plan: &BackfillExecutionPlan,
        clock: Arc<dyn OperatorWorkBudgetClock>,
    ) -> Result<Self> {
        Self::with_clock(OperatorWorkBudget::from_execution_plan(plan), clock)
    }

    /// Reconstruct a child guard from the exact cross-process interval sealed
    /// by its parent. Startup latency consumes this interval; it never creates
    /// a fresh deadline.
    pub fn from_execution_plan_with_absolute_deadline(
        plan: &BackfillExecutionPlan,
        interval: OperatorWorkBudgetDeadline,
    ) -> Result<Self> {
        Self::from_execution_plan_with_absolute_deadline_and_clock(
            plan,
            interval,
            system_operator_work_budget_clock(),
        )
    }

    /// Test seam for reconstructing an exact parent interval with an injected
    /// clock which shares the same monotonic epoch.
    pub fn from_execution_plan_with_absolute_deadline_and_clock(
        plan: &BackfillExecutionPlan,
        interval: OperatorWorkBudgetDeadline,
        clock: Arc<dyn OperatorWorkBudgetClock>,
    ) -> Result<Self> {
        Self::with_absolute_deadline_and_clock(
            OperatorWorkBudget::from_execution_plan(plan),
            plan.max_wall_seconds,
            interval,
            clock,
        )
    }

    fn with_absolute_deadline_and_clock(
        budget: OperatorWorkBudget,
        max_wall_seconds: u64,
        interval: OperatorWorkBudgetDeadline,
        clock: Arc<dyn OperatorWorkBudgetClock>,
    ) -> Result<Self> {
        interval.validate_for_max_wall_seconds(max_wall_seconds)?;
        let started_at = interval.started_at()?;
        let deadline = interval.deadline()?;
        let now = clock.now();
        ensure!(
            now >= started_at,
            "child monotonic clock regressed before sealed work-budget start"
        );
        ensure!(
            now < deadline,
            "child startup reached or exceeded the sealed work-budget deadline"
        );
        Self::with_clock_and_bounds(budget, clock, started_at, Some(deadline))
    }

    /// Build a guard with an injected monotonic clock.
    pub fn with_clock(
        budget: OperatorWorkBudget,
        clock: Arc<dyn OperatorWorkBudgetClock>,
    ) -> Result<Self> {
        let started_at = clock.now();
        let deadline = match budget {
            OperatorWorkBudget::Unbounded => None,
            OperatorWorkBudget::Backfill(work_budget) => {
                let wall = Duration::from_secs(work_budget.max_wall_seconds);
                Some(started_at.checked_add(wall).ok_or_else(|| {
                    anyhow!(
                        "max_wall_seconds deadline overflow: start {started_at:?}, limit {}",
                        work_budget.max_wall_seconds
                    )
                })?)
            }
        };
        Self::with_clock_and_bounds(budget, clock, started_at, deadline)
    }

    fn with_clock_and_bounds(
        budget: OperatorWorkBudget,
        clock: Arc<dyn OperatorWorkBudgetClock>,
        started_at: Duration,
        deadline: Option<Duration>,
    ) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(OperatorWorkBudgetGuardInner {
                budget,
                clock,
                started_at,
                deadline,
                source_rows_consumed: AtomicU64::new(0),
            }),
        })
    }

    /// Check the cooperative wall deadline at a named execution stage.
    pub fn check_deadline(&self, stage: OperatorWorkBudgetStage) -> Result<()> {
        self.check_deadline_at(stage, self.inner.clock.now())
    }

    fn check_deadline_at(&self, stage: OperatorWorkBudgetStage, now: Duration) -> Result<()> {
        let Some(deadline) = self.inner.deadline else {
            return Ok(());
        };
        let actual = now.checked_sub(self.inner.started_at).ok_or_else(|| {
            anyhow!(
                "monotonic work-budget clock regressed at stage {stage}: start {:?}, now {now:?}",
                self.inner.started_at
            )
        })?;
        if now >= deadline {
            let OperatorWorkBudget::Backfill(work_budget) = self.inner.budget else {
                unreachable!("deadline exists only for a backfill work budget");
            };
            bail!(
                "max_wall_seconds actual {:?} reached or exceeds limit {}s at stage {stage}",
                actual,
                work_budget.max_wall_seconds
            );
        }
        Ok(())
    }

    /// Return the remaining wall time for bounding an external request.
    pub fn remaining_wall_time(&self, stage: OperatorWorkBudgetStage) -> Result<Option<Duration>> {
        let now = self.inner.clock.now();
        self.check_deadline_at(stage, now)?;
        Ok(self.inner.deadline.map(|deadline| {
            deadline
                .checked_sub(now)
                .expect("deadline check guarantees strictly positive remaining wall time")
        }))
    }

    /// Return the configured total decoded-byte ceiling for backfill work.
    #[must_use]
    pub fn decoded_byte_limit(&self) -> Option<u64> {
        match self.inner.budget {
            OperatorWorkBudget::Unbounded => None,
            OperatorWorkBudget::Backfill(work_budget) => Some(work_budget.max_decoded_bytes),
        }
    }

    /// Return the configured total source-row ceiling for backfill work.
    #[must_use]
    pub fn source_row_limit(&self) -> Option<u64> {
        match self.inner.budget {
            OperatorWorkBudget::Unbounded => None,
            OperatorWorkBudget::Backfill(work_budget) => Some(work_budget.max_source_rows),
        }
    }

    /// Authorize exactly one local rename or remote conditional PUT at the
    /// sampled instant. The returned non-cloneable permit is consumed by that
    /// commit, so no later clock read can retroactively invalidate it.
    pub fn authorize_commit(
        &self,
        stage: OperatorWorkBudgetStage,
    ) -> Result<OperatorWorkBudgetCommitPermit> {
        let now = self.inner.clock.now();
        self.check_deadline_at(stage, now)?;
        Ok(OperatorWorkBudgetCommitPermit {
            stage,
            clock: self.inner.clock.clone(),
            started_at: self.inner.started_at,
            deadline: self.inner.deadline,
        })
    }

    /// Meter one source record before filtering, deduplication, or expansion.
    pub fn consume_source_row(&self, stage: OperatorWorkBudgetStage) -> Result<()> {
        self.check_deadline(stage)?;
        let OperatorWorkBudget::Backfill(work_budget) = self.inner.budget else {
            return Ok(());
        };
        let actual = self
            .inner
            .source_rows_consumed
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|actual| {
                anyhow!(
                    "max_source_rows actual overflow after {actual} rows at stage {stage}; limit {}",
                    work_budget.max_source_rows
                )
            })?;
        ensure!(
            actual <= work_budget.max_source_rows,
            "max_source_rows actual {actual} exceeds limit {} at stage {stage}",
            work_budget.max_source_rows
        );
        Ok(())
    }

    /// Number of source records consumed by the shared guarded core.
    pub fn source_rows_consumed(&self) -> u64 {
        self.inner.source_rows_consumed.load(Ordering::Relaxed)
    }

    /// Verify a decoded/uncompressed byte total without allocating from the
    /// ceiling or mutating a consumption counter.
    pub fn verify_decoded_bytes(&self, actual: u64, stage: OperatorWorkBudgetStage) -> Result<()> {
        self.check_deadline(stage)?;
        let OperatorWorkBudget::Backfill(work_budget) = self.inner.budget else {
            return Ok(());
        };
        ensure!(
            actual <= work_budget.max_decoded_bytes,
            "max_decoded_bytes actual {actual} exceeds limit {} at stage {stage}",
            work_budget.max_decoded_bytes
        );
        Ok(())
    }

    /// Verify a known source-row total without consuming the streaming row
    /// counter. Used by metadata preflights before row decoding begins.
    pub fn verify_source_rows(&self, actual: u64, stage: OperatorWorkBudgetStage) -> Result<()> {
        self.check_deadline(stage)?;
        let OperatorWorkBudget::Backfill(work_budget) = self.inner.budget else {
            return Ok(());
        };
        ensure!(
            actual <= work_budget.max_source_rows,
            "max_source_rows actual {actual} exceeds limit {} at stage {stage}",
            work_budget.max_source_rows
        );
        Ok(())
    }

    /// Enforce the pre-write projected row-group count.
    pub fn check_projected_row_groups(
        &self,
        actual: u64,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        self.check_deadline(stage)?;
        let OperatorWorkBudget::Backfill(work_budget) = self.inner.budget else {
            return Ok(());
        };
        ensure!(
            actual <= work_budget.max_projected_row_groups,
            "max_projected_row_groups actual {actual} exceeds limit {} at stage {stage}",
            work_budget.max_projected_row_groups
        );
        Ok(())
    }

    /// Re-enforce the same dimension using actual Parquet metadata.
    pub fn verify_actual_row_groups(
        &self,
        actual: u64,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        self.check_projected_row_groups(actual, stage)
    }
}

/// Reader adapter that observes the shared wall deadline on both sides of each
/// read operation.
pub struct CooperativeDeadlineReader<'a, R> {
    inner: R,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

impl<'a, R> CooperativeDeadlineReader<'a, R> {
    /// Wrap one reader at a named work-budget stage.
    pub const fn new(
        inner: R,
        work_budget: &'a OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Self {
        Self {
            inner,
            work_budget,
            stage,
        }
    }
}

impl<R: Read> Read for CooperativeDeadlineReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        let read_outcome = self.inner.read(buffer);
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        read_outcome
    }
}

/// Writer adapter that observes the shared wall deadline on both sides of each
/// write operation.
pub struct CooperativeDeadlineWriter<'a, W> {
    inner: W,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

impl<'a, W> CooperativeDeadlineWriter<'a, W> {
    /// Wrap one writer at a named work-budget stage.
    pub const fn new(
        inner: W,
        work_budget: &'a OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Self {
        Self {
            inner,
            work_budget,
            stage,
        }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CooperativeDeadlineWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        let write_outcome = self.inner.write(buffer);
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        write_outcome
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        let flush_outcome = self.inner.flush();
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        flush_outcome
    }
}

/// Deserialize exactly one JSON value through deadline-observed reads.
pub fn deserialize_json_with_budget<T: DeserializeOwned>(
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<T> {
    let reader = CooperativeDeadlineReader::new(Cursor::new(bytes), work_budget, stage);
    let outcome = serde_json::from_reader(BufReader::new(reader)).map_err(anyhow::Error::from);
    work_budget.check_deadline(stage)?;
    outcome
}

struct FallibleBudgetVecWriter<'a> {
    bytes: Vec<u8>,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

impl<'a> FallibleBudgetVecWriter<'a> {
    fn new(work_budget: &'a OperatorWorkBudgetGuard, stage: OperatorWorkBudgetStage) -> Self {
        Self {
            bytes: Vec::new(),
            work_budget,
            stage,
        }
    }

    fn finish(self) -> Result<Vec<u8>> {
        self.work_budget.check_deadline(self.stage)?;
        Ok(self.bytes)
    }
}

impl Write for FallibleBudgetVecWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        let next_len = self
            .bytes
            .len()
            .checked_add(buffer.len())
            .ok_or_else(|| std::io::Error::other("JSON output length overflow"))?;
        self.work_budget
            .verify_decoded_bytes(
                u64::try_from(next_len)
                    .map_err(|_| std::io::Error::other("JSON output length does not fit u64"))?,
                self.stage,
            )
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        self.bytes
            .try_reserve_exact(buffer.len())
            .map_err(|error| std::io::Error::other(format!("reserve JSON output: {error}")))?;
        self.bytes.extend_from_slice(buffer);
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))
    }
}

struct Sha256Writer<'a> {
    hasher: Sha256,
    bytes_written: u64,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

impl Write for Sha256Writer<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes_written = self
            .bytes_written
            .checked_add(
                u64::try_from(buffer.len())
                    .map_err(|_| std::io::Error::other("JSON hash length does not fit u64"))?,
            )
            .ok_or_else(|| std::io::Error::other("JSON hash length overflow"))?;
        self.work_budget
            .verify_decoded_bytes(self.bytes_written, self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        self.hasher.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sha256_digest_hex_fallible(digest: impl AsRef<[u8]>) -> Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = digest.as_ref();
    let capacity = bytes
        .len()
        .checked_mul(2)
        .context("SHA-256 hex capacity overflow")?;
    let mut output = String::new();
    output
        .try_reserve_exact(capacity)
        .context("reserve SHA-256 hex output")?;
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

struct BudgetJsonLengthWriter<'a> {
    bytes_written: u64,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

impl Write for BudgetJsonLengthWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        self.bytes_written = self
            .bytes_written
            .checked_add(
                u64::try_from(buffer.len())
                    .map_err(|_| std::io::Error::other("JSON length does not fit u64"))?,
            )
            .ok_or_else(|| std::io::Error::other("JSON serialized length overflow"))?;
        self.work_budget
            .verify_decoded_bytes(self.bytes_written, self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))
    }
}

/// Measure canonical struct JSON under the same deadline and decoded-memory
/// ceiling used for allocation, without materializing the document.
pub fn serialized_json_len_with_budget<T: Serialize>(
    value: &T,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<u64> {
    work_budget.check_deadline(stage)?;
    let mut writer = BudgetJsonLengthWriter {
        bytes_written: 0,
        work_budget,
        stage,
    };
    serde_json::to_writer(&mut writer, value).context("measure guarded canonical JSON")?;
    writer.flush().context("flush guarded JSON length writer")?;
    work_budget.check_deadline(stage)?;
    Ok(writer.bytes_written)
}

/// Serialize canonical struct JSON with fallible growth and work-budget bounds.
pub fn serialize_json_to_vec_guarded<T: Serialize>(
    value: &T,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<Vec<u8>> {
    work_budget.check_deadline(stage)?;
    let mut writer = FallibleBudgetVecWriter::new(work_budget, stage);
    serde_json::to_writer(&mut writer, value).context("serialize guarded canonical JSON")?;
    writer.finish()
}

/// Hash canonical struct JSON without materializing the serialized payload.
pub fn sha256_json_guarded<T: Serialize>(
    value: &T,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    work_budget.check_deadline(stage)?;
    let hash_writer = Sha256Writer {
        hasher: Sha256::new(),
        bytes_written: 0,
        work_budget,
        stage,
    };
    let mut writer = CooperativeDeadlineWriter::new(hash_writer, work_budget, stage);
    serde_json::to_writer(&mut writer, value).context("hash guarded canonical JSON")?;
    writer
        .flush()
        .context("flush guarded canonical JSON hash")?;
    work_budget.check_deadline(stage)?;
    let output = sha256_digest_hex_fallible(writer.into_inner().hasher.finalize())?;
    work_budget.check_deadline(stage)?;
    Ok(output)
}

/// Deserialize a whitespace-separated stream of top-level JSON values once.
pub fn deserialize_json_stream_with_budget<T: DeserializeOwned>(
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<Vec<T>> {
    let reader = CooperativeDeadlineReader::new(Cursor::new(bytes), work_budget, stage);
    let stream = serde_json::Deserializer::from_reader(BufReader::new(reader)).into_iter::<T>();
    let mut values = Vec::new();
    for value in stream {
        let outcome = value.map_err(anyhow::Error::from);
        work_budget.check_deadline(stage)?;
        values.push(outcome?);
    }
    work_budget.check_deadline(stage)?;
    Ok(values)
}

/// Hash one code-owned in-memory payload with deadline observations around the
/// hash operation.
pub fn sha256_hex_with_budget(
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    let mut hasher = Sha256::new();
    update_sha256_with_budget(&mut hasher, bytes, work_budget, stage)?;
    let output = sha256_digest_hex_fallible(hasher.finalize())?;
    work_budget.check_deadline(stage)?;
    Ok(output)
}

/// Feed one code-owned byte slice into an incremental SHA-256 state.
pub fn update_sha256_with_budget(
    hasher: &mut Sha256,
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    work_budget.check_deadline(stage)?;
    hasher.update(bytes);
    work_budget.check_deadline(stage)
}

/// Read one code-owned local file with deadline checks around each read.
pub fn read_file_with_budget(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<Vec<u8>> {
    work_budget.check_deadline(stage)?;
    let (mut file, identity) = open_pinned_regular_file(path)?;
    let expected_bytes = identity.byte_len;
    work_budget.verify_decoded_bytes(expected_bytes, stage)?;
    identity.revalidate(path, &file)?;
    let bytes = read_exact_sized_open_file_ref_guarded(
        &mut file,
        path,
        expected_bytes,
        work_budget,
        stage,
    )?;
    identity.revalidate(path, &file)?;
    work_budget.check_deadline(stage)?;
    Ok(bytes)
}

/// Incremental exact-size ingress shared by HTTP, cache, local-file, and
/// object-store readers. The declared size is a rejection boundary, never a
/// preallocation request; every growth is fallible and at most one sentinel
/// byte beyond the pin is observed before an oversize error.
pub(crate) struct ExactSizedObjectBuffer {
    expected_bytes: u64,
    sentinel_limit: u64,
    bytes: Vec<u8>,
}

impl ExactSizedObjectBuffer {
    pub(crate) fn new(expected_bytes: u64) -> Result<Self> {
        let sentinel_limit = expected_bytes
            .checked_add(1)
            .context("exact-size ingress expected byte count is too large")?;
        Ok(Self {
            expected_bytes,
            sentinel_limit,
            bytes: Vec::new(),
        })
    }

    pub(crate) fn push(
        &mut self,
        chunk: &[u8],
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        work_budget.check_deadline(stage)?;
        let current = u64::try_from(self.bytes.len())
            .context("exact-size ingress buffered length does not fit u64")?;
        let chunk_bytes = u64::try_from(chunk.len())
            .context("exact-size ingress chunk length does not fit u64")?;
        let remaining_to_sentinel = self
            .sentinel_limit
            .checked_sub(current)
            .context("exact-size ingress sentinel accounting underflow")?;
        let retained = usize::try_from(remaining_to_sentinel.min(chunk_bytes))
            .context("exact-size ingress retained chunk length does not fit usize")?;
        self.bytes
            .try_reserve_exact(retained)
            .context("reserve exact-size ingress bytes")?;
        self.bytes.extend_from_slice(&chunk[..retained]);
        let observed = u64::try_from(self.bytes.len())
            .context("exact-size ingress observed length does not fit u64")?;
        ensure!(
            observed <= self.expected_bytes && retained == chunk.len(),
            "object byte length exceeds pinned expected size {} (observed at least {observed})",
            self.expected_bytes
        );
        work_budget.check_deadline(stage)
    }

    pub(crate) fn finish(
        self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Vec<u8>> {
        work_budget.check_deadline(stage)?;
        let actual = u64::try_from(self.bytes.len())
            .context("exact-size ingress final length does not fit u64")?;
        ensure!(
            actual == self.expected_bytes,
            "object byte length {actual} does not match pinned expected size {}",
            self.expected_bytes
        );
        Ok(self.bytes)
    }
}

fn read_exact_sized_open_file_ref_guarded(
    file: &mut fs::File,
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<Vec<u8>> {
    let metadata = file
        .metadata()
        .with_context(|| format!("stat opened object {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "opened object is not a regular file: {}",
        path.display()
    );
    let metadata_bytes = metadata.len();
    ensure!(
        metadata_bytes == expected_bytes,
        "object byte length {metadata_bytes} does not match pinned expected size {expected_bytes}"
    );
    let sentinel_bytes = expected_bytes
        .checked_add(1)
        .context("exact-size file ingress sentinel length overflow")?;
    let scratch_len = usize::try_from(sentinel_bytes)
        .context("exact-size file ingress scratch length does not fit usize")?;
    let mut scratch = Vec::new();
    scratch
        .try_reserve_exact(scratch_len)
        .context("reserve exact-size file ingress scratch buffer")?;
    scratch.resize(scratch_len, 0);
    let mut output = ExactSizedObjectBuffer::new(expected_bytes)?;
    loop {
        work_budget.check_deadline(stage)?;
        let read = file
            .read(&mut scratch)
            .with_context(|| format!("read opened object {}", path.display()))?;
        if read == 0 {
            break;
        }
        output.push(&scratch[..read], work_budget, stage)?;
    }
    output.finish(work_budget, stage)
}

/// Read a local object through one opened handle, binding metadata and bytes
/// to the same inode/handle and rejecting truncation or growth.
pub fn read_exact_sized_file_guarded(
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<Vec<u8>> {
    let (bytes, _identity) =
        read_exact_sized_pinned_file_guarded(path, expected_bytes, work_budget, stage)?;
    Ok(bytes)
}

/// Read one exact-size local object and return the full pinned identity from
/// that same non-hashing traversal.
pub(crate) fn read_exact_sized_pinned_file_guarded(
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(Vec<u8>, PinnedRegularFileIdentity)> {
    work_budget.check_deadline(stage)?;
    work_budget.verify_decoded_bytes(expected_bytes, stage)?;
    let (mut file, identity) = open_pinned_regular_file(path)?;
    ensure!(
        identity.byte_len == expected_bytes,
        "object byte length {} does not match pinned expected size {expected_bytes}",
        identity.byte_len
    );
    identity.revalidate(path, &file)?;
    let bytes = read_exact_sized_open_file_ref_guarded(
        &mut file,
        path,
        expected_bytes,
        work_budget,
        stage,
    )?;
    identity.revalidate(path, &file)?;
    work_budget.check_deadline(stage)?;
    Ok((bytes, identity))
}

/// Hash one exact-size regular file through a no-follow handle without ever
/// materializing the complete payload in memory.
pub fn sha256_exact_sized_file_guarded(
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    work_budget.check_deadline(stage)?;
    let (mut file, identity) = open_pinned_regular_file(path)?;
    ensure!(
        identity.byte_len == expected_bytes,
        "object byte length {} does not match pinned expected size {expected_bytes}",
        identity.byte_len
    );
    identity.revalidate(path, &file)?;
    let sha256 =
        sha256_exact_sized_open_file_guarded(&mut file, path, expected_bytes, work_budget, stage)?;
    identity.revalidate(path, &file)?;
    work_budget.check_deadline(stage)?;
    Ok(sha256)
}

/// Authoritatively read one local object while binding its expected length and
/// SHA-256 to one fd-relative capability and one byte traversal.
pub fn read_exact_sized_hashed_file_guarded(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<Vec<u8>> {
    let (bytes, _identity) = read_exact_sized_hashed_pinned_file_guarded(
        path,
        expected_bytes,
        expected_sha256,
        work_budget,
        stage,
    )?;
    Ok(bytes)
}

/// Authoritatively read one local object and return the exact pinned identity
/// used for the single length/hash/byte traversal.
pub(crate) fn read_exact_sized_hashed_pinned_file_guarded(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(Vec<u8>, PinnedRegularFileIdentity)> {
    work_budget.check_deadline(stage)?;
    ensure!(
        expected_sha256.len() == 64
            && expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "expected SHA-256 must be exactly 64 lowercase hexadecimal characters"
    );
    work_budget.verify_decoded_bytes(expected_bytes, stage)?;
    let (mut file, identity) = open_pinned_regular_file(path)?;
    ensure!(
        identity.byte_len == expected_bytes,
        "object byte length {} does not match pinned expected size {expected_bytes}",
        identity.byte_len
    );
    identity.revalidate(path, &file)?;
    let (bytes, actual_sha256) = read_and_sha256_exact_sized_open_file_guarded(
        &mut file,
        path,
        expected_bytes,
        work_budget,
        stage,
    )?;
    identity.revalidate(path, &file)?;
    ensure!(
        actual_sha256 == expected_sha256,
        "object SHA-256 mismatch for {}: expected {expected_sha256}, got {actual_sha256}",
        path.display()
    );
    work_budget.check_deadline(stage)?;
    Ok((bytes, identity))
}

fn read_and_sha256_exact_sized_open_file_guarded(
    file: &mut fs::File,
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(Vec<u8>, String)> {
    let metadata = file
        .metadata()
        .with_context(|| format!("stat opened object {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() == expected_bytes,
        "object byte length {} does not match pinned expected size {expected_bytes}",
        metadata.len()
    );
    let sentinel_bytes = expected_bytes
        .checked_add(1)
        .context("exact-size hashed file ingress sentinel length overflow")?;
    let scratch_len = usize::try_from(sentinel_bytes)
        .context("exact-size hashed file ingress scratch length does not fit usize")?;
    let mut scratch = Vec::new();
    scratch
        .try_reserve_exact(scratch_len)
        .context("reserve exact-size hashed file ingress scratch buffer")?;
    scratch.resize(scratch_len, 0);
    let mut output = ExactSizedObjectBuffer::new(expected_bytes)?;
    let mut hasher = Sha256::new();
    loop {
        work_budget.check_deadline(stage)?;
        let read = file
            .read(&mut scratch)
            .with_context(|| format!("read and hash opened object {}", path.display()))?;
        if read == 0 {
            break;
        }
        output.push(&scratch[..read], work_budget, stage)?;
        hasher.update(&scratch[..read]);
        work_budget.check_deadline(stage)?;
    }
    let bytes = output.finish(work_budget, stage)?;
    work_budget.check_deadline(stage)?;
    let sha256 = sha256_digest_hex_fallible(hasher.finalize())?;
    work_budget.check_deadline(stage)?;
    Ok((bytes, sha256))
}

pub(crate) fn sha256_exact_sized_open_file_guarded(
    file: &mut fs::File,
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    let metadata = file
        .metadata()
        .with_context(|| format!("stat opened object {}", path.display()))?;
    ensure!(
        metadata.len() == expected_bytes,
        "object byte length {} does not match pinned expected size {expected_bytes}",
        metadata.len()
    );
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("seek opened object before hashing {}", path.display()))?;
    let scratch_len = usize::try_from(expected_bytes.max(1))
        .context("exact-size file hash scratch length does not fit usize")?;
    let mut scratch = Vec::new();
    scratch
        .try_reserve_exact(scratch_len)
        .context("reserve exact-size file hash scratch buffer")?;
    scratch.resize(scratch_len, 0);
    let mut observed_bytes = 0_u64;
    let mut hasher = Sha256::new();
    loop {
        work_budget.check_deadline(stage)?;
        let read = file
            .read(&mut scratch)
            .with_context(|| format!("hash opened object {}", path.display()))?;
        if read == 0 {
            break;
        }
        observed_bytes = observed_bytes
            .checked_add(u64::try_from(read).context("file hash read length does not fit u64")?)
            .context("file hash observed byte count overflow")?;
        ensure!(
            observed_bytes <= expected_bytes,
            "object byte length exceeds pinned expected size {expected_bytes} while hashing {}",
            path.display()
        );
        hasher.update(&scratch[..read]);
        work_budget.check_deadline(stage)?;
    }
    ensure!(
        observed_bytes == expected_bytes,
        "object byte length {observed_bytes} does not match pinned expected size {expected_bytes} while hashing {}",
        path.display()
    );
    let final_metadata = file
        .metadata()
        .with_context(|| format!("re-stat hashed object {}", path.display()))?;
    ensure!(
        final_metadata.file_type().is_file() && final_metadata.len() == expected_bytes,
        "opened object identity or length changed while hashing {}",
        path.display()
    );
    work_budget.check_deadline(stage)?;
    let sha256 = sha256_digest_hex_fallible(hasher.finalize())?;
    work_budget.check_deadline(stage)?;
    Ok(sha256)
}

/// Visit non-blank records using exactly [`str::lines`] line-number and CRLF
/// semantics, observing the deadline at each line and visitor boundary.
pub fn for_each_nonempty_text_record_with_budget(
    text: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    mut visit: impl FnMut(usize, &str) -> Result<()>,
) -> Result<()> {
    work_budget.check_deadline(stage)?;
    for (line_index, line) in text.lines().enumerate() {
        work_budget.check_deadline(stage)?;
        if !line.trim().is_empty() {
            let outcome = visit(line_index, line);
            work_budget.check_deadline(stage)?;
            outcome?;
        }
        work_budget.check_deadline(stage)?;
    }
    work_budget.check_deadline(stage)
}

fn stable_sort_position_is_less<T, F>(
    values: &[T],
    original_indices: &[usize],
    left: usize,
    right: usize,
    compare: &mut F,
) -> bool
where
    F: FnMut(&T, &T) -> CmpOrdering,
{
    compare(&values[left], &values[right])
        .then_with(|| original_indices[left].cmp(&original_indices[right]))
        .is_lt()
}

fn stable_sort_sift_down_by<T, F>(
    values: &mut [T],
    original_indices: &mut [usize],
    mut root: usize,
    end: usize,
    compare: &mut F,
    checkpoint: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()>
where
    F: FnMut(&T, &T) -> CmpOrdering,
{
    while let Some(left) = root.checked_mul(2).and_then(|value| value.checked_add(1)) {
        if left >= end {
            break;
        }
        let right = left + 1;
        let child = if right < end
            && stable_sort_position_is_less(values, original_indices, left, right, compare)
        {
            right
        } else {
            left
        };
        checkpoint(1)?;
        if !stable_sort_position_is_less(values, original_indices, root, child, compare) {
            break;
        }
        original_indices.swap(root, child);
        values.swap(root, child);
        root = child;
    }
    Ok(())
}

/// Stable in-place heap sort with one fallibly allocated original-index buffer and
/// deadline observations at natural heap-operation boundaries.
///
/// Stability is represented explicitly by the original index as the secondary
/// comparator. Values stay in their existing allocation and are swapped in lockstep
/// with the index heap, so peak live storage is the original value buffer
/// plus one `len`-sized metadata buffer—never the three value-sized buffers the
/// former merge implementation required. The comparator borrows values, so
/// dynamic keys such as `String` are never cloned into untracked heap storage.
/// The caller-owned value allocation is already live on entry; this function
/// separately accounts and bounds its one additional `len * size_of::<usize>()`
/// metadata allocation before reserving it.
pub fn cooperative_stable_sort_by<T, F>(
    values: &mut [T],
    mut compare: F,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()>
where
    F: FnMut(&T, &T) -> CmpOrdering,
{
    work_budget.check_deadline(stage)?;
    let len = values.len();
    if len < 2 {
        return Ok(());
    }
    let mut checkpoint = |_processed: usize| work_budget.check_deadline(stage);

    let item_bytes = std::mem::size_of::<usize>().max(1);
    let metadata_bytes = len
        .checked_mul(item_bytes)
        .context("stable-sort metadata byte count overflow")?;
    work_budget.verify_decoded_bytes(
        u64::try_from(metadata_bytes).context("stable-sort metadata bytes do not fit u64")?,
        stage,
    )?;
    let mut original_indices = Vec::new();
    original_indices
        .try_reserve_exact(len)
        .map_err(|error| anyhow::anyhow!("reserve stable-sort index buffer: {error}"))?;
    for original_index in 0..len {
        original_indices.push(original_index);
        checkpoint(1)?;
    }

    for root in (0..(len / 2)).rev() {
        stable_sort_sift_down_by(
            values,
            &mut original_indices,
            root,
            len,
            &mut compare,
            &mut checkpoint,
        )?;
        checkpoint(1)?;
    }
    for end in (1..len).rev() {
        original_indices.swap(0, end);
        values.swap(0, end);
        checkpoint(1)?;
        stable_sort_sift_down_by(
            values,
            &mut original_indices,
            0,
            end,
            &mut compare,
            &mut checkpoint,
        )?;
    }
    work_budget.check_deadline(stage)
}

/// Allocation-free-key adapter for [`cooperative_stable_sort_by`].
///
/// `K: Copy` prevents callers from materializing owned dynamic keys in the
/// metadata buffer. Call [`cooperative_stable_sort_by`] directly when ordering
/// is based on borrowed strings or other dynamic data.
pub fn cooperative_stable_sort_by_key<T, K, F>(
    values: &mut [T],
    mut key_of: F,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()>
where
    K: Ord + Copy,
    F: FnMut(&T) -> K,
{
    cooperative_stable_sort_by(
        values,
        |left, right| key_of(left).cmp(&key_of(right)),
        work_budget,
        stage,
    )
}

/// Run one non-terminal synchronous operation with an unconditional deadline
/// observation after it returns, including when the operation itself fails.
/// The deadline error takes precedence because an expired operation must never
/// be misclassified as an ordinary provider or parsing failure.
pub fn guarded_operation_outcome<T, E>(
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    operation: impl FnOnce() -> std::result::Result<T, E>,
) -> Result<std::result::Result<T, E>> {
    work_budget.check_deadline(stage)?;
    let outcome = operation();
    work_budget.check_deadline(stage)?;
    Ok(outcome)
}

/// Async counterpart of [`guarded_operation_outcome`] for non-terminal remote
/// and runtime operations.
pub async fn guarded_async_operation_outcome<T, E, F>(
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    operation: F,
) -> Result<std::result::Result<T, E>>
where
    F: Future<Output = std::result::Result<T, E>>,
{
    let remaining = work_budget.remaining_wall_time(stage)?;
    let outcome = match remaining {
        Some(remaining) => match tokio::time::timeout(remaining, operation).await {
            Ok(outcome) => outcome,
            Err(_) => {
                work_budget.check_deadline(stage)?;
                bail!(
                    "max_wall_seconds hard timeout exhausted after waiting {remaining:?} at stage {stage}"
                );
            }
        },
        None => operation.await,
    };
    work_budget.check_deadline(stage)?;
    Ok(outcome)
}

/// Observe the shared wall deadline while joining one blocking task without
/// detaching work after a timeout. Once the deadline elapses, the join is
/// reaped to quiescence before the deadline error is returned because dropping
/// a blocking [`tokio::task::JoinHandle`] does not cancel its thread.
///
/// This helper guarantees quiescence, not a hard upper bound on thread work.
/// Hard boundedness is process-owned because threads cannot be safely killed.
pub(crate) async fn guarded_blocking_join_outcome<T>(
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    mut task: tokio::task::JoinHandle<T>,
) -> Result<std::result::Result<T, tokio::task::JoinError>> {
    let remaining = work_budget.remaining_wall_time(stage)?;
    let outcome = match remaining {
        Some(remaining) => match tokio::time::timeout(remaining, &mut task).await {
            Ok(outcome) => outcome,
            Err(_) => {
                let _quiescence_outcome = task.await;
                work_budget.check_deadline(stage)?;
                bail!(
                    "max_wall_seconds deadline elapsed after waiting {remaining:?} at stage {stage}; blocking task was reaped to quiescence"
                );
            }
        },
        None => task.await,
    };
    work_budget.check_deadline(stage)?;
    Ok(outcome)
}

/// Exact checked sum of `ceil(table_rows / max_row_group_size)`.
pub fn projected_row_group_count(
    table_rows: impl IntoIterator<Item = u64>,
    max_row_group_size: u64,
) -> Result<u64> {
    ensure!(
        max_row_group_size > 0,
        "max_row_group_size must be positive"
    );
    table_rows.into_iter().try_fold(0_u64, |total, rows| {
        let groups = if rows == 0 {
            0
        } else {
            rows
                .checked_sub(1)
                .and_then(|value| value.checked_div(max_row_group_size))
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| {
                    anyhow!(
                        "max_projected_row_groups calculation overflow for table rows {rows} and max_row_group_size {max_row_group_size}"
                    )
                })?
        };
        total.checked_add(groups).ok_or_else(|| {
            anyhow!(
                "max_projected_row_groups calculation overflow: partial {total}, next {groups}"
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        future::pending,
        io::Write,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        sync::{Arc, Mutex, mpsc},
        time::Duration,
    };

    use super::{
        CooperativeDeadlineWriter, ExactSizedObjectBuffer, OperatorWorkBudget,
        OperatorWorkBudgetClock, OperatorWorkBudgetDeadline, OperatorWorkBudgetGuard,
        OperatorWorkBudgetStage, cooperative_stable_sort_by, cooperative_stable_sort_by_key,
        deserialize_json_stream_with_budget, deserialize_json_with_budget,
        for_each_nonempty_text_record_with_budget, guarded_operation_outcome,
        projected_row_group_count, read_exact_sized_file_guarded,
        read_exact_sized_hashed_file_guarded, read_exact_sized_hashed_pinned_file_guarded,
        read_exact_sized_pinned_file_guarded, read_file_with_budget,
        sha256_exact_sized_file_guarded, sha256_hex_with_budget, system_operator_work_budget_clock,
    };
    use crate::backfill_execution_plan::BackfillExecutionWorkBudget;

    #[derive(Debug, Default)]
    struct FakeClock {
        now: Mutex<Duration>,
    }

    impl FakeClock {
        fn advance(&self, elapsed: Duration) {
            let mut now = self.now.lock().expect("fake clock lock poisoned");
            *now = now.checked_add(elapsed).expect("fake clock overflow");
        }

        fn set(&self, now: Duration) {
            *self.now.lock().expect("fake clock lock poisoned") = now;
        }
    }

    impl OperatorWorkBudgetClock for FakeClock {
        fn now(&self) -> Duration {
            *self.now.lock().expect("fake clock lock poisoned")
        }
    }

    struct ExpiringObservationClock {
        observations: AtomicU64,
        expires_at: u64,
    }

    impl OperatorWorkBudgetClock for ExpiringObservationClock {
        fn now(&self) -> Duration {
            let observation = self.observations.fetch_add(1, Ordering::Relaxed);
            Duration::from_secs(u64::from(observation >= self.expires_at))
        }
    }

    struct RegressingObservationClock {
        observations: AtomicU64,
        regresses_at: u64,
    }

    impl OperatorWorkBudgetClock for RegressingObservationClock {
        fn now(&self) -> Duration {
            let observation = self.observations.fetch_add(1, Ordering::Relaxed);
            Duration::from_secs(if observation >= self.regresses_at {
                4
            } else {
                5
            })
        }
    }

    fn budget(
        max_source_rows: u64,
        max_projected_row_groups: u64,
        max_wall_seconds: u64,
    ) -> BackfillExecutionWorkBudget {
        BackfillExecutionWorkBudget {
            max_decoded_bytes: u64::MAX,
            max_source_rows,
            max_projected_row_groups,
            max_wall_seconds,
            require_object_selection_metadata: true,
        }
    }

    #[test]
    fn production_monotonic_clock_instances_share_one_kernel_epoch() {
        let first_clock = system_operator_work_budget_clock();
        std::thread::sleep(Duration::from_millis(2));
        let first_later = first_clock.now();
        let second_clock = system_operator_work_budget_clock();
        let second_now = second_clock.now();
        assert!(
            second_now >= first_later,
            "a newly constructed production clock must not reset its epoch"
        );
    }

    #[test]
    fn delayed_child_start_cannot_reset_or_authorize_after_parent_deadline() {
        let interval = OperatorWorkBudgetDeadline {
            started_at_seconds: 10,
            started_at_nanoseconds: 0,
            deadline_seconds: 11,
            deadline_nanoseconds: 0,
        };
        let before_deadline_clock = Arc::new(FakeClock::default());
        before_deadline_clock.set(Duration::from_millis(10_900));
        let guard = OperatorWorkBudgetGuard::with_absolute_deadline_and_clock(
            OperatorWorkBudget::Backfill(budget(2, 1, 1)),
            1,
            interval,
            before_deadline_clock.clone(),
        )
        .expect("child may enter before the sealed deadline");
        before_deadline_clock.advance(Duration::from_millis(100));
        let commit_error = guard
            .authorize_commit(OperatorWorkBudgetStage::Finalize)
            .expect_err("child cannot authorize a commit at the parent deadline");
        assert!(
            commit_error.to_string().contains("reached or exceeds"),
            "{commit_error:#}"
        );

        let delayed_clock = Arc::new(FakeClock::default());
        delayed_clock.set(Duration::from_secs(11));
        let error = OperatorWorkBudgetGuard::with_absolute_deadline_and_clock(
            OperatorWorkBudget::Backfill(budget(2, 1, 1)),
            1,
            interval,
            delayed_clock,
        )
        .expect_err("child startup at the sealed deadline must fail closed");
        assert!(
            error
                .to_string()
                .contains("startup reached or exceeded the sealed"),
            "{error:#}"
        );
    }

    #[test]
    fn commit_permit_consumption_never_extends_its_original_wall_deadline() {
        let clock = Arc::new(FakeClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(2, 1, 2)),
            clock.clone(),
        )
        .expect("construct finite guard");
        clock.advance(Duration::from_millis(500));

        let permit = guard
            .authorize_commit(OperatorWorkBudgetStage::Publish)
            .expect("authorize before deadline");
        assert_eq!(permit.stage(), OperatorWorkBudgetStage::Publish);
        clock.advance(Duration::from_millis(500));
        assert_eq!(
            permit
                .remaining_wall_time_at_consumption()
                .expect("consume before original deadline"),
            Some(Duration::from_millis(1_000)),
            "delayed consumption must subtract elapsed wall time"
        );

        let expired_permit = guard
            .authorize_commit(OperatorWorkBudgetStage::Publish)
            .expect("authorize second permit before deadline");
        clock.advance(Duration::from_secs(1));
        let error = expired_permit
            .remaining_wall_time_at_consumption()
            .expect_err("permit cannot be consumed at its original deadline");
        assert!(
            error.to_string().contains("original wall deadline"),
            "{error:#}"
        );

        let unbounded = OperatorWorkBudgetGuard::unbounded()
            .authorize_commit(OperatorWorkBudgetStage::Publish)
            .expect("authorize unbounded research commit");
        assert_eq!(
            unbounded
                .remaining_wall_time_at_consumption()
                .expect("consume unbounded permit"),
            None
        );
    }

    #[test]
    fn exact_sized_buffer_rejects_expected_plus_one_without_unbounded_growth() {
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");
        let mut buffer = ExactSizedObjectBuffer::new(4).expect("construct exact buffer");
        let error = buffer
            .push(
                b"12345",
                &guard,
                OperatorWorkBudgetStage::ObjectVerification,
            )
            .expect_err("expected+1 byte must fail closed");

        assert!(error.to_string().contains("exceeds pinned expected size"));
        assert_eq!(buffer.bytes.len(), 5, "only the sentinel byte is retained");
    }

    #[test]
    fn exact_sized_buffer_rejects_a_short_stream_at_finish() {
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");
        let mut buffer = ExactSizedObjectBuffer::new(4).expect("construct exact buffer");
        buffer
            .push(b"123", &guard, OperatorWorkBudgetStage::ObjectVerification)
            .expect("partial chunk remains bounded");
        let error = buffer
            .finish(&guard, OperatorWorkBudgetStage::ObjectVerification)
            .expect_err("short stream must fail closed");

        assert!(
            error
                .to_string()
                .contains("does not match pinned expected size")
        );
    }

    #[test]
    fn exact_sized_file_rejects_huge_declared_size_before_allocation() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        fs::write(file.path(), b"x").expect("write byte");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");

        let error = read_exact_sized_file_guarded(
            file.path(),
            u64::MAX,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("declared size mismatch must precede any declared-size allocation");

        assert!(
            error.to_string().contains("object byte length 1"),
            "{error:#}"
        );
    }

    #[test]
    fn exact_sized_file_observes_deadline_at_read_boundaries() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        fs::write(file.path(), b"1234567890123456").expect("write object");
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 6,
            }),
        )
        .expect("construct bounded guard");

        let error = read_exact_sized_file_guarded(
            file.path(),
            16,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("multi-chunk file read must observe the deadline");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn exact_sized_file_hash_matches_the_in_memory_hash_without_full_file_allocation() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        let payload = b"content-addressed sealed file";
        fs::write(file.path(), payload).expect("write object");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");

        let file_hash = sha256_exact_sized_file_guarded(
            file.path(),
            u64::try_from(payload.len()).expect("payload length fits u64"),
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect("hash exact-size file");

        assert_eq!(
            file_hash,
            sha256_hex_with_budget(payload, &guard, OperatorWorkBudgetStage::ObjectVerification,)
                .expect("hash in-memory payload")
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_file_reader_rejects_a_symlink_before_reading_target_bytes() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("target.json");
        let link = directory.path().join("link.json");
        fs::write(&target, b"{}").expect("write target");
        symlink(&target, &link).expect("create symlink");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");

        let error = read_file_with_budget(&link, &guard, OperatorWorkBudgetStage::Decode)
            .expect_err("symlink must fail closed");

        assert!(
            error.to_string().contains("symlink or special file"),
            "{error:#}"
        );
    }

    #[test]
    fn authoritative_file_read_binds_length_hash_and_bytes_in_one_traversal() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        let payload = b"one capability, one traversal";
        fs::write(file.path(), payload).expect("write object");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");
        let expected_sha256 =
            sha256_hex_with_budget(payload, &guard, OperatorWorkBudgetStage::ObjectVerification)
                .expect("hash expected payload");

        let (actual, identity) = read_exact_sized_hashed_pinned_file_guarded(
            file.path(),
            u64::try_from(payload.len()).expect("payload length fits u64"),
            &expected_sha256,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect("authoritative read");

        assert_eq!(actual, payload);
        identity
            .revalidate_path(file.path())
            .expect("returned identity remains bound to the traversed path");
    }

    #[test]
    fn exact_file_read_returns_the_identity_from_its_single_traversal() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        let payload = b"one non-hashing pinned traversal";
        fs::write(file.path(), payload).expect("write object");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");

        let (actual, identity) = read_exact_sized_pinned_file_guarded(
            file.path(),
            u64::try_from(payload.len()).expect("payload length fits u64"),
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect("authoritative non-hashing read");

        assert_eq!(actual, payload);
        identity
            .revalidate_path(file.path())
            .expect("returned identity remains bound to the traversed path");
    }

    #[test]
    fn authoritative_file_read_rejects_expected_hash_mismatch() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        let payload = b"content-addressed object";
        fs::write(file.path(), payload).expect("write object");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");
        let wrong_sha256 = "0".repeat(64);

        let error = read_exact_sized_hashed_file_guarded(
            file.path(),
            u64::try_from(payload.len()).expect("payload length fits u64"),
            &wrong_sha256,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("wrong expected hash must fail closed");

        assert!(error.to_string().contains("SHA-256 mismatch"), "{error:#}");
    }

    #[test]
    fn authoritative_file_read_rejects_expected_length_mismatch_before_traversal() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        let payload = b"content-addressed object";
        fs::write(file.path(), payload).expect("write object");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(2, 1, 1)))
            .expect("construct bounded guard");
        let expected_sha256 =
            sha256_hex_with_budget(payload, &guard, OperatorWorkBudgetStage::ObjectVerification)
                .expect("hash expected payload");

        let error = read_exact_sized_hashed_file_guarded(
            file.path(),
            u64::try_from(payload.len() - 1).expect("payload length fits u64"),
            &expected_sha256,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("wrong expected length must fail before traversal");

        assert!(
            error
                .to_string()
                .contains("does not match pinned expected size"),
            "{error:#}"
        );
    }

    #[test]
    fn local_readers_sample_the_clock_after_final_identity_revalidation() {
        const EMPTY_SHA256: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        fn read_empty_exact(
            path: &std::path::Path,
            guard: &OperatorWorkBudgetGuard,
            stage: OperatorWorkBudgetStage,
        ) -> anyhow::Result<Vec<u8>> {
            read_exact_sized_file_guarded(path, 0, guard, stage)
        }
        type LocalReader = fn(
            &std::path::Path,
            &OperatorWorkBudgetGuard,
            OperatorWorkBudgetStage,
        ) -> anyhow::Result<Vec<u8>>;
        let file = tempfile::NamedTempFile::new().expect("temp file");

        for (regresses_at, read) in [
            (5, read_file_with_budget as LocalReader),
            (5, read_empty_exact as LocalReader),
        ] {
            let clock = Arc::new(RegressingObservationClock {
                observations: AtomicU64::new(0),
                regresses_at,
            });
            let guard = OperatorWorkBudgetGuard::with_clock(
                OperatorWorkBudget::Backfill(budget(2, 1, 1)),
                clock,
            )
            .expect("construct regressing guard");
            let error = read(
                file.path(),
                &guard,
                OperatorWorkBudgetStage::ObjectVerification,
            )
            .expect_err("post-revalidation clock regression must fail closed");
            assert!(
                error
                    .to_string()
                    .contains("monotonic work-budget clock regressed"),
                "{error:#}"
            );
        }

        let clock = Arc::new(RegressingObservationClock {
            observations: AtomicU64::new(0),
            regresses_at: 5,
        });
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(2, 1, 1)),
            clock,
        )
        .expect("construct hash guard");
        let error = sha256_exact_sized_file_guarded(
            file.path(),
            0,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("post-hash identity clock regression must fail closed");
        assert!(
            error
                .to_string()
                .contains("monotonic work-budget clock regressed"),
            "{error:#}"
        );

        let clock = Arc::new(RegressingObservationClock {
            observations: AtomicU64::new(0),
            regresses_at: 7,
        });
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(2, 1, 1)),
            clock,
        )
        .expect("construct hashed read guard");
        let error = read_exact_sized_hashed_file_guarded(
            file.path(),
            0,
            EMPTY_SHA256,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("post-hashed-read identity clock regression must fail closed");
        assert!(
            error
                .to_string()
                .contains("monotonic work-budget clock regressed"),
            "{error:#}"
        );
    }

    #[test]
    fn local_file_reader_rejects_metadata_length_before_payload_allocation() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        fs::write(file.path(), b"oversize").expect("write object");
        let mut limits = budget(2, 1, 1);
        limits.max_decoded_bytes = 4;
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(limits))
            .expect("construct bounded guard");

        let error = read_file_with_budget(file.path(), &guard, OperatorWorkBudgetStage::Decode)
            .expect_err("metadata length above the decoded-byte cap must fail closed");

        assert!(error.to_string().contains("max_decoded_bytes"), "{error:#}");
    }

    #[test]
    fn production_cli_and_batch_entrypoints_cannot_select_unbounded_work() {
        const MAIN_SOURCE: &str = include_str!("main.rs");
        const OPERATOR_SOURCE: &str = include_str!("operator.rs");
        const BATCH_SOURCE: &str = include_str!("source_universe_batch_execution.rs");
        const BATCH_CLI_SOURCE: &str = include_str!("bin/source_universe_batch_execution.rs");

        let main_production = MAIN_SOURCE
            .rsplit_once("#[cfg(test)]\nmod tests")
            .map(|(source, _tests)| source)
            .expect("main production source before the test module");
        assert_eq!(
            main_production
                .matches("OperatorWorkBudget::from_execution_plan(&execution_plan)")
                .count(),
            1,
            "the local-only direct CLI must derive its sole guard from the validated plan"
        );
        assert!(!main_production.contains("OperatorWorkBudget::Unbounded"));
        assert!(!main_production.contains("OperatorWorkBudgetGuard::unbounded"));
        for forbidden in [
            concat!("DurableRun", "Dispatcher"),
            concat!("DurableRun", "Request"),
            concat!("DurableCompletion", "Locator"),
        ] {
            assert!(
                !main_production.contains(forbidden),
                "the direct CLI must not expose durable execution through {forbidden}"
            );
        }
        assert!(!OPERATOR_SOURCE.contains("pub struct DurableRunDispatcher"));
        assert!(
            !OPERATOR_SOURCE.contains("pub async fn run_from_run_spec_with_artifact_store_guarded")
        );
        assert!(
            !OPERATOR_SOURCE
                .contains("pub async fn run_from_run_spec_with_verified_registry_guarded")
        );

        let resolve_work_item = BATCH_SOURCE
            .split("fn resolve_work_item")
            .nth(1)
            .and_then(|source| source.split("fn process_fresh_work_item").next())
            .expect("resolve_work_item production source");
        assert_eq!(
            resolve_work_item
                .matches("OperatorWorkBudgetGuard::from_execution_plan_with_clock(")
                .count(),
            1,
            "every selected record must construct exactly one plan-derived guard"
        );
        assert!(!resolve_work_item.contains("OperatorWorkBudget::Unbounded"));
        assert!(!resolve_work_item.contains("OperatorWorkBudgetGuard::unbounded"));
        let durable_worker = BATCH_SOURCE
            .split("fn execute_source_universe_operator_worker_from_archive")
            .nth(1)
            .and_then(|source| {
                source
                    .split("fn verify_canonical_worker_request_manifest_bytes")
                    .next()
            })
            .expect("durable hidden-worker production source");
        assert!(
            !BATCH_SOURCE.contains("pub trait SourceUniverseWorkBudgetClockFactory")
                && !BATCH_SOURCE
                    .contains("pub fn execute_source_universe_batch_with_clock_factory"),
            "batch clock injection must remain private to tests"
        );
        let clock_trait = BATCH_SOURCE
            .split("trait SourceUniverseWorkBudgetClockFactory")
            .nth(1)
            .and_then(|source| source.split('}').next())
            .expect("private batch clock factory trait");
        assert!(clock_trait.contains("create_clock"));
        assert!(
            !clock_trait.contains("OperatorWorkBudgetGuard") && !clock_trait.contains("budget:"),
            "clock injection may supply time only, never a caller-selected budget or guard"
        );
        assert_eq!(
            durable_worker
                .matches("DurableRunDispatcher::prepare_guarded(")
                .count(),
            1,
            "only the hidden supervised worker may prepare durable dispatch"
        );
        assert_eq!(
            BATCH_SOURCE
                .matches("DurableRunDispatcher::prepare_guarded(")
                .count(),
            1,
            "batch production and tests must not assemble a second durable dispatcher path"
        );

        assert!(BATCH_CLI_SOURCE.contains("execute_source_universe_batch_process_isolated("));
        assert!(!BATCH_CLI_SOURCE.contains("LocalSourceUniverseOperatorRunner"));
        assert!(!BATCH_CLI_SOURCE.contains("SourceUniverseOperatorRunOutcome::NonTerminal"));
        assert!(
            BATCH_SOURCE.contains(".redirect(reqwest::redirect::Policy::none())"),
            "the exact HTTPS source transport must not follow redirects to a different URI"
        );
        let isolated_runner = BATCH_SOURCE
            .split(
                "impl SourceUniverseOperatorRunner for ProcessIsolatedSourceUniverseOperatorRunner",
            )
            .nth(1)
            .and_then(|source| source.split("struct PinnedWorkerDirectoryLease").next())
            .expect("process-isolated runner implementation source");
        assert_eq!(
            isolated_runner
                .matches("self.worker_termination_grace")
                .count(),
            2,
            "execute and deterministic discovery must share the one configured termination grace"
        );
        assert!(
            !isolated_runner.contains("execution_plan.max_wall_seconds"),
            "the execution wall budget must not be reused as a second termination grace"
        );
    }

    #[test]
    fn guarded_json_parsing_observes_deadline_between_byte_chunks() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 2,
            }),
        )
        .expect("construct bounded guard");
        let json = format!(r#"{{"payload":"{}"}}"#, "x".repeat(128));

        let error = deserialize_json_with_budget::<serde_json::Value>(
            json.as_bytes(),
            &guard,
            OperatorWorkBudgetStage::Normalize,
        )
        .expect_err("large single JSON value must observe the deadline while parsing");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn guarded_json_parse_error_still_observes_expired_deadline() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 3,
            }),
        )
        .expect("construct bounded guard");

        let error = deserialize_json_with_budget::<serde_json::Value>(
            b"!",
            &guard,
            OperatorWorkBudgetStage::Normalize,
        )
        .expect_err("the post-parse deadline must take precedence over the parse error");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn cooperative_writer_observes_deadline_between_byte_chunks() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 5,
            }),
        )
        .expect("construct bounded guard");
        let mut bytes = Vec::new();
        let mut writer =
            CooperativeDeadlineWriter::new(&mut bytes, &guard, OperatorWorkBudgetStage::Publish);

        let error = writer
            .write_all(&[b'x'; 128])
            .expect_err("large write must observe the deadline between bounded chunks");

        assert!(error.to_string().contains("max_wall_seconds"), "{error}");
        assert!(
            bytes.len() < 128,
            "writer must stop before the full payload"
        );
    }

    #[test]
    fn guarded_json_stream_parses_multiple_top_level_values_once() {
        let guard = OperatorWorkBudgetGuard::unbounded();
        let values = deserialize_json_stream_with_budget::<serde_json::Value>(
            br#"{"page":1}
{"page":2}"#,
            &guard,
            OperatorWorkBudgetStage::Normalize,
        )
        .expect("parse concatenated page stream");

        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["page"], 1);
        assert_eq!(values[1]["page"], 2);
    }

    #[test]
    fn guarded_sha256_matches_the_shared_digest_and_expires_between_work_units() {
        let payload = [b'x'; 128];
        let actual = sha256_hex_with_budget(
            &payload,
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect("hash payload");
        assert_eq!(actual, crate::hashing::sha256_hex(&payload));

        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 5,
            }),
        )
        .expect("construct bounded guard");
        let error = sha256_hex_with_budget(
            &payload,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("hash must observe the deadline after the hash operation");
        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn guarded_text_records_preserve_lines_semantics_and_check_line_boundaries() {
        let mut records = Vec::new();
        for_each_nonempty_text_record_with_budget(
            "first\r\n\n \t\r\nlast\n",
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Normalize,
            |index, line| {
                records.push((index, line.to_string()));
                Ok(())
            },
        )
        .expect("visit text records");
        assert_eq!(
            records,
            vec![(0, "first".to_string()), (3, "last".to_string())]
        );

        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 3,
            }),
        )
        .expect("construct bounded guard");
        let error = for_each_nonempty_text_record_with_budget(
            &" ".repeat(128),
            &guard,
            OperatorWorkBudgetStage::Normalize,
            |_, _| panic!("blank input must not be visited"),
        )
        .expect_err("blank record must observe the deadline after its line scan");
        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn cooperative_stable_sort_preserves_ties_and_can_expire_mid_sort() {
        let mut stable_rows = vec![(2, "first"), (1, "middle"), (2, "last")];
        let original_pointer = stable_rows.as_ptr();
        let original_capacity = stable_rows.capacity();
        cooperative_stable_sort_by_key(
            &mut stable_rows,
            |row| row.0,
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Normalize,
        )
        .expect("stable sort");
        assert_eq!(stable_rows, vec![(1, "middle"), (2, "first"), (2, "last")]);
        assert_eq!(stable_rows.as_ptr(), original_pointer);
        assert_eq!(stable_rows.capacity(), original_capacity);

        let mut dynamic_rows = vec![
            ("beta".to_string(), 0),
            ("alpha".to_string(), 1),
            ("beta".to_string(), 2),
        ];
        cooperative_stable_sort_by(
            &mut dynamic_rows,
            |left, right| left.0.cmp(&right.0),
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::Normalize,
        )
        .expect("borrowed dynamic-key stable sort");
        assert_eq!(
            dynamic_rows,
            vec![
                ("alpha".to_string(), 1),
                ("beta".to_string(), 0),
                ("beta".to_string(), 2),
            ]
        );

        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(100, 1, 1)),
            Arc::new(ExpiringObservationClock {
                observations: AtomicU64::new(0),
                expires_at: 5,
            }),
        )
        .expect("construct bounded guard");
        let mut descending = (0..128).rev().collect::<Vec<_>>();
        let error = cooperative_stable_sort_by_key(
            &mut descending,
            |value| *value,
            &guard,
            OperatorWorkBudgetStage::Normalize,
        )
        .expect_err("sort must observe the deadline between row chunks");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
    }

    #[test]
    fn guarded_error_outcome_still_observes_expired_deadline() {
        let clock = Arc::new(FakeClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 1, 1)),
            clock.clone(),
        )
        .expect("guard");

        let error = guarded_operation_outcome(
            &guard,
            OperatorWorkBudgetStage::Backtest,
            || -> std::result::Result<(), anyhow::Error> {
                clock.advance(Duration::from_secs(1));
                anyhow::bail!("synthetic inner error")
            },
        )
        .expect_err("deadline must take precedence over inner error");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(
            !error.to_string().contains("synthetic inner error"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn guarded_async_error_outcome_still_observes_expired_deadline() {
        let clock = Arc::new(FakeClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 1, 1)),
            clock.clone(),
        )
        .expect("guard");

        let error =
            super::guarded_async_operation_outcome(&guard, OperatorWorkBudgetStage::Fetch, async {
                clock.advance(Duration::from_secs(1));
                Err::<(), anyhow::Error>(anyhow::anyhow!("synthetic async inner error"))
            })
            .await
            .expect_err("deadline must take precedence over async inner error");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(
            !error.to_string().contains("synthetic async inner error"),
            "{error:#}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn guarded_async_operation_is_cancelled_at_the_hard_wall_deadline() {
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(budget(1, 1, 1)))
            .expect("guard");

        let error = super::guarded_async_operation_outcome(
            &guard,
            OperatorWorkBudgetStage::Fetch,
            pending::<std::result::Result<(), anyhow::Error>>(),
        )
        .await
        .expect_err("never-ready future must be cancelled at the wall deadline");

        assert!(error.to_string().contains("hard timeout"), "{error:#}");
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_blocking_join_is_quiescent_before_control_returns() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 1, 1)),
            Arc::new(FakeClock::default()),
        )
        .expect("guard");
        let quiesced = Arc::new(AtomicBool::new(false));
        let task_quiesced = quiesced.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            started_tx.send(()).expect("report blocking task start");
            release_rx.recv().expect("release blocking task");
            task_quiesced.store(true, Ordering::SeqCst);
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking task starts");

        let guarded =
            super::guarded_blocking_join_outcome(&guard, OperatorWorkBudgetStage::Backtest, task);
        tokio::pin!(guarded);
        tokio::select! {
            biased;
            outcome = &mut guarded => panic!("timed-out blocking task returned before quiescence: {outcome:?}"),
            () = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
        assert!(
            !quiesced.load(Ordering::SeqCst),
            "the blocked task must still be running before its release"
        );

        release_tx.send(()).expect("release blocking task");
        let error = guarded
            .await
            .expect_err("the elapsed wall deadline must still fail after quiescence");
        assert!(quiesced.load(Ordering::SeqCst));
        assert!(error.to_string().contains("deadline elapsed"), "{error:#}");
    }

    #[tokio::test]
    async fn guarded_blocking_join_preserves_successful_output() {
        let guard = OperatorWorkBudgetGuard::unbounded();
        let outcome = super::guarded_blocking_join_outcome(
            &guard,
            OperatorWorkBudgetStage::Backtest,
            tokio::task::spawn_blocking(|| 42_u8),
        )
        .await
        .expect("guarded join");

        assert_eq!(outcome.expect("join blocking task"), 42);
    }

    #[tokio::test]
    async fn guarded_blocking_join_preserves_join_failure() {
        let guard = OperatorWorkBudgetGuard::unbounded();
        let outcome = super::guarded_blocking_join_outcome(
            &guard,
            OperatorWorkBudgetStage::Backtest,
            tokio::task::spawn_blocking(|| -> u8 { panic!("synthetic blocking panic") }),
        )
        .await
        .expect("work-budget wrapper must not hide the join result");

        assert!(
            outcome
                .expect_err("blocking task must fail its join")
                .is_panic()
        );
    }

    #[test]
    fn source_row_limit_allows_equality_and_rejects_next_record() {
        let clock = Arc::new(FakeClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(2, 1, 1)),
            clock,
        )
        .expect("construct guard");

        guard
            .consume_source_row(OperatorWorkBudgetStage::Decode)
            .expect("row 1");
        guard
            .consume_source_row(OperatorWorkBudgetStage::Decode)
            .expect("row 2");
        let error = guard
            .consume_source_row(OperatorWorkBudgetStage::Decode)
            .expect_err("row 3 exceeds limit");

        assert_eq!(guard.source_rows_consumed(), 3);
        assert!(
            error
                .to_string()
                .contains("max_source_rows actual 3 exceeds limit 2"),
            "{error:#}"
        );
    }

    #[test]
    fn projected_row_group_limit_allows_equality_and_names_actual_and_limit() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 2, 1)),
            Arc::new(FakeClock::default()),
        )
        .expect("construct guard");

        guard
            .check_projected_row_groups(2, OperatorWorkBudgetStage::CatalogProjection)
            .expect("equality is allowed");
        let error = guard
            .check_projected_row_groups(3, OperatorWorkBudgetStage::CatalogProjection)
            .expect_err("three row groups exceed limit two");

        assert!(
            error
                .to_string()
                .contains("max_projected_row_groups actual 3 exceeds limit 2"),
            "{error:#}"
        );
    }

    #[test]
    fn projected_row_group_math_is_checked_and_exact() {
        assert_eq!(
            projected_row_group_count([0, 1, 5, 6], 5).expect("projected groups"),
            4
        );
        let error =
            projected_row_group_count([u64::MAX, u64::MAX], 1).expect_err("sum must overflow");
        assert!(
            error
                .to_string()
                .contains("max_projected_row_groups calculation overflow"),
            "{error:#}"
        );
    }

    #[test]
    fn wall_limit_expires_at_equality_and_names_expiry_stage() {
        let clock = Arc::new(FakeClock::default());
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 1, 2)),
            clock.clone(),
        )
        .expect("construct guard");

        clock.advance(Duration::from_secs(2));
        let error = guard
            .check_deadline(OperatorWorkBudgetStage::Publish)
            .expect_err("deadline equality is expired");

        let message = error.to_string();
        assert!(message.contains("max_wall_seconds actual"), "{error:#}");
        assert!(message.contains("limit 2s"), "{error:#}");
        assert!(message.contains("publish"), "{error:#}");
    }

    #[test]
    fn wall_deadline_construction_fails_closed_on_duration_overflow() {
        let clock = Arc::new(FakeClock {
            now: Mutex::new(Duration::from_secs(1)),
        });
        let error = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 1, u64::MAX)),
            clock,
        )
        .expect_err("deadline addition must overflow");

        assert!(
            error
                .to_string()
                .contains("max_wall_seconds deadline overflow"),
            "{error:#}"
        );
    }

    #[test]
    fn wall_clock_regression_fails_closed() {
        let clock = Arc::new(FakeClock {
            now: Mutex::new(Duration::from_secs(5)),
        });
        let guard = OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(budget(1, 1, 2)),
            clock.clone(),
        )
        .expect("construct guard");
        clock.set(Duration::from_secs(4));

        let error = guard
            .remaining_wall_time(OperatorWorkBudgetStage::Fetch)
            .expect_err("clock regression must fail closed");

        assert!(
            error
                .to_string()
                .contains("monotonic work-budget clock regressed"),
            "{error:#}"
        );
    }

    #[test]
    fn unbounded_mode_is_a_noop_through_the_same_guard() {
        let clock = Arc::new(FakeClock::default());
        let guard =
            OperatorWorkBudgetGuard::with_clock(OperatorWorkBudget::Unbounded, clock.clone())
                .expect("construct guard");

        clock.advance(Duration::from_secs(u32::MAX.into()));
        guard
            .consume_source_row(OperatorWorkBudgetStage::Decode)
            .unwrap();
        guard
            .check_projected_row_groups(u64::MAX, OperatorWorkBudgetStage::CatalogProjection)
            .unwrap();
        guard
            .check_deadline(OperatorWorkBudgetStage::Publish)
            .unwrap();
        assert_eq!(guard.source_rows_consumed(), 0);
    }
}
