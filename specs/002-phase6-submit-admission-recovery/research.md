# Research: Phase 6 Submit Admission Recovery

## Decision 1: Treat PR #317 As Reference-Only

**Decision**: PR #317 is not mergeable and must not be rebased, merged, or broadly cherry-picked. It may be used only as a reference for Phase 6 concepts.

**Rationale**: Current `main` is authoritative after merged Phase 3-5 work. PR #317 predates that current shape and carries stale Phase 3-5 code. Direct continuation risks reintroducing old registration, decision-evidence, runtime-capture, and helper-path designs.

**Alternatives considered**:
- Direct merge: rejected because stale files could overwrite current architecture.
- Rebase old stack: rejected because it preserves obsolete branch topology and review burden.
- Cherry-pick broad commits: rejected because commits mix valid Phase 6 logic with stale surrounding code.

## Decision 2: Use One Fresh Phase 6 Branch From Current Main

**Decision**: Future implementation starts from current `main` and creates one narrow Phase 6 PR.

**Rationale**: One narrow PR reduces CI cost, review cost, and drift risk. It also matches constitution slice discipline and repo source-of-truth rules.

**Alternatives considered**:
- Continue #316-#320 stack: rejected because stale bases make review expensive and unsafe.
- Merge several phases together: rejected until Phase 6 is recovered cleanly.

## Decision 3: Require Recovery Memo Before Code

**Decision**: `recovery-review.md` must exist before implementation and must classify all #317 Phase 6-relevant material as keep, rewrite, or reject.

**Rationale**: This converts implicit judgment into reviewable evidence. It also prevents hidden cherry-picks and accidental stale-code resurrection.

**Alternatives considered**:
- Chat-only summary: rejected because it is not durable enough.
- PR body only: rejected because no fresh PR exists yet.

## Decision 4: Review Strategy Before Implementation

**Decision**: Get recovery-strategy review on the memo before writing Phase 6 code.

**Rationale**: Current risk is wrong direction, not syntax. Reviewing the salvage strategy first is cheaper than reviewing code built on a bad premise.

**Alternatives considered**:
- Implement first, review later: rejected because it can waste CI/reviewer cycles.
- Ask reviewers to review stale #317: rejected because it asks reviewers to reason over obsolete branch state.

## Decision 5: Preserve Existing Phase 3-5 Architecture

**Decision**: Future Phase 6 must preserve current `main` behavior from merged Phase 3-5: single registration path, mandatory decision evidence, and existing runtime capture in the live-node runner.

**Rationale**: Phase 6 layers submit admission onto the current spine. It must not recreate old helper functions, duplicate evidence writers, or drop runtime capture.

**Alternatives considered**:
- Port stale #317 live-node code: rejected because it predates current runtime-capture restoration.
- Port stale archetype evidence-writer construction: rejected because current #322 centralizes writer construction in registration.
