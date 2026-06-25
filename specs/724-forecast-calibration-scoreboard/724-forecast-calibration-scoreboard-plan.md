# Implementation Plan: Forecast Calibration Scoreboard (#724)

**Branch**: `712-positional-sizing-engine` (design authoring; implementation branches from #724) | **Date**: 2026-06-25 | **Spec**: `specs/724-forecast-calibration-scoreboard/724-forecast-calibration-scoreboard-spec.md`
**Input**: Feature specification from `specs/724-forecast-calibration-scoreboard/724-forecast-calibration-scoreboard-spec.md`. Tracking: #724 (parent epic #723).

## Summary

Build a shared, generic, **offline** calibration scoreboard: it joins each persisted forecast to its machine-derived realized outcome and reports a reliability curve, Brier score, log loss, and per-bin counts. The scoring core is a venue/asset/oracle/family-agnostic shared **Bernoulli** scorer (forecast probability `p` + realized binary outcome `y ∈ {0,1}`); only forecast-record extraction and outcome derivation are per-family adapters, and many Bernoulli families score through the same helpers. Multi-outcome / multiclass scoring is explicitly out of scope and a separate extension spec. It is read-only over persisted decision evidence + resolution data and introduces no live-trading-path coupling. Its findings inform the sizing engine's conservatism out-of-band via config, and — only when a predeclared band-validity policy passes (a one-sided lower-bound test that `p_lower` is conservative per decision cell, NOT a meaningless 0/1-in-interval check) — it emits the versioned, machine-verifiable `BandCoverageAttestation` that the sizer loads to arm its growth model. Calibration evaluation is statistically valid: prequential / frozen out-of-sample, counting effective independent events (not rows).

## Technical Context

**Language/Version**: Rust (edition per workspace) for the offline scorer + report, reusing the read-only research/analytics lane; no live runtime code.
**Primary Dependencies**: Persisted decision evidence (the forecast source) and the family's resolution source (machine-available outcome). NautilusTrader provides no calibration surface (audited: no Brier/log-loss/reliability) — this is greenfield shared math. No new external datastore.
**Storage**: Read-only over persisted decision-evidence records + resolution data; writes an offline report artifact. No mutation of any live path.
**Testing**: `cargo test` — unit tests pinning the scoring helpers to analytic values (Brier/log loss/reliability over known `(p, outcome)` sets), a derived-outcome test, an agnostic-scorer review/grep, an offline-only (no live-path import) check, a cross-module attestation load/validate + tamper/expiry test (SC-006), and a statistical-validity test (effective independent event count vs row count; SC-007); `cargo fmt`/`clippy`/`deny` clean. Evidence class per slice below.
**Target Platform**: Offline batch (developer/research box or CI), not the LiveNode.
**Project Type**: Single Rust project — read-only analytics surface (Constitution VII).
**Constraints**: NO HARDCODES (no venue/asset/oracle/family literal in the scorer), NO DUAL PATHS (one scoring path; per-family adapters only), READ-ONLY (no live-trading coupling, no order/PnL/position mutation), GROUP BY CHANGE.
**Scale/Scope**: Many forecasting families across instances/venues/instruments score through the same helpers; minute-cadence binary markets accumulate resolved samples quickly.

## Constitution Check

*GATE: must pass before implementation; re-check after each slice.*

- **I. NT-First Thin Layer** — PASS. NT has no calibration surface (audited); this is bolt-owned offline analytics over NT-derived persisted evidence. It does not rebuild NT execution/lifecycle.
- **II. Generic Core, Concrete Edges** — PASS, and load-bearing. The Bernoulli scorer core (`p` + binary `y`) carries no venue/asset/oracle/family literal (FR-001/FR-002); many Bernoulli families plug in via thin extraction/outcome adapters (FR-003/FR-007). Multi-outcome generality is out of scope (a separate extension spec), so the claim matches the contracts.
- **III. Single Path And Config-Controlled Runtime** — PASS. One scoring path; report thresholds/bins are config; the config bridge to the sizer is offline-produced, not a live call (FR-008).
- **IV. Evidence-Driven Verification Gates** — PASS. Offline only; no live arming. Each slice carries an evidence class; scoring helpers are pinned to analytic values.
- **V. Evidence Before Claims** — PASS. Each done-claim maps to a named test or review/grep artifact.
- **VI. Minimal Slice Discipline** — PASS. Slices C0–C5 are independently shippable; each fails closed (never fabricates an outcome).
- **VII. Research/Analytics NT-First, Read-Only** — PASS, central. This is a read-only analytics surface: it reads persisted evidence + resolution and writes a report; it MUST NOT submit/cancel/mutate or define independent PnL/position truth, and MUST NOT become a live submit path (FR-005).

## Architecture — shared scorer, per-family adapters

- **Bernoulli scoring helpers (shared, generic)**: reliability binning, Brier, log loss, per-bin counts over `(probability, binary outcome)` pairs. No family/venue identity. Multi-outcome scoring is out of scope.
- **ForecastAdapter (per-family)**: extracts persisted forecast records from decision evidence into the generic Bernoulli pair.
- **OutcomeAdapter (per-family)**: derives the realized binary outcome from machine-available resolution (binary up/down is the first family; the helpers are reused unchanged by other Bernoulli families).
- **Report emitter (shared)**: writes the offline calibration artifact.
- **BandValidityPolicy (offline)**: the predeclared estimand + inference (FR-010A) — decision cells, the per-cell residual mean `E[y − p_lower] ≥ 0` certified via a simultaneous one-sided `LCB_c ≥ 0`, clustering key, one-forecast-per-event selection / fully-specified cluster estimator, confidence method/level, multiplicity correction, minimum effective-event count, failure behavior. It is the gate that decides whether an attestation may be emitted; it explicitly forbids a 0/1-outcome-in-interval definition.
- **Config bridge (offline)**: the versioned, eligibility-gated `BandCoverageAttestation` (FR-011) — produced here only when the BandValidityPolicy passes, binding the full field set (model/band-method/outcome-space/outcome-definition/`ForecastRecord`-schema/eval-impl/inference-method/attestation-schema versions under a canonical authenticated encoding), loaded by the sizer as config and bound into the substrate's prepared epoch, never a live call.

## Project Structure

```text
specs/724-forecast-calibration-scoreboard/
├── 724-forecast-calibration-scoreboard-spec.md   # the calibration spec
└── 724-forecast-calibration-scoreboard-plan.md   # this file
```

New offline modules sit in the read-only analytics lane (exact paths confirmed at implementation). The generic scorer/report are shared; the binary up/down forecast+outcome adapters are the first family. The current `shadow_pnl.rs` operator-typed `winning_side` is replaced by a derived outcome for grading (P&L recording itself is untouched — no dual path, no live coupling).

**Structure Decision**: One shared agnostic scorer + report + a per-family forecast/outcome adapter pair (binary up/down first). Read-only over persisted evidence + resolution. The sizer relationship is an offline-produced config artifact, never a live dependency.

## Slices (dependency-ordered; each fails closed; evidence class per slice)

- **C0 — Shared Bernoulli scoring helpers.** The generic `(probability, binary outcome)` pair + reliability binning + Brier + log loss + per-bin counts, with no family literal; multi-outcome scoring out of scope. *Evidence: unit tests pinned to analytic values (a known-calibration set reproduces its exact Brier/log-loss/reliability); review/grep for no venue/family literal.*
- **C1 — Forecast join + evidence contract.** Read persisted forecasts as the immutable, decision-time `ForecastRecord` (FR-009: decision id, instrument/outcome-definition versions, point + optional band, model/band-method versions, side, horizon, segment keys, `observed_at`, `valid_until`, join key); confirm the forecast survives persistence; fail loud on a missing forecast or required field; prove the record precedes its outcome. *Evidence: join test (present → graded; absent/missing-field → loud failure, not silent drop) + decision-time-identity test.*
- **C2 — Derived outcome labeling.** Per-family resolution adapter derives the realized outcome from machine-available resolution, replacing operator free text; pending/unresolvable pairs excluded and counted. *Evidence: SC-002 test (derived outcome matches resolution; operator text never consulted) + SC-005 exclusion test.*
- **C3 — Report + evaluation protocol.** Reliability curve + Brier + log loss + per-bin counts as an offline artifact over real resolved markets, with prequential / frozen out-of-sample evaluation that reports BOTH raw row count AND effective independent event count and records the evaluation cutoff + dataset hash (FR-010). *Evidence: SC-001 run + analytic-value unit test + SC-007 effective-event-count test (repeated forecasts on one terminal event do not inflate the independent count).*
- **C4 — Offline + agnostic verification.** No live-trading-path import; a second family's pairs score unchanged. *Evidence: SC-003 (no live-path coupling) + SC-004 (agnostic; second family no code change) — review/grep.*
- **C5 — Band-validity estimand + attestation + config bridge.** Implement the predeclared `BandValidityPolicy` (FR-010A): per-cell residual `d_i = y_i − p_lower_i`, certified property `E[d_i | cell] ≥ 0`, a simultaneous one-sided lower confidence bound `LCB_c ≥ 0` plus `n_effective ≥ n_min` per cell, emit only if every cell passes; the prohibited 0/1-outcome-in-interval definition must be unable to gate emission. **Launch inference method (named here, not deferred to the implementer): one deterministic forecast per terminal event; a one-sided lower confidence bound valid for the bounded residual `d_i ∈ [−1, 1]` (e.g. an empirical-Bernstein / Hoeffding one-sided bound); Holm multiplicity correction across the predeclared cells.** Other methods enter only through separately versioned policies. Emit the versioned, machine-verifiable `BandCoverageAttestation` (FR-011) ONLY when that policy passes (producer/approver separation, validity, revocation), binding the full field set including outcome-space and outcome-definition versions under a canonical authenticated encoding; prove it loads unchanged into the #712/#973 prepared epoch with its exact digest bound and that stale/mismatched/ineligible/expired/revoked/tampered attestations are rejected. *Evidence: SC-008 band-validity estimand test (`LCB_c ≥ 0` gate passes on known-conservative cells, fails on an anti-conservative cell; interval-membership definition rejected) + SC-006 cross-module load/validate + tamper/expiry test; eligibility-gate test (no attestation emitted on fail).*

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|----------|------------|--------------------------------------|
| Separate calibration module (not folded into the sizer) | The sizer must not judge forecast honesty; measurement is a distinct job consumed across many strategies | Folding calibration into the sizer hardcodes one strategy's grading into shared sizing and couples sizing to measurement |
| Offline, no live coupling | Calibration is measured over resolved history; a live dependency would couple admission to a slow batch process | A live calibration gate makes the trading path depend on offline scoring and invites a hidden second strategy authority |
| Derived outcome, not operator free text | Operator labels are unauditable and don't scale across instances/venues | Hand-entered winners can't be trusted as the grading truth and can't generalize |
| Versioned `BandCoverageAttestation` as the sole bridge | The sizer needs a precise, verifiable, revocable contract to arm Kelly; a loose "promote a report" bridge is unimplementable and unsafe | An operator free-form config record can't be machine-verified, can't expire/revoke, and lets an ineligible calibration arm growth sizing |
| Effective independent event count, not row count | Frequent forecasts on one market share one terminal event; counting rows fabricates statistical confidence | Treating each row as independent overstates coverage confidence and would arm Kelly on thin evidence |
| Per-cell residual-mean estimand `E[y − p_lower] ≥ 0` via `LCB_c` (FR-010A), not interval-membership | A single 0/1 outcome cannot reveal whether the latent probability lay inside a band; the validatable claim for lower-bound Kelly is that `p_lower` is conservative on average per predeclared cell, which the paired residual `d_i = y_i − p_lower_i` defines even when `p_lower` varies within the cell | Defining "band coverage" as a realized outcome falling in `[p_lower,p_upper]`, or as a single aggregate frequency vs a varying `p_lower`, is ill-defined and would arm Kelly on a bound never actually validated — the single biggest residual production risk |
| Bernoulli-only scope, multi-outcome deferred | The scorer is honestly generic across Bernoulli families; the live use case (binary up/down) is Bernoulli | Claiming outcome-space generality while every contract is a scalar `(p, y∈{0,1})` pair is an overclaim the code can't honor; multiclass needs a distribution/identity contract and its own diagnostics |
