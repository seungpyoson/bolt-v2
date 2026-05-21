# Research: NT-First Research Planning Package

Date: 2026-05-21

## Research Questions

1. Which required surfaces does upstream NT already support?
2. Which data providers fit the venues, data classes, source contracts, cost
   review, and NT catalog path?
3. How should raw evidence, NT catalog, analytics read models, notebooks, and dashboards relate without creating dual truth?
4. Which existing issues already cover pieces of this work?
5. Which claims are source-proven, user assumptions, gaps, or decision points?
6. What must be split into the future Backtesting Engine, Research Analytics,
   and Dashboard projects?

## Evidence Authority

`evidence.md` is authoritative for this draft. Any claim below is only valid to
the extent it is backed by a matching row in `evidence.md`.

## Artifact Storage Evidence

Canonical raw payloads, NT catalog projections, source proof artifacts, and
backtest outputs share one TOML/config-owned S3 `artifact_root`. Typed subpaths
under the root separate artifact kinds: `raw/`, `nt-catalog/`,
`source-proofs/`, and `backtests/`. Local storage is cache only.

Default lifecycle is retain forever with no automatic delete. Lifecycle profiles
move artifacts between active, archive, and deep-archive storage based on
configured tags/windows. Deep archive is very low cost, but not zero cost and
not immediately readable.

Lifecycle stays simple in this planning package: artifacts start `active`; after
the configured quiet window passes, they become `inactive`; inactive artifacts
may move colder but are not deleted.

For planning, archive storage under `$5/month` is treated as zero. Restore,
request, metadata, minimum-duration, and retrieval costs still remain explicit.

## NT Capability Evidence

| Surface | Evidence | Decision | Residual gate |
|---|---|---|---|
| Backtesting | NT docs define `BacktestEngine` and `BacktestNode`; docs recommend `BacktestNode` for production workflow and live transition; `BacktestNode` requires Parquet catalog. | Use NT backtest engine. Do not build Bolt simulator. | Prove the NT version resolved by the target `bolt-v2` branch compiles and exposes needed Rust/Python APIs. |
| Catalog | NT data docs define `ParquetDataCatalog` as central store with Rust-backed core types and S3-capable storage options. | Use NT catalog projection for replay/backtest data. | Define lineage sidecar if NT records cannot carry source hashes directly. |
| Hyperliquid HIP-4 | The NT version selected by the target `bolt-v2` implementation branch must be checked for HIP-4 outcome instruments, USDH settlement, BinaryOption modeling, reconciliation, userOutcome actions, Settlement fill parsing, and ordinary orders through `SubmitOrder`; upstream `develop` is drift evidence only. | Use NT HIP-4 support first when present in the `bolt-v2`-selected NT version. | Prove selected crate/features before implementation and prove historical data separately from live adapter support. |
| Kalshi | User instruction says assume Kalshi adapter support. | Use supported Kalshi adapter first. | Prove selected Kalshi data classes, historical depth, fill/report/account source contracts, and fidelity class; do not make adapter build feasibility a blocker in this package. |
| Perpetual futures venues | Checked NT docs/source show concrete venue examples, but architecture must stay venue-agnostic. | Use NT native adapters only after venue/product proof. | Historical replay provider choice, official-source proof, target `bolt-v2` NT-version proof, and data-quality gate per venue. |
| Polymarket | NT Polymarket docs include adapter/loader/fee model evidence and public API cap caveat. | Use NT Polymarket first; supplement only when cap/provenance gap proven. | Verify supplement license/cost. |
| Reports/dashboard source | Upstream NT docs include report/provider surfaces; upstream source exposes `PortfolioSnapshot`. | Dashboard/read model must source live truth from NT reports/events/snapshots. | Survey exact fields and export path; require #409 or equivalent proof before PnL completeness. |

## Provider Evidence

| Provider/source | Fit | Cost posture | Decision |
|---|---|---|---|
| Tardis | Strong fit for venue-agnostic crypto/perps historical tick replay where NT/Tardis venue support and data classes are source-proven. | Model all-in cost; do not reject solely on cost before user review. | Candidate for best-fidelity perps path after sample-to-NT-catalog proof. |
| Kaiko / CoinAPI / Amberdata | Paid crypto market-data alternatives with historical orderbook/trade products. | Paid vendor costs and license terms must be refreshed at selection. | Candidate alternatives or supplements when Tardis/official archive fit fails. |
| OKX official historical data | Official page lists trade history, candles, funding rates, and high-resolution L2 order book data. | Provider fee may be zero; storage/transfer/normalization still apply. | Strong official-source candidate for OKX products after schema/sample proof. |
| Binance Data Vision | Official public data repo shows trades, aggTrades, and klines; no full historical L2 proof from checked source. | Low provider fee; limited fidelity. | Trade/bar replay candidate, not L2 replay unless separate depth archive proof appears. |
| Bybit official API/data | V5 docs prove current orderbook snapshots and recent/history order endpoints; checked docs do not prove official historical L2 replay. | Unknown until data-download schema is proven. | Current/live or lower-fidelity candidate until historical L2 source proof. |
| Hyperliquid official archive/API | S3 archive has L2 snapshots and asset contexts, requester-pays transfer; HIP-4 outcome coverage still needs proof. | Provider fee low, AWS/transfer risk. | Use as candidate historical/fallback path after outcome coverage and quality gates. |
| Kimchi premium / Korean spot prices | Cross-market source family using TOML-selected Korean spot token prices such as Upbit/Bithumb, reference spot/perps prices, and FX/quote conversion. | Cost/license unknown until selected sources are proved. | Candidate research/backtest signal source; not execution-quality by itself and not a hardcoded venue path. |
| Kalshi official/API | Historical endpoints include markets, candlesticks, trades, fills, orders; current orderbook exists separately. | Provider fee not found in checked docs; capture/storage cost applies. | Trade/bar/fill replay candidate; prove whether historical L2 exists or label lower fidelity. |
| Polymarket official/API | Official docs prove current orderbook, WebSocket book/deltas/trades, authenticated trades, and price history. | Provider fee low; historical depth/cap limits matter. | Use for live/current and price/trade history; do not claim official historical L2 replay. |
| Telonex | Polymarket Parquet data candidate with trades, quotes, book snapshots, on-chain fills, and daily updates. | Commercial/team price and license need proof. | Candidate for Polymarket supplement after schema/license/sample proof. |
| MarketLens | Polymarket historical L2 snapshots/deltas/reconstruction API candidate. | Pricing/license needs proof. | Candidate for Polymarket L2 replay after sample and license proof. |
| PMXT | Free hourly Parquet orderbook archive candidate for Polymarket and Kalshi. | Download/storage cost can be high; license/support posture needs proof. | Candidate research archive after schema, coverage, freshness, and reliability proof. |
| PolyBackTest / PolymarketData | Productized prediction-market data/backtesting APIs with limited retention/coverage by plan. | Paid plan/retention limits need proof. | Candidate for fast research, not canonical until retention/source-contract gates pass. |
| Goldsky | Polymarket on-chain fills/positions/events and pipeline sinks. | Usage-based; can fit if controlled. | Use for provenance/on-chain views, not CLOB book replacement. |

## Dashboard/BI Product Evidence

| Product path | Fit | Cost posture | Decision |
|---|---|---|---|
| Grafana Cloud / Grafana | Ops observability, freshness, logs, metrics, alerts, and read-only panels over accepted query sources. | Free/Pro active-user and usage-based costs; Enterprise custom/minimum-commit. | Candidate for ops/current-state panels, not independent PnL truth. |
| Metabase | SQL BI over analytics/read-model database; good for quick dashboards and notebooks-adjacent exploration. | Open Source/self-hosted option; managed Starter and Pro can consume budget. | Candidate if read-model DB is SQL-first and cost fits reserve. |
| Preset Cloud / Apache Superset | Managed or self-hosted Superset for SQL dashboards, semantic layer, and governed BI. | Preset has free small-team and paid per-user tiers; Superset shifts cost to AWS/ops. | Candidate when SQL analytics governance matters more than bespoke UI polish. |
| Retool | Internal-tool/app builder over accepted SQL/API sources and operator workflows. | Free/Team/Business/Enterprise tiers; Business tier includes stronger controls such as audit logging and permissions per official pricing page. | Candidate for internal operator workflows if source-contract, permissions, auditability, and cost pass product gate. |
| Plotly/Dash | Custom visual app for domain-specific interactions that BI tools cannot express. | Free/Pro/Enterprise tiers; custom app work adds maintenance cost. | Fallback only after BI/observability products fail source-contract or UX needs. |

## Prior Art Evidence

| Source | Relevant pattern | Decision for this package |
|---|---|---|
| QuantConnect Lean | Mature engine separates local research, backtest, optimize, and live commands. | Keep Backtesting Engine, Research Analytics, and live trading promotion as separate lifecycle gates. |
| NT, QuantConnect, Freqtrade, Qlib, MLflow | Backtest/experiment systems record metrics, params, reports, artifacts, and lineage. Deployment or promotion status lives in a separate live-deployment, registry, tag, alias, or review workflow. | `BacktestResultContract` is an objective evidence/lookup contract. Strategy escalation belongs to Research Analytics `PromotionPackage` or a later review artifact, not the raw backtest result. |
| Microsoft Qlib | Quant research workflow spans data processing, model training, backtesting, alpha seeking, risk modeling, portfolio optimization, and analysis. | Research Analytics plan needs experiment metadata, metrics, artifacts, and promotion package, not just notebooks. |
| Freqtrade lookahead analysis | Backtesting can be invalidated when indicators or signals see future data; validation compares baseline and sliced backtests. | Require point-in-time rules and leakage checks for analytics features and notebooks. |
| Feast | Feature sets need point-in-time correctness to prevent future values leaking into training. | Analytics joins require as-of timestamps, source hashes, and claim limits. |

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

- No dedicated future Backtesting Engine spec/plan issue found.
- No dedicated future Research Analytics spec/plan issue found.
- No dedicated future Dashboard spec/plan issue found.
- No dedicated provider cost/fidelity gate issue found.
- No dedicated current-trades/outlook dashboard issue found. PnL dashboard
  completeness must still link #77, #36, and #409.
- No dedicated dashboard source contract issue found.
- Existing data-lake work does not fully cover research analytics, alpha exploration, or dashboard UX.
- Issue payload drafts now live in `github-issues.md`; creation/mutation still
  waits for explicit user approval.

## Review Challenges

These findings are inputs, not authority. The authoritative state is
`evidence.md`.

Rejected plan risks:

1. Cost model missing and provider costs can exceed target.
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
| Model cost after best-fidelity architecture. | Cost is a review/cut lever, not first-pass architecture limiter. | Blind Tardis commitment, weak forward-capture-only claim, premature budget-driven compromise. |
| Assume Kalshi adapter support. | User provided assumption. | Treat missing upstream search as adapter-build blocker. |
| Build dashboard from NT-derived read model. | Prevents second PnL/account truth. | Dashboard recomputes live trading truth. |
| Evaluate existing dashboard/BI products before custom UI. | User explicitly wants not to overbuild common dashboard/analytics surfaces. | Bespoke dashboard by default. |
| Split future work into three projects. | User does not want backtesting, analytics, and dashboard consolidated into one implementation project. | One broad implementation project. |
| Issue payloads after artifact review. | Existing issues overlap; need clean mapping before mutation. Draft payloads live in `github-issues.md`; creation waits for approval. | Create broad duplicate issues immediately. |
