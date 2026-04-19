# Review Round Completeness Experiment v1

## Hypothesis H7

`review_round` completeness can be expressed through the existing gate language using `nonempty`, without a bespoke validator loop over round fields.

## Validation Date

- 2026-04-20

## Treatment

1. Keep the current comparator set:
   - `string_eq`
   - `scalar_eq`
   - `nonempty`
   - `all_of`
2. Remove the bespoke `review_round` completeness loop from the validator.
3. Re-bind round completeness into the review gate:
   - `review_rounds/...#source` is nonempty
   - `review_rounds/...#review_target_ref` is nonempty
   - `review_rounds/...#raw_comment_refs` is nonempty
   - `review_rounds/...#status` is nonempty

## Evidence Set

Targeted H7 falsification corpus:

- 1 review pass fixture:
  - synthetic review package with nonempty round clauses
- 4 review fail fixtures:
  - `source = ""`
  - `review_target_ref = ""`
  - `raw_comment_refs = []`
  - `status = ""`

Broader regression evidence:

- 37 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- all-stage gate matrix still passes
- H3/H4/H5/H6 migrated surfaces still pass
- older process fixtures preserved prior pass/fail behavior

## "Statistically Significant" Metaphor

This is still not full closure.

What makes the evidence meaningful:

1. the migration reused the existing gate language with no new comparator
2. every previously bespoke `review_round` completeness field got its own direct falsifier
3. the broader regression suite remained green after removing the custom branch

## Result

H7 passes for this increment.

`review_round` completeness is now enforced by gate clauses instead of bespoke review-stage Rust logic.

## Remaining Bespoke Boundary

Still outside the closed comparator set:

- `claim_enforcement` coverage over true claims
- `orchestration_reachability` set/matrix coverage
