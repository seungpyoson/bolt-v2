# Realized Volatility Surface Root Redesign

> **Historical design record — not current authority or work direction.**
> Current `main`, `AGENTS.md`, and tracked issues are authoritative.

## Status

Implemented on PR #609 after Claude, Gemini, and Grok design review. This
document records the accepted root contract used by the current diff.

## Problem

The current PR removed the old taker-owned `RealizedVolEstimator` and added a
shared `RealizedVolEngine`, but several follow-up fixes were shallow patches.
They addressed reviewer-visible symptoms without re-deriving the engine
contract:

- zero realized volatility was accepted by changing scattered
  `is_positive_finite` filters to `is_non_negative_finite`
- disabled sources were made visible by adding fields to diagnostics and
  evidence
- unknown-source rejections were added to evidence
- evidence schema version and source-integrity hashes were updated
- grid overflow and fingerprint stability were hardened locally

Those fixes preserved valid requirements, but they did not fix the root design
mistake: the engine still conflates per-source diagnostics, per-source current
readiness, historical rejection audit state, and surface-level readiness.

## Non-Negotiable Invariant

Configured source diagnostics must be fully auditable, but surface readiness
must be decided only from current eligible ready contributors and the configured
surface policy.

In particular:

- a configured source can be visible in diagnostics without contributing to the
  aggregate
- a configured source can be currently non-ready without blocking the surface
  when quorum is already satisfied
- historical rejection state is audit data, not current readiness state
- `min_ready_sources` is the quorum contract; it must not degrade into
  all-enabled-sources-must-be-ready
- `RealizedVolBlockReason` may remain a shared reason-label vocabulary, but
  `RealizedVolSnapshot.blocked_reasons` is surface-level only

## Scope Boundary

This redesign owns only realized-volatility surface semantics and their
strategy integration boundary.

In scope:

- shared RV engine state model
- observation acceptance and rejection audit
- source diagnostics
- surface readiness and blockers
- surface snapshot evidence payload
- taker pricing consumption of ready surfaced snapshots
- tests that prove the above contract

Out of scope:

- reintroducing any taker-owned internal RV estimator
- changing execution admissibility, rounding, sizing, venue rules, or submit
  mechanics in strategy code
- adding a runtime RV service shared across multiple strategy instances
- changing production market/token mappings beyond keeping them TOML-owned

## Shallow Patch Revert Policy

The implementation must not mechanically keep the shallow patch mechanics just
because they pass existing tests.

Revert or replace the mechanics from these commits where they conflict with the
root model:

- `28dc4368 fix: accept zero realized volatility snapshots`
- `fd01a680 fix surfaced zero rv strategy paths`
- `1ada0d40 fix: close realized volatility evidence gaps`
- `0c9c253c fix: harden realized volatility surface diagnostics`

Do not revert the requirements they exposed:

- zero RV is valid when the engine is currently ready
- disabled and non-quorum sources remain auditable
- unknown-source rejections remain auditable
- evidence schema reflects real payload changes
- config fingerprints are stable and deterministic
- overflow cannot hang the engine

The redesigned implementation may keep code from those commits only when it
fits the root model directly.

## Root Model

### 1. Observation Audit State

Observation handling records accepted samples and rejection audit counters.

Each configured source state owns:

- immutable source config
- accepted sample deque
- last rejected reason, if any
- rejection counters by reason

Unknown source IDs are counted separately at the engine level.

Unknown source IDs must not be feed-controlled. In the strategy path, market
data is routed through TOML-derived source bindings before it can become a
`RealizedVolObservation`, so source-ID cardinality is bounded by configured
bindings. If a future ingestion path accepts raw external source IDs, it must
add a TOML-owned cardinality bound and overflow counter before exposing those
IDs in evidence.

Accepted observations must not clear rejection counters. Rejection counters are
historical audit totals. However, accepted observations also must not inherit a
historical rejection as a current readiness blocker.

### 2. Current Source Evaluation

At `snapshot_at(as_of_ms)`, every configured source is evaluated into a current
source evaluation.

The evaluation has two distinct pieces:

- `diagnostic`: always emitted for every configured source
- `contribution`: emitted only when the source is currently eligible and ready

Eligibility:

- `enabled == true`
- `counts_toward_quorum == true`
- current source computation returns a non-negative finite RV value
- source class and sample kind match the configured source contract

Current enabled quorum-counting source computation may produce current
source-level non-ready reasons:

- not warm
- source stale
- coverage below minimum
- inter-sample gap exceeded

There is no separate "invalid current source math" diagnostic label in the
initial reason vocabulary. Positive finite price validation, time validation,
and annualization validation must make non-finite per-source RV output
unreachable. If implementation discovers another reachable invalid-math state,
it must extend the closed reason vocabulary, schema docs, and tests before
merge rather than hiding that state under an unrelated label.

Disabled and non-quorum sources are diagnostic-only. They do not produce a
current source block reason and they do not contribute to quorum, aggregation,
class checks, kind checks, or dispersion.

`AnnualizationBasisInvalid` is surface-level policy, not a per-source current
block reason.

Historical rejection reasons are reported in `last_rejected_reason` and
`rejection_counters`, but they are not converted into `block_reason` when the
source currently computes a valid RV.

### 3. Source Diagnostics

Diagnostics are current plus audit:

- source identity and configured class/kind
- `enabled`
- `counts_toward_quorum`
- status:
  - `Ready`: current source computation produced a contribution
  - `Blocked`: source is enabled and quorum-counting, was evaluated for current
    readiness, and currently cannot contribute
  - `DiagnosticOnly`: source is configured but disabled or not quorum-counting
  - `Waiting`: source is enabled and quorum-counting, but there is no accepted
    sample state from which a current RV evaluation can be attempted
- current `annualized_realized_vol_decimal`, if ready
- current `block_reason`, if currently blocked
- historical `last_rejected_reason`
- historical `rejection_counters`
- grid/sample timing and coverage metrics

Evidence labels can preserve existing strings where possible, but internal
state should not overload `Rejected` to mean both "latest observation rejected"
and "source currently cannot contribute."

### 4. Ready Contributions

The surface aggregation pipeline consumes only `ReadyContribution` values:

```rust
struct ReadyContribution {
    source_id: String,
    source_class: RealizedVolSourceClass,
    sample_kind: RealizedVolSampleKind,
    annualized_realized_vol_decimal: ValidRealizedVol,
}
```

Only enabled quorum-counting current-ready sources produce contributions.
Disabled sources, non-quorum sources, unknown sources, stale sources, and
not-warm sources remain visible in diagnostics but do not enter aggregation.

### 5. Surface Blockers

`RealizedVolSnapshot.blocked_reasons` contains only surface-level blockers.
The broader `RealizedVolBlockReason` label vocabulary may still include
source-level diagnostic reasons for schema compatibility and evidence labeling,
but source-level reasons must not be inserted directly into
`snapshot.blocked_reasons`.

Surface blockers:

- `InvalidConfig`
- `AnnualizationBasisInvalid`
- `QuorumNotReady`
- `SourceClassMismatch`
- `SampleKindMismatch`
- `CrossSourceDispersion`

Rules:

- Add `QuorumNotReady` only when `ready_contributions.len() <
  min_ready_sources`.
- When quorum is short, add `QuorumNotReady` only. Do not also copy
  `NotWarm`, `SourceStale`, `CoverageBelowMinimum`, or
  `InterSampleGapExceeded` into surface blockers; diagnostics explain why each
  source did or did not contribute.
- Never add a source's `NotWarm`, `SourceStale`, `CoverageBelowMinimum`, or
  `InterSampleGapExceeded` directly to global blockers when quorum is already
  satisfied.
- Class/kind mismatch checks run only over ready contributions. A non-ready
  source with historical class/kind rejection must not block the surface when
  enough valid same-contract contributions exist.
- `SourceClassMismatch` and `SampleKindMismatch` may be emitted only if ready
  contributions disagree with each other or with an explicit surface-level
  class/kind contract. If validation makes mixed ready contracts unreachable,
  the implementation must prove that with tests instead of retaining dead
  blocker paths.
- The normative initial path is validation-time consistency: every enabled
  quorum-counting source in a surface must share one `source_class` and one
  `sample_kind`. Disabled and non-quorum diagnostic sources may differ because
  they never contribute. The snapshot-level class/kind blockers are
  defense-in-depth for direct engine construction that bypasses root
  validation.
- Dispersion runs only over ready contributions.
- If dispersion compares an aggregate of zero against any positive ready
  source RV, the surface blocks with `CrossSourceDispersion`; mixed zero and
  non-zero ready sources must not silently publish a zero aggregate.
- Aggregate is published only when there is no surface blocker and an aggregate
  can be computed.

### 6. Zero RV Contract

Zero realized volatility is valid only as a current engine output.

Rules:

- source computation accepts `rv >= 0.0`
- snapshot acceptance accepts non-negative finite RV
- pricing, probability, and evidence code consume ready RV from the
  ready-snapshot accessor
- a zero RV snapshot must still be `ready == true`; consumers must not treat a
  manually constructed or blocked `Some(0.0)` as ready unless the snapshot
  passes the surfaced readiness gate

Consumer code should use a single helper or typed wrapper for valid RV instead
of repeating raw numeric predicates.

The source-level numeric wrapper shape is:

```rust
struct ValidRealizedVol(f64);

impl ValidRealizedVol {
    fn new(value: f64) -> Option<Self> {
        if value.is_finite() && value >= 0.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    fn get(self) -> f64 {
        self.0
    }
}
```

The ready-consumer wrapper shape is:

```rust
struct ReadyRealizedVol(ValidRealizedVol);

impl RealizedVolSnapshot {
    fn ready_realized_vol(&self) -> Option<ReadyRealizedVol> {
        if self.ready && self.blocked_reasons.is_empty() {
            self.annualized_realized_vol_decimal
                .and_then(ValidRealizedVol::new)
                .map(ReadyRealizedVol)
        } else {
            None
        }
    }
}
```

The exact type and module can follow existing local style, but the acceptance
rule must be centralized. Engine source computation and ready contributions use
`ValidRealizedVol`; taker pricing, market-family probability inputs, and
strategy evidence assembly consume `ReadyRealizedVol` or an equivalent accessor
that has already checked snapshot readiness. Evidence may still serialize the
raw decimal value after validation.

### 7. Evidence Contract

Evidence records both:

- surface-level decision state (`ready`, aggregate RV, surface blockers,
  sources used, config fingerprint)
- full source diagnostics and unknown-source rejection counters

For blocked snapshots:

- aggregate RV fields are empty
- source diagnostics explain source-level current blockers and historical
  rejections
- `blocked_reasons` names only surface-level blockers

For ready snapshots:

- `blocked_reasons` is empty
- `sources_used` lists ready contributors only
- diagnostics may still include disabled, non-quorum, stale, or not-warm
  sources, but those diagnostics do not imply surface blockage

Evidence schema version must be bumped only when payload shape changes. Public
schema docs and runtime-literal audit must be updated with the same version.

### 8. Strategy Boundary

Strategy code remains intent-only for RV:

- subscribes to TOML-configured RV data sources
- converts quote/trade/index updates into normalized `RealizedVolObservation`
- forwards observations to `RealizedVolEngine`
- refreshes and forwards `RealizedVolSnapshot` to taker pricing
- records evidence from the latest matching snapshot

Strategy code must not implement:

- source quorum
- coverage checks
- staleness checks
- dispersion checks
- aggregation
- source readiness policy
- legacy fallback RV estimation

## Required Tests

### Engine Tests

1. Partial quorum succeeds:
   - two enabled quorum-counting sources
   - `min_ready_sources = 1`
   - source A ready
   - source B not warm/stale
   - snapshot is ready, uses only source A, and still reports source B
     diagnostic

2. Quorum short blocks:
   - two enabled quorum-counting sources
   - `min_ready_sources = 2`
   - source A ready
   - source B not warm/stale
   - snapshot is not ready with `QuorumNotReady`
   - snapshot-level blockers do not include source-level readiness reasons

3. Non-quorum source is diagnostic-only:
   - source A ready quorum contributor
   - source B enabled but `counts_toward_quorum = false`
   - source B ready or blocked
   - snapshot readiness and aggregate are unaffected by source B

4. Disabled source is diagnostic-only:
   - disabled source appears in diagnostics
   - disabled observations increment rejection counters
   - disabled source never contributes and never blocks a satisfied quorum

5. Historical rejection recovery:
   - source receives a class/kind mismatch rejection
   - source later receives enough valid observations to be ready
   - diagnostic preserves `last_rejected_reason`
   - rejection counters are not reset by the accepted recovery observations
   - current `block_reason` is empty
   - surface can become ready

6. Ready diagnostic has no current blocker:
   - every diagnostic with status `Ready` must have current
     `block_reason == None`

7. Zero RV end-to-end at engine:
   - flat valid source computes `Some(0.0)` and ready snapshot
   - `NaN`, positive infinity, negative infinity, and negative RV cannot pass
     the `ValidRealizedVol` contract

8. Overflow cannot hang:
   - `snapshot_at(u64::MAX)` terminates
   - the returned snapshot has deterministic observable state and does not
     publish an invalid aggregate

9. Fingerprint stability:
   - source order does not change fingerprint
   - policy/source field change does change fingerprint

10. Source-level blocker containment:
    - stale, not-warm, coverage, and gap failures appear in diagnostics
    - those source-level reasons never appear directly in
      `snapshot.blocked_reasons`

11. Dispersion with zero aggregate:
    - ready sources containing at least one zero RV and one positive RV do not
      publish a zero aggregate silently
    - the surface blocks with `CrossSourceDispersion`

12. Class/kind mismatch trigger:
    - root validation rejects mixed enabled quorum-counting source contracts
    - direct engine construction that bypasses root validation either blocks
      with `SourceClassMismatch` or `SampleKindMismatch`, or is proven
      unreachable by engine validation

13. Receive-lag recovery:
    - receive-lag or same-event rejection counters are preserved
    - a later valid accepted observation can still become a ready contribution

14. Unknown source cardinality:
    - strategy routing cannot create feed-controlled unknown source IDs
    - any future raw-ID ingestion path must test its TOML-owned cap and
      overflow counter

15. Valid-RV fence:
    - no raw `is_positive_finite`, `is_non_negative_finite`, or `>= 0.0`
      predicates are applied directly to RV values outside the shared
      valid-RV module
    - consumer paths obtain RV through the ready-snapshot accessor, not by
      validating a raw snapshot field themselves

### Pricing and Strategy Tests

1. Ready surfaced zero RV prices successfully.
2. Blocked surfaced snapshot fails closed with no legacy fallback.
3. Strategy entry evidence accepts ready zero RV.
4. Position probability and hold EV accept ready zero RV.
5. Blocked RV evidence includes source diagnostics and surface blockers.
6. Blocked RV evidence leaves aggregate RV fields absent or empty.
7. Invalid engine config leaves no source bindings/subscriptions.
8. Duplicate data-stream bindings fan out one observation to every matching
   configured source ID.

### Schema and Fence Tests

1. Decision evidence schema version matches public schema docs.
2. Runtime-literal audit passes.
3. Payload-shape tests fail when an evidence field is added or removed without
   a schema-version update.
4. Source fence proves strategy code does not own RV policy terms.
5. No references to removed `RealizedVolEstimator` remain outside historical
   docs/spec artifacts explicitly marked stale or superseded.

## Implementation Order After Approval

1. Add failing engine tests for partial quorum and rejection recovery.
2. Refactor engine into explicit source evaluation and ready contribution
   pipeline.
3. Update diagnostics/evidence shape only where required by the root model.
4. Replace scattered RV numeric gates with one valid-RV helper or type.
5. Update strategy tests only after engine semantics are correct.
6. Update schema docs, runtime-literal audit, and source-integrity guard.
7. Verify through CI on the exact PR head.

## External Review Questions

Reviewers should approve or request changes on the design, not the current
implementation.

Focus questions:

1. Does this design correctly separate historical source audit from current
   source readiness?
2. Does this design correctly implement `min_ready_sources` as quorum instead
   of all-source readiness?
3. Are the proposed surface blockers the right level of abstraction, or should
   source-level blockers also appear globally when quorum is satisfied?
4. Does the evidence contract preserve enough auditability for disabled,
   non-quorum, stale, rejected, and unknown sources?
5. Does the zero-RV contract prevent both false rejection and false readiness?
6. Are any of the shallow patch requirements incorrectly discarded?
7. Is there any hidden dual-path or strategy-owned RV policy left in the
   proposed boundary?

Approval means: this design is sound enough to implement, subject to normal
code review after implementation.
