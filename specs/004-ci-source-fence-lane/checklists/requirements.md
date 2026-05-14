# Requirements Checklist: CI Source-Fence Lane

## Requirement Completeness

- [x] CHK001 Does the spec require a top-level `source-fence` job by exact name? [Completeness, Spec FR-001]
- [x] CHK002 Does the spec include the full six-script verifier list from #342 without dropping missing scripts? [Completeness, Spec FR-003]
- [x] CHK003 Does the spec require both canonical source-fence cargo test filters named by #342? [Completeness, Spec FR-004]
- [x] CHK004 Does the spec cover the #332 duplicate-ownership constraint instead of silently treating sharding as done? [Completeness, Spec FR-008]
- [x] CHK005 Does the spec include #203's narrow linter/gate invariant requirement because the generic lint issue is still open? [Completeness, Spec FR-007]

## Requirement Clarity

- [x] CHK006 Is "early" defined by `test` depending on `source-fence`, not only by adding another parallel job? [Clarity, Spec FR-002]
- [x] CHK007 Is "minimal lane" defined as no full `cargo-nextest` install or full integration test suite? [Clarity, Spec FR-005]
- [x] CHK008 Is fail-closed behavior defined as accepting only `needs.source-fence.result == "success"`? [Clarity, Spec FR-006]

## Requirement Consistency

- [x] CHK009 Do workflow, recipe, and linter requirements all name the same `source-fence` lane? [Consistency, Spec FR-001/FR-006/FR-007]
- [x] CHK010 Does the spec keep #342 separate from #332, #195, #205, #335, #344, and #340? [Consistency, Spec SC-005]

## Acceptance Criteria Quality

- [x] CHK011 Is there a measurable red check before implementation? [Acceptance Criteria, Spec SC-001]
- [x] CHK012 Is there a measurable local source-fence command that proves all verifier scripts and filters run? [Acceptance Criteria, Spec SC-002]
- [x] CHK013 Is the deliberate stale assertion proof defined without committing the temporary mutation? [Acceptance Criteria, Spec SC-003]
- [x] CHK014 Is exact-head CI evidence required for both the new lane and existing required lanes? [Acceptance Criteria, Spec SC-004]

## Edge Case Coverage

- [x] CHK015 Does the spec cover missing verifier scripts as implementation work rather than an exclusion? [Coverage, Spec US2]
- [x] CHK016 Does the spec cover GitHub Actions' lack of automatic job cancellation? [Coverage, Edge Cases]
- [x] CHK017 Does the spec cover deterministic Python dependency behavior for CI verifier scripts? [Coverage, Spec US2]
