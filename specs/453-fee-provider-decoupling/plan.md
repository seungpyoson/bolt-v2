# Implementation Plan: Fee-Provider Binding Decoupling

**Branch**: `codex/453-fee-provider-decoupling` | **Date**: 2026-05-23 | **Spec**: `specs/453-fee-provider-decoupling/spec.md`
**Input**: Feature specification from `specs/453-fee-provider-decoupling/spec.md`

## Summary

Move fee-provider construction out of `binary_oracle_edge_taker` archetype runtime registration and behind a generic execution-client/provider capability boundary. The strategy should continue to receive `Arc<dyn FeeProvider>` through `StrategyBuildContext`; concrete Polymarket HTTP, secrets, token-id parsing, and CLOB fee-rate behavior stay in provider modules. No order-intent or admission semantics change in this issue.

## Technical Context

**Language/Version**: Rust, repository toolchain
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml` at rev `7c2aafb30fb143069c915a3f2057bb12174405f6`
**Storage**: TOML config, AWS SSM-resolved secrets, Spec Kit docs
**Testing**: Focused registration/source-fence tests, existing Polymarket fee-provider tests, `cargo fmt -- --check`, `just clippy`, relevant source-fence checks
**Target Platform**: bolt-v3 pure Rust LiveNode path
**Project Type**: Rust trading runtime registration and provider binding
**Performance Goals**: Provider resolution happens during strategy registration only; no new hot-path overhead beyond existing fee warm/cache behavior
**Constraints**: SSM-only secrets, no credential display, no shared-layer provider policy, no #451 extraction, no order-intent behavior change
**Scale/Scope**: #453 only. Existing Polymarket behavior preserved; future non-Polymarket providers become pluggable at the provider binding boundary.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. NT Polymarket fee surfaces and instrument fee fields are cited; Bolt only owns provider selection and cache-facing `FeeProvider`.
- Generic core, concrete edges: PASS if concrete Polymarket construction moves behind a provider binding selected by TOML client config.
- Single path and config-controlled runtime: PASS if strategy registration still builds exactly one `StrategyBuildContext` and no alternate fee source is added.
- Test-first safety gates: PASS only after a red registration/source-fence test proves current direct binding before production code changes.
- Evidence before claims: PASS. Research includes current main SHA, issue/PR SHAs, Bolt line evidence, and pinned NT line evidence.
- Minimal slice discipline: PASS. #451 remains context only and order/admission runtime code is not changed.

## Project Structure

### Documentation

```text
specs/453-fee-provider-decoupling/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── fee-provider-resolution.md
└── tasks.md
```

### Source Code

```text
src/
├── bolt_v3_strategy_registration.rs
├── bolt_v3_archetypes/
│   └── binary_oracle_edge_taker.rs
├── bolt_v3_providers/
│   ├── mod.rs
│   └── polymarket*.rs
└── strategies/
    └── registry.rs

tests/
└── bolt_v3_strategy_registration.rs
```

**Structure Decision**: Add or extend a provider-resolution boundary near provider/runtime registration code. Keep `FeeProvider` as the strategy-facing interface. Keep Polymarket-specific HTTP/secrets/token parsing in `src/bolt_v3_providers/polymarket*`.

## Phase 0: Research

Research output: `specs/453-fee-provider-decoupling/research.md`.

Required decisions:

- Identify the current direct binding path and all fee-provider call sites.
- Identify pinned NT fee surfaces that should be reused instead of rebuilt.
- Choose the smallest generic provider resolver shape.
- Define source-fence invariant for strategy/archetype registration.

## Phase 1: Design

Design outputs:

- `data-model.md`: resolver, provider binding, and strategy build context.
- `contracts/fee-provider-resolution.md`: behavior contract and non-goals.
- `quickstart.md`: verification commands and proof boundaries.

## Complexity Tracking

No constitution violations accepted. If implementation requires config migration or strategy behavior changes, stop and request user approval before coding.
