# #653 Local Lane Governance Implementation Plan (Part 2/2: Tasks 6–10)

> **For agentic workers:** Part 1 (`2026-06-12-653-local-lane-governance.md`) holds the goal, decision record, file structure, and Tasks 1–5. Execute in order; this part assumes Tasks 1–5 are committed.

---

### Task 6: Meta-check verifier (F3)

**Files:**
- Create: `scripts/verify_lane_governance.py`
- Create: `scripts/test_verify_lane_governance.py`

- [ ] **Step 1: Write the failing self-test**

Create `scripts/test_verify_lane_governance.py`:

```python
#!/usr/bin/env python3
"""Self-tests for the lane-governance meta-check (#653)."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("verify_lane_governance.py")
SPEC = importlib.util.spec_from_file_location("verify_lane_governance", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)

COMPLIANT = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
'''

MISSING_ACQUIRE = '''
def main():
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
'''

ACQUIRE_TOO_LATE = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    print("starting")
    lane_governor.acquire()
    raise SystemExit(main())
'''

NO_MAIN_BLOCK = '''
def helper():
    return 0
'''


def _violations(named_sources: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for name, source in named_sources.items():
            (root / name).write_text(source, encoding="utf-8")
        return CHECKER.lane_governance_violations(root)


def test_compliant_file_passes() -> None:
    assert _violations({"verify_sample.py": COMPLIANT}) == []


def test_missing_acquire_flagged() -> None:
    violations = _violations({"verify_sample.py": MISSING_ACQUIRE})
    assert len(violations) == 1 and "verify_sample.py" in violations[0]


def test_acquire_after_other_statement_flagged() -> None:
    violations = _violations({"test_sample.py": ACQUIRE_TOO_LATE})
    assert len(violations) == 1 and "first executable statement" in violations[0]


def test_module_without_main_is_exempt() -> None:
    assert _violations({"verify_sample.py": NO_MAIN_BLOCK}) == []


def test_non_matching_names_ignored() -> None:
    assert _violations({"leadlag_tool.py": MISSING_ACQUIRE}) == []


def test_real_scripts_dir_is_clean() -> None:
    assert CHECKER.lane_governance_violations(Path(__file__).resolve().parent) == []


def main() -> int:
    tests = [
        test_compliant_file_passes,
        test_missing_acquire_flagged,
        test_acquire_after_other_statement_flagged,
        test_module_without_main_is_exempt,
        test_non_matching_names_ignored,
        test_real_scripts_dir_is_clean,
    ]
    for test in tests:
        test()
    print("OK: lane-governance meta-check self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 2: Run to verify failure**

Run: `python3 scripts/test_verify_lane_governance.py`
Expected: FAIL — `verify_lane_governance.py` does not exist yet; `spec_from_file_location` either returns an unloadable spec (`RuntimeError: failed to load ...`) or `exec_module` raises `FileNotFoundError`. Either is the expected red.

- [ ] **Step 3: Implement `scripts/verify_lane_governance.py`**

```python
#!/usr/bin/env python3
"""Meta-check: every governed lane entry point acquires the lane lock (#653).

Rule: in every scripts/verify_*.py and scripts/test_*.py that has a module-level
``if __name__ == "__main__":`` block, the first non-import statement of that
block must be a bare ``lane_governor.acquire(...)`` call. Files without a
``__main__`` block cannot run as lanes and are exempt. This makes lane-coverage
drift a CI failure instead of a convention.
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent


def _is_main_guard(node: ast.stmt) -> bool:
    if not isinstance(node, ast.If):
        return False
    test = node.test
    if not isinstance(test, ast.Compare) or len(test.ops) != 1:
        return False
    if not isinstance(test.ops[0], ast.Eq):
        return False
    left, right = test.left, test.comparators[0]
    names = set()
    for side in (left, right):
        if isinstance(side, ast.Name):
            names.add(side.id)
        elif isinstance(side, ast.Constant):
            names.add(side.value)
    return "__name__" in names and "__main__" in names


def _is_acquire_call(node: ast.stmt) -> bool:
    return (
        isinstance(node, ast.Expr)
        and isinstance(node.value, ast.Call)
        and isinstance(node.value.func, ast.Attribute)
        and node.value.func.attr == "acquire"
        and isinstance(node.value.func.value, ast.Name)
        and node.value.func.value.id == "lane_governor"
    )


def lane_governance_violations(scripts_dir: Path) -> list[str]:
    violations: list[str] = []
    governed = sorted(
        list(scripts_dir.glob("verify_*.py")) + list(scripts_dir.glob("test_*.py"))
    )
    for path in governed:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        main_guards = [node for node in tree.body if _is_main_guard(node)]
        if not main_guards:
            continue
        for guard in main_guards:
            executable = [
                node
                for node in guard.body
                if not isinstance(node, (ast.Import, ast.ImportFrom))
            ]
            if not executable or not _is_acquire_call(executable[0]):
                violations.append(
                    f"{path.name}: first executable statement in the __main__ block "
                    "must be lane_governor.acquire()"
                )
    return violations


def main() -> int:
    violations = lane_governance_violations(SCRIPTS_DIR)
    if violations:
        print("Lane-governance violations:", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        return 1
    print("OK: all governed lane entry points acquire the lane lock.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
```

- [ ] **Step 4: Run the self-test — expect ONE remaining failure**

Run: `python3 scripts/test_verify_lane_governance.py`
Expected: fixture tests pass, but `test_real_scripts_dir_is_clean` FAILS listing ~36 unwired scripts. This is the intended red state for Task 7. Do not commit yet.

---

### Task 7: Wire all governed entry points

**Files:**
- Modify: every file the meta-check lists (all 34 existing `scripts/verify_*.py` + `scripts/test_*.py`, plus `test_lane_governor.py` and `test_verify_lane_governance.py` from Tasks 1–6; `verify_lane_governance.py` was created compliant)

- [ ] **Step 1: Get the authoritative red list**

Run: `python3 scripts/verify_lane_governance.py`
Expected: exit 1 with one line per unwired file. This list — not this plan — is the source of truth for which files to edit.

- [ ] **Step 2: Apply the mechanical edit to every listed file**

(Corrected 2026-06-12 after execution stop: the original step claimed one uniform
tail shape; the verified inventory at HEAD is three shapes, all single-statement.)

Every listed file's `__main__` block contains exactly one statement, in one of
three shapes: `raise SystemExit(main())`, `sys.exit(main())`, or
`unittest.main()`. For each listed file, insert the guard as the first
statements of the existing block and preserve the existing entry statement
unchanged:

```python
if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    <existing entry statement, verbatim — e.g. raise SystemExit(main())
     or sys.exit(main()) or unittest.main()>
```

Do not normalize the entry statements to one shape — that is out of scope. The
meta-check is the arbiter: its AST rule requires only that the first non-import
statement of the block is `lane_governor.acquire()`, so all three shapes pass
once the guard is inserted. (`unittest.main()` parses argv itself; `-h`/`--help`
is already handled before locking by the A4 fast-path.)

The import lives inside the `__main__` branch deliberately: these scripts import one another as modules (e.g. `verify_bolt_v3_runtime_literals` imports from `verify_bolt_v3_provider_leaks`), and module import must never take the lock. Test scripts that load their verifier via `importlib` `exec_module` are also safe — the loaded module's `__name__` is not `"__main__"`.

- [ ] **Step 3: Verify green + syntax across the tree**

Run: `python3 scripts/verify_lane_governance.py`
Expected: `OK: all governed lane entry points acquire the lane lock.`

Run: `python3 -m py_compile scripts/*.py && echo COMPILE-OK`
Expected: `COMPILE-OK`

Run: `python3 scripts/test_verify_lane_governance.py`
Expected: `OK: lane-governance meta-check self-tests passed.` (the real-dir test now passes)

Run: `python3 scripts/test_lane_governor.py`
Expected: `OK: lane governor self-tests passed.` (proves a governed script still runs end-to-end — this file is itself governed now)

- [ ] **Step 4: Smoke one real governed lane script end-to-end (a fast one)**

Run: `python3 scripts/verify_bolt_v3_status_map_current.py; echo "exit=$?"`
Expected: same pass/fail outcome as on `origin/main` (run it there first if unsure); the point is that acquiring the real `/tmp/rust-verification-lanes/bolt-v2.lane.lock` is invisible when uncontended.

- [ ] **Step 5: Commit (two commits: meta-check, then wiring)**

```bash
git add scripts/verify_lane_governance.py scripts/test_verify_lane_governance.py
git commit -m "feat: add lane-governance meta-check (#653)"
git add -u scripts/
git commit -m "feat: acquire lane lock in every verifier and test entry point (#653)"
```

(`git add -u` stages tracked modifications only — never blanket-add the directory, which would pick up untracked strays; A8.)

---

### Task 8: justfile + docs

**Files:**
- Modify: `justfile` (`source-fence-static` recipe)
- Modify: `AGENTS.md` (Rust verification section, after the bullet beginning `- Enforcement boundary:`)
- `CLAUDE.md`: NO change on this branch (see Step 3 — corrected 2026-06-12)

- [ ] **Step 1: Add the new checks to `source-fence-static`**

Append to the end of the `source-fence-static` recipe body (after the last existing `test_/verify_` pair), keeping the recipe's pair convention:

```
    python3 scripts/test_lane_governor.py
    python3 scripts/test_verify_lane_governance.py
    python3 scripts/verify_lane_governance.py
```

Placement rationale: CI's `source-fence` job depends on `source-fence-static`, so the meta-check and governor tests run on every PR — that is what makes F3 enforcement real. (`ci-lint-workflow` is not referenced by `.github/workflows/ci.yml`, so it is not a CI enforcement point; do not put the new checks only there.)

- [ ] **Step 2: Document in `AGENTS.md`**

Insert a new bullet immediately after the "Enforcement boundary:" bullet:

```markdown
- CPU-heavy local verifier lanes self-serialize: every `scripts/verify_*.py` / `scripts/test_*.py` entry point acquires the per-repo machine-level lane lock declared in `ci/rust-verification.toml` `[local_lane_policy]` before doing work. Concurrent local runs queue with stderr heartbeats and fail loud at the policy timeout; CI (`allowed_ci_env`) bypasses the lock; a holder that is a process ancestor passes through. Coverage drift is a CI failure via `scripts/verify_lane_governance.py` in `source-fence-static`.
```

- [ ] **Step 3: CLAUDE.md — intentionally NO edit on this branch (corrected 2026-06-12)**

The original step targeted a "Rust Verification" CLAUDE.md bullet that does not
exist at this branch's HEAD: that section belongs to the unmerged #645
cargo-shim docs series (local commits `6bf14f3c7..0277dcd36`, not on
`origin/main`). Adding it here would duplicate another branch's unmerged scope
(one-branch-one-scope) and guarantee a merge conflict. On this branch,
`AGENTS.md` (Step 2) is the single documentation home for lane governance.

Residual, to be stated in the PR body (partial-scope disclosure): once the #645
docs series lands on main, its CLAUDE.md Rust Verification single-source-of-truth
bullet should gain a `[local_lane_policy]` mention (one line) — tracked there,
not here.

- [ ] **Step 4: Sanity-check the justfile still parses**

Run: `just --summary | tr ' ' '\n' | grep -E "source-fence-static|ci-lint-workflow"`
Expected: both recipe names print.

- [ ] **Step 5: Commit**

```bash
git add justfile AGENTS.md
git commit -m "docs: wire lane governance into source-fence-static and agent docs (#653)"
```

---

### Task 9: Acceptance verification, push, PR

Maps the issue's verification list to evidence. Run from the worktree root.

- [ ] **Step 1: Cross-tree contention queue (issue criteria 1–2; F1/F2 cross-runtime proof; A1/A2/A3)**

Deterministic demo: a daemonized holder (double-fork, `ppid=1` — macOS has no `setsid` binary, hence the in-Python daemonization) takes the REAL lane lock from an unrelated process tree, then one governed script queues behind it. This proves exactly the mechanism that matters across agent runtimes — a non-ancestor holder must block us — without timing luck and without touching any other session's processes.

```bash
python3 - "$PWD/scripts" <<'EOF'
import os, sys, time
scripts_dir = sys.argv[1]
if os.fork() > 0:
    sys.exit(0)
os.setsid()
if os.fork() > 0:
    os._exit(0)
with open("/tmp/653_holder.pid", "w", encoding="utf-8") as fh:
    fh.write(str(os.getpid()))
sys.path.insert(0, scripts_dir)
import lane_governor
lane_governor.acquire("cross-tree-holder", honor_ci_env=False)
time.sleep(40)
EOF
sleep 2
ps -o ppid= -p "$(cat /tmp/653_holder.pid)"
time python3 scripts/verify_lane_governance.py 2>/tmp/653_queue.log; echo "exit=$?"
grep -m1 "lane-governor: waiting" /tmp/653_queue.log && echo "QUEUE-OBSERVED"
kill "$(cat /tmp/653_holder.pid)" 2>/dev/null || true
```

Expected: `ps` prints `1` (holder runs in an unrelated tree, so ancestry pass-through must NOT fire); the governed script waits with heartbeat lines at ~15s/30s naming `cross-tree-holder` and its pid, acquires when the holder exits (~40s total), prints `OK: ...`, `exit=0`; `QUEUE-OBSERVED`. Afterwards `python3 scripts/verify_lane_governance.py` acquires instantly (flock released on holder exit). While here, record the longest governed script's duration (`/usr/bin/time` one full `just source-fence-static` run when convenient, or take it from CI timings) as the F5 recalibration measurement for `acquire_timeout_seconds`.

Optional, non-gating secondary observation: run `just ci-lint-workflow` and `just source-fence-static` concurrently and watch the second's stderr for heartbeats. The lock alternates per-script, so absence of a heartbeat is NOT a failure (a continuous 15s wait may never occur). If you stop the lanes early, kill only the recorded job pids and their descendants (walk `pgrep -P <pid>` recursively) — NEVER `pkill -f` script-name patterns, which would kill other concurrent agent sessions' verifier runs.

- [ ] **Step 2: `cargo fmt --check` remains allowed and ungoverned (issue criterion 3)**

Start a holder against the real lock dir, then run the owner's fmt passthrough while it holds:

```bash
python3 - <<'EOF' &
import sys, time
sys.path.insert(0, "scripts")
import lane_governor
lane_governor.acquire("acceptance-holder", honor_ci_env=False)
time.sleep(60)
EOF
HOLDER=$!
sleep 2
python3 scripts/rust_verification.py cargo --repo . -- fmt --check; echo "exit=$?"
kill $HOLDER 2>/dev/null
```

Expected: `fmt --check` completes at normal speed with no `lane-governor:` output (the owner's cargo passthrough is intentionally not a governed lane).

- [ ] **Step 3: Compile-lane blocking and verify-remote unchanged (issue criteria 4–5)**

```bash
git diff origin/main -- scripts/rust_verification.py | grep -E "^[-+].*(verify_remote|refused_cargo)" || echo "NO-TOUCH"
```

Expected: `NO-TOUCH` (this branch adds one validator function and one call site only; #645 paths and `verify-remote` are untouched by construction).

- [ ] **Step 4: Push and open the PR**

Follow the repo gate flow (protected mode: `safe_git.py`; if any `🔒 GATE:` challenge appears, present it verbatim and halt for approval):

```bash
git push -u origin feat/653-local-lane-governance
```

PR title: `#653: govern CPU-heavy local static verifier lanes`
PR body bullets:
- Per-repo single-flight flock for CPU-heavy local verifier lanes; queue with heartbeats, fail-loud timeout (`[local_lane_policy]` in `ci/rust-verification.toml`)
- Env-independent lock path (F1), ancestry-based re-entrancy (F2), AST meta-check CI-enforced via `source-fence-static` (F3)
- CI bypassed via `allowed_ci_env`; `cargo fmt --check`, compile-lane policy (#645), and `just verify-remote` untouched
- Scope: full #653; verifier *performance* (review finding F4) tracked separately in the follow-up issue (link after Task 10)
- Docs: `AGENTS.md` is the single documentation home on this branch; the one-line CLAUDE.md SSOT-bullet extension rides with the unmerged #645 docs series (plan Task 8 Step 3 records why)
- `Closes #653`

Then run `just verify-remote` for exact-head CI evidence and report the result — do not declare merge-readiness; that is the user's call.

---

### Task 10: F4 follow-up issue (after PR exists)

- [ ] **Step 1: Search for an existing issue first**

Run: `gh issue list -R seungpyoson/bolt-v2 -S "verifier performance cache" --state all`
Expected: no open duplicate (if one exists, comment there instead of creating).

- [ ] **Step 2: Create ONE consolidated issue (requires user confirmation per create-issue flow)**

Title: `Static verifier lanes: profile and cache CPU-heavy scans`
Body (single consolidated issue, numbered sub-items, per MECE follow-up convention):

```markdown
## Problem

#653 serializes CPU-heavy local verifier lanes, but the lanes are expensive in
absolute terms. Sourced measurement: `verify_bolt_v3_runtime_literals.py` alone
measured real 90.19s / user 74.04s (2026-06-12, local, /usr/bin/time).
`source-fence-static` runs every test_/verify_ pair in its recipe sequentially
(<insert the script count from the justfile at HEAD and a fresh /usr/bin/time
sample of the full lane when filing — do not estimate>). Serialization fixes
contention, not cost; under #653 queueing, multi-session latency is cost × queue depth.

## Sub-items

1. Profile the top verifier scripts (where do the 74 CPU-seconds go: file walk,
   regex, per-file re-parse?).
2. Content-hash skip-cache: key each verifier run on the hash of its scanned
   inputs (`src/**/*.rs`, allowlist TOMLs, the verifier source itself); skip the
   scan on a clean hit. Cache location must follow the #653 F1 rule
   (env-independent, policy-declared), and a cache hit must still exit through
   the verifier's normal PASS path (fail-loud preserved).
3. Re-measure `source-fence-static` wall/CPU after 1–2 and update the
   `[local_lane_policy]` `acquire_timeout_seconds` calibration note if the
   profile changes materially.

## Relations

- Follow-up from #653 adversarial review (finding F4); see PR <PR-URL>.
```

- [ ] **Step 3: Bidirectional link**

Comment on #653: `Follow-up (verifier performance, review finding F4): #<new-number>` — the new issue body already points back.

---

## Self-Review (done at plan time)

- **Spec coverage:** issue's expected outcome → Tasks 2–7; classification (cheap / CPU-heavy static / forbidden compile) → name-pattern rule + meta-check (Task 6) with `cargo fmt` and owner passthrough exempt (Task 9 Step 2 proves it); each verification bullet in the issue → Task 9 steps; non-goals untouched (no CI topology, no #648 changes, no verify-remote changes). Review amendments F1–F3 in Tasks 1/4/6, F5 in policy defaults + decision record, F4 in Task 10.
- **Placeholder scan:** every code step contains complete code; the only deliberately deferred content is the PR URL inside the F4 issue body (knowable only after Task 9) and the meta-check red list (authoritative at run time by design).
- **Type consistency:** `acquire()` signature in Task 2 matches every call site in Tasks 4–7 fixtures and runners; `lane_governance_violations(Path) -> list[str]` matches Task 6 tests; `validate_local_lane_policy(data)` matches Task 1 tests; lock file name `<target_namespace>.lane.lock` consistent between Task 2 implementation and Task 2 metadata test.
- **Round-2 adversarial pass (A1–A10) applied:** deterministic cross-tree acceptance demo replaces the flaky two-lane/pkill version (A1/A2/A3); `--help` fast-path + test (A4); timeout 900 → 1800 with a Task 9 measurement step (A5); double-read ancestry confirmation (A6); sourced-metrics-only wording in the F4 issue body (A7); `git add -u` staging (A8).
- **Known residuals (documented, accepted):** a process started before this change, running outside any repo checkout, or hand-rolling flock-free script copies is ungoverned — same residual class as #645's documented bypasses. Single-user machine assumption: `/tmp` lock dir is not hardened against hostile local users. Pid-recycling into a live ancestor of a waiter could in theory defeat the (double-read-confirmed) ancestry check — bounded impact: one ungoverned script run. flock wakeups have no FIFO fairness, so a waiter at queue depth ≥2 can be overtaken; starvation is bounded by the fail-loud timeout. The contention self-tests add ~25s of deliberate sleeps to every local `source-fence-static` run (CI included). The justfile lane lines themselves are convention-enforced, the same as every other verifier pair in the recipe (watcher-of-the-watcher accepted).
