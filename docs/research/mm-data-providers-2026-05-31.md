# Market-Data Provider Survey — binary L2 + underlying, extensible to perps/CEX

**Date**: 2026-05-31 · **For**: bolt-v3 binary oracle maker (#488) · **Branch**: `docs/488-mm-multi-venue-survey`

**Method / provenance.** Two parallel agent sweeps on 2026-05-31, each provider's claims read live from its own docs/pricing via `browser-fetch-router` + web search: (1) a 5-cluster capability+pricing survey of ~25 providers, (2) a URL/identity disambiguation pass. **Findings marked ✅VERIFIED were reproduced by me directly** (downloaded/decoded real data); all others are vendor-doc claims at the stated confidence and may carry the noted caveats. Pricing captured as published; "unknown" = not public (not guessed).

---

## TL;DR — the decision

The use case splits into **two data needs**; **no single provider serves both**.

| Need | Use (free) | Paid upgrade if needed |
|---|---|---|
| **Binary L2** (Polymarket up/down book + trades/aggressor) | **pmxt free archive** ✅VERIFIED tick-level full-depth | Telonex (license-safe) or MarketLens (cheapest tick) |
| **Underlying** (spot for fair value / lead-lag) | **Binance public data** (free, aggTrades w/ aggressor) | Tardis (CEX/perp fan-out; only native-NT loader) or Crypto Lake (value) |
| **Resolution anchor** | **Chainlink Data Streams** ✅VERIFIED (testnet, live-only) | prod credentials later |

Net: **$0 covers the maker backtest + underlying + live anchor.** Paid options matter only for (a) a stated commercial license, (b) the CEX/perp fan-out, (c) deeper history.

---

## Scoring axes

Fidelity (true tick-delta vs snapshot vs aggregate) · trades-with-aggressor · history depth · prediction-market coverage of the *up/down churn* markets specifically · NT-mappability · commercial-licensing · price. The recurring trap: **"order book" means wildly different things** — true tick deltas vs 30s/1-min/hourly snapshots vs %-band aggregates. Only true tick + per-trade aggressor lets you measure spread-capture vs adverse-selection.

---

## Binary / prediction-market providers

| Provider | Fidelity | Trades+aggressor | History | NT-fit | Price | Verdict |
|---|---|---|---|---|---|---|
| **pmxt** ✅VERIFIED | **tick full-depth** (21-lvl ladders + BBO deltas) | **yes** (BUY/SELL) | ~Apr 20–May 25 2026 | thin custom loader (via BTE `bte_ingest`) | **free** (R2 Parquet) | **WINNER for binary L2.** Covers BTC/ETH/SPX up/down. License unstated. SDK hosted endpoints don't serve it free — direct Parquet only. |
| **MarketLens** (marketlens.trade) | tick snapshot+delta, ≤100 lvl | yes + paired underlying | "full" (floor unconfirmed) | cleanest mapping | free tier has bulk Parquet; Pro $39/mo | Strongest paid tech fit. **License for trading unstated (risk).** |
| **Telonex** (telonex.io) | tick full-depth (`book_snapshot_full`) | yes + Binance spot + Chainlink ticks | off-chain ~Oct 2025+ (reliable Jan 2026) | Parquet, custom loader | Plus $79/mo (personal only); Enterprise custom (**commercial license**) | **License-safe paid pick** — only one with explicit commercial license. |
| **PolyBackTest** (polybacktest.com) | 8/sec snapshots (not deltas) | partial (1s OHLCV aggregates) | **31–60 days only** | REST-JSON, no bulk | Pro configurator; AI $35/mo | Only one with Binance **spot+futures**, but snapshot-fidelity + short history. |
| **PolymarketData** (polymarketdata.co) | **1-min snapshots** (fatal) | **no** | from Aug 2025 | coarse | $60–$360/mo | Deep ladder history but 1-min + no trades rules it out for tick MM. |
| **PolyData** (polydata.live) ✅exists | tick ClickHouse L2 | (tier-gated) | tier-gated | SQL | $1/day, $59.90/mo | I hold a key; tier-locked. **Distinct from polymarketdata.co.** |
| **EntityML** (entityml.com) | snapshots+deltas (Poly+Kalshi) | no | non-continuous subset, data-loss gaps | custom | unpublished | Real L2, but sparse coverage of up/down churn (~1/12), no trades, no bulk. |
| **KalshiBackTest** (kalshibacktest.com) | 100ms snapshots + trade prints | aggressor unconfirmed | 31 days | REST-JSON | Pro $19.90/mo | Only **Kalshi** tick-ish source; narrow. |
| **Goldsky** | on-chain fills/positions — **no book** | taker/maker (settlement) | v2 migration broke subgraphs | poor (no depth) | free–PAYG | On-chain reconciliation only, not L2. |
| **Lychee** (lycheedata.com) | Kalshi trades+metadata — **no book** | unconfirmed | since 2021 (deepest) | weak | unpublished | Deep Kalshi trades, zero orderbook depth. |
| HuggingFace (SII-WANGZJ / jon-becker) | metadata / gated | no / unknown | shallow / unknown | poor | free | Market-universe catalog at best; one dataset 401-gated. |

---

## Underlying spot / CEX-perp fan-out (paired feed + future venues)

| Provider | Fidelity | Trades+aggressor | History | NT-fit | Price | Verdict |
|---|---|---|---|---|---|---|
| **Tardis.dev** | tick incremental L2 | yes | 7+ yrs, 50+ venues incl Hyperliquid/dYdX | **only native Rust NT adapter** | $300 min, configurator | **WINNER for fan-out** — near-zero integration. No prediction markets. |
| **Crypto Lake** | tick `book_delta_v2` (unlimited depth) | yes | several yrs, 10 venues | clean schema, custom loader | free sample; ~$64/mo (300GB) | **BEST VALUE** for underlying lead-lag. |
| **Binance public data** | trades + klines + **L1 top-of-book** (no full-depth spot L2) | **yes** (`isBuyerMaker`) | deep (2017+) | easy (TradeTick/Bar) | **free** | **Best-value underlying proxy** (aggTrades). Not the resolution source. |
| **CryptoHFTData** | full-depth L2 ticks | yes | only since Jul 2025 (~10mo) | custom Parquet loader | **free** (promo) | Best free **multi-venue** fan-out (Binance/OKX/Bybit/Kraken/Bitget/HL). |
| **OKX portal** | true incremental L2 (Mar 2023+) | yes | 2021/2023+ | via tardis-machine | free | Deepest free genuine tick L2, single venue. |
| **CoinAPI** | `limitbook_full` L2/L3 (S3 flat files) | yes | multi-yr, 300+ venues | custom | tiered, unknown | Solid bulk underlying; no NT loader. |
| **Amberdata** | tick L2 + 1-min snapshots (Korean venues) | yes | 2017+ via bulk; REST 18mo | custom | enterprise | Pick only for Bithumb/Upbit. |
| **Kaiko** | tick product exists; **default L2 = 30s snapshots** | yes | 2015+ | custom | enterprise (priciest) | Overkill/opaque vs Tardis. |
| **Gate.io archive** | hourly book files + trades | yes | months+ | custom | free | Single-venue free fallback. |
| **cryptofeed / tardis-machine** | true tick L2 (self-capture) | yes | **zero backfill** | native (tardis-machine) | free (self-host) | Forward-capture only; no history. |
| **Binance Vision** | trades + klines; **no full-depth L2** | yes | deep | mixed | free | Cheap trade tape; futures bookDepth is %-band aggregate, not a book. |
| **Databento** | — | — | — | has NT adapter | usage-based | **DISQUALIFIED**: only CME/Cboe BTC derivatives, no native crypto (unshipped roadmap). |

---

## Resolution / oracle feeds (what the binary settles on)

| Provider | Role | Fidelity | History | Verdict |
|---|---|---|---|---|
| **Chainlink Data Streams** ✅VERIFIED | **resolution-authoritative** (Polymarket crypto up/down settle on it) | sub-second mid + LWBA bid/ask | testnet **live-only, no backfill** | We have testnet creds, working. Live anchor + prod-equality check. See [chainlink guide](../Boltv2/chainlink-data-streams-guide.md). |
| **Binance** | proxy underlying | aggTrades sub-second w/ aggressor | deep | Free historical proxy; align to Data Streams. |
| Chainlink **Data Feeds** (on-chain) | — | ~hourly/deviation cadence | full on-chain | **Wrong feed** — binaries resolve on *Streams*, not Feeds. Reject for tick. |
| Pyth | backs Polymarket **equity/forex** binaries (not crypto) | ~1s, 60s interval cap, no trades | unknown | Reserve for equity-binary expansion only. |

---

## Empirically verified by me (not just vendor claims)

- **pmxt free archive — tick-level full-depth, confirmed.** Downloaded a real hour file (`r2v2.pmxt.dev/polymarket_orderbook_2026-05-11T12.parquet`, 386 MB, **71.9M events/hour**). Schema: `event_type` ∈ {`price_change` 99.9% = L2 deltas, `book` = full 21-level ladders, `last_trade_price` = trades, `tick_size_change`}; `side` BUY/SELL aggressor; `asset_id` = CLOB token; ms `timestamp` + `timestamp_received`. Window probed: **404 before ~Apr 20, present Apr 20–May 23, stops ~May 25**. Coverage: BTC/ETH/SPX up/down present. URL scheme: `polymarket_orderbook_<YYYY-MM-DD>T<HH>.parquet`, public, no key. The **SDK hosted** `fetch_trades`/historical `fetch_order_book` do NOT serve this on the free key — direct Parquet download only. Full detail in memory `project_pmxt_free_archive_verified`.
- **Chainlink Data Streams testnet — working.** Creds in SSM `/bolt/testnet/chainlink` (key 36 / secret 128). HMAC auth + V3 ABI decode correct; BTC ≈ $73.9k, ETH ≈ $2.0k decoded sane; `/api/v1/feeds` → 620 feeds. **Live-only — no historical backfill on testnet REST.**
- **PolyData ≠ PolymarketData.** `polydata.live` (ClickHouse tick L2, tier-locked, I hold a key) is a *different company* from `polymarketdata.co` (1-min snapshots). An earlier search guess conflated them; corrected.

---

## Canonical URLs + look-alikes to avoid

| Provider | Go here | ⚠ Avoid |
|---|---|---|
| MarketLens | **marketlens.trade** | `.pro/.app/.net/.in/.com.au/.xyz`, `marketlensai.com`, amphibiantrading — 8 unrelated products |
| Telonex | **telonex.io** | Telnyx (telecom), Talenox (HR) |
| pmxt | **pmxt.dev** + **archive.pmxt.dev** | `pmxt.com` (expired), `pmxt.io` (dead) |
| PolyData | **polydata.live** | `polymarketdata.co` (different), `polydata.org/.pro/.cc` (dashboards) |
| PolymarketData | **polymarketdata.co** | the `polydata.*` set |
| Tardis | **tardis.dev** | tardis-group.com, tardis.solutions |
| Crypto Lake | **crypto-lake.com** (hyphen) | `cryptolake.com` (for sale), cryptolakestock.com |
| Binance data | **github.com/binance/binance-public-data** → **data.binance.vision** | any `binance-public-data.com/.io` = phishing |
| Chainlink | **docs.chain.link/data-streams** | `docs.chain.link/data-feeds` = different/wrong product |
| EntityML | **entityml.com** | — |

---

## Cross-cutting findings & deferred

- **No one-stop provider.** Prediction-market vendors (pmxt/MarketLens/Telonex) carry no real CEX/perp L2; CEX vendors (Tardis/Kaiko/etc.) carry no prediction markets. Expect a 2-source stack.
- **Free is sufficient for the maker backtest** (pmxt L2 + Binance underlying); paid only buys license safety, fan-out breadth, or deeper history.
- **Licensing is the biggest unconfirmed risk on the cheap binary picks** — only Telonex states a commercial-trading license. Confirm with MarketLens/pmxt before any *live* commercial use (the backtest/research use is fine).
- **Deferred / unverified**: exact history floors (MarketLens), aggressor side on KalshiBackTest prints, pmxt Kalshi/Limitless granularity, pmxt commercial license. None block the maker backtest.
