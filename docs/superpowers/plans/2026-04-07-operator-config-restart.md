# Operator Config Restart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the patch-driven operator-config follow-up with one coherent materialization API, single-source-of-truth tests, and truthful operator workflow docs.

**Architecture:** The library owns parsing, rendering, diffing, atomic writes, permission repair, and outcome reporting for the operator-config lane. The CLI is a thin wrapper, pure render tests stay internal, tempdir-based behavior tests own the filesystem contract, and the tracked operator template becomes the runtime seam source of truth.

**Tech Stack:** Rust, Cargo tests, shell verification, `just`, TOML, filesystem permissions

---

### Task 1: Lock The Public Boundary And Outcome Contract

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/live_config.rs`
- Modify: `src/bin/render_live_config.rs`

- [ ] **Step 1: Write the failing behavior test for the new public API surface**

Add an integration test in `tests/render_live_config.rs` that calls `bolt_v2::materialize_live_config(...)` and matches on a `MaterializationOutcome::Created` result for a tempdir output.

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test --test render_live_config -q`
Expected: FAIL because `materialize_live_config` and `MaterializationOutcome` do not exist yet.

- [ ] **Step 3: Implement the public boundary and enum**

Expose one honest entrypoint from `src/lib.rs`, move the outcome enum into the library-owned live-config module, and make the CLI print the library outcome instead of owning the contract.

- [ ] **Step 4: Re-run the focused test**

Run: `cargo test --test render_live_config -q`
Expected: PASS for the new `Created` path or fail only on the next missing contract detail.

### Task 2: Implement Full Four-State Materialization Semantics

**Files:**
- Modify: `src/live_config.rs`
- Modify: `tests/render_live_config.rs`

- [ ] **Step 1: Write the failing tempdir-based behavior tests**

Cover:
- relative output path
- nested output path
- `Created`
- `Updated`
- `PermissionsRepaired`
- `Unchanged` with stable mtime

- [ ] **Step 2: Run the renderer behavior suite to verify it fails**

Run: `cargo test --test render_live_config -q`
Expected: FAIL on the not-yet-implemented update, unchanged, and permission-repair semantics.

- [ ] **Step 3: Implement materialization behavior**

Implement:
- parent-directory creation
- render-to-string comparison
- sibling temp-file staging
- read-only enforcement on the staged file
- atomic rename
- no-rewrite unchanged path
- permission-only repair path

- [ ] **Step 4: Re-run the renderer behavior suite**

Run: `cargo test --test render_live_config -q`
Expected: PASS.

### Task 3: Move Pure Mapping Verification To Internal Unit Tests

**Files:**
- Modify: `src/live_config.rs`
- Modify: `tests/config_schema.rs`

- [ ] **Step 1: Write the failing internal unit tests for tracked-template mapping and defaults**

Inside `src/live_config.rs`, add unit tests that:
- load `config/live.local.example.toml`
- render runtime TOML
- assert client kinds and names
- assert `client_name` threads to both client entries and strategy `client_id`
- assert `event_slug` maps to `event_slugs`
- assert timeout threading
- assert `instrument_id`, `signature_type`, `funder`, and secret paths
- assert a minimal-defaults input renders a valid runtime `Config`

- [ ] **Step 2: Run the library tests to verify at least one new test fails first**

Run: `cargo test live_config --lib -q`
Expected: FAIL until the internal helpers and assertions line up with the new contract.

- [ ] **Step 3: Refactor the existing schema tests**

Remove or simplify integration tests that only duplicate the operator-schema mapping already covered internally. Keep only tests that protect the public contract or runtime seam.

- [ ] **Step 4: Re-run the focused library tests**

Run: `cargo test live_config --lib -q`
Expected: PASS.

### Task 4: Move The Runtime Seam To The Tracked Template

**Files:**
- Modify: `tests/polymarket_bootstrap.rs`
- Modify: `tests/cli.rs`
- Delete: `config/examples/polymarket-exec-tester.toml`

- [ ] **Step 1: Write the failing seam changes against the tracked template**

Update the runtime seam test to materialize from `config/live.local.example.toml` into a temp file, load that file via `Config::load`, and build the real data client, exec client, strategy, and `LiveNode` seam from it.

- [ ] **Step 2: Replace any remaining operator-lane test dependency on the hand-maintained runtime example**

Move CLI completeness checks to either:
- a temp generated runtime config from the tracked template, or
- a smaller inline runtime config when the test is explicitly about CLI error handling rather than operator workflow.

- [ ] **Step 3: Remove the stale runtime example**

Delete `config/examples/polymarket-exec-tester.toml` once no operator-lane test depends on it.

- [ ] **Step 4: Run the focused seam tests**

Run: `cargo test --test polymarket_bootstrap -q`
Run: `cargo test --test cli -q`
Expected: PASS.

### Task 5: Make Docs And Workflow Text Match The Real Lane

**Files:**
- Modify: `justfile`
- Modify: `config/live.local.example.toml`
- Modify: `Tasks/next-session-prompt.md`
- Modify: `Tasks/next-session-deploy.md`
- Modify: `Tasks/next-session-featherwriter.md`

- [ ] **Step 1: Write the failing documentation checklist**

Create a local checklist from the design doc and confirm each touched surface says:
- local source of truth = `config/live.local.toml`
- tracked template = `config/live.local.example.toml`
- generated artifact = `config/live.toml`
- `just live-check` is completeness-only
- `just live-resolve` performs actual resolution
- `just live` runs with generated config

- [ ] **Step 2: Update the operator-facing docs**

Rewrite stale statements that imply `config/live.toml` is hand-authored or authoritative. Clarify that unsupported sections do not survive into the generated artifact until the operator schema supports them.

- [ ] **Step 3: Re-run quick grep validation**

Run: `rg -n "single source of truth|live.toml|live.local|live-check|live-resolve" justfile config Tasks tests`
Expected: only truthful workflow statements remain.

### Task 6: Full Verification

**Files:**
- No code changes expected

- [ ] **Step 1: Run focused renderer and seam tests**

Run: `cargo test --test render_live_config -q`
Run: `cargo test --test config_schema -q`
Run: `cargo test --test polymarket_bootstrap -q`

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -q`
Expected: PASS.

- [ ] **Step 3: Run the release-path verification**

Run: `bash tests/verify_build.sh`
Expected: PASS, or a clearly identified environment-only failure with evidence.
