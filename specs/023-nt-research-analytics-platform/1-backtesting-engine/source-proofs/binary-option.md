# SourceProofReport — `binary option` fixture (BTE-016)

Populated against the [schema](./schema.md). Surveyed 2026-05-30 (web), then
**substantially corrected from operator knowledge** the same day.

> **Commercial license — CLEARED.** The operator has direct BD agreements with
> Polymarket and Kalshi permitting use of their data for trading.

> **Survey correction (operator-driven).** An early web-only draft concluded "no
> historical L2 exists for either prediction venue." **That was wrong.** The web
> survey checked native APIs + on-chain datasets and missed the specialist vendors
> and free archives the operator already uses. **Both venues have L2 history** —
> free hourly snapshots and finer paid/operator sources. Operator experience is
> authoritative where it conflicts with the survey.

## L2 availability — corrected

| Venue | Free L2 | Finer L2 | Net |
|-------|---------|----------|-----|
| **Polymarket** | **pmxt** hourly snapshots (free) | **Telonex** tick (operator-validated, paid) | `L2_REPLAY` (tick) available |
| **Kalshi** | **pmxt** hourly snapshots (free) | **kalshi.com/market-data** (operator) + **kalshibacktest** 100ms (crypto markets) | `L2_REPLAY` available |
| Limitless, Opinion | **pmxt** hourly snapshots (free) | — | newer venues; free L2 if ever in scope |

## Candidates

### L2 order book

| Source | Coverage | Cadence / fidelity | NT map | Cost |
|--------|----------|--------------------|--------|------|
| **pmxt archive** (`archive.pmxt.dev`) | **Polymarket, Kalshi, Limitless, Opinion** — hourly orderbook snapshots, parquet (`kalshi_orderbook_YYYY-MM-DDTHH.parquet`, ~39–157 MB/hr) + free JSON API. | **hourly** → `DEPTH_SNAPSHOT_REPLAY` (coarse; good for features/research, not execution-grade) | `OrderBookDepth10` | **Free** |
| **Telonex** *(operator-validated)* | **Polymarket** (Binance on Plus; Kalshi not covered). "Full order book updates on every change — not interval-sampled," 3+ yrs, parquet via SDK/REST. | **tick** → `L2_REPLAY` | `OrderBookDelta`+`QuoteTick`+`TradeTick` | Free trial; Plus $79/mo personal; **Enterprise = commercial** |
| **kalshi.com/market-data** *(operator-pointed)* | **Kalshi** own data offering; operator confirms historical L2 fetchable here (broader than the public API). | `L2_REPLAY` (pin cadence on ingest) | `OrderBookDelta` | operator-validated; confirm cost/format |
| **kalshibacktest.com** | **Kalshi crypto 15-min** markets only (BTC/ETH/SOL/DOGE/XRP). Polls live book, persists depth, replay by timestamp. | **100ms** → `DEPTH_SNAPSHOT_REPLAY` (near-tick) | `OrderBookDelta`/`OrderBookDepth10` | $19.90/mo (Pro = 31 days history — likely operator's "cutoff") |
| Forward-capture (Polymarket/Kalshi WS) | both venues, live `book`/`orderbook_delta`. | **tick** going forward → `L2_REPLAY` | `OrderBookDelta` | Free (self-run) |

### Trades / prices — free, bulk

| Source | Coverage | Fidelity | NT map | Cost |
|--------|----------|----------|--------|------|
| **SII-WANGZJ/Polymarket_data** | 1.1B trade records, 107GB parquet, Polymarket inception→2026 (Polygon `OrderFilled`). | `TRADE_BAR_REPLAY` | `TradeTick` | Free, MIT |
| **jon-becker/prediction-market-analysis** | Polymarket **+ Kalshi** market+trade data, parquet on R2; academic. | `TRADE_BAR_REPLAY` | `TradeTick` | Free |
| **pmxt** OHLCV (`pmxt.dev`) | 3+ yrs OHLCV across major prediction markets, API. | `TRADE_BAR_REPLAY` | `Bar` | check docs |
| **Goldsky** Polymarket | on-chain Orders Matched/Filled, OI, positions, balances (operator-used). Good for **flow/OI features**. | metrics + trades | `TradeTick` + custom | Mirror pipelines (freemium) |
| **Lychee** (`lycheedata.com`) | ~36GB every Kalshi trade + market since launch (full L2 not confirmed). | `TRADE_BAR_REPLAY` | `TradeTick` | check site |
| Native APIs (Polymarket Data API; Kalshi `/historical/*`) | trades, prices, candlesticks (paginate-capped — bulk sets above avoid this). | `TRADE_BAR_REPLAY` | `TradeTick`/`Bar` | Free |
| `manja316` ($9) | Polymarket 15-min top-10 depth — **superseded** by pmxt (free) / Telonex (tick). | — | — | ignore |

## Recommendation

- **Free, now — research substrate:** **pmxt archive** (free hourly L2 snapshots,
  both venues + Limitless/Opinion) for book/feature research, plus free bulk
  **trades** (SII-WANGZJ, jon-becker). Covers the Phase-0 nimble Python lane at
  **$0**. Forbidden claims at hourly L2: no execution-grade fills — feature/signal
  research only.
- **Execution-grade L2 when a strategy earns it:**
  - **Polymarket → Telonex** (tick, Enterprise tier for the commercial license) —
    operator-validated; wire Telonex parquet → `OrderBookDelta` → catalog.
  - **Kalshi → `kalshi.com/market-data`** (operator-validated) for breadth, and
    **kalshibacktest** (100ms) for crypto 15-min markets.
  - **Gaps / latest tick:** free **forward-capture** of the live WS on both venues.

No forward-capture is *mandatory* anymore — it's the free tick-fidelity option and
gap-filler, not the only path.

## Required-check status

| Check | pmxt free L2 | Telonex (PM) | kalshi.com/market-data | kalshibacktest |
|-------|-------------|--------------|------------------------|----------------|
| schema | ✅ parquet | ✅ operator-used | ✅ operator-pointed | ✅ doc'd |
| sample pointer | ✅ archive.pmxt.dev | ✅ free trial | ✅ | ✅ free sample |
| fidelity (claimed = evidenced) | ✅ `DEPTH_SNAPSHOT` hourly | ✅ `L2_REPLAY` tick (operator-attested) | ⚠️ `L2_REPLAY` pending (confirm format) | ✅ `DEPTH_SNAPSHOT` 100ms |
| NT mapping | ✅ OrderBookDepth10 | ⚠️ wire-up pending | ⚠️ wire-up pending | ⚠️ wire-up pending |
| license (commercial) | ✅ free | ✅ Enterprise (operator-attested) | ✅ BD-cleared (operator-attested; no public clause on file) | ⚠️ confirm |
| forbidden-claims recorded | ✅ (hourly ≠ execution) | ✅ | ✅ | ✅ (crypto-only) |

> Resolves **BTE-023**: Kalshi L2 history **does** exist (`kalshi.com/market-data`,
> kalshibacktest 100ms, pmxt free hourly) — the public API simply doesn't expose
> archived books. No downgrade needed; pick the cadence the strategy requires.
