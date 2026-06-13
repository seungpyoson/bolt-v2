# Implementation Plan: Global Shadow Execution Policy

**Branch**: `codex/621-global-shadow-mode` | **Date**: 2026-06-13 | **Spec**: `specs/621-global-shadow-execution-policy/spec.md`  
**Input**: Follow-up to merged PR #621, making shadow/no-submit behavior global and shared.

## Summary

Move PR #621's strategy-local `parameters.submit_orders` behavior into a root-level execution policy that is constructed once from TOML, carried through `StrategyBuildContext`, and enforced by shared submit/cancel routing helpers. `binary_oracle_edge_taker` remains the first consumer, but it must stop owning the execution-mode field and must stop branching on `self.config.submit_orders`.

## Technical Context

**Language/Version**: Rust, repository toolchain  
**Primary Dependencies**: NautilusTrader Rust crates pinned in `Cargo.toml` at rev `7c2aafb30fb143069c915a3f2057bb12174405f6`  
**Storage**: TOML config, JSONL decision evidence, Spec Kit docs  
**Testing**: Red-green focused Rust tests, local non-compile gates, remote-first compile/test/clippy proof  
**Target Platform**: bolt-v3 pure Rust `LiveNode` path  
**Project Type**: Rust trading runtime and config parser  
**Performance Goals**: No new hot-path polling or adapter simulation; routing decisions are per venue-action and constant time  
**Constraints**: No hardcoded runtime values, no dual submit path, no strategy-owned execution gating, no live submit without approval, no NT lifecycle reimplementation  
**Scale/Scope**: One global runtime policy, one production strategy migration, reusable by future strategies through `StrategyBuildContext`

## Constitution Check

- NT-first thin layer: PASS. NT still owns order lifecycle, risk, execution, cache, portfolio, reconciliation, and adapter translation.
- Generic core, concrete edges: PASS. The new module owns live/shadow routing only and must not import strategy, venue, provider, or market-family modules.
- Single path and config-controlled runtime: PASS. Mode changes are one root TOML field; the old per-strategy field is removed.
- Test-first safety gates: PASS only if every production change starts with a failing focused test.
- Evidence before claims: PASS only if the PR cites current-source evidence, exact tests, and exact-head remote CI after implementation.
- Minimal slice discipline: PASS. This slice globalizes PR #621's execution policy only; no new order variants, venues, or live-readiness claims.

## Project Structure

### Documentation

```text
specs/621-global-shadow-execution-policy/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── order-execution-policy.md
├── review-prompt.md
├── internal-adversarial-review.md
└── tasks.md
```

### Source Code

```text
src/
├── bolt_v3_config.rs
├── bolt_v3_validate.rs
├── bolt_v3_strategy_registration.rs
├── bolt_v3_order_execution.rs
├── bolt_v3_submit_admission.rs
├── bolt_v3_archetypes/
│   └── binary_oracle_edge_taker.rs
└── strategies/
    ├── registry.rs
    └── binary_oracle_edge_taker/
        ├── config.rs
        ├── mod.rs
        └── tests/

tests/
├── config_parsing.rs
├── bolt_v3_strategy_registration.rs
├── bolt_v3_decision_evidence.rs
└── bolt_v3_order_execution.rs

scripts/
├── verify_bolt_v3_schema_current.py
├── test_verify_bolt_v3_schema_current.py
├── verify_bolt_v3_runtime_literals.py
└── test_verify_bolt_v3_runtime_literals.py
```

**Structure Decision**: Create `src/bolt_v3_order_execution.rs` as the shared execution-policy module. Keep order construction in `src/bolt_v3_order_intent.rs`, admission request valuation in `src/bolt_v3_submit_admission.rs`, and strategy economics/state transitions in strategy modules.

## Architecture

### Runtime Config

Add one required field under root `[runtime]`: `order_execution_mode = "live" | "shadow"`.

Remove `submit_orders` from `binary_oracle_edge_taker::ParametersBlock`, all strategy TOML files, fixture TOML, raw runtime mapping, and strategy config structs. `deny_unknown_fields` should reject stale per-strategy `submit_orders`.

### Shared Execution Policy

Add `BoltV3OrderExecutionPolicy` and `BoltV3OrderExecutionMode` in `src/bolt_v3_order_execution.rs`.

The policy provides:

- live versus shadow predicate
- shared submit routing outcome
- shared cancel routing outcome
- shared `SubmitContext` type currently local to `binary_oracle_edge_taker`

The submit helper accepts already-built order intent, already-built admission request, shared decision evidence, shared submit-admission state, and a closure that performs the NT `submit_order(...)` call. In live mode it records evidence, admits, consumes capacity, and runs the closure. In shadow mode it records evidence, evaluates admission without consuming capacity, skips the closure, and returns a typed skipped outcome.

The cancel helper accepts a closure that performs the NT `cancel_order(...)` call. In live mode it runs the closure. In shadow mode it skips the closure and returns a typed skipped outcome.

### Strategy Build Context

Extend `StrategyBuildContext` with `Arc<BoltV3OrderExecutionPolicy>` or an owned cloneable policy. Production construction in `bolt_v3_live_node.rs` derives the policy once from root TOML and passes it to the strategy registry. Tests get explicit constructors or helpers for live and shadow contexts.

### Managed Venue-Action Guard

Move the PR #621 fail-closed guard from strategy-specific config translation to shared validation. If root execution mode is shadow, validation rejects any loaded strategy with:

- `manage_stop = true`
- `manage_gtd_expiry = true`
- `manage_contingent_orders = true`
- non-empty `external_order_claims`

This remains a structural safety rule because these NT-managed features can mutate venue state outside Bolt's shared submit/cancel helpers.

### Boundary Rules

- `src/bolt_v3_order_intent.rs` remains submit/admission/shadow free.
- `src/bolt_v3_order_execution.rs` must not import strategy, provider, venue, or market-family modules.
- `binary_oracle_edge_taker` may still own strategy state transitions and strategy-local signal evidence, but not execution-mode config or admission/cancel gating policy.
- NT remains the only order lifecycle, risk, execution, and adapter owner.

## Implementation Phases

### Phase 0 - Review Gate

1. Commit this plan/spec packet.
2. Run internal adversarial review against `spec.md`, `plan.md`, `research.md`, and `contracts/order-execution-policy.md`.
3. Resolve all internal findings or record explicit disprovals.
4. Only after internal approval, run Gemini, Grok, and Claude adversarial review using `review-prompt.md`.
5. Implementation remains blocked unless all four reviews approve with no unresolved blockers.

### Phase 1 - Config and Context

1. Add failing config tests for root `order_execution_mode`.
2. Add failing tests that stale strategy-local `submit_orders` is rejected.
3. Add the config enum and root TOML field.
4. Build `BoltV3OrderExecutionPolicy` from loaded root config.
5. Thread the policy through `StrategyBuildContext`.

### Phase 2 - Shared Routing

1. Add failing shared-module tests for live submit, shadow submit, live cancel, and shadow cancel outcomes.
2. Add `src/bolt_v3_order_execution.rs`.
3. Move `SubmitContext` out of `binary_oracle_edge_taker` into the shared module.
4. Update `binary_oracle_edge_taker` to call shared routing helpers.

### Phase 3 - Strategy Migration

1. Add failing tests proving no production strategy code reads `submit_orders`.
2. Remove `submit_orders` from strategy config and archetype parameter mapping.
3. Update existing shadow-mode entry, exit, forced-flat, and external-close tests to configure shadow mode through context.
4. Preserve shadow PnL evidence behavior.

### Phase 4 - Managed Action Guard and Docs

1. Add failing validation tests for shadow mode plus each managed NT venue-action knob.
2. Implement shared validation in `bolt_v3_validate.rs`.
3. Update schema docs, fixtures, runtime literal audit, and source-fence/static checks.
4. Run local non-compile gates, commit, push, open or update a draft PR, and use `just verify-remote` for exact-head compile/test proof.

## Review Gate

Implementation must not start until:

- Internal adversarial review approves this packet.
- Gemini adversarial review approves this packet.
- Grok adversarial review approves this packet.
- Claude adversarial review approves this packet.
- Any finding from any reviewer is fixed or explicitly disproved in `internal-adversarial-review.md` or a follow-up review disposition.

## Complexity Tracking

No constitution violations are accepted in this plan.
