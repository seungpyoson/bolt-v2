# Fresh Issue Experiment Protocol v1

## Purpose

This is the next experiment required before calling the process production-worthy.

The goal is not more theory.
The goal is one fresh issue run where the process owns the blocker classes before external review.

## Objective

Run one new `bolt-v2` issue or one named slice through:

- intake lock
- seam lock
- proof-plan lock
- implementation
- proof gate
- finding resolution
- external review

and measure whether external review mostly confirms instead of discovering a new blocker class.

## Entry Criteria

The issue must satisfy all of:

1. One clear semantic seam
2. One main correctness contract
3. Small enough scope that one artifact directory stays tight
4. No dependency on unresolved giant refactors

## Required Artifacts

Before code:

- `issue_contract.toml`
- `seam_contract.toml`
- `proof_plan.toml`

Before external review:

- `evidence_bundle.toml`
- `finding_ledger.toml`
- `merge_claims.toml`
- `review_target.toml`

## Pass Condition

The experiment passes only if:

1. all required artifacts exist and validate
2. external review yields no new blocker class that should have been owned by intake, seam, or proof-plan lock
3. any review finding is either:
   - duplicate
   - stale
   - already-open canonical finding
   - low-risk note outside merge-blocking scope

## Fail Conditions

The experiment fails if any of the following happen:

1. external review finds a new blocker class that was not represented in the proof plan
2. external review finds semantic ambiguity that the seam lock should have blocked
3. reviewers have to reconstruct scope or exact review target from prose
4. the finding ledger cannot disposition all review findings MECE

## Metrics

These are not percentage scores.

They are structural outcomes:

- `new_blocker_class_after_review`
- `seam_ambiguity_survived_to_review`
- `review_target_drift_occurred`
- `finding_disposition_not_mece`

Any `true` value is a failed experiment.

## Expected Readout

The result document for the fresh issue should answer:

1. Did the process block the right things before implementation?
2. Did the proof plan force the real critical claims?
3. Did external review mostly confirm, or did it discover a new blocker class?
4. Which gate failed, if any?

## Recommendation

Do not call the process production-ready until this experiment passes once on a real fresh issue.
