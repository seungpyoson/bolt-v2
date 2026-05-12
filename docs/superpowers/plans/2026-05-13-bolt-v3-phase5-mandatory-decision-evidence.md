# Bolt-v3 Phase 5 Mandatory Decision Evidence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the initial bolt-v3 taker strategy impossible to construct or submit live orders without mandatory bolt-v3 decision evidence.

**Architecture:** Add one compact decision-evidence writer interface owned by bolt-v3, require it in `StrategyBuildContext`, and route the two existing taker submit sites through one evidence-recording helper before NT `submit_order`. The helper records Bolt-derived decision intent only; NT continues to own order lifecycle, adapter behavior, fills, cache, and reconciliation.

**Tech Stack:** Rust, NautilusTrader Rust APIs, TOML config, local JSONL evidence under configured persistence root, existing cargo test targets and bolt-v3 verifiers.

---

## Evidence Anchor

Current stack head for this plan: `c34f2cb72f1e0c496dd6e424aef65bd81d9b93a4` on `008-bolt-v3-live-node-strategy-registration`.

Hard evidence inspected before this plan:

- `.specify/memory/constitution.md:7-9`: bolt-v3 owns compact audit evidence for Bolt-derived decisions; NT owns order lifecycle, execution, cache, and reconciliation.
- `.specify/memory/constitution.md:13-19`: core must remain venue-, market-, and strategy-agnostic, with one config format, one secret source, one production build path, and one live submit path.
- `.specify/memory/constitution.md:25-33`: implementation must be TDD, live submit stays fail-closed, and no-mistakes is advisory until mapped to repo evidence.
- `specs/001-thin-live-canary-path/tasks.md:58-68`: Phase 5 open tasks T023-T027 require mandatory decision evidence and removal of fallback submit behavior.
- `src/strategies/registry.rs:18-22`: `StrategyBuildContext` currently carries only `fee_provider` and `reference_publish_topic`.
- `src/strategies/eth_chainlink_taker.rs:1216-1241`: `EthChainlinkTaker` stores the context but has no decision-evidence dependency.
- `src/strategies/eth_chainlink_taker.rs:2896` and `src/strategies/eth_chainlink_taker.rs:3099`: exit and entry submit currently call `self.submit_order(...)` directly.
- `src/strategies/eth_chainlink_taker.rs:3606-3621`: `EthChainlinkTakerBuilder` constructs and registers the strategy with no evidence dependency check.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:274-277`: the Phase 4e production runtime binding builds `StrategyBuildContext` without evidence.
- `src/bolt_v3_config.rs:179-207`: persistence config has `catalog_directory` and streaming fields, but no decision-evidence persistence path.
- `src/nt_runtime_capture.rs:624-687`: existing NT capture records NT events and must not become the Bolt decision-intent writer.

## Slice Boundary

This slice owns:

- TOML shape for the Bolt decision-evidence output path under `[persistence]`.
- A compact `BoltV3DecisionEvidenceWriter` interface for Bolt-derived decision intent.
- A file-backed writer initialized from TOML-controlled persistence config.
- `StrategyBuildContext` construction that fails closed when decision evidence is absent.
- Entry and exit submit evidence recording before NT `submit_order`.
- Source fences proving no direct taker `self.submit_order(` bypass remains outside the evidence helper.

This slice does not own:

- Submit-admission order-count or notional caps. That is Phase 6.
- No-submit real SSM/venue readiness. That is Phase 7.
- Tiny-capital live submit, cancel, fill, or reconciliation proof. That is Phase 8.
- NT lifecycle, adapter behavior, cache semantics, or reconciliation.

## File Map

- Create `src/bolt_v3_decision_evidence.rs`: evidence trait, record structs, file-backed JSONL writer, test writer helpers under `#[cfg(test)]`.
- Modify `src/lib.rs`: export the new module.
- Modify `src/bolt_v3_config.rs`: add `DecisionEvidenceBlock` under `PersistenceBlock`.
- Modify `tests/fixtures/bolt_v3/root.toml`: add the configured decision-evidence relative path under `[persistence.decision_evidence]`.
- Modify `tests/support/mod.rs`: add a fixture loader that rewrites the production catalog root to a temp directory before build-path tests construct file-backed evidence.
- Modify `src/strategies/registry.rs`: make `StrategyBuildContext` evidence-mandatory through a constructor and accessors.
- Modify `src/strategies/eth_chainlink_taker.rs`: use context accessors and submit through one evidence-recording helper.
- Modify `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`: build the production evidence writer from loaded config and pass it into the context.
- Modify `tests/bolt_v3_strategy_registration.rs`: prove live-node strategy registration carries a concrete decision-evidence writer.
- Create `tests/bolt_v3_decision_evidence.rs`: cross-module tests and source fences for mandatory evidence.

## Task 1: Red Tests For Missing Decision Evidence

**Files:**
- Create: `tests/bolt_v3_decision_evidence.rs`
- Modify: `src/strategies/registry.rs` test module only

- [ ] **Step 1: Write the failing integration test for context construction**

Add `tests/bolt_v3_decision_evidence.rs` with:

```rust
use std::sync::Arc;

use bolt_v2::{
    clients::polymarket::FeeProvider,
    strategies::registry::StrategyBuildContext,
};
use anyhow::Result;
use futures_util::future::{BoxFuture, FutureExt};
use rust_decimal::Decimal;

#[derive(Debug, Default)]
struct TestFeeProvider;

impl FeeProvider for TestFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<Decimal> {
        Some(Decimal::ZERO)
    }

    fn warm(&self, _token_id: &str) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

#[test]
fn strategy_build_context_rejects_missing_decision_evidence() {
    let error = StrategyBuildContext::try_new(
        Arc::new(TestFeeProvider),
        "platform.reference.test".to_string(),
        None,
    )
    .expect_err("missing decision evidence must reject context construction");

    assert!(
        error
            .to_string()
            .contains("decision evidence writer is required"),
        "{error:#}"
    );
}
```

- [ ] **Step 2: Run the red test**

Run:

```bash
cargo test --test bolt_v3_decision_evidence strategy_build_context_rejects_missing_decision_evidence -- --nocapture
```

Expected: FAIL because `tests/bolt_v3_decision_evidence.rs` or `StrategyBuildContext::try_new` does not exist.

## Task 2: Evidence Interface And Config Shape

**Files:**
- Create: `src/bolt_v3_decision_evidence.rs`
- Modify: `src/lib.rs`
- Modify: `src/bolt_v3_config.rs`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/support/mod.rs`
- Modify: `tests/config_parsing.rs`

- [ ] **Step 1: Add failing config parsing assertion**

In `tests/config_parsing.rs`, extend the existing bolt-v3 root fixture test with:

```rust
assert_eq!(
    loaded.root.persistence.decision_evidence.order_intents_relative_path,
    "bolt_v3/decision/order_intents.jsonl"
);
```

- [ ] **Step 2: Run the red config test**

Run:

```bash
cargo test --test config_parsing parses_minimal_bolt_v3_root_and_strategy_config -- --nocapture
```

Expected: FAIL because `decision_evidence` is not a field on `PersistenceBlock`.

- [ ] **Step 3: Add the minimal config block**

In `src/bolt_v3_config.rs`, change `PersistenceBlock` to:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PersistenceBlock {
    pub catalog_directory: String,
    pub decision_evidence: DecisionEvidenceBlock,
    pub streaming: StreamingBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionEvidenceBlock {
    pub order_intents_relative_path: String,
}
```

In `tests/fixtures/bolt_v3/root.toml`, add:

```toml
[persistence.decision_evidence]
order_intents_relative_path = "bolt_v3/decision/order_intents.jsonl"
```

- [ ] **Step 4: Add temp catalog fixture loader for build-path tests**

In `tests/support/mod.rs`, add:

```rust
pub fn load_bolt_v3_config_with_temp_catalog(
    label: &str,
) -> (TempCaseDir, bolt_v2::bolt_v3_config::LoadedBoltV3Config) {
    let tempdir = TempCaseDir::new(label);
    let strategy_dir = tempdir.path().join("strategies");
    fs::create_dir_all(&strategy_dir).expect("strategy fixture dir should be created");
    fs::copy(
        repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        strategy_dir.join("binary_oracle.toml"),
    )
    .expect("strategy fixture should be copied");
    let catalog_dir = tempdir.path().join("catalog");
    let root_text = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 root fixture should be readable")
        .replace(
            r#"catalog_directory = "/var/lib/bolt/catalog""#,
            &format!(r#"catalog_directory = "{}""#, catalog_dir.display()),
        );
    let root_path = tempdir.path().join("root.toml");
    fs::write(&root_path, root_text).expect("temp bolt-v3 root fixture should be written");
    let loaded = bolt_v2::bolt_v3_config::load_bolt_v3_config(&root_path)
        .expect("temp bolt-v3 fixture should load");
    (tempdir, loaded)
}
```

Any integration test that calls `build_bolt_v3_live_node_with_summary` after this slice must use this helper instead of `tests/fixtures/bolt_v3/root.toml` directly. Otherwise it will try to create the decision-evidence file under the production fixture path `/var/lib/bolt/catalog`.

- [ ] **Step 5: Add the evidence module skeleton**

In `src/lib.rs`, add:

```rust
pub mod bolt_v3_decision_evidence;
```

Create `src/bolt_v3_decision_evidence.rs`:

```rust
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::bolt_v3_config::LoadedBoltV3Config;

pub trait BoltV3DecisionEvidenceWriter: std::fmt::Debug + Send + Sync {
    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()>;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentKind {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoltV3OrderIntentEvidence {
    pub strategy_id: String,
    pub intent_kind: BoltV3OrderIntentKind,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
}

#[derive(Debug)]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    writer: Mutex<BufWriter<std::fs::File>>,
}

impl JsonlBoltV3DecisionEvidenceWriter {
    pub fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self> {
        let path = decision_evidence_path(loaded)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create decision evidence directory `{}`", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open decision evidence file `{}`", path.display()))?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("decision evidence writer lock is poisoned"))?;
        serde_json::to_writer(&mut *writer, intent).context("failed to serialize decision evidence")?;
        writer
            .write_all(b"\n")
            .context("failed to terminate decision evidence record")?;
        writer.flush().context("failed to flush decision evidence")?;
        Ok(())
    }
}

pub fn decision_evidence_path(loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
    let relative = Path::new(
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path
            .trim(),
    );
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow!(
            "persistence.decision_evidence.order_intents_relative_path must be non-empty, relative, and stay under catalog_directory"
        ));
    }
    Ok(Path::new(&loaded.root.persistence.catalog_directory).join(relative))
}
```

- [ ] **Step 6: Run config and module tests**

Run:

```bash
cargo test --test config_parsing parses_minimal_bolt_v3_root_and_strategy_config -- --nocapture
cargo test --test bolt_v3_decision_evidence strategy_build_context_rejects_missing_decision_evidence -- --nocapture
```

Expected after implementation in this task: config test passes; missing context constructor test still fails until Task 3.

## Task 3: Make StrategyBuildContext Evidence-Mandatory

**Files:**
- Modify: `src/strategies/registry.rs`
- Modify: `src/live_node_setup.rs`
- Modify: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- Modify: `src/strategies/eth_chainlink_taker.rs` test helper context
- Modify: any tests constructing `StrategyBuildContext` directly

- [ ] **Step 1: Change `StrategyBuildContext`**

In `src/strategies/registry.rs`, replace public fields with constructor plus accessors:

```rust
#[derive(Clone)]
pub struct StrategyBuildContext {
    fee_provider: Arc<dyn FeeProvider>,
    reference_publish_topic: String,
    decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
}

impl StrategyBuildContext {
    pub fn try_new(
        fee_provider: Arc<dyn FeeProvider>,
        reference_publish_topic: String,
        decision_evidence: Option<
            Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
        >,
    ) -> Result<Self> {
        let decision_evidence =
            decision_evidence.ok_or_else(|| anyhow!("decision evidence writer is required"))?;
        Ok(Self {
            fee_provider,
            reference_publish_topic,
            decision_evidence,
        })
    }

    pub fn fee_provider(&self) -> &dyn FeeProvider {
        self.fee_provider.as_ref()
    }

    pub fn fee_provider_arc(&self) -> Arc<dyn FeeProvider> {
        self.fee_provider.clone()
    }

    pub fn reference_publish_topic(&self) -> &str {
        &self.reference_publish_topic
    }

    pub fn decision_evidence(
        &self,
    ) -> &dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter {
        self.decision_evidence.as_ref()
    }
}
```

- [ ] **Step 2: Update existing context call sites**

Replace existing struct literals such as:

```rust
StrategyBuildContext {
    fee_provider,
    reference_publish_topic,
}
```

with:

```rust
StrategyBuildContext::try_new(
    fee_provider,
    reference_publish_topic,
    Some(decision_evidence),
)?
```

For unit tests inside the library crate, add a `#[cfg(test)]` helper writer in `src/bolt_v3_decision_evidence.rs`:

```rust
#[cfg(test)]
#[derive(Debug, Default)]
pub struct RecordingDecisionEvidenceWriter {
    records: Mutex<Vec<BoltV3OrderIntentEvidence>>,
}

#[cfg(test)]
impl RecordingDecisionEvidenceWriter {
    pub fn records(&self) -> Vec<BoltV3OrderIntentEvidence> {
        self.records.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

#[cfg(test)]
impl BoltV3DecisionEvidenceWriter for RecordingDecisionEvidenceWriter {
    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(intent.clone());
        Ok(())
    }
}
```

Integration tests cannot import `#[cfg(test)]` library items. If an integration test needs a fake writer, define a local test struct implementing the public `BoltV3DecisionEvidenceWriter` trait inside that integration test or in `tests/support/mod.rs`.

- [ ] **Step 3: Run context tests**

Run:

```bash
cargo test --test bolt_v3_decision_evidence strategy_build_context_rejects_missing_decision_evidence -- --nocapture
cargo test --lib strategies::registry -- --nocapture
```

Expected: both pass after every context literal has been replaced.

## Task 4: Record Evidence Before Entry And Exit Submit

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs`
- Modify: `tests/bolt_v3_decision_evidence.rs`

- [ ] **Step 1: Add a source fence test**

Add to `tests/bolt_v3_decision_evidence.rs`:

```rust
#[test]
fn eth_chainlink_taker_has_no_direct_submit_order_bypass() {
    let source = std::fs::read_to_string("src/strategies/eth_chainlink_taker.rs")
        .expect("strategy source should be readable");
    let direct_submit_count = source.matches("self.submit_order(").count();
    assert_eq!(
        direct_submit_count, 1,
        "only submit_order_with_decision_evidence may call NT submit directly"
    );
    let helper_index = source
        .find("fn submit_order_with_decision_evidence")
        .expect("strategy must expose one submit helper");
    let submit_index = source
        .find("self.submit_order(")
        .expect("helper must contain the only direct NT submit call");
    assert!(
        helper_index < submit_index,
        "the only direct submit call must be inside the evidence helper"
    );
    assert!(
        source[..submit_index].contains("record_order_intent"),
        "decision evidence must be recorded before the only direct NT submit call"
    );
}
```

- [ ] **Step 2: Run the red source fence**

Run:

```bash
cargo test --test bolt_v3_decision_evidence eth_chainlink_taker_has_no_direct_submit_order_bypass -- --nocapture
```

Expected: FAIL because the two current direct calls are present.

- [ ] **Step 3: Add the submit helper**

In `src/strategies/eth_chainlink_taker.rs`, add a helper near the existing submit methods:

```rust
fn submit_order_with_decision_evidence(
    &mut self,
    intent: crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
    order: nautilus_model::orders::OrderAny,
    client_id: ClientId,
) -> Result<()> {
    self.context.decision_evidence().record_order_intent(&intent)?;
    self.submit_order(order, None, Some(client_id))
}
```

Build `BoltV3OrderIntentEvidence` at the existing exit and entry submit sites using the values already computed immediately before the current `self.submit_order(...)` calls:

```rust
let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
    strategy_id: self.config.strategy_id.clone(),
    intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
    instrument_id: instrument_id.to_string(),
    client_order_id: client_order_id.to_string(),
    order_side: format!("{order_side:?}"),
    price: price.to_string(),
    quantity: quantity.to_string(),
};
```

Use `BoltV3OrderIntentKind::Exit` at the exit submit site.

- [ ] **Step 4: Re-run the source fence**

Run:

```bash
cargo test --test bolt_v3_decision_evidence eth_chainlink_taker_has_no_direct_submit_order_bypass -- --nocapture
```

Expected: PASS and source count for direct `self.submit_order(` is one: the single call inside `submit_order_with_decision_evidence`.

## Task 5: Prove Persistence Failure Blocks Before NT Submit

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs` test module

- [ ] **Step 1: Add failing writer helper**

In the `#[cfg(test)]` module in `src/strategies/eth_chainlink_taker.rs`, add:

```rust
#[derive(Debug)]
struct FailingDecisionEvidenceWriter;

impl crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter for FailingDecisionEvidenceWriter {
    fn record_order_intent(
        &self,
        _intent: &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("intent write failed")
    }
}
```

- [ ] **Step 2: Add focused unit test around the helper**

First change the existing helper at `src/strategies/eth_chainlink_taker.rs:4528-4563` into two helpers:

```rust
fn test_strategy() -> EthChainlinkTaker {
    test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::cold(),
        Arc::new(crate::bolt_v3_decision_evidence::RecordingDecisionEvidenceWriter::default()),
    )
}

fn test_strategy_with_fee_provider(
    fee_provider: Arc<dyn crate::clients::polymarket::FeeProvider>,
) -> EthChainlinkTaker {
    test_strategy_with_fee_provider_and_decision_evidence(
        fee_provider,
        Arc::new(crate::bolt_v3_decision_evidence::RecordingDecisionEvidenceWriter::default()),
    )
}

fn test_strategy_with_fee_provider_and_decision_evidence(
    fee_provider: Arc<dyn crate::clients::polymarket::FeeProvider>,
    decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
) -> EthChainlinkTaker {
    EthChainlinkTaker::new(
        EthChainlinkTakerConfig {
            strategy_id: "ETHCHAINLINKTAKER-001".to_string(),
            client_id: "POLYMARKET".to_string(),
            warmup_tick_count: 20,
            period_duration_secs: 300,
            reentry_cooldown_secs: 30,
            max_position_usdc: 1000.0,
            book_impact_cap_bps: 15,
            risk_lambda: 0.5,
            worst_case_ev_min_bps: -20,
            exit_hysteresis_bps: 5,
            vol_window_secs: 60,
            vol_gap_reset_secs: 10,
            vol_min_observations: 20,
            vol_bridge_valid_secs: 10,
            pricing_kurtosis: 0.0,
            theta_decay_factor: 0.0,
            forced_flat_stale_chainlink_ms: 1500,
            forced_flat_thin_book_min_liquidity: 100.0,
            lead_agreement_min_corr: 0.8,
            lead_jitter_max_ms: 250,
        },
        StrategyBuildContext::try_new(
            fee_provider,
            "platform.reference.test.chainlink".to_string(),
            Some(decision_evidence),
        )
        .expect("test strategy context should include decision evidence"),
    )
}
```

Then add a test that calls `submit_order_with_decision_evidence` with a failing writer and a constructed order from that helper:

```rust
#[test]
fn decision_evidence_failure_rejects_before_nt_submit() {
    let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
        RecordingFeeProvider::cold(),
        Arc::new(FailingDecisionEvidenceWriter),
    );
    let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
    let quantity = Quantity::new(1.0, 2);
    let price = Price::new(0.50, 2);
    let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
    let order = strategy.core.order_factory().limit(
        instrument_id,
        OrderSide::Buy,
        quantity,
        price,
        Some(TimeInForce::Fok),
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
        Some(client_order_id),
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
        .submit_order_with_decision_evidence(intent, order, ClientId::from("polymarket_main"))
        .expect_err("evidence failure must reject before NT submit");

    assert!(error.to_string().contains("intent write failed"), "{error:#}");
}
```

- [ ] **Step 3: Run the red/green helper test**

Run before helper implementation:

```bash
cargo test --lib decision_evidence_failure_rejects_before_nt_submit -- --nocapture
```

Expected before implementation: FAIL because helper/test support is absent.

Run after implementation:

```bash
cargo test --lib decision_evidence_failure_rejects_before_nt_submit -- --nocapture
```

Expected after implementation: PASS and error contains `intent write failed`.

## Task 6: Wire Production Evidence Writer Through The Archetype Binding

**Files:**
- Modify: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_client_registration.rs` if it constructs a full bolt-v3 live node after the writer becomes mandatory

- [ ] **Step 1: Add failing strategy-registration test**

Extend `tests/bolt_v3_strategy_registration.rs`:

```rust
#[test]
fn binary_oracle_runtime_binding_requires_decision_evidence_persistence_config() {
    let (_tempdir, loaded) =
        support::load_bolt_v3_config_with_temp_catalog("decision-evidence-config");

    assert_eq!(
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path,
        "bolt_v3/decision/order_intents.jsonl"
    );
}
```

Also update build-path tests in `tests/bolt_v3_strategy_registration.rs` that call `build_bolt_v3_live_node_with_summary` so they load config via `support::load_bolt_v3_config_with_temp_catalog(...)`. Keep pure config/raw-mapping tests on the shared fixture when no file-backed writer is constructed.

- [ ] **Step 2: Build writer in the runtime binding**

In `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`, add:

```rust
let decision_evidence = Arc::new(
    crate::bolt_v3_decision_evidence::JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(
        context.loaded,
    )
    .map_err(|error| binding_message(context, error.to_string()))?,
);
let build_context = StrategyBuildContext::try_new(
    fee_provider,
    parameters.runtime.reference_publish_topic,
    Some(decision_evidence),
)
.map_err(|error| binding_message(context, error.to_string()))?;
```

- [ ] **Step 3: Run registration tests**

Run:

```bash
cargo test --test bolt_v3_strategy_registration -- --nocapture
```

Expected: PASS after the production binding constructs the mandatory writer from TOML-controlled persistence config.

## Task 7: Phase Verification Gate

**Files:**
- Modify: `specs/001-thin-live-canary-path/tasks.md` only after all commands pass.

- [ ] **Step 1: Run exact Phase 5 verification commands**

Run:

```bash
cargo fmt --check
git diff --check
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_naming.py
cargo test --test bolt_v3_decision_evidence -- --nocapture
cargo test --test bolt_v3_strategy_registration -- --nocapture
cargo test --test config_parsing -- --nocapture
cargo test --lib decision_evidence -- --nocapture
cargo clippy --lib --test bolt_v3_decision_evidence --test bolt_v3_strategy_registration --test config_parsing -- -D warnings
```

- [ ] **Step 2: Run no-mistakes triage**

Run:

```bash
/private/tmp/no-mistakes-soak-bin status
/private/tmp/no-mistakes-soak-bin runs --limit 5
```

Record repo/branch, run id if present, final status, final error code if present, whether `runs` showed `error_code`, ask-user resurfacing if exercised, unrelated low/info autofix behavior if exercised, and daemon anomalies in `/private/tmp/no-mistakes-780-soak-log.md`.

- [ ] **Step 3: Mark tasks only after verification**

After the commands above pass on the exact local head, update `specs/001-thin-live-canary-path/tasks.md`:

```markdown
- [x] T023 [US2] Write failing tests that construct the strategy without decision evidence and expect construction rejection.
- [x] T024 [US2] Write failing tests that simulate evidence persistence failure and expect submit rejection before NT submit.
- [x] T025 [US2] Remove optional/fallback evidence submit path from `src/strategies/eth_chainlink_taker.rs`.
- [x] T026 [US2] Make bolt-v3 strategy registration provide mandatory decision evidence.
- [x] T027 [US2] Run targeted strategy tests and source-fence search for fallback direct submit branches.
```

## Self-Review

Spec coverage:

- FR-010 maps to Tasks 1, 3, 4, and 5.
- FR-011 maps to the slice boundary and Task 4 source fence; no lifecycle, reconciliation, adapter, or cache behavior is added.
- Phase 5 T023-T027 map directly to Tasks 1-7.
- Phase 6 submit-admission cap consumption is explicitly excluded and remains open.

Placeholder scan:

- This plan avoids open-ended placeholders and names exact files, commands, expected red failures, expected green outcomes, and verification gates.

Type consistency:

- `StrategyBuildContext::try_new` is used consistently by production binding and tests.
- `BoltV3DecisionEvidenceWriter::record_order_intent` is the only evidence call path required before taker NT submit.
- `JsonlBoltV3DecisionEvidenceWriter::from_loaded_config` consumes TOML persistence config, not code-level path values.

Execution handoff:

Plan complete and saved to `docs/superpowers/plans/2026-05-13-bolt-v3-phase5-mandatory-decision-evidence.md`.

Execution option for the next coding turn:

1. Subagent-driven: dispatch a fresh implementation worker for each task and review between tasks.
2. Inline execution: execute tasks in this session with TDD red-green checkpoints.

Do not begin implementation until the chosen execution mode is explicit and this planning branch is either merged into the stack or treated as the approved handoff artifact.
