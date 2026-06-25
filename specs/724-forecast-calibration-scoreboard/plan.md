# Implementation Plan: Forecast Calibration Scoreboard (#724)

**Branch**: `712-positional-sizing-engine` (design authoring; implementation branches from #724) | **Date**: 2026-06-25 | **Spec**: `specs/724-forecast-calibration-scoreboard/spec.md`
**Input**: Feature specification from `specs/724-forecast-calibration-scoreboard/spec.md`. Tracking: #724 (parent epic #723).

## Summary

Build a shared, generic, **offline** calibration scoreboard: it joins each persisted forecast to its machine-derived realized outcome and reports a reliability curve, Brier score, log loss, and per-bin counts. The scoring math is venue/asset/oracle/family-agnostic shared helpers; only forecast-record extraction and outcome derivation are per-family adapters. It is read-only over persisted decision evidence + resolution data and introduces no live-trading-path coupling. The binary up/down taker is the first family; its findings inform the sizing engine's conservatism out-of-band via config.

## Technical Context

**Language/Version**: Rust (edition per workspace) for the offline scorer + report, reusing the read-only research/analytics lane; no live runtime code.
**Primary Dependencies**: Persisted decision evidence (the forecast source) and the family's resolution source (machine-available outcome). NautilusTrader provides no calibration surface (audited: no Brier/log-loss/reliability) — this is greenfield shared math. No new external datastore.
**Storage**: Read-only over persisted decision-evidence records + resolution data; writes an offline report artifact. No mutation of any live path.
**Testing**: `cargo test` — unit tests pinning the scoring helpers to analytic values (Brier/log loss/reliability over known `(p, outcome)` sets), a derived-outcome test, an agnostic-scorer review/grep, and an offline-only (no live-path import) check; `cargo fmt`/`clippy`/`deny` clean. Evidence class per slice below.
**Target Platform**: Offline batch (developer/research box or CI), not the LiveNode.
**Project Type**: Single Rust project — read-only analytics surface (Constitution VII).
**Constraints**: NO HARDCODES (no venue/asset/oracle/family literal in the scorer), NO DUAL PATHS (one scoring path; per-family adapters only), READ-ONLY (no live-trading coupling, no order/PnL/position mutation), GROUP BY CHANGE.
**Scale/Scope**: Many forecasting families across instances/venues/instruments score through the same helpers; minute-cadence binary markets accumulate resolved samples quickly.

## Constitution Check

*GATE: must pass before implementation; re-check after each slice.*

- **I. NT-First Thin Layer** — PASS. NT has no calibration surface (audited); this is bolt-owned offline analytics over NT-derived persisted evidence. It does not rebuild NT execution/lifecycle.
- **II. Generic Core, Concrete Edges** — PASS, and load-bearing. The scorer grades `(probability, outcome)` pairs with no venue/asset/oracle/family literal (FR-002); families plug in via extraction/outcome adapters (FR-003/FR-007).
- **III. Single Path And Config-Controlled Runtime** — PASS. One scoring path; report thresholds/bins are config; the config bridge to the sizer is offline-produced, not a live call (FR-008).
- **IV. Evidence-Driven Verification Gates** — PASS. Offline only; no live arming. Each slice carries an evidence class; scoring helpers are pinned to analytic values.
- **V. Evidence Before Claims** — PASS. Each done-claim maps to a named test or review/grep artifact.
- **VI. Minimal Slice Discipline** — PASS. Slices C0–C4 are independently shippable; each fails closed (never fabricates an outcome).
- **VII. Research/Analytics NT-First, Read-Only** — PASS, central. This is a read-only analytics surface: it reads persisted evidence + resolution and writes a report; it MUST NOT submit/cancel/mutate or define independent PnL/position truth, and MUST NOT become a live submit path (FR-005).

## Architecture — shared scorer, per-family adapters

- **Scoring helpers (shared, generic)**: reliability binning, Brier, log loss, per-bin counts over `(probability, outcome)` pairs. No family/venue identity.
- **ForecastAdapter (per-family)**: extracts persisted forecast records from decision evidence into the generic pair.
- **OutcomeAdapter (per-family)**: derives the realized binary outcome from machine-available resolution (binary up/down first).
- **Report emitter (shared)**: writes the offline calibration artifact.
- **Config bridge (offline)**: an operator may promote a calibration result into a config artifact the sizer loads — produced here, consumed there, never a live call.

## Project Structure

```text
specs/724-forecast-calibration-scoreboard/
├── spec.md   # the calibration spec
└── plan.md   # this file
```

New offline modules sit in the read-only analytics lane (exact paths confirmed at implementation). The generic scorer/report are shared; the binary up/down forecast+outcome adapters are the first family. The current `shadow_pnl.rs` operator-typed `winning_side` is replaced by a derived outcome for grading (P&L recording itself is untouched — no dual path, no live coupling).

**Structure Decision**: One shared agnostic scorer + report + a per-family forecast/outcome adapter pair (binary up/down first). Read-only over persisted evidence + resolution. The sizer relationship is an offline-produced config artifact, never a live dependency.

## Slices (dependency-ordered; each fails closed; evidence class per slice)

- **C0 — Shared scoring helpers.** The generic `(probability, outcome)` pair + reliability binning + Brier + log loss + per-bin counts, with no family literal. *Evidence: unit tests pinned to analytic values (a known-calibration set reproduces its exact Brier/log-loss/reliability); review/grep for no venue/family literal.*
- **C1 — Forecast join.** Read persisted forecasts from decision evidence (binary up/down adapter); confirm the forecast survives persistence to the scoreboard input; fail loud on a missing forecast. *Evidence: join test (forecast present → graded; absent → loud failure, not silent drop).*
- **C2 — Derived outcome labeling.** Per-family resolution adapter derives the realized outcome from machine-available resolution, replacing operator free text; pending/unresolvable pairs excluded and counted. *Evidence: SC-002 test (derived outcome matches resolution; operator text never consulted) + SC-005 exclusion test.*
- **C3 — Report.** Reliability curve + Brier + log loss + per-bin counts as an offline artifact over real resolved markets. *Evidence: SC-001 run + analytic-value unit test.*
- **C4 — Offline + agnostic verification.** No live-trading-path import; a second family's pairs score unchanged. *Evidence: SC-003 (no live-path coupling) + SC-004 (agnostic; second family no code change) — review/grep.*

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|----------|------------|--------------------------------------|
| Separate calibration module (not folded into the sizer) | The sizer must not judge forecast honesty; measurement is a distinct job consumed across many strategies | Folding calibration into the sizer hardcodes one strategy's grading into shared sizing and couples sizing to measurement |
| Offline, no live coupling | Calibration is measured over resolved history; a live dependency would couple admission to a slow batch process | A live calibration gate makes the trading path depend on offline scoring and invites a hidden second strategy authority |
| Derived outcome, not operator free text | Operator labels are unauditable and don't scale across instances/venues | Hand-entered winners can't be trusted as the grading truth and can't generalize |
