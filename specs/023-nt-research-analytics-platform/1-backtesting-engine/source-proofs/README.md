# Source proofs (BTE-015/016/017)

Decision-grade survey of historical market-data sources for the two BTE fixtures.
A source becomes backtest-eligible only via an accepted `SourceProofReport`
([schema](./schema.md)). Binary-option surveyed + operator-corrected 2026-05-30;
perps/spot re-surveyed the same day via a 6-angle, 14-agent sweep (76 candidate
sources) after the first pass under-covered the free/native class. Tardis
subscription tiers personally verified; the Gate.io S3 archive listed directly;
other pricing/license survey-sourced (see each report's open-verifications).

- **[schema.md](./schema.md)** — the `SourceProofReport` contract (BTE-015)
- **[binary-option.md](./binary-option.md)** — Polymarket / Kalshi (BTE-016)
- **[perps-spot.md](./perps-spot.md)** — CEX / DEX / vendors (BTE-017)

## Bottom line

| Fixture | Free history? | L2 history? | Recommended start | Pay later? |
|---------|--------------|-------------|-------------------|-----------|
| **binary option** | **free bulk trades + free hourly L2 (pmxt)** | **YES, both venues** — free hourly snapshots; finer L2 paid | free pmxt L2 + bulk trades | **Telonex** (PM tick), **kalshi.com / kalshibacktest** (Kalshi) |
| **perps/spot** | trades+klines+snapshots, $0 (Binance/Bybit/OKX/Kraken/HL) | **YES — free native L2** (Gate.io, OKX) + Crypto Lake `book_delta_v2` free tier; self-capture for the rest | free **CryptoHFTData** + **OKX/Gate native L2** + **cryptofeed** self-capture | Tardis/CoinAPI only for deep history on a proven strategy |

## Key facts

**Both prediction venues have L2 history** (survey corrected by operator):
- **Free, hourly, both venues + Limitless/Opinion:** `archive.pmxt.dev` orderbook
  snapshots (parquet) → `DEPTH_SNAPSHOT_REPLAY`. Good for feature/signal research.
- **Polymarket tick L2:** **Telonex** (operator-validated; Enterprise = commercial).
- **Kalshi L2:** **kalshi.com/market-data** (operator-pointed) + **kalshibacktest**
  (100ms, crypto 15-min markets, $19.90/mo).
- **Free bulk trades:** SII-WANGZJ (1.1B, MIT), jon-becker (Polymarket+Kalshi).
- Forward-capture is now the free *tick-fidelity* / gap-fill option, not mandatory.

The early "no prediction-market L2 exists" finding was a **web-survey miss** — it
checked native APIs + on-chain only, not specialist vendors/archives the operator
uses. Operator knowledge supersedes the survey.

**Licensing — resolved for binary option.** The operator has direct BD agreements
with Polymarket and Kalshi to use their data for trading. Crypto vendors permit
internal use but restrict *redistribution* — fine for our own backtesting.

## Cost posture (decided)

Start free, escalate only when a strategy earns it. The re-survey corrected the
first pass's pessimism: **free, turnkey, true-`OrderBookDelta` history exists**
(Gate.io and OKX native archives; Crypto Lake `book_delta_v2` free tier; Tardis
free 1st-of-month samples), and free **self-capture** (cryptofeed) covers forward
deltas on every venue incl. the Korean pair. Paid vendors are a Phase-3
escalation, not the starting point. This mirrors the **Python-nimble →
Rust-final** model: free data to find a strategy, paid L2 only to validate
execution before production.

```
Phase 0  free (Binance Vision + Tardis free 1st-of-month L2)    → smoke fixture: prove NT ingest/replay
Phase 1  free multi-venue (CryptoHFTData + OKX/Gate native L2)  → months-long depth corpus, $0
Phase 2  free self-capture (cryptofeed → parquet)              → true tick deltas where a strategy needs them
Phase 3  paid per-leg (Tardis/CoinAPI/Amberdata) — only earned  → deep historical true L2
```

## Hard constraints to remember

- **Hyperliquid** has no turn-key tick-L2 history (snapshot-cadence only, or run
  your own node) — plan HL at `DEPTH_SNAPSHOT_REPLAY`.
- **Databento** carries **no** crypto — out for this fixture (would fit only
  CME-listed BTC/ETH derivatives).
- A **snapshot-derived** "incremental" feed (HL, Upbit via Tardis) is
  `DEPTH_SNAPSHOT_REPLAY`, not `L2_REPLAY` — don't overclaim fidelity.
- **Korean venues** (Upbit, Bithumb) have no native delta channel — full-book
  snapshots only → `DEPTH_SNAPSHOT_REPLAY` ceiling for free; book must be
  self-captured forward (cryptofeed) or bought (Tardis Upbit / Amberdata Bithumb).
- **`OrderBookDepth10`, not deltas:** CryptoHFTData, Bybit ob500, all Hyperliquid,
  Upbit/Bithumb, Kaiko/Amberdata, ccxt.pro. Only Gate.io/OKX native, Tardis
  `incremental_book_L2`, Crypto Lake `book_delta_v2`, CoinAPI `limitbook_full`,
  Binance T_DEPTH, and self-captured diff streams are true `OrderBookDelta`.

## Verify-before-commit (the weak layer)

Fidelity/coverage findings are strong (doc fetches; the Gate.io S3 archive was
listed directly this session); prediction-market licensing is BD-cleared and
Telonex is operator-validated. The crypto-side items still open (full list in
[perps-spot.md](./perps-spot.md#open-verifications-the-weak-layer)):
(1) **delta-vs-snapshot column schema** of the two free native L2 archives —
confirm by decompressing one Gate.io / OKX file; (2) **commercial/redistribution
license** for every nominally-free exchange archive (none publishes a formal one);
(3) paid-vendor pricing (CoinAPI/Kaiko/Amberdata `contact_sales`; Tardis bulk
quote); (4) Bybit ob500 per-symbol book start date; (5) Crypto Lake free-tier
`book_delta_v2` inclusion; (6) Dwellir HL raw-diffs authenticity before any buy.

## Method

`bte-source-proof-survey` workflow: 9 parallel researchers (one per source), each
fetching vendor docs/pricing and citing URLs, then a completeness critic. Every
claim carries a fetched-URL anchor; no pricing number was invented (missing
prices are recorded as `contact_sales` / `not_public`). Full per-source evidence
URLs are in the populated reports.
