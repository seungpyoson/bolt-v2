# Implementation Plan: NT Order Intent Layer

> **Historical implementation record.** The feature has been reconciled by
> later merged slices. Do not execute this plan as a current work queue; use
> `main`, `AGENTS.md`, the current source tree, and the repository verification
> recipes as authority.

**Branch**: `codex/maker-order-proof-clean` | **Date**: 2026-05-20 | **Spec**: `specs/023-nt-order-intent-layer/spec.md`
**Input**: Feature specification from `specs/023-nt-order-intent-layer/spec.md`

## Summary

Build the smallest Bolt-owned order intent layer that converts TOML-derived NT order templates and strategy runtime order inputs into NT `OrderAny`. The design keeps maker and taker on one `OrderFactory` construction path, removes hardcoded tuple policy, keeps submit context outside the shared order-template module, preserves NT submit/risk/execution/adapter ownership, and gates implementation through TDD plus multi-agent review.

## Technical Context

**Language/Version**: Rust, repository toolchain
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml` at rev `7c2aafb30fb143069c915a3f2057bb12174405f6`
**Storage**: TOML config, JSONL evidence, Spec Kit docs
**Testing**: `cargo test`, `cargo fmt -- --check`, `git diff --check`, focused strategy-free checks where available
**Target Platform**: bolt-v3 pure Rust LiveNode path
**Project Type**: Rust trading runtime and config parser
**Performance Goals**: No new hot-path adapter simulation or polling; order template compilation is per-order and bounded
**Constraints**: No hardcoded runtime values, no alternate submit path, no direct secret display, no live submit without approval, no NT lifecycle reimplementation
**Scale/Scope**: Existing bolt-v3 strategy path first. Additional NT order variants, strategies, provider bindings, and adapters require positive tests and evidence before support is claimed.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. NT owns order lifecycle, risk, execution, cache, portfolio, reconciliation, and venue translation.
- Generic core, concrete edges: PASS if the order template is venue-agnostic and provider-specific behavior stays in NT adapters or provider bindings.
- Single path and config-controlled runtime: PASS if maker and taker use one TOML-to-template-to-NT path and no hardcoded tuple whitelist remains.
- Test-first safety gates: PASS only if each production behavior change records red before green.
- Evidence before claims: PASS only if claims cite current branch files, pinned NT files, command output, or exact smoke artifacts.
- Minimal slice discipline: PASS if implementation starts with one behavior slice and does not add venue capability tables or mock exchange worlds.

## Project Structure

### Documentation

```text
specs/023-nt-order-intent-layer/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── order-intent-layer.md
└── tasks.md
```

### Source Code

```text
src/
├── bolt_v3_order_intent.rs
├── strategies/
│   └── binary_oracle_edge_taker/
│       ├── archetype.rs
│       ├── mod.rs
│       └── orders.rs
└── bolt_v3_current_evidence/
    ├── facts.rs
    └── mod.rs

tests/
├── bolt_v3_order_intent.rs
├── bolt_v3_current_evidence_runtime.rs
├── config_parsing.rs
└── bolt_v3_strategy_registration.rs
```

**Structure Decision**: Shared NT order-template mechanics live in `src/bolt_v3_order_intent.rs` and accept only typed NT template fields, typed runtime order inputs, and NT `OrderFactory`. Strategy/archetype modules own strategy economics, position contracts, evidence, admission, submit context, provider bindings, and live support claims.

## Phase 0: Research

Research outputs live in `research.md`.

Required decisions:

- Which order fields are NT order template fields versus strategy position contract fields.
- Which runtime facts NT cannot infer and must come from strategy build inputs.
- Which submit fields are NT submit context rather than order construction.
- Which NT model invariants Bolt must validate before panic-style factory calls for currently enabled variants.
- Which venue claims require adapter source or smoke evidence.

## Phase 1: Design

Design outputs:

- `data-model.md`: entities and ownership boundaries.
- `contracts/order-intent-layer.md`: public behavior contract and non-goals.
- `quickstart.md`: verification commands and proof boundaries.
- `tasks.md`: TDD execution checklist generated from this plan.

## Implementation Rules

- Do not implement outside `tasks.md`.
- For each production behavior change, run one red test before changing production code.
- Use NT `OrderFactory` only for order construction.
- Keep `position_side` in strategy contract logic, not in `NtOrderTemplate`.
- Keep optional `client_id`, optional `position_id`, and submit params outside the order template and pass them only at NT submit.
- Validate only enabled-variant NT model crash-prevention invariants locally, and do not encode venue policy.
- Do not claim live support from source reading or local unit tests.

## Complexity Tracking

No constitution violations accepted.
