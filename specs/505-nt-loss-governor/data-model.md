# Data Model: NT-First Loss Governor

## LossGovernorPolicy

Config-derived thresholds used by the evaluator. When enabled, live-node build carries the same policy into shared submit-admission state.

Fields:

- `max_snapshot_age_ns`: maximum accepted age of NT-derived facts.
- `[risk.loss_governor].rolling_window_ns`: config-owned rolling-window length for the live NT event feed.
- `max_per_trade_loss`: configured loss threshold for one trade.
- `max_daily_loss`: configured daily/session loss threshold. With `portfolio = None`, the AccountState-only fallback enforces this against process-session equity delta from the first observed AccountState until a PortfolioSnapshot becomes authoritative.
- `max_rolling_loss`: configured rolling-window loss threshold.
- `max_drawdown`: configured drawdown threshold from peak equity to current equity.

Validation:

- Freshness bound is required.
- Configured thresholds must be positive.
- When `[risk.loss_governor].enabled = true`, every threshold must be configured; missing values fail validation instead of falling back to defaults.

## LossSnapshot

A snapshot derived from NT portfolio/account state and position events.

Fields:

- `source`: non-empty attribution to the NT source or capture path.
- `observed_at_ns`: conservative NT-derived observation timestamp for the facts in the published snapshot.
- `per_trade_pnl`: optional PnL for current trade context.
- `daily_pnl`: optional daily/session PnL. Portfolio snapshots provide NT daily PnL facts; when `portfolio = None`, AccountState-only fallback uses current equity minus the first observed account equity in the process.
- `rolling_pnl`: optional rolling-window PnL from the configured live feed window.
- `current_equity`: optional current equity.
- `peak_equity`: optional peak equity for drawdown evaluation.

Validation:

- Missing or empty source fails closed.
- Missing required field for a configured policy dimension fails closed.
- Snapshot older than policy freshness fails closed.
- Fresh portfolio heartbeats refresh aggregate daily, rolling, current-equity, and drawdown facts, and evict expired rolling-window samples. AccountState heartbeats keep `portfolio = None` deployments live, but their daily-loss fallback is process-session scoped.

## LossGovernorRuntimeFeed

Live in-process feed for the configured account.

Inputs:

- NT `PortfolioSnapshot`: supplies realized PnL, unrealized PnL, total equity, account id, and event timestamp.
- NT `AccountState`: supplies configured-account equity and collateral facts when periodic portfolio snapshots are disabled.
- NT `PositionEvent`: supplies per-trade PnL facts from changed/closed position events. Adjustments do not overwrite a trade-level PnL fact.

Rules:

- Ignore events for other accounts.
- Use configured `rolling_window_ns`; no hardcoded window.
- Derive rolling PnL from deltas between accepted NT portfolio PnL snapshots, or from AccountState equity deltas before any portfolio PnL source is observed.
- Track peak equity from observed NT total-equity snapshots.
- Publish a `LossSnapshot` to submit admission whenever enough NT-derived facts exist.
- Invoke the configured halt-action handler only after a complete `LossSnapshot` is published.

## LossGovernorHaltActionPolicy

Config-derived action policy for active loss halts.

Fields:

- `on_loss_breach_trading_state`: NT trading-state action for threshold breaches.
- `on_untrusted_snapshot_trading_state`: NT trading-state action for missing, stale, or unattributed snapshots.
- `recovery_mode`: recovery policy; currently manual only.
- `manual_recovery_evidence_max_path_bytes`: config-owned bound for operator evidence path strings accepted by live manual recovery.

Validation:

- Enabled loss-governor configs must set every trading-state and recovery field explicitly.
- Manual recovery evidence path bounds must be configured and positive.

## LossGovernorManualRecoveryEvidence

Operator evidence required by the live manual-recovery method.

Rules:

- Recovery can target `Active` only when the current NT risk state is `Halted` or `Reducing`.
- The latest loss snapshot must be fresh, attributed, complete, and accepted by the configured loss policy.
- Evidence must carry a non-empty operator id, relative bounded evidence path, canonical SHA-256, and non-zero observation timestamp.
- The live method sets NT `RiskEngine::set_trading_state(Active)` only after the pure policy returns `Active`.

## LossAdmissionDecision

Output of evaluating proposed admission.

Fields:

- `accepted`: true only when every configured policy passes and snapshot is fresh.
- `halt_reasons`: deterministic list of `LossHaltReason`.
- Submit-admission decision evidence records halt reasons when admission is rejected by the loss governor.

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
