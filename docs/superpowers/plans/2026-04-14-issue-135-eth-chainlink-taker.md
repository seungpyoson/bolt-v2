# Issue 135 ETH Chainlink Taker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first production `eth_chainlink_taker` strategy kind on top of the landed `#134/#136/#157` seams without widening into runtime/config code.

**Architecture:** Keep one concrete strategy module with a thin NT-facing actor shell and pure decision helpers. The shell owns subscriptions, cache reads, switch handling, and order submission; pure helpers own side selection, uncertainty-band math, EV, sizing, cooldown, and forced-flat predicates so most behavior is proven by deterministic tests.

**Tech Stack:** Rust, Nautilus Trader `Strategy` + `DataActor`, `toml::Value`, `rust_decimal`, `tokio`, `msgbus`, repo test support in `tests/support`.

---

## File Map

- `src/strategies/eth_chainlink_taker.rs`
  - New production strategy kind, builder, config, actor shell, pure helpers, and unit tests.
- `src/strategies/mod.rs`
  - Register `EthChainlinkTakerBuilder` in `production_strategy_registry()`.
- `tests/eth_chainlink_taker_runtime.rs`
  - New integration tests for registry registration, selection/reference wiring, same-session attribution, and end-to-end harness behavior.
- `tests/support/mod.rs`
  - Extend existing mock helpers only if needed for recording strategy-side orders/subscriptions in integration tests.

No other production files are in scope. If implementation needs `src/main.rs`, `src/validate.rs`, `src/platform/*`, `src/clients/*`, or `src/live_config.rs`, stop and report the blocker.

### Task 1: Register The Concrete Strategy Kind

**Files:**
- Create: `src/strategies/eth_chainlink_taker.rs`
- Modify: `src/strategies/mod.rs`
- Test: `src/strategies/eth_chainlink_taker.rs`

- [ ] **Step 1: Write the failing registration and builder-validation tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::production_strategy_registry;

    #[test]
    fn production_registry_registers_eth_chainlink_taker_kind() {
        let registry = production_strategy_registry().expect("registry should build");
        assert!(registry.get("eth_chainlink_taker").is_some());
    }

    #[test]
    fn builder_requires_strategy_id_and_client_id() {
        let raw = toml::toml! {
            warmup_tick_count = 20
        }
        .into();
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(
            &raw,
            "strategies[0].config",
            &mut errors,
        );

        assert!(errors.iter().any(|e| e.field == "strategies[0].config.strategy_id"));
        assert!(errors.iter().any(|e| e.field == "strategies[0].config.client_id"));
    }
}
```

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test production_registry_registers_eth_chainlink_taker_kind --lib`
Expected: FAIL because the registry is currently empty and the new builder/module does not exist yet.

- [ ] **Step 3: Add the minimal production shell**

```rust
// src/strategies/mod.rs
pub mod eth_chainlink_taker;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<eth_chainlink_taker::EthChainlinkTakerBuilder>()?;
    Ok(registry)
}
```

```rust
// src/strategies/eth_chainlink_taker.rs
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct EthChainlinkTakerConfig {
    strategy_id: String,
    client_id: String,
    warmup_tick_count: u64,
    reentry_cooldown_secs: u64,
    max_position_usdc: f64,
    book_impact_cap_bps: u64,
    risk_lambda: f64,
    worst_case_ev_min_bps: i64,
    exit_hysteresis_bps: i64,
    forced_flat_stale_chainlink_ms: u64,
    forced_flat_thin_book_min_liquidity: f64,
    lead_agreement_min_corr: f64,
    lead_jitter_max_ms: u64,
}

#[derive(Debug)]
pub struct EthChainlinkTaker {
    core: StrategyCore,
    context: StrategyBuildContext,
    config: EthChainlinkTakerConfig,
}

impl DataActor for EthChainlinkTaker {}

nautilus_strategy!(EthChainlinkTaker);

pub struct EthChainlinkTakerBuilder;
```

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run: `cargo test production_registry_registers_eth_chainlink_taker_kind --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/strategies/mod.rs src/strategies/eth_chainlink_taker.rs
git commit -m "feat: register eth chainlink taker strategy"
```

### Task 2: Build The Strategy Shell And Pre-Math Gates

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs`
- Test: `src/strategies/eth_chainlink_taker.rs`

- [ ] **Step 1: Write the failing shell-state tests for items 1-7**

```rust
#[test]
fn switch_resets_only_active_market_state() {
    let mut strategy = test_strategy();
    strategy.cooldowns.insert("A".to_string(), 123);
    strategy.recovery = true;
    strategy.active.interval_open = Some(3_000.0);
    strategy.active.warmup_count = 7;

    strategy.apply_selection_snapshot(active_snapshot("B"));

    assert_eq!(strategy.cooldowns.get("A"), Some(&123));
    assert!(strategy.recovery);
    assert_eq!(strategy.active.market_id.as_deref(), Some("B"));
    assert!(strategy.active.interval_open.is_none());
    assert_eq!(strategy.active.warmup_count, 0);
    assert!(!strategy.active.fees_ready_for_active);
}

#[test]
fn interval_open_captures_first_chainlink_tick_at_or_after_market_start() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(reference_tick(900, 3_100.0));
    assert!(strategy.active.interval_open.is_none());

    strategy.observe_reference_snapshot(reference_tick(1_000, 3_101.0));
    assert_eq!(strategy.active.interval_open, Some(3_101.0));
}

#[test]
fn fees_ready_stays_false_until_fee_provider_is_warm() {
    let fee_provider = RecordingFeeProvider::cold();
    let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());
    strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

    assert!(!strategy.active.fees_ready_for_active);

    fee_provider.set_fee("UP-TOKEN", dec!(1.75));
    strategy.refresh_fee_readiness();

    assert!(strategy.active.fees_ready_for_active);
}

#[test]
fn warmup_requires_consecutive_fresh_ticks() {
    let mut strategy = test_strategy();
    strategy.config.warmup_tick_count = 3;
    strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

    strategy.observe_reference_snapshot(reference_tick(1_000, 3_100.0));
    strategy.observe_reference_snapshot(reference_tick(1_100, 3_101.0));
    assert!(!strategy.active.warmup_complete());

    strategy.observe_reference_snapshot(reference_tick(1_200, 3_102.0));
    assert!(strategy.active.warmup_complete());
}
```

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test switch_resets_only_active_market_state --lib`
Expected: FAIL because the selection, interval-open, fee-readiness, and warmup shell methods do not exist yet.

- [ ] **Step 3: Implement the shell methods and state containers**

```rust
#[derive(Debug, Clone, Default)]
struct ActiveMarketState {
    market_id: Option<String>,
    instrument_id: Option<InstrumentId>,
    active_token_id: Option<String>,
    interval_open: Option<f64>,
    warmup_count: u64,
    fees_ready_for_active: bool,
    forced_flat: bool,
}

impl EthChainlinkTaker {
    fn apply_selection_snapshot(&mut self, snapshot: RuntimeSelectionSnapshot) {
        let next_market_id = active_market_id(&snapshot);
        if self.active.market_id.as_deref() == next_market_id.as_deref() {
            return;
        }

        if let Some(previous) = self.active.instrument_id {
            self.unsubscribe_book_deltas(previous, None, None);
        }

        self.active = ActiveMarketState::from_snapshot(&snapshot);
        self.trigger_fee_warm();
    }

    fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        if self.try_capture_interval_open(snapshot) {
            self.active.warmup_count += 1;
        }
    }

    fn refresh_fee_readiness(&mut self) {
        self.active.fees_ready_for_active = self
            .active
            .active_token_id
            .as_deref()
            .and_then(|token| self.context.fee_provider.fee_bps(token))
            .is_some();
    }
}
```

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run: `cargo test interval_open_captures_first_chainlink_tick_at_or_after_market_start --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/strategies/eth_chainlink_taker.rs
git commit -m "feat: add eth taker subscription and gate shell"
```

### Task 3: Mid-Stream Contract Check

**Files:**
- Modify: `docs/superpowers/plans/2026-04-14-issue-135-eth-chainlink-taker.md`

- [ ] **Step 1: Freeze the pre-math checkpoint**

Add a short execution note under this task recording completion of:

- module scaffold and builder
- strategy shell
- selection/reference subscriptions
- switch handling
- fee-readiness gate
- interval-open capture
- warmup counter

- [ ] **Step 2: Run the one-time contract review before EV math**

Reviewer target:

```text
Review only implementation-order items 1-7 for #135.
Do not review EV math, sizing, entry, exit, cooldown, recovery, or forced-flat logic yet.
Check:
- selection/reference/book subscription scaffolding
- active-market reset boundaries
- fee readiness fail-closed behavior
- interval-open capture
- warmup accounting
- no widened runtime/config edits
Return only mechanical contract findings.
```

- [ ] **Step 3: Address any findings before continuing**

Run: `git diff --stat`
Expected: only `src/strategies/eth_chainlink_taker.rs` and `src/strategies/mod.rs` changed so far, plus this plan note if you record the checkpoint.

### Task 4: Add Pure Decision Logic For Lead, Band, EV, And Sizing

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs`
- Test: `src/strategies/eth_chainlink_taker.rs`

- [ ] **Step 1: Write the failing pure-logic tests**

```rust
#[test]
fn both_positive_sides_choose_higher_worst_case_ev() {
    let decision = decide_entry_side(&DecisionInputs {
        up_worst_ev_bps: dec!(9),
        down_worst_ev_bps: dec!(11),
        min_worst_case_ev_bps: dec!(8),
    });

    assert_eq!(decision.side, Some(OutcomeSide::Down));
}

#[test]
fn uncertainty_band_grows_with_jitter() {
    let narrow = uncertainty_band_bps(20.0, 30_000, dec!(1.5));
    let wide = uncertainty_band_bps(120.0, 30_000, dec!(1.5));
    assert!(wide > narrow);
}

#[test]
fn honest_fees_reduce_ev() {
    let zero_fee = compute_worst_case_ev_bps(test_ev_inputs(dec!(0)));
    let paid_fee = compute_worst_case_ev_bps(test_ev_inputs(dec!(200)));
    assert!(paid_fee < zero_fee);
}

#[test]
fn robust_sizing_shrinks_when_risk_lambda_increases() {
    let low_risk = choose_size(&test_size_inputs(dec!(0.1)));
    let high_risk = choose_size(&test_size_inputs(dec!(2.0)));
    assert!(high_risk < low_risk);
}
```

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test both_positive_sides_choose_higher_worst_case_ev --lib`
Expected: FAIL because the pure decision helpers do not exist yet.

- [ ] **Step 3: Implement the pure decision helpers**

```rust
fn decide_entry_side(inputs: &DecisionInputs) -> EntrySideDecision {
    let up_ok = inputs.up_worst_ev_bps > inputs.min_worst_case_ev_bps;
    let down_ok = inputs.down_worst_ev_bps > inputs.min_worst_case_ev_bps;

    let side = match (up_ok, down_ok) {
        (true, false) => Some(OutcomeSide::Up),
        (false, true) => Some(OutcomeSide::Down),
        (true, true) if inputs.down_worst_ev_bps > inputs.up_worst_ev_bps => Some(OutcomeSide::Down),
        (true, true) => Some(OutcomeSide::Up),
        (false, false) => None,
    };

    EntrySideDecision { side }
}

fn uncertainty_band_bps(jitter_ms: f64, millis_to_end: u64, fee_bps: Decimal) -> Decimal {
    let jitter_component = Decimal::from_f64_retain(jitter_ms / 10.0).unwrap_or(Decimal::ZERO);
    let time_component = Decimal::from(millis_to_end.max(1) as i64).recip();
    jitter_component + time_component + fee_bps / dec!(100)
}
```

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run: `cargo test uncertainty_band_grows_with_jitter --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/strategies/eth_chainlink_taker.rs
git commit -m "feat: add eth taker decision core"
```

### Task 5: Add Entry, Exit, Invariant, Cooldown, Forced-Flat, And Recovery Logic

**Files:**
- Modify: `src/strategies/eth_chainlink_taker.rs`
- Test: `src/strategies/eth_chainlink_taker.rs`

- [ ] **Step 1: Write the failing behavior tests**

```rust
#[test]
fn one_position_invariant_rejects_second_entry() {
    let mut strategy = test_strategy();
    strategy.pending_entry_order = Some(ClientOrderId::from("O-001"));

    let result = strategy.try_enter(&entry_context());

    assert!(result.is_err());
}

#[test]
fn forced_flat_blocks_entry_when_chainlink_is_stale() {
    let mut strategy = ready_to_trade_strategy();
    strategy.active.last_chainlink_ts_ms = 1_000;

    let allowed = strategy.entries_allowed_at(5_000);
    assert!(!allowed);
}

#[test]
fn exit_triggers_when_exit_ev_beats_hold_ev_with_hysteresis() {
    let decision = should_exit(dec!(12), dec!(10), dec!(1));
    assert!(decision);
}

#[test]
fn cooldown_is_per_market() {
    let mut strategy = test_strategy();
    strategy.cooldowns.insert("A".to_string(), 120);

    assert!(strategy.market_in_cooldown("A", 100));
    assert!(!strategy.market_in_cooldown("B", 100));
}

#[test]
fn recovery_mode_blocks_new_entries_until_flat() {
    let mut strategy = test_strategy();
    strategy.recovery = true;
    assert!(!strategy.can_attempt_entry());
}
```

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test one_position_invariant_rejects_second_entry --lib`
Expected: FAIL because invariant enforcement, exit logic, cooldown, and recovery methods do not exist yet.

- [ ] **Step 3: Implement the behavior paths**

```rust
impl EthChainlinkTaker {
    fn can_attempt_entry(&self) -> bool {
        !self.recovery
            && !self.active.forced_flat
            && self.active.fees_ready_for_active
            && self.pending_entry_order.is_none()
            && self.open_position_id.is_none()
    }

    fn try_enter(&mut self, ctx: &EntryContext) -> Result<()> {
        anyhow::ensure!(self.can_attempt_entry(), "entry blocked by strategy invariants");
        let order = self.core.order_factory().limit(
            ctx.instrument_id,
            ctx.order_side,
            ctx.quantity,
            ctx.price,
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
            None,
        );
        self.pending_entry_order = Some(order.client_order_id());
        self.submit_order(order, None, Some(ClientId::from(self.config.client_id.as_str())))
    }
}
```

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run: `cargo test forced_flat_blocks_entry_when_chainlink_is_stale --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/strategies/eth_chainlink_taker.rs
git commit -m "feat: add eth taker order and invariant logic"
```

### Task 6: Add Integration Coverage And Final Verification

**Files:**
- Create: `tests/eth_chainlink_taker_runtime.rs`
- Modify: `tests/support/mod.rs`
- Test: `tests/eth_chainlink_taker_runtime.rs`

- [ ] **Step 1: Write the failing integration tests**

```rust
#[test]
fn same_session_client_order_id_fill_is_attributed_to_strategy() {
    let harness = EthTakerHarness::new();
    harness.activate_market("MKT-B");
    harness.publish_ready_reference();
    harness.publish_ready_book();

    let client_order_id = harness.trigger_entry();
    harness.emit_fill_for(client_order_id.clone());

    let positions = harness
        .cache()
        .positions_open(None, None, Some(&harness.strategy_id()), None, None);
    assert_eq!(positions.len(), 1);
}

#[test]
fn end_to_end_harness_subscribes_warms_and_submits_on_ev_positive_side() {
    let harness = EthTakerHarness::new();
    harness.start();
    harness.activate_market("MKT-A");
    harness.publish_fee("UP-TOKEN", dec!(1.75));
    harness.publish_ready_reference();
    harness.publish_ready_book();

    let orders = harness.submitted_orders();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].time_in_force(), TimeInForce::Fok);
}
```

- [ ] **Step 2: Run the targeted integration tests to verify they fail**

Run: `cargo test --test eth_chainlink_taker_runtime same_session_client_order_id_fill_is_attributed_to_strategy`
Expected: FAIL because the harness and integration hooks do not exist yet.

- [ ] **Step 3: Implement the harness and integration helpers**

```rust
pub struct EthTakerHarness {
    cache: Rc<RefCell<Cache>>,
    submitted: Rc<RefCell<Vec<OrderAny>>>,
    strategy_id: StrategyId,
}

impl EthTakerHarness {
    fn activate_market(&self, market_id: &str) {
        publish_any(
            runtime_selection_topic(&self.strategy_id).into(),
            &active_snapshot_for_test(market_id),
        );
    }

    fn publish_ready_reference(&self) {
        publish_any(
            "platform.reference.test".into(),
            &reference_snapshot_for_test(3_250.0),
        );
    }
}
```

- [ ] **Step 4: Run the focused and final verification commands**

Run: `cargo test --lib eth_chainlink_taker`
Expected: PASS

Run: `cargo test --test eth_chainlink_taker_runtime`
Expected: PASS

Run: `cargo fmt --check && cargo clippy --lib --tests -- -D warnings`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/strategies/eth_chainlink_taker.rs tests/eth_chainlink_taker_runtime.rs tests/support/mod.rs
git commit -m "feat: implement eth chainlink taker strategy"
```

## Spec Coverage Check

- Concrete strategy module and builder: Task 1
- Subscriptions, switch handling, fee gate, interval-open, warmup: Task 2
- Mid-stream contract check after items 1-7: Task 3
- Lead arbitration, uncertainty band, EV, side selection, sizing: Task 4
- Entry, FOK, one-position invariant, exit, forced-flat, cooldown, recovery: Task 5
- Same-session attribution and end-to-end harness: Task 6

No `#135` spec requirement is intentionally left without a task.

## Placeholder Scan

- No `TODO`, `TBD`, or “implement later” markers remain.
- All tasks name exact files.
- Every code-changing step includes concrete code or a concrete command target.

## Type Consistency Check

- Production kind string is always `eth_chainlink_taker`.
- Builder type is always `EthChainlinkTakerBuilder`.
- Runtime state container is always `ActiveMarketState`.
- Side-selection output uses `OutcomeSide`.
- The same strategy file owns both actor shell and pure helpers to avoid cross-file naming drift.
