# Research: NT-First Loss Governor

## Cargo Pin

**Decision**: Use the current `Cargo.toml` and `Cargo.lock` NT pin, not older spec text.

**Evidence**:

- `Cargo.toml:24-37`, `Cargo.toml:52`: every configured `nautilus-*` crate in this repo points at `6e059dcbb59ac1e582132fc431a581936c216c3c`.
- Local pinned checkout used for audit: `/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6e059dc`.

## NT Provides PnL, Equity, Snapshot, And Event Inputs

**Decision**: Bolt should consume NT-derived facts for loss governor input.

**Evidence**:

- `crates/model/src/events/portfolio/snapshot.rs:46-69`: `PortfolioSnapshot` includes account id/type, balances, margins, unrealized PnL, realized PnL, total equity, and event/init timestamps.
- `crates/portfolio/src/portfolio.rs:426-537`: `Portfolio::unrealized_pnls` and `Portfolio::realized_pnls` aggregate PnL by venue/account.
- `crates/portfolio/src/portfolio.rs:602-690`: `Portfolio::unrealized_pnl`, `realized_pnl`, and `total_pnl` expose instrument/account scoped PnL.
- `crates/portfolio/src/portfolio.rs:704-735`: `Portfolio::total_pnls` sums realized plus unrealized PnL by venue/account.
- `crates/portfolio/src/portfolio.rs:775-850`: `Portfolio::equity` calculates per-currency equity from account balances plus mark values or unrealized PnL.
- `crates/portfolio/src/portfolio.rs:867-940`: `Portfolio::build_snapshot` rolls account balances, margins, realized PnL, unrealized PnL, and equity into a `PortfolioSnapshot`.
- `crates/model/src/events/portfolio/snapshot.rs:31-69`: `PortfolioSnapshot` is a point-in-time mark-to-market view; realized PnL is accumulated for positions opened in the current session and total equity is per-currency mark-to-market.
- `crates/model/src/events/position/changed.rs:72-76`: `PositionChanged` carries realized return, realized PnL, and unrealized PnL.
- `crates/model/src/events/position/closed.rs:77-81`: `PositionClosed` carries realized return, realized PnL, and unrealized PnL.
- `crates/model/src/events/position/adjusted.rs:60`: `PositionAdjusted` carries optional `pnl_change`.
- `crates/common/src/msgbus/api.rs:470`: `subscribe_portfolio_snapshot` subscribes typed handlers to portfolio snapshots.
- `crates/common/src/msgbus/api.rs:1088`: `publish_portfolio_snapshot` publishes snapshots to matching subscribers.

## NT Provides Trading State Controls, Not Bolt Loss Policy

**Decision**: NT should own the actual trading-state and execution controls. Bolt owns the policy that decides when its admission should halt.

**Evidence**:

- `crates/model/src/enums.rs:1928-1934`: `TradingState` has `Active`, `Halted`, and `Reducing`.
- `crates/risk/src/engine/mod.rs:373-386`: `RiskEngine::set_trading_state` changes state and publishes `TradingStateChanged`.
- `crates/risk/src/engine/mod.rs:465`: `RiskEngine::trading_state` exposes current state.
- `crates/risk/src/engine/mod.rs:735-748`: modify-order handling rejects `Halted` and blocks exposure-increasing changes in `Reducing`.
- `crates/risk/src/engine/mod.rs:1708-1798`: execution gateway denies submit commands in `Halted`, blocks exposure-increasing submit commands in `Reducing`, and forwards submits in `Active`.

## NT Does Not Provide Runtime Loss Governor Thresholds

**Decision**: Bolt must own configured thresholds, freshness policy, rolling/session accounting inputs, fail-closed evidence, and later routing into NT controls.

**Evidence**:

- `crates/risk/src/engine/config.rs:36-46`: `RiskEngineConfig` contains `bypass`, submit/modify rate limits, `max_notional_per_order`, and `debug`; no per-trade loss, daily loss, rolling loss, drawdown, or min-equity policy appears.
- Source search over `crates/risk/src`, `crates/portfolio/src`, and `crates/risk/README.md` for `drawdown`, daily/rolling loss phrases, loss-limit phrases, kill-switch phrases, and min-equity phrases returned no runtime loss-governor configuration.
- `crates/risk/README.md:10-18`: risk crate describes order validation, position sizing, trading controls, balance checks, and margin validation; it does not claim daily or rolling loss governor policy.
- `crates/analysis/src/statistics/max_drawdown.rs:40-86`: NT has an analysis statistic for max drawdown over returns, but it is not a live admission halt policy or `RiskEngine` configuration.

## Bolt Ownership

**Decision**: Bolt owns the policy and submit-admission halt boundary; NT remains the source for portfolio/account facts and execution controls.

**Bolt owns**:

- Config-derived policy thresholds.
- Snapshot freshness and source-attribution checks.
- Deterministic halt reasons: `per_trade_loss_limit`, `daily_loss_limit`, `rolling_loss_limit`, `max_drawdown_limit`, `stale_loss_snapshot`.
- Submit-admission halt decisions and loss halt evidence.
- In-process conversion of NT portfolio/position events into the shared loss snapshot consumed by submit admission.

**Bolt must not own**:

- Independent account balance, fill, or PnL truth.
- NT RiskEngine, Portfolio, cache, adapter, or execution behavior.
- Venue-specific cancel or flatten calls.

## Implemented Scope

**Implemented now**:

- Pure evaluator and tests in `src/bolt_v3_loss_governor.rs`, exported from `src/lib.rs`.
- `[risk.loss_governor]` TOML schema and validation.
- Configure policy wiring into shared submit admission during live-node build.
- Configured NT portfolio/position runtime feed wiring into live-node build.
- Loss snapshots derived from subscribed NT `PortfolioSnapshot` and `PositionEvent` messages for the configured account.
- Reject entry/replace new risk in submit admission on missing, stale, or breached loss facts.
- Emit decision evidence outcome `rejected_loss_governor_halted` plus `loss_halt_reasons`.

**Planned follow-up**:

- Wire positional-sizer decisions into the live submit path.

**Out of scope**:

- Strategy-specific risk logic.
- Cancel/flatten side effects.
- Explicit NT `RiskEngine::set_trading_state` side effects.
- Bespoke venue side effects.
- Strategy file changes.

## PR #507 Implementation Evidence

**Decision**: Implemented the pure loss-governor and positional-sizing core, plus configured submit-admission loss protection and configured NT portfolio/position runtime-feed derivation. Positional-sizer live-path enforcement, cancel/flatten, and NT trading-state side effects remain out of scope.

**Evidence**:

- `src/bolt_v3_loss_governor.rs`: adds config-derived `LossGovernorPolicy`, NT-derived `LossSnapshot`, `LossAdmissionDecision`, `LossHaltReason`, and `evaluate_loss_admission`.
- `src/lib.rs`: exports `bolt_v3_loss_governor` and `bolt_v3_loss_runtime_feed`.
- `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, `tests/config_parsing.rs`: add and validate `[risk.loss_governor]` fields, including configured account id, freshness, rolling window, and thresholds.
- `src/bolt_v3_capital_reservation.rs`: adds the reservation ledger used by the sizer core.
- `src/bolt_v3_sizing_state.rs`: validates NT-derived portfolio, order-lifecycle, product, reservation, and optional loss evidence.
- `src/bolt_v3_position_sizer.rs`: composes policy, worst-case binary liability, loss evaluation, state freshness, and capital reservation into a pure admission decision.
- `src/bolt_v3_submit_admission.rs`: carries optional configured loss-governor policy in shared submit admission, accepts explicit loss snapshots, rejects entry/replace risk before NT submit on missing/stale/breached facts, and leaves risk-reducing exits under existing lifecycle/count caps.
- `src/bolt_v3_loss_runtime_feed.rs`: subscribes to NT portfolio and position event topics, filters to the configured account, derives daily/session PnL, rolling PnL from portfolio PnL deltas, per-trade PnL from position changed/closed events, current equity, and peak equity, and publishes a conservative `LossSnapshot` to submit admission.
- `src/bolt_v3_live_node.rs`: maps enabled `[risk.loss_governor]` TOML into the shared submit-admission state and configured NT runtime feed during live-node build.
- `src/bolt_v3_decision_evidence.rs`: bumps decision evidence to schema v6 and records `rejected_loss_governor_halted` plus deterministic `loss_halt_reasons`.
- `config/root.example.toml`, `tests/fixtures/bolt_v3/root.toml`: add configured loss-governor and capital-pool policy values.
- TDD red/green evidence:
  - `per_trade_loss_breach_rejects_admission`: failed before per-trade evaluation, then passed.
  - `daily_loss_breach_rejects_admission`: failed before daily evaluation, then passed.
  - `stale_missing_or_unattributed_snapshot_fails_closed`: failed before fail-closed freshness/source/required-field checks, then passed.
  - `rolling_loss_breach_rejects_admission`: failed before rolling evaluation, then passed.
  - `max_drawdown_breach_rejects_admission`: failed before drawdown evaluation, then passed.
  - `sizer_rejects_when_loss_governor_rejects`: proves the pure sizer rejects when the pure governor rejects.
  - `restart_requires_rebuilt_open_order_reservations_before_admission`: proves the pure ledger starts fail-closed until reconciliation.
  - `configured_loss_governor_rejects_entry_without_fresh_snapshot_before_nt_submit`: proves configured submit admission fails closed before NT submit when no loss snapshot exists.
  - `configured_loss_governor_admits_entry_after_fresh_below_limit_snapshot`: proves explicit fresh below-limit snapshots admit otherwise-valid entries.
  - `configured_loss_governor_admit_uses_runtime_clock_after_fresh_snapshot_update`: failed while the live-facing `admit()` path had no runtime clock, then passed after `admit()` evaluated snapshots against system time.
  - `breached_loss_governor_halts_entries_but_allows_risk_reducing_exit_within_count_cap`: proves deterministic multi-breach halt evidence while preserving risk-reducing exits.
  - `live_node_build_carries_configured_loss_governor_into_submit_admission`: proves live-node build carries enabled TOML policy into the shared submit-admission state.
  - `nt_runtime_feed_publishes_fresh_portfolio_loss_snapshot_to_submit_admission`: failed before the runtime-feed API existed, then passed after NT portfolio facts published a baseline submit-admission loss snapshot.
  - `subscribed_nt_events_update_submit_admission_loss_snapshot`: failed before the subscription API existed, then passed after NT msgbus portfolio subscriptions fed the loss snapshot into submit admission.
  - `rolling_window_advances_from_portfolio_pnl_deltas_and_evicts_on_heartbeat`: failed while rolling PnL depended on position adjustments, then passed after portfolio PnL deltas and heartbeat eviction drove the rolling window.
  - `position_adjustment_does_not_mask_larger_per_trade_loss`: failed while position adjustments overwrote trade-level PnL, then passed after changed/closed position events became the per-trade source.
  - `stale_peak_timestamp_does_not_make_fresh_portfolio_snapshot_stale`: failed while historical peak timestamps could stale a fresh account heartbeat, then passed after snapshot freshness moved to the latest accepted NT account event.
  - `live_node_build_carries_configured_loss_governor_runtime_feed_subscription`: failed before live runtime owned the feed, then passed after enabled TOML created a subscribed runtime feed.

**Live protection boundary**: This PR proves submit-admission loss protection for configured policy plus configured NT-derived runtime snapshots. Enabled live builds now carry the policy and subscribed runtime feed, and fail closed until enough configured-account NT facts are observed. It does not call `RiskEngine::set_trading_state`, cancel orders, flatten positions, or wire the pure positional-sizer decision into the strategy submit path.
