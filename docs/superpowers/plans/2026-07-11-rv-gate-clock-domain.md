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
- The user-approved Task 7 exception adds a separate twelve-bit RV
  category/watermark-presence novelty mask alongside each current stored dedupe key.
  It modifies neither the existing key type nor its meaning. Raw timestamps and ages,
  every other dedupe-key finding, and `position_id=None` remain report-only.
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
  This exactly one invocation is both compilation proof and behavioral RED proof.
  Before it, source/static inspection may establish only that tests appear to use
  current APIs; it must not claim compile proof. A compiler error, zero matches,
  setup/harness failure, or interrupted command invalidates RED and requires an
  immediate stop plus explicit owner approval for any rerun, retry, check/no-run,
  probe, or second invocation.
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

- [x] Preserve the immutable review-amendment RED evidence for the already-completed
  Tasks 1–4 and their provisional tests; do not rerun those historical RED cases or
  revert production code on this fresh port. This prohibition does not apply to the
  new Task 7-only amendment tests or Task 7 Step 3's exactly one explicitly
  authorized local invocation.
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
- Modify: `src/bolt_v3_realized_volatility_runtime.rs`
- Modify: `src/strategies/registry.rs`
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
- `raw_taker_config` alone resolves the already cross-validated
  `realized_volatility_surface_id` against
  `loaded.root.realized_volatility_surfaces`. It distinguishes an absent surfaces
  block from a present block missing the configured ID and returns the existing
  binding/startup error for either. It copies the selected positive policy age into
  required raw strategy TOML. `parse_config` cannot resolve identifiers; it only
  requires the copied scalar, rejects zero, and constructs required
  `BinaryOracleEdgeTakerConfig.realized_volatility_max_source_age_ms: u64`. Root
  `validate_realized_volatility_surfaces` already rejects zero; direct parser
  rejection is a new defense for bypass callers, not a loaded-config behavior change.
  Add the field only through
  `binary_oracle_edge_taker_config_fields!` as `u64 => Integer`; the macro remains
  struct/parser/allowlist SSOT.
- Every entry, exit, and shared production pricing use reads the required config
  scalar. `taker_pricing_config` owns it; delete `runtime_taker_pricing_config` and
  rewire its three callers. Rewire the five direct policy consumers and the pricing
  test helper. Option-valued diagnostic boundaries receive `Some(stored_age)`. Delete
  `StrategyBuildContext::realized_volatility_max_source_age_ms_for_surface` and
  `RealizedVolSurfaceRuntime::max_source_age_ms_for_surface`, plus
  `BinaryOracleEdgeTaker::realized_volatility_max_source_age_ms`; require zero
  definitions/calls of all deleted policy wrappers. Retain runtime/context ownership
  of ingest/subscriptions/snapshots and preserve classifier wrapper
  `classify_realized_vol_gate`.
- `ExitRealizedVolatilityGateReceipt` is captured before any `ExitEvaluation` early
  return and transferred unchanged through `ExitSubmissionDecision`. From one
  immutable `latest_realized_vol_snapshot_for_surface` borrow, call free
  `classify_rv_gate` exactly once and build a compact owned exit-only projection of
  schema/decision-needed scalars and bounded cloned fields: gate,
  presence/as-of/watermark, raw+usable readiness, accepted RV/source, mapped exit
  blockers/diagnostics, signed delta, derived fair/uncertainty probabilities,
  surface/max-age/evaluation receive. Do not own `RealizedVolGateClassification`, a
  full `RealizedVolSnapshot`, or full entry `RealizedVolatilityEvidenceFields`; do not
  call clone-producing `classify_realized_vol_snapshot` on this path.
- `BoltV3ExitDecisionEvidence` and `BoltV3ExitEvaluationEvidence` add optional `rv_snapshot_receive_watermark_ms` and `rv_max_source_age_ms` fields; their wire structs decode omitted or `null` legacy values as `None` without changing `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` from `15`.
- The receipt watermark stays typed `LocalReceiveMs`. Decision durable/wire fields
  are `rv_snapshot_receive_watermark_ms: Option<u64>`,
  `rv_max_source_age_ms: Option<u64>`, and decision-only
  `rv_snapshot_has_ready_realized_vol: Option<bool>`. Evaluation durable/wire fields
  are `rv_snapshot_receive_watermark_ms: Option<i64>` with checked conversion and
  `rv_max_source_age_ms: Option<u64>`; evaluation continues to use existing
  `rv_ready: bool`. All five outbound evaluation absolute times use `i64::try_from`:
  trigger event/init, exit-evaluation now, RV as-of, and watermark. A private Result
  builder runs after existing dedupe marking. Build/writer error logs one
  field-specific error and skips the whole evidence record without changing trading;
  `record_exit_evaluation_evidence` stays unit-returning/non-aborting. Nothing wraps,
  saturates, substitutes, or becomes `None` on conversion error.
- Manual evaluation deserialization rejects negative trigger-init/watermark before
  durable construction; omission/null/zero/`i64::MAX` remain valid. Encoding also
  rejects manually constructed negative values and replay treats them unreplayable.
  Existing signed event/lifecycle/as-of inbound semantics remain out of scope.
- Decision keeps existing `realized_vol` gate-filtered semantics. Replay never uses
  it or the stored gate. Decision replay markers are positive max age plus present
  independent readiness; evaluation's marker is positive max age. Missing markers
  mean legacy/unreplayable. Once marked, absent snapshot/as-of, evaluation receive,
  and watermark are meaningful classifier inputs. New decision writes readiness
  `Some(false)` even for no snapshot. Replay mirrors free `classify_rv_gate` precedence:
  missing snapshot; missing evaluation receive; missing watermark -> not ready;
  future; stale; unusable readiness -> not ready; accepted. The strategy wrapper is
  `classify_realized_vol_gate` and must not redefine that order.
- Snapshot presence uses existing fields only: decision
  `rv_snapshot_as_of_ms.is_some()` and evaluation `rv_as_of_ms.is_some()`. No presence
  boolean is added; `None` is missing snapshot and `Some(0)` is present.
- When both operands exist, capture snapshot-as-of-minus-trigger-event losslessly as
  `i128::from(snapshot_as_of_ms) - i128::from(trigger_event_ms)` from the original
  `u64` values. Never use `VenueEventMs::signed_delta_since` or pre-cast either
  operand with `as i64`. The positive component reaches decision future-delta
  evidence only through checked `u64::try_from(delta)`; evaluation narrows with
  `i64::try_from(delta)` only after all five absolute-time conversions succeed. The
  evaluation field remains the historically misnamed `rv_as_of_minus_now_ms` with
  unchanged snapshot-as-of-minus-trigger-event semantics. `trigger_ts_init_ms`
  comes from the receipt's evaluation receive input, never `exit_eval_now_ms`.
- Preserve each path's current existing dedupe key without the newly added RV
  novelty dimension exactly; it may already contain RV diagnostic fields whose churn
  and evidence volume remain unmeasured. Store only
  `{ current_existing_key, rv_seen_mask: u16 }`; never retain prior keys. Map six
  gate results x watermark presence to bits 0..11. Current-key change clears the
  mask, so returning to a prior key emits again. For one fixed key each RV bit emits
  once; raw `Some` value churn is suppressed and presence changes discriminate.
  Existing resets clear key+mask; existing writer-mark timing is unchanged.
- Every new Rust test in this task starts with the already-authorized prefix
  `rv_clock_domain_amendment_`.

- [ ] **Step 1: Add the failing exit wire and single-capture tests.**
  - Build RED with dynamic TOML/JSON mutation and current public strategy/evidence
    APIs. Before the authorized command, source/static inspection may say only that
    tests appear to use current APIs; it cannot claim compilation. References to new
    typed fields and struct literals begin only after the RED transcript.
  - Add `rv_clock_domain_amendment_exit_wires_preserve_new_and_legacy_inputs` using
    serialized JSON values during RED. Require decision watermark `1_200u64`, max age
    `500u64`, and independent readiness `true`; require evaluation watermark
    `1_200i64` and max age `500u64`. Prove new missing-snapshot decisions write
    readiness `Some(false)`. Cover marker omission/null as legacy-unreplayable while
    marker-present snapshot/evaluation/watermark `None` values replay normally.
    Assert decision `rv_snapshot_as_of_ms = null` and evaluation `rv_as_of_ms = null`
    mean missing snapshot, while `0` means present and reaches later precedence.
  - Add `rv_clock_domain_amendment_exit_evaluation_conversion_failure_skips_record`
    with `u64::MAX` outbound cases for trigger event, trigger init,
    exit-evaluation now, RV as-of, and watermark. Each asserts one exact
    field-specific error, no record, unchanged callback/order outcome, and writer not
    reached when build fails. Add
    `rv_clock_domain_amendment_negative_receive_fields_fail_decode_and_encode` with
    direct-payload and full-line negative cases for
    trigger-init/watermark, encode rejection for manually negative durable values,
    defensive unreplayability, and omitted/null/0/`i64::MAX` round trips. Pin schema
    version `15` and keep both predeploy fixtures byte-identical.
  - Add `rv_clock_domain_amendment_exit_evidence_failure_is_non_aborting`: both a
    private-builder error and a writer error log once, emit no partial record, retain
    dedupe mark-before-failure, and leave submission/exposure/order outcome unchanged.
  - Add `rv_clock_domain_amendment_runtime_mapping_copies_required_surface_age`,
    `rv_clock_domain_amendment_runtime_mapping_rejects_absent_surface_block`,
    `rv_clock_domain_amendment_runtime_mapping_rejects_unknown_surface`, and
    `rv_clock_domain_amendment_runtime_config_requires_positive_surface_age` using
    dynamic TOML/current mapper APIs. Prove `raw_taker_config` copies configured
    `500`, distinguishes both missing shapes, and `parse_config` rejects missing,
    wrong-type, and zero derived values. Extend the existing
    `validate_table_allowlist_is_single_sourced_from_config_struct` test with a sanity
    assertion for `realized_volatility_max_source_age_ms`; because this is an
    existing test, the amendment-prefix rule does not apply to it. Update
    `valid_raw_config`, direct config literals, and surface-attaching helpers.
  - Add `rv_clock_domain_amendment_exit_records_share_the_captured_receipt` and
    `rv_clock_domain_amendment_exit_receipt_survives_early_returns`. In RED, drive
    current public exit APIs and assert their serialized projections; do not name or
    construct the not-yet-defined receipt type. Exercise every existing early-return
    shape.
  - Add `rv_clock_domain_amendment_exit_receipt_is_fully_immutable_after_snapshot_replacement`. Capture through current public behavior, replace the pricing snapshot, and assert the original receive stamp, configured surface, max age, snapshot presence/as-of/readiness/value/source/blockers/diagnostics, gate, watermark, fair probabilities, uncertainty probability and signed diagnostic delta remain the RV source. Durable records retain only schema-owned projections; non-RV market inputs claim only same-callback atomicity.
  - Include a raw-ready blocked snapshot. Decision raw readiness stays true, its new independent usable-readiness input is false, evaluation `rv_ready` is false, and existing decision `realized_vol` remains gate-filtered. Evaluation keeps the captured signed delta; decision keeps only its positive future projection. Pin the `rv_as_of_minus_now_ms` misnomer semantics.
  - Add extreme-delta cases using `u64::MAX` and zero in both operand orders. Prove
    receipt/decision projection never wraps or inverts sign, positive decision output
    uses checked `u64::try_from`, and evaluation evidence fails loud and skips the
    complete record whenever an absolute time cannot enter its signed wire domain.
    The callback/order result remains unchanged. Do not obtain the expected delta by
    calling the production delta helper.
  - Cover `Accepted`, `MissingSnapshot`, `MissingEvaluationEventTime`, `RejectedFutureDated`, `RejectedStale`, and `RejectedNotReady`. A present snapshot keeps its watermark even for `MissingEvaluationEventTime` and every rejection. Only an absent snapshot or a snapshot with no accepted watermark records `None`.
  - Add `rv_clock_domain_amendment_records_recompute_gate_from_owned_inputs` for
    serialized-and-decoded records. Recompute without reading stored gate or
    gate-filtered RV. Lock exact free-function precedence with cases for not-ready +
    future, not-ready + stale, blocker + stale, missing snapshot, missing evaluation,
    missing watermark, each of six results, and accepted zero. Only missing/nonpositive
    marker fields are legacy/unreplayable; legitimate optional classifier inputs
    retain their normative meanings after markers are present.
  - Prove a valid configured surface with no snapshot produces `MissingSnapshot` while retaining `Some(max_source_age_ms)` and an otherwise valid forced-flat exit remains available. Unknown surface identity is a startup failure, never a callback error, ordinary `MissingSnapshot`, or an unbounded-age fallback.

- [ ] **Step 2: Add the failing current-key RV-mask dedupe tests.**
  - Add `rv_clock_domain_amendment_entry_skip_current_key_tracks_twelve_rv_bits` and
    `rv_clock_domain_amendment_blocked_snapshot_current_key_tracks_twelve_rv_bits`.
    For each fixed current existing key, visit all twelve category/presence bits once
    and assert twelve emissions. During RED, derive a test-local observed `u16` mask
    from emitted records and assert `count_ones() == 12`; do not reference a
    nonexistent production `rv_seen_mask`. Direct production-state assertions may be
    added only after implementation, while behavior remains the primary contract.
  - Add `rv_clock_domain_amendment_current_key_rv_mask_suppresses_repeats`: run more
    than 100 repeats and `A -> B -> A` oscillations after all bits are seen; keep the
    test-local observed mask at `count_ones() == 12`. Suppress raw
    `Some(1_200) -> Some(1_201)` churn, but require
    `Some -> None` to select the paired presence bit.
  - Add `rv_clock_domain_amendment_existing_key_changes_reset_rv_mask` for both paths.
    Mutate every field of each current existing key independently and require a reset
    and emission. Return to a prior key and require another emission, proving no
    historical-key retention and preserving adjacent-change semantics.
  - Add `rv_clock_domain_amendment_entry_skip_writer_failure_marks_seen` and
    `rv_clock_domain_amendment_blocked_snapshot_writer_failure_retries`: entry-skip
    marks seen even when its writer error is swallowed. For blocked snapshot, require
    the exact sequence `A recorded -> B write fails -> A remains suppressed -> B
    retries and emits`; failed B must not replace the current key or clear A's mask.
  - Exercise both existing reset sites and assert they clear current key plus mask.
    Emitted records retain exact raw watermark values and gate categories.

- [ ] **Step 3: Capture the one authorized local RED.**
  - Run `just fmt-check` so formatting is not discovered remotely.
  - Before committing, inspect only source/static shape for apparent current-API use;
    do not claim compilation. Commit the complete Steps 1-2 tests-only state so the
    RED has an exact immutable commit. Do not publish this commit.
  - Run exactly `BOLT_ALLOW_LOCAL_RUST=1 cargo nextest run --locked rv_clock_domain_amendment_` against that committed tests-only state. This is the single owner-authorized local Rust exception; do not run Rust Probe, a second local Rust command, local Rust GREEN, clippy, or the full suite.
  - Accept the RED only when the named amendment tests execute and fail their named
    assertions for missing production behavior. This one invocation is compilation
    and behavioral proof. Compiler error, zero matches, setup/harness failure, or
    interruption invalidates RED: stop and request explicit owner approval before any
    rerun, retry, check/no-run, probe, or second invocation. Record the immutable test
    commit and exact assertion output.

- [ ] **Step 4: Implement one full immutable exit receipt.**
  - In `raw_taker_config`, distinguish absent surfaces block from missing configured ID
    and copy positive `surface.policy.max_source_age_ms`. In `parse_config`, require
    the copied scalar and reject zero without resolving IDs. Preserve
    `validate_strategies` cross-reference validation and existing startup errors. Add
    the derived field through `binary_oracle_edge_taker_config_fields!` only; do not
    hand-add struct/parser/allowlist entries. Disclose direct-parser zero rejection as
    defense-in-depth over the existing root positive-age invariant.
  - Rewire every entry, exit, and shared production pricing call to the required
    config scalar. Set `taker_pricing_config` from it; delete
    `runtime_taker_pricing_config` and rewire
    `current_entry_pricing_inputs_for_receive_at`,
    `current_fair_probability_up_for_receive_at`, and
    `current_scaled_min_edge_bps_at`. Rewire direct consumers
    `current_realized_vol_for_gate_at`,
    `current_realized_vol_source_for_gate_at`,
    `exit_realized_volatility_gate_fields_at`,
    `entry_realized_volatility_receipt_at`, and
    `record_exit_evaluation_evidence`, plus the pricing test helper. Delete
    `BinaryOracleEdgeTaker::realized_volatility_max_source_age_ms` and both optional
    policy lookup accessors from
    `registry.rs`/`bolt_v3_realized_volatility_runtime.rs`; require zero definitions/
    calls of these and `runtime_taker_pricing_config`. Retain ingest/subscription/
    snapshot responsibilities and distinct classifier wrapper
    `classify_realized_vol_gate`.
  - Construct `ExitRealizedVolatilityGateReceipt` once before entering `exit_evaluation_with_hold_ev_at`. Remove callback-time policy lookup; pass `Some(stored_age)` only at shared diagnostic Option boundaries. A valid surface with no snapshot is `MissingSnapshot`, not an error, so forced-flat remains available.
  - Borrow the latest surface snapshot once in a dedicated
    `exit_realized_volatility_gate_receipt_at` capture helper and invoke free
    `classify_rv_gate` exactly once even when receive context is absent. Build the
    compact exit-only projection; do not own full snapshot/classification/entry
    evidence and do not use `classify_realized_vol_snapshot`. Preserve the strategy
    wrapper only for other diagnostics/non-receipt paths.
  - Derive fair-up/down and uncertainty once. Move `ExitEvaluation` into
    `ExitSubmissionDecision` instead of `evaluation.clone()`. Log/durable builders
    borrow the single receipt. Delete post-capture queries/locks/classification and RV
    recomputation. Only schema-owned strings/vectors are copied; no-divergence remains
    scoped to RV-derived fields and non-RV inputs retain same-callback atomicity.
  - Preserve decision raw-ready and gate-filtered `realized_vol`. Add independent
    decision `rv_snapshot_has_ready_realized_vol: Option<bool>` durable+wire input;
    evaluation retains `rv_ready`. Serialize receipt watermark to decision
    `Option<u64>` and evaluation `Option<i64>`. Use `i64::try_from` for all five
    evaluation absolute times. After dedupe marking, a private Result helper builds
    the record; build/writer failure logs once and skips the complete record without
    changing the callback's trading result. Serialize max age as `Option<u64>` on both. Decision
    writes readiness `Some(false)` for missing snapshot. Only missing/nonpositive
    replay markers are legacy/unreplayable; marker-present optional classifier inputs
    retain their normative meanings. Preserve
    schema version 15 and gate taxonomy; do not add value/source fields to evaluation.
  - Serialize `trigger_ts_init_ms` from `receipt.evaluation_receive_ms`, never from
    `exit_eval_now_ms`. Compute the receipt delta from the original `u64` operands as
    `i128::from(snapshot_as_of_ms) - i128::from(trigger_event_ms)`; do not use
    `VenueEventMs::signed_delta_since` or `as i64`. Convert its positive decision
    projection with checked `u64::try_from`. Only after every absolute-time
    `i64::try_from` succeeds may the private record builder narrow the delta with
    `i64::try_from`. Preserve the existing signed `rv_as_of_minus_now_ms` field's
    snapshot-as-of-minus-trigger-event semantics despite its name. An
    unrepresentable absolute time or delta logs once, skips the record, and cannot
    alter submission/exposure/order behavior.
  - In manual deserialize, reject negative trigger-init/watermark before durable
    construction; make encode reject manually negative values; keep omitted/null/0/
    `i64::MAX` valid. Do not broaden inbound-negative policy to event/lifecycle/as-of.
  - Keep production startup/exit wrappers on their existing `Result` paths. Test helpers may unwrap those same paths only after supplying a real fixture surface policy and matching required config age; no test-only `None`, unlimited-age or alternate fallback path is allowed.

- [ ] **Step 5: Implement current-key RV-mask entry dedupe.**
  - Preserve each current existing dedupe key without the newly added RV novelty
    dimension byte-for-byte in meaning. It may contain RV diagnostic fields; their
    churn and evidence volume remain unmeasured. For each path, store only that
    current key plus `rv_seen_mask: u16`; retain no key history and introduce no
    `BTreeSet`/Cartesian semantic-state store.
  - Build semantic state directly from `decision.evaluation.realized_volatility_receipt`; never recover required state from optional serialized fields.
  - Map gate category/presence to bits 0..11. A current-key change replaces the key
    and clears the mask; returning to an older key therefore emits. For one fixed key,
    emit only the first occurrence of each bit. Ignore raw `Some` watermark movement.
  - Entry skip mutates key/mask before its swallowed-error write. Blocked snapshot
    stages a candidate key/mask without mutation, performs the propagating write, and
    commits both only on success. Preserve existing resets. Do not change
    `ExitOutcomeKey` or `ExitDecisionDedupeKey`. Claim only constant memory and
    twelve-state RV novelty per current key; evidence volume under existing-key churn,
    including RV diagnostics, remains unmeasured.

- [ ] **Step 6: Complete implementation and cheap local verification.**
  - Update `valid_raw_config`, the direct `BinaryOracleEdgeTakerConfig` fixture, and every surface-attaching helper in `pricing.rs`, `source_evidence.rs` and `reference_price.rs` with the matching configured surface age so unrelated exit tests remain on their intended paths.
  - Update the schema document with exact durable/wire types: decision optional
    `u64` watermark, optional `u64` max age, optional independent readiness boolean;
    evaluation optional checked `i64` watermark and optional `u64` max age. Document
    marker-based legacy replayability, legitimate marker-present optional inputs,
    `i64::try_from` on all five outbound evaluation absolute times, field-specific
    fail-loud-and-skip semantics, negative receive-field decode/encode rejection, unchanged gate-filtered
    decision RV, existing evaluation readiness, schema version 15, and the historical
    delta-field misnomer semantics.
  - Record the fixed-field capacity bound: the two numeric fields are at most 100
    bytes/record; decision readiness makes its conservative total at most 143 bytes.
    Pessimistically 106 x 143 adds at most 15,158 bytes, safely below 1 MiB for the
    fixed 106-record counterfactual. Keep semantic-entry record-count growth
    explicitly unmeasured; do not claim a final restart size.
  - Run only the permitted cheap gates: `just fmt-check`, `just deny`, `just ci-lint-workflow`, and `just source-fence-static`. Do not run local Rust GREEN or Rust Probe.
  - Do not add a Rust source-text unit test for receipt shape. After implementation,
    use
    `rg -n 'struct ExitRealizedVolatilityGateReceipt|RealizedVolSnapshot|RealizedVolGateClassification|RealizedVolatilityEvidenceFields' src/strategies/binary_oracle_edge_taker/`
    to locate and inspect the actual complete receipt definition wherever it is
    declared, proving it owns no full snapshot, classification, or entry-evidence
    field. Use
    `rg -n 'fn exit_realized_volatility_gate_receipt_at|classify_rv_gate\(|classify_realized_vol_snapshot\(|evaluation\.clone\(\)' src/strategies/binary_oracle_edge_taker/`
    and inspect the complete capture-to-`ExitSubmissionDecision` path across every
    file it traverses, proving exactly one free-classifier call, no clone-producing
    classifier call, and a moved rather than cloned `ExitEvaluation`. Record the
    inspected ranges, run
    `just source-fence-static`, and require the internal adversarial review in Step 7
    to repeat this structural inspection.
  - Commit the coherent implementation and lasting documentation. Keep the PR body head-agnostic: no current SHA, transient check status, or head-specific review receipt.

- [ ] **Step 7: Close the amendment locally before completion handoff.**
  - First restore/remove this implementation plan from the final PR diff while keeping
    the durable design/schema docs. Run the final changed-file census and stop if the
    plan remains.
  - Then request internal adversarial review of that exact publishable head and
    resolve every finding before Task 8. After approval, no source or repository-doc
    change is allowed; only timeless PR-body GitHub text may change. Publish only the
    reviewed head.
  - Do not reply to/resolve GitHub threads, publish, dispatch CI, run
    `just verify-remote`, or make transient PR-body claims in this step.

### Task 8: Capacity evidence and completion gates

**Files:**
- Modify: PR body only for lasting arithmetic, scope and timeless merge requirements; no repository runtime change and no mutable head/check/review receipt.
- Remove from the final PR diff:
  `docs/superpowers/plans/2026-07-11-rv-gate-clock-domain.md`; retain the durable
  design and schema documents.

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
- [x] State the replay limit exactly: the archive cannot reproduce final receive-domain free `classify_rv_gate` results because it lacks `latest_accepted_receive_ms`, and the historical Binance adapter never produced genuine local receive stamps. The strategy wrapper `classify_realized_vol_gate` preserves that free-function precedence while adding diagnostics. Use production-shaped differentials as classifier proof; do not relabel the capacity counterfactual as a final-classifier replay.
- [x] Report only the measured open-position counterfactual window: 47,551 bytes over 939.354 seconds. Do not extrapolate or project a future restart size; it remains unmeasured and subject to the 1 MiB fail-closed boundary and #1275 item 13 pre-soak requirement. Bound the two numeric additions at no more than 100 bytes per record and the decision record's numeric-plus-readiness additions at no more than 143 bytes. Pessimistically charging 106 records adds at most 15,158 bytes and remains safely below 1 MiB at that fixed record count. Semantic-entry record-count growth remains unmeasured and therefore is not folded into the historical arithmetic. #763 remains later and depends on #883; do not promote S3 archival into the soak blockers.
- [ ] After Task 7 is complete, perform one final scope/capacity/documentation consistency review.
- [ ] Mark the existing PR draft before the single final publish. Confirm the plan is absent from the changed-file census and the local head equals the internally approved publishable head. Publish that head once with `just sandbox-safe-push`, verify that the exact remote PR head equals it, then detach. No source/doc change or second publish is authorized. Do not dispatch or run `just verify-remote` from the executor.
- [ ] The user/reviewer marks that completed draft ready and owns the one final exact-head root/BVS full-CI wave, including root archive execution, root `gate`, BVS archive execution and `backtester-gate`. Iteration-only, skipped, prior-head, probe or no-op results are not proof.
- [ ] Only after that exact head is GREEN, reply inline to and resolve both existing review threads with the lasting correction and exact-head evidence. Then request external review and refresh the required native review for the same head.
- [ ] Required approval and merge queue remain reviewer/operator actions; no second implementation publish or executor-owned verification sequence is authorized.
