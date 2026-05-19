# Feature Specification: PR #331 Phase 9 Current-Head Audit

**Feature Branch**: `022-bolt-v3-phase9-current-main-audit`
**Created**: 2026-05-14
**Updated**: 2026-05-18
**Status**: PR #331 P9 audit artifact sync. Not live-readiness certification.
**Input**: Complete PR #331 Phase 9 audit/remediation through P9 with evidence, while keeping PR #392 downstream and out of PR #331.

## User Scenarios & Testing

### User Story 1 - Evidence-Backed Audit Report (Priority: P1)

The operator can read one PR #331 audit report that separates accepted source-review evidence, unrun live evidence, and remaining blockers across architecture, config, secrets, NT ownership, strategy assumptions, tests, stale artifacts, and live operations.

**Why this priority**: Phase 9 decides what PR #331 can truthfully claim. Source-review closure, no-submit live readiness, tiny-canary completion, staged live, and production live are separate claim levels.

**Independent Test**: Review `audit-report.md` and verify every material finding has file/line evidence, command output, PR metadata, test output, reviewer job record, or a named evidence gap.

**Acceptance Scenarios**:

1. **Given** PR #331 current head is checked with `gh pr view 331 --json headRefOid,baseRefOid,mergeStateStatus,state`, **When** the Phase 9 audit report is reviewed, **Then** it distinguishes source-review closure from live-readiness certification.
2. **Given** P7 and P8 source-review comments exist, **When** the report uses them, **Then** it states those reviews do not prove real SSM/venue no-submit execution or tiny-capital live execution.
3. **Given** a live-capital claim, **When** the report is checked, **Then** it blocks that claim unless explicit operator approval names exact head, exact command, config checksum, and redacted evidence.

---

### User Story 2 - Stale Artifact Cleanup Gate (Priority: P2)

The operator can see which P9 artifacts were stale and verify the cleanup is documentation-only unless a separate behavior test and approval gate exists.

**Why this priority**: Phase 9 cleanup must remove stale claims without drifting into unreviewed runtime refactor or PR #392 scope.

**Independent Test**: Run the stale-reference and debt-marker scans in `quickstart.md`; confirm P9 artifacts no longer carry retired path/head/PR references as current evidence.

**Acceptance Scenarios**:

1. **Given** an old P9 artifact references retired PR/path/head state, **When** Phase 9 sync runs, **Then** the artifact is updated or clearly marked historical and non-authoritative.
2. **Given** cleanup would touch runtime code, **When** Phase 9 executes, **Then** the task list blocks that cleanup until a behavior test, reviewed plan, and user approval exist.

---

### User Story 3 - Six-Reviewer P9 Gate (Priority: P3)

The operator can send the current P9 artifacts to Claude, Gemini, Kimi, DeepSeek, GLM, and Grok after branch is clean, pushed, and exact-head CI is green.

**Why this priority**: User policy requires adversarial external review before closing the P9 gate. The review is evidence adjudication, not majority voting.

**Independent Test**: After commit/push/CI, run custom/adversarial reviews against the exact PR head and record job IDs, approvals, blockers, nonblockers, and dispositions in PR evidence comments.

**Acceptance Scenarios**:

1. **Given** the branch is dirty, unpushed, stale, or CI is not green, **When** external review is requested, **Then** the workflow stops before source transmission.
2. **Given** DeepSeek or GLM source is sent, **When** source transmission occurs, **Then** approval-token evidence is recorded and no secrets are exposed.
3. **Given** a blocking reviewer finding, **When** triage runs, **Then** the finding is fixed or disproved with source evidence before P9 closure.

## Edge Cases

- P7/P8 were reviewed at a prior source head, then P9 documentation-only sync changes the PR head.
- Current main advances after P9 artifacts are committed.
- A tracked example TOML is present, but no active operator root TOML is present.
- A stale review disposition file exists from an older planning PR and could be mistaken for current P9 review evidence.
- A Python verification script exists even though the runtime must remain pure Rust.
- A strategy input is configurable, but its feed/economic assumption is not approved for live capital.

## Requirements

### Functional Requirements

- **FR-001**: The audit MUST anchor to live PR #331 metadata from GitHub before making PR-state claims.
- **FR-002**: The audit MUST classify P7 source review, P8 source review, and P9 audit review separately.
- **FR-003**: The audit MUST cover hardcoded runtime values, dual paths, debt markers, brittle architecture, AI slop, NT boundary violations, SSM-only secret source, pure Rust runtime, runtime config grouping, stale docs/specs/tasks, source fences, test quality, external review disposition, production readiness gaps, strategy math/feed assumptions, and live ops readiness.
- **FR-004**: The audit MUST distinguish runtime code from tests, fixtures, docs, tracked examples, and verifier scripts before labeling a finding as a production violation.
- **FR-005**: The audit MUST fail closed on unresolved live-capital, secret-exposure, Chainlink/feed-source, strategy-math, NT-boundary, or external-review blockers.
- **FR-006**: The audit MUST state that no live order, no soak, no deploy, and no real-capital command can run without explicit operator approval for exact head and command.
- **FR-007**: Cleanup MUST be bounded, behavior-test protected, externally reviewed when material, and user-approved before runtime implementation.
- **FR-008**: P9 external review MUST include Claude, Gemini, Kimi, DeepSeek, GLM, and Grok unless the operator explicitly waives one reviewer.
- **FR-009**: Direct API reviewer source transmission MUST use approval-token records and MUST not expose raw secrets.
- **FR-010**: The final recommendation MUST use the contract vocabulary: ready for no-submit only, ready for tiny live order approval, blocked with exact blockers, or stop. Accepted source-review scope MUST be named separately from live-readiness recommendation.
- **FR-011**: PR #392 work MUST remain downstream; PR #331 may document the dependency but MUST NOT implement PR #392 scope.

### Key Entities

- **AuditFinding**: A severity-ranked finding with category, evidence, decision impact, and recommended next action.
- **EvidenceCitation**: File/line, command output, PR metadata, reviewer job record, or test output supporting one claim.
- **CleanupCandidate**: A bounded code or doc cleanup item with required behavior tests and stop conditions.
- **ExternalReviewDisposition**: Reviewer job identity, approval status, findings, and accept/disprove/defer decision.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Audit report covers every FR-003 category with evidence or a named evidence gap.
- **SC-002**: No P9 external review starts until the branch is clean, committed, pushed, and exact-head CI is green.
- **SC-003**: All live-capital paths remain blocked unless explicit operator approval includes exact head and command.
- **SC-004**: Debt-marker scan over P9 artifacts returns no unresolved template markers.
- **SC-005**: Stale-reference scan over P9 artifacts returns no retired current-claim references.
- **SC-006**: Six-reviewer P9 gate has no unresolved blockers before P9 source-review closure is claimed.

## Assumptions

- P7 and P8 source-review gates are closed for PR #331 source review at the previously recorded source head; their live operator runs remain unexecuted unless later evidence proves otherwise.
- P9 documentation-only sync creates a new PR head; exact-head review prompts must inject the live head at review time instead of relying on a self-referential committed SHA.
- Absence of active local operator config in this checkout is not a secret failure by itself, but blocks source-backed live readiness claims.
