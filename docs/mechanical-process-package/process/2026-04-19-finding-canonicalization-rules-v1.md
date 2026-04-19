# Finding Canonicalization Rules v1

## Purpose

This document defines how review findings are turned into canonical findings without wording drift, stale-diff confusion, or unbounded review loops.

It is a process rule, not a code implementation.

## Canonicalization Inputs

Each raw review finding must be normalized into:

- `review_target`
- `kind`
- `subject`
- `predicate`
- `locus`
- `scope`
- `evidence_ref`

Free-form wording is not part of the canonical key.

## Canonical Key

`<scope>|<kind>|<subject>|<predicate>|<locus>`

The key ignores:

- reviewer identity
- wording style
- severity labels from the reviewer
- suggested fix text

## Precedence Rules

Canonicalization happens in this order:

1. Review target validation
2. Locus normalization
3. Subject normalization
4. Predicate normalization
5. Duplicate collapse
6. Disposition assignment

If step 1 fails, the finding never reaches duplicate collapse.
It becomes `stale_review` or `review_target_mismatch`.

## Review Target Validation

A raw review comment must first be checked against the active `review_target.toml`.

If the comment references:

- a file not in the frozen diff
- a hunk not in the frozen diff
- a superseded head

then it is not an open correctness finding on the active deliverable.

It becomes one of:

- `stale_review`
- `review_target_mismatch`

## Subject Normalization

The subject must be a compact thing-name, not a sentence.

Examples:

- `slug_fetch_concurrency`
- `legacy_event_slugs_schema_boundary`
- `review_target_identity`
- `nt_pointer_scope_drift`

## Predicate Normalization

The predicate must be a compact defect shape.

Examples:

- `unbounded_fanout`
- `subset_schema_rejects_legacy_field`
- `comment_targets_absent_diff`
- `scope_not_declared`

## Duplicate Collapse Rules

Two findings are duplicates only if:

- same normalized `kind`
- same normalized `subject`
- same normalized `predicate`
- same normalized `locus`
- same effective `scope`

Severity differences do not prevent collapse.

Reviewer wording differences do not prevent collapse.

## Non-Duplicate Rules

Two findings stay separate if any of these differ:

- one is stale and one is active
- one is scope and one is behavior
- same root cause but different locus requiring separate closure
- same locus but different predicate

## Allowed Terminal Dispositions

- `invalid`
- `stale`
- `duplicate`
- `fix_here`
- `defer_tracked`
- `boundary_accept`

No other terminal state is allowed.

## Exact-Head Rule

A finding cannot remain open on an active deliverable unless it is anchored to the exact review target head.

That single rule is what prevents endless loops from:

- rebuilt PRs
- stacked PRs
- stale merge-base reviews
- bot comments on superseded diffs

## Proof Of Resolution

A finding is resolved only when its disposition is accompanied by:

- contradiction evidence for `invalid` or `stale`
- canonical target ID for `duplicate`
- exact change + exact evidence for `fix_here`
- tracked issue reference for `defer_tracked`
- explicit assumption + monitor for `boundary_accept`

## Failure Condition

If a review round produces a new blocker that should have collapsed into an existing canonical finding but did not, the process failed its canonicalization gate.
