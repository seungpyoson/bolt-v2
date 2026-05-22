# Implementation Plan: Quote-Quantity SELL Limit Admission

**Branch**: `codex/452-quote-quantity-sell-admission` | **Date**: 2026-05-23 | **Spec**: `specs/452-quote-quantity-sell-admission/spec.md`
**Input**: Feature specification from `specs/452-quote-quantity-sell-admission/spec.md`

## Summary

Harden Bolt submit-admission notional derivation for compiled quote-quantity SELL Limit and StopLimit orders before shorts or quote-sized exits become reachable. Current Bolt strategy-local math mirrors pinned NautilusTrader risk math: it derives base quantity from the side-adjusted effective price but prices notional at `last_px`. For `bid > limit_price`, that can understate admission notional below quote quantity. The implementation slice must define a conservative, generic admission contract without implementing #451 wrapper extraction unless user approval expands scope.

## Technical Context

**Language/Version**: Rust, repository toolchain
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml` at rev `7c2aafb30fb143069c915a3f2057bb12174405f6`
**Storage**: TOML config, JSONL decision evidence, Spec Kit docs
**Testing**: Focused `cargo test` for strategy admission regression, existing quote-quantity admission tests, `cargo fmt -- --check`, `just clippy`, relevant source-fence checks if shared layers are touched
**Target Platform**: bolt-v3 pure Rust LiveNode path
**Project Type**: Rust trading runtime and config/admission layer
**Performance Goals**: Admission derivation remains per-order and bounded; no new polling or venue simulation
**Constraints**: No hardcoded runtime values, no venue/market/strategy policy in generic layers, no #451 extraction without approval, no live submit
**Scale/Scope**: #452 only. Long-only current behavior remains supported; future shorts and quote-sized exits remain gated.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. Pinned NT risk math is cited; Bolt only hardens its own pre-submit live-canary admission contract.
- Generic core, concrete edges: PASS if any shared helper accepts compiled order/instrument/cache-derived facts and contains no Polymarket, binary-oracle, up/down, or strategy identity.
- Single path and config-controlled runtime: PASS if existing submit-admission path remains single and no alternate submit path is added.
- Test-first safety gates: PASS for planning; execution remains pending until a red regression proves current SELL `bid > limit_price` behavior before production code changes.
- Evidence before claims: PASS. Research includes current main SHA, issue/PR SHAs, Bolt line evidence, and pinned NT line evidence.
- Minimal slice discipline: PASS. #451 remains architecture context only unless explicitly approved.

## Project Structure

### Documentation

```text
specs/452-quote-quantity-sell-admission/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── quote-quantity-admission.md
└── tasks.md
```

### Source Code

```text
src/
├── strategies/
│   └── binary_oracle_edge_taker.rs
├── bolt_v3_submit_admission.rs
└── bolt_v3_order_intent.rs

tests/
└── bolt_v3_submit_admission.rs
```

**Structure Decision**: Start with the strategy-local admission code because #452's current risk is in `src/strategies/binary_oracle_edge_taker.rs`. Extract only a minimal generic admission helper if the red test proves a shared compiled-order helper is necessary. Do not move evidence/admission/submit sequencing into a #451 wrapper in this issue.

## Phase 0: Research

Research output: `specs/452-quote-quantity-sell-admission/research.md`.

Required decisions:

- Decide whether Bolt admission mirrors pinned NT risk exactly or applies a conservative envelope for quote-quantity SELL Limit/StopLimit.
- Classify current reachable paths, latent paths, and future enablement requirements.
- Identify whether a small helper can be shared without implementing #451.
- Define the exact red test path and expected failure.

## Phase 1: Design

Design outputs:

- `data-model.md`: compiled-order admission inputs, notional contract, reachability classification.
- `contracts/quote-quantity-admission.md`: behavior contract and non-goals.
- `quickstart.md`: verification commands and proof boundaries.

## Complexity Tracking

No constitution violations accepted. If implementation requires moving the full evidence/admission/submit sequence out of the strategy, stop and request explicit #451 scope approval.
