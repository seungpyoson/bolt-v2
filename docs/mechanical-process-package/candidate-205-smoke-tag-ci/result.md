# Candidate #205 Seam/Proof Lock Result

## Stage

Implementation complete, review-stage package.

## What Is Proven Already

1. The cited main-push run and smoke-tag run both used the same head SHA:
   - `a1a6be0d94e887538ebcd9afced6c94046a557d6`
2. The smoke-tag run repeated heavy `test` and `build` lanes before deploy.
3. The deploy job itself was short relative to the heavy lanes.
4. The current workflow already freezes two safety guards on tag deploy:
   - tag commit must be on `main`
   - deploy is idempotent at the target S3 path

## What Was Frozen

The process froze these design defaults before implementation:

1. eligible prior proof is not "same SHA" alone; it is exactly one successful `main` push CI run attempt for the exact SHA
2. deploy may trust only the `bolt-v2-binary` artifact and `bolt-v2.sha256` produced by that exact eligible run attempt
3. if eligible proof or artifact is missing, ambiguous, failed, cancelled, or unreadable, the path fails closed by rerunning heavy lanes rather than deploying with partial proof

## What Was Implemented

On branch `issue-205-smoke-tag-proof` at commit `feb88d0f4344dd5116a62a77922157b26e401229`:

1. added tag-only `same_sha_proof` workflow job
2. selected exactly one successful same-SHA `main` push CI run
3. downloaded and sha256-verified the reusable `bolt-v2-binary` artifact
4. re-uploaded the verified artifact into the current tag run
5. skipped duplicate heavy tag lanes only when reuse was explicitly ready
6. preserved tag-on-main and idempotency guards

## Focused Verification

- `cargo test --test ci_same_sha_smoke_tag -- --nocapture`
- `just ci-lint-workflow`
- `git diff --check`

All three passed on the implementation branch.

## Review Findings During Implementation

Two real review findings were surfaced and addressed before closeout:

1. deploy fast path could still be skipped by GitHub Actions skip propagation if `build` stayed a hard dependency without `always()` handling
2. `run_attempt` was being claimed as part of the proof identity without the workflow actually enforcing attempt-specific artifact binding

Both were resolved on the implementation branch and recorded in the finding ledger.

## Exact Review Target

The active GitHub review target is now:

- PR `#210`
- head `6169428f9d401952b776971ffe7d03963401f7fb`

## Exact-Head CI Snapshot

Current exact-head GitHub CI snapshot recorded into the package:

- `same_sha_proof`: pass
- `detector`: pass
- `fmt-check`: pass
- `deny`: pass
- `clippy`: pass
- `test`: in progress
- `build`: in progress

Known unrelated blocker still visible on the PR surface:

- `nt-pointer-trust-root` fails independently as the already-known private-fetch trust-root issue
- this is not new `#205` logic drift

## Verdict

This is a stronger process result.

The process did not jump from symptom to workflow code.
It:

1. froze the current same-SHA duplication seam
2. froze the current safety guards
3. surfaced the missing design decisions
4. froze those decisions fail-closed before any implementation was admitted

The fresh-issue trial for `#205` has now crossed from planning into a validator-backed implementation package.

It is still not merge-ready, because:

1. exact-head CI is not fully complete yet
2. external review has not been recorded into the package yet
