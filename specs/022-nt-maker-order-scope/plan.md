# Implementation Plan: NT-Matched Maker Order Scope

**Branch**: `codex/maker-order-proof-clean` | **Date**: 2026-05-20 | **Spec**: `specs/022-nt-maker-order-scope/spec.md`
**Input**: Feature specification from `specs/022-nt-maker-order-scope/spec.md`

## Summary

Evaluate and, only after review gates, accept or minimally refine NautilusTrader-supported Polymarket maker-order behavior for `binary_oracle_edge_taker`, using TOML-driven NT order fields and the existing NT submit path. The branch already contains a committed candidate implementation; acceptance of that implementation and any further source edits are blocked until end-to-end evidence and adversarial review gates pass.

## Technical Context

**Language/Version**: Rust, repo toolchain as configured by current workspace
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml`; `nautilus-polymarket` at rev `7c2aafb30fb143069c915a3f2057bb12174405f6`
**Storage**: TOML config, docs/spec files, existing test fixtures
**Testing**: `cargo test`, focused Rust integration/unit tests, `cargo fmt -- --check`, `git diff --check`
**Target Platform**: bolt-v3 pure Rust LiveNode path
**Project Type**: Rust trading runtime plus config/strategy tests
**Performance Goals**: No new hot-path polling or adapter simulation; use existing NT submit path
**Constraints**: No hardcoded runtime values, no alternate submit path, no secret display, no live submit without separate approval
**Scale/Scope**: One strategy archetype slice: `binary_oracle_edge_taker` Polymarket maker limit orders

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. Plan uses pinned NT adapter behavior as source of truth.
- Generic core, concrete edges: PASS. Changes must remain inside strategy/archetype/docs/tests, not core runtime registration.
- Single path and config-controlled runtime: PASS if implementation keeps existing TOML to strategy to NT `submit_order` path and avoids hardcoded runtime values.
- Test-first safety gates: PASS only if tasks record red tests before code changes.
- Evidence before claims: PASS only if NT and bolt line evidence plus command evidence are recorded before implementation.
- Minimal slice discipline: PASS. Scope is one maker-order slice; no live readiness claim.

## Project Structure

### Documentation

```text
specs/022-nt-maker-order-scope/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── maker-order-config.md
└── tasks.md
```

### Source Code

```text
src/
├── bolt_v3_archetypes/binary_oracle_edge_taker.rs
└── strategies/binary_oracle_edge_taker.rs

tests/
├── config_parsing.rs
└── bolt_v3_strategy_registration.rs

docs/
└── bolt-v3/2026-04-25-bolt-v3-schema.md
```

**Structure Decision**: Keep the slice bounded to the existing archetype validation, strategy order construction, focused config/runtime tests, and schema docs. No new adapter, no new runtime path.

## Phase 0: Research

Research outputs live in `research.md`.

Required decisions:

- NT-supported maker order combinations.
- Whether GTD is part of the supported NT maker scope and whether bolt-v3 has an approved expiry config contract.
- How NT expects `expire_time` for GTD.
- Where bolt-v3 must validate unsupported combinations.
- Which tests prove bolt behavior and which only prove NT dependency behavior.

## Phase 1: Design

Design outputs:

- `data-model.md`: small entity map for `OrderParams`, `MakerOrderScope`, blocked `GtdExpiryPolicy`, and `ReviewGate`.
- `contracts/maker-order-config.md`: allowed TOML combinations, blocked GTD extension condition, and expected NT order fields.
- `quickstart.md`: verification commands and proof boundaries.

## Implementation Rules

- Do not implement from intuition. Implement only tasks in `tasks.md`.
- Do not keep provisional code unless it satisfies a checked task with evidence.
- Treat commit `97cbf828423578e09a604bf31bdaa91ec3573df3` as a candidate implementation, not as proof that the gated process was followed.
- TDD red must precede each production behavior change.
- External adversarial quorum must be attempted before implementation and audit quorum after implementation.
- If external review is blocked, record provider, command, and block reason in task evidence.

## Complexity Tracking

No constitution violations accepted.
