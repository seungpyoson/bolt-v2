# Market-Making Repo **Actual-Code** Audit — 2026-05-30

> **Status: authoritative.** This document supersedes the repo-by-repo robustness verdicts in the
> historical `mm-multi-venue-survey.md` (deleted from the live tree; preserved in git history). That
> survey labelled itself "adversarially verified, all 16" and named `joaquinbejar/market-maker-rs` the
> **primary candidate / base**. Those conclusions were drawn largely from READMEs + repo structure,
> **not** from reading the implementation source. This audit reads the actual `.rs`/`.c`/`.cpp` source
> of all 16 repos, grades every claimed algorithm with a `file:line` anchor, then **adversarially
> re-verifies** every module proposed for porting. The headline conclusion changed:
> **market-maker-rs is a port-source, not a base; GLFT must be reimplemented (every port is buggy);
> VPIN is NT-owned; the single highest-value artifact (tikr's FillSim) was not in the prior plan.**

---

## 0. Provenance (how this was produced)

- **Method:** all 16 repos shallow-cloned (`/tmp/mm-audit/`), then a deterministic workflow: 1 audit
  agent per repo reading real source (README explicitly distrusted; a feature present only in docs →
  graded `ABSENT`), then adversarial re-verification of **every** `IMPLEMENTED` port-target (re-read
  the actual function bodies, instructed to **refute**), then synthesis.
- **Scale:** 32 agents, ~2.09M subagent tokens, 471 tool calls, ~24 min.
- **Raw artifact:** workflow run `wf_5ff85448-ff8` — full JSON (per-repo audits + verdicts +
  synthesis) at the session task output `wmdfujfxx.output`.
- **NT baseline:** NT capabilities cross-checked at the **current pinned rev `38b912a`** (0.58 bump).
  (The synth agent re-read a stale `6e059dc` checkout; the cited NT crates/indicators are identical at
  `38b912a` and were re-confirmed first-hand this session.)
- **Caveat / next step:** subagent findings are treated as **drafts**. Every port-target below carries
  a `file:line`; each will be **personally re-verified at the source line before any code is ported.**
  This doc is a research artifact, not a build authorization.

---

## 1. Headline verdict

1. **Do not base bolt-v2 on any surveyed repo, and do not adopt one wholesale.** None is
   production-grade; every candidate is either not pure-Rust, license-blocked, self-labelled
   experimental, or has decorative execution. Adopting one wholesale means replacing NT's
   battle-tested exec/PnL/risk/backtest with a solo dev's experimental code — to move real capital.
2. **Keep NautilusTrader as the base; port a small slice of *verified* pure-Rust math onto it.**
3. **Is NT enough?** *Split verdict.* NT **infrastructure** = robust, more than enough. NT **quoting
   math** = thin; it genuinely lacks exactly four things (§5).

---

## 2. Full ranking — all 16 (real-code grade)

| Grade | Role | Repo | One-line (from actual source) |
|---|---|---|---|
| **B** | PORT-SOURCE | `joaquinbejar/market-maker-rs` | Best pure-Rust MM math here: real A-S closed-form + intensity-k calibration; but prototype-grade (cosmetic Decimal, hardcodes), no SVI/BL, greeks outsourced |
| **B** | PORT-SOURCE | `kryptic-sh/tikr` | Robust pure-Rust crypto spot/perp MM engine; **FillSim trade-through queue model is the only PRODUCTION-graded module in the survey**; no options/microstructure |
| **B** | PORT-SOURCE | `DaruFinance/quant-mm-simulator-rs` | Clean zero-unsafe MM **sim** with five verified-correct pure-function helpers; **its GLFT closed-form is mathematically wrong** |
| **C** | MATH-REF | `holypolyfoundation/bs-p` | C-SIMD/AVX-512 kernel (FFI) for Polymarket binary-outcome quoting; not pure Rust, not an MM system |
| **C** | MATH-REF | `davidakpele/atomic-mesh` | C++/FFI HFT mesh; "Avellaneda-Stoikov" is fixed-spread + linear inventory-skew (no reservation, no σ²(T−t), no κ) |
| **C** | MATH-REF | `Faraone-Dev/atomic-mesh` | Near-duplicate of `davidakpele/atomic-mesh` (same C++/FFI hot path, same mislabel) |
| **C** | IRRELEVANT | `trevortrinh/exchange` | Full-stack CLOB exchange + demo bots; genuine MM math ≈ a 6-line liquidity loop; transitive C/FFI |
| **C** | TOY | `Capataina/Nyquestro` | Well-tested pure-Rust matching engine + LOB sim, but README's MM agent / VaR breaker / risk limits are **unimplemented** |
| **D** | MATH-REF | `pranay123-stack/crypto-hft-…-profitable-system` | The serious A-S/grid/adaptive/backtest exists **only in C++** (unportable); the Rust crates are a shallow, never-wired subset |
| **D** | MATH-REF | `Jacobo-EG/market-making` | 316-LOC Kraken GLFT toy; its c1/c2 + intensity math is worth re-deriving but has inventory + ξ/γ bugs, zero tests, no license |
| **D** | IRRELEVANT | `ssanin82/blockops` | Trading-infra platform; exactly one non-canonical A-S, zero quant tests, **no LICENSE** |
| **D** | IRRELEVANT | `SemiuAdesina/poly-arb-engine-rust` | Polymarket YES+NO arbitrage bot (EIP-712 signing); zero MM math, zero tests |
| **F** | TOY | `HarmishTervadiya/crabex` | 909-LOC toy CEX matching demo; fixed-offset `i%10` "MM" loop; zero tests; panics in hot paths |
| **F** | IRRELEVANT | `athiyenarivalagan/hft-market-making-rust` | 497-LOC abandoned single-commit skeleton; core orderbook commented out, doesn't compile |
| **F** | TOY | `seyedb/agentic-mm-engine` | 392-LOC toy; one naive inventory-skew quote fn + random-walk sim; `.unwrap()` panics throughout |
| **F** | IRRELEVANT | `sqxiao000/rust-make-a-market` | 206-LOC CLI coin/dice/cards EV guessing game — not an MM system |

---

## 3. The three port-sources, in detail

### 3.1 `joaquinbejar/market-maker-rs` — B, pure Rust, MIT, 42,010 rs LOC, 792 tests

| Algo | Status | Evidence | Note |
|---|---|---|---|
| Avellaneda-Stoikov | **IMPLEMENTED** | `src/strategy/avellaneda_stoikov.rs:80-289` | `r = s − q·γ·σ²·τ`, `δ = γ·σ²·τ + (2/γ)·ln(1+γ/k)`; validated, ~20 tests |
| GLFT | PARTIAL | `src/strategy/glft.rs:229-573` | Terminal-inventory penalty (lin/exp/quad) + dynamic γ; **not** the full closed form (no c1/c2 / asymptotic quote); exp penalty is a Taylor approx |
| Grid | IMPLEMENTED | `src/strategy/grid.rs:82-160` | Geometric/arithmetic spacing + size progression |
| Adaptive spread | IMPLEMENTED | `src/strategy/adaptive_spread.rs:49-160` | OB + trade-flow imbalance → spread/skew; div-by-zero guarded |
| Intensity-k calibration | **IMPLEMENTED** | `src/analytics/intensity.rs`; `src/strategy/calibration.rs:591-771` | Weighted log-linear regression of ln(fill_rate) vs spread → k, R², CI; EWMA smoothing, clamps |
| VPIN | IMPLEMENTED | `src/analytics/vpin.rs:268-505` | Easley/López-de-Prado/O'Hara; tick + Lee-Ready classifier; ~25 tests **(redundant with NT)** |
| Order-flow toxicity | IMPLEMENTED | `src/analytics/order_flow.rs:230-547` | Rolling OFI, net flow, per-side VWAP **(redundant with NT)** |
| Circuit breaker | IMPLEMENTED | `src/risk/circuit_breaker.rs:38-130` | Active/Triggered/Cooldown FSM: daily-loss, vol-spike, consec-loss, rapid-DD, manual |
| Drawdown | IMPLEMENTED | `src/risk/drawdown.rs:60-90` | Peak-tracking, breach flag, history |
| Position limits | IMPLEMENTED | `src/risk/limits.rs:47-295` | max_position/notional, pre-trade gate, scale_order_size **(NT pre-trade covers)** |
| Realized vol | IMPLEMENTED | `src/market_state/volatility.rs:139-364` | stddev / EWMA(RiskMetrics) / Parkinson — **no kernel, no SVI** |
| SVI | **ABSENT** | — | not in source |
| Breeden-Litzenberger | **ABSENT** | — | not in source |
| Greeks | PARTIAL | `src/options/greeks.rs` | per-option BS **delegated to external `optionstratlib`**; repo does only portfolio aggregation/hedge sizing |
| Delta hedging | IMPLEMENTED | `src/options/greeks.rs:260-280` | shares-to-hedge + threshold trigger |
| Backtest engine | IMPLEMENTED | `src/backtest/engine.rs:431-585` | crossing-fill sim, vol slippage, fees, equity curve **(NT far more capable)** |
| Exchange execution | PARTIAL | `src/execution/` | trait + OrderManager + latency, but only mock + orderbook-rs connector |

**Disqualifiers as a base:** Decimal precision is **cosmetic** (`decimal_ln/sqrt/powi` round-trip
through f64, `src/types/decimal.rs:39-100`); **42 hardcoded magic constants** in `calibration.rs`
(NO-HARDCODES violation); options stack depends on `optionstratlib` (not pure-Rust-binary-pullable);
`vol-regime new()` can divide-by-zero panic on threshold=0.

### 3.2 `kryptic-sh/tikr` — B, pure-Rust math modules, 52,120 rs LOC, 375 tests

| Algo | Status | Evidence | Note |
|---|---|---|---|
| A-S | PARTIAL | `crates/tikr-strategy/src/avellaneda_stoikov.rs:130` | reservation skew real+tested; academic half-spread **deliberately** replaced by `base_spread_bps` (documented `:14-22`) |
| GLFT | PARTIAL | `crates/tikr-strategy/src/glft.rs:141` | infinite-horizon variant; η hardcoded to 1 (no k/A calibration) |
| Grid | IMPLEMENTED | `static_grid.rs` (1341 LOC) + `layered_grid.rs` (710) | full, inventory-capped |
| **FillSim queue model** | **IMPLEMENTED / PRODUCTION** | `crates/tikr-backtest/src/fill_sim.rs:589` (+ `match_trade:1112-1240`, `update_book_state:1030-1109`, `walk_book_ioc:192-237`) | **the standout.** Genuine queue-priority: queue-ahead snapshotted at placement, drained by trade-through, partial fills, post-only `-5022` rejects, IOC book-walk, cancel attribution |
| Adverse-selection tracker | IMPLEMENTED | `crates/tikr-strategy/src/spread_scalp/adverse_tracker.rs:133-201` | post-fill mid-drift EMA → dynamic min-spread widening; integrated in hot path |
| EWMA realized-vol | IMPLEMENTED | `crates/tikr-strategy/src/volatility.rs:74-118` | per-update half-life `λ=0.5^(Δt/hl)`, RiskMetrics σ² on log returns |
| Sticky risk gate | PARTIAL | `gate.rs:131-136` | circuit-breaker half real (sticky halt, 4 kill-switches); cancel-storm half weaker |

**Disqualifiers as a base:** full workspace pulls transitive C (`polars`→{zstd-sys,lz4-sys},
`reqwest` TLS→ring/aws-lc-sys+cmake, secp256k1-sys); ~328 `unwrap/expect` in non-test src;
self-labelled pre-alpha. **The math modules to port are themselves pure Rust — extract only those.**

### 3.3 `DaruFinance/quant-mm-simulator-rs` — B, pure Rust, zero-unsafe, 5,888 rs LOC

Verified-correct pure-function helpers (all HOLD, §4): inventory-penalty library, quote-shape
primitives, reference-price trackers, adverse-selection filter suite, threshold hedge engine.
**Its GLFT closed-form is mathematically WRONG** (`src/models/glft.rs:82-87`, refuted §4). It is a
backtest sandbox, not a live engine.

---

## 4. Adversarial verification (15 port-targets — "refute it" pass)

`REFUTE` = the `IMPLEMENTED` grade did **not** survive a skeptical re-read → **not port-ready as-is.**

| Verdict | Robustness | Module | Finding |
|---|---|---|---|
| **HOLD** | PROTOTYPE | mm-rs A-S closed form | Complete + correct; sign convention right (long inventory → reservation below mid); both terminal terms present |
| **HOLD** | PROTOTYPE | mm-rs intensity-k calibration | Real weighted log-linear regression; filters fill_rate∈(0,1], ≥2 spread levels, min_samples; **CI uses unweighted SE (bug)** |
| **REFUTE** | PROTOTYPE | mm-rs adaptive-spread/inventory-skew | Real + 23 tests pass, but **has no inventory skew at all** (skew lives in the A-S/GLFT siblings); mislabeled |
| **HOLD** | PROTOTYPE | mm-rs vol-regime detector | Real; ratio + zero-historical guard; Low/Normal/High/Extreme classify; **docstring/formula boundary mismatch** |
| **REFUTE** | PROTOTYPE | mm-rs VPIN | Real + 26 tests, but it's classified-trade VPIN **missing the namesake Bulk-Volume-Classification**, and a strict **subset of NT 0.58's VPIN + signed_vpin** |
| **HOLD** | **PRODUCTION** | **tikr FillSim queue model** | Tried to refute, could not. Genuine queue priority + drain + partial + post-only + IOC + cancel attribution |
| **HOLD** | PROTOTYPE | tikr adverse-selection tracker | Real + integrated in hot path; post-fill drift bps correct |
| **REFUTE** | PRODUCTION | tikr sticky risk gate | Circuit-breaker half real (sticky halt, 4 kill-switches); **cancel-storm half weaker → port the breaker, not the whole gate** |
| **HOLD** | PROTOTYPE | tikr EWMA vol | Standard time-aware RiskMetrics; correct |
| **REFUTE** | PROTOTYPE | **DaruFinance GLFT closed-form** | **A-term (the only thing distinguishing GLFT from A-S) is mathematically WRONG** (`glft.rs:85-86`) |
| **HOLD** | PROTOTYPE | DaruFinance inventory-penalty library | All six formulas re-derived + verified |
| **HOLD** | PROTOTYPE | DaruFinance adverse-selection filter suite | All six filters complete; microprice-dev ≡ standard |
| **HOLD** | PROTOTYPE | DaruFinance quote-shape primitives | All five pure generators correct |
| **HOLD** | PROTOTYPE | DaruFinance threshold hedge engine | Correct threshold-triggered inventory hedge |
| **HOLD** | PROTOTYPE | DaruFinance reference-price trackers | weighted_mid≡microprice at TOB (verified); Stoikov lean correct |

---

## 5. Is NT enough? — split verdict

**NT infrastructure: ROBUST (more than enough).** Confirmed: execution crate
(matching_engine / order_manager / `protection.rs` / fill models), risk crate (pre-trade
position/notional), portfolio crate (realized/unrealized PnL), analysis crate
(Sharpe/Sortino/Calmar/max-DD), backtest engine, `indicators/src/book/imbalance.rs`, **real VPIN +
`signed_vpin`** (`hurst_vpin_directional/strategy.rs:54-61,413-418`), greeks + Jäckel implied-vol
(`model/src/data/greeks.rs:192`, `implied-vol` 2.0.0). Every surveyed repo's
exec/backtest/PnL/risk/metrics/VPIN is **redundant with or weaker than** NT.

**NT quoting math: THIN.** A grep across `crates/indicators` + `crates/trading/src` (non-test) for
`avellaneda|reservation.price|gueant|glft|arrival.intensity` returns **nothing**. NT genuinely lacks:

1. **Avellaneda-Stoikov** reservation-price + optimal-spread closed form
2. **GLFT** closed form
3. **Order-arrival-intensity (A,k) calibration** from fills (`λ=A·e^{−kδ}` log-linear fit)
4. A realistic **passive-maker queue-position fill model** for backtests (NT's default fill model is
   simpler than tikr's trade-through queue sim)

These four are exactly where porting/reimplementation adds value. Everything else → defer to NT.

---

## 6. The verified port plan (pure-Rust math → NT)

> Effort S/M/L. `verified` = survived the §4 adversarial pass. Every port must **lift hardcodes to
> TOML**, **convert panics to `Result`** (no panic in LiveNode), and be **re-verified at `file:line`**
> before merge.

| # | Module | From → file | Onto | Effort | Verified |
|---|---|---|---|---|---|
| P1 | **FillSim trade-through queue model** (the single highest-value item — makes maker-fill backtests realistic) | `tikr` `crates/tikr-backtest/src/fill_sim.rs:589` (+match_trade/update_book/walk_book_ioc) | NT FillModel | **L** | ✅ PRODUCTION |
| P2 | **A-S reservation + optimal-spread** (replace cosmetic Decimal with real numeric path; constants→TOML) | `mm-rs` `src/strategy/avellaneda_stoikov.rs:111-289` | new strategy | M | ✅ |
| P3 | **Intensity-k calibration** (fix unweighted-SE CI bug; de-hardcode) | `mm-rs` `src/strategy/calibration.rs:591-720` (+`analytics/intensity.rs:418-501`) | new | M | ✅ |
| P4 | **Vol-regime detector + param multipliers** (fix boundary mismatch; `Result` not infallible) | `mm-rs` `src/strategy/calibration.rs:932-988` | new | S | ✅ |
| P5 | **Adverse-selection adaptive-spread tracker** | `tikr` `adverse_tracker.rs:133-201` | new | S | ✅ |
| P6 | **EWMA realized-vol** (replace Decimal→String→f64 round-trip; dt clamp) | `tikr` `volatility.rs:74-118` | new | S | ✅ |
| P7 | **Inventory-penalty skew library** (lin/quad/exp/asym/soft/hard cap; add NaN/scale=0 guards) | `DaruFinance` `src/quoter/inv_penalty.rs:38-142` | new | S | ✅ |
| P8 | **Quote-shape primitives** (ladder/geometric/paired/dynamic-depth; NaN/neg-spread guards) | `DaruFinance` `src/quoter/shapes.rs:82-185` | new | S | ✅ |
| P9 | **Reference-price trackers** (weighted-mid/Stoikov/VWAP/EWMA-fair; monotonic-ts guard) | `DaruFinance` `src/quoter/refprice.rs:42-188` | new | S | ✅ |
| P10 | **Adverse-selection filter suite** (OFI/toxicity/vol-surge/microprice-dev/queue-imb + Any/All; constructors→`Result`; use NT book-imbalance/VPIN as inputs) | `DaruFinance` `src/quoter/adverse.rs:79-393` | new | M | ✅ |
| P11 | **Threshold inventory-hedge engine** (cross-instrument; add fees/slippage before live) | `DaruFinance` `src/hedge/engine.rs:100-145` | new | S | ✅ |
| P12 | **Circuit-breaker FSM + drawdown** (port the breaker, not tikr's whole gate) | `mm-rs` `src/risk/circuit_breaker.rs:38-130`, `src/risk/drawdown.rs:60-90` | bolt (additive to NT pre-trade) | M | ✅ |
| **P13** | **GLFT closed form** — **REIMPLEMENT from Guéant 2013/2017 paper, do NOT port** | none (cross-ref `DaruFinance glft.rs:82-87` ⟂buggy, `Jacobo-EG lib.rs:49-53` ⟂γ-bug) | new | M | ❌ greenfield |

**Do NOT port (NT owns, robustly):** backtest engine, default fill sim, execution/order-manager/
connectors, PnL/positions, Sharpe/Sortino/Calmar/VaR/max-DD, **VPIN**, order-book imbalance, greeks,
implied-vol, position/notional limits.

---

## 7. Dependency / license / safety landmines (confirmed)

- **Not pure Rust → drop entirely (Rule 5):** `davidakpele/atomic-mesh` & `Faraone-Dev/atomic-mesh`
  (C++ hot path via `cc` + `extern "C"`; the **live** quoting path is C++), `holypolyfoundation/bs-p`
  (C AVX-512 kernel via `cc` + unsafe FFI; math duplicated 4×), `pranay123` (serious algos C++-only),
  `trevortrinh/exchange` (transitive C via native-tls/openssl-sys/aws-lc-sys/ring/bindgen/libsqlite3-sys).
- **License-blocked → re-derive public math only, never copy code:** `ssanin82/blockops` (no LICENSE,
  all-rights-reserved), `HarmishTervadiya/crabex`, `athiyenarivalagan/hft`, `Jacobo-EG` (none);
  `Capataina/Nyquestro`, `trevortrinh`, `pranay123` (Cargo/README claim MIT, **no LICENSE file**).
- **Transitive C if full crate pulled → extract pure-math modules only:** `tikr` (polars/TLS/secp256k1-sys).
- **Options dep not portable:** `mm-rs` greeks delegate to `optionstratlib` — reimplement or use NT greeks.
- **NO-HARDCODES conflict on every port:** mm-rs (42 consts in calibration.rs), tikr/DaruFinance/
  atomic-mesh bake thresholds/half-lives/seeds → must lift to TOML (Rule 1).
- **Panic-in-live-path:** mm-rs vol-regime div-by-zero; tikr ~328 unwrap/expect; DaruFinance `panic!`
  as sim assertions → convert to `Result` before any LiveNode use.

---

## 8. Corrections to the prior survey (historical `mm-multi-venue-survey.md`) / #488 port table

| Item | Prior (README-based) | This audit (actual code) |
|---|---|---|
| `market-maker-rs` role | **primary candidate / base** | **port-source** (B, prototype) — not a base |
| GLFT | `ADAPT★` (port from mm-rs) | **REIMPLEMENT from paper** — mm-rs's is A-S+penalty, DaruFinance's A-term is wrong, Jacobo-EG's has a γ-bug |
| VPIN | `PORT` (from mm-rs) | **NT-OWNS** — NT ships VPIN + signed_vpin; mm-rs's is a subset |
| FillSim (maker queue model) | *absent from plan* | **P1, highest-value port** (tikr, PRODUCTION-grade) |
| `volatility.rs` (RV) | "REUSE bolt's, do not port" | bolt's is crude naive RV; **realized kernel is net-new** (separate fair-value workstream) |
| adaptive-spread (mm-rs) | implied port-worthy | **refuted** — has no inventory skew |

---

## 9. Recommendation (bottom line)

Keep **NT** for exec / PnL / risk / backtest / VPIN / book-imbalance / greeks / grid_mm. Build a new
NT `Strategy` that quotes off the fair-value engine, and stand up a small **pure-Rust MM-math crate**
holding the §6 ports (P2–P12) + the reimplemented GLFT (P13). Port **tikr FillSim as an NT FillModel**
(P1) so maker backtests are realistic. **Adopt nothing wholesale. Copy no license-blocked code.
Re-verify every `file:line` before porting.**
