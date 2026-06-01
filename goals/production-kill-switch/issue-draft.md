# Issue Draft: Build A Production-Grade Global Kill Switch

Status: filed as https://github.com/seungpyoson/bolt-v2/issues/517 by direct user instruction. External model approval quorum has not passed; record that limitation in the issue rather than presenting the design as externally approved.

## Summary

Build a production-grade global bolt-v3 kill switch for configured accounts, instruments, and strategies. The system must detect halt triggers, durably latch the halt, block new risk globally, cancel all outstanding order risk, flatten open positions through NT-native paths, reconcile from NT state that there are no outstanding orders/non-flat positions/pending entry risk, and require authorized manual reset evidence before normal trading resumes.

This is not an implementation claim for the current branch. The existing loss governor is one trigger source; it is not a complete production kill switch by itself.

## Production Invariants

- The kill switch is global LiveNode/runtime infrastructure, not strategy-local logic.
- Halt state is durable and survives restart; unresolved, missing, stale, or corrupt evidence fails closed.
- Once latched, all entry and replace risk is blocked globally.
- Ordinary risk-reducing exits remain under ordinary caps; verified kill-switch forced reductions use a separate proof-bound admission/action class so normal notional/count caps cannot deadlock flattening.
- Only explicitly risk-reducing, cancel, forced-reduction flatten, and reconciliation actions remain eligible while halted.
- Cancel and flatten use NT-owned state and NT command paths only. No bespoke venue cancel, flatten, balance, or position calls.
- `TradingState::Reducing` is used when safely accessible for cancel/flatten phases; `TradingState::Halted` or local latch remains active until manual reset after flat proof.
- Flat is never claimed without NT proof of no outstanding order risk, no non-flat positions, and no pending entry risk.
- Outstanding order risk includes open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal order surfaces.
- Manual reset requires authorization, tamper-evident evidence, and fresh clean reconciliation proof.
- All runtime values are TOML-owned and validated when enabled.
- Source fences prevent strategy-local kill policy and direct venue kill paths.

## Implementation Phases

1. Pure state/config/evidence slice with no NT side effects.
2. Admission latch, `KillSwitchForcedReduction` policy model, and source fences.
3. NT trading-state integration and no-submit action-router skeleton.
4. Cancel loop with open/inflight/pending-cancel/emulated/algo/contingent accepted/rejected/expired/filled-race tests.
5. Flatten loop with forced-reduction admission, normal-cap-exhaustion, partial-fill/reject/residual/reconciliation tests.
6. Restart/recovery and operator runbook.
7. No-submit end-to-end kill drill.
8. Tiny-capital live drill only after separate approval.

## Proposed Implementation Targets

- `src/bolt_v3_kill_switch.rs`: pure state machine, trigger model, action model, reconciliation model.
- `src/bolt_v3_kill_switch_store.rs`: durable halt/reset evidence store.
- `src/bolt_v3_config.rs` and `src/bolt_v3_validate.rs`: `[risk.kill_switch]` TOML parsing and validation.
- `src/bolt_v3_submit_admission.rs`: global halt latch enforcement before NT submit and a proof-bound `KillSwitchForcedReduction` admission path.
- `src/bolt_v3_live_node.rs`: runtime wiring, NT risk state integration, and selected action-routing boundary.
- `src/bolt_v3_strategy_registration.rs`: shared runtime handles and optional per-strategy action ports without strategy-local kill policy.
- `src/bolt_v3_order_intent.rs`: reuse typed NT order construction for flatten orders.
- `scripts/verify_bolt_v3_strategy_policy_fence.py`: prevent strategy-local kill logic and direct venue bypasses.
- `docs/bolt-v3/runbooks/production-kill-switch.md`: operator kill/reset/no-submit drill runbook.

## Verification

- Unit tests for every state-machine transition, illegal transition, manual reset evidence, and fail-closed restart behavior.
- Unit tests for durable state write/read/fsync failure and startup behavior when kill-switch state is missing, corrupt, stale, or unresolved.
- Config validation tests for missing or invalid retry, timeout, template, account, instrument, strategy, freshness, and reset settings.
- Admission tests showing entry/replace blocked, ordinary risk-reducing exits still subject to ordinary caps, and verified forced reductions can proceed when normal order-count or notional caps would otherwise block flattening.
- Integration tests for cancel accepted, rejected, pending-cancel, inflight, emulated, algorithm-managed, contingent, expired, terminal-before-cancel, and filled-before-cancel races.
- Integration tests for flatten normal-cap exhaustion, over-normal-cap forced reduction, partial fills, rejected submits, residual positions, unknown side, stale state, unsupported instrument, thin book, timeout, and retry exhaustion.
- Reconciliation tests proving flat is refused when any mandatory proof stream is missing, stale, or contradictory.
- Restart/recovery tests proving unresolved durable halt evidence blocks startup admission.
- Manual-reset tests proving unauthorized, stale, or tampered reset evidence cannot return to armed trading.
- Source-fence checks for strategy-local kill policy and direct venue kill/cancel/flatten bypasses.
- No-submit end-to-end kill drill that persists evidence and produces dry-run cancel/flatten/reconciliation proof without live venue submits.

## Dependencies

- Live submit/cancel/flatten wiring depends on PR #480 landing on `main`.
- PR-independent slices are limited to design, pure state machine, config validation, evidence schema, and source-fence tests.
- Tiny-capital live drill is out of scope and requires separate approval after no-submit proof.

## External Review Evidence

External review quorum has not passed.

Required gate:

- At least four of six approvals.
- Claude and Gemini must be included.
- Blocking findings must be resolved and rerun before implementation approval.

Current evidence lives in `goals/production-kill-switch/reviews.md`.
