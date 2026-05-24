# Feature Specification: #464 Disk-Governance Verifier Decomposition Follow-Up

**Feature Branch**: `codex/464-verifier-decomposition`
**Created**: 2026-05-24
**Status**: Draft
**Input**: Issue #464 after merged PR #461

## User Scenarios & Testing

### User Story 1 - Select One Evidence-Proven Slice (Priority: P1)

Reviewers can inspect a current-main evidence map and see why the chosen slice is limited to cargo subcommand scanning helpers, not command tokenization, shell substitution, renamed executable detection, wrapper handling, or full target-routing policy.

**Why this priority**: Issue #464 is maintenance-risk work. Without a clear evidence map, an extraction can silently change accepted disk-governance behavior.

**Independent Test**: `specs/464-decompose-disk-governance-verifiers/evidence.md` lists each remaining helper family from issue #464 and classifies it as proven equivalent, divergent but characterizable, insufficient evidence, or non-goal for this slice.

**Acceptance Scenarios**:

1. **Given** current `origin/main` after PR #461, **When** a reviewer opens `evidence.md`, **Then** the selected cargo scanner slice is tied to exact current files and line ranges.
2. **Given** a divergent helper family, **When** a reviewer checks the evidence map, **Then** the family remains local and no shared export is claimed.

### User Story 2 - Characterize Cargo Scanner Behavior Before Movement (Priority: P1)

Maintainers can run focused characterization tests that fail before shared cargo scanner exports exist and pass only when runtime and static verifier behavior is preserved.

**Why this priority**: The implementation is refactoring. The value comes from proving behavior preservation before moving code.

**Independent Test**: `python3 scripts/test_command_understanding.py` includes cargo scanner cases for global options, start offsets, `nextest run`, command separators, and target-routing scan cutoffs.

**Acceptance Scenarios**:

1. **Given** current `scripts/command_understanding.py` without cargo scanner exports, **When** the new characterization test is run, **Then** it fails because the expected shared helper interface is absent.
2. **Given** the shared helper interface after extraction, **When** the same test is run, **Then** runtime verifier, static verifier, and shared helper classifications match for representative cargo scanner inputs.

### User Story 3 - Extract Cargo Scanner Helpers Without New Semantics (Priority: P2)

Runtime disk-governance verification and static CI workflow hygiene both use one shared cargo scanner helper family while keeping policy-specific target-routing and environment handling local.

**Why this priority**: The cargo subcommand scanning logic is duplicated and similar enough to deepen the shared command-understanding module, while full target-routing policy is still surface-specific.

**Independent Test**: `python3 scripts/test_command_understanding.py`, `python3 scripts/test_rust_verification_cache_retention.py`, and `python3 scripts/test_verify_ci_workflow_hygiene.py` pass after both verifier clients import the shared cargo scanner helpers.

**Acceptance Scenarios**:

1. **Given** a cargo command with global options, **When** either verifier scans for the subcommand, **Then** both use the shared helper and preserve the same index/subcommand result.
2. **Given** `cargo test -- --target-dir /tmp/raw`, **When** either verifier prepares target-routing scan tokens, **Then** the post-separator `--target-dir` remains excluded.
3. **Given** static workflow-only environment prefix checks, **When** the static verifier checks target routing, **Then** those checks remain local and are not moved into the shared module.

### User Story 4 - Keep Residual #464 Scope Explicit (Priority: P3)

Reviewers can see which #464 decomposition areas remain accepted scope and which follow-up evidence is required before moving them.

**Why this priority**: #464 can contain multiple bounded PRs. This PR must not claim broader completion.

**Independent Test**: The PR body and `evidence.md` identify remaining command tokenization, shell substitution, renamed executable, wrapper, target-routing policy, oversized-file split, and test `sys.path` cleanup scope.

**Acceptance Scenarios**:

1. **Given** this PR only extracts cargo scanner helpers, **When** a reviewer checks the PR body, **Then** it references #464 without claiming to close all decomposition work.
2. **Given** remaining work, **When** a reviewer checks `evidence.md`, **Then** the work remains tied to #464 or a named follow-up issue if the PR claims a narrower completion boundary.

## Edge Cases

- Cargo global flags with arguments: `--manifest-path Cargo.toml`, `--config key=value`, `-Z unstable-options`.
- Cargo global flags without arguments: `--locked`, `--offline`, `--verbose`, `-q`, `-v`.
- Runtime-only accepted no-argument tokens such as `--help`, `--list`, `--version`, and `-V`; current behavior skips unknown flags, so the shared superset must preserve results.
- Static start-offset scanning for token lists that include the `cargo` executable at index 0.
- `cargo nextest run --archive-file archive -- --target-dir /tmp/raw` must scan only before the post-`run` separator.
- `cargo test -- --target-dir /tmp/raw` must ignore binary/test arguments after the separator.
- Static target-routing environment prefix detection remains static-verifier policy and outside the shared cargo scanner helper family.
- Runtime target-routing refusal payload and option string formatting remain runtime-verifier policy and outside the shared cargo scanner helper family.

## Requirements

### Functional Requirements

- **FR-001**: The PR MUST start from `origin/main` after merge commit `817ddfc9af8cd835ee6143f0562595f73a1d2645` on a fresh #464 branch/worktree.
- **FR-002**: The PR MUST include issue-local Speckit-style artifacts under `specs/464-decompose-disk-governance-verifiers/`.
- **FR-003**: The evidence map MUST classify every helper family named in issue #464 before implementation.
- **FR-004**: The implementation MUST add characterization/parity tests before moving cargo scanner code.
- **FR-005**: The shared module MAY export cargo scanner helpers only after tests prove parity for representative runtime and static verifier cases.
- **FR-006**: The implementation MUST NOT add shell semantics, wrapper families, cargo behavior, regex cases, command-prediction behavior, or verifier policy.
- **FR-007**: The runtime verifier MUST preserve existing target-routing refusal behavior and message shape.
- **FR-008**: The static workflow verifier MUST preserve existing raw cargo/storage override detection behavior.
- **FR-009**: Divergent or unproven helper families MUST remain local and explicitly documented.
- **FR-010**: The plan/spec/tasks/evidence MUST receive adversarial review from Claude, Gemini, Grok, GLM, DeepSeek, and Kimi before implementation if each reviewer is available.
- **FR-011**: Exact-head implementation review MUST be recorded before merge readiness.
- **FR-012**: The PR MUST not merge without explicit operator approval.

### Key Entities

- **Cargo Scanner Helper Family**: Shared pure functions that parse Cargo and cargo-nextest argument lists for subcommand and pre-separator scan boundaries.
- **Runtime Verifier Client**: `scripts/rust_verification.py`, which enforces managed Rust execution and cache-retention policy at runtime.
- **Static Verifier Client**: `scripts/verify_ci_workflow_hygiene.py`, which detects raw cargo/storage policy violations in workflows and automation text.
- **Evidence Map**: `specs/464-decompose-disk-governance-verifiers/evidence.md`, the source of truth for chosen slice, non-goals, review outcomes, and verification.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Cargo scanner duplication is reduced by moving the common subcommand/scan-boundary helpers into `scripts/command_understanding.py`.
- **SC-002**: `python3 scripts/test_command_understanding.py` proves runtime, static, and shared cargo scanner results match for the selected representative cases.
- **SC-003**: `python3 scripts/test_rust_verification_cache_retention.py` and `python3 scripts/test_verify_ci_workflow_hygiene.py` pass after extraction.
- **SC-004**: `python3 -m py_compile` passes for every touched Python file.
- **SC-005**: `git diff --check` passes.
- **SC-006**: `just ci-lint-workflow` passes because verifier/CI hygiene paths are touched.
- **SC-007**: Exact-head GitHub CI is green before merge readiness.
- **SC-008**: External implementation review records approvals or explicit skipped/failed slots; missing review is not counted as approval.

## Assumptions

- Current `origin/main` after PR #461 is the only implementation source of truth.
- #464 permits multiple bounded follow-up PRs; this slice does not close all #464 work unless the operator explicitly accepts that scope.
- Issue-local Speckit artifacts follow the established #454 pattern and do not repoint `.specify/feature.json`, which currently names `specs/023-nt-order-intent-layer`.
- The selected cargo scanner helper family is pure Python standard library logic and can live in `scripts/command_understanding.py` without adding dependencies.
