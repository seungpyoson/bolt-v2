# Implementation Plan: #464 Cargo Scanner Helper Decomposition

**Branch**: `codex/464-verifier-decomposition` | **Date**: 2026-05-24 | **Spec**: `specs/464-decompose-disk-governance-verifiers/spec.md`
**Input**: Issue #464 follow-up after merged PR #461

## Summary

Continue disk-governance verifier decomposition with one bounded slice: extract the cargo scanner helper family from `scripts/rust_verification.py` and `scripts/verify_ci_workflow_hygiene.py` into `scripts/command_understanding.py`. The slice is limited to pure Cargo argument scanning helpers that are source-equivalent or behavior-equivalent under representative characterization: cargo global option skipping, cargo subcommand discovery with static start-offset support, cargo-nextest subcommand discovery, and target-routing scan-boundary trimming. Full target-routing policy, static environment prefix detection, runtime refusal payloads, command tokenization, shell substitutions, renamed executable detection, wrapper handling, oversized-file splitting, and test `sys.path` cleanup remain out of scope.

## Technical Context

**Language/Version**: Python 3.11+ compatible standard library code
**Primary Dependencies**: Existing repo Python verifier scripts only
**Storage**: Markdown evidence under `specs/464-decompose-disk-governance-verifiers/`
**Testing**: Direct Python self-tests and repo workflow verifier checks
**Target Platform**: Local and GitHub CI verifier execution
**Project Type**: Rust repo with Python governance/verifier scripts
**Constraints**: No new runtime behavior, no shell/cargo policy expansion, no new dependencies, no secret display, no merge without operator approval
**Scale/Scope**: One helper family and focused characterization tests; no broad verifier redesign

## Constitution Check

- Scope discipline: PASS. One issue (#464) and one declared slice.
- Source of truth: PASS. Fresh branch/worktree from `origin/main` at `817ddfc9af8cd835ee6143f0562595f73a1d2645`.
- No hardcodes: PASS. No runtime config values are introduced.
- No dual paths: PASS if both verifier clients import the selected cargo scanner helpers from `scripts/command_understanding.py`.
- No debts: PASS if residual scope is recorded as remaining #464 decomposition work, not hidden in code comments.
- Pure Rust binary: PASS. Python verifier scripts are repo tooling only and existing scope.
- SSM secret source: PASS. No credential path is touched.
- Group by change: PASS. One lifecycle group: cargo scanner helpers.

## Project Structure

### Documentation

```text
specs/464-decompose-disk-governance-verifiers/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── evidence.md
├── tasks.md
├── checklists/
│   └── requirements.md
└── contracts/
    └── cargo-scanner.md
```

### Source Code

```text
scripts/
├── command_understanding.py
├── rust_verification.py
├── verify_ci_workflow_hygiene.py
└── test_command_understanding.py
```

**Structure Decision**: The shared module owns only pure cargo scanner primitives. Runtime and static verifier clients keep policy decisions local. The shared module is a deeper module because callers get one tested interface for subcommand and scan-boundary logic without inheriting target-routing policy details.

## Chosen Slice

Move these helpers into `scripts/command_understanding.py` after characterization tests are red:

- `cargo_subcommand_with_index(tokens: list[str], start: int = 0) -> tuple[int, str] | None`
- `cargo_subcommand(tokens: list[str]) -> str | None`
- `nextest_subcommand_with_index(nextest_args: list[str]) -> tuple[int, str] | None`
- `cargo_args_for_target_routing_scan(cargo_args: list[str]) -> list[str]`

The shared module will also own the option sets needed by these helpers:

- `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT`
- `CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT`
- `NEXTEST_GLOBAL_OPTIONS_WITH_ARGUMENT`

The no-argument Cargo option set will use a superset that preserves existing behavior for both clients because both current implementations skip unknown leading dash tokens before returning a subcommand.

## Non-Goals

- Do not export `command_tokens`.
- Do not export shell command substitution helpers.
- Do not export renamed cargo/rustc executable detection helpers.
- Do not export wrapper handling helpers.
- Do not export `cargo_target_routing_override` or `tokens_have_target_routing_override`.
- Do not split oversized verifier files in this slice.
- Do not change test-only `sys.path` setup in this slice.
- Do not add new cargo subcommands, global option semantics, shell semantics, regex cases, wrapper families, command-prediction behavior, or verifier policy.

## Exact Files Touched

- `scripts/command_understanding.py`: add shared cargo scanner helper family.
- `scripts/rust_verification.py`: import shared cargo scanner helpers and remove local duplicate definitions.
- `scripts/verify_ci_workflow_hygiene.py`: import shared cargo scanner helpers and remove local duplicate definitions.
- `scripts/test_command_understanding.py`: add characterization/parity tests and update non-export guard.
- `specs/464-decompose-disk-governance-verifiers/*`: record plan, tasks, evidence, review outcomes, and residual risk.

## Behavior-Preservation Strategy

1. Add failing characterization tests in `scripts/test_command_understanding.py` that require shared cargo scanner exports and compare them against current runtime/static verifier behavior.
2. Confirm the test fails because shared exports are absent.
3. Mechanically copy the cargo scanner helper family into `scripts/command_understanding.py`.
4. Rewire both verifier clients to import the shared helpers.
5. Remove only duplicate local helper definitions superseded by the shared module.
6. Re-run focused tests and existing verifier suites.

## Characterization Tests To Add First

- `cargo_subcommand_with_index(["--manifest-path", "Cargo.toml", "test", "--", "--target-dir", "/tmp/raw"]) == (2, "test")`.
- `cargo_subcommand_with_index(["cargo", "--manifest-path", "Cargo.toml", "test"], start=1) == (3, "test")`.
- `cargo_subcommand(["--locked", "nextest", "run"]) == "nextest"`.
- `nextest_subcommand_with_index(["--profile", "ci", "run", "--archive-file", "archive"]) == (2, "run")`.
- `cargo_args_for_target_routing_scan(["test", "--", "--target-dir", "/tmp/raw"]) == ["test"]`.
- `cargo_args_for_target_routing_scan(["nextest", "run", "--archive-file", "archive", "--", "--target-dir", "/tmp/raw"]) == ["nextest", "run", "--archive-file", "archive"]`.
- Runtime and static verifier clients return the same values as the shared helpers for those cases.
- Static `tokens_have_target_routing_override(["CARGO_TARGET_DIR=/tmp/raw", "cargo", "test"])` remains true and local.
- Runtime `cargo_target_routing_override(["test", "--target-dir", "/tmp/raw"])` remains `"--target-dir"` and local.

## External Review Plan

Before implementation, request adversarial review of `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/cargo-scanner.md`, `quickstart.md`, `evidence.md`, and `tasks.md` from:

- Claude
- Gemini
- Grok
- GLM
- DeepSeek
- Kimi

Default gate is unanimous approval before implementation. Failed or timed-out reviewers are recorded as failed/skipped and do not count as approval. DeepSeek and GLM use standing source-send approval from user memory, with approval-request metadata still rendered and recorded.

## Residual Risk

- The shared helper interface may look broader than the selected slice if `cargo_args_for_target_routing_scan` is misread as full target-routing policy. The contract and tests keep policy local.
- The current static start-offset behavior requires a shared optional `start` parameter; omitting that would break static call sites.
- Full target-routing override detection remains duplicated policy. That is intentional for this slice because runtime and static clients have different inputs and return shapes.
- Oversized verifier/test file structure remains a #464 risk after this slice.

## Verification Commands

- `python3 scripts/test_command_understanding.py`
- `python3 scripts/test_rust_verification_cache_retention.py`
- `python3 scripts/test_verify_ci_workflow_hygiene.py`
- `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py`
- `git diff --check`
- `just ci-lint-workflow`
- Exact-head GitHub CI for the PR head
- Exact-head external implementation review before merge readiness

## Complexity Tracking

No accepted complexity violations. Any request to include wrapper handling, shell substitution, renamed executable detection, or oversized-file splitting is a separate #464 slice unless the operator explicitly expands scope before implementation.
