# Issue #789 Economic Proof Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make PR #1492 enforce the approved finite economic-lifecycle proof boundary without claiming general NautilusTrader object fidelity.

**Architecture:** Keep the existing admission → NT event-store integrity → causal assembly → independent fold → terminal cross-check pipeline. Tighten only the admitted grammar, proof-relevant identity fields, canonical fill projection, and finite terminal projections defined in the approved design. Do not add another ledger, matcher, compatibility path, or timestamp-based classifier.

**Tech Stack:** Rust 2024, NautilusTrader pinned at `d81be0bcc7a473c45d2dc8a8885638336073a218`, `anyhow`, `rust_decimal`, Cargo Nextest.

## Global Constraints

- Source of truth: `docs/superpowers/specs/2026-07-21-issue-789-economic-lifecycle-proof-boundary-design.md`.
- Scope is issue #789 and PR #1492 only; do not add #788 behavior.
- Modify only `crates/backtesting-vertical-slice/src/execution_contract.rs`, `crates/backtesting-vertical-slice/src/runner.rs`, the existing #789 plan, this plan, and the approved design when wording must stay synchronized.
- No `.github/`, `.mergify.yml`, `scripts/`, dependency, Cargo manifest, or lockfile changes.
- No Bolt-owned ledger, second matcher, terminal-state reconstruction, timestamp classification, compatibility path, or #1447 reversal.
- Runtime values continue to come from the manifest, instrument, catalog, or captured identities; test literals remain test fixtures only.
- Tests verify behavior, not source structure.
- Every code task follows RED → minimal GREEN → focused regression before commit.

---

### Task 0: Align the Existing #789 Contract Before Rust Changes

**Files:**
- Modify: `docs/superpowers/plans/2026-07-20-issue-789-event-store-evidence.md`

**Interfaces:**
- Consumes: approved proof-boundary design.
- Produces: one unambiguous plan source that names the proof-relevant terminal projection and exact
  inclusion/exclusion registry.

- [x] **Step 1: Replace the superseded boundary wording**

Link `docs/superpowers/specs/2026-07-21-issue-789-economic-lifecycle-proof-boundary-design.md` as the
field-level source of truth. Replace unqualified "complete terminal projection" language with
"proof-relevant terminal projection". Amend the matrix rows for role denomination, non-`Clear` sides,
market acknowledgements, canonical fills, identity uniqueness, exactly-one-fill settlement, and finite
terminal order/position fields.

- [x] **Step 2: Verify the documentation alignment**

Run:

```bash
rg -n 'complete terminal projection|proof-relevant terminal projection|economic-lifecycle-proof-boundary' \
  docs/superpowers/plans/2026-07-20-issue-789-event-store-evidence.md
git diff --check
```

Expected: no unqualified superseded phrase remains; the approved design link and replacement phrase are
present; `git diff --check` exits zero.

- [x] **Step 3: Commit the alignment**

```bash
git add docs/superpowers/plans/2026-07-20-issue-789-event-store-evidence.md
git commit -m "docs(backtesting): align issue 789 proof boundary"
```

### Task 1: Close Admission and Lifecycle-Role Grammar

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`

**Interfaces:**
- Consumes: `ManifestVenueConfig`, `OrderBookDelta`, `ExecutionOrderTrace`, `SubmittedOrderTrace`.
- Produces: admission that rejects acknowledgement-enabled runs and non-`Clear` `NoOrderSide`; role classification that requires quote entry, base reduction, and exactly one settlement fill.

- [x] **Step 1: Add failing admission behavior tests**

Add runner tests that mutate an otherwise-valid fixture:

```rust
#[test]
fn issue_789_static_admission_rejects_market_acks_and_no_side_rows() {
    let mut venue = issue_789_venue("POLYMARKET", "pUSD", "L2_MBP", true, true);
    venue.use_market_order_acks = true;
    assert!(ensure_issue_789_venue_shape(&venue).is_err());

    let instrument = issue_789_admission_instrument();
    let mut delta = issue_789_admission_delta(
        instrument.id(),
        BookAction::Update,
        "0.421",
        "1.00",
    );
    delta.order.side = OrderSide::NoOrderSide;
    assert!(validate_issue_789_book_domain(&instrument, &[delta]).is_err());
}
```

- [x] **Step 2: Run the admission test and confirm RED**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(issue_789_static_admission_rejects_market_acks_and_no_side_rows)'
```

Expected: FAIL because the current admission accepts at least one mutation.

- [x] **Step 3: Implement exact admission grammar**

Extend `ensure_issue_789_venue_shape` with `!venue.use_market_order_acks`. In
`validate_issue_789_book_domain`, after the `Clear` branch, require:

```rust
ensure!(
    matches!(delta.order.side, OrderSide::Buy | OrderSide::Sell),
    "#789 non-Clear book delta {index} has no executable side"
);
```

- [x] **Step 4: Add failing role-denomination and settlement-cardinality tests**

In `execution_contract.rs`, construct coherent fixtures proving that all other invariants pass. For the
entry mutation use the existing fixture shape directly:

```rust
let ExecutionOrderCause::Submitted {
    submitted_order,
    quote_conversion,
    ..
} = &mut fixture.orders[0].cause else {
    panic!("fixture entry order changed")
};
submitted_order.quantity = Quantity::from("2.71");
submitted_order.quote_quantity = false;
*quote_conversion = None;
```

For the reduction mutation set its submitted quantity to `0.86`, set `quote_quantity = true`, and install
`quote_conversion(&fixture.orders[1].fills[0], Quantity::from("2.00"))`.

For the multiple-settlement mutation split `0.71` into fills `0.30` and `0.41` at `1.000`, with unique
event/trade IDs. Replace the final position effect with `Changed(quantity=0.41,
last_quantity=0.30, realized_pnl=0.19400000 USDC)` followed by
`Closed(quantity=0.00, last_quantity=0.41, realized_pnl=0.43180000 USDC)`. Replace the final account
transition with `1000000.02180000 USDC` followed by `1000000.43180000 USDC`. This keeps exposure, P&L,
cash, and per-fill cardinality coherent, isolating the settlement-cardinality defect.

Use separate tests named:

- `rejects_base_denominated_entry_role`
- `rejects_quote_denominated_reduction_role`
- `rejects_multiple_settlement_fills`

Each test must assert its typed role/cardinality error, not merely `is_err()`.

- [x] **Step 5: Run the three tests and confirm RED**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(rejects_base_denominated_entry_role) | test(rejects_quote_denominated_reduction_role) | test(rejects_multiple_settlement_fills)'
```

Expected: the current validator accepts the denomination mutations and multiple settlement fills.

- [x] **Step 6: Bind role to denomination and settlement cardinality**

In `validate_execution_contract`, bind `submitted_order` in the entry/reduction arms:

```rust
ExecutionOrderCause::Submitted { submitted_order, .. }
    if before.is_zero() && !exposure.is_zero() => {
        ensure!(submitted_order.quote_quantity, "#789 entry must be quote-denominated");
        // existing one-entry guard
    }
ExecutionOrderCause::Submitted { submitted_order, .. }
    if !before.is_zero() && /* existing reduction predicate */ => {
        ensure!(!submitted_order.quote_quantity, "#789 reduction must be base-denominated");
        // existing one-reduction/nonflat guards
    }
ExecutionOrderCause::Settlement { .. } => {
    ensure!(order.fills.len() == 1, "#789 settlement must contain exactly one fill");
    // existing residual/price/final-order guards
}
```

- [x] **Step 7: Run Task 1 focused tests and commit**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(issue_789_static_admission) | test(rejects_base_denominated_entry_role) | test(rejects_quote_denominated_reduction_role) | test(rejects_multiple_settlement_fills)'
```

Expected: all selected tests PASS.

Commit:

```bash
git add crates/backtesting-vertical-slice/src/runner.rs crates/backtesting-vertical-slice/src/execution_contract.rs
git commit -m "fix(backtesting): close issue 789 run grammar"
```

### Task 2: Bind Causal Identity and Position Effects

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`

**Interfaces:**
- Consumes: decoded `OrderFilled` and `PositionOpened/Changed/Closed` events plus strict store sequence.
- Produces: enriched `PositionEffectTrace`; one global identity partition for UUIDs, client-order IDs, venue-order IDs, trade IDs, position ID, and configured account.

- [x] **Step 1: Add failing position-ownership tests**

Extend the execution-contract fixture with independent mutations of:

- effect trader ID;
- effect strategy ID;
- opening order ID;
- entry side;
- signed quantity;
- currency; and
- settlement closing order ID.

Each mutation must preserve price, last quantity, position ID, account, and P&L so it fails only the new
causal ownership check. Name the table test `rejects_position_effect_causal_identity_drift`.

- [x] **Step 2: Run the position-ownership test and confirm RED**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(rejects_position_effect_causal_identity_drift)'
```

Expected: at least one mutation is accepted by the current reduced `PositionEffectTrace`.

- [x] **Step 3: Enrich and validate `PositionEffectTrace`**

Add exact fields:

```rust
pub struct PositionEffectTrace {
    pub kind: PositionEffectKind,
    pub trader_id: TraderId,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub position_id: PositionId,
    pub account_id: AccountId,
    pub opening_order_id: ClientOrderId,
    pub closing_order_id: Option<ClientOrderId>,
    pub entry: OrderSide,
    pub side: PositionSide,
    pub signed_quantity: f64,
    pub quantity: Quantity,
    pub last_quantity: Quantity,
    pub last_price: Price,
    pub currency: Currency,
    pub realized_pnl: Option<Money>,
}
```

Populate them directly while decoding all three position-event types. Bind every effect to the
immediately preceding fill's trader/strategy/instrument/account/last quantity/last price, keep
`opening_order_id` equal to the entry order, and require only `PositionClosed.closing_order_id` to equal
the settlement order. Require the raw NT `signed_quantity` to be finite, normalize it at the instrument
size precision, and compare that value to the independently folded exposure. This preserves the captured
claim without treating binary floating-point representation noise as economic drift.

- [x] **Step 4: Add failing identity-partition tests**

Add behavior tests for:

- reused UUID across different persisted event types;
- entry and reduction sharing a client-order ID;
- two lifecycle orders sharing a venue-order ID;
- duplicate trade ID; and
- position effects using more than one position ID.

Use a pure helper with explicit sets so the tests mutate evidence values rather than inspect source.

- [x] **Step 5: Implement the identity partition**

Retain the existing global UUID registry. Add a helper that consumes the assembled orders/effects and
requires exactly three distinct client-order IDs, exactly three distinct venue-order IDs, globally unique
trade IDs, exactly one position ID, and exactly one configured account ID. Treat the embedded and stored
`OrderInitialized` equality as the sole intentional repeated semantic initialization.

- [x] **Step 6: Run Task 2 focused tests and commit**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(/issue_789.*identity/) | test(rejects_position_effect_causal_identity_drift)'
```

Expected: all selected tests PASS.

Commit:

```bash
git add crates/backtesting-vertical-slice/src/execution_contract.rs crates/backtesting-vertical-slice/src/runner.rs
git commit -m "fix(backtesting): bind issue 789 causal identities"
```

### Task 3: Implement Finite Terminal Projections

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`

**Interfaces:**
- Consumes: approved field registry, sealed causal fills, independent fold, current `OrderAny`, `Position`, and `AccountAny` caches.
- Produces: `Issue789ProofFill`, expanded `OrderTerminalRecord`, and exact terminal order/position/account comparisons limited to proof-relevant fields.

- [x] **Step 1: Define the canonical fill projection and failing comparison tests**

Create a private projection derived from both stored and terminal fills:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct Issue789ProofFill {
    trader_id: TraderId,
    strategy_id: StrategyId,
    instrument_id: InstrumentId,
    client_order_id: ClientOrderId,
    venue_order_id: VenueOrderId,
    account_id: AccountId,
    trade_id: TradeId,
    order_side: OrderSide,
    order_type: OrderType,
    quantity: Quantity,
    price: Price,
    currency: Currency,
    liquidity_side: LiquiditySide,
    event_id: UUID4,
    reconciliation: bool,
    commission: Option<Money>,
}
```

Add tests proving timestamp, `info`, `causation_id`, and raw captured `position_id` differences do not
change this projection, while every listed field does.

- [x] **Step 2: Expand terminal-order capture and add RED mutations**

Extend `OrderTerminalRecord` with:

```rust
trader_id: TraderId,
strategy_id: StrategyId,
instrument_id: InstrumentId,
account_id: Option<AccountId>,
venue_order_id: Option<VenueOrderId>,
position_id: Option<PositionId>,
current_quote_quantity: bool,
trade_ids: Vec<TradeId>,
commissions: Vec<(Currency, Money)>,
```

Capture each field from the live `OrderAny`. Extend the existing terminal-order mutation test to change
each field independently, including a commission map whose key differs from `Money.currency`.

- [x] **Step 3: Implement exact terminal-order checks**

Bind identity and routing to initialization, configured account, fills, and the single lifecycle
position. Compare exact `Issue789ProofFill` vectors, trade IDs, and keyed commission totals derived from
the causal fills. Retain the full-fill equation and current quote flag (`false` after conversion).
Do not add checks for excluded timestamps, average price, previous status, cached non-fill history,
normal time-in-force/reduce-only, voided quantity, or overfill quantity.

- [x] **Step 4: Add RED terminal-position mutations**

Clone a valid terminal `Position` and independently mutate:

- trader, strategy, opening order, closing order, and entry side;
- trade-ID set;
- commission key and commission value;
- one adjustment;
- one missing, extra, or non-fill replay event; and
- one fill void.

Retain existing nonflat, wrong-instrument, extra-position, fill, and P&L controls. Every mutation must
assert the terminal-position boundary error.

- [x] **Step 5: Implement the terminal-position boundary**

Replace the flatness-only helper with a validator that consumes the expected configured account,
entry/reduction/settlement IDs, canonical fills, one position ID, independent P&L, and keyed commission
map. Require exact included fields, empty adjustment/void collections, and the pinned NT
`replay_events` collection to equal the exact ordered fill-only canonical mirror. Leave excluded
timestamps, duration, cached averages/returns, peak quantity, and aggregate buy/sell quantities unchecked.

- [x] **Step 6: Add run-envelope RED controls and implement them**

Extract a pure envelope helper over `(seq, payload_type)` and add tests for missing, duplicate, reversed,
non-boundary, and every admitted economic/control payload outside the unique interval. Require exactly one
`RunStarted` as first entry and one `RunEnded` as last entry.

- [x] **Step 7: Run Task 3 focused tests and commit**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(/issue_789.*terminal/) | test(/issue_789.*envelope/) | test(/issue_789.*proof_fill/)'
```

Expected: all selected tests PASS.

Commit:

```bash
git add crates/backtesting-vertical-slice/src/runner.rs
git commit -m "fix(backtesting): close issue 789 terminal projections"
```

Implementation evidence:

- Task 0: boundary alignment commits `e7f0cfa8a` and `4ae5e20aa`; targeted text checks and
  `git diff --check` passed.
- Task 1: commit `95ae3c14d`; admission, denomination-role, and exactly-one-settlement-fill controls
  passed.
- Task 2: commit `b7a682bde`; causal-position and identity-partition controls passed.
- Task 3: commit `7f8054549`; 14 terminal/envelope/canonical-fill controls passed. The real replay also
  passed after normalizing NT's finite signed `f64` at instrument size precision before comparing it to
  the independent decimal exposure fold.

### Task 4: Verify and Publish the Aligned Contract

**Files:**
- Modify: `docs/superpowers/plans/2026-07-21-issue-789-economic-proof-boundary-implementation.md`

**Interfaces:**
- Consumes: Tasks 1–3 and the approved design.
- Produces: synchronized documentation, exact-head evidence, one resolved internal adversarial review, and an updated PR #1492.

- [x] **Step 1: Mark implementation evidence in the plans**

Mark Tasks 0–3 complete only after their named behavior evidence passes. Do not broaden or narrow the
approved field registry while recording results.

- [x] **Step 2: Run focused #789 controls excluding the real replay**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(/issue_789/) & not test(issue_789_first_real_free_data_taker_pl)'
```

Expected: all selected tests PASS.

- [x] **Step 3: Run execution-contract controls**

Run:

```bash
cargo nextest run --locked --filter-expr 'test(execution_contract::tests)'
```

Expected: all selected tests PASS.

- [x] **Step 4: Run static and full BTE verification**

Run:

```bash
cargo fmt --manifest-path crates/backtesting-vertical-slice/Cargo.toml -- --check
cargo clippy --manifest-path crates/backtesting-vertical-slice/Cargo.toml --locked -- -D warnings
cargo nextest run --manifest-path crates/backtesting-vertical-slice/Cargo.toml --locked
git diff --check
```

Expected: formatting, strict Clippy, every BTE test, the real #789 replay, and diff hygiene PASS.

Exact local evidence before internal review:

- focused #789 controls excluding the replay: 50/50 passed;
- execution-contract controls: 55/55 passed;
- BTE formatting and strict Clippy: passed;
- full all-target BTE Nextest battery after resolving the internal-review findings: 1,404/1,404 passed,
  including the real #789 replay in 131.025 seconds; and
- `git diff --check`: passed.

- [x] **Step 5: Conduct one internal adversarial review against the approved boundary**

The review may block only on the design's stop-rule categories. Resolve every concrete finding, rerun the
affected focused control, then rerun Steps 2–4 if Rust changed.

The internal review found one bounded-account-waiver mismatch and stale design status. Commit
`b97a39e1d` tightened unrelated account initialization evidence to the strict prefix before the first
normal `SubmitOrder`; the authoritative registry now names that waiver. The follow-up review returned
`APPROVE` with no remaining boundary violation.

- [x] **Step 6: Commit documentation alignment**

```bash
git add docs/superpowers/specs/2026-07-21-issue-789-economic-lifecycle-proof-boundary-design.md \
  docs/superpowers/plans/2026-07-20-issue-789-event-store-evidence.md \
  docs/superpowers/plans/2026-07-21-issue-789-economic-proof-boundary-implementation.md
git commit -m "docs(backtesting): align issue 789 proof contract"
```

- [ ] **Step 7: Update PR #1492 without merging**

Fast-forward the local PR branch to the verified implementation branch, switch to the PR branch, and use
plain `git push`. Post exact-head evidence, request fresh review from the login resolving to node ID
`U_kgDOEZMFhA`, report the exact SHA, and detach without waiting for CI. Do not approve or merge.
