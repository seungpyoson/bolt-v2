# Data Model: NT-First Loss Governor

## LossGovernorPolicy

Config-derived thresholds used by the evaluator and shared submit-admission state.

Fields:

- `max_snapshot_age_ns`: maximum accepted age of NT-derived facts.
- `[risk.loss_governor].rolling_window_ns`: config-owned rolling-window length used by the live NT event feed.
- `max_per_trade_loss`: configured loss threshold for one trade.
- `max_daily_loss`: configured session/day loss threshold.
- `max_rolling_loss`: configured rolling-window loss threshold.
- `max_drawdown`: configured drawdown threshold from peak equity to current equity.

Validation:

- Freshness bound is required.
- Configured thresholds must be positive.
- When `[risk.loss_governor].enabled = true`, every threshold must be configured; missing values fail validation instead of falling back to defaults.

## LossSnapshot

A snapshot derived from NT Portfolio/account state and position events.

Fields:

- `source`: non-empty attribution to the NT source or capture path.
- `observed_at_ns`: oldest timestamp among the currently combined NT-derived loss/equity facts.
- `per_trade_pnl`: optional PnL for the current open-position trade context; the live feed publishes `0` when no position PnL facts are tracked, and `None` represents an incomplete/unsafe snapshot.
- `daily_pnl`: optional day/session PnL from NT realized plus unrealized PnL facts.
- `rolling_pnl`: optional rolling-window PnL from the configured live feed window.
- `current_equity`: optional current equity.
- `peak_equity`: optional peak equity for drawdown evaluation.

Validation:

- Missing or empty source fails closed.
- Missing required field for a configured policy dimension fails closed; `per_trade_pnl = 0` is the explicit flat/no-tracked-position value.
- Snapshot older than policy freshness fails closed.
- Mixed NT event facts remain conservative: a fresh position event cannot refresh older portfolio PnL/equity facts, and a fresh portfolio snapshot cannot refresh older per-trade facts.

## LossGovernorRuntimeFeed

Live in-process feed for the configured account.

Inputs:

- NT `PortfolioSnapshot`: supplies realized PnL, unrealized PnL, total equity, account id, and event timestamp.
- NT `PositionEvent`: supplies per-trade PnL facts from opened/changed/adjusted position events for currently open positions. Closed position events remove the position from the current-trade context; closed realized losses flow through NT portfolio PnL facts.

Rules:

- Ignore events for other accounts.
- Use configured `rolling_window_ns`; no hardcoded window.
- Publish `rolling_pnl = 0` on the first valid startup sample, then publish rolling PnL only when a prior baseline sample exists inside the configured rolling window; otherwise leave it missing so admission fails closed.
- Track peak equity from observed NT total-equity snapshots.
- Preserve the prior peak equity across invalid portfolio snapshots, while publishing missing portfolio fields for that invalid snapshot.
- Ignore position adjustment deltas for positions without an absolute PnL baseline.
- Publish a `LossSnapshot` to submit admission whenever enough NT-derived facts exist.

## LossAdmissionDecision

Output of evaluating proposed admission.

Fields:

- `accepted`: true only when every configured policy passes and snapshot is fresh.
- `halt_reasons`: deterministic list of `LossHaltReason`.
- submit-admission decision evidence records halt reasons when admission is rejected.

Validation:

- `accepted` is false when `halt_reasons` is non-empty.
- `accepted` is true only when no halt reasons are present.

## LossHaltReason

Public reason for rejection or fail-closed behavior.

Values:

- `per_trade_loss_limit`
- `daily_loss_limit`
- `rolling_loss_limit`
- `max_drawdown_limit`
- `stale_loss_snapshot`
