# Implementation Plan: Bolt-v3 Phase 9 Comprehensive Audit

**Branch**: `019-bolt-v3-phase9-audit-fresh` | **Date**: 2026-05-14 | **Spec**: `spec.md`
**Input**: Feature specification from `specs/019-bolt-v3-phase9-audit-fresh/spec.md`

## Summary

Create a current-main Phase 9 audit package that records source-backed readiness blockers, cleanup gates, stale-artifact disposition, and external-review protocol. This branch does not edit runtime code, does not push or merge without user approval, and does not run live capital.

## Technical Context

**Language/Version**: Rust crate; docs-only slice
**Primary Dependencies**: NautilusTrader Rust crates, Rust AWS SDK for SSM, existing repo verifiers
**Storage**: Markdown audit artifacts under `specs/019-bolt-v3-phase9-audit-fresh/`
**Testing**: `cargo test --lib`, source scans, debt-marker scans, `git diff --check`, no-mistakes runtime proof
**Target Platform**: Local development repo and future GitHub PR review
**Project Type**: Rust CLI/library with bolt-v3 live-node path
**Performance Goals**: None for docs-only slice
**Constraints**: main is source of truth; no stale branch continuation; no live capital; no hardcoded runtime values; no dual paths; SSM-only secrets; pure Rust runtime; NT owns lifecycle/reconciliation/cache/adapters/orders
**Scale/Scope**: Audit current main and fresh Phase 7/8 residual scope; implementation cleanup requires a later reviewed task

## Constitution Check

*GATE: Must pass before research and re-check before tasks.*

- NT-first thin layer: PASS for this docs-only plan. Any later cleanup must not rebuild NT lifecycle, reconciliation, cache, adapter behavior, or order machinery.
- Generic core, concrete edges: WARNING. Existing status-map evidence still marks provider-specific adapter, secret, and client registration placement as partial.
- Single path and config-controlled runtime: WARNING. Current main has live runner gate and submit admission, but no accepted Phase 7 no-submit readiness on main and no accepted Phase 8 canary path.
- Test-first safety gates: PASS for this plan. Runtime cleanup is blocked until one behavior test per vertical slice exists.
- Evidence before claims: PASS for audit artifacts; BLOCKED for final readiness certification until Phase 7/8 are accepted.
- Minimal slice discipline: PASS. This branch is Phase 9 audit planning only.

Spec-kit note: runtime-listed `speckit-*` skill paths were absent on disk in this session. Fallback used repo `.specify` templates and recorded this as tool availability evidence.

## Current Evidence

- Anchor: Phase 9 worktree `HEAD`, `main`, and `origin/main` all equal `d6f55774c32b71a242dcf78b8292a7f9e537afab`.
- no-mistakes: `/Users/spson/.local/bin/no-mistakes`; version `v1.17.0-6-gc0008cf`; daemon running pid `53732`.
- Baseline test: `cargo test --lib` passed with 446 passed, 0 failed, 1 ignored.
- Phase 7/8 on main: current main still has stale unchecked Phase 7/8 tasks in `specs/001-thin-live-canary-path/tasks.md:87-106`; fresh local Phase 7/8 branches are not accepted main scope.
- Live config: `ls -l config/live.local.toml` returned no such file in this fresh worktree.
- Live canary gate: `src/bolt_v3_live_node.rs:350-364` gates `LiveNode::run` on `[live_canary]` and arms submit admission.
- Strategy submit ordering: `src/strategies/eth_chainlink_taker.rs:2827-2838` records decision evidence, runs submit admission, then calls NT `submit_order`.
- Strategy inputs: `config/live.local.example.toml:132-155` warns the active example is BTC and ETH template must not be uncommented without matching ETH ruleset/reference venues.
- Runtime contract: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:718-724` lists strategy input snapshot, volatility, kurtosis, theta, sizing, and order construction surfaces as runtime contract concerns.
- Status map: `docs/bolt-v3/2026-04-28-source-grounded-status-map.md:101-112` marks order construction, dry-run, execution gate, shadow mode, deploy trust, panic/service policy, tiny live canary, production live trading, provider-leak verifier, and cost/fee facts as missing or partial.
- Live ops: `rg -n "runbook|rollback|on-call|incident response|alert" docs/bolt-v3 docs/postmortems config/live.local.example.toml` found only a reconnect alert threshold and archived rollback mention; current runbook/rollback/on-call evidence is missing.

## Project Structure

### Documentation

```text
specs/019-bolt-v3-phase9-audit-fresh/
├── spec.md
├── checklists/requirements.md
├── plan.md
├── research.md
├── data-model.md
├── contracts/audit-evidence.md
├── quickstart.md
├── audit-report.md
├── ai-slop-cleanup-report.md
├── external-review-phase9-prompt.md
└── tasks.md
```

### Source Code

```text
src/
tests/
docs/
config/
scripts/
```

**Structure Decision**: Phase 9 artifacts live under one spec directory. No runtime files are touched in this planning slice.

## Phase Plan

### Phase 0 - Research

Classify current-main evidence across required audit categories. Output: `research.md` and `audit-report.md`.

### Phase 1 - Contracts

Define audit evidence schema and cleanup decision rules. Output: `data-model.md` and `contracts/audit-evidence.md`.

### Phase 2 - Tasks

Create TDD-oriented task list that blocks implementation until external reviews approve the plan and user approves the next action. Output: `tasks.md`.

### Phase 3 - Review Gate

After user-approved push, run exact-head checks and external reviews. Minimum reviewers for this session: Claude, DeepSeek, GLM. Record findings in `external-review-phase9-disposition.md`.

### Phase 4 - Implementation Gate

No runtime cleanup starts unless Phase 3 produces unanimous non-blocking approval or all blocking findings are fixed/disproved and reviewed.

## Risks

- Final Phase 9 cannot certify readiness while Phase 7/8 are not accepted on main.
- Strategy-input safety remains blocked for live action because live ETH feed, market, venue references, economics, and current config are not source-proven.
- Live ops readiness is incomplete without current runbook, rollback, alerting, and incident response evidence.
- Provider-boundary verifiers are partial; cleanup may be larger than Phase 9 should absorb.

## Stop Conditions

- Dirty worktree not understood.
- Any stale branch artifact treated as accepted scope.
- Any reviewer blocking finding unresolved.
- Any live-order or soak command requested without exact user-approved head/SHA and command.
- Any secret exposure risk.
- Any unresolved Chainlink/feed, strategy math, or NT boundary ambiguity.
