# Tasks: NT Research Planning Package

These are package reference tasks only. Project implementation tasks live in
the numbered subdirectories.

## Package Reference Tasks

- [x] ROOT-001 Keep root package research/planning-only.
- [x] ROOT-002 Keep original cross-project evidence in `reference/evidence.md`.
- [x] ROOT-003 Fold useful evidence, issue dependencies, and residual risks into
  each numbered project spec/plan.
- [x] ROOT-004 Keep venue/provider fidelity archive in `archive/fidelity-matrix.md`.
- [x] ROOT-005 Keep cost/product review archive in `archive/cost-model.md`.
- [x] ROOT-006 Keep existing issue audit and staged payloads in `archive/`.
- [x] ROOT-007 Split future implementation work into numbered project directories.
- [x] ROOT-008 Provide human triage in `README.md`.
- [x] ROOT-010 Move answered/open question trace into `archive/open-questions.md`.

ROOT-009 is intentionally deferred; see Future Cross-Project Tasks below.

## Project Task Lists

- [Backtesting Engine tasks](1-backtesting-engine/tasks.md)
- [Research Analytics tasks](2-research-analytics/tasks.md)
- [Dashboard tasks](3-dashboard/tasks.md)

## Future Cross-Project Tasks

- [x] ROOT-009 Create a cross-project status/label/legend registry before
  implementation or UI work. It must define each internal enum/status, plain
  display label, tooltip/legend meaning, owner/source of truth, which vertical
  may set it, and which vertical may only display it. Covered concepts include
  fidelity classes, source roles, data status/gap reasons, lifecycle states,
  run purposes, promotion statuses, artifact kinds, proof statuses, warning
  labels, and product-gate outcomes.

## Cross-Project Issue Summaries

Canonical staged payloads remain in `archive/github-issues.md` until the user
approves GitHub mutation. These summaries preserve the two cross-cutting
payloads that are not owned by a numbered implementation project.

### Issue A: Cross-Project NT/Data Evidence Gates

Title: `Research gate: NT/data evidence gates for future backtesting, analytics, and dashboard`

Existing relation: link #19, #20, #21, #22, #23, #24, #112, #115, #127,
#148, #158, #236, #254, #407, and #409.

Evidence rows: E-001, E-002, E-007, E-008, E-009, E-010, E-011, E-013,
E-014, E-015, E-016, E-020, E-021, E-022, E-024, E-027, E-028, E-030,
and E-031.

Accepted scope:

- Record target `bolt-v2` branch NT-version expectations and manifest gaps for
  `nautilus-backtest`, `nautilus-tardis`, and `nautilus-hyperliquid`.
- Prove or classify NT backtest, catalog, report/event/snapshot, Polymarket,
  HIP-4, Kalshi assumption, and perps adapter/source surfaces.
- Classify official Polymarket, Kalshi, Hyperliquid, OKX, Binance, Bybit,
  Tardis, Kaiko, CoinAPI, Amberdata, Telonex, MarketLens, PMXT, PolyBackTest,
  PolymarketData, and Goldsky.
- Finalize and refresh best-first all-in cost scenarios and flag over-target cost for user
  review instead of prematurely weakening architecture.
- Keep Kalshi adapter readiness as `USER_ASSUMPTION`; prove data,
  fidelity, and source contracts separately.
- Keep official live support separate from historical L2 replay claims.

Out of scope: runtime code, provider recorder, backtesting runner, dashboard
UI, and GitHub issue mutation without approval.

Acceptance evidence: updated reference evidence/contracts plus archived
research, fidelity, cost, and issue-audit artifacts with every claim labeled
`SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, or `DECISION_NEEDED`, plus
follow-up adversarial review recorded in the analysis archive.

### Issue E: Approved Task-To-Issues Conversion

Title: `Process: convert approved NT research planning tasks into GitHub issues`

Existing relation: depends on Issues A-D and explicit user approval.

Evidence rows: E-020. Process gate: root stop conditions and CHK022.

Accepted scope:

- Re-run `speckit-analyze` after Issues A-D payloads are accepted.
- Consolidate accepted review findings into canonical artifacts only.
- Run task-to-issues conversion only after user explicitly approves mutation.
- Ensure created issue scope remains one meaningful slice per issue.

Out of scope: creating issues before approval or combining the three future
verticals into one implementation issue.

Acceptance evidence: passing analysis report, explicit user approval recorded
in conversation or issue comment, and created GitHub issues matching the
verified `git remote`.

## Stop Conditions

- Do not implement runtime code from this root package.
- Do not combine the three project task lists into one implementation branch.
- Do not create or mutate GitHub issues without explicit user approval.
