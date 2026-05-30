# SourceProofReport — `binary option` fixture (BTE-016)

Populated against the [schema](./schema.md). Surveyed 2026-05-30 (web-cited).
Headline: **both prediction venues require forward-capture for L2, and both
restrict commercial use of their data** — these are the two gating facts.

> **Confidence asymmetry (carry into every decision):** the *data-structure*
> findings below are high-confidence (direct doc fetches). The *commercial-license*
> findings are the weak layer — Polymarket's TOS clause came from a search excerpt
> (JS-rendered page), and Kalshi's API developer agreement could not be fetched
> (429). **Legal review required before any commercial use of either.**

## Candidates (official/free first)

### 1. Polymarket — `official_free`, `selection_status: candidate`

| Field | Finding |
|-------|---------|
| Venues | Polymarket CTF / neg-risk CTF binary outcome-token markets (Polygon). |
| Data classes | LIVE L2 (WS `book` snapshot + `price_change` deltas); historical **trades** (Data API `/trades`); 1-min price timeseries (`/prices-history`); on-chain fills/OI/volume (Goldsky/The Graph). |
| **Highest historical fidelity** | **`TRADE_BAR_REPLAY`** — trades + 1-min price points. **No historical L2 store exists anywhere.** |
| `forward_capture_status` | **`required`** for any L2_REPLAY (live WS `book`+`price_change` → `OrderBookDelta`). |
| NT mapping | Historical `/trades` → `TradeTick` (NT `PolymarketDataLoader.load_trades()`); live WS → `OrderBookDelta`. NT loader exposes **no** order-book-history helper. |
| License | **Restricted — commercial use prohibited without written consent** (TOS: *"Commercial use of any of the Content is prohibited"*, bars derivative indices from its prices). ⚠️ excerpt-sourced, confirm verbatim. |
| Cost | Native CLOB/Data/Gamma APIs **free** (rate-limited). Goldsky (on-chain only, not L2) freemium. |
| Evidence | docs.polymarket.com (CLOB/timeseries, get-prices-history, get-order-book, trades, WS market-channel); nautilustrader.io/docs/integrations/polymarket. |

### 2. Kalshi — `official_free`, `selection_status: candidate`

| Field | Finding |
|-------|---------|
| Venues | Kalshi (CFTC-regulated US event/prediction exchange), single venue. |
| Data classes | Historical **candlesticks** (OHLC of yes_bid/yes_ask/price + vol + OI) and **trades** (`/historical/trades`); LIVE-only orderbook (REST snapshot + WS `orderbook_delta`). |
| **Highest historical fidelity** | **`TRADE_BAR_REPLAY`** — candlesticks + trades. **No archived orderbook** (historical tier lists no orderbook). |
| `forward_capture_status` | **`required`** for L2 (WS `orderbook_delta` is a true price-level book, live only). |
| NT mapping | candlesticks → `Bar`; `/historical/trades` → `TradeTick`. |
| License | **Restricted** — Data Terms: *"access content only for your personal use for non-commercial purposes."* ⚠️ API developer agreement not fetched (429) — confirm. |
| Cost | Market data **free** with an account; advanced API access = application (not public). |
| Evidence | docs.kalshi.com (historical_data, historical-cutoff, market-candlesticks, market-orderbook, quick_start_websockets). |

> Kalshi orderbook history is exactly the open question in **BTE-023** — confirmed
> here: **no archived orderbook snapshots/deltas exist**, so BTE-023 resolves to
> "downgrade to `TRADE_BAR_REPLAY` / forward-capture for L2."

## Recommendation

**Selection: both venues `accepted` at `TRADE_BAR_REPLAY` for history;
`forward_capture_status: required` for any L2_REPLAY.** No paid candidate is
admitted — paid vendors do **not** archive prediction-market order books either
(checked; the free official APIs are the only realistic source, and they top out
at trades).

Concretely:
- **Now (free, $0):** load historical **trades** via NT's `PolymarketDataLoader`
  → `TradeTick`. Fidelity `TRADE_BAR_REPLAY`. **Forbidden claims:** no order-book
  or fill realism — trade-through / signal backtests only.
- **For L2 execution realism:** stand up **forward capture** of the live WS book
  feeds (`OrderBookDelta`) — there is no shortcut; L2 history does not exist. This
  is the `FORWARD_CAPTURE_PENDING` state the project already anticipated for
  Polymarket.
- **Blocker — commercial license:** both venues restrict commercial use of their
  data. This must clear **legal review** before a commercial (real-money) strategy
  relies on it. Until then, treat binary-option backtests as research-only.

## Required-check status

| Check | Polymarket | Kalshi |
|-------|-----------|--------|
| schema | ✅ | ✅ |
| sample pointer | ✅ (Data API /trades) | ✅ (/historical/trades) |
| time coverage | ✅ to inception (paginate-capped) | ✅ rolling → historical tier |
| fidelity (claimed = evidenced) | ✅ `TRADE_BAR_REPLAY` | ✅ `TRADE_BAR_REPLAY` |
| NT mapping | ✅ TradeTick | ✅ Bar/TradeTick |
| **license (commercial)** | ⚠️ **blocked — legal review** | ⚠️ **blocked — legal review** |
| forbidden-claims recorded | ✅ | ✅ |
