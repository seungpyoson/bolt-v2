# NT-First Market-Family Architecture Gate

**Status**: Draft for external adversarial approval.
**Scope owner**: #488.
**Current branch**: `feat/488-w3-pricing`. Older references to `docs/488-mm-multi-venue-survey` are superseded for this slice.
**First consumer**: crypto binary-option market making.

## Decision

Use Option B: reuse the existing seams and make them enforceable before building more strategy logic.

Do not add a new generic valuation platform up front. In particular, do not introduce a parallel `MarketContract`, generic `EvidenceSnapshot`, generic `ValuationSnapshot`, or generic `ValuationModel` core while `VenueContract` and the maker family seam already exist.

Do not hardcode the crypto binary path and hope to generalize later. Shared modules must be family-neutral and venue-neutral now, with tests that prove a second family can plug in without edits to the quote lifecycle, admission, execution, or strategy core.

The first real implementation remains crypto binary options. Other markets are represented as seam invariants and compile-time proof families until there is a real issue to build them.

## Architecture

The flow is:

```text
TOML config + NT instrument metadata
        |
        v
VenueContract + market-family config
        |
        v
shared evidence helpers: RV, IV, book/flow, domain-specific evidence
        |
        v
market-family adapter computes fair value, quote targets, inventory projection, settlement rule
        |
        v
shared quote lifecycle + requote budget + admission + execution intent
        |
        v
NT adapters own venue execution
```

### Layer 1: NT Execution Adapters

NT owns order submission, cancellation, fills, positions, PnL, backtest replay, pre-trade limits where available, option greeks/IV where available, and venue adapters. Bolt shared code submits intent through NT; it does not branch on a venue name to decide execution mechanics.

Before Bolt builds a primitive, the issue must source-prove whether NT already provides it at the rev pinned by `Cargo.toml`. If NT provides it, Bolt wraps or adapts NT. If NT does not provide it, the issue must record the source proof before building the missing helper.

### Layer 2: VenueContract

`VenueContract` owns venue facts:

- order modify support
- request/rate budget
- maintenance windows
- stream availability and provenance
- fee schedule
- settlement kind
- venue-specific limits that affect admissibility

Shared quote/admission/execution code reads these fields. It must not match on `"polymarket"`, `"deribit"`, `"bybit"`, `"sportsbook"`, or any other venue string.

### Layer 3: Market-Family Adapter

A market family owns instrument math and payout structure:

- fair-value interpretation
- quote-target layout
- inventory projection
- settlement rule
- family-local kill predicates

The shared engine consumes family-neutral quote targets and settlement outputs. It must not know whether a target came from a binary option, listed option, perpetual future, sports market, politics market, or sportsbook-style outcome market.

The current binary/updown path is allowed as the first real family, but binary names must stay inside binary-specific modules and strategy-local display/evidence fields. New shared surfaces must use neutral names such as `leg_a`, `leg_b`, `outcome`, `quote_target`, `settlement_kind`, and `family_key`.

## RV, IV, And FV

RV and IV are separate shared helpers. They are not buried inside a strategy and they are not one pricing layer.

- RV helper: realized-volatility estimation from observed underlying/market prices.
- IV helper: implied-volatility/surface inputs from listed option markets when observable through NT or an NT-backed adapter.
- FV helper: fair-value orchestration that asks the relevant market-family adapter which evidence it needs and returns family-specific fair-value inputs for quote construction.

Crypto price binaries can use RV and may use IV as an overlay or longer-horizon input. Listed options can use NT option greeks/IV directly where available. Perpetual futures may use mark/index/funding/book evidence rather than IV. Weather, sports, and politics binaries are still binary options economically, but they do not automatically have meaningful RV/IV; their FV comes from domain evidence such as forecasts, polls, odds, or model probabilities. The architecture must allow those evidence sources without changing quote lifecycle, admission, or execution.

## Anti-Hardcode Checks

These checks are part of the architecture gate and must become CI or review gates before downstream workstreams claim the architecture is agnostic.

1. **No venue-name branching in shared code**
   - Shared quote lifecycle, requote budget, admission, execution intent, and settlement orchestration must not branch on venue string literals.
   - Venue-specific facts must come from `VenueContract`.

2. **No binary-only shared API growth**
   - New shared APIs must not introduce `binary`, `yes`, `no`, `up`, or `down` names.
   - Existing binary names are allowed only in binary-specific modules, existing legacy surfaces awaiting a mechanical rename, tests that explicitly assert quarantine, and strategy-local evidence/display fields.

3. **Second-family compile proof**
   - A bounded-scalar proof family must compile against the same quote-target seam without touching quote lifecycle, admission, execution, or strategy core.
   - A continuous-mark proof family must compile against the same seam without touching quote lifecycle, admission, execution, or strategy core.
   - These proof families are not live-tradeable products.

4. **N-outcome settlement proof**
   - Settlement primitives must admit N-outcome payouts.
   - Binary settlement is represented as the N=2 case.
   - A future sportsbook or multi-outcome prediction market must not require a new settlement engine.

5. **Group-by-change proof**
   - Adding a market family touches one family adapter and one config section.
   - It must not require edits to quote lifecycle, requote budget, venue execution, admission, or shared strategy plumbing.

6. **NT-first proof**
   - Each helper issue must include a source-proof section listing the NT APIs checked at the `Cargo.toml` pinned rev.
   - If Bolt builds a helper that NT already provides, the PR fails review unless the user explicitly approves the duplication.

7. **No dual paths**
   - One fill truth: NT execution/backtest fill path unless NT is source-proven insufficient.
   - One settlement primitive shared by backtest and live.
   - One fair-value path per market family, shared by maker and taker consumers.

## Future-Family Admission Contract

Future families are admitted only by proving they fit the seam:

| Future family | Expected adapter change | Shared engine change allowed? |
| --- | --- | --- |
| Sports or politics binary | bounded-outcome FV adapter using domain probabilities; may reuse binary payout shape | No |
| Weather binary | bounded-outcome FV adapter using forecast/model evidence | No |
| Sportsbook or multi-outcome market | N-outcome settlement + odds/probability FV adapter | No, unless the N-outcome proof reveals a missing neutral primitive |
| Listed options | adapter around NT option instruments, greeks, IV/surface, strike/expiry evidence | No |
| Perpetual futures | adapter for continuous mark, mark/index, funding, margin, inventory/risk | No, unless a reviewed architecture gate approves a missing primitive |

If a future family requires shared-engine edits, the issue must first show which existing invariant was too narrow and get adversarial review before implementation.

## Work Process From Here

This section is the handoff for future agents and reviewers.

### Step 0: Architecture Approval Gate

This document is W0. No new implementation issue should be opened from #488 until this artifact passes adversarial review.

Approval means:

- reviewers approve Option B as the direction;
- reviewers agree the anti-hardcode checks are strong enough;
- any `REQUEST_CHANGES` findings are either fixed in this artifact or explicitly waived by the user.

### Step 1: Update The Epic

After W0 approval, update #488 as the umbrella epic. The epic must say:

- first implementation slice is crypto binary-option maker;
- the architecture is market-family based, not binary-only;
- future families are deferred but protected by CI/review invariants;
- each workstream below gets its own issue/PR.

### Step 2: Create Child Issues In This Order

1. **W1 VenueContract capability extension**
   - Extend typed venue facts.
   - Add no-venue-name branching checks.

2. **W1B Market-family seam hardening**
   - Neutralize quote-target surfaces.
   - Add bounded-scalar and continuous-mark proof families.
   - Quarantine or mechanically rename binary-only shared names.

3. **RV shared helper**
   - Source-prove NT coverage first.
   - Implement only missing Bolt helper logic.
   - Keep the helper independent from maker/taker strategy files.

4. **IV shared helper**
   - Source-prove NT option chain, greeks, and IV surfaces first.
   - Wrap NT where available.
   - Keep exchange/instrument-specific data access outside strategy logic.

5. **FV helper/adapters**
   - Define the shared fair-value call path.
   - Crypto binary adapter is first.
   - Maker and taker must use the same family fair-value path.

6. **W2 Quote lifecycle**
   - Shared lifecycle and requote budget consume `VenueContract` and quote targets.
   - No family-specific or venue-specific branches.

7. **W3 Crypto binary maker model**
   - GM/CG spread, book/flow evidence, inventory skew, fail-closed predicates.
   - Crypto binary only, behind the family adapter.

8. **W4 Settlement**
   - Shared N-outcome settlement primitive.
   - Binary 0/1 resolution is the first concrete case.
   - Backtest and live use the same module.

9. **BT Pre-live backtest gate**
   - Run the built maker through NT/BTE replay.
   - No live capital before PASS.

10. **W5 Capital reservation and portfolio**
    - Per-market reservation first.
    - Portfolio/multi-market allocation after per-market safety.

11. **W6 Operations**
    - Heartbeat, stale quote alarms, maintenance pull, compliance precondition.

12. **W7 Rewards**
    - Additive only after core-edge PASS.
    - Safety wins over reward continuity.

13. **W8 Dual-path kill**
    - Remove hand-rolled digital once NT-backed IV/greeks or the approved FV path is authoritative.

### Step 3: PR Rules For Every Child Issue

Each PR must declare exactly one issue or one explicitly named slice. A PR may not claim to close broader #488 unless every accepted scope item is done.

Every PR must include:

- NT-first source proof for any finance/math/execution primitive it touches;
- anti-hardcode proof relevant to the touched seam;
- tests or static checks that fail if venue/family specifics leak into shared code;
- config/TOML sourcing for runtime values;
- a statement of remaining accepted scope and where it is tracked.

External model review happens only after:

- the worktree is clean except the intended PR diff;
- local checks pass or failures are documented;
- the exact PR head is pushed;
- CI is green when CI exists for the PR.

Agents must not merge anything unless the user explicitly asks for a merge.

### Step 4: What Future Agents Should Read First

For any #488 follow-up, read these files in order:

1. `AGENTS.md`
2. `specs/488-binary-oracle-maker/architecture.md`
3. `specs/488-binary-oracle-maker/spec.md`
4. `specs/488-binary-oracle-maker/plan.md`
5. the specific child issue being implemented

If these files disagree, `architecture.md` controls the #488 architecture direction until a newer reviewed architecture gate supersedes it.
