# Research: Decompose Disk-Governance Verifiers

## Decision: Start from merged #375 main and keep #454 isolated

**Rationale**: #375 closed as completed at 2026-05-23T14:52:18Z, and `origin/main` was fetched to `f354efbaa2afc78575e9cc40580cf2b682bd66e6`, the PR #460 merge commit. Branch `codex/454-decompose-disk-governance-verifiers` was created from that exact commit.

**Alternatives considered**: Reusing the #375 worktree or stale local `main` was rejected because repo rules make post-merge `main` authoritative and #454 requires fresh `origin/main`.

## Decision: Characterize current behavior before moving parser code

**Rationale**: Baseline commands passed on the fresh branch:

| Command | Result |
|---|---|
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |

The issue risk is behavior drift between current runtime and static verifiers. Characterization tests preserve current behavior while extraction happens.

**Alternatives considered**: Moving code first and trusting existing suites was rejected because existing suites validate each surface separately and do not prove shared parity.

## Decision: Add one shared command-understanding module

**Rationale**: The duplicated helper families are concrete and source-located:

| Helper family | Runtime verifier | Static workflow verifier |
|---|---:|---:|
| `command_tokens` | `scripts/rust_verification.py:507` | `scripts/verify_ci_workflow_hygiene.py:1215` |
| `shell_command_substitution_at` | `scripts/rust_verification.py:655` | `scripts/verify_ci_workflow_hygiene.py:2339` |
| `python_command_string` | `scripts/rust_verification.py:759` | `scripts/verify_ci_workflow_hygiene.py:1407` |
| `python_inline_command_payloads` | `scripts/rust_verification.py:792` | `scripts/verify_ci_workflow_hygiene.py:1440` |
| `path_name_looks_like_renamed_cargo` | `scripts/rust_verification.py:1626` | `scripts/verify_ci_workflow_hygiene.py:2605` |
| `path_executable_looks_like_cargo` | `scripts/rust_verification.py:1632` | `scripts/verify_ci_workflow_hygiene.py:2609` |
| `path_name_looks_like_renamed_rustc` | `scripts/rust_verification.py:1645` | `scripts/verify_ci_workflow_hygiene.py:2618` |
| `path_executable_looks_like_rustc` | `scripts/rust_verification.py:1649` | `scripts/verify_ci_workflow_hygiene.py:2622` |
| target-routing scan | N/A as a full static policy | `scripts/verify_ci_workflow_hygiene.py:1862` |

**Alternatives considered**: Splitting the oversized test files first was rejected because it reduces line counts but does not address the duplicate parser behavior that can drift.

## Decision: Limit the first extraction to parser/scanner behavior already accepted

**Rationale**: #454 explicitly forbids adding shell edge cases, regex cases, wrapper families, command-prediction behavior, or policy changes. The first shared path should copy/centralize existing behavior and let existing tests plus new parity tests prove no behavior drift.

**Alternatives considered**: Redesigning command parsing into a richer shell model was rejected as out of scope and likely to recreate the edge-case chase #454 warns against.

## Decision: Keep no-mistakes out of this issue unless explicitly requested

**Rationale**: Issue #454 says "Do not use no-mistakes unless explicitly requested by the operator." The required gate is exact-head GitHub CI plus external exact-head review.

**Alternatives considered**: Running no-mistakes by default was rejected because it would violate the issue process contract.
