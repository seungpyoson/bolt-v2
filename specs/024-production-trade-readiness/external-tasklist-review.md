# External Task-List Review

Scope: task-list packet only; no production code was approved by this review round.

Reviewed files:

- `.specify/feature.json`
- `AGENTS.md`
- `specs/024-production-trade-readiness/spec.md`
- `specs/024-production-trade-readiness/plan.md`
- `specs/024-production-trade-readiness/tasks.md`
- `specs/024-production-trade-readiness/evidence.md`

## Round 1

- Claude job `9b6c69f0-7995-43a6-a816-1412e7598ba0`: APPROVE. Nonblocking findings: stale #466 branch identity, incomplete quorum/waiver wording, soft #409 close criteria, and possible parallel config-write conflict.
- Gemini job `88de4a17-b60d-4f49-af76-80d87c074194`: APPROVE. Nonblocking findings: stale active-feature pointer note and ensure existing cancel-if-open collector remains represented.
- Grok job `job_080bbbfd-5237-4b60-9c2a-b66e3621b160`: APPROVE for the selected packet only. Nonblocking findings: stale branch identity, stale pointer note, many ledger files, and ensure Grok remains included in review tasks.
- DeepSeek job `job_9c9a45d5-ed23-4dcf-81e5-2a84607e473c`: REQUEST_CHANGES. Blocking finding: PR #478 and the feature packet carried the stale `goal/466-command-tokenization-characterization` branch identity while #466 is explicitly out of scope.
- GLM job `job_ea59d73d-8992-40db-ab7d-09e0bfdc61a4`: APPROVE. Nonblocking findings: branch identity mismatch, stale pointer note, external-review bottleneck, #409 deferred, and explicit `.specify/feature.json` tracking.
- Kimi job `f7c9b831-7cf7-443d-bd34-d5e98de257ba`: FAILED with `step_limit_exceeded`; no valid verdict.

## Round 1 Disposition

- Blocking branch-identity finding: fixed by moving active readiness work to `goal/024-production-trade-readiness`. GitHub closed historical PR #478 after the rename, so PR #480 is now the single active readiness PR.
- Six-reviewer quorum wording: fixed in `spec.md` and `tasks.md`; the required reviewers are Claude, Gemini, DeepSeek, GLM, Kimi, and Grok.
- Scope-contamination handling: tightened T003 to require removal of #466 verifier-characterization/decomposition files from PR #480 before implementation resumes.
- Stale active-feature pointer: `.specify/feature.json` now points to `specs/024-production-trade-readiness`.
- #409 criteria: T007 remains a required evidence task before issue close/update.

## Current Gate

Unanimous task-list approval is not yet achieved. Round 2 must include the corrected packet and produce six valid approvals, unless the operator explicitly waives a reviewer with exact failure evidence.
