# Feature Specification: Forecast Calibration Scoreboard (venue/asset/family-agnostic, offline)

**Feature Branch**: `712-positional-sizing-engine` (design authoring; implementation branches from #724)
**Created**: 2026-06-25
**Status**: Draft — under review
**Tracking**: #724 (parent epic #723). Produces the calibrated-trust signal that the sizing engine (#712) consumes via config; calibration measurement itself is offline.
**Input**: A shared module that grades probabilistic forecasts against realized outcomes and reports calibration. The measuring stick for forecast quality. It is **offline** — a report/artifact, not a live-trading dependency.

## Overview

A forecast (a probability that some outcome occurs) drives entry decisions, but today it is **never graded**: the offline result path records only money (P&L) and an operator-typed winner label — it never measures whether the forecast was *right*. P&L cannot substitute; it conflates forecast skill with fees, sizing, adverse selection, and luck. Calibration — "when the model says 70%, does the outcome happen ~70% of the time?" — is orthogonal and unmeasured. No calibration scorer exists in the codebase or in NautilusTrader.

This module is a **shared, generic helper**: it grades `(forecast_probability, realized_binary_outcome)` pairs and emits standard calibration metrics (reliability curve, Brier score, log loss, per-bin counts). It hard-codes **nothing** about any venue, asset, oracle, or market family — the binary up/down taker is simply the **first** source of pairs; any forecasting strategy plugs in through a thin outcome-derivation adapter. The scoring math is the same for all.

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

- **FR-001**: The scorer MUST grade `(forecast_probability, realized_binary_outcome)` pairs and compute a reliability curve (binned predicted probability vs realized frequency), Brier score, log loss, and per-bin sample counts. These MUST be shared, generic helpers with unit tests pinned to known analytic values.
- **FR-002**: The scorer MUST contain no venue, asset, oracle, cadence, or market-family literal (agnostic, per the shared-infra mandate). Verified by review/grep.
- **FR-003**: The realized outcome MUST be derived from machine-available resolution data wherever it exists; operator-typed free text MUST NOT be an outcome source. Outcome derivation is a per-family/venue adapter that hands the generic scorer only `(probability, binary outcome)` plus grouping keys.
- **FR-004**: The module MUST join each persisted forecast to its realized outcome. The forecast persistence path MUST be confirmed to carry the forecast to the scoreboard input; a forecast that does not survive persistence MUST fail loud at the join, not be silently dropped.
- **FR-005**: Output MUST be an offline report/artifact. The module MUST introduce NO live-trading-path coupling — it reads persisted decision evidence + resolution data and writes a report; it is never a runtime dependency of admission, sizing, or submit.
- **FR-006**: A pair with no machine-derivable outcome MUST be excluded from scoring and counted (fail-closed: never fabricate or guess an outcome).
- **FR-007** (shared helpers): The scoring math, binning, and report emission MUST be shared helpers reusable by any family; only forecast-record extraction and outcome derivation are per-family adapters. No dual scoring path.
- **FR-008** (config bridge, not live coupling): Calibration findings inform trust **out-of-band** — a calibration result MAY be promoted by an operator into a config artifact (e.g. a per-market calibrated-trust / band-coverage record) that the sizing engine (#712) loads as config. This module MUST NOT be invoked live by the sizer; the bridge is offline-produced config, not a runtime call.

### Key Entities *(include if feature involves data)*

- **ForecastRecord**: a persisted forecast probability + grouping keys (timestamp, market key, family) — agnostic; produced by a per-family extraction adapter from decision evidence.
- **RealizedOutcome**: a machine-derived binary outcome + its resolution source — produced by a per-family resolution adapter.
- **CalibrationReport**: reliability curve + Brier + log loss + per-bin counts + excluded/pending counts; the offline artifact.
- **OutcomeAdapter / ForecastAdapter**: the per-family seams (binary up/down first) that feed the generic scorer.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The report shows a reliability curve + Brier + log loss + per-bin counts over real resolved markets — proven by a run on resolved data and a unit test against analytic values.
- **SC-002**: Outcome labeling is derived from machine-available resolution wherever it exists; operator free text is never an outcome source — verified by test + review.
- **SC-003**: No live-trading-path coupling — the module is offline-only — verified by review/grep (no import from admission/sizing/submit paths).
- **SC-004**: Agnostic — no venue/asset/oracle/family literal in the scorer; a second family scores with no code change — verified by review/grep.
- **SC-005**: A forecast that fails to survive persistence fails loud at the join; a pair with no derivable outcome is excluded and counted — proven by tests.

## Assumptions

- The forecast is already persisted to decision evidence; this module confirms it survives to the scoreboard input (it does not change the live persistence path).
- Machine-available resolution exists for the first family (the binary up/down taker, via its resolution anchor); other families supply their own resolution adapter.
- This is an offline analytics/research surface (read-only over persisted evidence + resolution), consistent with the research-NT-first principle — not a live runtime module.
- The sizing engine (#712) consumes a config-tuned cautious edge; calibration findings tune that config out-of-band. There is no live calibration→sizer dependency.
- Forecast-quality fixes (basis #725, vol-freshness #726, correctness guards) are siblings under epic #723; this scoreboard is the measuring stick that proves whether they help.

## References

- Parent epic #723 (forecast robustness & measurement); siblings #725 (basis), #726 (vol freshness), #722 (current-price feed).
- Consumer: `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md` (the sizer consumes a config-tuned cautious edge informed by this module's offline findings).
- Code anchors (at `main`): forecast `fair_probability_up` persisted to decision evidence; the offline result path (`shadow_pnl.rs`) records only P&L + operator-typed `winning_side` today.
