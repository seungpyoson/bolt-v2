# Feature Specification: Phase 9 Current-main Audit

**Feature Branch**: `022-bolt-v3-phase9-current-main-audit`
**Created**: 2026-05-14
**Status**: Audit and approved remediation complete
**Input**: User request to audit current `main` after PR #328, then remediate accepted production hardcode/policy-hardcode findings with evidence and no live-capital action.

## User Scenarios & Testing

### User Story 1 - Prove The Audit Is Current-main Anchored

As the bolt-v3 operator, I can see the exact branch, worktree, and head SHA used for the audit before trusting any finding.

**Independent Test**: Audit artifacts record the original source anchor, and the branch is refreshed onto current `origin/main` before final verification.

**Acceptance Scenarios**:

1. **Given** the fresh worktree, **When** the audit records source provenance, **Then** it names branch `022-bolt-v3-phase9-current-main-audit`, original audit source anchor `23acab30b73990302765ea441550fabcbf03f570`, and final refreshed `origin/main` base `fde50d3452859a51f7f27b807913b1f12697b273`.
2. **Given** stale branches `019` and `021`, **When** the audit references prior work, **Then** those branches are treated as reference-only and no proof is accepted from them unless reproduced on current main.

### User Story 2 - Classify Runtime Literals And Policy Values

As the operator, I can distinguish config-owned runtime values from true hardcodes, protocol labels, diagnostic text, test fixtures, and accepted residuals.

**Independent Test**: The audit runs literal and policy coverage scans over every required source, test, script, fixture, spec, and doc surface, then classifies material findings with file and line evidence.

**Acceptance Scenarios**:

1. **Given** literal scan output, **When** a production runtime value appears in code, **Then** the audit classifies it as TOML-owned, accepted residual, policy violation, protocol label, diagnostic text, test fixture, NT/API glue, or bounded internal constant.
2. **Given** policy scan output, **When** provider, venue, strategy, risk, admission, default, or bypass language appears, **Then** the audit classifies whether it is part of the current bolt-v3 contract or a gap requiring explicit disposition.

### User Story 3 - Decide Whether Tiny Live Order Approval Is Safe

As the operator, I can use the Phase 9 audit to decide between no-submit-only readiness, tiny live order approval, blocked, or stop.

**Independent Test**: The audit report includes a severity-ranked findings table, runtime-capture concern disposition, cleanup candidates with behavior locks, and an explicit decision outcome.

**Acceptance Scenarios**:

1. **Given** current main, **When** the audit finds stale status docs, residual runtime literals, untested capture failure behavior, or live-capital gaps, **Then** the decision must not be "ready tiny live order approval".
2. **Given** no approved live capital action, **When** the audit completes, **Then** no live order, soak, merge, runtime cleanup, or source-bearing external review has run.

## Requirements

### Functional Requirements

- **FR-001**: Audit MUST record original source anchor `23acab30b73990302765ea441550fabcbf03f570` and final branch refresh to current `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`.
- **FR-002**: Audit MUST treat `019` and `021` work as reference-only.
- **FR-003**: Audit MUST cover `src/bolt_v3_*.rs`, `src/bolt_v3_*/**/*.rs`, the runtime-used shared paths, all bolt-v3 tests, fixtures, scripts, verifiers, docs, and specs named in the plan.
- **FR-004**: Audit MUST run and record literal coverage, policy coverage, verifier inspection, and roadmap-doc inspection evidence.
- **FR-005**: Audit MUST classify hardcoded runtime values, policy hardcodes, dual paths, debt/AI slop, NT boundary violations, SSM-only secret compliance, pure Rust runtime compliance, config grouping, stale specs/docs, test/source fences, strategy/feed assumptions, live ops readiness, and production roadmap gaps.
- **FR-006**: Audit MUST classify `run_bolt_v3_live_node` runtime-capture failure handling as real bug, false positive, or needs test.
- **FR-007**: Audit/remediation MUST NOT run live capital, run soak, merge, or clean runtime state.
- **FR-008**: Runtime remediation MUST be limited to accepted production violations, use behavior locks, and avoid speculative cleanup.
- **FR-009**: Audit MUST produce a decision outcome: no-submit only, ready tiny live order approval, blocked, or stop.

### Non-functional Requirements

- **NFR-001**: Every substantive claim must cite source, command, verifier, or doc evidence.
- **NFR-002**: Findings must be severity-ranked.
- **NFR-003**: No raw credentials or secret values may appear in audit artifacts.
- **NFR-004**: The audit/remediation artifacts must be deterministic and reviewable as a scoped Phase 9 change.

## Success Criteria

- **SC-001**: Fresh-main provenance is recorded with exact SHA and branch.
- **SC-002**: Coverage matrix includes every required surface and proof command.
- **SC-003**: Runtime literal and policy classifications identify accepted residuals separately from config-owned values.
- **SC-004**: Runtime-capture concern disposition is explicit and does not trigger implementation without approval.
- **SC-005**: Decision outcome is explicit and blocks live-capital approval when evidence is insufficient.

## Out Of Scope

- Unapproved runtime code changes.
- Speculative cleanup outside accepted findings.
- Real SSM or venue run.
- Live order, soak, merge, deploy, or cleanup.
- Source-bearing external review without approval-request evidence.
