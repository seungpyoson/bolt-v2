# Issue 135 ETH Chainlink Taker Design

## Goal

Implement the first real production strategy kind for `bolt-v2`: an ETH 5m Chainlink adaptive taker on Polymarket rotating `eth-updown-5m-*` markets.

This design covers only `#135` strategy-layer work on top of the already-landed runtime seams from `#134`, `#136`, and `#157`.

It does not reopen runtime/config architecture, live-local materialization, selector policy, or client plumbing.

## Proven Base

This design assumes the fresh branch state based on `#157`:

- `StrategyBuildContext` provides both `fee_provider` and `reference_publish_topic`.
- Runtime publishes `RuntimeSelectionSnapshot` on the strategy-specific selection topic.
- `ReferenceActor` publishes typed `ReferenceSnapshot` messages on the configured reference topic.
- Ruleset runtime persists one strategy across market switches.
- Selector no longer suppresses candidate loading/publishing solely because positions are open.

These seams are already present on this branch. `#135` must consume them, not rebuild them.

## Frozen Decisions

The following decisions are now explicit and frozen for this issue:

1. If both sides have positive worst-case EV, the strategy chooses the side with the higher worst-case EV.
   It does not reject entry purely because both sides are positive.

2. The implementation uses one concrete strategy module with a thin NT actor shell and pure decision helpers.
   The shell owns subscriptions, cache reads, state transitions, and order submission.
   Pure helpers own lead arbitration, uncertainty-band math, EV, side selection, sizing, and forced-flat predicates.

3. `#135` targets runtime `[[strategies]]` config only.
   `live_config.rs` currently renders zero `[[strategies]]` entries by design and is out of scope for this issue.

4. Missing or degraded input always means fail closed.
   The strategy never fabricates defaults for missing fees, missing reference state, or missing market data.

5. All strategy-specific validation remains in the builder.
   `#135` must not widen into `src/validate.rs`.

## Scope

### In Scope

- New `src/strategies/eth_chainlink_taker.rs`
- Registering `eth_chainlink_taker` in `src/strategies/mod.rs`
- Strategy-local config parsing and validation
- Strategy-local subscriptions and state transitions
- Strategy-local EV, sizing, entry, exit, cooldown, recovery, and invariant enforcement
- Strategy-focused unit/integration tests

### Out of Scope

- `src/main.rs`
- `src/validate.rs`
- `src/platform/*`
- `src/clients/*`
- `src/live_config.rs`
- Any new runtime or config seam
- Any `#154` follow-up work
- Any new issue creation unless a real blocker is proven

If implementation proves a missing seam in those areas, stop and report it instead of patching around it.

## Strategy Shape

### Module Layout

`src/strategies/eth_chainlink_taker.rs` should contain:

- `EthChainlinkTakerConfig`
- `EthChainlinkTakerBuilder`
- `EthChainlinkTaker`
- strategy-local state structs
- pure decision helpers
- unit tests for pure logic

No generic framework should be introduced. This issue adds one concrete production strategy kind.

## Runtime Shell Responsibilities

The NT-facing shell is responsible for:

- subscribing to `RuntimeSelectionSnapshot`
- subscribing to `ReferenceSnapshot`
- subscribing and unsubscribing Polymarket `OrderBookDeltas` for the active token
- calling `fee_provider.warm(active_token_id)` on activation and switch
- reading `cache.positions_open(...)`, order state, and fill state
- creating and submitting `Limit` + `TimeInForce::Fok` orders
- reacting to order, fill, and position events

## Pure Decision Core Responsibilities

Pure helpers are responsible for:

- selecting the active side from `CandidateMarket` plus current reference/book state
- scoring fast venues for dynamic lead arbitration
- computing uncertainty band width
- computing worst-case EV by side
- deciding whether entry is allowed
- computing robust size subject to caps
- comparing `hold_ev` versus `exit_ev`
- evaluating forced-flat predicates

This split is required so the behavior can be proven by deterministic tests rather than only actor-harness tests.

## State Model

State is split into two buckets.

### Cross-Switch Persistent State

This state survives A -> B rotation:

- `component_id`
- global recovery flag
- per-market cooldown ledger
- per-market churn counters
- current pending entry/exit order tracking
- one-position invariant bookkeeping

### Active-Market State

This state resets when the active market changes:

- active market snapshot
- active side and active token id
- active instrument id
- active book subscription target
- interval-open price capture
- warmup counter
- fee-readiness for the active token
- current lead-venue score state
- active-market forced-flat suppression state

Only the second bucket is reset on switch.

## Subscription Contract

### Startup

On `on_start` the strategy must:

1. inspect NT cache for existing open positions under its `strategy_id`
2. enter recovery mode if any are found
3. subscribe to runtime selection snapshots
4. subscribe to reference snapshots on `context.reference_publish_topic`

It prepares during both `Active` and `Freeze`.

That means:

- runtime selection and reference subscriptions start on `on_start`
- current-market order-book subscriptions may be established during `Active` or `Freeze`
- no new entries are allowed during `Freeze`

## Selection Handling

When a selection snapshot arrives:

- `Idle` means no active trading target
- `Freeze` means prepare but do not open new trades
- `Active` means one candidate market is now tradable

If the active market identity has changed:

- explicitly replace current-market book subscriptions
- keep both outcome books prepared for the selected market until later side-selection logic chooses one side for action
- clear only active-market state
- trigger fee-rate warmup for both outcome tokens
- reset fee readiness fail-closed for the new market

If the same market changes phase between `Active` and `Freeze`:

- preserve interval-open, warmup, and prepared market state
- update only phase-local state
- re-trigger fee-rate refresh when re-entering `Active`

Repeated `Active` snapshots for the same effective market must be idempotent.

## Active Side Selection

Given a runtime-selected market, the strategy computes both Up and Down worst-case EVs using the same fused reference state and current book state.

The active side is:

- Up if only Up clears the entry threshold
- Down if only Down clears the entry threshold
- the higher worst-case-EV side if both clear
- no entry if neither clears

This side choice determines the order side/price used for entry.
Pre-math shell preparation remains side-neutral.

## Data Readiness Gates

An entry attempt is allowed only if all of the following are true:

- runtime state is `Active`, not `Idle` or `Freeze`
- active instrument metadata matches the current selection snapshot
- the active book is populated enough for pricing
- the interval-open reference has been captured
- `warmup_tick_count` consecutive fresh Chainlink ticks have been observed since activation
- `fee_provider.fee_bps(active_token_id)` returns `Some`
- the strategy is not in recovery mode
- the active market is not cooling down
- forced-flat conditions are not active
- there is no open or inflight position/order state violating the one-position invariant

If any gate fails, the strategy does nothing except log the reason in structured operator-facing form.

## Reference and Lead Model

`ReferenceSnapshot` is the primary anchor. Fast venues are modifiers, not replacements.

For each fast venue, maintain rolling state for:

- latest observed timestamp
- inter-arrival jitter
- agreement versus Chainlink
- current eligibility against configured thresholds

Lead selection uses a composite score over freshness, jitter, and agreement. There is no hardcoded venue ranking.

If no fast venue clears thresholds, the strategy falls back to Chainlink-only reference handling.

`Freeze` does not stop reference preparation:

- reference freshness should continue updating
- interval-open capture should still occur if still missing
- warmup progress should continue updating
- entries remain blocked because phase is not `Active`

## Interval-Open Capture

The interval-open price is anchored to the market start timestamp from `snapshot.decision.Active.market.start_ts_ms`.

Rules:

- if a fresh Chainlink observation exists at-or-after `start_ts_ms`, use the first such tick
- if the strategy activates late, capture the first Chainlink tick received at-or-after `start_ts_ms`
- until interval-open is captured, entries are disallowed

This capture belongs to active-market state and resets on market switch.

## Fee Model

Fees come only from `Arc<dyn FeeProvider>`.

Rules:

- market activation and market switch trigger fee-rate warmup for both outcome tokens asynchronously
- same-market `Freeze -> Active` reactivation triggers a fee-rate refresh again
- if cached fee rates already exist for the current market, the strategy may remain fee-ready immediately while refreshing in the background
- if cached fee rates do not exist, readiness remains fail-closed until both outcome fee rates are available
- no Gamma fee fields are read or propagated
- final fee amounts are computed later from fee rate, shares, price, and the venue formula; warmup here is only for fee rates

## Negative Telemetry

The strategy must log not only what it uses, but also what it explicitly does not use and why.

Required omission logging includes:

- fee-rate unavailable, so entry stays fail-closed
- maker rebate unavailable on the trusted seam
- market category unavailable on the strategy-visible seam
- final fee amount not yet known until price and size are chosen
- fast-venue lead unavailable, so the strategy falls back to Chainlink-only handling
- entry blocked by warmup, cooldown, recovery, forced-flat, missing interval-open, or missing fee readiness

This is not optional debug noise. It is part of the operator-facing audit trail for first-live strategy behavior.

## EV and Band Model

The strategy computes:

- a fair-value estimate from fused reference data
- an uncertainty band whose width grows with lead jitter, time remaining, and fee uncertainty
- worst-case EV for Up using the lower bound
- worst-case EV for Down using the upper bound

Entry is considered only if the chosen side clears `worst_case_ev_min_bps`.

This issue does not widen into profitability research. It only enforces honest, configurable, and fail-closed EV plumbing.

## Sizing

Sizing is robust, not fixed.

The chosen size maximizes:

`E[EV over band] - risk_lambda * size^2`

subject to:

- `size <= max_position_usdc`
- `size <= book-depth-derived impact cap`

The strategy must not hardcode fixed trade sizes or fallback sizes.

## Orders and Position Discipline

### Entry

Entries use:

- `Limit`
- `TimeInForce::Fok`
- best-ask price for Up side entries
- best-bid price for Down side entries

Each order carries a strategy-owned `ClientOrderId`.

The builder enforces the FOK-only policy for this strategy kind. That policy does not belong in the platform validator.

## One-Position Invariant

Before every entry attempt, the strategy checks that it does not already own an open/inflight position or order path that would violate one-position behavior.

Behavior:

- debug builds panic on invariant violation
- release builds reject the attempt and emit `log::error!`

No second entry is ever intentionally staged while the invariant is already occupied.

## Exit

On each relevant book update, recompute:

- `hold_ev`
- `exit_ev`

Exit when:

`exit_ev >= hold_ev - exit_hysteresis_bps`

No blind carry-to-resolution behavior is allowed. Any intentional hold is the result of this explicit comparison, not omission.

`modify_order` is not used. Exit updates must cancel and replace if they need to move.

## Forced-Flat Rules

Any forced-flat trigger:

- closes the current position if one exists
- suppresses new entries until the trigger clears

The required triggers are:

- stale Chainlink beyond configured threshold
- all fast venues incoherent while Chainlink alone is also not fresh enough
- thin Polymarket book below configured liquidity threshold
- runtime `Freeze` state
- instrument metadata mismatch between runtime selection and observed instrument

## Cooldown and Recovery

## Cooldown

After any fill, the active market enters cooldown for `reentry_cooldown_secs`.

Cooldown is tracked per market, not globally, so a stale-blocked A must not suppress B.

### Recovery

On startup:

- if `cache.positions_open(None, None, Some(&strategy_id), None, None)` is non-empty, set `recovery = true`
- do not open new positions while recovery is true
- clear recovery only when the position is flat
- recovery mode still keeps the shell in full preparation mode: books, fee-rate warmup, reference updates, and readiness tracking continue running

This honors the accepted `external_order_claims: None` boundary without widening into `#132`.

## Invariants

These invariants must remain true throughout implementation:

1. No entry without fees, reference, active book, warmup, and explicit active selection.
2. Market switch resets only interval-local state.
3. Cooldown is per market and must not suppress a different market.
4. One-position invariant is checked on every entry path.
5. Every fee-dependent calculation reads only `FeeProvider`.
6. Every degraded input path fails closed.
7. Book subscription swaps are explicit unsubscribe/subscribe pairs.
8. The strategy remains additive and does not widen into runtime/config code.
9. Operator-visible logs must say both what inputs were used and what relevant inputs were unavailable or intentionally excluded.
10. `Freeze` means prepare fully but never open a new trade.

## Test Strategy

Implementation must be test-first and split into two layers.

## Pure Logic Tests

Pure helpers should prove:

- side selection
- uncertainty-band widening under jitter
- EV with honest fees
- robust sizing under caps
- forced-flat predicates
- cooldown logic
- interval-open capture rules

## Actor and Integration Tests

Actor/harness tests should prove:

- startup subscriptions
- activation and switch behavior
- fee-readiness fail-closed behavior
- recovery mode
- book subscribe/unsubscribe correctness
- same-session `ClientOrderId` attribution behavior
- entry and exit behavior through the harness

## Execution Order

Implementation proceeds in the same staged order already frozen for this issue:

1. config + builder + registration
2. core strategy shell
3. startup subscriptions
4. switch handling
5. fee-readiness gate
6. interval-open capture
7. warmup counter
8. lead arbitration
9. uncertainty band
10. EV + side selection
11. sizing
12. entry path
13. one-position invariant
14. dynamic exit
15. forced-flat triggers
16. cooldown + churn
17. recovery mode
18. builder validation

After items 1-7, run the required mid-stream contract check before continuing.

## Non-Goals

- 15m, 1h, or 1d variants
- non-ETH markets
- maker or hybrid quoting
- multi-position support
- runtime or config refactors
- live-local strategy materialization
- dynamic `external_order_claims`
- strategy-side Gamma HTTP calls
- profitability validation or model tuning

## Acceptance Boundary

`#135` is ready for implementation when code matches this contract and no new infra seam is required.

If a new seam is discovered outside this design, stop and track it explicitly instead of absorbing it into strategy code.
