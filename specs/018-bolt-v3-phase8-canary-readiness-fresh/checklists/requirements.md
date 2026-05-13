# Requirements Quality Checklist: Phase 8 Tiny-capital Canary Machinery

**Purpose**: Validate requirement quality before implementation planning or code changes.
**Created**: 2026-05-14
**Feature**: `specs/018-bolt-v3-phase8-canary-readiness-fresh/spec.md`

## Scope Clarity

- [x] Scope names Phase 8 tiny-capital canary machinery and excludes actual live order execution without explicit runtime approval.
- [x] Scope states stale PR #318, #319, and #320 are reference-only.
- [x] Scope states current `main` is source of truth.
- [x] Scope separates dry/no-submit proof from live-capital action.
- [x] Scope states Phase 7 no-submit evidence is a dependency, not assumed present.

## Fail-closed Behavior

- [x] Missing Phase 7 report blocks Phase 8 live action.
- [x] Rejected live canary gate blocks Phase 8 live action.
- [x] Missing or blocked strategy-input safety audit blocks live action.
- [x] Missing exact command/head approval blocks live order execution.
- [x] Missing NT lifecycle evidence blocks live-readiness claim.

## NT Boundary

- [x] Requirements assign submit, accept, reject, fill, cancel, cache, lifecycle, and reconciliation ownership to NT.
- [x] Requirements forbid Bolt-owned order lifecycle and reconciliation.
- [x] Requirements forbid adapter forks, mock venue proof, and NT cache forks.
- [x] Requirements require production bolt-v3 runner wrapper and forbid direct `LiveNode::run` in Phase 8 harness code.

## Secrets And Config

- [x] SSM remains the only credential source.
- [x] Operator env vars are limited to non-secret paths, hashes, approval ids, and exact command identity.
- [x] Requirements avoid raw secret display or secret material persistence.
- [x] Runtime values remain TOML/config-derived; no runtime hardcodes are permitted.

## Evidence Artifacts

- [x] Dry/no-submit evidence is required before live action.
- [x] Evidence join keys include decision intent, gate result, admission result, runtime capture, and NT lifecycle evidence.
- [x] Evidence artifact is redacted and hash-based where config or SSM identity is involved.
- [x] Tests can validate evidence shape without real capital or real secrets.

## Testability

- [x] Each user story has an independent local test path.
- [x] Behavior tests are required before implementation tasks.
- [x] Operator harness is ignored by default.
- [x] Source fences verify no forbidden live-order bypasses.

## Out Of Scope

- [x] Soak tests are out of scope without explicit approval.
- [x] Actual live order is out of scope without exact runtime approval.
- [x] Backtesting and research analytics are out of scope.
- [x] Phase 9 cleanup/audit is not hidden inside Phase 8 implementation.

## Review Readiness

- [x] Spec has no unresolved clarification markers.
- [x] Spec has no placeholders.
- [x] Spec has no deferred-work debt.
- [x] Spec states external review is required before implementation.
