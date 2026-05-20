# Implementation Plan: NT-First Research Analytics Platform

**Branch**: `023-nt-research-analytics-platform` | **Date**: 2026-05-20 | **Spec**: `specs/023-nt-research-analytics-platform/spec.md`

## Summary

Build no new backtesting engine. Plan and implement a thin research/backtesting/analytics layer around NautilusTrader:

1. Prove or classify NT support for backtesting, catalog, Hyperliquid HIP-4,
   Kalshi, Polymarket, selected perpetual-futures venues, reporting, and
   dashboard source data.
2. Select data providers through cost and fidelity gates.
3. Use NT catalog and NT reports/results as canonical projections.
4. Use Python/Jupyter only for research, not production trading.
5. Build read-only dashboard/read models from NT-derived sources.
6. Convert work into small issue slices with dependencies and existing-issue links.

## Technical Context

**Language/Runtime**: Rust production binary; Python/Jupyter allowed for research-only workflows.

**Primary Dependency**: NautilusTrader. Current Bolt pin is `7c2aafb30fb143069c915a3f2057bb12174405f6`; upstream `develop` was checked at `dbf4d8c90af06f0f1f1e56d8b5130ada763f1953`.

**Storage**: NT `ParquetDataCatalog` for backtest/replay data. Raw evidence store for immutable provider payloads. Analytics/read model derived from NT reports/results and catalog lineage.

**Secrets**: AWS SSM through Rust AWS SDK only for live credentials.

**Config**: TOML or NT-native config. No hardcoded runtime values, venue IDs,
provider IDs, product IDs, account IDs, or adapter binding choices.

**Providers Under Review**: Tardis, Telonex, Goldsky, official Hyperliquid
archives/API, official Kalshi data/API, and venue-specific official
archives/APIs for selected perpetual-futures venues. Official capture remains a
per-venue `GAP` until current official-source proof is added.

**Dashboard/BI Product Gate**: Evaluate Grafana for ops metrics/logs,
Metabase/Preset/Superset for SQL analytics/read-model dashboards, and
Plotly/Dash only for custom visual app needs. Custom UI is fallback, not
default. Selection must pass source-contract, security, query-backend, UX, and
all-in monthly cost gates.

## Constitution Check

- **NT-first thin layer**: PASS. Backtest engine, adapter semantics, catalog types, portfolio/order/fill state, and reports remain NT-owned.
- **Generic core, concrete edges**: PASS. Venues/providers live in registry/config decisions, not core runtime loops.
- **Single path/config-controlled runtime**: PASS. Runtime values must come from TOML or NT-native config; live credentials from SSM only.
- **Evidence before claims**: PASS WITH GATES. NT support, provider cost, and fidelity must be proven before implementation.
- **Minimal slice discipline**: PASS. Work decomposed into gates, support proofs, provider prototypes, and platform MVPs.
- **Research analytics amendment**: PASS. Dashboards/read models remain read-only and must not create second PnL truth.

## Sources Checked

- `evidence.md`: controlling source ledger for all plan claims.
- NT upstream `develop` at `dbf4d8c90af06f0f1f1e56d8b5130ada763f1953`: HIP-4, PortfolioSnapshot, Polymarket, Tardis, selected venue adapter examples, and backtest/catalog surfaces.
- NT Hyperliquid docs: HIP-4 product support and standard order path for ordinary outcome side-token orders.
- NT backtesting docs: `BacktestNode` recommended for production workflows and requires Parquet catalog.
- NT data docs: `ParquetDataCatalog`, S3 support, `BacktestDataConfig`, Rust-backed core data types.
- NT Tardis docs: normalized data mapping, venue mappings, replay to NT catalog-compatible Parquet.
- NT selected venue docs: checked concrete venue examples only as evidence instances; use the generic venue gate before any venue-specific implementation claim.
- NT Polymarket docs: loader, fee model, public API cap caveat.
- Provider pages: Tardis, Telonex, Goldsky, Hyperliquid historical S3, Kalshi historical API.
- Existing GitHub issues: #19, #20, #21, #22, #23, #24, #34, #36, #39, #75, #77, #88, #112, #115, #127, #148, #158, #176, #236, #254, #369, #385, #407, #409.

## Architecture

### Data Flow

```text
Provider/API/archive
  -> raw evidence record
  -> deterministic transform/replay
  -> NT catalog projection
  -> NT BacktestNode / live NT reports
  -> analytics/read model
  -> read-only dashboard and notebooks
```

### Source Of Truth

Raw evidence is immutable audit input. NT catalog is canonical replay/backtest projection. NT reports/results are trading truth for PnL, positions, orders, fills, and account state. Analytics/read model is derived and never authoritative.

### Run Manifest

Run manifest is a thin map to NT config:

- venue/provider selection
- instrument IDs
- data classes
- time range
- fill model
- fee model
- strategy config reference
- catalog path
- output path
- fidelity class
- lineage hash

Fields without NT equivalent are Bolt orchestration metadata and must be marked.

### Fidelity Classes

- **L2 replay**: historical order-book data exists and NT fill/matching model can use it.
- **Trade/bar replay**: trades or bars only; execution-quality claims forbidden.
- **Signal-only**: no execution simulation; feature/alpha research only.
- **Forward-capture pending**: recorder started, but historical depth not yet sufficient.

## Venue Plan

### Hyperliquid HIP-4

Use upstream NT HIP-4 support. Do not build Bolt HIP-4 order/data path before proof.

Proof matrix:

- load HIP-4 instruments
- subscribe to data
- submit/cancel ordinary orders in safe test/dry path
- parse order fills, partial fills, and settlement fills
- reconcile USDH account state
- verify split/merge/mergeQuestion/negate access or mark out of scope
- verify risk, notional, position, and settlement constraints
- verify historical HIP-4 outcome data source separately from live adapter support

### Kalshi

Assume adapter support exists per user instruction. This is a `USER_ASSUMPTION`
until the exact pointer/source is identified. Use supported adapter first.

Proof matrix:

- load Kalshi market/instrument definitions
- map outcomes/resolution lifecycle into NT-compatible vocabulary
- subscribe or capture supported data classes
- define which historical sources support L2, trade/bar, signal-only, or forward-capture mode
- verify order/fill/account/report surfaces if live trading enters scope
- avoid Bolt-specific adapter glue unless proof shows missing surface

### Perpetual Futures Venues

Use NT native adapters for live surfaces only after venue/product proof. Use
Tardis replay into NT catalog only if cost/fidelity gates pass. Use official
forward capture as fallback only after official-source proof; until then,
official venue capture remains `GAP` per selected venue.

### Polymarket

Use NT Polymarket loader/adapter/fee model first. Use Telonex or Goldsky only for proven public API cap, historical depth, on-chain provenance, or CLOB data gaps. Verify Telonex license before team/commercial use.

## Cost Plan

Cost gate must compare at least three modes:

1. **Tardis replay mode**: strong perps L2 replay candidate where NT/Tardis venue support and data classes are source-proven; Professional replay access is `$900/month`, so all-in cost must be proven under cap or waived.
2. **Budget-safe prediction/on-chain mode**: Telonex personal Plus is `$79/month`; commercial/team use needs custom price. Goldsky is metered. Strong for Polymarket/on-chain provenance, weak for perps historical depth.
3. **Official capture mode**: low provider cost; forward-looking only for venues without complete archives.

Every mode must include:

- provider subscription
- S3 storage
- catalog storage
- backtest compute
- recorder compute
- dashboard/BI
- logs/metrics
- Athena/query costs
- transfer costs
- reserve

## Phase Plan

### Phase 0: Gates

1. Evidence-ledger gate.
2. Cost/provider gate.
3. NT pin/delta gate.
4. Source-of-truth and lineage gate.

### Phase 1: NT Support Proofs

4. HIP-4 lifecycle proof.
5. Kalshi adapter/data/backtest proof.
6. NT observability proof.
7. NT Tardis/catalog proof.
8. NT Polymarket proof.

### Phase 2: Provider And Venue Prototypes

9. Perps data prototype.
10. Kalshi data/fidelity prototype.
11. Polymarket supplement prototype.

### Phase 3: Platform MVPs

12. Thin NT BacktestNode runner.
13. Analytics/read model MVP.
14. Read-only ops dashboard MVP.
15. Research notebook boundary and sample workflow.

## Complexity Tracking

No constitution violation accepted.

Known complexity:

- Dual storage surfaces are allowed only as evidence/projection/read-model chain, not dual runtime truth.
- Python/Jupyter is allowed only with CI/import guard before production adjacency.
- Multiple dashboard products are not allowed in v1; choose one ops dashboard path first.
- Kalshi adapter support is assumed; work focuses on proof and data fidelity, not adapter invention.

## Verification Strategy

- Inspect exact NT pointer and release/diff evidence.
- Run compile/config proof before depending on new NT pointer.
- For provider work, produce sample catalog artifact or source-free proof when credentials unavailable.
- For analytics/dashboard, prove source field comes from NT report/event/snapshot or mark exploratory.
- For tasks/issues, run checklist against spec, plan, research, data model, and task dependencies.

## Deliverables

- `spec.md`: feature requirements and user stories.
- `plan.md`: NT-first architecture and phases.
- `research.md`: research summary aligned to the evidence ledger.
- `evidence.md`: source-proven/user-assumption/gap/decision ledger.
- `cost-model.md`: provider/AWS/dashboard cost model and cap gate.
- `fidelity-matrix.md`: venue/source data-fidelity classification.
- `data-model.md`: key entities and lineage.
- `contracts/nt-research-analytics.md`: source-of-truth and interface contract.
- `issue-audit.md`: refreshed existing GitHub issue map and duplicate check.
- `tasks.md`: dependency-ordered work.
- `checklists/requirements.md`: Spec Kit quality checklist.
- `analysis.md`: cross-artifact consistency analysis.
- Issue payloads after user approval or explicit issue-creation instruction.
- Draft issue payloads in `github-issues.md`; no GitHub mutation before approval.
  Dashboard work is split into source-contract/dependency mapping and later UI
  MVP so PnL scope cannot bypass #409, #77, or #36.
