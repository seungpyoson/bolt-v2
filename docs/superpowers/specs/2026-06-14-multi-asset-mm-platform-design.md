# Multi-Asset Market-Making Platform — Architecture Design (Grounded)

> **Status:** architecture-settled design — **NOT yet implementation-ready.**
> The §16 Must-Resolve decisions (settlement-signal availability, canonical
> pricing-path chaining, inventory composite-exposure, and the promoted
> backtest/governor/fence gates) must be applied into §4/§8/§16 before an
> implementation plan is written. Grounded against `origin/main`
> (`e2c726c25a7560ffe67221b8ee251d500685a08d`) and worktree HEAD
> (`0bf6f2f7e447cb63aef713e0fb866c75120984ed`), with NautilusTrader pinned at
> `6e059dcbb59ac1e582132fc431a581936c216c3c` (`Cargo.toml:25-46`). Every
> component below carries a build-vs-reuse tag and a file:line anchor. The
> grounding pass (13-agent investigation + adversarial verification, 2026-06-14)
> corrected several assumptions; those corrections are surfaced inline and again
> in §14 (Contradictions) / §15 (Open Gaps). PARTIAL adversarial verdicts are
> not papered over.
>
> A second, external multi-model review (4 engines, 2026-06-14) was then
> personally re-verified finding-by-finding against the same SHAs; its CONFIRMED
> results are folded in as truth-in-labeling corrections (stubs / zero-caller
> code relabeled to their real status) plus four engineering constraints
> (settlement double-booking §8, TradeTick-corpus fills §10, μ=0 guard §15,
> atomic requote budget §15).
>
> A **third, external multi-model review (4 engines, 2026-06-15)** was likewise
> re-verified finding-by-finding at the pinned SHAs (23 findings: 7 confirmed
> blockers, 5 majors, 2 minors, 8 partial, 1 refuted). It surfaced genuine design
> gaps the earlier passes missed: the settlement signal the slot relies on
> (`InstrumentStatus::Close`) is **undeliverable live** under bolt's forced
> `subscribe_new_markets=false` (§8); `gm_binary_quote` and `compose_binary_legs`
> are **sequential stages of one chain, not rival paths** (§4/§16#8); and the
> passive maker pays **zero** Polymarket fee — NT `compute_commission` returns 0
> for makers, so the earlier net-negative-fee claim was wrong (§5). Those
> corrections are folded in below. The architecture seam stands, but the **§16
> Must-Resolve** decisions — now including settlement-signal availability, the
> canonical pricing-path chaining, the inventory composite-exposure definition,
> and the promoted backtest/governor/fence gates (Rules #2/#6) — must be
> **resolved in-text before** an implementation plan is written; the doc is not
> implementation-ready until they are.
>
> A **fourth review (3 engines: GPT, GPT-5.5-pro, GLM, 2026-06-15)** re-verified
> the third-review fold at the pinned SHAs. It broke no architecture but caught
> **doc-consistency defects the prior folds introduced**, all corrected below:
> §4's six-pipeline-file commit anchor was stale (only `quote_plan` is at
> `e002139d3`; all six are present at `d4159c0a9`); the round-2 fold over-claimed
> `MakerInventory` as the composite accumulator when the type is
> **confirmed-fill-only** (§4/§16#4); §15 still said "pick one source of truth"
> after §16#4 chose a composite; §17 still called settlement "settled" while §16#5
> leaves three sub-decisions open; `maker_binary_fee_curve`'s "wire before live" is
> a no-op for the Polymarket maker (`fee_rate=0`, §5); "forbid any other writer" of
> the reservation band needs a **type-level** newtype since `FamilyQuoteInputs`
> fields are all `pub` (§16#8); and the requote budget conflated the
> **submit-governor (40/min)** with the venue **REST budget (100/min)** (§9/§16#3).
> One reviewer claim was **refuted** — the Polymarket contract DOES mark
> instrument-close `unsupported`, in `[streams.instrument_closes]`.
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
| **D1** | **Slot boundary = medium.** One shared engine on NT + two per-asset slots (pricing model, settlement). Five shared parts on NT: market-data, fair-value, quote-lifecycle/placing, inventory, risk. | The slot seam **already exists in code** — the family binding carries `fair_probability_up` + `maker_quote_targets` + `maker_settlement_payout` + `maker_binary_fee_curve` fn pointers per family (§3, surprise). The 4th hook (`maker_binary_fee_curve`) is binary-shaped (`fee_rate·p·(1-p)`) — a **de-facto third per-asset slot** needing generalization for non-binary families (§4). |
| **D2** | **Engine↔model = advisor.** The model is a pure function returning TARGET quotes (price only — `QuoteTargetLeg` carries {side, price}; **maker quote sizing is UNBUILT** — `choose_robust_size` is taker-only today and returns ZERO when EV is not strictly positive (`bolt_v3_sizing.rs:31`), exactly the GM/CG break-even regime, so the engine-side sizing seam must feed an edge/half-spread proxy and is a §16 open item; ttl not yet in the advisor contract); the shared engine reconciles target→live and does all placing/cancel. No side effects → backtestable; one backtest grades every asset (once per-asset slots implement their accounting — FR-001..FR-005 cover the binary slot only; perp funding/margin and options-surface accounting FRs are absent, deferred to future slots per §11). | Confirmed compatible: GM/CG model is pure tested math, UNWIRED (no NT type, no order placement). |
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
  TARGET quotes (price only — `QuoteTargetLeg` carries {side, price}; **maker quote
  sizing is UNBUILT**: `choose_robust_size` is taker-only and returns ZERO on
  non-positive EV (the GM/CG break-even regime), so the engine-side sizing seam
  must feed an edge/half-spread proxy — a §16 open item; ttl not yet in the advisor
  contract). Binary = GM/CG. No side effects → backtestable in isolation; one
  backtest harness grades every asset (D2) — once each per-asset slot implements
  its own accounting (binary only today; §11).
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
   │  returns TARGET quotes (price only — size UNBUILT/§16, ttl not yet
   │  in contract), reacts to danger (widen/skew/pull)
   ▼
[shared] quote-lifecycle controller  reconciles target → live
   │  cancel+resubmit (venue lacks modify), throttle to rate budget, track resting set
   ▼
NT order submission (RiskEngine notional cap + submit-rate limit) → venue
   │
   ▼
fills ──► [shared] inventory (NT Portfolio.net_position + maker accumulator)
resolution signal [UNRESOLVED §16#5a — InstrumentStatus::Close is undeliverable live under forced subscribe_new_markets=false] ──► [per-asset] SETTLEMENT slot books 0/1 payout, resets inventory
```

The model is a **pure function** (no NT type, no order placement). The **engine
reconciles target → live** and does all submit/cancel. This is what makes the
model backtestable in isolation and lets one backtest harness grade every asset
— once each per-asset slot implements its own settlement/accounting (binary only
today; perp/options accounting deferred per §11).

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
- **CANONICAL-CHAIN GAP (Rule #2):** `gm_binary_quote` and `compose_binary_legs`
  are **NOT two rival pricing paths** — they are sequential stages of ONE chain.
  `gm_binary_quote(p, μ)` (`:61`, μ-driven) is the upstream **producer** of a
  reservation bid/ask band (`BinaryGmQuote{bid,ask}`, `:44`); `compose_binary_legs`
  (`bolt_v3_quoting.rs:109`) is the downstream **consumer** that takes
  `reservation_bid`/`reservation_ask` from `FamilyQuoteInputs` (`:32`) and applies
  the floor/time-widening/inventory-skew/clamp to lay out the YES/NO legs. The real
  Rule #2 hazard is that today `updown::maker_quote_targets` (`updown.rs:89-114`)
  calls `compose_binary_legs` with a reservation band that is **NOT sourced from
  `gm_binary_quote`** (which has **zero production callers**) — i.e. compose runs
  with no GM upstream. Wiring GM/CG = make `gm_binary_quote` the **sole** producer
  of `FamilyQuoteInputs.reservation_bid/ask` (add `informed_fraction`/μ to that
  producer's inputs) and **forbid any other writer** of those fields. **All ten
  `FamilyQuoteInputs` fields are `pub` today (`bolt_v3_quoting.rs:30-42`), so
  "forbid" needs a type-level mechanism** — a `GmReservationBand` newtype (or private
  reservation fields behind a GM/CG constructor) that `compose_binary_legs`
  consumes — a convention alone cannot stop a direct struct literal. Keep
  `compose_binary_legs` — it is the layout stage, not a deletable loser (§16#8).

### Fair value (p) — **[ALREADY-BUILT; NT-backed path planned]**
- `src/bolt_v3_market_families/updown.rs:1102` `fair_probability_up` =
  Black-Scholes digital N(d2) (`d2` at `:1128-1131`), hand-rolled. Live-consumed
  by the taker via the family seam.
- **FR-070**: the hand-rolled digital MUST be removed once NT greeks/IV is
  authoritative. NT *provides* the replacement: `black_scholes_greeks_exact` +
  `imply_vol` (`crates/model/src/data/greeks.rs:142,192,238`), `itm_prob`
  usable as the binary win probability **after a boundary clamp**. **Clamp caveat:**
  `imply_vol_and_greeks` applies a `safe_vol = max(vol, 1e-8)` floor on
  convergence failure (`greeks.rs:251`); at that floor `d2 → ±∞` so
  `itm_prob = N(d2)` degenerates to 0 or 1 by moneyness. Callers MUST clamp
  `itm_prob` to (ε, 1−ε) before using it as a probability — this guard is **not**
  in NT. Planned consolidation, not a current gap.

### Realized-vol / FV stack — **[ALREADY-BUILT, LIVE-CONSUMED]**
- `bolt_v3_realized_volatility{,_runtime}.rs`, wired through
  `StrategyBuildContext` (`registry.rs:98-108`); feeds `fair_probability_up`.
  There is **no separate "FV combiner" module** — combination is inline in
  `bolt_v3_taker_pricing.rs`. RV subscribes via config (`spot_quote`/`midpoint`);
  trade subscription is config-gated and inactive today (see μ, below).

### Market-family seam — **[ALREADY-BUILT, MAKER HOOKS PRESENT]** — *load-bearing surprise*
- `src/bolt_v3_market_families/mod.rs`: the binding struct already carries
  **four maker fn pointers**: `fair_probability_up` (`:85`),
  `maker_quote_targets` (`:86`), `maker_settlement_payout` (`:87`),
  `maker_binary_fee_curve` (`:88`) — the fee a maker *would* pay under the family's
  fee schedule, `fee_rate·p·(1-p)`; **for Polymarket this is structurally zero**
  because the passive maker's `fee_rate` is 0 (§5)
  (binary outcome-token variance; `hyperliquid_instrument` stubs it to `None` at
  `:302` — **not cleanly fillable by a non-binary family**, so this 4th hook is a
  **de-facto third per-asset slot whose signature is binary-shaped** and must be
  generalized/renamed before a perp/option family can implement it). **No
  production caller consumes `maker_binary_fee_curve` yet** — its dispatchers
  `maker_binary_fee_curve_for_family{,_with_bindings}` (`:493/:506`) are **public
  production functions** (above the `#[cfg(test)]` boundary at `:817`), wired into
  the production binding (`:288`), but with **zero non-test callers**; it must be
  wired into the quoting/sizing/admission path before live use (§15).
- **Two families registered in the dispatch table:** `updown` (`:278-288`)
  implements all four maker hooks; `hyperliquid_instrument` (`:291-…`) registers
  **compile-time `unsupported_*` stubs that return `None`** for all maker hooks —
  it demonstrates the dispatch seam, not a working maker. Dispatcher
  `fair_probability_up_for_family` (`:432`).
- **Implication for D1/D2/D7:** the per-asset *pricing-model* and *settlement*
  slots already have a typed home at the family layer. New families add to
  `VALIDATION_BINDINGS` without touching the dispatcher. The slot seam exists in
  code — stronger than the assumed architecture. **Caveat:** the shared
  `FamilyQuoteInputs` struct (`bolt_v3_quoting.rs:30-42`) is binary-scoped — ten
  `f64` scalars (`tau`/`reference_tau`/`time_widen_cap`/`reservation_bid/ask`/…),
  **no `funding_rate`, `vol_surface`, `informed_fraction`/μ, or `size` field**. A
  perp or options slot cannot reuse this type as-is; the first such slot must
  extend `FamilyQuoteInputs` or introduce a per-advisor input type before wiring
  `maker_quote_targets` (future-extension gap; binary-first build unaffected).

### Quote-lifecycle / placing — **[ALREADY-BUILT (core) + IN-PROGRESS (pipeline), all UNWIRED]**
- On `origin/main`: `bolt_v3_quote_lifecycle.rs` (NT-free single-leg state
  machine), `bolt_v3_requote_budget.rs` (requote-rate throttle),
  `bolt_v3_maker_microprice.rs`, `bolt_v3_maker_event_fence.rs`.
- **Six pipeline files** (`bolt_v3_maker_{quote_plan,quote_control,quote_set,
  order_plan,order_compile,order_dispatch}.rs`): all six present at commit
  `d4159c0a9` on `codex/reference-price-architecture` (introduced incrementally —
  `quote_plan` `e002139d3`, `quote_control` `1a2775921`, `quote_set` `e815ca1a8`,
  `order_plan` `cbf65e922`, `order_compile` `2471c9488`, `order_dispatch`
  `72a1696ec`; only `quote_plan` exists at `e002139d3`), absent from `origin/main`
  (verified `git ls-tree`/`--diff-filter=A`). `d4159c0a9` is the same commit §15/§17
  cite for the `MarketAction` executor layer. Real in-progress work, not stale
  artifacts — to be ported into the generic archetype, not the World-Cup strategy.
- **WIRED status: NONE.** FR-010..FR-013: cancel+resubmit reprice (no venue
  modify), throttle to a *capability variable* (not a constant), track the full
  resting set + reconcile against accepted-order truth, requote-on-post-only-reject.
- **Port references (NT examples):** NT ships working implementations of all four
  FR-010..FR-013 patterns in `crates/trading/src/examples/strategies/grid_mm/`
  and `composite_market_maker/` (behind `feature="examples"`): cancel-inflight
  dedup via `pending_self_cancels: AHashSet`, worst-case exposure from the
  open+inflight union with dedup, and cancel-all+resubmit lifecycle. They cannot
  be imported directly but should be **ported as the implementation reference**
  rather than built from scratch.

### Inventory — **[NT-PROVIDES tracking] + [ALREADY-BUILT accumulator, UNWIRED]**
- NT Portfolio is **Rust-native**: `crates/portfolio/src/portfolio.rs:1170
  net_position`, `:426 unrealized_pnls`, `:475 realized_pnls`. It supplies only the
  **filled, leg-raw** per-instrument position (`signed_qty`, fill-driven — it does
  NOT reflect inflight orders, `:1486-1501`), so it is **one input to a composite
  exposure snapshot, not the sole "source of truth."**
- `src/bolt_v3_maker_inventory.rs` `MakerInventory` (`:11`) is a
  **confirmed-fill-only** YES-normalized accumulator — a single `net_position: f64`
  field (`:12`) folded by `apply_fill(leg, side, qty)` (`:29`), with **no**
  open-order, inflight-order, or NT-Cache input — and is UNWIRED. The maker exposure
  the gate reads is a **net-new composite snapshot** (NOT this type): NT Portfolio
  filled positions + `cache.orders_open` + `cache.orders_inflight`, reconciled
  through the No-leg sign adapter (`net_yes = yes − no`). `MakerInventory` may serve
  as **one input** to that composite, **not** as the accumulator over the union
  (§15, §16#4).

### Risk infrastructure — **[NT-PROVIDES, mostly] + [NET-NEW residue]**
- **NT provides** (reuse, do not rebuild): pre-trade **notional-per-order** cap
  (`crates/risk/src/engine/config.rs:44`, enforced `engine/mod.rs:1186-1192`),
  submit/modify rate limits, **TradingState{Active,Halted,Reducing}**
  (`crates/model/src/enums.rs:1928`) with halt→deny and Reducing→block-exposure-
  increasing (`risk/src/engine/mod.rs:1707-1730`).
- **`cancel_all_orders` is a Strategy trait method** (`trading/src/strategy/mod.rs:742`),
  **NOT** a RiskEngine primitive (the risk engine has zero occurrences of it).
  `TradingState::Halted` only **denies new submits** — resting orders stay live
  on halt. FR-023 hard-flat and any halt sequence MUST explicitly call
  `self.cancel_all_orders(…)` from within the strategy.
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
  maker source must be tamper-gated, add a **new `MAKER_KEY` `GatedSourceRoot`**
  in `source_canonicalization.rs` (after `SUBMIT_ADMISSION_KEY` at `:579-581`) —
  **do NOT expand `STRATEGY_KEY`** (`:561-570`), which would break the taker
  digest test — and derive a parallel `GOLDEN_MAKER_DIGEST` constant + pinned
  test in `bolt_v3_source_integrity.rs` (`:287`). Not laundering.

### Position sizer / kill switch — **[ALREADY-BUILT, LIVE on taker]**
- `src/bolt_v3_sizing.rs choose_robust_size` (EV-fraction→notional), consumed by
  the taker. `bolt_v3_kill_switch{,_store}.rs` state machine gating submissions
  via `bolt_v3_submit_admission.rs` (the consolidated chokepoint). The maker
  reuses this chokepoint.

## 5. Risk Design (D3, D4)

Three problems → three responses, collapsing to **Normal / Defensive / Stop**
(finer gradations are config thresholds, not new machinery). Optional 4th = a
single hard total-loss backstop; optional 5th = a reward-continuity soft-hold
(FR-060). The "5-rung ladder" is rejected as convention
(D4) — note the grounding pass found **no repo artifact named "5-rung ladder"**;
treat D4 as a design stance, and the spec actively contraindicates multi-level
quoting for thin binary books.

| # | Problem | Response | Evidence / confidence |
|---|---------|----------|------------------------|
| 1 | Stale / bad feed | **STOP (fail-closed)** | Taker pattern proven: `evaluate_forced_flat_predicates` pushes `StaleReference` (`exposure.rs:385-400`), `is_none_or` defense-in-depth — one expression handling both the never-connected (`None`) and live-but-aged (`Some(ts)`, `now−ts>threshold`) cases. The only live run (Jun 3-6) priced **0 / 137,157** evals, all `ForcedFlat(StaleReference)` because the reference feed was never connected (`docs/research/leadlag-subsecond-fillability-2026-06-10.md:186-189`). **CONFIRMED (None arm); the `Some`-aged arm is structurally present but not live-exercised.** Maker-side gate **absent** — the predicate is `pub(super)` and must be **hoisted to a shared module** and wired into the maker admission gate, not copied (§15). |
| 2 | Pick-off via lag | **WIDEN / PULL (in-model)** | **EMPIRICALLY MEASURED, not assumed:** BTC taker edge **+13.27c/share @1s, 95% CI [+9.68,+16.87]**, n=127 (`leadlag-trades-leader-2026-06-11.md:92`); HL clock fires ~+1s late, 80-100% leader-first. Maker mark-outs: 30s = -0.013c CI[-0.073,+0.047] (spans zero), 60s = +0.141c CI[+0.057,+0.226] (`leadlag-taker-edge-2026-06-10.md:380-381`) — **gross** (`leadlag_session4.py` maker mark-out loop applies no maker fee), but under the pinned NT path the **passive maker pays ZERO Polymarket fee**: `compute_commission` returns `0.0` for `LiquiditySide::Maker` (`crates/adapters/polymarket/src/execution/parse.rs:399`), the HTTP path maps `maker_fee`→`Decimal::ZERO`, and bolt **hard-fails pre-run** if maker commission is nonzero (`fee_behavior_source.rs:52,56`, `maker_zero_fee_verified`). `makerBaseFee=1000` is a raw Gamma fixture field NT does **not** charge makers. So no fee flips the maker mark-out negative; the only surviving caveat is that the **30s gross CI spans zero** (`[-0.073,+0.047]`). Any residual maker-cost concern must be re-grounded on real costs (gas / LP-reward inversion FR-060), not `makerBaseFee`. GM/CG μ-driven spread is the response to pick-off. |
| 3 | Inventory into 0/1 resolution | **INVENTORY MGMT (cap + skew — NO active flatten)** | `inventory_skew` hard cap returns None at `\|net\|≥cap` (`bolt_v3_maker_model.rs:119`); FR-040 per-market reserved-collateral worst-case-liability gate; "resting maker is structurally left holding inventory into expiry … settlement is the dominant P&L term" (`spec.md:65`). **Risk CONFIRMED**. **Active-flatten (crossing the book to reduce a stuck position) is ABSENT** — `None` stops new accumulation but no code places aggressive/reduce orders; on a thin CLOB a stuck one-sided position has no forced-liquidation path before expiry (§15). Actual fill/accumulation rate is **runtime-only** (§13). |
| 4 (opt.) | Runaway total loss | **single hard backstop** | Real, non-redundant: the position cap stops *accumulation* but does not bound *settlement loss on already-held inventory*. `KillSwitchHaltTriggerKind::LossGovernorBreach` + constructor exist (`bolt_v3_kill_switch.rs:45,51,61`) but **no code computes running P&L and fires it** (§15 gap). The graduated governor FR-023 (cancel-only / reduce-only / hard-flat / soft-hold) is the spec's response. |
| 5 (opt.) | Reward-continuity vs safety tension | **soft-hold (reward-preserving)** | FR-060 mandates safety wins when reward-eligibility (continuous resting) conflicts with a pull signal; soft-hold is the governor state that defers a full pull when the safety threshold is not yet breached, preserving LP-reward eligibility. Same D10 principle as rows 1-4 — gives soft-hold a named real problem rather than an unmapped 4th FR-023 state. |

**Governor shape (FR-023):** graduated states, not a boolean; kill predicates
(σ-floor, basis-cap, τ-floor, plus existing stale/thin/incoherent) as TOML
thresholds that fail closed. Additional spec-level failure modes — basis blowout
(basis-cap predicate, FR-023), maintenance stranding (FR-050 / `maintenance_window`),
fee/rebate inversion (FR-060 / FR-001 net accounting), and self-crossing legs
(FR-022 / SC-002) — have design homes in the spec; this table covers the three
evidence-ranked primary risks, **not an exhaustive enumeration**.

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
- **Double-booking guard required:** if a backtest harness calls
  `set_settlement_price` on the matching engine (`engine.rs:408`) before the slot
  runs, NT's general expiry path (`engine.rs:2102-2111`) **already** closes all
  open positions via `apply_fills` at that price. The slot's
  `maker_settlement_payout` must therefore either (a) delegate entirely to NT's
  expiry path (pre-set `settlement_price`, skip independent booking) or (b) treat
  NT's auto-fill as the authoritative close after an `is_expiration_processed()`
  check. **Caveat on (b):** `is_expiration_processed()` is `pub const`
  (`engine.rs:1941`), and the matching engine *is* reachable via the **public**
  accessor `SimulatedExchange::get_matching_engine` (`exchange.rs:414`) — but only
  from the backtest harness that owns the `SimulatedExchange`. A **Strategy holds no
  exchange handle at all** (zero `SimulatedExchange` references under
  `crates/trading/src`; it talks only to the message bus), and there is **no
  `settlement_price` getter** (only `set_settlement_price`) — so (b) is **not
  implementable from the strategy**; it must route through a harness/exchange-owned
  hook, or the design commits to (a). Running both paths double-closes positions —
  **decided in §16#5, not deferred.**
- **Critical correction (D7):** `InstrumentStatus::Close` is **NOT** NT's
  settlement mechanism. `process_status(MarketStatusAction::Close)` only sets
  `market_status = Closed` (`engine.rs:1890-1912`, body read — no payout logic).
  NT settlement is driven by the *distinct* `InstrumentClose` event or timestamp
  expiration. The Polymarket contract marks NT's `InstrumentClose` `unsupported`
  (`contracts/polymarket.toml [streams.instrument_closes] capability = "unsupported"`,
  reason "Polymarket has no close events; resolution is a separate concept"),
  so the **resolution signal the strategy actually receives is
  `InstrumentStatus::Close`** — a notification the per-asset settlement slot must
  observe and then **explicitly book the 0/1 payout** (FR-030).
  **Winner detection:** NT emits `InstrumentStatus::Close` for **all** tokens in
  the resolved market (both Yes and No), each carrying the same freeform `reason`
  string `"Winner: {winning_asset_id} ({winning_outcome})"` — there is **no**
  structured settlement-price or winning-side field on `InstrumentStatus`
  (`data.rs:1060-1087`). The slot must extract `winning_asset_id` from the reason
  string and compare it against the instrument's own Polymarket `token_id` to
  resolve payout = 1.0 (winner) or 0.0 (loser); FR-030 must specify this
  comparison, and `maker_settlement_payout` must accept the resolved payout value
  rather than re-derive it (its current `fn(OutcomeSide, Leg)` signature, `mod.rs:87`,
  re-derives from the enum at `updown.rs:115` — a **signature change**, §16#5c).
  FR-004 requires one shared settlement primitive reused by backtest and live.
- **BLOCKER — the resolution signal is NOT deliverable live as configured.** The
  ONLY NT emitter of `InstrumentStatus::Close` carrying the `"Winner: …"` reason is
  the Polymarket `MarketResolved` handler (`crates/adapters/polymarket/src/data.rs:1052-1088`),
  which the server delivers **only when `subscribe_new_markets=true`**
  (`websocket/messages.rs:181`; the all-markets subscribe at `data.rs:1189` is gated
  on the same flag). Bolt **forces that flag `false`** (`config/root.toml:473`) **and
  fail-closes if it is set `true`** (`src/bolt_v3_providers/polymarket.rs:419,755`).
  So as configured the settlement slot's load-bearing notification **never arrives
  live.** FR-030 MUST name the reconciliation: either (a) the market-subscription
  slice owns a **controlled `subscribe_new_markets=true` path** that lifts bolt's
  fail-closed guard, or (b) settlement is driven by an **alternate resolution
  source**. Until one is chosen, `InstrumentStatus::Close` is **not** an available
  live signal (§16#5a).
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
  (**40/min**) at `config/root.toml:413` (with a matching `max_order_modify_rate =
  "40/00:01:00"` at `:414`; **no cancel-rate field exists**). These are **two
  distinct budgets in different units**: the NT `max_order_submit_rate` governs
  **submit commands** (40/min), while the venue's `clob_per_minute = 100`
  (`contracts/polymarket.toml [rate_budget]`, **not** `root.toml`) is a **REST-call**
  budget that every cancel **and** submit consumes. A cancel+resubmit cycle costs
  **1 submit token + 2 REST tokens**, so the controller MUST size against **both**
  (submits ≤ 40/min AND REST ≤ 100/min), not collapse them into a single
  "whichever is lower" window (FR-011; see §16#3).
- FR-080: the controller MUST read modify/budget/maintenance/depth/fee/
  settlement-kind as variables and MUST NOT branch on a venue name.
  **Enforcement gap (open):** FR-080 has **no CI fence** — the existing
  `verify_bolt_v3_core_boundary.py` catches only `match venue.kind`/`VenueKind`
  enum dispatch, not string-literal venue-name comparisons. A
  `verify_bolt_v3_no_venue_name_branch.py` script must be added to
  `source-fence-static`, denying e.g. `venue_id == "polymarket"` /
  `venue_name.contains("binance")` outside `src/bolt_v3_providers/`. Until it
  ships and is wired into the `justfile` recipe, FR-080 is a policy assertion
  only, not an enforced invariant.

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
- **Queue-realism precondition (hard, two parts):** (1) NT's queue model is
  **inert unless `queue_position=true`** — it defaults **false**
  (`matching_engine/config.rs:45,79`), and with the default the fill gate returns
  full leaves at touch (`engine.rs:547`), i.e. optimistic at-touch fills regardless
  of corpus shape. (2) With `queue_position=true`, only **TradeTicks** decrement
  same-price queue volume (`decrement_queue_on_trade` ← `process_trade_tick`,
  `engine.rs:459,1820`); level Delete/Clear and shrinking Update also unblock the
  gate (`engine.rs:599-664`), so a delta-only corpus still produces **some** fills
  on level removal — just with **unrealistic fill timing and no partial-fill
  granularity** (more dangerous than zero fills: the backtest looks alive). So the
  precondition is **not** "avoid zero fills" — it is: enable `queue_position=true`
  AND assemble a TradeTick+OrderBookDelta corpus, or same-price fill *timing* is
  wrong (§16).
- FR-005: underlying spot for the window must be backfilled and point-in-time
  aligned to the oracle the maker saw (look-ahead controlled).

## 11. Multi-Asset Extension

- **Binary first** to prove the framework (the GM/CG **pricing model** is built +
  tested but **UNWIRED**; the binary **maker framework** — quote-lifecycle wiring,
  settlement, governor — is **unbuilt**, gated by §16). Then each new asset fills the
  two slots behind the **MarketFamily seam** (already carrying `fair_probability_up`
  + `maker_quote_targets` + `maker_settlement_payout` + `maker_binary_fee_curve`
  per family; FR-081). A second family (`hyperliquid_instrument`) is registered
  but currently uses `unsupported_*` stubs returning `None` for all maker hooks —
  it demonstrates the registration path only, not a working maker.
  **Structural caveat:** the shared `FamilyQuoteInputs` struct
  (`bolt_v3_quoting.rs:30-42`) is binary-scoped (no `funding_rate`/`vol_surface`/μ/
  `size`); perp advisors (need funding) and options advisors (need a vol surface)
  must extend or replace it before their `maker_quote_targets` can be wired.
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
   (`root.toml:413`), not "100/second"; the VenueContract W1 schema extension is
   already merged. Ground from main, not `spec.md:177-178`.
8. **`plan.md` references a nonexistent file:** `specs/488-binary-oracle-maker/
   plan.md:68,114,131` cite `src/bte_ingest.rs` as the landed raw→catalog loader;
   `git ls-tree` at this anchor finds no such path. The BTE catalog loaders live
   under `crates/backtesting-vertical-slice/src/` — `catalog_projection.rs`
   (TradeTick projection + instrument write) and `pmxt_one_off_backfill_projection.rs`
   (pmxt L2). **Which exact module the plan author meant is not confirmed** (the
   "L2 / `write_instruments` gap" wording maps to neither cleanly); pin it against
   the BTE epic (#437/#438) when the maker plan is (re)written, and correct those
   three references then — do NOT substitute a guessed path now.

## 15. Open Gaps (must be addressed in the build, not research)

- **Maker-side stale-feed gate ABSENT (hoist, do not copy):** `ForcedFlatInputs`,
  `ForcedFlatReason`, and `evaluate_forced_flat_predicates` are `pub(super)` in
  `binary_oracle_edge_taker` (`exposure.rs:364-400`) and depend on `pub(super)`
  `SelectionPhase` (`selection.rs:32`). A literal copy into the maker is the
  dual-state debt Rule #6 forbids. **Hoist** these three into a shared
  `bolt_v3_*` module (e.g. `bolt_v3_feed_health`) and wire BOTH the taker and the
  new maker admission gate against the shared function.
- **4th-risk loss backstop has no firing code:** `LossGovernorBreach` + ctor
  exist; nothing computes running P&L against thresholds and fires it. The
  orphaned `loss_governor` TOML is deploy-local and `RiskBlock` has
  `deny_unknown_fields` with no field for it. Firing-code + a parsed config field
  are both net-new.
- **Inventory composite-exposure boundary — resolved in §16#4** (NOT "pick one
  source of truth"): the gate reads a **net-new composite** of NT Portfolio filled
  positions + `cache.orders_open` + `cache.orders_inflight` via the No-leg sign
  adapter; `MakerInventory` is **confirmed-fill-only** and serves as at most one
  input. The open build item is wiring `cache.orders_inflight` into the snapshot —
  neither existing tracker reads it (Rule #6 no-dual-state).
- **μ estimator + aggressor-side classifier is net-new** (FR-021); trade
  subscription exists but is config-gated and inactive — μ is starved today.
- **Future-GLFT port verdicts rest on unverifiable claims** (tikr/DaruFinance/
  Jacobo-EG clones gone) — re-clone before authorizing a GLFT port.
- **SVI/BL absence confirmed only for NT + market-maker-rs** (14 repos
  unverified) — re-clone before relying on the universal claim for options.
- **μ=0 (informed_fraction=0) go-live is PROHIBITED:** `gm_binary_quote` collapses
  bid=ask=fair_p_up at μ=0 (`bolt_v3_maker_model.rs:61-88`, asserted in its own
  tests), yielding **zero spread**; a zero-spread quote earns **no compensation for
  pick-off, inventory, gas, or operational risk** (the passive maker fee itself is
  zero, §5 — the loss is the missing edge, not a fee). The go-live gate MUST reject
  any config where the μ source is absent or produces a constant 0.
- **Active-flatten path ABSENT:** when `inventory_skew` returns `None` the governor
  goes reduce-only, but no code emits an aggressive/crossing order to reduce a
  stuck position (`bolt_v3_maker_model.rs:112-126`; grep finds no taker-mode/cross
  path). On a thin CLOB a stuck one-sided position has no forced-liquidation path
  before expiry. Decide: wire a taker-mode reduce path off the `None` signal, or
  explicitly accept cap-only defense and document the residual settlement-loss risk.
- **NT Portfolio ignores inflight orders (inventory lag):** `net_position` updates
  only on `PositionEvent`/fill (`portfolio.rs:1486-1501`); `SUBMITTED`/`ACCEPTED`
  orders live in `cache.orders_inflight` and are NOT reflected. On a thin CLOB the
  SubmitOrder→OrderFilled window is the primary one-sided-accumulation interval —
  the inventory gate MUST read `cache.orders_inflight` (or the `MakerInventory`
  accumulator) to cover it; NT Portfolio alone under-counts pending exposure.
- **Inventory sign mismatch (No-leg) needs an adapter:** NT Portfolio tracks raw
  per-instrument `signed_qty` (Buy=+qty, Sell=−qty regardless of leg);
  `MakerInventory` normalizes to YES-equivalent ((No,Buy)=−qty,
  `bolt_v3_maker_inventory.rs:35-42`). For a No-leg Buy these disagree in sign. If
  NT Portfolio is the chosen truth (§16#4), the caller of `inventory_skew` MUST
  invert the No-leg position before summing: `net_yes = yes_position − no_position`.
- **FR-040 worst-case-liability has no in-flight data source:** neither NT
  Portfolio (filled positions only; `margins_init` is margin-account-only,
  inapplicable to cash-settled Polymarket) nor `MakerInventory` (fills only,
  `:11-45`) tracks the live resting-order set. FR-040 needs a third input — the
  sum of open order quantities per market from NT Cache (`cache.orders_open`) at
  gate-evaluation time. Wire it explicitly; no existing tracker provides it.
- **RequoteBudget has no production caller and no atomic reservation:** every
  `RequoteBudget::new` call site is `#[cfg(test)]` (`bolt_v3_requote_budget.rs`),
  and the struct has a single window-cost accumulator with no separate
  cancel-vs-submit reservation slot. Wiring MUST (a) resolve the
  min(`clob_per_minute=100`, NT-governor=40/min) cap at construction, and
  (b) reserve budget **atomically** for the cancel+resubmit pair before issuing the
  cancel — mid-window exhaustion after cancel but before resubmit strands a side.
- **`maker_binary_fee_curve` has no production consumer:** the 4th binding slot and
  its `updown` impl (`fee_rate·p·(1-p)`) are wired into the production binding
  (`mod.rs:288`) and their dispatchers `maker_binary_fee_curve_for_family{,_with_bindings}`
  (`mod.rs:493/506`) are **public production functions** (above the `#[cfg(test)]`
  boundary at `mod.rs:817`) — but with **zero non-test callers**: nothing in any
  quoting/sizing/admission path calls them (`mod.rs:88`). "Public dispatcher, no
  production consumer," **not** "test-only." **But it is NOT a binary-maker live
  blocker:** for Polymarket the maker `fee_rate` is 0 (§5), so the curve returns 0
  and wiring it into spread/EV is a no-op on the only live venue. It becomes
  load-bearing only for a future family/venue with a nonzero maker fee — wire it
  then; do not gate binary go-live on it.
- **`MarketAction`→NT submit/cancel executor is World-Cup-scoped, not generic:**
  the executor wiring layer exists only on `codex/reference-price-architecture`
  (commit `d4159c0a9`) targeting the single-venue World-Cup strategy; porting it to
  the agnostic maker archetype is the **primary executor build gap** (§17) and must
  be rebuilt/ported as part of the quote-lifecycle wiring (FR-010..FR-013).

## 16. Must Resolve Before Implementation (decided now — no research mid-build)

1. **Live-registration shape:** make the `StrategyRuntimeBinding` slice
   injectable from a non-scanned caller (`main.rs` / `build_bolt_v3_live_node`);
   keep the maker `StrategyBuilder` + register fn under
   `src/strategies/binary_oracle_maker/`. Do NOT mirror the taker archetype.
2. **GOLDEN-digest gating:** add a **new `MAKER_KEY` `GatedSourceRoot`** (separate
   from `STRATEGY_KEY`) covering `src/strategies/binary_oracle_maker/`, and derive
   a parallel `GOLDEN_MAKER_DIGEST` + pinned test. **Do NOT expand `STRATEGY_KEY`**
   — that breaks the existing taker digest test. Scope into the maker PR up front.
3. **Requote-budget — two budgets + atomic reservation:** model the **two distinct
   constraints (different units)**: the NT submit-governor `max_order_submit_rate` =
   **40/min** (`root.toml:413`; counts submit commands) AND the venue CLOB
   **REST-call** budget `clob_per_minute=100` (`contracts/polymarket.toml`; every
   cancel and submit consumes one). A cancel+resubmit cycle costs **1 submit token +
   2 REST tokens** — size the controller so **submits ≤ 40/min AND REST ≤ 100/min**
   (do NOT collapse to a single "whichever is lower" window). AND reserve budget
   **atomically for the cancel+resubmit pair as ONE acquisition** before issuing the
   cancel, so mid-window exhaustion can never strand a cancelled side — `RequoteBudget`
   today has a single `try_acquire` with no cancel-vs-submit slot and
   `#[cfg(test)]`-only constructors (§15).
4. **Inventory composite-exposure definition:** define the single **net-new** exposure
   snapshot the gate reads — NT Portfolio filled positions + `cache.orders_open` +
   `cache.orders_inflight`, reconciled through the No-leg sign adapter
   (`net_yes = yes − no`). `MakerInventory` is **confirmed-fill-only** (one
   `net_position` field, `apply_fill` only — `bolt_v3_maker_inventory.rs:11-45`) and
   may serve as **at most one input** to the composite, **not** the accumulator over
   the union (avoid Rule #6 dual-state).
5. **Settlement contract (signal + authority + signature):**
   **(a) Signal availability** — resolve the §8 BLOCKER: either own a controlled
   `subscribe_new_markets=true` path (lifting bolt's fail-closed guard at
   `polymarket.rs:419,755`) or name an alternate resolution source; do not assume
   `InstrumentStatus::Close` is deliverable live until then.
   **(b) Double-booking authority** — pick ONE: delegate entirely to NT's expiry
   path (pre-set `settlement_price`, slot skips booking), or the slot books and NT's
   auto-fill is suppressed — and because the strategy cannot reach
   `is_expiration_processed()` (§8 caveat), the latter requires a harness/exchange
   hook. State how the unchosen path is prevented from also closing.
   **(c) Payout signature** — the existing
   `maker_settlement_payout: fn(OutcomeSide, Leg) -> Option<f64>` (`mod.rs:87`)
   **re-derives** payout from the enum (`updown.rs:115`); §8 winner-detection
   requires changing it to **accept the resolved payout / winner result**, with
   `updown` reworked. All three reuse ONE shared settlement primitive across
   backtest+live (FR-004).
6. **μ source + health gate:** config-activated `(Trade,Trade)` RV source +
   net-new aggressor-classifier + VPIN estimator; specify the classification rule.
   AND a **μ-health gate that blocks quoting AND go-live** when μ is absent, stale
   (no fresh estimate within a TOML window), NaN/non-finite, or **constant-zero** —
   `gm_binary_quote` accepts μ=0 and returns bid=ask=fair, half-spread≈0
   (`bolt_v3_maker_model.rs:61-88`, test `:135`), so a defaulted/starved μ silently
   emits zero-spread quotes (carries the §15 "μ=0 PROHIBITED" prohibition into the
   gate). GM/CG cannot run without a live, nondegenerate μ.
7. **Loss-backstop wiring:** add a parsed config field (replacing the orphaned
   deploy-local `loss_governor`) and firing code that computes running P&L and
   calls `loss_governor_breach`; thresholds in TOML.
8. **Canonical pricing chain (Rule #2):** `gm_binary_quote` and `compose_binary_legs`
   are sequential stages, **not rivals** — wire `gm_binary_quote(p, μ)` as the
   **sole producer** of `FamilyQuoteInputs.reservation_bid/ask` and add
   `informed_fraction`/μ to that producer's inputs. Because **all ten
   `FamilyQuoteInputs` fields are `pub`** (`bolt_v3_quoting.rs:30-42`), "sole
   producer" must be enforced at the **type level** — introduce a `GmReservationBand`
   newtype (or make the reservation fields private behind a GM/CG constructor) that
   `compose_binary_legs` consumes; a convention alone cannot forbid a direct struct
   literal. Keep `compose_binary_legs` (the layout stage). The live hazard to close
   is exactly today's state: compose wired, `gm_binary_quote` with zero production
   callers (§4 CANONICAL-CHAIN GAP).
9. **Correct spec.md Assumptions (FR authority):** update `spec.md:177` bolt-config
   budget from "100/second" to "40/min (`root.toml:413`)" and remove `spec.md:178`
   "cannot hold the maker capability variables" — `VenueContract` already carries
   execution/rate_budget/maintenance_window/depth_availability/fee_schedule/
   settlement at schema_version=3. Also fix `plan.md:68,114,131` (the nonexistent
   `src/bte_ingest.rs` path — see §14#8; pin the real BTE module first).
10. **Backtest queue-realism (corpus + config):** the FR-001/FR-003 backtest MUST
    run with `queue_position=true` (default is **false** → optimistic at-touch
    fills, `matching_engine/config.rs:45,79`) AND a corpus containing **TradeTick +
    OrderBookDelta** events; assert both in the gate harness. TradeTicks are
    necessary but not sufficient — without `queue_position=true` the queue model is
    entirely bypassed (§10).
11. **Active-flatten decision (FR-023):** decide now whether to wire a taker-mode
    **crossing reduce** path off `inventory_skew`'s `None` (hard-flat), or to scope
    FR-023's hard-flat state OUT for the binary slot and accept cap-only defense
    with documented residual settlement-loss risk. `LifecycleAction`/`MarketAction`
    emit only passive Submit/Cancel/Modify today, so hard-flat needs a **new
    aggressive-reduce action that does not exist** (§15). Name the in-scope FR-023
    states (cancel-only / reduce-only / hard-flat / soft-hold).
12. **FR-080 venue-name fence:** add `scripts/verify_bolt_v3_no_venue_name_branch.py`
    denying venue-name string-literal branches (e.g. `venue_id == "…"`,
    `.contains("…")`) outside `src/bolt_v3_providers/`, wire it into the
    `source-fence-static` justfile recipe, and scope it into the maker PR — the
    existing `verify_bolt_v3_core_boundary.py` catches only enum/kind dispatch over
    four fixed files, so FR-080 is unenforced until this ships (§9).
13. **Maker quote sizing seam:** `QuoteTargetLeg`/`QuoteTargets` carry no
    size/depth/ttl, and `choose_robust_size` is taker-only and returns **ZERO** on
    non-positive EV (`bolt_v3_sizing.rs:31`) — exactly the GM/CG break-even regime.
    Decide the engine-side maker sizing input (feed an edge/half-spread proxy, not
    raw 0-EV) before wiring, or the maker emits zero-size quotes (§4 D2).

## 17. Implementation-Readiness: Settled vs Runtime-Dependent

**Settled (build now, no further research):**
- GM/CG model built, tested, units-correct, UNWIRED — wire it **after** the
  §16#8 canonical-chain decision (make `gm_binary_quote` the sole producer of the
  reservation band that `compose_binary_legs` consumes — they are stages, not rivals).
- Family seam + four maker fn pointers exist (`market_families/mod.rs:85-88`);
  `updown` implements them, `hyperliquid_instrument` is `unsupported_*` stubs.
- VenueContract schema is DONE; read all venue facts from it; effective order
  budget = **40/min**.
- **NT API map** (what the API provides — settled): Portfolio (Rust-native
  net_position/pnls), TradingState halt/reduce + `cancel_all_orders` (a Strategy
  method, not a risk primitive — §4), notional cap, BS greeks/IV. **Which
  component is authoritative for inventory is NOT settled** — see §16#4. Build:
  signed-VPIN/μ estimator, position cap, settlement-payout booking, graduated
  governor, loss-backstop firing code, active-flatten path.
- Settlement: **NOT settled — see §16#5** (signal availability §16#5a,
  double-booking authority §16#5b, payout signature §16#5c are all open), and
  **ZERO existing impl** — only the family fn-pointer type (`mod.rs:87`) + its
  `(side,leg)` re-derivation (`updown.rs:115`) exist; the `on_instrument_status`
  observation handler and the reason-string winner parser are **net-new build**, and
  the signal's live availability is a §8 BLOCKER (`subscribe_new_markets`). Winner
  parsed from the freeform reason string (`data.rs:1061`, format re-verified — §8).
  Listed here only to record the NT API surface; the design itself is gated by §16#5.
- Live-registration clean path = injectable binding slice + maker code under
  `src/strategies/` (not the taker-mirror archetype). The `MarketAction`→NT
  submit/cancel wiring layer exists only on `codex/reference-price-architecture`
  (commit `d4159c0a9`) and targets the World-Cup single-venue strategy, **not**
  the agnostic maker archetype; porting it to the generic archetype is the
  primary executor build gap (§15).

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
