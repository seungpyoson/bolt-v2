# Implementation Plan: #466 Disk-Governance Verifier Decomposition

**Branch**: `goal/466-disk-governance-verifier-decomposition` | **Date**: 2026-05-24 | **Spec**: `specs/466-decompose-disk-governance-verifiers/spec.md`
**Input**: Feature specification from `specs/466-decompose-disk-governance-verifiers/spec.md`

## Summary

#466 completes the remaining disk-governance verifier decomposition after PR #465's cargo scanner extraction. The plan is ledger-first: preserve every #466 item, classify current-main runtime/static behavior, create reviewable PR slices only after characterization evidence, and keep the goal active until the full ledger is resolved and the operator explicitly approves issue closure.

## Technical Context

**Language/Version**: Python 3 standard-library verifier scripts plus Rust repository context; no Rust runtime behavior is planned for the first decomposition slices.  
**Primary Dependencies**: Python standard library, GitHub CLI for issue/PR state, Spec Kit docs, existing `just ci-lint-workflow` lane.  
**Storage**: Markdown evidence/spec artifacts under `specs/466-decompose-disk-governance-verifiers/`; no runtime data storage.  
**Testing**: `python3 scripts/test_command_understanding.py`, `python3 scripts/test_rust_verification_cache_retention.py`, `python3 scripts/test_verify_ci_workflow_hygiene.py`, `python3 -m scripts.test_command_understanding` when import setup changes, targeted `py_compile`, `git diff --check`, `just ci-lint-workflow` when verifier/CI hygiene paths are touched.  
**Target Platform**: Repository verifier/CI hygiene tooling on macOS/Linux-compatible Python standard library surfaces.  
**Project Type**: Internal verifier/decomposition and governance docs; possible Python helper/test refactors.  
**Performance Goals**: Preserve verifier behavior and keep file splits mechanical; no new hot-path runtime work.  
**Constraints**: Fresh `origin/main` only; no stale branch proof; no new shell/cargo/wrapper semantics without explicit operator approval; no implementation before ledger, plan/tasks, and required external plan review; no merge/issue closure without explicit operator approval.  
**Scale/Scope**: Eight #466 ledger items, likely multiple PR slices, final whole-issue verification and external review.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Evidence |
|---|---|---|
| NT-first thin layer | Pass | Verifier decomposition does not rebuild NT runtime, venue, order, cache, portfolio, or adapter surfaces. |
| Generic core, concrete edges | Pass | Work is repo-verifier helper cleanup; no venue or strategy-specific core behavior is added. |
| Single path and config-controlled runtime | Pass | No runtime config/secret path changes. Static verifier cleanup must not introduce alternate secret, build, or runtime paths. |
| Test-first safety gates | Pass | Spec requires characterization/RED evidence before moved or generalized behavior. |
| Evidence before claims | Pass | `evidence.md` is the source of truth; completion requires ledger, local verification, exact-head CI, external review, and operator approval. |
| Minimal slice discipline | Pass | Plan allows one coherent helper family or mechanical split per PR and requires residual scope in every non-final PR. |
| Research and analytics stay NT-first | Pass | Not applicable; no research/backtest/dashboard behavior. |

## Project Structure

### Documentation

```text
specs/466-decompose-disk-governance-verifiers/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── evidence.md
├── quickstart.md
├── contracts/
│   └── ledger-resolution.md
├── checklists/
│   └── requirements.md
└── tasks.md
```

### Source Code

```text
scripts/
├── command_understanding.py
├── rust_verification.py
├── verify_ci_workflow_hygiene.py
├── test_command_understanding.py
├── test_rust_verification_cache_retention.py
└── test_verify_ci_workflow_hygiene.py

AGENTS.md
.specify/feature.json
```

**Structure Decision**: Start with specs/evidence only. Implementation slices may touch only the listed verifier/test/helper files unless the ledger and external review approve a narrower or different file set. File splitting is allowed only when mechanical and behavior-preserving.

## Phase 0 Research

Research output: `specs/466-decompose-disk-governance-verifiers/research.md`.

Required decisions:

- Which ledger items are already proven divergent and must remain local.
- Which ledger items are cleanup candidates after pre-implementation review.
- Which file split boundaries reduce review risk without behavior change.
- Which tests prove direct-script versus module import coverage after test setup cleanup.
- How to record external reviewer approval, failed slots, skipped slots, and operator waivers.

## Phase 1 Design

Design outputs:

- `data-model.md`: ledger, helper family, PR slice, review gate, and verification evidence entities.
- `contracts/ledger-resolution.md`: ledger state, PR slice, review, and completion contract.
- `quickstart.md`: verification commands and proof boundaries.
- `tasks.md`: generated from this plan and spec by `/speckit-tasks`.

## Post-Design Constitution Check

| Principle | Status | Evidence |
|---|---|---|
| NT-first thin layer | Pass | Design artifacts stay in verifier/decomposition scope. |
| Generic core, concrete edges | Pass | No concrete trading edge added. |
| Single path and config-controlled runtime | Pass | No runtime path or secret-source changes. |
| Test-first safety gates | Pass | Tasks require characterization before implementation code. |
| Evidence before claims | Pass | Ledger and review contract define proof for all completion claims. |
| Minimal slice discipline | Pass | Tasks split work by issue ledger item and PR slice. |
| Research and analytics stay NT-first | Pass | Not applicable. |

## Complexity Tracking

No constitution violations accepted.
