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
- Book and signal triggers use NT `ts_init`. Maker pricing receives an explicit
  evaluation stamp from its caller. Selection/local triggers explicitly convert
  their strategy-clock evaluation instant at the timestamp-domain boundary and
  therefore do not fall into `MissingEvaluationEventTime`.
- A signal quote without receive metadata is converted explicitly to the strategy
  clock at the trigger boundary. Production `QuoteTick` handlers provide `ts_init`,
  so this is a defensive boundary conversion, not an implicit pricing fallback.
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

This closes the ownership boundary in function signatures instead of relying on
call-site convention. No tolerance window or fallback comparison is introduced.

## Behavioral Blast Radius

- Entry may price on ticks previously blocked only by cross-venue event ordering.
- Normal exit may compute hold EV on those ticks instead of defaulting to Hold.
- Maker fair value may become available on those ticks.
- Submit-linked entry evidence no longer re-decides RV freshness after an entry has
  been admitted. It records the evaluation's snapshot and therefore cannot veto an
  otherwise valid submission merely because strategy wall time advanced.
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
whole-file limit over a long unattended run, so #1275/#763 rotation and segmented
recovery remain required long-term but are not bundled into this change.

## Verification Contract

- Archived inputs reproduce every current classifier result. The reproducible input
  is the archived session at
  `s3://bolt-deploy-artifacts/archives/bolt-v2/evidence/order-intents-v0111-session-20260711T074342Z.jsonl.gz`;
  the PR report records the replay command, exact code head, counts, and byte totals.
- Alternating cross-venue book clocks fail red before the production change and
  collapse to one evidence key afterward.
- Alternating book, signal, and selection triggers share the receive clock and do not
  oscillate.
- Entry, exit, and maker pricing all consume the same ownership rule.
- Independent near-stale entry differentials prove the trigger receive stamp owns
  initial uncertainty pricing, sized fee adjustment, resized fee adjustment,
  log/skip evidence, and submit-linked evidence. Each test places strategy wall time
  beyond the stale boundary while the trigger stamp remains valid, so fixing an
  earlier gate cannot mask a later re-gate.
- A cutoff surface ignores both accepted observations newer than its event-time
  cutoff and eligible-but-unused observations after the final selected grid point.
  A separate regression case uses ascending event times with a larger receive time
  on an earlier contributing event and requires the maximum contributing receive
  timestamp.
- Maker pricing remains available when the RV snapshot is newer than the selected
  reference quote but not newer than the explicit maker evaluation stamp.
- Stale receive age and truly missing receive context stay fail-closed.
- Rejected observations cannot advance the event or receive watermark.
- Restart bootstrap with an open position succeeds below 1 MiB and enters existing
  blind recovery above 1 MiB.
- Dedupe-key and `position_id=None` findings remain report-only under #1354.

Reference-current-price freshness remains on its existing event-time contract and is
not changed by #1354. Its multi-venue ordering deserves a separate ownership census;
it must not be silently folded into this RV-specific PR. Maker production wiring is
also outside this change: this PR closes the shared maker pricing interface contract
and its route-level tests without adding a new runtime caller.

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
