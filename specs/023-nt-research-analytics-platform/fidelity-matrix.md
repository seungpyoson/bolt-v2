# Fidelity Matrix: NT-First Research Analytics Platform

**Status**: Draft planning artifact.
**Refreshed**: 2026-05-20.

Fidelity labels:

- `L2_REPLAY`: historical order book replay supports execution-quality backtest
  claims after NT catalog projection proof.
- `TRADE_BAR_REPLAY`: trades, fills, candles, or bars support price/alpha
  research but not full queue/execution simulation.
- `SIGNAL_ONLY`: data can inform signals, features, or dashboards, but not
  execution-quality backtests.
- `FORWARD_CAPTURE_PENDING`: no sufficient history; start capture now and
  backtest only after enough captured data exists.

## Matrix

| Venue/source family | Candidate source | Current fidelity class | Evidence | Allowed claims | Forbidden claims | Next proof |
|---|---|---|---|---|---|---|
| NT backtest core | NT `BacktestNode`, `BacktestEngine`, `ParquetDataCatalog` | `L2_REPLAY` capable when input data supports it | `evidence.md` E-001, E-002 | NT-native replay/backtest using catalog data classes. | Bolt-owned simulator as first path. | Compile selected NT pointer and prove Rust/Python API mapping. |
| Hyperliquid HIP-4 live | NT upstream Hyperliquid adapter | Live support source-proven; historical class separate | `evidence.md` E-003..E-007 | Live/adapter planning can use NT first. | Historical execution-quality claim from live docs alone. | Prove selected Bolt pointer and historical outcome data. |
| Hyperliquid HIP-4 history | Official archive/API, Tardis, or forward capture | `FORWARD_CAPTURE_PENDING` until outcome L2/fill history is proven | `evidence.md` E-007 | Signal/research only if lower-fidelity data found. | L2 outcome replay without source proof. | Check outcome-market coverage in official archive/Tardis; if absent, record forward-capture start/skip trigger. |
| Kalshi adapter | User-assumed NT adapter | `USER_ASSUMPTION`; fidelity not final | `evidence.md` E-008 | Plan from supported adapter premise. | Claim checked-clone source proof. | Identify selected pointer/source and adapter surfaces. |
| Kalshi official historical | Kalshi historical API | `TRADE_BAR_REPLAY` or `SIGNAL_ONLY`; not `L2_REPLAY` yet | `evidence.md` E-009; Kalshi docs list historical markets, candlesticks, trades, fills, orders. | Market/trade/fill/candle research and realized-history analysis. | Historical L2 orderbook replay unless adapter/source proves archived books. | Prove whether historical orderbook snapshots/deltas exist. |
| Polymarket official APIs | Gamma, CLOB, Data API | `TRADE_BAR_REPLAY` or `SIGNAL_ONLY` until cap/depth proof | `evidence.md` E-013, E-014; Polymarket docs list markets, prices, books, price history, positions, trades. | Discovery, market metadata, trades, price history, positions/activity. | Full historical L2 execution replay without depth/cap proof. | Prove public API pagination/depth limits and NT loader behavior. |
| Polymarket Telonex | Telonex Parquet files | `L2_REPLAY` candidate for snapshots; `TRADE_BAR_REPLAY` for trades/quotes | Telonex docs list trades, quotes, multi-level/full book snapshots, on-chain fills, Parquet. | Tick/snapshot research after schema and license proof. | Queue-position/delta replay unless cadence and transform support it. | Sample Parquet to NT catalog projection; license gate. |
| Polymarket Goldsky | Goldsky subgraph/Mirror/Turbo | `SIGNAL_ONLY` or provenance supplement | Goldsky docs show subgraphs and Mirror/Turbo pipelines with metered event writes. | On-chain provenance, fills, indexes, dashboard evidence. | Replacement for market microstructure L2 unless paired with orderbook source. | Estimate events/storage and prove schema against Polymarket contracts. |
| Selected perpetual-futures venues | NT live adapter + Tardis replay | `L2_REPLAY` candidate | `evidence.md` E-010, E-021, E-026 | Execution-quality replay only after selected venue/product proof and NT catalog sample. | Venue-specific code branches or hardcoded venue identity. | TOML/registry binding proof plus replay-to-catalog sample. |
| Selected perpetual-futures venues | Official archives/APIs | `GAP` until per-venue official-source proof | `evidence.md` E-022; official source proof required per venue. | Venue-specific fidelity only after official docs prove classes. | Generic official-archive suitability or L2 replay claim before per-venue proof. | Fetch official docs for each selected venue and classify freshness/completeness. |
| Dashboard PnL/current trades | NT reports/events/snapshots plus #409/#77/#36/#369 decisions | Source-truth capable; incomplete until dependencies accepted | `evidence.md` E-017, E-018, E-020, E-023 | Read-only current trades, positions, PnL, freshness, missing-source labels. | Independent PnL/account truth, mutation controls, or production-readiness closure claim. | Define source contract; resolve #409/#77/#36 and link #369 as non-closure context. |
| Dashboard/BI product | Grafana, Metabase, Preset/Superset, Plotly/Dash, or bespoke UI fallback | `DECISION_NEEDED` | `evidence.md` E-028 | View/query layer over NT-derived read model after product-fit proof. | Product-created PnL truth, mutation controls, or bespoke UI by default. | Select product path from source contract, query backend, security, UX, cost, and ops burden. |

## Current Gaps

1. Selected NT pointer compile/API proof is missing.
2. Kalshi adapter source proof is missing but planning assumes support per user
   instruction.
3. HIP-4 historical execution-quality data is unproven.
4. Official archive/API capture remains per-venue `GAP` until source-proven.
5. Tardis replay is not selected until cost and sample catalog proof pass.
6. Dashboard PnL completeness is blocked on #409, #77, #36 scope decision, and #369 non-closure context.
7. Dashboard/BI product path is not selected until product-fit proof passes.
8. Venue/product/provider identity must stay TOML/registry-selected.
