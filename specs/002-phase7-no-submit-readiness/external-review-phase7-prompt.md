# Phase 7 External Review Prompt

Review-only. Do not edit files.

Scope:

- `specs/002-phase7-no-submit-readiness/spec.md`
- `specs/002-phase7-no-submit-readiness/checklists/requirements.md`
- `specs/002-phase7-no-submit-readiness/checklists/phase7-requirements.md`
- `specs/002-phase7-no-submit-readiness/plan.md`
- `specs/002-phase7-no-submit-readiness/research.md`
- `specs/002-phase7-no-submit-readiness/data-model.md`
- `specs/002-phase7-no-submit-readiness/contracts/no-submit-readiness.md`
- `specs/002-phase7-no-submit-readiness/quickstart.md`
- `specs/002-phase7-no-submit-readiness/tasks.md`

Context:

- Current source of truth is `main == origin/main == d6f55774c32b71a242dcf78b8292a7f9e537afab`.
- PR #324 merged Phase 6 submit admission.
- PRs #318/#319/#320 are closed stale/superseded and may be read only as forensic input.
- This branch must not merge, rebase, or continue stale stacked branches.
- Phase 7 target is authenticated no-submit readiness evidence.
- No live capital, no soak, no real order placement.

Review questions:

1. Does the Phase 7 plan preserve the thin NautilusTrader boundary?
2. Does it avoid stale `BoltV3BuiltLiveNode` / `node_mut` design from PR #319?
3. Does it keep SSM as the only secret source and avoid environment secret fallback?
4. Does it clearly fail closed for missing approval, SSM failure, venue auth/geoblock, wrong market/instrument, stale data, missing reference readiness, malformed report, or unsatisfied stage?
5. Does it prevent submit, cancel, replace, amend, subscribe, and runner-loop behavior in Phase 7?
6. Does it keep Phase 8 live-capital action blocked pending real no-submit report plus strategy-input safety audit?
7. Are tasks TDD vertical slices, not horizontal test dump?
8. Are there missing blocking requirements before implementation?

Required output:

- APPROVE only if implementation may begin after all three required reviewers approve.
- REQUEST_CHANGES for any blocking issue.
- Findings first, severity-ranked, with file/section references.
