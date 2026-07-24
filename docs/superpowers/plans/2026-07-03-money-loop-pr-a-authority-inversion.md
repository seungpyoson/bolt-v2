# Money Loop PR-A Authority Inversion Implementation Plan

> **Superseded plan (2026-07-24).** Do not execute this plan. Use
> `specs/1354-current-evidence-thin-nt-boundary/plan.md`. The selected boundary
> deletes Bolt venue/order reconciliation and keeps NautilusTrader as the sole
> lifecycle authority.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Polymarket venue REST observations the continuous runtime authority for money state after causal reconciliation, and latch a whole-node halt on any venue delta unexplainable by recorded orders, fills, terminal events, or already-booked settlements.

**Architecture:** Add a focused venue-truth module that normalizes CLOB balance/open-order and Data API position reads. Add a causal reconciler that derives explanations from the already-captured NT order-event stream instead of a parallel durable store. Promote only explainable venue snapshots into capital admission, and route unexplainable deltas to the existing submit-admission kill-switch path.

**Tech Stack:** Rust 1.96.0, Nautilus Trader Rust Polymarket adapter at the pinned git checkout, TOML config, existing `just` verification recipes, remote-first Rust CI.

## Global Constraints

- Part of #1179; every PR body must say `Part of #1179` and must not use closing keywords.
- Runtime values come from TOML config; no hardcoded IDs, quantities, timeouts, thresholds, tolerance bands, or cadences.
- No time-tolerance windows and no numeric-equality divergence gate.
- No alternate money path or secret source.
- The causal ledger derives from the captured NT order-event stream; do not create a parallel durable bookkeeping store.
- Strategies produce intent only; do not add strategy submit mechanics or strategy-local money gates.
- Whole-node halt is the approved unexplainable-delta scope.
- Manual operator transfers while running are unexplainable venue deltas and must halt.
- PR-A excludes governance mode, exit clamp, settlement booking, and Lane 5 shutdown drain.
- Tests must be written before production code for each changed behavior.
- Local compile-heavy Rust verification is not default; use explicit `BOLT_ALLOW_LOCAL_RUST=1` only for targeted fast gates requested by this lane.

---

### Task 1: Venue Truth Snapshot Model

**Files:**
- Create: `src/bolt_v3_polymarket_venue_truth.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `PolymarketVenueTruthSnapshot`, `PolymarketVenueTruthOpenOrder`, `extract_polymarket_token_id(instrument_id: &InstrumentId) -> Option<String>`, and conversion helpers consumed by the reconciler and live poller.

- [ ] **Step 1: Write the failing tests**

Add focused tests for these cases:

```text
extract_polymarket_token_id("condition-with-dash-token123.POLYMARKET") returns "token123"
snapshot conversion preserves collateral balance and allowance
snapshot conversion indexes open orders by venue order id
snapshot conversion indexes positions by token id
```

- [ ] **Step 2: Run test to verify it fails**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked bolt_v3_polymarket_venue_truth --lib`

Expected: FAIL because `bolt_v3_polymarket_venue_truth` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

Create the module with normalized snapshot structs, token-id extraction, and conversion helpers using existing `Money`, `AccountId`, `InstrumentId`, `VenueOrderId`, and `Decimal` types.

- [ ] **Step 4: Run test to verify it passes**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked bolt_v3_polymarket_venue_truth --lib`

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_polymarket_venue_truth.rs src/lib.rs
git commit -m "feat: add Polymarket venue truth snapshots"
```

### Task 2: Event-Derived Causal Reconciler

**Files:**
- Modify: `src/bolt_v3_polymarket_venue_truth.rs`
- Test: module tests in `src/bolt_v3_polymarket_venue_truth.rs`

**Interfaces:**
- Consumes: previous accepted `PolymarketVenueTruthSnapshot`, current `PolymarketVenueTruthSnapshot`, and normalized order events derived from the captured NT order-event stream.
- Produces: `PolymarketVenueTruthReconciliation` with `accepted` and `unexplainable_delta` outcomes.

- [ ] **Step 1: Write the failing tests**

Add tests for these cases:

```text
new venue open order is explainable by an Accepted event carrying the same venue_order_id and client_order_id
position increase is explainable by a Filled event for a mapped accepted order
open quantity reduction is explainable by a Filled event for a mapped accepted order
manual collateral deposit without an order/fill/settlement cause is unexplainable
settlement-shaped position removal without booked settlement evidence is unexplainable
```

- [ ] **Step 2: Run test to verify it fails**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked polymarket_venue_truth_reconciler --lib`

Expected: FAIL because the reconciliation API does not exist yet.

- [ ] **Step 3: Write minimal implementation**

Implement an in-memory reconciliation pass that is derived only from the captured order-event inputs supplied to it. It may build an ephemeral projection while reconciling one snapshot, but it must not write a durable causal store.

- [ ] **Step 4: Run test to verify it passes**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked polymarket_venue_truth_reconciler --lib`

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_polymarket_venue_truth.rs
git commit -m "feat: reconcile Polymarket venue truth causally"
```

### Task 3: Runtime Feed Promotion

**Files:**
- Modify: `src/bolt_v3_capital_admission_runtime_feed.rs`
- Test: `tests/bolt_v3_capital_admission_runtime_feed.rs`

**Interfaces:**
- Consumes: `PolymarketVenueTruthSnapshot` and the normalized event-derived causal reconciliation inputs.
- Produces: runtime feed ingestion that promotes only accepted venue snapshots into `BoltV3SubmitCapitalAdmissionNtComponents`.

- [ ] **Step 1: Write the failing tests**

Add tests for these behaviors:

```text
explainable venue truth replaces NT account/cache collateral authority
explainable venue truth supplies yes/no positions keyed by token id
NT account state after accepted venue truth remains advisory and cannot increase spendability
unexplainable venue truth is not promoted into capital admission
```

- [ ] **Step 2: Run test to verify it fails**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked bolt_v3_capital_admission_runtime_feed --test bolt_v3_capital_admission_runtime_feed`

Expected: FAIL because venue-truth ingestion API is missing.

- [ ] **Step 3: Write minimal implementation**

Add venue-truth ingestion to the runtime feed. Derive causal event inputs from the order events already observed by the feed/capture path, promote accepted snapshots, and leave NT account/cache events advisory after accepted venue truth.

- [ ] **Step 4: Run test to verify it passes**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked bolt_v3_capital_admission_runtime_feed --test bolt_v3_capital_admission_runtime_feed`

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_capital_admission_runtime_feed.rs tests/bolt_v3_capital_admission_runtime_feed.rs
git commit -m "feat: promote causally reconciled venue truth"
```

### Task 4: Whole-Node Halt And Alarm

**Files:**
- Modify: `src/bolt_v3_kill_switch.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_capital_admission_runtime_feed.rs`
- Test: `tests/bolt_v3_capital_admission_runtime_feed.rs`

**Interfaces:**
- Produces: `KillSwitchTriggerKind::VenueTruthDivergence`.
- Produces: a runtime-feed path that replaces submit admission kill-switch state with a non-armed state when a venue delta is unexplainable.

- [ ] **Step 1: Write the failing tests**

Add tests for these behaviors:

```text
unexplained venue open order latches submit admission into non-armed state
unexplained manual collateral transfer latches submit admission into non-armed state
unbooked settlement-shaped position removal latches submit admission into non-armed state
repeated already-accepted explainable snapshots do not repeatedly halt
```

- [ ] **Step 2: Run test to verify it fails**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked venue_truth_divergence --test bolt_v3_capital_admission_runtime_feed`

Expected: FAIL because the kill-switch trigger and latch wiring are missing.

- [ ] **Step 3: Write minimal implementation**

Add the trigger kind and feed-to-submit-admission latch path using existing kill-switch state replacement APIs. Include structured alarm fields for account id, field, prior venue value, current venue value, and missing causal explanation.

- [ ] **Step 4: Run test to verify it passes**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked venue_truth_divergence --test bolt_v3_capital_admission_runtime_feed`

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_kill_switch.rs src/bolt_v3_submit_admission.rs src/bolt_v3_capital_admission_runtime_feed.rs tests/bolt_v3_capital_admission_runtime_feed.rs
git commit -m "feat: halt on unexplained venue truth"
```

### Task 5: Live Poller And Config

**Files:**
- Modify: `src/bolt_v3_providers/polymarket.rs`
- Modify: `src/bolt_v3_providers/polymarket/venue_account_state_source.rs`
- Modify: `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `config/root.toml`

**Interfaces:**
- Consumes: `PolymarketExecutionConfig::venue_truth_poll_interval_ms`.
- Produces: a live-node-owned poller that periodically reads venue truth, runs causal reconciliation, promotes accepted snapshots, and halts on read failure or unexplainable deltas according to existing fail-closed live-node behavior.

- [ ] **Step 1: Write the failing tests**

Add tests for these behaviors:

```text
missing venue-truth poll interval fails config validation when Polymarket capital admission is enforced
zero venue-truth poll interval fails config validation
configured positive interval is accepted
```

- [ ] **Step 2: Run test to verify it fails**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked venue_truth_poll_interval --test config_parsing`

Expected: FAIL because the config field and validation are missing.

- [ ] **Step 3: Write minimal implementation**

Add the TOML field, build the REST clients from existing Polymarket execution config, reuse the filtered Data API read-window logic, and call runtime feed ingestion on every poll.

- [ ] **Step 4: Run test to verify it passes**

Run: `BOLT_ALLOW_LOCAL_RUST=1 cargo test --locked venue_truth_poll_interval --test config_parsing`

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_providers/polymarket.rs src/bolt_v3_providers/polymarket/venue_account_state_source.rs src/bolt_v3_providers/polymarket/collateral_accounting_source.rs src/bolt_v3_live_node.rs config/root.toml
git commit -m "feat: poll Polymarket venue truth continuously"
```

### Task 6: Verification, PR Notes, And Draft PR

**Files:**
- All changed files.

- [ ] **Step 1: Run static checks**

Run `just fmt-check`, `just deny`, `just ci-lint-workflow`, and `just source-fence-static` unless a check is unavailable; record exact output status.

- [ ] **Step 2: Run targeted Rust tests**

Run the targeted PR-A tests with `BOLT_ALLOW_LOCAL_RUST=1`. Record exact commands.

- [ ] **Step 3: Prepare PR body**

The PR body must contain:

```text
Part of #1179
```

It must avoid closing keywords. If captured order-event completeness is a limitation because Lane 5 has not landed, state that the causal reconciler depends on the captured NT order-event stream and that shutdown-drain completeness is tracked by Lane 5.

Also state that PR-D must prove the hold-to-resolution replay books settlement payout and does not trigger a false venue-truth halt.

- [ ] **Step 4: Push and open draft PR**

Push `fix/1179-money-loop`, open a draft PR against `main`, and request the required reviewer only after local findings are resolved and exact-head CI is green.
