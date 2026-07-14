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
`realized_volatility_surface_id`. Independently, `raw_taker_config` is the only
runtime-mapping layer that resolves that identifier against
`loaded.root.realized_volatility_surfaces`: it distinguishes an absent surfaces
block from a present block missing the configured identifier, rejects either through
the existing binding/startup `Result` path, requires the selected policy age to be
positive, and copies `surface.policy.max_source_age_ms` into required raw strategy
TOML. `parse_config` cannot resolve surface identifiers; it only requires the copied
scalar and rejects zero before constructing the required in-memory
`BinaryOracleEdgeTakerConfig.realized_volatility_max_source_age_ms: u64`.
Root `validate_realized_volatility_surfaces` already rejects a zero policy age as
"must be a positive integer," so this does not change validated loaded-config
behavior. The direct `parse_config` zero rejection is a deliberate new fail-closed
defense for callers that bypass root validation. The config field is added only to
the `binary_oracle_edge_taker_config_fields!` single-source macro as
`realized_volatility_max_source_age_ms: u64 => Integer`; the generated struct,
required-field parser, and allowlist must not be hand-maintained separately.

Every entry, exit, and shared production pricing call reads that stored scalar.
Shared pricing APIs that retain an optional diagnostic boundary receive
`Some(config.realized_volatility_max_source_age_ms)`. The obsolete optional
`StrategyBuildContext::realized_volatility_max_source_age_ms_for_surface` and
`RealizedVolSurfaceRuntime::max_source_age_ms_for_surface` accessors are removed;
the runtime/context remains responsible for RV ingest, subscriptions, and current
snapshots, not strategy-policy lookup. This keeps forced-flat available even when a
valid configured surface has no current snapshot.

Concretely, `taker_pricing_config` receives the required parsed scalar directly and
`runtime_taker_pricing_config` is deleted. Its three callers
(`current_entry_pricing_inputs_for_receive_at`,
`current_fair_probability_up_for_receive_at`, and
`current_scaled_min_edge_bps_at`) use `taker_pricing_config`. Direct policy consumers
in `current_realized_vol_for_gate_at`,
`current_realized_vol_source_for_gate_at`,
`exit_realized_volatility_gate_fields_at`,
`entry_realized_volatility_receipt_at`, and
`record_exit_evaluation_evidence`, plus the pricing test helper, use the required
config scalar rather than either deleted accessor. Delete
`BinaryOracleEdgeTaker::realized_volatility_max_source_age_ms`, both runtime/context
accessors, and `runtime_taker_pricing_config`; the final tree has zero definitions or
calls of those policy wrappers. The distinct strategy classifier wrapper
`classify_realized_vol_gate` remains.

The receive-domain classifier inputs must remain auditable after the in-memory
snapshot changes. Each exit evaluation therefore owns one immutable
`ExitRealizedVolatilityGateReceipt`, captured before any evaluation early return and
transferred unchanged through `ExitSubmissionDecision`. Capture takes one immutable
borrow from `latest_realized_vol_snapshot_for_surface`, invokes free
`classify_rv_gate` exactly once, and immediately builds a compact owned exit-only
projection. The receipt owns only schema/decision-needed values: gate,
presence/as-of/watermark, raw and usable readiness, accepted RV/source, mapped exit
blockers/diagnostics, signed delta, derived fair/uncertainty probabilities,
configured surface/max-age, and optional typed evaluation receive stamp. It must not
own `RealizedVolGateClassification`, a full `RealizedVolSnapshot`, or the full
entry-oriented `RealizedVolatilityEvidenceFields`.

Only schema-owned strings/vectors are copied at this boundary. `ExitEvaluation` is
moved into `ExitSubmissionDecision` rather than cloned, and log/durable builders
borrow the one compact receipt. This removes the current full-snapshot/classification
deep clone and repeated RV queries/locks while preserving immutable evidence.
`classify_realized_vol_gate` remains available for non-receipt diagnostics; the exit
receipt does not call clone-producing `classify_realized_vol_snapshot`.

The decision, log and both exit durable-record builders derive every RV-derived
field that their existing schemas own from that receipt plus the immutable trigger
context.
The receipt's complete value/source/probability projection does not silently widen
either durable schema: decision evidence keeps its existing value/source fields,
while exit-evaluation evidence keeps only the RV fields it already owns plus the two
new numeric classifier inputs below. After capture the builders do not query the current
pricing snapshot, call `classify_realized_vol_gate`, or recompute an RV-derived
probability. `ExitEvaluation` uses the receipt's fair probability to compute hold EV,
so RV-derived decision and evidence fields cannot diverge after the pricing state is
replaced. Non-RV market inputs are not historicalized by this receipt; their
consistency is the existing same-callback atomicity contract.

The two existing readiness projections remain deliberately distinct. Decision
evidence keeps `rv_snapshot_ready = snapshot.ready`; evaluation evidence keeps
`rv_ready = snapshot.ready_realized_vol().is_some()`. Decision evidence additionally
stores the independent optional replay input
`rv_snapshot_has_ready_realized_vol: Option<bool>`, with every new record writing
`Some(snapshot.ready_realized_vol().is_some())`, including `Some(false)` when the
snapshot is absent. The existing decision
`realized_vol` remains gate-filtered output and is not redefined as a replay input.
A raw-ready snapshot carrying a blocker therefore discriminates the projections,
and replacement after receipt capture cannot change any of them. One captured signed
snapshot-as-of-minus-trigger-event delta maps unchanged to evaluation evidence's
signed `rv_as_of_minus_now_ms`. That existing field name is a historical misnomer:
its preserved semantics are snapshot `as_of_ms` minus the trigger's venue-event
timestamp, not wall-clock "now." The same captured delta maps only its positive
component to decision evidence's `rv_future_dating_delta_ms`. The receipt's
evaluation receive stamp, not `exit_eval_now_ms`, is the sole source of serialized
`trigger_ts_init_ms`; a second timestamp read is not permitted.

A configured surface policy is required and is captured at startup, never recovered
inside an exit evaluation. A valid configured surface with no current snapshot
produces `MissingSnapshot`, retains the configured surface identifier and effective
maximum age, records no snapshot watermark or value, and does not prevent an
otherwise valid forced-flat exit. An unknown surface fails binding/startup and can
never become ordinary `MissingSnapshot` or an unbounded
`max_source_age_ms = None` policy. A structurally missing evaluation receive stamp
remains `MissingEvaluationEventTime`; if a snapshot exists, its receive watermark and
all other schema-owned captured fields remain present in evidence.

The receipt keeps the watermark typed as `LocalReceiveMs`; durable/wire conversion is
explicit and asymmetric because the existing record families use different integer
domains. `BoltV3ExitDecisionEvidence` stores
`rv_snapshot_receive_watermark_ms: Option<u64>`,
`rv_max_source_age_ms: Option<u64>`, and the decision-only
`rv_snapshot_has_ready_realized_vol: Option<bool>`. `BoltV3ExitEvaluationEvidence`
stores `rv_snapshot_receive_watermark_ms: Option<i64>` using checked conversion and
`rv_max_source_age_ms: Option<u64>` while retaining its existing `rv_ready: bool`.
All five outbound `u64` absolute-time values in exit-evaluation evidence use
`i64::try_from`: trigger event, trigger init, exit-evaluation now, RV as-of, and the
new receive watermark. `record_exit_evaluation_evidence` remains unit-returning and
non-aborting. After its existing dedupe mark timing, a private `Result` helper builds
the complete record. A field-specific build-conversion error or writer error produces
one `log::error!`, writes no partial record, substitutes no value/`None`, preserves
the submission/exposure/order result, and continues the callback. Mark-before-failure
is retained so identical ticks do not produce repeated errors. This is
fail-loud-and-skip for that evidence record, not propagation into trading.

Manual `BoltV3ExitEvaluationEvidence` deserialization rejects negative
`trigger_ts_init_ms` and `rv_snapshot_receive_watermark_ms` with field-specific serde
errors before durable construction. Omitted, `null`, zero, and `i64::MAX` remain
valid. `encode_exit_evaluation_line` independently rejects manually constructed
negative values, and replay defensively treats them as unreplayable. Negative inbound
validation for the existing event/lifecycle/as-of fields is unchanged and out of
scope. New writes store the representable watermark,
`Some(receipt.max_source_age_ms)`, and decision replay boolean. These inputs remain
optional in version 15 as explicit replay markers; schema version and six-value gate
taxonomy do not change.

The durable proof is record-local, not dependent on retained strategy memory. It
replays from reconstruction inputs and must not consult the stored gate result or
the decision record's gate-filtered `realized_vol`. Decision evidence is replayable
iff `rv_max_source_age_ms = Some(age > 0)` and
`rv_snapshot_has_ready_realized_vol = Some(bool)`. Evaluation evidence is replayable
iff `rv_max_source_age_ms = Some(age > 0)`. Missing markers identify legacy evidence
and are unreplayable. Once those markers are present, `None` snapshot/as-of,
evaluation-receive, or watermark values are legitimate classifier inputs, not legacy
defaults. Decision replay uses the independent readiness boolean; evaluation replay
uses its existing `rv_ready`.

Snapshot presence adds no boolean: decision replay uses
`rv_snapshot_as_of_ms.is_some()` and evaluation replay uses
`rv_as_of_ms.is_some()`. `None` therefore selects `MissingSnapshot`; `Some(0)` is a
present snapshot and proceeds to later precedence checks.

Replay mirrors the free `classify_rv_gate` function exactly, in this normative
precedence order:

1. absent snapshot -> `MissingSnapshot`;
2. absent evaluation receive stamp -> `MissingEvaluationEventTime`;
3. absent snapshot receive watermark -> `RejectedNotReady`;
4. watermark later than evaluation receive -> `RejectedFutureDated`;
5. same-domain age greater than the effective maximum age -> `RejectedStale`;
6. unusable readiness -> `RejectedNotReady`;
7. otherwise -> `Accepted`.

The production strategy wrapper remains named `classify_realized_vol_gate`; it owns
strategy diagnostics around, but does not redefine, the free
`classify_rv_gate` precedence. Record tables cover all six results plus overlapping
states that lock precedence: not-ready plus future, not-ready plus stale, and blocker
plus stale, as well as missing snapshot/evaluation/watermark and a valid zero RV.
Assertions for each record stop at the fields that record's schema owns.

The two entry flood guards preserve their current existing dedupe key without the
newly added RV novelty dimension exactly; this amendment neither shrinks that key nor
retains historical keys. The existing key may itself contain RV diagnostic fields,
whose churn and evidence volume remain unmeasured. Each path
stores only `{ current_existing_key, rv_seen_mask: u16 }`. A key change replaces the
current key and clears the mask. Consequently, returning to an older existing key is a
new adjacent change and emits again, preserving existing behavior.

For a fixed current key, the six gate results crossed with watermark absence/presence
map to bits 0 through 11. In gate order `Accepted`, `MissingSnapshot`,
`MissingEvaluationEventTime`, `RejectedFutureDated`, `RejectedStale`,
`RejectedNotReady`, absence uses the even bit and presence the following odd bit.
The first occurrence of a bit emits and sets it; repeats or oscillations among seen
bits do not. Changing a raw `Some` watermark value does not emit, while changing
watermark presence selects the paired bit and does. The existing admitted-entry and
left-RV-not-ready reset sites clear both current key and mask.

Entry-skip preserves mark-before-swallowed-writer-error behavior. Blocked-snapshot
stages the candidate `{ current_existing_key, rv_seen_mask }` without mutating stored
state, attempts its propagating write, and commits both key and mask only after
success. Tests prove `A recorded -> B write fails -> A remains suppressed -> B
retries and emits`; failed B neither replaces the key nor clears A's mask. RED tests
derive a test-local twelve-bit observed mask from emitted records rather than naming
a nonexistent production field. Tests cover all twelve bits once, more than 100
repeats/oscillations with test-local `count_ones() == 12`, raw-watermark churn, every field of each existing key,
returning to a prior key, both resets, and both writer semantics. This precisely
answers `discussion_r3571669050`: it distinguishes gate diagnoses without retaining
every historical existing-key value. Memory is constant and RV novelty is bounded to
twelve states per current key; evidence-record volume under existing-key churn,
including its RV diagnostic fields, remains unmeasured.

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

The two additive numeric exit fields have a conservative compact-JSON bound of at
most 100 bytes per record, including field names, separators, and maximum-width
integer spellings. The decision-only readiness boolean adds at most 43 bytes, so the
fixed-field bound is at most 143 bytes for a decision record and 100 bytes for an
evaluation record. Even pessimistically charging 143 bytes to every one of the 106
counterfactual records adds at most 15,158 bytes, leaving that fixed-record-count
counterfactual safely below 1 MiB. This bound does not claim a final restart size:
semantic entry states may add records that the old collapsed key suppressed, and
that record-count growth remains unmeasured. The amendment does not change the
existing 1 MiB reader boundary, the #1275 item 13 pre-soak requirement, or the
archived-session arithmetic above. Historical records cannot be backfilled with
watermarks, thresholds, or readiness inputs that they never captured.

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
  carry the optional snapshot receive watermark and effective maximum source age;
  decision records also carry the independent optional usable-readiness input. Those
  additive fields cannot restore data absent from the archived session.
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
- The two approved entry guards preserve each current existing dedupe key without the
  newly added RV novelty dimension and add only a twelve-bit RV category/presence
  mask. That existing key may include RV diagnostic fields. Repeats and raw `Some`
  watermark churn are suppressed; presence changes select another bit; every current
  key-field change clears the mask and emits, including a return to a prior key. The
  existing real reset sites clear key and mask. Other dedupe-key and
  `position_id=None` findings remain report-only under #1354; total evidence volume
  under existing-key churn, including RV diagnostics, remains unmeasured.
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
