# Feature Specification: CI Baseline Measurement

**Feature Branch**: `codex/ci-333-baseline`
**Created**: 2026-05-15
**Status**: Complete
**Input**: User description: "Review issue #333 and child issues with hard evidence. Start by addressing #343 without narrowing child issue requirements."

## User Scenarios & Testing

### User Story 1 - Exact CI Baseline Exists (Priority: P1)

As the maintainer, I can cite one stable baseline artifact for the current CI wall time, job durations, critical path, billed runner-minute estimate, and cache warmth before topology changes.

**Why this priority**: #332, #342, #195, #205, #335, and #344 require before/after comparisons. Without a stable baseline, later speedup claims become local anecdotes.

**Independent Test**: Inspect the baseline artifact and verify it names exact GitHub Actions run IDs, SHAs, events, timestamps, job durations, critical paths, runner-minute estimates, and cache evidence.

**Acceptance Scenarios**:

1. **Given** an issue reviewer opens the baseline, **When** they inspect each run row, **Then** every row names a run ID, event type, SHA, timestamp, status, URL, and job durations.
2. **Given** a child issue reports a speedup, **When** it compares against the baseline, **Then** the comparison can reference a matching current run shape instead of a guessed average.
3. **Given** cache warmth is discussed, **When** the baseline marks a run warm, cold, rerun, or unknown, **Then** that label is backed by cache-log evidence or explicitly marked unknown.

### User Story 2 - Child Issues Keep Full Scope (Priority: P1)

As the maintainer, I can see how #343 relates to every #333 child without cutting or silently deferring any child requirement.

**Why this priority**: The epic is MECE. Measurement must support all children while avoiding hidden scope changes.

**Independent Test**: The plan and tasks map #343 outputs to child issue consumers and list blocked or non-measurement items as dependencies rather than removed scope.

**Acceptance Scenarios**:

1. **Given** #340 is blocked by claude-config #677, **When** the #333 map is reviewed, **Then** #340 is marked blocked with evidence and not treated as done.
2. **Given** #344 contains blocked and unblocked work, **When** the #333 map is reviewed, **Then** unblocked work remains available and blocked work names #332/#195/#205 dependencies.
3. **Given** #335 is closed, **When** the #333 map is reviewed, **Then** the delivered PR and residual #344 scope are both named.

## Edge Cases

- A run is still `in_progress`; it must not be used as completed baseline evidence.
- A workflow run is successful but a lane is skipped; the baseline must state whether the skip is expected and whether it affects the comparison.
- GitHub logs expose cache hits but not true cold/warm intent; the baseline must not infer more than the logs prove.
- PR and push/tag paths have different semantics; PR critical path and post-merge deploy path must stay separate.
- Existing issue body evidence may be stale; live GitHub run metadata must be checked before relying on it.
- Live issue bodies and comments may conflict; the baseline must record the conflict and name the source of the chosen interpretation instead of silently narrowing scope.

## Requirements

### Functional Requirements

- **FR-001**: The baseline MUST include exact GitHub Actions run IDs, commit SHAs, event types, timestamps, status, conclusion, and source URLs.
- **FR-002**: The baseline MUST include at least one representative PR run and one representative main or tag path where available, including the same-SHA main/tag pair when a child issue names both.
- **FR-003**: The baseline MUST distinguish PR wall time, main-push wall time, tag/smoke wall time, critical-path lane, and estimated billed runner minutes.
- **FR-004**: The baseline MUST estimate billed runner minutes from job durations, not workflow elapsed time alone.
- **FR-005**: The baseline MUST include cache warmth only where observable from log evidence; otherwise it MUST say unknown.
- **FR-006**: The baseline MUST preserve exact evidence links or run IDs so future child issues can verify numbers.
- **FR-007**: The baseline MUST remain measurement-only and MUST NOT change workflow topology or runtime behavior.
- **FR-008**: The issue map MUST include all nine #333 children: #343, #342, #332, #195, #205, #203, #335, #344, and #340.
- **FR-009**: Closed, blocked, and partially blocked children MUST be classified from live issue state and comments, not memory.
- **FR-010**: No child issue requirement may be removed, narrowed, or silently deferred without explicit evidence and a link to the owning issue.
- **FR-011**: Known live-source conflicts MUST be recorded with the conflicting issue numbers and the interpretation used for the current branch.

### Key Entities

- **BaselineRun**: GitHub Actions run ID, URL, event, head SHA, branch/tag, timestamps, status, conclusion, and reason for inclusion.
- **JobTiming**: Job name, status, conclusion, start/completion timestamps, wall duration, and skipped/required meaning.
- **CacheObservation**: Log-backed cache key, hit/miss/unknown state, restored size where logged, and compile/test signal where logged.
- **ChildIssueState**: Issue number, title, open/closed/blocked state, acceptance ownership, dependencies, implementation status, and any body/comment conflict that affects scope.

## Success Criteria

### Measurable Outcomes

- **SC-001**: One baseline document exists in `docs/ci/` and is linked from #333 or #343.
- **SC-002**: The baseline contains at least one current PR run, one current main-push run, and the known same-SHA main/tag path from #205 when still available in GitHub Actions.
- **SC-003**: Every run table row includes exact run ID, SHA, event, timestamp, wall time, critical path, raw runner minutes, and rounded runner-minute estimate.
- **SC-004**: Cache warmth labels cite concrete log evidence such as cache key, hit/miss, restored size, or explicitly say unknown.
- **SC-005**: A child issue map lists all nine #333 children with current live state and dependencies.
- **SC-006**: No workflow behavior changes are present in the #343 measurement diff.
- **SC-007**: Any detected child-scope conflict is explicitly documented rather than omitted.

## Assumptions

- GitHub Actions metadata and logs available through `gh` are authoritative for the baseline.
- This spec snapshots live issue bodies, comments, and run metadata as of 2026-05-15. GitHub issue state remains authoritative for later changes.
- Rounded billed minutes are estimates because GitHub billing may apply account-specific rules; raw active job minutes are still reported.
- #343 is complete with a linked baseline artifact and does not require workflow topology changes.
