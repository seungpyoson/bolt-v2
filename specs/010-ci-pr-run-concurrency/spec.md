# Feature Specification: CI PR Run Concurrency

**Feature Branch**: `codex/ci-355-pr-concurrency`
**Created**: 2026-05-17
**Status**: Draft
**Input**: GitHub issue #355: cancel superseded pull-request CI runs per branch head.

## User Scenarios & Testing

### User Story 1 - PR-Only Cancellation Policy Is Guarded (Priority: P1)

As the maintainer, I can keep the CI workflow's top-level concurrency policy scoped to pull requests so superseded PR heads are cancelled without changing main, tag, deploy, scheduled, or manual semantics.

**Why this priority**: #355's primary risk is weakening non-PR flows while trying to save Actions minutes on obsolete PR heads.

**Independent Test**: `python3 scripts/test_verify_ci_workflow_hygiene.py` mutates a representative CI fixture and fails if top-level concurrency is missing, PR grouping does not use the PR number, non-PR grouping does not include ref and SHA, or `cancel-in-progress` is not limited to `pull_request`.

**Acceptance Scenarios**:

1. **Given** top-level `concurrency` is missing, **When** the workflow hygiene verifier runs, **Then** it fails with an actionable PR-only concurrency error.
2. **Given** the PR concurrency group is not keyed by `github.event.number`, **When** the verifier runs, **Then** it fails because superseded PR-head cancellation is not tied to the PR.
3. **Given** non-PR runs are not keyed by both `github.ref_name` and `github.sha`, **When** the verifier runs, **Then** it fails because main/tag/manual flows could collide across distinct commits.
4. **Given** `cancel-in-progress` is `true` for all events, **When** the verifier runs, **Then** it fails because non-PR flows could be cancelled.

### User Story 2 - Runtime Cancellation Evidence Is Recorded (Priority: P1)

As the maintainer, I can inspect real GitHub Actions run history and see that older PR-head CI runs were cancelled after newer heads started while the newest PR head still ran the required gate.

**Why this priority**: Static workflow shape is necessary but not enough; #355 explicitly requires hard evidence from real PR iteration.

**Independent Test**: A GitHub Actions run inventory names exact run IDs, branch, old/new SHAs, conclusions, and required-check status for a real PR branch where older PR-head CI was cancelled after supersession.

**Acceptance Scenarios**:

1. **Given** a PR branch has multiple CI runs, **When** a newer head starts, **Then** an older run for the same PR branch is cancelled or recorded as already completed before supersession.
2. **Given** the newest PR head run completes, **When** required checks are inspected, **Then** the aggregate gate remains fail-closed and the required CI jobs pass.
3. **Given** main or tag runs exist, **When** their concurrency group is evaluated, **Then** it includes ref and SHA and does not cancel in progress.

## Edge Cases

- The workflow already contains a top-level concurrency block on current `main`; this feature must guard it against drift rather than add a second job-level policy.
- `pull_request` grouping must use PR number rather than branch name because branch names can collide across forks or stack shapes.
- Non-PR grouping must include SHA so consecutive `main` or tag runs for different commits do not cancel one another.
- The verifier must not add a YAML dependency; existing line-based standard-library parsing is sufficient for this repository workflow shape.

## Requirements

### Functional Requirements

- **FR-001**: `.github/workflows/ci.yml` MUST define top-level `concurrency`.
- **FR-002**: The `concurrency.group` expression MUST branch on `github.event_name == 'pull_request'`.
- **FR-003**: Pull-request concurrency group MUST be keyed by `format('pr-{0}', github.event.number)`.
- **FR-004**: Non-PR concurrency group MUST be keyed by `format('{0}-{1}', github.ref_name, github.sha)`.
- **FR-005**: `cancel-in-progress` MUST be limited to `${{ github.event_name == 'pull_request' }}`.
- **FR-006**: The workflow hygiene verifier MUST fail with actionable errors if any FR-001..FR-005 invariant is missing or weakened.
- **FR-007**: The verifier and self-tests MUST use only standard-library Python and existing repository commands.
- **FR-008**: Runtime evidence MUST record exact GitHub Actions run IDs, branch names, old/new SHAs, conclusions, and newest-head required-check status.
- **FR-009**: This issue MUST NOT weaken source-fence, test, build, deploy, or aggregate gate fail-closed behavior.

### Key Entities

- **PrConcurrencyPolicy**: Top-level CI workflow concurrency expression that cancels only superseded pull-request runs.
- **WorkflowHygieneVerifier**: Standard-library verifier that rejects missing or weakened concurrency invariants.
- **SupersededRunEvidence**: GitHub Actions run evidence showing stale PR-head cancellation and newest-head gate status.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Hygiene self-tests fail on missing concurrency, wrong PR grouping, wrong non-PR grouping, and all-event cancellation.
- **SC-002**: `python3 scripts/test_verify_ci_workflow_hygiene.py`, `python3 scripts/verify_ci_workflow_hygiene.py`, and `just ci-lint-workflow` pass locally.
- **SC-003**: Exact-head PR CI passes with the newest PR head running the full required gate.
- **SC-004**: Issue #355 receives a comment or PR body section with exact cancellation evidence from real GitHub Actions runs.

## Assumptions

- Current `main` already has the intended top-level concurrency expression.
- This PR's root code change is verifier coverage plus evidence, not a new CI topology rewrite.
- Non-PR cancellation policy changes are out of scope for #355.
