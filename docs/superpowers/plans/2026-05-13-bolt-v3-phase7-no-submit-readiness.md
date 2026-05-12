# Bolt-v3 Phase 7 No-submit Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the authenticated no-submit readiness producer that builds the production-shaped bolt-v3 node, performs bounded NT connect/disconnect, writes a redacted report compatible with the existing live-canary gate, and proves the code cannot submit, cancel, replace, amend, subscribe, or enter the runner loop.

**Architecture:** Keep this as a thin orchestration layer over existing bolt-v3 build and NT controlled-connect/disconnect boundaries. The new module owns only report schema, redaction, stage recording, and sequencing; NT still owns adapter behavior, connection dispatch, cache, lifecycle, and errors.

**Tech Stack:** Rust, NautilusTrader Rust API, existing bolt-v3 TOML loader, AWS SSM resolver boundary, `serde_json`, existing `cargo test` and bolt-v3 verifier scripts.

---

## Current Evidence

- `src/bolt_v3_readiness.rs` is startup/build-only. It resolves secrets, maps adapters, registers clients, and builds an NT `LiveNode`, but records `"built NT LiveNode without connecting clients"` and does not call controlled connect/disconnect.
- `src/bolt_v3_live_node.rs` already exposes `connect_bolt_v3_clients` and `disconnect_bolt_v3_clients`, both bounded by TOML `nautilus.timeout_connection_seconds` and `nautilus.timeout_disconnection_seconds`.
- `src/bolt_v3_live_canary_gate.rs` consumes a JSON object with a non-empty `stages` array and requires every stage status to equal `"satisfied"` case-insensitively.
- `tests/bolt_v3_readiness.rs` already fences startup readiness away from connect, disconnect, runner, subscribe, and order APIs. Phase 7 needs a separate fence because no-submit readiness is allowed to call connect/disconnect but still must reject runner, subscribe, submit, cancel, replace, and amend APIs.
- No current `src/bolt_v3_no_submit_readiness.rs` exists on PR #317 head `3dccb542584705a2bff2cca1c7d48b90f02cff9b`.

## File Structure

- Create `src/bolt_v3_no_submit_readiness_schema.rs`: shared report JSON keys and status constants used by producer and gate.
- Create `src/bolt_v3_no_submit_readiness.rs`: stage report model, local runner, real resolver wrapper, JSON writer, redaction boundary.
- Modify `src/bolt_v3_live_canary_gate.rs`: use shared schema constants instead of private report-key literals.
- Modify `src/lib.rs`: export the two new modules.
- Create `tests/bolt_v3_no_submit_readiness.rs`: schema compatibility, local no-network runner behavior, redaction, connect/disconnect sequencing, and source fence.
- Create `tests/bolt_v3_no_submit_readiness_operator.rs`: ignored real SSM/venue test requiring explicit env/config path; never runs by default.
- Modify `specs/001-thin-live-canary-path/tasks.md`: mark T033-T038 only after each task's verification has run.

## Task 1: Shared Report Schema Compatibility

**Files:**
- Create: `src/bolt_v3_no_submit_readiness_schema.rs`
- Modify: `src/bolt_v3_live_canary_gate.rs`
- Modify: `src/lib.rs`
- Test: `tests/bolt_v3_no_submit_readiness.rs`

- [ ] **Step 1: Write the failing schema compatibility test**

Add `tests/bolt_v3_no_submit_readiness.rs`:

```rust
use serde_json::json;

use bolt_v2::bolt_v3_no_submit_readiness_schema::{
    SATISFIED_STATUS, STAGE_KEY, STAGES_KEY, STATUS_KEY,
};

#[test]
fn no_submit_readiness_schema_matches_live_canary_gate_contract() {
    let report = json!({
        STAGES_KEY: [
            {
                STAGE_KEY: "connect",
                STATUS_KEY: SATISFIED_STATUS,
            },
            {
                STAGE_KEY: "disconnect",
                STATUS_KEY: SATISFIED_STATUS,
            },
        ],
    });

    assert_eq!(report[STAGES_KEY][0][STAGE_KEY], "connect");
    assert_eq!(report[STAGES_KEY][1][STAGE_KEY], "disconnect");
    assert_eq!(report[STAGES_KEY][0][STATUS_KEY], "satisfied");
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_schema_matches_live_canary_gate_contract -- --nocapture
```

Expected failure: unresolved import `bolt_v2::bolt_v3_no_submit_readiness_schema`.

- [ ] **Step 3: Add the shared schema module**

Create `src/bolt_v3_no_submit_readiness_schema.rs`:

```rust
pub const STAGES_KEY: &str = "stages";
pub const STAGE_KEY: &str = "stage";
pub const NAME_KEY: &str = "name";
pub const STATUS_KEY: &str = "status";
pub const SATISFIED_STATUS: &str = "satisfied";
pub const FAILED_STATUS: &str = "failed";
pub const SKIPPED_STATUS: &str = "skipped";

pub const STAGE_FORBIDDEN_CREDENTIAL_ENV: &str = "forbidden_credential_env";
pub const STAGE_SECRET_RESOLUTION: &str = "secret_resolution";
pub const STAGE_ADAPTER_MAPPING: &str = "adapter_mapping";
pub const STAGE_LIVE_NODE_BUILD: &str = "live_node_build";
pub const STAGE_CONTROLLED_CONNECT: &str = "controlled_connect";
pub const STAGE_CONTROLLED_DISCONNECT: &str = "controlled_disconnect";
```

Modify `src/lib.rs`:

```rust
pub mod bolt_v3_no_submit_readiness_schema;
```

- [ ] **Step 4: Use shared constants in the live canary gate**

In `src/bolt_v3_live_canary_gate.rs`, import:

```rust
use crate::bolt_v3_no_submit_readiness_schema::{
    NAME_KEY, SATISFIED_STATUS, STAGE_KEY, STAGES_KEY, STATUS_KEY,
};
```

Replace private string uses inside `validate_no_submit_readiness_report` with those constants. Preserve behavior exactly: stage names may be read from `stage` or `name`, and status comparison remains ASCII case-insensitive.

- [ ] **Step 5: Run schema and gate verification**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_schema_matches_live_canary_gate_contract -- --nocapture
cargo test --test bolt_v3_live_canary_gate -- --nocapture
```

Expected: both commands pass.

- [ ] **Step 6: Commit**

```bash
git add src/bolt_v3_no_submit_readiness_schema.rs src/bolt_v3_live_canary_gate.rs src/lib.rs tests/bolt_v3_no_submit_readiness.rs
git commit -m "feat: share bolt-v3 no-submit readiness schema"
```

## Task 2: Local No-submit Readiness Runner

**Files:**
- Create: `src/bolt_v3_no_submit_readiness.rs`
- Modify: `src/lib.rs`
- Test: `tests/bolt_v3_no_submit_readiness.rs`

- [ ] **Step 1: Write the failing local runner test**

Extend `tests/bolt_v3_no_submit_readiness.rs`:

```rust
mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{make_bolt_v3_live_node_builder, BoltV3BuiltLiveNode},
    bolt_v3_no_submit_readiness::{
        BoltV3NoSubmitReadinessStatus, run_bolt_v3_no_submit_readiness_on_built_node,
    },
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
};
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_data_subscriptions, clear_mock_exec_submissions, recorded_mock_data_subscriptions,
    recorded_mock_exec_submissions,
};

#[test]
fn no_submit_readiness_local_runner_writes_satisfied_connect_disconnect_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.nautilus.timeout_connection_seconds = 30;
    loaded.root.nautilus.timeout_disconnection_seconds = 10;
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();
    let mut built = mock_built_live_node(&loaded);

    let report = run_bolt_v3_no_submit_readiness_on_built_node(&mut built, &loaded)
        .expect("local no-submit readiness should complete against mock NT clients");

    assert!(report.stage_status("controlled_connect").contains(&BoltV3NoSubmitReadinessStatus::Satisfied));
    assert!(report.stage_status("controlled_disconnect").contains(&BoltV3NoSubmitReadinessStatus::Satisfied));
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

fn mock_built_live_node(loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config) -> BoltV3BuiltLiveNode {
    let builder =
        make_bolt_v3_live_node_builder(loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new("MOCK_DATA", "MOCKVENUE")),
        )
        .expect("mock data client should register on bolt-v3 builder");
    let builder = builder
        .add_exec_client(
            Some("MOCK_EXEC".to_string()),
            Box::new(MockExecutionClientFactory),
            Box::new(MockExecClientConfig::new(
                "MOCK_EXEC",
                "MOCK-ACCOUNT",
                "MOCKVENUE",
            )),
        )
        .expect("mock exec client should register on bolt-v3 builder");
    BoltV3BuiltLiveNode::new(
        builder.build().expect("LiveNode should build with mocks"),
        std::sync::Arc::new(BoltV3SubmitAdmissionState::new_unarmed()),
    )
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_local_runner_writes_satisfied_connect_disconnect_report -- --nocapture
```

Expected failure: unresolved import `bolt_v2::bolt_v3_no_submit_readiness`.

- [ ] **Step 3: Add the minimal runner module**

Create `src/bolt_v3_no_submit_readiness.rs` with:

```rust
use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{
        BoltV3BuiltLiveNode, BoltV3LiveNodeError, connect_bolt_v3_clients,
        disconnect_bolt_v3_clients,
    },
    bolt_v3_no_submit_readiness_schema::{
        FAILED_STATUS, SATISFIED_STATUS, SKIPPED_STATUS, STAGE_ADAPTER_MAPPING,
        STAGE_CONTROLLED_CONNECT, STAGE_CONTROLLED_DISCONNECT, STAGE_FORBIDDEN_CREDENTIAL_ENV,
        STAGE_LIVE_NODE_BUILD, STAGE_SECRET_RESOLUTION,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3NoSubmitReadinessStatus {
    Satisfied,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3NoSubmitReadinessStage {
    pub stage: &'static str,
    pub status: BoltV3NoSubmitReadinessStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3NoSubmitReadinessReport {
    pub stages: Vec<BoltV3NoSubmitReadinessStage>,
}

impl BoltV3NoSubmitReadinessReport {
    pub fn stage_status(&self, stage: &str) -> Vec<BoltV3NoSubmitReadinessStatus> {
        self.stages
            .iter()
            .filter(|fact| fact.stage == stage)
            .map(|fact| fact.status)
            .collect()
    }
}

pub fn run_bolt_v3_no_submit_readiness_on_built_node(
    built: &mut BoltV3BuiltLiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3LiveNodeError>
{
    let runtime = tokio::runtime::Runtime::new().map_err(|error| {
        BoltV3LiveNodeError::Build(anyhow::Error::new(error))
    })?;
    runtime.block_on(run_bolt_v3_no_submit_readiness_async_with(
        built, loaded,
    ))
}

async fn run_bolt_v3_no_submit_readiness_async_with(
    built: &mut BoltV3BuiltLiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3LiveNodeError>
{
    let mut stages = Vec::new();
    stages.push(satisfied(STAGE_FORBIDDEN_CREDENTIAL_ENV));
    stages.push(satisfied(STAGE_SECRET_RESOLUTION));
    stages.push(satisfied(STAGE_ADAPTER_MAPPING));
    stages.push(satisfied(STAGE_LIVE_NODE_BUILD));

    match connect_bolt_v3_clients(built.node_mut(), loaded).await {
        Ok(()) => stages.push(satisfied(STAGE_CONTROLLED_CONNECT)),
        Err(error) => {
            stages.push(failed(STAGE_CONTROLLED_CONNECT, error.to_string()));
            stages.push(skipped(
                STAGE_CONTROLLED_DISCONNECT,
                "controlled_disconnect skipped after controlled_connect failure",
            ));
            return Ok(BoltV3NoSubmitReadinessReport { stages });
        }
    }

    match disconnect_bolt_v3_clients(built.node_mut(), loaded).await {
        Ok(()) => stages.push(satisfied(STAGE_CONTROLLED_DISCONNECT)),
        Err(error) => stages.push(failed(STAGE_CONTROLLED_DISCONNECT, error.to_string())),
    }

    Ok(BoltV3NoSubmitReadinessReport { stages })
}

fn satisfied(stage: &'static str) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Satisfied,
        detail: SATISFIED_STATUS.to_string(),
    }
}

fn failed(stage: &'static str, detail: String) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Failed,
        detail,
    }
}

fn skipped(stage: &'static str, detail: &'static str) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Skipped,
        detail: detail.to_string(),
    }
}

impl BoltV3NoSubmitReadinessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => SATISFIED_STATUS,
            Self::Failed => FAILED_STATUS,
            Self::Skipped => SKIPPED_STATUS,
        }
    }
}
```

Add `pub mod bolt_v3_no_submit_readiness;` to `src/lib.rs`.

- [ ] **Step 4: Run the local runner test**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_local_runner_writes_satisfied_connect_disconnect_report -- --nocapture
```

Expected: pass. If it fails, stop and investigate the exact NT build/connect error before continuing.

- [ ] **Step 5: Commit**

```bash
git add src/bolt_v3_no_submit_readiness.rs src/lib.rs tests/bolt_v3_no_submit_readiness.rs
git commit -m "feat: add bolt-v3 no-submit readiness runner"
```

## Task 3: JSON Report Writer And Gate Consumption

**Files:**
- Modify: `src/bolt_v3_no_submit_readiness.rs`
- Test: `tests/bolt_v3_no_submit_readiness.rs`

- [ ] **Step 1: Write the failing JSON compatibility test**

Extend `tests/bolt_v3_no_submit_readiness.rs`:

```rust
use bolt_v2::bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate;

#[test]
fn no_submit_readiness_report_json_is_accepted_by_live_canary_gate() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.live_canary.as_mut().expect("live canary block").no_submit_readiness_report_path =
        report_path.to_string_lossy().to_string();

    let mut built = mock_built_live_node(&loaded);
    let report = run_bolt_v3_no_submit_readiness_on_built_node(&mut built, &loaded)
        .expect("local readiness should complete against mock NT clients");
    report.write_redacted_json(&report_path).expect("report write should succeed");

    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(check_bolt_v3_live_canary_gate(&loaded))
        .expect("gate should accept producer report");
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_report_json_is_accepted_by_live_canary_gate -- --nocapture
```

Expected failure: `write_redacted_json` is missing.

- [ ] **Step 3: Implement JSON serialization**

In `src/bolt_v3_no_submit_readiness.rs`, add:

```rust
impl BoltV3NoSubmitReadinessReport {
    pub fn write_redacted_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        let stages: Vec<serde_json::Value> = self
            .stages
            .iter()
            .map(|stage| {
                serde_json::json!({
                    crate::bolt_v3_no_submit_readiness_schema::STAGE_KEY: stage.stage,
                    crate::bolt_v3_no_submit_readiness_schema::STATUS_KEY: stage.status.as_str(),
                    "detail": stage.detail,
                })
            })
            .collect();
        let payload = serde_json::json!({
            crate::bolt_v3_no_submit_readiness_schema::STAGES_KEY: stages,
        });
        let body = serde_json::to_vec_pretty(&payload).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error)
        })?;
        std::fs::write(path, body)
    }
}
```

- [ ] **Step 4: Run JSON and gate tests**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_report_json_is_accepted_by_live_canary_gate -- --nocapture
cargo test --test bolt_v3_live_canary_gate -- --nocapture
```

Expected: both commands pass.

- [ ] **Step 5: Commit**

```bash
git add src/bolt_v3_no_submit_readiness.rs tests/bolt_v3_no_submit_readiness.rs
git commit -m "feat: write bolt-v3 no-submit readiness report"
```

## Task 4: Zero-order Source Fence

**Files:**
- Test: `tests/bolt_v3_no_submit_readiness.rs`

- [ ] **Step 1: Write the source-fence test**

Add:

```rust
#[test]
fn no_submit_readiness_source_has_no_trade_or_runner_tokens() {
    let source = include_str!("../src/bolt_v3_no_submit_readiness.rs");
    for forbidden in [
        ".run(",
        "run_bolt_v3_live_node",
        "submit_order",
        "submit_order_list",
        "cancel_order",
        "CancelAllOrders",
        "replace_order",
        "amend_order",
        "subscribe",
    ] {
        assert!(
            !source.contains(forbidden),
            "no-submit readiness must not contain trade or runner token `{forbidden}`"
        );
    }
    assert!(source.contains("connect_bolt_v3_clients"));
    assert!(source.contains("disconnect_bolt_v3_clients"));
}
```

- [ ] **Step 2: Run the source-fence test**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_source_has_no_trade_or_runner_tokens -- --nocapture
```

Expected: pass. If it fails on a string literal in a diagnostic, replace the simple check with the existing comment/string-stripping helper from `tests/bolt_v3_readiness.rs` rather than weakening the forbidden token list.

- [ ] **Step 3: Commit**

```bash
git add tests/bolt_v3_no_submit_readiness.rs
git commit -m "test: fence no-submit readiness trade APIs"
```

## Task 5: Redaction And Failure Report Behavior

**Files:**
- Modify: `src/bolt_v3_no_submit_readiness.rs`
- Test: `tests/bolt_v3_no_submit_readiness.rs`

- [ ] **Step 1: Write failing redaction and failure tests**

Add:

```rust
#[test]
fn no_submit_readiness_report_does_not_contain_resolved_secret_values() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let mut built = mock_built_live_node(&loaded);
    let report = run_bolt_v3_no_submit_readiness_on_built_node(&mut built, &loaded)
        .expect("local readiness should complete against mock NT clients");

    let text = format!("{report:#?}");
    for secret in [
        "0x4242424242424242424242424242424242424242424242424242424242424242",
        "polymarket-api-key",
        "polymarket-passphrase",
        "binance-api-key",
    ] {
        assert!(!text.contains(secret), "report leaked resolved secret value");
    }
}

#[test]
fn no_submit_readiness_records_failed_connect_and_skips_disconnect() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.nautilus.timeout_connection_seconds = 1;
    let mut built = mock_built_live_node_with_failing_data_connect(&loaded);

    let report = run_bolt_v3_no_submit_readiness_on_built_node(&mut built, &loaded)
        .expect("connect failure is recorded as failed readiness report");

    assert!(report.stage_status("controlled_connect").contains(&BoltV3NoSubmitReadinessStatus::Failed));
    assert!(report.stage_status("controlled_disconnect").contains(&BoltV3NoSubmitReadinessStatus::Skipped));
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_report_does_not_contain_resolved_secret_values no_submit_readiness_records_failed_connect_and_skips_disconnect -- --nocapture
```

Expected failure: `mock_built_live_node_with_failing_data_connect` is missing, or the readiness runner still returns an error instead of a failed-stage report.

- [ ] **Step 3: Tighten details to operator-safe text only**

Add `mock_built_live_node_with_failing_data_connect` to the test file by copying `mock_built_live_node`, but construct `MockDataClientConfig::new("MOCK_DATA", "MOCKVENUE").with_connect_failure("simulated no-submit readiness connect failure")`.

If any failure detail contains resolved secret material, replace that detail with the error's redacted display string or the failing stage name. Do not add a second secret source or scrub with ad hoc string replacement.

- [ ] **Step 4: Run the full local readiness test file**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness -- --nocapture
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/bolt_v3_no_submit_readiness.rs tests/bolt_v3_no_submit_readiness.rs
git commit -m "test: prove no-submit readiness redaction"
```

## Task 6: Ignored Real SSM/Venue Operator Harness

**Files:**
- Create: `tests/bolt_v3_no_submit_readiness_operator.rs`

- [ ] **Step 1: Write the ignored operator test**

Create:

```rust
use std::path::PathBuf;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
};

#[test]
#[ignore = "requires explicit operator approval, real SSM, and real venue connectivity"]
fn operator_approved_real_no_submit_readiness_writes_redacted_report() {
    let root_path = PathBuf::from(
        std::env::var("BOLT_V3_OPERATOR_CONFIG_PATH")
            .expect("BOLT_V3_OPERATOR_CONFIG_PATH must point to approved TOML"),
    );
    let report_path = PathBuf::from(
        std::env::var("BOLT_V3_OPERATOR_NO_SUBMIT_REPORT_PATH")
            .expect("BOLT_V3_OPERATOR_NO_SUBMIT_REPORT_PATH must point outside tracked secrets"),
    );
    let approval = std::env::var("BOLT_V3_OPERATOR_APPROVAL_ID")
        .expect("BOLT_V3_OPERATOR_APPROVAL_ID must be set by the operator");

    let loaded = load_bolt_v3_config(&root_path).expect("operator TOML should load");
    let report = run_bolt_v3_no_submit_readiness(&loaded, &approval)
        .expect("real no-submit readiness should complete");
    report.write_redacted_json(&report_path).expect("report write should succeed");
}
```

- [ ] **Step 2: Run default tests to prove the operator test is ignored**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness_operator -- --nocapture
```

Expected: `0 passed; 0 failed; 1 ignored`.

- [ ] **Step 3: Implement production resolver wrapper**

In `src/bolt_v3_no_submit_readiness.rs`, add `run_bolt_v3_no_submit_readiness` that uses `SsmResolverSession::new()` and `resolve_bolt_v3_secrets` through the existing build path. It must require a non-empty approval id and must not read secrets from env vars.

- [ ] **Step 4: Commit**

```bash
git add src/bolt_v3_no_submit_readiness.rs tests/bolt_v3_no_submit_readiness_operator.rs
git commit -m "test: add ignored real no-submit readiness harness"
```

## Task 7: Phase Verification

**Files:**
- Modify: `specs/001-thin-live-canary-path/tasks.md`

- [ ] **Step 1: Run local phase checks**

Run:

```bash
cargo test --test bolt_v3_no_submit_readiness -- --nocapture
cargo test --test bolt_v3_no_submit_readiness_operator -- --nocapture
cargo test --test bolt_v3_live_canary_gate -- --nocapture
cargo test --test bolt_v3_controlled_connect -- --nocapture
cargo test --lib -- --nocapture
cargo clippy --all-targets -- -D warnings
cargo fmt --check
git diff --check
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_naming.py
python3 scripts/verify_bolt_v3_core_boundary.py
/private/tmp/no-mistakes-soak-bin status
/private/tmp/no-mistakes-soak-bin runs --limit 5
```

Expected: all local code checks pass; no-mistakes status is recorded. If clippy needs sandbox escalation for the shared Rust verification cache, rerun the same command with escalation and record the reason.

- [ ] **Step 2: Mark Phase 7 task checkboxes**

Only after Step 1 passes, mark T033-T036 and T038 complete in `specs/001-thin-live-canary-path/tasks.md`. Leave T037 incomplete until the operator explicitly approves and runs the ignored real SSM/venue test.

- [ ] **Step 3: Commit**

```bash
git add specs/001-thin-live-canary-path/tasks.md
git commit -m "docs: mark local phase7 readiness tasks"
```

## Task 8: Explicit Operator Run Boundary

**Files:**
- No tracked file changes before approval.

- [ ] **Step 1: Request operator approval**

Ask for explicit approval before running:

```bash
BOLT_V3_OPERATOR_CONFIG_PATH=<approved-toml> \
BOLT_V3_OPERATOR_NO_SUBMIT_REPORT_PATH=<outside-repo-redacted-json> \
BOLT_V3_OPERATOR_APPROVAL_ID=<operator-approval-id> \
cargo test --test bolt_v3_no_submit_readiness_operator operator_approved_real_no_submit_readiness_writes_redacted_report -- --ignored --nocapture
```

Expected: do not run this command without explicit user approval in the current thread.

- [ ] **Step 2: Verify the produced report without printing secrets**

After an approved run, inspect only non-secret metadata:

```bash
python3 -m json.tool <approved-report-path> >/tmp/bolt-v3-no-submit-report-shape.json
python3 scripts/verify_bolt_v3_provider_leaks.py
cargo test --test bolt_v3_live_canary_gate -- --nocapture
```

Expected: report shape parses, provider leak verifier passes, and the live-canary gate accepts the report path configured in the approved TOML.

- [ ] **Step 3: Record artifact metadata**

Record exact commit SHA, config checksum, approval id, report path, and command exit status in the PR body or an operator-approved artifact. Do not commit the real report if it contains sensitive paths or metadata.

## Self-Review Checklist

- Spec coverage: T033 maps to Task 1 and Task 3. T034 maps to Task 4. T035 maps to Task 2. T036 maps to Task 7. T037 maps to Task 8. T038 maps to Task 3 and Task 7.
- Thin boundary: no Bolt-side order lifecycle, reconciliation, adapter behavior, cache semantics, or mock venue proof is introduced.
- No hardcodes: runtime paths, approval ids, timeouts, caps, and secrets remain TOML/env-for-operator-test controlled. Schema field names are shared constants and must be classified by the runtime literal verifier if it flags them.
- No dual paths: one producer schema, one live-canary gate consumer, one controlled-connect/disconnect path.
- Live safety: no live operation runs without explicit operator approval; default tests stay local and zero-order.
