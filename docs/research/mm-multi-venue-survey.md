# Multi-Venue Market-Making Strategy Survey

> ⚠️ **Superseded for repo verdicts (2026-05-30):** the per-repo robustness grades, the "primary
> candidate" call, and the port table in this survey were re-audited by a fresh **actual-source** read
> of all 16 repos with adversarial verification — see
> [`mm-code-audit-2026-05-30.md`](./mm-code-audit-2026-05-30.md), which is **authoritative**. Headline
> changes: `market-maker-rs` → port-source (not base); GLFT → reimplement from paper (every port is
> buggy); VPIN → NT-owned; `tikr` FillSim → highest-value port (was absent here). Treat the framing
> below as the broader multi-venue direction; trust the audit doc for per-repo robustness + the plan.

> **Status:** survey complete — 16/16 repos cloned + read + adversarially verified, 7 resource topics
> scouted (ultracode, 2026-05-30). Evidence-backed; **not a build approval**.
> **Scope declaration (rule #10):** this **broadens** issue #488's "External strategy assessment"
> section — which evaluated 4 repos through a **Polymarket-only** lens — into a **16-repo,
> multi-venue** survey (Polymarket binary → Hyperliquid perps → CEX spot/futures → perp DEXs).
> #488 remains the canonical *Polymarket archetype* proposal; this doc is the broader framework
> direction that feeds it. If preferred, this can be re-homed under a dedicated umbrella issue.
> **Method:** every repo cloned `--depth 1` into `var/mm-research/clones/<name>` and read directly
> (one analysis agent per repo + an adversarial verifier on every port-worthy / production-grade
> verdict). The primary candidate (`market-maker-rs`) was additionally read first-hand by the
> lead session (anchors below). No claim here is from a README alone; file:line anchors cite
> cloned HEAD as of 2026-05-30 and may drift — function/module names are the durable anchors.

---

## TL;DR — the converged design

Building a profitable MM strategy from scratch is hard; the **valuable, hard part is not the
~30-line Avellaneda-Stoikov formula — it is the surrounding apparatus** (order-flow/VPIN toxicity,
intensity calibration, runtime risk kill-switches, inventory skew, calibration + backtest harness).
The strategy therefore splits cleanly into two layers:

1. **A shared, robust FRAMEWORK** (one codebase, reused across every venue) — **NT-first**:
   - **Substrate + as much apparatus as NT already ships.** NautilusTrader 0.58 (this repo's pinned
     version, PR #487) is **not just an execution substrate** — it ships, tested and venue-agnostic:
     an inventory-skewed **grid market-making strategy** (`grid_mm`), a **VPIN toxicity** signal
     (`hurst_vpin_directional`), Sharpe/Sortino/WinRate analysis, volatility indicators, a pre-trade
     `RiskEngine`, `Position`/PnL, post-only orders, and a backtest engine. *Use these; do not
     reinvent them.* (Evidence section below.)
   - **Port from donors ONLY the genuine gaps NT lacks** (pure-math, unit-tested): GLFT expiry-aware
     quoting (dynamic γ → terminal), intensity calibration (k from fills), a **runtime**
     circuit-breaker FSM (NT's `RiskEngine` is *pre-trade only*), and an equity-curve drawdown monitor.
   - **Substrate glue** — bolt-v3's own: archetype binding, submit-admission, decision-evidence,
     order-intent (post-only limit) layers, and the oracle fair-value edge.
2. **A best-of-breed, tunable QUOTE ENGINE per instrument type** (pluggable, may be sourced from
   *different* places — **NT first, then a donor**):
   - Pick the *best* engine for each instrument's microstructure — NT `grid_mm` (+ (0,1) Instrument
     price-band + oracle-anchored mid) or bs-p's logit-space technique for binary; expiry-aware GLFT for resolving
     markets; funding-aware skew for perps; NT `grid_mm` / classic A-S for deep CEX books.
     **Specialized beats generalist where it exists; NT-native beats a port where NT has it.**

This satisfies the stated requirements: multi-venue, per-venue tuning, a robust shared framework,
**use NT as much as possible, agnostic + no-hardcode, and not inventive** — without forcing one
algorithm onto every instrument.

---

## Assessment rubric (multi-venue robustness lens)

Each repo is judged on:

1. **What it actually is** (read the core loop, not the README): live MM engine / backtest-sim /
   arbitrage bot (not MM) / toy-template / library.
2. **Stack & purity** — language; FFI/C/AVX/unsafe (bolt is PURE RUST, `#![deny(unsafe_code)]` —
   FFI is a hard porting cost).
3. **License** — verified from the actual `LICENSE` file (README claims do not count).
4. **Core quoting algo** — A-S / GLFT / grid / adaptive-spread / depth-imbalance / fixed / none;
   real implementation vs named-but-stubbed.
5. **Apparatus depth** — order-flow, VPIN, intensity calibration, inventory skew, circuit-breaker,
   drawdown, backtest (the hard, valuable part).
6. **Venue coupling & multi-venue portability** — how tightly the exec layer binds the algo; can the
   quoting+apparatus lift to binary(0,1) / perp / spot / perp-DEX? Flag bounded-price, annualization,
   and orderbook-depth assumptions.
7. **Production signals** — tests, CI, error handling, async runtime, maintenance.
8. **Robustness verdict** — production-grade / partial / educational / toy / arb-not-mm / inaccessible.
9. **Profit-tuning** — which params to tune, what the edge is, best instrument fit.
10. **Port verdict** — PRIMARY / IDEAS-ONLY / REFERENCE / SKIP.

---

## Instrument-type microstructure seams (why one engine can't fit all)

The quote-engine math has venue-specific seams. These are the parameters/assumptions that must be
re-tuned or re-derived per instrument; they are *also* the reason a specialized engine can beat a
generalist.

| Instrument | Price domain | Terminal/horizon | Key extra signal | Book depth | Adverse-selection driver |
|---|---|---|---|---|---|
| **Polymarket binary** | bounded **(0,1)**, YES+NO=1 | **hard resolution/expiry** | oracle fair-value (already in bolt-v3) | thin (1–2 levels) | oracle move picks off stale quotes |
| **Hyperliquid perp** | unbounded > 0 | rolling (no expiry) + **funding** | funding rate, on-chain OB | medium–deep | informed flow + funding skew |
| **CEX spot/futures** | unbounded > 0 | rolling | maker rebate tiers, latency | deep | latency / informed flow |
| **Perp DEX (dYdX/Drift/GMX)** | unbounded > 0 | rolling + funding | oracle vs CLOB vs AMM mechanics, on-chain latency | varies | oracle/AMM arb, MEV |

The two seams that recur in every A-S/GLFT implementation (confirmed first-hand in the primary
candidate, see below): **(a) annualized volatility** assumption, and **(b) `time_to_terminal`**.
Binary/options have a real terminal and need the (0,1) clamp + de-annualization; perps/CEX have no
terminal (use a rolling horizon) and the native annualization is fine.

---

## Use NT first — what NT 0.58 already ships (hard evidence)

**This corrects a #488 premise.** #488 (2026-05-28) asserted "[the MM apparatus] is absent today and
NautilusTrader does not ship it either (NT is execution/backtest substrate, not a strategy brain)."
**That is outdated for NT 0.58** (this repo's pin since PR #487). Read first-hand in the NT cargo
checkout at bolt's **pinned NT rev `6e059dc`** (re-verified — examples confirmed present at the pin):

**NT ships, tested + venue-agnostic — REUSE, do not reinvent:**
- **Grid market-making strategy** — `crates/trading/src/examples/strategies/grid_mm/` (strategy 380
  LOC + tests 379 LOC). `GridMarketMakerConfig` (`config.rs:31`): `num_levels`, geometric
  `grid_step_bps`, **`skew_factor`** (inventory), `requote_threshold_bps`, `on_cancel_resubmit`,
  optional per-level size. `grid_orders` (`strategy.rs:84`): `skew = skew_factor·net_position`
  (`:96`), `buy = mid·(1−pct)^level − skew` / `sell = mid·(1+pct)^level − skew` (`:108-109`), each snapped to a valid tick via `instrument.next_bid_price`/`next_ask_price`
  (`:113-114`) → **price-band bounding is delegated to the Instrument definition, NOT hardcoded in the
  strategy** (ideal for binary + agnostic: a correctly-specified (0,1) Polymarket instrument bounds
  quotes natively — verify whether out-of-band returns `None` vs clamps to the boundary tick).
  `should_requote` on bps move (`:70`); full `on_quote` cancel-all+resubmit lifecycle (`:193`) with
  **`post_only`** orders (`:326`), `on_order_filled/canceled/rejected/expired` handlers, cancel-all
  on stop (`:187`). **Agnostic:** strategy takes `InstrumentId` from config — only *tests* hardcode a
  `ETHUSDT-PERP.BINANCE` fixture. → *This is the first-cut MM engine.* (≈ market-maker-rs
  `grid.rs::generate_grid_with_inventory`, already in NT idiom.)
- **VPIN toxicity** — `hurst_vpin_directional/strategy.rs:41-61`: `vpin` + **`signed_vpin`**,
  volume-bucket buy/sell imbalance over `vpin_window` (Volume-synchronized Probability of Informed
  Trading). → *This is the toxicity signal; lift it, don't port market-maker-rs `vpin.rs`.*
- **`composite_market_maker`** (also at the pin) — quotes around an anchor with `inventory_skew_factor`
  **plus a `signal_skew_factor`** driven by an external `signal_instrument_id` residual
  `(signal_mid − baseline)/baseline` (`composite_market_maker/config.rs:38-56`). → *Point the signal at
  the oracle fair-value and it becomes an oracle-anchored maker natively — the best NT-native base.*
- Other example strategies: `delta_neutral_vol`, `ema_cross`.
- **Analysis:** `SharpeRatio`, `SortinoRatio`, `WinRate` (`crates/analysis/src/analyzer.rs:90-91`).
- **Volatility indicators:** `crates/indicators/src/volatility/` (ATR, DC, fuzzy).
- **Pre-trade risk:** `RiskEngine` (`crates/risk/src/engine/`). **Position/PnL:**
  `crates/model/src/position.rs` + position events. **Post-only orders:** order events expose
  `post_only` (`crates/model/src/events/order/*.rs`). **Backtest:** `crates/backtest/`.

**NT does NOT ship — these are the real PORT targets (from `market-maker-rs`, pure-math, unit-test each):**
- **GLFT expiry-aware quoting** — dynamic γ rising into a hard terminal (`glft.rs:401,477`); fits
  binary/options resolution. NT's grid has no terminal model.
- **Intensity calibration** — k estimated from fills via log-linear OLS (`intensity.rs`); required to
  run A-S/GLFT non-trivially.
- **Runtime circuit-breaker FSM** — daily-loss / vol-spike / consecutive-loss / drawdown halt
  (`circuit_breaker.rs`). **NT `RiskEngine` is pre-trade only** → additive, not duplicative.
- **Equity-curve drawdown monitor** (`drawdown.rs`) — NT doesn't track a peak-to-trough equity curve.
- (Optional) **inventory-aware size scaling** `RiskLimits::scale_order_size` if NT grid's flat
  per-level size is insufficient.

**Net effect:** the "don't be inventive / use NT / agnostic / no-hardcode" directive is satisfied by
starting from NT `grid_mm` + NT VPIN and porting only the four gaps above — a much smaller, safer
surface than #488's wholesale port plan.

---

## Primary candidate — `joaquinbejar/market-maker-rs` (first-hand verified)

**Verdict: co-PRIMARY pure-Rust donor (with `quant-mm-simulator-rs`) — the apparatus source.**
Read first-hand by the lead session and survey-confirmed (full ranking below).

**Provenance (read directly, clone HEAD 2026-05-30):**
- **License:** MIT, © 2025 Joaquin Bejar Garcia — verified from `LICENSE` (not just README).
- **Maturity:** `Cargo.toml` v0.3.0, edition 2024, ~37k LOC; last commit merges PR #66 (event
  system) → actively developed. `#![deny(unsafe_code)]` at `src/lib.rs:401` → pure-safe Rust.
- **Dependency purity:** core math path = `thiserror` + `rust_decimal` only. Heavy infra
  (`tokio`/`axum`/`prometheus`/`optionstratlib`/`orderbook-rs`) is **feature-gated optional** →
  the PORT set translates without dragging async/web/FFI. **No C/AVX/FFI.**

**Architecture shape (decisive for "robust framework"):** the strategy layer is a **library of
parallel pure-math engines**, not a monolith — the right donor shape. bolt-v3 supplies the
*framework*; this supplies the *math*.
- A-S trait contract: `src/strategy/interface.rs:86` `trait AvellanedaStoikov` →
  `calculate_reservation_price` (`:108`), `calculate_optimal_spread` (`:135`),
  `calculate_optimal_quotes(mid, inventory, γ, volatility, time_to_terminal_ms, k) -> (bid, ask)`
  (`:163`). Async variant `:211` is a documented placeholder.
  - **Adaptation seams visible right in the signature:** `volatility` is documented "annualized"
    (`:98`); `time_to_terminal_ms` (`:99`) is a real terminal. → binary/options: de-annualize +
    clamp to (ε, 1−ε) + real expiry; perps/CEX: rolling horizon, native annualization.
- Engines (each its own pure-math module):
  - `strategy/glft.rs` — `calculate_reservation_price:258`, `calculate_optimal_spread:327`,
    `calculate_optimal_quotes:401`, **`calculate_dynamic_gamma:477`** (γ rises near terminal →
    best fit for **expiry-bearing** instruments: binary, options).
  - `strategy/grid.rs` — `generate_grid:323`, `generate_grid_with_inventory:384`,
    `calculate_price:418`, `calculate_level_size:460` (simplest; LP-reward harvesting; no OB dep).
  - `strategy/adaptive_spread.rs` — `calculate_orderbook_imbalance:439`,
    `calculate_tradeflow_imbalance:542`, `calculate_spread:581`, `…_with_volatility:637`.
  - `strategy/depth_based.rs` — `calculate_ask_size:103` / `calculate_bid_size:128` (deep CLOB;
    **defer** for thin binary books, **relevant for deep CEX/perp books**).
- Apparatus (the valuable part): `analytics/{order_flow,vpin,intensity,live_metrics}.rs`,
  `strategy/calibration.rs` (1,529 LOC — γ from inventory half-life, k from fills, vol-regime),
  `risk/circuit_breaker.rs` (FSM: `TriggerReason:13`, `CircuitBreakerState:41` {Active /
  Triggered{reason,at} / Cooldown}, `new():147`), `risk/drawdown.rs`, and
  `risk/limits.rs::scale_order_size` (inventory-aware sizing).
- **NT-OWNS / skip** (do not dual-path): `execution/*`, `position/{inventory,pnl}.rs`,
  `backtest/engine.rs`, `api/*`, `persistence/*`, `data_feeds/*`.
- **Misread-corrected (why first-hand reading matters):** `chain/` is **options-chain MM**
  (multi-strike, Greeks; `option_chain_orderbook::ExpirationOrderBook`), **not** on-chain/DEX —
  feature-gated behind `options`, not relevant to the perp-DEX fan-out. `multi_underlying/` is the
  genuinely multi-venue-relevant extra (multi-asset portfolio/risk).

**Survey-confirmed:** adversarial verifier could not refute any claim (refuted=false, confidence=high);
production-grade, 792 tests, CI, `deny(unsafe)`. For a NT-first consumer, port the **gap set**
(intensity calibration, circuit-breaker, drawdown) — not the whole library (NT owns exec/PnL/backtest).
Co-primary pure-Rust donor: `quant-mm-simulator-rs` (cleaner `Quoter` trait + Cartea-Jaimungal/Ho-Stoll).

### Binary specialist — `holypolyfoundation/bs-p` (first-hand verified)

**Verdict: IDEAS-ONLY — but the idea is the best-of-breed *binary* quoting technique.**
- **License:** MIT, © 2026 borkiss — verified from `LICENSE`.
- **Stack:** the quoting math is a **C/AVX-512 kernel** (`packages/{bun,python,crates}/c_src/kernel.c`);
  the 5 `.rs` files are FFI bindings, not the algorithm. → **direct reuse violates bolt's PURE RUST
  rule.**
- **Core idea (`kernel.c:234` `calculate_quotes_logit`):** Avellaneda-Stoikov done in **logit space** —
  `(x_t logit-price, q_t inventory, sigma_b, gamma, tau, k)` → bid/ask computed in logit space →
  `kernel_sigmoid_exact` (`:31`) maps back to (0,1). **Quotes are bounded in (0,1) by construction —
  no clamp needed.** `kernel_greeks_from_logit:86` is `p(1−p)` logit derivatives, *not* option greeks.
- **Confirmed absent:** no N(d2)/Gaussian-CDF/digital pricer (grep over `*.c`/`*.h` returns none) —
  the "Black-Scholes for prediction markets" name is a misnomer; bolt-v3's taker already has the
  real digital pricer (`compute_fair_probability_up`).
- **Best-of-breed recommendation (binary instrument):** reimplement the **logit-space A-S technique
  in Rust (~5 lines)** — it is more binary-native than clamping a generic A-S. Do **not** link the C
  kernel. This is the binary-specific engine; market-maker-rs GLFT-with-(0,1)-clamp is the
  alternative for expiry-bearing markets.

**Survey-confirmed** (verifier re-read `kernel.c:135` at HEAD `b9b8e62`: `r_x = x_t − q_t·risk_term`,
real logit-space A-S; refuted=false). Verdict stands: IDEAS-ONLY — take the technique, not the C kernel.

---

## Per-repo findings (full survey — all 16, adversarially verified)

Every repo was cloned, read by an analysis agent, and its port-worthy/production verdict re-checked
by an adversarial verifier. `market-maker-rs`, `bs-p`, `tikr`, and `quant-mm-simulator-rs` were
additionally spot-verified first-hand by the lead session. Sorted by usefulness to a **pure-Rust,
NT-first** consumer.

| Repo | Lang / FFI | Robustness | Port | Core algos (real) | Apparatus | License |
|---|---|---|---|---|---|---|
| **joaquinbejar/market-maker-rs** | Rust / no | **production-grade** | **PRIMARY** | A-S, GLFT, grid, adaptive, depth | OFI, VPIN, intensity, skew, circuit-breaker, drawdown, backtest (all real) | MIT |
| **DaruFinance/quant-mm-simulator-rs** | Rust / no | partial | **PRIMARY** | A-S, GLFT, **Cartea-Jaimungal**, **Ho-Stoll**, microprice, grid, ewma-fair | OFI, skew, backtest; unified `Quoter` trait | MIT |
| **kryptic-sh/tikr** | Rust / FFI? | partial | PRIMARY→**REFERENCE\*** | A-S, GLFT, micro-price, grid, adaptive, depth, ToB | OFI, skew, circuit-breaker, drawdown, backtest; venue-agnostic + **Hyperliquid adapter** | MIT |
| **ssanin82/blockops** | Rust / no | partial | REFERENCE | A-S, inventory-skew, fixed | inventory; live perp platform, IPC bus (~31k LOC) | none |
| **Faraone-Dev/atomic-mesh** | Rust + **C++** / yes | partial | IDEAS-ONLY | A-S, depth, adaptive | OFI, VPIN, skew, CB, DD, backtest; C++17 hot-path | MIT |
| **davidakpele/atomic-mesh** | Rust + **C++** / yes | partial | IDEAS-ONLY | (fork of Faraone — LICENSE still © Faraone-Dev) | same; live path is C++ FFI | MIT (mislabeled) |
| **holypolyfoundation/bs-p** | **C/AVX** + FFI / yes | partial | IDEAS-ONLY | A-S (logit-space) | OFI, skew; bound-by-construction (0,1) | MIT |
| **pranay123/crypto-hft-…-profitable** | C++/Rust/Py / no | educational | IDEAS-ONLY | A-S, grid, adaptive, skew, depth (template-grade) | claimed VPIN/CB/DD; + AI-generated marketing docs | none (README claims MIT) |
| **Jacobo-EG/market-making** | Rust / no | educational | IDEAS-ONLY | GLFT (316-line prototype, Kraken demo) | intensity, skew | none |
| SemiuAdesina/poly-arb-engine-rust | Rust / no | arb-not-mm | SKIP | complete-set arb (not quoting) | none | MIT |
| trevortrinh/exchange | Rust + TS / no | educational | SKIP | LMSR/fixed (demo bots) + self-hosted CLOB | none | none |
| Capataina/Nyquestro | Rust / no | educational | SKIP | none (matching engine + TUI) | OFI only | MIT (Cargo.toml) |
| seyedb/agentic-mm-engine | Rust / no | toy | SKIP | fixed + linear inventory-skew (~392 LOC) | skew, backtest | MIT |
| athiyenarivalagan/hft-market-making-rust | Rust / no | toy | SKIP | none (does not compile) | none | none |
| sqxiao000/rust-make-a-market | Rust / no | toy | SKIP | none (CLI guessing game) | none | MIT |
| HarmishTervadiya/crabex | Rust / no | toy | SKIP | none (toy CEX, 33-line demo bot) | none | none |

**\* tikr port verdict downgraded PRIMARY → REFERENCE for *this* consumer:** tikr is a genuinely good
live venue-agnostic engine, but it brings its **own** execution/risk/runner layer — adopting it
wholesale would dual-path against NT (rule #2: NO DUAL PATHS; NT owns exec/PnL/risk). So tikr is a
**high-value architecture reference** (its venue trait + `RiskGate` + `tikr-hyperliquid` adapter are
the closest existing model for the multi-venue trait design) — study it, don't import it.

**Tiers (for a pure-Rust, NT-first system):**
1. **Pure-Rust math donors (port the math into NT):** `market-maker-rs` (PRIMARY — the apparatus:
   VPIN, intensity, circuit-breaker, drawdown; production-grade, 792 tests, CI, `deny(unsafe)`) +
   `quant-mm-simulator-rs` (PRIMARY — pure-Rust `Quoter`-trait model zoo incl. Cartea-Jaimungal &
   Ho-Stoll that market-maker-rs lacks; + sim/backtest). **Complementary, both MIT.**
2. **Architecture references (study, don't import — NT owns exec):** `tikr` (venue trait + RiskGate +
   Hyperliquid adapter), `blockops` (live perp infra), `atomic-mesh` ×2 (A-S live engines, but C++
   FFI hot-path → can't reuse under PURE-RUST; ideas only). **Note: the two `atomic-mesh` repos are
   the same project — `davidakpele` is a fork; its LICENSE copyright still reads "Faraone-Dev".**
3. **Single technique:** `bs-p` — logit-space A-S for binary (C kernel → reimplement ~5 lines in Rust).
4. **SKIP** (8 repos): arbitrage-not-MM (`poly-arb`), matching engines not strategies
   (`exchange`, `Nyquestro`), and toys/educational scaffolds (`hft-market-making-rust`,
   `rust-make-a-market`, `agentic-mm-engine`, `crabex`, `jacobo` prototype, `pranay123` template).

**License cautions (relevant to "use for profit"):** permissive MIT donors = `market-maker-rs`,
`quant-mm-simulator-rs`, `bs-p`. **No-LICENSE repos** (`blockops`, `exchange`, `hft-market-making-rust`,
`jacobo`, `crabex`, plus `pranay123` README-only claim) are **all-rights-reserved by default — do not
copy code**, reference patterns only.

---

## Shared-framework apparatus taxonomy (build once, reuse every venue)

Each component, who owns it, and the source — **NT first, port only the gaps, bolt-v3 for the edge.**

| Component | Verdict | Source |
|---|---|---|
| Order execution / lifecycle | **NT-OWNS** | NT `execution` + `OrderFactory`; never dual-path (rule #2) |
| Position / PnL tracking | **NT-OWNS** | NT `Portfolio` / `model/position.rs` |
| Pre-trade position/notional limits | **NT-OWNS** | NT `RiskEngine` (`crates/risk/engine`) |
| Backtest engine + Sharpe/Sortino/WinRate | **NT-OWNS** | NT `backtest` + `analysis/analyzer.rs:90` |
| Post-only resting limits | **NT-OWNS** | NT order events `post_only`; bolt-v3 `order_intent.rs` builds them |
| Volatility (ATR/DC) | **NT-OWNS** | NT `indicators/volatility/` |
| Grid quoting + inventory skew | **NT-EXAMPLE → adopt/extend** | NT `grid_mm` (tested, agnostic) — first-cut engine |
| VPIN / toxicity | **NT-EXAMPLE → lift** | NT `hurst_vpin_directional` (`vpin`/`signed_vpin`) |
| GLFT expiry-aware quoting (dynamic γ) | **PORT** | `market-maker-rs glft.rs` / `quant-mm-simulator-rs glft.rs` |
| A-S / Cartea-Jaimungal / Ho-Stoll / micro-price | **PORT (choose per instrument)** | `quant-mm-simulator-rs` (`Quoter` trait, widest zoo) |
| Intensity calibration (k from fills) | **PORT** | `market-maker-rs intensity.rs:418` + `calibration.rs:591` |
| Runtime circuit-breaker FSM | **PORT** (NT risk is pre-trade only) | `market-maker-rs circuit_breaker.rs:41` |
| Equity-curve drawdown monitor | **PORT** | `market-maker-rs drawdown.rs:62` |
| Inventory-aware size scaling | **PORT (optional)** | `market-maker-rs limits.rs::scale_order_size` |
| Order-flow / OFI | **PORT (optional)** | `market-maker-rs order_flow.rs:364` |
| Logit-space (0,1) quoting (binary) | **REIMPLEMENT ~5 lines** | technique from `bs-p kernel.c:234` (do not link C) |
| **Oracle fair-value mid (the edge)** | **bolt-v3 OWNS** | `binary_oracle_edge_taker.rs:5885 compute_fair_probability_up` |
| Realized-vol estimator | **bolt-v3 (hoist to shared)** | `binary_oracle_edge_taker.rs:791` (currently taker-local; see #451) |
| Archetype binding / submit-admission / decision-evidence | **bolt-v3 OWNS** | `bolt_v3_archetypes/mod.rs`, `…strategy_registration.rs` |

The PORT set is small, pure-math, unit-testable, and **pure-Rust + MIT** from two donors — satisfying
NO-HARDCODE (all params TOML), PURE-RUST (no FFI), NO-DUAL-PATHS (NT owns exec/PnL/risk/backtest).

---

## Best-of-breed quote engine per instrument

Shared apparatus everywhere (above); the **quote engine** is chosen per instrument. Specialized beats
generalist where one exists; NT-native beats a port where NT has it.

| Instrument | First-cut engine | Specialized upgrade | Key tuning / venue signal |
|---|---|---|---|
| **Polymarket binary** | NT **`composite_market_maker`** (signal = oracle fair-value) or `grid_mm` + (0,1) Instrument price-band | **bs-p logit-space A-S** (Rust reimpl, bound-by-construction) **or GLFT** w/ terminal penalty (expiry flatten) | γ, k; VPIN toxicity gate on oracle-delta; **reward-aware spread** (Polymarket liquidity-rewards quadratic score); skip depth-based (thin books) |
| **Hyperliquid perp** | NT `grid_mm` + A-S/GLFT (`market-maker-rs`) | depth-based / adaptive-spread (deeper books) | k calibrated from fills; **funding-rate** as inventory-skew bias; ref: `tikr-hyperliquid` adapter, HL `basic_adding.py` |
| **CEX spot/futures** | classic **A-S / GLFT** (`market-maker-rs`) — native habitat | depth-based + adaptive-spread (L2 depth) | maker-rebate-aware spread; latency; rolling-horizon (no terminal) |
| **Perp DEX (dYdX / Drift)** | oracle-anchored quoting (Drift floating-maker pattern); dYdX v4 CLOB ≈ CEX | A-S/GLFT + funding bias | funding; oracle drift re-center; maker-rewards params (dYdX `v4-chain` rewards module) |
| **AMM perp DEX (GMX)** | *not CLOB* — LP/keeper model, **different strategy class** | — | deferred: pool deposit/withdraw + keeper exec, not resting quotes |

**Sequencing recommendation:** start binary (NT `grid_mm` + oracle mid + (0,1) band) → graduate to
GLFT for expiry flattening → reuse the same apparatus on Hyperliquid perp (add funding bias) → CEX →
dYdX/Drift. AMM venues (GMX) are a separate LP strategy, out of scope for the quoting framework.

---

## bolt-v3 integration map (framework anchors, verified at HEAD 2026-05-30)

The shared framework already exists in bolt-v3 — the maker work plugs a quote engine into it.

- **Second-archetype extension point:** `src/bolt_v3_archetypes/mod.rs:18` (comment: "When a second
  archetype is introduced…"), `VALIDATION_BINDINGS:36`, `RUNTIME_BINDINGS:41`. Core validation does
  not change — a maker archetype adds one module + two binding entries.
- **Existing edge (oracle fair-value):** `src/strategies/binary_oracle_edge_taker.rs` —
  `compute_fair_probability_up:5885` (N(d2) digital pricer) → `standard_normal_cdf:5912`;
  `compute_worst_case_ev_bps:5945`; `choose_entry_side:5980`; `choose_robust_size:6008`.
  *Maker mid = oracle fair-value (not a trade-derived mid) is the defining edge over a generic MM.*
- **Volatility signal exists but is NOT yet shared:** `RealizedVolEstimator` at
  `binary_oracle_edge_taker.rs:791` (per-venue map `:945`, config-driven `:1309`). It lives **inside
  the taker strategy**, so a shared-framework maker would need it **hoisted** out (aligns with
  **#451** "extract generic Bolt order admission/submission wrapper", OPEN/P1). *Correction to #488's
  "runtime seed already carries realized_volatility" — the estimator exists, but seed-level hoisting
  is not yet confirmed and is implementation work.*
- **Order-intent post-only limits:** `src/bolt_v3_order_intent.rs` — `is_post_only` field (`:23`,
  `:48`), validated against order type → resting maker quotes are already expressible.
- **Binary pair model:** `src/bolt_v3_market_families/updown.rs:52` (`KEY="updown"`, YES/NO pair).
- **Maker-order surface specs:** `specs/022-nt-maker-order-scope`, `specs/023-nt-order-intent-layer`,
  `specs/023-nt-research-analytics-platform` (backtest/analytics).

---

## Additional resources (beyond the 16 — scout-verified, URLs resolve)

**Foundational literature (the math the donors implement):**
- Avellaneda & Stoikov 2008, *High-frequency trading in a limit order book* — `https://people.orie.cornell.edu/sfs33/LimitOrderBook.pdf` (open author PDF; the DOI is paywalled). Defines reservation price + optimal spread + `λ=A·e^(−kδ)`.
- Guéant, Lehalle & Fernández-Tapia (GLFT), *Dealing with the Inventory Risk* — `https://arxiv.org/abs/1105.3115`. Closed-form, infinite-horizon-stable; the model you most likely run in production.
- Cartea-Jaimungal & Ho-Stoll models — already implemented in `quant-mm-simulator-rs` (pure-Rust reference).

**Calibration / tuning (the real profit loop):**
- **hftbacktest** (nkaz001, MIT, 4.1k★, Rust+Py) — `https://github.com/nkaz001/hftbacktest`. **Best calibration code reference:** measures order-arrival intensity, fits `λ(δ)=A·e^(−kδ)` by log-linear regression, rolling σ, GLFT half-spreads; queue-position + latency backtest. Tutorial: *GLFT Market Making Model and Grid Trading* — explicitly warns the global intensity fit is inaccurate near the touch (refit on the resting-depth range).
- Faustian Dreams, *A-S parameter calibration* — `https://faustiandreams.github.io/2022-10-03/avellaneda-stoikov-calibration` (deriving A,k from time-to-fill; pitfalls).
- **Polymarket liquidity-rewards** — `https://docs.polymarket.com/market-makers/liquidity-rewards`. The quadratic spread score `S=((v−s)/v)²·b`, one-sided penalty, two-sided requirement in tails — **reward-aware spread shaping** for the launch venue.
- Hummingbot *Guide to the A-S Strategy* — `https://hummingbot.org/blog/guide-to-the-avellaneda--stoikov-strategy/` (γ/κ/η operator knobs; finite- vs infinite-horizon caveat).

**Production OSS (reference architecture, not dependencies):**
- **Hummingbot** (Apache-2.0, 18.7k★, 50+ venues) — `https://github.com/hummingbot/hummingbot`. The reference for the **multi-venue connector-normalization** problem (CLOB-CEX / CLOB-DEX / AMM-DEX taxonomy) — exactly bolt's fan-out.
- passivbot (Unlicense, multi-venue grid/martingale, **Rust orchestrator + Python connectors**) — `https://github.com/enarjord/passivbot`.
- binance-rs (MIT/Apache, 854★) — `https://github.com/ccxt/binance-rs` (CEX connector; Futures still "under development").

**Per-venue SDKs (for the fan-out — mind the licenses):**
- Hyperliquid — `https://github.com/hyperliquid-dex/hyperliquid-python-sdk` (MIT, `examples/basic_adding.py` resting-order MM).
- Drift — `https://github.com/drift-labs/keeper-bots-v2` (Apache-2.0; **JIT + floating-maker** bots) + `protocol-v2` SDK.
- dYdX v4 — `https://github.com/dydxprotocol/v4-clients` (**AGPL-3.0 + ToS — copyleft, review before linking**) + `v4-chain` (maker-rewards params).
- Vertex — `https://github.com/vertex-protocol/vertex-rust-sdk` (**native Rust**, but **no LICENSE file** — confirm terms).
- GMX V2 — `https://github.com/gmx-io/gmx-synthetics` (**BSL-1.1, production-restricted until ~Aug 2026**; AMM/keeper model, not CLOB).
- Aevo — `https://github.com/aevoxyz/aevo-sdk` (stale ~2yr, no license).

**NautilusTrader-native MM (use first):**
- **NT pinned rev `6e059dc` (re-verified at the pin):** Rust example strategies `grid_mm`, `hurst_vpin_directional`, **`composite_market_maker`**, `delta_neutral_vol`, `ema_cross` under `crates/trading/src/examples/strategies/`. **`composite_market_maker` is the best NT-native base for the oracle-anchored maker** — `inventory_skew_factor` + a `signal_skew_factor` on an external signal residual (`config.rs:38-56`); point the signal at the oracle fair-value.
- **NT develop / Python (reference only, not pure-Rust):** `examples/backtest/polymarket_simple_quoter.py` (**Polymarket binary quoting** pattern); `bitmex_grid_market_maker.py`; grid-MM tutorials for **BitMEX & dYdX** (`https://nautilustrader.io/docs/latest/tutorials/grid_market_maker_bitmex/`).
- NT backtesting fidelity — `https://nautilustrader.io/docs/latest/concepts/backtesting/` (`FillModel.prob_fill_on_limit` queue-position; L2/L3 matching — *fill realism IS the MM backtest*).
- NT `PortfolioAnalyzer` — Sharpe/Sortino now Rust-native (`crates/analysis/src/statistics/`).

**Historical L2 data (for fill-realistic backtests):**
- Tardis.dev — `https://nautilustrader.io/docs/latest/integrations/tardis/` (**first-party NT adapter**; tick-level L2 since 2019; `tardis-machine` replay is MPL-2.0 OSS).
- Crypto Lake (`https://crypto-lake.com/`, 20-level depth) and Kaiko (`https://www.kaiko.com/products/l1-l2-data`, 100+ venues incl. DEX, SOC-2) — secondary/compliance-grade.

---

## Recommended build path (bottom line)

1. **Framework = NT + bolt-v3, not a new invention.** NT owns exec/PnL/pre-trade-risk/backtest/
   Sharpe-Sortino; NT `grid_mm` is the first-cut MM engine; NT `hurst_vpin` is the toxicity signal;
   bolt-v3 owns archetype binding / submit-admission / decision-evidence / order-intent **and the
   oracle fair-value mid (the edge)**.
2. **Port a small pure-Rust gap set** (both MIT, unit-test each): from `market-maker-rs` — intensity
   calibration, runtime circuit-breaker FSM, drawdown monitor; from `quant-mm-simulator-rs` — GLFT +
   Cartea-Jaimungal/Ho-Stoll via its `Quoter` trait. Nothing else gets imported.
3. **Swap only the quote engine per instrument** (matrix above) — binary → perp → CEX → dYdX/Drift;
   AMM-DEX (GMX) is a separate LP strategy, deferred.
4. **First PR (smallest viable, = #488 re-framed NT-first):** `binary_oracle_maker` archetype = NT
   grid + oracle-anchored mid + (0,1) Instrument price-band + circuit-breaker; prove quotes stay in
   (0,1) and the kill-switch fires in an NT backtest; then layer NT-VPIN + ported intensity calibration.
5. **Reference, never import:** `tikr` (multi-venue venue trait + Hyperliquid adapter), Hummingbot
   (connector normalization), hftbacktest (calibration). FFI engines (`atomic-mesh` ×2, `bs-p` kernel)
   — take the idea, reimplement in Rust (PURE-RUST rule).
6. **Track:** update #488 to this multi-venue + NT-first framing (it currently assumes wholesale
   market-maker-rs port + "NT ships no apparatus" — both superseded here).

---

## Open questions / risks

- **Adverse selection** is the primary MM risk on every venue; the shared defense is VPIN/order-flow
  toxicity (ported) + venue-specific fast signals (oracle delta on Polymarket, funding on perps).
- **Calibration is only proven on live data** — γ/k/spread tuning is the real optimization loop
  after the port; backtest first.
- **Pure-Rust constraint** excludes FFI engines from direct reuse (e.g. a C/AVX kernel must be
  reimplemented in Rust, not linked).
- **Per-venue connectors** — bolt-v3 ships Polymarket + a Binance provider; Hyperliquid / perp-DEX
  connectors are net-new (NT adapter or custom). Tracked separately from this strategy survey.

---

## Options-implied vol for fair value — venue + cadence fit (investigated 2026-05-30, verified)

The binary fair-value pricer's only uncertain input is **σ**. Options-implied σ is an alternative to
bolt's realized σ — but only where a *listed option matches bolt's tenor*. Hard findings:

**Shortest listed option tenor — Deribit / OKX / Binance are all DAILY (08:00 UTC settle); none list
intraday / 0DTE / hourly options.** NT adapter exposure (pinned rev `6e059dc`): **Deribit CONFIRMED**
(`subscribe_option_greeks` → `OptionGreeks{mark_iv,bid_iv,ask_iv,δ,γ,ν,θ,ρ}` + DVOL custom data),
**OKX CONFIRMED** (opt-summary feed, no ρ, no native vol index), **Binance EAPI NOT IMPLEMENTED**
(adapter is a stub — has the data via `/eapi/v1/mark` + BVOL but NT would have to build the path).

**Cadence fit vs bolt's 1m–4h:**
| bolt cadence | options-IV usable? |
|---|---|
| **1h, 4h** | **yes** — tenor-matched (esp. in the hours before the 08:00 UTC daily expiry); digital fair value readable **model-free off the smile** via Breeden-Litzenberger `P(up@K)=−∂C/∂K` |
| **15m** | borderline — only near a daily expiry; else fragile term extrapolation |
| **1m, 5m** | **no** — no listed option near tenor; T→0 extrapolation (θ/γ blow-up); realized-vol is more honest |

**What to collect (Deribit primary, OKX secondary):** front-daily smile (mark+bid+ask IV + δ/γ/ν/θ)
across strikes around spot + per-strike mark prices (→ model-free digital `−∂C/∂K`) + index/perp price
+ DVOL/BVOL as a 30-day regime feature.

**Bottom line:** options-IV improves fair value **only for 1h/4h markets** (15m near expiry). For the
default **5m** markets it is *not* a usable input — keep realized-vol. The most accurate route where
tenor matches is the **model-free digital from the smile (Breeden-Litzenberger), not a BS+single-σ
plug-in.** This is a *shared* fair-value upgrade (benefits the taker too), gated on trading 1h/4h
cadence — orthogonal to, and not a blocker for, the maker (which starts on the existing mid).

---

## Provenance

- 16 repos cloned `--depth 1` into `var/mm-research/clones/` and read by per-repo analysis agents;
  every port-worthy / production-grade verdict re-checked by an adversarial verifier.
- `market-maker-rs` additionally read first-hand by the lead session (anchors above).
- bolt-v3 anchors re-grepped at working-tree HEAD 2026-05-30 (line numbers current; #488's were
  stale by ~580 lines on the edge-taker file, as #488 warned).
- External-repo line numbers cite cloned HEAD 2026-05-30 and may drift; module/function names are
  authoritative.
