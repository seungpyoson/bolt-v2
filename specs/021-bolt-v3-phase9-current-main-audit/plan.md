# Implementation Plan: PR #331 Phase 9 Completion

**Branch**: `022-bolt-v3-phase9-current-main-audit` | **Date**: 2026-05-18 | **Spec**: `specs/021-bolt-v3-phase9-current-main-audit/spec.md`
**Input**: Complete PR #331 P9 audit closure with exact-head evidence. Keep PR #392 downstream.

## Summary

Finish PR #331 as one systematic Phase 9 audit/remediation branch. P0-P8 source-review gates are closed or already recorded for PR #331; P9 remains open until current artifacts are synchronized, committed, pushed, CI is green, and six external reviewers return no unresolved blockers. Live readiness remains blocked by missing real no-submit and tiny-canary evidence.

## Technical Context

**Language/Version**: Rust workspace, current repo toolchain
**Primary Dependencies**: NautilusTrader Rust crates, Rust AWS SDK for SSM, existing Bolt-v3 verifiers, cargo tests
**Storage**: TOML config, Markdown SpecKit/docs artifacts, redacted evidence references only
**Testing**: targeted cargo tests, workflow hygiene, `git diff --check`, `just fmt-check`, source-fence/verifier gates where touched, GitHub CI
**Target Platform**: PR #331 exact-head review and later GitHub merge gate
**Project Type**: Pure Rust live trading binary over NautilusTrader
**Constraints**: no live capital, no secret display, TOML-only runtime values, SSM-only secrets, no Python runtime, no dual submit/readiness paths, no stale branch proof, no production-ready claim without evidence
**Scale/Scope**: PR #331 Phase 9 packets P0-P9. PR #392 remains downstream and rebases after PR #331 lands.

## Constitution Check

*GATE: Re-check after this P9 artifact sync and before P9 external review.*

- NT-first thin layer: PASS. P9 documentation sync does not add Bolt-owned order lifecycle, reconciliation, cache, adapter behavior, or order machinery.
- Generic core, concrete edges: PASS. Existing status-map rows separate implemented current bindings from missing live-readiness gates.
- Single path and config-controlled runtime: PASS. P6/P7/P8 evidence records one readiness/gate path; no env-var or alternate secret path is introduced.
- Test-first safety gates: PASS for prior semantic fixes. Current remaining edits are documentation-only artifact synchronization; validation is scans, diff hygiene, CI, and external review.
- Evidence before claims: PASS only after local checks and exact-head PR checks are current.
- Minimal slice discipline: PASS. PR #331 remains Phase 9 audit/remediation; PR #392 remains downstream.

## Current Evidence

- PR #331 branch: `022-bolt-v3-phase9-current-main-audit`.
- PR #331 purpose: Phase 9 hardcode/dual-path audit and remediation.
- PR #331 live metadata must be checked with `gh pr view 331 --json headRefOid,baseRefOid,mergeStateStatus,state` before head/base claims.
- P0-P6 status: closed before P7/P8/P9 window; P6 linkage gate fixed and re-verified in PR #331.
- P7 status: source/review gate closed in PR evidence comment `4479704791`; no real SSM/venue operator run claimed.
- P8 status: source/review gate closed in PR evidence comment `4479505648`; no tiny-capital canary run claimed.
- P9 status: committed artifacts are the protocol snapshot; exact-head CI, reviewer job IDs, and final closure state belong in PR #331 evidence comments after push.
- PR #392 relationship: downstream PR #392 remains open and separate; it should rebase after PR #331 lands.

## Project Structure

```text
specs/021-bolt-v3-phase9-current-main-audit/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── contracts/audit-evidence.md
├── quickstart.md
├── audit-report.md
├── ai-slop-cleanup-report.md
├── external-review-phase9-disposition.md
├── external-review-phase9-prompt.md
├── external-review-phase9-relay-prompts.md
└── tasks.md

docs/bolt-v3/
├── 2026-04-28-source-grounded-status-map.md
└── 2026-05-18-production-readiness-contract.md
```

**Structure Decision**: Keep PR #331 Phase 9 execution state in `specs/021-bolt-v3-phase9-current-main-audit/`. Keep downstream production-readiness and PR #392 implementation work out of PR #331.

## Phase Plan

### Phase 0 - State Anchor

Verify local branch state, PR #331 metadata, PR #392 metadata, and main CI state from source commands before editing or reviewing.

### Phase 1 - P9 Artifact Synchronization

Replace stale P9 current-claim artifacts with current PR #331 language. Do not hardcode the future final commit SHA inside committed docs; inject exact head into external review prompts at runtime and record evidence in PR comments.

### Phase 2 - Local Verification

Run stale-reference scan, debt-marker scan, and `git diff --check`. Run additional repo checks only if touched artifacts or CI rules require them. Do not use green checks as proof of live readiness.

### Phase 3 - Commit, Push, CI

Commit documentation-only P9 artifact sync, push PR #331, and wait for exact-head GitHub CI green before external review.

### Phase 4 - Six-Reviewer P9 Review

Run Claude, Gemini, Kimi, DeepSeek, GLM, and Grok custom/adversarial review on current P9 artifacts and supporting docs. Do not majority-vote; fix or disprove every blocker with evidence.

### Phase 5 - PR #331 Evidence Comment

After no unresolved blockers remain, post P9 closure evidence to PR #331 with exact head, CI run, local checks, reviewer job IDs, residual live blockers, and PR #392 next step.

### Phase 6 - PR #392 Boundary Audit

After PR #331 P9 source-review gate closes, inspect PR #392 metadata and body for its declared dependency. Do not implement PR #392 inside PR #331.

## AI-Slop Cleanup Plan

Scope is limited to stale documentation/evidence artifacts touched in this P9 sync.

- Remove stale current-claim text, not historical facts needed for audit trail.
- Replace overbroad readiness language with the contract recommendation vocabulary and separate source-review scope language.
- Keep runtime/trading/provider/secret code unchanged unless a new blocker proves a code defect.
- Put exact-head external review evidence in PR comments to avoid self-referential commit-SHA churn.

## Stop Conditions

- Same fix approach fails twice.
- Any reviewer blocker remains unresolved.
- Any branch/head/PR claim cannot be verified from source command output.
- Any live-order, deploy, secret, or production action appears.
- PR #392 implementation starts inside PR #331.
