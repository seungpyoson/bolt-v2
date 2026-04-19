# Completeness Gate Experiment v1

## Hypothesis H6

`execution_target` and `ci_surface` core-field completeness can be expressed through the existing gate language, without adding a new comparator beyond `nonempty`.

## Validation Date

- 2026-04-20

## Treatment

1. Keep the current generic gate set:
   - `string_eq`
   - `scalar_eq`
   - `nonempty`
   - `all_of`
2. Remove bespoke review/merge completeness checks for:
   - `execution_target.toml`
   - `ci_surface.toml`
3. Re-bind those checks into gate clauses:
   - nonempty `execution_target` fields
   - nonempty `ci_surface` core fields
   - nonempty active-stage job list in `required_jobs_by_stage`

## Evidence Set

Targeted H6 falsification corpus:

- 1 review pass fixture:
  - synthetic review package with completeness clauses in the gate
- 3 review fail fixtures:
  - `execution_target.repo = ""`
  - `ci_surface.workflow = ""`
  - `required_jobs_by_stage.review = []`
- 2 merge-candidate fail fixtures:
  - `execution_target.repo = ""`
  - `required_jobs_by_stage.merge_candidate = []`

Broader regression evidence:

- 33 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- all-stage gate matrix still passes
- H3 merge-candidate scalar gate still passes
- H4 review exact-head `all_of` gate still passes
- old experiment fixtures preserve their prior outcomes

## "Statistically Significant" Metaphor

This is still not total closure.

What makes the evidence meaningful:

1. the migration did not add a new comparator type
2. the same `nonempty` operator was reused across review and merge-candidate
3. direct falsifiers hit both scalar metadata and stage-job-list completeness
4. the broader regression surface stayed green after the bespoke completeness branches were removed

## Result

H6 passes for this increment.

The completeness slice for `execution_target` and `ci_surface` now lives in generic gates rather than bespoke validator code.

## Remaining Bespoke Boundary

Still outside the closed comparator set:

- `claim_enforcement` coverage over true claims
- `orchestration_reachability` set/matrix coverage
- broader `review_round` completeness beyond exact-head admission
