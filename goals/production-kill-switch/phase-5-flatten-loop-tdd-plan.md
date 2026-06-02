# #517 Phase 5 TDD Plan: Flatten Loop Proof Model

## Current Base

- Issue: https://github.com/seungpyoson/bolt-v2/issues/517
- Branch: `codex/517-kill-switch-phase5-flatten-loop-plan`
- Stacked base: `codex/517-kill-switch-phase4-cancel-loop-plan` at `d5a33e91e26208bba5d77eef0e6b39b5d58889de`
- Upstream Phase 1 PR: https://github.com/seungpyoson/bolt-v2/pull/521
- Upstream Phase 2 PR: https://github.com/seungpyoson/bolt-v2/pull/523
- Upstream Phase 3 PR: https://github.com/seungpyoson/bolt-v2/pull/525
- Upstream Phase 4 PR: https://github.com/seungpyoson/bolt-v2/pull/530
- Scope: Phase 5 planning first. The future implementation slice may add a typed no-submit flatten-loop proof model, TOML-owned flatten policy validation, and forced-reduction order-intent proof records. It must not add live NT submit calls, venue-specific flatten calls, final global flat proof, loss-governor trigger ingestion, operator reset UX, no-submit end-to-end drill behavior, or tiny-capital live-drill behavior.

## Decision

Plan Phase 5 as a flatten-loop proof and forced-reduction admission-contract slice rather than live position flattening. Phase 4 modeled outstanding order cancellation and explicitly routes filled-before-cancel into later position reconciliation. Phase 5 should define the global flatten supervisor's inputs, NT position evidence, route proof, forced-reduction order-intent shape, retry/outcome state, and fail-closed decisions so a later live NT adapter can submit reduce-only exits without changing the safety model.

This phase intentionally separates four concerns:

1. NT-owned position enumeration and residual exposure proof metadata.
2. Forced-reduction order-intent shape and admission proof.
3. Flatten outcome/retry state transitions.
4. The live NT submit adapter, which remains out of scope until the source-proven route and exact-head CI/review gates are available.

## Invariants

- Durable kill-switch state remains the authority. Flatten planning is valid only in `KillSwitchState::Flattening`.
- NT runtime trading state must be `TradingState::Reducing` before any flatten proof decision is valid.
- Flatten candidates are evidence over NT-owned position/cache/report state, not a replacement position lifecycle model.
- Every flatten candidate carries NT identifiers and types where available: `AccountId`, `InstrumentId`, `StrategyId`, `PositionId`, `PositionSide` or `PositionSideSpecified`, and `Quantity`.
- `PositionSide::Long` maps to NT `OrderSide::Sell`; `PositionSide::Short` maps to NT `OrderSide::Buy`; `Flat` and `NoPositionSide` cannot produce a forced-reduction order.
- A forced-reduction order is reduce-only, policy-bound, and tied to a `BoltV3KillSwitchForcedReductionClaim`; ordinary exits remain ordinary exits and continue to obey ordinary caps.
- The existing `bolt_v3_order_intent` NT order template validation/building contract is reused for flatten order shape. Phase 5 must not invent venue-specific order construction.
- Top-level `[risk.kill_switch]` forced-reduction settings own admission policy hash, forced live-order cap, and max notional per forced-reduction order. `[risk.kill_switch.flatten]` owns flatten-specific retry, source freshness, route, and order-template settings.
- Missing position proof, stale source timestamps, unsupported route proof, invalid side, unsupported instrument, invalid quantity, exhausted retry budget, rejected submit evidence, or residual position evidence returns manual-intervention or residual-risk evidence rather than silently claiming flat.
- Partial fills and filled-before-cancel races do not prove flat. They remain unresolved until fresh NT position evidence proves no residual position.
- Phase 5 emits proof and planned forced-reduction commands only. It must not call NT `submit_order`, direct execution engine methods, `close_position`, `close_all_positions`, `flatten_all_positions`, or venue-specific APIs.
- Strategy files must remain unable to import or instantiate global kill-switch flatten policy, global flatten supervisors, or direct flatten bypasses.

## Option A: No-Submit Flatten Proof Model First (Recommended)

Approach:
- Add a focused `bolt_v3_kill_switch_flatten` module with pure Rust data types for flatten policy, position snapshots, route proof, planned forced-reduction command, attempt outcome, retry state, and supervisor decisions.
- Add tests that prove position evidence uses NT account, instrument, strategy, position, side, and quantity types and fails closed on stale, unknown, or flat-only evidence.
- Add tests that prove forced-reduction commands bind halt/action/config/policy/source metadata plus the admission claim and order-intent proof.
- Add TOML validation for `[risk.kill_switch.flatten]` policy fields without enabling live submits.
- Keep the Phase 3 action router as the no-submit boundary and do not expose a live flatten handle.

Upside:
- Implements the hard part before side effects: forced reductions must not be blocked by ordinary caps, but must remain proof-bound and reduce-only.
- Gives the later live NT adapter an auditable contract and test matrix.
- Preserves the no-live-side-effect property while stacked PRs still wait on merge/CI.

Downside:
- Does not actually submit live NT forced-reduction orders yet.
- Requires another later slice to bind the proof model to NT strategy ports or a live-node command router.

Blast radius if wrong:
- Medium. The model will shape later live flattening. Tests must be precise about NT position evidence, side-to-order mapping, reduce-only order shape, and failure modes to avoid false flatten proof.

## Option B: Live NT Flatten Adapter Now

Approach:
- Add an adapter that submits reduce-only exits through NT strategy or runtime command paths in this phase.

Upside:
- Moves visible flatten behavior sooner.

Downside:
- Violates the current stack state: upstream implementation PRs are still stacked and do not have GitHub CI checks because only `main` PRs trigger CI.
- Risks strategy-identity and route-proof bugs before the pure flatten contract exists.
- Hard to test partial-fill, stale-position, unsupported-instrument, thin-book, and retry-exhaustion cases without first having a pure outcome model.

Blast radius if wrong:
- High. A broken adapter could leave live positions active after a halt, submit a non-reducing order, or submit under the wrong strategy/order identity.

## Option C: Final Reconciliation First

Approach:
- Skip forced-reduction planning and build the global flat-proof reconciler first.

Upside:
- Produces proof-oriented code before side effects.

Downside:
- Reconciliation depends on both Phase 4 cancel outcomes and Phase 5 flatten outcomes. Starting with final reconciliation would either duplicate this phase or falsely treat partial fills/residuals as flat.

Blast radius if wrong:
- High. A reconciler without a complete flatten outcome model can falsely prove no position risk.

## Recommendation

Use Option A. Phase 5 should produce a reviewable no-submit flatten-loop proof model, forced-reduction admission contract, and config validation. It should make no production claim beyond complete NT-position evidence handling, fail-closed flatten planning, and typed outcomes for later live NT forced-reduction submission.

## Planned File Structure

- Create `src/bolt_v3_kill_switch_flatten.rs`
  - Owns pure flatten-loop policy, NT position snapshot, route proof, planned forced-reduction command, attempt outcome, retry decision, and supervisor decision types.
  - Contains no NT client calls, no strategy calls, and no venue-specific API calls.
- Modify `src/lib.rs`
  - Exports `bolt_v3_kill_switch_flatten`.
- Modify `src/bolt_v3_config.rs`
  - Adds optional `[risk.kill_switch.flatten]` config fields under the existing kill-switch config block.
- Modify `src/bolt_v3_validate.rs`
  - Validates flatten policy only when `[risk.kill_switch]` and `[risk.kill_switch.flatten]` are enabled.
- Modify `scripts/verify_bolt_v3_strategy_policy_fence.py`
  - Rejects strategy imports or direct references to the global flatten-loop module/policy if the implementation introduces names that strategies could misuse.
- Test `tests/bolt_v3_kill_switch_flatten.rs`
  - Unit tests for NT position evidence, metadata binding, stale proof rejection, side-to-order mapping, forced-reduction admission claim binding, route-proof rejection, outcome transitions, retry exhaustion, and no-submit command decisions.
- Test `tests/bolt_v3_kill_switch_config.rs`
  - Config parsing/validation tests for flatten policy bounds and order-template settings.

## Phase 5 TDD Sequence

1. RED: add `tests/bolt_v3_kill_switch_flatten.rs` proving a flatten snapshot distinguishes open long, open short, flat, and unknown/no-position-side NT position evidence.
2. GREEN: add `BoltV3KillSwitchFlattenCandidate` and `BoltV3KillSwitchFlattenSnapshot` with NT `AccountId`, `InstrumentId`, `StrategyId`, `PositionId`, `PositionSide`, and `Quantity` metadata.
3. RED: test proves flatten planning rejects empty snapshots, stale request timestamps, stale candidate timestamps, missing action id, invalid config hash, and invalid policy hash.
4. GREEN: add `BoltV3KillSwitchFlattenPolicy` with source freshness and metadata validation.
5. RED: test proves flatten planning only works for `KillSwitchState::Flattening`; `Armed`, `Halting`, `Halted`, `Cancelling`, `Flat`, and `FailedManualIntervention` reject before planned commands are emitted.
6. GREEN: add `BoltV3KillSwitchFlattenSupervisor::plan_flatten` with state-kind guard and no-submit decision output.
7. RED: test proves every planned forced-reduction command binds halt id, action id, config hash, policy hash, source timestamp, NT account id, NT instrument id, NT strategy id, NT position id, NT position side, NT quantity, route proof, and forced-reduction claim.
8. GREEN: thread metadata from the action-router-style request into `BoltV3KillSwitchFlattenPlan` and command records.
9. RED: test proves long positions produce sell forced-reduction intent and short positions produce buy forced-reduction intent using `bolt_v3_position_contract::expected_exit_order_side_for_position`.
10. GREEN: add side-to-order mapping that rejects `Flat` and `NoPositionSide`.
11. RED: test proves forced-reduction commands require a `BoltV3KillSwitchForcedReductionClaim` whose halt id, action id, and policy hash match the flatten request.
12. GREEN: add claim validation and propagate the claim into each planned command.
13. RED: admission test proves a planned forced-reduction request can be admitted while ordinary entry/replace risk is blocked and ordinary risk-reducing exits still obey normal exit caps.
14. GREEN: reuse existing `BoltV3SubmitAdmissionState` forced-reduction policy and claim checks without adding a parallel admission path.
15. RED: test proves flatten order shape is reduce-only, not quote-quantity, and validated by the shared NT order-template contract before a command can be planned.
16. GREEN: add flatten order-template proof fields using `NtOrderTemplateConfig` / `NtOrderTemplate` validation from `bolt_v3_order_intent`.
17. RED: test proves unsupported or missing route proof returns fail-closed manual-intervention evidence instead of a planned flatten command.
18. GREEN: add `BoltV3KillSwitchFlattenRouteProof` with route kind `PerStrategyActionPort` and `LiveNodeCommandRouter`; do not implement live adapter calls.
19. RED: test proves flatten outcomes distinguish submit planned, submit accepted, submit rejected, partial fill, residual position remains, flat position observed, stale position proof, unsupported instrument, and thin-book/no-fillability proof.
20. GREEN: add outcome aggregation that reports `AllFlat`, `ResidualPositionRemains`, `OutstandingFlattenSubmit`, or `FailedManualIntervention` without claiming final global reconciliation.
21. RED: test proves retry attempts, timeout, and backoff are policy-owned and retry exhaustion produces `FailedManualIntervention`.
22. GREEN: add retry budget fields and pure retry-decision logic.
23. RED: config test proves enabled `[risk.kill_switch.flatten]` rejects missing or zero retry, timeout, backoff, freshness, route, max position proof age, and invalid order-template settings.
24. GREEN: extend `KillSwitchConfigBlock`, parsing, and validation for flatten policy settings and order-template validation.
25. RED: source-fence test proves strategies cannot import `bolt_v3_kill_switch_flatten` or call global flatten supervisor APIs directly.
26. GREEN: extend the strategy source fence without weakening existing strategy submit/cancel bypass checks.
27. REFACTOR: keep position snapshot/proof types, supervisor planning, retry/outcome aggregation, order-template proof, and config validation separated so the later live NT adapter can be added without changing the pure model.

## Phase 5 Acceptance

- The PR remains a stacked Phase 5 slice and does not claim to close #517.
- No live NT submit calls, venue-specific flatten calls, final global flat proof, loss-governor triggers, operator reset UI, no-submit drill, or tiny-capital live drill are added.
- Flatten planning is valid only for durable `Flattening` state plus NT `TradingState::Reducing` route context.
- Position evidence uses NT account, instrument, strategy, position, side, and quantity types.
- Missing, stale, flat-only, unknown-side, unsupported-instrument, invalid-quantity, or incomplete position proof fails closed.
- Planned forced-reduction commands bind halt id, action id, config hash, policy hash, source timestamp, NT position identity, route proof, order-intent proof, and forced-reduction admission claim.
- Long-to-sell and short-to-buy mapping comes from the shared position contract; `Flat` and `NoPositionSide` cannot produce forced-reduction order intent.
- Flatten order shape reuses shared NT order-template validation and requires reduce-only, non-quote-quantity order intent.
- Ordinary exits remain ordinary exits and remain cap-bound; only `KillSwitchForcedReduction` with matching proof can bypass ordinary caps.
- Partial fills and filled-before-cancel races cannot be counted as flat; residual position proof remains unresolved until fresh NT position evidence proves flat.
- Retry exhaustion, unsupported route proof, stale proof, rejected submit evidence, unsupported instrument, or thin-book/no-fillability proof returns manual-intervention or residual-risk evidence instead of success.
- `[risk.kill_switch.flatten]` values are TOML-owned and validated when enabled.
- Strategy source fences reject strategy-local global flatten supervisor policy and direct flatten bypasses.
- `just test` passes after implementation.
- `just fmt-check` passes after implementation.
- `just clippy` passes after implementation.
- `just source-fence` passes after implementation.

## Deferred Scope

- Live NT forced-reduction submit adapter through per-strategy action ports.
- Live-node command-router flatten adapter.
- Final reconciliation proof that all outstanding order risk and positions are clear.
- Restart/recovery proof for flatten progress.
- Loss-governor or manual runtime trigger ingestion.
- Authorized manual reset and restoration to `Armed` / `TradingState::Active`.
- No-submit end-to-end kill drill.
- Tiny-capital live drill.

## External Review Gate

Before implementation, DeepSeek and GLM must both approve this Phase 5 plan or provide actionable findings. If a provider cannot be used for more than two consecutive attempts in this session, it is skipped for this phase per user instruction and the skip is recorded in PR evidence.

After implementation and a green exact PR head, the exact Phase 5 diff receives another DeepSeek and GLM review. Both usable reviewers must approve before the Phase 5 PR is marked ready.
