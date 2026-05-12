# Bolt-v3 Phase 8 Tiny-capital Canary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove one explicitly approved, tiny, cap-enforced live order through the production-shaped bolt-v3 path and NautilusTrader, with redacted evidence for submit, venue result, strategy-driven exit/cancel if needed, and restart reconciliation.

**Architecture:** Canary mode is not a separate trading architecture. It is the stacked bolt-v3 production path run with tiny TOML caps, mandatory decision evidence, armed submit admission, PR #305 live-canary gating, and an operator-only harness that records proof without owning NT lifecycle, adapter behavior, cache semantics, or reconciliation.

**Tech Stack:** Rust, NautilusTrader Rust API, existing bolt-v3 TOML loader, AWS SSM resolver boundary, `sha2`, `serde_json`, existing strategy/registry/admission modules, ignored operator tests, GitHub PR checks, patched `/private/tmp/no-mistakes-soak-bin`.

---

## Investigation Summary

- Current planning base is PR #319 head `9d50725a077a7e7790aa51dbabf150c1f18c9cd3`, stacked on PRs #306-#318. None of those stacked phases should be treated as merged into `main` until they actually merge.
- PR #319 remains blocked on T037: real SSM plus real venue no-submit readiness has not run, and no redacted real report exists yet.
- `src/main.rs` on the stacked Phase 7 head already routes `bolt-v2 run --config <path>` through `load_bolt_v3_config`, `build_bolt_v3_live_node`, and `run_bolt_v3_live_node`.
- `run_bolt_v3_live_node` already consumes `check_bolt_v3_live_canary_gate`, arms `BoltV3SubmitAdmissionState`, then enters `LiveNode::run`.
- `src/strategies/eth_chainlink_taker.rs` already records mandatory decision evidence and asks submit admission before calling NT `submit_order`.
- NT already owns submit, cancel APIs, startup reconciliation, periodic reconciliation, cache state, adapter wire behavior, and external-order registration. Bolt-v3 must not duplicate those surfaces.
- The patched no-mistakes binary currently reports no active run. Latest observed run for PR #319 branch was cancelled after review-step staleness and did not produce an `error_code` in `runs`.

## Hard Preconditions

Phase 8 implementation MUST NOT start until all of these are true:

1. PR #319 or its exact Phase 7 content is merged or accepted as the explicit base for a stacked Phase 8 PR.
2. T037 has produced a redacted real no-submit readiness report from real SSM and real NT venue connect/disconnect.
3. The `[live_canary]` block in the approved TOML points at that report and has `max_live_order_count` and `max_notional_per_order` set to tiny values.
4. The operator approval id supplied to the harness exactly matches `[live_canary].approval_id`.
5. Exact commit SHA, root TOML checksum, SSM path manifest hash, cap values, and approval id are recorded before any submit.
6. The branch is clean, pushed, and exact-head CI is green before requesting external review.

If any precondition fails, stop. Do not add mock venue worlds, Bolt-owned order state, or adapter workarounds.

## Scope

In scope:

- Add local fail-closed tests for the canary operator preconditions.
- Add the thinnest redacted canary evidence artifact needed to prove the live run.
- Add an ignored operator harness that uses the production bolt-v3 build/run path and submits at most one configured order through NT.
- Capture evidence from existing NT events/reports and existing strategy decision evidence.
- Prove any open-order cleanup uses strategy/NT cancel surfaces, not exec-engine-direct test commands.
- Prove restart reconciliation evidence comes from NT adapter state and NT reports, not Bolt reconciliation logic.

Out of scope:

- No Bolt-owned order lifecycle.
- No Bolt-owned reconciliation engine.
- No Bolt adapter fork or local venue behavior fork.
- No NT cache semantics fork.
- No new provider, market-family, or strategy hardcoding in core.
- No F13-style expansion into test-local scalar verifier rules.
- No backtesting or research analytics.
- No external review request until exact-head CI is green and the PR is clean.

## File Map

- Create `src/bolt_v3_tiny_canary_evidence.rs`: minimal redacted evidence structs, checksum helper, and artifact writer.
- Modify `src/lib.rs`: export `bolt_v3_tiny_canary_evidence`.
- Create `tests/bolt_v3_tiny_canary_preconditions.rs`: local fail-closed tests and source fences for the operator harness.
- Create `tests/bolt_v3_tiny_canary_operator.rs`: ignored real operator harness guarded by explicit environment variables and exact approval/config checks.
- Modify `specs/001-thin-live-canary-path/tasks.md`: mark T039-T045 only after each task has its verification evidence.
- Modify `specs/001-thin-live-canary-path/quickstart.md`: add the approved Phase 8 operator command only after the local harness exists.
- Modify `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`: classify new diagnostic/schema literals only if runtime-literal verification requires it.

## Task 0: Pre-implementation Investigation Gate

**Files:**
- Inspect: `src/main.rs`
- Inspect: `src/bolt_v3_live_node.rs`
- Inspect: `src/bolt_v3_submit_admission.rs`
- Inspect: `src/strategies/eth_chainlink_taker.rs`
- Inspect: `~/.cargo/git/checkouts/nautilus_trader-*/48d1c12/crates/live/src/node.rs`
- Inspect: `~/.cargo/git/checkouts/nautilus_trader-*/48d1c12/crates/trading/src/strategy/mod.rs`

- [ ] **Step 1: Re-anchor exact stacked base**

Run:

```bash
git status --short --branch
git rev-parse HEAD
gh pr view 319 --json number,headRefName,headRefOid,baseRefName,isDraft,mergeable,statusCheckRollup
```

Expected:

- branch is `014-bolt-v3-phase7-no-submit-readiness` or Phase 8 branch stacked directly on it
- head is the intended Phase 7 base
- no uncommitted changes
- PR #319 is still the immediate predecessor branch

- [ ] **Step 2: Confirm T037 evidence exists**

Run:

```bash
rg -n "T037|no-submit readiness|report path|BOLT_V3_OPERATOR_APPROVAL_ID" \
  specs/001-thin-live-canary-path docs/superpowers/plans
```

Expected:

- T037 remains unchecked until an approved operator run exists
- if no redacted real report path exists, stop Phase 8 implementation

- [ ] **Step 3: Confirm NT-owned surfaces**

Run:

```bash
rg -n "fn submit_order|fn cancel_order|perform_startup_reconciliation|external_order_claims|register_external_order" \
  ~/.cargo/git/checkouts/nautilus_trader-*/48d1c12/crates/live/src/node.rs \
  ~/.cargo/git/checkouts/nautilus_trader-*/48d1c12/crates/trading/src/strategy/mod.rs
```

Expected:

- submit and cancel are NT strategy APIs
- reconciliation is NT live-node behavior
- any Phase 8 implementation records evidence from these surfaces instead of replacing them

- [ ] **Step 4: Record investigation result**

Add a short PR comment or local implementation note with:

- exact Phase 7 base SHA
- T037 report status
- NT submit/cancel/reconciliation source references
- decision to proceed or stop

Do not write runtime code if T037 is missing.

## Task 1: Red Tests For Canary Preconditions

**Files:**
- Create: `tests/bolt_v3_tiny_canary_preconditions.rs`
- Future operator file under test: `tests/bolt_v3_tiny_canary_operator.rs`

- [ ] **Step 1: Write failing source fence for operator-only live submit**

Create `tests/bolt_v3_tiny_canary_preconditions.rs`:

```rust
#[test]
fn tiny_canary_operator_harness_uses_production_bolt_v3_runner() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("tiny canary operator harness should exist");

    assert!(source.contains("#[ignore]"));
    assert!(source.contains("load_bolt_v3_config"));
    assert!(source.contains("build_bolt_v3_live_node"));
    assert!(source.contains("run_bolt_v3_live_node"));
    assert!(!source.contains("LiveNode::run("));
    assert!(!source.contains("node.run()"));
}

#[test]
fn tiny_canary_operator_harness_requires_explicit_operator_inputs() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("tiny canary operator harness should exist");

    for required in [
        "BOLT_V3_ROOT_TOML",
        "BOLT_V3_OPERATOR_APPROVAL_ID",
        "BOLT_V3_CANARY_COMMIT_SHA",
        "BOLT_V3_SSM_MANIFEST_SHA256",
        "BOLT_V3_CANARY_EVIDENCE_PATH",
    ] {
        assert!(source.contains(required), "missing required env var {required}");
    }
}

#[test]
fn tiny_canary_operator_harness_does_not_use_direct_exec_engine_cancel() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("tiny canary operator harness should exist");

    for forbidden in [
        "CancelAllOrders",
        "exec_engine_execute",
        "send_trading_command",
        "TradingCommand::Cancel",
    ] {
        assert!(!source.contains(forbidden), "forbidden direct cancel token {forbidden}");
    }
}
```

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture
```

Expected failure: missing `tests/bolt_v3_tiny_canary_operator.rs`.

- [ ] **Step 3: Commit red test**

Run:

```bash
git add tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "test: define tiny canary operator preconditions"
```

## Task 2: Minimal Redacted Canary Evidence Artifact

**Files:**
- Create: `src/bolt_v3_tiny_canary_evidence.rs`
- Modify: `src/lib.rs`
- Test: `tests/bolt_v3_tiny_canary_preconditions.rs`

- [ ] **Step 1: Add failing evidence serialization test**

Append to `tests/bolt_v3_tiny_canary_preconditions.rs`:

```rust
use std::path::PathBuf;

use bolt_v2::bolt_v3_tiny_canary_evidence::{
    CanaryConfigIdentity, CanaryRunEvidence, CanaryRunOutcome,
};

#[test]
fn tiny_canary_evidence_serializes_without_secret_values() {
    let evidence = CanaryRunEvidence {
        commit_sha: "9d50725a077a7e7790aa51dbabf150c1f18c9cd3".to_string(),
        config: CanaryConfigIdentity {
            root_toml_path: PathBuf::from("/redacted/root.toml"),
            root_toml_sha256: "0123456789abcdef".repeat(4),
            ssm_manifest_sha256: "abcdef0123456789".repeat(4),
            approval_id: "operator-approved-canary-001".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
        },
        outcome: CanaryRunOutcome::BlockedBeforeSubmit {
            reason: "precondition probe".to_string(),
        },
    };

    let encoded = serde_json::to_string_pretty(&evidence).expect("evidence should serialize");

    assert!(encoded.contains("root_toml_sha256"));
    assert!(encoded.contains("ssm_manifest_sha256"));
    assert!(!encoded.contains("private_key"));
    assert!(!encoded.contains("api_secret"));
    assert!(!encoded.contains("POLYMARKET_PK"));
}
```

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions tiny_canary_evidence_serializes_without_secret_values -- --nocapture
```

Expected failure: unresolved module `bolt_v3_tiny_canary_evidence`.

- [ ] **Step 3: Add minimal evidence structs**

Create `src/bolt_v3_tiny_canary_evidence.rs`:

```rust
use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryConfigIdentity {
    pub root_toml_path: PathBuf,
    pub root_toml_sha256: String,
    pub ssm_manifest_sha256: String,
    pub approval_id: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CanaryRunOutcome {
    BlockedBeforeSubmit { reason: String },
    Submitted { client_order_id: String },
    VenueAccepted { client_order_id: String, venue_order_id: String },
    VenueRejected { client_order_id: String, reason: String },
    Filled { client_order_id: String, venue_order_id: String },
    StrategyCancelSubmitted { client_order_id: String },
    ReconciledAfterRestart { client_order_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryRunEvidence {
    pub commit_sha: String,
    pub config: CanaryConfigIdentity,
    pub outcome: CanaryRunOutcome,
}

pub fn sha256_file_hex(path: &PathBuf) -> io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
```

Modify `src/lib.rs`:

```rust
pub mod bolt_v3_tiny_canary_evidence;
```

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions tiny_canary_evidence_serializes_without_secret_values -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit evidence artifact type**

Run:

```bash
git add src/bolt_v3_tiny_canary_evidence.rs src/lib.rs tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "feat: define tiny canary evidence artifact"
```

## Task 3: Ignored Operator Harness Skeleton

**Files:**
- Create: `tests/bolt_v3_tiny_canary_operator.rs`
- Test: `tests/bolt_v3_tiny_canary_preconditions.rs`

- [ ] **Step 1: Add ignored harness that cannot submit yet**

Create `tests/bolt_v3_tiny_canary_operator.rs`:

```rust
use std::{env, path::PathBuf};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_tiny_canary_evidence::{
        sha256_file_hex, CanaryConfigIdentity, CanaryRunEvidence, CanaryRunOutcome,
    },
};

#[test]
#[ignore = "requires explicit operator approval, real SSM, real venue, and tiny-capital risk acceptance"]
fn operator_approved_tiny_canary_uses_production_bolt_v3_path() {
    let root_toml = PathBuf::from(
        env::var("BOLT_V3_ROOT_TOML").expect("BOLT_V3_ROOT_TOML is required"),
    );
    let approval_id = env::var("BOLT_V3_OPERATOR_APPROVAL_ID")
        .expect("BOLT_V3_OPERATOR_APPROVAL_ID is required");
    let commit_sha = env::var("BOLT_V3_CANARY_COMMIT_SHA")
        .expect("BOLT_V3_CANARY_COMMIT_SHA is required");
    let ssm_manifest_sha256 = env::var("BOLT_V3_SSM_MANIFEST_SHA256")
        .expect("BOLT_V3_SSM_MANIFEST_SHA256 is required");
    let evidence_path = PathBuf::from(
        env::var("BOLT_V3_CANARY_EVIDENCE_PATH")
            .expect("BOLT_V3_CANARY_EVIDENCE_PATH is required"),
    );

    let loaded = load_bolt_v3_config(&root_toml).expect("root TOML should load");
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .expect("[live_canary] is required");

    assert_eq!(approval_id.trim(), live_canary.approval_id.trim());

    let root_toml_sha256 = sha256_file_hex(&root_toml).expect("root TOML checksum should compute");
    let initial = CanaryRunEvidence {
        commit_sha,
        config: CanaryConfigIdentity {
            root_toml_path: root_toml.clone(),
            root_toml_sha256,
            ssm_manifest_sha256,
            approval_id,
            max_live_order_count: live_canary.max_live_order_count,
            max_notional_per_order: live_canary.max_notional_per_order.clone(),
        },
        outcome: CanaryRunOutcome::BlockedBeforeSubmit {
            reason: "operator harness preflight reached production runner boundary".to_string(),
        },
    };
    std::fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&initial).expect("evidence should encode"),
    )
    .expect("evidence file should be writable");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build");
    let local = tokio::task::LocalSet::new();
    runtime
        .block_on(local.run_until(async move {
            let mut built = build_bolt_v3_live_node(&loaded)?;
            run_bolt_v3_live_node(&mut built, &loaded).await
        }))
        .expect("production bolt-v3 runner should complete under operator control");
}
```

- [ ] **Step 2: Run source-fence tests**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture
```

Expected: PASS for source fences and evidence serialization.

- [ ] **Step 3: Confirm ignored harness is not run by default**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_operator -- --nocapture
```

Expected: 0 passed, 0 failed, 1 ignored.

- [ ] **Step 4: Commit operator skeleton**

Run:

```bash
git add tests/bolt_v3_tiny_canary_operator.rs tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "test: add ignored tiny canary operator harness"
```

## Task 4: Controlled Single-order Submit Evidence

**Files:**
- Modify: `tests/bolt_v3_tiny_canary_operator.rs`
- Modify: `src/bolt_v3_tiny_canary_evidence.rs`
- Test: `tests/bolt_v3_tiny_canary_preconditions.rs`

- [ ] **Step 1: Add fail-closed source fence for one-order budget**

Append to `tests/bolt_v3_tiny_canary_preconditions.rs`:

```rust
#[test]
fn tiny_canary_operator_harness_requires_one_order_budget() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("tiny canary operator harness should exist");

    assert!(source.contains("max_live_order_count"));
    assert!(source.contains("assert_eq!(live_canary.max_live_order_count, 1)"));
    assert!(!source.contains("for _ in"));
    assert!(!source.contains("while true"));
    assert!(!source.contains("loop {"));
}
```

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions tiny_canary_operator_harness_requires_one_order_budget -- --nocapture
```

Expected failure: harness has not asserted the one-order budget.

- [ ] **Step 3: Add one-order assertion before runner entry**

In `tests/bolt_v3_tiny_canary_operator.rs`, immediately after reading `live_canary`:

```rust
assert_eq!(live_canary.max_live_order_count, 1);
```

Do not add loops or manual submit calls. The configured strategy decides whether an order candidate exists; submit admission enforces the count and cap before NT submit.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions tiny_canary_operator_harness_requires_one_order_budget -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit one-order guard**

Run:

```bash
git add tests/bolt_v3_tiny_canary_operator.rs tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "test: require one-order tiny canary budget"
```

## Task 5: Strategy-driven Exit Or Cancel Evidence

**Files:**
- Modify: `tests/bolt_v3_tiny_canary_preconditions.rs`
- Modify: `tests/bolt_v3_tiny_canary_operator.rs`
- Modify: `src/bolt_v3_tiny_canary_evidence.rs`

- [ ] **Step 1: Add source fence for no exec-engine-direct cancel**

Extend `tiny_canary_operator_harness_does_not_use_direct_exec_engine_cancel` with:

```rust
for forbidden in [
    "CancelAllOrders",
    "exec_engine_execute",
    "send_trading_command",
    "TradingCommand::Cancel",
    "msgbus::send",
] {
    assert!(!source.contains(forbidden), "forbidden direct cancel token {forbidden}");
}
```

- [ ] **Step 2: Run the fence**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions tiny_canary_operator_harness_does_not_use_direct_exec_engine_cancel -- --nocapture
```

Expected: PASS if the harness still relies on strategy/NT surfaces only.

- [ ] **Step 3: Add evidence outcome variant for strategy exit/cancel**

In `src/bolt_v3_tiny_canary_evidence.rs`, ensure the enum includes:

```rust
StrategyCancelSubmitted { client_order_id: String },
```

If this variant already exists from Task 2, do not add another one.

- [ ] **Step 4: Document live-run interpretation in the harness**

In `tests/bolt_v3_tiny_canary_operator.rs`, add a comment above the runner call:

```rust
// Open-order cleanup must be caused by configured strategy behavior through NT.
// This harness must not send exec-engine-direct cancel commands.
```

- [ ] **Step 5: Commit strategy cancel evidence boundary**

Run:

```bash
git add src/bolt_v3_tiny_canary_evidence.rs tests/bolt_v3_tiny_canary_operator.rs tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "test: fence tiny canary cancel evidence boundary"
```

## Task 6: Restart Reconciliation Evidence Boundary

**Files:**
- Modify: `tests/bolt_v3_tiny_canary_preconditions.rs`
- Modify: `tests/bolt_v3_tiny_canary_operator.rs`
- Modify: `src/bolt_v3_tiny_canary_evidence.rs`

- [ ] **Step 1: Add source fence against Bolt-owned reconciliation**

Append to `tests/bolt_v3_tiny_canary_preconditions.rs`:

```rust
#[test]
fn tiny_canary_operator_harness_does_not_implement_reconciliation() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("tiny canary operator harness should exist");

    for forbidden in [
        "reconcile_order",
        "reconcile_position",
        "ExecutionMassStatus::new",
        "OrderStatusReport::new",
        "register_external_order",
    ] {
        assert!(!source.contains(forbidden), "forbidden Bolt reconciliation token {forbidden}");
    }
}
```

- [ ] **Step 2: Run the fence**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions tiny_canary_operator_harness_does_not_implement_reconciliation -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Add evidence outcome variant for NT restart reconciliation**

In `src/bolt_v3_tiny_canary_evidence.rs`, ensure the enum includes:

```rust
ReconciledAfterRestart { client_order_id: String },
```

If this variant already exists from Task 2, do not add another one.

- [ ] **Step 4: Record the operator requirement**

In `tests/bolt_v3_tiny_canary_operator.rs`, add a comment near the evidence write:

```rust
// Restart reconciliation evidence must come from NT adapter state and NT reports
// after a fresh process start. Bolt records the redacted result; it does not
// synthesize reconciliation reports.
```

- [ ] **Step 5: Commit reconciliation boundary**

Run:

```bash
git add src/bolt_v3_tiny_canary_evidence.rs tests/bolt_v3_tiny_canary_operator.rs tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "test: fence tiny canary reconciliation boundary"
```

## Task 7: Verification And Branch Gate

**Files:**
- Modify: `specs/001-thin-live-canary-path/tasks.md`
- Modify: `specs/001-thin-live-canary-path/quickstart.md`
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` only if literal verification fails for new diagnostic/schema strings.

- [ ] **Step 1: Run local verification**

Run:

```bash
cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture
cargo test --test bolt_v3_tiny_canary_operator -- --nocapture
cargo fmt --check
git diff --check
```

Expected:

- precondition tests pass
- operator test reports 0 passed, 0 failed, 1 ignored
- formatting passes
- whitespace check passes

- [ ] **Step 2: Run static bolt-v3 verifiers**

Run:

```bash
python3 scripts/test_verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/test_verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_naming.py
python3 scripts/verify_bolt_v3_core_boundary.py
```

Expected: all pass. If runtime-literal audit classification is required, add only the minimal classifications for the new evidence diagnostic/schema strings.

- [ ] **Step 3: Run patched no-mistakes triage**

Run:

```bash
/private/tmp/no-mistakes-soak-bin status
/private/tmp/no-mistakes-soak-bin runs --limit 5
```

If starting a no-mistakes run is approved for this branch, use `/private/tmp/no-mistakes-soak-bin`, not the installed binary, and record:

- repo and branch
- run id
- final status
- final `error_code`, if any
- whether TUI or `runs` showed `error_code`
- whether ask-user findings resurfaced after a fix
- whether no-mistakes kept auto-fixing unrelated low/info findings instead of pausing
- daemon log anomaly

Append the result to `/private/tmp/no-mistakes-780-soak-log.md`.

- [ ] **Step 4: Update task and quickstart docs**

Only after Steps 1-3 pass, update:

- `specs/001-thin-live-canary-path/tasks.md`: mark T039-T044 according to actual completed work.
- `specs/001-thin-live-canary-path/quickstart.md`: add the exact ignored operator command.

Do not mark T045 until the approved live tiny-capital run has actually happened.

- [ ] **Step 5: Commit verification docs**

Run:

```bash
git add specs/001-thin-live-canary-path/tasks.md specs/001-thin-live-canary-path/quickstart.md docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml
git commit -m "docs: mark tiny canary local readiness tasks"
```

## Task 8: Approved Tiny-capital Operator Run

**Files:**
- No tracked source file changes required.
- Output: redacted evidence artifact outside tracked secrets.

- [ ] **Step 1: Confirm exact-head CI and approval**

Run:

```bash
gh pr view <phase8-pr-number> --json headRefOid,statusCheckRollup,reviewDecision,isDraft
git status --short --branch
```

Expected:

- exact head matches the intended commit
- CI is green
- branch is clean
- user/operator has explicitly approved the live canary run

- [ ] **Step 2: Run the ignored operator harness**

Run only with explicit operator approval:

```bash
BOLT_V3_ROOT_TOML=/absolute/path/to/approved-root.toml \
BOLT_V3_OPERATOR_APPROVAL_ID='<approval id matching [live_canary].approval_id>' \
BOLT_V3_CANARY_COMMIT_SHA='<exact git commit sha under review>' \
BOLT_V3_SSM_MANIFEST_SHA256='<sha256 of approved SSM path manifest>' \
BOLT_V3_CANARY_EVIDENCE_PATH=/absolute/path/to/redacted-canary-evidence.json \
cargo test --test bolt_v3_tiny_canary_operator \
  operator_approved_tiny_canary_uses_production_bolt_v3_path \
  -- --ignored --nocapture
```

Expected:

- at most one order can pass submit admission
- order notional is bounded by `[live_canary].max_notional_per_order`
- all live submit calls go through NT
- evidence artifact records exact SHA, config checksum, approval id, cap values, and observed NT/venue outcome

- [ ] **Step 3: Stop conditions**

Stop and report as blocker if any of these occurs:

- no-submit readiness report is missing, stale, or unsatisfied
- operator approval id mismatch
- config checksum differs from the approved value
- submit admission is unarmed
- decision evidence write fails
- more than one order is admitted
- order notional exceeds the canary cap
- cleanup requires exec-engine-direct cancel
- restart reconciliation cannot be proven through NT adapter state
- NT adapter lacks the required venue capability

- [ ] **Step 4: Record final live artifact status**

Update PR body or a single PR comment with:

- exact commit SHA
- command used
- redacted evidence path or storage location
- final outcome: blocked before submit, accepted, rejected, filled, cancelled, or reconciled after restart
- remaining blockers before generalized production trading

Do not paste secrets, private keys, raw SSM values, or unredacted venue credentials.

## Completion Criteria

Phase 8 is complete only when:

- T037 no-submit readiness evidence exists and is accepted by the live canary gate.
- The local precondition suite passes.
- The ignored operator harness is present, ignored by default, and uses the production bolt-v3 path.
- The live canary run is explicitly approved.
- The evidence artifact proves at most one configured tiny live order through NT.
- Any open-order cleanup is strategy-driven through NT.
- Restart reconciliation evidence comes from NT adapter state.
- Exact-head CI is green.
- no-mistakes triage result is recorded with the patched binary.
- External review is requested only after the branch is clean, pushed, and verified.

If the live adapter cannot provide the necessary evidence, the correct result is a documented blocker, not a Bolt-side replacement for NT behavior.
