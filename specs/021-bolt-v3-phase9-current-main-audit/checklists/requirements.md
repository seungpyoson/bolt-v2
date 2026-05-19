# Requirements Checklist: PR #331 Phase 9 Current-Head Audit

**Purpose**: Validate that P9 requirements are specific, testable, scoped, and fail-closed before external review.
**Created**: 2026-05-14
**Updated**: 2026-05-18
**Feature**: `specs/021-bolt-v3-phase9-current-main-audit/spec.md`

## Scope

- [x] CHK001 P9 is scoped as audit/evidence sync, not live-capital execution.
- [x] CHK002 Requirements state live PR #331 metadata and checked-out source are authoritative for PR claims.
- [x] CHK003 Requirements separate P7 source review, P8 source review, P9 audit review, no-submit readiness, tiny-canary readiness, staged live, and production live.
- [x] CHK004 Out-of-scope soak, deploy, and live order execution are explicit.
- [x] CHK005 PR #392 is explicitly downstream and out of PR #331 implementation scope.

## Fail-Closed Behavior

- [x] CHK006 Missing real SSM/venue no-submit evidence blocks no-submit live readiness claims.
- [x] CHK007 Missing tiny-capital canary evidence blocks tiny-canary completion claims.
- [x] CHK008 Secret exposure risk blocks reviewer transmission and live action.
- [x] CHK009 Chainlink/feed-source or strategy-math uncertainty blocks live action.
- [x] CHK010 Missing staged/production runbook, monitoring, deploy provenance, panic/service policy, lifecycle, or reconciliation evidence blocks staged and production live claims.

## NT Boundary

- [x] CHK011 Requirements keep NT ownership over lifecycle, reconciliation, cache, adapter behavior, order state, and venue wire translation.
- [x] CHK012 Requirements allow Bolt-v3 to own TOML parsing, SSM resolution, provider registration, strategy policy, submit admission, and compact decision evidence.
- [x] CHK013 Requirements require source-backed proof before claiming no dual submit/readiness path.

## SSM-Only Secrets

- [x] CHK014 Requirements name AWS SSM through Rust SDK as the only accepted secret source.
- [x] CHK015 Requirements reject environment fallback, AWS CLI subprocess, and non-SSM secret backends.
- [x] CHK016 Requirements distinguish test fixture literals from runtime credential display.

## Evidence Artifacts

- [x] CHK017 Requirements demand file/line, command, PR metadata, test output, or reviewer job evidence for each claim.
- [x] CHK018 Requirements require exact PR head injection at external review time.
- [x] CHK019 Requirements require PR comments for final exact-head review evidence to avoid self-referential commit-SHA churn.
- [x] CHK020 Requirements require exact head and command for any live-capital approval.

## Testability

- [x] CHK021 Requirements require a debt-marker scan over P9 artifacts.
- [x] CHK022 Requirements require a stale-reference scan over P9 artifacts.
- [x] CHK023 Requirements require cleanup to be protected by behavior tests before runtime code edits.
- [x] CHK024 Requirements require exact-head CI green before external review.

## Open Gates

- [x] CHK025 P9 source-review closure is explicitly blocked until six-reviewer review has no unresolved blockers.
- [x] CHK026 Direct API reviewer source transmission is session-approved by the user, but approval-token evidence remains required before source transmission.
- [x] CHK027 Final live-readiness certification is explicitly blocked by missing no-submit and tiny-canary operator evidence.
