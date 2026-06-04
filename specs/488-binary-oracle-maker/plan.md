# Implementation Plan: Binary Oracle Market-Maker (robustness-gated, venue/instrument-agnostic)

**Branch**: `feat/488-w3-pricing` | **Date**: 2026-06-04 | **Spec**: `specs/488-binary-oracle-maker/spec.md`
**Input**: Feature specification from `specs/488-binary-oracle-maker/spec.md`. Tracking: #488.

## Summary

A two-sided resting market-maker for crypto binary-option markets, anchored to bolt's existing oracle fair value. The economic edge is the oracle mid priced against informed flow — so the **primary spread is a Glosten-Milgrom/Copeland-Galai adverse-selection half-spread**, with inventory skew secondary. The edge's *existence* is assumed (the maker market is mature and competitive); the open question is implementation **robustness** — capturing it without being adversely selected. So build proceeds **robustness-first** (each workstream fails closed), and **live capital is gated on a pre-live backtest of the built maker** scored with production-equivalent accounting on real full-depth L2. The design is **agnostic through existing seams** (NT execution adapters / an extended venue capability contract / a market-family quote-target seam); the first real implementation is crypto binary options, while future families are protected by W0 checks in `architecture.md`. This plan supersedes the original #488 framing ("port the market-maker-rs apparatus; grid→GLFT; anchor an A-S reservation price"), which an actual-code audit + an internal 5-lens review + a Codex review overturned: A-S/GLFT are a units category error for a bounded (0,1) martingale whose variance blows up ~1/√τ into expiry.

## Technical Context

**Language/Version**: Rust (edition per workspace).
**Primary Dependencies**: NautilusTrader Rust crates, at the rev **pinned in `Cargo.toml`** (the single source of truth — this plan does not restate the SHA, so it cannot drift). NT owns execution, position/PnL, pre-trade limits, backtest engine + returns analytics (Sharpe/Sortino/Calmar), VPIN, greeks/implied-vol. Source-proofs MUST read the NT checkout at the rev `Cargo.toml` currently pins. A separate hygiene issue tracks ~20 specs/docs that hard-code a now-stale rev.
**Storage**: Two backtest substrates. (1) bolt's own historical S3 parquet lake — full-population but **mid/top-level book only, no L2 queue depth**, ~3 weeks, no underlying spot. (2) the **free pmxt archive** (`r2v2.pmxt.dev`, hourly Parquet, ~Apr 20–May 25 2026) — verified 2026-05-31 to carry genuine **tick-level full-depth L2 + trades-with-aggressor** for the up/down markets (BTC/ETH/SPX present); this is the **preferred** substrate (lifts the no-L2-queue limit). Neither holds underlying spot (backfill externally). Distinct from the in-repo decision-evidence JSONL writer, which is entry-gated/selection-biased **and not deployed** — the backtest does not use it. NT `ParquetDataCatalog` reads S3 **only with the `cloud` feature** (off in bolt today) and ships **no generic external-parquet loader** — a one-time raw→catalog conversion is required (E-037 investigated 2026-05-31; see "Backtest Substrate — Investigation Findings").
**Testing**: `cargo test` (unit + property), NT backtest; `cargo fmt`/`clippy`/`deny` clean (CI on main).
**Target Platform**: Linux (EC2 LiveNode); crypto binary first, future market families through reviewed adapters.
**Project Type**: Single Rust project (NT `LiveNode` strategy archetype + offline replay tooling).
**Constraints**: NO HARDCODES (TOML), PURE RUST (no C/FFI/Python in the binary; offline data-prep scripts exempt), NO DUAL PATHS, SSM-only secrets, GROUP BY CHANGE.
**Scale/Scope**: Many concurrent thin binary markets; per-market caps rolling up to a portfolio budget.

## Constitution Check

*GATE: must pass before any build. Re-checked per workstream.*

- **NO HARDCODES** → venue facts in `contracts/<venue>.toml`, model params + thresholds in strategy TOML. ✅ by design (FR-080, FR-023).
- **NO DUAL PATHS** → single fill truth (NT ExecutionModel; custom fill only if NT source-proven insufficient), single settlement module (shared backtest↔live), single fair-value path (W8 kills the hand-rolled digital). ✅ (FR-003, FR-004, FR-070).
- **PURE RUST BINARY** → strategy + controller are Rust on NT; offline backtest-scoring tooling may be a script but produces no runtime dependency. ✅.
- **SSM single secret source** → unchanged; reuse existing credential resolution. ✅.
- **GROUP BY CHANGE** → swapping venue touches one contract file + config; instrument type touches one market-family adapter and one config section. ✅ once W0/W1 checks are in place.
- **ONE BRANCH / ONE SCOPE** → this architecture slice is on `feat/488-w3-pricing`; **robustness-first** means build advances workstream by workstream (each fail-closed) and live capital is gated on the pre-live backtest.

## Architecture — W0 Gate

`specs/488-binary-oracle-maker/architecture.md` is the W0 architecture gate and work-process handoff. It chooses Option B: reuse existing seams, do not add a speculative generic valuation platform, and do not hardcode the first crypto binary path into shared code.

The agnostic design is grounded in seams that already exist (verified), but two need work:

1. **Execution plumbing — NT adapters (already uniform).** The strategy submits/cancels through NT's generic order interface; per-venue adapters (Polymarket/Binance/Bybit/Deribit/…) handle venue specifics. The maker MUST NOT branch on a venue name for order plumbing.
2. **Venue facts — the capability contract (seam exists; schema must be EXTENDED).** `src/venue_contract.rs` loads `contracts/<venue>.toml`, but `VenueContract` today carries only `schema_version/venue/adapter_version/streams` (data-stream completeness). **W1 extends it** with typed sections: `supports_modify`, request/rate budget, maintenance window, depth availability, fee schedule, settlement kind — plus fail-closed startup validation when a required maker capability is absent. The quote controller reads these variables; it never hardcodes a venue fact.
3. **Instrument-type math — the market-family seam (exists; binary first).** `MarketIdentityTarget`/`family_key()` in `src/bolt_v3_market_families/` and the maker quote-target family seam are the adapters. Fair-value interpretation, quote layout, settlement, and inventory math plug in here per instrument type. Crypto binary is implemented first; future families are admitted only through W0 anti-hardcode checks — **no speculative universal framework**.

**The corrected model.** PRIMARY = Glosten-Milgrom/Copeland-Galai adverse-selection half-spread (oracle pickoff is the dominant P&L term). SECONDARY = small inventory skew. Anchor = the hoisted oracle fair value (composite spot + naive windowed RV + N(d2) digital) **plus a book-imbalance/micro-price term** (a maker, unlike a taker, must read the venue book). Time-to-expiry **widens** the spread (binary variance ~1/√τ). A-S/GLFT/SVI/Breeden-Litzenberger/realized-kernel are CUT for sub-daily binaries and deferred to perps/longer tenors behind the W3-validation gate.

## Project Structure

### Documentation (this feature)

```text
specs/488-binary-oracle-maker/
├── architecture.md # W0 — architecture gate, anti-hardcode checks, work process
├── spec.md         # WHAT — requirements, robustness-gated user stories, acceptance
├── plan.md         # THIS FILE — implementation sequencing
└── (tasks.md)  # later, via /speckit.tasks — NOT created here
```

### Source Code (new files only; no rewrite of the taker)

```text
contracts/<venue>.toml                 # EXTENDED schema (W1): execution/rate/maintenance/fee/settlement
src/venue_contract.rs                  # EXTENDED struct + fail-closed validation (W1)
src/bolt_v3_market_families/           # target/family validation and fair-value family seam
src/strategies/maker_quote.rs          # maker quote-target family seam and proof families
src/<shared>/settlement_accounting.rs  # shared primitive used by the backtest AND live (FR-004)
src/strategies/binary_oracle_maker.rs  # the maker archetype (W2/W3) — built workstream-by-workstream; live-gated on the backtest
var/mm-research/ (offline)             # backtest-scoring tooling (unbiased replay; diagnostic scripts)
```

**Structure Decision**: New maker archetype + a shared settlement module + an extended venue contract; reuse the taker's fair value/RV/governor/pair-model/order-intent. Nothing in the taker is rewritten; the dual digital is removed only in W8 after NT is authoritative.

## Workstreams (robustness-first order; each fails closed)

**BT — Pre-live backtest (the go-live gate; runs on the *built* maker after W2/W3/W4, before live capital).** Not an upfront edge-existence proof — the mature maker market already establishes the edge exists; BT validates *our implementation's* robustness (net-positive after pickoff + settlement). It **rides on the BTE epic (#437/#438)**, which owns and has landed the engine substrate — it does NOT rebuild any of it (see "Backtest ↔ BTE boundary"). Sub-steps: (a) ingest the full-depth L2 corpus (**preferred: the free pmxt archive**; fallback: bolt's mid/top lake) via BTE's **`src/bte_ingest.rs`** loader behind the **`bte-gate-proof`** feature (`+ nautilus-persistence/cloud` already wired) — no new conversion code; (b) the **shared settlement/accounting** for 0/1 payout (FR-004), one module shared with live — coordinate with BTE's binary-option analytics, no fork; (c) backfill underlying spot externally, aligned by the causal join rule (spot@T → decisions@≥T, key on reference `ts_event`) (FR-005); (d) replay the **built maker** over the full-population corpus (FR-002 — every market, so deployment selection bias does not apply) with NT passive fills (FR-003) — with full-depth L2, fill realism now includes queue position. **Score in the Python research workspace** reading the Rust-written catalog (sanctioned research lane): net = captured-spread − fees − adverse-selection − settlement-loss vs **pre-registered thresholds**. **Gate: no net edge → do not go live.**

**W0 — Architecture gate and work-process lock.** Land `architecture.md`, link it from this spec/plan, and get adversarial approval. This is not implementation. It defines the no-hardcode checks, issue order, PR rules, and handoff instructions future agents must read when conversation context is gone.

**W1 — Foundation & agnostic seams.** Extend the venue capability contract schema + validation (architecture layer 2). Hoist fair value behind the market-family seam (file the correct tracking issue — #451 is the order-submission wrapper, not this). Add the **signed trade-flow subscription** (FR-021; GM + VPIN both need it). Taker hardening: crossed-book + spike guards.

**W2 — Quote Lifecycle / Execution Control (the missing core).** Multi-resting-quote state; requote loop using `supports_modify` (cancel+resubmit) and the rate-budget variable; order-accepted reconciliation + deferred-cancel handling; post-only-reject requote; two-leg cancel-scope; reconnect resync + cancel-all-on-kill. Carries its own no-submit/canary proof against a simulated venue at the real budget. *(Built as pure NT-free modules `src/strategies/quote_lifecycle.rs` + `requote_budget.rs`, slices 1–4 landed: single-leg lifecycle, `supports_modify` modify-in-place branch, two-leg `MarketQuote` + cancel scope, sliding-window requote throttle.)*

**W1B — Market-family seam hardening and proof families.** Replace the former WG product-shaped "lean perp" wording with architecture checks. The seam must expose family-neutral quote targets; binary/updown naming must be quarantined or mechanically renamed out of new shared surfaces; bounded-scalar and continuous-mark proof families must compile through the same seam without changing `quote_lifecycle`, `RequoteBudget`, admission, execution, or strategy core. These proof families are not live-tradeable products and carry no venue adapter, funding, margin, liquidation, or backtest gate.

**W3 — Maker model.** GM/CG half-spread with a defined parameter source + calibration (from the historical full-depth corpus); two-sided YES/NO joint quote + inventory; book-imbalance term; inventory→skew functional form; new kill predicates (σ-floor/basis-cap/τ-floor) as TOML thresholds, fail-closed; graduated governor states (cancel-only/reduce-only/hard-flat/reward-preserving soft-hold); offset-composition precedence + (ε,1−ε) clamp/prune.

**W4 — Settlement & resolution lifecycle.** The shared settlement primitive (built here, also consumed by the backtest — one module, no fork), wired live: handle `InstrumentStatus::Close`, book the 0/1 payout, close positions, redemption path, settlement P&L.

**W5 — Capital reservation, then multi-market.** First a **per-market reserved-collateral gate** (FR-040; worst-case simultaneous fills across resting YES/NO + in-flight + inventory; fails closed) — this cannot wait. Then the portfolio layer (FR-041; bankroll split, market selection, per-market state/health/kill isolation; depends on #41).

**W6 — Observability & ops.** Heartbeat, stale-quote alarm, equity/PnL stream (#409), maintenance-window pull schedule (non-retryable restart), geo/compliance precondition.

**W7 — Rewards (additive; gated on core-edge PASS).** Reward-data bindings (#446/#447), reward-aware spread shaper, quiet-market phantom-LP selector, maker rebates; reward-eligibility-aware pull policy (safety wins).

**W8 — Dual-path kill.** Remove the hand-rolled N(d2) digital once NT greeks/IV is the single fair-value source.

**Sequence**: W0 → W1 → W1B → W2 → RV helper → IV helper → FV helper/adapters → W3 → W4 (incl. shared settlement) → **BT (pre-live backtest gate)** → W5/W6 → W7/W8. Production-grade perps, listed options, sports/politics/weather binaries, and sportsbook-style multi-outcome markets are future family adapters, not part of this first implementation slice. If any future family needs shared-engine changes, it gets a new architecture gate first.

## Implementation Rules

- Robustness-first: build advances workstream by workstream, each fail-closed; no live capital before the pre-live backtest PASS. (Edge existence is assumed, not proven upfront.)
- Verify-everywhere: each workstream ships its own fail-closed proof gate; no advancement on assertion.
- Subagent/review findings are drafts — re-verify each load-bearing claim at the pinned rev before it drives a change.
- No dual paths: one fill truth (NT), one settlement module (backtest↔live), one fair-value path (post-W8).
- No hardcodes: venue facts → contract; model params/thresholds → TOML.
- Never restate the NT rev SHA in docs; reference `Cargo.toml` and read the NT checkout at the rev it pins.

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|---|---|---|
| New maker archetype (not extend taker) | Maker = multi-resting-quote lifecycle; taker is a one-position single-shot machine | Extending the taker would entangle two fundamentally different state machines |
| Extend venue contract schema | Agnostic layer 2 needs execution/rate/maintenance/fee/settlement vars | Hardcoding venue facts in code/root config recreates the drift the contract removes |
| Shared settlement primitive (W4, also used by the backtest) | The backtest's net edge depends on 0/1 settlement; must match live accounting | A throwaway backtest settlement path is a dual-path violation and a false pass |
| Per-market reservation gate before multi-market | Removing the one-position invariant needs a replacement before W5 | Deferring to W5 risks over-quoting/rejects during W2/W3 |

## Backtest Substrate — Investigation Findings (2026-05-31)

Three read-only probes against actual source (NT pinned rev + bolt repo + the S3-lake facts verified read-only 2026-05-30). Each load-bearing claim re-verified at HEAD before landing here.

**A — NT reads the S3 lake (E-037): PROVEN viable, two conditions.**
- `ParquetDataCatalog` exists and `BacktestNode` consumes it natively (`persistence/src/backend/catalog.rs`; `backtest/src/node.rs`). It serves QuoteTick / TradeTick / OrderBookDelta / InstrumentStatus / InstrumentClose / Bar.
- S3 reads are `#[cfg(feature = "cloud")]` (`persistence/src/parquet.rs:516-526,679`); bolt's `nautilus-persistence` dep enables **no features** (`Cargo.toml:38`) → S3 is compiled out today. **Condition 1: enable `cloud`.**
- NT has **no generic "load external parquet files" function** — only `write_data_enum`/`write_to_parquet` from in-memory data, plus a strict on-disk layout (`data/{type}/{instrument_id}/{start}-{end}.parquet`, FixedSizeBinary prices, UInt64 ts, required metadata). **Condition 2: a one-time raw-lake→catalog conversion.** (Alternative — query raw parquet via DataFusion directly — is rejected: it bypasses the NT replay path = a dual path.)
- **Both conditions are ALREADY MET by the BTE epic (#437/#438), not the maker's work:** the `bte-gate-proof` feature wires `nautilus-backtest` + `nautilus-persistence/cloud`, and **`src/bte_ingest.rs`** is the landed config-driven raw→catalog loader (proven on 108k real L2 rows). The backtest *reuses* these. Known BTE-side blocker to a *valid* backtest, surfaced there: the loader writes deltas but **not the instrument** (`catalog.rs:701 write_instruments`) → an L2_MBP backtest needs the instrument resolvable from the catalog; this is BTE's to close, and the maker backtest inherits it.

**B — unbiased corpus: BUILDABLE for the binary side.**
- The S3 lake is full-population (every market, not entry-gated) and **carries ground-truth resolutions in-lake** (`polymarket_market_resolved`) — no Gamma fetch, no selection bias. The entry-biased writer the first probe found (`bolt_v3_decision_evidence.rs`, fires only in `try_submit_entry_order`) is the *un-deployed* route and is irrelevant to the backtest.
- Substrate fidelity (UPDATED 2026-05-31): bolt's own lake is mid/top-level (no L2 queue), but the **free pmxt archive provides full-depth L2** (verified: 21-level ladders + `last_trade_price` trades with BUY/SELL aggressor, ~72M events/hour, covers BTC/ETH/SPX up/down, ~Apr 20–May 25). So **queue-aware passive-fill sim IS possible on the pmxt substrate — prefer it** over the lake. Remaining gap: underlying spot is in neither (see C).

**C — spot backfill: external, with a mandatory causal join rule.**
- Reference feed is config-driven (`reference_venue`/`reference_instrument_id` = `Option<String>`, `taker:295-296`; resolution feed `chainlink_data_streams`). Spot is **not** persisted as a stream — only one `spot_price` per decision snapshot. → backfill historical spot externally (free CEX klines per the calibration-data finding).
- Alignment key = the reference quote's **exchange `ts_event`** (`taker:4110-4112`), not ingest/derivation time. Join rule (fail-closed): **backfilled spot at time T may serve only decisions at time ≥ T**; never join a bar-close price into a mid-bar decision; carry the granularity gap (kline period vs decision cadence) as an explicit look-ahead budget.

## Backtest ↔ BTE boundary (no duplication — single source of truth)

The backtesting **engine** is built in a separate effort (Epic #437/#438, the BTE). The maker backtest must not rebuild it. Ownership split:

| Concern | Owner | Status |
|---|---|---|
| NT backtest engine + `cloud`/`bte-gate-proof` feature wiring | **BTE** | landed (PR #496, 5/5 proof) |
| Raw-lake→NT-catalog ingest (`bte_ingest.rs`) | **BTE** | landed (real-L2 proven); `write_instruments` gap open |
| Polymarket data-fidelity / source decision (mid-top lake vs `pmxt` full-depth archive vs paid Telonex/MarketLens) | **BTE** | RESOLVED 2026-05-31: free pmxt archive = tick-level full-depth L2 + trades, $0 — the substrate |
| Python research read-path (catalog dir-name shim) | **BTE** | landed |
| Shared 0/1 settlement/accounting module | **coordinate** | one module, consumed by both — no fork (Codex #1) |
| Built-maker replay + passive-fill scoring + pre-registered thresholds | **maker (this spec)** | net-new |
| External underlying-spot backfill + causal join | **maker (this spec)** | maker-specific; BTE doesn't need spot |

**Dependency:** the maker backtest is gated on BTE closing the `write_instruments` gap (instrument resolvable from the catalog for an L2_MBP replay). The data-fidelity question is resolved (free pmxt full-depth archive). The scorer can be prototyped against BTE's existing proof catalog meanwhile, but the *verdict* needs valid instrument-resolved data.

## Open Decisions / NEEDS-VERIFY (post-investigation)

- **Pre-registered backtest thresholds — TWO bars, locked (user 2026-05-31: "conduct balanced and strict and compare").** The backtest runs **once**; the single net-edge distribution is scored against **both** gates — the comparison reports clears-Balanced / clears-Strict / neither, which measures implementation robustness (does the built maker net-positive after pickoff + settlement, and how comfortably). Shared floor for both: net edge > 0 with a statistical-significance test (bootstrap over resolved markets); power floor ≥ N simulated passive fills across ≥ M resolved markets; corpus floor ≥ K resolved markets (if the available corpus can't reach K, that is itself an expand-data signal, not a fail). The two bars differ on three dials:

  | Dial | Balanced | Strict |
  |---|---|---|
  | Confidence that net edge > 0 | 95% | 99% |
  | Net edge vs round-trip fees | ≥ 1.5× | ≥ 2× |
  | Adverse-selection ≤ % of gross spread | ≤ 50% | ≤ 35% |

  N/M/K and the exact fee figures are locked *after* the corpus build reveals what the lake holds but *before* any scoring (still pre-registered, not fitted). The fee numbers need the venue fee schedule.
- **NT ExecutionModel fill realism** — with the pmxt full-depth L2 substrate (B), queue-aware fills are now possible; source-prove NT's passive-fill model consumes depth adequately, or justify a documented fill assumption (no custom FillSim without NT-insufficiency proof).
- **GM/CG parameter estimators** (informed-trade probability, adverse-selection magnitude) and their inputs from the corpus — defined in W3; the estimator's data needs must be confirmed present in the full-depth corpus during the corpus build.
