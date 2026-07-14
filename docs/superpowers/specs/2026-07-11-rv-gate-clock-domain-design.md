# RV Gate Clock-Domain Ownership Design

## Decision

Issue #1354 moves realized-volatility consumption freshness into one pricing-owned
receive-time domain. Venue event timestamps remain inputs to realized-volatility
computation and evidence, but never decide whether another venue may consume a
ready snapshot.

The core clock-ownership implementation is present on a fresh branch from
authoritative `main`; the user-approved durable-input and semantic-dedupe amendment
below remains an implementation task. Local static and exact-head remote Rust
verification, external review, and required native approval remain completion gates;
this document does not claim merge or live-soak readiness.

## Invariant

A realized-volatility snapshot is usable only when all of these hold:

1. the configured surface snapshot exists;
2. the snapshot is ready;
3. the snapshot carries the maximum receive timestamp among the accepted
   observations that actually contributed to its published numeric computation;
4. the pricing evaluation carries a receive-domain timestamp; and
5. same-domain receive age is no greater than the surface's existing
   `max_source_age_ms`.

No tolerance, sampling, rate limit, byte-limit change, venue exception, or symbol
exception participates in the decision.

## Root Cause

Before this change, `classify_rv_gate` compares a surface `as_of_ms` with a consuming trigger's
event timestamp. Both values use `VenueEventMs`, but the surface timestamp is the
maximum of independently clocked spot-source venues while exit book triggers use the
execution venue's clock. The shared wrapper proves only "venue event" versus "local";
it does not prove that two different venues share an ordering domain.

The archived session records the v0.1.12 defect: 20,398 accepted, 20,263 rejected
as future-dated, and 188 missing an evaluation event time. Future deltas are
1–347 ms, median 20 ms and p95 28 ms. Those records preserve the old event-domain
inputs and result; they do not preserve every input required by the final
receive-domain classifier.

## Owning Types and Data Flow

`LocalReceiveMs` owns the freshness comparison.

- Each accepted RV observation already carries `recv_ts_ms`. The RV engine preserves
  sample identity through the base, coarse, and subsampled grids and derives a
  snapshot receive watermark only from accepted observations actually selected by
  the published numeric computation for quorum-contributing sources. The watermark
  is the maximum `recv_ts_ms` over that contributing set, not the receive timestamp
  of the last event-time sample.
- Rejected observations may update rejection diagnostics, but cannot advance either
  the surface event clock or the receive watermark.
- `RealizedVolSnapshot` carries the typed receive watermark beside event-domain
  `as_of_ms`.
- Production `FairValuePricingRequest` and `TakerPricingRequest` require a
  `LocalReceiveMs` evaluation stamp. RV-consuming helpers accept that required stamp
  directly. The lower-level diagnostic classifier retains an optional boundary so a
  structurally absent timestamp remains representable and fail-closed without making
  absence constructible in production pricing.
- Book triggers use NT `ts_init`. Signal triggers may use NT `ts_init` only after
  the producing adapter proves that it is captured from the local receive clock;
  a field name alone is not timestamp-domain evidence. Maker pricing receives an
  explicit evaluation stamp from its caller. Selection/local triggers explicitly
  convert their strategy-clock evaluation instant at the timestamp-domain boundary
  and therefore do not fall into `MissingEvaluationEventTime`.
- Production signal `QuoteTick` handling requires a non-optional local adapter-
  initialization `ts_init` governed by the pinned adapter contract; it never
  substitutes strategy-clock time. Bolt cannot infer provenance from a numeric
  timestamp or from `ts_event != ts_init`, so adapter provenance is enforced by the
  reviewed pin, boundary registry, source fence, and differentials. Structurally
  missing signal receive context remains fail-closed. A genuinely local trigger uses
  a distinct typed constructor that captures strategy time once at handler entry.
- Signal-triggered exits capture one local strategy-clock timestamp at `on_quote`
  handler entry and use it for lifecycle, expiry, and refresh evaluation. The quote's
  venue `ts_event` remains trigger-event evidence, while `ts_init` remains the
  receive-domain stamp for RV classification and provenance. Valid and invalid signal
  observations use the same three-domain ownership.
- `MissingEvaluationEventTime` remains fail-closed only for a structurally absent
  receive stamp. No production trigger is allowed to construct that shape.

### Review-Follow-Up Ownership Closure

Every pricing evaluation owns one receive stamp from its initiating trigger. Helpers
that consume realized volatility must accept that stamp explicitly; they must not
reconstruct one from `now_ms` or borrow the receive time of a pricing input.

- Entry evaluation threads the book delta's typed receive stamp through fair-value
  pricing, uncertainty-band pricing, fee-uncertainty adjustment, log fields, and
  submit-linked strategy-input evidence. `now_ms` remains the wall-clock input for
  expiry and lifecycle calculations only.
- Entry RV evaluation produces an immutable typed receipt owned by `EntryEvaluation`
  and transferred into `EntrySubmissionDecision`. The receipt contains the gate
  result and, when admitted, the exact surface identity, event `as_of_ms`, receive
  watermark, realized-volatility value/source, and evidence fields used by that
  decision. Entry logs, skip evidence, and submit-linked evidence consume this
  receipt; they cannot query the mutable current RV state or re-run the gate.
- Maker fair-value evaluation receives an explicit typed evaluation stamp from its
  caller. The selected reference quote's receive timestamp describes that input; it
  does not own the maker evaluation time.
- A cutoff realized-volatility snapshot derives each source's receive watermark from
  the exact accepted observations selected into every grid used by the published
  numeric computation. This includes base-grid observations and any observations
  used by the configured coarse or subsampled estimator. An accepted observation
  that is merely before `as_of_ms`, but is not selected into the computation, cannot
  advance the watermark. The watermark is the maximum receive timestamp across that
  exact set, including when receive time regresses across ascending event times.

This cutoff contract covers the numeric computation and receive watermark within
retained accepted history. It does not promise a fully historical reconstruction of
present-state rejection diagnostics, source status outside the computation, or
already-pruned samples.

Across sources, the snapshot watermark is the maximum contributing receive timestamp
over every ready quorum-counting source that can affect readiness or dispersion.
Sources numerically excluded by a trimmed-mean or quantile selection remain causal
contributors because their presence and value still affect readiness, dispersion,
and which aggregate value is selected. This intentionally conservative rule prevents
the snapshot from appearing older than any input that could change its published
state.

This closes the ownership boundary in function signatures instead of relying on
call-site convention. No tolerance window or fallback comparison is introduced.

### Durable Exit Inputs and Semantic Entry Dedupe

Surface-policy ownership is resolved before the strategy can start. The existing
`validate_strategies` pass continues to cross-reference each strategy's
`realized_volatility_surface_id`, and `raw_taker_config` independently resolves that
identifier against `loaded.root.realized_volatility_surfaces` and copies
`surface.policy.max_source_age_ms` into the required in-memory
`BinaryOracleEdgeTakerConfig.realized_volatility_max_source_age_ms: u64`. A missing
surface fails the existing binding/startup `Result` path. The exit callback never
looks up an optional policy or fails because a policy disappeared; shared pricing
APIs that retain an optional compatibility boundary receive
`Some(config.realized_volatility_max_source_age_ms)`. This keeps forced-flat
available even when the valid configured surface has no current snapshot.

The receive-domain classifier inputs must remain auditable after the in-memory
snapshot changes. Each exit evaluation therefore owns one immutable
`ExitRealizedVolatilityGateReceipt`, captured before any evaluation early return and
transferred unchanged through `ExitSubmissionDecision`. It owns the evaluation's
optional `LocalReceiveMs`, configured surface identity, required effective
`max_source_age_ms: u64`, one owned `RealizedVolGateClassification` (or an equivalent
single-classification result), and one owned `RealizedVolatilityEvidenceFields`
projection. It also owns the classified snapshot-presence, as-of, readiness, value,
source, blockers, diagnostics and receive watermark, plus the fair-up, fair-down and
uncertainty probabilities derived from the accepted RV value and the immutable
snapshot-versus-trigger diagnostic delta.

The decision, log and both exit durable-record builders derive every RV field that
their existing schemas own from that receipt plus the immutable trigger context.
The receipt's complete value/source/probability projection does not silently widen
either durable schema: decision evidence keeps its existing value/source fields,
while exit-evaluation evidence keeps only the RV fields it already owns plus the two
new classifier inputs below. After capture the builders do not query the current
pricing snapshot, call `classify_realized_vol_gate`, or recompute an RV-derived
probability. `ExitEvaluation` uses the receipt's fair probability to compute hold EV,
so decision and evidence cannot diverge after the pricing state is replaced.

The two existing readiness projections remain deliberately distinct. Decision
evidence writes `rv_snapshot_ready = snapshot.ready`; evaluation evidence writes
`rv_ready = snapshot.ready_realized_vol().is_some()`. A ready snapshot carrying a
blocker therefore discriminates the two projections, and replacement after receipt
capture cannot change either recorded value. One captured signed
snapshot-as-of-minus-trigger-event delta maps unchanged to evaluation evidence's
signed `rv_as_of_minus_now_ms` and maps only its positive component to decision
evidence's `rv_future_dating_delta_ms`. The receipt's evaluation receive stamp, not
`exit_eval_now_ms`, is the sole source of serialized `trigger_ts_init_ms`; a second
timestamp read is not permitted.

A configured surface policy is required and is captured at startup, never recovered
inside an exit evaluation. A valid configured surface with no current snapshot
produces `MissingSnapshot`, retains the configured surface identifier and effective
maximum age, records no snapshot watermark or value, and does not prevent an
otherwise valid forced-flat exit. An unknown surface fails binding/startup and can
never become ordinary `MissingSnapshot` or an unbounded
`max_source_age_ms = None` policy. A structurally missing evaluation receive stamp
remains `MissingEvaluationEventTime`; if a snapshot exists, its receive watermark and
all other schema-owned captured fields remain present in evidence.

`BoltV3ExitDecisionEvidence` and `BoltV3ExitEvaluationEvidence` both serialize
`rv_snapshot_receive_watermark_ms` and `rv_max_source_age_ms` from the receipt. New
writes always store `Some(receipt.max_source_age_ms)`. The fields remain optional only
in the existing version-15 durable/wire shape so a legacy record that omits either
field, or explicitly stores `null`, decodes it as `None`; this amendment does not bump
the evidence schema version or alter the gate taxonomy.

The durable proof is record-local, not dependent on retained strategy memory. For
each of the six gate categories, both serialized-and-decoded record types carry
enough schema-owned input to recompute the gate using only their trigger receive
stamp, snapshot presence/readiness projection, receive watermark and maximum age.
For decision evidence the usable-readiness projection is exactly its existing raw
`rv_snapshot_ready && rv_snapshot_blockers.is_empty()` plus a present
`realized_vol` string that parses to a finite value greater than or equal to zero,
matching `ready_realized_vol()`. Evaluation evidence uses its canonical existing
`rv_ready`. The recomputed result must equal the stored gate. The table includes
divergent event and receive clocks, `MissingEvaluationEventTime` with a present
watermark, a valid configured surface with no snapshot, raw-ready snapshots that
have a blocker, lack an RV value, or carry an invalid/non-finite durable RV spelling
when representable, and an accepted zero RV value. Assertions for each record stop
at the fields that record's existing schema owns.

Entry flood guards track semantic RV gate state without admitting tick-varying time
into their stable base keys. The existing fields become `EntrySkipDedupeBaseKey` and
`BlockedStrategyInputDedupeBaseKey`. Each path owns an episode with one stable base
key and a finite seen set of `RvGateDedupeState { gate_result: BoltV3RvGateResult,
watermark_present: bool }` values:

- The first occurrence of each semantic state in a stable-base episode emits.
- Repeated or returning states, including `A -> B -> A` oscillation, remain
  suppressed after their first occurrence in that episode.
- Numeric watermark movement with unchanged presence remains suppressed. Raw
  watermark values, timestamps, ages, prices, counters and volatility values remain
  outside both base keys and semantic-state keys.
- A stable-base-key change begins a new episode. The entry-skip episode also ends at
  the existing admitted-entry reset; the blocked-snapshot episode ends at the
  existing transition out of the RV-not-ready path.
- Entry-skip preserves its existing mark-even-when-the-swallowed-writer-fails
  behavior. Blocked-snapshot preserves its existing mark-only-after-the-propagating
  writer-succeeds behavior.

The semantic set is bounded by the closed six-category gate enum times two watermark
presence values. Durable records still retain the exact gate result and raw optional
watermark even though dedupe compares only category and presence.

This is the user-approved exception to the earlier report-only dedupe ruling. Other
dedupe-key findings and the `position_id=None` finding remain report-only under
#1354. Exit outcome keys remain unchanged and timestamp-free, so the correction does
not restore the incident's per-tick exit-evidence flood.

### Binance Spot SBE Timestamp-Ownership Prerequisite

The live BTC signal path uses Binance Spot SBE. The historical pre-fix NT revision
`9e71b2b1305a66945ba07f0aba2d1eb63208263d` copied Binance event time into both
`QuoteTick.ts_event` and `QuoteTick.ts_init`. The independently reviewed correction
is already governed on `main` through #1367 at
`afc014a55b51463641cc19c68bffe25cdac6588a`:

- `BinanceSpotDataClient::handle_ws_message` captures one local
  `AtomicTime::get_time_ns()` value per decoded SBE message and passes it to trades,
  BBO, depth snapshot, and depth diff.
- `ts_event` remains provider time; every emitted datum, inner delta, and aggregate
  wrapper uses the supplied local value as `ts_init`.
- Exact pin, handler/parser source, dependency behavior, archive execution, and the
  BTC SBE route are governed by #1367. This PR leaves all of those surfaces unchanged.
- #1354 separately proves that RV ingest derives `as_of_ms` from `ts_event`, derives
  `latest_accepted_receive_ms` from `ts_init`, and classifies signal-trigger evidence
  in the receive domain without re-stamping or fallback.

## Behavioral Blast Radius

- Entry may price on ticks previously blocked only by cross-venue event ordering.
- Normal exit may compute hold EV on those ticks instead of defaulting to Hold.
- Maker fair value may become available on those ticks.
- Submit-linked entry evidence no longer re-decides RV freshness after an entry has
  been admitted. It records the evaluation's snapshot and therefore cannot veto an
  otherwise valid submission merely because strategy wall time advanced.
- When a live refresh's newest accepted observation falls after the final selected
  grid point, that unused observation no longer refreshes the snapshot watermark.
  Staleness may therefore fail closed earlier, while spurious future-dated rejection
  from data absent from the published computation is removed.
- Forced-flat exits, not-ready RV, stale RV, missing snapshots, and genuinely missing
  receive context remain fail-closed.
- Historical `RejectedFutureDated` evidence remains decodable. Event-clock deltas stay
  diagnostic and no longer decide admission.

## Flood and Recovery Evidence

The archived file contains 40,849 exit evaluations over 939.446 seconds. Normalizing
only the cross-venue future result leaves 380 records because 188 selection updates
interleave `MissingEvaluationEventTime` with accepted book/signal evaluations. That
is a failed design: approximately 2.2–2.4 MB per open-position hour and 1 MiB in about
26–28 minutes.

For capacity estimation, the deterministic counterfactual assumes that every ready
production trigger is receive-fresh under the corrected pipeline, normalizes its RV
gate result to `accepted`, and then replays the exact last-key dedupe guards. That
leaves four archived exit-evaluation transitions: two `exit_hold` entries and two
`position_interval_ended` entries. Together with v0.1.12 blocked-snapshot dedupe and
all preserved action/lifecycle evidence, the captured session counterfactual is
199,023 bytes. The captured open-position window contains 47,551 bytes over exactly
939.354 seconds. No future-restart size is measured or projected from this window.
Any future restart remains subject to the existing 1 MiB fail-closed boundary, and
#1275 item 13 segmented recovery remains a pre-soak requirement but is not bundled
into this change. #763 S3 archival remains later and depends on #883 redaction; it is
not a soak blocker.

The two additive optional exit fields modestly increase each newly written exit
decision and exit-evaluation record, and semantic entry transitions may add records
that the old collapsed key suppressed. The byte increase is unmeasured. It does not
change the existing 1 MiB reader boundary, the #1275 item 13 pre-soak requirement, or
the archived-session arithmetic above. Historical records cannot be backfilled with
watermarks or thresholds that they never captured.

## Verification Contract

- The archived session at
  `s3://bolt-deploy-artifacts/archives/bolt-v2/evidence/order-intents-v0111-session-20260711T074342Z.jsonl.gz`
  deterministically supports the dedupe-and-capacity counterfactual:
  166,086 records / 760,791,685 bytes become 106 records / 199,023 bytes under the
  explicit receive-fresh assumption above. The PR report records the read-only recipe,
  recipe digest, audited code head, counts, and byte totals.
- The archive cannot reproduce the final receive-domain `classify_rv_gate` result for
  every historical record. Those legacy durable exit records omit
  `RealizedVolSnapshot.latest_accepted_receive_ms`, and the faulty historical Binance
  adapter never produced the genuine local receive stamps later required by the fixed
  classifier. Final classifier behavior is proved by production-shaped differentials,
  not retroactively inferred from missing historical data. New version-15 records
  carry the optional snapshot receive watermark and effective maximum source age, but
  those additive fields cannot restore data absent from the archived session.
- Alternating cross-venue book clocks fail red before the production change and
  collapse to one evidence key afterward.
- Alternating book, signal, and selection triggers share the receive clock and do not
  oscillate.
- Bolt's internal structurally absent receive-context diagnostic boundary remains
  fail-closed; NT `QuoteTick.ts_init` itself is non-optional. No strategy-clock
  fallback silently changes an evaluation instant.
- Entry, exit, and maker pricing all consume the same ownership rule.
- Independent near-stale entry differentials prove the trigger receive stamp owns
  initial uncertainty pricing, sized fee adjustment, resized fee adjustment,
  log/skip evidence, and submit-linked evidence. Each test places strategy wall time
  beyond the stale boundary while the trigger stamp remains valid, so fixing an
  earlier gate cannot mask a later re-gate.
- After entry evaluation, replacing the pricing state's latest RV snapshot cannot
  change log, skip, or submit evidence: all three must retain the evaluation receipt's
  gate result and admitted snapshot identity.
- A cutoff surface ignores both accepted observations newer than its event-time
  cutoff and eligible-but-unused observations after the final selected grid point.
  A separate regression case uses ascending event times with a larger receive time
  on an earlier contributing event and requires the maximum contributing receive
  timestamp.
- Trimmed-mean and quantile multi-source cases require the watermark to cover every
  ready quorum source that affects readiness or dispersion, including a source whose
  numeric value is not selected into the final aggregate.
- Maker pricing remains available when the RV snapshot is newer than the selected
  reference quote but not newer than the explicit maker evaluation stamp.
- Stale receive age and truly missing receive context stay fail-closed.
- Rejected observations cannot advance the event or receive watermark.
- Restart bootstrap with an open position succeeds at or below 1 MiB and enters existing
  blind recovery above 1 MiB.
- The two approved entry keys record RV gate-category and watermark-presence
  states once per stable-base episode while suppressing raw watermark advances,
  repeated/returning states, and long alternating-state floods. Base changes and the
  existing real episode-end resets permit the states to record again. Other
  dedupe-key and `position_id=None` findings remain report-only under #1354.
- The archived replay reads the existing incident artifact in place; this PR creates
  no new archive upload. Classification and handling of that pre-#883 artifact remain
  with #883/#763.

## Confirmed Separate Reference-Price Clock-Domain Defect

Reference-current-price freshness remains on its existing event-time contract and is
not changed by #1354. Review at exact head `f572791db` confirmed that contract has
the same clock-domain ownership defect, but it is separable from the RV correction:

- `current_reference_pricing_event_ms` selects the maximum event timestamp across the
  selected pricing spot, the reference-current-price observation, and active
  reference state. Those values may originate from independently clocked venues.
- `reference_current_price_stale_at` subtracts each observation's venue event time
  from that combined evaluation value. If the clocks are independent, a fresh input
  can appear old and fail closed as `SpotPriceMissing` or reference-price stale.
- All live configurations use a 2,000 ms age limit. A cross-clock difference of
  2,001 ms changes behavior even when both inputs are receive-fresh. The incident's
  1-347 ms measurement covered a different clock pair and does not bound this path;
  present production frequency remains unmeasured.

The existing defect currently fails closed; it remains tracked in this section for a
separate follow-up and is not changed by the signal lifecycle correction.

The follow-up ownership rule is evaluation receive time compared only with each
input's receive time. Venue event timestamps remain available for same-source
ordering, interval membership, and diagnostics. The follow-up must retain typed
receive timestamps throughout pricing and active state; cover primary, failover,
taker entry, maker selection, source-health/live-window checks, and forced-flat
reference freshness; and correct stale spot evidence currently labeled
`SpotPriceMissing`. No tolerance window, venue exception, or enlarged freshness
limit is acceptable.

This work requires its own issue, branch, design, and PR after #1354. Filing the
issue requires explicit user approval. It is not bundled into this PR, and this
document does not authorize implementation or GitHub state changes.

Maker production wiring is also outside this change: #1354 closes the shared maker
pricing interface contract and its route-level tests without adding a new runtime
caller.

## Sequenced Follow-On Work

This pricing PR does not solve cumulative evidence retention. Follow-on work remains
split by owner:

1. #1275 item 13 segments local decision evidence and teaches recovery readers to
   span immutable segments, removing the single-file 1 MiB restart ceiling.
2. #883 classifies exported identifiers and replaces sensitive plaintext with stable
   HMAC pseudonyms using a key resolved from SSM.
3. #763 uploads only closed, redacted segments to S3, retains local segments on
   upload failure, and deletes local copies only after confirmed upload and recovery
   retention permit it.

The first follow-on is required before the long soak. S3 archival is explicitly
later work and depends on the redaction contract.
