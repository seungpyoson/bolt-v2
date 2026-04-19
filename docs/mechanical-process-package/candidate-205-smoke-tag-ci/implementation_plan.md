# Implementation Plan: Candidate #205

## Goal

Remove duplicate heavy proof work for the exact same-SHA smoke-tag case without weakening current deploy safety.

## Frozen Defaults

Implementation must preserve the already-frozen design defaults:

1. reusable proof source is exactly one successful `main` push CI run attempt for the exact SHA
2. reusable artifact is exactly the `bolt-v2-binary` artifact plus `bolt-v2.sha256` from that run attempt
3. if proof or artifact is missing, ambiguous, failed, cancelled, or unreadable, the path fails closed by rerunning heavy lanes

## Scope

In scope:

- `.github/workflows/ci.yml`
- any narrowly-scoped helper script needed for GitHub run/artifact lookup
- tests or lint checks for workflow contract

Out of scope:

- deploy artifact format changes beyond what is needed for provenance
- generic CI optimization outside same-SHA smoke-tag proof
- changes to application/runtime code

## Implementation Shape

### Task 1: Eligible Proof Record Lookup

Add a tag-path lookup step/job that:

- runs only for `refs/tags/v*`
- queries Actions history for `repo=seungpyoson/bolt-v2`, `ref=main`, `event=push`, `workflow=CI`, exact `github.sha`
- requires exactly one successful eligible run attempt

Outputs:

- eligible run id
- eligible run attempt
- proof-reuse allowed true/false

Fail-closed behavior:

- if zero eligible runs exist, ambiguous multiple runs exist, or lookup fails, set `proof-reuse allowed = false`

### Task 2: Exact Artifact Binding

When proof reuse is allowed:

- fetch the `bolt-v2-binary` artifact from the eligible run
- require `bolt-v2` and `bolt-v2.sha256`
- verify digest before deploy path continues

Fail-closed behavior:

- if artifact download fails, artifact is missing, or digest check fails, set `proof-reuse allowed = false`

### Task 3: Conditional Heavy-Lane Bypass

For the exact same-SHA tag case only:

- if `proof-reuse allowed = true`, do not rerun duplicate heavy lanes on the tag-triggered run
- if `proof-reuse allowed = false`, preserve the current heavy-lane path

This is the key constraint:

the optimization applies only to exact same-SHA smoke-tag proof, not to generic tag pushes.

### Task 4: Preserve Current Safety Guards

Keep unchanged:

- `Verify tag is on main`
- S3 idempotency check

No implementation is acceptable if it weakens either.

## Verification Plan

### V1: Exact Same-SHA Fast Path

Prove that:

- merged `main` push run succeeds for SHA `X`
- immediate smoke tag on SHA `X` does not wait on fresh heavy `test/build` work before deploy begins

### V2: Missing/Unreadable Eligible Proof Fails Closed

Prove that:

- if eligible run lookup fails or artifact download/digest verification fails, the tag path reruns heavy lanes instead of deploying with partial proof

### V3: Wrong-SHA Reuse Is Impossible

Prove that:

- a different SHA cannot satisfy the lookup
- a mismatched artifact cannot pass provenance verification

### V4: Current Safety Guards Still Hold

Prove that:

- tag not on `main` still blocks deploy
- existing idempotency behavior still blocks duplicate upload

## Completion Condition

This candidate is not complete when the workflow changes merely look plausible.

It is complete only when the exact same-SHA smoke-tag path:

- avoids duplicate heavy proof work
- preserves exact-sha proof identity
- preserves exact artifact lineage
- fails closed on ambiguity or missing proof
- preserves current tag-on-main and idempotency guards
