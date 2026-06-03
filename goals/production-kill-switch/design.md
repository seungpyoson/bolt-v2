# Production Kill Switch Design

## Goal

Build a production-grade bolt-v3 kill switch that detects halt triggers, durably latches the halt, blocks new risk globally, cancels all outstanding order risk, flattens open positions through NT, proves the system is flat/no-open-risk from NT state, and requires authorized manual reset evidence before normal risk admission resumes.

This design is for a future implementation issue. The current setup goal does not implement production code.

## Non-Goals

- No bespoke venue cancel, flatten, balance, or position calls.
- No strategy-local kill-switch logic.
- No claim that the existing loss governor is a full kill switch.
- No tiny-capital live drill in this issue; that requires separate approval after no-submit proof.

## Runtime Architecture

### Components

1. `BoltV3KillSwitchStateMachine`
   - Pure Rust state machine.
   - Owns legal transitions and trigger/action/reconciliation result validation.
   - Testable without NT.

2. `BoltV3KillSwitchStore`
   - Durable evidence store.
   - Atomic write/rename or create-new append semantics.
   - Stores current state, prior state, trigger facts, action attempts, reconciliation proof, and manual reset evidence.
   - Missing/corrupt/stale unresolved evidence fails closed on startup.

3. `BoltV3KillSwitchAdmissionGate`
   - Shared global latch read by `BoltV3SubmitAdmissionState`.
   - Blocks `Entry` and `ReplaceSubmit`.
   - Allows ordinary risk-reducing exits only under the ordinary submit-admission caps.
   - Requires a separate `KillSwitchForcedReduction` submit/action class for verified flatten orders, because ordinary max-notional and live-order-count caps can otherwise deadlock flattening after a halt.
   - `KillSwitchForcedReduction` may bypass ordinary entry/live-order caps only when bound to a durable halt id, current NT position proof, config-owned forced-reduction policy, and a proven order route that cannot increase exposure.
   - Writes deterministic rejection evidence.

4. `BoltV3KillSwitchRuntime`
   - Wired from `src/bolt_v3_live_node.rs`.
   - Receives triggers from loss-governor admission outcomes, manual operator command, missing/stale NT proof, reconciliation mismatch, and action failures.
   - Holds cloned NT `Rc<RefCell<...>>` handles for cache, portfolio, risk engine, and action routing where safe.

5. `BoltV3KillSwitchActionRouter`
   - Global action coordinator, not a standalone strategy-local policy.
   - Must prove its NT command routing before any production side effects.
   - May be implemented as narrow per-registered-strategy action ports or as a live-node command router.
   - If per-strategy ports are used, each registered strategy only executes its own NT cancel/flatten commands; trigger policy, sequencing, evidence, and reconciliation remain global.
   - If a live-node command router is used, it must preserve original strategy/client/order/position identity and send standard NT trading commands through NT risk/execution routes.
   - Must not rely on one standalone kill-switch strategy using `Strategy::cancel_order` or `Strategy::close_all_positions` to manage other strategies' orders or positions.

6. `BoltV3KillSwitchReconciler`
   - Reads NT cache/portfolio state.
   - Proves no outstanding order risk, no non-flat positions, and no pending entry risk for configured accounts/instruments/strategies.
   - Outstanding order risk includes open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and locally accepted-but-not-terminal orders.
   - Writes reconciliation evidence.

7. Operator CLI/runbook path
   - Manual kill command.
   - Manual reset command with authorization, operator identity, evidence path/hash, and tamper-evident append/hash-chain record.
   - No-submit kill drill command.
   - Redacted event-log viewer.

## State Machine

States:

- `Armed`: normal runtime; no active halt.
- `Halting`: trigger accepted, durable evidence write in progress, admission latch must already block new risk.
- `Halted`: durable halt is latched and no new risk may be admitted.
- `Cancelling`: outstanding-order-risk cancellation loop is active.
- `Flattening`: position flatten loop is active.
- `Flat`: NT reconciliation proves no outstanding order risk, no non-flat positions, and no pending entry risk. Manual reset is still required before returning to `Armed`.
- `FailedManualIntervention`: action or proof failed; runtime remains fail-closed until operator intervention.

Transition rules:

- `Armed -> Halting`: any accepted trigger.
- `Halting -> Halted`: trigger and latch evidence persisted.
- `Halting -> FailedManualIntervention`: durable state write, append, rename, or fsync fails. Runtime remains locally fail-closed; if the durable store cannot record the halt, recovery requires operator intervention and the enabled kill-switch startup path treats missing/corrupt state as fail-closed.
- `Halted -> Flat`: reconciliation immediately proves no outstanding order risk, no non-flat positions, and no pending entry risk.
- `Halted -> Cancelling`: cancel policy is enabled and outstanding cancellable order risk exists.
- `Halted -> Flattening`: outstanding order risk is terminal or reconciled, and open positions exist.
- `Halted -> FailedManualIntervention`: outstanding order risk exists but cancel policy is disabled, unavailable, or cannot route through a proven NT path.
- `Cancelling -> Flattening`: all outstanding order risk is terminal/cancelled/filled into known positions, and open positions exist.
- `Cancelling -> Flat`: all outstanding order risk is terminal/cancelled and reconciliation proves no open positions or pending entry risk.
- `Cancelling -> FailedManualIntervention`: any outstanding order risk remains unresolved after configured retry/timeout budget, or filled-before-cancel cannot be reconciled into known positions.
- `Flattening -> Flat`: reconciliation proves flat/no-open-risk.
- `Flattening -> FailedManualIntervention`: proof missing, timeout exhausted, unsupported instrument, unknown side, contradictory NT state, rejected order, forced-reduction admission denied, thin-book failure, or configured retry budget exhausted.
- `Flat -> Armed`: manual reset evidence accepted, operator authorization verified, reset evidence is append-only/tamper-evident, and fresh NT proof remains clean.
- `FailedManualIntervention -> Armed`: only with explicit authorized operator reset evidence, repaired durable state, and fresh NT proof; implementation may require process restart if safer.

The transition table must be exhaustive over these live-state booleans before implementation: `has_outstanding_order_risk`, `has_open_positions`, `cancel_policy_enabled`, `cancel_route_proven`, `flatten_route_proven`, `forced_reduction_admission_proven`, `state_store_healthy`, `state_write_succeeded`, `mandatory_proof_streams_fresh`, `reconciliation_fresh`, `retry_budget_exhausted`, `operator_authorized`, and `manual_reset_evidence_valid`.

## Trading-State Integration

Pinned NT exposes `RiskEngine::set_trading_state`.

Recommended behavior:

- On latch, set NT risk state to `TradingState::Reducing` if the safe runtime handle is accessible.
- Keep the local durable admission latch as the primary fail-closed authority.
- During `Cancelling` and `Flattening`, `Reducing` allows reducing orders while NT rejects accidental exposure-increasing submits.
- Do not rely on `TradingState::Reducing` alone as flatten authorization. Bolt must still prove a kill-switch forced-reduction admission path so ordinary admission caps cannot block required flatten orders and so forced exits cannot increase exposure.
- After `Flat`, either keep local halt latch active or set NT to `Halted` until manual reset. The implementation issue should require tests for whichever choice is made.
- On manual reset, restore NT to `TradingState::Active` only after durable reset evidence and fresh clean reconciliation proof.

If a future pinned NT or live boundary prevents safe access to `set_trading_state`, implementation must document the exact source gap and rely on local fail-closed admission blocking.

## Trigger Sources

Initial trigger set:

- Loss-governor breach or stale/missing loss proof.
- Manual operator kill.
- Stale or missing NT portfolio/order/position proof.
- Reconciliation mismatch.
- Cancel or flatten failure.
- Extension points for future risk, heartbeat, or venue health triggers.

Every trigger writes:

- Trigger kind.
- Reason set.
- Source event timestamp.
- NT source timestamp where available.
- Runtime observed timestamp.
- Operator identity or manual evidence ref where applicable.
- Config hash/root path binding.

## Cancel Design

Cancel loop:

1. Enumerate outstanding order risk from NT cache using config-owned account/instrument/strategy filters.
   - Required surfaces: open orders, inflight orders, pending-cancel orders, emulated orders, algorithm-managed orders, contingent orders, and any locally accepted entry/replace submit whose terminal event is not proven.
2. Snapshot client order IDs and current order facts without holding long cache borrows.
3. Route cancellation through a proven NT path:
   - per-strategy action port calling NT strategy methods for that strategy's own orders, or
   - live-node command router sending standard NT cancel commands while preserving original order strategy/client identity.
4. Track `cancel_requested`, `cancel_accepted`, `cancel_rejected`, `pending_cancel`, `expired`, `filled_before_cancel`, `terminal_before_cancel`, emulated/algo cancellation cascades, retry attempts, and final status.
5. Retry only according to TOML-owned budgets and backoff.
6. Never call a venue-specific cancel API directly.

Filled-before-cancel is not success by itself; it must flow into position reconciliation and possible flattening.

## Flatten Design

Flatten loop:

1. Enumerate open positions from NT cache/portfolio using config-owned filters.
2. Reject or enter `FailedManualIntervention` for unknown side, unsupported instrument, missing market data required by the configured flatten template, or stale position state.
3. Build reduce-only or forced-exit orders through existing NT order construction path.
   - Preferred path is reduce-only.
   - If an instrument/venue cannot support reduce-only, the forced-exit template must prove side and quantity are bounded by current NT position proof and cannot increase exposure; otherwise enter `FailedManualIntervention`.
   - Forced reductions must use `KillSwitchForcedReduction`, not ordinary `RiskReducingExit`.
4. Submit through a proven NT path:
   - per-strategy action port for that strategy's own positions, or
   - live-node command router that initializes NT order/cache state correctly and submits through risk/execution with preserved identity.
5. Track partial fills, rejected submits, forced-reduction admission denials, residual positions, and retry budget.
6. Continue until reconciliation proves flat or failure policy requires manual intervention.

The existing strategy-local forced-flat code is evidence that NT paths exist, not the production boundary for the global kill switch.

Forced-reduction admission policy:

- Bound every forced flatten submit to halt id, trigger evidence hash, position id, account id, instrument id, side, position quantity, order quantity, price source, route, and operator/config policy hash.
- Exempt verified forced reductions from ordinary entry/replace count caps and ordinary `max_notional_per_order` only if TOML explicitly enables that kill-switch policy.
- Never exempt from NT instrument validity, side/quantity sanity, route proof, or exposure-increase checks.
- If forced-reduction policy is disabled, too small to flatten the proven position, or cannot prove exposure reduction, enter `FailedManualIntervention` rather than silently falling back to ordinary submit admission.

## Reconciliation

Flat proof requires all of:

- NT cache reports no outstanding order risk for configured filters: open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal entry/replace submits.
- NT cache/portfolio reports no open positions for configured filters.
- Submit admission has no pending entry risk and no in-flight entry/replace accepted after latch timestamp.
- Mandatory captured order/position/risk event streams are fresh and consistent with the cache proof.
- Optional proof streams may be absent only if config marks them optional and the proof records the stronger cache/portfolio query evidence, source timestamp, and reason that substitutes for the missing stream.

If any mandatory proof is missing, stale, or contradictory, the state remains halted or enters `FailedManualIntervention`.

## Config Contract

Suggested TOML ownership:

- `[risk.kill_switch]`
  - `enabled`
  - `state_path`
  - `event_log_path`
  - `accounts`
  - `instruments`
  - `strategies`
  - `max_state_age_ns`
  - `manual_reset_required`
- `[risk.kill_switch.cancel]`
  - retry count, retry backoff, timeout, batch size.
- `[risk.kill_switch.flatten]`
  - order template, reduce-only requirement, forced-reduction policy, time-in-force, price source/fallback, timeout, retry budget.
- `[risk.kill_switch.reconciliation]`
  - freshness bounds, mandatory and optional proof streams, outstanding-order surface requirements, max wait.
- `[risk.kill_switch.operator]`
  - manual trigger path, reset evidence path, authorization source, operator identity binding, append-only or hash-chained evidence policy, redaction policy.

All runtime values must be TOML-owned and validated when enabled.

## Verification Matrix

- Unit: state machine transition table, illegal transitions, manual reset evidence, fail-closed restart.
- Unit: config validation rejects missing or non-positive runtime values.
- Unit: durable store write/read/fsync failure forces fail-closed/manual-intervention behavior.
- Unit: admission gate blocks entry/replace, keeps ordinary risk-reducing exits under ordinary caps, and admits only verified `KillSwitchForcedReduction` actions through the forced-reduction policy.
- Unit: unauthorized, stale, or tampered manual reset evidence cannot transition to `Armed`.
- Integration: cancel loop handles accepted, rejected, pending-cancel, inflight, emulated, algorithm-managed, contingent, expired, terminal-before-cancel, and filled-before-cancel races.
- Integration: flatten loop handles ordinary cap exhaustion, over-normal-cap forced reductions, partial fills, rejects, residual positions, unknown side, stale state, unsupported instrument, and retry exhaustion.
- Integration: reconciliation refuses flat when any mandatory proof is missing, stale, or contradictory.
- Restart: unresolved durable halt evidence blocks startup admission.
- Source fence: strategies cannot import or instantiate kill-switch policy/runtime and cannot bypass global submit/cancel/flatten paths.
- No-submit drill: simulated live node can trigger kill, persist evidence, execute dry-run cancel/flatten decisions, and produce reconciliation proof without live venue submits.
- Docs: runbook documents manual kill, manual reset, proof inspection, and failure escalation.

## Phasing

1. Design and issue only: this setup goal.
2. Pure state/config/evidence slice with no NT side effects.
3. Admission latch, `KillSwitchForcedReduction` policy model, and source fences.
4. NT trading-state integration and no-submit action actor skeleton.
5. Cancel loop over all outstanding-order surfaces with event/race tests.
6. Flatten loop with forced-reduction admission and reconciliation tests.
7. Restart/recovery and runbook.
8. No-submit end-to-end kill drill.
9. Tiny-capital live drill only after separate approval.

## PR #480 Dependency

PR #480 currently owns production trade-readiness consolidation and may reshape order-intent/admission boundaries. The kill-switch issue should mark live submit/cancel/flatten wiring as dependent on #480 landing on `main`. PR-independent slices are limited to design, pure state machine, config validation, evidence schema, and source-fence tests.
