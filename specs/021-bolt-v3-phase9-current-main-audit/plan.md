# Implementation Plan: PR #331 Phase 9 Completion

**Branch**: `022-bolt-v3-phase9-current-main-audit` | **Date**: 2026-05-18 | **Spec**: `specs/021-bolt-v3-phase9-current-main-audit/spec.md`
**Input**: Complete PR #331 Phase 9 packet closure through P9, while preserving merged `origin/main` production-readiness artifacts that PR #392 depends on.

## Summary

Finish PR #331 as one systematic Phase 9 audit/remediation branch. The current work is not only P6: P6 is the active blocking packet, while P7-P9 remain required review packets before PR #331 can be called ready. The merge from `origin/main` must preserve all accepted PR #331 fixes, preserve the merged production-readiness surface for PR #392, and leave a clean exact head for remaining packet review.

## Technical Context

**Language/Version**: Rust workspace, current repo toolchain
**Primary Dependencies**: NautilusTrader Rust crates, Rust AWS SDK for SSM, existing Bolt-v3 verifiers, cargo tests
**Storage**: TOML config, Markdown SpecKit/docs artifacts, redacted evidence references only
**Testing**: targeted cargo tests, full `cargo test`, `cargo fmt --check`, `git diff --check`, `just clippy`, source-fence/verifier gates where touched
**Target Platform**: PR #331 exact-head review and later GitHub merge gate
**Project Type**: Pure Rust live trading binary over NautilusTrader
**Performance Goals**: No runtime performance goal for this merge/review slice; preserve live-gate fail-closed behavior
**Constraints**: no live capital, no secret display, TOML-only runtime values, SSM-only secrets, no Python runtime, no dual submit paths, no stale branch proof, no production-ready claim without evidence
**Scale/Scope**: PR #331 Phase 9 packets P0-P9. PR #392 remains downstream and rebases after PR #331 lands.

## Constitution Check

*GATE: Must pass before continuing merge cleanup. Re-check after P7-P9 review.*

- NT-first thin layer: PASS. Merge work must not add Bolt-owned order lifecycle, reconciliation, cache, adapter behavior, or order machinery.
- Generic core, concrete edges: PASS unless P7-P9 review proves a concrete provider or family leak remains.
- Single path and config-controlled runtime: PASS. P6 linkage fix keeps readiness/live gate state tied to one configured TOML/report path; no env-var replacement path is allowed.
- Test-first safety gates: PASS. Any semantic conflict fix must first reproduce by compiler/test failure or existing regression test.
- Evidence before claims: PASS only after local checks and exact-head PR checks are current.
- Minimal slice discipline: PASS. PR #331 remains Phase 9 audit/remediation; PR #392 remains downstream.

## Current Evidence

- PR #331 branch: `022-bolt-v3-phase9-current-main-audit`.
- PR #331 purpose: Phase 9 hardcode/dual-path audit and remediation.
- P0-P5 status: closed before this merge window; must be re-verified by exact-head packet review, not assumed from stale branch state.
- P6 status: blocking finding fixed in PR #331, then merge conflicts appeared after `origin/main` advanced.
- P7-P9 status: pending.
- PR #392 relationship: downstream PR #392 says PR #331 must merge first, then PR #392 rebases on new `main`.
- Active merge condition: `origin/main` production-readiness artifacts under `specs/013-production-live-readiness/` and related docs/tests must be preserved unless they conflict with PR #331 safety rules.

## Project Structure

### Documentation

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
└── tasks.md

specs/013-production-live-readiness/
├── plan.md
├── tasks.md
└── ...
```

### Source Code

```text
src/
tests/
docs/bolt-v3/
scripts/
config/
```

**Structure Decision**: Keep PR #331 Phase 9 execution state in `specs/021-bolt-v3-phase9-current-main-audit/`. Keep downstream production-readiness artifacts in `specs/013-production-live-readiness/` because they are already present on `origin/main` and PR #392 depends on them.

## Phase Plan

### Phase 0 - Behavior Lock

Use existing compiler errors, targeted tests, and source-fence checks as the behavior lock. Do not make speculative cleanup edits. If a conflict produces a design choice, stop and request review; if it produces a stale fixture or mechanical merge mismatch, fix only that mismatch and rerun the targeted test.

### Phase 1 - Merge Completion

Resolve all merge conflicts from `origin/main` into PR #331. Preserve PR #331 P0-P6 safety behavior and `origin/main` production-readiness artifacts. Prove no conflict markers or unresolved index entries remain.

### Phase 2 - P6 Re-Verification

Re-run focused no-submit/live-canary tests proving the gate validates readiness linkage fields and fails closed on drift. Fixtures must satisfy current linkage contract instead of weakening the gate.

### Phase 3 - P7-P9 Packet Review

Run remaining packet review in order. Use external/adversarial review where packet process requires it. Do not majority-vote subjective architecture findings; accept only evidence-backed approvals or fix/disprove findings.

### Phase 4 - Exact-Head Verification

After P7-P9 close, run full local verification, push, verify PR #331 exact head and CI. Do not claim readiness or mergeability from stale metadata.

### Phase 5 - Downstream PR #392 Handoff

After PR #331 is clean and green, confirm PR #392 still expects rebase after #331. Do not start PR #392 implementation inside PR #331.

## AI-Slop Cleanup Plan

Scope is limited to merge-owned edits and review artifacts touched in this session.

- Dead code deletion: only remove code proven unused by compiler/tests or stale conflict residue.
- Duplication: only collapse repeated fixture helpers if tests are green and behavior stays identical.
- Naming/error handling: only adjust names/errors required by current schema or failing tests.
- Test reinforcement: prefer current public behavior tests over implementation-shape assertions.

## Stop Conditions

- Same fix approach fails twice.
- Any unresolved design choice appears in P6-P9.
- Any reviewer blocker remains unresolved.
- Any branch/head/PR claim cannot be verified from source command output.
- Any live-order, deploy, secret, or production action appears.
