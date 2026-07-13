# RV Gate Clock-Domain Ownership Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RV admission and evidence stable across independently clocked venues by using a pricing-owned receive-time domain for every trigger.

**Architecture:** The RV engine attaches an accepted-observation receive watermark to each snapshot. Shared fair-value pricing compares that watermark with a typed receive-domain evaluation stamp, and edge-taker/maker boundaries provide the stamp for every production trigger. Rejected event timestamps no longer advance the RV surface clock or causal watermark.

**Tech Stack:** Rust 2024, Nautilus Trader event timestamps, nextest, cheap local static gates and automatic PR CI, JSONL decision evidence.

## Global Constraints

- Tasks 1–7 work only under reopened issue #1354. The Binance adapter correction
  and governed Bolt pin prerequisite landed separately through #1367 and are part of
  the authoritative `main` base for this implementation.
- Do not add tolerance windows, rate caps, sampling, byte-limit changes, venue policy, or symbol policy.
- Preserve all action, lifecycle, settlement, and recovery evidence.
- Keep dedupe-key and `position_id=None` findings report-only.
- Treat the root-fix tests and recovery-boundary tests at `2b83f512a` only as
  historical, provisional evidence from the superseded branch, not as scope landed
  on `main`. Their fresh-port equivalents require authoritative GREEN proof from the
  final exact head through remote verification. Cite the historical RED transcripts;
  do not revert production code to manufacture new RED output.
- Capture RED only for review-amendment tests not present in the historical
  `2b83f512a` checkpoint. The owner
  explicitly approves one exceptional scoped local break-glass command for those new
  Bolt RED tests:
  `BOLT_ALLOW_LOCAL_RUST=1 cargo nextest run --locked rv_clock_domain_amendment_`.
  Every new amendment test must use that name prefix. Retain the exact command and
  RED output in the PR report. This exception does not authorize the full local
  suite, local clippy, Rust Probe, or any CI dispatch. Fork RED/GREEN evidence uses
  the fork PR's own CI. Bolt GREEN and final proof use ready-state exact-head full CI
  only after production changes are complete and local findings are resolved.
- Use cheap local gates plus reviewer-operated exact-head remote Rust verification
  for GREEN. Automatic draft checks and draft-time `just verify-remote` do not run
  nextest and are not Rust evidence.
- Treat `2b83f512a` only as immutable historical RED/GREEN evidence from a
  superseded branch. It is not an ancestor of the authoritative base or this fresh
  head and is not merge proof. The final implementation is rebuilt from authoritative
  `main`; its proof must come from the fresh final head and exact-head remote
  verification, while preserving #1367's parser proof, pin census, and Binance
  same-event-millisecond regression unchanged.
- Keep PR-body verification statements head-neutral. Never embed a mutable current
  PR head SHA; merge proof always means the then-current exact PR head.

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
- [ ] Obtain explicit user approval before filing the separate issue. Do not use closing keywords or bundle it into this PR.
- [ ] In that separate design, census primary, failover, taker, maker, selector/source-health live-window, forced-flat freshness, retained receive timestamps, and the `SpotPriceMissing` mislabel.

---

### Task 0A: Binance Spot SBE timestamp contract — landed prerequisite

The independently reviewed Nautilus Trader correction is already landed in the
governed Bolt dependency through #1367. The historical pre-fix base was
`9e71b2b1305a66945ba07f0aba2d1eb63208263d`; the governed corrected revision is
`afc014a55b51463641cc19c68bffe25cdac6588a`.

- [x] `BinanceSpotDataClient::handle_ws_message` captures one local
  `clock.get_time_ns()` value per decoded SBE message.
- [x] Trades, BBO, depth snapshot, and depth diff preserve provider time as
  `ts_event` and use the supplied local value as `ts_init` for every emitted datum,
  every inner delta, and each aggregate wrapper.
- [x] The fork range and source-breaking parser signatures received independent
  review and exact-head CI before the Bolt pin landed.

---

### Task 0B: Governed Bolt NT pin slice — landed through #1367

PR #1367 landed the corrected NT revision and is authoritative for every pin,
registry, parser-proof, workflow, and source-fence surface. This PR must not modify
or duplicate those owners. It consumes their contract and adds only the RV ingest,
pricing, trigger, and evidence behavior that requires #1354's receive-domain types.

- [x] Both manifests, both lockfiles, runtime contract, naming ledger, verifier
  constant/fixture, and transitive NT packages resolve to the one governed revision.
- [x] The canonical pin census, Binance registry rows, exact parser/handler symbols,
  dependency-level unequal-stamp proof, and archive execution proof are enforced.
- [x] Checked-in BTC configuration routes Binance Spot SBE quotes through the
  corrected parser path.
- [x] This implementation starts from `main` containing #1367; all #1367-owned
  files remain byte-identical to that base.

---

### Task 1: Pin the receive-domain classifier contract

**Files:**
- Modify: `src/bolt_v3_fair_value_pricing.rs`
- Test: `tests/bolt_v3_fair_value_pricing.rs`

**Interfaces:**
- Consumes: `RealizedVolSnapshot` and `LocalReceiveMs`.
- Produces: `classify_rv_gate(snapshot, evaluation_receive_ms, max_source_age_ms)` with same-domain freshness semantics.

- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: event `as_of_ms` leads and lags an unrelated venue trigger while snapshot receive time and evaluation receive time remain ordered; both evaluations are `Accepted`.
- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: stale and missing-receive cases preserve `RejectedStale` and `MissingEvaluationEventTime` respectively.
- [x] Production fair-value/taker requests and RV-consuming helpers require `LocalReceiveMs`; optionality remains only at the lower diagnostic classifier boundary.
- [ ] Verify the fresh-port classifier tests GREEN through exact-head remote verification and cite the historical `2b83f512a` RED transcript; do not recreate RED or treat the checkpoint as authoritative proof.

### Task 2: Pin accepted-only RV watermarks

**Files:**
- Modify: `src/bolt_v3_realized_volatility.rs`
- Modify: `src/bolt_v3_realized_volatility_runtime.rs`
- Test: existing inline tests in both modules and `tests/bolt_v3_realized_volatility_source_fence.rs`

**Interfaces:**
- Produces: a typed receive watermark on `RealizedVolSnapshot` derived only from accepted contributing samples.
- Preserves: rejection diagnostics without allowing a rejected event timestamp to advance the surface clock.

- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: a rejected far-future observation advances neither event `as_of_ms` nor the accepted receive watermark.
- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: the ready snapshot watermark follows accepted quorum-contributing sources.
- [x] Route an unequal-stamped Binance-shaped quote through RV observation; assert surface `as_of_ms` follows `ts_event` and `latest_accepted_receive_ms` follows `ts_init`.
- [x] Add a cutoff differential with an accepted observation after the final selected grid point; assert that eligible-but-unused input does not advance the watermark.
- [x] Add a contributing-set differential with ascending event times and a larger receive timestamp on an earlier selected observation; assert that the watermark is the maximum receive timestamp over every observation used by the base, coarse, and subsampled computation.
- [x] Add trimmed-mean and quantile multi-source differentials proving the watermark covers every ready quorum source that affects readiness or dispersion, including numerically unselected sources.

### Task 3: Pin all trigger classes and the evidence flood

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exit_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Produces: receive-domain evaluation context for production book, signal, and selection trigger constructors; test-only reference and structurally absent unknown/other constructors retain explicit test coverage.
- Consumes: the shared pricing-layer classifier from Task 1.

- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: the six-tick alternating-book differential retains one record after the original six-record RED.
- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: the mixed book/signal/selection differential retains one record after the original four-record RED.
- [x] Structurally missing receive context uses the test-only diagnostic constructor and retains `MissingEvaluationEventTime → Hold` without making production pricing stamps optional.
- [x] Using the authoritative NT pin landed on `main` through #1367, add an `on_quote`/evidence differential proving stored `trigger_ts_event_ms`, `trigger_ts_init_ms`, and `rv_gate_result` follow their owning domains and do not reproduce the #1354 signal flap.
- [x] Production signal handling requires typed `QuoteTick.ts_init`; no strategy-clock fallback remains. Genuine local selection evaluation uses the distinct typed local-handler constructor.
- [ ] Verify the fresh-port six-tick, mixed-trigger, and missing-context tests GREEN through exact-head remote verification and cite their historical RED transcripts; do not recreate RED or treat `2b83f512a` as authoritative proof.

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

- [x] Make production fair-value and taker pricing requests require `LocalReceiveMs`; keep optionality only at the diagnostic classifier boundary for the structurally absent fail-closed case.
- [x] Add a typed entry RV evaluation receipt to `EntryEvaluation` and `EntrySubmissionDecision`. Capture the gate result and exact admitted snapshot/evidence identity once; make logs, skip evidence, and submit-linked evidence consume the receipt without querying current RV state.
- [x] Add independent near-stale entry mutation differentials for initial uncertainty, sized fee adjustment, resized fee adjustment, log/skip evidence, and submit-linked strategy-input evidence. In each case the trigger stamp remains valid while strategy `now_ms` is stale.
- [x] Add a submit-path differential proving that evidence records the admitted evaluation snapshot and cannot abort submission by re-gating at later strategy wall time.
- [x] Add a state-replacement differential: replace the latest RV snapshot after evaluation and prove logs, skip evidence, and submit evidence retain the receipt's original gate result and snapshot identity.
- [x] Add a route-level maker differential with `quote_receive < snapshot_receive <= explicit_evaluation_receive`; expect pricing to remain available. Do not add maker production wiring.
- [x] Add entry and maker differentials where event clocks disagree but receive freshness is valid; pre-change behavior must block as future-dated/RV-not-ready.

### Task 5: Implement the minimal ownership change

**Files:**
- Modify all production files named in Tasks 1–4.

- [x] Preserve the immutable review-amendment RED evidence from the provisional branch; do not rerun RED or revert production code on this fresh port.
- [x] Add the snapshot receive watermark and accepted-only surface-clock derivation.
- [x] Change production shared pricing requests to required `LocalReceiveMs`, keep only the diagnostic classifier's structurally absent input optional, and remove production dependence on cross-venue event ordering.
- [x] Thread receive context through every production entry and exit trigger and through the existing maker pricing route boundary; do not add maker production wiring.
- [x] Delete the signal strategy-clock fallback: production signal triggers require NT `ts_init`, genuinely local triggers use a distinct typed handler-entry constructor, and structurally missing signal context stays fail-closed.
- [x] Preserve exact contributing sample identity through configured RV grids and derive each ready source's watermark as the maximum receive timestamp over that set.
- [x] Keep a truly absent receive stamp fail-closed and preserve historical evidence enums.
- [ ] Run every named differential through the repository-approved Rust path and confirm GREEN.
- [x] Run formatting and source-fence gates; correct only issue-owned failures.

### Task 6: Pin recovery behavior at the byte boundary

**Files:**
- Test: `src/strategies/binary_oracle_edge_taker/tests/adverse_path_harness.rs`

**Interfaces:**
- Consumes: existing `recovery_evidence_max_bytes` and settlement/open-position bootstrap.
- Produces: proof that a valid open-position recovery stream succeeds at or below the bound and fails into existing blind recovery above it.

- [x] Historical provisional evidence at superseded checkpoint `2b83f512a`: boundary tests prove exactly 1 MiB recovers managed exposure and 1 MiB + 1 enters `SettlementEvidenceRecoveryFailed` blind recovery.
- [ ] Verify both fresh-port boundary tests GREEN through exact-head remote verification; the historical checkpoint is not authoritative proof.

### Task 7: Capacity evidence and completion gates

**Files:**
- Modify: PR body only for arithmetic and evidence; no repository runtime change.

The replacement PR body retains the six immutable pre-change RED transcripts rather
than manufacturing a new failing head:

```text
fair_value_pricing_does_not_compare_independent_venue_clocks
a one-millisecond lead on the RV source venue clock must not reject pricing: Err([RealizedVolNotReady])

taker_entry_pricing_does_not_compare_independent_venue_clocks
entry pricing must not reject an RV source venue clock that leads by one millisecond: Err([RealizedVolNotReady])

maker_pricing_does_not_compare_independent_venue_clocks
maker pricing must not reject an RV source venue clock that leads by one millisecond

rejected_routed_observation_does_not_advance_the_surface_watermark
left: 50000
right: 4000

exit_evaluation_dedupe_ignores_alternating_consuming_venue_clock_lead
left: 6
right: 1

exit_evaluation_dedupe_does_not_oscillate_across_trigger_sources
left: 4
right: 1
```

- [x] Run the saved read-only dedupe-and-capacity recipe against `s3://bolt-deploy-artifacts/archives/bolt-v2/evidence/order-intents-v0111-session-20260711T074342Z.jsonl.gz` (or its byte-identical local copy); report recipe digest `af2704f6c85201c4d51c0d530800176d63f839d46750f54fb023f81abe4ad226`, the explicit receive-fresh assumption, and the deterministic 166,086 records / 760,791,685 bytes to 106 records / 199,023 bytes result.
- [x] State the replay limit exactly: the archive cannot reproduce final receive-domain `classify_rv_gate` results because it lacks `latest_accepted_receive_ms`, and the historical Binance adapter never produced genuine local receive stamps. Use production-shaped differentials as classifier proof; do not relabel the capacity counterfactual as a final-classifier replay.
- [x] Report only the measured open-position counterfactual window: 47,551 bytes over 939.354 seconds. Do not extrapolate or project a future restart size; it remains unmeasured and subject to the 1 MiB fail-closed boundary and #1275 item 13 pre-soak requirement. #763 remains later and depends on #883; do not promote S3 archival into the soak blockers.
- [x] Run cheap local formatting, deny, workflow-lint, and source-fence-static gates.
- [ ] Commit and publish the fresh branch with `just sandbox-safe-push`, then open a replacement draft PR without closing keywords. Do not force-update or reuse the stale #1361 branch, and do not mark the replacement ready.
- [ ] Detach after publishing the draft head. The user/reviewer owns ready-state exact-head CI, external review, required approval, and merge queue.
- [ ] After the user/reviewer marks the completed replacement PR ready, report the automatic exact-head full-CI Rust test/clippy results they supply. External review occurs only after that head is GREEN; do not wait on or dispatch ad hoc CI. The PR body must describe this requirement without naming a mutable current head SHA.
