# Cross-source clock alignment — lead-lag measurement stack (#633 item 3)

Date: 2026-06-11. Scope: quantify the clock offset between the Polymarket event
timestamps used by the lead-lag studies (#617/#631) and the leader-venue clocks, and
state how it changes the published actionability grid. Tool:
`scripts/leadlag_clock_alignment.py` (this branch); analysis-side fix:
`--pm-clock` selection in `scripts/leadlag_session4.py` (consumed by
`leadlag_subsecond.py` and `leadlag_trades_leader.py`).

## Problem

The published grids timestamped Polymarket events with `timestamp_received` — the pmxt
collector's receive clock — while leader events (Hyperliquid l2Book, Bybit trades) carry
exchange-side timestamps. Any systematic receive delay inflates the apparent PM lag, so
each published δ mixes true venue lag with collection latency. The raw pmxt parquet also
carries `timestamp` — Polymarket's own event timestamp — which the extracts never used.
That column lets us measure the offset directly over the full study window instead of
estimating it.

## Method

1. **Lake measurement** (`lake` subcommand): for every raw pmxt row in the study window
   (event_type `price_change`, `last_trade_price`), compute
   `epoch_ms(timestamp_received) − epoch_ms(timestamp)` and aggregate per date:
   count, null venue-ts count, min, p5/p25/p50/p75/p95/p99, max, and the range of
   hourly medians (intraday stability). Per-date results checkpoint to
   `<workdir>/clock_offsets/<date>.json` so the scan resumes after interruption.
2. **Live venue-clock honesty probe** (`live-probe` subcommand): connect to all three
   venues' public websockets simultaneously (Polymarket CLOB market channel on the
   rotating 5-minute up/down cycle tokens, Bybit v5 spot publicTrade, Hyperliquid
   l2Book), NTP-correct the local clock (median of 5 samples against pool.ntp.org),
   and record `local_receive − venue_ts` per message. A tight non-negative floor per
   venue bounds that venue's clock skew from true UTC: the venue stamps before we can
   possibly receive, so floor ≥ 0 means the venue clock is not running ahead.
   The local clock is guarded (`CorrectedClock`): every sample checks the
   wall-vs-monotonic delta and treats a jump beyond 50 ms as a local clock step
   (compensated, transition sample dropped, reported), and the NTP anchor is
   re-measured every 5 minutes with each re-anchor's residual reported as the run's
   error bar. This guard exists because the first 60-minute capture was contaminated —
   macOS stepped the wall clock ~2 s mid-run, shifting all three venues' offsets
   identically — and was discarded.

The lake measurement gives the magnitude over the actual study window; the live probe
verifies the venue clocks themselves are honest (so the lake offset is collection
latency, not venue clock error).

## Results — lake (full study window 2026-04-22..2026-04-28)

Offsets in ms: `epoch_ms(timestamp_received) − epoch_ms(timestamp)` per event row.

| date | event_type | n | null_venue_ts | min | p5 | p25 | p50 | p75 | p95 | p99 | max | hourly p50 range |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2026-04-22 | price_change | 1978256341 | 0 | 17 | 34 | 71 | 120 | 178 | 248 | 304 | 18528 | 117..130 |
| 2026-04-22 | last_trade_price | 2988992 | 0 | 20 | 36 | 72 | 121 | 178 | 247 | 295 | 8894 |  |
| 2026-04-23 | price_change | 1984908708 | 0 | 18 | 34 | 71 | 121 | 178 | 250 | 312 | 9906 | 116..132 |
| 2026-04-23 | last_trade_price | 2808920 | 0 | 20 | 36 | 72 | 120 | 177 | 247 | 300 | 1297 |  |
| 2026-04-24 | price_change | 1925975824 | 0 | 18 | 34 | 72 | 123 | 182 | 252 | 301 | 15339 | 117..134 |
| 2026-04-24 | last_trade_price | 2620656 | 0 | 21 | 36 | 74 | 125 | 183 | 252 | 298 | 8098 |  |
| 2026-04-25 | price_change | 1970769830 | 0 | 18 | 35 | 72 | 122 | 181 | 252 | 299 | 12269 | 113..135 |
| 2026-04-25 | last_trade_price | 2473906 | 0 | 20 | 36 | 73 | 123 | 182 | 253 | 297 | 10155 |  |
| 2026-04-26 | price_change | 1937363289 | 0 | 18 | 35 | 72 | 123 | 181 | 252 | 304 | 18025 | 115..143 |
| 2026-04-26 | last_trade_price | 2383232 | 0 | 21 | 37 | 74 | 125 | 183 | 254 | 306 | 8910 |  |
| 2026-04-27 | price_change | 1951577138 | 0 | 16 | 35 | 72 | 123 | 182 | 250 | 294 | 16881 | 114..139 |
| 2026-04-27 | last_trade_price | 2522118 | 0 | 20 | 37 | 76 | 126 | 185 | 253 | 297 | 8282 |  |
| 2026-04-28 | price_change | 1324047393 | 0 | 18 | 40 | 94 | 163 | 244 | 395 | 592 | 18020 | 127..266 |
| 2026-04-28 | last_trade_price | 1979867 | 0 | 21 | 44 | 101 | 172 | 257 | 415 | 615 | 9286 |  |

Reading: six of seven dates are tightly consistent — p50 120–123 ms, p95 248–252 ms,
intraday-stable (hourly p50 ranges of ~15–30 ms), zero null venue timestamps across
~13 B rows. 2026-04-28 is a degraded collector day: p50 163 ms, p95 395 ms, hourly
medians drifting to 266 ms, and ~33% fewer events captured. The offset is therefore
day-dependent: ~120 ms is the median-day planning number, ~165 ms the degraded-day
bound observed in this window.

## Results — live probe (45 min, 2026-06-11)

Started 2026-06-11T07:42:53Z, asset btc. Local-clock guard: zero wall-clock steps,
9 NTP re-anchors, max |residual| 7.7 ms (the run's error bar). Offsets in ms,
NTP-corrected local receive minus venue timestamp:

| source | n | min | p5 | p25 | p50 | p75 | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|
| polymarket | 1417779 | 100 | 105 | 127 | 131 | 135 | 159 | 338 | 18817 |
| bybit_trades | 21861 | 31 | 32 | 34 | 37 | 38 | 53 | 95 | 441 |
| hyperliquid_l2book | 5011 | 215 | 281 | 308 | 336 | 395 | 811 | 999 | 1865 |

Reading: all three floors are non-negative (within the ±7.7 ms error bar), so no venue
clock runs ahead of true UTC. Bybit's floor (31 ms) is ordinary network transit;
Polymarket's (100 ms, p50 131 ms) matches the lake-measured receive offset — i.e. the
lake offset is transit/collection delay, not venue clock error; Hyperliquid's l2Book
floor (215 ms) reflects its snapshot publication cadence rather than clock error.

## Corrected reading of the published grids

Every published δ on the receive-clock grids (subsecond fillability, trades-leader)
corresponds to a true end-to-end latency budget of roughly **δ − 120 ms** on a median
day, **δ − 165 ms** on a degraded day like 2026-04-28: the Polymarket quote logged as
alive at receive-time δ actually disappeared from the venue ~offset earlier, so an
order must reach the venue within δ − offset to capture the published edge.

- **btc — GO, unchanged.** The fast-clock edge (+13.3 c/share at δ = 1 s) keeps a true
  budget of ~0.88 s; the correction consumes a small fraction of a multi-second edge.
- **eth — conditional-GO, margin thinner than published.** The settlement-confirmed
  ≤ 0.25 s window shrinks to ~0.13 s true budget on a median day, ~0.09 s on a
  degraded day. The condition hardens: proven sub-100 ms signal-to-venue execution,
  measured end-to-end, before any eth deployment.
- **sol/xrp — NO-GO, unchanged.** The correction only shrinks budgets; it cannot
  revive a NO-GO.

## Analysis-side fix shipped with this report

Extraction now captures both clocks (`ts_ms` = receive, `ts_venue_ms` = venue) and
analysis selects one at load time through a single choke point
(`leadlag_session4.select_pm_clock`, `--pm-clock auto|receive|venue`, default `auto` =
venue when present). Every report artifact now opens with a `<!-- pm-clock: ... -->`
provenance line; with that one declared line set aside, old caches without
`ts_venue_ms` reproduce the published tables byte-identically under `auto`/`receive`
(manual regression re-run verified against the April cache tables). Guard self-tests
live in `scripts/manual_leadlag_clock_alignment_checks.py`. Requesting `venue` on an old cache fails loud, as does any run
whose loads resolve to different clocks (e.g. per-date sized extracts spanning two
cache generations under `auto` — a mix the per-date selection would otherwise pass
silently). The next harness window
(`scripts/leadlag_remeasure.py`, PR #639) therefore measures offset-free by default —
no re-extract of the April window is needed for the published verdicts, because the
offset correction below is uniform and the verdict directions are unchanged.

## Residual assumptions

- Leader-side timestamps (Bybit `T`, HL l2Book `time`) are taken as true event time;
  the live probe bounds their skew but cannot separate venue-internal stamping delay.
- The lake offset is measured collector-side (pmxt infra in its April deployment); a
  different collector deployment would need a fresh `lake` run, which is one command.
- Live probe and lake measurement are six weeks apart; the probe validates clock
  honesty today, not April's. The April hourly-median stability band (narrow on six
  of seven dates, see table) is the in-window evidence.
- The offset is day-dependent (2026-04-28 was degraded). Receive-clock grids inherit
  that day's collector health; the venue-clock default in the next harness window
  removes the dependency entirely.
