# Decision Packet: Candidate #205

## Purpose

These are the exact decisions that must be frozen before implementation on `#205` can begin.

This is not a solution proposal.
It is the blocking decision surface.

## Decision 1: Eligible Prior Proof Source

### Question

What exact prior run may satisfy same-SHA proof for a smoke tag?

### Frozen Default

Use exactly one successful `main` push CI run attempt for the exact SHA.

The effective proof identity is:

- repo
- `ref = main`
- `event = push`
- `workflow = CI`
- exact `head_sha`
- exact `run_id/run_attempt`

### Why It Blocks

Without this, implementation can “reuse proof” from the wrong run or a semantically weaker run.

## Decision 2: Artifact Lineage Contract

### Question

What exact artifact provenance is required before deploy may trust a prior same-SHA build?

### Frozen Default

Deploy may trust only the `bolt-v2-binary` artifact and its `bolt-v2.sha256` digest from that exact eligible run attempt.

### Why It Blocks

Without this, the process cannot prove that the deployed binary is the already-proven binary for the exact SHA.

## Decision 3: Fail-Closed Fallback Rule

### Question

What happens if eligible same-SHA proof is missing, failed, cancelled, unreadable, or incomplete?

### Frozen Default

If eligible proof or artifact is missing, failed, cancelled, unreadable, or non-unique, rerun heavy lanes fail-closed rather than deploy with partial proof.

### Why It Blocks

Without this, the optimization can silently weaken the current safety boundary.

## Non-Decision Facts Already Frozen

These do not need further debate:

1. `main` push and immediate `v*` tag on the same SHA currently create separate CI runs.
2. The current tag path re-enters heavy proof lanes before deploy.
3. Current deploy safety guards include:
   - tag commit must be on `main`
   - S3-path idempotency check

## Exit Condition

These 3 decisions are now frozen into the seam/proof artifacts.
