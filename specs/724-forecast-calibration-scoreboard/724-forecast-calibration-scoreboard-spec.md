# Feature Specification: Forecast Calibration Scoreboard (venue/asset/family-agnostic, offline)

**Feature Branch**: `712-positional-sizing-engine` (design authoring; implementation branches from #724)
**Created**: 2026-06-25
**Status**: Draft — under review
**Tracking**: #724 (parent epic #723). Produces the calibrated-trust signal that the sizing engine (#712) consumes via config; calibration measurement itself is offline.
**Input**: A shared module that grades probabilistic forecasts against realized outcomes and reports calibration. The measuring stick for forecast quality. It is **offline** — a report/artifact, not a live-trading dependency.

## Overview

A forecast (a probability that some outcome occurs) drives entry decisions, but today it is **never graded**: the offline result path records only money (P&L) and an operator-typed winner label — it never measures whether the forecast was *right*. P&L cannot substitute; it conflates forecast skill with fees, sizing, adverse selection, and luck. Calibration — "when the model says 70%, does the outcome happen ~70% of the time?" — is orthogonal and unmeasured. No calibration scorer exists in the codebase or in NautilusTrader.

This module is a **shared, generic helper for Bernoulli forecasts**: its core grades a forecast probability `p` against a realized binary outcome `y ∈ {0,1}` using **proper scoring rules**, and emits standard calibration metrics (reliability curve, Brier score, log loss, per-bin counts). It hard-codes **nothing** about any venue, asset, oracle, or market family — "agnostic" here means the SAME scoring math is reused across many Bernoulli forecasting families (venues, instruments, strategies), each plugging in via a thin forecast-extraction + outcome-derivation adapter. Multi-outcome / multiclass forecasting and its calibration diagnostics are **explicitly out of scope** and require a separate extension specification; a second binary family proves family reuse, not multi-outcome generality.

Beyond the report, this module also emits the one artifact the live sizer depends on: a versioned, machine-verifiable `BandCoverageAttestation` (FR-011) that the positional sizer (#712) loads as config to arm its growth model. That artifact — its schema, statistical validity, eligibility gate, and revocation — is part of this module's contract, not the sizer's.

It is the measuring stick for epic #723: the basis-correction (#725) and vol-freshness (#726) fixes can only be *shown* to help once calibration is measurable.

**It does not feed the live trading path.** Its output informs config and human trust decisions out-of-band; the sizing engine consumes a config-tuned cautious edge, not a live call into this module.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Grade forecasts against realized outcomes (Priority: P1, the core)

The scoreboard joins each recorded forecast to the outcome that actually happened and reports calibration: a reliability curve (binned predicted probability vs realized frequency), Brier score, log loss, and sample counts per bin, over real resolved markets.

**Why this priority**: Without it, the only feedback is P&L, which cannot isolate forecast quality from fees, sizing, and luck. This is the measuring stick the rest of #723 depends on.

**Independent Test**: Feed a set of `(probability, outcome)` pairs with known calibration (e.g., forecasts of 0.7 that resolve true 70% of the time) and assert the reported reliability curve, Brier, and log loss match the analytic values.

**Acceptance Scenarios**:

1. **Given** recorded forecasts joined to realized outcomes, **When** the scoreboard runs, **Then** it emits a reliability curve + Brier + log loss + per-bin counts over the resolved markets.
2. **Given** a forecast with no machine-derivable realized outcome, **When** scoring, **Then** that pair is excluded (not graded against operator free text) and the exclusion is counted.

### User Story 2 - Derived outcome labeling, not operator free text (Priority: P1)

The realized outcome is **derived** from machine-available resolution data wherever it exists, replacing the hand-entered winner label. This derivation is the per-family/venue adapter; the generic scorer never sees venue specifics.

**Why this priority**: Operator-typed outcomes are unauditable and don't scale across instances/venues. A derived outcome is the only trustworthy grading input.

**Independent Test**: Given machine-available resolution for a market, assert the derived outcome matches the resolution and no operator free text is consulted.

**Acceptance Scenarios**:

1. **Given** machine-available resolution, **When** labeling outcomes, **Then** the outcome is derived from it (operator free text is never an outcome source).
2. **Given** a family's resolution adapter, **When** it derives an outcome, **Then** the generic scorer receives only `(probability, binary outcome)` — no venue/asset/oracle identity.

### User Story 3 - Agnostic, shared scorer (Priority: P1)

The scoring core grades `(probability, outcome)` pairs and contains no venue, asset, oracle, or family literal. New forecasting families reuse it unchanged by supplying their own forecast-record and outcome-derivation adapters.

**Why this priority**: This is shared infrastructure consumed by many strategies across instances, venues, and instruments — per the shared-infra mandate. A venue/family literal in the scorer breaks reuse.

**Independent Test**: Grep the scorer for any venue/asset/oracle/family literal (none); register a second family's pairs and assert the scorer runs unchanged.

**Acceptance Scenarios**:

1. **Given** the scorer, **When** reviewed/grepped, **Then** it contains no venue/asset/oracle/family-specific identifier.
2. **Given** a second forecasting family's `(probability, outcome)` pairs, **When** scored, **Then** the same scorer produces a valid report with no code change.

### Edge Cases

- A forecast was persisted but the resolution is not yet machine-available → the pair is pending, excluded from scoring, and counted (never graded against a guessed outcome).
- A forecast did not survive persistence to the scoreboard input → fail loud (the join surfaces the gap), not silently drop.
- A bin has too few samples to be meaningful → report the per-bin count so thin bins are visible; never smooth them away.
- Operator free text disagrees with machine-derived resolution → the machine-derived outcome wins; the discrepancy is recorded.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The scorer core MUST grade Bernoulli forecasts — a forecast probability `p` paired with a realized binary outcome `y ∈ {0,1}` — using **proper scoring rules**, computing a reliability curve (binned predicted probability vs realized frequency), Brier score, log loss, and per-bin sample counts. These MUST be shared, generic helpers, reused unchanged across Bernoulli forecasting families (many venues, instruments, and strategies score through the SAME helpers), with unit tests pinned to known analytic values. Multi-outcome / multiclass forecasting and its calibration diagnostics are explicitly OUT OF SCOPE for this module and require a separate extension specification.
- **FR-002**: The scorer core MUST contain no venue, asset, oracle, cadence, or market-family literal, and MUST be reused across Bernoulli families through a thin per-family adapter rather than branching on a family (agnostic, per the shared-infra mandate). Verified by review/grep.
- **FR-003**: The realized outcome MUST be derived from machine-available resolution data wherever it exists; operator-typed free text MUST NOT be an outcome source. Outcome derivation is a per-family/venue adapter that hands the generic scorer only `(probability, binary outcome)` plus grouping keys.
- **FR-004**: The module MUST join each persisted forecast to its realized outcome. The forecast persistence path MUST be confirmed to carry the forecast to the scoreboard input; a forecast that does not survive persistence MUST fail loud at the join, not be silently dropped.
- **FR-005**: Output MUST be an offline report/artifact. The module MUST introduce NO live-trading-path coupling — it reads persisted decision evidence + resolution data and writes a report; it is never a runtime dependency of admission, sizing, or submit.
- **FR-006**: A pair with no machine-derivable outcome MUST be excluded from scoring and counted (fail-closed: never fabricate or guess an outcome).
- **FR-007** (shared helpers): The scoring math, binning, and report emission MUST be shared helpers reusable by any family; only forecast-record extraction and outcome derivation are per-family adapters. No dual scoring path.
- **FR-008** (config bridge, not live coupling): The bridge from calibration to sizing is the versioned `BandCoverageAttestation` defined in FR-011 — an offline-produced config artifact, never a live call. Promoting an attestation into the active config/policy epoch is an operator+approver step taken out-of-band; this module MUST NOT be invoked live by the sizer, admission, or submit.

**Forecast evidence, evaluation protocol, and the consumed attestation**

- **FR-009** (forecast evidence contract): Each graded forecast MUST be a persisted, immutable `ForecastRecord` carrying at least: a decision id; instrument and outcome-definition identity + version; the point forecast and, where the family produces one, the lower/upper probability band; model id+version and band-method id+version; side; decision horizon; market-segment keys; a decision-time `observed_at`; a `valid_until`; and the resolution join key. The record MUST be immutable and MUST be demonstrably created before its outcome became observable, so a forecast cannot be edited to fit the outcome. A record missing a required field MUST be excluded and counted, never defaulted.
- **FR-010** (evaluation protocol): Calibration evaluation MUST be prequential or use a frozen out-of-sample cutoff — never in-sample fitting. The evaluator MUST account for forecasts that share one terminal event and for overlapping horizons: repeated forecasts on one market MUST NOT be counted as independent observations merely because they are separate rows. Every report MUST state BOTH the raw row count AND the effective independent event count, and any coverage/calibration confidence bound MUST be computed on the effective count. The evaluation cutoff and the input dataset hash MUST be recorded in the report.
- **FR-010A** (band-validity estimand and inference): The eligibility policy MUST define the EXACT statistical property certified for the forecast lower band `p_lower`. It MUST NOT define validity by comparing a single Bernoulli realization with a probability interval — a 0/1 outcome cannot reveal whether the latent true probability lay inside a band, so an "outcome ∈ [p_lower, p_upper]" rule is meaningless and is prohibited. The certified property is one-sided conservatism of `p_lower`, defined precisely as follows. Within every predeclared attestation cell c, after applying the predeclared one-forecast-per-terminal-event selection rule (or a fully specified cluster estimator), define the per-observation residual `d_i = y_i − p_lower_i` (with `y_i ∈ {0,1}`, so `d_i ∈ [−1, 1]`). The certified property is `E[d_i | cell c] ≥ 0` — i.e. `p_lower` is conservative on average within the cell — which is well-defined even when `p_lower_i` varies across observations in the cell. For every attested cell, compute a simultaneous one-sided lower confidence bound `LCB_c` for `E[d_i | cell c]` using the configured, named inference method and multiplicity correction. A cell PASSES only if `LCB_c ≥ 0` AND `n_effective,c ≥ n_min,c`. An attestation MAY be emitted only if EVERY cell in its declared scope passes. The policy MUST bind: the terminal-event clustering key; the deterministic one-forecast-per-event selection rule (a cluster estimator MAY replace it only when its statistic, weighting, variance estimator, and overlapping-horizon handling are fully specified by the policy version); the confidence-bound method and confidence level; the multiplicity correction across cells; the minimum effective independent-event count per cell; and the failure behavior. Point-only records (no band) MAY enter point-calibration reports but MUST NOT support a `BandCoverageAttestation`. (The implementation plan names the launch inference method; see #724 plan C5.)
- **FR-010B** (upper-band conservatism — DEFERRED, out of Phase-1 scope): Short-side RCK sizing uses the upper band `p_upper` as its adverse end, whose conservatism FR-010A does NOT certify; until this requirement is built, short-side RCK is unsupported (#712 FR-012 rejects it). When added, it MUST mirror FR-010A for the upper band: per predeclared cell, residual `u_i = p_upper_i − y_i` (with `y_i ∈ {0,1}`, so `u_i ∈ [−1, 1]`), certified property `E[u_i | cell c] ≥ 0`, a simultaneous one-sided lower confidence bound `ULCB_c ≥ 0` AND `n_effective,c ≥ n_min,c`, emission only if every cell passes; the resulting attestation MUST bind `certified_bound_end = upper` (FR-011) so #712 FR-013 matches it to short-side decisions.
- **FR-011** (BandCoverageAttestation — the artifact #712 consumes): When, and only when, configured eligibility criteria (FR-010A) pass, the module MUST emit a machine-verifiable, versioned `BandCoverageAttestation` binding: model + band-method versions; instrument family/segment, side, and horizon; evaluation cutoff + dataset hash; the certified property (`E[d_i | cell] ≥ 0` one-sided conservatism of `p_lower`, FR-010A); the `certified_bound_end` (= `lower` in Phase 1 — the band end whose conservatism this attestation certifies, which #712 FR-013 matches to the decision's adverse end); per attested cell — the cell statistic, the observed mean residual, and the simultaneous one-sided lower confidence bound `LCB_c`; the confidence method + level; the multiplicity method; the effective event count per cell; eligibility-policy version + pass/fail result; and producer, approver, issuance time, expiry, and revocation identity. It MUST ADDITIONALLY bind, under a canonical authenticated encoding: outcome-space id/version; outcome-definition id/version (the same identity `ForecastRecord` records, so an attestation cannot be reused after the economic outcome definition changes); `ForecastRecord` schema version; evaluation-implementation version; dependence/inference-method version; and attestation schema version. Producer and approver MUST be distinct, independently authenticated roles. No attestation may be emitted when eligibility fails. This attestation is the SOLE artifact the sizing engine (#712 FR-013) loads to arm its growth model; it is offline-produced config, never a live call. It MUST load unchanged into the substrate's prepared policy epoch (#973 FR-050); #712 and #973 MUST require exact matches for every bound field; a stale, mismatched, ineligible, expired, revoked, or tampered attestation MUST be rejected — failing closed to no growth-model arming.

### Key Entities *(include if feature involves data)*

- **ForecastRecord**: the immutable, decision-time forecast-evidence contract of FR-009 (decision id, instrument/outcome-definition versions, point forecast + optional band, model/band-method versions, side, horizon, segment keys, `observed_at`, `valid_until`, resolution join key); produced by a per-family extraction adapter from decision evidence.
- **RealizedOutcome**: a machine-derived outcome (binary first) + its resolution source — produced by a per-family resolution adapter.
- **ScoringRule / BernoulliFamilyAdapter**: the proper scoring rules (Brier, log loss) + reliability binning of the shared Bernoulli scorer core (FR-001), reused across Bernoulli forecasting families via a thin per-family extraction/outcome adapter; multi-outcome scoring is a separate extension spec.
- **BandValidityPolicy**: the predeclared eligibility policy of FR-010A — decision-cell map, the one-sided lower-bound test on the mean residual `E[y − p_lower]` per cell (`LCB_c ≥ 0`), clustering key, one-forecast-per-event selection / fully-specified cluster estimator, confidence method/level, multiplicity correction, minimum effective-event count, and failure behavior — that gates whether a `BandCoverageAttestation` may be emitted.
- **CalibrationReport**: reliability curve + Brier + log loss + per-bin counts + excluded/pending counts + raw row count and effective independent event count + evaluation cutoff + dataset hash; the offline artifact.
- **BandCoverageAttestation**: the versioned, machine-verifiable, eligibility-gated artifact of FR-011 — the SOLE calibration→sizing bridge; loaded by #712 (FR-013) as config and bound into the substrate's prepared epoch (#973 FR-050).
- **OutcomeAdapter / ForecastAdapter**: the per-family seams (binary up/down first) that feed the generic scorer.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The report shows a reliability curve + Brier + log loss + per-bin counts over real resolved markets — proven by a run on resolved data and a unit test against analytic values.
- **SC-002**: Outcome labeling is derived from machine-available resolution wherever it exists; operator free text is never an outcome source — verified by test + review.
- **SC-003**: No live-trading-path coupling — the module is offline-only — verified by review/grep (no import from admission/sizing/submit paths).
- **SC-004**: Agnostic — no venue/asset/oracle/family literal in the scorer; a second family scores with no code change — verified by review/grep.
- **SC-005**: A forecast that fails to survive persistence fails loud at the join; a pair with no derivable outcome is excluded and counted — proven by tests.
- **SC-006** (cross-module compatibility): A `BandCoverageAttestation` emitted by this module loads unchanged into the prepared policy epoch consumed by #712/#973, and a stale, mismatched, ineligible, expired, revoked, or tampered attestation is rejected — proven by a cross-module load/validate test and a tamper/expiry test.
- **SC-007** (statistically valid evaluation): Evaluation is prequential or frozen out-of-sample, and the report states both the raw row count and the effective independent event count, with confidence bounds computed on the effective count — proven by a test in which repeated forecasts on one terminal event do not inflate the independent count.
- **SC-008** (band-validity estimand): Emitting a `BandCoverageAttestation` requires, for every predeclared cell, the simultaneous one-sided lower confidence bound `LCB_c` on the mean residual `E[y_i − p_lower_i | cell]` to be ≥ 0 (with the bound confidence method + multiplicity correction) AND `n_effective ≥ n_min` — NOT a single 0/1 realization tested against an interval, which is rejected by construction — proven by a unit test over known cells and a negative test that the prohibited interval-membership definition cannot gate emission.

## Assumptions

- The forecast is already persisted to decision evidence; this module confirms it survives to the scoreboard input (it does not change the live persistence path).
- Machine-available resolution exists for the first family (the binary up/down taker, via its resolution anchor); other families supply their own resolution adapter.
- This is an offline analytics/research surface (read-only over persisted evidence + resolution), consistent with the research-NT-first principle — not a live runtime module.
- The sizing engine (#712) consumes a config-tuned cautious edge plus, to arm its growth model, the FR-011 `BandCoverageAttestation`; calibration findings tune that config out-of-band. There is no live calibration→sizer dependency — the attestation is loaded as config (and bound into the substrate's prepared epoch), never called at runtime.
- Forecasts on one market are frequently repeated and share one terminal event; statistically valid calibration MUST count effective independent events, not rows (FR-010). The first family's decision evidence already carries (or is extended by FR-009 to carry) the decision-time identity needed to prove a forecast preceded its outcome.
- Forecast-quality fixes (basis #725, vol-freshness #726, correctness guards) are siblings under epic #723; this scoreboard is the measuring stick that proves whether they help.

## References

- Parent epic #723 (forecast robustness & measurement); siblings #725 (basis), #726 (vol freshness), #722 (current-price feed).
- Consumer: `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md` (the sizer consumes a config-tuned cautious edge informed by this module's offline findings).
- Code anchors (at `main`): forecast `fair_probability_up` persisted to decision evidence; the offline result path (`shadow_pnl.rs`) records only P&L + operator-typed `winning_side` today.
