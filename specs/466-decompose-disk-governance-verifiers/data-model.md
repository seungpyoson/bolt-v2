# Data Model: #466 Disk-Governance Verifier Decomposition

## Scope Ledger Item

- `id`: Stable numeric item from the #466 list.
- `name`: Human-readable scope item.
- `runtime_evidence`: Current-main runtime verifier path and line evidence.
- `static_evidence`: Current-main static verifier path and line evidence.
- `test_doc_evidence`: Tests, specs, issue, PR, or review docs proving current state.
- `equivalence_verdict`: `proven equivalent`, `divergent but characterizable`, `insufficient evidence`, or `not applicable`.
- `chosen_resolution`: Extract shared helper, characterize and keep local, mechanically split, cleanup, or operator-approved out-of-scope movement.
- `files`: Touched or intentionally untouched files.
- `tests_required`: Local and CI verification needed for closure.
- `review_evidence`: External reviewer verdicts, findings, failed slots, skipped slots, and operator waivers.
- `final_state`: `open`, `resolved`, `blocked`, or `operator-moved`; final completion permits only the last three, with `blocked` not acceptable for issue closure.

## Helper Family

- `name`: Candidate behavior group.
- `runtime_contract`: Runtime verifier behavior and return shape.
- `static_contract`: Static verifier behavior and return shape.
- `shared_contract`: Proposed shared behavior when applicable.
- `semantic_boundary`: Filesystem, tokenization, wrapper, policy, or import-mode boundary.
- `extraction_eligibility`: Evidence-backed state.

## PR Slice

- `ledger_items_covered`: One or more coherent #466 ledger items.
- `non_goals`: Residual #466 scope explicitly left open.
- `behavior_preservation_strategy`: Characterization, parity, identity, or mechanical-move proof.
- `verification`: Local commands, exact-head CI, and reviewer evidence.
- `closure_claim`: Whether #466 remains open; must remain open until the final ledger is resolved.

## Review Gate

- `reviewer`: Claude, Gemini, Grok, GLM, DeepSeek, or Kimi.
- `route`: CLI/plugin/direct API route.
- `source_scope`: Files and docs sent for review.
- `verdict`: Approve, request changes, no verdict, failed, or skipped.
- `operator_waiver`: Explicit waiver text and date when a failed/skipped slot is accepted.
- `audit_metadata`: Provider, prompt hash, auth path, request settings, source-send state, and artifact path where applicable.

## Verification Evidence

- `command`: Exact command or GitHub check.
- `head`: Exact commit SHA when applicable.
- `result`: Pass, fail, skipped, inconclusive.
- `coverage`: Ledger item or PR slice covered.
- `notes`: Failure reason, rerun requirement, or residual risk.
