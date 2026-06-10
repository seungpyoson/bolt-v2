# Bolt-v3 Kill-Switch Loss Protection

This document describes the session-2 loss-protection kill switch wired through `[risk.kill_switch]`.

## Breach Behavior

When enabled, the live runner subscribes to NT position events and records realized PnL for configured `account_ids` and `instrument_ids`. If same-day realized loss reaches `daily_realized_loss_limit`, the controller:

- transitions the shared kill-switch state from `Armed` to `Halting`
- persists the halt evidence to `store_path`
- replaces submit admission state so new entry orders are rejected
- emits cancel-open-orders and flatten-positions actions
- dispatches NT `Trader::market_exit_strategy` once per strategy for the halt

The market-exit command is the forced-exit path: NT sends each strategy an exit command, and the strategy performs its managed cancel and close sequence.

## Restart Recovery

Startup loads `store_path` before the NT runner loop starts.

- `Armed`, `Halted`, and `Flat` recover as stored.
- `Halting` and `FailedManualIntervention` fail closed and block entries.
- Missing, corrupt, or unsupported evidence fails closed and blocks entries.

Deleting the store file is not a reset. A missing store is treated as missing evidence and restarts into `FailedManualIntervention`.

## Manual Reset Evidence

A reset requires operator evidence that the account is flat, there are no open orders, mandatory proof streams are fresh, and no pending entry risk remains. Evidence must include an authorized operator id, a root-relative evidence path, a SHA-256 of the evidence packet, and a request timestamp within `manual_reset_evidence_max_age_ms`.

Only an approved reset path may write a recovered `Armed` state to `store_path`. Operators must archive the prior halt store and reset evidence packet together so the halt id, policy hash, account ids, instrument ids, and proof hashes remain reviewable after restart.
