# RV Gate Clock-Domain Ownership Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RV admission and evidence stable across independently clocked venues by using a pricing-owned receive-time domain for every trigger.

**Architecture:** The RV engine attaches an accepted-observation receive watermark to each snapshot. Shared fair-value pricing compares that watermark with a typed receive-domain evaluation stamp, and edge-taker/maker boundaries provide the stamp for every production trigger. Rejected event timestamps no longer advance the RV surface clock or causal watermark.

**Tech Stack:** Rust 2024, Nautilus Trader event timestamps, nextest, cheap local static gates and automatic PR CI, JSONL decision evidence.

## Global Constraints

- Tasks 1–8 work only under reopened issue #1354. The Binance adapter correction
  and governed Bolt pin prerequisite landed separately through #1367 and are part of
  the authoritative `main` base for this implementation.
- Do not add tolerance windows, rate caps, sampling, byte-limit changes, venue policy, or symbol policy.
- Preserve all action, lifecycle, settlement, and recovery evidence.
- The user-approved Task 7 exception may add RV gate category and watermark-presence
  to the two entry dedupe keys. Raw timestamps and ages, every other dedupe-key
  finding, and the `position_id=None` finding remain report-only.
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
behavior in #1354. The current behavior fails closed and remains tracked in the
design's confirmed separate-defect section and this task for its own follow-up.

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
- [x] Signal-triggered exits capture one local timestamp at `on_quote` handler entry for lifecycle, expiry, and refresh evaluation; venue `ts_event` remains event evidence and `ts_init` remains receive/RV provenance for both valid and invalid signal observations.
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

### Task 7: Close RV gate evidence and semantic-dedupe review findings

**Files:**
- Modify: `src/bolt_v3_decision_evidence.rs`
- Modify: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/config.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/exit_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/entry_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/bolt_v3_decision_evidence.rs` (inline wire tests)
- Test: `src/strategies/binary_oracle_edge_taker/exit_decision.rs` (inline receipt-to-record test)
- Test: `src/strategies/binary_oracle_edge_taker/tests/config.rs`
- Test/support: `src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs`
- Test/support: `src/strategies/binary_oracle_edge_taker/tests/pricing.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`
- Test/support: `src/strategies/binary_oracle_edge_taker/tests/reference_price.rs`
- Test: `tests/bolt_v3_decision_evidence.rs` (all three exit evidence literals and legacy omission/null coverage)
- Test: `tests/bolt_v3_strategy_registration.rs`
- Modify: `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`
- Modify: `docs/superpowers/specs/2026-07-11-rv-gate-clock-domain-design.md`
- Modify: `docs/superpowers/plans/2026-07-11-rv-gate-clock-domain.md`
- Preserve unchanged as legacy omission fixtures:
  `tests/fixtures/bolt_v3/predeploy_exit_decision_evidence.jsonl` and
  `tests/fixtures/bolt_v3/predeploy_exit_evaluation_evidence.jsonl`.

**Interfaces:**
- `raw_taker_config` resolves the already cross-validated
  `realized_volatility_surface_id` against
  `loaded.root.realized_volatility_surfaces`, copies the configured policy into
  required `BinaryOracleEdgeTakerConfig.realized_volatility_max_source_age_ms: u64`,
  and returns the existing binding/startup error when the surface is absent. Exit
  callbacks read only this stored scalar; they never perform an optional policy
  lookup or fail forced-flat because the valid surface has no snapshot. Shared
  pricing Option APIs receive `Some(stored_age)`.
- `ExitRealizedVolatilityGateReceipt` is captured before any `ExitEvaluation` early
  return and transferred unchanged through `ExitSubmissionDecision`. It owns
  `evaluation_receive_ms: Option<LocalReceiveMs>`, the configured surface identity,
  required effective `max_source_age_ms: u64`, one owned
  `RealizedVolGateClassification` (or an equivalent single-classification result),
  one owned `RealizedVolatilityEvidenceFields` projection, snapshot presence,
  watermark, readiness, value/source, mapped blockers/diagnostics, the immutable
  snapshot-versus-trigger delta, and captured fair-up/fair-down/uncertainty
  probabilities.
- `BoltV3ExitDecisionEvidence` and `BoltV3ExitEvaluationEvidence` add optional `rv_snapshot_receive_watermark_ms` and `rv_max_source_age_ms` fields; their wire structs decode omitted or `null` legacy values as `None` without changing `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` from `15`.
- Each durable record receives only the receipt fields its existing schema owns plus
  those two new classifier inputs. Decision readiness remains
  `rv_snapshot_ready = snapshot.ready`; evaluation readiness remains
  `rv_ready = snapshot.ready_realized_vol().is_some()`. One captured signed
  as-of-minus-trigger-event delta maps signed to evaluation evidence and positive-only
  to decision future-delta evidence. `trigger_ts_init_ms` comes from the receipt's
  evaluation receive input, never `exit_eval_now_ms`.
- Existing key fields become `EntrySkipDedupeBaseKey` and
  `BlockedStrategyInputDedupeBaseKey`. Each path owns one stable-base episode with a
  finite seen set of `RvGateDedupeState { gate_result: BoltV3RvGateResult,
  watermark_present: bool }`. Neither base key nor semantic state stores a raw
  watermark, timestamp, age, price, counter, or volatility value.
- Every new Rust test in this task starts with the already-authorized prefix
  `rv_clock_domain_amendment_`.

- [ ] **Step 1: Add the failing exit wire and single-capture tests.**
  - Add `rv_clock_domain_amendment_exit_wires_preserve_new_and_legacy_inputs` across the inline and integration evidence tests. Assert both record kinds serialize `rv_snapshot_receive_watermark_ms = 1_200` and `rv_max_source_age_ms = 500`; delete each key and separately replace it with `null`, then assert both forms decode to `None`. Update all three integration struct literals. Keep both predeploy JSONL fixtures byte-identical and assert their omitted fields decode to `None`. Pin schema version `15`.
  - Extend the startup/config tests with `rv_clock_domain_amendment_runtime_mapping_copies_required_surface_age`, `rv_clock_domain_amendment_runtime_mapping_rejects_unknown_surface`, and required/nonzero runtime-config coverage. Assert the mapper copies the fixture surface's configured `500` rather than a test fallback, and the unknown ID reaches the binding/startup error path.
  - Add `rv_clock_domain_amendment_exit_records_share_the_captured_receipt` and `rv_clock_domain_amendment_exit_receipt_survives_early_returns`. Build one required-threshold receipt and exercise every existing early-return shape.
  - Add `rv_clock_domain_amendment_exit_receipt_is_fully_immutable_after_snapshot_replacement`. Capture a decision, replace the current pricing snapshot, and assert the receipt retains its original receive stamp, configured surface, max age, snapshot presence/as-of/readiness/value/source/blockers/diagnostics, gate, watermark, fair probabilities, uncertainty probability and signed diagnostic delta. Assert each durable record retains only its schema-owned projection plus the two new inputs; do not add receipt value/source fields to exit-evaluation evidence.
  - Include a ready snapshot carrying a blocker before replacement. Assert decision `rv_snapshot_ready` remains raw `snapshot.ready`, evaluation `rv_ready` remains `snapshot.ready_realized_vol().is_some()`, the evaluation record keeps the captured signed delta, and the decision record keeps only its positive future-delta projection.
  - Cover `Accepted`, `MissingSnapshot`, `MissingEvaluationEventTime`, `RejectedFutureDated`, `RejectedStale`, and `RejectedNotReady`. A present snapshot keeps its watermark even for `MissingEvaluationEventTime` and every rejection. Only an absent snapshot or a snapshot with no accepted watermark records `None`.
  - Add `rv_clock_domain_amendment_records_recompute_gate_from_owned_inputs`: table all six categories for both serialized-and-decoded record types, recompute from only that record's trigger receive, snapshot presence/readiness projection, receive watermark and maximum age, and require equality with the stored gate. Decision defines usable readiness exactly as raw `rv_snapshot_ready && rv_snapshot_blockers.is_empty()` plus a present `realized_vol` string that parses finite and `>= 0`, matching `ready_realized_vol()`; evaluation uses its canonical existing `rv_ready`. Include divergent event/receive stamps, `MissingEvaluationEventTime` with a present watermark, and a valid surface with no snapshot. Keep assertions limited to each record's existing schema-owned fields.
  - Add explicit RED discriminators with raw ready `true`, empty blockers and a present watermark but missing RV, plus invalid/non-finite RV spellings when the durable representation permits them; each must remain `RejectedNotReady`. A present finite zero RV must remain usable and reach the otherwise-accepted case.
  - Prove a valid configured surface with no snapshot produces `MissingSnapshot` while retaining `Some(max_source_age_ms)` and an otherwise valid forced-flat exit remains available. Unknown surface identity is a startup failure, never a callback error, ordinary `MissingSnapshot`, or an unbounded-age fallback.

- [ ] **Step 2: Add the failing episode-bounded semantic-dedupe tests.**
  - Add `rv_clock_domain_amendment_entry_skip_episode_tracks_finite_rv_states` and `rv_clock_domain_amendment_blocked_snapshot_episode_tracks_finite_rv_states` in `source_evidence.rs`.
  - For both paths, cover all six gate categories crossed with watermark absence/presence and verify the exact mapping into `RvGateDedupeState`.
  - Run more than 100 `A -> B -> A` oscillations and assert only the first `A` and first `B` emit. Cover `Some -> None -> Some`, numeric `Some(1_200) -> Some(1_201)` churn, and a state returning after intervening states.
  - Assert a stable base-key change begins a fresh episode and both states may emit again. Exercise the real existing episode-end reset for each path and assert the next identical state emits.
  - Preserve writer semantics with dedicated tests: entry-skip marks the state seen even when its writer error is swallowed; blocked-snapshot marks only after a successful write and retries after a propagated failure.
  - Assert emitted records retain exact raw watermark values and gate categories even though the episode set compares only category and presence.

- [ ] **Step 3: Capture the one authorized local RED.**
  - Run `just fmt-check` so formatting is not discovered remotely.
  - Commit the complete Steps 1-2 tests-only state before executing Rust so the RED has an exact immutable commit. Do not publish this commit.
  - Run exactly `BOLT_ALLOW_LOCAL_RUST=1 cargo nextest run --locked rv_clock_domain_amendment_` against that committed tests-only state. This is the single owner-authorized local Rust exception; do not run Rust Probe, a second local Rust command, local Rust GREEN, clippy, or the full suite.
  - A compiler-error RED caused by the not-yet-defined receipt or fields is acceptable. Record the exact commit and compiler/test evidence; named runtime failures are not required.

- [ ] **Step 4: Implement one full immutable exit receipt.**
  - In `raw_taker_config`, resolve the strategy surface against `loaded.root.realized_volatility_surfaces` and copy `surface.policy.max_source_age_ms` into new required `BinaryOracleEdgeTakerConfig.realized_volatility_max_source_age_ms: u64`. Preserve `validate_strategies` cross-reference validation and fail the existing binding/startup `Result` path if direct/post-load construction supplies an unknown surface. Add required positive config validation where the runtime-config layer already enforces positive policy values.
  - Construct `ExitRealizedVolatilityGateReceipt` once before entering `exit_evaluation_with_hold_ev_at`. Its `max_source_age_ms` comes from the stored required config field. Remove callback-time policy lookup; pass `Some(stored_age)` only at shared pricing Option boundaries. A valid surface with no snapshot is `MissingSnapshot`, not an error, so forced-flat remains available.
  - With a receive stamp, consume one `RealizedVolGateClassification`; without one, use the actual `classify_realized_vol_gate` diagnostic path once. Preserve `RealizedVolGateClassification`, `BoltV3RvGateResult`, and existing `exit_rv_gate_result_from_shared` names and roles.
  - Derive fair-up, fair-down and uncertainty probabilities once from the captured accepted RV value. Use the captured fair probability for hold EV. Capture `RealizedVolatilityEvidenceFields`, mapped exit blockers/diagnostics and one signed snapshot-as-of-minus-trigger-event delta at the same boundary.
  - Thread the receipt through `ExitEvaluation`, `ExitSubmissionDecision`, `ExitEvaluationLogFields`, `BoltV3ExitDecisionEvidence::from_exit_decision`, logging and `record_exit_evaluation_evidence`. Every schema-owned RV field in both records comes only from the receipt plus immutable trigger data. Delete all post-capture pricing snapshot lookups, `classify_realized_vol_gate` calls and RV-derived probability recomputation. Preserve decision raw-ready versus evaluation usable-ready semantics; map the captured signed delta unchanged to evaluation and positive-only to decision.
  - Serialize `trigger_ts_init_ms` from `receipt.evaluation_receive_ms`, never from `exit_eval_now_ms`. Add `rv_snapshot_receive_watermark_ms: Option<LocalReceiveMs>` and `rv_max_source_age_ms: Option<u64>` to both durable structs and optional `u64` fields to both wire structs. New records write `Some(receipt.max_source_age_ms)`; missing/null `None` is legacy-only. Preserve schema version 15 and gate taxonomy; do not add value/source fields to the exit-evaluation schema.
  - Keep production startup/exit wrappers on their existing `Result` paths. Test helpers may unwrap those same paths only after supplying a real fixture surface policy and matching required config age; no test-only `None`, unlimited-age or alternate fallback path is allowed.

- [ ] **Step 5: Implement episode-bounded semantic entry dedupe.**
  - Preserve every existing key field in `EntrySkipDedupeBaseKey` and `BlockedStrategyInputDedupeBaseKey`. Define `RvGateDedupeState` and one episode per path containing a stable base key plus `BTreeSet<RvGateDedupeState>` (or an equivalent bounded seen set).
  - Build semantic state directly from `decision.evaluation.realized_volatility_receipt`; never recover required state from optional serialized fields.
  - On stable-base change, replace the episode and begin an empty seen set. Within a stable base, insert and emit only the first occurrence of each state. Preserve the existing admitted-entry and left-RV-not-ready reset sites as the real episode ends.
  - On entry-skip, mark the state before its swallow-on-error write so failure cannot create a tight retry loop. On blocked-snapshot, commit the state only after the propagating write succeeds so failure remains retryable.
  - Keep raw watermark values and every other tick-varying value out of both keys. Do not change `ExitOutcomeKey` or `ExitDecisionDedupeKey`.

- [ ] **Step 6: Complete implementation and cheap local verification.**
  - Update `valid_raw_config`, the direct `BinaryOracleEdgeTakerConfig` fixture, and every surface-attaching helper in `pricing.rs`, `source_evidence.rs` and `reference_price.rs` with the matching configured surface age so unrelated exit tests remain on their intended paths.
  - Update the schema document with the two additive optional version-15 fields and legacy omission/null behavior. Keep the historical archive limitation/arithmetic, unmeasured byte increase, 1 MiB boundary and #1275 constraint unchanged.
  - Run only the permitted cheap gates: `just fmt-check`, `just deny`, `just ci-lint-workflow`, and `just source-fence-static`. Do not run local Rust GREEN or Rust Probe.
  - Commit the coherent implementation and lasting documentation. Keep the PR body head-agnostic: no current SHA, transient check status, or head-specific review receipt.

- [ ] **Step 7: Close the amendment locally before completion handoff.**
  - Request internal adversarial review of the complete local source and resolve every local finding before Task 8 performs the single final publish-and-detach handoff.
  - Do not reply to or resolve GitHub review threads, publish, dispatch CI, run `just verify-remote`, or make transient PR-body claims in this step. Review discussion resumes only after the committed corrections are published and exact-head proof is green in Task 8.

### Task 8: Capacity evidence and completion gates

**Files:**
- Modify: PR body only for lasting arithmetic, scope and timeless merge requirements; no repository runtime change and no mutable head/check/review receipt.

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
- [x] State the replay limit exactly: the archive cannot reproduce final receive-domain `classify_realized_vol_gate` results because it lacks `latest_accepted_receive_ms`, and the historical Binance adapter never produced genuine local receive stamps. Use production-shaped differentials as classifier proof; do not relabel the capacity counterfactual as a final-classifier replay.
- [x] Report only the measured open-position counterfactual window: 47,551 bytes over 939.354 seconds. Do not extrapolate or project a future restart size; it remains unmeasured and subject to the 1 MiB fail-closed boundary and #1275 item 13 pre-soak requirement. The Task 7 additive fields and semantic transitions have an unmeasured byte cost and do not change that arithmetic. #763 remains later and depends on #883; do not promote S3 archival into the soak blockers.
- [ ] After Task 7 is complete, perform one final scope/capacity/documentation consistency review.
- [ ] Mark the existing PR draft before the single final publish. Publish the completed draft head once with `just sandbox-safe-push`, verify that the exact remote PR head equals the published local SHA, then detach. Do not dispatch or run `just verify-remote` from the executor.
- [ ] The user/reviewer marks that completed draft ready and owns the one final exact-head root/BVS full-CI wave, including root archive execution, root `gate`, BVS archive execution and `backtester-gate`. Iteration-only, skipped, prior-head, probe or no-op results are not proof.
- [ ] Only after that exact head is GREEN, reply inline to and resolve both existing review threads with the lasting correction and exact-head evidence. Then request external review and refresh the required native review for the same head.
- [ ] Required approval and merge queue remain reviewer/operator actions; no second implementation publish or executor-owned verification sequence is authorized.
