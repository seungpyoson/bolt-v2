# Loss Halt Market Exit Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the NT-first loss governor halts new risk, optionally trigger NT's own strategy market-exit path so working orders are canceled and open positions are closed through NautilusTrader primitives.

**Architecture:** Bolt continues to own only the halt decision, latching, config, and audit boundary. NautilusTrader remains the execution owner: `RiskEngine::set_trading_state(TradingState::Reducing)` blocks new risk while still allowing NT market-exit close orders, and `Trader::market_exit_strategy` sends `StrategyCommand::ExitMarket`; NT `Strategy::market_exit` then calls NT `cancel_all_orders` and `close_all_positions`. `TradingState::Halted` remains valid only when active market exit is disabled, because NT rejects order submission while halted. This slice does not modify sizing math, reservation accounting, maker quote-set semantics, or product calculators.

**Tech Stack:** Rust, NautilusTrader pinned rev `Cargo.toml`, TOML config, existing Bolt v3 submit admission and loss runtime feed, GitHub PR CI for final verification.

---

## Scope And Source Proof

Current PR #507 already implements:

- submit-admission loss halts for entry risk;
- NT-derived loss runtime feed;
- monotonic NT `RiskEngine::set_trading_state`;
- manual recovery decision helper;
- no active market-exit behavior.

Pinned NT source supports the missing active stop path:

- `crates/system/src/trader.rs:1026` exposes `Trader::market_exit_strategy`.
- `crates/trading/src/strategy/mod.rs:1345` implements `Strategy::market_exit`.
- `crates/trading/src/strategy/mod.rs:1399` calls `cancel_all_orders` per instrument.
- `crates/trading/src/strategy/mod.rs:1407` calls `close_all_positions` per instrument.
- `crates/trading/src/strategy/config.rs:76` exposes `manage_stop`; `:81` and `:87` expose market-exit interval and max attempts; `:91` and `:95` expose close order TIF and reduce-only controls.

The next slice must use those NT controls, not a Bolt-built order canceler or venue-specific flattener.

## File Structure

- Modify `src/bolt_v3_loss_halt_actions.rs`
  - Add config-facing market-exit action enum.
  - Add pure decision helpers mapping loss halt reasons to trading-state and market-exit actions.
  - Add an idempotent in-memory latch type so repeated stale/breach snapshots do not repeatedly dispatch market exit.
- Modify `src/bolt_v3_config.rs`
  - Add explicit TOML fields under `[risk.loss_governor]` for market-exit action selection.
- Modify `src/bolt_v3_validate.rs`
  - Require the new fields whenever the loss governor is enabled.
  - Reject market-exit enablement when no strategy is registered in the loaded config.
  - Reject `TradingState::Halted` for a loss reason whose market-exit action is `AllRegisteredStrategies`; active market exit must use `TradingState::Reducing`.
- Modify `src/bolt_v3_live_node.rs`
  - Parse the new action config into `LossGovernorHaltActionPolicy`.
  - Extend the live halt-action handler to call `Trader::market_exit_strategy` for each registered strategy only when the pure decision returns a market-exit action.
  - Keep existing `RiskEngine::set_trading_state` behavior before market exit so new risk is blocked immediately.
- Modify `tests/support/stub_runtime_strategy.rs`
  - Add optional process-local market-exit recording for the stub strategy's `on_market_exit` hook.
- Modify `tests/bolt_v3_submit_admission.rs`
  - Keep live-node level coverage proving breached loss snapshots still set NT risk state through the configured handler.
- Modify `tests/bolt_v3_strategy_registration.rs`
  - Add a direct NT strategy-control smoke test proving `Trader::market_exit_strategy` reaches a running strategy's market-exit hook.
- Modify `tests/config_parsing.rs` and `tests/fixtures/bolt_v3/root.toml`
  - Cover required TOML fields and round-trip parsing.
- Modify `specs/505-nt-loss-governor/tasks.md` and `specs/506-nt-position-sizer-submit/tasks.md`
  - Move active market-exit gap from remaining work to completed for this NT market-exit slice only, while preserving remaining gaps for maker quote sets, replace-submit, adapter allowance evidence, and non-binary calculators.

## Public Interface

Add this enum:

```rust
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LossGovernorMarketExitAction {
    None,
    AllRegisteredStrategies,
}
```

Extend the existing policy:

```rust
pub struct LossGovernorHaltActionPolicy {
    pub on_loss_breach_trading_state: LossGovernorTradingStateAction,
    pub on_untrusted_snapshot_trading_state: LossGovernorTradingStateAction,
    pub on_loss_breach_market_exit: LossGovernorMarketExitAction,
    pub on_untrusted_snapshot_market_exit: LossGovernorMarketExitAction,
    pub recovery_mode: LossGovernorRecoveryMode,
}
```

Add pure helper output:

```rust
pub struct LossGovernorHaltActionDecision {
    pub target_trading_state: Option<TradingState>,
    pub market_exit_action: LossGovernorMarketExitAction,
}
```

Add a pure helper:

```rust
pub fn next_loss_governor_halt_action(
    policy: &LossGovernorHaltActionPolicy,
    current_state: TradingState,
    decision: &LossAdmissionDecision,
) -> LossGovernorHaltActionDecision;
```

The helper preserves existing trading-state behavior and maps reasons as follows:

- `StaleLossSnapshot` uses `on_untrusted_snapshot_trading_state` and `on_untrusted_snapshot_market_exit`.
- `PerTradeLossLimit`, `DailyLossLimit`, `RollingLossLimit`, and `MaxDrawdownLimit` use `on_loss_breach_trading_state` and `on_loss_breach_market_exit`.
- Multiple reasons choose the strongest trading-state action and enable market exit if any configured reason maps to `AllRegisteredStrategies`.
- Accepted decisions produce `target_trading_state = None` and `market_exit_action = None`.

Validation must enforce this runtime compatibility rule:

- If a configured reason maps to `LossGovernorMarketExitAction::AllRegisteredStrategies`, that same reason must map to `LossGovernorTradingStateAction::Reducing`, not `Halted`.
- If a configured reason maps to `LossGovernorTradingStateAction::Halted`, that same reason must map to `LossGovernorMarketExitAction::None`.

## Tasks

### Task 1: Pure Market-Exit Action Decision

**Files:**
- Modify: `src/bolt_v3_loss_halt_actions.rs`

- [x] **Step 1: Write the failing tests**

Add these tests in the existing `mod tests`:

```rust
#[test]
fn loss_breach_maps_to_market_exit_action() {
    let policy = halt_policy(
        LossGovernorTradingStateAction::Reducing,
        LossGovernorTradingStateAction::None,
        LossGovernorMarketExitAction::AllRegisteredStrategies,
        LossGovernorMarketExitAction::None,
    );
    let decision = rejected(vec![LossHaltReason::DailyLossLimit]);

    let action = next_loss_governor_halt_action(&policy, TradingState::Active, &decision);

    assert_eq!(action.target_trading_state, Some(TradingState::Reducing));
    assert_eq!(
        action.market_exit_action,
        LossGovernorMarketExitAction::AllRegisteredStrategies
    );
}

#[test]
fn untrusted_snapshot_can_leave_market_exit_disabled() {
    let policy = halt_policy(
        LossGovernorTradingStateAction::Halted,
        LossGovernorTradingStateAction::Reducing,
        LossGovernorMarketExitAction::AllRegisteredStrategies,
        LossGovernorMarketExitAction::None,
    );
    let decision = rejected(vec![LossHaltReason::StaleLossSnapshot]);

    let action = next_loss_governor_halt_action(&policy, TradingState::Active, &decision);

    assert_eq!(action.target_trading_state, Some(TradingState::Reducing));
    assert_eq!(action.market_exit_action, LossGovernorMarketExitAction::None);
}

#[test]
fn accepted_loss_decision_does_not_market_exit() {
    let policy = halt_policy(
        LossGovernorTradingStateAction::Halted,
        LossGovernorTradingStateAction::Halted,
        LossGovernorMarketExitAction::AllRegisteredStrategies,
        LossGovernorMarketExitAction::AllRegisteredStrategies,
    );

    let action = next_loss_governor_halt_action(&policy, TradingState::Active, &accepted());

    assert_eq!(action.target_trading_state, None);
    assert_eq!(action.market_exit_action, LossGovernorMarketExitAction::None);
}
```

- [x] **Step 2: Run the narrow RED check**

Run:

```bash
cargo test --locked --lib market_exit
```

Expected: tests fail to compile because `LossGovernorMarketExitAction` and `next_loss_governor_halt_action` are not implemented.

- [x] **Step 3: Implement the pure helper**

Add `LossGovernorMarketExitAction`, `LossGovernorHaltActionDecision`, `next_loss_governor_halt_action`, and a small `strongest_market_exit_action` helper. Keep `next_loss_governor_trading_state` as a compatibility wrapper around the new helper.

- [x] **Step 4: Run the narrow GREEN check**

Run the same command. Expected: the three new tests pass.

### Task 2: Config Surface And Validation

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate.rs`
- Modify: `tests/config_parsing.rs`
- Modify: `tests/fixtures/bolt_v3/root.toml`

- [x] **Step 1: Write failing config tests**

Add parsing coverage for:

- enabled loss governor accepts `on_loss_breach_market_exit = "all_registered_strategies"`;
- enabled loss governor accepts `on_untrusted_snapshot_market_exit = "none"`;
- enabled loss governor fails validation if either field is missing;
- enabled market exit fails validation when `strategies = []`.
- enabled market exit fails validation if the matching trading-state action is `halted`.

- [x] **Step 2: Run the narrow RED check**

Run:

```bash
cargo test --locked --test config_parsing loss_governor
```

Expected: the new tests fail because the fields do not exist or are not validated.

- [x] **Step 3: Implement config and validation**

Add to `LossGovernorBlock`:

```rust
pub on_loss_breach_market_exit: Option<LossGovernorMarketExitAction>,
pub on_untrusted_snapshot_market_exit: Option<LossGovernorMarketExitAction>,
```

Update validation to require both when `enabled = true`. If either configured value is `AllRegisteredStrategies`, require at least one configured strategy in the loaded config and require the matching trading-state action to be `Reducing`. Reject these unsafe combinations:

```toml
on_loss_breach_trading_state = "halted"
on_loss_breach_market_exit = "all_registered_strategies"
```

```toml
on_untrusted_snapshot_trading_state = "halted"
on_untrusted_snapshot_market_exit = "all_registered_strategies"
```

- [x] **Step 4: Run the narrow GREEN check**

Run the same command. Expected: new parsing and validation tests pass.

### Task 3: Live Handler Uses NT Market Exit

**Files:**
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `tests/support/stub_runtime_strategy.rs`
- Modify: `tests/bolt_v3_submit_admission.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`

- [x] **Step 1: Write failing live handler test**

Add a test that builds a live node with:

- one registered stub strategy;
- enabled loss governor;
- `on_loss_breach_trading_state = "reducing"`;
- `on_loss_breach_market_exit = "all_registered_strategies"`;
- `on_untrusted_snapshot_market_exit = "none"`.

Publish a breached NT-derived loss snapshot through the runtime feed and add a direct NT primitive smoke test. Assert:

- `nt_risk_trading_state()` becomes `TradingState::Reducing`;
- a running stub strategy's `on_market_exit` hook records one call when invoked through `Trader::market_exit_strategy`, proving the NT primitive delivers `StrategyCommand::ExitMarket` through the strategy control endpoint;
- the Bolt-owned latch marks a strategy once and clears on recovery.

- [x] **Step 2: Run the narrow RED check**

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission live_node_loss_breach_snapshot_sets_nt_risk_trading_state_from_feed
cargo test --locked --test bolt_v3_strategy_registration nt_market_exit_strategy_reaches_running_stub_strategy_hook
```

Expected: tests fail before the handler/test support dispatch path is implemented. The original build-only hook assertion exposed an NT limitation instead: unstarted strategies reject market exit before `on_market_exit`, so the final test split verifies Bolt risk-state wiring, Bolt latch behavior, and NT primitive delivery separately.

- [x] **Step 3: Implement NT market-exit dispatch**

Update `loss_governor_halt_action_handler_from_node` to capture:

- `node.kernel().risk_engine().clone()`;
- `node.kernel().trader().clone()`;
- an `Rc<RefCell<LossGovernorMarketExitLatch>>`.

On each invoked snapshot:

1. Evaluate the loss decision.
2. Read current NT trading state.
3. If the decision is accepted and current NT trading state is `Active`, clear the market-exit latch and return.
4. Set NT trading state when the pure helper returns a stricter state. Validation ensures any market-exit-enabled reason targets `Reducing`, not `Halted`.
5. If market exit is `AllRegisteredStrategies`, fetch `trader.borrow().strategy_ids()`.
6. For each strategy id not already latched for the current halt action, call `Trader::market_exit_strategy(&trader, &strategy_id)`.
7. Log failures with strategy id and reason; do not downgrade NT trading state or reopen admission.

- [x] **Step 4: Run the narrow GREEN check**

Run the same command. Expected: market exit is dispatched once and trading state is still set.

### Task 4: Spec And Evidence Boundary

**Files:**
- Modify: `specs/505-nt-loss-governor/tasks.md`
- Modify: `specs/506-nt-position-sizer-submit/tasks.md`
- Modify: `docs/superpowers/plans/2026-06-03-loss-halt-market-exit-slice.md`

- [x] **Step 1: Update specs without overclaiming**

Mark active loss-halt market exit as completed only for NT `Trader::market_exit_strategy`. Preserve these remaining production gaps:

- operator clear-to-Active live command surface with caller-side evidence file/content-hash verification, operator authorization, command serialization, durable audit evidence, fresh reconciliation, and NT `RiskEngine::set_trading_state(Active)`;
- maker quote-set simultaneous-fill reservation;
- replace-submit reservation transition;
- adapter/venue allowance and collateral spendability evidence;
- dynamic market metadata;
- non-binary calculators;
- actual live reconnect/runtime tests beyond current unit-level coverage.

- [x] **Step 2: Run text and diff checks**

Run:

```bash
git diff --check
rg -n "cancel/flatten|market exit|production-grade by itself|Remaining For Production Grade" specs/505-nt-loss-governor specs/506-nt-position-sizer-submit docs/superpowers/plans/2026-06-03-loss-halt-market-exit-slice.md
```

Expected: no whitespace errors; wording clearly says this slice uses NT market exit and does not close unrelated sizing gaps.

### Task 5: CI And Review

**Files:**
- All files changed in Tasks 1-4.

- [ ] **Step 1: Commit the completed slice**

Run:

```bash
git diff --name-only -z | xargs -0 git add
git commit -m "Trigger NT market exit on configured loss halts"
```

- [ ] **Step 2: Push only after the slice is coherent**

Run:

```bash
git push origin codex/nt-loss-halt-market-exit-slice
```

- [ ] **Step 3: Verify through PR CI**

Run:

```bash
gh pr checks 507 --watch
```

Expected: required PR CI jobs pass. Local targeted cargo checks are only the TDD loop; PR CI remains the verification source of truth.

- [ ] **Step 4: Request external review**

Request Claude adversarial review of the exact PR head and every path in the current `git diff --name-only`.

Review focus:

```text
Adversarially review whether this slice correctly uses NautilusTrader's own market-exit controls for loss-governor halts without inventing a Bolt cancel/flatten path. Block on any overclaim that this makes the full positional sizer production-grade, any repeated market-exit dispatch risk, any mismatch between TradingState gating and market-exit dispatch, any config-default or hardcode violation, and any strategy/account boundary issue.
```

## Self-Review

- Spec coverage: This plan closes only the active loss-halt market-exit gap identified in `specs/506-nt-position-sizer-submit/tasks.md`. It does not claim to close maker quote-set reservations, replace-submit, allowance proof, dynamic market metadata, non-binary calculators, or live reconnect coverage.
- Placeholder scan: The plan has concrete files, functions, tests, commands, expected outcomes, and review focus.
- NT-first check: The active stop action is `Trader::market_exit_strategy`, which routes to NT `Strategy::market_exit`, `cancel_all_orders`, and `close_all_positions`.
- User preference check: Broad verification is PR CI. Local cargo commands are narrow TDD loop checks only and are not treated as the source of truth.
