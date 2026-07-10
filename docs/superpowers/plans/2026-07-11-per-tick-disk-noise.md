# Per-Tick Disk-Write Noise Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the blocked-RV strategy-input evidence path emit only on semantic state changes while preserving every event-keyed append and proving the complete per-tick append class.

**Architecture:** Keep dedupe inside `BinaryOracleEdgeTaker` at the distinct blocked-snapshot call site. Derive a private equality key from semantic fields of the already-built blocked snapshot, clear it when the blocked-RV condition ends, and leave the submit-linked snapshot append untouched.

**Tech Stack:** Rust, NautilusTrader strategy callbacks, GitHub Actions Rust Probe, repository remote-first verification.

## Global Constraints

- Scope is GitHub issue #1354 and one PR related to #1275 and #1179.
- Enumerate quote, book, timer, and index-price reachability for every decision-evidence kind and every non-evidence appender.
- Do not change evidence schemas.
- Do not change capture, rotation, journald, uploaders, or recovery byte limits.
- Do not add writer-level dedupe.
- Event-keyed evidence completeness is untouchable.
- In-memory dedupe keys reset on restart; one duplicate per key per process lifetime is accepted.
- Delete this plan and the design document before the final implementation commit so specs do not outlive the code.

---

### Task 1: Prove the blocked-snapshot flood with a differential

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Consumes: `BinaryOracleEdgeTaker::try_submit_entry_order(now_ms)` and `RecordingSequencedDecisionEvidenceWriter::events()`.
- Produces: test `blocked_strategy_input_evidence_records_state_transitions_not_ticks`.

- [ ] **Step 1: Replace the current two-call observation with an explicit RED differential**

Rename `strategy_input_evidence_records_realized_volatility_not_ready_pricing_block` and retain its setup. Evaluate at `1_200..=1_204`, change the installed not-ready snapshot blocker from `QuorumNotReady` to `SourceStale`, evaluate once more, then assert:

```rust
let blocked_snapshots = evidence
    .events()
    .into_iter()
    .filter_map(|event| match event {
        RecordedDecisionEvidenceEvent::StrategyInput(snapshot)
            if snapshot.client_order_id.is_empty() =>
        {
            Some(snapshot)
        }
        _ => None,
    })
    .collect::<Vec<_>>();
assert_eq!(blocked_snapshots.len(), 2);
assert_eq!(
    blocked_snapshots[0].realized_volatility_blockers,
    vec!["quorum_not_ready".to_string()]
);
assert_eq!(
    blocked_snapshots[1].realized_volatility_blockers,
    vec!["source_stale".to_string()]
);
```

- [ ] **Step 2: Commit and publish the RED-only head**

Run:

```bash
git add src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs
git commit -m "test: expose blocked evidence tick flood"
just sandbox-safe-push
```

Expected: clean named branch whose remote head equals local `HEAD`.

- [ ] **Step 3: Run the smallest remote RED probe**

Before dispatch, state: changed file is `source_evidence.rs`; suspected failure is repeated blocked snapshots; mode is `nextest-lib-name`; target is the single differential; this is the smallest probe that demonstrates current behavior.

Run:

```bash
just rust-probe suggest
just rust-probe nextest-lib-name blocked_strategy_input_evidence_records_state_transitions_not_ticks
```

Expected: FAIL because current code emits one blocked `StrategyInput` record per evaluation rather than two records total.

---

### Task 2: Add source-local semantic state keying

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/entry_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Consumes: `BoltV3StrategyInputEvidenceSnapshot` produced only by `blocked_entry_strategy_input_evidence_snapshot_at`.
- Produces: `BlockedStrategyInputDedupeKey::from_snapshot(&BoltV3StrategyInputEvidenceSnapshot)` and `record_blocked_entry_strategy_input_snapshot_once(now_ms, decision)`.

- [ ] **Step 1: Define private semantic key types**

Add `BoltV3StrategyInputEvidenceSnapshot` to the evidence imports and define:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockedStrategyInputSourceStateKey {
    source_id: String,
    enabled: bool,
    counts_toward_quorum: bool,
    status: String,
    block_reason: Option<String>,
    last_rejected_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockedStrategyInputDedupeKey {
    configured_target_id: String,
    market_selection_outcome: String,
    market_id: Option<String>,
    gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    fast_venue_name: Option<String>,
    fast_venue_available: bool,
    reference_current_price_source_id: Option<String>,
    reference_current_price_available: bool,
    reference_current_price_failed_over: Option<bool>,
    fast_venue_incoherent: bool,
    realized_volatility_surface_id: String,
    realized_volatility_blockers: Vec<String>,
    realized_volatility_source_states: Vec<BlockedStrategyInputSourceStateKey>,
    realized_volatility_unknown_source_rejection_categories: Vec<String>,
}
```

Implement `from_snapshot` by cloning only those fields, mapping diagnostics to the source-state type, and collecting `realized_volatility_unknown_source_rejections.keys().cloned()`. Do not copy any value, timestamp, age, coverage, sample count, or rejection count.

- [ ] **Step 2: Add strategy state and the blocked-only recording helper**

Import `BlockedStrategyInputDedupeKey`, add:

```rust
last_recorded_blocked_strategy_input: Option<BlockedStrategyInputDedupeKey>,
```

initialize it to `None`, and add:

```rust
fn record_blocked_entry_strategy_input_snapshot_once(
    &mut self,
    now_ms: u64,
    decision: &EntrySubmissionDecision,
) -> Result<()> {
    let snapshot = self.blocked_entry_strategy_input_evidence_snapshot_at(now_ms, decision)?;
    let key = BlockedStrategyInputDedupeKey::from_snapshot(&snapshot);
    if self.last_recorded_blocked_strategy_input.as_ref() == Some(&key) {
        return Ok(());
    }
    self.context
        .decision_evidence()
        .record_strategy_input_snapshot(&snapshot)?;
    self.last_recorded_blocked_strategy_input = Some(key);
    Ok(())
}
```

- [ ] **Step 3: Route only the blocked call site through the helper and clear on unblock**

In `try_submit_entry_order`, compute the existing blocked-RV predicate once. When true, call `record_blocked_entry_strategy_input_snapshot_once`. Otherwise set `last_recorded_blocked_strategy_input = None`. Keep the submit-linked `entry_strategy_input_evidence_snapshot_at` plus direct `record_strategy_input_snapshot` chain unchanged.

- [ ] **Step 4: Run non-compile checks and inspect the diff**

Run:

```bash
just fmt-check
just source-fence-static
git diff --check
git diff -- src/strategies/binary_oracle_edge_taker/entry_decision.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs
```

Expected: all local gates exit zero; the submit-linked append remains a direct call.

- [ ] **Step 5: Commit, publish, and run the GREEN probe**

Run:

```bash
git add src/strategies/binary_oracle_edge_taker/entry_decision.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs
git commit -m "fix: key blocked evidence on state changes"
just sandbox-safe-push
just rust-probe nextest-lib-name blocked_strategy_input_evidence_records_state_transitions_not_ticks
```

Expected: PASS for the single differential at the pushed exact head.

---

### Task 3: Freeze the class census and publish exact-head proof

**Files:**
- Delete: `docs/superpowers/specs/2026-07-11-per-tick-disk-noise-design.md`
- Delete: `docs/superpowers/plans/2026-07-11-per-tick-disk-noise.md`
- Modify only if required by changed literals: `ci/bolt-v3-runtime-literals.toml`

**Interfaces:**
- Consumes: the complete `BoltV3DecisionEvidenceWriter` method list, all four handler families, all direct file-write APIs, and named evidence readers.
- Produces: issue #1354 implementation commit, one draft PR with the 19-row append-path table, and exact-head remote full-suite evidence.

- [ ] **Step 1: Re-run the exhaustive source census**

Run:

```bash
rg -n 'fn record_' src/bolt_v3_decision_evidence.rs
rg -n 'impl DataActor|fn on_quote|fn on_book_deltas|fn on_time_event|fn on_index_price' src/strategies src/bolt_v3_live_node.rs src/bolt_v3_live_node --glob '*.rs'
rg -n 'write_all|sync_data|OpenOptions|append_jsonl|File::create|BufWriter|csv::Writer|serde_json::to_writer' src/strategies src/bolt_v3_live_node.rs src/bolt_v3_live_node --glob '*.rs'
rg -n '^pub fn read_|^fn read_' src/bolt_v3_decision_evidence.rs
```

Expected: 18 record kinds, 19 rows after splitting `strategy_input_snapshot`, explicit coverage of quote/book/timer/index-price handlers, and no direct non-evidence file appender in the swept modules.

- [ ] **Step 2: Delete transient specifications and commit the final implementation tree**

Use `apply_patch` to delete both documents, then run:

```bash
git add docs/superpowers/specs/2026-07-11-per-tick-disk-noise-design.md docs/superpowers/plans/2026-07-11-per-tick-disk-noise.md
git commit -m "docs: retire implemented disk-noise spec"
```

Expected: neither transient document exists at final `HEAD`.

- [ ] **Step 3: Run all required local non-compile gates**

Run:

```bash
just fmt-check
just source-fence-static
python3 scripts/test_verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_runtime_literals.py
git diff --check origin/main...HEAD
```

Expected: every command exits zero.

- [ ] **Step 4: Publish the exact final head and open one draft PR**

Run:

```bash
just sandbox-safe-push
```

Create one draft PR for #1354. Its body must include the exhaustive 19-row bucket table, sweep commands, restart-reset acceptance, RED/GREEN probe evidence, remaining #1275 scope, and no closing keyword for #1275/#1179.

- [ ] **Step 5: Trigger the full unfiltered exact-head suite**

Mark the PR ready for review so the required pull-request full CI runs at the intended head, then run:

```bash
just verify-remote
```

Expected: full unfiltered exact-head suite and required gates green with no scope filters.

- [ ] **Step 6: Review, request the required owner, and report**

Perform internal adversarial review after local findings are resolved. Request review from the login resolving to node ID `U_kgDOEZMFhA` only after exact-head CI is green. Report the final head SHA and the complete bucket table.
