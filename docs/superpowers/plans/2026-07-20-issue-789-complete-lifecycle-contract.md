# Issue #789 Complete Lifecycle Contract Implementation Plan

> **Execution note:** Implement this issue-owned plan in the isolated #789 worktree. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore trustworthy #789 backtesting evidence by validating entry, normal exit, settlement, commissions, realized P&L, and terminal cash across the complete position lifecycle.

**Architecture:** Replace the entry-only trace with an ordered list of position-affecting order traces. Submitted market orders carry the NautilusTrader order book replayed at their submission timestamp; the engine-generated expiration order has no submission timestamp or book and is accepted only when its position effect closes the exact instrument-precision remainder at the configured settlement price. Replay the same ordered typed fills through NautilusTrader `Position` and reconcile its commissions and realized P&L with terminal account cash.

**Tech Stack:** Rust, NautilusTrader Rust model/order-book/position primitives, `anyhow`, Cargo tests.

## Global Constraints

- Scope is GitHub issue #789 only; do not mix #788 or any later roadmap work.
- Do not revert PR #1447, alter official parser behavior, or add a compatibility path.
- Derive quantity precision from `Instrument::size_precision()`; do not hardcode venue decimal places.
- Tests verify runtime behavior and typed values, never source text.
- No `.github/`, `.mergify.yml`, or `scripts/` changes.

---

### Task 1: Ordered lifecycle contract

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`

**Interfaces:**
- Consumes: `InstrumentAny`, ordered `OrderFilled` events, per-order submission timestamps and executable books, settlement price, cash/P&L/commission observations, resolved-config provenance.
- Produces: owned `ExecutionOrderTrace` values, complete-lifecycle `ExecutionContractTrace<'a>`, and `validate_execution_contract` report counts.

- [ ] **Step 1: Write failing complete-lifecycle tests**

Add a fixture with an entry BUY, a smaller normal SELL at a second executable book, and an engine settlement SELL for the exact remainder. Add behavior tests that require acceptance of that lifecycle and rejection of a normal exit without a submission-time book.

- [ ] **Step 2: Run the focused contract tests and verify RED**

Run:

```bash
BOLT_ALLOW_LOCAL_RUST=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib execution_contract::tests -- --nocapture
```

Expected: the new complete-lifecycle acceptance test fails because the entry-only contract cannot represent or validate the normal exit.

- [ ] **Step 3: Implement the minimal ordered lifecycle API**

Introduce the following shape and replace the entry-only fields:

```rust
pub struct ExecutionOrderTrace {
    pub submission_timestamp: Option<UnixNanos>,
    pub executable_book: Option<OrderBook>,
    pub submitted_quantity: Quantity,
    pub quote_quantity: bool,
    pub effective_base_quantity: Quantity,
    pub fills: Vec<OrderFilled>,
}

pub struct ExecutionContractTrace<'a> {
    pub instrument: &'a InstrumentAny,
    pub orders: &'a [ExecutionOrderTrace],
    pub position_fills: &'a [OrderFilled],
    pub settlement_price: Price,
    // existing accounting and provenance fields remain
}
```

Flatten order fills and require exact equality with the typed position events. For every submitted order, validate quote/base conversion and deterministic `OrderBook::simulate_fills` results. Track the signed position effect with checked `Quantity` addition/subtraction from `Quantity::zero(instrument.size_precision())`; reject flips or reopening. Require exactly one opening effect, at least one normal closing effect for this regression, and one final unsubmitted settlement effect that closes the exact remainder at `settlement_price`.

- [ ] **Step 4: Reconcile complete-lifecycle economics**

Replay all ordered fills through `Position`, sum every fill commission in the position settlement currency, compare with the cached position commission, compare replayed and cached realized P&L, and require `terminal_cash - initial_cash == replayed_pnl`.

- [ ] **Step 5: Run focused contract tests and verify GREEN**

Run the Task 1 Step 2 command. Expected: all execution-contract unit tests pass with no failures.

### Task 2: #789 runner lifecycle adapter

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`

**Interfaces:**
- Consumes: the selected NT `Position`, all captured `OrderTerminalRecord` values, PMXT projected deltas, and manifest settlement/config/account data.
- Produces: ordered `ExecutionOrderTrace` values for the complete lifecycle contract.

- [ ] **Step 1: Keep the focused #789 scenario as the integration RED**

Run:

```bash
BOLT_ALLOW_LOCAL_RUST=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib runner::tests::issue_789_first_real_free_data_taker_pl -- --exact --nocapture
```

Expected: fail with `requires exactly one pre-settlement entry order, got 2`.

- [ ] **Step 2: Build orders from position identity and effect**

Select the single #789 position, then walk `position.events` in their recorded order. Resolve each unique `client_order_id` to exactly one captured terminal record whose fills share that position ID and instrument. Do not classify orders by a broad pre-settlement timestamp filter.

- [ ] **Step 3: Replay a book for every normal order**

For every position-affecting order with `submission_timestamp: Some(ts)`, call `replay_executable_book_at_submission(instrument_id, projection.order_book_deltas, ts)` and attach the result. For the engine expiration order, require `submission_timestamp: None` and attach no book; the shared contract must still prove its closing position effect, exact remaining quantity, price, and accounting.

- [ ] **Step 4: Run the focused #789 scenario and verify GREEN**

Run the Task 2 Step 1 command. Expected: the production-strategy replay succeeds with entry, normal exit, and residual settlement all validated.

### Task 3: Negative controls and publication evidence

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs` only if a behavior failure exposes a missing adapter assertion.

**Interfaces:**
- Consumes: the complete lifecycle validator from Task 1 and #789 adapter from Task 2.
- Produces: fail-closed tests for every approved requirement and exact-head local evidence.

- [ ] **Step 1: Add one failing behavior test per invariant**

Cover wrong order/position identity, an added opening effect, normal fill/book price or quantity drift, missing normal-order book, position flip, wrong settlement side, wrong settlement price, under/over settlement quantity, non-instrument precision, commission mismatch, correlated P&L/cash drift, and configuration hash drift. Each test must assert the intended semantic error.

- [ ] **Step 2: Verify each new negative test fails before its guard exists, then passes after the minimal guard**

Use the focused execution-contract command from Task 1 for every red/green cycle.

- [ ] **Step 3: Run formatting and focused verification**

Run:

```bash
cargo fmt --check
BOLT_ALLOW_LOCAL_RUST=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib execution_contract::tests -- --nocapture
BOLT_ALLOW_LOCAL_RUST=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib runner::tests::issue_789_first_real_free_data_taker_pl -- --exact --nocapture
```

Expected: formatting succeeds and both focused test commands report zero failures.

- [ ] **Step 4: Run the repository-defined Backtesting Vertical Slice battery**

Resolve the exact battery from the current `justfile`/workflow source of truth, run it locally where permitted, and record any independently reproduced pre-existing failure without treating it as a #789 pass.

- [ ] **Step 5: Self-review the complete diff adversarially**

Check every user requirement against code and tests, inspect `git diff --check`, confirm no #788 or unrelated files, and resolve every substantive finding before commit.

- [ ] **Step 6: Commit, push, and open the issue-owned PR**

Use a plain `git push`, open one PR scoped to #789, request fresh review from the login resolving node ID `U_kgDOEZMFhA`, do not merge or approve, and report the exact head SHA without waiting on CI.
