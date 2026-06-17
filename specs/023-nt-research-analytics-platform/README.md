# NT Research Planning Triage

This package is split into three future implementation projects. Start from the
project directory that matches the work you intend to do.

Default implementation sequence is Backtesting Engine first. Research Analytics
and Dashboard are downstream consumer projects unless a future session
explicitly selects one of them.

| If you are working on... | Open this first | Scope |
|---|---|---|
| Backtests, replay, run manifests, NT catalog input, fill/fee/latency behavior, or result claim limits | [`1-backtesting-engine/spec.md`](1-backtesting-engine/spec.md) | NT-native backtesting orchestration only. |
| Experiments, notebooks, feature joins, alpha research, point-in-time correctness, or promotion to runtime config | [`2-research-analytics/spec.md`](2-research-analytics/spec.md) | Research-only analytics and promotion gates. |
| Current trades, PnL, exposure, freshness, strategy state, BI/product choice, or read-only UI | [`3-dashboard/spec.md`](3-dashboard/spec.md) | Read-only NT-derived dashboard/source contract. |

## How To Triage

1. Pick exactly one project directory.
   If no vertical is explicitly selected, pick `1-backtesting-engine/`.
2. Read that project's `spec.md`, then `plan.md`, then `tasks.md`.
3. Use the issue dependencies in that project doc to decide whether to update an
   existing issue or create a new one.
4. Do not implement from the root package.
5. Do not combine Backtesting Engine, Research Analytics, and Dashboard in one
   implementation branch unless explicitly approved.

## Venue Name Rule

Venue and provider names in this package are evidence examples or candidate
bindings only. They are not architecture names, required first fixtures, or
permission to branch code around Polymarket, Kalshi, Hyperliquid, Tardis,
Bybit, OKX, Binance, or any other concrete venue/provider.

Backtesting proof fixtures must be named by market structure first:
`binary option` and `perps/spot`. The selected venue/provider for each fixture
must be TOML/registry data.

## Root Package Role

Root `spec.md`, `plan.md`, and `tasks.md` are Speckit compatibility pointers.
They are not implementation specs.

The numbered project docs contain the implementation-facing scope, gates, tasks,
and local evidence summaries. The `reference/` directory is the live
cross-project authority layer, not a fourth project:

- `reference/evidence.md`, `reference/data-model.md`, and `reference/contracts.md` are
  authoritative cross-project inputs inherited by the numbered project docs.
  Per-project evidence tables are derived views of `reference/evidence.md`.

The `archive/` directory is historical/audit context only. It holds cost and
fidelity snapshots, staged issue payload drafts, issue-search evidence, review
notes, open-question history, checklist history, and source-research notes.

Consult `reference/` before changing any cross-project evidence, contract, or
data-model rule. Consult `archive/` only for provenance, issue-draft recovery,
external review history, or decision reconstruction.

## Project Boundaries

- `1-backtesting-engine`: owns NT `BacktestNode` orchestration, catalog replay,
  run manifests, extension-surface classification, result contracts, and
  fidelity claim limits.
- `2-research-analytics`: owns research datasets, notebooks, point-in-time
  feature joins, experiment metadata, leakage checks, verdicts, and promotion
  config refs on experiment results.
- `3-dashboard`: owns field-source matrix, freshness behavior, read-only product
  gate, no-mutation controls, and dashboard/PnL dependencies.
