# Implementation Plan: Binary Oracle Market-Maker (prove-first, venue/instrument-agnostic)

**Branch**: `docs/488-mm-multi-venue-survey` | **Date**: 2026-05-31 | **Spec**: `specs/488-binary-oracle-maker/spec.md`
**Input**: Feature specification from `specs/488-binary-oracle-maker/spec.md`. Tracking: #488.

## Summary

A two-sided resting market-maker for Polymarket binary (YES/NO) markets, anchored to bolt's existing oracle fair value. The economic edge is the oracle mid priced against informed flow — so the **primary spread is a Glosten-Milgrom/Copeland-Galai adverse-selection half-spread**, with inventory skew secondary. Build is **gated on a prove-first offline edge proof (P0)** scored with production-equivalent accounting. The design is **agnostic via three layers** (NT execution adapters / an extended venue capability contract / a per-instrument `MarketFamily` math seam); only the Polymarket-binary path is implemented first. This plan supersedes the original #488 framing ("port the market-maker-rs apparatus; grid→GLFT; anchor an A-S reservation price"), which an actual-code audit + an internal 5-lens review + a Codex review overturned: A-S/GLFT are a units category error for a bounded (0,1) martingale whose variance blows up ~1/√τ into expiry.

## Technical Context

**Language/Version**: Rust (edition per workspace).
**Primary Dependencies**: NautilusTrader Rust crates, at the rev **pinned in `Cargo.toml`** (the single source of truth — this plan does not restate the SHA, so it cannot drift). NT owns execution, position/PnL, pre-trade limits, backtest engine + returns analytics (Sharpe/Sortino/Calmar), VPIN, greeks/implied-vol. Source-proofs MUST read the NT checkout at the rev `Cargo.toml` currently pins. A separate hygiene issue tracks ~20 specs/docs that hard-code a now-stale rev.
**Storage**: Historical S3 parquet data lake — **full-population** (every market: `polymarket_book` mid-over-time, `polymarket_trades`, `polymarket_new_market`, `polymarket_market_resolved` ground-truth outcomes; date-partitioned; ~3 weeks, sparse; **mid/top-level book only — no L2 queue depth**; **no underlying spot**). Distinct from the in-repo decision-evidence JSONL writer, which is entry-gated/selection-biased **and not deployed** — P0 does not use it. NT `ParquetDataCatalog` reads S3 **only with the `cloud` feature** (off in bolt today) and ships **no generic external-parquet loader** — a one-time raw→catalog conversion is required (E-037 investigated 2026-05-31; see "P0 Substrate — Investigation Findings").
**Testing**: `cargo test` (unit + property), NT backtest; `cargo fmt`/`clippy`/`deny` clean (CI on main).
**Target Platform**: Linux (EC2 LiveNode); Polymarket binary first, perps/CEX later.
**Project Type**: Single Rust project (NT `LiveNode` strategy archetype + offline replay tooling).
**Constraints**: NO HARDCODES (TOML), PURE RUST (no C/FFI/Python in the binary; offline data-prep scripts exempt), NO DUAL PATHS, SSM-only secrets, GROUP BY CHANGE.
**Scale/Scope**: Many concurrent thin binary markets; per-market caps rolling up to a portfolio budget.

## Constitution Check

*GATE: must pass before any build. Re-checked per workstream.*

- **NO HARDCODES** → venue facts in `contracts/<venue>.toml`, model params + thresholds in strategy TOML. ✅ by design (FR-080, FR-023).
- **NO DUAL PATHS** → single fill truth (NT ExecutionModel; custom fill only if NT source-proven insufficient), single settlement module (shared P0↔live), single fair-value path (W8 kills the hand-rolled digital). ✅ (FR-003, FR-004, FR-070).
- **PURE RUST BINARY** → strategy + controller are Rust on NT; offline edge-proof tooling may be a script but produces no runtime dependency. ✅.
- **SSM single secret source** → unchanged; reuse existing credential resolution. ✅.
- **GROUP BY CHANGE** → swapping venue touches one contract file + config; instrument type touches one `MarketFamily` impl. ✅ once the contract schema is extended (W1).
- **ONE BRANCH / ONE SCOPE** → all work on `docs/488-mm-multi-venue-survey`; **prove-first** means P0 is the only thing that runs until it passes.

## Architecture — agnostic in three layers

The agnostic design is grounded in seams that already exist (verified), but two need work:

1. **Execution plumbing — NT adapters (already uniform).** The strategy submits/cancels through NT's generic order interface; per-venue adapters (Polymarket/Binance/Bybit/Deribit/…) handle venue specifics. The maker MUST NOT branch on a venue name for order plumbing.
2. **Venue facts — the capability contract (seam exists; schema must be EXTENDED).** `src/venue_contract.rs` loads `contracts/<venue>.toml`, but `VenueContract` today carries only `schema_version/venue/adapter_version/streams` (data-stream completeness). **W1 extends it** with typed sections: `supports_modify`, request/rate budget, maintenance window, depth availability, fee schedule, settlement kind — plus fail-closed startup validation when a required maker capability is absent. The quote controller reads these variables; it never hardcodes a venue fact.
3. **Instrument-type math — the `MarketFamily` seam (exists; binary only).** `MarketIdentityTarget`/`family_key()` in `src/bolt_v3_market_families/` is the trait seam. Pricing + settlement + inventory math plug in here per instrument type. Binary (digital fair value, 0/1 settlement, bounded-martingale spread) is implemented first; perps/spot are added behind the same seam when a second venue/instrument actually arrives — **no speculative universal framework**.

**The corrected model.** PRIMARY = Glosten-Milgrom/Copeland-Galai adverse-selection half-spread (oracle pickoff is the dominant P&L term). SECONDARY = small inventory skew. Anchor = the hoisted oracle fair value (composite spot + naive windowed RV + N(d2) digital) **plus a book-imbalance/micro-price term** (a maker, unlike a taker, must read the venue book). Time-to-expiry **widens** the spread (binary variance ~1/√τ). A-S/GLFT/SVI/Breeden-Litzenberger/realized-kernel are CUT for sub-daily binaries and deferred to perps/longer tenors behind the W3-validation gate.

## Project Structure

### Documentation (this feature)

```text
specs/488-binary-oracle-maker/
├── spec.md     # WHAT — requirements, prove-first user stories, acceptance
├── plan.md     # THIS FILE — architecture + workstream sequencing
└── (tasks.md)  # later, via /speckit.tasks — NOT created here
```

### Source Code (new files only; no rewrite of the taker)

```text
contracts/<venue>.toml                 # EXTENDED schema (W1): execution/rate/maintenance/fee/settlement
src/venue_contract.rs                  # EXTENDED struct + fail-closed validation (W1)
src/bolt_v3_market_families/           # binary pricing+settlement+inventory behind the seam
src/<shared>/settlement_accounting.rs  # shared primitive used by P0 AND live (FR-004)
src/strategies/binary_oracle_maker.rs  # the maker archetype (W2/W3) — built only after P0 PASS
var/mm-research/ (offline)             # P0 edge-proof tooling (unbiased replay; diagnostic scripts)
```

**Structure Decision**: New maker archetype + a shared settlement module + an extended venue contract; reuse the taker's fair value/RV/governor/pair-model/order-intent. Nothing in the taker is rewritten; the dual digital is removed only in W8 after NT is authoritative.

## Workstreams (prove-first order; each fails closed)

**P0 — Edge proof (first; LEAN, one-time; no strategy code, no engine build).** P0 is a thin experiment that **rides on the BTE epic (#437/#438)**, which already owns and has landed the engine substrate — it does NOT rebuild any of it (see "P0 ↔ BTE boundary"). Concrete sub-steps, in order: (a) ingest the lake slice via BTE's **`src/bte_ingest.rs`** loader behind the existing **`bte-gate-proof`** feature (`+ nautilus-persistence/cloud` already wired) — no new conversion code; (b) the **shared settlement/accounting** for 0/1 payout (FR-004) — coordinate ownership with BTE's binary-option analytics, one module, no fork; (c) backfill underlying spot externally and align by the causal join rule (spot@T → decisions@≥T, key on reference `ts_event`) (FR-005); (d) replay the existing oracle fair value over the full-population corpus (FR-002 — built fresh over every market, so the deployed strategy's selection bias does not apply) as resting quotes with NT passive fills (FR-003), noting the mid/top-only book bounds queue realism. **Score in the Python research workspace** reading the Rust-written catalog (sanctioned research-lane; no Rust scorer needed for a one-time test): net = captured-spread − fees − adverse-selection − settlement-loss vs **pre-registered thresholds**; check statistical power on the sparse lake. **Gate: no edge → stop. Output is a verdict, not an apparatus.**

**W1 — Foundation & agnostic seams.** Extend the venue capability contract schema + validation (architecture layer 2). Hoist fair value behind the `MarketFamily` seam (file the correct tracking issue — #451 is the order-submission wrapper, not this). Add the **signed trade-flow subscription** (FR-021; GM + VPIN both need it). Taker hardening: crossed-book + spike guards.

**W2 — Quote Lifecycle / Execution Control (the missing core).** Multi-resting-quote state; requote loop using `supports_modify` (cancel+resubmit) and the rate-budget variable; order-accepted reconciliation + deferred-cancel handling; post-only-reject requote; two-leg cancel-scope; reconnect resync + cancel-all-on-kill. Carries its own no-submit/canary proof against a simulated venue at the real budget.

**W3 — Maker model.** GM/CG half-spread with a defined parameter source + calibration (from P0 data); two-sided YES/NO joint quote + inventory; book-imbalance term; inventory→skew functional form; new kill predicates (σ-floor/basis-cap/τ-floor) as TOML thresholds, fail-closed; graduated governor states (cancel-only/reduce-only/hard-flat/reward-preserving soft-hold); offset-composition precedence + (ε,1−ε) clamp/prune.

**W4 — Settlement & resolution lifecycle.** The shared primitive from P0, wired live: handle `InstrumentStatus::Close`, book the 0/1 payout, close positions, redemption path, settlement P&L. (Built early as part of P0; W4 is its live wiring.)

**W5 — Capital reservation, then multi-market.** First a **per-market reserved-collateral gate** (FR-040; worst-case simultaneous fills across resting YES/NO + in-flight + inventory; fails closed) — this cannot wait. Then the portfolio layer (FR-041; bankroll split, market selection, per-market state/health/kill isolation; depends on #41).

**W6 — Observability & ops.** Heartbeat, stale-quote alarm, equity/PnL stream (#409), maintenance-window pull schedule (non-retryable restart), geo/compliance precondition.

**W7 — Rewards (additive; gated on core-edge PASS).** Reward-data bindings (#446/#447), reward-aware spread shaper, quiet-market phantom-LP selector, maker rebates; reward-eligibility-aware pull policy (safety wins).

**W8 — Dual-path kill.** Remove the hand-rolled N(d2) digital once NT greeks/IV is the single fair-value source.

**Sequence**: P0 (incl. shared settlement) → W1 + W2 → W3 → **(W3 validation gate)** → W5/W6 → W7/W8. The validation gate decides whether any cut math (A-S/GLFT/SVI) returns for later instruments.

## Implementation Rules

- Prove-first: no maker archetype code merges before a P0 PASS.
- Verify-everywhere: each workstream ships its own fail-closed proof gate; no advancement on assertion.
- Subagent/review findings are drafts — re-verify each load-bearing claim at the pinned rev before it drives a change.
- No dual paths: one fill truth (NT), one settlement module (P0↔live), one fair-value path (post-W8).
- No hardcodes: venue facts → contract; model params/thresholds → TOML.
- Never restate the NT rev SHA in docs; reference `Cargo.toml` and read the NT checkout at the rev it pins.

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|---|---|---|
| New maker archetype (not extend taker) | Maker = multi-resting-quote lifecycle; taker is a one-position single-shot machine | Extending the taker would entangle two fundamentally different state machines |
| Extend venue contract schema | Agnostic layer 2 needs execution/rate/maintenance/fee/settlement vars | Hardcoding venue facts in code/root config recreates the drift the contract removes |
| Shared settlement primitive before P0 | P0's net edge depends on 0/1 settlement; must match live accounting | A throwaway P0 settlement path is a dual-path violation and a false edge pass |
| Per-market reservation gate before multi-market | Removing the one-position invariant needs a replacement before W5 | Deferring to W5 risks over-quoting/rejects during W2/W3 |

## P0 Substrate — Investigation Findings (2026-05-31)

Three read-only probes against actual source (NT pinned rev + bolt repo + the S3-lake facts verified read-only 2026-05-30). Each load-bearing claim re-verified at HEAD before landing here.

**A — NT reads the S3 lake (E-037): PROVEN viable, two conditions.**
- `ParquetDataCatalog` exists and `BacktestNode` consumes it natively (`persistence/src/backend/catalog.rs`; `backtest/src/node.rs`). It serves QuoteTick / TradeTick / OrderBookDelta / InstrumentStatus / InstrumentClose / Bar.
- S3 reads are `#[cfg(feature = "cloud")]` (`persistence/src/parquet.rs:516-526,679`); bolt's `nautilus-persistence` dep enables **no features** (`Cargo.toml:38`) → S3 is compiled out today. **Condition 1: enable `cloud`.**
- NT has **no generic "load external parquet files" function** — only `write_data_enum`/`write_to_parquet` from in-memory data, plus a strict on-disk layout (`data/{type}/{instrument_id}/{start}-{end}.parquet`, FixedSizeBinary prices, UInt64 ts, required metadata). **Condition 2: a one-time raw-lake→catalog conversion.** (Alternative — query raw parquet via DataFusion directly — is rejected: it bypasses the NT replay path = a dual path.)
- **Both conditions are ALREADY MET by the BTE epic (#437/#438), not P0's work:** the `bte-gate-proof` feature wires `nautilus-backtest` + `nautilus-persistence/cloud`, and **`src/bte_ingest.rs`** is the landed config-driven raw→catalog loader (proven on 108k real L2 rows). P0 *reuses* these. Known BTE-side blocker to a *valid* backtest, surfaced there: the loader writes deltas but **not the instrument** (`catalog.rs:701 write_instruments`) → an L2_MBP backtest needs the instrument resolvable from the catalog; this is BTE's to close, and P0 inherits it.

**B — unbiased corpus: BUILDABLE for the binary side.**
- The S3 lake is full-population (every market, not entry-gated) and **carries ground-truth resolutions in-lake** (`polymarket_market_resolved`) — no Gamma fetch, no selection bias. The entry-biased writer the first probe found (`bolt_v3_decision_evidence.rs`, fires only in `try_submit_entry_order`) is the *un-deployed* route and is irrelevant to P0.
- Residual gaps that bound P0 fidelity (not blockers): (1) book is **mid/top-level, no L2 queue** (Polymarket adapter `order_book_depths` disabled) → a passive-fill sim cannot model queue position, only "a trade crossed my resting price"; (2) underlying spot absent (see C).

**C — spot backfill: external, with a mandatory causal join rule.**
- Reference feed is config-driven (`reference_venue`/`reference_instrument_id` = `Option<String>`, `taker:295-296`; resolution feed `chainlink_data_streams`). Spot is **not** persisted as a stream — only one `spot_price` per decision snapshot. → backfill historical spot externally (free CEX klines per the calibration-data finding).
- Alignment key = the reference quote's **exchange `ts_event`** (`taker:4110-4112`), not ingest/derivation time. Join rule (fail-closed): **backfilled spot at time T may serve only decisions at time ≥ T**; never join a bar-close price into a mid-bar decision; carry the granularity gap (kline period vs decision cadence) as an explicit look-ahead budget.

## P0 ↔ BTE boundary (no duplication — single source of truth)

The backtesting **engine** is built in a separate effort (Epic #437/#438, the BTE). P0 must not rebuild it. Ownership split:

| Concern | Owner | Status |
|---|---|---|
| NT backtest engine + `cloud`/`bte-gate-proof` feature wiring | **BTE** | landed (PR #496, 5/5 proof) |
| Raw-lake→NT-catalog ingest (`bte_ingest.rs`) | **BTE** | landed (real-L2 proven); `write_instruments` gap open |
| Polymarket data-fidelity / source decision (mid-top self-capture vs `archive.pmxt.dev` L2 vs Telonex) | **BTE** | open (the real gate on a *valid* backtest) |
| Python research read-path (catalog dir-name shim) | **BTE** | landed |
| Shared 0/1 settlement/accounting module | **coordinate** | one module, consumed by both — no fork (Codex #1) |
| Oracle-fair-value-as-resting-quote replay + passive-fill scoring + thresholds | **P0 (this spec)** | the only net-new work |
| External underlying-spot backfill + causal join | **P0 (this spec)** | maker-specific; BTE doesn't need spot |

**Dependency:** P0 is gated on BTE closing the `write_instruments` gap and the Polymarket data-fidelity decision. P0 can prototype the scorer against BTE's existing proof catalog meanwhile, but the *verdict* needs valid instrument-resolved data.

## Open Decisions / NEEDS-VERIFY (post-investigation)

- **Pre-registered P0 thresholds — TWO bars, locked (user 2026-05-31: "conduct balanced and strict and compare").** P0 runs **once**; the single net-edge distribution is scored against **both** gates — the comparison reports clears-Balanced / clears-Strict / neither, which measures edge robustness. Shared floor for both: net edge > 0 with a statistical-significance test (bootstrap over resolved markets); power floor ≥ N simulated passive fills across ≥ M resolved markets; corpus floor ≥ K resolved markets (if the sparse ~3-week lake can't reach K, that is itself a STOP / expand-data signal, not a fail). The two bars differ on three dials:

  | Dial | Balanced | Strict |
  |---|---|---|
  | Confidence that net edge > 0 | 95% | 99% |
  | Net edge vs round-trip fees | ≥ 1.5× | ≥ 2× |
  | Adverse-selection ≤ % of gross spread | ≤ 50% | ≤ 35% |

  N/M/K and the exact fee figures are locked *after* the corpus build reveals what the lake holds but *before* any scoring (still pre-registered, not fitted). The fee numbers need the venue fee schedule.
- **NT ExecutionModel fill realism on a mid/top-only book** — source-prove NT's passive-fill model is adequate, or justify a documented fill assumption (no custom FillSim without NT-insufficiency proof). The no-L2-queue limitation (B) is the binding constraint here.
- **GM/CG parameter estimators** (informed-trade probability, adverse-selection magnitude) and their inputs from the corpus — defined in W3, but the estimator's data needs must be confirmed present in the corpus during P0.
