# Fidelity Matrix: NT-First Research Planning Package

> Archive note: this file is historical/audit context. Live authority is
> `../reference/evidence.md`, `../reference/data-model.md`, and
> `../reference/contracts.md`.

**Status**: Draft planning artifact.
**Refreshed**: 2026-05-21.

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
| NT backtest core | NT `BacktestNode`, `BacktestEngine`, `ParquetDataCatalog` | `L2_REPLAY` capable when input data supports it | `../reference/evidence.md` E-001, E-002 | NT-native replay/backtest using catalog data classes. | Bolt-owned simulator as first path. | Compile the NT version resolved by the target `bolt-v2` branch and prove Rust/Python API mapping. |
| Hyperliquid HIP-4 live | NT upstream Hyperliquid adapter | Live support source-proven; historical class separate | `../reference/evidence.md` E-003..E-007 | Live/adapter planning can use NT first. | Historical execution-quality claim from live docs alone. | Prove target `bolt-v2` branch support and historical outcome data. |
| Hyperliquid HIP-4 history | Official archive/API, Tardis, or forward capture | `FORWARD_CAPTURE_PENDING` until outcome L2/fill history is proven | `../reference/evidence.md` E-007 | Signal/research only if lower-fidelity data found. | L2 outcome replay without source proof. | Check outcome-market coverage in official archive/Tardis; if absent, record forward-capture start/skip trigger. |
| Kalshi adapter | User-assumed NT adapter | `USER_ASSUMPTION`; fidelity not final | `../reference/evidence.md` E-008 | Plan from supported adapter premise. | Claim checked-clone source proof. | Identify target `bolt-v2` NT-version support, selected data source, and adapter surfaces. |
| Kalshi official historical | Kalshi historical API | `TRADE_BAR_REPLAY` or `SIGNAL_ONLY`; not `L2_REPLAY` yet | `../reference/evidence.md` E-009; Kalshi docs list historical markets, candlesticks, trades, fills, orders. | Market/trade/fill/candle research and realized-history analysis. | Historical L2 orderbook replay unless adapter/source proves archived books. | Prove whether historical orderbook snapshots/deltas exist. |
| Polymarket official APIs | Gamma, CLOB, Data API | `TRADE_BAR_REPLAY` or `SIGNAL_ONLY` until cap/depth proof | `../reference/evidence.md` E-013, E-014; Polymarket docs list markets, prices, books, price history, positions, trades. | Discovery, market metadata, trades, price history, positions/activity. | Full historical L2 execution replay without depth/cap proof. | Prove public API pagination/depth limits and NT loader behavior. |
| Polymarket Telonex | Telonex Parquet files | `L2_REPLAY` candidate for snapshots; `TRADE_BAR_REPLAY` for trades/quotes | Telonex docs list trades, quotes, multi-level/full book snapshots, on-chain fills, Parquet. | Tick/snapshot research after schema and license proof. | Queue-position/delta replay unless cadence and transform support it. | Sample Parquet to NT catalog projection; license gate. |
| Polymarket MarketLens | MarketLens historical orderbook API | `L2_REPLAY` candidate | MarketLens docs list point-in-time L2 reconstruction plus history snapshots/deltas/trades. | Polymarket L2 replay candidate after sample/schema/license proof. | Canonical provider claim before sample and license proof. | Sample history endpoint; map snapshots/deltas to NT catalog projection. |
| Polymarket/ Kalshi PMXT | PMXT hourly Parquet archive | `L2_REPLAY` candidate for archived snapshots | PMXT archive lists hourly Polymarket and Kalshi orderbook Parquet files and freshness dashboard. | Research archive after coverage/freshness/schema proof. | Production-grade source claim without license/support/reliability proof. | Validate schema, coverage, gaps, file size/storage, and license/support. |
| Polymarket PolyBackTest | PolyBackTest API | `L2_REPLAY` candidate for supported crypto up/down markets; retention-limited | PolyBackTest docs list historical snapshots, order book depth, 31-day retention, BTC/ETH/SOL coverage, and rate limits. | Fast research for supported markets inside retention. | Unlimited historical coverage or non-supported market claims. | Verify plan, retention, market coverage, API schema, and export path. |
| PolymarketData | PolymarketData API/export | `L2_REPLAY` candidate by paid tier | Public page lists historical orderbook availability by tier. | Candidate commercial feed after proof. | Source-of-truth claim before schema/license/SLA proof. | Verify API docs, retention, export, license, and sample. |
| Polymarket Goldsky | Goldsky subgraph/Mirror/Turbo | `SIGNAL_ONLY` or provenance supplement | Goldsky/Polymarket docs show on-chain Order Filled, Orders Matched, balances, positions, and data resources. | On-chain provenance, fills, indexes, dashboard evidence. | Replacement for market microstructure L2 unless paired with orderbook source. | Estimate events/storage and prove schema against Polymarket contracts. |
| Perpetual futures Tardis | NT live adapter + Tardis replay | `L2_REPLAY` candidate | Tardis and NT Tardis docs list normalized historical replay and NT Parquet/catalog path. | Execution-quality replay only after selected venue/product proof and NT catalog sample. | Venue-specific code branches or hardcoded venue identity. | TOML/registry binding proof plus replay-to-catalog sample. |
| OKX official historical | OKX historical data download | `L2_REPLAY` candidate | OKX official page lists high-resolution L2 order book data from March 2023, trades, candles, and funding rates. | OKX replay candidate after schema/sample proof. | Generic perps fidelity claim for non-OKX venues. | Download/sample schema; map to NT data classes. |
| Hyperliquid official archive | Hyperliquid S3 archive | `L2_REPLAY` candidate for covered assets; HIP-4 coverage unproven | Hyperliquid docs list S3 paths for `market_data/[date]/[hour]/[datatype]/[coin].lz4` including `l2Book`. | Covered-asset replay/fallback after quality proof. | HIP-4 outcome replay before outcome coverage proof. | Check HIP-4 outcome symbols/data types and sample archive file. |
| Binance Data Vision | Binance public data | `TRADE_BAR_REPLAY` candidate | Binance public data repo lists aggTrades, klines, trades; checked source does not prove full historical L2. | Trade/bar research for supported symbols. | L2 replay without separate depth archive proof. | Search official depth archive proof or keep lower fidelity. |
| Kimchi premium / Korean spot prices | TOML-selected Korean spot venue price sources such as Upbit/Bithumb, reference spot/perps price source, and FX/quote source | `SIGNAL_ONLY` as a premium feature unless component sources prove stronger replay fidelity | `../reference/evidence.md` E-033 | Cross-market premium/spread features, research signals, and backtest strategy inputs with claim limits. | Execution-quality claim from premium value alone, hardcoded Korean venue branches, or future-leaking FX/reference joins. | Prove source availability, historical depth, schema, sample, license, event/availability time, token mapping, FX/reference source, and point-in-time join rules. |
| Bybit official docs/data | Bybit V5/current data and historical data page | `GAP` for historical L2; current snapshot proven | Bybit V5 orderbook docs describe current snapshot; historical L2 replay not proven in docs checked. | Current/live source proof or lower-fidelity research if files prove trades/bars. | Historical L2 replay from current orderbook endpoint. | Prove historical data download schema and retention. |
| Kaiko/CoinAPI/Amberdata | Vendor historical orderbook/trades APIs | `L2_REPLAY` candidate by product | Public docs show historical orderbook snapshots or orderbook history endpoints. | Paid vendor candidate after license/sample proof. | NT-ready catalog claim without transform proof. | Sample vendor payload; map to NT data classes; model license/cost. |
| Dashboard PnL/current trades | NT reports/events/snapshots plus #409/#77/#36/#369 decisions | Source-truth capable; incomplete until dependencies accepted | `../reference/evidence.md` E-017, E-018, E-020, E-023 | Read-only current trades, positions, PnL, freshness, missing-source labels. | Independent PnL/account truth, mutation controls, or production-readiness closure claim. | Define source contract; resolve #409/#77/#36 and link #369 as non-closure context. |
| Dashboard/BI product | Grafana, Metabase, Preset/Superset, Retool, Plotly/Dash, or bespoke UI fallback | `DECISION_NEEDED` | `../reference/evidence.md` E-028 | View/query layer over NT-derived read model after product-fit proof. | Product-created PnL truth, mutation controls, or bespoke UI by default. | Select product path from source contract, query backend, security, UX, cost, and ops burden. |

## Future Implementation Gates

1. Target `bolt-v2` NT-version compile/API proof is missing.
2. Kalshi adapter readiness is user-assumed; data/fidelity/source proof remains.
3. HIP-4 historical execution-quality data is unproven.
4. Official archive/API capture remains per-venue `GAP` until source-proven.
5. No provider is selected until fidelity, license, schema/sample, and cost
   impact evidence pass.
6. Dashboard PnL completeness is blocked on #409, #77, #36 scope decision, and #369 non-closure context.
7. Dashboard/BI product path is not selected until product-fit proof passes.
8. Venue/product/provider identity must stay TOML/registry-selected.
