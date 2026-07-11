# RV Gate Clock-Domain Ownership Design

## Decision

Issue #1354 moves realized-volatility consumption freshness into one pricing-owned
receive-time domain. Venue event timestamps remain inputs to realized-volatility
computation and evidence, but never decide whether another venue may consume a
ready snapshot.

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

`classify_rv_gate` currently compares a surface `as_of_ms` with a consuming trigger's
event timestamp. Both values use `VenueEventMs`, but the surface timestamp is the
maximum of independently clocked spot-source venues while exit book triggers use the
execution venue's clock. The shared wrapper proves only "venue event" versus "local";
it does not prove that two different venues share an ordering domain.

The archived session reproduces the defect through the v0.1.12 classifier with zero
mismatches: 20,398 accepted, 20,263 rejected as future-dated, and 188 missing an
evaluation event time. Future deltas are 1–347 ms, median 20 ms and p95 28 ms.

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

### Binance Spot SBE Timestamp-Ownership Prerequisite

The live BTC signal path uses Binance Spot SBE. In pinned Nautilus Trader
`9e71b2b`, `parse_bbo_event` assigns the Binance event timestamp to both
`QuoteTick.ts_event` and `QuoteTick.ts_init`. Bolt then treats `ts_init` as local
receive time. Consequently, the current BTC signal path can still compare an RV
local-receive watermark with a Binance event clock and preserve the #1354 flap.

The July 11, 2026 review found no correction in the then-current Nautilus Trader
release, upstream `develop`, or fork `develop`; Task 0A records the inspected SHAs
and dates so this external observation remains reproducible. Bolt uses an exact
reviewed commit from `seungpyoson/nautilus_trader`; it does not wait for public-
upstream acceptance or a release. This is a blocking prerequisite for the #1354
ownership change, not a Bolt-side venue exception:

- `BinanceSpotDataClient::handle_ws_message` must call its existing
  `AtomicTime::get_time_ns()` once per decoded SBE message and pass that local
  adapter-initialization timestamp to every data parser reached from the boundary:
  trades, BBO, depth snapshot, and depth diff. This is the adapter's established
  post-decode initialization convention; the change does not claim physical socket-
  frame receipt time or add raw-stream plumbing.
- `ts_event` remains the Binance event timestamp. `ts_init` becomes the actual local
  adapter-initialization timestamp; the two values must be independently testable.
- Bolt consumes the corrected typed `ts_init` without re-stamping, fallback, venue
  branching, or tolerance logic.
- Fork tests with deliberately unequal stamps must cover the production handler and
  all four parser families. Bolt differentials must separately prove that RV ingest
  derives `as_of_ms` from `ts_event` and `latest_accepted_receive_ms` from `ts_init`,
  and that signal-trigger classification/evidence follows `ts_init`.
- The fork PR targets `pin/6be5a50-sbe-schema-3-5` at exact base
  `9e71b2b1305a66945ba07f0aba2d1eb63208263d`, discloses its public parser API change,
  and is reviewed and verified at its own immutable SHA. A dedicated Bolt pin-slice
  PR then updates every governed NT revision reference, preserves all fork-only
  commits, registers and source-fences the Binance SBE timestamp boundary, and proves
  the dependency contract. RV consumer behavior remains in #1354 because the RV-
  ingest differential's watermark types and the signal differential's receive-domain
  gate semantics are not present on the pin slice's base.
- The implementation already present at `2b83f512a` is provisional. Further #1354
  behavior work is frozen until the fork and pin prerequisites land; completed work
  is revalidated after merging the landed `main` pin into the branch rather than
  reverted or rewritten to manufacture RED.

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

Giving every production trigger receive-domain context leaves four archived
exit-evaluation transitions: two `exit_hold` entries and two
`position_interval_ended` entries. Together with v0.1.12 blocked-snapshot dedupe and
all preserved action/lifecycle evidence, the captured session counterfactual is
199,023 bytes. The open-position window contains 47,551 bytes over 939.354 seconds,
an observed amortized 182,236 bytes/hour; it is a per-position transition cost, not a
continuing steady-state writer. A fresh file remains below 1 MiB for the planned first
open-position restart. Repeated legitimate position lifecycles can still exceed the
whole-file limit over a long unattended run, so #1275 item 13 segmented recovery
remains a pre-soak requirement but is not bundled into this change. #763 S3 archival
remains later and depends on #883 redaction; it is not a soak blocker.

## Verification Contract

- Archived inputs reproduce every current classifier result. The reproducible input
  is the archived session at
  `s3://bolt-deploy-artifacts/archives/bolt-v2/evidence/order-intents-v0111-session-20260711T074342Z.jsonl.gz`;
  the PR report records the replay command, exact code head, counts, and byte totals.
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
- Restart bootstrap with an open position succeeds below 1 MiB and enters existing
  blind recovery above 1 MiB.
- Dedupe-key and `position_id=None` findings remain report-only under #1354.
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

The follow-up ownership rule is evaluation receive time compared only with each
input's receive time. Venue event timestamps remain available for same-source
ordering, interval membership, and diagnostics. The follow-up must retain typed
receive timestamps throughout pricing and active state; cover primary, failover,
taker entry, maker selection, source-health/live-window checks, and forced-flat
reference freshness; and correct stale spot evidence currently labeled
`SpotPriceMissing`. No tolerance window, venue exception, or enlarged freshness
limit is acceptable.

This work requires its own issue, branch, design, and PR after #1354. Filing the
issue requires explicit user approval. It is not added to the four already approved
PRs, and this document does not authorize implementation or GitHub state changes.

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
