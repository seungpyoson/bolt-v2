# CI Failed Job Log Snippets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface bounded failed-job log snippets for the exact full-CI run before the whole workflow reaches terminal state.

**Architecture:** Keep `verify-remote` as the exact-head proof waiter. Add a focused CI log collection module that inspects failed completed jobs for the current full-CI run attempt and returns sanitized snippets; `verify-remote` calls it opportunistically while polling, and `just ci-logs` runs the same collector once for the current exact PR head.

**Tech Stack:** Python 3.12+ stdlib, GitHub CLI `gh`, existing TOML configs in `ci/`, existing `just` recipes, GitHub Actions job-log REST/CLI behavior.

---

## Investigation Summary

- Clean slice worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/ci-log-collector-plan`
- Branch: `codex/ci-log-collector-plan`
- Base: `origin/main` at `532b9ba3623d9bd4fdc87361c10593c593cf6827`
- Current high-value workflow: `.github/workflows/ci.yml` named `CI`
- Current shard jobs: `nextest shard ${{ matrix.shard }} of 4` with `strategy.fail-fast: false`
- Existing job map: `ci/github-actions-runners.toml` already defines `[ci_provenance.full_ci.jobs]`, including the test-shard check-name template and shard count.
- Existing verifier: `scripts/rust_verification.py` already finds/dispatches exact-head full-CI runs but only watches workflow-run status.
- Existing config split: `ci/rust-verification.toml` owns `verify-remote` polling; `ci/github-actions-runners.toml` owns GitHub workflow/job identity and API page sizes.
- GitHub API fact: job metadata can be listed by workflow run attempt, and job logs are fetched by job id through a redirecting endpoint whose link expires quickly. GitHub CLI also supports `gh run view --log --job <job-id>` and `--log-failed`.

## Scope

Implement one slice only:

- Full-CI workflow only, using `ci_provenance.workflow_name = "CI"` and `workflow_path = ".github/workflows/ci.yml"`.
- Current branch PR only.
- Exact pushed HEAD only.
- Latest matching run/attempt only.
- Failed completed jobs only.
- Bounded sanitized snippets only.
- No workflow YAML changes.
- No cargo/nextest behavior changes.
- No persisted full logs by default.

## File Structure

- Create `scripts/ci_log_collection.py`: pure collection library. It loads log-collection config, lists jobs for a run attempt, fetches failed-job logs, redacts and clips snippets, and tracks already-reported job ids.
- Create `scripts/ci_logs.py`: operator CLI for one-shot current PR diagnostics. It reuses exact-head/run-selection helpers from `scripts/rust_verification.py` and calls `ci_log_collection`.
- Create `scripts/test_ci_log_collection.py`: unit tests for config validation, job filtering, log availability lag, redaction, clipping, and report-once behavior.
- Create `scripts/test_ci_logs.py`: unit tests for current-PR exact-head CLI behavior and exit-code semantics.
- Modify `ci/github-actions-runners.toml`: add `[ci_log_collection]` values for snippet caps, redaction fragments, API behavior, and output behavior.
- Modify `scripts/rust_verification.py`: import `ci_log_collection`, include `attempt` in workflow-run JSON fields, and call the collector during `wait_for_full_ci_run`.
- Modify `scripts/test_verify_remote.py`: assert `verify-remote` prints a failed-job snippet for an in-progress run with a failed completed shard, and continues/fails according to final workflow state.
- Modify `scripts/test_rust_verification.py`: update harness payloads to include `attempt` and keep existing remote-verification expectations intact.
- Modify `justfile`: add `ci-logs` recipe that calls `scripts/ci_logs.py`.

## Design Decisions

1. Do not build a generic multi-workflow collector.
   The first implementation is full-CI only. The config may name `workflow_key = "ci"` to bind to existing provenance config, but it does not scan all workflow files.

2. Do not make log fetching part of pass/fail proof.
   `verify-remote` still returns based on the workflow run conclusion. Log collection failures are diagnostic warnings, never proof failures.

3. Do not add a second long-running poller.
   `verify-remote` calls the collector inside its existing poll loop. `just ci-logs` is one-shot.

4. Do not persist logs by default.
   Output goes to stderr/stdout as bounded snippets. Full-log files are out of scope for this slice.

5. Do not change cargo/nextest flags.
   Current shard `fail-fast: false` already ensures all four shards run. Snippet extraction should first target existing nextest and Rust failure markers.

---

## Task 1: Add CI Log Collection Config

**Files:**
- Modify: `ci/github-actions-runners.toml`
- Test: `scripts/test_ci_log_collection.py`

- [ ] **Step 1: Write failing config validation tests**

Add tests that require this table and reject unsafe values:

```python
def assert_raises(fragment: str, func) -> None:
    try:
        func()
    except CiLogCollectionError as exc:
        if fragment not in str(exc):
            raise AssertionError(str(exc))
        return
    raise AssertionError(f"expected error containing {fragment!r}")


def assert_load_config_requires_positive_snippet_caps() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            """
            [ci_log_collection]
            schema_version = 1
            workflow_key = "ci"
            failed_jobs_only = true
            snippet_max_lines = 0
            snippet_context_lines = 20
            snippet_max_bytes = 20000
            job_log_max_bytes = 200000
            unavailable_log_notice = "failed job log is not available yet"
            failure_markers = ["panicked at", "error:", "FAIL ["]
            redacted_key_fragments = ["TOKEN", "SECRET", "PASSWORD", "PRIVATE_KEY", "API_KEY"]
            """
        )
        assert_raises("snippet_max_lines", lambda: load_ci_log_collection_config(config))
```

Expected failure before implementation: `NameError` or missing module/function.

- [ ] **Step 2: Add TOML values**

Append to `ci/github-actions-runners.toml`:

```toml
[ci_log_collection]
schema_version = 1
workflow_key = "ci"
failed_jobs_only = true
snippet_max_lines = 120
snippet_context_lines = 20
snippet_max_bytes = 20000
job_log_max_bytes = 200000
unavailable_log_notice = "failed job log is not available yet"
failure_markers = ["panicked at", "error:", "FAIL [", "FAILED", "thread '"]
redacted_key_fragments = ["TOKEN", "SECRET", "PASSWORD", "PRIVATE_KEY", "API_KEY", "ACCESS_KEY", "SESSION_TOKEN"]
```

- [ ] **Step 3: Implement config loader**

In `scripts/ci_log_collection.py`, add:

```python
@dataclasses.dataclass(frozen=True)
class CiLogCollectionConfig:
    workflow_key: str
    failed_jobs_only: bool
    snippet_max_lines: int
    snippet_context_lines: int
    snippet_max_bytes: int
    job_log_max_bytes: int
    unavailable_log_notice: str
    failure_markers: tuple[str, ...]
    redacted_key_fragments: tuple[str, ...]
```

Validation rules:

- `schema_version` must be `1`.
- `workflow_key` must be `ci`.
- boolean fields must be booleans.
- caps must be positive integers.
- `snippet_context_lines <= snippet_max_lines`.
- string arrays must be non-empty.

- [ ] **Step 4: Run tests**

Run:

```bash
python3 scripts/test_ci_log_collection.py
```

Expected: config tests pass.

- [ ] **Step 5: Commit**

```bash
git add ci/github-actions-runners.toml scripts/ci_log_collection.py scripts/test_ci_log_collection.py
git commit -m "test: add ci log collection config"
```

---

## Task 2: Implement Failed-Job Snippet Collection

**Files:**
- Modify: `scripts/ci_log_collection.py`
- Test: `scripts/test_ci_log_collection.py`

- [ ] **Step 1: Write failing tests for failed-job discovery**

Use a fake client with two jobs:

```python
def test_collects_only_completed_failed_jobs():
    client = FakeGhApi(
        jobs=[
            {"id": 11, "name": "nextest shard 1 of 4", "status": "completed", "conclusion": "failure", "html_url": "https://example.invalid/job/11"},
            {"id": 12, "name": "nextest shard 2 of 4", "status": "in_progress", "conclusion": None, "html_url": "https://example.invalid/job/12"},
        ],
        logs={11: "thread 'case' panicked at src/lib.rs:10\nerror: assertion failed\n"},
    )
    report = collect_failed_job_snippets(client, config(), "seungpyoson/bolt-v2", run_id=101, attempt=2, reported_job_ids=set())
    assert [item.job_id for item in report.items] == [11]
    assert "nextest shard 1 of 4" in report.rendered
    assert "panicked at" in report.rendered
```

Expected failure before implementation: collector function missing.

- [ ] **Step 2: Implement the collection API**

Public API:

```python
def collect_failed_job_snippets(
    client: GhApiClient,
    config: CiLogCollectionConfig,
    repo_full_name: str,
    *,
    run_id: int,
    attempt: int,
    reported_job_ids: set[int],
) -> CiLogSnippetReport:
    ...
```

Behavior:

- Calls `GET repos/{owner}/{repo}/actions/runs/{run_id}/attempts/{attempt}/jobs`.
- Uses `per_page` from existing `ci_provenance.api_limits.run_jobs_per_page`.
- Selects jobs where `status == "completed"` and `conclusion == "failure"`.
- Skips ids already in `reported_job_ids`.
- Fetches logs through `GET repos/{owner}/{repo}/actions/jobs/{job_id}/logs`.
- Treats `404`, `410`, empty response, or redirect/log failures as unavailable diagnostics, not fatal proof failures.
- Adds successfully reported failed job ids to `reported_job_ids`.

- [ ] **Step 3: Implement redaction and clipping**

Rules:

- Split log text into lines.
- Redact assignment-like values when the key contains any configured `redacted_key_fragments`.
- Prefer the first configured failure marker and include `snippet_context_lines` around it.
- Fall back to the last `snippet_max_lines` lines.
- Clip final snippet to `snippet_max_bytes`.
- Strip ANSI escape sequences before marker matching and output.

- [ ] **Step 4: Run tests**

Run:

```bash
python3 scripts/test_ci_log_collection.py
```

Expected: snippet, redaction, unavailable-log, and report-once tests pass.

- [ ] **Step 5: Commit**

```bash
git add scripts/ci_log_collection.py scripts/test_ci_log_collection.py
git commit -m "feat: collect failed ci job log snippets"
```

---

## Task 3: Add One-Shot `just ci-logs`

**Files:**
- Create: `scripts/ci_logs.py`
- Create: `scripts/test_ci_logs.py`
- Modify: `justfile`

- [ ] **Step 1: Write failing CLI tests**

Test exit-code contract:

```python
def test_ci_logs_requires_exact_pushed_pr_head(fake_remote):
    result = run_ci_logs(fake_remote, local_head="abc", pr_head="def")
    assert result.exit_code == 2
    assert "does not match local HEAD" in result.stderr
```

Test one-shot success:

```python
def test_ci_logs_prints_failed_job_snippet_for_latest_attempt(fake_remote):
    fake_remote.matching_run(run_id=101, attempt=3, status="in_progress")
    fake_remote.failed_job(job_id=11, name="nextest shard 1 of 4", log="thread 'case' panicked at src/lib.rs:10")
    result = run_ci_logs(fake_remote, local_head="abc", pr_head="abc")
    assert result.exit_code == 0
    assert "workflow run 101 attempt 3" in result.stderr
    assert "nextest shard 1 of 4" in result.stderr
```

- [ ] **Step 2: Implement `scripts/ci_logs.py`**

CLI behavior:

```bash
python3 scripts/ci_logs.py --repo .
```

Semantics:

- Requires a clean worktree, pushed branch, and open/draft PR using the same exact-head checks as `verify-remote`.
- Uses `ci_provenance.workflow_name` to find matching full-CI runs.
- Selects the newest matching run for the current PR head and allowed event set.
- Uses that run's latest `attempt`.
- Runs the collector once.
- Exits `0` if diagnostics were attempted, including no failed completed jobs.
- Exits `2` for auth, tool, PR, config, or malformed API errors.

- [ ] **Step 3: Add just recipe**

Add:

```make
ci-logs: check-workspace require-rust-verification-owner
    python3 scripts/ci_logs.py --repo "{{repo_root}}"
```

- [ ] **Step 4: Run tests**

Run:

```bash
python3 scripts/test_ci_logs.py
python3 scripts/test_ci_log_collection.py
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add justfile scripts/ci_logs.py scripts/test_ci_logs.py scripts/ci_log_collection.py scripts/test_ci_log_collection.py
git commit -m "feat: add ci log diagnostics command"
```

---

## Task 4: Integrate Snippets Into `verify-remote`

**Files:**
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_verify_remote.py`
- Modify: `scripts/test_rust_verification.py`

- [ ] **Step 1: Write failing verify-remote test**

Add a test where a workflow run is still `in_progress`, one job is already completed with `failure`, and another job is still running:

```python
def assert_verify_remote_reports_failed_job_before_run_completes() -> None:
    owner = load_owner_module()
    # Harness returns run 701 as in_progress on first poll and completed failure on the next poll.
    # Fake collector returns one rendered snippet for job 11.
    result, stdout, stderr = run_verify_remote_with_failed_job_harness(owner)
    if "nextest shard 1 of 4" not in stderr:
        raise AssertionError(stderr)
    if "Remote full CI failed" not in stderr:
        raise AssertionError(stderr)
```

Expected failure before implementation: no failed job snippet appears before final run failure.

- [ ] **Step 2: Include run attempt in workflow-run fields**

Change:

```python
WORKFLOW_RUN_FIELDS = "databaseId,event,headSha,status,conclusion,createdAt,url"
```

to:

```python
WORKFLOW_RUN_FIELDS = "attempt,databaseId,event,headSha,status,conclusion,createdAt,url"
```

Add helper:

```python
def run_attempt(run: dict[str, Any]) -> int | None:
    attempt = run.get("attempt")
    if isinstance(attempt, int) and not isinstance(attempt, bool) and attempt > 0:
        return attempt
    if isinstance(attempt, str) and attempt.isdigit() and int(attempt) > 0:
        return int(attempt)
    return None
```

- [ ] **Step 3: Call collector during existing wait loop**

Inside `wait_for_full_ci_run`, after a run is known and before `evaluate_full_ci_run`:

```python
if tracked_run_id is not None and (attempt := run_attempt(run)) is not None:
    report = log_collector.collect(run_id=tracked_run_id, attempt=attempt)
    if report.rendered:
        print(report.rendered, file=sys.stderr)
```

Keep these constraints:

- One collector instance per `wait_for_full_ci_run` call.
- `reported_job_ids` lives inside that instance.
- Collector exceptions render a bounded warning and do not change verify-remote's final exit code.
- The collector runs on the existing poll interval only.

- [ ] **Step 4: Run targeted tests**

Run:

```bash
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
python3 scripts/test_ci_log_collection.py
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add scripts/rust_verification.py scripts/test_verify_remote.py scripts/test_rust_verification.py scripts/ci_log_collection.py scripts/test_ci_log_collection.py
git commit -m "feat: show failed ci job snippets during verify-remote"
```

---

## Task 5: Verification And Review

**Files:**
- Modify only if tests reveal a gap: files from Tasks 1-4.

- [ ] **Step 1: Run non-compile local checks**

Run:

```bash
python3 scripts/test_ci_log_collection.py
python3 scripts/test_ci_logs.py
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
just ci-lint-workflow
```

Expected: all pass. If `just ci-lint-workflow` queues behind the lane lock, wait for completion or report the exact timeout/termination reason.

- [ ] **Step 2: Commit any test-only cleanup**

```bash
git status --short
git add scripts/ci_log_collection.py scripts/ci_logs.py scripts/rust_verification.py scripts/test_ci_log_collection.py scripts/test_ci_logs.py scripts/test_verify_remote.py scripts/test_rust_verification.py ci/github-actions-runners.toml justfile
git commit -m "test: cover ci log diagnostics"
```

- [ ] **Step 3: Push and open draft PR**

```bash
git push -u origin HEAD
gh pr create --draft --fill
```

- [ ] **Step 4: Run remote proof**

Run:

```bash
just verify-remote
```

Expected:

- If CI passes, `verify-remote` reports the exact-head run pass.
- If a shard fails while other jobs continue, `verify-remote` prints a bounded failed-job snippet before the workflow reaches terminal state, then still waits for the terminal run conclusion.

## Self-Review

- Spec coverage: The plan addresses the validated findings by narrowing scope, adding TOML-owned caps, avoiding persistent logs, using existing run selection, handling log unavailability, avoiding duplicate polling, and leaving cargo behavior unchanged.
- Plan-language scan: every task names files, behavior, and commands.
- Type consistency: `CiLogCollectionConfig`, `CiLogSnippetReport`, and `collect_failed_job_snippets` are introduced before use.
