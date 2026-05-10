# Bolt-v3 Slice 3-10 Completion Audit

Date: 2026-05-11
Branch context: `codex/bolt-v3-decision-event-contract`
Base chain head: `origin/codex/bolt-v3-reconciliation`

This audit checks the requested F3-F10 work against actual artifacts. It is not a production-readiness approval and does not open or merge PRs.

## Objective Restatement

Work F3-F10 sequentially with hard evidence, TDD discipline where implementation is attempted, NT-aligned vocabulary/ownership, no hardcodes, no dual paths, no Python production path, and one narrow branch per selected slice.

## Prompt-To-Artifact Checklist

| Requirement | Evidence | Status |
| --- | --- | --- |
| Use fresh branches/worktrees from origin | `codex/bolt-v3-reference-facts`, `codex/bolt-v3-decision-events`, `codex/bolt-v3-risk-gate`, `codex/bolt-v3-order-lifecycle`, `codex/bolt-v3-reconciliation` were created as stacked branches from pushed origin refs | met |
| Commit and push each slice | Branches pushed at `d68dd9e`, `efa289c`, `9fd384f`, `224caa7`, `42e4ba1` | met |
| Do not merge without approval | No merge command used; no PR opened from these branches | met |
| F3 ETH/USD reference contract | Tracker marks F3 blocked; current eth tracer only has `parameters.reference_publish_topic`, no v3 `[reference_data]` contract | met as blocker evidence |
| F4 fused-price policy | Tracker marks F4 blocked; no accepted source/weight/freshness/disagreement contract exists | met as blocker evidence |
| F5 reference producer wiring | Tracker marks F5 blocked; existing producer is legacy `Config.reference` / `ReferenceActor`, not v3 TOML | met as blocker evidence |
| F6 instrument readiness | `codex/bolt-v3-instrument-gate` added verified-local readiness tests; F6c remains blocked on NT `LiveNode::start` private flush/start sequence | met |
| F7 decision-event persistence | Follow-up TDD probes prove registered custom data with `event_facts: Params` preserves explicit JSON null through NT local catalog, and one production `market_selection_result` event writes through the canonical NT catalog handoff | first event/handoff verified, order gating incomplete |
| F8 risk/order admission | Tracker records NT RiskEngine exists and bolt-v3 maps risk config, but bolt-owned admission lacks decision event + order-intent path | met as blocker evidence |
| F9 order lifecycle | Tracker records NT/legacy strategy order machinery exists, but bolt-v3 has no accepted v3 run/order path | met as blocker evidence |
| F10 reconciliation/restart | Tracker records NT reconciliation primitives and bolt-v3 config mapping, but no v3 restart proof and Polymarket external-order registration hook is empty | met as blocker evidence |
| No Python production path | All F3-F10 tracker entries reject Python or avoid it; no Python runtime artifact added | met |
| No direct venue bypass | F6c, F9, and F10 tracker entries reject direct provider/venue bypasses | met |
| No hardcoded runtime values added | F3-F10 follow-up work after instrument gate is doc-only; instrument-gate tests use fixtures and source fences | met locally |
| Verification before claims | Each doc slice used `git diff --check` and marker scan; instrument-gate slice ran focused cargo tests listed in branch handoff | met locally |

## Result

F3-F10 are not production-complete. They are classified with evidence:

- F3-F5: blocked by missing v3 reference contract.
- F6a-F6b: verified locally.
- F6c: blocked by NT startup/instrument-load boundary.
- F7: nullable-field encoding unblocked by NT `Params` proof; one production `market_selection_result` event and direct catalog handoff are verified locally; order-submission/pre-submit events and submit-blocking integration remain unverified.
- F8: blocked until F7 and v3 order intent exist.
- F9: blocked until F6c, F7, and F8 unblock.
- F10: blocked until F6c-F9 unblock and Polymarket external-order tracking is proven.

## Residual Acceptance Gates

External reviews remain pending and are intentionally not requested here. CI on these branches has not been used as acceptance evidence. Merge approval remains required.

The next implementation decision should not start F9 or F10. The next credible slice is order-submission/pre-submit decision-event types plus v3 order-intent submit-blocking integration.
