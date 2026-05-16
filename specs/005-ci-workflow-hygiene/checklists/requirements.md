# Requirements Checklist: CI Workflow Hygiene

**Purpose**: Validate #203 requirements quality before implementation.
**Created**: 2026-05-15
**Feature**: `specs/005-ci-workflow-hygiene`

## Requirement Completeness

- [x] CHK001 Are all deferred #203 items represented: fmt-check detector dependency, lane-specific setup trimming, deploy direct needs, exact job-name lint, actionable errors, and optional parser generality? [Completeness, Spec FR-001..FR-011]
- [x] CHK002 Are #333 conditional requirements for #342 represented because #342 exists in the stacked base? [Completeness, Spec FR-005]
- [x] CHK003 Are #332, #205, and #344 conditional requirements explicitly excluded until their workflow surfaces exist? [Completeness, Spec Edge Cases]
- [x] CHK004 Are local and exact-head CI verification requirements specified? [Completeness, Spec FR-013, SC-004]
- [x] CHK004A Is the prebuilt CI tool install contract represented for cargo-deny, cargo-nextest, cargo-zigbuild, action fallback behavior, and pinned cargo-zigbuild SHA256? [Completeness, Spec FR-014, SC-006]

## Requirement Clarity

- [x] CHK005 Are exact required job ids listed by name instead of described generically? [Clarity, Spec FR-003]
- [x] CHK006 Is "lane-specific setup trimming" defined as managed target-dir opt-in, not removal of the managed Rust owner from fmt-check? [Clarity, Spec FR-009..FR-010]
- [x] CHK007 Is deploy defense-in-depth defined as direct needs plus retained aggregate gate? [Clarity, Spec FR-007]
- [x] CHK007A Is cargo-zigbuild checksum verification defined as an in-repo pinned hash rather than same-origin release checksum trust? [Clarity, Spec FR-014]

## Requirement Consistency

- [x] CHK008 Does the spec preserve #342 source-fence ordering while avoiding #332 sharding work? [Consistency, Spec FR-005, FR-012]
- [x] CHK009 Does the spec preserve build detector gating while removing only fmt-check detector serialization? [Consistency, Spec FR-006, FR-008]
- [x] CHK010 Does the spec keep one aggregate gate signal instead of replacing it with direct deploy needs? [Consistency, Spec FR-004, FR-007]

## Acceptance Criteria Quality

- [x] CHK011 Are success criteria measurable by exact commands and exact CI jobs? [Acceptance Criteria, Spec SC-001..SC-004]
- [x] CHK012 Are negative lint cases required, not only happy-path lint success? [Acceptance Criteria, Spec SC-001]
- [x] CHK012A Do negative lint cases cover source-build, fallback, missing install step, same-origin checksum, and `crate@version`/toolchain-prefixed cargo-install regressions? [Acceptance Criteria, Spec SC-006]

## Scenario Coverage

- [x] CHK013 Are missing job, missing dependency, missing gate result, missing setup opt-in, and stale detector serialization scenarios covered? [Coverage, User Story 1..3]
- [x] CHK014 Are future topologies named so they cannot be silently treated as complete? [Coverage, Spec FR-012]

## Dependencies & Assumptions

- [x] CHK015 Is the stacked base dependency on #342 explicit? [Assumption, Spec Assumptions]
- [x] CHK016 Is the no-new-dependency constraint explicit for the verifier? [Assumption, Spec FR-001]

## Scope Boundaries

- [x] CHK017 Does the spec keep LiveNode-heavy nextest serialization out of #203 and leave it to #332? [Boundary, Spec FR-012]
- [x] CHK018 Does the spec keep source-fence verifier internals out of #203 while linting the #342 workflow lane topology? [Boundary, Spec FR-005, FR-012]
