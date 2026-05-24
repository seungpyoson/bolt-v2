# External Task-List Review

Scope: task-list packet only; no production code was approved by this review round.

Reviewed files:

- `.specify/feature.json`
- `AGENTS.md`
- `specs/024-production-trade-readiness/spec.md`
- `specs/024-production-trade-readiness/plan.md`
- `specs/024-production-trade-readiness/tasks.md`
- `specs/024-production-trade-readiness/evidence.md`
- `specs/024-production-trade-readiness/external-tasklist-review.md`
- `specs/024-production-trade-readiness/scope-resolution.md`

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

Unanimous task-list approval is not yet achieved. Round 3 must include the corrected packet and produce six valid approvals, unless the operator explicitly waives a reviewer with exact failure evidence.

## Round 2

Reviewed packet head: `e8db9598883efb14932fdd179627acb6c9ac6fcd`.

- Claude job `9ef1d86b-80ba-48bf-9ecc-94d11751a395`: FAILED before source transmission with `oauth_inference_rejected` / HTTP 401; no valid verdict.
- Gemini job `df6596d3-f46a-458c-b087-45bf25022eae`: APPROVE. Nonblocking finding: `evidence.md` recorded stale head `aee45c97108219e82034bb2730aa4f1ddf7da5e8` while the reviewed head was `e8db9598883efb14932fdd179627acb6c9ac6fcd`.
- Grok job `job_023c001b-a209-44f7-80f2-24dafee0adc0`: REQUEST_CHANGES. Blocking finding: evidence baseline had stale head/base identity and old `466-command-tokenization-characterization` worktree references, so the packet did not prove the exact reviewed head.
- DeepSeek job `job_19644ecc-a253-4c85-a224-7e547997dce8`: APPROVE. Nonblocking findings: stale evidence head, external-review quorum incomplete, and parallel config-write coordination risk.
- GLM job `job_3553dcf3-3909-43b5-93fd-a40a55ea5c90`: APPROVE. Nonblocking findings: stale evidence head, old worktree path, T007 needs explicit #409 acceptance criteria, T038 audit method could be tighter, and parallel collector config-write risk.
- Kimi job `56fd6061-20de-4f1b-98b8-31dcd817be68`: CANCELLED after Grok returned a blocking finding against the packet; source was sent but no valid verdict was produced.

## Round 2 Disposition

- Stale exact-head evidence: fixed by changing `evidence.md` to record exact PR #480 base/head evidence from `gh pr view 480` and to state that external-review audit manifests are authoritative for later docs-only review commits.
- Old worktree identity: fixed by moving the active local worktree to `/Users/spson/Projects/Claude/bolt-v2/.worktrees/024-production-trade-readiness` and updating `evidence.md`.
- Reviewed-files list: fixed to include `external-tasklist-review.md` and `scope-resolution.md`.
- T004 scope packet: fixed to include `scope-resolution.md`.
- #409 criteria, T038 audit method, and parallel config-write coordination remain nonblocking execution-hygiene items because they are tracked by T007, T006, and T023/T033/T037 respectively.

Round 2 does not unblock implementation because Grok requested changes, Claude failed before source transmission, and Kimi produced no verdict. Round 3 is required.
