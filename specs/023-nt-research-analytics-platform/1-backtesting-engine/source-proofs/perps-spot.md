# SourceProofReport — `perps/spot` fixture (BTE-017)

Populated against the [schema](./schema.md). First surveyed 2026-05-30 (web);
**substantially re-surveyed the same day** with a 6-angle fan-out aimed at the
free / community / exchange-native / self-capture class the first pass
under-covered (14 agents, 76 candidate sources, completeness critic + gap-fill).

> **Survey correction.** The first pass concluded "free crypto L2 barely exists —
> start free at trade-bar fidelity, pay Tardis/CoinAPI for any real book." **That
> was too pessimistic.** It leaned on enterprise vendors and missed the
> exchange-native archives and free aggregator tiers that already publish L2.
> **Free, turnkey, true-`OrderBookDelta` history exists** (Gate.io and OKX native
> archives; Crypto Lake `book_delta_v2` free tier; Tardis free first-of-month
> samples), and free self-capture (cryptofeed) covers forward deltas on every
> venue including the Korean pair. Paid vendors are now a Phase-3 escalation, not
> the starting point.

> **Confidence asymmetry.** Fidelity/coverage and the *existence* of the free
> archives are high-confidence (doc fetches; the Gate.io S3 archive was listed
> directly this session). The weak layer is (a) exact **delta-vs-snapshot column
> schema** of the two free native L2 archives — confirm by decompressing one file
> — and (b) **commercial-license / exact-pricing** for nearly every source. See
> [open verifications](#open-verifications-the-weak-layer).

## L2 availability — corrected

| Venue (bolt-v3 MM) | Free true L2 (`OrderBookDelta`) | Free depth snapshot (`OrderBookDepth10`) | Tick L2 ceiling |
|---|---|---|---|
| **Binance** | self-capture only (diff-depth WS; native NT loader) | futures `bookDepth` (top-5/10/20, ~100ms) | paid T_DEPTH / Tardis, or self-capture |
| **Bybit** | self-capture (v5 snapshot+delta WS) | `public.bybit.com` ob500 (coalesced ~10ms) | self-capture or Tardis |
| **OKX** | **native free download** (since 2023-03) ⚠️ confirm schema | CryptoHFTData free tier | native books-l2-tbt (10ms) self-capture |
| **Hyperliquid** | **none** — self-node only (forward) | official S3 (0.5s) + Hydromancer (1-min), free | self-run node `--write-raw-book-diffs` |
| **Upbit** (kimchi) | none (no native deltas) | native WS top-15/30 snapshot (self-capture); Tardis paid | DEPTH_SNAPSHOT only (venue limit) |
| **Bithumb** (kimchi) | none (no native deltas) | native WS snapshot (self-capture); Amberdata 1-min paid | DEPTH_SNAPSHOT only (venue limit) |
| Gate.io (non-primary) | **native free archive** (100ms-merged, since ~2021) ✅ archive verified | — | as above |

**Reading it:** the two **primary** MM venues with the cleanest free book story are
**OKX** (native L2 download) and — if Gate.io is ever added — **Gate.io** (verified
free archive). Binance/Bybit have free *trades + coarse depth*, with true deltas a
short self-capture script away. Hyperliquid and the Korean pair are
snapshot-ceiling for free, tick only via self-capture.

## Candidates (free / official first)

### A. Free exchange-native archives

| Source | Venues | Highest fidelity | NT map | Cost | Notes |
|--------|--------|------------------|--------|------|-------|
| **Gate.io archive** `download.gatedata.org` / `gateio-public-data` S3 | Gate spot, futures_usdt/btc, delivery | `L2_REPLAY` — orderbooks = set (snapshot) + make/take incremental, 100ms-merged | `OrderBookDelta` / `TradeTick` / `Bar` | **$0** | **Archive existence + public listing VERIFIED this session** (hourly `.csv.gz` under monthly prefixes, e.g. `spot/orderbooks/202107/100X_USDT-2021071300.csv.gz`). Delta column schema (`timestamp,side,action,price,amount,id,flag`) from search — decompress one file to confirm. Not a primary MM venue. |
| **OKX historical-data portal** | OKX spot, swap, futures, options | `L2_REPLAY` (Order Book since 2023-03) — incremental-vs-snapshot **unconfirmed** | `OrderBookDelta`?/`TradeTick`/`Bar` | **$0** | Page fetch confirms "L2 from March 2023, trades from Sept 2021, candles from July 2023, funding from March 2022." Pull one L2 file; if it carries `seqId/action` it's `OrderBookDelta`, else `OrderBookDepth10`. Primary MM venue → high value. |
| **Binance Vision** `data.binance.vision` | Binance spot, USD-M, COIN-M | `TRADE_BAR_REPLAY` (+ futures depth snapshots) | `TradeTick`/`Bar`/`QuoteTick`; `bookDepth`→`OrderBookDepth10` | **$0** | Backbone for trades + klines + top-of-book (single wget). **No incremental L2 here** — `bookTicker`=L1 only; futures `bookDepth`=top-5/10/20 snapshots; spot has none. |
| **Bybit public archive** `public.bybit.com` + `quote-saver.bycsi.com` | Bybit spot, linear, inverse | `DEPTH_SNAPSHOT_REPLAY` — ob500 carries `type(snapshot\|delta),u,seq` but coalesced ~10ms | `TradeTick`; ob500→`OrderBookDepth10` | **$0**, no reg | Trades from 2020-05; **ob500 book backfill per-symbol much later (~2023+)** — verify earliest file. No date-window cap on the bulk archive (only the web tool caps at 7 days). |
| **Kraken downloadable history** | Kraken spot | `TRADE_BAR_REPLAY` (OHLCVT + time-and-sales) | `Bar`/`TradeTick` | **$0** | Full history via Google-Drive links (less scriptable). **No native L2 download.** |

### B. Free multi-venue aggregator tiers

| Source | Venues | Highest fidelity | NT map | Cost | Notes |
|--------|--------|------------------|--------|------|-------|
| **CryptoHFTData free tier** (`pip install cryptohftdata`) | Binance, Bybit, OKX, Kraken, **Hyperliquid**, Bitget, BitMEX (+others). **No Korean.** | `DEPTH_SNAPSHOT_REPLAY` — level-indexed top-N snapshots | `OrderBookDepth10`/`TradeTick`/`Bar` | **$0** free tier (API key) | **Reclassified L2→DEPTH_SNAPSHOT**: shipped client schema = `timestamp,side,level,price,size`, no `update_id/seq/is_snapshot` → snapshots, not deltas. Still the strongest single free source covering 4/4 non-Korean MM venues in NT-friendly parquet. "Free forever" is a homepage claim — no terms page found. |
| **Crypto Lake free tier** (`pip install lakeapi`) | ~10 exchanges, mainly **Binance**; no Korean | `book`=20-level 100ms snapshot (`DEPTH_SNAPSHOT`); **`book_delta_v2`=true `L2_REPLAY`** | `OrderBookDelta` (delta_v2) / `OrderBookDepth10` / `QuoteTick` / `TradeTick` / `Bar` | free ~most-recent 1yr | Parquet hive layout maps ~1:1 to NT `ParquetDataCatalog`. **Confirm `book_delta_v2` is in the free tier for your venue** before relying on it. |

### C. Free Hyperliquid

| Source | Fidelity | NT map | Cost | Notes |
|--------|----------|--------|------|-------|
| **Official S3** `s3://hyperliquid-archive/market_data` | `DEPTH_SNAPSHOT_REPLAY` (l2Book, ≥0.5s) | `OrderBookDepth10` | requester-pays egress | Canonical; "data may be missing" (gaps); **no trades/candles/raw-diffs via S3**. From ≥2023-09. |
| **Hydromancer Reservoir** `hydromancer.xyz` | `DEPTH_SNAPSHOT_REPLAY` (1-min L2) + fills + 1s OHLCV | `OrderBookDepth10`/`TradeTick`/`Bar` | free + requester-pays egress | Best free HL **fills/OHLCV** source incl. HIP-3 venues; book is 1-min (coarser than official 0.5s). Not tick L2. |

### D. Free Korean (kimchi leg)

| Source | Fidelity | NT map | Cost | Notes |
|--------|----------|--------|------|-------|
| **Upbit native REST/WS** | candles+trades `api_pull`; depth `self_capture` (top-15/30 snapshot) | `Bar`/`TradeTick`/`OrderBookDepth10` | free, keyless | Candles to Oct-2017 (200/req `to`-paging). **No historical book backfill** — depth is forward-capture only. |
| **Bithumb native REST/WS** | candles+trades `api_pull`; depth `self_capture` (full-book snapshot) | `Bar`/`TradeTick`/`OrderBookDepth10` | free, keyless | Candles ~2017 (v1 full series in one call). **No book archive; not on Tardis** → self-capture or Amberdata. |
| **Stooq USDKRW** daily CSV | `SIGNAL_ONLY` (FX reference) | `Bar` | free | Direct CSV for kimchi-premium normalization (daily; Yahoo `USDKRW=X` cross-check). |

### E. Free community datasets

| Source | Fidelity | NT map | Cost | Notes |
|--------|----------|--------|------|-------|
| **HF `linxy/CryptoCoin`** | `TRADE_BAR_REPLAY` (Binance OHLCV, 130+ pairs, since 2018) | `Bar` | free, MIT | Cleanest free HF card, **bars only** — no free HF card carries true crypto L2 for these venues. Long-horizon context, not microstructure. |

### F. Self-capture tooling (forward-only; turns "free" into true deltas)

| Tool | Venues | Captures true deltas? | NT map | License | Notes |
|------|--------|----------------------|--------|---------|-------|
| **cryptofeed** | 40+ incl. Binance/Bybit/OKX/**Upbit/Bithumb**/Kraken/Gate (no HL) | yes, where venue sends deltas (Binance/Bybit/OKX); Korean venues degrade to snapshot | `OrderBookDelta`/`TradeTick` | MIT-style | **Best single self-capture tool**; uniform delta schema incl. Korean. **No parquet backend ships** — write a ~20-line pyarrow callback. |
| **tardis-machine** (self-host) | many | yes (`book_change`) | `OrderBookDelta` | MPL-2.0 | Cleanest normalized-delta schema for NT. **Free real-time WS recording**; historical replay free only for 1st-of-month (else paid key). |
| **binance-LOB** / **pjschneiCMU/binance-websocket** | Binance only | yes (diff-depth + snapshot) | `OrderBookDelta` | MIT | Purpose-built LOB recorders. **Precision pitfall**: binance-LOB stores Float64 — re-parse raw strings into NT `Price`/`Quantity`. |
| **Native WS DIY** (Binance diff-depth / Bybit v5 / OKX books) | per-venue | yes, if you pick the *incremental* channel | `OrderBookDelta` | n/a | Fidelity = which channel you subscribe to. Max control, but you build reconnection/sequence-gap handling. cryptofeed wraps this. |
| **NT loaders/wranglers** (ingestion target) | Binance loader shipped; generic wrangler | — | → `ParquetDataCatalog` | LGPL-3.0 | Sets the target schema: capture → venue CSV/parquet → `OrderBookDeltaDataWrangler.process()` → `catalog.write_data()`. Write thin custom loaders for OKX/Upbit/Bithumb/HL. |
| **ccxt.pro / Freqtrade** | all venues | **no** (ccxt abstracts deltas away) | `OrderBookDepth10`/`TradeTick`/`Bar` | MIT / GPL-3.0 | ccxt.pro `watchOrderBook` hands you the merged book → snapshots only. Freqtrade = best free **historical trades/OHLCV** backfill (native parquet), not L2. |

### G. Paid L2 (Phase-3 escalation only)

| Source | Venues | Fidelity | Cost | Notes |
|--------|--------|----------|------|-------|
| **Tardis.dev** | Binance/Bybit/OKX (native deltas), HL/Upbit (snapshot-derived), Deribit, Coinbase L3, +30 | `L2_REPLAY` (real-delta venues) | **Subscription tiers personally verified 2026-05-30 at tardis.dev**: Perps $350/700/900/2500; Spot $450/900/1350/3500; Options $350–2500; All-Exchanges $650–6000 (yearly for full history). **Free 1st-of-month CSV samples, no key** (new finding). | Cleanest NT loader. For HL/Upbit the "incremental_book_L2" is snapshot-derived → `DEPTH_SNAPSHOT`, do **not** label `L2_REPLAY`. No Bithumb. |
| **CoinAPI Flat Files** | 380+ at L2; Coinbase/Bitso L3 | `L2_REPLAY` (`limitbook_full`) | `contact_sales` — blog scenarios ~$70–464/mo, $25 free credits; **pricing page 403'd, unverified** | Turnkey daily `.csv.gz`/parquet. Verify history-start + price before commit. |
| **Kaiko** | 100+; L3 = Coinbase/Bitstamp/Bitfinex | `DEPTH_SNAPSHOT_REPLAY` (L2 = interval snapshots); Coinbase L3 = tick | `contact_sales` (~$1–2.5k/mo unverified) | Deep history its only edge; ADX channel unanchored. Deprioritize. |
| **Amberdata** | Bithumb (+broad) | `DEPTH_SNAPSHOT_REPLAY` (1-min) | `contact_sales` | **Only turnkey pre-recorded Bithumb book** (since 2021-06-02). Use only if you can't self-capture forward. |
| **Binance T_DEPTH** | Binance futures | `L2_REPLAY` (depth_snap + depth_update) | `unknown` — VIP-1+ & API-only | Corrects "no Binance L2 anywhere": official deltas exist but VIP-gated with documented gaps. Self-capture usually more accessible. |
| **Dwellir** | Hyperliquid | `L2_REPLAY` *claimed* (`node_raw_book_diffs_by_block`) | `contact_sales`, no price | **Only** non-self-capture lead for HL tick L2, but claimed start Jan-2026+, **low confidence** — get a sample to confirm genuine per-block diffs before paying. |
| **Databento** | — | — | — | **Rejected: zero crypto** (would only fit CME-listed BTC/ETH derivatives if ever a fixture). |

## Hyperliquid verdict

**No turnkey true-tick-L2 source exists for Hyperliquid, and the cause is
structural.** The public `l2Book` WS channel is, per the official docs, a
"Snapshot feed, pushed on each block that is at least 0.5s since last push" — there
is **no incremental/diff channel** on the public API. So every public-API vendor
(Tardis, CoinAPI, Amberdata, Kaiko, Hydromancer) is capped at
`DEPTH_SNAPSHOT_REPLAY` at ≥0.5s — including Tardis's HL "incremental_book_L2,"
which is diffed from those ≥0.5s snapshots, not native deltas. Genuine tick L2
(per-block order-level diffs) exists **only** at the L1/node layer: run
`hyperliquid-dex/node --write-raw-book-diffs --batch-by-block` (or the
`order_book_server` l4book/StreamL4Book gRPC) — both **forward-capture only**
(16 vCPU / 128 GB / 500 GB+ SSD, ~100 GB logs/day), with **no public historical
backfill** (raw diffs are absent from every official S3 bucket). The one
non-self-capture lead is **Dwellir** (bulk `node_raw_book_diffs_by_block`, claimed
Jan-2026+) — contact-sales, low confidence. **Plan HL at `DEPTH_SNAPSHOT_REPLAY`
(free official S3 or Hydromancer); for tick, start a self-node recorder now.**
This confirms the first survey's HL conclusion, now with the upstream-docs reason.

## Korean verdict

**Best free/native path = the exchange APIs directly; the real fidelity ceiling
for both Korean venues is `DEPTH_SNAPSHOT_REPLAY`, never `L2_REPLAY`** — neither
Upbit nor Bithumb publishes a native incremental delta channel (each pushes the
full top-N book every WS message: Upbit ≤15, cap 30; Bithumb full book). **Upbit
(free, keyless):** candles to Oct-2017, recent trades, 30-level snapshots — **no
native book backfill** (depth = self-capture forward; paid Tardis since 2021-03-03
is the only book *history*, and snapshot-grade). **Bithumb (free, keyless):**
candles since ~2017, trades — **no book archive, not on Tardis** → self-capture
forward (cryptofeed covers it) or Amberdata 1-min (since 2021-06-02, paid).
**USDKRW** free daily from Stooq (`SIGNAL_ONLY`). **Net:** backfill Upbit+Bithumb
candles+trades from native APIs now, **start a forward cryptofeed depth recorder on
both immediately** (the kimchi book is the weakest-covered leg), Stooq for FX.

## Recommendation — start free, escalate only when a strategy earns it

The prior "Tardis-first" framing is corrected: the cheapest *true-L2* paths are
**venue-native archives (Gate.io/OKX) and self-capture**, not a paid vendor.

| Phase | Goal | Sources | Fidelity | Cost |
|-------|------|---------|----------|------|
| **0 — smoke fixture** (hours) | prove `ParquetDataCatalog` ingest + NT replay end-to-end | Binance Vision trades+klines; **Tardis free 1st-of-month** `incremental_book_L2` (Binance/Bybit/OKX) | `TradeTick`/`Bar` + one `OrderBookDelta` slice | **$0** |
| **1 — free multi-venue corpus** (days) | months-long depth across primary venues, $0 | **CryptoHFTData free** (Binance/Bybit/OKX/Kraken/HL snapshots); **OKX & Gate.io native L2** (confirm schema); HL official S3 + Hydromancer; **Upbit/Bithumb native** candles+trades + forward depth recorder; Stooq USDKRW | mostly `DEPTH_SNAPSHOT`; `L2` where native | **$0** |
| **2 — forward self-capture** (set up once) | true tick deltas where the strategy needs them | **cryptofeed** (Binance/Bybit/OKX + Korean) → parquet; HL self-node for sub-0.5s | `L2_REPLAY` | **$0** (infra/ops) |
| **3 — pay per-leg, only when earned** | deep historical true L2 on a proven strategy | Tardis / CoinAPI `limitbook_full` (Binance/Bybit/OKX); Tardis Upbit; Amberdata Bithumb; Coinbase L3 for queue research; Dwellir HL (verify first) | `L2_REPLAY` | quote first |

**Is Tardis "really recommended"?** Still not to start. Free native archives +
self-capture now cover far more than the first survey credited, and Tardis's own
**free 1st-of-month samples** handle the smoke fixture at $0. Tardis wins later on
**breadth + the cleanest NT loader** for deep multi-venue history — a Phase-3
escalation, not the opening move.

**Cross-cutting fidelity honesty:** only Gate.io/OKX native L2, Tardis
`incremental_book_L2` (real-delta venues), Crypto Lake `book_delta_v2`, CoinAPI
`limitbook_full`, Binance T_DEPTH, and self-captured diff streams are true
`OrderBookDelta`. Everything else (CryptoHFTData, Bybit ob500, all Hyperliquid,
Upbit/Bithumb, Kaiko/Amberdata) is `OrderBookDepth10` — never overclaim.

**Precision:** preserve raw string price/qty into NT `Price`/`Quantity` — never
route book data through Float64 (the binance-LOB pitfall).

## Required-check status (selected)

| Check | Gate.io | OKX native | CryptoHFTData | Crypto Lake | cryptofeed | Tardis |
|-------|---------|-----------|---------------|-------------|------------|--------|
| schema | ⚠️ confirm cols | ⚠️ confirm cols | ✅ snapshot | ✅ delta_v2/snapshot | ✅ delta | ✅ delta |
| sample pointer | ✅ S3 listed | ✅ portal | ✅ pip | ✅ pip | ✅ examples | ✅ free 1st/mo |
| fidelity (claimed=evidenced) | ⚠️ L2 pending col check | ⚠️ L2 pending col check | ✅ DEPTH_SNAPSHOT | ✅ both tiers | ✅ L2 (delta venues) | ✅ L2 (real-delta) |
| NT mapping | ✅ OrderBookDelta | ✅ pending | ✅ OrderBookDepth10 | ✅ OrderBookDelta | ✅ OrderBookDelta | ✅ OrderBookDelta |
| license (commercial) | ⚠️ unknown | ⚠️ unknown | ⚠️ no terms page | ⚠️ ToS verify | ✅ permissive | ⚠️ internal-use verify |
| cost (units + URL) | ✅ $0 (verified archive) | ✅ $0 | ✅ $0 free tier | ✅ free ~1yr | ✅ $0 | ✅ tiers verified |

## Open verifications (the weak layer)

Decision-grade *coverage/fidelity* is strong; these must be closed before any
**purchase** or before treating a free archive as production-eligible:

1. **Gate.io** — decompress one `spot/orderbooks/*.csv.gz` to confirm set/make/take delta columns + 100ms granularity + earliest month for target symbols; confirm redistribution license (article 21688 says "for quantitative backtesting," no formal license).
2. **OKX native L2** — pull one Order Book file: incremental (`seqId/action`→`OrderBookDelta`) vs snapshot, depth levels, format, volume caps; verify redistribution terms.
3. **CryptoHFTData** — no terms/license page fetchable; "free forever" is a homepage claim. Re-fetch written terms; discover real `history_start` per (exchange,symbol) via `list_symbols()`.
4. **Tardis bulk pricing** — subscription tiers verified, but bulk/all-exchanges quote and Coinbase-L3 raw-API tier need an instant quote; confirm ToS internal-use clause.
5. **CoinAPI** — pricing page Cloudflare-403; $70–464/mo + $25 credits are blog-sourced. Re-fetch pricing + `limitbook` catalog + Coinbase-L3 history-start.
6. **Kaiko** — $1–2.5k/mo unverified; ADX channel unanchored; confirm HL/Upbit/Bithumb coverage.
7. **Amberdata Bithumb** — contact-sales, confirm cost/license; 1-min cadence acceptable?
8. **Binance T_DEPTH** — cost, VIP-1 gating, spot availability, documented gaps.
9. **Dwellir HL** — sample to confirm genuine per-block raw diffs (not reconstructed) + real history depth before paying.
10. **License for all "free" exchange archives** (Binance Vision, Bybit public, Kraken, Upbit/Bithumb, Gate.io, OKX) — none publishes a formal data-redistribution license; confirm each permits backtest/commercial use.
11. **Bybit ob500** — per-symbol book backfill start (may be 2023–2025, not the 2020 trades start) — verify earliest file for target symbols.
12. **Crypto Lake free tier** — confirm `book_delta_v2` is in the free ~1yr tier for your venue; confirm paid plan price.

## Method

Two passes. First: web-cited single-pass survey (Tardis pricing personally
verified). Second (this doc): `crypto-data-source-sweep` workflow — 6 parallel
angle agents (exchange-native / community datasets / free-cheap archives /
self-capture tooling / Hyperliquid-tick / Korean), a completeness critic, 6
gap-fill agents, then synthesis. The Gate.io S3 archive was additionally listed
directly this session. Every claim carries a fetched-URL anchor; no price was
invented (missing prices recorded as `contact_sales` / `unknown`).
