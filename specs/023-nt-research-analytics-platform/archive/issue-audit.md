# Existing Issue Audit: NT-First Research Planning Package

> Archive note: this file is historical/audit context. Live authority is
> `../reference/evidence.md`, `../reference/data-model.md`, and
> `../reference/contracts.md`.

Date: 2026-05-21
Remote: `https://github.com/seungpyoson/bolt-v2.git`

Purpose: prove existing GitHub issue state before proposing new
`/speckit-taskstoissues` payloads. This file is read-only evidence; it does not
create, close, or mutate issues.

## Live Checks Run

- `gh issue list --repo seungpyoson/bolt-v2 --state all --limit 500 --json number,title,state`
  refreshed the issue-state map on 2026-05-21.
- `gh issue view 19 --repo seungpyoson/bolt-v2`
- `gh issue view 20 --repo seungpyoson/bolt-v2`
- `gh issue view 21 --repo seungpyoson/bolt-v2`
- `gh issue view 22 --repo seungpyoson/bolt-v2`
- `gh issue view 23 --repo seungpyoson/bolt-v2`
- `gh issue view 24 --repo seungpyoson/bolt-v2`
- `gh issue view 34 --repo seungpyoson/bolt-v2`
- `gh issue view 36 --repo seungpyoson/bolt-v2`
- `gh issue view 39 --repo seungpyoson/bolt-v2`
- `gh issue view 75 --repo seungpyoson/bolt-v2`
- `gh issue view 77 --repo seungpyoson/bolt-v2`
- `gh issue view 88 --repo seungpyoson/bolt-v2`
- `gh issue view 112 --repo seungpyoson/bolt-v2`
- `gh issue view 115 --repo seungpyoson/bolt-v2`
- `gh issue view 148 --repo seungpyoson/bolt-v2`
- `gh issue view 158 --repo seungpyoson/bolt-v2`
- `gh issue view 176 --repo seungpyoson/bolt-v2`
- `gh issue view 236 --repo seungpyoson/bolt-v2`
- `gh issue view 369 --repo seungpyoson/bolt-v2`
- `gh issue view 409 --repo seungpyoson/bolt-v2`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "Polymarket"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "Hyperliquid"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "Kalshi"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "dashboard"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "backtest"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "backtesting"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "research analytics"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "PnL"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "Tardis"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "Telonex"`
- `gh issue list --repo seungpyoson/bolt-v2 --state all --search "Goldsky"`

## Existing Open Issues To Reuse Or Link

| Issue | State | Relation |
|---|---:|---|
| #19 Data lake lineage metadata | OPEN | Raw evidence/catalog lineage. Future vertical plans should reuse or link. |
| #20 Canonical normalized lake layout | OPEN | Athena/DuckDB layout. Provider gate must not redefine layout. |
| #21 Normalized resolutions with provenance | OPEN | Resolution history source for analytics and redemption/PnL context. |
| #22 Versioned normalized markets dimension | OPEN | Market dimension lineage. Provider gate must not duplicate. |
| #23 NT instrument stream spool bridge | OPEN | Instrument/catalog completeness dependency. |
| #24 NT-first data lake follow-on epic | OPEN | Parent data-lake scope. |
| #34 Flexible Polymarket strategy platform | OPEN | Strategy consumer; future Backtesting Engine and Research Analytics plans must not silently broaden it. |
| #36 Auto redemption with realized PnL updates | OPEN | Dashboard must include or explicitly exclude redemption-realized-PnL. |
| #39 Adaptive venue weighting | OPEN | Future analytics consumer, not baseline platform prerequisite. |
| #75 Offline verified allowlist/research participation | OPEN | Related research workflow; do not fold into a generic analytics plan without explicit scope. |
| #77 Trade-history/PnL durable path | OPEN | Historical PnL dashboard dependency. |
| #88 Deferred Phase 1 strategy-platform reconciliation | OPEN | PnL/strategy reconciliation context; dashboard must not claim to close it. |
| #112 Kalshi venue integration | OPEN | Kalshi proof payload should update or depend on it. |
| #115 Hyperliquid HIP-4 outcome contracts | OPEN | HIP-4 payload should update/link it, not replace it without approval. |
| #148 Inline capture sidecar extraction | OPEN | Capture expansion risk constraint. |
| #158 Sidecar collectors for market data NT adapters drop across all exchanges | OPEN | Directly relevant to all-exchange market-data capture; provider gate must decide whether to reuse, split, or supersede it. |
| #176 Agent-readiness triage and autonomy roadmap | OPEN | Agent/tooling readiness context, not research-platform implementation scope. |
| #236 Thin NT rebuild epic | OPEN | Architecture parent: thinnest layer over NT, no dual paths. |
| #369 Production-grade live trading readiness beyond tiny-canary | OPEN | Production observability context; research/dashboard work may feed it but must not imply closure. |
| #407 Controlled Polymarket broad-discovery mode | OPEN | Polymarket discovery breadth constraint for data collection. |
| #409 PortfolioSnapshot stream capture | OPEN | Account-wide MTM/PnL dashboard completeness dependency. |

## Relevant Open Polymarket Readiness Issues

| Issue | State | Relation |
|---|---:|---|
| #127 Native Polymarket `order_book_depths` support | OPEN | Polymarket depth/fidelity proof may depend on this or must label gap. |
| #254 Track Polymarket V2 adoption blockers before live enablement | OPEN | Polymarket live/source readiness constraint. |
| #385 Real no-order live connectivity test | OPEN | Live connectivity proof, not historical backtest proof. |

## Searches With No Direct Existing Issue Found

| Search | Result | Planning impact |
|---|---:|---|
| `Tardis` | none | Provider cost/fidelity gate appears new; still link #158/#24 if capture/data-lake scope overlaps. |
| `Telonex` | none | Provider cost/fidelity gate appears new; still link Polymarket data/fidelity issues if selected. |
| `Goldsky` | none | Provider cost/fidelity gate appears new; still link resolution/provenance and on-chain data issues if selected. |
| `backtest` | no direct backtesting-platform issue | Future Backtesting Engine spec/plan appears new; implementation runner is deferred until that vertical is approved. |
| `dashboard` | no direct dashboard implementation issue | Future Dashboard spec/plan and source contract appear new; implementation is deferred and must depend on #409/#77/#36 decisions plus #369 non-closure context. |
| `research analytics` | #158, #24, #176 | Confirms market-data collector and data-lake overlap; no direct Research Analytics spec/plan issue found. |

## Required Adjustments Before Issue Creation

- Add #158 to provider/source capture relations.
- Add #407, #127, and #254 to Polymarket data/fidelity relations.
- Keep Tardis, Telonex, Goldsky, cross-project source-contract gates, Backtesting
  Engine spec/plan, Research Analytics spec/plan, and Dashboard spec/plan as
  proposed new slices unless later issue search finds direct duplicates.
- Do not propose runtime runner, analytics read-model, or dashboard
  implementation issues until the user approves the relevant vertical plan.
- Do not mutate #112, #115, #158, #407, #409, or any other issue without
  explicit user approval.
