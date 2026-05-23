# Implementation Plan: Decompose Disk-Governance Verifiers

**Branch**: `codex/454-decompose-disk-governance-verifiers` | **Date**: 2026-05-23 | **Spec**: `specs/454-decompose-disk-governance-verifiers/spec.md`
**Input**: Feature specification from `/specs/454-decompose-disk-governance-verifiers/spec.md`

## Summary

#454 reduces maintenance risk created by duplicated command-understanding logic across the runtime Rust verification owner and the static CI/no-mistakes workflow hygiene verifier. The plan is characterization-first: capture current accepted classifications, then mechanically extract shared parser/scanner logic into one repo-local Python module used by both surfaces. The PR must not add new shell semantics, wrapper families, policy behavior, or unrelated verifier redesign.

## Technical Context

**Language/Version**: Python 3 repo tooling and tests; Rust product/runtime remains untouched.  
**Primary Dependencies**: Python standard library only; existing direct Python self-test style. No new dependency is planned.  
**Storage**: Source files and Markdown evidence only; no runtime storage or operator home cleanup.  
**Testing**: TDD with characterization/parity tests, `python3 scripts/test_rust_verification_cache_retention.py`, `python3 scripts/test_verify_ci_workflow_hygiene.py`, `python3 -m py_compile` for touched Python files, and `git diff --check`.  
**Target Platform**: macOS developer workstation for local verification; GitHub Actions Linux runners for exact-head CI.  
**Project Type**: Developer tooling governance; no product runtime behavior.  
**Performance Goals**: Preserve current test practicality; shared parsing must avoid adding subprocess or filesystem work to static classification paths.  
**Constraints**: One #454 PR; no #375 follow-up work; no no-mistakes unless explicitly requested; no new command semantics; no broad verifier redesign; external reviewer slots over 15 minutes are skipped and recorded.  
**Scale/Scope**: Current oversized surfaces are `scripts/rust_verification.py` (2738 lines), `scripts/verify_ci_workflow_hygiene.py` (6175 lines), `scripts/test_rust_verification_cache_retention.py` (3175 lines), and `scripts/test_verify_ci_workflow_hygiene.py` (5102 lines). This slice targets duplicated parser/scanner logic, not every oversized function.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Gate | Status | Evidence |
|---|---|---|
| NT-first thin layer | Pass | #454 touches repo tooling only; no NT adapter/runtime logic is planned. |
| Generic core, concrete edges | Pass | No provider, venue, market, wallet, or strategy branches are added. |
| Single path and config-controlled runtime | Pass | Product runtime config and secret paths are untouched. The shared tooling parser reduces dual paths in verifier code. |
| Test-first safety gates | Pass | Tasks require RED shared-module/parity tests before moving parser code. |
| Evidence before claims | Pass | `evidence.md` records exact base SHA, issue state, baseline commands, line counts, and duplicated helper locations. |
| Minimal slice discipline | Pass | Scope is exactly #454. #375 is closed; no no-mistakes or broad redesign is included. |
| Pure Rust binary / SSM / TOML runtime | Pass | No production Rust binary, SSM, or runtime TOML behavior changes are planned. |

Pre-implementation gate: adversarial plan/spec/tasks review must complete before implementation. Claude/Gemini/GLM/DeepSeek slots may be used; any slot exceeding 15 minutes is recorded as skipped, not approved.

## Project Structure

### Documentation (this feature)

```text
specs/454-decompose-disk-governance-verifiers/
|-- evidence.md
|-- spec.md
|-- plan.md
|-- research.md
|-- data-model.md
|-- quickstart.md
|-- contracts/
|   `-- command-understanding.md
|-- checklists/
|   `-- requirements.md
`-- tasks.md
```

### Source Code (repository root)

```text
scripts/
|-- command_understanding.py                  # proposed shared command/parser module
|-- test_command_understanding.py             # proposed characterization/parity tests
|-- rust_verification.py                      # runtime cache/process verifier client
|-- verify_ci_workflow_hygiene.py             # static workflow/no-mistakes verifier client
|-- test_rust_verification_cache_retention.py # existing runtime verifier tests
`-- test_verify_ci_workflow_hygiene.py        # existing static verifier tests
```

**Structure Decision**: Add one shared Python module under `scripts/` and one focused characterization test file. Existing verifier scripts become clients of that module for the extracted helper families. Product `src/`, CI workflow policy, and #375 files are not in scope unless direct import fallout requires a mechanical path update.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Shared tooling module | Both verifier surfaces need one command-understanding path to prevent drift | Leaving helper copies in each oversized verifier preserves the #454 risk. |

## Phase 0 Research

Output: `research.md`

## Phase 1 Design

Outputs: `data-model.md`, `contracts/command-understanding.md`, `quickstart.md`

## Post-Design Constitution Check

| Gate | Status | Evidence |
|---|---|---|
| NT-first thin layer | Pass | Design changes only Python developer-tooling surfaces. |
| Generic core, concrete edges | Pass | No trading/provider-specific branch is introduced. |
| Single path and config-controlled runtime | Pass | The design consolidates duplicate verifier parser paths while leaving runtime config untouched. |
| Test-first safety gates | Pass | `tasks.md` starts implementation with RED shared-module/parity tests. |
| Evidence before claims | Pass | Evidence map and quickstart name the exact verification commands and current baseline. |
| Minimal slice discipline | Pass | #454 excludes no-mistakes, #375 follow-up, new semantics, and broad redesign. |
