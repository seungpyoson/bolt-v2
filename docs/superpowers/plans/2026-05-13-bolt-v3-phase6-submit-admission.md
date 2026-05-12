# Bolt-v3 Phase 6 Submit Admission Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every bolt-v3 live submit consume the validated `BoltV3LiveCanaryGateReport` order-count and notional bounds before NT `submit_order`.

**Architecture:** Add one shared, in-memory `BoltV3SubmitAdmissionState` created during bolt-v3 live-node build, passed into strategy construction, and armed by `run_bolt_v3_live_node` after the live canary gate returns a valid `BoltV3LiveCanaryGateReport`. Strategies record decision evidence first, then ask submit admission to consume one order budget, then call NT `submit_order`. NT still owns order lifecycle, fills, cancel behavior, cache, adapter behavior, and reconciliation.

**Tech Stack:** Rust, NautilusTrader Rust APIs, `rust_decimal::Decimal`, TOML-loaded config, AWS SSM-backed loaded runtime, existing bolt-v3 registry bindings.

---

## Evidence Read Before Planning

- `.specify/memory/constitution.md` requires one live submit admission path, config-controlled runtime, TDD, NT-first boundaries, and no Bolt-owned lifecycle/reconciliation/adapter/cache semantics.
- `specs/001-thin-live-canary-path/tasks.md:70-80` defines Phase 6 tasks T028-T032.
- `specs/001-thin-live-canary-path/spec.md:28-38` requires submit rejection when the gate report is absent, order count is exhausted, notional exceeds cap, or decision evidence is missing.
- `specs/001-thin-live-canary-path/data-model.md:56-87` defines `LiveCanaryGateReport` and `SubmitAdmissionState`.
- `src/bolt_v3_live_canary_gate.rs:27-38` already exposes the validated report fields Phase 6 must consume.
- `src/bolt_v3_live_node.rs:281-289` currently validates the gate report and drops it before `node.run().await`.
- `src/strategies/eth_chainlink_taker.rs:2826-2835` is currently the only direct NT submit helper after Phase 5 and records decision evidence before `self.submit_order(order, None, Some(client_id))`.
- `src/bolt_v3_strategy_registration.rs:22-30` currently passes loaded config, loaded strategy, and resolved secrets into concrete strategy bindings; it does not yet carry shared admission state.

## Scope

In scope:

- Add `src/bolt_v3_submit_admission.rs`.
- Add `tests/bolt_v3_submit_admission.rs`.
- Thread one shared admission state through bolt-v3 build, strategy registration, and strategy context.
- Arm admission from the actual `BoltV3LiveCanaryGateReport` returned by `check_bolt_v3_live_canary_gate`.
- Enforce `max_live_order_count` and `max_notional_per_order` before NT submit.
- Preserve mandatory decision evidence and prove evidence failure does not consume order budget.
- Consume admission budget before NT submit and do not refund on NT submit error; this is fail-closed for the one-order canary.
- Update Phase 6 task checkboxes only after verification.

Out of scope:

- No authenticated no-submit readiness run. That is Phase 7.
- No tiny-capital live submit/cancel/fill/reconciliation proof. That is Phase 8.
- No Bolt-owned order lifecycle, cancel loop, reconciliation, adapter behavior, cache semantics, or mock venue readiness proof.
- No provider, market-family, or strategy hardcoding in core admission logic.
- No `src/main.rs` production entrypoint adoption in this slice.

## File Map

- Create `src/bolt_v3_submit_admission.rs`: state, request, permit, and fail-closed errors for submit admission.
- Modify `src/lib.rs`: export `bolt_v3_submit_admission`.
- Modify `src/bolt_v3_live_node.rs`: create one shared admission state during build and arm it in `run_bolt_v3_live_node`.
- Modify `src/bolt_v3_strategy_registration.rs`: carry the shared admission state in `StrategyRegistrationContext`.
- Modify `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`: pass shared admission into `StrategyBuildContext`.
- Modify `src/strategies/registry.rs`: make `StrategyBuildContext` require admission state alongside decision evidence.
- Modify `src/strategies/eth_chainlink_taker.rs`: compute order notional and call admission after evidence persistence but before NT submit.
- Modify tests that construct `StrategyBuildContext` or build a bolt-v3 live node.
- Modify `specs/001-thin-live-canary-path/tasks.md` only after all verification passes.

---

## Task 1: Red Tests For Submit Admission State

**Files:**
- Create: `tests/bolt_v3_submit_admission.rs`

- [ ] **Step 1: Write failing state tests**

Create `tests/bolt_v3_submit_admission.rs`:

```rust
use std::path::PathBuf;

use rust_decimal::Decimal;

use bolt_v2::{
    bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    },
};

fn report(max_live_order_count: u32, max_notional_per_order: Decimal) -> BoltV3LiveCanaryGateReport {
    BoltV3LiveCanaryGateReport {
        approval_id: "APPROVAL-001".to_string(),
        no_submit_readiness_report_path: PathBuf::from("reports/no-submit-readiness.json"),
        max_no_submit_readiness_report_bytes: 4096,
        max_live_order_count,
        max_notional_per_order,
        root_max_notional_per_order: Decimal::new(10, 0),
    }
}

fn request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "ETHCHAINLINKTAKER-001".to_string(),
        client_order_id: "O-19700101-000000-001-001-1".to_string(),
        instrument_id: "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
        notional,
    }
}

#[test]
fn submit_admission_rejects_when_gate_report_is_missing() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();

    let error = admission
        .admit(&request(Decimal::new(50, 2)))
        .expect_err("unarmed admission must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::MissingGateReport
    ));
}

#[test]
fn submit_admission_rejects_second_order_after_count_is_exhausted() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("valid report should arm admission");

    admission
        .admit(&request(Decimal::new(50, 2)))
        .expect("first order should consume the one-order canary budget");
    let error = admission
        .admit(&request(Decimal::new(50, 2)))
        .expect_err("second order must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::OrderCountExhausted {
            admitted_order_count: 1,
            max_live_order_count: 1
        }
    ));
}

#[test]
fn submit_admission_rejects_notional_above_gate_cap() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("valid report should arm admission");

    let error = admission
        .admit(&request(Decimal::new(101, 2)))
        .expect_err("over-cap notional must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalExceedsCap {
            notional,
            max_notional_per_order
        } if notional == Decimal::new(101, 2)
            && max_notional_per_order == Decimal::new(100, 2)
    ));
}

#[test]
fn submit_admission_rejects_non_positive_notional_without_consuming_budget() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("valid report should arm admission");

    let error = admission
        .admit(&request(Decimal::ZERO))
        .expect_err("zero notional must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidNotional { notional }
            if notional == Decimal::ZERO
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}
```

- [ ] **Step 2: Run the red tests**

Run:

```bash
cargo test --test bolt_v3_submit_admission -- --nocapture
```

Expected: FAIL because `bolt_v3_submit_admission` does not exist.

---

## Task 2: Implement The Admission State

**Files:**
- Create: `src/bolt_v3_submit_admission.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add the module export**

In `src/lib.rs`, add:

```rust
pub mod bolt_v3_submit_admission;
```

- [ ] **Step 2: Implement the minimal admission module**

Create `src/bolt_v3_submit_admission.rs`:

```rust
use std::sync::Mutex;

use rust_decimal::Decimal;

use crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoltV3SubmitAdmissionPermit {
    pub admitted_order_count: u32,
    pub max_live_order_count: u32,
    pub max_notional_per_order: Decimal,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    MissingGateReport,
    AlreadyArmed,
    InvalidNotional {
        notional: Decimal,
    },
    NotionalExceedsCap {
        notional: Decimal,
        max_notional_per_order: Decimal,
    },
    OrderCountExhausted {
        admitted_order_count: u32,
        max_live_order_count: u32,
    },
    LockPoisoned,
}

impl std::fmt::Display for BoltV3SubmitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingGateReport => {
                write!(f, "bolt-v3 submit admission is missing live canary gate report")
            }
            Self::AlreadyArmed => write!(f, "bolt-v3 submit admission is already armed"),
            Self::InvalidNotional { notional } => write!(
                f,
                "bolt-v3 submit admission notional must be positive, got {notional}"
            ),
            Self::NotionalExceedsCap {
                notional,
                max_notional_per_order,
            } => write!(
                f,
                "bolt-v3 submit admission notional {notional} exceeds cap {max_notional_per_order}"
            ),
            Self::OrderCountExhausted {
                admitted_order_count,
                max_live_order_count,
            } => write!(
                f,
                "bolt-v3 submit admission order count exhausted: admitted {admitted_order_count}, max {max_live_order_count}"
            ),
            Self::LockPoisoned => write!(f, "bolt-v3 submit admission lock is poisoned"),
        }
    }
}

impl std::error::Error for BoltV3SubmitAdmissionError {}

#[derive(Debug, Default)]
struct BoltV3SubmitAdmissionInner {
    gate_report: Option<BoltV3LiveCanaryGateReport>,
    admitted_order_count: u32,
}

#[derive(Debug, Default)]
pub struct BoltV3SubmitAdmissionState {
    inner: Mutex<BoltV3SubmitAdmissionInner>,
}

impl BoltV3SubmitAdmissionState {
    pub fn new_unarmed() -> Self {
        Self::default()
    }

    pub fn arm(
        &self,
        report: BoltV3LiveCanaryGateReport,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| BoltV3SubmitAdmissionError::LockPoisoned)?;
        if inner.gate_report.is_some() {
            return Err(BoltV3SubmitAdmissionError::AlreadyArmed);
        }
        inner.gate_report = Some(report);
        Ok(())
    }

    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        if request.notional <= Decimal::ZERO {
            return Err(BoltV3SubmitAdmissionError::InvalidNotional {
                notional: request.notional,
            });
        }

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| BoltV3SubmitAdmissionError::LockPoisoned)?;
        let Some(report) = inner.gate_report.clone() else {
            return Err(BoltV3SubmitAdmissionError::MissingGateReport);
        };
        if request.notional > report.max_notional_per_order {
            return Err(BoltV3SubmitAdmissionError::NotionalExceedsCap {
                notional: request.notional,
                max_notional_per_order: report.max_notional_per_order,
            });
        }
        if inner.admitted_order_count >= report.max_live_order_count {
            return Err(BoltV3SubmitAdmissionError::OrderCountExhausted {
                admitted_order_count: inner.admitted_order_count,
                max_live_order_count: report.max_live_order_count,
            });
        }

        inner.admitted_order_count += 1;
        Ok(BoltV3SubmitAdmissionPermit {
            admitted_order_count: inner.admitted_order_count,
            max_live_order_count: report.max_live_order_count,
            max_notional_per_order: report.max_notional_per_order,
        })
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admitted_order_count
    }
}
```

- [ ] **Step 3: Run the state tests**

Run:

```bash
cargo test --test bolt_v3_submit_admission -- --nocapture
```

Expected: PASS.

---

## Task 3: Thread One Admission State Through Build And Run

**Files:**
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- Modify: `src/strategies/registry.rs`
- Modify tests that call `build_bolt_v3_live_node_with`, `build_bolt_v3_live_node_with_summary`, or `StrategyBuildContext::try_new`.

- [ ] **Step 1: Write failing build/run tests**

Extend `tests/bolt_v3_submit_admission.rs`:

```rust
mod support;

use bolt_v2::bolt_v3_live_node::{build_bolt_v3_live_node_with_summary, run_bolt_v3_live_node};

#[test]
fn bolt_v3_build_returns_unarmed_submit_admission_state() {
    let (_tempdir, loaded) =
        support::load_bolt_v3_config_with_temp_catalog("submit-admission-build");

    let (built, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("bolt-v3 build should succeed");

    let error = built
        .submit_admission()
        .admit(&request(Decimal::new(50, 2)))
        .expect_err("admission must be unarmed before run gate validates report");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::MissingGateReport
    ));
}
```

Expected red failure: `built.submit_admission()` does not exist and the builder still returns `LiveNode`.

- [ ] **Step 2: Add a built runtime wrapper**

In `src/bolt_v3_live_node.rs`, add near the build functions:

```rust
use std::sync::Arc;

use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;

pub struct BoltV3BuiltLiveNode {
    node: LiveNode,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
}

impl BoltV3BuiltLiveNode {
    pub fn new(node: LiveNode, submit_admission: Arc<BoltV3SubmitAdmissionState>) -> Self {
        Self {
            node,
            submit_admission,
        }
    }

    pub fn node(&self) -> &LiveNode {
        &self.node
    }

    pub fn node_mut(&mut self) -> &mut LiveNode {
        &mut self.node
    }

    pub fn submit_admission(&self) -> &BoltV3SubmitAdmissionState {
        self.submit_admission.as_ref()
    }
}
```

Change the public build functions so the one bolt-v3 build path returns the wrapper:

```rust
pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3BuiltLiveNode, BoltV3LiveNodeError>
```

```rust
pub fn build_bolt_v3_live_node_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<BoltV3BuiltLiveNode, BoltV3LiveNodeError>
```

```rust
pub fn build_bolt_v3_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3BuiltLiveNode, BoltV3RegistrationSummary), BoltV3LiveNodeError>
```

Inside `build_live_node_with_clients`, create the shared state before strategy registration:

```rust
let submit_admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed());
```

Pass `submit_admission.clone()` into strategy registration and return:

```rust
Ok((BoltV3BuiltLiveNode::new(node, submit_admission), summary))
```

- [ ] **Step 3: Pass admission through strategy registration context**

In `src/bolt_v3_strategy_registration.rs`, add:

```rust
use std::sync::Arc;

use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
```

Extend the context:

```rust
#[derive(Clone)]
pub struct StrategyRegistrationContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy: &'a LoadedStrategy,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
}
```

When invoking a binding:

```rust
StrategyRegistrationContext {
    loaded,
    strategy,
    resolved,
    submit_admission: submit_admission.clone(),
}
```

- [ ] **Step 4: Require admission in strategy build context**

In `src/strategies/registry.rs`, import:

```rust
use crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState;
```

Add field and constructor parameter:

```rust
submit_admission: Arc<BoltV3SubmitAdmissionState>,
```

```rust
pub fn try_new(
    fee_provider: Arc<dyn FeeProvider>,
    reference_publish_topic: String,
    decision_evidence: Option<Arc<dyn BoltV3DecisionEvidenceWriter>>,
    submit_admission: Option<Arc<BoltV3SubmitAdmissionState>>,
) -> Result<Self> {
    let decision_evidence =
        decision_evidence.ok_or_else(|| anyhow!("decision evidence writer is required"))?;
    let submit_admission =
        submit_admission.ok_or_else(|| anyhow!("submit admission state is required"))?;
    Ok(Self {
        fee_provider,
        reference_publish_topic,
        decision_evidence,
        submit_admission,
    })
}
```

Add accessor:

```rust
pub fn submit_admission(&self) -> &BoltV3SubmitAdmissionState {
    self.submit_admission.as_ref()
}
```

- [ ] **Step 5: Wire the binary-oracle runtime binding**

In `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`, pass:

```rust
Some(context.submit_admission.clone()),
```

to `StrategyBuildContext::try_new`.

- [ ] **Step 6: Arm admission in the runner wrapper**

Change `run_bolt_v3_live_node` to take the wrapper:

```rust
pub async fn run_bolt_v3_live_node(
    built: &mut BoltV3BuiltLiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let gate_report = check_bolt_v3_live_canary_gate(loaded)
        .await
        .map_err(BoltV3LiveNodeError::LiveCanaryGate)?;
    built
        .submit_admission()
        .arm(gate_report)
        .map_err(BoltV3LiveNodeError::SubmitAdmission)?;
    built.node_mut().run().await.map_err(BoltV3LiveNodeError::Run)
}
```

Add `BoltV3LiveNodeError::SubmitAdmission(BoltV3SubmitAdmissionError)` and wire `Display`/`source`.

- [ ] **Step 7: Update build tests**

Every test using the returned NT node changes from:

```rust
let (node, summary) =
    build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)?;
assert_eq!(node.state(), NodeState::Idle);
```

to:

```rust
let (built, summary) =
    build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)?;
assert_eq!(built.node().state(), NodeState::Idle);
```

Every test needing `&mut LiveNode` changes from:

```rust
connect_bolt_v3_clients(&mut node, &loaded).await?;
```

to:

```rust
connect_bolt_v3_clients(built.node_mut(), &loaded).await?;
```

- [ ] **Step 8: Run build wiring tests**

Run:

```bash
cargo test --test bolt_v3_submit_admission bolt_v3_build_returns_unarmed_submit_admission_state -- --nocapture
cargo test --test bolt_v3_strategy_registration -- --nocapture
cargo test --test bolt_v3_client_registration -- --nocapture
cargo test --test live_node_run -- --nocapture
```

Expected: all pass after build API updates.

---

## Task 4: Enforce Admission Before Strategy NT Submit

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs`
- Modify: `tests/bolt_v3_submit_admission.rs`

- [ ] **Step 1: Add a source-order fence**

Append to `tests/bolt_v3_submit_admission.rs`:

```rust
#[test]
fn eth_chainlink_taker_records_evidence_then_admits_then_submits_once() {
    let source = std::fs::read_to_string("src/strategies/eth_chainlink_taker.rs")
        .expect("strategy source should be readable");
    let helper_index = source
        .find("fn submit_order_with_decision_evidence")
        .expect("strategy must expose the submit helper");
    let evidence_index = source
        .find("record_order_intent(&intent)")
        .expect("helper must record decision evidence");
    let admission_index = source
        .find("submit_admission().admit(")
        .expect("helper must call submit admission");
    let submit_index = source
        .find("self.submit_order(")
        .expect("helper must contain the only NT submit call");

    assert!(helper_index < evidence_index);
    assert!(evidence_index < admission_index);
    assert!(admission_index < submit_index);
    assert_eq!(source.matches("self.submit_order(").count(), 1);
}
```

Run:

```bash
cargo test --test bolt_v3_submit_admission eth_chainlink_taker_records_evidence_then_admits_then_submits_once -- --nocapture
```

Expected: FAIL because `submit_admission().admit(` is not present.

- [ ] **Step 2: Compute strategy-owned order notional**

In `src/strategies/eth_chainlink_taker.rs`, import:

```rust
use std::str::FromStr;

use rust_decimal::Decimal;
```

Add helper near `submit_order_with_decision_evidence`:

```rust
fn order_notional(price: Price, quantity: Quantity) -> Result<Decimal> {
    let price = Decimal::from_str(&price.to_string())
        .map_err(|error| anyhow::anyhow!("entry/exit order price is not decimal: {error}"))?;
    let quantity = Decimal::from_str(&quantity.to_string())
        .map_err(|error| anyhow::anyhow!("entry/exit order quantity is not decimal: {error}"))?;
    Ok(price * quantity)
}
```

This stays strategy-owned because only the strategy knows how to interpret the economic notional for its order shape. Core admission only compares a supplied positive Decimal against the gate cap.

- [ ] **Step 3: Extend the submit helper**

Change the helper signature:

```rust
fn submit_order_with_decision_evidence(
    &mut self,
    intent: BoltV3OrderIntentEvidence,
    order: OrderAny,
    client_id: ClientId,
    notional: Decimal,
) -> Result<()> {
    self.context
        .decision_evidence()
        .record_order_intent(&intent)?;
    self.context
        .submit_admission()
        .admit(&crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest {
            strategy_id: intent.strategy_id.clone(),
            client_order_id: intent.client_order_id.clone(),
            instrument_id: intent.instrument_id.clone(),
            notional,
        })?;
    self.submit_order(order, None, Some(client_id))
}
```

Update entry and exit call sites:

```rust
let notional = Self::order_notional(price, quantity)?;
if let Err(error) =
    self.submit_order_with_decision_evidence(intent, order, client_id, notional)
{
    self.clear_pending_entry_state();
    return Err(error);
}
```

Use the existing exit rollback branch for exit.

- [ ] **Step 4: Run the source-order fence**

Run:

```bash
cargo test --test bolt_v3_submit_admission eth_chainlink_taker_records_evidence_then_admits_then_submits_once -- --nocapture
```

Expected: PASS.

---

## Task 5: Behavior Tests For Admission Fail-closed Strategy Submit

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs`

- [ ] **Step 1: Add strategy test for over-cap rejection before NT submit**

In the `#[cfg(test)]` module in `src/strategies/eth_chainlink_taker.rs`, add a helper:

```rust
fn armed_submit_admission(
    max_live_order_count: u32,
    max_notional_per_order: Decimal,
) -> Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
    let admission = Arc::new(crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed());
    admission
        .arm(crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport {
            approval_id: "APPROVAL-001".to_string(),
            no_submit_readiness_report_path: std::path::PathBuf::from("reports/no-submit-readiness.json"),
            max_no_submit_readiness_report_bytes: 4096,
            max_live_order_count,
            max_notional_per_order,
            root_max_notional_per_order: Decimal::new(10, 0),
        })
        .expect("test admission should arm");
    admission
}
```

Change the existing test strategy helper to accept admission:

```rust
fn test_strategy_with_fee_provider_decision_evidence_and_admission(
    fee_provider: Arc<dyn crate::clients::polymarket::FeeProvider>,
    decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState>,
) -> EthChainlinkTaker
```

Then add:

```rust
#[test]
fn submit_admission_rejects_over_cap_before_nt_submit() {
    let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_admission(
        RecordingFeeProvider::cold(),
        Arc::new(crate::bolt_v3_decision_evidence::RecordingDecisionEvidenceWriter::default()),
        armed_submit_admission(1, Decimal::new(10, 2)),
    );
    let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
    let order = OrderAny::Limit(
        nautilus_model::orders::LimitOrder::new_checked(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from(strategy.config.strategy_id.as_str()),
            instrument_id,
            client_order_id,
            OrderSide::Buy,
            quantity,
            price,
            TimeInForce::Fok,
            None,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("limit order should be valid"),
    );
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
        strategy_id: strategy.config.strategy_id.clone(),
        intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        instrument_id: instrument_id.to_string(),
        client_order_id: client_order_id.to_string(),
        order_side: "Buy".to_string(),
        price: price.to_string(),
        quantity: quantity.to_string(),
    };

    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            ClientId::from("POLYMARKET"),
            Decimal::new(50, 2),
        )
        .expect_err("over-cap admission must reject before NT submit");

    assert!(
        error.to_string().contains("exceeds cap"),
        "{error:#}"
    );
}
```

This test uses an unregistered direct order. If NT submit were reached, the failure would be an NT registration/core error instead of the admission error.

- [ ] **Step 2: Add strategy test for exhausted count before NT submit**

Add:

```rust
#[test]
fn submit_admission_rejects_second_strategy_submit_before_nt_submit() {
    let admission = armed_submit_admission(1, Decimal::new(100, 2));
    admission
        .admit(&crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest {
            strategy_id: "ETHCHAINLINKTAKER-001".to_string(),
            client_order_id: "O-19700101-000000-001-001-0".to_string(),
            instrument_id: "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
            notional: Decimal::new(50, 2),
        })
        .expect("first submit should consume budget");

    let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_admission(
        RecordingFeeProvider::cold(),
        Arc::new(crate::bolt_v3_decision_evidence::RecordingDecisionEvidenceWriter::default()),
        admission,
    );
    let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
    let order = OrderAny::Limit(
        nautilus_model::orders::LimitOrder::new_checked(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from(strategy.config.strategy_id.as_str()),
            instrument_id,
            client_order_id,
            OrderSide::Buy,
            quantity,
            price,
            TimeInForce::Fok,
            None,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("limit order should be valid"),
    );
    let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
        strategy_id: strategy.config.strategy_id.clone(),
        intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
        instrument_id: instrument_id.to_string(),
        client_order_id: client_order_id.to_string(),
        order_side: "Buy".to_string(),
        price: price.to_string(),
        quantity: quantity.to_string(),
    };

    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            ClientId::from("POLYMARKET"),
            Decimal::new(50, 2),
        )
        .expect_err("exhausted admission must reject before NT submit");

    assert!(
        error.to_string().contains("order count exhausted"),
        "{error:#}"
    );
}
```

- [ ] **Step 3: Prove evidence failure does not consume admission budget**

Update `decision_evidence_failure_rejects_before_nt_submit` to construct the strategy with an armed admission handle and assert evidence failure leaves budget untouched:

```rust
let admission = armed_submit_admission(1, Decimal::new(100, 2));
let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_admission(
    RecordingFeeProvider::cold(),
    Arc::new(FailingDecisionEvidenceWriter),
    admission.clone(),
);
```

Call the helper with the computed notional argument:

```rust
let error = strategy
    .submit_order_with_decision_evidence(
        intent,
        order,
        ClientId::from("POLYMARKET"),
        Decimal::new(50, 2),
    )
    .expect_err("evidence failure must reject before NT submit");

assert!(
    error.to_string().contains("intent write failed"),
    "{error:#}"
);
assert_eq!(
    admission.admitted_order_count(),
    0,
    "evidence failure must happen before admission budget consumption"
);
```

- [ ] **Step 4: Run strategy admission tests**

Run:

```bash
cargo test --lib submit_admission_rejects_over_cap_before_nt_submit -- --nocapture
cargo test --lib submit_admission_rejects_second_strategy_submit_before_nt_submit -- --nocapture
cargo test --lib decision_evidence_failure_rejects_before_nt_submit -- --nocapture
```

Expected: all pass.

---

## Task 6: Update Existing Tests And Source Fences

**Files:**
- Modify: `tests/bolt_v3_decision_evidence.rs`
- Modify: `tests/bolt_v3_client_registration.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/live_node_run.rs`
- Modify: `tests/eth_chainlink_taker_runtime.rs`
- Modify: `tests/support/mod.rs`
- Modify: `src/live_node_setup.rs` tests if still compiling against `StrategyBuildContext::try_new`.

- [ ] **Step 1: Update support writer/context helpers**

Where integration tests create `StrategyBuildContext`, add:

```rust
Arc::new(bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed())
```

or an armed helper when the test expects submit success.

For runtime submit tests in `tests/eth_chainlink_taker_runtime.rs`, use an armed helper:

```rust
fn armed_submit_admission() -> Arc<bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
    let admission = Arc::new(bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed());
    admission
        .arm(bolt_v2::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport {
            approval_id: "APPROVAL-001".to_string(),
            no_submit_readiness_report_path: std::path::PathBuf::from("reports/no-submit-readiness.json"),
            max_no_submit_readiness_report_bytes: 4096,
            max_live_order_count: 1,
            max_notional_per_order: rust_decimal::Decimal::new(1000, 0),
            root_max_notional_per_order: rust_decimal::Decimal::new(1000, 0),
        })
        .expect("runtime test admission should arm");
    admission
}
```

- [ ] **Step 2: Strengthen decision evidence source fence**

In `tests/bolt_v3_decision_evidence.rs`, keep:

```rust
assert_eq!(
    direct_submit_count, 1,
    "only submit_order_with_decision_evidence may call NT submit directly"
);
```

Add:

```rust
assert!(
    source[..submit_index].contains("submit_admission().admit("),
    "submit admission must run before the only direct NT submit call"
);
```

- [ ] **Step 3: Run updated existing tests**

Run:

```bash
cargo test --test bolt_v3_decision_evidence -- --nocapture
cargo test --test bolt_v3_submit_admission -- --nocapture
cargo test --test bolt_v3_client_registration -- --nocapture
cargo test --test bolt_v3_strategy_registration -- --nocapture
cargo test --test live_node_run -- --nocapture
cargo test --test eth_chainlink_taker_runtime -- --nocapture
```

Expected: all pass.

---

## Task 7: Verification Gate And no-mistakes Triage

**Files:**
- Modify: `specs/001-thin-live-canary-path/tasks.md` only after all commands pass.

- [ ] **Step 1: Run exact Phase 6 verification commands**

Run:

```bash
cargo fmt --check
git diff --check
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_naming.py
python3 scripts/verify_bolt_v3_core_boundary.py
cargo test --test bolt_v3_submit_admission -- --nocapture
cargo test --test bolt_v3_decision_evidence -- --nocapture
cargo test --test bolt_v3_client_registration -- --nocapture
cargo test --test bolt_v3_strategy_registration -- --nocapture
cargo test --test live_node_run -- --nocapture
cargo test --test eth_chainlink_taker_runtime -- --nocapture
cargo test --lib submit_admission -- --nocapture
cargo test --lib decision_evidence_failure_rejects_before_nt_submit -- --nocapture
cargo test --test config_parsing -- --nocapture
cargo clippy --all-targets -- -D warnings
```

If `cargo clippy` fails with `Operation not permitted` on the shared rust-verification target lock, rerun the same command with sandbox escalation and record that in the PR body.

- [ ] **Step 2: Run no-mistakes triage**

Run:

```bash
/private/tmp/no-mistakes-soak-bin status
/private/tmp/no-mistakes-soak-bin runs --limit 5
```

Record repo/branch, run id if present, final status, final error code if present, whether `runs` showed `error_code`, ask-user resurfacing if exercised, unrelated low/info autofix behavior if exercised, and daemon anomalies in `/private/tmp/no-mistakes-780-soak-log.md`.

- [ ] **Step 3: Mark Phase 6 tasks complete only after verification**

After the commands above pass on the exact local head, update `specs/001-thin-live-canary-path/tasks.md`:

```markdown
- [x] T028 [US2] Write failing tests in `tests/bolt_v3_submit_admission.rs` for one-order cap, over-notional rejection, missing gate report, and evidence-failure-before-admission ordering.
- [x] T029 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture`; expected failures show missing submit admission module.
- [x] T030 [US2] Add `src/bolt_v3_submit_admission.rs` with config-derived admission state initialized from `BoltV3LiveCanaryGateReport`.
- [x] T031 [US2] Wire strategy submit calls through submit admission before NT submit.
- [x] T032 [US2] Run `cargo test --test bolt_v3_submit_admission`, targeted strategy submit tests, and source-fence checks for direct `submit_order` bypasses.
```

## Self-Review

Spec coverage:

- FR-009 maps to Tasks 1-4 and 6.
- FR-010 maps to Tasks 4-6 and preserves the Phase 5 decision-evidence failure tests.
- FR-011 maps to the Scope and File Map: no lifecycle, reconciliation, adapter, or cache semantics are introduced.
- Phase 6 T028-T032 map directly to Tasks 1-7.
- Phase 7 and Phase 8 remain explicitly out of scope.

Placeholder scan:

- No open-ended placeholder markers or deferred-work labels are present.
- Every code-changing task names exact files, code shapes, commands, and expected red/green results.

Type consistency:

- `BoltV3SubmitAdmissionState` is the single admission state type.
- `BoltV3SubmitAdmissionRequest` carries strategy/order/instrument labels and a strategy-computed `Decimal` notional.
- `BoltV3BuiltLiveNode` is the wrapper that lets `run_bolt_v3_live_node` arm the same admission state held by registered strategies.

Execution note:

- Implement this in a child branch from PR #316 head so the planning artifact stays in the implementation stack.
- Do not request external review until the branch is clean, pushed, and exact-head CI/checks are green.
- Do not merge without explicit user approval.
