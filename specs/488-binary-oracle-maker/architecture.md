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
thin FV dispatch selects the family adapter and evidence set
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
- settlement mechanics: timing, redemption path, currency, and venue settlement workflow
- venue-specific limits that affect admissibility

Shared quote/admission/execution code reads these fields. It must not match on venue string literals; venue-name checks must be generated from the configured `contracts/*.toml` names rather than from a stale hardcoded list.

`VenueContract` does not own payout math. A venue can host more than one market family; the venue contract describes mechanics, and the market-family adapter describes the economic payout rule.

### Layer 3: Market-Family Adapter

A market family owns instrument math and payout structure:

- fair-value interpretation
- quote-target layout
- inventory projection
- settlement rule: outcome ids, payout-vector semantics, and terminal payout math
- family-local kill predicates

The shared engine consumes family-neutral quote targets and settlement outputs. It must not know whether a target came from a binary option, listed option, perpetual future, sports market, politics market, or sportsbook-style outcome market.

The current binary/updown path is allowed as the first real family, but binary names must stay inside binary-specific modules and strategy-local display/evidence fields. New shared surfaces must use neutral names such as `outcome_id`, `outcomes`, `quote_target`, `settlement_mechanics`, `payout_rule`, and `family_key`.

## RV, IV, And FV

RV and IV are separate shared helpers. They are not buried inside a strategy and they are not one pricing layer.

- RV helper: realized-volatility estimation from observed underlying/market prices.
- IV helper: implied-volatility/surface inputs from listed option markets when observable through NT or an NT-backed adapter.
- FV helper: thin fair-value dispatch that asks the relevant market-family adapter which evidence it needs and returns family-specific fair-value inputs for quote construction. It must not become a central registry that knows every domain model.

Crypto price binaries can use RV and may use IV as an overlay or longer-horizon input. Listed options can use NT option greeks/IV directly where available. Perpetual futures may use mark/index/funding/book evidence rather than IV. Weather, sports, and politics binaries are still binary options economically, but they do not automatically have meaningful RV/IV; their FV comes from domain evidence such as forecasts, polls, odds, or model probabilities. The architecture must allow those evidence sources without changing quote lifecycle, admission, or execution.

Only cross-family evidence belongs in shared helpers. Family-specific evidence, such as polls, weather forecasts, or sport odds, lives with the market-family adapter until at least two real families need the same helper.

## Anti-Hardcode Checks

These checks are part of the architecture gate and must become CI or review gates before downstream workstreams claim the architecture is agnostic.

1. **No venue-name branching in shared code**
   - Shared quote lifecycle, requote budget, admission, execution intent, and settlement orchestration must not branch on venue string literals.
   - Venue-specific facts must come from `VenueContract`.
   - The static check must derive forbidden venue identifiers from `contracts/*.toml` so new venues are covered automatically.

2. **No binary-only shared API growth**
   - New shared APIs must not introduce `binary`, `yes`, `no`, `up`, or `down` names.
   - Existing binary names are allowed only in binary-specific modules, existing legacy surfaces awaiting a mechanical rename, tests that explicitly assert quarantine, and strategy-local evidence/display fields.

3. **Market-family proof suite**
   - A bounded-scalar proof family must compile against the same quote-target seam without touching quote lifecycle, admission, execution, or strategy core.
   - A continuous-mark proof family must compile against the same seam without touching quote lifecycle, admission, execution, or strategy core.
   - A finite multi-outcome proof family with N >= 3 outcomes must compile against the same seam without touching quote lifecycle, admission, execution, strategy core, or shared settlement orchestration.
   - At least one smoke test must route a proof family through the shared quote lifecycle with synthetic quote targets; a proof family that only type-checks without touching the engine is insufficient.
   - Proof families must implement every method exposed by the seam they exercise and cover the arity boundaries: scalar N=1, binary N=2, and multi-outcome N>=3.
   - These proof families are not live-tradeable products.

4. **N-outcome settlement proof**
   - Settlement primitives must accept family-neutral outcome ids and a payout vector or equivalent payout map.
   - Binary settlement is represented as the N=2 payout vector case, not as a separate YES/NO settlement type in shared code.
   - W4 tests must exercise both N=2 and N>=3 through the same module.
   - A future sportsbook or multi-outcome prediction market must not require a new settlement engine.

5. **Group-by-change proof**
   - Adding a market family touches one family adapter and one config section.
   - It must not require edits to quote lifecycle, requote budget, venue execution, admission, or shared strategy plumbing.

6. **NT-first proof**
   - Each issue touching a shared finance/math/execution primitive must include a source-proof section listing the NT APIs checked at the `Cargo.toml` pinned rev.
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
   - Add bounded-scalar, continuous-mark, and finite multi-outcome proof families.
   - Quarantine or mechanically rename binary-only shared names.
   - First deliverable is an inventory of existing `binary`, `updown`, `yes`, `no`, `up`, and `down` names outside binary-specific modules, with each item classified as quarantined, mechanically renamed, or strategy-local display/evidence.
   - Land the enforcement gates here: compile-proof tests, quote-lifecycle proof-family smoke tests, an N>=3 settlement proof, and static checks for forbidden venue/family names in shared modules. No later PR may claim architecture agnosticism until W1B passes.

3. **W2 Quote lifecycle**
   - Shared lifecycle and requote budget consume `VenueContract` and family-neutral quote targets.
   - No family-specific or venue-specific branches.
   - This can be built before final RV/IV/FV helpers because it must operate on synthetic/proof quote targets as well as real strategy quote targets.

4. **RV shared helper**
   - Source-prove NT coverage first.
   - Implement only missing Bolt helper logic.
   - Keep the helper independent from maker/taker strategy files.

5. **IV shared helper**
   - Source-prove NT option chain, greeks, and IV surfaces first.
   - Wrap NT where available.
   - Keep exchange/instrument-specific data access outside strategy logic.

6. **FV helper/adapters**
   - Define the shared fair-value call path.
   - Crypto binary adapter is first.
   - Maker and taker must use the same family fair-value path.

7. **W3 Crypto binary maker model**
   - GM/CG spread, book/flow evidence, inventory skew, fail-closed predicates.
   - Crypto binary only, behind the family adapter.
   - Any `YES`/`NO`, `up`/`down`, or bounded-probability logic must stay inside the crypto binary family adapter, binary maker model, or strategy-local evidence/display fields. It must not enter quote lifecycle, requote budget, admission, execution, or shared settlement orchestration.

8. **W4 Settlement**
   - Shared N-outcome settlement primitive.
   - Binary 0/1 resolution is the first concrete N=2 case.
   - The same primitive must also be tested with a finite N>=3 payout vector or map.
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
- the exact NT commit SHA resolved from `Cargo.toml` at review time, cited in the PR source-proof section without hardcoding that SHA back into long-lived docs;
- the source-proof format: NT checkout rev, NT files/modules inspected, relevant function/type signatures, and verdict of `NT provides`, `NT partially provides`, or `NT does not provide`;
- anti-hardcode proof relevant to the touched seam;
- tests or static checks that fail if venue/family specifics leak into shared code;
- config/TOML sourcing for runtime values;
- a statement of remaining accepted scope and where it is tracked.

Existing open PRs that predate this W0 gate are not automatically approved. Before merge, each must be audited against this architecture and must either satisfy the W0 checks relevant to its scope or be explicitly resliced.

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
