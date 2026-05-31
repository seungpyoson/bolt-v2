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
- Configured policy wiring into shared submit admission during live-node build.
- NT `PortfolioSnapshot` plus `PositionEvent` feed into submit admission loss snapshots.
- Submit-admission rejection for entry/replace new risk on missing, stale, or breached loss facts.
- Decision evidence outcome `rejected_loss_governor_halted` plus `loss_halt_reasons`.

**Out of scope**:

- Strategy-specific risk logic.
- Cancel/flatten side effects.
- Explicit NT `RiskEngine::set_trading_state` side effects.
- Bespoke venue side effects.
- Strategy file changes.

## Implementation Evidence

**Decision**: Implemented submit-admission protection without cancel/flatten side effects.

**Evidence**:

- `src/bolt_v3_loss_governor.rs`: adds config-derived `LossGovernorPolicy`, NT-derived `LossSnapshot`, `LossAdmissionDecision`, `LossHaltReason`, and `evaluate_loss_admission`.
- `src/lib.rs`: exports `bolt_v3_loss_governor`.
- `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, `tests/config_parsing.rs`: add and validate `[risk.loss_governor]` fields, including configured account id, freshness, rolling window, and thresholds.
- `src/bolt_v3_submit_admission.rs`, `tests/bolt_v3_submit_admission.rs`: add configured loss-governor state, snapshot updates, deterministic halt evidence, fail-closed new-risk admission, and risk-reducing-exit allowance under existing caps.
- `src/bolt_v3_live_node.rs`: maps loaded TOML policy into submit admission and subscribes to NT portfolio/position events to refresh the loss snapshot before and during the live runner.
- `src/bolt_v3_decision_evidence.rs`: records `rejected_loss_governor_halted` and `loss_halt_reasons`.
- Decision-evidence envelope schema is bumped to v6 for the loss-governor halt outcome and `loss_halt_reasons`.
- Live feed snapshots use the oldest timestamp among combined NT facts so unrelated newer events cannot make stale PnL/equity facts appear fresh.
- TDD red/green evidence:
  - `per_trade_loss_breach_rejects_admission`: failed before per-trade evaluation, then passed.
  - `daily_loss_breach_rejects_admission`: failed before daily evaluation, then passed.
  - `stale_missing_or_unattributed_snapshot_fails_closed`: failed before fail-closed freshness/source/required-field checks, then passed.
  - `rolling_loss_breach_rejects_admission`: failed before rolling evaluation, then passed.
  - `max_drawdown_breach_rejects_admission`: failed before drawdown evaluation, then passed.
  - `loss_governor_rejects_new_risk_without_fresh_snapshot_before_nt_submit`: failed before submit-admission loss wiring, then passed.
  - `loss_governor_halts_entries_but_allows_risk_reducing_exits_within_count_cap`: failed before halt evidence/exit allowance, then passed.
  - `loss_governor_feed_builds_snapshots_from_nt_portfolio_and_position_events`: failed before runtime feed, then passed.

**Live protection boundary**: This slice proves submit-admission protection for new entry/replace risk after the live runner is armed and NT-derived loss snapshots are fresh. It does not call `RiskEngine::set_trading_state`, cancel orders, or flatten positions.
