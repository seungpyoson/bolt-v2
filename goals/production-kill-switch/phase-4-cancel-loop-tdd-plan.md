# #517 Phase 4 TDD Plan: Cancel Loop Proof Model

## Current Base

- Issue: https://github.com/seungpyoson/bolt-v2/issues/517
- Branch: `codex/517-kill-switch-phase4-cancel-loop-plan`
- Stacked base: `codex/517-kill-switch-phase3-runtime-router-plan` at `ecfb0dd779e78970199d4e2606c5f6731dacfaca`
- Upstream Phase 1 PR: https://github.com/seungpyoson/bolt-v2/pull/521
- Upstream Phase 2 PR: https://github.com/seungpyoson/bolt-v2/pull/523
- Upstream Phase 3 PR: https://github.com/seungpyoson/bolt-v2/pull/525
- Scope: Phase 4 planning first. The future implementation slice may add a typed outstanding-order-risk cancel-loop proof model, TOML-owned cancel policy validation, and no-submit cancel-supervisor decisions. It must not add live NT cancel side effects, live flatten routing, final flat proof, loss-governor trigger ingestion, operator reset UX, or tiny-capital live-drill behavior.

## Decision

Plan Phase 4 as a cancel-loop proof and routing-contract slice rather than live order cancellation. Phase 3 created a no-submit action router that can emit `CancelOutstandingRisk` dry-run metadata only while the durable kill-switch state is `Cancelling`. Phase 4 should define the global cancellation supervisor's inputs, coverage, route proof, retry/outcome state, and fail-closed decisions so a later live NT adapter can route actual cancellation without changing the safety model.

This phase intentionally separates three concerns:

1. Outstanding-order-risk enumeration and proof metadata.
2. Cancel outcome/retry state transitions.
3. The live NT execution adapter, which remains out of scope until the source-proven route and exact-head CI/review gates are available.

## Invariants

- Durable kill-switch state remains the authority. Cancel planning is valid only in `KillSwitchState::Cancelling`.
- Entering `Cancelling`, leaving `Cancelling`, and replacing the current `Halted -> Flat` reconciliation shortcut are orchestration/state-machine integration work for a later reconciliation phase. Phase 4 consumes an already-durable `Cancelling` state and proves cancel-loop behavior only.
- Submit admission remains the global exposure-opening block; cancel planning does not create any submit path.
- Outstanding order risk includes open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and locally accepted-but-not-terminal order surfaces.
- Every cancel candidate is scoped by config-owned account, instrument, and strategy filters.
- Every cancel candidate and planned command carries NT identifiers (`AccountId`, `InstrumentId`, `StrategyId`, `ClientOrderId`) and NT `OrderStatus`. Bolt-specific surfaces are evidence classifications over NT-owned order/cache state, not a replacement lifecycle model.
- Cancel route proof must preserve original strategy/client/order identity. One standalone kill-switch strategy cannot cancel other strategies' orders unless a source-backed route proves it can preserve that identity through NT.
- Phase 4 emits proof and planned commands only. It must not call NT `cancel_order`, `cancel_orders`, direct execution engine methods, or venue-specific APIs; later live adapters must route through NT `CancelOrder`, `CancelAllOrders`, or `BatchCancelOrders` shapes.
- `pending_cancel`, `filled_before_cancel`, and `terminal_before_cancel` are distinct labels derived from NT `OrderStatus` / later `OrderStatusReport` evidence. Filled-before-cancel is not success by itself; it must flow into later position reconciliation/flattening.
- Retry attempts, timeout budgets, and backoff values are TOML-owned when cancel policy is enabled.
- Missing mandatory order-risk surfaces, stale source timestamps, unsupported route proof, or exhausted retry budget returns `FailedManualIntervention`-class evidence rather than silently claiming cancellation success.
- Strategy files must remain unable to import or instantiate global kill-switch cancel policy or bypass global cancel routing.

## Option A: No-Submit Cancel Proof Model First (Recommended)

Approach:

- Add a focused `bolt_v3_kill_switch_cancel` module with pure Rust data types for cancel policy, outstanding-order-risk snapshots, cancel route proof, cancel attempt status, and supervisor decisions.
- Add tests that enumerate every required order-risk surface and prove all are treated as outstanding risk until a terminal outcome is proven.
- Add tests that prove cancel planning only works for `KillSwitchState::Cancelling`, binds halt/action/config/policy/source/scope metadata, and rejects stale or incomplete proof.
- Add TOML validation for `[risk.kill_switch.cancel]` policy fields without enabling live cancel execution.
- Keep the Phase 3 action router as the no-submit boundary and do not expose a live cancel handle.

Upside:

- Implements the hard part reviewers already flagged: open-order-only cancellation is unsafe.
- Gives the later live NT adapter an auditable contract and test matrix.
- Preserves the no-live-side-effect property while stacked PRs still wait on merge/CI.

Downside:

- Does not actually cancel live NT orders yet.
- Requires another later slice to bind the proof model to NT strategy ports or a live-node command router.

Blast radius if wrong:

- Medium. The model will shape later live cancellation. Tests must be precise about surfaces, outcomes, and failure modes to avoid false cancellation proof.

## Option B: Live NT Cancel Adapter Now

Approach:

- Add an adapter that calls NT strategy `cancel_order` / `cancel_orders` directly from the global kill-switch runtime.

Upside:

- Moves visible cancellation behavior sooner.

Downside:

- Violates the current stack state: Phase 2 and Phase 3 implementation review gates are not complete because their PRs are still stacked and have no exact-head CI.
- Risks strategy-identity bugs. NT strategy helpers are scoped to the calling strategy, and the design explicitly says a standalone kill-switch strategy is not enough unless source proof shows it can cancel every configured strategy's orders safely.
- Hard to test real inflight/pending/emulated/algorithm-managed races without first having a pure outcome model.

Blast radius if wrong:

- High. A broken adapter could leave live orders active after a halt or cancel the wrong strategy/order identity.

## Option C: Reconciliation First

Approach:

- Skip cancel routing and build the flat-proof reconciler first.

Upside:

- Produces proof-oriented code before side effects.

Downside:

- Reconciliation depends on the same outstanding-order-risk surfaces and outcomes the cancel loop must define. Starting with reconciliation would duplicate or prematurely invent the cancel outcome model.

Blast radius if wrong:

- High. A reconciler without a complete cancel outcome model can falsely prove no outstanding risk.

## Recommendation

Use Option A. Phase 4 should produce a reviewable no-submit cancel-loop proof model and supervisor contract. It should make no production claim beyond complete outstanding-order-risk coverage, fail-closed cancel planning, and typed outcomes for later live NT cancellation.

## Planned File Structure

- Create `src/bolt_v3_kill_switch_cancel.rs`
  - Owns pure cancel-loop policy, risk snapshot, route proof, planned command, attempt outcome, and supervisor decision types.
  - Contains no NT client calls, no strategy calls, and no venue-specific API calls.
- Modify `src/lib.rs`
  - Exports `bolt_v3_kill_switch_cancel`.
- Modify `src/bolt_v3_config.rs`
  - Adds optional `[risk.kill_switch.cancel]` config fields under the existing kill-switch config block.
- Modify `src/bolt_v3_validate.rs`
  - Validates cancel policy only when `[risk.kill_switch]` and `[risk.kill_switch.cancel]` are enabled.
- Modify `scripts/verify_bolt_v3_strategy_policy_fence.py`
  - Rejects strategy imports or direct references to the global cancel-loop module/policy if the implementation introduces names that strategies could misuse.
- Test `tests/bolt_v3_kill_switch_cancel.rs`
  - Unit tests for outstanding-risk coverage, metadata binding, stale proof rejection, route-proof rejection, outcome transitions, retry exhaustion, and no-submit command decisions.
- Test `tests/bolt_v3_kill_switch_config.rs`
  - Config parsing/validation tests for cancel policy bounds.

## Phase 4 TDD Sequence

1. RED: add `tests/bolt_v3_kill_switch_cancel.rs` proving all required order-risk surfaces are represented and treated as outstanding until terminal proof exists: open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal.
2. GREEN: add `BoltV3KillSwitchOutstandingOrderRiskSurface`, `BoltV3KillSwitchCancelCandidate`, and `BoltV3KillSwitchCancelSnapshot` with dedupe by client order id plus account/instrument/strategy/surface metadata.
3. RED: test proves cancel planning rejects snapshots that omit any mandatory configured surface when cancel policy says the surface is mandatory.
4. GREEN: add `BoltV3KillSwitchCancelPolicy` with mandatory surface set and a validator that fails closed on missing mandatory surface proof.
5. RED: test proves cancel planning only works for `KillSwitchState::Cancelling`; `Armed`, `Halting`, `Halted`, `Flattening`, `Flat`, and `FailedManualIntervention` reject before planned commands are emitted.
6. GREEN: add `BoltV3KillSwitchCancelSupervisor::plan_cancel` with a state-kind guard and no-submit decision output.
7. RED: test proves every planned cancel command binds halt id, action id, config hash, policy hash, source timestamp, NT account scope, NT instrument scope, NT strategy id, NT client order id, NT order status, and surface.
8. GREEN: thread the metadata from the Phase 3 router-style request into `BoltV3KillSwitchCancelPlan` and command records.
9. RED: test proves stale source timestamps and empty account/instrument/strategy filters reject.
10. GREEN: add freshness and scope validation without hardcoded runtime values; tests construct runtime values explicitly.
11. RED: test proves unsupported or missing route proof returns a fail-closed manual-intervention decision instead of planned cancellation.
12. GREEN: add `BoltV3KillSwitchCancelRouteProof` with route kind `PerStrategyActionPort` and `LiveNodeCommandRouter`, preserving original strategy/client/order identity. Do not implement live adapter calls.
13. RED: test proves NT `OrderStatus` / later `OrderStatusReport` evidence maps `cancel_requested`, `cancel_accepted`, `cancel_rejected`, `pending_cancel`, `expired`, `filled_before_cancel`, and `terminal_before_cancel` labels distinctly and cannot collapse them into success.
14. GREEN: add outcome aggregation over NT `OrderStatus` evidence that reports unresolved risk unless terminal/cancelled proof exists for every candidate.
15. RED: test proves filled-before-cancel stays unresolved for cancel success and produces a `RequiresPositionReconciliation` result for later flatten/reconciliation phases.
16. GREEN: add aggregate decision result variants for `AllTerminal`, `OutstandingRiskRemains`, `RequiresPositionReconciliation`, and `FailedManualIntervention`.
17. RED: test proves retry attempts, timeout, and backoff are policy-owned and retry exhaustion produces `FailedManualIntervention`.
18. GREEN: add retry budget fields and pure retry-decision logic.
19. RED: config test proves enabled `[risk.kill_switch.cancel]` rejects missing/zero retry, timeout, backoff, freshness, and mandatory-surface settings.
20. GREEN: extend `BoltV3KillSwitchConfig`, parsing, and validation for cancel policy settings.
21. RED: source-fence test proves strategies cannot import `bolt_v3_kill_switch_cancel` or call global cancel supervisor APIs directly.
22. GREEN: extend the strategy source fence without weakening existing strategy submit/cancel bypass checks.
23. REFACTOR: keep cancel snapshot/proof types, supervisor planning, retry/outcome aggregation, and config validation separated so the later live NT adapter can be added without changing the pure model.

## Phase 4 Acceptance

- The PR remains a stacked Phase 4 slice and does not claim to close #517.
- No live NT cancel calls, venue-specific cancel calls, flatten submits, final flat proof, loss-governor triggers, or operator reset UI are added.
- Cancel planning is valid only for durable `Cancelling` state.
- Outstanding-order-risk coverage includes open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal surfaces.
- Missing mandatory surface proof fails closed.
- Planned cancel commands bind halt id, action id, config hash, policy hash, source timestamp, NT account scope, NT instrument scope, NT strategy id, NT client order id, NT order status, route proof, and surface.
- Route proof preserves original strategy/client/order identity and does not assume a standalone kill-switch strategy can cancel other strategies' orders.
- Cancel outcomes distinguish requested, accepted, rejected, pending-cancel, expired, terminal-before-cancel, and filled-before-cancel using NT `OrderStatus` / `OrderStatusReport` evidence rather than bespoke venue lifecycle state.
- Filled-before-cancel cannot be counted as cancellation success; it must require later position reconciliation.
- Retry exhaustion or unsupported route proof returns manual-intervention evidence instead of success.
- `[risk.kill_switch.cancel]` values are TOML-owned and validated when enabled.
- Strategy source fences reject strategy-local global cancel supervisor policy and direct cancel bypasses.
- `cargo test --locked --test bolt_v3_kill_switch_cancel` passes.
- `cargo test --locked --test bolt_v3_kill_switch_config` passes for focused cancel-config cases.
- `python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py` passes.
- `python3 scripts/verify_bolt_v3_strategy_policy_fence.py` passes.
- `cargo fmt --check` passes.
- `just clippy` passes so the repo wrapper checks the configured lint surface instead of only the library target.
- `just source-fence` passes.

## Deferred Scope

- Live NT cancel adapter through per-strategy action ports.
- Live-node command-router cancel adapter.
- Durable transition orchestration into and out of `Cancelling`, including replacing the current `Halted -> Flat` reconciliation shortcut with the later `Cancelling -> Flattening -> Flat` proof sequence.
- Flatten routing and forced-reduction order construction.
- Reconciliation proof that all outstanding order risk and positions are clear.
- Loss-governor or manual runtime trigger ingestion.
- Authorized manual reset and restoration to `Armed` / `TradingState::Active`.
- No-submit end-to-end kill drill.
- Tiny-capital live drill.

## External Review Gate

Before implementation, DeepSeek and GLM must both approve this Phase 4 plan or provide actionable findings. If a provider cannot be used for more than two consecutive attempts in this session, it is skipped for this phase per user instruction and the skip is recorded in PR evidence.

After implementation and a green exact PR head, the exact Phase 4 diff receives another DeepSeek and GLM review. Both usable reviewers must approve before the Phase 4 PR is marked ready.
