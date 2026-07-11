# RV Gate Clock-Domain Ownership Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RV admission and evidence stable across independently clocked venues by using a pricing-owned receive-time domain for every trigger.

**Architecture:** The RV engine attaches an accepted-observation receive watermark to each snapshot. Shared fair-value pricing compares that watermark with a typed receive-domain evaluation stamp, and edge-taker/maker boundaries provide the stamp for every production trigger. Rejected event timestamps no longer advance the RV surface clock or causal watermark.

**Tech Stack:** Rust 2024, Nautilus Trader event timestamps, nextest, repository-approved local verification and automatic PR CI, JSONL decision evidence.

## Global Constraints

- Work only under reopened issue #1354 on `fix/1354-rv-gate-clock-domain`.
- Do not add tolerance windows, rate caps, sampling, byte-limit changes, venue policy, or symbol policy.
- Preserve all action, lifecycle, settlement, and recovery evidence.
- Keep dedupe-key and `position_id=None` findings report-only.
- Capture every differential RED before production code changes.
- Run the full unfiltered local suite and exact-head remote full CI; do not use manual CI dispatch.
- Do not use Rust Probe for this work because the ruling forbids manual CI dispatches.

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
- [ ] Run the smallest repository-approved local test command that exercises these tests and retain the exact RED output.
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
- [ ] Add a cutoff differential with an accepted observation after the final selected grid point; assert that eligible-but-unused input does not advance the watermark.
- [ ] Add a contributing-set differential with ascending event times and a larger receive timestamp on an earlier selected observation; assert that the watermark is the maximum receive timestamp over every observation used by the base, coarse, and subsampled computation.
- [ ] Run the smallest repository-approved local test command that exercises these tests and retain exact RED output.

### Task 3: Pin all trigger classes and the evidence flood

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exit_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Produces: receive-domain evaluation context for production book, signal, and selection trigger constructors; test-only reference and structurally absent unknown/other constructors retain explicit test coverage.
- Consumes: the shared pricing-layer classifier from Task 1.

- [ ] Add the six-tick alternating-book differential; pre-change output must report six retained records where one is expected.
- [ ] Add a mixed book/signal/selection differential around one unchanged ready snapshot; pre-change output must show selection-induced oscillation.
- [ ] Add a structurally missing receive-context case and retain `MissingEvaluationEventTime → Hold`.
- [ ] Run the smallest repository-approved local test command that exercises these tests and retain all exact RED outputs.

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
- Maker consumes an explicit evaluation receive stamp supplied by its caller; the selected reference quote receive stamp describes only that input.

- [ ] Make production fair-value and taker pricing requests require `LocalReceiveMs`; keep optionality only at the diagnostic classifier boundary for the structurally absent fail-closed case.
- [ ] Add independent near-stale entry mutation differentials for initial uncertainty, sized fee adjustment, resized fee adjustment, log/skip evidence, and submit-linked strategy-input evidence. In each case the trigger stamp remains valid while strategy `now_ms` is stale.
- [ ] Add a submit-path differential proving that evidence records the admitted evaluation snapshot and cannot abort submission by re-gating at later strategy wall time.
- [ ] Add a route-level maker differential with `quote_receive < snapshot_receive <= explicit_evaluation_receive`; expect pricing to remain available. Do not add maker production wiring.
- [ ] Add entry and maker differentials where event clocks disagree but receive freshness is valid; pre-change behavior must block as future-dated/RV-not-ready.
- [ ] Run the smallest repository-approved local test command that exercises these tests and retain exact RED output.

### Task 5: Implement the minimal ownership change

**Files:**
- Modify all production files named in Tasks 1–4.

- [ ] Add the snapshot receive watermark and accepted-only surface-clock derivation.
- [ ] Change production shared pricing requests to required `LocalReceiveMs`, keep only the diagnostic classifier's structurally absent input optional, and remove production dependence on cross-venue event ordering.
- [ ] Thread receive context through every production entry and exit trigger and through the existing maker pricing route boundary; do not add maker production wiring.
- [ ] Preserve exact contributing sample identity through configured RV grids and derive each ready source's watermark as the maximum receive timestamp over that set.
- [ ] Keep a truly absent receive stamp fail-closed and preserve historical evidence enums.
- [ ] Run every named differential through the repository-approved Rust path and confirm GREEN.
- [ ] Run formatting and source-fence gates; correct only issue-owned failures.

### Task 6: Pin recovery behavior at the byte boundary

**Files:**
- Test: `src/strategies/binary_oracle_edge_taker/tests/adverse_path_harness.rs`

**Interfaces:**
- Consumes: existing `recovery_evidence_max_bytes` and settlement/open-position bootstrap.
- Produces: proof that a valid open-position recovery stream succeeds at or below the bound and fails into existing blind recovery above it.

- [ ] Add a below-bound open-position restart fixture and assert managed/recovered exposure.
- [ ] Add the same fixture with one extra byte beyond the bound and assert `SettlementEvidenceRecoveryFailed` blind recovery.
- [ ] Run both tests and retain exact GREEN output.

### Task 7: Capacity evidence and completion gates

**Files:**
- Modify: PR body only for arithmetic and evidence; no repository runtime change.

- [ ] Re-run the archived replay from `s3://bolt-deploy-artifacts/archives/bolt-v2/evidence/order-intents-v0111-session-20260711T074342Z.jsonl.gz` using the final semantics; report the replay command, exact code head, original/retained records, and bytes.
- [ ] Report open-position bytes/hour, bytes per genuine phase transition, projected bytes at the planned restart, and whether #1275/#763 becomes pre-soak.
- [ ] Run the full unfiltered local suite and report run/pass/skip counts.
- [ ] Run clippy with warnings denied for library and binary through the allowed repository path.
- [ ] Commit and publish with `just sandbox-safe-push`, open a draft PR without closing keywords, and mark ready only when local findings are resolved.
- [ ] Obtain exact-head remote full CI, external panel review, required reviewer approval, and queue through `just merge-queue`.
