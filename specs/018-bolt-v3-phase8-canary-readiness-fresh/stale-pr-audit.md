# Phase 8 Stale PR Audit

## Source Of Truth

- Current main/origin main: `d6f55774c32b71a242dcf78b8292a7f9e537afab`
- Phase 8 planning branch: `018-bolt-v3-phase8-canary-readiness-fresh`
- Rule: #318/#319/#320 are forensic input only. Do not merge, rebase, or continue them.

## PR #318: Phase 7 Plan

- State: closed, unmerged, draft.
- Base: `012-bolt-v3-phase6-submit-admission` at `d365bbbb78190653f67163fa61aecc4ad8d0d476`.
- Head: `013-bolt-v3-phase7-no-submit-readiness-plan` at `462333d5fdc94289467b2220637036216d69e1e3`.
- Files: `docs/superpowers/plans/2026-05-13-bolt-v3-phase7-no-submit-readiness.md`.

Classification:

- Valid but stale: no-submit readiness must be thin over existing build and NT connect/disconnect; report must be accepted by live canary gate; ignored real operator harness requires approval.
- Already superseded locally but not main: newer Phase 7 fresh-main work exists on local branch `017-bolt-v3-phase7-no-submit-readiness-fresh`, not current main.
- Stale due stacked base drift: the PR base is not current main and is closed.
- Do not port: detailed old module shape (`bolt_v3_no_submit_readiness_schema`) because fresh Phase 7 branch used a smaller schema path and NT-cache reference readiness after revised review.

Recommendation:

- Keep closed. Do not salvage branch. Use only as historical requirements evidence.

## PR #319: Phase 7 Implementation

- State: closed, unmerged, draft.
- Base: PR #318 branch.
- Head: `014-bolt-v3-phase7-no-submit-readiness` at `729acf795d2c1d0ae66753a78574adf77a25dc67`.
- Files include old Phase 7 modules, live canary gate edits, tests, quickstart, and runtime literal audit.

Classification:

- Valid but stale: local no-submit producer, redacted report writing, ignored operator harness, zero-order fences.
- Already superseded locally but not main: local Phase 7 fresh branch implements revised NT-cache reference readiness and stop semantics.
- Stale due stacked base drift: closed PR stacked on closed #318, not current main.
- Do not port: any implementation detail that assumes old controlled-connect-only readiness instead of revised `LiveNode::start`/`stop` Phase 7 design.

Recommendation:

- Keep closed. Do not merge or rebase. Use only as comparison evidence for Phase 7 dependency.

## PR #320: Phase 8 Plan

- State: closed, unmerged, draft.
- Base: PR #319 branch.
- Head: `015-bolt-v3-phase8-tiny-canary-plan` at `b15f9f5549b7ffb4923bcb0fe8dfdbbdb621fb25`.
- Files: `docs/superpowers/plans/2026-05-13-bolt-v3-phase8-tiny-capital-canary.md`.

Classification:

- Valid and missing from main: Phase 8 must not start live action until no-submit readiness evidence exists; canary is production path with tiny caps; local tests must prove fail-closed preconditions; ignored operator harness must require exact approval inputs; evidence must include cap/config/approval identity and NT lifecycle result.
- Already present on current main: Phase 6 live canary gate and submit admission are present; strategy submit path already records decision evidence before admission and NT submit.
- Stale due stacked base drift: plan assumes PR #319 Phase 7 files exist. Current main lacks them.
- Wrong design / should not port: suggested evidence enum variants that imply Bolt can classify venue accepted/filled/reconciled without specifying the NT event/report source. Fresh plan must require NT evidence references instead of synthesized outcome labels.

Recommendation:

- Keep closed. Salvage requirements only into fresh speckit artifacts. Do not port branch files or old task numbering.

## Phase 8 Scope That Remains

- Preflight and dry/no-submit evidence on fresh current-main branch.
- Strategy-input safety audit before any live action.
- Ignored operator harness skeleton only after local tests.
- Live-order execution remains stopped.

## Phase 9 Scope Proposed

- Comprehensive audit after Phase 7/8 local readiness: hardcodes, dual paths, deferred-work debt, brittle code, NT boundary, SSM-only secrets, pure Rust, config grouping, stale branches/docs/specs/tasks, source fences, tests, external review disposition, production-readiness gaps, strategy math/feed assumptions, live ops readiness.
