# GitHub Issue Payloads: NT-First Research Analytics Platform

Do not create or mutate GitHub issues from this file until explicit user
approval. This file is the `/speckit-taskstoissues` payload draft.

Remote verified: `https://github.com/seungpyoson/bolt-v2.git`.
Live issue state refreshed 2026-05-20 with `gh issue list`/`gh issue view`.
See `issue-audit.md` for commands and search results.

## Existing Issue Relationship Map

| Existing issue | Current relation |
|---|---|
| #19 Data lake lineage metadata | Related to raw-evidence/catalog lineage. Do not duplicate; reuse or link from research runner issue. |
| #20 Canonical normalized lake layout | Existing lake layout work. Provider gate may depend on it, but should not redefine layout. |
| #21 Normalized resolutions with provenance | Existing normalized resolution work. Research analytics can consume it later. |
| #22 Versioned normalized markets dimension | Existing market dimension work. Provider gate should not replace it. |
| #23 NT instrument stream spool bridge | Dependency for instrument/catalog completeness. |
| #24 NT-first data lake follow-on epic | Parent data-lake scope. New issues should link to it and avoid broad ETL duplication. |
| #34 Flexible Polymarket strategy platform | Related strategy consumer. Research runner should not silently expand #34. |
| #36 Auto redemption with realized PnL updates | Dashboard must link or explicitly exclude redemption-realized-PnL scope. |
| #39 Adaptive venue weighting | Future analytics consumer, not baseline platform prerequisite. |
| #75 Offline verified allowlist/research participation | Related research workflow; do not fold into generic research runner without explicit scope. |
| #77 Trade-history/PnL durable path | Dashboard historical PnL depends on it or must label the gap. |
| #88 Deferred Phase 1 strategy-platform reconciliation | PnL/strategy reconciliation context; dashboard must not claim to close it. |
| #112 Kalshi venue integration | Kalshi proof issue should update or depend on this. |
| #115 Hyperliquid HIP-4 outcome contracts | Premise is stale relative to upstream NT `develop`; update/link with selected Bolt-pointer proof. Do not imply closure without approval. |
| #127 Native Polymarket order_book_depths support | Polymarket depth/fidelity proof may depend on this or must label gap. |
| #148 Inline capture sidecar extraction | Capture expansion risk constraint. Provider/dashboard issues must respect it. |
| #158 Sidecar collectors for market data NT adapters drop across all exchanges | Directly relevant to all-exchange market-data capture; Issue C must decide reuse/split/supersede before new provider capture scope. |
| #176 Agent-readiness triage and autonomy roadmap | Agent/tooling readiness context, not research-platform implementation scope. |
| #236 Thin NT rebuild epic | Architecture parent: thinnest layer over NT, no dual paths. |
| #254 Polymarket V2 adoption blockers | Polymarket source readiness constraint. |
| #369 Production-grade live trading readiness beyond tiny-canary | Production observability context; research/dashboard work may feed it but must not imply closure. |
| #385 Real no-order live connectivity test | Live connectivity proof, not historical backtest proof. |
| #407 Controlled Polymarket broad-discovery mode | Polymarket discovery breadth constraint for comprehensive collection. |
| #409 PortfolioSnapshot stream capture | Prerequisite for account-wide MTM/PnL dashboard completeness. |

## Proposed Creation Order

1. Update/link #115 with Issue A. Do not close or deprecate #115 without approval.
2. Update/link #112 with Issue B.
3. Create Issue C; final Kalshi fidelity row depends on Issue B evidence.
4. Create Issue D after Issues A and C have accepted evidence and Issue B has
   either positive Kalshi proof or documented contradiction with fallback scope.
5. Create Issue E after Issues C and D have accepted contracts.
6. Create Issue F to define dashboard source contract and PnL dependency map.
7. Create Issue G after Issue F is accepted. PnL completeness stays blocked by
   #409/#77; MVP may proceed only with omitted/missing-source labels if those
   dependencies are explicitly scoped as gaps.

## Issue A: NT Pointer And Hyperliquid HIP-4 Lifecycle Proof

Title: `Research gate: NT pointer and Hyperliquid HIP-4 lifecycle proof`

Existing relation: update or link #115; link #236. Do not close or deprecate
#115 from this payload.

Problem:

#115 assumes NT does not support HIP-4. Current evidence says upstream NT
`develop` supports HIP-4 instruments, ordinary order submission, settlement
pathways with E-006 caveats, reconciliation, and userOutcome actions. Bolt still
needs selected-pointer proof before using that capability.

Why now:

Research/backtesting architecture must not build a Bolt HIP-4 adapter or custom
execution model if NT already owns the surface. Pointer proof is the gate before
HIP-4 data collection or dashboard assumptions. Historical HIP-4 backtests also
depend on Issue C/E-007 data-fidelity proof.

Evidence rows: E-001, E-002, E-003, E-004, E-005, E-006, E-007, E-021.

Accepted scope:

- Choose and record the NT pointer to prove: current Bolt pin, upstream
  `develop`, or explicit future pin.
- Prove Bolt manifest/feature enablement for required NT crates on that pointer,
  including `nautilus-backtest`, `nautilus-hyperliquid`, `nautilus-tardis`, and
  any selected perps adapter.
- Prove `BacktestNode`/catalog mapping needed for research runs.
- Prove HIP-4 `BinaryOption` instruments and `HyperliquidProductType::Outcome`.
- Prove ordinary HIP-4 order flow uses NT standard order submission.
- Prove settlement, reconciliation, and userOutcome surfaces on selected pointer.
- Keep live adapter support separate from historical replay/data-fidelity proof.

Out of scope:

- Building a Bolt-owned HIP-4 adapter.
- Building a Bolt backtest engine.
- Live production submit.
- Selecting Tardis or another historical provider.

Acceptance evidence:

- Exact Bolt commit and NT pointer.
- Compile/API proof command output.
- Selected pointer satisfies E-001/E-002 `BacktestNode` and catalog APIs.
- Source refs for each HIP-4 surface.
- Lifecycle matrix: instrument load, order submit API, fill/report path,
  settlement, reconciliation, userOutcome.
- Historical-data class marked `SOURCE_PROVEN`, `GAP`, or `DECISION_NEEDED`.
- Draft update/comment text for #115 that records current NT evidence without
  closing or deprecating #115 unless user approves.

## Issue B: Kalshi Adapter And Data-Fidelity Proof

Title: `Research gate: Kalshi NT adapter and historical data fidelity proof`

Existing relation: update or depend on #112.

Problem:

Kalshi adapter support is a user-provided planning assumption. Current checked
NT clone did not source-prove that adapter. Kalshi official historical APIs
prove markets, candlesticks, trades, fills, and orders, but not historical L2
orderbook replay.

Why now:

The platform must plan from Kalshi support, per instruction, but issue
acceptance must say exactly what proof confirms adapter and data-fidelity
surfaces before implementation claims.

Evidence rows: E-008, E-009, E-020.

Accepted scope:

- Preserve Kalshi adapter support as `USER_ASSUMPTION` until pointer/source proof.
- Prove selected NT pointer/source has Kalshi adapter, data client, execution
  client, instruments, account state, fills, reports, and catalog path.
- Classify Kalshi historical data fidelity as `L2_REPLAY`,
  `TRADE_BAR_REPLAY`, `SIGNAL_ONLY`, or `FORWARD_CAPTURE_PENDING`.
- If historical L2 cannot be proven, explicitly downgrade claims and define
  forward-capture requirements.

Out of scope:

- Building a Kalshi adapter before contradiction proof; adapter existence is
  `USER_ASSUMPTION`, not source-proven.
- Live Kalshi order submission.
- Credential or SSM work.

Acceptance evidence:

- Exact source refs for Kalshi adapter surfaces or documented contradiction.
- Kalshi data-class matrix with official/API/provider evidence.
- Backtest claim limits for any non-L2 data class.
- Links to #112 and residual scope left there.

## Issue C: Provider Cost And Fidelity Gate

Title: `Research gate: provider cost and fidelity matrix for NT catalog inputs`

Existing relation: link #19, #20, #21, #22, #23, #24, #127, #148, #158,
#254, #407, Issue A, and Issue B.

Problem:

Flexible backtesting depends on complete-enough data. Tardis, Telonex, Goldsky,
official archives/APIs, and forward capture each cover different venues, data
classes, costs, licensing, and freshness. No provider should be selected from
prose or preference.

Why now:

Tardis is strong for crypto L2 replay but its Professional replay tier is
`$900/month`, leaving little room under the <= `$1000/month` all-in cap for AWS,
dashboard, logs, queries, and reserve. Polymarket/Kalshi need separate license
and fidelity gates.

Evidence rows: E-007, E-009, E-010, E-011, E-012, E-013, E-014, E-015,
E-016, E-019, E-021, E-022, E-024, E-026.

Accepted scope:

- Build all-in monthly run-rate table: provider subscription, AWS storage,
  compute, transfer, logs, query, dashboard, and reserve.
- Build venue/source fidelity matrix for Polymarket, HIP-4, Kalshi, and
  selected perpetual-futures venues.
- Treat `cost-model.md` and `fidelity-matrix.md` artifacts as acceptance
  prerequisites, not optional supporting notes.
- Prove NT Polymarket support plus public API pagination/depth limits before
  classifying Polymarket fidelity or selecting Telonex/Goldsky supplements.
- Decide whether #158 sidecar-collector scope is reused, split, or superseded
  for all-exchange market-data capture before creating new capture work.
- Link Polymarket fidelity decisions to #407 discovery breadth, #127 order-book
  depth support, and #254 V2 readiness instead of duplicating those tracks.
- Consume Polymarket adapter/source proof from accepted evidence-led capability
  rows; this issue classifies provider cost/fidelity against that proof rather
  than creating a second adapter-capability path.
- Prove NT live support surfaces for each selected perpetual-futures venue and
  classify official API/archive capture before selecting any official venue
  path; checked unresolved venue examples are not special architecture paths.
- Prove selected venues through generic TOML/registry binding keys; no provider
  or venue-specific source branches are accepted as part of this issue.
- Keep Kalshi final fidelity classification provisional until Issue B adapter
  and data-surface proof is accepted.
- Classify each source as `L2_REPLAY`, `TRADE_BAR_REPLAY`, `SIGNAL_ONLY`, or
  `FORWARD_CAPTURE_PENDING`.
- Record licensing and commercial-use status.
- Keep every official archive/API path as `GAP` until venue-specific
  official-source proof exists.

Out of scope:

- Provider implementation.
- Dashboard UI.
- Research runner implementation.
- Waiving the monthly cap.

Acceptance evidence:

- Cost model artifact with source links and dated price evidence.
- Fidelity matrix with allowed/forbidden claims per venue/source.
- Evidence that venue/provider choices are represented as TOML-selected
  registry bindings, not hardcoded implementation branches.
- Selection or rejection rationale per provider.
- Explicit waiver request if all-in cost exceeds cap.

## Issue D: NT-First Research Runner And Catalog Lineage

Title: `Build NT-first research runner manifest and catalog lineage MVP`

Existing relation: depends on Issues A and C; Issue B must be accepted either
with positive Kalshi proof or documented contradiction plus explicit fallback
scope. Link #19, #23, #24, #34, #148, and #236.

Problem:

Research needs flexible repeatable backtests, but Bolt must not become a second
backtest engine. The runner should orchestrate NT `BacktestNode`, NT catalog
inputs, and NT reports while preserving raw-data lineage and fidelity limits.

Why now:

Trade readiness is close enough that research/alpha discovery needs a durable
path, but production trading rules still require TOML/NT config and thinnest
layer over NT.

Evidence rows: E-001, E-002, E-010, E-017, E-020, E-025, E-026.

Accepted scope:

- Define `ResearchRunManifest` with direct NT config mapping and TOML-selected
  venue/provider binding keys.
- Define `RawEvidenceRecord` and `CatalogProjection` lineage with hashes,
  source refs, NT pointer, and fidelity class.
- Build the thinnest runner around NT `BacktestNode` only after pointer,
  adapter, provider, fidelity, manifest, and lineage gates pass.
- Emit NT reports/results plus claim-limit metadata.
- Respect #148 capture-reliability risk before claiming raw evidence or catalog
  lineage completeness.
- Keep Python/Jupyter notebooks research-only unless strategy behavior
  graduates into typed production config.

Out of scope:

- New execution engine.
- New venue adapter.
- Production submit path.
- Dashboard UI.

Acceptance evidence:

- Manifest schema and example.
- Proof that switching selected venue/provider updates TOML/registry binding
  data only, not runner core logic.
- Catalog projection schema and example.
- Command or test proving one NT backtest run from catalog input.
- Result artifact includes NT pointer, source hashes, and fidelity claim limits.

## Issue E: Research Analytics Read Model

Title: `Build research analytics read model from NT reports and lineage`

Existing relation: depends on Issues C and D; link #34 and #39 as consumers.

Problem:

Alpha research needs queryable metrics, comparisons, and explanations across
runs. Those analytics must derive from NT reports/results and catalog lineage,
not recompute independent trading truth.

Why now:

The user needs research/data analytics, not just trade readiness. Analytics
should support strategy iteration and later venue weighting without becoming
another production runtime.

Evidence rows: E-002, E-007, E-009, E-010, E-013, E-017, E-020, E-021, E-022,
E-025, E-026.

Accepted scope:

- Define analytics read model for run metadata, source hashes, strategy config,
  reports, metrics, drawdowns, exposure, fills, positions, and fidelity limits.
- Define notebook/query workflow for exploration only; notebooks cannot become
  production runtime, submit path, or strategy authority.
- Add guardrails so lower-fidelity inputs cannot produce execution-quality
  claims.
- Define comparison-query interfaces over stored run results and strategies;
  UI/visualization remains out of scope.

Out of scope:

- Live dashboard UI.
- Order submission.
- Strategy productionization.
- Replacing NT reports.

Acceptance evidence:

- Read-model schema.
- Example query/notebook using stored run results.
- Tests or checks for fidelity claim limits.
- Clear graduation path from research finding to typed production config.

## Issue F: Dashboard Source Contract And PnL Dependency Map

Title: `Define dashboard source contract for current trades, PnL, and outlook`

Existing relation: depends on #409 for account-wide MTM/PnL and #77 for
durable trade-history/PnL. Link #36 for redemption-realized-PnL inclusion or
explicit exclusion. Link #369 as production-readiness context without claiming
closure. Link Issue E and #148. Revisit the field map if Issues A, B, or C
change the accepted source list or fidelity class for any selected venue.

Problem:

The operator needs a visually easy current-trade and outlook dashboard, but the
source contract must come first. Cumulative/historical PnL also touches #77 and,
for redemption-realized-PnL, #36. Production-readiness visibility also touches
#369, but this dashboard source contract must not claim to close #369. The
dashboard must not create a second PnL/account truth or add trading controls.

Why now:

Research analytics and live readiness are converging. Visibility must be
planned from NT-derived truth before UI work starts.

Evidence rows: E-017, E-018, E-019, E-020, E-023, E-028.

Accepted scope:

- Define dashboard source contract for orders, fills, positions, account state,
  portfolio snapshots, PnL, exposure, data health, strategy state, and outlook.
- Mark account-wide MTM/PnL incomplete until #409 or equivalent capture is accepted.
- Mark historical PnL incomplete until #77 or equivalent durable path is accepted.
- Include or explicitly exclude #36 redemption-realized-PnL from dashboard scope.
- Link #369 as production-readiness context and state that dashboard source
  contract work does not close it.
- Mark outlook/strategy-state fields as omitted or non-trading-truth until an
  accepted source contract proves them.
- Show freshness/staleness for every source family.
- Evaluate Grafana, Metabase, Preset/Superset, Plotly/Dash, and bespoke UI
  fallback against source contract, query backend, security, UX, and all-in
  monthly cost.
- Keep mutation capability `none`.

Out of scope:

- Submit/cancel/transfer/credential controls.
- Independent PnL computation that contradicts NT.
- Claiming MTM completeness without `PortfolioSnapshot` capture.
- Dashboard UI implementation.
- Selecting bespoke UI without product-fit evidence.

Acceptance evidence:

- Source contract and field map.
- Dashboard/BI product-fit decision table.
- Dashboard stale-source tests.
- #409/#77/#36/#369 dependency decision table.
- Outlook/strategy-state source decision or explicit omission.

## Issue G: Read-Only Dashboard MVP

Title: `Build read-only current trades, PnL, and outlook dashboard MVP`

Existing relation: depends on Issue F's dependency decision table, dashboard/BI
product-fit decision, and Issue E.
PNL completeness is blocked by #409 and #77. MVP may proceed without those only
if the accepted source contract shows omitted/missing-source labels. Include #36
only if redemption-realized-PnL is in MVP scope; otherwise label it out of scope.
Link #369 as production-readiness context and explicitly state that dashboard
MVP does not close #369.

Problem:

After the source contract is accepted, the operator still needs a visual current
trades dashboard: cumulative/historical PnL, open positions, exposure, data
health, strategy state, and stale-source warnings.

Why now:

Research analytics and live readiness need operator visibility, but UI work must
not run ahead of source-truth and freshness decisions.

Evidence rows: E-017, E-018, E-019, E-020, E-023, E-028.

Accepted scope:

- Build read-only read model and dashboard MVP from accepted source contract
  using the accepted dashboard/BI product path, or bespoke UI only if the
  product-fit gate rejects existing products with evidence.
- Consume the accepted source contract and shared analytics/read-model outputs
  where applicable; do not create a separate PnL or trading-truth path.
- Show source freshness/staleness for every panel.
- Show missing-source labels for any omitted PnL/outlook/strategy-state fields.
- Keep mutation capability `none`.

Out of scope:

- Submit/cancel/transfer/credential controls.
- Independent PnL computation that contradicts NT.
- Claiming historical PnL completeness without #77 or equivalent proof.
- Claiming account-wide MTM completeness without #409 or equivalent proof.
- Building bespoke dashboard UI before product-fit gate acceptance.

Acceptance evidence:

- Rendered dashboard artifact or screenshot.
- Product-fit decision trace showing why the selected dashboard path was chosen.
- Tests for stale source data and missing-source labels.
- Source-hash/freshness proof for each displayed panel.
- Explicit #36 inclusion/exclusion if redemption PnL is shown or hidden.
