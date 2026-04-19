# CI Surface Comparator Experiment v1

## Hypothesis H5

`ci_surface` stage admission can be expressed through generic gate comparators, without bespoke validator branches for:

- exact-head equality between `ci_surface` and `execution_target`
- active-stage job declaration presence in `required_jobs_by_stage`

## Validation Date

- 2026-04-20

## Treatment

1. Extend gate evaluation with one new generic comparator:
   - `nonempty`
2. Keep existing generic comparators:
   - `string_eq`
   - `scalar_eq`
   - `all_of`
3. Remove bespoke `ci_surface` admission checks from the validator:
   - `ci_surface.head_sha == execution_target.head_sha`
   - `required_jobs_by_stage[stage]` must exist and be non-empty
4. Re-bind those checks into gates:
   - review gate
   - merge-candidate gate

## Evidence Set

Targeted H5 falsification corpus:

- 1 review pass fixture:
  - synthetic review package with `ci_surface` clauses in `all_of`
- 3 review fail fixtures:
  - `ci_surface.head_sha` mismatch
  - `required_jobs_by_stage.review = []`
  - execution head mismatch still fails through the same composed gate
- 1 merge-candidate fail fixture:
  - `required_jobs_by_stage.merge_candidate = []`

Broader regression evidence:

- 30 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- all-stage gate matrix still passes:
  - 5 pass cases
  - 20 shared fail cases
- H3 merge-candidate scalar gate still passes
- H4 review exact-head gate still passes
- old process fixtures preserve their prior outcomes

## "Statistically Significant" Metaphor

This is not proof of total closure.

What makes the evidence meaningful:

1. the migrated rule was replayed through both review and merge-candidate paths
2. it was exercised by direct falsifiers, not only by one happy-path package
3. the validator no longer has a bespoke `ci_surface` admission branch for those two checks
4. the previous H2/H3/H4 regression surfaces remained green after the migration

## Result

H5 passes for this increment.

The exact-head and active-stage-job admission parts of `ci_surface` are now in the generic gate language.

## Remaining Bespoke Boundary

Still outside the closed comparator set:

- `execution_target.toml` completeness
- `ci_surface.toml` core-field completeness
- `claim_enforcement` coverage over true claims
- `orchestration_reachability` set/matrix coverage
- broader `review_round` completeness beyond exact-head admission
