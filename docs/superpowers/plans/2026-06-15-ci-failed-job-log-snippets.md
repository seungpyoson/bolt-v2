# CI Failed Job Log Snippets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface bounded failed-job log snippets for the exact full-CI run before the whole workflow reaches terminal state.

**Architecture:** Keep `verify-remote` as the exact-head proof waiter. Add a focused CI log collection module that inspects failed completed jobs for the current full-CI run attempt and returns sanitized snippets; `verify-remote` calls it opportunistically while polling, and a `rust_verification.py ci-logs` subcommand runs the same collector once for the current exact PR head.

**Tech Stack:** Python 3.12+ stdlib, GitHub CLI `gh` for operator authentication, existing TOML configs in `ci/`, existing `just` recipes, GitHub Actions workflow-job REST behavior.

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
- GitHub API fact: job metadata can be listed by workflow run attempt, and job logs are fetched by job id through a redirecting endpoint whose link expires quickly. The implementation must reuse the safe redirect behavior already present in `scripts/ci_provenance.py` so authorization headers are not forwarded to non-GitHub storage redirects.

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

- Create `scripts/ci_log_collection.py`: pure collection library. It loads log-collection config, lists jobs for a run attempt, fetches failed-job logs through a safe redirect handler, redacts and clips snippets, renders a stable report, and tracks already-reported job ids.
- Create `scripts/test_ci_log_collection.py`: unit tests for config validation, job filtering, log availability lag, redaction, clipping, and report-once behavior.
- Modify `ci/github-actions-runners.toml`: add `[ci_log_collection]` values for snippet caps, redaction fragments, API behavior, and output behavior.
- Modify `scripts/rust_verification.py`: import `ci_log_collection`, include `attempt` in workflow-run JSON fields, add a `ci-logs` subcommand, and call the collector during `wait_for_full_ci_run`.
- Modify `scripts/test_verify_remote.py`: assert `verify-remote` prints a failed-job snippet for an in-progress run with a failed completed shard, assert `ci-logs` enforces current exact PR head, and assert both commands preserve their exit-code contracts.
- Modify `scripts/test_rust_verification.py`: update harness payloads to include `attempt` and keep existing remote-verification expectations intact.
- Modify `justfile`: add `ci-logs` recipe that calls `python3 "{{rust_verification_owner}}" ci-logs --repo "{{repo_root}}"`, and add the new Python self-test to `ci-lint-workflow`.

## Design Decisions

1. Do not build a generic multi-workflow collector.
   The first implementation is full-CI only. It binds to the existing `[ci_provenance]` workflow identity instead of adding a new workflow selector.

2. Do not make log fetching part of pass/fail proof.
   `verify-remote` still returns based on the workflow run conclusion. Log collection failures are diagnostic warnings, never proof failures.

3. Do not add a second long-running poller.
   `verify-remote` calls the collector inside its existing poll loop. `just ci-logs` is one-shot.

4. Do not persist logs by default.
   Output goes to stderr/stdout as bounded snippets. Full-log files are out of scope for this slice.

5. Do not change cargo/nextest flags.
   Current shard `fail-fast: false` already ensures all four shards run. Snippet extraction should first target existing nextest and Rust failure markers.

6. Keep command ownership in `rust_verification.py`.
   `scripts/ci_log_collection.py` owns collection logic. `scripts/rust_verification.py` owns operator-facing `verify-remote` and `ci-logs` command dispatch so recipes keep the existing verifier entry point.

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
            failed_jobs_only = true
            snippet_max_lines = 0
            snippet_context_lines = 20
            snippet_max_bytes = 20000
            job_log_max_bytes = 200000
            api_timeout_seconds = 30
            unavailable_log_notice_interval_attempts = 3
            unavailable_log_notice = "failed job log is not available yet"
            failure_markers = ["panicked at", "error:", "FAIL ["]
            redacted_key_fragments = ["TOKEN", "SECRET", "PASSWORD", "PRIVATE_KEY", "API_KEY"]
            redacted_value_patterns = ['gh[pousr]_[A-Za-z0-9_]{20,}', 'github_pat_[A-Za-z0-9_]{20,}']
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
failed_jobs_only = true
snippet_max_lines = 120
snippet_context_lines = 20
snippet_max_bytes = 20000
job_log_max_bytes = 200000
api_timeout_seconds = 30
unavailable_log_notice_interval_attempts = 3
unavailable_log_notice = "failed job log is not available yet"
failure_markers = [
    "panicked at",
    "error:",
    "FAIL [",
    "FAILED",
    "test result: FAILED",
    "thread '",
    "Caused by:",
]
redacted_key_fragments = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PRIVATE_KEY",
    "API_KEY",
    "ACCESS_KEY",
    "SESSION_TOKEN",
    "AUTHORIZATION",
    "BEARER",
]
redacted_value_patterns = [
    'gh[pousr]_[A-Za-z0-9_]{20,}',
    'github_pat_[A-Za-z0-9_]{20,}',
    'AKIA[0-9A-Z]{16}',
    'ASIA[0-9A-Z]{16}',
    'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}',
    '-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----',
    'Bearer\s+[A-Za-z0-9._~+/=-]{20,}',
    'https?://[^\s/:@]+:[^\s@]+@',
]
```

- [ ] **Step 3: Implement config loader**

In `scripts/ci_log_collection.py`, add:

```python
@dataclasses.dataclass(frozen=True)
class CiLogCollectionConfig:
    failed_jobs_only: bool
    snippet_max_lines: int
    snippet_context_lines: int
    snippet_max_bytes: int
    job_log_max_bytes: int
    api_timeout_seconds: int
    unavailable_log_notice_interval_attempts: int
    unavailable_log_notice: str
    failure_markers: tuple[str, ...]
    redacted_key_fragments: tuple[str, ...]
    redacted_value_patterns: tuple[re.Pattern[str], ...]
```

Validation rules:

- `schema_version` must be `1`.
- boolean fields must be booleans.
- caps must be positive integers.
- `snippet_context_lines <= snippet_max_lines`.
- `unavailable_log_notice_interval_attempts` and `api_timeout_seconds` must be positive integers.
- `failure_markers`, `redacted_key_fragments`, and `redacted_value_patterns` must be non-empty string arrays.
- every `redacted_value_patterns` item must compile with `re.compile`.

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
def assert_collects_only_completed_failed_jobs() -> None:
    client = FakeGhApi(
        jobs=[
            {"id": 11, "name": "nextest shard 1 of 4", "status": "completed", "conclusion": "failure", "html_url": "https://example.invalid/job/11"},
            {"id": 12, "name": "nextest shard 2 of 4", "status": "in_progress", "conclusion": None, "html_url": "https://example.invalid/job/12"},
        ],
        logs={11: "thread 'case' panicked at src/lib.rs:10\nerror: assertion failed\n"},
    )
    report = CiLogCollector(client, config(), "seungpyoson/bolt-v2", run_jobs_per_page=100).collect(run_id=101, attempt=2)
    assert [item.job_id for item in report.items] == [11]
    assert "nextest shard 1 of 4" in report.rendered
    assert "panicked at" in report.rendered


def main() -> int:
    assert_collects_only_completed_failed_jobs()
    assert_redacts_pat_aws_jwt_pem_bearer_and_credentialed_urls()
    assert_unavailable_log_notice_is_throttled_without_stopping_retries()
    assert_reported_jobs_are_not_repeated()
    assert_marker_context_and_snippet_limits_are_stable()
    return 0
```

Expected failure before implementation: collector class missing.

- [ ] **Step 2: Implement the collection API**

Public API:

At the top of `scripts/ci_log_collection.py`, import `dataclasses`, `pathlib`, `re`, `subprocess`, `typing`, `urllib.error`, `urllib.request`, and the local `ci_provenance` module.

```python
class CiLogCollectionError(RuntimeError):
    """Diagnostic CI log collection failed."""


class CiLogUnavailable(CiLogCollectionError):
    """A failed job exists, but GitHub has not made its log downloadable yet."""


@dataclasses.dataclass(frozen=True)
class CiLogSnippetItem:
    job_id: int
    job_name: str
    job_url: str
    snippet: str
    unavailable: bool = False


@dataclasses.dataclass(frozen=True)
class CiLogSnippetReport:
    items: tuple[CiLogSnippetItem, ...]
    rendered: str


class GhActionsClient:
    @classmethod
    def from_gh_cli(cls) -> "GhActionsClient":
        try:
            result = subprocess.run(
                ["gh", "auth", "token"],
                check=True,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        except FileNotFoundError as exc:
            raise CiLogCollectionError("gh is required for CI log collection") from exc
        except subprocess.CalledProcessError as exc:
            raise CiLogCollectionError("gh auth token failed for CI log collection") from exc
        token = result.stdout.strip()
        if not token:
            raise CiLogCollectionError("gh auth token returned an empty token")
        return cls(token)

    def __init__(self, token: str) -> None:
        self._token = token

    def _api_json(self, repo: str, path: str, query: dict[str, str]) -> dict[str, object]:
        try:
            return ci_provenance.github_api_json(repo, self._token, path, query)
        except ci_provenance.ProvenanceError as exc:
            raise CiLogCollectionError(str(exc)) from exc

    def jobs_for_attempt(self, repo: str, run_id: int, attempt: int, *, per_page: int) -> list[dict[str, object]]:
        payload = self._api_json(repo, f"actions/runs/{run_id}/attempts/{attempt}/jobs", {"per_page": str(per_page)})
        jobs = payload.get("jobs")
        if not isinstance(jobs, list):
            raise CiLogCollectionError(f"workflow run {run_id} attempt {attempt} jobs payload is malformed")
        try:
            ci_provenance.require_complete_first_page(payload, jobs, per_page=per_page, label=f"workflow run {run_id} attempt {attempt} jobs")
        except ci_provenance.ProvenanceError as exc:
            raise CiLogCollectionError(str(exc)) from exc
        if not all(isinstance(job, dict) for job in jobs):
            raise CiLogCollectionError(f"workflow run {run_id} attempt {attempt} jobs payload contains malformed jobs")
        return typing.cast(list[dict[str, object]], jobs)

    def job_log_text(self, repo: str, job_id: int, *, timeout: int, max_bytes: int) -> str:
        url = f"https://api.github.com/repos/{repo}/actions/jobs/{job_id}/logs"
        request = urllib.request.Request(url, headers={"Authorization": f"Bearer {self._token}", **ci_provenance.GITHUB_API_HEADERS})
        try:
            with ci_provenance.open_github_api_request(request, timeout=timeout) as response:
                payload = response.read(max_bytes + 1)
        except urllib.error.HTTPError as exc:
            if exc.code in {404, 410}:
                raise CiLogUnavailable(str(exc)) from exc
            raise CiLogCollectionError(f"GitHub job log request failed for job {job_id}: {exc}") from exc
        except urllib.error.URLError as exc:
            raise CiLogUnavailable(str(exc)) from exc
        if len(payload) > max_bytes:
            payload = payload[:max_bytes]
        return payload.decode("utf-8", errors="replace")


class CiLogCollector:
    def __init__(self, client: GhActionsClient, config: CiLogCollectionConfig, repo_full_name: str, *, run_jobs_per_page: int) -> None:
        self.client = client
        self.config = config
        self.repo_full_name = repo_full_name
        self.run_jobs_per_page = run_jobs_per_page
        self.reported_job_ids: set[int] = set()
        self.unavailable_notice_attempts: dict[int, int] = {}
        self.warned_collection_error = False

    def collect(self, *, run_id: int, attempt: int) -> CiLogSnippetReport:
        items: list[CiLogSnippetItem] = []
        jobs = self.client.jobs_for_attempt(
            self.repo_full_name,
            run_id,
            attempt,
            per_page=self.run_jobs_per_page,
        )
        for job in completed_failed_jobs(jobs):
            job_id = require_positive_int(job.get("id"), "job id")
            if job_id in self.reported_job_ids:
                continue
            job_name = require_text(job.get("name"), "job name")
            job_url = optional_text(job.get("html_url"))
            try:
                log_text = self.client.job_log_text(
                    self.repo_full_name,
                    job_id,
                    timeout=self.config.api_timeout_seconds,
                    max_bytes=self.config.job_log_max_bytes,
                )
            except CiLogUnavailable:
                attempts = self.unavailable_notice_attempts.get(job_id, 0) + 1
                self.unavailable_notice_attempts[job_id] = attempts
                if attempts == 1 or attempts % self.config.unavailable_log_notice_interval_attempts == 0:
                    items.append(CiLogSnippetItem(job_id, job_name, job_url, self.config.unavailable_log_notice, unavailable=True))
                continue
            snippet = sanitize_and_clip_snippet(log_text, self.config)
            items.append(CiLogSnippetItem(job_id, job_name, job_url, snippet))
            self.reported_job_ids.add(job_id)
        return render_snippet_report(run_id, attempt, tuple(items))
```

Implement `completed_failed_jobs`, `require_positive_int`, `require_text`, `optional_text`, `sanitize_and_clip_snippet`, and `render_snippet_report` in `scripts/ci_log_collection.py`; keep them module-private except where tests need direct coverage of redaction and clipping.

Behavior:

- Calls `GET repos/{owner}/{repo}/actions/runs/{run_id}/attempts/{attempt}/jobs`.
- Uses `per_page` from existing `ci_provenance.api_limits.run_jobs_per_page`.
- Uses `ci_provenance.require_complete_first_page` so pagination saturation or malformed counts fail closed instead of silently hiding failed jobs.
- Selects jobs where `status == "completed"` and `conclusion == "failure"`.
- Skips ids already in `reported_job_ids`.
- Fetches logs through `GET repos/{owner}/{repo}/actions/jobs/{job_id}/logs`.
- Reuses `ci_provenance.open_github_api_request` so `Authorization` and other sensitive headers are stripped on cross-host log redirects.
- Treats `404`, `410`, empty response, or redirect/log failures as unavailable diagnostics, not fatal proof failures.
- Retries unavailable failed-job logs on every poll until the job is reported, the workflow reaches terminal state, or `verify-remote` times out.
- Throttles repeated unavailable-log notices with `unavailable_log_notice_interval_attempts` so stderr is bounded without giving up on later log availability.
- Adds successfully reported failed job ids to `reported_job_ids`.

- [ ] **Step 3: Implement stable rendering, redaction, and clipping**

Render one block per job. Use this exact format:

````text
CI failed job log snippet
workflow_run=<run_id> attempt=<attempt> job_id=<job_id>
job=<job_name>
url=<html_url>
```text
<sanitized snippet text>
```
````

Rules:

- Split log text into lines.
- Redact assignment-like values when the key contains any configured `redacted_key_fragments`.
- Redact high-confidence secret values using `redacted_value_patterns`, including GitHub PATs, GitHub fine-grained PATs, AWS access key ids, JWTs, PEM private key blocks, bearer tokens, and credentialed URLs.
- After redaction, run a safety scan with the same high-confidence patterns; if any still match, emit only `snippet suppressed: unsafe log content matched redaction safety check`.
- Process `failure_markers` in config order. For the first marker that matches, choose the earliest matching line for that marker.
- `snippet_context_lines` means lines before and after the marker, so the raw marker window is at most `2 * snippet_context_lines + 1` lines before byte clipping.
- Fall back to the last `snippet_max_lines` lines.
- Clip final snippet to `snippet_max_bytes`.
- Strip ANSI escape sequences before marker matching and output.
- If marker context produces more than `snippet_max_lines`, clamp the window to `snippet_max_lines` while keeping the marker line inside the retained range.

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

## Task 3: Add One-Shot `rust_verification.py ci-logs`

**Files:**
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_verify_remote.py`
- Modify: `scripts/test_rust_verification.py`
- Modify: `justfile`

- [ ] **Step 1: Write failing CLI tests**

Test exit-code contract:

```python
def assert_ci_logs_requires_exact_pushed_pr_head() -> None:
    owner = load_owner_module()
    result, _stdout, stderr = run_ci_logs_harness(owner, local_head="abc", pr_head="def")
    if result != 2:
        raise AssertionError(result)
    if "does not match local HEAD" not in stderr:
        raise AssertionError(stderr)
```

Test one-shot success:

```python
def assert_ci_logs_prints_failed_job_snippet_for_latest_attempt() -> None:
    owner = load_owner_module()
    result, _stdout, stderr = run_ci_logs_harness(
        owner,
        local_head="abc",
        pr_head="abc",
        run={"databaseId": 101, "attempt": 3, "status": "in_progress", "conclusion": None},
        failed_job={"id": 11, "name": "nextest shard 1 of 4", "log": "thread 'case' panicked at src/lib.rs:10"},
    )
    if result != 0:
        raise AssertionError(result)
    if "workflow_run=101 attempt=3 job_id=11" not in stderr:
        raise AssertionError(stderr)
    if "nextest shard 1 of 4" not in stderr:
        raise AssertionError(stderr)
```

Add both new assertions to the explicit runner in `scripts/test_verify_remote.py` before the final `print`:

```python
def main() -> int:
    assert_verify_remote_precondition_errors()
    assert_verify_remote_pr_errors()
    assert_pr_lookup_preserves_gh_errors()
    assert_pr_checks_allows_pending_exit_code_with_json()
    assert_verify_remote_waits_then_passes()
    assert_verify_remote_uses_latest_full_run_over_stale_deferred_run()
    assert_verify_remote_rejects_branch_advance_during_watch()
    assert_verify_remote_reports_failing_full_ci_run()
    assert_verify_remote_rechecks_head_before_reporting_failed_run()
    assert_verify_remote_run_list_api_error_fails_closed()
    assert_verify_remote_no_matching_run_times_out()
    assert_verify_remote_rechecks_head_before_no_matching_run_timeout()
    assert_verify_remote_rechecks_head_before_overall_timeout()
    assert_ci_logs_requires_exact_pushed_pr_head()
    assert_ci_logs_prints_failed_job_snippet_for_latest_attempt()
    print("OK: remote verification watcher self-tests passed.")
    return 0
```

- [ ] **Step 2: Implement the `ci-logs` subcommand in `scripts/rust_verification.py`**

CLI behavior:

```bash
python3 scripts/rust_verification.py ci-logs --repo .
```

Semantics:

- Requires a clean worktree, pushed branch, and open/draft PR using the same exact-head checks as `verify-remote`.
- Uses `ci_provenance.workflow_name` and `ci_provenance.workflow_path` to find matching full-CI runs.
- Selects the newest matching run for the current PR head and allowed event set.
- Uses that run's latest `attempt`.
- Extends `ci_provenance_dispatch_config(repo)` to validate and return `ci_provenance.api_limits.run_jobs_per_page`, then constructs `GhActionsClient.from_gh_cli()`, loads `CiLogCollectionConfig` from `repo / CI_RUNNERS_RELATIVE_PATH`, passes `dispatch_config["run_jobs_per_page"]` into `CiLogCollector`, and runs the collector once.
- Exits `0` if diagnostics were attempted, including no failed completed jobs.
- Exits `2` for auth, tool, PR, config, or malformed API errors.

- [ ] **Step 3: Add just recipe**

Add:

```make
ci-logs: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" ci-logs --repo "{{repo_root}}"
```

- [ ] **Step 4: Wire the new self-test into `ci-lint-workflow`**

Inside the existing `ci-lint-workflow` bash recipe, after `scripts/test_ci_provenance.py` and before `scripts/test_find_same_sha_main_evidence.py`, add:

```bash
if ! python3 scripts/test_ci_log_collection.py; then
    failed=1
fi
```

- [ ] **Step 5: Run tests**

Run:

```bash
python3 scripts/test_ci_log_collection.py
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add justfile scripts/rust_verification.py scripts/test_verify_remote.py scripts/test_rust_verification.py scripts/ci_log_collection.py scripts/test_ci_log_collection.py
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
    snippet_index = stderr.find("nextest shard 1 of 4")
    failure_index = stderr.find("Remote full CI failed")
    if snippet_index == -1:
        raise AssertionError(stderr)
    if failure_index == -1:
        raise AssertionError(stderr)
    if snippet_index > failure_index:
        raise AssertionError(stderr)
```

Add this new assertion to the explicit runner in `scripts/test_verify_remote.py` before the final `print`, after `assert_verify_remote_rechecks_head_before_overall_timeout()`.

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

- [ ] **Step 3: Pass explicit log-collection context into the wait loop**

Extend `ci_provenance_dispatch_config(repo)` so it validates and returns `run_jobs_per_page` from `[ci_provenance.api_limits]` alongside the existing workflow fields:

```python
raw_job_limit = api_limits.get("run_jobs_per_page") if isinstance(api_limits, dict) else None
if raw_job_limit is None or not isinstance(raw_job_limit, int) or isinstance(raw_job_limit, bool) or raw_job_limit <= 0:
    return None, "ci_provenance.api_limits.run_jobs_per_page must be a positive integer"

return {
    "workflow_name": workflow_name,
    "workflow_path": workflow_path,
    "workflow_input": workflow_input,
    "workflow_runs_per_page": run_limit,
    "run_jobs_per_page": raw_job_limit,
}, None
```

Update `write_verify_remote_config` in `scripts/test_rust_verification.py` so existing remote-verification harness repos include the required page-size table:

```toml
[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
```

Add this assertion to `scripts/test_rust_verification.py` and call it from `main()` before `assert_verify_remote_dispatches_draft_full_ci_and_waits_run_scoped()`:

```python
def assert_verify_remote_config_includes_run_jobs_page_size() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        config, error = owner.ci_provenance_dispatch_config(repo)
        if error is not None or config is None:
            raise AssertionError(error)
        if config["run_jobs_per_page"] != 100:
            raise AssertionError(config)
```

In `cmd_verify_remote`, compute the repository name once and pass it into every `wait_for_full_ci_run` call:

```python
identity, error = repository_identity(repo)
if error is not None or identity is None:
    return verify_remote_fail(error or "unable to inspect repository identity")
repo_full_name = f"{identity[0]}/{identity[1]}"
```

Add these parameters to `wait_for_full_ci_run`:

```python
    repo_full_name: str,
    run_jobs_per_page: int,
```

Each call site passes:

```python
repo_full_name=repo_full_name,
run_jobs_per_page=dispatch_config["run_jobs_per_page"],
```

- [ ] **Step 4: Call collector during existing wait loop**

Add a helper in `scripts/rust_verification.py`:

```python
def build_ci_log_collector(
    *,
    repo: pathlib.Path,
    repo_full_name: str,
    run_jobs_per_page: int,
) -> ci_log_collection.CiLogCollector:
    return ci_log_collection.CiLogCollector(
        ci_log_collection.GhActionsClient.from_gh_cli(),
        ci_log_collection.load_ci_log_collection_config(repo / CI_RUNNERS_RELATIVE_PATH),
        repo_full_name,
        run_jobs_per_page=run_jobs_per_page,
    )
```

Inside `wait_for_full_ci_run`, create one collector only after `tracked_run_id` is set for the current exact-head full-CI run. Then call it inside the existing poll loop before `evaluate_full_ci_run`:

```python
log_collector: ci_log_collection.CiLogCollector | None = None
log_collector_disabled = False

if tracked_run_id is not None and (attempt := run_attempt(run)) is not None and not log_collector_disabled:
    if log_collector is None:
        try:
            log_collector = build_ci_log_collector(
                repo=repo,
                repo_full_name=repo_full_name,
                run_jobs_per_page=run_jobs_per_page,
            )
        except ci_log_collection.CiLogCollectionError as exc:
            log_collector_disabled = True
            print(f"CI log snippets unavailable: {exc}", file=sys.stderr)
    if log_collector is not None:
        try:
            report = log_collector.collect(run_id=tracked_run_id, attempt=attempt)
        except ci_log_collection.CiLogCollectionError as exc:
            if not log_collector.warned_collection_error:
                log_collector.warned_collection_error = True
                print(f"CI log snippets unavailable: {exc}", file=sys.stderr)
        else:
            if report.rendered:
                print(report.rendered, file=sys.stderr)
```

Keep these constraints:

- One collector instance per `wait_for_full_ci_run` call.
- `reported_job_ids` lives inside that instance.
- Collector exceptions render at most one bounded warning per wait call and do not change verify-remote's final exit code.
- Collector construction failures render one bounded warning and disable further log collection for that wait call.
- The collector runs on the existing poll interval only.
- Collection starts only after `tracked_run_id` is known, so no logs are fetched from stale deferred-gate runs or wrong-head runs.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
python3 scripts/test_ci_log_collection.py
```

Expected: pass.

- [ ] **Step 6: Commit**

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
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
just ci-lint-workflow
```

Expected: all pass. If `just ci-lint-workflow` queues behind the lane lock, wait for completion or report the exact timeout/termination reason.

- [ ] **Step 2: Commit any test-only cleanup**

```bash
git status --short
git add scripts/ci_log_collection.py scripts/rust_verification.py scripts/test_ci_log_collection.py scripts/test_verify_remote.py scripts/test_rust_verification.py ci/github-actions-runners.toml justfile
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
- Review coverage: the plan uses repo-native Python self-tests wired into `ci-lint-workflow`, keeps command dispatch in `scripts/rust_verification.py`, reuses safe GitHub redirect behavior, removes the redundant workflow selector, defines output format and marker semantics, broadens redaction, retries unavailable logs until terminal/timeout while throttling notices, passes verifier data explicitly, and starts polling only after `tracked_run_id` is known.
- Plan-language scan: every task names files, behavior, and commands.
- Type consistency: `CiLogCollectionConfig`, `GhActionsClient`, `CiLogCollector`, and `CiLogSnippetReport` are introduced before use.
