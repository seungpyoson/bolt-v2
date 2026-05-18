# Feature Specification: Disk Pressure Governance

**Feature Branch**: `codex/123-disk-pressure-speckit`
**Created**: 2026-05-18
**Status**: Draft
**Input**: User description: "Issue #123: review all issue bodies/comments and child issues, verify disk-pressure symptoms end to end, plan reliable disk-space savings, then address the epic with TDD and external review gates."

## User Scenarios & Testing

### User Story 1 - Classify Disk Growth Before Acting (Priority: P1)

As the operator, I can map any large bolt-v2-related disk consumer to one owning issue, current evidence, and allowed action before deleting or changing anything.

**Why this priority**: The epic exists because multiple disk-pressure classes were being collapsed into one vague cleanup problem.

**Independent Test**: A reviewer can use the issue map and walkthrough to classify each known large path without relying on chat history.

**Acceptance Scenarios**:

1. **Given** a large path under `~`, repo root, `/private/tmp`, or macOS temp, **When** it matches a known class, **Then** the walkthrough names the owning issue and allowed next action.
2. **Given** a large path does not match a known class, **When** it crosses the configured "newly-large" threshold, **Then** it is routed to unknown-class detection rather than silently cleaned.

---

### User Story 2 - Prevent Unmanaged Rust Artifacts (Priority: P1)

As the developer, I can run bolt-v2 Rust checks through one managed path so bash, zsh, agents, and wrappers do not create uncontrolled repo-local `target/` directories.

**Why this priority**: #374 documents fresh 2026-05-17 evidence that bash-shell bypass recreated an 18 GB repo-local target despite earlier fixes.

**Independent Test**: The Phase 1 enumeration for #374 covers launcher, environment, invocation form, cwd, and target dimensions before any shim/preflight implementation.

**Acceptance Scenarios**:

1. **Given** a cargo invocation comes from zsh, bash, agent shell, `just`, no-mistakes, IDE, or clean env, **When** it is in bolt-v2 scope, **Then** requirements define how it is classified and routed or explicitly excluded.
2. **Given** a heavy Rust command would start with low disk or oversized managed cache, **When** the preflight policy applies, **Then** requirements define a fail-closed or explicitly overridden result.

---

### User Story 3 - Right-Size Known Caches And Logs (Priority: P2)

As the operator, I can reclaim disk from managed caches, AI tooling logs/sessions, and obsolete toolchains through dry-run-first policies that preserve hot cache and current toolchains.

**Why this priority**: #286 and #375 describe large but partly useful consumers where blind deletion trades disk pressure for slower or broken future work.

**Independent Test**: Requirements distinguish hot cache, rebuildable-but-expensive cache, stale cache, rotating logs, session TTL, and protected toolchains.

**Acceptance Scenarios**:

1. **Given** the managed Rust cache exceeds its configured soft limit, **When** status/prune is requested, **Then** the policy reports per-subtree size, free disk, active processes, and dry-run actions before apply.
2. **Given** old Codex sessions, large rolling logs, or stale Rust toolchains exist, **When** hygiene policy runs, **Then** requirements protect active/pinned items and define configurable retention.

---

### User Story 4 - Cover Unmeasured And Unknown Consumers (Priority: P3)

As the operator, I can prove every known disk-writing surface is inventoried and future unknown consumers surface early enough to act.

**Why this priority**: #376 and #377 exist because completing known-class fixes still leaves unmeasured runtime, CI, cargo-state, and future-tool consumers.

**Independent Test**: The plan names inventory deliverables for unmeasured surfaces and a lightweight detection contract for unknown classes without duplicating other issue enforcement.

**Acceptance Scenarios**:

1. **Given** bolt-v3 runtime, local CI/test artifacts, cargo registry, or cargo git growth occurs, **When** representative runs are measured, **Then** requirements capture path, size, growth rate, policy, and owner.
2. **Given** a new path family crosses the configured newly-large threshold under `~`, **When** it is not owned by existing issues, **Then** requirements define how it is surfaced and triaged.

## Edge Cases

- A path belongs to machine-level caches such as npm, Homebrew, or Xcode DerivedData: this epic records it as out of scope and routes it to machine/dev-environment config, not bolt-v2.
- #125 Claude task-output containment belongs to `claude-config` implementation after the bolt-v2 incident anchor; bolt-v2 must not pretend to fix it locally. The current external owner is `seungpyoson/claude-config#597`.
- #70 is closed as resolved-by-investigation; repeating that incident requires equivalent scratch diagnostics, not current bolt-v2 runtime.
- A cleanup command finds active `cargo`, `rustc`, `rust_verification.py`, agent, or holder processes: destructive apply behavior must refuse or require explicit operator action.
- Full local test execution is requested while free disk is low, shell routing is unproven, or exact-head CI can provide the broad signal: requirements prefer draft PR plus CI runners instead of duplicate raw local cargo.

## Requirements

### Functional Requirements

- **FR-001**: The repo MUST preserve a MECE child-issue map for #123 covering #48, #70, #124, #125, #286, #374, #375, #376, and #377.
- **FR-002**: Each issue entry MUST state whether it is an investigation anchor, implementation owner, closed resolved-by-investigation track, or out-of-repo implementation dependency.
- **FR-003**: The repo MUST define a 1:1 issue-to-PR plan for implementation owners; if a PR cannot map 1:1, the issue must be decomposed or the PR scope must be broadened explicitly before coding.
- **FR-004**: #374 implementation MUST NOT start until a Phase 1 MECE enumeration covers cargo invocation launchers, env state, invocation forms, cwd classes, and targets with explicit gap/overlap review.
- **FR-005**: #375 implementation MUST NOT start until a Phase 1 MECE enumeration covers developer tools, exact written paths, growth shape, native rotation support, and ownership.
- **FR-006**: #377 implementation MUST NOT start until a Phase 1 MECE enumeration covers known-owned classes, unbounded known classes, future-class dimensions, operator interface, and detection failure modes.
- **FR-007**: #286 MUST define managed Rust cache status and pruning policy with per-subtree size, recency, current free disk, active-process refusal, dry-run default, and explicit apply mode.
- **FR-008**: #376 MUST inventory bolt-v3 runtime output, local CI/test artifacts, cargo registry, and cargo git steady-state with exact paths, representative run size, growth rate, retention policy, and owner issue.
- **FR-009**: The operator walkthrough MUST answer whether to run cargo tests locally: CI is the default broad verifier after a PR exists; local cargo is exception-only for narrow TDD, CI failure reproduction, or local routing/cache behavior; full local suite is allowed only after disk preflight, routing proof, and an explicit reason not covered by CI.
- **FR-010**: The repo MUST NOT treat S3 as an active Cargo target cache. S3 may be used only for reviewed artifacts/evidence or deploy bundles, not concurrent mutable build output.
- **FR-011**: no-mistakes MUST NOT invoke raw bolt-v2 cargo commands into no-mistakes worktree-local `target/` directories. It must either reuse managed repo commands/target routing or consume exact-head CI evidence when broad verification already exists.
- **FR-012**: The raw-cargo source fence MUST include no-mistakes repo configuration or another reviewed guard that prevents future no-mistakes command drift.
- **FR-013**: Runtime thresholds, retention ages, size caps, and override behavior MUST be config-controlled or documented as operator policy, not hardcoded runtime constants.
- **FR-014**: Cleanup/prune operations MUST avoid credential display, raw secret output, and destructive action without dry-run evidence plus explicit apply.
- **FR-015**: TDD is required for every implementation PR: failing test or verifier first, observed red, minimal change, observed green, then broader verification.
- **FR-016**: External review gates for implementation PRs MUST include no-mistakes plus Claude, Gemini, DeepSeek, GLM, and Kimi review/adversarial review after branch is clean, pushed, and exact-head CI is green.
- **FR-017**: No PR from this epic may claim to close #123 unless all child issue deliverables are complete or explicitly waived by the operator.

### Key Entities

- **DiskPressureIssueTrack**: One child issue or implementation owner with scope, status, evidence, PR mapping, and residuals.
- **DiskSurface**: Path family that writes or accumulates bytes, with owner, growth shape, current size, representative growth rate, and policy.
- **RetentionPolicy**: Configurable cap, TTL, dry-run/apply behavior, protected classes, and active-process safety rules.
- **VerificationLane**: Local managed test, CI full-suite lane, artifact/evidence lane, or external review gate.
- **ImplementationSlice**: One issue-to-PR unit with TDD test seam, verification commands, review gates, and residual scope.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A reviewer can classify every current #123 child issue into exactly one owner category and see residual work.
- **SC-002**: The first implementation slice can start from a finite Phase 1 enumeration instead of speculative cleanup or shim design.
- **SC-003**: The walkthrough gives a deterministic local-vs-CI Rust verification policy and rejects raw cargo, no-mistakes duplicate full-local cargo, and S3-cache workarounds.
- **SC-004**: Requirements for #374, #375, and #377 make the May 17 Phase-1 research gates explicit preconditions.
- **SC-005**: Future PRs can be reviewed against a finite checklist of issue mapping, TDD, local verification, CI, no-mistakes, and external review gates.

## Assumptions

- `origin/main` at `4e6aacbd38f7dc254fe8d9893e514fa2f29c52e6` is the fetched source of truth for this planning slice as of 2026-05-18.
- Issue bodies and comments fetched on 2026-05-18 are authoritative for current #123 scope.
- Implementation that changes local Claude/Codex runtime belongs in `claude-config` unless a bolt-v2 repo artifact is explicitly required.
- Existing untracked files in the root checkout are user-owned and out of scope for this planning worktree.
