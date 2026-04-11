# ETH 5m+15m Adaptive Chainlink Taker V1

## Status

Draft design for review.

## Scope

This spec defines the first real live strategy slice for `bolt-v2`.

It is intentionally narrow:

- One live process.
- ETH only.
- `5m` and `15m` markets only.
- Chainlink-settled markets only.
- Taker-only execution using aggressive entry with `FOK`.
- One live position at a time.

This spec does **not** define:

- SOL support.
- `1h` markets.
- Maker or hybrid quoting.
- Holding to resolution as a normal strategy path.
- Multi-process capital coordination.
- Portfolio optimization across multiple assets.

## Dependencies

- Issue `#37`: shared Chainlink ingest path behind the reference-feed interface.
- Issue `#109`: generic resolution-basis modeling and removal of asset-specific selector hardcodes.

This strategy must consume Chainlink through the shared reference pipeline. It must not add direct strategy-specific Chainlink plumbing.

## Problem

`bolt-v2` currently has the strategy-platform skeleton and a placeholder `exec_tester`, but not a real production strategy.

For the first live slice, the goal is not a general market-making engine or a broad multi-asset framework. The goal is to prove one narrow, defensible edge path:

- fast external ETH venues move first
- Chainlink ETH/USD follows later
- Polymarket sometimes prices closer to the lagging state than the leading state
- executable edge remains positive after fees and orderbook impact

If this fails on ETH, adding SOL only multiplies uncertainty. If this works on ETH, the same pattern can later be evaluated for other assets.

## Design Principles

1. No hardcoded asset or venue assumptions in strategy logic.
2. No fixed alpha thresholds in the signal path.
3. Hard risk controls are required, but they must be config-driven.
4. Shared reference data is the only source for Chainlink and fast-venue reference inputs.
5. The strategy is non-carry by design. It does not intentionally hold to resolution.
6. One position at a time. No pyramiding or averaging down.
7. Fail closed whenever data quality or execution quality becomes ambiguous.

## Current Runtime Constraints

The current platform path has three important constraints:

1. The runtime currently operates one active ruleset at a time.
2. The generic selector currently filters eligible markets and then chooses the winner primarily by liquidity.
3. The operator-facing Polymarket path still centers on one fixed `event_slug`, while this strategy needs a rolling ETH `5m`/`15m` market set.

These constraints are acceptable for the platform baseline, but they are not sufficient for this strategy as-is. This issue therefore includes a narrow runtime extension:

- keep the current ruleset-based eligibility filter
- add a strategy-scoped opportunity ranking layer over the eligible ETH `5m`/`15m` set
- support a rolling Polymarket subscription universe for that active ETH candidate set without introducing asset-specific code literals

This is still one strategy slice, not a broad platform rewrite.

## Target Runtime Behavior

The process owns one ETH ruleset covering both `5m` and `15m` Chainlink-settled markets.

At runtime it does five things continuously:

1. Discover eligible ETH `5m` and `15m` candidate markets.
2. Maintain the active Polymarket market-data universe for those candidates.
3. Estimate the current fast ETH price from multiple fast venues through a dynamic lead-arbitration layer.
4. Compute conservative executable EV for each eligible candidate market.
5. Arm and trade only the single best live opportunity.

The runtime never intentionally carries a position to resolution. If the edge decays or time runs short, the process exits and stands down.

## Inputs

### 1. Candidate Market Metadata

For each candidate market:

- `market_id`
- `instrument_id`
- declared resolution basis
- accepting-orders status
- liquidity
- time to end

This metadata comes from Polymarket market discovery and must be parsed using the generic resolution-basis layer introduced by `#109`.

### 2. Shared Reference Pipeline

The strategy consumes Chainlink and fast-venue information through the shared reference layer.

Required strategy-visible information:

- current Chainlink ETH/USD observation
- current per-venue fast observations
- venue freshness and health
- enough shared telemetry to support live lead-source arbitration

If the current shared snapshot surface is not rich enough for dynamic lead arbitration, this slice may extend the shared reference output. It must not bypass it with a direct strategy-specific Chainlink path.

### 3. Polymarket Executable Orderbook State

The strategy needs side-aware executable pricing, not midpoint pricing.

For each armed candidate:

- live best bid/ask
- enough depth to estimate sweep cost for entry
- enough opposing depth to estimate a conservative exit path

### 4. Live Fee Data

The strategy must use live Polymarket fee data. It must not hardcode a fee schedule in alpha logic.

If fee data is unavailable, the strategy fails closed and does not enter.

### 5. Interval Open Price

The strategy needs the relevant interval open price `O` for the market’s settlement convention.

If it cannot infer or validate `O`, the market is not tradable.

## Component Design

### A. Eligibility Filter

The generic platform layer continues to do coarse fail-closed filtering:

- tag/family match
- structured/canonical resolution-basis match
- accepting-orders check
- minimum liquidity
- minimum and maximum time to expiry
- freeze-window transition

This layer answers:

- which markets are eligible right now
- which markets are frozen
- which markets are rejected and why

It does **not** decide final trade opportunity ranking for this strategy.

### B. Opportunity Coordinator

New strategy-scoped component.

Responsibilities:

- consume the full eligible ETH candidate set
- compute live strategy opportunity score per candidate
- choose the single armed market
- trigger market switches only when a different candidate remains superior after switching cost and dynamic stability penalty derived from recent score volatility

The coordinator replaces the current assumption that the highest-liquidity eligible market is automatically the correct market to trade.

The coordinator is still constrained by the coarse eligibility filter. It only ranks already-eligible markets.

### C. Adaptive Lead Estimator

The strategy must not hardcode a permanent venue ranking such as “Deribit first” or “Binance first”.

Instead it maintains a dynamic score per configured fast venue using:

- local arrival freshness
- recent update cadence
- observed jitter
- peer agreement
- observed utility during recent Chainlink lead/lag episodes

Outputs:

- `F_trigger`: price from the current best lead venue
- `F_consensus`: peer-confirmed fast ETH price
- `lead_confidence`: confidence derived from winner quality, winner-runner-up margin, and peer coherence

If a venue is fast but quiet, it can still win when it is genuinely informative. If it goes stale or becomes noisy, the estimator degrades it automatically.

### D. Probability Engine

Use one dynamic model for both `5m` and `15m`.

Definitions:

- `O`: interval open price
- `C`: latest Chainlink ETH/USD price
- `F_consensus`: current fast ETH price
- `tau`: remaining time to settlement
- `sigma_remaining`: realized-vol estimate scaled to remaining time

Convert price relative to the interval open into fair win probability:

`p(x) = Phi( ln(x / O) / sigma_remaining )`

Then compute:

- `p_cl = p(C)`
- `p_fast = p(F_consensus)`

The model is intentionally simple. It relies on live volatility and remaining time instead of separate hand-tuned logic for `5m` and `15m`.

### E. Dynamic Uncertainty Band

The strategy must not treat `p_fast` as a perfect point estimate.

Build a live uncertainty band from:

- lead confidence
- fast-venue dispersion
- winner-runner-up margin
- recent fast/Chainlink noise

Outputs:

- `p_fast_low`
- `p_fast_high`

Higher disagreement or weaker lead confidence widens the band and removes trades automatically. This replaces fixed alpha thresholds.

### F. Executable EV Engine

For each candidate market and each side:

- sweep the real Polymarket book for candidate size
- compute executable entry price
- compute implied PM probability for that entry
- compute conservative fair value using the adverse side of the uncertainty band
- subtract:
  - live entry fee
  - conservative exit fee bound
  - entry impact
  - conservative exit impact bound

This yields:

- `robust_ev(size, side)`

The signal path has no fixed alpha threshold. A trade is valid only if `robust_ev(size, side) > 0`.

## Entry Logic

The strategy evaluates both `Up` and `Down` on the armed market.

Entry is permitted only when all conditions hold:

- the market is eligible and not frozen
- shared reference inputs are healthy
- fee data is present
- lead arbitration yields a bounded uncertainty band whose adverse side still supports positive executable EV
- Polymarket book depth supports the candidate size
- `robust_ev(size, side) > 0`
- enough time remains to manage the position before forced flatten

Execution:

- use aggressive taker entry with `FOK`
- reject partial exposure on entry
- no retry loop that assumes the same edge still exists after a failed `FOK`

## Per-Market Re-entry Policy

V1 does not impose a fixed cap such as “one trade per market” or “N trades per market”.

For the first live slice:

- re-entry into the same `market_id` is allowed
- there is still only one live position at a time
- re-entry remains constrained by:
  - positive `robust_ev`
  - cooldown after exit
  - forced-flat timing
  - max notional and daily loss controls
  - current book depth and fee conditions

This keeps V1 simple and avoids introducing a hardcoded churn cap.

However, V1 must record enough per-market state to support a stricter dynamic re-entry budget later. At minimum:

- `market_id`
- entry count
- realized P&L
- paid fees
- estimated impact costs
- last exit timestamp

That future follow-up can then allow re-entry only while cumulative realized result plus conservative remaining expected value for the market stays positive.

## Position Sizing

Sizing is fully dynamic in the alpha path.

The chosen size is the largest size that still satisfies all of these:

- `robust_ev(size, side) > 0`
- worst-case adverse-resolution loss is within the configured per-trade risk budget
- conservative emergency-exit loss is within remaining daily risk budget
- book depth and estimated exit depth remain acceptable

Size automatically scales down when:

- fast venues disagree more
- lead-source dominance weakens
- PM depth thins
- PM spread widens
- fees or impact consume more of the edge
- time remaining compresses the manageable exit window

What remains fixed and config-driven:

- max notional per trade
- max one live position
- max daily loss
- max total emergency liquidation loss budget

## Exit Logic

This strategy is explicitly non-carry.

It does not assume that the best exit is “wait longer” or “hold to resolution”.

At every position-management tick it compares:

- `hold_ev`: conservative value of continuing to hold
- `exit_ev`: conservative value of flattening now

Normal exit:

- exit when `exit_ev >= hold_ev`
- or when PM has converged enough that the remaining edge no longer justifies staying in

Risk-driven exit:

- exit when lead confidence collapses
- exit when fast venues lose coherence
- exit when Chainlink catches up enough that the original lag thesis is largely gone
- exit when time remaining falls into the forced-flat regime

Forced-flat mode:

- once the forced-flat deadline is reached, profit-seeking is over
- the only objective is to get flat
- exit aggressiveness rises automatically as time remaining shrinks
- the strategy may use smaller aggressive clips if one-shot flattening is not feasible

The strategy must not intentionally carry a position into resolution.

## If Exit Opportunities Do Not Come

The answer is not “hold and hope”.

If normal convergence does not appear:

- the strategy de-risks progressively as time compresses
- once forced-flat mode begins, it exits for risk reasons rather than waiting for ideal alpha capture
- if the position cannot be flattened under acceptable conditions, the process halts new entries and stays in protective mode until risk is resolved

## If the Market Is Collapsing Toward 0 or 1

If the position is moving toward terminal loss and the fair thesis is broken:

- the strategy exits immediately
- it does not wait for a rebound
- it does not convert into a resolution hold

The strategy is an arb-style engine, not a directional conviction engine.

## Hard Risk Controls

These controls are fixed in purpose but config-driven in value:

- one live position maximum
- max notional per trade
- max daily realized + unrealized loss
- forced-flat deadline before settlement
- max hold time
- startup warmup gate
- operator halt
- no-entry after emergency liquidation failure until recovery criteria pass

These are not optional and are separate from alpha estimation.

## Subscription and Market-Data Universe

The operator-facing path for `bolt-v2` currently centers on one fixed Polymarket `event_slug`. That is not sufficient for a rolling ETH `5m`/`15m` candidate set.

This strategy therefore requires a bounded rolling market-data universe for the active ETH candidate set.

The intended design is:

- derive the active candidate set from the ruleset and market discovery
- keep the Polymarket market-data subscription universe aligned with that bounded set
- avoid hardcoding ETH-specific slugs or manually maintaining rolling slugs in operator config

This is a narrow runtime extension required by the strategy slice, not a broad platform rewrite.

## Telemetry and Audit

At minimum, audit and metrics must capture:

- eligible, rejected, frozen, and armed candidate markets
- reason for no-trade decisions
- winning lead venue, runner-up, and score margin
- uncertainty-band width over time
- `robust_ev` before entry
- chosen size and why larger sizes were rejected
- per-market entry count and last exit timestamp
- realized fees and impact by `market_id`
- entry attempt result
- exit reason
- forced-flat transitions
- strategy halt reasons

This telemetry is required for validating whether the strategy is failing because the alpha is not real or because the data/execution assumptions are wrong.

## Testing

Required test coverage for this slice:

### Strategy and Probability

- no-trade when uncertainty band is too wide
- no-trade when `robust_ev <= 0`
- dynamic sizing shrinks under weaker confidence or thinner books
- exit when `exit_ev >= hold_ev`
- forced-flat transition before settlement

### Lead Arbitration

- fastest venue can win without being permanently hardcoded
- stale or quiet leader degrades correctly
- noisy outlier venue cannot force entry without peer coherence
- runner-up margin influences confidence behavior

### Market Coordination

- coordinator prefers the better live ETH opportunity between eligible `5m` and `15m` markets
- coordinator does not switch markets unless the new candidate stays superior after dynamic switching penalty and recent-score stability checks
- frozen or failing market is dropped without breaking the process

### Failure Modes

- no fee data -> fail closed
- stale Chainlink -> fail closed
- stale or incoherent fast venues -> fail closed
- PM book too thin or too wide -> fail closed
- emergency liquidation path blocks new entries after failure

## Acceptance Criteria

- one ETH process can scan eligible `5m` and `15m` Chainlink-settled markets
- final market choice is based on live opportunity ranking, not only liquidity
- fast-feed selection is dynamic and not permanently hardcoded to one venue
- signal gating uses conservative executable EV after fees and impact, not fixed alpha thresholds
- the strategy never intentionally carries to resolution
- one live position maximum is enforced
- forced-flat behavior exists and is tested
- all Chainlink usage stays behind the shared reference pipeline

## Rollout Intention

This strategy should launch as the first real live ETH slice only after:

1. shared Chainlink ingest is available through the reference pipeline
2. resolution-basis modeling is generic enough to support ETH without new asset-specific literals
3. the runtime can coordinate the rolling ETH `5m`/`15m` candidate set

Only after ETH is proven should the same pattern be considered for another asset.
