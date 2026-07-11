# RV Gate Clock-Domain Ownership Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RV admission and evidence stable across independently clocked venues by using a pricing-owned receive-time domain for every trigger.

**Architecture:** The RV engine attaches an accepted-observation receive watermark to each snapshot. Shared fair-value pricing compares that watermark with a typed receive-domain evaluation stamp, and edge-taker/maker boundaries provide the stamp for every production trigger. Rejected event timestamps no longer advance the RV surface clock or causal watermark.

**Tech Stack:** Rust 2024, Nautilus Trader event timestamps, nextest, cheap local static gates and automatic PR CI, JSONL decision evidence.

## Global Constraints

- Tasks 1–7 work only under reopened issue #1354 on
  `fix/1354-rv-gate-clock-domain`. Task 0A uses a separately reviewed NT-fork PR;
  Task 0B uses a dedicated Bolt pin-slice branch/PR named as a prerequisite slice of
  #1354.
- Do not add tolerance windows, rate caps, sampling, byte-limit changes, venue policy, or symbol policy.
- Preserve all action, lifecycle, settlement, and recovery evidence.
- Keep dedupe-key and `position_id=None` findings report-only.
- Treat the root-fix tests and recovery-boundary tests delivered at `2b83f512a` as landed scope: verify them GREEN at the final head and cite their existing RED transcripts; do not revert production code to manufacture new RED output.
- Capture RED only for review-amendment tests not present at `2b83f512a`. The owner
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
- Treat the implementation at `2b83f512a` as provisional landed branch history, not
  completed prerequisite proof. Freeze further #1354 behavior changes until the NT
  fork correction and dedicated Bolt pin slice land. Then merge `main`, containing
  the landed pin slice, into `fix/1354-rv-gate-clock-domain` and revalidate every
  affected path at the merged head; do not rewrite the provisional commit history.

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

### Task 0A: Land the Binance Spot SBE timestamp contract in the NT fork

This is a blocking #1354 prerequisite. BTC production uses Binance Spot SBE, and
pinned NT `9e71b2b` currently assigns the Binance event time to both `ts_event` and
`ts_init`. Current public-upstream and fork `develop` do not contain a correction.
Implement and review the change in `seungpyoson/nautilus_trader`; do not wait for a
public-upstream release.

**Fork files to inspect and change:**
- `crates/adapters/binance/src/spot/websocket/streams/parse.rs`
- `crates/adapters/binance/src/spot/data.rs`
- Adapter tests covering the production handler plus trades, BBO, depth snapshot,
  and depth diff parsing

**Interfaces:**
- Producer: `BinanceSpotDataClient::handle_ws_message` calls its existing
  `AtomicTime::get_time_ns()` once per decoded SBE message.
- Parsers: trades, BBO, depth snapshot, and depth diff receive the explicit local
  adapter-initialization timestamp and assign it to every emitted datum's `ts_init`;
  Binance timestamps remain `ts_event`.
- Review boundary: one fork PR based on the currently pinned fork lineage, with no
  unrelated fork commits included. Target fork branch
  `pin/6be5a50-sbe-schema-3-5` at immutable base
  `9e71b2b1305a66945ba07f0aba2d1eb63208263d`.

- [ ] Record the checked upstream/fork develop heads and release state by SHA and date; these observations do not replace the immutable fork PR base.
- [ ] Create a fork test-only commit with deliberately unequal event and initialization timestamps for the production handler and all four parser families; assert every emitted trade, every inner depth delta, each aggregate wrapper, and the BBO quote; retain the exact fork RED CI URL and SHA.
- [ ] Capture `clock.get_time_ns()` once per decoded SBE message in `handle_ws_message` and pass it explicitly to every parser without adding raw-frame plumbing.
- [ ] Verify fork exact-head CI GREEN: each emitted datum preserves provider event time as `ts_event` and the supplied local adapter-initialization time as `ts_init`.
- [ ] Disclose and review the source-breaking public Rust parser signature changes; enumerate every in-repository caller and state that unknown external Rust callers must pass the new initialization timestamp.
- [ ] Obtain independent review of the fork PR and record base `9e71b2b1305a66945ba07f0aba2d1eb63208263d`, exact RED/GREEN heads, CI run/job URLs, and full commit range. Do not rely on public-upstream acceptance.

---

### Task 0B: Land the governed Bolt NT pin slice

Create a dedicated Bolt PR explicitly named as a prerequisite slice of #1354. It
must contain only the reviewed NT revision migration, governed boundary evidence,
and a dependency-level test/source contract for the corrected public NT parser. RV
watermark and signal-classification/evidence differentials remain on #1354 because
the RV-ingest differential's watermark types and the signal differential's receive-
domain gate semantics do not exist on base `9ac211fe`. The pin slice does not claim
the broader #1354 implementation complete and uses no closing keywords.

**Bolt files and governed surfaces:**
- `Cargo.toml`
- `Cargo.lock`
- `crates/backtesting-vertical-slice/Cargo.toml`
- `crates/backtesting-vertical-slice/Cargo.lock`
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md`
- `docs/bolt-v3/research/naming/nt-owned-name-audit.yaml`
- `scripts/verify_bolt_v3_boundary_evidence.py`
- `scripts/test_verify_bolt_v3_boundary_evidence.py`
- `scripts/run_fences.py`
- `src/bolt_v3_providers/boundary_registry.rs`
- Relevant canonical pin-census, dependency-contract, and source-fence tests

**Interfaces:**
- Consumes: exact independently reviewed NT fork SHA from Task 0A.
- Produces: one governed NT revision across both manifests, both lockfiles, the
  runtime contract, naming ledger, verifier constant/fixture, registered Binance SBE
  timestamp provenance, and a dependency-level parser/source contract.

- [ ] Audit every current NT revision reference and enumerate the fork-only commits between the pinned base and proposed head; reject unrelated or missing fork changes.
- [ ] Update every governed NT pin and recorded revision atomically; prove no mixed revisions remain and preserve the existing Binance schema 3:5 fork correction.
- [ ] Add one negative-tested canonical pin census spanning both manifests, both lockfiles, the runtime contract, naming ledger, verifier constant, and verifier fixture; prove a mismatch in any one surface fails. Register the census in `scripts/run_fences.py` so `source-fence-static` enforces it on every PR, including drafts.
- [ ] Register Binance Spot SBE timestamp provenance in the authoritative boundary registry, but do not claim the registry row alone proves SHA lineage or timestamp semantics. Bind the reviewed SHA and handler/parser symbols through source-fence/static evidence.
- [ ] Add a mandatory direct dependency-level unequal-stamp test of the corrected public `nautilus-binance` parser. Keep reviewed-SHA and handler/parser source fencing as additional lineage evidence, never as a behavioral substitute.
- [ ] Verify checked-in BTC configuration selects `binance_spot_data`, the SBE endpoint, and the corrected parser path; record the exact NT SHA.
- [ ] Publish a clean draft pin-slice PR and detach. Draft checks provide static feedback only; the user/reviewer owns the ready-state exact-head full-CI proof.
- [ ] Obtain the required exact-head review and land the pin slice before resuming this branch. Do not merge or queue from the implementation agent.
- [ ] Merge `main`, containing the landed pin slice, into `fix/1354-rv-gate-clock-domain`; then revalidate every previously landed classifier, entry, exit, maker, evidence, and recovery-boundary differential at the merged head.

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
- [x] Production fair-value/taker requests and RV-consuming helpers require `LocalReceiveMs`; optionality remains only at the lower diagnostic classifier boundary.
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
- [ ] After merging the landed NT pin, route an unequal-stamped Binance-shaped quote through RV observation; assert surface `as_of_ms` follows `ts_event` and `latest_accepted_receive_ms` follows `ts_init`.
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
- [x] Structurally missing receive context uses the test-only diagnostic constructor and retains `MissingEvaluationEventTime → Hold` without making production pricing stamps optional.
- [x] After merging the landed NT pin, add an `on_quote`/evidence differential proving stored `trigger_ts_event_ms`, `trigger_ts_init_ms`, and `rv_gate_result` follow their owning domains and do not reproduce the #1354 signal flap.
- [x] Production signal handling requires typed `QuoteTick.ts_init`; no strategy-clock fallback remains. Genuine local selection evaluation uses the distinct typed local-handler constructor.
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

- [ ] Commit all review-amendment tests from Tasks 2–4 without amendment production changes. Run the one owner-approved scoped break-glass command `BOLT_ALLOW_LOCAL_RUST=1 cargo nextest run --locked rv_clock_domain_amendment_`; retain the exact RED output and confirm every failure matches the intended old behavior before production changes. Do not publish or mark a deliberately failing head ready.
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
- [ ] Report open-position bytes/hour, bytes per genuine phase transition, projected bytes at the planned restart, and whether #1275 item 13 remains a pre-soak requirement. #763 remains later and depends on #883; do not promote S3 archival into the soak blockers.
- [ ] Run cheap local formatting, deny, workflow-lint, and source-fence-static gates.
- [ ] Commit and publish with `just sandbox-safe-push`, and open or update the draft PR without closing keywords. Do not mark it ready.
- [ ] Detach after publishing the draft head. The user/reviewer owns ready-state exact-head CI, external review, required approval, and merge queue.
- [ ] After the user/reviewer marks the completed PR ready, report the automatic exact-head full-CI Rust test/clippy results they supply. External review occurs only after that head is GREEN; do not wait on or dispatch ad hoc CI.
