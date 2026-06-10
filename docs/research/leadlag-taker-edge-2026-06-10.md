# Lead-Lag Measurement — does the spot→Polymarket taker edge exist? (Session 4)

Date: 2026-06-10. Issue: #617. Declared scope: the session-4 brief
(`session4leadlagmeasurement.md`) only. Produced by `scripts/leadlag_session4.py`
(section 8 has the exact reproduction commands).

## 0. Verdict summary

**The lead-lag premise is real and large.** Across 7 full days and four assets, a ≥5 bps
1-second spot move produces a Polymarket up/down mid response of **+10 to +28 cents per
share** over the following 1–60 s — the book under-reacts for tens of seconds, not
milliseconds.

Per-asset GO/NO-GO (net taker edge after half-spread + the 1000 bps taker fee):

| asset | best (X,h) | max net pre-move (c) | 95% CI | net executable (c) | events | verdict |
|---|---|---|---|---|---|---|
| btc | X=5bps, h=30s | +16.98 | [+14.22, +19.73] | +10.57 | 134 | **GO** |
| eth | X=10bps, h=30s | +16.42 | [+11.46, +21.37] | +2.54 | 48 | NO-GO at 1s reaction (pre-move edge only) |
| sol | X=10bps, h=60s | +18.41 | [+11.22, +25.60] | +1.42 | 42 | NO-GO at 1s reaction (pre-move edge only) |
| xrp | X=5bps, h=30s | +11.85 | [+8.66, +15.04] | -2.22 | 188 | NO-GO at 1s reaction (pre-move edge only) |

Reading: "pre-move" enters at the ask quoted 1 s before the spot move completed (the
brief's definition — capturable only by a sub-second taker); "executable" enters at the
ask observable *after* the 1-second move is complete. **BTC clears costs decisively even
at a 1-second reaction** (+10.6 c/share mean, CI excludes zero, and the executable edge
is still rising at the 30 s horizon — the BTC book drifts toward the new fair value for
~30 s). ETH reprices within ~1 s (its response at h=1 already equals its response at
h=30), so its +10–16 c edge belongs to whoever reacts in well under a second; SOL is the
same shape; XRP's wide spreads plus the fee eat the entire response at any reaction
speed.

The maker counterfactual (section 7) does **not** rescue the NO-GO assets: passive fills
mark out at −0.0 to −0.4 c/share mean (median −0.5 c) at 10–60 s horizons — adverse
selection slightly exceeds spread capture on these books.

Caveats that bound the GO (section 1, "Caveats"): top-of-book sizes were not measured
(fillability at the quoted ask is assumed, not proven), and cross-venue clock skew is
unverified. The recommended next step before sizing real capital is a live shadow run on
the host comparing quoted-ask-at-signal vs achieved fill price.

## 1. Data and method

The brief targets the live host's capture catalog (`/var/lib/bolt/catalog`). Interactive
access to the production host was denied by the session permission layer, so the
measurement runs on the project's S3 data lake plus public metadata instead — the same
underlying market data, independently captured:

| Input | Source | Window used |
|---|---|---|
| Polymarket up/down CLOB top-of-book + trades | pmxt archive staged in the lake: `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/` (hourly parquet) | 2026-04-22 → 2026-04-28 (7 full UTC days) |
| Spot leader mid | Hyperliquid perp `l2Book` 20-level snapshots, ~0.54 s cadence: `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-core-targeted-.../source_family=l2Book` | same 7 days |
| Market identity, fees, settlement | Polymarket Gamma API, slug `{asset}-updown-5m-{period_start}` (the runtime slug contract in `src/bolt_v3_market_families/updown.rs`) | all 288 cycles/day/asset; 8,064 cycles, 100% resolved |

Assets: btc, eth, sol, xrp — the four 5-minute up/down families with leader coverage.

**Leader substitution (stated plainly).** The brief says "OKX mid moves". OKX L2 books in
the lake cover 2026-03-01..03-11 only — zero overlap with any Polymarket book coverage
(2026-03-20 onward) — so no OKX *mid* series exists for any measurable window. The leader
here is the Hyperliquid perp book mid (a true top-of-book mid at 0.54 s cadence). The
premise under test is "spot venues lead Polymarket"; HL is a valid fast-venue
representative. OKX *trades* (which do cover the window) could serve as a robustness
leader in a follow-up; a trade-print leader is noisier than a mid and was kept out of
scope.

**Event study definitions.**
- Event: 1-second leader mid return |r| ≥ X bps, X ∈ {5, 10, 20}; events closer than
  120 s to the previous kept event are dropped, so response windows never overlap.
- The event maps to the asset's live 5-minute cycle (clock-aligned `t // 300 × 300`);
  events in the first 10 s of a cycle are dropped.
- Side: an up-move buys the Up token, a down-move buys the Down token (each token has its
  own book; no mirroring assumed).
- Entries per event at move-completion second t: **pre-move** = ask at t−1;
  **executable** = ask at t.
- Net taker edge at horizon h = token mid(t+h) − entry ask − fee(entry ask), in cents per
  share of $1 payout. Horizons h ∈ {1, 2, 5, 10, 30, 60} s, evaluated only when the
  cycle has ≥ h+2 s of life left.
- Quotes older than 10 s, or one-sided/degenerate books (bid ≤ 0, ask ≥ 1, ask ≤ bid),
  are treated as no observable book.

**Fee model.** The production fee path (`src/bolt_v3_providers/polymarket/fees.rs` →
NautilusTrader `compute_commission`) is `fee = rate × p × (1−p)` dollars per share,
taker side only; `rate` comes from the market's fee schedule. Gamma reports
`takerBaseFee = 1000` bps (rate 0.10) on every one of the 8,064 cycles, and the
venue-stamped `fee_rate_bps` on executed trades confirms it (section 4).

**Caveats.**
- **Fillability is assumed, not proven.** Edges are measured against the last-observed
  top-of-book ask; top-of-book *size* was not extracted, and in fast markets the quote
  can be gone before an IOC arrives. The GO number is an upper bound on what a real
  taker captures at 1 s reaction.
- **Clock skew.** PM events carry the pmxt collector receive time; the leader carries
  Hyperliquid's exchange-published time. Skew between the two is unmeasured. A PM clock
  that *leads* the exchange clock would inflate the executable edge. The ETH pattern
  (full repricing within 1 s of the leader move) bounds plausible skew to roughly ±1 s.
- The pmxt `best_bid`/`best_ask` are venue-provided on every `price_change` event (100%
  populated in the window) — no book reconstruction was performed.
- Settlement outcomes come from Gamma `outcomePrices` on closed markets — the venue's own
  resolution record, which resolves off the same Chainlink stream the live strategy uses.

## 2. Data coverage

| date | pm hours | tob rows | trade rows | leader snapshots |
|---|---|---|---|---|
| 2026-04-22 | 24 | 2,589,464 | 1,474,289 | 619,504 |
| 2026-04-23 | 24 | 2,505,542 | 1,378,040 | 618,116 |
| 2026-04-24 | 24 | 2,183,869 | 1,237,125 | 621,821 |
| 2026-04-25 | 24 | 1,978,803 | 1,028,867 | 617,377 |
| 2026-04-26 | 24 | 2,043,257 | 1,009,031 | 624,417 |
| 2026-04-27 | 24 | 2,654,670 | 1,242,611 | 621,546 |
| 2026-04-28 | 24 | 2,444,106 | 967,993 | 621,599 |

7 days × 24 h on both legs; 16.4 M top-of-book changes, 8.3 M trades across the up/down
tokens; ~155 k leader snapshots per coin-day (~0.54 s cadence, max gap 0.7 s). "tob rows"
counts top-of-book *changes* (consecutive duplicates dropped at extraction).

## 3. Spread reality (measurement 1)

Top-of-book spread of the Up token, sampled every 5 s inside each cycle, bucketed by
time-to-expiry. Cents per share and % of mid; "two-sided" = share of sample points with a
usable two-sided book.

| asset | TTE bucket | samples | median (c) | p25 (c) | p75 (c) | median %mid | two-sided |
|---|---|---|---|---|---|---|---|
| btc | 240-300s | 23,455 | 1.00 | 1.00 | 1.00 | 2.2% | 97% |
| btc | 180-240s | 23,536 | 1.00 | 1.00 | 1.00 | 2.3% | 97% |
| btc | 120-180s | 23,342 | 1.00 | 1.00 | 1.00 | 2.4% | 96% |
| btc | 60-120s | 22,582 | 1.00 | 1.00 | 1.00 | 2.6% | 93% |
| btc | 0-60s | 16,329 | 1.00 | 1.00 | 1.00 | 3.1% | 74% |
| eth | 240-300s | 22,224 | 1.00 | 1.00 | 1.00 | 2.2% | 92% |
| eth | 180-240s | 21,922 | 1.00 | 1.00 | 1.00 | 2.3% | 91% |
| eth | 120-180s | 21,602 | 1.00 | 1.00 | 1.00 | 2.4% | 89% |
| eth | 60-120s | 20,368 | 1.00 | 1.00 | 1.00 | 2.5% | 84% |
| eth | 0-60s | 14,839 | 1.00 | 1.00 | 2.00 | 3.2% | 67% |
| sol | 240-300s | 22,304 | 1.00 | 1.00 | 2.00 | 2.3% | 92% |
| sol | 180-240s | 21,747 | 1.00 | 1.00 | 2.00 | 2.5% | 90% |
| sol | 120-180s | 21,815 | 1.00 | 1.00 | 2.00 | 2.7% | 90% |
| sol | 60-120s | 21,269 | 1.00 | 1.00 | 2.00 | 2.9% | 88% |
| sol | 0-60s | 15,651 | 1.00 | 1.00 | 2.00 | 3.3% | 71% |
| xrp | 240-300s | 22,467 | 2.00 | 1.00 | 3.00 | 4.3% | 93% |
| xrp | 180-240s | 22,537 | 2.00 | 1.00 | 4.00 | 5.4% | 93% |
| xrp | 120-180s | 22,081 | 3.00 | 1.00 | 4.00 | 6.5% | 91% |
| xrp | 60-120s | 20,735 | 3.00 | 1.00 | 5.00 | 7.7% | 86% |
| xrp | 0-60s | 15,112 | 3.00 | 1.00 | 9.00 | 18.2% | 68% |

BTC/ETH/SOL quote at the 1-cent tick essentially all session; XRP is structurally wider
(2–3 c, exploding to 9 c p75 in the last minute). Books thin out near expiry everywhere
(two-sided availability drops to ~67–74% in the 0–60 s bucket).

## 4. Fee reality (measurement 2)

Observed venue-stamped `fee_rate_bps` on executed trades in the window (not assumptions):

| asset | observed fee_rate_bps | trades | share |
|---|---|---|---|
| btc | 0 | 384,480 | 5.9% |
| btc | 1000 | 6,138,193 | 94.1% |
| eth | 0 | 48,997 | 4.5% |
| eth | 1000 | 1,032,651 | 95.5% |
| sol | 0 | 21,809 | 4.6% |
| sol | 1000 | 447,380 | 95.4% |
| xrp | 0 | 13,464 | 5.1% |
| xrp | 1000 | 250,982 | 94.9% |

Gamma `takerBaseFee` across all 8,064 cycles: 0.10 (1000 bps), `feesEnabled` everywhere.
~95% of trade events carry the 1000 bps stamp (the ~5% zero-stamp events were taken as
observed; the conservative 1000 bps rate is what the edge numbers charge). In price terms
under the production formula: fee(p=0.5) = 2.5 c/share, fee(p=0.3) = 2.1 c,
fee(p=0.1) = 0.9 c.

The brief's preferred source — `up_fee_bps`/`down_fee_bps` from the live node's
entry-evaluation log and decision-evidence JSONL — lives on the production host, which
this session could not reach (access denied). The venue-stamped rates above are the same
quantity observed from the venue side.

## 5. Lead-lag event study (measurement 3)

Events found (after de-overlap, with an observable pre-move book):

| asset | X (bps) | events evaluated |
|---|---|---|
| btc | 5 | 171 |
| btc | 10 | 17 |
| btc | 20 | 2 |
| eth | 5 | 295 |
| eth | 10 | 66 |
| eth | 20 | 4 |
| sol | 5 | 274 |
| sol | 10 | 62 |
| sol | 20 | 4 |
| xrp | 5 | 232 |
| xrp | 10 | 11 |
| xrp | 20 | 0 |

X=20 bps cells are too thin to read (≤4 events); they are reported for completeness only.

| asset | X (bps) | h (s) | n | mean response (c) | net pre-move (c) | pre-move 95% CI | net executable (c) | executable 95% CI |
|---|---|---|---|---|---|---|---|---|
| btc | 5 | 1 | 164 | +10.32 | +7.48 | [+5.69, +9.27] | -0.01 | [-1.43, +1.41] |
| btc | 5 | 2 | 157 | +12.39 | +9.74 | [+7.87, +11.62] | +3.22 | [+1.81, +4.62] |
| btc | 5 | 5 | 152 | +16.20 | +13.53 | [+11.45, +15.61] | +6.95 | [+4.89, +9.01] |
| btc | 5 | 10 | 150 | +17.48 | +14.79 | [+12.50, +17.07] | +8.22 | [+5.92, +10.52] |
| btc | 5 | 30 | 134 | +19.75 | +16.98 | [+14.22, +19.73] | +10.57 | [+7.49, +13.65] |
| btc | 5 | 60 | 123 | +17.93 | +15.14 | [+11.62, +18.66] | +8.47 | [+4.71, +12.22] |
| btc | 10 | 1 | 17 | +12.12 | +9.65 | [+2.73, +16.57] | +2.99 | [-0.43, +6.41] |
| btc | 10 | 2 | 16 | +18.47 | +15.94 | [+5.00, +26.88] | +8.93 | [+0.62, +17.23] |
| btc | 10 | 5 | 15 | +21.57 | +19.30 | [+7.85, +30.76] | +14.68 | [+4.62, +24.73] |
| btc | 10 | 10 | 15 | +25.23 | +22.97 | [+12.22, +33.72] | +18.34 | [+8.75, +27.93] |
| btc | 10 | 30 | 11 | +27.45 | +24.99 | [+10.24, +39.74] | +19.02 | [+4.96, +33.07] |
| btc | 10 | 60 | 11 | +28.27 | +25.74 | [+8.78, +42.69] | +20.64 | [+4.60, +36.68] |
| btc | 20 | 1 | 2 | +9.75 | +6.71 | [-0.86, +14.29] | +4.89 | [-6.25, +16.04] |
| btc | 20 | 2 | 2 | +11.00 | +7.96 | [-1.08, +17.01] | +6.15 | [-6.47, +18.76] |
| btc | 20 | 5 | 2 | +16.00 | +12.96 | [-2.94, +28.87] | +11.14 | [-8.33, +30.62] |
| btc | 20 | 10 | 2 | +14.75 | +11.72 | [-3.70, +27.13] | +9.90 | [-9.09, +28.88] |
| btc | 20 | 30 | 2 | +17.00 | +13.96 | [-6.84, +34.77] | +12.15 | [-12.23, +36.52] |
| btc | 20 | 60 | 2 | +18.50 | +15.46 | [-7.30, +38.23] | +13.65 | [-12.69, +39.98] |
| eth | 5 | 1 | 292 | +13.27 | +10.45 | [+8.85, +12.06] | +0.02 | [-0.84, +0.88] |
| eth | 5 | 2 | 283 | +13.63 | +10.80 | [+9.19, +12.41] | +0.37 | [-0.67, +1.40] |
| eth | 5 | 5 | 280 | +13.62 | +10.79 | [+9.05, +12.53] | +0.32 | [-0.95, +1.60] |
| eth | 5 | 10 | 269 | +13.71 | +10.87 | [+9.01, +12.73] | +0.69 | [-0.79, +2.16] |
| eth | 5 | 30 | 227 | +13.25 | +10.29 | [+7.82, +12.77] | +0.03 | [-2.22, +2.29] |
| eth | 5 | 60 | 204 | +12.56 | +9.55 | [+6.52, +12.58] | -0.77 | [-3.61, +2.08] |
| eth | 10 | 1 | 66 | +14.47 | +11.41 | [+8.32, +14.50] | +0.09 | [-1.80, +1.97] |
| eth | 10 | 2 | 62 | +16.60 | +13.56 | [+9.97, +17.15] | +1.80 | [-0.73, +4.33] |
| eth | 10 | 5 | 59 | +16.09 | +13.00 | [+9.41, +16.60] | +0.83 | [-1.68, +3.35] |
| eth | 10 | 10 | 57 | +18.70 | +15.49 | [+11.66, +19.32] | +2.95 | [-0.12, +6.02] |
| eth | 10 | 30 | 48 | +19.83 | +16.42 | [+11.46, +21.37] | +2.54 | [-1.57, +6.66] |
| eth | 10 | 60 | 41 | +19.50 | +16.01 | [+9.38, +22.63] | +1.97 | [-3.63, +7.58] |
| eth | 20 | 1 | 4 | +6.00 | +3.27 | [-0.41, +6.94] | -1.16 | [-3.73, +1.42] |
| eth | 20 | 2 | 4 | +18.00 | +15.27 | [-10.94, +41.48] | +10.84 | [-11.59, +33.28] |
| eth | 20 | 5 | 2 | +8.00 | +5.25 | [-3.65, +14.14] | +0.12 | [-0.50, +0.75] |
| eth | 20 | 10 | 3 | +25.67 | +22.22 | [-14.15, +58.59] | +15.42 | [-17.53, +48.36] |
| eth | 20 | 30 | 2 | -4.25 | -7.00 | [-19.18, +5.17] | -12.13 | [-32.57, +8.32] |
| eth | 20 | 60 | 2 | +1.25 | -1.50 | [-4.86, +1.85] | -6.63 | [-18.25, +5.00] |
| sol | 5 | 1 | 272 | +14.72 | +11.03 | [+9.01, +13.05] | -1.91 | [-2.69, -1.14] |
| sol | 5 | 2 | 270 | +14.67 | +10.97 | [+8.91, +13.03] | -2.03 | [-2.98, -1.09] |
| sol | 5 | 5 | 266 | +14.11 | +10.42 | [+8.31, +12.52] | -2.76 | [-3.95, -1.56] |
| sol | 5 | 10 | 255 | +14.91 | +11.11 | [+8.86, +13.36] | -2.69 | [-4.00, -1.37] |
| sol | 5 | 30 | 222 | +14.80 | +11.09 | [+8.29, +13.90] | -2.21 | [-4.22, -0.21] |
| sol | 5 | 60 | 196 | +13.92 | +10.06 | [+6.43, +13.69] | -3.15 | [-6.04, -0.26] |
| sol | 10 | 1 | 61 | +19.62 | +15.29 | [+9.48, +21.10] | -0.82 | [-2.25, +0.60] |
| sol | 10 | 2 | 61 | +19.45 | +15.12 | [+9.46, +20.77] | -1.00 | [-2.14, +0.15] |
| sol | 10 | 5 | 59 | +19.58 | +15.13 | [+9.29, +20.96] | -1.54 | [-3.20, +0.13] |
| sol | 10 | 10 | 55 | +20.95 | +16.22 | [+9.91, +22.53] | -2.03 | [-4.14, +0.07] |
| sol | 10 | 30 | 48 | +18.78 | +13.77 | [+7.32, +20.22] | -1.36 | [-4.86, +2.13] |
| sol | 10 | 60 | 42 | +23.95 | +18.41 | [+11.22, +25.60] | +1.42 | [-3.29, +6.13] |
| sol | 20 | 1 | 4 | +11.87 | +4.99 | [-1.07, +11.05] | -0.01 | [-2.70, +2.69] |
| sol | 20 | 2 | 4 | +12.12 | +5.24 | [+0.70, +9.78] | +0.24 | [-4.31, +4.80] |
| sol | 20 | 5 | 4 | +9.75 | +2.87 | [-6.71, +12.44] | -2.13 | [-13.21, +8.94] |
| sol | 20 | 10 | 4 | +7.75 | +0.87 | [-9.98, +11.71] | -4.13 | [-16.87, +8.60] |
| sol | 20 | 30 | 3 | +9.67 | +1.40 | [-14.58, +17.38] | -4.96 | [-24.01, +14.09] |
| sol | 20 | 60 | 3 | +12.17 | +3.90 | [-4.98, +12.77] | -2.46 | [-17.67, +12.75] |
| xrp | 5 | 1 | 232 | +15.16 | +10.70 | [+8.58, +12.82] | -2.43 | [-3.30, -1.57] |
| xrp | 5 | 2 | 230 | +15.39 | +10.92 | [+8.74, +13.10] | -2.12 | [-3.12, -1.12] |
| xrp | 5 | 5 | 224 | +15.09 | +10.68 | [+8.40, +12.96] | -2.55 | [-3.67, -1.44] |
| xrp | 5 | 10 | 216 | +15.19 | +10.91 | [+8.40, +13.43] | -2.55 | [-4.03, -1.07] |
| xrp | 5 | 30 | 188 | +16.01 | +11.85 | [+8.66, +15.04] | -2.22 | [-4.33, -0.10] |
| xrp | 5 | 60 | 171 | +14.42 | +10.27 | [+6.70, +13.84] | -2.48 | [-5.47, +0.50] |
| xrp | 10 | 1 | 11 | +15.91 | +11.05 | [+0.56, +21.55] | -2.52 | [-7.03, +1.99] |
| xrp | 10 | 2 | 11 | +15.91 | +11.05 | [+0.00, +22.11] | -2.52 | [-8.09, +3.05] |
| xrp | 10 | 5 | 9 | +16.89 | +12.00 | [+0.24, +23.76] | -3.98 | [-11.88, +3.92] |
| xrp | 10 | 10 | 8 | +17.06 | +11.76 | [-1.39, +24.92] | -6.22 | [-15.70, +3.27] |
| xrp | 10 | 30 | 6 | +20.42 | +16.98 | [+3.10, +30.86] | -0.39 | [-8.89, +8.10] |
| xrp | 10 | 60 | 5 | +13.30 | +7.37 | [-15.54, +30.29] | -12.90 | [-33.16, +7.35] |

Structure worth noting: ETH/SOL/XRP responses are essentially **complete at h=1 s** (the
h=1 response ≈ the h=30 response) — those books reprice within a second, so everything
after the move is already in the price. **BTC is the outlier: its response keeps growing
from +10 c at 1 s to +20 c at 30 s**, which is why its executable edge is large and
positive while the others' is ~0.

TTE breakdown at each asset's best cell (net pre-move):

**btc at X=5bps, h=30s:**

| TTE bucket | n | mean net (c) | 95% CI |
|---|---|---|---|
| 240-300s | 39 | +16.19 | [+10.45, +21.93] |
| 180-240s | 44 | +16.90 | [+13.06, +20.75] |
| 120-180s | 32 | +19.61 | [+14.64, +24.59] |
| 60-120s | 18 | +15.45 | [+5.47, +25.43] |
| 0-60s | 1 | -6.10 | - |

**eth at X=10bps, h=30s:**

| TTE bucket | n | mean net (c) | 95% CI |
|---|---|---|---|
| 240-300s | 14 | +15.34 | [+7.69, +22.99] |
| 180-240s | 19 | +15.48 | [+5.95, +25.00] |
| 120-180s | 8 | +15.04 | [+4.11, +25.97] |
| 60-120s | 7 | +22.70 | [+10.94, +34.46] |
| 0-60s | 0 | - | - |

**sol at X=10bps, h=60s:**

| TTE bucket | n | mean net (c) | 95% CI |
|---|---|---|---|
| 240-300s | 9 | +14.78 | [+3.49, +26.07] |
| 180-240s | 15 | +11.88 | [-0.70, +24.45] |
| 120-180s | 14 | +25.41 | [+13.46, +37.36] |
| 60-120s | 4 | +26.57 | [-7.74, +60.88] |
| 0-60s | 0 | - | - |

**xrp at X=5bps, h=30s:**

| TTE bucket | n | mean net (c) | 95% CI |
|---|---|---|---|
| 240-300s | 44 | +9.19 | [+4.25, +14.13] |
| 180-240s | 65 | +7.53 | [+3.28, +11.78] |
| 120-180s | 45 | +14.66 | [+8.85, +20.48] |
| 60-120s | 29 | +18.93 | [+8.75, +29.10] |
| 0-60s | 5 | +25.15 | [-33.83, +84.14] |

The edge is broadly flat-to-rising as expiry approaches (binary delta grows near expiry),
with the thin 0–60 s bucket unreliable (books are pulled — section 3).

## 6. Model calibration (measurement 4)

The brief asks for logged `fair_probability_up` vs realized settlement. That log lives in
the live node's decision evidence on the production host (unreachable this session), and
no decision-evidence stream exists in S3 for the measured window. **The model-calibration
question is therefore a documented gap** (follow-up in section 9). What follows is the
*market-implied* calibration — Polymarket Up-token mid vs realized outcome — which tests
whether the market itself prices honestly and is the benchmark the BS-digital + RV
pipeline must beat:

| asset | TTE probe (s) | n | Brier |
|---|---|---|---|
| btc | 120 | 1,926 | 0.1640 |
| btc | 60 | 1,783 | 0.1243 |
| btc | 30 | 1,553 | 0.1096 |
| eth | 120 | 1,730 | 0.1587 |
| eth | 60 | 1,608 | 0.1278 |
| eth | 30 | 1,389 | 0.1025 |
| sol | 120 | 1,763 | 0.1600 |
| sol | 60 | 1,665 | 0.1223 |
| sol | 30 | 1,478 | 0.0997 |
| xrp | 120 | 1,807 | 0.1559 |
| xrp | 60 | 1,611 | 0.1229 |
| xrp | 30 | 1,390 | 0.0957 |

Reliability (pooled across assets and probes, 10 buckets):

| p(up) bucket | n | mean p | realized freq |
|---|---|---|---|
| 0.0-0.1 | 3,424 | 0.048 | 0.042 |
| 0.1-0.2 | 1,974 | 0.146 | 0.127 |
| 0.2-0.3 | 1,542 | 0.248 | 0.238 |
| 0.3-0.4 | 1,374 | 0.348 | 0.336 |
| 0.4-0.5 | 1,446 | 0.448 | 0.445 |
| 0.5-0.6 | 1,343 | 0.550 | 0.565 |
| 0.6-0.7 | 1,445 | 0.650 | 0.704 |
| 0.7-0.8 | 1,623 | 0.751 | 0.776 |
| 0.8-0.9 | 2,030 | 0.854 | 0.865 |
| 0.9-1.0 | 3,502 | 0.951 | 0.960 |

The market is well calibrated with a mild favorite-longshot tilt (realized frequencies
slightly more extreme than prices in the 0.6–1.0 buckets). Nothing here contradicts the
event-study result: prices are right *eventually*; the edge is in the seconds in between.

## 7. Verdict (measurement 5)

Per-asset verdict table: see section 0 (it is the deliverable headline).

Maker counterfactual — passive-fill mark-outs (maker is hit at the trade price, marked at
the book mid h seconds later; positive = spread capture survives adverse selection).
8,337,956 fills in window; mark-outs computed on a deterministic every-Nth sample of
397,046:

| asset | mark-out h (s) | fills | mean pnl (c) | 95% CI (c) | median (c) |
|---|---|---|---|---|---|
| btc | 10 | 274,527 | -0.034 | [-0.072, +0.003] | -0.500 |
| btc | 30 | 248,104 | -0.013 | [-0.073, +0.047] | -0.500 |
| btc | 60 | 215,170 | +0.141 | [+0.057, +0.226] | -0.500 |
| eth | 10 | 43,014 | -0.382 | [-0.481, -0.282] | -0.500 |
| eth | 30 | 37,393 | -0.272 | [-0.440, -0.103] | -0.500 |
| eth | 60 | 32,350 | -0.222 | [-0.453, +0.009] | -1.000 |
| sol | 10 | 17,860 | -0.233 | [-0.389, -0.077] | -0.500 |
| sol | 30 | 14,890 | -0.322 | [-0.575, -0.069] | -0.500 |
| sol | 60 | 12,375 | -0.120 | [-0.474, +0.234] | -1.500 |
| xrp | 10 | 9,540 | +0.058 | [-0.145, +0.260] | +0.000 |
| xrp | 30 | 7,906 | -0.376 | [-0.718, -0.033] | -0.500 |
| xrp | 60 | 6,778 | -0.414 | [-0.908, +0.081] | -0.500 |

For the NO-GO assets the brief asks whether observed spreads would compensate a maker
instead: **no** — at-the-touch passive fills mark out at roughly −0.4 to +0.1 c/share
mean (median −0.5 c) before any rebate, i.e. adverse selection from exactly the flow
measured in section 5 slightly exceeds the captured half-spread. A maker would need
selective quoting (e.g. pulling quotes on leader moves — the same signal, used
defensively) rather than naive at-touch quoting.

## 8. Reproduction

```
uv run scripts/leadlag_session4.py resolve        --dates 2026-04-22:2026-04-28
uv run scripts/leadlag_session4.py extract-leader --dates 2026-04-22:2026-04-28
uv run scripts/leadlag_session4.py extract-pm     --dates 2026-04-22:2026-04-28
uv run scripts/leadlag_session4.py analyze        --dates 2026-04-22:2026-04-28 \
    --report /tmp/leadlag_tables.md
```

Requires `uv` and an `aws` CLI with read access to `s3://bolt-parquet`. The extract stage
downloads ~67 GB (168 hourly objects, processed and discarded one at a time); all stages
cache under `~/.cache/bolt-leadlag-session4/` and are resumable. The report's tables are
the verbatim `analyze` output sections.

## 9. Gaps and follow-ups

1. **Model calibration against logged `fair_probability_up`** — requires live-host access
   (decision-evidence JSONL); re-run section 6 against the model when a session has host
   access.
2. **Live-logged fee confirmation** (`up_fee_bps`/`down_fee_bps` in the entry-evaluation
   log) — same host gap; venue-side fee evidence (section 4) stands in.
3. **Fill realism for the BTC GO** — measure top-of-book size at signal time and/or run a
   live shadow comparing quoted-ask-at-signal vs achieved fills before sizing capital.
4. **OKX-trade-print robustness leader** — OKX spot trades cover 2026-03→05 in the lake
   if a second, independent leader is wanted.
5. **Live capture catalog** — the brief's original target (Feather spool on the host) was
   not inventoried this session for the same access reason; the S3 archive covered the
   measurement instead.
