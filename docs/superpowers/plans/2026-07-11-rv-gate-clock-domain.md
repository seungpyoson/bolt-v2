# RV Gate Clock-Domain Ownership Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RV admission and evidence stable across independently clocked venues by using a pricing-owned receive-time domain for every trigger.

**Architecture:** The RV engine attaches an accepted-observation receive watermark to each snapshot. Shared fair-value pricing compares that watermark with a typed receive-domain evaluation stamp, and edge-taker/maker boundaries provide the stamp for every production trigger. Rejected event timestamps no longer advance the RV surface clock or causal watermark.

**Tech Stack:** Rust 2024, Nautilus Trader event timestamps, nextest, cheap local static gates and automatic PR CI, JSONL decision evidence.

## Global Constraints

- Work only under reopened issue #1354 on `fix/1354-rv-gate-clock-domain`.
- Do not add tolerance windows, rate caps, sampling, byte-limit changes, venue policy, or symbol policy.
- Preserve all action, lifecycle, settlement, and recovery evidence.
- Keep dedupe-key and `position_id=None` findings report-only.
- Treat the root-fix tests and recovery-boundary tests delivered at `2b83f512a` as landed scope: verify them GREEN at the final head and cite their existing RED transcripts; do not revert production code to manufacture new RED output.
- Capture RED only for the review-amendment tests not present at `2b83f512a`. Commit those tests without amendment production changes, publish the draft branch, and use the automatically triggered draft-PR Rust checks as the RED vehicle. The implementer detaches after publishing; the reviewer/orchestrator confirms the RED result before implementation resumes.
- Use cheap local gates plus automatic exact-head remote Rust verification. Do not run the full local Rust suite, local clippy, Rust Probe, or manually dispatched CI.

---

### Task 0: Record the reference-price ruling and gate the separate follow-up

The read-only review gate is complete at exact head `f572791db`: independently
clocked signal/reference event timestamps are directly compared on the live taker
entry path. This is a confirmed, separable defect. Do not change reference-price
behavior in #1354.

**Files to inspect:**
- `src/strategies/binary_oracle_edge_taker/mod.rs`
- `src/bolt_v3_taker_pricing.rs`
- `src/bolt_v3_reference_price.rs`
- Relevant strategy TOML freshness configuration and reference/failover tests

**Interfaces:**
- Producer: reference and pricing-spot observations with event and receive timestamps.
- Evaluation owner: entry trigger receive timestamp and the existing `reference_gate_event_ms` path.
- Consumer: `reference_current_price_stale_at` and its entry block reasons/evidence.

- [x] Trace the combined values and their physical clocks: Binance/OKX signal event time and Chainlink/PolyResearch reference event time are not normalized upstream.
- [x] Confirm the live consequence: receive-fresh inputs can fail closed as `SpotPriceMissing` or `ReferenceCurrentPriceStale`; alternating reason sets can churn entry-skip evidence.
- [x] Confirm the threshold: all live configurations use 2,000 ms and a 2,001 ms event-clock difference is discriminating; current frequency is unmeasured.
- [x] Rule B: confirmed separate issue/branch/design/PR, sequenced after #1354 so it can consume the evaluation receive stamp established here.
- [ ] Obtain explicit user approval before filing the separate issue. Do not use closing keywords or bundle it into the four already approved PRs.
- [ ] In that separate design, census primary, failover, taker, maker, selector/source-health live-window, forced-flat freshness, retained receive timestamps, and the `SpotPriceMissing` mislabel.

---

### Task 0A: Correct the Binance SBE receive timestamp before #1354 implementation

This is a blocking #1354 prerequisite and must be completed before Task 1. BTC
production uses Binance Spot SBE, and pinned NT `9e71b2b` currently assigns the
Binance event time to both `ts_event` and `ts_init`. Do not treat that `ts_init` as
receive-domain evidence.

**Upstream files to inspect and change in the pinned NT dependency source:**
- `crates/adapters/binance/src/spot/websocket/streams/parse.rs`
- The Binance Spot SBE WebSocket message handler that invokes `parse_bbo_event`
- Its adapter tests covering BBO parsing and handler timestamp capture

**Bolt files to verify:**
- `Cargo.toml`
- `Cargo.lock`
- `src/strategies/binary_oracle_edge_taker/mod.rs`
- `config/root.toml`
- `config/strategies/binary_oracle_btc.toml`

**Interfaces:**
- Producer: Binance SBE handler captures one local receive timestamp at message handling.
- Parser: `parse_bbo_event` receives both provider event time and local receive time and assigns them to `QuoteTick.ts_event` and `QuoteTick.ts_init` respectively.
- Consumer: Bolt's signal observation and exit trigger use corrected `QuoteTick.ts_init` as `LocalReceiveMs` without restamping.

- [ ] Add an NT adapter differential with deliberately unequal event and receive timestamps; on pinned `9e71b2b`, record the RED showing `ts_init == ts_event` instead of the supplied receive time.
- [ ] Change the Binance SBE handler/parser ownership boundary so local receive time is captured once by the handler and passed explicitly into BBO parsing.
- [ ] Verify the NT differential GREEN: `ts_event` preserves Binance event time and `ts_init` preserves the supplied local receive time.
- [ ] Update Bolt's pinned NT revision through the repository's existing dependency update path; do not patch the Cargo checkout or introduce a Bolt-side venue branch.
- [ ] Add or strengthen a Bolt differential where `ts_event != ts_init` and an RV watermark is receive-fresh only under `ts_init`; prove signal-trigger classification and evidence follow `ts_init`.
- [ ] Verify BTC configuration reaches the corrected Binance SBE path and record the exact NT revision as evidence.
- [ ] Keep #1354 blocked until automatic exact-head remote Rust verification covers both the adapter-domain differential and the Bolt consumer differential.

---

### Task 1: Pin the receive-domain classifier contract

**Files:**
- Modify: `src/bolt_v3_fair_value_pricing.rs`
- Test: `tests/bolt_v3_fair_value_pricing.rs`

**Interfaces:**
- Consumes: `RealizedVolSnapshot` and `LocalReceiveMs`.
- Produces: `classify_rv_gate(snapshot, evaluation_receive_ms, max_source_age_ms)` with same-domain freshness semantics.

- [x] Landed at `2b83f512a`: event `as_of_ms` leads and lags an unrelated venue trigger while snapshot receive time and evaluation receive time remain ordered; both evaluations are `Accepted`.
- [x] Landed at `2b83f512a`: stale and missing-receive cases preserve `RejectedStale` and `MissingEvaluationEventTime` respectively.
- [ ] Verify the landed classifier tests remain GREEN at the final head and cite the existing `2b83f512a` RED transcript; do not recreate RED.

### Task 2: Pin accepted-only RV watermarks

**Files:**
- Modify: `src/bolt_v3_realized_volatility.rs`
- Modify: `src/bolt_v3_realized_volatility_runtime.rs`
- Test: existing inline tests in both modules and `tests/bolt_v3_realized_volatility_source_fence.rs`

**Interfaces:**
- Produces: a typed receive watermark on `RealizedVolSnapshot` derived only from accepted contributing samples.
- Preserves: rejection diagnostics without allowing a rejected event timestamp to advance the surface clock.

- [x] Landed at `2b83f512a`: a rejected far-future observation advances neither event `as_of_ms` nor the accepted receive watermark.
- [x] Landed at `2b83f512a`: the ready snapshot watermark follows accepted quorum-contributing sources.
- [ ] Add a cutoff differential with an accepted observation after the final selected grid point; assert that eligible-but-unused input does not advance the watermark.
- [ ] Add a contributing-set differential with ascending event times and a larger receive timestamp on an earlier selected observation; assert that the watermark is the maximum receive timestamp over every observation used by the base, coarse, and subsampled computation.
- [ ] Add trimmed-mean and quantile multi-source differentials proving the watermark covers every ready quorum source that affects readiness or dispersion, including numerically unselected sources.

### Task 3: Pin all trigger classes and the evidence flood

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exit_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Produces: receive-domain evaluation context for production book, signal, and selection trigger constructors; test-only reference and structurally absent unknown/other constructors retain explicit test coverage.
- Consumes: the shared pricing-layer classifier from Task 1.

- [x] Landed at `2b83f512a`: the six-tick alternating-book differential retains one record after the original six-record RED.
- [x] Landed at `2b83f512a`: the mixed book/signal/selection differential retains one record after the original four-record RED.
- [x] Landed at `2b83f512a`: structurally missing receive context retains `MissingEvaluationEventTime → Hold`.
- [ ] Add an amendment differential proving a signal trigger without NT `ts_init` remains fail-closed and never substitutes strategy wall time.
- [ ] Verify the landed six-tick, mixed-trigger, and missing-context tests remain GREEN at the final head and cite their existing RED transcripts; do not recreate RED.

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
- [ ] Add a typed entry RV evaluation receipt to `EntryEvaluation` and `EntrySubmissionDecision`. Capture the gate result and exact admitted snapshot/evidence identity once; make logs, skip evidence, and submit-linked evidence consume the receipt without querying current RV state.
- [ ] Add independent near-stale entry mutation differentials for initial uncertainty, sized fee adjustment, resized fee adjustment, log/skip evidence, and submit-linked strategy-input evidence. In each case the trigger stamp remains valid while strategy `now_ms` is stale.
- [ ] Add a submit-path differential proving that evidence records the admitted evaluation snapshot and cannot abort submission by re-gating at later strategy wall time.
- [ ] Add a state-replacement differential: replace the latest RV snapshot after evaluation and prove logs, skip evidence, and submit evidence retain the receipt's original gate result and snapshot identity.
- [ ] Add a route-level maker differential with `quote_receive < snapshot_receive <= explicit_evaluation_receive`; expect pricing to remain available. Do not add maker production wiring.
- [ ] Add entry and maker differentials where event clocks disagree but receive freshness is valid; pre-change behavior must block as future-dated/RV-not-ready.

### Task 5: Implement the minimal ownership change

**Files:**
- Modify all production files named in Tasks 1–4.

- [ ] Commit all review-amendment tests from Tasks 2–4 without amendment production changes, publish that draft SHA, and detach. Resume only after the reviewer/orchestrator supplies the automatically triggered CI RED result and confirms the failures match the intended old behavior.
- [ ] Add the snapshot receive watermark and accepted-only surface-clock derivation.
- [ ] Change production shared pricing requests to required `LocalReceiveMs`, keep only the diagnostic classifier's structurally absent input optional, and remove production dependence on cross-venue event ordering.
- [ ] Thread receive context through every production entry and exit trigger and through the existing maker pricing route boundary; do not add maker production wiring.
- [ ] Delete the signal strategy-clock fallback: production signal triggers require NT `ts_init`, genuinely local triggers use a distinct typed handler-entry constructor, and structurally missing signal context stays fail-closed.
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

- [x] Landed at `2b83f512a`: boundary tests prove exactly 1 MiB recovers managed exposure and 1 MiB + 1 enters `SettlementEvidenceRecoveryFailed` blind recovery.
- [ ] Verify both landed boundary tests remain GREEN at the final head.

### Task 7: Capacity evidence and completion gates

**Files:**
- Modify: PR body only for arithmetic and evidence; no repository runtime change.

- [ ] Re-run the archived replay from `s3://bolt-deploy-artifacts/archives/bolt-v2/evidence/order-intents-v0111-session-20260711T074342Z.jsonl.gz` using the final semantics; report the replay command, exact code head, original/retained records, and bytes.
- [ ] Report open-position bytes/hour, bytes per genuine phase transition, projected bytes at the planned restart, and whether #1275/#763 becomes pre-soak.
- [ ] Run cheap local formatting, deny, workflow-lint, and source-fence-static gates.
- [ ] Publish the exact head and report the automatically triggered remote Rust test/clippy results when supplied by the reviewer/orchestrator; do not wait on or manually dispatch CI.
- [ ] Commit and publish with `just sandbox-safe-push`, open a draft PR without closing keywords, and mark ready only when local findings are resolved.
- [ ] Detach after publishing the draft head. The user/reviewer owns ready-state exact-head CI, external review, required approval, and merge queue.
