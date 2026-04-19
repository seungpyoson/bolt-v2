# Mechanical Delivery Process v1

## Purpose

This document defines a process for turning a single issue or named slice into a deliverable that can be judged mechanically instead of conversationally.

The process is designed for `bolt-v2`, but the structure is general:

- freeze the exact problem before code changes
- freeze the exact semantic seam before design and implementation
- require explicit proof obligations before merge claims
- collapse reviewer wording into canonical findings
- make every merge claim refer to machine evidence

This is not a generic planning memo.
This is the exact process suggestion to test.

## Why The Previous Loops Failed

The failure mode was not "AI is not intelligent enough."
The failure mode was that the process admitted work before the exact problem and the exact seam were frozen.

That creates five predictable breakdowns:

1. Symptom-to-fix drift
   The session sees a symptom, imagines a plausible cause, and starts editing before proving the seam.

2. Reviewer-discovered-too-late blockers
   Reviewers become the first place where hidden assumptions, wrong-layer fixes, or missing proof obligations are discovered.

3. Finding explosion
   Different models describe the same underlying problem with different wording, so the issue appears to have many unresolved findings when it really has one unresolved claim.

4. Passing the wrong proof
   CI, tests, or local runs go green on a branch even though the branch never proved the issue's real acceptance claim.

5. Hidden fallback semantics
   The implementation picks a fallback that seems operationally reasonable, but the fallback silently changes the meaning of a critical field.

`bolt-v2` has already exhibited this class of failure around selector handling, review debt, CI-proof mismatches, and seam ambiguity.

## Design Principles

The process is built on these rules:

1. One deliverable, one scope contract.
   A branch or slice can satisfy only one declared issue or one named slice.

2. A problem statement must not contain a fix.
   The issue intake artifact may contain symptoms, invariants, non-goals, and success criteria, but not implementation proposals.

3. A seam must be frozen before implementation.
   If a field, function, or state transition has ambiguous meaning, the process blocks before code work.

4. A proof obligation must exist before work claims.
   Every acceptance claim must name the proof class that will discharge it.

5. Findings are first-class data.
   Review comments are not the system of record. Canonical findings are.

6. Merge claims are not prose.
   A merge claim is valid only if it points to exact evidence and exact closed findings.

7. Fail-closed by default.
   If the process cannot classify, dedupe, prove, or attribute, the state is blocked.

8. One stage, one gate.
   Stage advancement happens only through one declared gate for that stage.
   Supporting artifacts may feed the gate, but they do not independently advance the deliverable.

## Process Architecture

Each deliverable owns one directory:

`docs/delivery/<issue-or-slice-id>/`

That directory contains six artifacts:

1. `issue_contract.toml`
2. `seam_contract.toml`
3. `proof_plan.toml`
4. `finding_ledger.toml`
5. `evidence_bundle.toml`
6. `merge_claims.toml`
7. `review_target.toml`

These artifacts are the only source of truth for closure.

GitHub issue text, PR text, review comments, CI, local testing, and reviewer notes remain upstream inputs or downstream outputs, but they do not define closure on their own.

## Artifact 7: review_target.toml

### Role

`review_target.toml` freezes the exact thing under review for one review round.

This artifact exists because stale review artifacts are common in stacked branches and rebuilt PRs.
Without an explicit review target, external reviewers can produce valid comments on the wrong diff, wrong base, or wrong head.

### Required Fields

- `repo`
- `pr_number`
- `base_ref`
- `head_sha`
- `diff_identity`
- `round_id`
- `status`

### Mechanical Checks

The validator must fail if:

- a review comment is recorded without an active review target
- a finding from review does not reference the review target that produced it
- a finding points to files or hunks absent from the frozen diff identity

### Stale Review Rule

If a review comment points to code absent from the frozen `base_ref..head_sha` diff, the finding is not debated informally.
It is mechanically classified as `stale_review`.

## Artifact 1: issue_contract.toml

### Role

`issue_contract.toml` freezes the problem and the declared slice.

It answers:

- what exact issue or slice is being addressed
- what exact outcomes are required
- what is explicitly out of scope
- what assumptions are currently accepted
- what surfaces are allowed to change

### Forbidden Content

The following content is forbidden in `issue_contract.toml`:

- proposed code edits
- "fix by" language
- hidden adjacent cleanup
- implementation justifications

### Required Fields

`issue_contract.toml` must contain:

- `issue_id`
- `title`
- `repo`
- `slice_id`
- `status`
- `problem_statement`
- `required_outcomes`
- `non_goals`
- `allowed_surfaces`
- `forbidden_surfaces`
- `assumptions`
- `semantic_terms`

### Mechanical Checks

The validator must fail if:

- no `required_outcomes` are declared
- no `non_goals` are declared
- allowed surfaces are empty
- the same path appears in allowed and forbidden surfaces
- fix-shaped language appears in the problem statement
- a semantic term is referenced later but not declared here

## Artifact 2: seam_contract.toml

### Role

`seam_contract.toml` freezes the exact semantic boundary that matters for correctness.

This is the most important artifact in the system.

The seam contract exists because most high-cost failures are not random bugs.
They are field-semantics bugs:

- a value means one thing at write time and another thing at read time
- a fallback silently changes the meaning of a field
- a staleness clock is broader than the source being protected
- a "reference" value is actually a fused operational surrogate

### Required Fields

The seam contract is a table of semantic rows.
Each row must contain:

- `seam_id`
- `semantic_term`
- `writer_path`
- `writer_symbol`
- `reader_path`
- `reader_symbol`
- `storage_field`
- `authoritative_source`
- `allowed_sources`
- `forbidden_sources`
- `fallback_order`
- `freshness_clock`
- `status`

### Mechanical Checks

The validator must fail if:

- one storage field maps to more than one semantic term in the same slice
- a fallback source is used but not declared
- a forbidden source appears in fallback order
- a seam row has writer without reader or reader without writer
- freshness clock is unspecified for a time-sensitive seam
- status is `unknown`

### Fail-Closed Rule

If the team cannot write a seam row unambiguously, implementation is blocked.

That is the intended behavior.

## Artifact 3: proof_plan.toml

### Role

`proof_plan.toml` maps each required outcome and each seam-sensitive claim to a proof obligation.

This avoids two opposite failures:

- "we tested something" without proving the issue
- "we used many proofs" but forgot the critical claim

### Proof Classes

The process does not force one tool.
It allows a small set of proof classes:

- `direct_artifact`
  Exact logs, exact runtime traces, exact external artifact matches.

- `code_path_trace`
  Proven writer/reader path with exact source flow.

- `model_check`
  Spec-level or seam-level model checking for transitions and invariants.

- `bounded_proof`
  Exhaustive bounded proof on pure kernels or unsafe boundaries.

- `property_stateful`
  Generated operation sequences and shrunk counterexamples.

- `runtime_monitor`
  Monitor over execution traces enforcing a declared property.

- `negative_counterexample`
  Required falsifier case that must fail when the wrong semantics are present.

### Required Fields

Each proof obligation must contain:

- `claim_id`
- `claim_text`
- `claim_kind`
- `covered_by`
- `falsified_by`
- `required_before`
- `status`

### Mechanical Checks

The validator must fail if:

- a required outcome has no proof obligation
- a seam-sensitive claim has no falsifier
- a proof obligation names an unknown semantic term
- a merge claim later references a claim not present here

## Artifact 4: finding_ledger.toml

### Role

`finding_ledger.toml` is the canonical record of all findings.

Reviewer comments are inputs.
The ledger is the truth.

### Canonical Finding Key

A finding key is deterministic:

`<slice>|<kind>|<subject>|<predicate>|<locus>`

This key is intended to collapse wording drift.

Two comments become one finding if their normalized:

- kind
- subject
- predicate
- locus

are the same.

### Allowed Finding Kinds

The initial MECE finding kinds are:

- `scope_violation`
- `semantic_ambiguity`
- `missing_proof`
- `wrong_layer_fix`
- `behavior_mismatch`
- `artifact_mismatch`
- `environment_assumption`
- `test_gap`
- `stale_review`
- `duplicate_review`
- `review_target_mismatch`

These are not severity levels.
They are claim types.

### Allowed Dispositions

Each finding must have exactly one disposition:

- `invalid`
- `stale`
- `duplicate`
- `fix_here`
- `defer_tracked`
- `boundary_accept`

If none of the six applies, the finding stays open and the merge blocks.

### Mechanical Checks

The validator must fail if:

- a finding has zero or multiple dispositions
- duplicate findings do not point to a canonical finding
- a deferred finding has no tracking reference
- a boundary acceptance has no explicit assumption and no monitor
- the same canonical key appears twice as a non-duplicate

## Artifact 5: evidence_bundle.toml

### Role

`evidence_bundle.toml` stores the exact artifacts that support claims and findings.

Evidence is not "I ran it locally."
Evidence is an exact machine artifact with provenance.

### Allowed Evidence Types

- `repo_source_ref`
- `test_output`
- `ci_log`
- `external_artifact`
- `code_path_trace`
- `model_checker_output`
- `bounded_proof_output`
- `stateful_counterexample`
- `runtime_monitor_output`

### Required Fields

Each evidence row must contain:

- `evidence_id`
- `type`
- `producer`
- `subject`
- `artifact_ref`
- `proves`
- `contradicts`
- `captured_at`

### Mechanical Checks

The validator must fail if:

- an evidence row proves and contradicts the same target
- an evidence row points to a missing target
- a merge claim references evidence that does not exist
- a finding marked `invalid` or `stale` has no contradiction evidence

## Artifact 6: merge_claims.toml

### Role

`merge_claims.toml` is the pre-merge truth table.

It records exactly what is being claimed on the current head.

### Required Fields

- `merge_ready`
- `head_scope_hash`
- `claims`
- `open_blockers`
- `closed_findings`
- `required_evidence`

Each claim row must contain:

- `claim_id`
- `value`
- `supported_by`
- `depends_on_closed_findings`

### Mechanical Checks

The validator must fail if:

- `merge_ready = true` while any blocker finding is open
- a claim is `true` but has no evidence
- a claim is omitted even though it was required in `proof_plan.toml`
- any evidence row is referenced before it exists

## Gate Sequence

The process has seven gates.

### Gate 1: Intake Lock

Inputs:

- issue text
- operator intent

Outputs:

- `issue_contract.toml`

Fail conditions:

- hidden fix language
- missing required outcomes
- missing non-goals
- broad or mixed scope

### Gate 2: Seam Lock

Inputs:

- issue contract
- exact code path trace of current implementation
- exact external or domain source of truth

Outputs:

- `seam_contract.toml`

Fail conditions:

- storage field with mixed semantics
- undeclared fallback
- freshness clock mismatch
- authoritative source not frozen

### Gate 3: Proof Plan Lock

Inputs:

- issue contract
- seam contract

Outputs:

- `proof_plan.toml`

Fail conditions:

- outcome without proof obligation
- seam-sensitive claim without falsifier
- proof classes chosen but not applicable to the claim

### Gate 4: Implementation Window

Inputs:

- locked issue contract
- locked seam contract
- locked proof plan

Outputs:

- code changes, if any

Fail conditions:

- changes outside allowed surfaces
- changes that introduce undeclared semantic terms
- changes that require seam contract edits without reopening the gate

### Gate 5: Proof Gate

Inputs:

- exact head
- exact proof outputs

Outputs:

- `evidence_bundle.toml`

Fail conditions:

- proof artifact missing
- proof artifact not from the exact head
- proof artifact does not discharge the named claim

### Gate 6: Finding Resolution Gate

Inputs:

- reviewer comments
- frozen review target
- discovered mismatches
- evidence bundle

Outputs:

- `finding_ledger.toml`

Fail conditions:

- free-form comments left uncategorized
- duplicate wording not canonicalized
- finding with no terminal disposition
- review finding that does not match the active review target and is not classified as `stale_review` or `review_target_mismatch`

### Gate 7: Merge Gate

Inputs:

- all six artifacts

Outputs:

- `merge_claims.toml`

Fail conditions:

- any required claim unresolved
- any blocker finding open
- any merge truth asserted without evidence

## Proof Trigger Matrix

The process is general, but proof classes are triggered by shape.

### Trigger: semantic seam

If the issue changes the meaning of a field or transition:

- seam contract required
- direct artifact proof required
- code-path trace required
- at least one negative falsifier required

### Trigger: concurrency or order lifecycle

- seam contract required
- model check preferred
- stateful property test required

### Trigger: unsafe code, arithmetic limits, parser/codec, pure kernel

- bounded proof preferred

### Trigger: external system behavior

- direct artifact proof required
- runtime monitor preferred if the behavior persists after merge

### Trigger: CI-only claim

- CI log evidence from exact head required
- local proof is not sufficient

## Reviewer Role In This Process

Reviewers are not asked to infer correctness from prose.

Reviewers do three things:

1. challenge the seam contract
2. challenge missing claims in the proof plan
3. challenge whether evidence actually discharges the claim

If a reviewer finds a new blocker after Gate 5, the process records that as a process failure, not just a code failure.

The question is:

"Which gate should have blocked this?"

## Expected Failure Modes

This process does not make all bugs impossible.
It changes where failure happens.

The acceptable failure mode is:

- blocked before implementation
- blocked before merge
- blocked because claim, seam, or proof is incomplete

The unacceptable failure mode is:

- implementation proceeds with ambiguous semantics
- external review is the first place that discovers a blocker class

## Experiment Protocol

This process should be tested before rollout.

### Experiment Objective

Determine whether the process blocks semantic ambiguity before implementation on a real `bolt-v2` seam.

### First Experiment

Use the ETH anchor semantics seam:

- the strategy field `interval_open`
- the market anchor `price_to_beat`
- the fallback chain `price_to_beat -> oracle -> fused fair_value`
- the staleness clock protecting Chainlink usage

### Hypothesis

If the process is working, the seam lock will block implementation until:

- `interval_open` has one frozen meaning
- allowed and forbidden sources are explicit
- stale-clock semantics are explicit
- proof obligations cover disagreement cases

### Falsification

The process fails this experiment if:

- implementation would be allowed while `interval_open` still has mixed semantics
- fused `fair_value` can still serve as a silent anchor fallback without being declared
- stale-clock semantics remain broader than the seam they are supposed to protect

## Rollout Recommendation

Do not roll this out repo-wide yet.

Run it on a small number of high-value seams first.

The rollout criterion is:

- external review mostly dedupes, goes stale, or confirms evidence
- external review does not introduce a new blocker class that should have been caught by Intake, Seam, or Proof gate

## Summary

The core thesis of this process is simple:

`bolt-v2` does not mainly need more reviewers.
It needs a fail-closed path from issue to seam to proof to finding to merge.

The seam lock is the center of gravity.

Without it, the system will keep producing work that is well-tested, locally justified, and still semantically wrong.
