//! Shared cooperative work-budget enforcement for operator execution.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail, ensure};

use crate::backfill_execution_plan::{BackfillExecutionPlan, BackfillExecutionWorkBudget};

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
#[derive(Debug)]
pub struct OperatorWorkBudgetCommitPermit {
    stage: OperatorWorkBudgetStage,
}

impl OperatorWorkBudgetCommitPermit {
    /// Stage authorized by the guard. The permit itself is intentionally not
    /// cloneable and must be consumed by the commit operation.
    pub const fn stage(&self) -> OperatorWorkBudgetStage {
        self.stage
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

#[derive(Debug)]
struct SystemOperatorWorkBudgetClock {
    epoch: Instant,
}

impl Default for SystemOperatorWorkBudgetClock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl OperatorWorkBudgetClock for SystemOperatorWorkBudgetClock {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
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
        Self::with_clock(budget, Arc::new(SystemOperatorWorkBudgetClock::default()))
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

    /// Authorize exactly one local rename or remote conditional PUT at the
    /// sampled instant. The returned non-cloneable permit is consumed by that
    /// commit, so no later clock read can retroactively invalidate it.
    pub fn authorize_commit(
        &self,
        stage: OperatorWorkBudgetStage,
    ) -> Result<OperatorWorkBudgetCommitPermit> {
        self.check_deadline(stage)?;
        Ok(OperatorWorkBudgetCommitPermit { stage })
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
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{
        OperatorWorkBudget, OperatorWorkBudgetClock, OperatorWorkBudgetGuard,
        OperatorWorkBudgetStage, projected_row_group_count,
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

    fn budget(
        max_source_rows: u64,
        max_projected_row_groups: u64,
        max_wall_seconds: u64,
    ) -> BackfillExecutionWorkBudget {
        BackfillExecutionWorkBudget {
            max_source_rows,
            max_projected_row_groups,
            max_wall_seconds,
            require_object_selection_metadata: true,
        }
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

    #[test]
    fn validated_plan_entrypoints_construct_backfill_guards_only() {
        const CLI_SOURCE: &str = include_str!("main.rs");
        const BATCH_SOURCE: &str = include_str!("source_universe_batch_execution.rs");

        for (label, source) in [("CLI", CLI_SOURCE), ("batch", BATCH_SOURCE)] {
            assert!(
                source.contains("OperatorWorkBudget::from_execution_plan"),
                "{label} entrypoint must derive its guard from the validated execution plan"
            );
            assert!(
                !source.contains("OperatorWorkBudgetGuard::unbounded")
                    && !source.contains("OperatorWorkBudget::Unbounded"),
                "{label} plan entrypoint must not select an unbounded budget"
            );
        }
    }
}
