# SourceProofReport — `perps/spot` fixture (BTE-017)

Populated against the [schema](./schema.md). Surveyed 2026-05-30 (web-cited).
Tardis.dev pricing was **personally re-verified** at `tardis.dev` (2026-05-30);
CoinAPI / Kaiko / Amberdata pricing is survey-sourced and **not** independently
re-verified.

> **Confidence asymmetry:** fidelity/coverage findings are high-confidence (direct
> doc fetches); commercial-license and (except Tardis) exact-pricing are the weak
> layer — verify a vendor's clause and quote before any purchase.

## Candidates (official/free first)

### 1. Binance public data (`data.binance.vision`) — `free_public`

| Field | Finding |
|-------|---------|
| Venues | Binance Spot, USD-M (USDT) Futures/Perps, COIN-M. |
| Data classes | Spot: trades/aggTrades/klines only. Futures: + `bookTicker` (L1 BBO, since 2023) + `bookDepth` (aggregated ±%-band snapshots, ~1-min). |
| **Highest fidelity** | Spot → **`TRADE_BAR_REPLAY`**; Futures → **`DEPTH_SNAPSHOT_REPLAY`** (L1 + 1-min band snapshots). **No full L2 incremental book anywhere.** |
| NT mapping | trades/aggTrades → `TradeTick`; `bookTicker` → `QuoteTick`; klines → `Bar`; `bookDepth` → no clean NT class (aggregated bands). |
| History | Spot trades/klines since 2017-08; futures bookTicker/bookDepth since 2023. Daily, ~1-day lag. |
| License | ⚠️ README inline "MIT" only (no LICENSE file; unclear if covers data or scripts). Free, so low budget risk; commercial-redistribution unconfirmed. |
| **Cost** | **$0** (public S3, no key). |

### 2. Hyperliquid official archive — `official_free`

| Field | Finding |
|-------|---------|
| Data classes | `s3://hyperliquid-archive/market_data`: L2 **snapshots** (20 lvl/side, per-block ≥0.5s) + trades + asset ctxs. True tick-L2 only by running your own node (`--write-raw-book-diffs`). |
| **Highest fidelity** | Archive → **`DEPTH_SNAPSHOT_REPLAY`**; tick **`L2_REPLAY`** only via self-run node (free SW, Apache-2.0; operational effort). |
| NT mapping | l2Book snapshots → `OrderBookDepth10` / snapshot-flagged deltas. |
| History | since ~2024-10-29 (per Tardis); archive uploaded ~monthly, "data may be missing." |
| License | ⚠️ node SW Apache-2.0; **data license not confirmed**. Archive is **requester-pays** (AWS egress ~$0.09/GB std). |
| **Cost** | AWS requester-pays egress only (no vendor fee). |

### 3. Deribit (options/futures) — `official_free` (trades) / needs vendor for L2

| Field | Finding |
|-------|---------|
| Data classes | Own API: **free historical trades** back to platform start. **No** historical book/L2/quotes (`get_order_book` is live-only). |
| **Highest fidelity** | Own API → **`TRADE_BAR_REPLAY`**; **`L2_REPLAY`** only via Tardis (Deribit's official historical partner). |
| **Cost** | Deribit trades **free**; L2 via Tardis (see below). |

### 4. CoinAPI Flat Files — `paid_vendor` (admitted: free sources lack crypto L2)

| Field | Finding |
|-------|---------|
| Data classes | Full tick **L2/L3** (`limitbook_full`), quotes/BBO, trades, OHLCV — 380+ exchanges incl. Binance/Deribit/Hyperliquid. |
| **Highest fidelity** | **`L2_REPLAY`** (`OrderBookDelta`). |
| License | Commercial/internal use (backtesting, prop, research) allowed; raw-data redistribution prohibited. |
| **Cost (⚠️ survey-sourced, not re-verified — 403 on re-fetch)** | Flat Files **pay-as-you-go ~$1.00/GB** for L2&3 (down to ~$0.71/GB committed); requests ~$10/1k. **Pay only for what you pull.** |
| Evidence | coinapi.io/products/flat-files (verify before commit). |

### 5. Tardis.dev — `paid_vendor` (admitted: gold-standard breadth)

| Field | Finding |
|-------|---------|
| Data classes | `incremental_book_L2` (true tick deltas) for Binance spot, Binance USD-M perps, Deribit; **snapshot-derived** for Hyperliquid/Upbit; + trades, BBO, depth snapshots, derivative ticker, liquidations. |
| **Highest fidelity** | **`L2_REPLAY`** for Binance/Deribit (native deltas); **`DEPTH_SNAPSHOT_REPLAY`** for HL/Upbit (snapshot-derived — do **not** label L2_REPLAY). |
| NT mapping | `incremental_book_L2` → `OrderBookDelta` (NT `TardisCSVDataLoader.load_deltas()`). Cleanest NT integration of any vendor. |
| License | ⚠️ "internal business use" permitted; redistribution restricted (verify Clause 9.1/9.2). |
| **Cost (personally verified at tardis.dev, 2026-05-30)** | Subscription/mo: **Perpetuals $350 / $700 / $900 / $2,500** (Academic/Solo/Pro/Business); **Spot $450 / $900 / $1,350 / $3,500**; **Options $350–$2,500**; **All-Exchanges $650 / $1,200 / $2,200 / $6,000**. Billed yearly for full history. |

### 6. Databento — **rejected** (no crypto)

Equities/futures/options only; **zero** crypto data (Binance is "considering" on
roadmap). Not usable for this fixture. (Note: *would* cover CME-listed BTC/ETH
futures+options at L3 if that ever becomes a fixture.)

### 7. Kaiko / Amberdata — `paid_vendor` alternates

Both `L2_REPLAY`-capable (Kaiko bids-and-asks tick since 2015; Amberdata
order-book events). Pricing not public (Kaiko indicative ~$1–2.5k/mo, unverified).
Hold as alternates to CoinAPI/Tardis.

### 8. Kimchi-premium leg (conditional, BTE-017) — Upbit + Bithumb + USDKRW

- **Upbit:** Tardis `incremental_book_L2` since 2021-03-03 (snapshot-derived →
  `DEPTH_SNAPSHOT_REPLAY`). **Bithumb:** not on Tardis; Amberdata gives 1-min
  snapshots + trades. **USDKRW:** `SIGNAL_ONLY` reference (free FX APIs).
- Only pursue when the kimchi-premium signal family is actually selected.

## Recommendation — start free, escalate only when a strategy earns it

The fixture needs L2 *eventually* (market-making is a book game), but **not from
day one**, and **not via a Tardis subscription to start** — it's the most
expensive option and overkill for early research.

| Phase | Source | Fidelity | Cost | Use |
|-------|--------|----------|------|-----|
| **0 — nimble research** | Binance public data (+ HL free archive, Deribit free trades) | `TRADE_BAR_REPLAY` / `DEPTH_SNAPSHOT` | **$0** | Python lane: signal sanity, strategy logic. Forbidden: execution-quality fill claims. |
| **1 — targeted L2** | **CoinAPI Flat Files** (pay-per-GB) | `L2_REPLAY` | **~$1/GB**, no subscription | Pull L2 for the *specific* symbols/dates of a promising strategy → Rust execution validation. |
| **2 — breadth (only if earned)** | Tardis.dev subscription | `L2_REPLAY` (Binance/Deribit native) | **$350+/mo** | Only once many strategies need broad, continuous multi-venue L2. Cleanest NT loader. |

**Is Tardis "really recommended"?** Not to start. For a small initial scope,
CoinAPI's pay-per-GB L2 delivers the *same fidelity* at a fraction of the cost,
and free Binance/HL data covers nimble Python research outright. Tardis wins only
on **breadth + convenience** (one subscription, all venues, the cleanest NT
`TardisCSVDataLoader` path) — worth it later if scope justifies the subscription,
not before. This phasing also matches the Python-nimble → Rust-final model: free
data to find a promising strategy, paid L2 only to validate its execution.

**Hyperliquid caveat (hard constraint):** HL is a marquee bolt-v3 venue but has
**no turn-key historical tick-L2** — the free archive and every vendor are
snapshot-cadence (≥0.5s). True tick-L2 requires running your own HL node. Plan HL
backtests at `DEPTH_SNAPSHOT_REPLAY` unless/until a node is stood up.

## Required-check status (selected)

| Check | Binance-public | CoinAPI | Tardis | Hyperliquid |
|-------|---------------|---------|--------|-------------|
| fidelity (claimed = evidenced) | ✅ TRADE_BAR / SNAPSHOT | ✅ L2_REPLAY | ✅ L2 (Binance/Deribit) | ✅ SNAPSHOT (archive) |
| NT mapping | ✅ | ✅ OrderBookDelta | ✅ OrderBookDelta | ✅ OrderBookDepth10 |
| **license (commercial)** | ⚠️ MIT-as-stated, unconfirmed | ⚠️ allowed, verify | ⚠️ internal-use, verify | ⚠️ not confirmed |
| **cost (with units + URL)** | ✅ $0 | ⚠️ ~$1/GB survey-sourced | ✅ verified at vendor | ✅ AWS egress |
| time coverage | ✅ | ✅ | ✅ | ⚠️ ~2024-10 start |
