# Bolt-v3 Kill-Switch Loss Protection

This document records the initial session-2 loss-protection kill-switch design.
The current implementation is the hardened durable-realized-loss path described in `docs/bolt-v3/2026-06-13-kill-switch-loss-protection.md`.

## Breach Behavior

When `[risk.kill_switch]` is enabled, the live runner records realized PnL for configured `account_ids` and `instrument_ids`.
If same-day realized loss reaches the configured daily limit, the controller:

- transitions the shared kill-switch state from `Armed` to `Halting`
- persists halt evidence to `state_path`
- replaces submit-admission state so new entry orders are rejected
- emits a flatten-intent halt action for proof and retry bookkeeping
- moves the NT risk engine to `Reducing`

The current accepted boundary does not call `Trader::market_exit_strategy`, direct NT cancel, or direct venue flatten paths.
`flatten_open_positions_on_breach = true` is rejected until a shared execution-policy flatten path exists.

## Runtime Snapshot

`state_path` contains the kill-switch state and the loss-protection runtime snapshot:

- current UTC day bucket
- UTC-daily realized PnL accumulator
- settlement currency bound to the realized PnL accumulator
- per-position cumulative realized PnL baselines
- closed-position and adjusted-position replay guards
- pending halt-action retry schedule, when proof/action recording has not completed

The controller persists the runtime snapshot with the durable kill-switch state so restart recovery cannot silently discard daily loss evidence.

## Restart Recovery

Startup loads `state_path` before the NT runner loop starts.

- `Armed` recovers only with a valid loss-protection runtime snapshot.
- `Halted` and `Flat` recover as stored.
- `Halting` and `FailedManualIntervention` fail closed and block entries.
- Missing, corrupt, oversized, incomplete, or unsupported evidence fails closed and blocks entries.

Deleting the store file is not a reset path. A missing store is treated as missing evidence.

On a fresh install, initialize the first store file before setting `enabled = true`:

```bash
bolt-v2 ops init-kill-switch-store --config config/root.toml
```

The initializer writes an `Armed` state with an empty loss-protection snapshot and refuses to overwrite an existing store.

## Manual Reset Evidence

A reset requires operator evidence that the account is flat, there are no open orders, mandatory proof streams are fresh, and no pending entry risk remains.
Evidence must include an authorized operator id, a root-relative evidence path, a SHA-256 of the evidence packet, and a request timestamp within `manual_reset_evidence_max_age_ms`.

Only an approved reset path may write a recovered `Armed` state to `state_path`.
Operators must archive the prior halt store and reset evidence packet together so halt id, policy hash, account ids, instrument ids, and proof hashes remain reviewable after restart.
