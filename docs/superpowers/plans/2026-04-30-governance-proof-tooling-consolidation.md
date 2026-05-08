# Governance Proof Tooling Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate current-main-valid governance, proof, CI evidence, and NT pointer-probe tooling work into one clean branch without mixing in trading runtime behavior or Bolt-v3 feature work.

**Architecture:** Start from `origin/main` and port only current-valid pieces from old branches/PRs. Avoid merging old branches wholesale because several are stale, partially superseded, or based on older control-plane state. Keep product/runtime branches (`#231`, `#238`, issue `#126`, Bolt-v3 PR `#248`) outside this lane.

**Tech Stack:** Rust, GitHub Actions, Python helper scripts, Markdown process docs.

---

### Task 1: Restore Process Proof Documents

**Files:**
- Create: `docs/process/2026-04-19-bolt-v2-proof-matrix-v1.md`
- Create: `docs/process/2026-04-30-governance-proof-tooling-lane.md`
- Create: `docs/superpowers/plans/2026-04-30-governance-proof-tooling-consolidation.md`

- [x] **Step 1: Port proof matrix from deleted branch SHA**

Source commit: `d38b96c57e804f419db6be17013c516fb0619f53`.

Expected file:
`docs/process/2026-04-19-bolt-v2-proof-matrix-v1.md`

- [x] **Step 2: Add lane scope document**

Expected file:
`docs/process/2026-04-30-governance-proof-tooling-lane.md`

The scope document must say graphify is not replayed from `issue-573-graphify-install` as-is and must keep Bolt-v3/product runtime work out of this lane.

- [ ] **Step 3: Verify doc hygiene**

Run:

```bash
git diff --check
```

Expected: exit `0`.

### Task 2: Port CI Mtime Proof Work From PR #201 If Still Valid

**Files To Inspect Before Editing:**
- `.github/actions/setup-environment/action.yml`
- `.github/workflows/ci.yml`
- `scripts/ci_tracked_mtime_manifest.py`
- `tests/test_ci_tracked_mtime_manifest.py`

- [x] **Step 1: Compare old branch against current main**

Run:

```bash
git diff origin/main...origin/issue-195-nextest-mtime-normalization -- .github/actions/setup-environment/action.yml .github/workflows/ci.yml scripts/ci_tracked_mtime_manifest.py tests/test_ci_tracked_mtime_manifest.py
```

Expected: understand the exact helper and workflow wiring before editing.

- [x] **Step 2: Port only current-main-valid helper/test changes**

Do not port stale workflow output names without checking current `.github/actions/setup-environment/action.yml`.

- [x] **Step 3: Verify helper**

Run:

```bash
python3 -m py_compile scripts/ci_tracked_mtime_manifest.py
python3 tests/test_ci_tracked_mtime_manifest.py
```

Expected: both exit `0`.

### Task 3: Port Selector Proof Closure From PR #202 If Still Valid

**Files To Inspect Before Editing:**
- `src/clients/polymarket.rs`

- [x] **Step 1: Compare old proof-only branch against current main**

Run:

```bash
git diff origin/main...origin/issue-198-proof-closure -- src/clients/polymarket.rs
```

Expected: identify tests only; reject runtime behavior changes.

- [x] **Step 2: Port only proof tests**

If the diff changes production behavior, stop and reclassify instead of porting silently.

- [x] **Step 3: Run focused tests**

Run:

```bash
cargo test --lib clients::polymarket::tests:: -- --nocapture
```

Expected: exit `0`.

### Task 4: Port Same-SHA Smoke Tag Proof Work From PR #210 If Still Valid

**Files To Inspect Before Editing:**
- `.github/workflows/ci.yml`
- `tests/ci_same_sha_smoke_tag.rs`

- [x] **Step 1: Compare old branch against current main**

Run:

```bash
git diff origin/main...origin/issue-205-smoke-tag-proof -- .github/workflows/ci.yml tests/ci_same_sha_smoke_tag.rs
```

Expected: understand workflow changes and tests.

- [x] **Step 2: Port current-main-valid workflow contract and test**

Keep this limited to same-SHA artifact reuse. Do not include deploy/product changes.

- [x] **Step 3: Run focused verification**

Run:

```bash
cargo test --test ci_same_sha_smoke_tag -- --nocapture
```

Expected: exit `0`.

### Task 5: Evaluate NT Pointer-Probe Engine From Issue #163

**Files To Inspect Before Editing:**
- `src/nt_pointer_probe/classify.rs`
- `src/nt_pointer_probe/engine.rs`
- `src/nt_pointer_probe/evidence.rs`
- `src/nt_pointer_probe/inventory.rs`
- `src/nt_pointer_probe/upstream.rs`
- `tests/nt_pointer_probe_engine.rs`

- [x] **Step 1: Compare direct current-main delta**

Run:

```bash
git diff origin/issue-163-nt-pointer-probe-engine..origin/main -- src/nt_pointer_probe tests/nt_pointer_probe_engine.rs
```

Expected: confirm which engine modules are absent from current `main` and whether current `main` control-plane changes conflict.

- [x] **Step 2: Decide whether to port now**

Decision: do not port in this pass. `issue-163-nt-pointer-probe-engine` depends on a dry-run CLI and fixture/control-plane contract that no longer matches current `main`; the attempted current-main adaptation failed `cargo test --test nt_pointer_probe_engine -- --nocapture`.

Port only if the engine still matches the current control-plane design and can be tested independently.

- [x] **Step 3: Run focused verification if ported**

Not applicable after the no-port decision. The failed adaptation was reverted.

Run:

```bash
cargo test --test nt_pointer_probe_engine -- --nocapture
cargo test --test nt_pointer_probe_control_plane -- --nocapture
```

Expected: both exit `0`.

### Task 6: Final Branch Verification

- [ ] **Step 1: Run formatting/check commands**

Run:

```bash
git diff --check
cargo fmt --check
```

Expected: both exit `0`.

- [ ] **Step 2: Run focused tests for every ported area**

Run every focused command listed above for tasks that were ported.

- [ ] **Step 3: Summarize exact scope**

Before PR creation, list:

- files changed
- old branch/PR source for each change
- intentionally excluded branches/items
- verification commands and results
