# Mechanical Validator Spec v1

## Purpose

This document defines the exact mechanical enforcement surface for the delivery process.

It is not the implementation.
It is the contract the implementation must satisfy.

The validator reads one delivery directory and returns:

- pass
- fail with structured findings

The validator must not emit vague prose.

## Input Surface

Required files:

- `issue_contract.toml`
- `seam_contract.toml`
- `proof_plan.toml`
- `finding_ledger.toml`
- `evidence_bundle.toml`
- `merge_claims.toml`
- `review_target.toml` if review findings are present

Optional files:

- `README.md`
- `result.md`
- `stage_promotion.toml`
- `promotion_gate.toml`

## Output Contract

Every validator failure must be emitted as:

- `STATUS`
- `KIND`
- `WHERE`
- `WHY`
- `NEXT`

### Allowed STATUS Values

- `BLOCK`
- `WARN`

### Allowed KIND Values

- `schema`
- `scope`
- `semantic`
- `proof`
- `review_target`
- `finding`
- `evidence`
- `merge_claim`

### Output Example

```text
STATUS: BLOCK
KIND: semantic
WHERE: seam_contract.toml / seams[0]
WHY: storage_field `active.interval_open` maps to unresolved authoritative_source `UNFROZEN`
NEXT: set exactly one authoritative_source or keep deliverable blocked
```

## Pass 1: Schema Validation

Checks:

- all required files exist
- all required top-level keys exist
- all enums are legal
- no unknown terminal dispositions

Failure examples:

- missing file
- missing key
- malformed TOML
- illegal disposition

## Pass 2: Scope Validation

Checks:

- one issue or one named slice only
- allowed surfaces non-empty
- forbidden surfaces non-empty if there are known exclusions
- no path appears in both
- no fix language in `problem_statement`

Failure examples:

- mixed issue scope
- empty scope
- hidden fix language

## Pass 3: Seam Validation

Checks:

- each seam row has exactly one semantic term
- each storage field maps to only one semantic term
- authoritative source is not `UNFROZEN` unless deliverable status is blocked
- forbidden sources are not listed in fallback order
- freshness clock is declared for time-sensitive seams

Failure examples:

- mixed semantics
- undeclared fallback
- frozen deliverable with unfrozen seam

## Pass 4: Proof Plan Validation

Checks:

- every required outcome has at least one claim
- every seam-sensitive claim has at least one falsifier
- every claim references known evidence IDs or placeholders to be collected
- every claim has `required_before`

Failure examples:

- missing claim
- missing falsifier
- orphaned claim

## Pass 5: Review Target Validation

Checks:

- if review-derived findings exist, `review_target.toml` exists
- each review finding references the active target round
- review-target mismatch findings are explicitly classified

Failure examples:

- review finding without review target
- stale review still open as active correctness finding

## Pass 6: Finding Ledger Validation

Checks:

- each finding has one canonical key
- each finding has exactly one resolution kind if resolved
- duplicates point to canonical finding
- deferred findings point to tracked issue
- boundary accepts name an assumption and a monitor

Failure examples:

- unresolved duplicate
- two terminal dispositions
- deferred with no tracked issue

## Pass 7: Evidence Validation

Checks:

- every evidence row points to a real claim or finding
- every `invalid` or `stale` resolution has contradiction evidence
- every `fix_here` resolution has supporting evidence
- every merge claim references present evidence

Failure examples:

- claim without evidence
- stale without contradiction evidence
- fix without proof

## Pass 8: Merge Validation

Checks:

- `merge_ready = true` only if no blocker findings are open
- all required claims are present
- no required claim is `unknown`
- all supported_by references exist

Failure examples:

- merge true with open blocker
- missing merge claim
- unsupported true claim

## Pass 9: Review Drift Validation

Checks:

- if the review target changed across rounds, earlier comments are either superseded or stale
- comments on absent files/hunks cannot remain active findings

This pass exists because stacked/rebuilt PRs routinely produce stale bot findings.

## Terminal Behavior

The validator returns success only if:

- schema passes
- scope passes
- seam passes for the deliverable status
- proof plan is complete for the current stage
- finding ledger is MECE
- evidence coverage is complete for current claims
- merge truth table is internally consistent

Otherwise it returns a set of structured failures.

## Stage Awareness

The validator must support stage-aware enforcement:

- `intake`
- `seam_locked`
- `proof_locked`
- `review`
- `merge_candidate`

Earlier stages may permit unresolved merge claims.
Later stages may not.

## Exclusive Stage Gate

Stage advancement must not happen through a loose accumulation of artifacts.

For the active stage, the package must declare exactly one stage-promotion row and exactly one promotion-gate artifact.
That gate must run one declared comparator against bound artifacts or literals.

The validator must fail if:

- the active stage has zero promotion rows
- the active stage has more than one promotion row
- the active promotion row does not name one gate artifact
- the gate artifact is missing
- the gate artifact contains zero or multiple gates
- the gate comparator cannot resolve its bound refs
- the declared gate verdict is not explicitly `pass`

## Success Criterion

This validator is good enough only if a reviewer can mostly confirm:

- seam correctness
- proof completeness
- evidence relevance

and does not need to invent a new closure structure in prose.
