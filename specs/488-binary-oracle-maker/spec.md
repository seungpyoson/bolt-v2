# Feature Specification: Binary Oracle Market-Maker (prove-first, venue/instrument-agnostic)

**Feature Branch**: `docs/488-mm-multi-venue-survey`
**Created**: 2026-05-31
**Status**: Draft — PROVE-FIRST. No strategy build until the P0 edge-proof passes its pre-registered gate.
**Input**: Add a two-sided resting market-maker for Polymarket binary (YES/NO up/down) markets first, then fan out to perps/CEX behind a shared agnostic framework. NT-first, no-hardcode, no-dual-paths, pure-Rust. Tracking issue: #488. Supersedes the original #488 "port MM apparatus from market-maker-rs / A-S-GLFT" framing (overturned by an actual-code audit + an internal 5-lens adversarial review + a Codex adversarial review).

## Overview

bolt-v3 ships one strategy: a single-leg directional **taker** (`binary_oracle_edge_taker`). The next direction is a **maker** that rests two-sided limit quotes on the YES/NO pair, anchored to bolt's existing oracle fair value. The user's own research found market-making more profitable than the taker on pure P&L (rewards excluded), so the *strategy* is chosen; the *implementation* is unproven.

Two principles govern this spec:

1. **Prove-first.** We have repeatedly built-first and proven-later. Not here. No strategy code lands until an offline replay proves a positive maker edge on real historical data, scored with production-equivalent accounting, against thresholds registered **before** the run.
2. **Verify-everywhere.** Every workstream carries its own proof gate. Each gate fails closed.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Offline edge proof before any build (Priority: P0 — the gate)

As the operator, before committing any maker build, I need an offline replay that proves or disproves a positive maker edge on real historical data — scored with the **same** settlement/fee/adverse-selection accounting production will use — against pre-registered numeric thresholds, so the decision to build is evidence-led.

**Why this priority**: This is the entire premise. Every later workstream is gated on a PASS. A FAIL stops the initiative (or sends us to a different instrument/tenor) at near-zero build cost.

**Independent Test**: Run the replay over the data-lake window + ground-truth resolutions using the **unbiased full-candidate corpus** (every market/tick/book/fair-value decision, including no-entry rows), with passive-fill simulation via NT's ExecutionModel and the shared settlement primitive. It emits net-edge metrics and a go/no-go verdict without any new strategy code.

**Acceptance Scenarios**:

1. **Given** the unbiased replay corpus and the shared settlement/accounting primitive, **When** the edge proof runs, **Then** it reports net edge = captured-spread − fees − adverse-selection − settlement-loss against the pre-registered thresholds and renders PASS/FAIL.
2. **Given** a FAIL verdict, **When** any downstream workstream (W1–W8) is attempted, **Then** it is blocked — no maker code proceeds on a failed or absent edge proof.
3. **Given** the entry-only decision-evidence log (`calibration_check.py` corpus), **When** it is offered as the edge gate, **Then** it is rejected as selection-biased and only admitted as a diagnostic.

### User Story 2 - Two-sided resting quotes that survive the venue's real mechanics (Priority: P1)

As the maker, I rest bid+ask quotes on the YES/NO pair and continuously reprice them as the oracle and book move, without stranding orders, over-quoting capital, or exceeding the venue's request budget.

**Why this priority**: This quote-lifecycle controller is the genuinely new core of being a maker; the taker is a single-shot, single-position state machine and provides none of it.

**Independent Test**: Drive the controller against a simulated/sandbox venue that emits accepts/rejects/cancels and enforces the real rate budget; assert no orphaned orders, no rate-limit breach, correct reprice behavior, and full flatten on kill.

**Acceptance Scenarios**:

1. **Given** the venue does not support order modification, **When** a reprice is required, **Then** the controller cancels and resubmits and tolerates the cancel-ack→new-accept gap (no duplicate live quotes, no assumption of in-place amend).
2. **Given** the venue request budget, **When** quotes reprice under load, **Then** the requote rate is throttled to stay within budget (driven by a capability variable, not a code constant).
3. **Given** a kill-switch fires, **When** the maker pulls, **Then** all resting quotes on both legs are cancelled (cancel-all-on-kill), not only a single pending order.
4. **Given** a disconnect/reconnect, **When** the session resumes, **Then** local quote state is reconciled against the venue's accepted-order truth before new quoting.
5. **Given** a post-only quote would cross, **When** the venue rejects it, **Then** the controller requotes at a safe price rather than erroring or spinning into the rate limit.

### User Story 3 - A maker model that prices binaries correctly (Priority: P1)

As the maker, I quote around the oracle fair value with a spread that prices the dominant cost (informed pickoff) and a skew that manages inventory, using math valid for a bounded (0,1) binary near a hard expiry.

**Why this priority**: The spread model **is** the economic edge. The prior A-S/GLFT plan is a units category error for a bounded martingale; it must be replaced, not tuned.

**Independent Test**: Unit + property tests on the quote function: quotes always in (ε, 1−ε); spread responds to the adverse-selection parameter; skew responds to inventory; spread widens (not tightens) as expiry approaches.

**Acceptance Scenarios**:

1. **Given** the corrected model, **When** quotes are computed, **Then** the primary half-spread is a Glosten-Milgrom / Copeland-Galai adverse-selection spread with a defined parameter source and calibration, not an Avellaneda-Stoikov/GLFT inventory-control formula.
2. **Given** the complementary YES/NO pair, **When** both legs are quoted, **Then** they are kept jointly consistent (P(NO)=1−P(YES)) and inventory is tracked across the pair, never producing a self-crossing/arbitrageable joint quote.
3. **Given** a frozen feed, stale σ, basis blowout, or near-zero time-to-expiry, **When** the corresponding kill predicate trips, **Then** the maker fails closed (pulls/flattens) per a TOML-configured threshold.

### User Story 4 - Inventory settles correctly at resolution (Priority: P1)

As the operator, when a market resolves I need held inventory to settle to its 0/1 payout in our accounting, because settlement is the dominant P&L term for a binary maker and the platform does not auto-settle it for us.

**Why this priority**: A resting maker is structurally left holding inventory into expiry; without settlement accounting, both live P&L and the backtest are systematically wrong.

**Independent Test**: Feed a resolution event to the settlement module; assert the 0/1 payout is booked, the position closes, and the realized P&L matches the payout (not mark-to-mid). The **same** module is exercised by P0.

**Acceptance Scenarios**:

1. **Given** a market-resolution event (`InstrumentStatus::Close`), **When** the maker holds inventory, **Then** the shared settlement primitive books the terminal 0/1 payout and closes the position.
2. **Given** P0 and live both compute settlement, **When** either runs, **Then** they use the **same** settlement/accounting module (no dual settlement path).

### User Story 5 - Safe under capital, then across many markets (Priority: P2)

As the operator, the maker must never commit more capital than reserved for worst-case simultaneous fills, first within a single market, then across the many markets a maker quotes.

**Why this priority**: Removing the taker's one-position invariant without a replacement reservation model risks over-quoting and exchange rejects. The single-market reservation cannot wait for the multi-market capital layer.

**Independent Test**: With multiple resting quotes + in-flight cancel/resubmit + open inventory, assert the worst-case simultaneous-fill liability never exceeds reserved collateral; submission fails closed otherwise.

**Acceptance Scenarios**:

1. **Given** resting YES/NO quotes, in-flight cancel/resubmit orders, and existing inventory, **When** a new quote is submitted, **Then** a per-market reserved-collateral gate verifies worst-case liability and fails closed if exceeded.
2. **Given** multiple markets, **When** the maker allocates capital, **Then** a portfolio layer splits bankroll, selects markets, and isolates per-market state/health/kill.

### User Story 6 - Observable and operable while live (Priority: P2)

As the operator, a continuous two-sided quoter must be observable (heartbeat, stale-quote alarm, real-time equity/PnL) and survive scheduled venue maintenance and geo constraints.

**Acceptance Scenarios**:

1. **Given** the maker is live, **When** quotes go stale, equity drops, or a requote loop runs away, **Then** an alarm/heartbeat surfaces it and the relevant kill state engages.
2. **Given** the venue's weekly maintenance restart (returns a non-retryable status), **When** the window approaches, **Then** the maker pulls quotes on a maintenance-window schedule rather than relying on retry.
3. **Given** venue geo/compliance constraints, **When** the maker is started, **Then** a compliance precondition is checked before live quoting.

### User Story 7 - Reward/rebate capture (Priority: P3 — additive, gated on core-edge PASS)

As the operator, once the core trading edge is proven net-positive, I want to additively capture platform LP rewards, maker rebates, and quiet-market incentives — without compromising the core maker.

**Acceptance Scenarios**:

1. **Given** a core-edge PASS, **When** reward capture is enabled, **Then** reward-data bindings, a reward-aware spread shaper, and a quiet-market selector layer on top of the maker without changing its core spread logic.
2. **Given** the kill-switch pulls quotes, **When** reward eligibility (continuous-resting) is in tension with safety, **Then** a defined reward-eligibility-aware pull policy reconciles them (safety wins; reward loss is accepted, not silently traded off).

### User Story 8 - Single source of truth for fair value (Priority: P2)

As the maintainer, the maker must price from one fair-value path so there is no dual digital pricer to drift.

**Acceptance Scenarios**:

1. **Given** the hoisted fair-value module behind the MarketFamily seam, **When** the maker and taker both price, **Then** they use the same path; the hand-rolled `standard_normal_cdf` digital is removed once NT greeks/IV is the single source.

### Edge Cases

- Venue maintenance restart returns a **non-retryable** status mid-quote → orders may strand; requires scheduled pre-restart pull.
- Reconnect after a transient disconnect → orphaned resting orders if not reconciled.
- Crossed/locked book (`bid > ask`) and single-tick spikes → no guard exists in the taker today.
- Frozen/stale feed → σ collapses, digital rails to 0/1; must be caught by the forced-flat governor + new kill predicates.
- Near-expiry: binary ATM variance blows up ~1/√τ → spread must widen, never tighten.
- Thin books (often 1–2 levels) → multi-level/depth quoting is degenerate.
- Selection-biased corpus → entry-only logs cannot prove a maker edge.
- Doc/rev drift → specs citing a stale NT rev mislead source-proofing.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001 (P0)**: The system MUST produce an offline edge proof scored as net = captured-spread − fees − adverse-selection − settlement-loss over the historical lake window, against thresholds registered before the run, with a PASS/FAIL verdict.
- **FR-002 (P0)**: The edge-proof corpus MUST be the unbiased full-candidate set (every market/tick/book/fair-value decision incl. no-entry reasons, top-of-book, quote eligibility). The entry-only decision-evidence log MUST NOT be the gate.
- **FR-003 (P0)**: The edge proof MUST use NT's ExecutionModel for fills; a custom fill model MUST NOT be introduced unless NT's is first source-proven insufficient (no dual fill truth).
- **FR-004 (P0/W4)**: A single shared settlement/accounting primitive MUST exist before the edge gate and MUST be reused by both the edge proof and live (no dual settlement path).
- **FR-005 (P0)**: Underlying spot for the lake window MUST be backfilled and point-in-time aligned to the oracle the strategy actually saw (look-ahead controlled), since the maker mid is the oracle fair value (a function of spot).
- **FR-010 (W2)**: The quote-lifecycle controller MUST reprice via cancel+resubmit where the venue lacks order modification, tolerating the un-quoted gap and avoiding duplicate live quotes.
- **FR-011 (W2)**: Requote rate MUST be throttled to the venue request budget, sourced from a capability variable (not a code constant).
- **FR-012 (W2)**: The controller MUST track the full set of live resting quotes, reconcile against the venue's accepted-order truth (handle order-accepted + deferred-cancel), and cancel all resting quotes on kill.
- **FR-013 (W2)**: On post-only rejection, the controller MUST requote at a safe price.
- **FR-020 (W3)**: The primary half-spread MUST be a Glosten-Milgrom/Copeland-Galai adverse-selection spread with a defined parameter source and calibration. A-S/GLFT/SVI/Breeden-Litzenberger MUST NOT be used for the sub-daily binary (deferred to perps/longer tenors behind a backtest gate).
- **FR-021 (W3)**: GM/CG and any toxicity signal (VPIN) require signed order flow; the maker MUST subscribe to trades and classify aggressor side (no reliance on a test fixture).
- **FR-022 (W3)**: Quotes MUST always land in (ε, 1−ε); the YES/NO pair MUST stay jointly consistent and non-self-crossing; offset composition (spread, skew, time-widening, reward-shaping) MUST have a defined precedence + clamp/prune rule.
- **FR-023 (W3)**: Inventory→skew MUST be a defined functional form; kill predicates (σ-floor, basis-cap, τ-floor, plus the existing stale/thin/incoherent) MUST be TOML thresholds that fail closed. The governor MUST express graduated states (cancel-only / reduce-only / hard-flat / reward-preserving soft-hold), not a single boolean.
- **FR-030 (W4)**: On resolution (`InstrumentStatus::Close`), the system MUST book the terminal 0/1 payout, close the position, and feed settlement P&L; the platform does not auto-settle.
- **FR-040 (W5)**: A per-market reserved-collateral gate MUST verify worst-case simultaneous-fill liability across resting quotes + in-flight cancel/resubmit + inventory and fail closed; this MUST NOT wait for the multi-market layer.
- **FR-041 (W5)**: A portfolio layer MUST split capital, select markets, and isolate per-market state/health/kill.
- **FR-050 (W6)**: The system MUST provide heartbeat, stale-quote alarm, real-time equity/PnL, a maintenance-window pull schedule, and a venue geo/compliance precondition.
- **FR-060 (W7)**: Reward capture MUST be additive and gated on a core-edge PASS; a reward-eligibility-aware pull policy MUST reconcile reward continuity with the safety kill (safety wins).
- **FR-070 (W8)**: Fair value MUST resolve through one path; the hand-rolled digital MUST be removed once NT greeks/IV is authoritative.
- **FR-080 (agnostic)**: Venue-specific facts (modify support, request budget, maintenance window, depth availability, fee schedule, settlement kind) MUST be variables in the venue capability contract, read by the controller — the controller MUST NOT branch on a venue name.
- **FR-081 (agnostic)**: Instrument-type math (pricing/settlement/inventory) MUST plug in behind the `MarketFamily` seam; only the Polymarket-binary path is implemented first.

### Key Entities

- **EdgeProofCorpus**: the unbiased full-candidate replay set (markets × ticks × book states × fair-value decisions × no-entry reasons × resolutions). Source of the P0 verdict.
- **SettlementAccount**: shared primitive that books the 0/1 terminal payout for held inventory; used by both P0 and live.
- **VenueCapabilityContract**: extended `contracts/<venue>.toml` — adds typed execution / rate-limit / maintenance / fee / settlement sections to the current data-stream-only schema.
- **MarketFamily**: instrument-type seam (`MarketIdentityTarget`/`family_key()`) carrying pricing + settlement + inventory math; binary first.
- **QuoteSet**: the controller's model of all live resting quotes (per leg, per market) with accept/cancel/in-flight state.
- **MakerModel**: GM/CG half-spread + inventory skew + time-widening + clamp/precedence, anchored to the hoisted fair value + a book-imbalance term.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: P0 renders a PASS/FAIL on net edge vs pre-registered thresholds using the unbiased corpus and production-equivalent accounting; no maker code merges before a PASS.
- **SC-002**: 100% of generated quotes land in (ε, 1−ε), proven by a property test; zero self-crossing joint YES/NO quotes.
- **SC-003**: Under a simulated venue at the real request budget, the controller breaches neither the rate budget nor leaves orphaned/duplicate quotes across reprice, kill, and reconnect.
- **SC-004**: Kill predicates and the per-market reserved-collateral gate fire (fail closed) on synthetic adverse inputs in an NT backtest.
- **SC-005**: Settlement P&L for held inventory matches the 0/1 payout (not mark-to-mid) in both P0 and live, via one module.
- **SC-006**: No dual paths — single fill-truth (NT ExecutionModel), single settlement module, single fair-value path; verified by review.
- **SC-007**: No venue-name branch in maker logic; all venue facts resolve from the capability contract; verified by review/grep.

## Assumptions

- **NT pin = `6e059dc`** (Cargo.toml is the source of truth). NOTE: ~20 tracked specs/docs cite a stale `7c2aafb`; that drift is a **separate hygiene issue**, not fixed here — but this spec and #488 cite `6e059dc`, and source-proofs must compare documented revs against Cargo.toml.
- Verified venue/runtime facts (NT rev 6e059dc): Polymarket adapter has **no order modify** (`execution/mod.rs:1272`) → cancel+resubmit; REST budget = **100/minute** (`rate_limits.rs` `Quota::per_minute`, `consts.rs:81`) vs bolt config `100/second` (`config/root.toml:84-85`); **`order_book_depths` disabled** (L2 deltas only, no per-order queue identity) (`contracts/polymarket.toml:20-23`); NT does **not auto-settle** — resolution emits only `InstrumentStatus::Close` (`data.rs:1052`) and the taker has **zero** resolution handling; strategy **does not subscribe to trades** (`AggressorSide` only in a test fixture) so GM/CG + VPIN are starved today; maintenance-restart status is **non-retryable** (`http/error.rs` `is_retryable` = `status>=500`).
- The venue capability contract schema must be **extended** — `VenueContract` (`src/venue_contract.rs:68-72`) currently carries only `schema_version/venue/adapter_version/streams` and cannot hold the maker capability variables.
- The data lake is **sparse** (~3 weeks, thin books), holds only 0–1 prices + ground-truth resolutions; underlying spot is **not** in the lake (backfill externally). Fill realism — not data volume — is the backtest's binding constraint.
- NT owns execution, position/PnL, pre-trade limits, backtest engine + returns analytics, and VPIN; per the accepted BTE spec, **no Bolt-owned fill simulator** unless NT is source-proven unable.
- `binary_oracle_edge_taker` already provides composite-spot fair value, naive windowed RV, the `updown` YES/NO pair model, the forced-flat governor, decision-evidence/submit-admission gating, and the order-intent post-only limit build — reused, not rebuilt.
- This is a planning/research artifact. DO NOT BUILD until P0 passes. Work lives on `docs/488-mm-multi-venue-survey`, not the disk-retention branch.

## References

- Tracking issue: **#488** (umbrella; being renamed/rewritten to this plan).
- Authoritative actual-code audit: `docs/research/mm-code-audit-2026-05-30.md`.
- Reward research: `docs/bolt-v3/research/polymarket-rewards-2026-05-13.md`.
- Implementation approach: `specs/488-binary-oracle-maker/plan.md`.
- Precursors (execution plumbing only — they stop at NT OrderFactory construction): `specs/022-nt-maker-order-scope`, `specs/023-nt-order-intent-layer`, `specs/023-nt-research-analytics-platform/1-backtesting-engine`.
- Related issues: #451 (order admission/submission wrapper — NOT fair-value), #41 (multi-market substrate), #409 (PortfolioSnapshot equity stream), #446/#447 (fee/reward-data provider decoupling), #135/#133 (sibling strategy lines).
