# Data Model: CI Baseline Measurement

## BaselineRun

- `run_id`: GitHub Actions database ID.
- `url`: GitHub Actions run URL.
- `event`: `pull_request`, `push`, tag push, or other event type.
- `head_sha`: Commit SHA reported by the run.
- `head_ref`: Branch or tag shown by GitHub.
- `created_at`: Run creation timestamp.
- `updated_at`: Last update timestamp.
- `status`: Run status.
- `conclusion`: Run conclusion.
- `reason_included`: Why this run is part of the baseline.
- `workflow_wall_time`: Elapsed time from run creation to update or completion where completed.

## JobTiming

- `run_id`: Parent `BaselineRun`.
- `job_name`: GitHub Actions job name.
- `started_at`: Job start timestamp.
- `completed_at`: Job completion timestamp.
- `status`: Job status.
- `conclusion`: Job conclusion.
- `duration_seconds`: Derived from job timestamps.
- `required_for_gate`: Whether the `gate` job checks this lane in current workflow.
- `skip_meaning`: Empty for executed jobs; otherwise why skipped matters.

## CacheObservation

- `run_id`: Parent `BaselineRun`.
- `job_name`: Job where cache evidence was observed.
- `cache_key`: Log-reported cache key or restore key.
- `cache_result`: `hit`, `miss`, or `unknown`; summary tables may list multiple job-specific results when one run has mixed cache evidence.
- `archive_size`: Log-reported cache archive size when present.
- `compile_or_test_signal`: Concrete log signal, such as nextest command time, first-test time, failure line, or build command time.

## ChildIssueState

- `issue_number`: GitHub issue number.
- `title`: Issue title.
- `state`: Open, closed, blocked, or partially blocked.
- `current_scope`: Scope from live issue body and comments.
- `dependencies`: Blocking issue or PR references.
- `measurement_use`: Which baseline row this child should compare against.
- `scope_conflicts`: Any live body/comment inconsistency that affects current interpretation.
