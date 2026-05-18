# Requirements Checklist: CI PR Run Concurrency

**Purpose**: Validate #355 requirements quality before implementation.
**Created**: 2026-05-17
**Feature**: `specs/010-ci-pr-run-concurrency`

## Requirement Completeness

- [x] CHK001 Does the spec include automatic cancellation of superseded pull-request runs? [Completeness, Spec US1, FR-001..FR-006]
- [x] CHK002 Does the spec include non-PR flow protection for main, tag, deploy, scheduled, and manual semantics? [Completeness, Spec US1, FR-004..FR-005]
- [x] CHK003 Does the spec require runtime evidence, not just static workflow shape? [Completeness, Spec US2, FR-008, SC-004]

## Requirement Clarity

- [x] CHK004 Is PR grouping specified by exact expression `format('pr-{0}', github.event.number)`? [Clarity, Spec FR-003]
- [x] CHK005 Is non-PR grouping specified by exact expression `format('{0}-{1}', github.ref_name, github.sha)`? [Clarity, Spec FR-004]
- [x] CHK006 Is `cancel-in-progress` scoped to pull_request by exact expression? [Clarity, Spec FR-005]

## Requirement Consistency

- [x] CHK007 Does the spec preserve aggregate gate fail-closed behavior instead of bypassing checks through cancellation? [Consistency, Spec FR-009]
- [x] CHK008 Does the spec keep #355 separate from #250 build duration and #203 generic hygiene scope? [Consistency, Spec Assumptions]

## Acceptance Criteria Quality

- [x] CHK009 Are negative verifier tests required for missing concurrency, wrong PR group, wrong non-PR group, and all-event cancellation? [Acceptance Criteria, Spec SC-001]
- [x] CHK010 Are exact run IDs, SHAs, conclusions, and check status required for runtime evidence? [Acceptance Criteria, Spec FR-008, SC-004]

## Dependencies & Assumptions

- [x] CHK011 Is current-main concurrency treated as existing behavior to guard, not reimplemented blindly? [Assumption, Spec Edge Cases]
- [x] CHK012 Is no-new-dependency verifier constraint explicit? [Assumption, Spec FR-007]
