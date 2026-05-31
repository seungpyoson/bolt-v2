# Contract: NT-First Loss Governor

## Purpose

Evaluate whether Bolt may admit new risk using only fresh NT-derived loss/equity facts and configured policy thresholds. PR #507 provides the pure evaluator, positional-sizing core, configured enforcement at the shared submit-admission boundary, and a configured NT portfolio/position runtime feed that publishes loss snapshots to submit admission.

## Public Behavior

The governor accepts:

- `LossGovernorPolicy`
- optional `LossSnapshot`
- `now_ns`

The governor returns `LossAdmissionDecision`.

Acceptance requires:

- snapshot is present
- snapshot source is non-empty
- snapshot age is within `max_snapshot_age_ns`
- every configured threshold has the required snapshot field
- no configured loss/drawdown threshold is breached

Rejection reasons:

- `per_trade_loss_limit`: per-trade loss breaches configured threshold
- `daily_loss_limit`: daily/session loss breaches configured threshold
- `rolling_loss_limit`: rolling-window loss breaches configured threshold
- `max_drawdown_limit`: peak equity minus current equity breaches configured threshold
- `stale_loss_snapshot`: snapshot missing, source missing, required field missing, timestamp invalid, or stale

Threshold equality rejects admission. A loss or drawdown exactly equal to its configured threshold is a breach.

## Submit Admission Behavior

When `[risk.loss_governor].enabled = true`:

- entry and replace submits are rejected before NT submit if the latest loss snapshot is missing, stale, unattributed, incomplete for a configured policy dimension, or breached
- risk-reducing exits bypass loss-threshold halt policy but still obey existing lifecycle and live-order count caps
- decision evidence records `rejected_loss_governor_halted` and the deterministic `loss_halt_reasons`

## Live Feed Behavior

The live integration subscribes to NT portfolio snapshot and position event topics for the configured account. The feed derives:

- per-trade PnL from NT position changed/closed events, with a zero baseline before the first trade-level position PnL event
- daily/session PnL from NT realized plus unrealized PnL in `PortfolioSnapshot`
- rolling PnL from configured-window deltas between NT portfolio PnL snapshots
- current and peak equity from NT total-equity snapshots

The feed publishes a `LossSnapshot` on accepted account heartbeats. `observed_at_ns` is the latest accepted NT event timestamp for that account, while stale or expired rolling-window samples are evicted on each fresh portfolio snapshot. Historical peak equity remains an input to drawdown but does not make an otherwise fresh portfolio heartbeat stale.

## Non-Goals

- No strategy-local risk logic.
- No cancel/flatten side effects.
- No direct `RiskEngine::set_trading_state` side effect in this slice.
- No independent PnL/account truth.
- No venue-specific code.
- No positional-sizer live-path enforcement in PR #507.

## Scope Guards

This slice must not edit:

- `src/strategies/binary_oracle_edge_taker.rs`

The final report must state that submit-admission protection does not prove cancel/flatten or NT trading-state side effects.
