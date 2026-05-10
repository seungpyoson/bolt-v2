# Bolt-v3 Slice 3-10 Completion Audit

Date: 2026-05-11
Branch context: `codex/bolt-v3-reference-actor-registration`
Base chain head: `origin/codex/bolt-v3-chainlink-provider`

This audit checks the requested F3-F10 work against actual artifacts. It is not a production-readiness approval and does not open or merge PRs.

## Objective Restatement

Work F3-F10 sequentially with hard evidence, TDD discipline where implementation is attempted, NT-aligned vocabulary/ownership, no hardcodes, no dual paths, no Python production path, and one narrow branch per selected slice.

## Prompt-To-Artifact Checklist

| Requirement | Evidence | Status |
| --- | --- | --- |
| Use fresh branches/worktrees from origin | Current branch `codex/bolt-v3-reference-actor-registration` was created in `.worktrees/bolt-v3-reference-actor-registration` from pushed origin ref `origin/codex/bolt-v3-chainlink-provider` | met |
| Commit and push each slice | Current reference-actor-registration slice committed as `d1a8ae2` and pushed to `origin/codex/bolt-v3-reference-actor-registration`; prior slices remain pushed separately | met |
| Do not merge without approval | No merge command used; no PR opened from these branches | met |
| F3 ETH/USD reference contract | Root TOML now defines `[reference_streams.eth_usd]`; existing strategy TOML selects it with `parameters.reference_stream_id`; validation rejects missing stream IDs and invalid stream values; strategy registration resolves selected stream to NT context `reference_publish_topic` | verified-local |
| F4 fused-price policy | `tests/bolt_v3_reference_policy.rs` proves v3 root reference streams use the existing fusion algorithm for configured source IDs, source types, weights, freshness windows, disabled inputs, and topic; no disagreement fail-closed policy exists yet | partial |
| F5 reference producer wiring | `tests/bolt_v3_adapter_mapping.rs` proves v3 Chainlink adapter mapping from stream feed metadata; `tests/bolt_v3_reference_producer.rs` proves existing eth stream builds a Chainlink `ReferenceActor` plan from TOML; `tests/bolt_v3_reference_actor_registration.rs` proves selected `ReferenceActor` registration on an idle v3 LiveNode | partial |
| F6 instrument readiness | `codex/bolt-v3-instrument-gate` added verified-local readiness tests; F6c remains blocked on NT `LiveNode::start` private flush/start sequence | met |
| F7 decision-event persistence | Follow-up TDD probes prove registered custom data with `event_facts: Params` preserves explicit JSON null through NT local catalog, and the market-selection/order-boundary event subset writes through the canonical NT catalog handoff | event/handoff subset verified, evaluation/gating incomplete |
| F8 risk/order admission | Tracker records NT RiskEngine exists and bolt-v3 maps risk config, but bolt-owned admission lacks decision event + order-intent path | met as blocker evidence |
| F9 order lifecycle | Tracker records NT/legacy strategy order machinery exists, but bolt-v3 has no accepted v3 run/order path | met as blocker evidence |
| F10 reconciliation/restart | Tracker records NT reconciliation primitives and bolt-v3 config mapping, but no v3 restart proof and Polymarket external-order registration hook is empty | met as blocker evidence |
| No Python production path | All F3-F10 tracker entries reject Python or avoid it; no Python runtime artifact added | met |
| No direct venue bypass | F6c, F9, and F10 tracker entries reject direct provider/venue bypasses | met |
| No hardcoded runtime values added | Runtime stream/topic/source/client/freshness values live in TOML fixtures; Rust only parses, validates, and looks up configured IDs | met locally |
| Verification before claims | Focused reference-actor-registration tests pass locally; `cargo test --tests --no-run` passes; `cargo fmt`, `git diff --check`, and changed-file debt-marker scan are clean | met-local |

## Result

F3-F10 are not production-complete. They are classified with evidence:

- F3: verified locally for logical root reference stream contract and strategy selection.
- F4: partial local proof for v3 stream projection into existing fusion policy; disagreement fail-closed policy remains unaccepted.
- F5: partial local proof for v3 Chainlink adapter mapping, TOML-to-ReferenceActor planning, and selected ReferenceActor registration on an idle LiveNode; observed runtime delivery remains unverified.
- F6a-F6b: verified locally.
- F6c: blocked by NT startup/instrument-load boundary.
- F7: nullable-field encoding unblocked by NT `Params` proof; `market_selection_result`, entry/exit order submission, entry/exit pre-submit rejection, and direct catalog handoff are verified locally; `entry_evaluation`, `exit_evaluation`, and submit-blocking integration remain unverified.
- F8: blocked until F7 and v3 order intent exist.
- F9: blocked until F6c, F7, and F8 unblock.
- F10: blocked until F6c-F9 unblock and Polymarket external-order tracking is proven.

## Residual Acceptance Gates

External reviews remain pending and are intentionally not requested here. CI on these branches has not been used as acceptance evidence. Merge approval remains required.

The next implementation decision should not start F9 or F10. With this reference-actor-registration slice pushed, the next credible slice is observed reference delivery behind an accepted start/run gate, still no orders.
