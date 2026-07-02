# Money Loop PR-A Authority Inversion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Polymarket venue REST truth the continuous runtime authority for money state and latch a whole-node submit-admission halt on any structural divergence from NT advisory state.

**Architecture:** Add a focused venue-truth module that normalizes CLOB balance/open-order and Data API position reads. Feed those snapshots into capital admission as authoritative state, keep NT account/cache events advisory after the first venue snapshot, and wire divergence into the existing kill-switch path used by submit admission. Live-node wiring owns the poller lifecycle and reads cadence from TOML.

**Tech Stack:** Rust 1.96.0, Nautilus Trader Rust Polymarket adapter at the pinned git checkout, TOML config, existing `just` verification recipes, remote-first Rust CI.

## Global Constraints

- Runtime values come from TOML config; no hardcoded IDs, quantities, timeouts, thresholds, or cadences.
- No alternate money path or secret source.
- Strategies produce intent only; do not add strategy submit mechanics or strategy-local money gates.
- Whole-node halt is the approved divergence scope.
- PR-A excludes governance mode, exit clamp, and settlement booking.
- Tests must be written before production code for each changed behavior.
- Local compile-heavy Rust verification is not default; use explicit `BOLT_ALLOW_LOCAL_RUST=1` only for targeted fast gates requested by this lane.

---

### Task 1: Venue Truth Snapshot Model

**Files:**
- Create: `src/bolt_v3_polymarket_venue_truth.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `PolymarketVenueTruthSnapshot`, `PolymarketVenueTruthOpenOrder`, `extract_polymarket_token_id(instrument_id: &InstrumentId) -> Option<String>`, and conversion helpers consumed by the runtime feed and live poller.

- [ ] **Step 1: Write failing tests**

Add tests for extracting token ids from Polymarket binary instrument ids and converting venue balance/allowance/positions into one snapshot.

- [ ] **Step 2: Verify red**

Run a targeted Rust test for the new module. Expected failure: unresolved module/types/functions.

- [ ] **Step 3: Implement minimal snapshot model**

Create the module with normalized snapshot structs, token-id extraction, and conversion helpers using existing `Money`, `AccountId`, `InstrumentId`, and decimal types already used by capital admission.

- [ ] **Step 4: Verify green**

Run the same targeted Rust test. Expected result: tests pass.

### Task 2: Runtime Feed Authority Inversion

**Files:**
- Modify: `src/bolt_v3_capital_admission_runtime_feed.rs`
- Test: existing unit test module in the same file or the repo's existing integration-test location if that file already routes feed tests elsewhere.

**Interfaces:**
- Consumes: `PolymarketVenueTruthSnapshot`.
- Produces: `CapitalAdmissionRuntimeFeed::on_polymarket_venue_truth_snapshot(...)` or equivalent, returning whether state was accepted or divergence was detected.

- [ ] **Step 1: Write failing tests**

Cover these behaviors:

```text
venue truth replaces NT account/cache collateral authority
venue truth supplies yes/no position quantities keyed by token id
NT account state after venue truth remains advisory and cannot increase spendability
```

- [ ] **Step 2: Verify red**

Run the focused runtime-feed test. Expected failure: venue-truth ingestion API missing.

- [ ] **Step 3: Implement minimal authority inversion**

Add venue-truth ingestion to update capital admission components from venue collateral, allowance, positions, and open orders. Preserve existing NT event capture as advisory evidence.

- [ ] **Step 4: Verify green**

Run the focused runtime-feed test. Expected result: tests pass.

### Task 3: Whole-Node Divergence Halt

**Files:**
- Modify: `src/bolt_v3_kill_switch.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_capital_admission_runtime_feed.rs`

**Interfaces:**
- Produces: `KillSwitchTriggerKind::VenueTruthDivergence`.
- Produces: a runtime-feed path that can replace submit admission kill-switch state with a non-armed state when divergence is structural.

- [ ] **Step 1: Write failing tests**

Cover these behaviors:

```text
venue/NT collateral conflict latches submit admission into non-armed state
venue/NT position conflict latches submit admission into non-armed state
repeated matching snapshots do not repeatedly halt
```

- [ ] **Step 2: Verify red**

Run the focused divergence tests. Expected failure: kill-switch kind and latch wiring missing.

- [ ] **Step 3: Implement minimal halt wiring**

Add the trigger kind and feed-to-submit-admission latch path using existing kill-switch state replacement APIs.

- [ ] **Step 4: Verify green**

Run the focused divergence tests. Expected result: tests pass.

### Task 4: Live Poller And Config

**Files:**
- Modify: `src/bolt_v3_providers/polymarket.rs`
- Modify: `src/bolt_v3_providers/polymarket/venue_account_state_source.rs`
- Modify: `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `config/root.toml`

**Interfaces:**
- Consumes: `PolymarketExecutionConfig::venue_truth_poll_interval_ms`.
- Produces: a live-node-owned poller that periodically reads venue truth, feeds capital admission, and halts on read failure or divergence according to existing fail-closed live-node behavior.

- [ ] **Step 1: Write failing tests**

Cover these behaviors:

```text
missing venue-truth poll interval fails config validation when Polymarket capital admission is enforced
zero venue-truth poll interval fails config validation
configured positive interval is accepted
```

- [ ] **Step 2: Verify red**

Run the focused config/live-node tests. Expected failure: config field and poller wiring missing.

- [ ] **Step 3: Implement minimal poller wiring**

Add the TOML field, build the REST clients from existing Polymarket execution config, reuse the filtered Data API read-window logic, and call runtime feed ingestion on every poll.

- [ ] **Step 4: Verify green**

Run the focused config/live-node tests. Expected result: tests pass.

### Task 5: Verification, Commit, PR

**Files:**
- All changed files.

- [ ] **Step 1: Run static checks**

Run `just fmt-check`, `just deny`, `just ci-lint-workflow`, and `just source-fence-static` unless a check is unavailable; record exact output status.

- [ ] **Step 2: Run targeted Rust tests**

Run only the targeted tests needed for PR-A with `BOLT_ALLOW_LOCAL_RUST=1`. Record exact commands.

- [ ] **Step 3: Commit**

Commit with conventional commits. Keep docs and code reviewable.

- [ ] **Step 4: Push and open draft PR**

Push `fix/money-loop`, open a draft PR against `main`, and request the required reviewer only after local findings are resolved and exact-head CI is green.
