# Lead-Lag Follow-up — sub-second repricing and fill realism (issue #626)

Date: 2026-06-10. Issue: #626 (follow-up to #617 / PR #624). Declared scope: the two
questions the session-4 report (`docs/research/leadlag-taker-edge-2026-06-10.md`) left
open — sub-second repricing and fillability. Produced by `scripts/leadlag_subsecond.py`
over the same 7-day window and caches as session 4. The live shadow test (question 3 of
#626) remains blocked on production-host authorization and is NOT covered here.

## 0. Verdict summary

1. **ETH/SOL/XRP are dead at any realistic reaction latency.** Their +10–17 c/share
   pre-move edge is fully repriced within **100 ms of detection** (net at δ=0.1 s:
   eth −0.6 c, sol −2.4 c, xrp −2.6 c). Sub-second capability does not rescue them; the
   session-4 NO-GO hardens from "NO-GO at 1 s" to "NO-GO at ≥100 ms".
2. **BTC's GO is robust to latency** — the edge decays smoothly, not cliff-like:
   +10.1 c at 100 ms, +9.1 c at 250 ms, +8.0 c at 500 ms, +6.5 c at 1 s, +3.7 c at 2 s
   (all 95% CIs above zero at X=5 bps).
3. **But BTC touch-size is small.** The displayed size at the best ask 0.25 s after a
   signal is median **~105 shares (~$60 notional)**; only 40% of events show ≥$100 and
   8% show ≥$1000 at the touch. At median touch size the edge is worth roughly
   $10/event gross (~134 events/week at 5 bps). Real capacity depends on depth beyond
   the touch (unmeasured here) — with a 10–20 c response, walking 2–3 c into the book
   remains profitable in expectation but needs the live shadow test to confirm.

## 1. Sub-second repricing (net edge vs reaction latency)

Method: same events, books, and fee model as session 4 (1-second leader moves ≥X bps,
de-overlapped; entry = ask observable at t0+δ using the millisecond-stamped Polymarket
top-of-book; mark at mid(t0+30 s); net of the 1000 bps taker fee). δ is reaction latency
*after detection*; the leader feed is ~0.54 s-cadence snapshots, so detection itself can
lag the true move start by up to ~0.54 s (caveat in section 3).

| asset | X (bps) | entry at detection | n (mark 30s) | mean net (c) | 95% CI |
|---|---|---|---|---|---|
| btc | 5 | pre-move (t-1s) | 134 | +16.98 | [+14.22, +19.73] |
| btc | 5 | +0.1s | 134 | +10.13 | [+7.12, +13.14] |
| btc | 5 | +0.25s | 134 | +9.05 | [+6.11, +11.99] |
| btc | 5 | +0.5s | 134 | +7.96 | [+5.06, +10.87] |
| btc | 5 | +0.75s | 134 | +7.44 | [+4.51, +10.36] |
| btc | 5 | +1s | 134 | +6.52 | [+3.67, +9.36] |
| btc | 5 | +2s | 134 | +3.74 | [+0.93, +6.54] |
| btc | 10 | pre-move (t-1s) | 11 | +24.99 | [+10.24, +39.74] |
| btc | 10 | +0.1s | 11 | +18.93 | [+4.89, +32.97] |
| btc | 10 | +0.25s | 11 | +16.62 | [+4.13, +29.11] |
| btc | 10 | +0.5s | 11 | +14.28 | [+2.18, +26.37] |
| btc | 10 | +0.75s | 11 | +11.55 | [-1.08, +24.18] |
| btc | 10 | +1s | 11 | +9.25 | [-4.52, +23.02] |
| btc | 10 | +2s | 11 | +3.66 | [-16.80, +24.12] |
| eth | 5 | pre-move (t-1s) | 227 | +10.29 | [+7.82, +12.77] |
| eth | 5 | +0.1s | 227 | -0.58 | [-2.73, +1.57] |
| eth | 5 | +0.25s | 227 | -1.05 | [-3.15, +1.05] |
| eth | 5 | +0.5s | 227 | -2.01 | [-4.04, +0.03] |
| eth | 5 | +0.75s | 227 | -2.29 | [-4.31, -0.27] |
| eth | 5 | +1s | 227 | -2.70 | [-4.69, -0.72] |
| eth | 5 | +2s | 227 | -2.69 | [-4.55, -0.82] |
| eth | 10 | pre-move (t-1s) | 48 | +16.42 | [+11.46, +21.37] |
| eth | 10 | +0.1s | 48 | +1.76 | [-1.85, +5.37] |
| eth | 10 | +0.25s | 48 | +1.45 | [-2.06, +4.97] |
| eth | 10 | +0.5s | 48 | +0.33 | [-2.88, +3.55] |
| eth | 10 | +0.75s | 48 | +0.12 | [-3.08, +3.33] |
| eth | 10 | +1s | 48 | +0.11 | [-3.04, +3.26] |
| eth | 10 | +2s | 48 | -0.86 | [-3.98, +2.26] |
| sol | 5 | pre-move (t-1s) | 222 | +11.09 | [+8.29, +13.90] |
| sol | 5 | +0.1s | 222 | -2.40 | [-4.36, -0.43] |
| sol | 5 | +0.25s | 222 | -2.77 | [-4.73, -0.81] |
| sol | 5 | +0.5s | 222 | -2.77 | [-4.72, -0.82] |
| sol | 5 | +0.75s | 222 | -2.94 | [-4.90, -0.98] |
| sol | 5 | +1s | 222 | -3.17 | [-5.08, -1.25] |
| sol | 5 | +2s | 222 | -2.92 | [-4.79, -1.05] |
| sol | 10 | pre-move (t-1s) | 48 | +13.77 | [+7.32, +20.22] |
| sol | 10 | +0.1s | 48 | -1.71 | [-5.24, +1.82] |
| sol | 10 | +0.25s | 48 | -2.42 | [-5.72, +0.88] |
| sol | 10 | +0.5s | 48 | -2.48 | [-5.79, +0.84] |
| sol | 10 | +0.75s | 48 | -2.61 | [-5.93, +0.71] |
| sol | 10 | +1s | 48 | -2.34 | [-5.76, +1.07] |
| sol | 10 | +2s | 48 | -2.21 | [-5.41, +0.99] |
| xrp | 5 | pre-move (t-1s) | 188 | +11.85 | [+8.66, +15.04] |
| xrp | 5 | +0.1s | 188 | -2.56 | [-4.61, -0.52] |
| xrp | 5 | +0.25s | 188 | -3.41 | [-5.44, -1.37] |
| xrp | 5 | +0.5s | 188 | -3.34 | [-5.35, -1.32] |
| xrp | 5 | +0.75s | 188 | -3.34 | [-5.24, -1.43] |
| xrp | 5 | +1s | 188 | -3.58 | [-5.46, -1.70] |
| xrp | 5 | +2s | 188 | -3.68 | [-5.58, -1.78] |
| xrp | 10 | pre-move (t-1s) | 6 | +16.98 | [+3.10, +30.86] |
| xrp | 10 | +0.1s | 6 | -0.55 | [-8.93, +7.84] |
| xrp | 10 | +0.25s | 6 | -0.55 | [-8.93, +7.84] |
| xrp | 10 | +0.5s | 6 | -2.23 | [-9.71, +5.25] |
| xrp | 10 | +0.75s | 6 | -2.23 | [-9.71, +5.25] |
| xrp | 10 | +1s | 6 | -2.54 | [-9.68, +4.61] |
| xrp | 10 | +2s | 6 | -2.99 | [-9.75, +3.77] |

(n differs slightly from the session-4 study: this table requires the full 30 s mark
window for every event. X=20 bps omitted — ≤4 events in the window.)

The structural read: the eth/sol/xrp pre-move → +0.1 s collapse means those books are
repriced by participants faster than 100 ms after our detection timestamp — the +10–17 c
sits entirely inside the leader feed's blind window. BTC's gradual decay (still +3.7 c
at a 2-second reaction) is the same 30-second drift the session-4 study found, and it is
what makes BTC operationally forgiving.

## 2. Fill realism: displayed size at the best ask

Method: the top-of-book stream was re-extracted with the level-delta `price`/`size`
columns (`extract-sizes`); for each session-4 event, the size displayed at the best ask
0.25 s after detection is the last SELL-side level update at that price (120 s lookback).
This is the *displayed* touch size only — an upper bound on a single IOC's capture at
the quoted price, and a lower bound on total capacity (depth behind the touch is not
measured).

| asset | X (bps) | events | median shares | p25 shares | median notional | p25 notional | ≥$100 | ≥$1000 |
|---|---|---|---|---|---|---|---|---|
| btc | 5 | 178 | 105 | 33 | $60 | $15 | 40% | 8% |
| btc | 10 | 17 | 107 | 29 | $73 | $17 | 35% | 6% |
| eth | 5 | 307 | 43 | 17 | $27 | $8 | 21% | 3% |
| eth | 10 | 67 | 122 | 37 | $89 | $28 | 46% | 12% |
| sol | 5 | 298 | 17 | 5 | $10 | $4 | 15% | 1% |
| sol | 10 | 68 | 16 | 5 | $14 | $4 | 19% | 1% |
| xrp | 5 | 258 | 15 | 5 | $11 | $3 | 12% | 1% |
| xrp | 10 | 10 | 100 | 44 | $95 | $39 | 40% | 0% |

Economic read for the btc GO: median touch capture ≈ 105 shares × ~10 c ≈ **$10/event
gross**, ~134 5 bps events per week → order of $1–2 k/week gross *at the touch only*.
Scaling beyond that requires walking the book: with a +10–20 c expected response,
crossing 2–3 c of depth stays positive in expectation, but per-level depth and the race
for it are exactly what the live shadow test must measure before sizing.

## 3. Caveats

- **Leader cadence bounds the latency claim.** Detection rides ~0.54 s-cadence
  Hyperliquid snapshots; δ is measured from detection, not from the true move start. A
  millisecond leader feed (e.g. OKX trade prints, which the lake holds for Mar–May)
  could expose whether an eth/sol edge exists in the first ~100 ms — a different study.
- **Displayed ≠ achievable.** The 0.25 s touch size is what was *quoted*; an IOC racing
  other takers may capture less (or more, if hidden/refreshing liquidity). Live shadow
  test territory.
- Clock-skew and fillability caveats from the session-4 report carry over unchanged.

## 4. Live shadow test status (question 3 of #626)

Blocked: production-host access was denied by the session permission layer twice (the
classifier requires the operator to explicitly authorize the named production instance).
What it should measure once authorized: per signal, the quoted ask + displayed size at
signal time vs the fill actually achievable (the node's entry-evaluation log already
records quoted entry costs per decision, so the passive variant may be nearly free to
collect). This is the final gate before sizing capital on the btc GO.

## 5. Reproduction

```
# session-4 caches must exist first (see leadlag-taker-edge-2026-06-10.md §8)
uv run scripts/leadlag_subsecond.py subsecond     --dates 2026-04-22:2026-04-28
uv run scripts/leadlag_subsecond.py extract-sizes --dates 2026-04-22:2026-04-28
uv run scripts/leadlag_subsecond.py fillability   --dates 2026-04-22:2026-04-28
```

`extract-sizes` re-downloads the 168 hourly objects (~67 GB transfer, ~4.5 GB on disk
under `~/.cache/bolt-leadlag-session4/pm_tob_sized/`, resumable).
