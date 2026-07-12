# BVS S3 sccache and Target-Cache Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove every BVS whole-target GitHub Actions cache, preserve exact full-source nextest/sidecar artifacts, and reuse compatible compiler outputs through the existing governed S3 `sccache` path without weakening tests or disk-pressure refusal.

**Architecture:** The correctness slice deletes the unsafe mutable-target restore/save family and makes a cold local compile the only fallback. The performance slice wires the existing content-addressed `sccache` action with trusted-main write authority and read-only PR, merge-queue, scheduled, manual, and flaky consumers. Static workflow contracts, mutation tests, truthful pressure telemetry, exact-head remote proof, and post-main measurements provide the evidence; a sub-20% warm result stops the performance claim without reverting the correctness fix.

**Tech Stack:** GitHub Actions YAML, Python 3 workflow/policy verifiers, the repository-managed Rust verification wrapper, S3-backed `sccache`, exact full-source S3 nextest/sidecar artifacts, GitHub Actions cache inventory APIs.

## Global Constraints

- The approved design is `docs/superpowers/specs/2026-07-12-bvs-compatibility-cache-design.md` at `8b792e58a6fa037693bdcaf59b82173a94daff08`, approved by Claude Fable 5/xhigh in the Round 4 review.
- This is one BVS CI ownership slice. It does not change or close PR #1367, #1354, the separate reference-clock defect, #1275, #883, or #763.
- Do not change Binance/NT pins, trading/pricing/evidence/recovery/archive code, root cache allocation, GitHub cache deletion, S3 lifecycle/retention, buckets, prefixes, credentials, or either cache threshold.
- BVS GitHub Actions whole-target cache budget is permanently zero keys, zero bytes, and zero saves. Do not add a replacement target cache or a broad/exact fallback.
- Keep `Swatinem/rust-cache` registry/git-only with `cache-targets: false`.
- Keep both `soft_limit_bytes = 32212254720` declarations and the `10,737,418,240`-byte Actions-cache warning threshold unchanged.
- Preserve the complete BVS nextest archive, binary sidecars, four partitions, MinIO S3 smoke, issue-specific coverage, JUnit/Mergify behavior, and both full-source S3 metadata checks.
- Use the existing `./.github/actions/sccache-setup` and `./.github/actions/sccache-stats` only. Main may write; PR/MQ/flaky consumers are read-only; cache failure must cold-compile.
- Do not add workflow `CARGO_INCREMENTAL`. `managed_env` owns and scrubs it. Preserve applicable debug-profile values and reject raw Rust flag/wrapper overrides.
- Do not run compile-heavy Rust locally. Local verification is limited to public cheap gates, Python self-tests, workflow lint, source fences, and `git diff --check`.
- A separate implementation reviewer must approve each publishable checkpoint before it is pushed. External review is requested only after the final exact-head required CI is green.
- The correctness fix may land independently. Performance qualification is post-main because only trusted main runs may populate the compiler cache.

---

## File Map

- `.github/workflows/backtester-ci.yml`: remove clippy/archive target caches and SHA bootstrap identity; keep one artifact digest; add governed compiler-cache setup/stats to archive/sidecar builds.
- `.github/workflows/flaky-test-detection.yml`: remove two BVS target restores; add read-only compiler-cache setup/stats and effective compile env to both BVS jobs.
- `.github/workflows/flaky-test-smoke.yml`: remove two BVS target restores/digests while retaining existing read-only compiler-cache setup/stats in both BVS jobs.
- `scripts/verify_ci_workflow_hygiene.py`: invert the old BVS target-cache contract, govern exact S3 identity, sccache roles/fail-open/profile inputs, and unchanged test topology.
- `scripts/test_verify_ci_workflow_hygiene.py`: mutation tests for each independent workflow invariant.
- `scripts/ci_workflow_hygiene_test_helpers.py`: update only fixture/allowlist text mechanically required by the workflow verifier tests.
- `scripts/rust_verification.py`: report measured managed-target bytes and distinguish measured from unmeasured reclaimability.
- `scripts/test_rust_verification_cache_retention.py`: pin the telemetry schema, both 32 GiB policies, and existing `CARGO_INCREMENTAL` scrub behavior.

Read-only authorities: `.github/actions/sccache-setup/action.yml`, `.github/actions/sccache-stats/action.yml`, `scripts/sccache_eligibility.py`, `ci/sccache-location.toml`, `ci/rust-ci-inputs.toml`, `scripts/ci_input_sets.py`, both `ci/rust-verification.toml` files, storage governance, `.github/workflows/ci.yml`, and all root cache code.

---

### Task 1: Make disk-pressure telemetry truthful

**Files:**
- Modify: `scripts/rust_verification.py` (`cache_prune_payload`, `refusal_payload`, `disk_preflight_refusal_payload`, and payload builders that expose `reclaimable_bytes`)
- Test: `scripts/test_rust_verification_cache_retention.py`

**Interfaces:**
- Consumes: `cache_status_payload(repo) -> dict[str, Any]`, whose `total_bytes` is the scanned managed-target allocation used by `cache_pressure`.
- Produces: refusal JSON with `total_bytes: int` when target scanning completed, `reclaimability_measured: bool`, and `reclaimable_bytes: int | None`.

- [ ] **Step 1: Add focused failing telemetry tests**

Add assertions that construct an over-limit managed target and call `disk_preflight_refusal_payload` directly:

```python
payload = owner.disk_preflight_refusal_payload(repo, owner.load_policy(repo))
assert payload is not None
assert payload["refusal_code"] == "disk_pressure"
assert payload["total_bytes"] == disk_bytes(target / "debug" / "oversize.bin")
assert payload["reclaimability_measured"] is False
assert payload["reclaimable_bytes"] is None
```

Extend prune/refusal tests so a completed candidate classification reports `reclaimability_measured is True` with an integer (including integer zero), while any payload emitted before candidate classification reports `False` and `None`. Retain `assert_all_managed_cache_policies_are_bounded_to_30_gib` and explicitly assert both policy paths contain `soft_limit_bytes = 32212254720`.

- [ ] **Step 2: Run the focused self-test and capture RED**

Run:

```bash
python3 scripts/test_rust_verification_cache_retention.py
```

Expected: FAIL because `disk_preflight_refusal_payload` lacks `total_bytes`/`reclaimability_measured` and reports the fabricated zero.

- [ ] **Step 3: Implement one consistent reclaimability schema**

Use the scanned status in `disk_preflight_refusal_payload`:

```python
return {
    "candidates": [],
    "dry_run": False,
    "filesystem": status["filesystem"],
    "legacy_target_dir": str(repo / "target"),
    "managed_target_dir": target,
    "pressure_reasons": status["pressure_reasons"],
    "total_bytes": int(status["total_bytes"]),
    "reclaimability_measured": False,
    "reclaimable_bytes": None,
    "refusal_code": "disk_pressure",
    "refusal_reason": "managed Rust command refused before execution because free disk/cache pressure failed preflight",
    "refused": True,
    "target_dir": target,
    "thresholds": status["thresholds"],
}
```

Add `reclaimability_measured: True` to the normal `cache_prune_payload` result because it classified candidates. Normalize other builders in this file that expose `reclaimable_bytes` so pre-classification refusals cannot continue to claim measured zero. Do not change any refusal threshold, exit code, pruning authority, or deletion behavior.

- [ ] **Step 4: Run telemetry tests GREEN**

Run:

```bash
python3 scripts/test_rust_verification_cache_retention.py
git diff --check
```

Expected: both exit 0; output ends with `OK: Rust verification cache retention self-tests passed.`

- [ ] **Step 5: Commit the telemetry slice**

```bash
git add scripts/rust_verification.py scripts/test_rust_verification_cache_retention.py
git commit -m "fix(ci): report measured BVS disk pressure"
```

---

### Task 2: Delete Backtester whole-target caches and preserve exact artifact identity

**Files:**
- Modify: `.github/workflows/backtester-ci.yml` (`clippy`, `test-archive`)
- Modify: `scripts/verify_ci_workflow_hygiene.py` (`backtester_managed_target_cache_errors`, `backtester_test_shard_errors` and BVS archive fragments)
- Test: `scripts/test_verify_ci_workflow_hygiene.py` (BVS cache mutation coverage)

**Interfaces:**
- Produces: one `bvs_artifact_inputs` output used only by nextest/sidecar S3 keys and `DIGEST` metadata.
- Produces: no BVS `actions/cache/restore`, `actions/cache/save`, `restore-keys`, cache-hit guard, target-cache digest, or `managed-target-bvs-*` string.

- [ ] **Step 1: Rewrite the verifier tests for the new contract**

Replace tests that require `bvs_cache_inputs`, bootstrap SHA identity, target restores, and main-only target saves. Add independent mutations that must fail when they:

```text
inject managed-target-bvs-* into clippy or test-archive
inject actions/cache/restore or actions/cache/save for the BVS managed target
inject restore-keys or bootstrap-${GITHUB_SHA}
remove python3 scripts/ci_input_sets.py hash backtester_cache
cross-wire either S3 CACHE_KEY or DIGEST away from steps.bvs_artifact_inputs.outputs.digest
remove nextest/sidecar metadata validation or main-only S3 save authority
change Swatinem/rust-cache cache-targets from false
remove any archive, sidecar, partition, MinIO, issue-specific, or gate step
```

Name the consolidated test `assert_backtester_bvs_target_cache_removal_and_artifact_identity_contract` and call it from `main()`.

- [ ] **Step 2: Run the workflow self-test and capture RED**

Run:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
```

Expected: FAIL because the current workflow/verifier still requires target restores, target saves, shared digest identity, and the SHA bootstrap branch.

- [ ] **Step 3: Invert the verifier contract**

Refactor `backtester_managed_target_cache_errors` into the BVS cache-family contract (rename only if all callers/tests are changed together):

```python
for forbidden in ("managed-target-bvs-", "bootstrap-${GITHUB_SHA}"):
    if forbidden in text:
        errors.append(f"backtester BVS contract must not contain {forbidden}")

for job_id, job_lines in parse_jobs(text).items():
    for block in action_blocks(job_lines, "actions/cache/restore@") + action_blocks(job_lines, "actions/cache/save@"):
        if "steps.crate_target.outputs.dir" in uncommented_text(block):
            errors.append(f"backtester {job_id} must not restore or save the BVS managed target")
```

In `backtester_test_shard_errors`, delete target-cache-required fragments and require one `bvs_artifact_inputs` digest step, exact key/metadata use, no bootstrap branch, unchanged full-source integrity, payload builds, tests, partitions, and gate dependencies.

- [ ] **Step 4: Remove the Backtester cache family**

In `clippy`, delete `Resolve crate managed target dir`, `Compute BVS cache input hash`, `Restore BVS clippy managed target cache`, and `Save BVS clippy managed target cache`. Keep setup, registry/git cache with `cache-targets: false`, and `just bte-clippy` unchanged.

In `test-archive`, replace the conditional digest with:

```yaml
      - name: Compute BVS artifact input hash
        id: bvs_artifact_inputs
        shell: bash
        run: |
          echo "digest=$(python3 scripts/ci_input_sets.py hash backtester_cache)" >> "$GITHUB_OUTPUT"
```

Use `${{ steps.bvs_artifact_inputs.outputs.digest }}` in both S3 `CACHE_KEY` values, both `DIGEST` values, and both save paths. Delete `Restore archive build target cache` and `Save archive build target cache` completely. Keep `Resolve crate managed target dir` because sidecar extraction and archive execution still use it. Do not add sccache to the primary producer yet; this commit is the immutable cold-baseline candidate.

- [ ] **Step 5: Run the Backtester static contract GREEN**

Run:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

Expected: all exit 0; no `managed-target-bvs`, `restore-keys`, or `bootstrap-${GITHUB_SHA}` remains in `backtester-ci.yml`.

- [ ] **Step 6: Commit the Backtester correctness slice**

```bash
git add .github/workflows/backtester-ci.yml scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py
git commit -m "fix(ci): remove BVS target caches"
```

---

### Task 3: Remove flaky target restores and make all flaky BVS compiles read-only sccache consumers

**Files:**
- Modify: `.github/workflows/flaky-test-detection.yml` (jobs `flaky-detection-rust-backtester`, `flaky-detection-rust-backtester-issue-789`)
- Modify: `.github/workflows/flaky-test-smoke.yml` (jobs `flaky-smoke-rust-backtester`, `flaky-smoke-rust-backtester-issue-789`)
- Modify: `scripts/verify_ci_workflow_hygiene.py` (`BVS_BACKTESTER_ALLOWED_SIBLING_RUN_STEPS`, `BVS_BACKTESTER_ALLOWED_USES_STEPS`, `flaky_test_detection_workflow_errors`)
- Test: `scripts/test_verify_ci_workflow_hygiene.py`
- Modify only if fixture literals require it: `scripts/ci_workflow_hygiene_test_helpers.py`

**Interfaces:**
- Produces: all four BVS flaky jobs call the shared setup with only `role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}` and report stats after their required compile/test step.
- Preserves: matrices, partitions, MinIO, JUnit fallback, Mergify reporting, exit-code capture, and issue-789 selection.

- [ ] **Step 1: Add failing per-job mutation tests**

For each of the four BVS flaky jobs, mutate the real workflow independently and require an error when it has a target cache/digest, omits sccache setup/stats, passes `write-role-arn`, changes the read role, hardcodes `BOLT_RUST_VERIFICATION_SCCACHE`, removes an applicable debug-profile value, or adds any raw `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `RUSTC_WRAPPER`, or `RUSTC_WORKSPACE_WRAPPER`.

Retain separate mutations proving that cache ineligibility cannot remove/guard the `Run tests` step and that JUnit/MinIO/Mergify topology is unchanged.

- [ ] **Step 2: Run the self-test and capture RED**

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
```

Expected: FAIL for the two detection jobs, which lack sccache, and for the stale allowlist that permits `Compute BVS cache input hash` and `Restore test target cache`.

- [ ] **Step 3: Update the flaky-workflow verifier and allowlist**

Remove `Compute BVS cache input hash` and `Restore test target cache` from the BVS allowlists. Require these exact shared-action fragments on every BVS flaky job:

```yaml
      - name: Setup read-only sccache
        id: sccache
        uses: ./.github/actions/sccache-setup
        with:
          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}
```

Require `BOLT_RUST_VERIFICATION_SCCACHE` to be conditional on `steps.sccache.outputs.enabled`, applicable test/dev debug profile values, and an `if: always()` stats action after `Run tests`. Reject `write-role-arn`, inline AWS/cache setup, alternate locations, raw flags/wrappers, and conditional test skipping.

- [ ] **Step 4: Update both flaky workflows**

Delete `Compute BVS cache input hash` and `Restore test target cache` from all four BVS jobs. Keep registry/git cache `cache-targets: false`.

For both detection jobs, add job permissions:

```yaml
    permissions:
      contents: read
      id-token: write
```

Add the shared read-only setup before `Run tests`, set the existing applicable debug-profile variables plus:

```yaml
          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}
```

and add `./.github/actions/sccache-stats` after `Run tests` with `if: always()`. In smoke, retain the already-correct setup/env/stats blocks and only remove the target digest/restore.

- [ ] **Step 5: Run static verification GREEN**

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

Expected: all exit 0; all three workflows contain zero `managed-target-bvs-*`, zero BVS `restore-keys`, and no BVS target save/restore.

- [ ] **Step 6: Commit the flaky consumer slice**

```bash
git add .github/workflows/flaky-test-detection.yml .github/workflows/flaky-test-smoke.yml scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py scripts/ci_workflow_hygiene_test_helpers.py
git commit -m "fix(ci): use read-only sccache for BVS flaky jobs"
```

If `scripts/ci_workflow_hygiene_test_helpers.py` is unchanged, omit it from `git add`.

---

### Task 4: Internally review and capture the immutable cold baseline

**Files:**
- Review only: all Task 1-3 files
- Record outside the implementation diff: PR report with immutable SHA, run/job IDs, step timestamps/durations, S3 restore outcomes, target telemetry, and cache inventory.

**Interfaces:**
- Produces: a reviewed, coherent correctness-only head with no BVS target restore and no primary Backtester sccache wrapper.
- Produces: cold compile evidence, not merge readiness and not the warm performance claim.

- [ ] **Step 1: Run every cheap local gate from a clean worktree**

```bash
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just fmt-check
just ci-lint-workflow
just source-fence-static
git diff --check
git status --short
```

Expected: all gates exit 0 and `git status --short` is empty. Do not run local Rust build/test/clippy.

- [ ] **Step 2: Run a separate internal implementation review before publication**

Give the reviewer the approved spec SHA, plan SHA, base/head SHAs, changed-file list, and Tasks 1-3 diff. The reviewer must independently verify target-cache deletion, exact artifact identity, truthful telemetry, read-only flaky authority, complete tests, and scope. Resolve every finding in a new commit and repeat the cheap gates; do not push while findings remain.

- [ ] **Step 3: Publish the reviewed cold-baseline head as a draft PR**

```bash
just sandbox-safe-push
```

Open one draft PR for this BVS CI slice. Record the exact remote head SHA. The PR body must state that primary sccache wiring and post-main performance qualification remain in this same accepted slice; it must not claim to close #1367 or #1354.

- [ ] **Step 4: Obtain exact-head remote cold proof**

Use the repository-approved remote verifier from the pushed head:

```bash
just verify-remote
```

Accept only an exact-SHA full Backtester run in which `bvs-test archive` and `backtester-gate` complete successfully. A draft/iteration run that skips `bvs-test archive` is not evidence; if policy yields only skipped heavy jobs, stop and have the reviewer/operator use the governed full-CI/ready-state route without requesting external review before green.

The immutable baseline is valid only if both full-source S3 payloads miss and the archive/sidecar build steps run without `RUSTC_WRAPPER`. Record the two GitHub-reported compile-step durations, their sum, full gate result, target `total_bytes`, and `reclaimability_measured` state. The Task 1 change is in `backtester_cache`, so this head should mint a new artifact digest; if either payload hits, do not relabel it cold—stop and obtain a reviewed source-only, dependency/toolchain-identical measurement head.

- [ ] **Step 5: Capture zero-budget baseline inventory**

Use the GitHub Actions cache API read-only and record exact keys, sizes, last-access times, total repository bytes, and the count/bytes for keys beginning `managed-target-bvs-`. Expected BVS result: zero entries and zero bytes. Repository total is report-only and does not fail this slice.

---

### Task 5: Wire governed sccache into the primary archive/sidecar producer

**Files:**
- Modify: `.github/workflows/backtester-ci.yml` (`test-archive` only)
- Modify: `scripts/verify_ci_workflow_hygiene.py` (BVS archive sccache contract)
- Test: `scripts/test_verify_ci_workflow_hygiene.py`

**Interfaces:**
- Consumes: shared `sccache-setup` outputs `enabled`, `eligible`, and `cache-mode`.
- Produces: one setup covering missing archive and/or sidecar compilation, conditional `BOLT_RUST_VERIFICATION_SCCACHE`, and one always-run stats record after both compile opportunities.

- [ ] **Step 1: Add failing primary-producer mutation tests**

Require errors when a mutation removes the shared setup/stats action, changes its active guard, omits the read role or write role, gives untrusted events write authority, hardcodes the opt-in, removes `CARGO_PROFILE_TEST_DEBUG: "0"` or `CARGO_PROFILE_DEV_DEBUG: "0"`, adds a raw flag/wrapper override, puts stats before either build, or allows setup failure to skip a build/test.

Also retain action-level fail-open evidence for missing role/location, AWS failure, install failure, and server-start failure; do not duplicate or edit the shared action when its existing tests already prove those conditions.

- [ ] **Step 2: Run the workflow self-test and capture RED**

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
```

Expected: FAIL because the primary BVS archive producer does not yet use governed sccache.

- [ ] **Step 3: Add the exact primary setup and compile opt-in**

After cargo-nextest installation and before either possible compile, add:

```yaml
      - name: Setup governed sccache
        id: sccache
        uses: ./.github/actions/sccache-setup
        with:
          active: ${{ (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && 'true' || 'false' }}
          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}
          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}
```

Add the conditional opt-in to both build steps while preserving their existing profile inputs:

```yaml
          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}
```

After both possible compile steps, add one `Print sccache stats` step with `if: always()` and `enabled: ${{ steps.sccache.outputs.enabled || 'false' }}`. Keep artifact S3 save, metadata validation, payload checks, sidecar extraction, all partitions/tests, and gate wiring unchanged.

- [ ] **Step 4: Enforce the primary authority/fail-open contract**

In `backtester_test_shard_errors`, use named-step parsing to require the exact shared actions and ordering. Require both role inputs; rely on the unchanged `scripts/sccache_eligibility.py` contract for main `read_write` versus PR/MQ/workflow consumers `read_only`. Reject inline AWS credentials, alternate locations, raw wrappers/flags, cache-controlled skip guards, and any reintroduced target cache. Require the existing managed-env scrub test rather than a workflow `CARGO_INCREMENTAL` value.

- [ ] **Step 5: Run focused static tests GREEN**

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

Expected: all exit 0; the primary producer is read-only on PR/MQ, read/write only on trusted main, and cold-compiles when setup outputs `enabled=false`.

- [ ] **Step 6: Commit the primary compiler-cache slice**

```bash
git add .github/workflows/backtester-ci.yml scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py
git commit -m "perf(ci): reuse BVS compiler outputs"
```

---

### Task 6: Final internal review, exact-head proof, and external review handoff

**Files:**
- Review only: complete base-to-head diff
- Report: PR body/comment with immutable evidence; no new repository file unless the approved issue requires one.

**Interfaces:**
- Produces: final exact-head safety/authority proof and a clean external-review package.
- Does not produce: post-main 20% qualification.

- [ ] **Step 1: Run the complete cheap local gate set**

```bash
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just fmt-check
just deny
just ci-lint-workflow
just source-fence-static
git diff --check
git status --short
```

Expected: all exit 0 and worktree clean. Do not run local compile-heavy Rust.

- [ ] **Step 2: Run a fresh separate implementation review before pushing the final head**

The reviewer receives the approved spec/plan SHAs, cold-baseline evidence, every commit, and the complete diff. They must check every invariant and explicitly verify rollback/fail-open semantics: removing/denying cache authority yields the same required cold compile; it does not restore a target tree, skip a test, add a backend, or weaken either 32 GiB guard. Resolve all findings and rerun Step 1 before publication.

- [ ] **Step 3: Publish the reviewed final head**

```bash
just sandbox-safe-push
```

Record and verify the exact remote SHA. Do not request external review yet.

- [ ] **Step 4: Obtain exact-head remote merge proof**

```bash
just verify-remote
```

Require exact-head root gates and the full Backtester `bvs-test archive` plus `backtester-gate`; distinguish required full checks from iteration/skipped jobs. Remote logs must show no BVS target restore/save, correct sccache mode, required archive/sidecar behavior, all tests, unchanged disk limit, and stats or honest disabled state. A cache failure is acceptable only when the full cold compilation and gate succeed.

- [ ] **Step 5: Request final external review only after exact-head CI is green**

The request includes base/head SHAs, changed files, spec/plan approvals, cold-baseline run, exact-head gate URLs, local static results, internal-review disposition, known S3-growth and runner-image residuals, and the explicit scope exclusions. Request native review from the repository-required code owner and verify the active main ruleset before queueing. Do not merge without required approval and resolved threads.

- [ ] **Step 6: Land through governed merge mechanics**

Use `just merge-queue <pr-number>` only after exact-head green checks, native approval, thread resolution, and queue preflight all pass. Do not admin-merge or treat the downstream #1367 branch as evidence until this slice is authoritative on `main`.

---

### Task 7: Post-main inventory and performance qualification

**Files:**
- No implementation files.
- Evidence: immutable GitHub run/job logs and API-backed cache inventories attached to the CI slice report.

**Interfaces:**
- Consumes: trusted main `read_write` compiler-cache population and later dependency/toolchain-identical source-only main heads.
- Produces: either a passed `>= 20%` performance qualification or an explicit stopped/unsolved performance result.

- [ ] **Step 1: Record post-main correctness and writer authority**

On the landed main SHA, record a full green Backtester run, `sccache cache_mode=read_write`, cache enabled/disabled state, compile requests, cacheable requests, hits, read errors, archive/sidecar step durations, and full gate result. The first trusted main compile may populate objects; it is not automatically the warm comparison.

- [ ] **Step 2: Record the first post-main Actions-cache inventory**

Query the Actions-cache API and assert zero `managed-target-bvs-*` keys, bytes, and saves. Report repository totals to the existing storage-tripwire owner without making them this slice's pass/fail condition.

- [ ] **Step 3: Wait for a comparable source-only main head**

Select a later main head with the same runner class, OS, architecture, Rust toolchain, dependency graph, debug profiles, job-count policy, archive/sidecar commands, and no relevant compile-input change. Both exact full-source payloads must miss so both compile steps execute. Reject incomparable or payload-hit runs rather than adjusting the formula.

- [ ] **Step 4: Calculate and record warm performance**

Use GitHub-reported step timestamps only:

```text
cold_compile_seconds = cold_archive_build_seconds + cold_sidecar_build_seconds
warm_compile_seconds = warm_archive_build_seconds + warm_sidecar_build_seconds
improvement_percent = 100 * (cold_compile_seconds - warm_compile_seconds) / cold_compile_seconds
```

The warm run must be full green with sccache enabled, appropriate mode, nonzero requests/cacheable requests/hits, and zero read errors. Queue, checkout, downloads, tests, stats, and uploads remain separately reported and excluded.

- [ ] **Step 5: Apply the hard qualification ruling**

If improvement is at least 20%, record performance qualified with exact run/job/step/counter evidence. If it is below 20%, retain the landed no-target-cache safety correction, state that normal-change performance is unsolved, and stop. Do not raise limits, restore a target cache, change root allocation, or begin a broader design without a new spec and explicit user approval.

- [ ] **Step 6: Complete the four-point zero-budget observation**

After each of two later dependency/toolchain-identical source-only main heads, capture another Actions-cache inventory. Both must continue to show zero BVS target keys/bytes/saves and no BVS eviction/re-save loop. Report S3 object growth as the accepted in-repo-unbounded residual; do not add lifecycle/delete work to this slice.

---

## Plan Self-Review Checklist

- [ ] Every approved spec invariant maps to a task and explicit evidence.
- [ ] No task changes a read-only authority or out-of-scope product/runtime file.
- [ ] No step requires local compile-heavy Rust or treats an iteration/skipped Backtester job as proof.
- [ ] Cold correctness is proven before primary sccache wiring; final exact-head proof is repeated after wiring.
- [ ] Every publishable head receives a separate internal implementation review first.
- [ ] External review starts only after final exact-head required CI is green.
- [ ] Post-main performance evidence is not misrepresented as a prerequisite for landing the correctness fix.
- [ ] A sub-20% result stops the performance claim without undoing the safety correction.
- [ ] No placeholder, deferred implementation debt, alternate cache, threshold increase, or scope expansion remains.
