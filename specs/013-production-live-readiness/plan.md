# Implementation Plan: Production Live Readiness

**Branch**: `issue-369-production-readiness` | **Date**: 2026-05-18 | **Spec**: `specs/013-production-live-readiness/spec.md`
**Input**: Feature specification from `/specs/013-production-live-readiness/spec.md`

## Summary

Define production-grade Bolt-v3 live-readiness levels beyond Issue #360's one-canary proof. The implementation is a docs-plus-test slice: canonical readiness contract, SpecKit artifacts, contract ledger/status-map links, and a regression test proving the artifacts exist and name the required gates.

## Technical Context

**Language/Version**: Rust, current workspace toolchain
**Primary Dependencies**: NautilusTrader Rust crates, existing Bolt-v3 docs/specs, cargo test
**Storage**: Markdown SpecKit/docs artifacts and redacted evidence package references
**Testing**: `cargo test`, `cargo fmt`, `git diff --check`, no-mistakes status
**Target Platform**: Local/operator production Rust binary and GitHub PR review workflow
**Project Type**: Rust live trading binary over NautilusTrader
**Performance Goals**: N/A for docs/test gate; no runtime hot path change
**Constraints**: no live capital, no secret display, SSM-only evidence references, no hardcoded runtime policy in code, no production-ready claim without evidence
**Scale/Scope**: One Issue #369 readiness-definition slice. Future implementation slices close individual staged live and production live blockers.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. This slice does not add Bolt-owned order lifecycle or reconciliation machinery.
- Generic core: PASS. No provider, venue, market, wallet, or strategy policy branch is added.
- Single path and config-controlled runtime: PASS. No runtime config or secret path changes.
- Test-first safety gates: PASS. Regression test first fails on missing SpecKit artifacts, then passes after artifacts exist.
- Evidence before claims: PASS. Contract explicitly blocks broader claims without exact evidence.
- Minimal slice discipline: PASS. Scope is Issue #369 readiness-definition docs/test only.

## Project Structure

### Documentation (this feature)

```text
specs/013-production-live-readiness/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
├── checklists/
│   └── requirements.md
└── contracts/
    └── production-readiness.md
```

### Source Code

```text
docs/bolt-v3/
├── 2026-04-25-bolt-v3-contract-ledger.md
├── 2026-04-28-source-grounded-status-map.md
└── 2026-05-18-production-readiness-contract.md

tests/
└── bolt_v3_production_readiness_contract.rs
```

**Structure Decision**: Keep the canonical claim-level contract in `docs/bolt-v3/` with SpecKit artifacts under `specs/013-production-live-readiness/`. Use one cargo test as a cheap guard that future edits do not drop Issue #369 readiness gates.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| None | N/A | N/A |

## Phase 0 Research

Output: `research.md`

## Phase 1 Design

Outputs: `data-model.md`, `contracts/production-readiness.md`, `quickstart.md`

## Post-Design Constitution Check

- NT-first thin layer: PASS.
- Generic core: PASS.
- Single path and config-controlled runtime: PASS.
- Test-first safety gates: PASS.
- Evidence before claims: PASS.
- Minimal slice discipline: PASS.
