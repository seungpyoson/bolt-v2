# Candidate #205 Seam/Proof Lock Result

## Stage

Seam lock and proof-plan lock only.

No implementation work has been admitted.

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

## Verdict

This is a stronger process result.

The process did not jump from symptom to workflow code.
It:

1. froze the current same-SHA duplication seam
2. froze the current safety guards
3. surfaced the missing design decisions
4. froze those decisions fail-closed before any implementation was admitted

The fresh-issue trial for `#205` is now ready for implementation planning, but not yet for merge claims.
