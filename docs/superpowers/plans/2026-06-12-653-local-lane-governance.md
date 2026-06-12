# #653 Local Lane Governance Implementation Plan (Part 1/2: Tasks 1–5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Tasks 6–10 and the plan self-review are in `2026-06-12-653-local-lane-governance-part2.md`.

**Goal:** At most one CPU-heavy repo verifier script runs locally per repo at a time — across all worktrees, checkouts, and agent runtimes (Claude Code, Codex, no-mistakes, manual shells) — while CI, `cargo fmt --check`, and `just verify-remote` remain untouched.

**Architecture:** In-script self-governance (approved Option A). Every `scripts/verify_*.py` / `scripts/test_*.py` entry point acquires a per-repo machine-level exclusive flock before doing work. The lock path is declared in `ci/rust-verification.toml` `[local_lane_policy]` (committed, env-independent — review amendment F1), re-entrancy is detected by walking the caller's process ancestry, not env markers (F2), and a meta-check verifier makes coverage drift a CI failure (F3). Waiters queue with stderr heartbeats and fail loud at a policy timeout (F5: no-mistakes `ci_timeout` is 4h ≫ worst queue wait; default timeout 1800s — the longest governed script is unmeasured at plan time, so the default carries depth-3 headroom and Task 9 captures a real measurement). Verifier performance itself (90s wall / 74s CPU for one literal scan) is filed as follow-up F4, not fixed here.

**Tech Stack:** Python 3.11+ stdlib only (`fcntl`, `ast`, `json`, `subprocess`), TOML policy via the existing repo verification owner `scripts/rust_verification.py`, `just` recipes, GitHub Actions CI.

---

## Decision Record (approved 2026-06-12)

- **Issue:** seungpyoson/bolt-v2#653 — "Agent: govern CPU-heavy local static verifier lanes". Related: #645 (compile-lane enforcement), #648 (CI cost, untouched). Discovered in PR #650 review follow-up.
- **Chosen:** Option A (in-script self-governance) over a `just`-recipe wrapper (leaves direct-script bypass) and an agent-hook layer (covers only one of the user's agent runtimes; cannot queue; duplicates classification outside the repo).
- **Queue semantics:** block with heartbeat, fail loud at policy timeout. Pure rejection would make no-mistakes `test`/`lint` gates flaky whenever two sessions overlap.
- **Adversarial-review amendments (all mandatory):**
  - **F1:** lock path must not derive from `HOME` or any env var (`RUST_VERIFICATION_ROOT_BASE`-style overrides void mutual exclusion across sandboxed harnesses). Fixed absolute `lock_dir` committed in the repo TOML.
  - **F2:** re-entrancy via lock-holder pid ancestry walk (env markers are lost when children are spawned with scrubbed env, e.g. `SCRUB_ENV_KEYS`).
  - **F3:** meta-check (`scripts/verify_lane_governance.py`) asserts every governed entry point acquires the guard; wired into `source-fence-static`, which CI runs via the `source-fence` job.
  - **F5:** timeout calibration evidence: worst measured single-script hold 90s (`verify_bolt_v3_runtime_literals.py`, 2026-06-12, local run: real 90.19 / user 74.04); lock is held per-script, not per-lane, so a waiter's worst single wait ≈ longest single script × queue depth. The longest governed script is UNMEASURED at plan time, so the default is `acquire_timeout_seconds = 1800` (depth-3 headroom for multi-minute scripts, still ≪ the 4h no-mistakes ceiling, `~/.no-mistakes/config.yaml` `ci_timeout: "4h"`); Task 9 Step 1 records a real longest-script measurement to recalibrate from.
  - **F4 (follow-up, separate issue, Task 10):** profile/cache the verifiers themselves.
- **Round-2 plan-review amendments (2026-06-12 adversarial pass over this plan):**
  - **A1/A2/A3:** Task 9's acceptance demo uses a daemonized (`ppid=1`, unrelated-tree) deterministic holder killed by exact recorded pid — it proves cross-tree contention (the F1/F2 mechanism), is not timing-flaky, and never uses `pkill -f` patterns that could kill other agent sessions' verifier runs.
  - **A4:** `-h`/`--help` invocations bypass the lock (a help call must never queue behind a multi-minute holder); pinned by test in Task 5.
  - **A5:** timeout default raised 900 → 1800 (see F5).
  - **A6:** ancestry pass-through requires the same holder pid on two consecutive busy polls, closing the stale-metadata window between a new holder's flock and its metadata write; pid-recycling into a live ancestor remains a documented residual (bounded impact: one ungoverned script run).
  - **A7:** the F4 follow-up issue body cites only sourced measurements; lane script counts are captured at filing time, not estimated.
  - **A8:** wiring commit stages with `git add -u scripts/` (tracked modifications only), never blanket `git add scripts/`.
- **Pre-flight verified (2026-06-12):** repo validator `validate_policy_data` tolerates new top-level sections; cargo shim parses only `[local_compile_policy]` (section-scoped fallback parser); agent-layer `~/.claude/lib/rust_verification.py` strict key check applies to its v1 schema only — bolt-v2's TOML is v2 and is loaded via the repo-local owner. Adding `[local_lane_policy]` breaks neither.
- **CI exemption:** `allowed_ci_env = "GITHUB_ACTIONS"` presence bypasses the lock (CI jobs are isolated runners; CI runs `just fmt-check` whose prerequisites are governed scripts — bypass keeps CI semantics identical).
- **Out of scope:** `scripts/rust_verification.py` cargo passthrough (keeps `cargo fmt --check` instant), `just verify-remote`, Ubicloud cost (#648), CI topology, non-repo CPU consumers (`agent-doctor-log.mjs`).

## File Structure

```
ci/rust-verification.toml                      MODIFY  add [local_lane_policy] section
scripts/rust_verification.py                   MODIFY  add validate_local_lane_policy(), call it from validate_policy_data()
scripts/lane_governor.py                       CREATE  flock acquire/queue/timeout/ancestry/CI-bypass (~150 lines)
scripts/test_lane_governor.py                  CREATE  self-tests incl. policy-validation tests (governed itself)
scripts/verify_lane_governance.py              CREATE  AST meta-check: every governed entry acquires the guard
scripts/test_verify_lane_governance.py         CREATE  meta-check self-tests against fixture files
scripts/verify_*.py, scripts/test_*.py (34)    MODIFY  insert acquire() as first executable stmt of __main__
justfile                                       MODIFY  add meta-check pair + test_lane_governor to source-fence-static
AGENTS.md                                      MODIFY  document lane governance in Rust-verification section
CLAUDE.md                                      NO-OP   corrected 2026-06-12: target section belongs to the unmerged #645 docs series (Task 8 Step 3)
docs/superpowers/plans/2026-06-12-653-*.md     CREATE  this plan (2 parts)
```

Worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/653-local-lane-governance`, branch `feat/653-local-lane-governance` off `origin/main` (926720086). All commands below run from the worktree root.

---

### Task 1: Policy section + validator

**Files:**
- Modify: `ci/rust-verification.toml` (append after `[local_compile_policy]` block, before `[remote_verification]`)
- Modify: `scripts/rust_verification.py` (add `validate_local_lane_policy`, call from `validate_policy_data`)
- Test: `scripts/test_lane_governor.py` (created here with validation tests only; lock tests arrive in Tasks 2–5)

- [ ] **Step 1: Write the failing test file**

Create `scripts/test_lane_governor.py`:

```python
#!/usr/bin/env python3
"""Self-tests for lane_governor and the local_lane_policy validator (#653)."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


def _load(name: str):
    path = Path(__file__).with_name(f"{name}.py")
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


RV = _load("rust_verification")
REPO_ROOT = Path(__file__).resolve().parent.parent


def _valid_lane_policy() -> dict:
    return {
        "enabled": True,
        "allowed_ci_env": "GITHUB_ACTIONS",
        "lock_dir": "/tmp/rust-verification-lanes",
        "acquire_timeout_seconds": 900,
        "heartbeat_seconds": 15,
        "poll_interval_seconds": 1,
    }


def _expect_policy_error(data: dict, fragment: str) -> None:
    try:
        RV.validate_local_lane_policy(data)
    except RV.PolicyError as exc:
        assert fragment in str(exc), f"expected {fragment!r} in {exc}"
        return
    raise AssertionError(f"expected PolicyError containing {fragment!r}")


def test_valid_lane_policy_passes() -> None:
    RV.validate_local_lane_policy({"local_lane_policy": _valid_lane_policy()})


def test_missing_lane_policy_rejected() -> None:
    _expect_policy_error({}, "local_lane_policy table is required")


def test_disabled_lane_policy_rejected() -> None:
    policy = _valid_lane_policy()
    policy["enabled"] = False
    _expect_policy_error({"local_lane_policy": policy}, "enabled must be true")


def test_relative_lock_dir_rejected() -> None:
    policy = _valid_lane_policy()
    policy["lock_dir"] = "var/lanes"
    _expect_policy_error({"local_lane_policy": policy}, "absolute path")


def test_env_expansion_lock_dir_rejected() -> None:
    for bad in ("/tmp/$USER/lanes", "~/lanes"):
        policy = _valid_lane_policy()
        policy["lock_dir"] = bad
        _expect_policy_error({"local_lane_policy": policy}, "must not contain")


def test_heartbeat_must_be_below_timeout() -> None:
    policy = _valid_lane_policy()
    policy["heartbeat_seconds"] = 900
    _expect_policy_error({"local_lane_policy": policy}, "less than acquire_timeout_seconds")


def test_non_positive_intervals_rejected() -> None:
    for key in ("acquire_timeout_seconds", "heartbeat_seconds", "poll_interval_seconds"):
        policy = _valid_lane_policy()
        policy[key] = 0
        _expect_policy_error({"local_lane_policy": policy}, key)


def test_repo_policy_file_declares_lane_policy() -> None:
    data = RV.load_policy(REPO_ROOT)
    assert "local_lane_policy" in data, "ci/rust-verification.toml must declare [local_lane_policy]"


def main() -> int:
    tests = [
        test_valid_lane_policy_passes,
        test_missing_lane_policy_rejected,
        test_disabled_lane_policy_rejected,
        test_relative_lock_dir_rejected,
        test_env_expansion_lock_dir_rejected,
        test_heartbeat_must_be_below_timeout,
        test_non_positive_intervals_rejected,
        test_repo_policy_file_declares_lane_policy,
    ]
    for test in tests:
        test()
    print("OK: lane governor self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

(Task 7 later inserts the `lane_governor.acquire()` line into this file's `__main__` block along with all other governed scripts.)

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 scripts/test_lane_governor.py`
Expected: FAIL — `AttributeError: module 'rust_verification' has no attribute 'validate_local_lane_policy'`

- [ ] **Step 3: Add the validator to `scripts/rust_verification.py`**

Insert after `validate_local_compile_policy` (ends near line 267):

```python
def validate_local_lane_policy(data: dict[str, Any]) -> None:
    policy = data.get("local_lane_policy")
    if not isinstance(policy, dict):
        raise PolicyError("local_lane_policy table is required")
    if policy.get("enabled") is not True:
        raise PolicyError("local_lane_policy.enabled must be true")
    if policy.get("allowed_ci_env") != "GITHUB_ACTIONS":
        raise PolicyError("local_lane_policy.allowed_ci_env must be 'GITHUB_ACTIONS'")
    lock_dir = policy.get("lock_dir")
    if not isinstance(lock_dir, str) or not lock_dir.startswith("/"):
        raise PolicyError("local_lane_policy.lock_dir must be an absolute path")
    if "$" in lock_dir or "~" in lock_dir:
        raise PolicyError("local_lane_policy.lock_dir must not contain env or home expansions")
    values: dict[str, int] = {}
    for key in ("acquire_timeout_seconds", "heartbeat_seconds"):
        value = policy.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            raise PolicyError(f"local_lane_policy.{key} must be a positive integer")
        values[key] = value
    poll = policy.get("poll_interval_seconds")
    if not isinstance(poll, (int, float)) or isinstance(poll, bool) or poll <= 0:
        raise PolicyError("local_lane_policy.poll_interval_seconds must be a positive number")
    if values["heartbeat_seconds"] >= values["acquire_timeout_seconds"]:
        raise PolicyError("local_lane_policy.heartbeat_seconds must be less than acquire_timeout_seconds")
```

In `validate_policy_data`, after the `validate_local_compile_policy(data)` line, add:

```python
    validate_local_lane_policy(data)
```

(The section is required, matching `local_compile_policy`'s fail-loud style. The same commit adds the section to the TOML, so no checkout state exists where validation fails.)

- [ ] **Step 4: Add the policy section to `ci/rust-verification.toml`**

Insert between the `[local_compile_policy]` block and `[remote_verification]`:

```toml
[local_lane_policy]
enabled = true
allowed_ci_env = "GITHUB_ACTIONS"
lock_dir = "/tmp/rust-verification-lanes"
acquire_timeout_seconds = 1800
heartbeat_seconds = 15
poll_interval_seconds = 1
```

`lock_dir` is intentionally a fixed absolute path outside `$HOME` (F1): every checkout/worktree/harness of this repo resolves the identical path regardless of environment. The lock file name is derived from `target_namespace` (already in this TOML), so nothing is duplicated.

- [ ] **Step 5: Run tests to verify they pass**

Run: `python3 scripts/test_lane_governor.py`
Expected: `OK: lane governor self-tests passed.`

Run: `python3 scripts/rust_verification.py validate-policy --repo . >/dev/null && echo PASS`
Expected: `PASS`

- [ ] **Step 6: Commit**

```bash
git add ci/rust-verification.toml scripts/rust_verification.py scripts/test_lane_governor.py
git commit -m "feat: declare and validate local_lane_policy (#653)"
```

---

### Task 2: `lane_governor.py` — uncontended acquire + queue

**Files:**
- Create: `scripts/lane_governor.py`
- Test: `scripts/test_lane_governor.py` (extend)

- [ ] **Step 1: Add failing tests**

Add to `scripts/test_lane_governor.py` after the validation tests. These drive subprocesses so flock contention is real (flock is per-process):

```python
import json
import os
import subprocess
import tempfile
import time

SCRIPTS_DIR = Path(__file__).resolve().parent

# Subprocess runner: acquire, write a sentinel, hold for --hold seconds, exit.
HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, sentinel, hold = sys.argv[2], sys.argv[3], float(sys.argv[4])
handle = lane_governor.acquire(
    "hold-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text(str(time.time()), encoding="utf-8")
time.sleep(hold)
print("released", time.time())
"""

# Subprocess runner: acquire once, print acquisition wall time, exit immediately.
ONCE_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, timeout = sys.argv[2], float(sys.argv[3])
handle = lane_governor.acquire(
    "once-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=timeout, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("acquired", time.time())
"""


def _spawn(snippet: str, *args: str, env: dict | None = None) -> subprocess.Popen:
    return subprocess.Popen(
        [sys.executable, "-c", snippet, str(SCRIPTS_DIR), *args],
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
    )


def _wait_for(path: Path, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.05)
    raise AssertionError(f"sentinel {path} never appeared")


def test_uncontended_acquire_is_fast() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        start = time.monotonic()
        proc = _spawn(ONCE_RUNNER, tmp, "30")
        out, err = proc.communicate(timeout=20)
        assert proc.returncode == 0, err
        assert "acquired" in out
        assert time.monotonic() - start < 10, "uncontended acquire must not wait"


def test_second_acquire_queues_until_release() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "3")
        _wait_for(sentinel)
        t0 = time.monotonic()
        waiter = _spawn(ONCE_RUNNER, tmp, "30")
        out, err = waiter.communicate(timeout=30)
        waited = time.monotonic() - t0
        holder.communicate(timeout=10)
        assert waiter.returncode == 0, err
        assert waited >= 2.0, f"waiter should queue behind holder, waited only {waited:.2f}s"


def test_holder_metadata_written() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "3")
        _wait_for(sentinel)
        data = RV.load_policy(REPO_ROOT)
        lock_path = Path(tmp) / f"{data['target_namespace']}.lane.lock"
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
        holder.communicate(timeout=10)
        assert payload["pid"] == holder.pid
        assert payload["lane"] == "hold-runner"
```

Register in `main()`'s `tests` list:

```python
        test_uncontended_acquire_is_fast,
        test_second_acquire_queues_until_release,
        test_holder_metadata_written,
```

- [ ] **Step 2: Run to verify failure**

Run: `python3 scripts/test_lane_governor.py`
Expected: FAIL — `ModuleNotFoundError: No module named 'lane_governor'` (from the first subprocess test; stderr shown by the assertion).

- [ ] **Step 3: Implement `scripts/lane_governor.py`**

```python
#!/usr/bin/env python3
"""Per-repo single-flight governor for CPU-heavy local verifier lanes (#653).

Every governed script (scripts/verify_*.py, scripts/test_*.py) calls
``acquire()`` as the first executable statement of its ``__main__`` block.
Policy lives in ci/rust-verification.toml [local_lane_policy]. The lock path
is committed and environment-independent so every checkout, worktree, and
agent harness of this repo contends on the same machine-level file. CI
(allowed_ci_env present) bypasses the lock. A waiter whose lock holder is one
of its own process ancestors proceeds without the lock: the ancestor already
serializes the repo. Coverage is enforced by scripts/verify_lane_governance.py.
"""

from __future__ import annotations

import fcntl
import json
import os
import subprocess
import sys
import time
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).resolve().parent
if str(_SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS_DIR))

import rust_verification

REPO_ROOT = _SCRIPTS_DIR.parent

# Handles held for the lifetime of the process; flock releases on exit/kill.
_HELD_HANDLES: list[object] = []


class LaneLockTimeout(SystemExit):
    """Raised (exit code 1) when the lane lock is not acquired in time."""


def _parent_pid(pid: int) -> int | None:
    try:
        completed = subprocess.run(
            ["ps", "-o", "ppid=", "-p", str(pid)],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
    except OSError:
        return None
    raw = completed.stdout.strip()
    if not raw:
        return None
    try:
        value = int(raw)
    except ValueError:
        return None
    return value if value > 0 else None


def holder_is_ancestor(holder_pid: int) -> bool:
    pid: int | None = os.getpid()
    seen: set[int] = set()
    while pid is not None and pid > 1 and pid not in seen:
        if pid == holder_pid:
            return True
        seen.add(pid)
        pid = _parent_pid(pid)
    return False


def _read_holder(lock_path: Path) -> dict:
    try:
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return payload if isinstance(payload, dict) else {}


def acquire(
    lane: str | None = None,
    *,
    lock_dir: str | os.PathLike[str] | None = None,
    honor_ci_env: bool = True,
    acquire_timeout_seconds: float | None = None,
    heartbeat_seconds: float | None = None,
    poll_interval_seconds: float | None = None,
):
    """Acquire the per-repo lane lock; return the held handle, or None.

    None is returned when governance does not apply: CI environment, or the
    current holder is an ancestor process (re-entrant call).
    """
    policy = rust_verification.load_policy(REPO_ROOT)
    lane_policy = policy["local_lane_policy"]
    if honor_ci_env and os.environ.get(lane_policy["allowed_ci_env"]):
        return None
    if {"-h", "--help"}.intersection(sys.argv[1:]):
        # A help invocation does no heavy work and must never queue behind a
        # multi-minute holder (A4).
        return None
    label = lane or Path(sys.argv[0]).name or "unknown-lane"
    directory = Path(lock_dir) if lock_dir is not None else Path(lane_policy["lock_dir"])
    timeout = (
        acquire_timeout_seconds
        if acquire_timeout_seconds is not None
        else lane_policy["acquire_timeout_seconds"]
    )
    heartbeat = (
        heartbeat_seconds if heartbeat_seconds is not None else lane_policy["heartbeat_seconds"]
    )
    poll = (
        poll_interval_seconds
        if poll_interval_seconds is not None
        else lane_policy["poll_interval_seconds"]
    )
    directory.mkdir(parents=True, exist_ok=True)
    lock_path = directory / f"{policy['target_namespace']}.lane.lock"
    handle = open(lock_path, "a+", encoding="utf-8")
    started = time.monotonic()
    last_heartbeat = started
    last_busy_holder_pid: int | None = None
    while True:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            holder = _read_holder(lock_path)
            holder_pid = holder.get("pid")
            if (
                isinstance(holder_pid, int)
                and holder_pid == last_busy_holder_pid
                and holder_is_ancestor(holder_pid)
            ):
                # Same holder pid observed on two consecutive busy polls (A6):
                # closes the window where a new holder has the flock but has
                # not yet written its metadata, which would otherwise let a
                # waiter pass through on the PREVIOUS holder's pid.
                handle.close()
                return None
            last_busy_holder_pid = holder_pid if isinstance(holder_pid, int) else None
            now = time.monotonic()
            waited = now - started
            if waited >= timeout:
                handle.close()
                print(
                    f"lane-governor: FAILED to acquire {lock_path} after {waited:.0f}s; "
                    f"held by pid {holder.get('pid')} lane {holder.get('lane')!r}. "
                    "Another CPU-heavy local verifier lane is running; retry when it "
                    "finishes, or raise [local_lane_policy].acquire_timeout_seconds.",
                    file=sys.stderr,
                )
                raise LaneLockTimeout(1)
            if now - last_heartbeat >= heartbeat:
                print(
                    f"lane-governor: waiting for {lock_path} held by pid "
                    f"{holder.get('pid')} lane {holder.get('lane')!r} ({waited:.0f}s elapsed)",
                    file=sys.stderr,
                )
                last_heartbeat = now
            time.sleep(poll)
            continue
        handle.seek(0)
        handle.truncate()
        json.dump({"pid": os.getpid(), "lane": label, "started_at": time.time()}, handle)
        handle.write("\n")
        handle.flush()
        _HELD_HANDLES.append(handle)
        return handle
```

Design notes the implementer must not "fix": the lock is held for the remainder of the process (released by the OS on any exit — no stale-lock cleanup path needed, deliberately no `release()`); metadata reads by waiters are unlocked and may race, so `_read_holder` tolerates garbage; ancestry pass-through deliberately requires the same holder pid on two consecutive busy polls (A6) and re-checks every iteration because the holder can change while waiting; the `--help` fast-path (A4) is intentionally before any lock work.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python3 scripts/test_lane_governor.py`
Expected: `OK: lane governor self-tests passed.` (queue test takes ~3–5s by design)

- [ ] **Step 5: Commit**

```bash
git add scripts/lane_governor.py scripts/test_lane_governor.py
git commit -m "feat: add lane_governor single-flight flock with queue (#653)"
```

---

### Task 3: Timeout fail-loud

**Files:**
- Modify: `scripts/test_lane_governor.py`
- (Implementation already exists in Task 2's `acquire`; this task proves the timeout path by pinning the observable contract.)

- [ ] **Step 1: Add the contract-pin test**

```python
def test_timeout_fails_loud_with_holder_info() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "15")
        _wait_for(sentinel)
        waiter = _spawn(ONCE_RUNNER, tmp, "2")
        out, err = waiter.communicate(timeout=30)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"expected exit 1, got {waiter.returncode}"
        assert "FAILED to acquire" in err
        assert "hold-runner" in err, "timeout message must name the holding lane"
        assert str(holder.pid) in err, "timeout message must name the holding pid"
```

Register `test_timeout_fails_loud_with_holder_info` in `main()`'s `tests` list.

- [ ] **Step 2: Run tests**

Run: `python3 scripts/test_lane_governor.py`
Expected: `OK: lane governor self-tests passed.` If the message assertions fail, fix the implementation, not the test — exit code 1 plus holder pid/lane in stderr is the contract agents act on.

- [ ] **Step 3: Commit**

```bash
git add scripts/test_lane_governor.py
git commit -m "test: pin lane lock timeout fail-loud contract (#653)"
```

---

### Task 4: Ancestry re-entrancy (F2)

**Files:**
- Modify: `scripts/test_lane_governor.py`

- [ ] **Step 1: Add test — child with scrubbed env passes through while parent holds**

```python
# Parent: acquire, then spawn a child runner WITH A SCRUBBED ENV that attempts
# acquire on the same lock dir. The child must pass through (ancestor holds).
PARENT_CHILD_RUNNER = """
import os, subprocess, sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
handle = lane_governor.acquire(
    "parent-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
child_code = (
    "import sys, time; sys.path.insert(0, sys.argv[1]); import lane_governor; "
    "t0 = time.monotonic(); "
    "lane_governor.acquire('child-runner', lock_dir=sys.argv[2], honor_ci_env=False, "
    "acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1); "
    "print('child-done', time.monotonic() - t0)"
)
scrubbed = {"PATH": "/usr/bin:/bin"}
completed = subprocess.run(
    [sys.executable, "-c", child_code, scripts_dir, lock_dir],
    env=scrubbed, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("child-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""


def test_scrubbed_env_child_reenters_while_parent_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = _spawn(PARENT_CHILD_RUNNER, tmp)
        out, err = proc.communicate(timeout=40)
        assert proc.returncode == 0, err
        assert "child-rc 0" in out, f"child must succeed, got: {out}\n{err}"
        line = [l for l in out.splitlines() if l.startswith("child-done")][0]
        elapsed = float(line.split()[1])
        assert elapsed < 5.0, f"child must pass through re-entrantly, took {elapsed:.1f}s"
```

Register `test_scrubbed_env_child_reenters_while_parent_holds` in `main()`.

- [ ] **Step 2: Run tests**

Run: `python3 scripts/test_lane_governor.py`
Expected: `OK: lane governor self-tests passed.` — Task 2's `holder_is_ancestor` already implements this; the test exists to keep F2 from regressing (it fails after a <20s wait-timeout if ancestry detection breaks, e.g. if someone "simplifies" it to an env marker, which the scrubbed `env=` would strip).

- [ ] **Step 3: Commit**

```bash
git add scripts/test_lane_governor.py
git commit -m "test: pin ancestry re-entrancy under scrubbed env (#653)"
```

---

### Task 5: CI bypass + `--help` fast-path

**Files:**
- Modify: `scripts/test_lane_governor.py`

- [ ] **Step 1: Add tests — CI env and `--help` pass through even while another process holds**

```python
CI_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("ci-result", result is None, time.monotonic() - t0)
"""


def test_ci_env_bypasses_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "true"
        ci = _spawn(CI_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "CI bypass must return None without locking"
        assert elapsed < 5.0, "CI bypass must not wait"
```

Then add the `--help` fast-path test (A4 — a help invocation must never queue behind a multi-minute holder):

```python
HELP_RUNNER = """
import sys, time
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
sys.path.insert(0, scripts_dir)
import lane_governor
sys.argv = ["verify_sample.py", "--help"]
t0 = time.monotonic()
result = lane_governor.acquire(
    "help-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("help-result", result is None, time.monotonic() - t0)
"""


def test_help_invocation_bypasses_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        helper = _spawn(HELP_RUNNER, tmp)
        out, err = helper.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert helper.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "--help must not take or wait for the lane lock"
        assert elapsed < 5.0, "--help fast-path must not wait"
```

Register `test_ci_env_bypasses_lock` and `test_help_invocation_bypasses_lock` in `main()`. Note `CI_RUNNER` deliberately omits `honor_ci_env=False` — it exercises the production default path under the policy's `allowed_ci_env`.

- [ ] **Step 2: Run tests**

Run: `python3 scripts/test_lane_governor.py`
Expected: `OK: lane governor self-tests passed.` (bypass and help fast-path logic shipped in Task 2; this pins both. The full suite runs both in CI and locally because every contention test forces `honor_ci_env=False` except the CI one, which sets the env explicitly.)

- [ ] **Step 3: Commit**

```bash
git add scripts/test_lane_governor.py
git commit -m "test: pin CI bypass and --help fast-path for lane governance (#653)"
```

---

**Continue with Part 2:** `docs/superpowers/plans/2026-06-12-653-local-lane-governance-part2.md` — Task 6 (meta-check, F3), Task 7 (wire all entry points), Task 8 (justfile + docs), Task 9 (acceptance + PR), Task 10 (F4 follow-up issue), and the plan self-review.
