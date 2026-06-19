# #517 Phase 3 TDD Plan: Runtime State Sync And No-Submit Action Router Skeleton

Historical note: this is the original stacked Phase 3 planning record.
PR #738 consolidates the accepted proof-only implementation on current `main`; stacked-PR language below is retained for chronology, not as the current review path.

## Current Base

- Issue: https://github.com/seungpyoson/bolt-v2/issues/517
- Branch: `codex/517-kill-switch-phase3-runtime-router-plan`
- Stacked base: `codex/517-kill-switch-phase2-admission` at `36b64490a868129cbcf823aeeaf2ccc59245c53b`
- Upstream Phase 1 PR: https://github.com/seungpyoson/bolt-v2/pull/521
- Upstream Phase 2 PR: https://github.com/seungpyoson/bolt-v2/pull/523
- Scope: Phase 3 planning gate. The implementation slice may wire durable kill-switch state into live-node startup admission, add non-executing `Cancelling`/`Flattening` durable states for future NT action phases, and add a no-submit action-router skeleton. It must not add live NT cancel, live NT flatten, final reconciliation proof, runtime loss-governor triggers, or operator reset UX.

## Decision

Plan the next implementation slice around live-node runtime state synchronization, not action execution. Phase 1 created durable state/evidence. Phase 2 created the shared submit-admission latch and proof-bound forced-reduction admission model. Phase 3 should connect those boundaries at runtime startup and define a typed no-submit action router that later cancel and flatten phases can implement without creating a second submit path.

## Invariants

- Durable kill-switch state remains the primary fail-closed authority.
- Submit admission remains the single global pre-submit boundary for exposure-opening actions.
- Runtime startup must load the durable kill-switch state before strategy registration can create submit-capable runtime behavior.
- Disabled kill-switch enforcement must preserve existing startup behavior and must not fail because durable kill-switch evidence is absent.
- Missing, corrupt, unreadable, or unresolved durable kill-switch state fails closed when kill-switch enforcement is enabled.
- Structurally valid but semantically unknown durable state values are treated as corrupt/unresolved and fail closed.
- Any failure while seeding the submit-admission latch from durable state fails closed before strategy registration.
- `Armed` maps to normal startup admission behavior.
- Phase 3 introduces `Cancelling` and `Flattening` as durable, non-executing in-flight states because the current Phase 2 head only defines `Armed`, `Halting`, `Halted`, `Flat`, and `FailedManualIntervention`.
- Phase 3 does not add transition rules into or out of `Cancelling` and `Flattening`; later NT cancel/flatten phases own those transitions.
- `Halting`, `Halted`, `Cancelling`, `Flattening`, `Flat`, and `FailedManualIntervention` sync into the Phase 2 admission latch before any submit path can admit new entry or replace risk.
- Pinned NT `RiskEngine::set_trading_state` is used only if a safe runtime handle is available at this boundary.
- NT remains preferred whenever the pinned API exposes a safe primitive at the live boundary. If safe NT trading-state mutation is not accessible from the live-node boundary, Phase 3 documents the exact source gap and proves local admission blocking instead of inventing a parallel execution system.
- If a safe pinned NT risk-engine handle is available, `Halting`, `Halted`, `Cancelling`, and `Flattening` map to `TradingState::Reducing`; `Flat` and `FailedManualIntervention` map to `TradingState::Halted`; `Armed` does not restore or set `TradingState::Active`.
- `TradingState::Reducing` is never treated as flatten authorization by itself.
- Any future transition back to `TradingState::Active` stays out of scope until manual reset authorization and clean reconciliation proof are implemented.
- The no-submit action router emits typed dry-run action decisions and proof metadata only. It must not submit, cancel, replace, amend, flatten, transfer, or call venue-specific APIs.
- Strategy code must not import, instantiate, or route kill-switch runtime policy.

## Option A: Runtime State Sync Plus Dry-Run Router Skeleton (Recommended)

Approach:
- Add tests that force the live-node startup path to load durable kill-switch state and seed the Phase 2 admission latch before submit-capable runtime behavior exists.
- Add tests for `Cancelling` and `Flattening` durable state serde, state-kind mapping, and fail-closed reset behavior before runtime sync depends on those states.
- Add disabled-mode tests proving absent durable kill-switch state does not block startup when enforcement is disabled.
- Add unknown-state and latch-sync-failure tests proving those failure modes close the startup path before strategy registration.
- Add a narrow runtime state-sync boundary that maps durable kill-switch states to local admission latch state.
- Add a typed no-submit action-router model for future cancel and flatten decisions, but make every action dry-run/proof-only.
- Probe pinned NT source/API accessibility for `RiskEngine::set_trading_state`; use it when the live boundary exposes a safe handle, or record a source-backed gap with tests proving local fail-closed behavior.
- Extend source fences for strategy-local kill-switch runtime/router policy and direct action bypass tokens.

Upside:
- Connects the two implemented slices without introducing live order side effects.
- Gives later cancel and flatten loops a typed routing contract.
- Preserves submit admission as the single exposure boundary.

Downside:
- Does not cancel orders, flatten positions, or prove flat state.

Blast radius if wrong:
- Medium. This touches live-node startup and policy wiring, but the planned tests keep side effects no-submit and fail closed.

## Option B: NT Trading-State Mutation First

Approach:
- Wire `RiskEngine::set_trading_state` first and defer local state sync/router tests.

Upside:
- Exercises the NT integration point earlier if the handle is directly accessible.

Downside:
- Can create false confidence because NT trading state alone does not prove global Bolt admission blocking or forced-reduction authorization.
- Risks choosing `Halted` or `Reducing` semantics before later flatten routing proves what state sequence is safe.

Blast radius if wrong:
- High. A trading-state-only slice can block required flatten orders or leave local submit admission inconsistent.

## Option C: Cancel/Flatten Router Execution First

Approach:
- Add live cancel or flatten action execution through NT APIs now.

Upside:
- Moves toward visible kill action behavior.

Downside:
- Mixes routing, ordering, event races, reconciliation, and forced-reduction proof in one slice.
- Violates the current review boundary because Phase 2 exact-head implementation review is not complete and PR #523 is still stacked.

Blast radius if wrong:
- High. Live order actions without a reviewed runtime state-sync contract can leave unresolved order risk or block flattening.

## Recommendation

Use Option A. Phase 3 should be a reviewable runtime-connection plan and, after approval, a small TDD implementation slice. It should make no production claim beyond local fail-closed startup sync and dry-run action routing.

## Phase 3 TDD Sequence

1. RED: durable state-machine tests prove the current Phase 2 state enum lacks non-executing `Cancelling` and `Flattening` variants required by the design.
2. GREEN: add `Cancelling` and `Flattening` durable states, state-kind mapping, serde round trips, and tests proving reset/transition attempts involving these states remain fail-closed until later action phases define those transitions.
3. RED: disabled-mode live-node startup test proves missing, unreadable, corrupt, or unknown durable kill-switch state does not block startup when kill-switch enforcement is disabled.
4. RED: startup-ordering test proves enabled kill-switch enforcement loads durable state and seeds the submit-admission latch before strategy registration can create submit-capable runtime behavior.
5. RED: live-node startup test proves enabled kill-switch enforcement fails closed when durable state evidence is missing or unreadable.
6. RED: live-node startup test proves corrupt durable state, semantically unknown state values, and unresolved evidence fail closed and seed submit admission with a latched state before any entry or replace request can pass.
7. RED: latch-sync-failure test proves an error while seeding submit admission fails closed before strategy registration.
8. GREEN: add the minimal durable-state load and admission-latch sync boundary with an injectable/testable failure path.
9. RED: startup/admission test proves `Armed` durable state preserves existing submit-admission behavior.
10. RED: startup/admission test proves `Halting`, `Halted`, `Cancelling`, `Flattening`, `Flat`, and `FailedManualIntervention` block `Entry` and `ReplaceSubmit` through the Phase 2 latch.
11. GREEN: map every durable kill-switch state into the local admission context exhaustively.
12. RED: NT source/API probe test proves whether the pinned live boundary can safely call `RiskEngine::set_trading_state`.
13. GREEN: if accessible, add the minimal trading-state sync adapter and map `Halting`, `Halted`, `Cancelling`, and `Flattening` to `TradingState::Reducing`, `Flat` and `FailedManualIntervention` to `TradingState::Halted`, and `Armed` to no NT mutation; if not accessible, add a source-gap artifact enforced by a machine-checkable test and keep local admission blocking as the enforced behavior.
14. RED: test proves no Phase 3 path restores `TradingState::Active`.
15. RED: test proves `TradingState::Reducing` alone cannot authorize flatten-equivalent action output or bypass forced-reduction proof metadata.
16. GREEN: keep reactivation impossible until the later manual-reset and reconciliation slices, and keep `Reducing` as a guardrail rather than an authorization source.
17. RED: add no-submit action-router tests for dry-run `CancelOutstandingRisk` and `FlattenPositions` action classes, each bound to halt id, action id, config hash, source timestamp, and scope filters.
18. GREEN: add typed dry-run action requests and proof metadata without executing NT cancel/flatten calls.
19. RED: router tests prove entry, replace, live submit, live cancel, live flatten, and venue-specific calls are rejected as Phase 3 action outputs.
20. GREEN: constrain router outputs to proof-only dry-run actions.
21. RED: source-fence tests prove strategies cannot import or instantiate kill-switch runtime/router policy and cannot reference direct kill/cancel/flatten bypass calls.
22. GREEN: extend existing source fences without weakening current strategy submit-policy checks.
23. REFACTOR: keep runtime sync, admission sync, and router model separate enough that later cancel and flatten phases can implement execution without changing submit-admission semantics.

## Phase 3 Acceptance

- The PR remains a stacked Phase 3 slice and does not claim to close #517.
- No live NT cancel, flatten, reconciliation, loss-governor trigger, or operator reset UI is added.
- Disabled kill-switch enforcement preserves existing startup behavior when durable kill-switch state is absent or invalid.
- Enabled kill-switch runtime startup fails closed on missing, corrupt, unreadable, or unresolved durable state.
- Enabled kill-switch runtime startup fails closed on semantically unknown persisted state values.
- Startup ordering proves durable-state load and admission-latch sync happen before strategy registration or submit-capable runtime behavior.
- Admission-latch sync failure is an explicit typed failure path that closes startup before strategy registration.
- Durable non-`Armed` kill-switch state seeds the Phase 2 admission latch before entry/replace admission can pass.
- `Cancelling` and `Flattening` are added only as durable non-executing state-machine states; no NT cancel or flatten execution is introduced in Phase 3.
- Transitions into and out of `Cancelling` and `Flattening` remain deferred to later NT cancel/flatten phases.
- `Armed` durable state preserves current startup and submit-admission behavior.
- The chosen NT trading-state behavior is source-backed: either a safe `RiskEngine::set_trading_state` handle is used, or an explicit source-gap artifact explains why local admission blocking is the only enforced behavior in this slice.
- Any `RiskEngine::set_trading_state` source-gap artifact is enforced by a test, not only by prose documentation.
- If a safe NT handle is used, the mapping is exact: `Halting`/`Halted`/`Cancelling`/`Flattening` -> `TradingState::Reducing`, `Flat`/`FailedManualIntervention` -> `TradingState::Halted`, and `Armed` -> no `TradingState::Active` mutation.
- Phase 3 never restores `TradingState::Active`.
- `TradingState::Reducing` cannot authorize flatten-equivalent router output or bypass forced-reduction metadata.
- The no-submit action router produces only dry-run/proof metadata and cannot execute order lifecycle actions.
- Router metadata binds halt id, action id, config hash, source timestamp, account/instrument scope, and action class.
- Source fences reject strategy-local kill-switch runtime/router policy and direct venue kill/cancel/flatten bypasses.
- `cargo test --locked --test bolt_v3_kill_switch --test bolt_v3_kill_switch_store --test bolt_v3_kill_switch_config` passes.
- `cargo test --locked --test bolt_v3_submit_admission` passes for focused admission-sync cases.
- Focused live-node/runtime-sync tests pass under the exact test target introduced by the first RED implementation commit.
- `cargo fmt --check` passes.
- `just clippy` passes so the repo wrapper checks the configured lint surface instead of only the library target.
- `just source-fence` passes.

## Deferred Scope

- Live loss-governor or manual runtime trigger ingestion.
- Live NT cancel routing.
- Live NT flatten routing.
- Transition rules into or out of `Cancelling` and `Flattening`.
- Full outstanding-order reconciliation across open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal risk.
- Final `Flat` proof.
- Authorized manual reset and any restoration to `Armed` or `TradingState::Active`.
- No-submit end-to-end kill drill.
- Tiny-capital live drill.

## External Review Gate

Before implementation, DeepSeek and GLM must both approve this plan or provide actionable findings. If a provider cannot be used for more than two consecutive attempts in this session, it is skipped for this phase per user instruction. Any provider skip must be recorded in the issue or PR evidence so the audit trail survives outside this plan file.

After implementation and a green exact PR head, the exact diff receives another DeepSeek and GLM review. Both usable reviewers must approve before the Phase 3 PR is marked ready.
