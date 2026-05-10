# Bolt-v3 Slice 3-10 Completion Audit

Date: 2026-05-11
Branch context: `codex/bolt-v3-strategy-order-intent-wiring`
Base chain head: `origin/codex/bolt-v3-release-identity`

This audit checks the requested F3-F10 work against actual artifacts. It is not a production-readiness approval and does not open or merge PRs.

## Objective Restatement

Work F3-F10 sequentially with hard evidence, TDD discipline where implementation is attempted, NT-aligned vocabulary/ownership, no hardcodes, no dual paths, no Python production path, and one narrow branch per selected slice.

## Prompt-To-Artifact Checklist

| Requirement | Evidence | Status |
| --- | --- | --- |
| Use fresh branches/worktrees from origin | Current branch `codex/bolt-v3-strategy-order-intent-wiring` was created in `.worktrees/bolt-v3-strategy-order-intent-wiring` from pushed origin ref `origin/codex/bolt-v3-release-identity` | met |
| Commit and push each slice | Current order-intent wiring slice is committed on this branch and pushed to origin; prior slices remain pushed separately | met |
| Do not merge without approval | No merge command used; no PR opened from these branches | met |
| F3 ETH/USD reference contract | Root TOML now defines `[reference_streams.eth_usd]`; existing strategy TOML selects it with `parameters.reference_stream_id`; validation rejects missing stream IDs and invalid stream values; strategy registration resolves selected stream to NT context `reference_publish_topic` | verified-local |
| F4 fused-price policy | `tests/bolt_v3_reference_policy.rs` proves v3 root reference streams use the existing fusion algorithm for configured source IDs, source types, weights, freshness windows, disabled inputs, and topic; no disagreement fail-closed policy exists yet | partial |
| F5 reference producer wiring | `tests/bolt_v3_adapter_mapping.rs` proves v3 Chainlink adapter mapping from stream feed metadata; `tests/bolt_v3_reference_producer.rs` proves existing eth stream builds a Chainlink `ReferenceActor` plan from TOML; `tests/bolt_v3_reference_actor_registration.rs` proves selected `ReferenceActor` registration on an idle v3 LiveNode; `tests/bolt_v3_reference_delivery.rs` proves mock-only NT start delivery to `ReferenceSnapshot` | partial |
| F6 instrument readiness | `codex/bolt-v3-instrument-gate` added verified-local readiness tests; this branch adds `src/bolt_v3_start_readiness.rs` and `tests/bolt_v3_start_readiness_gate.rs` as the canonical pre-start report surface over the instrument gate; F6c remains blocked on NT `LiveNode::start` private flush/start sequence | met-local |
| F7 decision-event persistence | Follow-up TDD probes prove registered custom data with `event_facts: Params` preserves explicit JSON null through NT local catalog; `tests/bolt_v3_decision_event_handoff.rs` proves market-selection, entry/exit evaluation, entry/exit order submission, and entry/exit pre-submit rejection events write through the canonical NT catalog handoff; `tests/bolt_v3_order_intent_gate.rs` proves entry/exit submit closures are blocked when order-submission event handoff fails; `tests/bolt_v3_decision_event_context.rs` proves common fields derive from loaded v3 TOML plus supplied release identity; `tests/bolt_v3_release_identity.rs` proves release identity loads from configured manifest path with config-hash and NT-pin verification; `tests/eth_chainlink_taker_runtime.rs` proves existing-strategy entry and exit order-submission events are written before NT submit and failed entry event write blocks submit | order-submission wiring verified; full strategy market-selection/evaluation/pre-submit-rejection emission remains incomplete |
| F8 risk/order admission | Tracker records NT RiskEngine exists and bolt-v3 maps risk config, but bolt-owned admission lacks decision event + order-intent path | met as blocker evidence |
| F9 order lifecycle | Tracker records NT/legacy strategy order machinery exists, but bolt-v3 has no accepted v3 run/order path | met as blocker evidence |
| F10 reconciliation/restart | Tracker records NT reconciliation primitives and bolt-v3 config mapping, but no v3 restart proof and Polymarket external-order registration hook is empty | met as blocker evidence |
| No Python production path | All F3-F10 tracker entries reject Python or avoid it; no Python runtime artifact added | met |
| No direct venue bypass | F6c, F9, and F10 tracker entries reject direct provider/venue bypasses | met |
| No hardcoded runtime values added | Production path derives stream/topic/source/client/freshness values from TOML and release identity from the configured manifest path; direct runtime tests use explicit test fixture context only | met locally |
| Verification before claims | `cargo test --test bolt_v3_release_identity -- --nocapture`, `cargo test --test bolt_v3_decision_event_context -- --nocapture`, `cargo test --test bolt_v3_order_intent_gate -- --nocapture`, `cargo test --test bolt_v3_decision_event_handoff -- --nocapture`, `cargo test --test config_parsing -- --nocapture`, `cargo test --test bolt_v3_strategy_registration -- --nocapture`, `cargo test --test bolt_v3_adapter_mapping -- --nocapture`, `cargo test --test config_schema -- --nocapture`, `cargo test --tests --no-run`, `cargo fmt`, `git diff --check`, and changed-file marker scan pass locally | met-local |

## Result

F3-F10 are not production-complete. They are classified with evidence:

- F3: verified locally for logical root reference stream contract and strategy selection.
- F4: partial local proof for v3 stream projection into existing fusion policy; disagreement fail-closed policy remains unaccepted.
- F5: partial local proof for v3 Chainlink adapter mapping, TOML-to-ReferenceActor planning, selected ReferenceActor registration on an idle LiveNode, and mock-only NT start delivery to `ReferenceSnapshot`; real-provider runtime delivery remains unverified.
- F6a-F6b: verified locally, including canonical pre-start report composition over the instrument gate.
- F6c: blocked by NT startup/instrument-load boundary; this branch does not call `LiveNode::start`.
- F7: nullable-field encoding unblocked by NT `Params` proof; `market_selection_result`, `entry_evaluation`, `exit_evaluation`, entry/exit order submission, entry/exit pre-submit rejection, direct catalog handoff, order-submission gate helper, common event-field context, release-identity manifest loading, and actual existing-strategy entry/exit order-submission wiring are verified locally. Actual strategy market-selection/evaluation/pre-submit-rejection emission remains unverified.
- F8: blocked until F7 and v3 order intent exist.
- F9: blocked until F6c, F7, and F8 unblock.
- F10: blocked until F6c-F9 unblock and Polymarket external-order tracking is proven.

## Residual Acceptance Gates

External reviews remain pending and are intentionally not requested here. CI on these branches has not been used as acceptance evidence. Merge approval remains required.

The next implementation decision should not start F9 or F10. With this order-intent wiring slice complete, the next credible F7 slice is actual strategy market-selection, evaluation, and pre-submit-rejection event emission through the same decision-event context. Still no production start/run wrapper and no live orders.
