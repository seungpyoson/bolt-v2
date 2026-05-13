# Requirements Checklist: Bolt-v3 Phase 9 Comprehensive Audit

**Purpose**: Validate that Phase 9 requirements are specific, testable, scoped, and fail-closed before implementation.
**Created**: 2026-05-14
**Feature**: `specs/019-bolt-v3-phase9-audit-fresh/spec.md`

## Scope

- [x] CHK001 Phase 9 is scoped as audit and cleanup planning, not live-capital execution.
- [x] CHK002 Requirements state current main is authoritative and stale PR branches are forensic input only.
- [x] CHK003 Requirements separate Phase 7, Phase 8, and Phase 9 readiness decisions.
- [x] CHK004 Out-of-scope soak and live order execution are explicit.

## Fail-Closed Behavior

- [x] CHK005 Missing accepted Phase 7 no-submit readiness blocks final Phase 9 live-readiness certification.
- [x] CHK006 Missing accepted Phase 8 strategy safety and canary machinery blocks tiny live order approval.
- [x] CHK007 Secret exposure risk blocks reviewer transmission and live action.
- [x] CHK008 Chainlink/feed-source or strategy-math uncertainty blocks live action.

## NT Boundary

- [x] CHK009 Requirements keep NT ownership over lifecycle, reconciliation, cache, adapter behavior, order state, and venue wire translation.
- [x] CHK010 Requirements allow Bolt-v3 to own TOML parsing, SSM resolution, provider registration, strategy policy, submit admission, and compact decision evidence.
- [x] CHK011 Requirements require source-backed proof before claiming no dual submit path.

## SSM-Only Secrets

- [x] CHK012 Requirements name AWS SSM through Rust SDK as the only accepted secret source.
- [x] CHK013 Requirements reject environment fallback, AWS CLI subprocess, and non-SSM secret backends.
- [x] CHK014 Requirements distinguish test fixture literals from runtime credential display.

## Evidence Artifacts

- [x] CHK015 Requirements demand file/line, command, PR metadata, test output, or reviewer job evidence for each claim.
- [x] CHK016 Requirements require no-mistakes runtime proof before review or readiness claims.
- [x] CHK017 Requirements require external review disposition before implementation.
- [x] CHK018 Requirements require exact head/SHA and command for any live-capital approval.

## Testability

- [x] CHK019 Requirements require a debt-marker scan over Phase 9 artifacts.
- [x] CHK020 Requirements require cleanup to be protected by one behavior test before code edits.
- [x] CHK021 Requirements require source-fence and architecture checks relevant to any cleanup scope.

## Open Gaps

- [x] CHK022 Final Phase 9 certification is explicitly blocked until Phase 7 and Phase 8 are accepted or explicitly waived.
- [x] CHK023 Direct API reviewer approval is session-approved by the user, but approval-token evidence remains required before source transmission.
