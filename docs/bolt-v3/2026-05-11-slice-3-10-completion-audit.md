# Bolt-v3 Slice 3-10 Completion Audit

Date: 2026-05-11
Branch context: `codex/bolt-v3-reference-contract`
Base chain head: `origin/codex/bolt-v3-order-decision-events`

This audit checks the requested F3-F10 work against actual artifacts. It is not a production-readiness approval and does not open or merge PRs.

## Objective Restatement

Work F3-F10 sequentially with hard evidence, TDD discipline where implementation is attempted, NT-aligned vocabulary/ownership, no hardcodes, no dual paths, no Python production path, and one narrow branch per selected slice.

## Prompt-To-Artifact Checklist

| Requirement | Evidence | Status |
| --- | --- | --- |
| Use fresh branches/worktrees from origin | Current branch `codex/bolt-v3-reference-contract` was created in `.worktrees/bolt-v3-reference-contract` from pushed origin ref `origin/codex/bolt-v3-order-decision-events` | met |
| Commit and push each slice | Current reference-contract slice is still local until commit/push; prior slices remain pushed separately | pending |
| Do not merge without approval | No merge command used; no PR opened from these branches | met |
| F3 ETH/USD reference contract | Root TOML now defines `[reference_streams.eth_usd]`; existing strategy TOML selects it with `parameters.reference_stream_id`; validation rejects missing stream IDs and invalid stream values; strategy registration resolves selected stream to NT context `reference_publish_topic` | verified-local |
| F4 fused-price policy | Tracker marks F4 blocked; no accepted source/weight/freshness/disagreement contract exists | met as blocker evidence |
| F5 reference producer wiring | Tracker marks F5 blocked; existing producer is legacy `Config.reference` / `ReferenceActor`, not v3 TOML | met as blocker evidence |
| F6 instrument readiness | `codex/bolt-v3-instrument-gate` added verified-local readiness tests; F6c remains blocked on NT `LiveNode::start` private flush/start sequence | met |
| F7 decision-event persistence | Follow-up TDD probes prove registered custom data with `event_facts: Params` preserves explicit JSON null through NT local catalog, and the market-selection/order-boundary event subset writes through the canonical NT catalog handoff | event/handoff subset verified, evaluation/gating incomplete |
| F8 risk/order admission | Tracker records NT RiskEngine exists and bolt-v3 maps risk config, but bolt-owned admission lacks decision event + order-intent path | met as blocker evidence |
| F9 order lifecycle | Tracker records NT/legacy strategy order machinery exists, but bolt-v3 has no accepted v3 run/order path | met as blocker evidence |
| F10 reconciliation/restart | Tracker records NT reconciliation primitives and bolt-v3 config mapping, but no v3 restart proof and Polymarket external-order registration hook is empty | met as blocker evidence |
| No Python production path | All F3-F10 tracker entries reject Python or avoid it; no Python runtime artifact added | met |
| No direct venue bypass | F6c, F9, and F10 tracker entries reject direct provider/venue bypasses | met |
| No hardcoded runtime values added | Runtime stream/topic/source/freshness values live in TOML fixtures; Rust only parses, validates, and looks up configured IDs | met locally |
| Verification before claims | Focused reference-contract tests pass locally; full branch verification still pending before commit/push | pending |

## Result

F3-F10 are not production-complete. They are classified with evidence:

- F3: verified locally for logical root reference stream contract and strategy selection.
- F4-F5: blocked by missing fused-price policy and v3 producer wiring.
- F6a-F6b: verified locally.
- F6c: blocked by NT startup/instrument-load boundary.
- F7: nullable-field encoding unblocked by NT `Params` proof; `market_selection_result`, entry/exit order submission, entry/exit pre-submit rejection, and direct catalog handoff are verified locally; `entry_evaluation`, `exit_evaluation`, and submit-blocking integration remain unverified.
- F8: blocked until F7 and v3 order intent exist.
- F9: blocked until F6c, F7, and F8 unblock.
- F10: blocked until F6c-F9 unblock and Polymarket external-order tracking is proven.

## Residual Acceptance Gates

External reviews remain pending and are intentionally not requested here. CI on these branches has not been used as acceptance evidence. Merge approval remains required.

The next implementation decision should not start F9 or F10. After this reference-contract slice is committed and pushed, the next credible slice is F4 fused-price policy or F5 reference producer wiring, before F7 evaluation events can honestly include reference facts.
