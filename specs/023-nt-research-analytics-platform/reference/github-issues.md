# GitHub Issue Payloads: NT-First Research Planning Package

Do not create or mutate GitHub issues from this file until explicit user
approval. This file stages future `/speckit-taskstoissues` payloads only.

Remote verified: `https://github.com/seungpyoson/bolt-v2.git`.
Live issue state refreshed 2026-05-21 with `gh issue list`/`gh issue view`.
See `issue-audit.md` for command evidence.

## Existing Issue Relationship Map

| Existing issue | Current relation |
|---|---|
| #19 Data lake lineage metadata | Related to raw evidence/catalog lineage. Link, do not duplicate. |
| #20 Canonical normalized lake layout | Existing lake layout work. Do not redefine layout here. |
| #21 Normalized resolutions with provenance | Existing resolution provenance work. Analytics may consume later. |
| #22 Versioned normalized markets dimension | Existing market dimension work. Provider gates should not replace it. |
| #23 NT instrument stream spool bridge | Dependency for instrument/catalog completeness. |
| #24 NT-first data lake follow-on epic | Parent data-lake scope. Avoid broad ETL duplication. |
| #34 Flexible Polymarket strategy platform | Related strategy consumer. Do not silently expand it. |
| #36 Auto redemption with realized PnL updates | Dashboard must include or explicitly exclude redemption-realized PnL. |
| #39 Adaptive venue weighting | Future analytics consumer, not baseline prerequisite. |
| #75 Offline verified allowlist/research participation | Related research workflow; avoid accidental scope merge. |
| #77 Trade-history/PnL durable path | Dashboard historical PnL depends on it or must label gap. |
| #88 Deferred Phase 1 strategy-platform reconciliation | PnL/strategy reconciliation context; dashboard must not claim closure. |
| #112 Kalshi venue integration | Kalshi source/fidelity issue should update or depend on this. |
| #115 Hyperliquid HIP-4 outcome contracts | Premise stale relative to current NT evidence; update/link, no closure without approval. |
| #127 Native Polymarket order_book_depths support | Polymarket depth/fidelity proof may depend on it or must label gap. |
| #148 Inline capture sidecar extraction | Capture expansion risk constraint. |
| #158 Sidecar collectors for data NT adapters drop | Relevant to all-exchange market-data capture; decide reuse/split before new capture scope. |
| #176 Agent-readiness triage and autonomy roadmap | Tooling context, not implementation scope. |
| #236 Thin NT rebuild epic | Architecture parent: thinnest layer over NT, no dual paths. |
| #254 Polymarket V2 adoption blockers | Polymarket source readiness constraint. |
| #369 Production-grade live trading readiness beyond tiny-canary | Observability context; dashboard must not imply closure. |
| #385 Real no-order live connectivity test | Live connectivity proof, not historical backtest proof. |
| #407 Controlled Polymarket broad-discovery mode | Polymarket discovery breadth constraint. |
| #409 PortfolioSnapshot stream capture | Prerequisite for account-wide MTM/PnL dashboard completeness. |

## Proposed Creation Order

1. Issue A: cross-project evidence gates and source contracts.
2. Issue B: Backtesting Engine implementation-ready spec/plan in `1-backtesting-engine/`.
3. Issue C: Research Analytics implementation-ready spec/plan in `2-research-analytics/`.
4. Issue D: Dashboard implementation-ready spec/plan in `3-dashboard/`.
5. Issue E: task-to-issues conversion after A-D are reviewed and user approves
   GitHub mutation.

## Issue A: Cross-Project NT/Data Evidence Gates

Title: `Research gate: NT/data evidence gates for future backtesting, analytics, and dashboard`

Existing relation: link #19, #20, #21, #22, #23, #24, #112, #115, #127, #148,
#158, #236, #254, #407, and #409.

Evidence rows: E-001, E-002, E-007, E-008, E-009, E-010, E-011, E-013,
E-014, E-015, E-016, E-020, E-021, E-022, E-024, E-027, E-028, E-030,
and E-031.

Problem:

Future Backtesting Engine, Research Analytics, and Dashboard work cannot be
implementation-ready until cross-project NT, source, fidelity, and issue-overlap gates
are evidence-backed. Current docs must avoid one merged implementation project.

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
- Keep Kalshi adapter readiness as `USER_ASSUMPTION`; prove data/fidelity/source
  contracts separately.
- Keep official live support separate from historical L2 replay claims.

Out of scope:

- Runtime code.
- Provider recorder.
- Backtesting runner.
- Dashboard UI.
- GitHub issue mutation without approval.

Acceptance evidence:

- Updated `evidence.md`, `research.md`, `fidelity-matrix.md`, `cost-model.md`,
  `issue-audit.md`, and `contracts.md`.
- Every claim labeled `SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, or
  `DECISION_NEEDED`.
- Follow-up adversarial review recorded in `analysis.md`.

## Issue B: Backtesting Engine Spec And Plan

Title: `Plan: NT-first backtesting engine spec for flexible venue/data replay`

Existing relation: depends on Issue A; link #19, #23, #24, #34, #112, #115,
#127, #148, #158, #236, #254, and #407.

Evidence rows: E-001, E-002, E-003, E-004, E-005, E-006, E-007, E-008,
E-009, E-010, E-011, E-012, E-013, E-015, E-016, E-021, E-022, E-024,
E-026, E-027, E-029, E-030, and E-032.

Problem:

Backtesting must be flexible across Polymarket, HIP-4, Kalshi, and perps without
building a second engine. Future implementation needs a concrete NT-native spec
before code.

Accepted scope:

- Define implementation-ready spec/plan in `1-backtesting-engine/` for thinnest
  layer over NT
  `BacktestNode`, `BacktestEngine`, `BacktestRunConfig`, `BacktestDataConfig`,
  `BacktestVenueConfig`, and `ParquetDataCatalog`.
- Define run manifest, venue/provider bindings, execution model, catalog
  projection, result contract, claim gates, and backtesting extension-surface
  matrix in the Backtesting Engine project docs.
- Require two venue/provider binding fixtures proving swaps are TOML/registry
  data changes only.
- Define fill/fee/slippage/latency ownership in NT terms.
- Define claim limits for `L2_REPLAY`, `TRADE_BAR_REPLAY`, `SIGNAL_ONLY`, and
  `FORWARD_CAPTURE_PENDING`.

Out of scope:

- Building the runner.
- Adding NT crates to Cargo.
- Provider downloads or sample transforms.
- Live submit/cancel.

Acceptance evidence:

- `1-backtesting-engine/spec.md`, `1-backtesting-engine/plan.md`, and
  `1-backtesting-engine/tasks.md`.
- Cross-project contract coverage in `contracts.md`.
- Tasks scoped to future Backtesting Engine session only.
- Spec Kit analysis shows no runtime task in this research package.

## Issue C: Research Analytics Spec And Plan

Title: `Plan: NT-derived research analytics and alpha workflow spec`

Existing relation: depends on Issue A; link #19, #20, #21, #22, #24, #34, #39,
#75, #148, #158, #176, #236, and #407.

Evidence rows: E-002, E-014, E-015, E-016, E-017, E-020, E-024, E-025,
E-026, E-029, E-030, and E-031.

Problem:

Alpha research needs Python/Jupyter, experiments, analytics, and flexible data
joins, but must not become a production runtime or second trading truth.

Accepted scope:

- Define implementation-ready spec/plan in `2-research-analytics/`.
- Define raw evidence and deterministic projection lineage.
- Define point-in-time correctness rules and lookahead-leakage checks.
- Define experiment metadata: parameters, metrics, artifacts, source hashes,
  NT pointer, strategy config hash, data source binding, and fidelity class.
- Define notebook boundary: research-only, no submit/cancel/transfer/credential
  mutation, no production authority.
- Define promotion path from research finding to typed TOML/NT-compatible runtime
  contract.
- Define analytics read model as derived from NT reports/results and catalog
  lineage.

Out of scope:

- Building analytics DB/read model.
- Notebook implementation.
- Strategy productionization.
- Replacing NT reports.

Acceptance evidence:

- `2-research-analytics/spec.md`, `2-research-analytics/plan.md`, and
  `2-research-analytics/tasks.md`.
- Reference source notes in `research.md` and cross-project contract coverage in
  `contracts.md`.
- Explicit forbidden claims for lower-fidelity inputs.
- Spec Kit analysis shows analytics tasks are planning-only.

## Issue D: Dashboard Spec And Plan

Title: `Plan: read-only current trades, PnL, outlook dashboard source contract`

Existing relation: depends on Issue A; link #36, #77, #88, #148, #236, #369,
and #409.

Evidence rows: E-017, E-018, E-019, E-020, E-023, E-024, E-026, E-028,
E-029, and E-031.

Problem:

Operator needs visual current trades, outlook, cumulative/historical PnL,
exposure, data health, and strategy state. Dashboard must stay read-only and
must not create second PnL/account truth.

Accepted scope:

- Define implementation-ready spec/plan in `3-dashboard/`.
- Define field-by-field source matrix for orders, fills, positions, account
  state, portfolio snapshots, PnL, exposure, data health, strategy state, and
  outlook.
- Mark account-wide MTM/PnL incomplete until #409 or equivalent
  `PortfolioSnapshot` capture is accepted.
- Mark historical PnL incomplete until #77 or equivalent durable path is
  accepted.
- Include or explicitly exclude #36 redemption-realized PnL.
- Link #369 as production-readiness context and state dashboard source contract
  does not close it.
- Evaluate Grafana, Metabase, Superset/Preset, Retool, Plotly/Dash, and custom fallback
  before bespoke UI.
- Require no submit/cancel/transfer/credential/runtime mutation controls.

Out of scope:

- Building dashboard UI.
- Building dashboard read model.
- Independent PnL calculation.
- Production readiness closure.

Acceptance evidence:

- `3-dashboard/spec.md`, `3-dashboard/plan.md`, and `3-dashboard/tasks.md`.
- Reference source notes in `research.md` and cross-project contract coverage in
  `contracts.md`.
- Product-fit decision table with source-contract/security/query/UX/cost notes.
- #409/#77/#36/#369 dependency decision table.

## Issue E: Approved Task-To-Issues Conversion

Title: `Process: convert approved NT research planning tasks into GitHub issues`

Existing relation: depends on Issues A-D and explicit user approval.

Evidence rows: E-020. Process gate: root `tasks.md` stop conditions and
`requirements-checklist.md` CHK022.

Problem:

Spec Kit can convert tasks to GitHub issues, but mutation must not happen until
the staged payloads are reviewed and approved.

Accepted scope:

- Re-run `speckit-analyze` after Issues A-D payloads are accepted.
- Consolidate accepted review findings into canonical artifacts only.
- Run task-to-issues conversion only after user explicitly approves mutation.
- Ensure created issue scope remains one meaningful slice per issue.

Out of scope:

- Creating issues before approval.
- Combining the three future verticals into one implementation issue.

Acceptance evidence:

- Passing analysis report in `analysis.md`.
- User approval recorded in conversation or issue comment.
- Created GitHub issues match `git remote`.
