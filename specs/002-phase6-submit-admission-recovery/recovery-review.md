# Recovery Review: Phase 6 Submit Admission From Stale PR #317

## Status

Draft after internal review. Must be completed, committed, and pushed before recovery-strategy review and before any Phase 6 implementation.

## Current Baseline

- Current source of truth: `origin/main`
- Current main SHA: `461e80985b94bc527f53782f17b44c16e35a8555`
- Current Phase 3-5 source: PR #322 merged into `main`
- Current local checkout while drafting this memo: `main` at `2787c3d2bee23da66d50574ea8225c6898fab8bc`, behind `origin/main` by 2 commits

Local-state constraint:
- Do not implement from the current local checkout until it is refreshed or a fresh worktree/branch is created from `origin/main`.
- Do not request recovery-strategy review while these planning artifacts are uncommitted or unpushed.

Current architecture facts to preserve:
- One exported strategy-registration path: `src/bolt_v3_strategy_registration.rs` exposes `register_bolt_v3_strategies_on_node_with_bindings` as the registration surface.
- Mandatory decision-evidence writer: `src/bolt_v3_strategy_registration.rs` constructs `JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded)` and passes `decision_evidence.clone()` through `StrategyRegistrationContext`.
- Runtime capture remains wired inside the live-node runner: `src/bolt_v3_live_node.rs` orders live canary gate, `wire_bolt_v3_runtime_capture`, then `node.run()` under capture failure handling.
- Current `StrategyBuildContext` already requires decision evidence and has no submit admission field yet.
- Current `BoltV3LiveCanaryGateReport` carries `max_live_order_count`, `max_notional_per_order`, and `root_max_notional_per_order`; gate validation rejects max notional above root risk.
- Stale branches are reference-only after merge.

## Stale PR Context

- Stale PR: #317
- Title: `Bolt-v3 Phase 6: enforce submit admission`
- Stale head SHA: `d365bbbb78190653f67163fa61aecc4ad8d0d476`
- Stale base branch: `011-bolt-v3-phase6-submit-admission-plan`
- Stale base SHA: `9672714b71d3bbdf044cf73872b2a5eba3635050`
- Merge-base with current `main`: `ff8e43c33040007ad0fcfc2ce5e9353d0c77cedc`

Drift evidence:
- PR-local diff: 19 files, +709/-88.
- Current-main-to-stale-head diff: 43 files, +4537/-876, broad Phase 3-5 churn.
- Conclusion: not mergeable or broadly cherry-pickable.

Evidence commands already run:
- `git rev-parse origin/main origin/012-bolt-v3-phase6-submit-admission origin/011-bolt-v3-phase6-submit-admission-plan`
- `git merge-base origin/main origin/012-bolt-v3-phase6-submit-admission`
- `git diff --shortstat origin/main..origin/012-bolt-v3-phase6-submit-admission`
- `gh pr view 317 --json number,title,state,isDraft,baseRefName,baseRefOid,headRefName,headRefOid,changedFiles,additions,deletions,url`

## Keep / Rewrite / Reject Map

| Item | Source | Classification | Reason | Current-main constraint |
|------|--------|----------------|--------|-------------------------|
| Submit admission state concept | `src/bolt_v3_submit_admission.rs` | Keep concept, rewrite as needed | Phase 6 needs armed gate report, count cap, notional cap, fail-closed admission | Must integrate with current main, not stale registration shapes |
| Missing-report, double-arm, count-cap, notional-cap tests | `tests/bolt_v3_submit_admission.rs` | Keep concept, rewrite tests | These are real Phase 6 behaviors | Tests must target current APIs |
| Unarmed-until-gate and arm-once behavior | stale admission/live-node diff | Keep concept, rewrite code | Submit admission must not allow live submit before live canary gate passes | Arm must happen after gate report, before any submit path is reachable |
| Evidence -> admission -> submit order | strategy wrapper diff | Keep concept, rewrite code | Correct safety order | Must preserve current decision-evidence writer and current order-side behavior |
| Admission count consumed before NT submit | stale `admit()` call before `self.submit_order` | Rewrite with explicit contract | Fail-closed behavior may consume canary budget even if NT submit returns an error | Must be documented and tested, or redesigned before implementation |
| Root risk cap handling | stale admission checks `max_notional_per_order` only | Rewrite with explicit decision | Current gate report already validates live cap <= root risk; submit path may rely on that or re-check it | Reviewers must confirm no root-risk bypass exists |
| Order notional computation | stale `order_notional(price, quantity)` | Rewrite | Valid need, but must avoid float drift and hidden hardcodes | Compute from actual order price/quantity in a deterministic decimal form |
| Add admission to strategy context | `src/strategies/registry.rs` | Rewrite | Valid concept, stale signature | Current `StrategyBuildContext::new` already requires decision evidence |
| Add admission to registration context | `src/bolt_v3_strategy_registration.rs` | Rewrite | Valid concept, stale surrounding file | Must preserve single exported registration path; do not restore deleted helpers |
| Arm admission during run | `src/bolt_v3_live_node.rs` | Rewrite | Valid concept | Must preserve current runtime capture ordering |
| `BoltV3BuiltLiveNode` wrapper | stale live-node diff | Rewrite only if still smallest design | May be useful to carry admission state with node | Must not create alternate build/run path |
| Archetype wiring | `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` | Rewrite | Valid admission pass-through | Must not recreate decision-evidence writer in archetype |
| `pub mod bolt_v3_submit_admission` | `src/lib.rs` | Keep if module added | Needed if module used by tests/other modules | No extra exports |
| Stale Phase 3-5 file churn | broad stale diff | Reject | Already superseded by PR #322 | Do not port |
| Public generic registration helpers | stale registration file | Reject | Removed to avoid dual paths | Do not restore |
| Stale live-node run code without capture | stale live-node file | Reject | Would regress PR #307 behavior | Preserve capture failure handling |
| Chainlink/provider/reference-role additions | stale diff | Reject for Phase 6 | Out of scope | No Phase 3-5 or provider churn |
| `.gitignore`, fixture, broad docs churn | stale diff | Reject unless separately justified | Not needed for Phase 6 | Keep diff narrow |

## Allowed Future Touch Surface

Allowed if justified by tests:
- `src/bolt_v3_submit_admission.rs`
- `src/lib.rs`
- `src/strategies/registry.rs`
- `src/strategies/eth_chainlink_taker.rs`
- `src/bolt_v3_strategy_registration.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- `tests/bolt_v3_submit_admission.rs`
- `tests/bolt_v3_strategy_registration.rs`
- `tests/bolt_v3_decision_evidence.rs`
- `tests/bolt_v3_controlled_connect.rs`
- `tests/live_node_run.rs`
- runtime-literal audit only if verifier requires exact diagnostic classification

Disallowed unless separately approved:
- stale Phase 3-5 implementation files
- Phase 7 readiness code
- Phase 8 live canary execution code
- provider/market-family expansion
- broad fixture churn
- unrelated docs or `.gitignore` changes

## Stop Conditions

Stop before implementation if:
- current `main` SHA changes and recovery facts are not refreshed
- local checkout is still behind `origin/main` when implementation is about to start
- recovery-strategy review is requested while planning artifacts are uncommitted or unpushed
- recovery-strategy reviewers disagree that #317 is reference-only
- a valid Phase 6 item lacks classification
- proposed implementation needs files outside allowed touch surface
- plan would restore removed helper paths or drop runtime capture
- admission count consumption on NT-submit failure is not explicitly accepted or redesigned
- root risk cap handling is not explicitly accepted or redesigned

## Review Questions

1. Is #317 correctly treated as reference-only?
2. Does this map keep all valid Phase 6 concepts?
3. Does this map reject all stale Phase 3-5 churn?
4. Is any kept item too broad?
5. Is any rejected item actually needed for Phase 6?
6. Is the allowed touch surface narrow enough?
7. Does future ordering preserve evidence -> admission -> submit?
8. Does future live-node ordering preserve gate -> arm admission -> runtime capture -> NT run?
9. Is fail-closed canary budget consumption before NT submit acceptable?
10. Is relying on live-canary gate validation for root risk cap acceptable, or must submit admission re-check it?
11. What must change before implementation starts?
