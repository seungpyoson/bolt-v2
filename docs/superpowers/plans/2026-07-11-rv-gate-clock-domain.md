# RV Gate Clock-Domain Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RV admission and evidence stable across independently clocked venues by using a pricing-owned receive-time domain for every trigger.

**Architecture:** The RV engine attaches an accepted-observation receive watermark to each snapshot. Shared fair-value pricing compares that watermark with a typed receive-domain evaluation stamp, and edge-taker/maker boundaries provide the stamp for every production trigger. Rejected event timestamps no longer advance the RV surface clock or causal watermark.

**Tech Stack:** Rust 2024, Nautilus Trader event timestamps, nextest, repository Rust Probe/remote CI, JSONL decision evidence.

## Global Constraints

- Work only under reopened issue #1354 on `fix/1354-rv-gate-clock-domain`.
- Do not add tolerance windows, rate caps, sampling, byte-limit changes, venue policy, or symbol policy.
- Preserve all action, lifecycle, settlement, and recovery evidence.
- Keep dedupe-key and `position_id=None` findings report-only.
- Capture every differential RED before production code changes.
- Run the full unfiltered local suite and exact-head remote full CI; do not use manual CI dispatch.

---

### Task 1: Pin the receive-domain classifier contract

**Files:**
- Modify: `src/bolt_v3_fair_value_pricing.rs`
- Test: `tests/bolt_v3_fair_value_pricing.rs`

**Interfaces:**
- Consumes: `RealizedVolSnapshot` and `LocalReceiveMs`.
- Produces: `classify_rv_gate(snapshot, evaluation_receive_ms, max_source_age_ms)` with same-domain freshness semantics.

- [ ] Add a differential where event `as_of_ms` leads and lags an unrelated venue trigger while snapshot receive time and evaluation receive time remain ordered; expect both evaluations to be `Accepted`.
- [ ] Add stale and missing-receive cases; expect `RejectedStale` and `MissingEvaluationEventTime` respectively.
- [ ] Run the smallest Rust Probe suggested for these tests and retain the exact RED output.
- [ ] Do not modify production code until the failures are confirmed to be semantic assertion failures.

### Task 2: Pin accepted-only RV watermarks

**Files:**
- Modify: `src/bolt_v3_realized_volatility.rs`
- Modify: `src/bolt_v3_realized_volatility_runtime.rs`
- Test: existing inline tests in both modules and `tests/bolt_v3_realized_volatility_source_fence.rs`

**Interfaces:**
- Produces: a typed receive watermark on `RealizedVolSnapshot` derived only from accepted contributing samples.
- Preserves: rejection diagnostics without allowing a rejected event timestamp to advance the surface clock.

- [ ] Add a differential that accepts one observation, then delivers a rejected far-future observation and asserts both event `as_of_ms` and receive watermark remain on the accepted observation.
- [ ] Add a multi-source differential proving the ready snapshot watermark follows an accepted quorum-contributing source.
- [ ] Run the smallest Rust Probe suggested for the tests and retain exact RED output.

### Task 3: Pin all trigger classes and the evidence flood

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exit_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Produces: receive-domain evaluation context for book, signal, selection, reference, and local/unknown trigger constructors.
- Consumes: the shared pricing-layer classifier from Task 1.

- [ ] Add the six-tick alternating-book differential; pre-change output must report six retained records where one is expected.
- [ ] Add a mixed book/signal/selection differential around one unchanged ready snapshot; pre-change output must show selection-induced oscillation.
- [ ] Add a structurally missing receive-context case and retain `MissingEvaluationEventTime → Hold`.
- [ ] Run the smallest Rust Probe suggested for the tests and retain all exact RED outputs.

### Task 4: Pin entry and maker blast radius

**Files:**
- Modify: `src/bolt_v3_taker_pricing.rs`
- Modify: `src/bolt_v3_maker_runtime_quote.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `tests/bolt_v3_taker_pricing.rs`
- Test: `tests/bolt_v3_maker_runtime_quote.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/pricing.rs`

**Interfaces:**
- Entry consumes the triggering book receive stamp.
- Maker consumes the selected reference quote receive stamp.

- [ ] Add entry and maker differentials where event clocks disagree but receive freshness is valid; pre-change behavior must block as future-dated/RV-not-ready.
- [ ] Run the smallest Rust Probe suggested for the tests and retain exact RED output.

### Task 5: Implement the minimal ownership change

**Files:**
- Modify all production files named in Tasks 1–4.

- [ ] Add the snapshot receive watermark and accepted-only surface-clock derivation.
- [ ] Change shared pricing requests/classifier to `LocalReceiveMs` and remove production dependence on cross-venue event ordering.
- [ ] Thread receive context through every production entry, exit, and maker trigger.
- [ ] Keep a truly absent receive stamp fail-closed and preserve historical evidence enums.
- [ ] Run every named differential through the repository-approved Rust path and confirm GREEN.
- [ ] Run formatting and source-fence gates; correct only issue-owned failures.

### Task 6: Pin recovery behavior at the byte boundary

**Files:**
- Test: `src/strategies/binary_oracle_edge_taker/tests/adverse_path_harness.rs`
- Test: `tests/bolt_v3_decision_evidence.rs`

**Interfaces:**
- Consumes: existing `recovery_evidence_max_bytes` and settlement/open-position bootstrap.
- Produces: proof that a valid open-position recovery stream succeeds at or below the bound and fails into existing blind recovery above it.

- [ ] Add a below-bound open-position restart fixture and assert managed/recovered exposure.
- [ ] Add the same fixture with one extra byte beyond the bound and assert `SettlementEvidenceRecoveryFailed` blind recovery.
- [ ] Run both tests and retain exact GREEN output.

### Task 7: Capacity evidence and completion gates

**Files:**
- Modify: PR body only for arithmetic and evidence; no repository runtime change.

- [ ] Re-run the archived replay using the final semantics and report original/retained records and bytes.
- [ ] Report open-position bytes/hour, bytes per genuine phase transition, projected bytes at the planned restart, and whether #1275/#763 becomes pre-soak.
- [ ] Run the full unfiltered local suite and report run/pass/skip counts.
- [ ] Run clippy with warnings denied for library and binary through the allowed repository path.
- [ ] Commit and publish with `just sandbox-safe-push`, open a draft PR without closing keywords, and mark ready only when local findings are resolved.
- [ ] Obtain exact-head remote full CI, external panel review, required reviewer approval, and queue through `just merge-queue`.

