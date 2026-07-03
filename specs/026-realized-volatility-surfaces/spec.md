# Realized Volatility Surfaces Specification

## Goal

Build an RV-specific, strategy-agnostic realized-volatility engine that can
consume multiple configured underlying price sources and publish an audit-grade
`RealizedVolSnapshot` for taker pricing and any future strategy that requires
realized volatility.

## Non-Goals

- This does not define implied volatility, option volatility surfaces, strike or
  tenor volatility modeling, or generic market-state infrastructure.
- This does not use outcome-market books, resolution/oracle price-to-beat data,
  or strategy lead-quality policy as RV inputs.
- This does not encode asset-specific or venue-specific defaults in Rust.
- This does not change submit, admission, execution, position sizing, or venue
  adapter policy.

## Terms

- **Realized volatility surface**: A TOML-owned RV input surface keyed by an
  opaque `<surface_id>`. The id is data, not an asset branch.
- **RV source**: A configured observation source keyed by an opaque
  `source_id`. Engine state is keyed by `source_id`, not provider, venue, or
  instrument name.
- **RV observation**: One normalized positive finite price observation with
  source id, source class, sample kind, event timestamp, receive timestamp, and
  canonical units.
- **RV snapshot**: The only runtime output consumed by pricing/strategy code.
  It carries the annualized realized-volatility value, readiness, blockers, and
  per-source evidence.

## Functional Requirements

### RV-001 Explicit RV Naming

All new public names that represent this feature must use
`realized_volatility`, `RealizedVol`, or `RV`. New generic `volatility` names are
rejected unless the name is already pre-existing compatibility surface.

### RV-002 TOML-Owned Surface

Root TOML owns a map:

```toml
[realized_volatility_surfaces.<surface_id>]
canonical_base_asset = "<BASE_ASSET>"
canonical_quote_asset = "<QUOTE_ASSET>"

[realized_volatility_surfaces.<surface_id>.policy]
window_ms = "<CONFIGURED_INTEGER>"
sampling_interval_ms = "<CONFIGURED_INTEGER>"
min_ready_sources = "<CONFIGURED_INTEGER>"
max_source_age_ms = "<CONFIGURED_INTEGER>"
max_inter_sample_gap_ms = "<CONFIGURED_INTEGER>"
min_coverage_ratio = "<CONFIGURED_DECIMAL>"
max_cross_source_dispersion = "<CONFIGURED_DECIMAL>"
seconds_per_annum = "<CONFIGURED_DECIMAL>"
aggregation = "upper_quantile"
upper_quantile = "<CONFIGURED_DECIMAL>"

[[realized_volatility_surfaces.<surface_id>.sources]]
source_id = "<SOURCE_ID>"
data_client_id = "<DATA_CLIENT_ID>"
instrument_id = "<INSTRUMENT_ID>"
source_class = "spot_quote"
sample_kind = "midpoint"
enabled = true
counts_toward_quorum = true
canonical_base_asset = "<BASE_ASSET>"
canonical_quote_asset = "<QUOTE_ASSET>"
```

The strings above are placeholders for operator-owned TOML values, not defaults,
examples, or literal strings. Implemented TOML fields must use their declared
types: integers for `*_ms` and `min_ready_sources`, decimals for ratios and
`seconds_per_annum`, strings for ids/enums, and booleans for `enabled` /
`counts_toward_quorum`. Runtime code must not special-case placeholder text.

### RV-003 One RV Runtime Path

A taker strategy contains exactly one TOML-owned RV selector:

```toml
realized_volatility_surface_id = "<surface_id>"
```

Validation rejects legacy strategy-owned RV knobs as RV inputs. Existing signal
data bindings remain valid for fast-spot pricing, but the strategy must not use
them to warm or compute realized volatility. There must be one runtime source of
truth for RV policy and RV source identity.

### RV-004 Source Identity

Each source must have a unique non-empty `source_id`. The engine stores source
state by `source_id` only. Provider, venue, client, and instrument identifiers
are provenance fields and must not drive engine branching. If multiple source IDs
bind the same data stream, strategy forwarding must fan out each observation to
all matching source IDs so no configured source stays shadowed.

### RV-005 Observation Validation

The RV engine accepts only observations that pass RV-specific validation:

- known configured `source_id`
- source enabled
- positive finite price
- event time must not regress per source
- a same-`event_ts_ms` update with a larger `recv_ts_ms` replaces the prior
  same-event sample for that source and does not create a realized-return
  interval by itself
- a same-`event_ts_ms` update with an equal or lower `recv_ts_ms` is rejected
  and must not overwrite the stored same-event sample
- receive time must be greater than or equal to event time
- event-to-receive lag must be within TOML policy
- sample kind and source class must match the configured source
- quote-currency and canonical-unit policy must match the surface

Invalid observations are rejected into source diagnostics and rejection
counters; unknown `source_id` values are counted separately at engine/snapshot
level because they do not belong to any configured source diagnostic. Rejections
must not mutate a ready source into a low-volatility value.

### RV-006 Fixed-Cadence Sampling

Per-source RV is computed on demand from `snapshot_at(as_of_ms)` using a
configured fixed wall-clock grid. Event-driven update counts must not determine
RV weight. At each grid timestamp, the engine uses last-observation-carried-
forward only when the latest valid source observation at or before that grid
timestamp is no older than `max_source_age_ms`; this includes the latest valid
pre-window observation for the first grid cell when it is still fresh. Otherwise
that grid cell is missing. Missing cells reduce coverage and do not create
synthetic returns. Inter-sample gap is measured across valid grid timestamps,
not across raw event timestamps only. A computed RV of exactly zero is valid for
a sufficiently covered flat source. Readiness requires:

- minimum grid coverage ratio
- maximum inter-sample gap
- maximum source age at decision time
- enough grid prices to compute at least one log return
- explicit `seconds_per_annum` basis

### RV-007 Source-Class Separation

`source_class` and `sample_kind` are part of the source contract. Quote, trade,
mark, and index observations do not share one estimator contract. The initial
runtime contract aggregates only quorum-counting sources with the same
`source_class` and `sample_kind`. Root validation rejects a surface whose enabled
quorum-counting sources mix source classes or sample kinds. Disabled and
non-quorum diagnostic sources may be recorded with different contracts, but they
do not satisfy quorum, class checks, kind checks, aggregation, or dispersion.

### RV-008 Per-Source RV Before Aggregation

RV is computed independently per source. RV must not be computed from a
venue-switching or source-switching composite price, because source switching can
synthesize false returns.

### RV-009 Conservative Aggregation

Aggregation must not publish a low RV when sources disagree. Initial aggregation
uses TOML-owned upper quantile after source-integrity filtering. `upper_quantile`
uses the nearest-rank rule over sorted ready per-source RV values and must be in
`[0.5, 1.0]`, so operator config cannot select below-median aggregation. If
cross-source dispersion exceeds the configured threshold, the snapshot is not
ready and emits `CrossSourceDispersion`. Dispersion is defined as
`(max_ready_source_rv - min_ready_source_rv) / aggregate_rv`. If
`aggregate_rv == 0.0` and every ready source RV is also zero, dispersion is zero.
If `aggregate_rv == 0.0` and any ready source RV is positive, the snapshot
blocks with `CrossSourceDispersion`. A non-finite or negative aggregate is not
ready.

Initial outlier handling has no return winsorization. The engine filters only
invalid observations and stale/misaligned source intervals. A high isolated
return is preserved in that source's per-source RV; if peers disagree beyond the
dispersion threshold, the aggregate blocks rather than removing the high return.

### RV-010 Audit-Grade Snapshot

`RealizedVolSnapshot` must include:

- `surface_id`
- `as_of_ms`
- `annualized_realized_vol_decimal`
- `ready`
- `seconds_per_annum`
- `aggregation`
- `sources_used`
- per-source status, timestamps, coverage, gap, sample count, source class,
  sample kind, and RV
- rejected source reasons
- per-source rejection counters
- unknown source rejection counters keyed by rejected `source_id`
- blockers
- config fingerprint as a stable SHA-256 hash over the canonical serialized
  realized-volatility surface config

Pricing and strategy code consume this snapshot instead of peeking into engine
internals.

The closed reason-label vocabulary used by surface blockers and source
diagnostics is:

- `InvalidConfig`
- `QuorumNotReady`
- `SourceStale`
- `CoverageBelowMinimum`
- `InterSampleGapExceeded`
- `SourceClassMismatch`
- `SampleKindMismatch`
- `CrossSourceDispersion`
- `AnnualizationBasisInvalid`
- `NotWarm`

`RealizedVolSnapshot.blocked_reasons` uses only the surface-level subset:

- `InvalidConfig`
- `QuorumNotReady`
- `SourceClassMismatch`
- `SampleKindMismatch`
- `CrossSourceDispersion`
- `AnnualizationBasisInvalid`

Source-level readiness reasons (`SourceStale`, `CoverageBelowMinimum`,
`InterSampleGapExceeded`, and `NotWarm`) appear only in per-source diagnostics.
When quorum is short, the snapshot-level blocker is `QuorumNotReady` and source
diagnostics explain why individual sources did or did not contribute.

### RV-011 Pricing Boundary

`TakerPricingState` may consume `RealizedVolSnapshot`, but it does not own
multi-source RV source selection, quorum, dispersion, source readiness, or RV
sampling. A missing or not-ready snapshot must block pricing; it must not fall
back to a strategy-owned internal RV estimator.

RV consumers must not validate raw snapshot numeric fields independently. The
engine exposes a centralized valid-RV contract: source computation may create a
finite non-negative RV value, and strategy/pricing/probability/evidence
consumers obtain RV only through a ready-snapshot accessor that verifies
`snapshot.ready`, empty snapshot blockers, and the finite non-negative numeric
contract. This accessor is the only path that treats `0.0` as valid RV for
consumer logic.

### RV-012 Strategy Boundary

Strategy code may subscribe to configured sources and forward normalized
observations. Strategy code must not implement RV quorum, RV dispersion,
readiness, fail-closed RV policy, source-class comparability, or RV aggregation.

### RV-013 Health Diagnostics Separation

Provider health diagnostics are not trading gates by themselves. Trading blocks
only when the realized-volatility engine emits explicit `RealizedVolBlockReason`
values.

### RV-014 Evidence

Entry/exit decision evidence must include the RV snapshot fields needed to prove
which sources contributed, which sources were rejected, why the snapshot was or
was not ready, what annualization and aggregation policy produced the value, and
which config fingerprint governed the snapshot. Decimal evidence fields must use
deterministic non-locale formatting. When a snapshot is blocked, aggregate RV
evidence fields must be absent or empty; source diagnostics and surface blockers
explain the blocked state.

## Acceptance Criteria

- Config validation accepts exactly one TOML-owned surfaced RV path for a taker
  strategy.
- Config validation rejects duplicate source ids, unknown data client ids,
  empty source lists, invalid policy values, and legacy RV knobs.
- Unit tests prove per-source fixed-grid RV, coverage blocking, stale blocking,
  source-class mismatch blocking, upper-quantile aggregation, and
  dispersion blocking.
- Taker pricing tests prove pricing consumes `RealizedVolSnapshot` and no longer
  needs estimator internals for surfaced RV mode.
- Source-fence tests prove strategy code does not own RV quorum, dispersion,
  readiness, fail-closed, or aggregation logic.
- Evidence tests prove admitted and blocked decisions contain audit-grade RV
  snapshot fields.
- Evidence version tests fail when the payload shape changes without a schema
  version update.
