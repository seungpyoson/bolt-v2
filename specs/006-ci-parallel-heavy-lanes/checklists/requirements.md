# Requirements Checklist: CI Parallel Heavy Lanes

**Purpose**: Validate #332 requirements quality before implementation.
**Created**: 2026-05-15
**Feature**: `specs/006-ci-parallel-heavy-lanes`

## Requirement Completeness

- [x] CHK001 Are both heavy-lane targets represented: host `clippy` and `check-aarch64`? [Completeness, Spec FR-001..FR-003]
- [x] CHK002 Is four-way nextest sharding represented with exact shard values and partition syntax? [Completeness, Spec FR-004..FR-005]
- [x] CHK003 Is managed `just test` passthrough represented instead of adding a second test path? [Completeness, Spec FR-006]
- [x] CHK004 Are shard reproduction logs required? [Completeness, Spec FR-007]
- [x] CHK005 Is fail-closed aggregate gate behavior represented for clippy, check-aarch64, source-fence, aggregate test, and build skip semantics? [Completeness, Spec FR-008..FR-010]
- [x] CHK006 Is #342 source-fence ownership explicitly resolved instead of silently narrowed? [Completeness, Spec FR-011..FR-012]
- [x] CHK007 Are #195 cache-key coordination and bounded shard keys represented? [Completeness, Research]
- [x] CHK008 Is exact before/after timing evidence required but not overclaimed before CI exists? [Completeness, Spec FR-015]

## Requirement Clarity

- [x] CHK009 Are exact job ids listed by name? [Clarity, Data Model]
- [x] CHK010 Is `count:` partitioning documented as issue-required despite nextest recommending `slice:`? [Clarity, Research]
- [x] CHK011 Is `fail-fast: false` defined as evidence preservation for every shard result? [Clarity, Research]
- [x] CHK012 Is duplicate source-fence execution defined as intentional and gate-covered? [Clarity, Quickstart]

## Requirement Consistency

- [x] CHK013 Does the spec keep #342 source-fence before full test? [Consistency, Spec FR-011]
- [x] CHK014 Does the spec avoid #195, #205, #344, #340, and generic #203 work while still preserving #332 requirements? [Consistency, Spec FR-016]
- [x] CHK015 Does the spec preserve the single aggregate gate instead of requiring each matrix child as a separate required status? [Consistency, Spec FR-009..FR-010]
- [x] CHK016 Does the cache-key decision stay shard-aware but bounded? [Consistency, Research]

## Acceptance Criteria Quality

- [x] CHK017 Are negative lint cases required for missing job, matrix, partition command, fail-fast, cache key, and reproduction log? [Acceptance Criteria, Spec SC-001..SC-002]
- [x] CHK018 Is local passthrough validation measurable without necessarily running all tests? [Acceptance Criteria, Quickstart]
- [x] CHK019 Is exact CI timing tied to real run ids and SHAs? [Acceptance Criteria, Data Model]

## Scenario Coverage

- [x] CHK020 Does the spec cover failed, cancelled, skipped, and missing required lanes? [Coverage, User Story 1..2]
- [x] CHK021 Does the spec cover source-fence duplicate/exclusion decision and chosen duplicate path? [Coverage, User Story 3]
- [x] CHK022 Does the spec cover actionable reviewer/operator logs for each shard? [Coverage, User Story 2]

## Dependencies & Assumptions

- [x] CHK023 Is the stacked base dependency on #342/#203 explicit? [Assumption, Plan]
- [x] CHK024 Is the exact-head CI limitation for stacked PRs explicit? [Assumption, Quickstart]
- [x] CHK025 Are external reviews blocked until exact-head CI is available under repo rules? [Assumption, Quickstart]
