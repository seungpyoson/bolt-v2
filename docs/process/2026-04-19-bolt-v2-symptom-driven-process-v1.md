# Bolt-v2 Symptom-Driven Process v1

## Why this exists

`proof-matrix-v1` is useful as a classification artifact, but it is too broad to be the primary operating process for `bolt-v2`.

Stored repo memory already points to the right correction:
- do not run broad invariant-mapping pipelines as the main stabilization process
- start from the actual operator pain
- trace the real code path
- derive the relevant obligations only for that seam

So the production-oriented `bolt-v2` process should be symptom-driven first, class-driven second.

## The Process

### 1. Intake

Start from one concrete operator-visible symptom.

Required artifact:
- one-sentence symptom
- one-sentence intended end state
- one-sentence explicit non-goal

If there are multiple symptoms, split them before any code work.

### 2. Mechanism Trace

Trace the exact code path that produces the symptom.

Required artifact:
- entrypoint
- key transforms
- state transitions
- exit condition
- exact files/functions read

Rule:
- no edits before this trace exists

### 3. Change-Class Selection

Select the smallest relevant bolt-v2 change class from the library:
- CI/cache/workflow state
- selector/discovery/admission
- config/validation/fail-closed parsing
- strategy semantics/external truth source
- control-plane/trust-root
- scope/decomposition discipline

Rule:
- one primary class
- at most one secondary class
- if more are needed, the slice is too broad

### 4. Hazard Pack

Pull only the hazards for the selected class.

Rule:
- do not apply the full matrix
- apply only the rows that are relevant to the traced mechanism

This keeps the process MECE enough to be mechanical, without becoming a giant abstract checklist.

### 5. Proof Plan

Before code, define the minimum proof set for this symptom.

Allowed proof types:
- exact-head acceptance proof
- negative-path / fail-closed proof
- regression proof
- persistence-across-next-run proof
- merge-ref / workflow-shape proof
- rollback / lifecycle proof
- external-truth equivalence proof

Rule:
- every selected hazard must map to one proof artifact
- if no proof artifact exists, the PR is not ready

### 6. Minimal Change

Implement the smallest change that satisfies the proof plan.

Rule:
- no bundled cleanup
- no adjacent redesign
- no second issue in the same branch

### 7. Exact-Head Verification

Verify the issue-level claim on the exact current head.

This is the critical anti-false-ready gate.

Rule:
- CI-green is not enough
- the issue acceptance must be true on the exact head

### 8. Residual Mapping

Every residual must land in exactly one bucket:
- disproven
- fixed now
- deferred with tracked issue
- duplicated into another tracked item
- not actionable in practice

No free-floating notes.

### 9. External Review Packet

External review gets a packet, not a free-form prompt:
- issue
- exact head
- actual changed files
- exact acceptance claim
- exact evidence
- known stale context to ignore

Reviewer job:
- attack whether the exact head really satisfies the issue
- attack whether the proof artifacts are sufficient

## Hard Gates

The PR is not ready if any of these are true:
- no mechanism trace
- wrong or ambiguous change class
- missing proof artifact for a selected hazard
- exact-head issue acceptance not proven
- residuals not mechanically mapped

## How this differs from vanilla AI

Vanilla AI:
- propose fix
- write code
- let review discover what mattered

This process:
- start from symptom
- trace the mechanism
- choose the relevant class
- predeclare the needed proofs
- only then change code

## How this differs from the rejected matrix-first approach

Rejected matrix-first mode:
- starts broad
- applies too many abstract obligations
- becomes shallow and expensive

This process:
- starts narrow
- uses the matrix only as a hazard library
- keeps the work tied to the actual pain and code path

## Cheap Validation Experiment

To validate this process without new issue work:

1. Freeze this symptom-driven process.
2. Replay it against held-out bolt-v2 historical cases.
3. Ask one question:
   would this process have blocked the miss before review?

Pass:
- yes on the holdout cases

Fail:
- if reviewers would still be the first place where the relevant hazard is discovered

## Current Verdict

This is closer to a production-usable bolt-v2 process than `proof-matrix-v1` alone because it:
- uses operator pain as the root object
- keeps the process local to the real mechanism
- still preserves mechanical hazard/proof obligations

It is still not proven production-ready until the frozen process is replayed on a larger historical holdout set.
