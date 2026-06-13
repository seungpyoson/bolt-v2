# Bolt-v3 Kill-Switch Loss Protection

This document describes the durable realized-loss kill switch wired through `[risk.kill_switch]`.

It is the **hard** control in a two-control design: the instantaneous, unrealized-inclusive `LossGovernorRuntimeFeed` feeds the soft admission gate (`max_daily_loss`), while this controller owns the durable, realized-only daily accumulator that latches the kill switch at `daily_realized_loss_limit` and persists across restarts.

## Breach Behavior

When enabled, the live runner subscribes to NT position events and records realized PnL for configured `account_ids` and `instrument_ids`. If same-day realized loss reaches `daily_realized_loss_limit`, the controller:

- transitions the shared kill-switch state from `Armed` to `Halting`
- persists the halt evidence to `state_path`
- replaces submit admission state so new entry orders are rejected
- emits the flatten-positions forced-exit action
- dispatches NT `Trader::market_exit_strategy` once per strategy for the halt

The market-exit command is the forced-exit path: NT sends each strategy an exit command, and the strategy performs its managed cancel and close sequence.
If one strategy dispatch fails, retry state records the strategies that already accepted the halt so subsequent retries only target the incomplete strategies for that halt.

## Runtime Snapshot

`state_path` contains the kill-switch state and the loss-protection runtime snapshot:

- current UTC day bucket
- same-day realized PnL accumulator
- per-position cumulative realized PnL baselines
- pending halt-action retry schedule, when a forced-exit dispatch failed

The accumulator rotates only when an observed event moves to a later UTC day bucket. Older out-of-order events are ignored for the current bucket so delayed prior-day position events cannot clear same-day losses.

Closed positions remove their cumulative PnL baseline after their final delta is applied.

Pending halt actions are retried from a live timer using `action_retry_interval_ms`, not from position-event arrival. If the retry deadline is exceeded, the controller persists `FailedManualIntervention` and blocks entries.

## Restart Recovery

Startup loads `state_path` before the NT runner loop starts.

- `Armed` recovers only with a valid loss-protection runtime snapshot.
- `Halted` and `Flat` recover as stored.
- `Halting` and `FailedManualIntervention` fail closed and block entries.
- Missing, corrupt, incomplete, or unsupported evidence fails closed and blocks entries.

Deleting the store file is not a reset. A missing store is treated as missing evidence and restarts into `FailedManualIntervention`.

## Manual Reset Evidence

A reset requires operator evidence that the account is flat, there are no open orders, mandatory proof streams are fresh, and no pending entry risk remains. Evidence must include an authorized operator id, a root-relative evidence path, a SHA-256 of the evidence packet, and a request timestamp within `manual_reset_evidence_max_age_ms`.

Only an approved reset path may write a recovered `Armed` state to `state_path`. Operators must archive the prior halt store and reset evidence packet together so the halt id, policy hash, account ids, instrument ids, and proof hashes remain reviewable after restart.
