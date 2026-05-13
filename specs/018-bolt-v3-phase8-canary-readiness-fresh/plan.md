# Implementation Plan: Bolt-v3 Phase 8 Tiny-capital Canary Machinery

**Branch**: `018-bolt-v3-phase8-canary-readiness-fresh` | **Date**: 2026-05-14 | **Spec**: `specs/018-bolt-v3-phase8-canary-readiness-fresh/spec.md`
**Input**: Feature specification from `specs/018-bolt-v3-phase8-canary-readiness-fresh/spec.md`

## Summary

Build Phase 8 only to implementation readiness: local fail-closed canary preflight, redacted dry/no-submit canary evidence, and an ignored operator harness skeleton that cannot execute live capital unless exact user approval names the head SHA and command. Actual live order remains stopped. Phase 8 is blocked for live action until Phase 7 authenticated no-submit readiness is accepted by the live canary gate and strategy-input safety audit is approved.

## Technical Context

**Language/Version**: Rust 2024 edition in existing crate
**Primary Dependencies**: NautilusTrader Rust API, `serde`, `serde_json`, `rust_decimal`, existing AWS SSM resolver, existing bolt-v3 config loader
**Storage**: Redacted JSON evidence artifact outside tracked secrets; existing decision evidence JSONL and NT runtime capture surfaces remain authoritative
**Testing**: `cargo test`, `cargo clippy`, `cargo fmt`, existing bolt-v3 verifier scripts
**Target Platform**: Local/operator macOS development runtime; production path remains the Rust binary and NT LiveNode
**Project Type**: Rust trading runtime crate and operator test harness
**Performance Goals**: Preflight and dry evidence are operator-gate actions; no hot-path latency target
**Constraints**: No live capital without exact approval; no hardcodes; no dual paths; no deferred-work debt; SSM-only secrets; NT owns lifecycle, reconciliation, cache, adapters, and order machinery
**Scale/Scope**: One tiny canary order maximum, only after explicit approval; local readiness only before approval

## Constitution Check

- **Main source of truth**: PASS. Worktree is fresh from `origin/main@d6f55774c32b71a242dcf78b8292a7f9e537afab`.
- **No stale branch continuation**: PASS. PR #318/#319/#320 are classified as reference-only in `stale-pr-audit.md`.
- **No hardcodes**: PASS for plan. Runtime values must come from TOML/operator approval envelope; tests may use fixtures only.
- **No dual paths**: PASS for plan. Phase 8 consumes existing live canary gate and submit admission; no alternate runner or admission path.
- **NT boundary**: PASS for plan. NT owns lifecycle/order/reconciliation/cache/adapter behavior.
- **SSM-only secrets**: PASS for plan. Operator env vars carry non-secret file paths, hashes, and approval ids only.
- **No live capital**: PASS for plan. Actual live order remains blocked without exact approval.
- **TDD**: PASS for plan. Tasks require red behavior test before each implementation slice.
- **External review**: BLOCKED until this planning branch is clean, pushed, and exact-head checks are available. Direct API reviewer approval is available for this session, but source transmission still waits until review gate is allowed.
- **Speckit skill availability**: BLOCKED/PARTIAL. Listed `$speckit-*` skill paths were absent on disk in this session, so artifacts are produced using repo `.specify` templates and scripts as closest local fallback.

## Project Structure

### Documentation

```text
specs/018-bolt-v3-phase8-canary-readiness-fresh/
├── spec.md
├── checklists/
│   └── requirements.md
├── plan.md
├── research.md
├── stale-pr-audit.md
├── strategy-input-safety-audit.md
├── data-model.md
├── contracts/
│   └── tiny-canary-evidence.md
├── quickstart.md
└── tasks.md
```

### Source Code

```text
src/
├── bolt_v3_live_canary_gate.rs
├── bolt_v3_submit_admission.rs
├── bolt_v3_live_node.rs
├── bolt_v3_decision_evidence.rs
├── bolt_v3_tiny_canary_evidence.rs      # proposed
└── strategies/
    └── eth_chainlink_taker.rs

tests/
├── bolt_v3_tiny_canary_preconditions.rs # proposed
├── bolt_v3_tiny_canary_operator.rs      # proposed ignored harness
├── bolt_v3_live_canary_gate.rs
├── bolt_v3_submit_admission.rs
└── bolt_v3_decision_evidence.rs
```

**Structure Decision**: Add only a tiny evidence module plus local tests/harness. Do not alter NT adapter logic, strategy math, or core runner path until plan review approves exact minimal scope.

## Current-main Evidence

- Fresh Phase 8 worktree baseline: `cargo test --lib` passed with 446 passed, 0 failed, 1 ignored.
- Current main has Phase 6 submit admission: `src/bolt_v3_submit_admission.rs:6`, `:26`, `:42`, `:58`, `:61`.
- Current main arms submit admission after live canary gate and before NT runner: `src/bolt_v3_live_node.rs:354-372`.
- Current main has no Phase 7 no-submit readiness producer files under `src/` or `tests/`; `rg --files src tests ... | rg "no_submit"` returned no local Phase 7 files.
- Strategy submit path records decision evidence before admission and NT submit: `src/strategies/eth_chainlink_taker.rs:2827-2838`; fence in `tests/bolt_v3_decision_evidence.rs:83-96`.
- Live canary gate currently validates only approval and prior no-submit readiness evidence and is read-only: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:1408-1416`.
- `config/live.local.toml` is gitignored and absent in the fresh worktree; tracked config is only example/snapshot.

## Phase 8 Scope Remaining

1. Create and review Phase 8 spec/checklist/plan/tasks.
2. Keep live action blocked until Phase 7 is accepted and no-submit report exists.
3. Implement local fail-closed preflight and dry evidence only after external plan approval.
4. Prepare ignored operator harness only after local tests prove default inert behavior.
5. Stop before real live order pending exact approval, exact SHA, and exact command.

## Risks And Unknowns

- Phase 7 is not on current main in this worktree; Phase 8 live action is blocked by dependency.
- Strategy-input safety audit currently blocks live action due missing approved live config, unresolved Chainlink feed proof for ETH, and unresolved fee/economics contract.
- External review cannot be requested under repo review bar until branch is clean, pushed, and exact-head checks are available.
- `config/live.local.toml` is intentionally gitignored; any operator config audit must use redacted structural checks and never print secrets.

## Stop Conditions

- Any live order requirement without exact user approval.
- Any uncertain Chainlink feed/source semantics.
- Any unresolved strategy math/feed/economics safety audit blocker.
- Any missing Phase 7 no-submit readiness report.
- Any path that bypasses submit admission or direct-calls NT runner from Phase 8 harness.
- Any secret exposure risk.
- Any dirty worktree not understood.
