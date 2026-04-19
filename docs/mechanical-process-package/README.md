# Mechanical Process Package

## Operator Packet

This folder is the entire current process package.

If you only want the shortest path, read these 4 files in order:

1. [process/2026-04-19-process-status-matrix-v1.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/process/2026-04-19-process-status-matrix-v1.md)
2. [process/2026-04-19-mechanical-delivery-process-v1.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/process/2026-04-19-mechanical-delivery-process-v1.md)
3. [process/2026-04-19-validator-spec-v1.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/process/2026-04-19-validator-spec-v1.md)
4. [process/2026-04-19-fresh-issue-experiment-protocol-v1.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/process/2026-04-19-fresh-issue-experiment-protocol-v1.md)

## What Is In This Folder

- `process/`
  The core process design, validator contract, gate ownership, and current status.

- `experiments/`
  Evidence packs that test specific parts of the process.

- `candidate-205-smoke-tag-ci/`
  The current best fresh-issue trial candidate, now through intake + seam/proof lock and blocked on an explicit decision packet.

## Current Readout

What is actually proven:

- seam lock can block semantic ambiguity before implementation
- finding canonicalization can collapse repeated reviewer wording and reject stale review-target artifacts
- proof-plan adequacy can be tested against real late blocker classes

What is not yet proven:

- one full fresh issue run end to end where external review mostly confirms instead of discovering a new blocker class
- the actual validator implementation

Fresh issue trial progress:

- `#205` is the active fresh-issue candidate
- it has cleared intake
- it has cleared seam/proof lock
- it has frozen fail-closed design defaults in [candidate-205-smoke-tag-ci/decision_packet.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/candidate-205-smoke-tag-ci/decision_packet.md)
- it has a drafted [implementation_plan.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/candidate-205-smoke-tag-ci/implementation_plan.md)
- it now has real implementation evidence and a validator-backed review-stage package

## Recommended Reading Modes

If you want the minimum:

- read the 4 files above only

If you want to verify the evidence:

- read `experiments/exp-eth-anchor-semantics/`
- read `experiments/exp-finding-canonicalization/`
- read `experiments/exp-proof-plan-selector-path/`

If you want the next real execution step:

- read [process/2026-04-19-fresh-issue-candidates-v1.md](/Users/spson/Projects/Claude/bolt-v2/docs/mechanical-process-package/process/2026-04-19-fresh-issue-candidates-v1.md)
- then read `candidate-205-smoke-tag-ci/`

If you want the concrete V2 follow-up for `#208`:

- read [process/2026-04-19-issue-208-v2-checklist.md](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-process-validator/docs/mechanical-process-package/process/2026-04-19-issue-208-v2-checklist.md)

## Short Verdict

This package is no longer just prose.
It has:

- a process architecture
- a validator contract
- validator fixtures
- three evidence-bearing experiments
- a fresh-issue protocol

It is still pre-production because the validator is not implemented yet and the full fresh-issue run has not been completed.
