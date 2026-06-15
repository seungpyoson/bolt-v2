# Implementation Plan: NT-First Loss Governor

**Branch**: `codex/nt-loss-governor-circuit-breaker` | **Date**: 2026-06-01 | **Spec**: `specs/505-nt-loss-governor/spec.md`
**Input**: Feature specification from `specs/505-nt-loss-governor/spec.md`

## Summary

Build a Bolt-owned loss governor that consumes NT-derived loss/equity facts and evaluates admission against configured per-trade, daily/session, rolling-window, max-drawdown, and freshness policy. PR #507 implements the pure evaluator, positional-sizing core, configured shared submit-admission loss gate, configured NT portfolio/position/account runtime feed, and NT risk-state halt actions. Active market exit is not part of this slice; any later active market-exit path must call NautilusTrader's owned `Trader::market_exit_strategy` primitive directly from a real live boundary.

## Technical Context

**Language/Version**: Rust, repository toolchain (`rust-version = "1.95.0"`)
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml` at rev `6e059dcbb59ac1e582132fc431a581936c216c3c`; existing `rust_decimal`
**Storage**: Existing decision-evidence JSONL includes submit admission outcomes; no new persistence store
**Testing**: `cargo test --locked --lib`, `cargo test --locked --test config_parsing`, `cargo fmt --check`, `git diff --check`, `just source-fence`
**Target Platform**: bolt-v3 pure Rust LiveNode path
**Project Type**: Rust trading runtime and shared policy module
**Performance Goals**: Bounded admission evaluation and bounded in-process rolling-window sample retention; no polling, adapter simulation, or venue calls
**Constraints**: No hardcoded runtime thresholds, no alternate account truth, no Bolt-built cancel/flatten side effects, and no flat-position claim from submit-admission or NT risk-state
**Scale/Scope**: Shared policy module, TOML config binding, capital reservation, NT-derived sizing-state validation, configured submit-admission enforcement, NT portfolio/position/account runtime feed, configured NT risk-state loss-halt actions, a live manual-recovery method, and focused tests; any later active market-exit slice must call NautilusTrader's owned `Trader::market_exit_strategy` primitive directly from a real live boundary, while the external operator clear-to-Active command surface and remaining production-grade position-sizer gaps stay separate follow-ups

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
├── bolt_v3_config.rs
├── bolt_v3_validate.rs
├── bolt_v3_capital_reservation.rs
├── bolt_v3_sizing_state.rs
├── bolt_v3_position_sizer.rs
├── bolt_v3_loss_runtime_feed.rs
└── lib.rs
```

**Structure Decision**: Keep strategy files unchanged. Put policy math, reservation, sizing-state validation, product liability calculation, NT portfolio/account/position feed derivation, and loss-halt action policy in shared modules. Configured active exits remain disabled until they can use a Bolt-owned submit/cancel path.

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
- Do not claim positional-sizer live-path enforcement, cancel/flatten, or NT risk-engine state changes from the current submit-admission/runtime-feed slice.

## Complexity Tracking

No constitution violations accepted.
