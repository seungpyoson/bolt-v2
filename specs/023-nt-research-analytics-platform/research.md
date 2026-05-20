# Research: NT-First Research Analytics Platform

Date: 2026-05-20

## Research Questions

1. Which required surfaces does upstream NT already support?
2. Which data providers fit the venues, data classes, cost cap, and NT catalog path?
3. How should raw evidence, NT catalog, analytics read models, notebooks, and dashboards relate without creating dual truth?
4. Which existing issues already cover pieces of this work?
5. Which claims are source-proven, user assumptions, gaps, or decision points?

## Evidence Authority

`evidence.md` is authoritative for this draft. Any claim below is only valid to
the extent it is backed by a matching row in `evidence.md`.

## NT Capability Evidence

| Surface | Evidence | Decision | Residual gate |
|---|---|---|---|
| Backtesting | NT docs define `BacktestEngine` and `BacktestNode`; docs recommend `BacktestNode` for production workflow and live transition; `BacktestNode` requires Parquet catalog. | Use NT backtest engine. Do not build Bolt simulator. | Prove selected NT pointer compiles and exposes needed Rust/Python APIs. |
| Catalog | NT data docs define `ParquetDataCatalog` as central store with Rust-backed core types and S3-capable storage options. | Use NT catalog projection for replay/backtest data. | Define lineage sidecar if NT records cannot carry source hashes directly. |
| Hyperliquid HIP-4 | Upstream NT `develop` at `dbf4d8c90af06f0f1f1e56d8b5130ada763f1953` supports HIP-4 outcome instruments, USDH settlement, BinaryOption modeling, reconciliation, userOutcome actions, Settlement fill parsing, and ordinary orders through `SubmitOrder`. | Use NT HIP-4 support first. | Prove selected Bolt NT pointer exposes the upstream support; prove historical data separately from live adapter support. |
| Kalshi | User instruction says assume Kalshi adapter support. Checked upstream clone had no `Kalshi|kalshi` hits, so this is not source-proven from that clone. | Use supported Kalshi adapter first. | Prove adapter data classes, market lifecycle, historical depth, fill/report/account surfaces, and fidelity class after pointer/source is identified. |
| Perpetual futures venues | Checked NT docs/source show concrete venue examples, but architecture must stay venue-agnostic. | Use NT native adapters only after venue/product proof. | Historical replay provider choice, official-source proof, current-pointer proof, and data-quality gate per venue. |
| Polymarket | NT Polymarket docs include adapter/loader/fee model evidence and public API cap caveat. | Use NT Polymarket first; supplement only when cap/provenance gap proven. | Verify supplement license/cost. |
| Reports/dashboard source | Upstream NT docs include report/provider surfaces; upstream source exposes `PortfolioSnapshot`. | Dashboard/read model must source live truth from NT reports/events/snapshots. | Survey exact fields and export path; require #409 or equivalent proof before PnL completeness. |

## Provider Evidence

| Provider | Fit | Cost posture | Decision |
|---|---|---|---|
| Tardis | Strong fit for venue-agnostic crypto/perps historical tick replay where NT/Tardis venue support and data classes are source-proven. | Professional is `$900/month`, the first tier with replay APIs; this leaves little room under a `$1000/month` all-in cap. | Select only if total run-rate and per-venue fidelity proof pass or user waives cap. |
| Telonex | Polymarket Parquet data candidate with trades, quotes, book snapshots, and on-chain fills. | Plus is `$79/month` for personal use; commercial/team price is custom. | Use for Polymarket supplement only after license check. |
| Goldsky | Polymarket on-chain fills/positions/events and pipeline sinks. | Usage-based; can fit if controlled. | Use for provenance/on-chain views, not CLOB book replacement. |
| Hyperliquid official archive/API | S3 archive has L2 snapshots and asset contexts, requester-pays transfer, no timeliness/completeness guarantee; API/WS supports capture. | Provider fee low, AWS/transfer risk. | Use as forward-capture or fallback with data-quality gate. |
| Selected official venue API/archive | Not source-proven by default; each venue needs current official-source proof. | Unknown until venue docs and AWS/transfer impact are checked. | Do not use as fallback until official-source proof is added for that venue. |
| Kalshi official/API | Historical endpoints include markets, candlesticks, trades, fills, orders per checked docs. Adapter support assumed. | Provider fee not found in checked docs; capture/storage cost applies. | Use supported adapter; prove whether L2 replay exists or label lower fidelity. |

## Dashboard/BI Product Evidence

| Product path | Fit | Cost posture | Decision |
|---|---|---|---|
| Grafana Cloud / Grafana | Ops observability, freshness, logs, metrics, alerts, and read-only panels over accepted query sources. | Free/Pro active-user and usage-based costs; Enterprise custom/minimum-commit. | Candidate for ops/current-state panels, not independent PnL truth. |
| Metabase | SQL BI over analytics/read-model database; good for quick dashboards and notebooks-adjacent exploration. | Open Source/self-hosted option; managed Starter and Pro can consume budget. | Candidate if read-model DB is SQL-first and cost fits reserve. |
| Preset Cloud / Apache Superset | Managed or self-hosted Superset for SQL dashboards, semantic layer, and governed BI. | Preset has free small-team and paid per-user tiers; Superset shifts cost to AWS/ops. | Candidate when SQL analytics governance matters more than bespoke UI polish. |
| Plotly/Dash | Custom visual app for domain-specific interactions that BI tools cannot express. | Free/Pro/Enterprise tiers; custom app work adds maintenance cost. | Fallback only after BI/observability products fail source-contract or UX needs. |

## Existing Issues

Open related issues found:

- #19 Data lake lineage metadata.
- #20 Canonical normalized lake layout for Athena/DuckDB.
- #21 Normalized resolutions with provenance.
- #22 Versioned normalized markets dimension.
- #23 NT instrument stream spool to normalized instruments.
- #24 NT-first data lake follow-on epic.
- #34 Flexible Polymarket strategy platform.
- #36 Auto redemption with completion verification and realized PnL updates.
- #39 Adaptive venue weighting.
- #77 Trade-history/PnL durable path and repo query tooling.
- #112 Kalshi venue integration.
- #115 Hyperliquid HIP-4 outcome contracts. Its "NT does not support HIP-4" premise is stale relative to upstream NT `develop`.
- #236 Thin NT rebuild epic.
- #409 NT `PortfolioSnapshot` stream for live observability.

Gaps:

- No dedicated NT-first research/backtesting runner issue found.
- No dedicated provider cost/fidelity gate issue found.
- No dedicated current-trades/outlook dashboard issue found. PnL dashboard
  completeness must still link #77, #36, and #409.
- No dedicated dashboard source contract issue found.
- Existing data-lake work does not fully cover research analytics, alpha exploration, or dashboard UX.
- Issue payload drafts now live in `github-issues.md`; creation/mutation still
  waits for explicit user approval.

## External Review Rejections

These findings are inputs, not authority. The authoritative state is
`evidence.md`.

Rejected plan risks:

1. Cost model missing and Tardis can violate cap.
2. Raw lake vs NT catalog needs source-of-truth contract.
3. HIP-4 support must prove full lifecycle, not only order submission.
4. Kalshi plan must not overclaim historical execution-quality backtests.
5. Dashboard must consume NT observability/report surfaces before custom state.
6. Python/Jupyter boundary needs enforcement.
7. Issue slices need dependencies and existing-issue mapping.

Applied changes:

- Added cost/provider gate as Phase 0.
- Added raw evidence -> NT catalog -> NT reports -> analytics/dashboard chain.
- Split HIP-4 live support from HIP-4 historical backtest data.
- Changed Kalshi from adapter-build concern to supported-adapter proof and fidelity classification.
- Added NT observability proof before dashboard implementation.
- Added Python/Jupyter guardrail requirements.
- Added dependency-ordered tasks and issue-slice structure.

## Decision Log

| Decision | Reason | Alternatives Rejected |
|---|---|---|
| Use NT backtest engine, not Bolt engine. | NT already provides `BacktestNode`/`BacktestEngine`; duplicating simulator violates constitution. | Bolt-owned simulator, mock venue world. |
| Treat raw evidence as immutable input and NT catalog as projection. | Preserves auditability without dual runtime truth. | Raw lake as semantic query model, catalog-only with no provenance. |
| Gate Tardis on total monthly cost. | Tardis is strong but can consume cap. | Blind Tardis commitment, weak forward-capture-only claim. |
| Assume Kalshi adapter support. | User provided assumption. | Treat missing upstream search as adapter-build blocker. |
| Build dashboard from NT-derived read model. | Prevents second PnL/account truth. | Dashboard recomputes live trading truth. |
| Evaluate existing dashboard/BI products before custom UI. | User explicitly wants not to overbuild common dashboard/analytics surfaces. | Bespoke dashboard by default. |
| Issue payloads after artifact review. | Existing issues overlap; need clean mapping before mutation. Draft payloads live in `github-issues.md`; creation waits for approval. | Create broad duplicate issues immediately. |
