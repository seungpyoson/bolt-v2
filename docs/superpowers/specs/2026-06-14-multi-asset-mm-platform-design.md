# Multi-Asset Market-Making Platform — Architecture Design (Grounded)

> **Status:** implementation-ready design. Grounded against `origin/main`
> (`e2c726c25a7560ffe67221b8ee251d500685a08d`) and worktree HEAD
> (`0bf6f2f7e447cb63aef713e0fb866c75120984ed`), with NautilusTrader pinned at
> `6e059dcbb59ac1e582132fc431a581936c216c3c` (`Cargo.toml:25-46`). Every
> component below carries a build-vs-reuse tag and a file:line anchor. The
> grounding pass (13-agent investigation + adversarial verification, 2026-06-14)
> corrected several assumptions; those corrections are surfaced inline and again
> in §14 (Contradictions) / §15 (Open Gaps). PARTIAL adversarial verdicts are
> not papered over.
>
> This is the **agnostic-platform architecture**. The detailed binary-maker
> requirements live in `specs/488-binary-oracle-maker/spec.md` (FR-001..FR-081);
> this doc references those FRs rather than restating them.

## 1. Purpose & Scope

Build a **shared market-making engine on NautilusTrader** that hosts **per-asset
slots** (pricing model + settlement), proving the framework with the **binary
up/down outcome token as the first instrument**, then extending to perps / spot
/ options / RWA by filling the two slots — without rewriting the engine.

The binary maker exists to answer the one question the lead-lag research left
open: can *our* implementation capture the maker edge without being picked off,
net of fees, adverse selection, and settlement? The edge is not in question;
robustness is. Go-live is gated on a backtest of the maker **as built** on real
historical full-depth L2 (FR-001).

In scope for instruments: prediction-market binaries (GM/CG — first), crypto
perps, crypto/equity spot, options, equities/RWA perps. This doc covers the
engine/slot architecture, the component build-vs-reuse map, the risk design, the
sourcing policy, market selection, settlement, the venue contract, the backtest
gate, and the multi-asset extension path. §13 separates what is settled (build
now) from what genuinely needs runtime/backtest data.

## 2. Agreed Architecture Decisions (D1–D10)

These were settled one-at-a-time during brainstorming and confirmed (with the
grounding corrections noted) here.

| # | Decision | Grounding note |
|---|----------|----------------|
| **D1** | **Slot boundary = medium.** One shared engine on NT + two per-asset slots (pricing model, settlement). Five shared parts on NT: market-data, fair-value, quote-lifecycle/placing, inventory, risk. | The slot seam **already exists in code** — the family binding carries `fair_probability_up` + `maker_quote_targets` + `maker_settlement_payout` fn pointers per family (§3, surprise). |
| **D2** | **Engine↔model = advisor.** The model is a pure function returning TARGET quotes (price/size/ttl); the shared engine reconciles target→live and does all placing/cancel. No side effects → backtestable; one backtest grades every asset. | Confirmed compatible: GM/CG model is pure tested math, UNWIRED (no NT type, no order placement). |
| **D3** | **Defense = shared danger detectors → per-asset model reacts.** Detect shared (book-imbalance + lag + signed-flow), react in-model (widen/skew/pull). | **CORRECTED:** NT's signed-VPIN is example-only (not reusable); the signed-flow/μ detector is **NET-NEW build**, not an NT consume. Book-imbalance + lag *can* use NT primitives. |
| **D4** | **Risk = 3 evidence-backed problems → 3 responses**, collapsing to Normal/Defensive/Stop. Optional 4th = single hard total-loss backstop. The "5-rung ladder" is rejected as convention. | All 3 risks confirmed real; pick-off is **empirically measured**. 4th backstop has a trigger type but **no firing code** (gap). |
| **D5** | **Sourcing = NT-first.** Binary GM/CG is built; port ONLY genuine residue NT lacks, only for in-scope assets, each re-verified at file:line, hardcodes→TOML, panics→Result. Never blanket-port. | Confirmed NT gaps (A-S/GLFT/intensity-k, signed-VPIN, position cap, vol-surface). A-S/GLFT are MUST-NOT for binary. |
| **D6** | **Market selection = mechanism + config.** Ship "B" (auto-discover + eligibility filter + concurrency cap + auto-rotate) now; layer "C" (edge-ranking) later. → **Task #1.** | "C" needs a per-market profitability estimate that does not exist until a backtest produces one. |
| **D7** | **Settlement = per-asset slot**, triggered by the venue resolution signal; books payout + resets inventory. | **CORRECTED:** `InstrumentStatus::Close` is the *notification*, not NT's settlement mechanism. NT does **not** auto-compute the 0/1 payout — the slot books it explicitly. |
| **D8** | **Venue capability contract = config-driven** (`src/venue_contract.rs`); no venue hardcoding. | **CORRECTED:** the contract carries rate-budget / modify-vs-cancel / settlement-kind / fee / depth / streams — **NOT** tick/lot (those come from NT instrument metadata). Schema extension (W1) is **already merged**. |
| **D9** | **Backtest gate = whole loop on historical data** as the go-live gate; realistic passive-fill modeling. | **CORRECTED:** NT ships a native queue-position drain model; per FR-003 a custom FillSim is a **contingent fallback**, not a required port. |
| **D10** | **Standing principles:** always research NT first; never blanket-port; every control maps to a real problem or is cut; problem-first, not convention-first. | — |

## 3. Architecture Overview

**One shared engine on NT + two per-asset slots (D1):**

- **Slot 1 — Pricing model** (per asset): a *pure advisor function* returning
  TARGET quotes (price/size/ttl). Binary = GM/CG. No side effects →
  backtestable in isolation; one backtest harness grades every asset (D2).
- **Slot 2 — Settlement** (per asset): books the terminal payout and resets
  inventory, triggered off the venue resolution signal (D7).

**Five shared parts on NT:** market-data, fair-value, quote-lifecycle/placing,
inventory, risk.

**Data-flow loop (advisor pattern, D2):**

```
NT market data (QuoteTick/TradeTick/OrderBookDelta, each ts_event+ts_init)
   │
   ├─► [shared] fair value  ──► fair_probability_up (p)        [family seam]
   ├─► [NET-NEW] signed-flow / VPIN estimator ──► informed-fraction (μ)
   ├─► [partly NT] danger detectors (book-imbalance + lag check) ──► danger level
   │
   ▼
[per-asset MODEL = pure advisor]  gm_binary_quote(p, μ) + inventory_skew(net, …)
   │  returns TARGET quotes (price/size/ttl), reacts to danger (widen/skew/pull)
   ▼
[shared] quote-lifecycle controller  reconciles target → live
   │  cancel+resubmit (venue lacks modify), throttle to rate budget, track resting set
   ▼
NT order submission (RiskEngine notional cap + submit-rate limit) → venue
   │
   ▼
fills ──► [shared] inventory (NT Portfolio.net_position + maker accumulator)
resolution (InstrumentStatus::Close) ──► [per-asset] SETTLEMENT slot books 0/1 payout, resets inventory
```

The model is a **pure function** (no NT type, no order placement). The **engine
reconciles target → live** and does all submit/cancel. This is what makes the
model backtestable in isolation and lets one backtest harness grade every asset.

## 4. Component-by-Component (build-vs-reuse map)

### Pricing model — binary (GM/CG) — **[ALREADY-BUILT, UNWIRED]**
- `src/bolt_v3_maker_model.rs` (identical to `origin/main`; `git diff
  origin/main` empty). Doc header names Glosten-Milgrom / Copeland-Galai
  (`:1-34`). Public fns: `gm_binary_quote(fair_p_up, informed_fraction)` (`:61`),
  `gm_half_spread(…)` (`:91`), `inventory_skew(net_position, skew_gain,
  position_cap)` (`:112`). **11** `#[test]` fns.
- **Units-correct** for a bounded (0,1) binary: outputs are Bayesian-posterior
  ratios structurally confined to (0,1); μ=0 collapses the spread, μ=1 opens to
  [0,1]. A-S/GLFT are a units category error here and a hard MUST-NOT in the
  ratified spec (FR-020; framing overturned by code audit). Adversarial verdict:
  **REAL**.
- **WIRED status: UNWIRED.** No `src/strategies/` or `bolt_v3_live_node.rs`
  import. Pure tested math awaiting a maker archetype to wire it.

### Fair value (p) — **[ALREADY-BUILT; NT-backed path planned]**
- `src/bolt_v3_market_families/updown.rs:1102` `fair_probability_up` =
  Black-Scholes digital N(d2) (`d2` at `:1128-1131`), hand-rolled. Live-consumed
  by the taker via the family seam.
- **FR-070**: the hand-rolled digital MUST be removed once NT greeks/IV is
  authoritative. NT *provides* the replacement: `black_scholes_greeks_exact` +
  `imply_vol` (`crates/model/src/data/greeks.rs:142,192,238`), `itm_prob`
  directly usable as the binary win probability. Planned consolidation, not a
  current gap.

### Realized-vol / FV stack — **[ALREADY-BUILT, LIVE-CONSUMED]**
- `bolt_v3_realized_volatility{,_runtime}.rs`, wired through
  `StrategyBuildContext` (`registry.rs:98-108`); feeds `fair_probability_up`.
  There is **no separate "FV combiner" module** — combination is inline in
  `bolt_v3_taker_pricing.rs`. RV subscribes via config (`spot_quote`/`midpoint`);
  trade subscription is config-gated and inactive today (see μ, below).

### Market-family seam — **[ALREADY-BUILT, MAKER HOOKS PRESENT]** — *load-bearing surprise*
- `src/bolt_v3_market_families/mod.rs`: the binding struct already carries
  **three maker fn pointers**: `fair_probability_up` (`:85`),
  `maker_quote_targets` (`:86`), `maker_settlement_payout` (`:87`).
- **Two families already registered**, both implementing all three hooks:
  `updown` (`:278-288`) and `hyperliquid_instrument` (`:291-…`). Dispatcher
  `fair_probability_up_for_family` (`:432`).
- **Implication for D1/D2/D7:** the per-asset *pricing-model* and *settlement*
  slots already have a typed home at the family layer. New families add to
  `VALIDATION_BINDINGS` without touching the dispatcher. The slot seam exists in
  code — stronger than the assumed architecture.

### Quote-lifecycle / placing — **[ALREADY-BUILT (core) + IN-PROGRESS (pipeline), all UNWIRED]**
- On `origin/main`: `bolt_v3_quote_lifecycle.rs` (NT-free single-leg state
  machine), `bolt_v3_requote_budget.rs` (requote-rate throttle),
  `bolt_v3_maker_microprice.rs`, `bolt_v3_maker_event_fence.rs`.
- **Six pipeline files** (`bolt_v3_maker_{quote_plan,quote_control,quote_set,
  order_plan,order_compile,order_dispatch}.rs`): committed on another branch
  (commit `e002139d3`), absent from `origin/main`, staged in this worktree.
  Real in-progress work, not stale artifacts.
- **WIRED status: NONE.** FR-010..FR-013: cancel+resubmit reprice (no venue
  modify), throttle to a *capability variable* (not a constant), track the full
  resting set + reconcile against accepted-order truth, requote-on-post-only-reject.

### Inventory — **[NT-PROVIDES tracking] + [ALREADY-BUILT accumulator, UNWIRED]**
- NT Portfolio is **Rust-native**: `crates/portfolio/src/portfolio.rs:1170
  net_position`, `:426 unrealized_pnls`, `:475 realized_pnls`. Inventory source
  of truth.
- `src/bolt_v3_maker_inventory.rs` `MakerInventory` (`:11`) holds a
  `net_position` accumulator (`apply_fill` `:29`), UNWIRED; its boundary against
  NT Portfolio is undecided (§15 open gap; recommend NT Portfolio as truth).

### Risk infrastructure — **[NT-PROVIDES, mostly] + [NET-NEW residue]**
- **NT provides** (reuse, do not rebuild): pre-trade **notional-per-order** cap
  (`crates/risk/src/engine/config.rs:44`, enforced `engine/mod.rs:1186-1192`),
  submit/modify rate limits, **TradingState{Active,Halted,Reducing}**
  (`crates/model/src/enums.rs:1928`) with halt→deny and Reducing→block-exposure-
  increasing (`risk/src/engine/mod.rs:1707-1730`), `cancel_all_orders`.
- **NT does NOT provide:** a **position/inventory-level cap** — `RiskEngineConfig`
  has only `max_notional_per_order`, no `position_limit`/`max_position`. bolt's
  `inventory_skew` hard cap (`bolt_v3_maker_model.rs:119`) is exactly this
  residue.
- **Adversarial verdict PARTIAL (surfaced):** "NT provides VPIN + position
  limits — don't rebuild any" is REFUTED on the universal "any": NT's VPIN is
  not a reusable component, and NT has no position cap.

### Danger detectors — **[partly NT] + [NET-NEW]**
- **BookImbalanceRatio** IS a reusable NT indicator
  (`crates/indicators/src/book/imbalance.rs:32`) — but **symmetric [0,1]**, not
  signed/directional. Usable as a raw input, not a VPIN substitute.
- **VPIN/signed_vpin** exists in NT only as `pub(super)` fields inside a
  **feature-gated example strategy** (`crates/trading/src/examples/strategies/
  hurst_vpin_directional/strategy.rs:60-61`, behind `#[cfg(feature="examples")]`),
  hand-rolled. `grep vpin crates/indicators/` = 0. A production maker cannot
  consume it; bolt must build/extract its own **signed-flow / VPIN estimator**
  to supply μ (FR-021). **[NET-NEW]**

### Informed-fraction (μ) + signed order flow — **[NET-NEW]**
- GM/CG and VPIN both need signed order flow. FR-021: the maker MUST subscribe
  to trades and classify aggressor side (no test-fixture reliance). The taker's
  `on_trade` handler + `SignedTradeFlow` buffer exist but fire only when a
  `(Trade, Trade)` RV source is added to config — none deployed today, so μ is
  starved. Activating the subscription is config-driven; the **aggressor-side
  classifier + μ estimator feeding GM/CG is net-new build**.

### Live registration / fence path — **[NET-NEW restructuring]** — *PARTIAL verdict, surfaced loudly*
- The dependency-direction fence scans **every `src/bolt_v3_*` file**
  (`scripts/verify_bolt_v3_dependency_direction.py:90,759`); its allowlist is
  **shrink-only vs origin/main** (adding an entry fails CI, `:908-944`).
- `bolt_v3_live_node.rs` **IS under `src/bolt_v3_`** (refutes the earlier "clean
  one-liner" claim). `RUNTIME_BINDINGS` is a const slice naming the taker binding
  inside scanned `bolt_v3_archetypes/mod.rs:65`; `runtime_bindings()` (`:71`)
  feeds `register_bolt_v3_strategies_on_node_with_bindings(…)` at
  `live_node.rs:1880-1884` — both scanned.
- The naive "copy the taker archetype" path emits NEW
  `crate::strategies::binary_oracle_maker::*` references inside scanned files →
  NEW allowances → **blocked by shrink-only.** Live registration is **NOT
  routine wiring.**
- **Clean path (a deliberate design call, Rule 11):** keep the maker's
  `StrategyBuilder` + register fn in `src/strategies/binary_oracle_maker/` (not
  scanned), and make the binding slice **injectable** — hoist its construction
  out of `bolt_v3_archetypes/mod.rs` into a non-scanned caller
  (`main.rs`/`build_bolt_v3_live_node`) and pass it to
  `register_bolt_v3_strategies_on_node_with_bindings`, which already accepts
  `bindings: &[StrategyRuntimeBinding]` (`bolt_v3_strategy_registration.rs:108-112`).
  `production_strategy_registry()` (single taker today, `src/strategies/mod.rs:8-12`)
  gains a `register::<BinaryOracleMakerBuilder>()` line; that file is not scanned.
- **Source-integrity (GOLDEN digest)** is a sanctioned maintenance flow: if the
  maker source must be tamper-gated, add a `GatedSourceRoot` in
  `source_canonicalization.rs` (`STRATEGY_KEY` roots `:561-570`) and re-derive
  `GOLDEN_STRATEGY_DIGEST` (`bolt_v3_source_integrity.rs:287`). Not laundering.

### Position sizer / kill switch — **[ALREADY-BUILT, LIVE on taker]**
- `src/bolt_v3_sizing.rs choose_robust_size` (EV-fraction→notional), consumed by
  the taker. `bolt_v3_kill_switch{,_store}.rs` state machine gating submissions
  via `bolt_v3_submit_admission.rs` (the consolidated chokepoint). The maker
  reuses this chokepoint.

## 5. Risk Design (D3, D4)

Three problems → three responses, collapsing to **Normal / Defensive / Stop**
(finer gradations are config thresholds, not new machinery). Optional 4th = a
single hard total-loss backstop. The "5-rung ladder" is rejected as convention
(D4) — note the grounding pass found **no repo artifact named "5-rung ladder"**;
treat D4 as a design stance, and the spec actively contraindicates multi-level
quoting for thin binary books.

| # | Problem | Response | Evidence / confidence |
|---|---------|----------|------------------------|
| 1 | Stale / bad feed | **STOP (fail-closed)** | Taker pattern proven: `evaluate_forced_flat_predicates` pushes `StaleReference` (`exposure.rs:385-400`), `is_none_or` defense-in-depth. The only live run (Jun 3-6) priced **0 / 137,157** evals, all `ForcedFlat(StaleReference)` because the reference feed was never connected (`docs/research/leadlag-subsecond-fillability-2026-06-10.md:186-189`). **CONFIRMED**; maker-side gate **absent** — must replicate the taker predicate in the maker admission gate (§15). |
| 2 | Pick-off via lag | **WIDEN / PULL (in-model)** | **EMPIRICALLY MEASURED, not assumed:** BTC taker edge **+13.27c/share @1s, 95% CI [+9.68,+16.87]**, n=127 (`leadlag-trades-leader-2026-06-11.md:92`); HL clock fires ~+1s late, 80-100% leader-first. Maker mark-outs: 30s = -0.013c CI[-0.073,+0.047] (spans zero), 60s = +0.141c CI[+0.057,+0.226] (`leadlag-taker-edge-2026-06-10.md:380-381`). Sub-60s adverse selection is statistically inseparable from noise. **CONFIRMED.** GM/CG μ-driven spread is the response. |
| 3 | Inventory into 0/1 resolution | **INVENTORY MGMT (cap + skew/flatten)** | `inventory_skew` hard cap returns None at `\|net\|≥cap` (`bolt_v3_maker_model.rs:119`); FR-040 per-market reserved-collateral worst-case-liability gate; "resting maker is structurally left holding inventory into expiry … settlement is the dominant P&L term" (`spec.md:65`). **Risk CONFIRMED**; actual fill/accumulation rate is **runtime-only** (§13). |
| 4 (opt.) | Runaway total loss | **single hard backstop** | Real, non-redundant: the position cap stops *accumulation* but does not bound *settlement loss on already-held inventory*. `KillSwitchHaltTriggerKind::LossGovernorBreach` + constructor exist (`bolt_v3_kill_switch.rs:45,51,61`) but **no code computes running P&L and fires it** (§15 gap). The graduated governor FR-023 (cancel-only / reduce-only / hard-flat / soft-hold) is the spec's response. |

**Governor shape (FR-023):** graduated states, not a boolean; kill predicates
(σ-floor, basis-cap, τ-floor, plus existing stale/thin/incoherent) as TOML
thresholds that fail closed.

## 6. Sourcing Policy (D5, D10) — NT-FIRST gate

**Rule:** use NT for everything it provides; the binary model is built; port
ONLY genuine residue NT lacks, only for an in-scope asset, each port re-verified
at file:line, hardcodes→TOML, panics→Result. Never blanket-port.

**Confirmed NT genuine gaps (residue *candidates*, NOT a mandate):**
- A-S, GLFT, Guéant closed-form, arrival-intensity (A,k) calibration — `grep`
  returns zero at 6e059dc. **But A-S/GLFT are MUST-NOT for the binary (FR-020);
  deferred to perps/longer tenors only.**
- A production signed-VPIN / informed-flow detector (NT's is example-only).
- A position/inventory cap (NT has none).
- SVI / Breeden-Litzenberger volatility-surface tooling (absent from NT *and*
  from the best reference repo `market-maker-rs`; net-new for options).

**Port-source reality check (CANDIDATES, with caveats):**
- `market-maker-rs` A-S closed form is correct & tested
  (`/tmp/mm-ref/src/strategy/avellaneda_stoikov.rs:80-195`) but operates on an
  **unbounded** mid (no [0,1] clamp) — wrong tool for binary, needs adaptation
  for perps. Its GLFT is a terminal-penalty A-S extension, **not** the Guéant
  closed form (no c1/c2/asymptotic intensities). Its `calibration.rs` has **42**
  hardcoded constants (NO-HARDCODES violation) and a weighted-regression/
  unweighted-SE bug. `optionstratlib` is a feature-gated external dep (Rule 5
  concern if enabled).
- **UNVERIFIABLE this session (clones gone):** tikr FillSim "production-grade,"
  DaruFinance GLFT A-term bug, Jacobo-EG γ-bug. These drive the "reimplement
  from paper, don't port" verdict for any future GLFT work — **re-clone and
  re-anchor before authorizing a GLFT port.** None are load-bearing for the
  binary first-instrument. → **Task #2.**

## 7. Market Selection (D6, Task #1)

Ship **"B" now**: shared mechanism + config rules — auto-discover candidate
markets + eligibility filter + concurrency cap + auto-rotate. Layer **"C"
(edge-ranking) later**, once a per-market profitability estimate exists (it does
not yet — requires backtest data). FR-041: a portfolio layer splits capital,
selects markets, isolates per-market state/health/kill. The family seam already
exposes `market_selection_candidate_windows` / `selected_market_requirement` /
`select_binary_option_market` per family (`mod.rs:282-284`) — selection plugs in
there. **Task #1** tracks "C."

## 8. Settlement Slot (D7) — **[NET-NEW per-asset; NT does not auto-settle]** — *PARTIAL verdict, surfaced*

- **NT does NOT auto-compute a binary 0/1 payout.** `BinaryOption.option_kind()`
  returns `None` (`crates/model/src/instruments/binary_option.rs:307`), so it is
  excluded from `process_option_expiry`; the generic path fills at an
  **externally-supplied** `settlement_price`. Adversarial verdict on "NT does
  not auto-settle": **REAL.**
- **Critical correction (D7):** `InstrumentStatus::Close` is **NOT** NT's
  settlement mechanism. `process_status(MarketStatusAction::Close)` only sets
  `market_status = Closed` (`engine.rs:1890-1912`, body read — no payout logic).
  NT settlement is driven by the *distinct* `InstrumentClose` event or timestamp
  expiration. The Polymarket contract marks NT's `InstrumentClose` `unsupported`,
  so the **resolution signal the strategy actually receives is
  `InstrumentStatus::Close`** — a notification the per-asset settlement slot must
  observe and then **explicitly book the 0/1 payout** (FR-030). FR-004 requires
  one shared settlement primitive reused by backtest and live.
- The taker has **zero** resolution handling (no `on_instrument_status` /
  `on_instrument_close` handler — confirmed absent). The maker adds the handler +
  the family `maker_settlement_payout` path (`mod.rs:87`, already typed per
  family).

## 9. Venue Capability Contract (D8) — **[ALREADY-BUILT, schema DONE]** — *surprise*

- `VenueContract` (`src/venue_contract.rs:213`, identical on main) already
  carries: `execution: ExecutionCapabilities` (`supports_modify`), `rate_budget`
  (clob/gamma per-minute, batch limit), `maintenance_window`, `depth_availability`,
  `fee_schedule`, `settlement` (`kind`), `streams`. **The W1 schema extension is
  already merged** — the spec Assumptions (`spec.md:178`) describing it as
  pending are **stale.**
- **CORRECTION (D8):** VenueContract carries **NO tick_size/lot_size** — those
  come from NT instrument metadata at runtime, not the contract. The assumed
  "tick/lot in the contract" is wrong.
- Polymarket facts (config-driven, `contracts/polymarket.toml`): `supports_modify
  = false` (`:13`) → cancel+resubmit; `clob_per_minute = 100`, `gamma_per_minute
  = 100`, `batch_submit_limit = 15` (`:19-21`); `order_book_depths` disabled (L2
  deltas only, no native queue identity).
- **Rate-budget contradiction (loud):** spec Assumptions (`spec.md:177`) claim
  bolt config = `100/second`. Actual: `max_order_submit_rate = "40/00:01:00"`
  (**40/min**) at `config/root.toml:412`. The NT governor caps the maker at
  **40/min**, more restrictive than the venue's 100/min. **Maker requote-budget
  sizing MUST use 40/min**, reconciled against the contract's `clob_per_minute`,
  whichever is lower (FR-011).
- FR-080: the controller MUST read modify/budget/maintenance/depth/fee/
  settlement-kind as variables and MUST NOT branch on a venue name.

## 10. Backtest Go-Live Gate (D9)

- FR-001: before live capital, produce a backtest of the **built maker** scored
  net = captured-spread − fees − adverse-selection − settlement-loss over a real
  historical full-depth L2 window, against thresholds registered **before** the
  run, PASS/FAIL.
- FR-002: corpus = unbiased full-candidate set (not the entry-only
  decision-evidence log).
- FR-003: MUST use **NT's ExecutionModel** for fills; a custom fill model MUST
  NOT be introduced unless NT's is first source-proven insufficient (no dual
  fill truth).
- **Fill-realism caveat:** NT's `DefaultFillModel` fills all limit orders at
  touch (prob=1.0, no queue) — optimistic for thin binary books. BUT NT also
  ships a **native queue-position drain model** (`queue_ahead`/`qty_ahead`,
  config-gated `config.rs:45 queue_position:bool`) — precisely the "FillSim"
  capability. So per FR-003 the order is: **first enable/source-prove NT's
  queue-position model**; only if proven insufficient does a custom FillSim
  become a candidate port. **FillSim is a contingent fallback, not a required
  port** (refutes the earlier "needs FillSim" minimization).
- FR-005: underlying spot for the window must be backfilled and point-in-time
  aligned to the oracle the maker saw (look-ahead controlled).

## 11. Multi-Asset Extension

- **Binary first** to prove the framework (built). Then each new asset fills the
  two slots behind the **MarketFamily seam** (already carrying `fair_probability_up`
  + `maker_quote_targets` + `maker_settlement_payout` per family; FR-081). A
  second family (`hyperliquid_instrument`) is already registered, demonstrating
  the path.
- **Perps / spot:** A-S/GLFT become *in-scope* here (deferred from binary per
  FR-020) — but only via reimplement-from-paper after re-anchoring the port
  sources (§6).
- **Options:** known **net-new gap** — a volatility **SURFACE (SVI/SABR)** +
  Breeden-Litzenberger, **absent from NT and from `market-maker-rs`** (confirmed
  for the two most relevant codebases; absence from the other 14 surveyed repos
  is UNVERIFIABLE this session). NT *does* provide per-option BS greeks + Jäckel
  IV inversion (`greeks.rs`), which feed a surface but are not the surface. Flag
  recorded now so it is not a surprise when options come into scope.
- **RWA:** fills the same two slots; no specific evidence gathered this session.

## 12. Standing Principles (D10)

Research/investigate NT first; never blanket-port; every control maps to a real
problem or is cut; problem-first, not convention-first. Each port re-verified at
file:line, hardcodes→TOML, panics→Result. NT-first strip mandate: if NT offers
it, use NT's and strip ours (e.g., FR-070 hand-rolled digital removal once NT
greeks/IV is authoritative).

## 13. Tracked Tasks

- **Task #1 — Market selection:** ship "B" (auto-discover + eligibility +
  concurrency cap + auto-rotate); "C" edge-ranking deferred until a backtest
  profitability estimate exists (§7).
- **Task #2 — Sourcing/residue ports:** NT-first; port only genuine residue for
  in-scope assets; CANDIDATES in §6, each gated on re-verification (and, for
  GLFT, on re-cloning the unverifiable port sources).
- **Untasked, recorded:** options volatility-surface (SVI/SABR + Breeden-
  Litzenberger) — net-new when options come into scope (§11).

These tasks are promoted to tracked GitHub issues at spec+plan time.

## 14. Contradictions surfaced by grounding (do not paper over)

1. **D8 "tick/lot in the venue contract": CONTRADICTED.** VenueContract carries
   no tick/lot — sourced from NT instrument metadata. Contract owns rate_budget /
   modify-vs-cancel / settlement-kind / fee / depth / streams.
2. **D3 "NT provides the danger detectors to consume": PARTIAL/REFUTED.** NT's
   VPIN is feature-gated example-only; BookImbalanceRatio is symmetric [0,1].
   The signed-VPIN/μ estimator is NET-NEW build.
3. **"NT provides position/inventory limits": REFUTED.** NT has only
   `max_notional_per_order`. The inventory cap is bolt's residue (already in the
   GM model).
4. **D7 settlement via `InstrumentStatus::Close` as the *mechanism*: PARTIAL.**
   NT's `process_status(Close)` only flips market_status; the strategy observes
   the notification and books the 0/1 payout itself.
5. **Live-registration as "routine wiring": PARTIAL.** `bolt_v3_live_node.rs` IS
   scanned; copy-the-taker triggers forbidden shrink-only allowances. Clean path
   = injectable binding slice (Rule 11 design call), not a one-liner.
6. **Backtest "needs FillSim": minimization REFUTED.** NT ships a native
   config-gated queue-position model; FillSim is a contingent fallback.
7. **Spec Assumptions are stale on two points:** bolt order budget is 40/min
   (`root.toml:412`), not "100/second"; the VenueContract W1 schema extension is
   already merged. Ground from main, not `spec.md:177-178`.

## 15. Open Gaps (must be addressed in the build, not research)

- **Maker-side stale-feed gate ABSENT:** replicate the taker's
  `evaluate_forced_flat_predicates(StaleReference)` (`exposure.rs:385-400`) in
  the maker admission gate.
- **4th-risk loss backstop has no firing code:** `LossGovernorBreach` + ctor
  exist; nothing computes running P&L against thresholds and fires it. The
  orphaned `loss_governor` TOML is deploy-local and `RiskBlock` has
  `deny_unknown_fields` with no field for it. Firing-code + a parsed config field
  are both net-new.
- **MakerInventory vs NT Portfolio boundary undecided:** pick one source of
  truth before wiring (recommend NT Portfolio for truth; Rule #6 no-dual-state).
- **μ estimator + aggressor-side classifier is net-new** (FR-021); trade
  subscription exists but is config-gated and inactive — μ is starved today.
- **Future-GLFT port verdicts rest on unverifiable claims** (tikr/DaruFinance/
  Jacobo-EG clones gone) — re-clone before authorizing a GLFT port.
- **SVI/BL absence confirmed only for NT + market-maker-rs** (14 repos
  unverified) — re-clone before relying on the universal claim for options.

## 16. Must Resolve Before Implementation (decided now — no research mid-build)

1. **Live-registration shape:** make the `StrategyRuntimeBinding` slice
   injectable from a non-scanned caller (`main.rs` / `build_bolt_v3_live_node`);
   keep the maker `StrategyBuilder` + register fn under
   `src/strategies/binary_oracle_maker/`. Do NOT mirror the taker archetype.
2. **GOLDEN-digest gating:** decide whether the maker source is tamper-gated; if
   yes, add a `GatedSourceRoot` and re-derive the digest, scoped into the maker
   PR up front.
3. **Requote-budget number:** size to **40/min** (`root.toml:412`), reconciled
   against the contract's `clob_per_minute=100`, whichever is lower (FR-011).
4. **Inventory source of truth:** pin NT Portfolio vs MakerInventory before
   wiring (avoid Rule #6 dual-state).
5. **Settlement observation path:** the slot observes `InstrumentStatus::Close`
   and books the 0/1 payout via `maker_settlement_payout`, reusing ONE shared
   primitive across backtest+live (FR-004).
6. **μ source:** config-activated `(Trade,Trade)` RV source + net-new aggressor-
   classifier + VPIN estimator; specify the classification rule (GM/CG cannot run
   without μ).
7. **Loss-backstop wiring:** add a parsed config field (replacing the orphaned
   deploy-local `loss_governor`) and firing code that computes running P&L and
   calls `loss_governor_breach`; thresholds in TOML.

## 17. Implementation-Readiness: Settled vs Runtime-Dependent

**Settled (build now, no further research):**
- GM/CG model built, tested, units-correct, UNWIRED — wire it.
- Family seam + maker fn pointers exist for two families
  (`market_families/mod.rs:85-87`).
- VenueContract schema is DONE; read all venue facts from it; effective order
  budget = **40/min**.
- NT reuse map fixed: Portfolio (Rust-native net_position/pnls), TradingState
  halt/reduce + cancel_all, notional cap, BS greeks/IV. Build: signed-VPIN/μ
  estimator, position cap, settlement-payout booking, graduated governor, loss-
  backstop firing code.
- Settlement hooks on `InstrumentStatus::Close` (notification); strategy books
  0/1 explicitly.
- Live-registration clean path = injectable binding slice + maker code under
  `src/strategies/` (not the taker-mirror archetype).

**Genuinely needs runtime/backtest data (cannot settle from source):**
- Realistic fill rate & one-sided inventory accumulation on Polymarket CLOB
  (core Risk-3 unknown; FR-001 corpus).
- Whether the 60s-positive maker mark-out survives without the position cap
  binding.
- Whether NT's native queue-position fill model is "sufficient" for FR-001 (a
  source-proof + backtest decision).
- ETH end-to-end achievability at ~0.13s true budget (live latency measurement).
- The per-market profitability estimate that enables market-selection
  edge-ranking "C" (does not exist until a backtest produces it).
