# Bolt-v3 Durable Kill-Switch Loss Protection

This document describes the durable realized-loss kill switch wired through `[risk.kill_switch]`.

It is the hard control in a two-control design: the instantaneous, unrealized-inclusive `LossGovernorRuntimeFeed` feeds the soft admission gate, while this controller owns the durable, realized-only UTC-daily accumulator that latches the kill switch at `max_utc_daily_realized_loss` and persists across restarts.

## Breach Behavior

When enabled, the live runner subscribes to NT position events and records realized PnL for configured `account_ids` and `instrument_ids`.
If same-day realized loss reaches `max_utc_daily_realized_loss`, the controller:

- transitions the shared kill-switch state from `Armed` to `Halting`
- persists the halt evidence and loss-protection snapshot to `state_path`
- replaces submit-admission state so new entry orders are rejected
- emits the flatten-positions halt action for proof and retry bookkeeping
- moves the NT risk engine to `Reducing`

The accepted #738 boundary is proof-only for cancel and flatten side effects.
Active market exit is intentionally not wired: direct NT `ExitMarket` or market-exit control paths bypass Bolt's submit/cancel chokepoints.
Config validation rejects `flatten_open_positions_on_breach = true` until a shared execution-policy flatten path exists.

## Runtime Snapshot

`state_path` contains the kill-switch state and the loss-protection runtime snapshot:

- current UTC day bucket
- same-day realized PnL accumulator
- per-position cumulative realized PnL baselines
- closed-position replay guards
- adjusted-position replay guards
- pending halt-action retry schedule, when a halt action has not completed

The accumulator rotates only when an observed event moves to a later UTC day bucket.
Older out-of-order events for prior UTC day buckets are ignored and cannot clear same-day losses.

Closed positions move their cumulative PnL baseline into replay guards after the final delta is applied.
A later lifecycle for the same position id can clear the guard only through fresh position evidence.

Pending halt actions are retried from a live timer using `action_retry_interval_ms`, not from position-event arrival.
If the retry deadline is exceeded, the controller persists `FailedManualIntervention` and blocks entries.

## Restart Recovery

Startup loads `state_path` before the NT runner loop starts.

- `Armed` recovers only with a valid loss-protection runtime snapshot.
- `Halted` and `Flat` recover as stored.
- `Halting` and `FailedManualIntervention` fail closed and block entries.
- Missing, corrupt, oversized, incomplete, or unsupported evidence fails closed and blocks entries.

Deleting the store file is not a reset. A missing store is treated as missing evidence.

## Manual Reset Evidence

A reset requires operator evidence that the account is flat, there are no open orders, mandatory proof streams are fresh, and no pending entry risk remains.
Evidence must include an authorized operator id, a root-relative evidence path, a SHA-256 of the evidence packet, and a request timestamp within `manual_reset_evidence_max_age_ms`.

Only an approved reset path may write a recovered `Armed` state to `state_path`.
Operators must archive the prior halt store and reset evidence packet together so the halt id, policy hash, account ids, instrument ids, and proof hashes remain reviewable after restart.
