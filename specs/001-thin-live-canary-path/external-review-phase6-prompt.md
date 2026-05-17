# External Review Prompt: Phase 6 Submit Admission Plan

Review the Phase 6 plan for bolt-v2 / bolt-v3 submit admission. Be adversarial. Focus on correctness, stale-branch drift, NT boundary violations, fail-closed semantics, and hidden implementation traps. Do not review Phase 7 no-submit readiness or Phase 8 live canary implementation except to flag scope leaks.

## Authoritative State

- Repo: `/Users/spson/Projects/Claude/bolt-v2`
- Current main: `a5c60f2b6a4fe67fc80cf9d234f1512af09bec03`
- `main == origin/main`
- PR #322 merged Phases 3-5 into main.
- PR #323 merged docs/tool hygiene into main.
- PR #316 and #317 are stale stacked drafts, evidence inputs only.

## Phase 6 Target

Submit admission consumes validated `BoltV3LiveCanaryGateReport` before live submit reaches NT.

Required order:

1. decision evidence persistence
2. submit admission check and budget consumption
3. NT `submit_order`

Every entry, exit, and replace-submit candidate consumes one global submit-admission budget. Plain cancel requests are not submits.

## Review Inputs

- `specs/001-thin-live-canary-path/spec.md`
- `specs/001-thin-live-canary-path/plan.md`
- `specs/001-thin-live-canary-path/research.md`
- `specs/001-thin-live-canary-path/data-model.md`
- `specs/001-thin-live-canary-path/contracts/live-canary-gates.md`
- `specs/001-thin-live-canary-path/checklists/phase6-requirements.md`
- `specs/001-thin-live-canary-path/tasks.md`
- Current code surfaces:
  - `src/main.rs`
  - `src/bolt_v3_live_node.rs`
  - `src/bolt_v3_live_canary_gate.rs`
  - `src/bolt_v3_strategy_registration.rs`
  - `src/strategies/registry.rs`
  - `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
  - `src/strategies/binary_oracle_edge_taker.rs`

## Hard Constraints

- Main is source of truth.
- Do not continue from stale stacked branches.
- No hardcodes.
- No dual paths.
- No deferred-debt markers.
- SSM remains only secret source.
- Bolt-v3 stays thin over NautilusTrader.
- NT owns lifecycle, reconciliation, cache, adapter behavior, and order machinery.
- Phase 6 must not implement no-submit readiness or tiny-capital canary proof.
- Runtime-capture behavior around `run_bolt_v3_live_node` must survive Phase 6.

## Questions To Answer

1. Is the Phase 6 plan sufficient to prevent every strategy live submit from reaching NT before consuming the validated gate report?
2. Does the proposed interface avoid provider, market-family, and strategy hardcoding in core?
3. Are count-cap semantics, notional semantics, double-arm/stale-arm behavior, and evidence-failure ordering specified clearly enough for TDD?
4. Does the plan preserve NT ownership and current `run_bolt_v3_live_node` runtime-capture behavior?
5. Is the shared admission state carrier across build, strategy registration, and runner arming specified enough to avoid globals, cloned counters, or stale report reads?
6. What findings must be fixed before implementation approval?

## Output Format

Return severity-ranked findings only:

- Critical/High/Medium/Low
- Evidence: file/section/line or exact requirement text
- Why it matters
- Required fix or clarification

If no findings, say `No findings` and list residual risks.
