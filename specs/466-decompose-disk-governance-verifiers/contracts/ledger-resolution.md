# Contract: #466 Ledger Resolution

## Scope

This contract defines when #466 ledger items, PR slices, and final issue completion may be claimed.

## Ledger Contract

- The ledger lives at `specs/466-decompose-disk-governance-verifiers/evidence.md`.
- It must include all eight #466 scope items.
- Each item must record runtime evidence, static evidence, test/doc evidence, equivalence verdict, chosen resolution, files touched or intentionally untouched, tests required, review evidence, and final state.
- Final #466 completion is invalid if any item is open, insufficient evidence, unreviewed, partial, tracked later, or TBD.

## PR Slice Contract

Each #466 PR must state:

- Ledger items covered.
- Non-goals and remaining accepted #466 scope.
- Behavior-preservation strategy.
- Characterization/parity tests.
- What remains local and why.
- Verification commands/results.
- External review results and failed/skipped slots, with superseded heads labeled as historical evidence.
- Residual risk.
- Whether #466 remains open.

A non-final PR must not say it closes #466.

## Review Contract

- Required pre-implementation reviewers: Claude, Gemini, Grok, GLM, DeepSeek, and Kimi.
- Required post-implementation reviewers for each PR-ready slice: Claude, Gemini, Grok, GLM, DeepSeek, and Kimi.
- Missing output, timeout, failed slot, shallow output, no verdict, or source-send failure is not approval.
- A failed/skipped slot may be accepted only by explicit operator waiver.
- DeepSeek and GLM direct-API source sends have standing approval, but audit metadata must still be recorded.
- Post-implementation review must target the current pushed PR head after exact-head CI is green.
- If the branch moves after CI/review records are written, those records become historical and cannot satisfy merge readiness for the new head.

## Completion Contract

#466 is complete only when:

- Every ledger item is resolved or explicitly operator-moved.
- Every needed #466 implementation PR is merged.
- No unresolved review comments remain.
- No uncommitted work remains except unrelated pre-existing user changes explicitly called out.
- Required local verification has passed for touched surfaces.
- Exact-head GitHub CI passed for every merged PR.
- Final whole-#466 external review has no blocking findings or explicit operator waivers.
- The operator explicitly approves issue closure.
