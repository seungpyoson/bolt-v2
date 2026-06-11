# Lead-lag robustness: trades-based leader clock (#631)

**Date:** 2026-06-11 · **Follow-up to:** `leadlag-taker-edge-2026-06-10.md` (#617, PR #624)
and `leadlag-subsecond-fillability-2026-06-10.md` (#626, PR #627) · **Tool:**
`scripts/leadlag_trades_leader.py`

## 1. Summary — the operator's challenge was right, and it changes two verdicts

The baseline study clocked spot moves off Hyperliquid book mids (~0.54s snapshots) — the
only book-level leader overlapping the Polymarket archive. The operator challenged this:
faster venues (OKX/Binance/Bybit) may move first, so the HL clock starts late and the
alt verdicts could be artifacts of a delayed starting gun.

Re-running the event study with **Bybit spot tick trades (millisecond timestamps)** as
the leader clock, on the 4-day overlap where both clocks have data (2026-04-22..25),
with the HL baseline recomputed on the same window:

| asset | HL-clock verdict (#626) | Bybit-clock verdict (this run) |
|---|---|---|
| btc | GO at any latency ≤2s | **GO, edge larger**: +18.7c @0.1s, +13.3c @1s, +7.4c @2s; survives settlement marking (§4.1) |
| eth | NO-GO at any latency (+0.7c NS @0.1s) | **FLIPS: conditional GO ≤0.25s firm, 0.5s suggestive** (+9.9c @0.1s; settlement-marked CI>0 at ≤0.25s, point-positive NS at 0.5s) |
| sol | NO-GO (−1.8c @0.1s) | mid-mark suggests ≤0.25s edge (+6.0c @0.1s) but **not confirmed under settlement marking** (§4.1) — treat as NO-GO |
| xrp | NO-GO (−3.2c @0.1s) | still NO-GO (+3.5c @0.1s barely clears zero on mid-mark; NS under settlement marking) |

All values: mean net edge in cents/share, X=5bps events, entry at best ask at
detection+δ, mark = book mid at detection+30s, taker fee (archive-stamped 1000bps)
included. The HL clock fires a median **1 second later** than the Bybit clock on the
same moves (80–93% of matched events: Bybit first). One second is exactly the regime
where thin-book asks get pulled — which is why the late clock made eth/sol look
untouchable.

What does NOT change: the cross-asset structure survives the better clock. btc's edge
is bigger, lasts seconds, and decays slowly; alt edges exist only in the first few
hundred milliseconds after a properly-clocked signal. The depth/inertia mechanism from
#626 §1.1 stands.

## 2. Data

- **Leader:** Bybit spot tick trades, per-symbol daily csv.gz in the lake
  (`bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/`), millisecond
  timestamps, BTCUSDT/ETHUSDT/SOLUSDT/XRPUSDT. Lake coverage for these symbols ends
  2026-04-25 (dt=2026-04-26..28 partitions contain only BNBUSDC), so the comparison
  window is **2026-04-22..25** — 4 of the baseline's 7 days. 24.8k–242k price changes
  per coin-day after collapsing runs of identical prints.
- **Baseline re-run:** `leadlag_subsecond.py subsecond --dates 2026-04-22:2026-04-25`
  so both tables below cover identical days (the published #626 table is the 7-day
  window; its 4-day restriction is reproduced in §5).
- **Polymarket side:** unchanged — same pmxt top-of-book extracts, same Gamma cycle
  identities, same fee formula, same event de-overlap (120s) and cycle guards.
- OKX remains impossible for this window (no April data in the lake); Binance spot
  trades exist only as ~656MB monthly zips per symbol and would duplicate, not improve,
  this check.

## 3. Lead-time: how late was the Hyperliquid clock?

Events detected independently on each clock (same 1s-grid LOCF detector, X bps 1s
move); a Bybit event matches the nearest same-direction HL event within ±15s.

| asset | X (bps) | bybit events | hl events | matched | median HL-minus-Bybit | IQR | % Bybit first |
|---|---|---|---|---|---|---|---|
| btc | 5 | 208 | 120 | 41% | +1s | [+1, +1]s | 93% |
| btc | 10 | 26 | 12 | 35% | +1s | [+1, +1]s | 100% |
| eth | 5 | 375 | 244 | 43% | +1s | [+1, +1]s | 90% |
| eth | 10 | 82 | 50 | 50% | +1s | [+1, +1]s | 100% |
| sol | 5 | 302 | 217 | 43% | +1s | [+1, +1]s | 90% |
| sol | 10 | 70 | 41 | 40% | +1s | [+1, +1]s | 93% |
| xrp | 5 | 163 | 184 | 44% | +1s | [+1, +1]s | 80% |
| xrp | 10 | 27 | 10 | 30% | +1s | [+1, +1]s | 88% |

Offsets are quantized to 1s by the detector grid; "+1s median, 80–100% Bybit first" is
the resolution-limited statement of "HL lags by roughly one second." The Bybit clock
also *finds 1.5–2× more events*: many moves never register as a 1s/5bps move on HL's
0.54s snapshot mid at all. The unmatched share (~57–70%) is mostly these
threshold-straddling moves, not clock disagreement.

Attribution caveat: part of the +1s offset is HL's own 0.54s snapshot cadence
interacting with the 1s detection grid, not pure venue speed — a move landing between
snapshots is seen one grid-second late even if HL's *book* moved instantly. The two
causes are not separated here, and they don't need to be: either way, a snapshot-mid
clock is structurally late, which is exactly the operational point (§6). The edge
tables below depend only on each clock's own event times, not on this attribution.

## 4. Net edge under the Bybit trades clock (2026-04-22..25)

| asset | X (bps) | entry at detection | n (mark 30s) | mean net (c) | 95% CI |
|---|---|---|---|---|---|
| btc | 5 | pre-move (t-1s) | 127 | +19.83 | [+16.49, +23.17] |
| btc | 5 | +0.1s | 127 | +18.69 | [+15.35, +22.03] |
| btc | 5 | +0.25s | 127 | +18.20 | [+14.80, +21.60] |
| btc | 5 | +0.5s | 127 | +16.25 | [+12.53, +19.96] |
| btc | 5 | +0.75s | 127 | +14.39 | [+10.82, +17.95] |
| btc | 5 | +1s | 127 | +13.27 | [+9.68, +16.87] |
| btc | 5 | +2s | 127 | +7.36 | [+3.54, +11.19] |
| btc | 5 | +5s | 127 | +1.65 | [-1.50, +4.80] |
| btc | 5 | +10s | 124 | +0.25 | [-2.11, +2.60] |
| btc | 10 | pre-move (t-1s) | 15 | +27.03 | [+19.31, +34.76] |
| btc | 10 | +0.1s | 15 | +26.54 | [+18.49, +34.58] |
| btc | 10 | +1s | 15 | +19.70 | [+12.89, +26.50] |
| btc | 10 | +2s | 15 | +15.01 | [+8.43, +21.60] |
| eth | 5 | pre-move (t-1s) | 231 | +17.06 | [+14.59, +19.52] |
| eth | 5 | +0.1s | 230 | +9.85 | [+7.64, +12.06] |
| eth | 5 | +0.25s | 229 | +8.35 | [+6.10, +10.59] |
| eth | 5 | +0.5s | 230 | +5.15 | [+2.87, +7.44] |
| eth | 5 | +0.75s | 230 | +2.89 | [+0.70, +5.09] |
| eth | 5 | +1s | 230 | +1.49 | [-0.72, +3.71] |
| eth | 5 | +2s | 229 | -0.33 | [-2.38, +1.72] |
| eth | 10 | +0.1s | 44 | +16.69 | [+11.29, +22.10] |
| eth | 10 | +0.5s | 43 | +12.11 | [+6.58, +17.65] |
| eth | 10 | +1s | 43 | +6.60 | [+1.27, +11.94] |
| eth | 10 | +2s | 43 | +0.60 | [-3.93, +5.13] |
| sol | 5 | +0.1s | 196 | +6.02 | [+3.17, +8.88] |
| sol | 5 | +0.25s | 196 | +3.35 | [+0.55, +6.14] |
| sol | 5 | +0.5s | 196 | -0.10 | [-2.59, +2.38] |
| sol | 5 | +1s | 196 | -2.10 | [-4.36, +0.16] |
| sol | 10 | +0.1s | 42 | +14.14 | [+6.90, +21.37] |
| sol | 10 | +0.25s | 42 | +12.02 | [+4.70, +19.34] |
| sol | 10 | +0.5s | 42 | +5.72 | [-0.40, +11.85] |
| xrp | 5 | +0.1s | 113 | +3.47 | [+0.34, +6.61] |
| xrp | 5 | +0.25s | 114 | +1.92 | [-1.00, +4.83] |
| xrp | 5 | +0.5s | 114 | -0.99 | [-3.75, +1.77] |
| xrp | 10 | +0.1s | 11 | +11.52 | [-0.00, +23.04] |
| xrp | 10 | +0.25s | 12 | +9.10 | [+0.05, +18.15] |
| xrp | 10 | +0.5s | 12 | +7.39 | [+0.30, +14.48] |

(Full grid including every threshold/delta cell is emitted by the tool; rows with no
events omitted here for readability — none of the omissions change a verdict.)

### 4.1 Settlement-marked robustness (is the mid mark real money?)

The tables above mark each entry to the book **mid 30s later** — an interim value
proxy, not cash. The adversarial objection: mid embeds half-spread you may never
monetize, and an exit trade would pay spread plus a second taker fee. The live
strategy's answer is that it holds to expiry, so the honest robustness check is to
mark every event to the **venue's own settlement payout** (bought token pays $1 or
$0, no exit trade, no exit fee). Same event definitions and guards; `--mark
settlement`:

| asset | X (bps) | entry | n (mark settlement) | mean net (c) | 95% CI |
|---|---|---|---|---|---|
| btc | 5 | +0.1s | 146 | +15.89 | [+8.82, +22.96] |
| btc | 5 | +0.5s | 145 | +13.78 | [+6.55, +21.00] |
| btc | 5 | +1s | 144 | +10.88 | [+3.79, +17.98] |
| btc | 5 | +2s | 143 | +5.60 | [-1.62, +12.82] |
| eth | 5 | +0.1s | 266 | +8.34 | [+3.33, +13.35] |
| eth | 5 | +0.25s | 266 | +6.66 | [+1.63, +11.68] |
| eth | 5 | +0.5s | 266 | +3.72 | [-1.26, +8.70] |
| eth | 5 | +1s | 264 | +0.30 | [-4.77, +5.36] |
| sol | 5 | +0.1s | 228 | +3.20 | [-2.61, +9.02] |
| sol | 5 | +0.25s | 228 | +0.76 | [-4.99, +6.52] |
| xrp | 5 | +0.1s | 127 | +6.09 | [-1.08, +13.26] |
| xrp | 5 | +0.25s | 128 | +4.45 | [-2.56, +11.47] |

Binary payouts are far noisier than 30s mid marks, so CIs widen (~±7c at n≈140) —
that is expected, not a discrepancy. Settlement marking is also horizon-free, which
answers a second objection: the 30s mark horizon was chosen as the baseline study's
best-responding cell, so mid-marked levels could carry selection bias — the
settlement-marked levels cannot. Read against the §4 table:

- **btc: the GO survives marking to actual cash.** +15.9c @0.1s and +10.9c @1s with
  CIs clear of zero. The mid-mark objection is disproven for btc.
- **eth: holds at ≤0.25s (CI>0), softens at 0.5s** (+3.7c point estimate, NS). The §1
  verdict is stated accordingly: firm to 0.25s, suggestive at 0.5s.
- **sol: the mid-mark flip does NOT survive** (+3.2c NS @0.1s). Treat sol as NO-GO.
- **xrp: NS everywhere** (wide-spread entries cost what the lag pays).

## 5. Same-window HL-clock baseline (2026-04-22..25)

For comparison, the #626 measurement restricted to the identical 4 days:

| asset | X (bps) | entry | n | mean net (c) | 95% CI |
|---|---|---|---|---|---|
| btc | 5 | +0.1s | 74 | +13.90 | [+9.87, +17.94] |
| btc | 5 | +0.5s | 74 | +11.52 | [+7.57, +15.48] |
| btc | 5 | +1s | 74 | +9.56 | [+5.69, +13.43] |
| btc | 5 | +2s | 74 | +7.20 | [+3.44, +10.97] |
| eth | 5 | +0.1s | 144 | +0.69 | [-2.05, +3.43] |
| eth | 5 | +0.5s | 144 | -0.57 | [-3.23, +2.10] |
| eth | 5 | +1s | 144 | -1.30 | [-3.88, +1.27] |
| sol | 5 | +0.1s | 141 | -1.78 | [-4.23, +0.67] |
| sol | 5 | +1s | 141 | -2.19 | [-4.52, +0.14] |
| xrp | 5 | +0.1s | 111 | -3.20 | [-6.29, -0.11] |
| xrp | 5 | +1s | 111 | -4.12 | [-6.91, -1.33] |

Differences vs the Bybit-clock table are therefore clock effects, not window effects.

## 6. Updated verdicts and what they require operationally

- **btc — GO, stronger than published.** The published +10.1c@0.1s / +6.5c@1s (HL
  clock, 7 days) was understated by the late clock: +18.7c@0.1s / +13.3c@1s on the
  same-window Bybit clock. Still positive at 2s. The pilot (#630) conclusion is
  unchanged, slightly reinforced.
- **eth — conditional GO, ≤0.25s firm / 0.5s suggestive.** +9.9c@0.1s decaying to
  zero by ~1s; settlement-marked CIs clear zero at ≤0.25s and go NS at 0.5s (§4.1).
  Tradable **only if** the live signal is clocked off a fast venue feed (the deployed
  bot's reference is OKX live WS — appropriate) **and** end-to-end reaction including
  Polymarket order placement is in the 0.25–0.5s range or better. Touch size is the
  binding constraint: #626 §2 measured eth median $27 at touch (X=5) — roughly half
  btc's, so dollar capacity is small even where the per-share edge is real.
- **sol — NO-GO.** The mid-mark table suggests a ≤0.25s edge, but it is not confirmed
  under settlement marking (§4.1), and sol touch size (median $10) makes it
  near-untradable in dollars regardless.
- **xrp — NO-GO unchanged.** Only the 0.1s mid-mark cell clears zero, barely; NS
  under settlement marking; touch size is $11 median with 2–3c spreads.
- **Pilot implication:** none of this changes #630's scope (btc, supervised, tiny).
  It does add one design requirement for any later alt extension: the decision clock
  must be the fast-venue feed, never an HL-style snapshot mid.

## 7. Caveats

1. **4-day window** (Bybit lake coverage ends 2026-04-25). Same days for both clocks,
   so the *comparison* is clean, but absolute levels carry the small-window noise the
   7-day study was designed to reduce.
2. **Bybit may itself lag the true first mover.** This is a one-sided robustness
   check: a still-faster clock could shift the alt curves further up/left. It cannot
   un-flip btc (its edge is clock-insensitive at +2s).
3. **Trade prints carry bid-ask bounce.** Detection thresholds (5/10bps on a 1s grid)
   are an order of magnitude above these books' sub-bp ticks, so bounce-only triggers
   are rare; they would bias measured edge toward zero, not create it.
4. **Fillability for the flipped cells is assumed, not proven.** The #626 size probe
   used HL-clock events; alt touch sizes were small there and nothing here remeasures
   them under the earlier clock.
5. **Cross-source clock skew hits the sub-second cells hardest.** Bybit rows carry
   exchange timestamps; pmxt rows carry collector receive timestamps. A systematic
   offset of a few hundred ms would shift the entire latency grid left or right —
   immaterial for btc (positive out to 2s) but material for eth, whose firm window is
   0.25s wide. The no-anticipation pattern (Polymarket books never react *before*
   spot events) bounds skew well under 1s, but the eth conditional GO should be read
   as "edge exists in the first few hundred ms," not as a calibrated 250ms cutoff.
6. **Statistical fine print.** X=10 cells have n≤44 (btc n=15) — directional color
   only; verdicts rest on the X=5 cells (n≥113). CIs assume independent events;
   120s de-overlap removes response-window overlap but not shared intraday volatility
   regimes, so true intervals are somewhat wider than printed. Multiple testing across
   the grid: btc cells are nowhere near marginal; eth's settlement-marked 0.1s cell
   (p≈0.001) survives any reasonable correction, the 0.25s cell (p≈0.009) only
   marginally — a further reason the eth window is stated as "first few hundred ms."
7. **Reproducibility tolerance ≈0.1c.** ~60% of Polymarket book updates share a
   millisecond (batched venue messages), and within-ms row order is engine-dependent
   after extraction's non-stable sort, so as-of lookups landing on a batched
   millisecond can return different intermediate book states across engines/runs. An
   independent from-scratch re-derivation (duckdb/pandas/numpy, no pipeline code) of
   the btc X=5 +1s cells reproduced settlement n exactly (144) with mean +10.82 vs
   +10.88, and mid-mark n 126 vs 127 with mean +13.21 vs +13.27 — i.e. expect cell
   means to reproduce to ~±0.1c, not byte-exactly. No verdict cell is within that
   tolerance of a sign change. Exact within-ms determinism would require re-extracting
   with a source sequence column; not warranted at ±2–7c CI widths.
8. **Leader feed composition.** 0.14% of Bybit spot prints are RPI (retail price
   improvement) executions; their prices sit inside the normal range and cannot move
   a 5bps/1s detector — immaterial to the leader clock.
9. All baseline caveats (one historical week, archive 1000bps fee vs 700bps live)
   carry over.

## 8. Reproduction

```bash
uv run scripts/leadlag_trades_leader.py extract-leader --dates 2026-04-22:2026-04-25
uv run scripts/leadlag_trades_leader.py analyze --dates 2026-04-22:2026-04-25 \
    --report /tmp/leadlag-s4/trades_leader.md
# settlement-marked robustness (section 4.1):
uv run scripts/leadlag_trades_leader.py analyze --dates 2026-04-22:2026-04-25 \
    --mark settlement --report /tmp/leadlag-s4/trades_leader_settle.md
# same-window HL baseline:
uv run scripts/leadlag_subsecond.py subsecond --dates 2026-04-22:2026-04-25 \
    --report /tmp/leadlag-s4/subsecond_4day.md
```

Requires the #617 caches (`gamma/`, `pm_tob/`, `leader/`) in
`~/.cache/bolt-leadlag-session4/`; build them with `leadlag_session4.py resolve` /
`extract-pm` / `extract-leader` as documented in the baseline report.
