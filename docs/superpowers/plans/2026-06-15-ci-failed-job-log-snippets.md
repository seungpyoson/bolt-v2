# CI Failed Job Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** While `verify-remote` is waiting on the exact full-CI run for the current PR head, print useful best-effort diagnostics for completed failed jobs before the whole workflow finishes.

**Architecture:** Keep diagnostics inside `scripts/rust_verification.py`. Reuse the existing exact-head run tracking; once a run id and attempt are known, poll completed failed jobs for that run, fetch `gh run view --job <job-id> --log-failed`, print a bounded excerpt when available, and keep retrying unavailable logs without changing `verify-remote` pass/fail behavior.

**Tech Stack:** Python 3.12+ stdlib, GitHub CLI `gh`, existing `ci/rust-verification.toml`, existing `scripts/test_verify_remote.py` and `scripts/test_rust_verification.py` self-test style, existing `just` recipes.

---

## Scope

This is a middle-path diagnostics slice.

In scope:

- Exact current PR head only.
- Full-CI workflow run already selected by `verify-remote`.
- Completed failed jobs only.
- Best-effort failed-step log excerpts.
- Retry when GitHub knows the job failed but has not exposed the log yet.
- A one-shot `ci-logs` command using the same diagnostics path.

Out of scope:

- Guaranteed failure summaries through new CI artifacts.
- Workflow YAML changes.
- A separate log-ingestion module.
- Enterprise-grade secret scanning.
- Persisting full logs.
- Changing whether `verify-remote` passes or fails.

## File Structure

- Modify `ci/rust-verification.toml`: add small diagnostics caps under `[remote_verification]`.
- Modify `scripts/rust_verification.py`: add run-attempt parsing, failed-job diagnostics state, `gh run view --job ... --log-failed` fetching, and `ci-logs` dispatch.
- Modify `scripts/test_verify_remote.py`: cover diagnostics during an in-progress tracked run, unavailable-log retry behavior, and no duplicate printed logs.
- Modify `scripts/test_rust_verification.py`: cover policy parsing, command dispatch, and `ci-logs` exact-head behavior.
- Modify `justfile`: add `ci-logs` recipe that calls `scripts/rust_verification.py ci-logs`.

## Design Decisions

1. Do not promise early logs.
   GitHub can report a failed job before it exposes that job's log. The tool should say "log not available yet" and retry.

2. Keep the implementation local to `scripts/rust_verification.py`.
   This is diagnostic behavior for the remote verifier, not a reusable log system.

3. Use `gh run view --job <job-id> --log-failed`.
   This delegates GitHub's job-log fallback behavior to `gh` and avoids hand-rolling redirect handling.

4. Keep redaction simple.
   Clip output by line and byte caps from TOML, strip ANSI escapes, and mask obvious assignment-style secrets such as `TOKEN=...`, `PASSWORD=...`, `SECRET=...`, `API_KEY=...`, and `Authorization: Bearer ...`.

5. Keep state per wait loop.
   Each `verify-remote` wait loop remembers which failed jobs already printed logs and which unavailable notices were recently shown. Nothing is persisted.

---

## Task 1: Add Remote Diagnostics Policy

**Files:**
- Modify: `ci/rust-verification.toml`
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_verify_remote.py`
- Modify: `scripts/test_rust_verification.py`

- [ ] **Step 1: Write failing policy parsing test**

Add this test to `scripts/test_rust_verification.py` and call it from `main()` after `assert_oversized_policy_fails_closed()`:

```python
def assert_remote_diagnostics_policy_loads() -> None:
    owner = load_owner_module()
    policy = {
        "remote_verification": {
            "poll_interval_seconds": 15,
            "checks_appear_timeout_seconds": 300,
            "overall_timeout_seconds": 3600,
            "diagnostic_log_max_lines": 160,
            "diagnostic_log_max_bytes": 20000,
            "diagnostic_unavailable_notice_interval_polls": 4,
        }
    }
    loaded = owner.remote_verification_policy(policy)
    if loaded["diagnostic_log_max_lines"] != 160:
        raise AssertionError(loaded)
    if loaded["diagnostic_log_max_bytes"] != 20000:
        raise AssertionError(loaded)
    if loaded["diagnostic_unavailable_notice_interval_polls"] != 4:
        raise AssertionError(loaded)
```

Expected before implementation: `KeyError` or missing returned policy values.

- [ ] **Step 2: Add TOML values**

In `ci/rust-verification.toml`, extend `[remote_verification]`:

```toml
diagnostic_log_max_lines = 160
diagnostic_log_max_bytes = 20000
diagnostic_unavailable_notice_interval_polls = 4
```

- [ ] **Step 3: Extend policy validation and loading**

In `scripts/rust_verification.py`, add `import dataclasses` and extend `validate_remote_verification_policy` to require the three new positive integer fields:

```python
for key in (
    "poll_interval_seconds",
    "checks_appear_timeout_seconds",
    "overall_timeout_seconds",
    "diagnostic_log_max_lines",
    "diagnostic_log_max_bytes",
    "diagnostic_unavailable_notice_interval_polls",
):
    value = policy.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise PolicyError(f"remote_verification.{key} must be a positive integer")
    values[key] = value
```

Return the new fields from `remote_verification_policy`:

```python
return {
    "poll_interval_seconds": int(raw["poll_interval_seconds"]),
    "checks_appear_timeout_seconds": int(raw["checks_appear_timeout_seconds"]),
    "overall_timeout_seconds": int(raw["overall_timeout_seconds"]),
    "diagnostic_log_max_lines": int(raw["diagnostic_log_max_lines"]),
    "diagnostic_log_max_bytes": int(raw["diagnostic_log_max_bytes"]),
    "diagnostic_unavailable_notice_interval_polls": int(raw["diagnostic_unavailable_notice_interval_polls"]),
}
```

Update `write_policy` in `scripts/test_verify_remote.py` so the generated `[remote_verification]` table includes:

```toml
diagnostic_log_max_lines = 160
diagnostic_log_max_bytes = 20000
diagnostic_unavailable_notice_interval_polls = 4
```

Update `VerifyRemoteHarness.fake_load_policy` in `scripts/test_rust_verification.py` so its returned `remote_verification` dict includes:

```python
"diagnostic_log_max_lines": 160,
"diagnostic_log_max_bytes": 20000,
"diagnostic_unavailable_notice_interval_polls": 4,
```

- [ ] **Step 4: Run test**

Run:

```bash
python3 scripts/test_rust_verification.py
python3 scripts/test_verify_remote.py
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add ci/rust-verification.toml scripts/rust_verification.py scripts/test_verify_remote.py scripts/test_rust_verification.py
git commit -m "feat: add remote diagnostic policy"
```

---

## Task 2: Add Failed Job Diagnostics Helpers

**Files:**
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_verify_remote.py`

- [ ] **Step 1: Write failing helper tests**

Add these tests to `scripts/test_verify_remote.py` and call them from `main()` before the final `print`:

```python
def assert_diagnostic_excerpt_is_bounded_and_masked() -> None:
    owner = load_owner_module()
    text = "\x1b[31mline0\x1b[0m\nTOKEN=abc123\nAuthorization: Bearer secretvalue\nline3\nline4\n"
    excerpt = owner.diagnostic_log_excerpt(
        text,
        max_lines=3,
        max_bytes=200,
    )
    if "\x1b" in excerpt:
        raise AssertionError(excerpt)
    if "abc123" in excerpt or "secretvalue" in excerpt:
        raise AssertionError(excerpt)
    if len(excerpt.splitlines()) > 3:
        raise AssertionError(excerpt)


def assert_run_attempt_accepts_positive_ints_only() -> None:
    owner = load_owner_module()
    if owner.run_attempt({"attempt": 2}) != 2:
        raise AssertionError("integer attempt rejected")
    if owner.run_attempt({"attempt": "3"}) != 3:
        raise AssertionError("string attempt rejected")
    if owner.run_attempt({"attempt": True}) is not None:
        raise AssertionError("boolean attempt accepted")
    if owner.run_attempt({"attempt": 0}) is not None:
        raise AssertionError("zero attempt accepted")
```

Expected before implementation: helper functions are missing.

- [ ] **Step 2: Implement small data types and helpers**

In `scripts/rust_verification.py`, add `cast` to the existing `typing` import, then add:

```python
@dataclasses.dataclass
class RemoteFailureDiagnosticsState:
    reported_job_ids: set[int] = dataclasses.field(default_factory=set)
    unavailable_notice_polls: dict[int, int] = dataclasses.field(default_factory=dict)


def run_attempt(run: dict[str, Any]) -> int | None:
    attempt = run.get("attempt")
    if isinstance(attempt, int) and not isinstance(attempt, bool) and attempt > 0:
        return attempt
    if isinstance(attempt, str) and attempt.isdecimal() and int(attempt) > 0:
        return int(attempt)
    return None
```

Add log cleanup helpers:

```python
ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
SECRET_ASSIGNMENT_RE = re.compile(
    r"(?i)\b(TOKEN|SECRET|PASSWORD|API_KEY|ACCESS_KEY|SESSION_TOKEN)\b\s*[:=]\s*\S+"
)
BEARER_RE = re.compile(r"(?i)\bAuthorization\s*:\s*Bearer\s+\S+")


def mask_obvious_secrets(line: str) -> str:
    line = SECRET_ASSIGNMENT_RE.sub(lambda match: f"{match.group(1)}=<redacted>", line)
    return BEARER_RE.sub("Authorization: Bearer <redacted>", line)


def diagnostic_log_excerpt(text: str, *, max_lines: int, max_bytes: int) -> str:
    cleaned = ANSI_ESCAPE_RE.sub("", text)
    lines = [mask_obvious_secrets(line) for line in cleaned.splitlines()]
    excerpt = "\n".join(lines[-max_lines:])
    encoded = excerpt.encode("utf-8")
    if len(encoded) > max_bytes:
        excerpt = encoded[-max_bytes:].decode("utf-8", errors="replace")
    return excerpt.strip()
```

- [ ] **Step 3: Run test**

Run:

```bash
python3 scripts/test_verify_remote.py
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add scripts/rust_verification.py scripts/test_verify_remote.py
git commit -m "feat: add remote failure diagnostic helpers"
```

---

## Task 3: Emit Diagnostics During `verify-remote`

**Files:**
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_verify_remote.py`
- Modify: `scripts/test_rust_verification.py`

- [ ] **Step 1: Write failing in-progress diagnostics test**

Add this test to `scripts/test_verify_remote.py` and call it from `main()` before the final `print`:

```python
def assert_verify_remote_reports_failed_job_while_run_is_in_progress() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)

        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_run_view = owner.workflow_run_view
        original_emit = owner.emit_failed_job_diagnostics
        original_sleep = owner.time.sleep

        emitted_states: list[str] = []

        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (
                [
                    {
                        "databaseId": 101,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "in_progress",
                        "conclusion": None,
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/run",
                    }
                ],
                None,
            )
            views = iter(
                [
                    {
                        "databaseId": 101,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "in_progress",
                        "conclusion": None,
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/run",
                    },
                    {
                        "databaseId": 101,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "completed",
                        "conclusion": "failure",
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/run",
                    },
                ]
            )
            owner.workflow_run_view = lambda _repo, _run_id: (next(views), None)

            def fake_emit_failed_job_diagnostics(*, run: dict[str, object], **_kwargs: object) -> None:
                emitted_states.append(str(run["status"]))
                print("CI failed job: nextest shard 1 of 4", file=sys.stderr)

            owner.emit_failed_job_diagnostics = fake_emit_failed_job_diagnostics
            owner.time.sleep = lambda _seconds: None

            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_view = original_run_view
            owner.emit_failed_job_diagnostics = original_emit
            owner.time.sleep = original_sleep

    if "in_progress" not in emitted_states:
        raise AssertionError(emitted_states)
    if "CI failed job: nextest shard 1 of 4" not in output:
        raise AssertionError(output)
    if result != 1:
        raise AssertionError((result, output))
```

Expected before implementation: `emit_failed_job_diagnostics` is missing or is never called while the run is in progress.

- [ ] **Step 2: Add job listing and log fetch functions**

In `scripts/rust_verification.py`, add:

```python
def workflow_run_jobs(repo: pathlib.Path, run_id: int, attempt: int) -> tuple[list[dict[str, Any]] | None, str | None]:
    payload, error = load_json_command(
        ["gh", "run", "view", str(run_id), "--attempt", str(attempt), "--json", "jobs"],
        repo=repo,
    )
    if error is not None or not isinstance(payload, dict):
        return None, error or f"verify-remote could not inspect workflow run {run_id} jobs"
    jobs = payload.get("jobs")
    if not isinstance(jobs, list) or not all(isinstance(job, dict) for job in jobs):
        return None, f"verify-remote received malformed jobs for workflow run {run_id}"
    return cast(list[dict[str, Any]], jobs), None


def job_log_failed(repo: pathlib.Path, job_id: int) -> tuple[str | None, str | None]:
    argv = ["gh", "run", "view", "--job", str(job_id), "--log-failed"]
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return None, "gh is required for remote failure diagnostics"
    if result.returncode != 0:
        return None, command_error(argv, result)
    if not result.stdout.strip():
        return None, "failed job log is not available yet"
    return result.stdout, None
```

- [ ] **Step 3: Add emitter**

Add:

```python
def job_id(job: dict[str, Any]) -> int | None:
    value = job.get("databaseId", job.get("id"))
    if isinstance(value, int) and not isinstance(value, bool) and value > 0:
        return value
    if isinstance(value, str) and value.isdecimal() and int(value) > 0:
        return int(value)
    return None


def job_text(job: dict[str, Any], key: str) -> str:
    value = job.get(key)
    return value if isinstance(value, str) else ""


def emit_failed_job_diagnostics(
    *,
    repo: pathlib.Path,
    run: dict[str, Any],
    state: RemoteFailureDiagnosticsState,
    remote_policy: dict[str, int],
) -> None:
    run_id = run_database_id(run)
    attempt = run_attempt(run)
    if run_id is None or attempt is None:
        return
    jobs, error = workflow_run_jobs(repo, run_id, attempt)
    if error is not None or jobs is None:
        print(f"CI failed-job diagnostics unavailable: {error}", file=sys.stderr)
        return
    for job in jobs:
        if job_text(job, "status") != "completed" or job_text(job, "conclusion") != "failure":
            continue
        current_job_id = job_id(job)
        if current_job_id is None or current_job_id in state.reported_job_ids:
            continue
        name = job_text(job, "name") or f"job {current_job_id}"
        url = job_text(job, "url") or job_text(job, "html_url")
        log_text, log_error = job_log_failed(repo, current_job_id)
        if log_text is None:
            notices = state.unavailable_notice_polls.get(current_job_id, 0) + 1
            state.unavailable_notice_polls[current_job_id] = notices
            if notices == 1 or notices % remote_policy["diagnostic_unavailable_notice_interval_polls"] == 0:
                print(f"CI failed job: {name}", file=sys.stderr)
                if url:
                    print(f"job_url={url}", file=sys.stderr)
                print(f"job_log=unavailable yet: {log_error}", file=sys.stderr)
            continue
        print(f"CI failed job: {name}", file=sys.stderr)
        if url:
            print(f"job_url={url}", file=sys.stderr)
        excerpt = diagnostic_log_excerpt(
            log_text,
            max_lines=remote_policy["diagnostic_log_max_lines"],
            max_bytes=remote_policy["diagnostic_log_max_bytes"],
        )
        if excerpt:
            print("failed_log_excerpt:", file=sys.stderr)
            print(excerpt, file=sys.stderr)
        state.reported_job_ids.add(current_job_id)
```

- [ ] **Step 4: Call emitter from the wait loop**

Inside `wait_for_full_ci_run`, create state once:

```python
diagnostics_state = RemoteFailureDiagnosticsState()
```

After `run` is known and before `evaluate_full_ci_run(run, ...)`, call:

```python
emit_failed_job_diagnostics(
    repo=repo,
    run=run,
    state=diagnostics_state,
    remote_policy=remote_policy,
)
```

Keep this call diagnostic-only. It must not alter `evaluate_full_ci_run` or any head recheck.

- [ ] **Step 5: Update run payloads with attempt**

In `scripts/test_rust_verification.py`, add `"attempt": 1` to the `workflow_run(...)` helper return value so existing remote-verification harness payloads include the field.

- [ ] **Step 6: Run tests**

Run:

```bash
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add scripts/rust_verification.py scripts/test_verify_remote.py scripts/test_rust_verification.py
git commit -m "feat: show failed ci job diagnostics while waiting"
```

---

## Task 4: Add One-Shot `ci-logs`

**Files:**
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_rust_verification.py`
- Modify: `justfile`

- [ ] **Step 1: Write failing command dispatch test**

Add this test to `scripts/test_rust_verification.py` and call it from `main()` after `assert_verify_remote_preflight_rejects_dirty_or_unpushed_head_before_ci()`:

```python
def assert_ci_logs_command_uses_exact_head_run() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        write_verify_remote_config(repo)
        harness = VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[[workflow_run(301, status="in_progress", conclusion=None)]],
        )
        emitted: list[int] = []
        original_emit = owner.emit_failed_job_diagnostics
        try:
            with harness:
                owner.emit_failed_job_diagnostics = lambda **kwargs: emitted.append(int(kwargs["run"]["databaseId"]))
                args = type("Args", (), {"repo": str(repo)})()
                result = owner.cmd_ci_logs(args)
        finally:
            owner.emit_failed_job_diagnostics = original_emit
        if result != 0:
            raise AssertionError(result)
        if emitted != [301]:
            raise AssertionError(emitted)
```

Expected before implementation: `cmd_ci_logs` or parser entry is missing.

- [ ] **Step 2: Implement `cmd_ci_logs`**

Add a command that reuses `verify-remote` preconditions and PR lookup, but does not dispatch CI and does not wait:

```python
def cmd_ci_logs(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
        remote_policy = remote_verification_policy(policy)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        return verify_remote_fail(str(exc))
    head, branch, error = ensure_verify_remote_preconditions(repo)
    if error is not None or head is None or branch is None:
        return verify_remote_fail(error or "unable to inspect git state")
    pr, error = pr_for_exact_head(repo, branch, head, during_watch=False)
    if error is not None or pr is None:
        return verify_remote_fail(error or "unable to inspect pull request")
    dispatch_config, error = ci_provenance_dispatch_config(repo)
    if error is not None or dispatch_config is None:
        return verify_remote_fail(error or "unable to inspect CI dispatch config")
    events = FULL_CI_DRAFT_EVENTS if bool(pr.get("isDraft")) else FULL_CI_READY_EVENTS
    runs, error = workflow_run_list(repo, dispatch_config, branch)
    if error is not None or runs is None:
        return verify_remote_fail(error or "unable to inspect workflow runs")
    matching = matching_full_ci_runs(runs, head=head, events=events)
    if not matching:
        return verify_remote_fail(f"no matching full-CI workflow run found for {head}")
    emit_failed_job_diagnostics(
        repo=repo,
        run=matching[0],
        state=RemoteFailureDiagnosticsState(),
        remote_policy=remote_policy,
    )
    return 0
```

- [ ] **Step 3: Add parser and Just recipe**

In `build_parser`, add:

```python
ci_logs = subparsers.add_parser("ci-logs")
ci_logs.add_argument("--repo", required=True)
ci_logs.set_defaults(func=cmd_ci_logs)
```

In `justfile`, add:

```make
ci-logs: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" ci-logs --repo "{{repo_root}}"
```

- [ ] **Step 4: Run tests**

Run:

```bash
python3 scripts/test_rust_verification.py
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add scripts/rust_verification.py scripts/test_rust_verification.py justfile
git commit -m "feat: add ci log diagnostics command"
```

---

## Task 5: Verification

**Files:**
- Modify only if checks reveal a gap: files from Tasks 1-4.

- [ ] **Step 1: Run local non-compile checks**

Run:

```bash
python3 scripts/test_verify_remote.py
python3 scripts/test_rust_verification.py
just ci-lint-workflow
```

Expected: pass. If `just ci-lint-workflow` queues behind the lane lock or times out, report the exact result.

- [ ] **Step 2: Commit cleanup if needed**

```bash
git status --short
git add ci/rust-verification.toml scripts/rust_verification.py scripts/test_verify_remote.py scripts/test_rust_verification.py justfile
git commit -m "test: cover ci failure diagnostics"
```

- [ ] **Step 3: Remote proof**

After implementation commits are pushed to a draft/open PR, run:

```bash
just verify-remote
```

Expected:

- If CI passes, `verify-remote` behavior is unchanged.
- If a job fails while the workflow is still running, `verify-remote` prints the failed job name and URL.
- If `gh` can fetch failed logs, `verify-remote` prints a bounded failed-log excerpt.
- If GitHub has not exposed logs yet, `verify-remote` prints an unavailable notice and retries on later polls.

## Internal Adversarial Review

Findings from review of this plan:

- **No high findings.**
- **Medium:** `gh run view --job <job-id> --log-failed` behavior can vary when GitHub cannot map logs to failed steps. The plan accepts this by treating nonzero or empty output as unavailable and retrying; it does not guarantee early logs.
- **Medium:** `ci-logs` does not dispatch CI. This is intentional. It is a one-shot diagnostics command for an already existing exact-head run.
- **Low:** The simple secret masking is not comprehensive. This matches the personal-project scope and still avoids obvious credential display.
- **Resolved during review:** required remote policy fixture updates are now part of Task 1, and unavailable polls no longer print a duplicate failed-job header when the unavailable notice is throttled.

Review conclusion: **APPROVE the plan for implementation** as a best-effort diagnostics slice. It is smaller than the previous plan, keeps exact-head safety, avoids a new module, and has tests for the main failure modes.

## Self-Review

- Scope: focused on best-effort GitHub job diagnostics only.
- Placeholder scan: no open placeholders remain; helper names introduced in tests have corresponding implementation steps or are existing local harness helpers.
- Type consistency: helper and command names are introduced before use.
- Repo rules: runtime caps live in TOML; no local Rust compile checks are required for this Python/tooling slice.
