# #517 Phase 1 TDD Plan: Pure Kill-Switch State, Config, And Evidence

Historical note: this is the original stacked Phase 1 planning record.
PR #738 consolidates the accepted durable/proof-only implementation on current `main`; stacked-PR language below is retained for chronology, not as the current review path.

## Current Base

- Issue: https://github.com/seungpyoson/bolt-v2/issues/517
- Branch: `codex/517-kill-switch-phase1`
- Base: `origin/main` at `2938bc6f`
- PR #480 dependency: satisfied by merge commit `92ef8e7d`
- Scope: Phase 1 only, with no NT side effects and no live submit/cancel/flatten routing.

## Decision

Implement the kill switch as a pure, test-first core before wiring any live runtime path.

## Invariant

Once a halt is triggered, restart, missing/corrupt/unresolved evidence, stale mandatory proof, or unauthorized reset must never silently re-enable new risk admission.

## Option A: Pure Core First (Recommended)

Approach:
- Add `src/bolt_v3_kill_switch.rs` for the state/event/reconciliation/reset model.
- Add `src/bolt_v3_kill_switch_store.rs` for durable JSON evidence read/write semantics.
- Extend `[risk.kill_switch]` config parsing and validation after the pure model tests are green.
- Export modules from `src/lib.rs`.
- Keep all NT cache/order/trading-state integration out of this phase.

Upside:
- Small blast radius and deterministic tests.
- Gives Phase 2 admission latch a stable API.
- Makes fail-closed restart semantics reviewable before live wiring exists.

Downside:
- Does not yet cancel, flatten, or block the existing submit path.

Blast radius if wrong:
- Low at runtime because Phase 1 is not wired into live execution, but high for future phases if the state model is too permissive.

## Option B: Admission Latch First

Approach:
- Extend `src/bolt_v3_submit_admission.rs` before adding the durable state store.

Upside:
- Produces visible submit-blocking behavior earlier.

Downside:
- Risks encoding halt semantics in the admission path before durable evidence, reset authorization, and reconciliation states are specified.

Blast radius if wrong:
- Medium. A local latch could diverge from durable restart state and create a false sense of production readiness.

## Option C: No-Submit Runtime Skeleton First

Approach:
- Add runtime command/action skeletons before implementing the pure state machine.

Upside:
- Forces early interaction with the live-node boundary.

Downside:
- Wider dependency surface and harder TDD loop.
- More likely to touch PR #480-owned live wiring before state/config/evidence invariants are pinned down.

Blast radius if wrong:
- High. Premature live-node wiring can make later safety fixes invasive.

## Recommendation

Use Option A. Phase 1 should produce only pure state, durable evidence, config parsing/validation, and tests. Phase 2 can then consume the Phase 1 API to block new risk globally.

## Phase 1 TDD Sequence

1. RED: state transition test proves a loss-governor trigger moves `Armed` to `Halting` and requires durable evidence before any later state can be treated as recoverable.
2. GREEN: add the minimal state/event/transition API.
3. RED: illegal transition tests prove `Flat -> Armed` fails without authorized reset evidence and fresh clean proof.
4. GREEN: add reset authorization/proof gates.
5. RED: reconciliation tests prove missing, stale, or contradictory mandatory proof cannot produce `Flat`.
6. GREEN: add reconciliation proof model.
7. RED: durable store tests prove missing, corrupt, unresolved, or failed write state loads as fail-closed.
8. GREEN: add atomic JSON evidence store with private file mode, file sync, rename, and parent directory sync.
9. RED: config tests prove `[risk.kill_switch]` rejects missing/invalid retry, timeout, path, account, instrument, proof freshness, and reset settings when enabled.
10. GREEN: add config structs and validation.
11. REFACTOR: collapse duplicated state-test fixtures while keeping all tests green.

## Phase 1 Acceptance

- No live NT submit, cancel, flatten, or trading-state code is wired.
- No strategy-local kill policy is introduced.
- All runtime values are TOML-owned when the kill switch is enabled.
- Startup recovery treats missing/corrupt/unresolved durable halt evidence as fail-closed.
- Manual reset requires authorization identity, evidence hash/path, and fresh clean reconciliation proof.
- `cargo test --locked --lib bolt_v3_kill_switch` passes.
- `cargo test --locked --lib bolt_v3_kill_switch_store` passes.
- Targeted config validation tests pass.
- `cargo fmt --check` passes.

## External Review Gate

Before implementation, DeepSeek and GLM must both approve this plan or provide actionable findings. If a provider cannot be used for more than two consecutive attempts in this session, it is skipped for this phase per user instruction. After Phase 1 implementation, the exact diff receives another DeepSeek and GLM review and both usable reviewers must approve before Phase 2 begins.
