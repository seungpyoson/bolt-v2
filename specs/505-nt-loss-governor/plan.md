# Implementation Plan: NT-First Loss Governor

**Branch**: `codex/nt-loss-governor-circuit-breaker` | **Date**: 2026-06-01 | **Spec**: `specs/505-nt-loss-governor/spec.md`
**Input**: Feature specification from `specs/505-nt-loss-governor/spec.md`

## Summary

Build a Bolt-owned loss governor that consumes NT-derived loss/equity facts, evaluates admission against configured per-trade, daily, rolling-window, max-drawdown, and freshness policy, and wires that policy into the shared live submit-admission boundary. This slice stops new entry/replace risk before NT submit; it does not execute cancel/flatten side effects.

## Technical Context

**Language/Version**: Rust, repository toolchain (`rust-version = "1.95.0"`)
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml` at rev `6e059dcbb59ac1e582132fc431a581936c216c3c`; existing `rust_decimal`
**Storage**: Existing decision-evidence JSONL includes submit admission outcomes; no new persistence store
**Testing**: `cargo test --locked --lib`, `cargo test --locked --test bolt_v3_submit_admission`, `cargo test --locked --test config_parsing`, `cargo test --locked --test bolt_v3_decision_evidence`, `cargo fmt --check`, `git diff --check`
**Target Platform**: bolt-v3 pure Rust LiveNode path
**Project Type**: Rust trading runtime and shared policy module
**Performance Goals**: Bounded admission evaluation and bounded in-process rolling-window sample retention; no polling, adapter simulation, or venue calls
**Constraints**: No hardcoded runtime thresholds, no alternate account truth, no cancel/flatten side effects, no live protection claim beyond submit-admission halt
**Scale/Scope**: Shared policy module, TOML config binding, live NT event feed, submit-admission enforcement, and focused tests; later slices can add NT-routed halt actions

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. NT owns portfolio/account truth, snapshots, risk engine state, execution, and adapters.
- Generic core, concrete edges: PASS. The governor accepts typed loss/equity facts and does not mention strategy, venue, or market family.
- Single path and config-controlled runtime: PASS if all thresholds are caller-provided policy fields and there are no runtime defaults.
- Test-first safety gates: PASS only if each production behavior change records red before green.
- Evidence before claims: PASS only if NT support and gaps cite current Cargo pin and pinned source paths.
- Minimal slice discipline: PASS if submit/live changes stay limited to loss-governor policy wiring and do not alter unrelated strategy behavior.

## Project Structure

### Documentation

```text
specs/505-nt-loss-governor/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── loss-governor.md
└── tasks.md
```

### Source Code

```text
src/
├── bolt_v3_loss_governor.rs
├── bolt_v3_submit_admission.rs
├── bolt_v3_live_node.rs
├── bolt_v3_config.rs
├── bolt_v3_validate.rs
├── bolt_v3_decision_evidence.rs
├── nt_runtime_capture.rs
└── lib.rs
```

**Structure Decision**: Keep strategy code unchanged. Put policy math in a shared module, wire policy into submit admission, derive live snapshots from NT portfolio/position events in the LiveNode boundary, and reuse existing NT runtime-capture message-bus topics.

## Phase 0: Research

Research outputs live in `research.md`.

Required decisions:

- Which NT sources provide realized PnL, unrealized PnL, total PnL, and equity.
- Whether NT exposes snapshots/events usable as governor inputs.
- Whether NT provides runtime per-trade, daily, rolling loss, max drawdown, or kill-switch policy.
- Which policy and evidence fields Bolt must own.
- Which side effects stay out of scope.

## Phase 1: Design

Design outputs:

- `data-model.md`: policy, snapshot, decision, and halt-reason entities.
- `contracts/loss-governor.md`: public behavior contract and non-goals.
- `quickstart.md`: verification commands and proof boundaries.
- `tasks.md`: TDD execution checklist generated using `$speckit-tasks`.

## Implementation Rules

- Keep implementation aligned with `tasks.md`.
- For each production behavior change, run one red test before changing production code.
- Consume NT-derived snapshots only; do not build independent PnL/account truth.
- Do not add runtime threshold defaults.
- Do not touch `src/strategies/binary_oracle_edge_taker.rs`.
- Do not claim cancel/flatten or NT risk-engine state changes from submit-admission tests.

## Complexity Tracking

No constitution violations accepted.
