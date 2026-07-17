# Root Artifact Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make root-artifact cache evidence run-local and add executable mutation evidence for every #1392 workflow invariant identified by internal review.

**Architecture:** A dedicated static verifier owns the root-artifact workflow contract and is automatically discovered by `scripts/run_fences.py`; its paired test suite mutates the real workflow text and proves each forbidden form is rejected. The workflow itself performs a mandatory sccache reset and verifies a zero JSON baseline immediately before the sole Cargo build.

**Tech Stack:** Python 3 standard library, GitHub Actions YAML, Bash, jq, sccache v0.10.0.

## Global Constraints

- Do not change the shared sccache action's degradation policy.
- Do not add a second build, test lane, artifact consumer, installer, or authority path.
- Do not treat the root artifact as deploy, readiness, merge, or trading permission.
- Do not run local compile-heavy Rust verification.

---

### Task 1: Root-artifact mutation policy

**Files:**
- Create: `scripts/verify_root_artifact_workflow.py`
- Create: `scripts/test_verify_root_artifact_workflow.py`

**Interfaces:**
- Consumes: the complete workflow text from `.github/workflows/root-artifact.yml`.
- Produces: `root_artifact_workflow_errors(text: str) -> list[str]` and a CLI `main() -> int` that checks the repository workflow.

- [ ] **Step 1: Write the failing mutation test**

Create a test harness that imports `verify_root_artifact_workflow.py`, loads the real workflow, requires an empty baseline error list, and defines `assert_mutation_rejected(label, old, new, expected)` using exact single replacements. Cover mutations for automatic triggers, another producer job, a second Cargo build, `cargo test`, `cargo nextest`, wrapper-check removal, zero-stat removal, retry insertion, result fallback, overlay omission, byte-check removal, digest-check removal, artifact upload, and install/launch/readiness/merge/deploy/trading consumers.

- [ ] **Step 2: Run the test to verify RED**

Run: `python3 scripts/test_verify_root_artifact_workflow.py`

Expected: FAIL because `scripts/verify_root_artifact_workflow.py` does not exist.

- [ ] **Step 3: Implement the minimal verifier**

Implement explicit contract checks over uncommented workflow text and job/step boundaries. Require:

```python
def root_artifact_workflow_errors(text: str) -> list[str]:
    errors: list[str] = []
    # exact workflow_dispatch trigger, preflight + produce jobs, one Cargo build,
    # prohibited test/retry/fallback/authority tokens, wrapper/reset/overlay/byte/digest guards
    return errors
```

The CLI reads `REPO_ROOT / ".github/workflows/root-artifact.yml"`, prints each error to stderr, returns 1 on any error, and prints a single OK line on success. Keep the verifier dependency-free so `run_fences.py` can import it.

- [ ] **Step 4: Run the focused suite to verify GREEN**

Run: `python3 scripts/test_verify_root_artifact_workflow.py`

Expected: PASS with every mutation rejected for its intended reason.

- [ ] **Step 5: Commit the policy slice**

```bash
git add scripts/verify_root_artifact_workflow.py scripts/test_verify_root_artifact_workflow.py
git commit -m "test: fence root artifact workflow mutations"
```

### Task 2: Fail-closed cache-stat baseline

**Files:**
- Modify: `.github/workflows/root-artifact.yml` in the sccache validation section before the build step.
- Test: `scripts/test_verify_root_artifact_workflow.py`

**Interfaces:**
- Consumes: `SCCACHE_PATH` emitted by `.github/actions/sccache-setup` and sccache v0.10.0 JSON statistics.
- Produces: `$RUNNER_TEMP/root-artifact-sccache-baseline.json` containing zero compile requests, hits, and misses before Cargo.

- [ ] **Step 1: Extend the test to require a verified zero baseline**

Require the accepted workflow to contain a pre-build block equivalent to:

```bash
baseline_stats="$RUNNER_TEMP/root-artifact-sccache-baseline.json"
"$SCCACHE_PATH" --zero-stats
"$SCCACHE_PATH" --show-stats --stats-format json > "$baseline_stats"
[[ "$(jq -er '.stats.compile_requests' "$baseline_stats")" -eq 0 ]]
[[ "$(jq -er '[.stats.cache_hits.counts[]?] | add // 0' "$baseline_stats")" -eq 0 ]]
[[ "$(jq -er '[.stats.cache_misses.counts[]?] | add // 0' "$baseline_stats")" -eq 0 ]]
```

Add separate mutations that remove the reset, each zero assertion, or move the baseline after Cargo.

- [ ] **Step 2: Run the focused test to verify RED**

Run: `python3 scripts/test_verify_root_artifact_workflow.py`

Expected: FAIL because the workflow currently trusts the shared action's best-effort reset.

- [ ] **Step 3: Add the minimal pre-build baseline step**

Add `Verify empty sccache statistics baseline` after sccache setup validation and before `Build root artifact`. Use `set -euo pipefail`; do not suppress errors. Validate all three counter families are zero.

- [ ] **Step 4: Run the focused test to verify GREEN**

Run: `python3 scripts/test_verify_root_artifact_workflow.py`

Expected: PASS.

- [ ] **Step 5: Commit the workflow fix**

```bash
git add .github/workflows/root-artifact.yml scripts/test_verify_root_artifact_workflow.py scripts/verify_root_artifact_workflow.py
git commit -m "fix: isolate root artifact cache evidence"
```

### Task 3: Governed verification and handoff

**Files:**
- Verify: all files changed by Tasks 1-2.

**Interfaces:**
- Consumes: the completed workflow and mutation policy.
- Produces: local non-compile evidence suitable for the updated PR head.

- [ ] **Step 1: Run focused and syntax checks**

Run:

```bash
python3 scripts/test_verify_root_artifact_workflow.py
python3 -m py_compile scripts/verify_root_artifact_workflow.py scripts/test_verify_root_artifact_workflow.py
```

Expected: both commands exit 0.

- [ ] **Step 2: Run repository formatting and workflow gates**

Run:

```bash
just fmt-check
just ci-lint-workflow
just source-fence-static
```

Expected: all public recipes exit 0 without local Rust compilation.

- [ ] **Step 3: Inspect the final diff and worktree**

Run:

```bash
git diff --check HEAD~2..HEAD
git status --short
```

Expected: no whitespace errors and no uncommitted files.

- [ ] **Step 4: Publish with the governed sandbox path**

Run: `just sandbox-safe-push`

Expected: remote branch head equals local `HEAD`. Report the exact SHA; do not claim runtime ARM64 or post-landing evidence until an exact-current-`main` dispatch exists.
