# Feature Specification: #344 Residual Minute-Consumption Work

**Feature Branch**: `codex/ci-344-residual-minute-work`  
**Created**: 2026-05-15  
**Status**: Draft  
**Input**: GitHub issue #344 plus #333/#335 current live comments

## Scope

This slice owns unblocked #344 residual work after #335: branch hygiene inventory, dry-run path-filter docs, pass-stub compatibility for future required checks, and issue handoff. It does not alter #335 `pull_request.paths-ignore` behavior. It does not perform the post-#332/#195/#205 minute rebaseline until those stacked PRs land and real run evidence exists.

## User Stories & Tests

### User Story 1 - Explain path-filter behavior (P1)

As maintainer, I can inspect one document that shows representative PR path classes and whether full CI or docs pass-stub behavior should apply.

**Independent Test**: A verifier rejects docs that omit required path rows or diverge from the workflow safe-path list.

### User Story 2 - Preserve future required-check compatibility (P1)

As maintainer, I can later make `gate` required without docs-only PRs becoming permanently pending when CI is skipped by `paths-ignore`.

**Independent Test**: A workflow verifier rejects missing pass-stub workflow, wrong required-check job name, missing path classifier, missing fail-closed changed-file collection, or drift between CI `paths-ignore` and the classifier safe patterns.

### User Story 3 - Record branch hygiene state (P2)

As maintainer, I can see every non-`main` branch classified as active, reference-only, or dead-merged-prunable before any deletion is proposed.

**Independent Test**: The branch-hygiene artifact/comment lists every current remote non-`main` branch and makes no destructive changes.

### User Story 4 - Keep blocked evidence explicit (P2)

As maintainer, I can distinguish what #344 implements now from evidence that remains blocked until stacked CI changes land.

**Independent Test**: PR and issue handoff name the blocked docs-only PR evidence and post-stack minute rebaseline separately from completed docs/pass-stub/branch inventory work.

## Requirements

- **FR-001**: Add `docs/ci/paths-ignore-behavior.md` with rows for docs-only `AGENTS.md`, workflow change, Rust source, managed rust-verification TOML, `Cargo.lock`, mixed docs+source, and ignored config directories.
- **FR-002**: Add a pass-stub workflow that can emit the same required-check job name `gate` for ignored-safe PRs.
- **FR-003**: Pass-stub eligibility MUST be determined from actual changed files, not only trigger path filters.
- **FR-004**: Changed-file collection or classification failure MUST fail closed.
- **FR-005**: The classifier MUST treat the current CI `pull_request.paths-ignore` list as the safe source of truth and reject drift.
- **FR-006**: Mixed docs+source changes MUST remain full-CI cases, not docs-only cases.
- **FR-007**: `push` and tag CI semantics MUST remain unchanged.
- **FR-008**: The workflow verifier/self-tests MUST cover pass-stub job name, changed-file classifier wiring, safe-pattern drift, required docs rows, and non-weakening of real CI gate.
- **FR-009**: Branch hygiene output MUST classify every current remote non-`main` branch and include no deletion action.
- **FR-010**: Real docs-only PR evidence and post-stack minute rebaseline MUST remain blocked until their prerequisite PR stack state exists.

## Key Entities

- **SafePathSet**: Exact path patterns currently ignored by CI `pull_request.paths-ignore`.
- **PathClassification**: Classification of changed files as docs-only safe, full-CI required, or invalid/missing evidence.
- **PassStubGate**: A `gate` job that succeeds only when changed-file classification proves the PR is ignored-safe.
- **BranchHygieneEntry**: Branch name, SHA, classification, rationale, and proposed action.

## Edge Cases

- Mixed ignored-safe and source files are not docs-only.
- `.claude/rust-verification.toml` remains build-affecting and must not be ignored.
- `docs/**` and `specs/**/*.md` remain build inputs in this repo and must not be broad-ignored.
- If branch protection later requires `gate`, pass-stub must not replace full CI for source changes.
- Branch deletion requires separate explicit approval and is not performed here.

## Success Criteria

- **SC-001**: `python3 -B scripts/test_verify_ci_path_filters.py` passes.
- **SC-002**: `python3 -B scripts/verify_ci_path_filters.py` passes against workflow/docs files.
- **SC-003**: `python3 -B scripts/test_verify_ci_workflow_hygiene.py` and `python3 -B scripts/verify_ci_workflow_hygiene.py` pass.
- **SC-004**: `just ci-lint-workflow` passes.
- **SC-005**: #344 gets a branch-hygiene comment and explicit blocked-evidence status.
