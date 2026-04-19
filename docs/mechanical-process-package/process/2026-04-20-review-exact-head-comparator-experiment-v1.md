# Review Exact-Head Comparator Experiment v1

## Hypothesis H4

The `review` stage's exact-head admission can be expressed through generic gate comparators, without bespoke review-stage head-matching branches in the validator.

## Validation Date

- 2026-04-20

## Treatment

1. Add `all_of` as a generic gate composition operator.
2. Keep one gate artifact and one gate row.
3. Move review exact-head admission into one composed gate with three clauses:
   - `execution_target.toml#head_sha == review_target.toml#head_sha`
   - `review_rounds/<round>.toml#absorbed_by_head == review_target.toml#head_sha`
   - `review_rounds/<round>.toml#round_id == review_target.toml#round_id`
4. Remove the bespoke validator branches that compared:
   - execution head vs review target head
   - review round id/head vs review target id/head

## Evidence Set

Targeted H4 evidence:

- 1 pass fixture:
  - synthetic review package with one `all_of` gate
- 3 exact-head falsifiers:
  - execution head mismatch
  - absorbed head mismatch
  - round id mismatch

Regression evidence kept green:

- 27 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- all-stage H2 matrix still passes:
  - 5 stage pass cases
  - 20 shared fail cases
- H3 migrated merge-candidate scalar gate still passes
- older experiment fixtures still preserve prior behavior:
  - finding canonicalization pass
  - proof-plan adequacy pass-with-warning
  - ETH anchor fail-closed block

## "Statistically Significant" Metaphor

This is still not proof of total closure.

What makes it meaningful:

1. the migrated review rule was not tested once in isolation; it was tested as:
   - one direct pass
   - three direct falsifiers
   - plus the full existing validator regression suite
2. the migration removed bespoke review head/round matching code rather than duplicating it elsewhere
3. the broader stage-gate matrix remained green after the review gate became composite

## Result

H4 passes for this increment.

The review stage no longer needs bespoke exact-head admission checks.
That part is now expressed by the generic gate language through `all_of` + `string_eq`.

## Remaining Boundary

This does not prove full comparator closure.

Still bespoke or only partly representable:

- `claim_enforcement` coverage over true claims
- `orchestration_reachability` set/matrix checks
- `ci_surface` stage-job declaration semantics
- `review_round` completeness beyond exact-head admission
