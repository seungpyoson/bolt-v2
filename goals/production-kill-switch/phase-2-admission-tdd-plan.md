# #517 Phase 2 TDD Plan: Admission Latch And Forced-Reduction Model

Historical note: this is the original stacked Phase 2 planning record.
PR #738 consolidates the accepted durable/proof-only implementation on current `main`; stacked-PR language below is retained for chronology, not as the current review path.

## Current Base

- Issue: https://github.com/seungpyoson/bolt-v2/issues/517
- Branch: `codex/517-kill-switch-phase2-admission`
- Stacked base: `origin/codex/517-kill-switch-phase1` at `e3cad16e37fbdee6f5101f52cff7ff27c400b137`
- Upstream Phase 1 PR: https://github.com/seungpyoson/bolt-v2/pull/521
- Scope: Phase 2 only. This slice may extend the shared submit-admission boundary and source fences. It must not add live NT cancel, flatten, reconciliation routing, runtime kill triggers, or operator reset UI.

## Decision

Implement the first global admission latch inside `src/bolt_v3_submit_admission.rs`, because that is the existing pre-NT-submit gate used by the strategy submit path. The latch consumes the Phase 1 kill-switch state model and rejects new entry/replace risk whenever the durable kill-switch state is not `Armed`.

## Invariants

- The existing loss governor is not yet wired as a live trigger in this phase.
- `Armed` kill-switch state preserves existing submit-admission behavior.
- `Halting`, `Halted`, `Flat`, and `FailedManualIntervention` block `Entry` and `ReplaceSubmit` before NT submit.
- Ordinary `RiskReducingExit` remains admissible only through the existing normal count/notional/lifecycle checks.
- Kill-switch forced flatten submits use a distinct `KillSwitchForcedReduction` intent and require proof-bound admission metadata before they may bypass normal count/notional caps.
- A forced-reduction request without halt identity, action identity, or configured proof policy must fail closed.
- Plain cancel remains outside submit admission and must not consume submit budget.
- No strategy-local kill policy is introduced.
- All new runtime policy values are TOML-owned when the kill switch is enabled.

## Option A: Admission Boundary First (Recommended)

Approach:
- Extend the submit-admission request/evaluation model with kill-switch admission context.
- Add a kill-switch state provider/latch API that can be updated by later runtime wiring without exposing manual admission bypasses.
- Add a forced-reduction intent and proof-bound policy model, but keep live flatten routing out of scope.
- Extend decision evidence outcomes so every kill-switch rejection is durable before the request returns.
- Add source-fence coverage that rejects strategy-local kill policy and direct venue kill/cancel/flatten bypasses.

Upside:
- Uses the existing single pre-submit boundary.
- Does not create a second submit path.
- Gives later cancel/flatten phases a typed forced-reduction lane.

Downside:
- Does not yet persist a runtime halt trigger into the store or issue cancel/flatten commands.

Blast radius if wrong:
- Medium. This touches submit admission, but the implementation is test-first and preserves existing armed behavior.

## Option B: Live Runtime Latch First

Approach:
- Wire kill-switch state loading into `src/bolt_v3_live_node.rs` before changing submit admission.

Upside:
- Exercises live-node construction earlier.

Downside:
- Risks local runtime state bypassing the shared admission checks.
- Makes external review harder because the slice mixes runtime assembly with policy semantics.

Blast radius if wrong:
- High. A live-node-local latch can diverge from the submit boundary and leave strategy paths inconsistently gated.

## Option C: Forced Flatten Routing First

Approach:
- Add flatten order construction and route it through existing order-intent code before admission latch changes.

Upside:
- Moves toward visible kill action behavior faster.

Downside:
- Can deadlock on normal admission caps unless the forced-reduction model exists first.
- Requires NT cache/portfolio proof that is explicitly later-phase scope.

Blast radius if wrong:
- High. Flattening without a proven global latch and proof-bound admission lane is unsafe.

## Recommendation

Use Option A. Phase 2 should produce a reviewable admission contract that later runtime wiring can consume, while preserving Phase 1's no-live-side-effect property except for the existing normal submit-admission path.

## Phase 2 TDD Sequence

1. RED: submit-admission test proves `Armed` kill-switch context preserves an otherwise-admitted entry.
2. RED: submit-admission test proves `Halting`, `Halted`, `Flat`, and `FailedManualIntervention` reject `Entry` and do not consume count.
3. GREEN: add the minimal kill-switch admission context and rejection outcome.
4. RED: submit-admission test proves `ReplaceSubmit` is rejected while halted even if lifecycle policy enables replace.
5. GREEN: reuse the same kill-switch block for replace intent.
6. RED: submit-admission test proves ordinary `RiskReducingExit` while halted still goes through normal count/notional caps and cannot bypass an exhausted count cap.
7. GREEN: keep ordinary exit on the existing normal evaluation path.
8. RED: submit-admission test proves a `KillSwitchForcedReduction` request without halt/action/proof metadata rejects before normal cap checks.
9. GREEN: add the forced-reduction intent and proof-bound metadata model.
10. RED: submit-admission test proves a valid forced-reduction request while halted can bypass normal count/notional caps but records a distinct admitted outcome/intention.
11. GREEN: add the minimal forced-reduction admission path.
12. RED: config tests prove enabled `[risk.kill_switch]` requires TOML-owned forced-reduction policy settings.
13. GREEN: extend `KillSwitchConfigBlock` and validation.
14. RED: source-fence tests prove strategy-local kill policy and direct venue kill/cancel/flatten bypass tokens are rejected.
15. GREEN: extend the strategy policy fence and wire it through existing `just source-fence`.
16. REFACTOR: consolidate submit-admission fixtures while preserving all existing admission tests.

## Phase 2 Acceptance

- The PR remains a stacked Phase 2 slice and does not claim to close #517.
- No live NT cancel, flatten, trading-state, or reconciliation routing is added.
- No strategy-local kill policy is introduced.
- `Entry` and `ReplaceSubmit` are blocked before NT submit whenever kill-switch state is latched.
- Ordinary `RiskReducingExit` remains subject to normal submit-admission caps.
- `KillSwitchForcedReduction` is a separate proof-bound admission class and cannot proceed without halt/action/proof metadata.
- Forced reduction can bypass normal count/notional caps only after the kill-switch proof model validates.
- Decision evidence records kill-switch blocked and forced-reduction outcomes.
- Config validation owns every new forced-reduction policy value.
- `cargo test --locked --test bolt_v3_submit_admission` passes.
- `cargo test --locked --test bolt_v3_kill_switch --test bolt_v3_kill_switch_store --test bolt_v3_kill_switch_config` passes.
- `cargo fmt --check` passes.
- `cargo clippy --locked --lib -- -D warnings` passes.
- `just source-fence` passes.

## External Review Gate

Before implementation, DeepSeek and GLM must both approve this plan or provide actionable findings. If a provider cannot be used for more than two consecutive attempts in this session, it is skipped for this phase per user instruction.

After implementation and a green exact PR head, the exact diff receives another DeepSeek and GLM review. Both usable reviewers must approve before the Phase 2 PR is marked ready.
