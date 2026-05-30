# Source proofs (BTE-015/016/017)

Decision-grade survey of historical market-data sources for the two BTE fixtures.
A source becomes backtest-eligible only via an accepted `SourceProofReport`
([schema](./schema.md)). Surveyed 2026-05-30 across 9 sources (web-cited);
Tardis pricing personally re-verified, other pricing/license survey-sourced.

- **[schema.md](./schema.md)** — the `SourceProofReport` contract (BTE-015)
- **[binary-option.md](./binary-option.md)** — Polymarket / Kalshi (BTE-016)
- **[perps-spot.md](./perps-spot.md)** — CEX / DEX / vendors (BTE-017)

## Bottom line

| Fixture | Free history? | L2 history? | Recommended start | Pay later? |
|---------|--------------|-------------|-------------------|-----------|
| **binary option** | **free bulk trades + free hourly L2 (pmxt)** | **YES, both venues** — free hourly snapshots; finer L2 paid | free pmxt L2 + bulk trades | **Telonex** (PM tick), **kalshi.com / kalshibacktest** (Kalshi) |
| **perps/spot** | trades + futures snapshots (Binance, $0) | only paid / self-node | **free Binance/HL** for nimble research | **CoinAPI ~$1/GB** for targeted L2; Tardis only if breadth earns it |

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

Start free, escalate only when a strategy earns it. Do **not** open with a Tardis
subscription — for a small scope, CoinAPI's pay-per-GB L2 matches Tardis fidelity
far cheaper, and free Binance/HL data covers the nimble Python research lane. This
phasing mirrors the **Python-nimble → Rust-final** model: free data to find a
promising strategy, paid L2 only to validate its execution before production.

```
Phase 0  free (Binance public, HL archive, Deribit trades)  → Python research / signal sanity
Phase 1  CoinAPI Flat Files (~$1/GB, pay-per-use)           → targeted L2 for a promising strategy
Phase 2  Tardis ($350+/mo) — only if breadth justifies      → broad continuous multi-venue L2
```

## Hard constraints to remember

- **Hyperliquid** has no turn-key tick-L2 history (snapshot-cadence only, or run
  your own node) — plan HL at `DEPTH_SNAPSHOT_REPLAY`.
- **Databento** carries **no** crypto — out for this fixture (would fit only
  CME-listed BTC/ETH derivatives).
- A **snapshot-derived** "incremental" feed (HL, Upbit via Tardis) is
  `DEPTH_SNAPSHOT_REPLAY`, not `L2_REPLAY` — don't overclaim fidelity.

## Verify-before-commit (the weak layer)

Fidelity/coverage findings are strong (doc fetches); prediction-market licensing
is BD-cleared and Telonex is operator-validated. The remaining unverified items,
all on the crypto side: (1) CoinAPI per-GB pricing (403'd on re-fetch),
(2) Tardis ToS internal-use clause, (3) Binance-public data license,
(4) Hyperliquid data license + true archive start date.

## Method

`bte-source-proof-survey` workflow: 9 parallel researchers (one per source), each
fetching vendor docs/pricing and citing URLs, then a completeness critic. Every
claim carries a fetched-URL anchor; no pricing number was invented (missing
prices are recorded as `contact_sales` / `not_public`). Full per-source evidence
URLs are in the populated reports.
