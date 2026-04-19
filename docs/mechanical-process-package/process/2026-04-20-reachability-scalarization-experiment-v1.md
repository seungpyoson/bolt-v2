# Reachability Scalarization Experiment v1

## Hypothesis H10

`orchestration_reachability` can stay outside the gate language as one canonical scalar summary artifact, and the gate can compare only that scalar result.

## Validation Date

- 2026-04-20

## Treatment

1. Keep the gate language closed:
   - `string_eq`
   - `scalar_eq`
   - `nonempty`
   - `all_of`
2. Do **not** add a graph or set comparator.
3. Introduce one upstream scalar artifact:
   - `orchestration_reachability_summary.toml`
4. Bind gates to that artifact through scalar clauses only:
   - `status` nonempty
   - `stage` equals active stage
   - `reachability_status == "pass"`
   - all summary counts equal `0`
5. Remove the bespoke reachability case loop from the validator.

## Evidence Set

Targeted H10 falsification corpus:

- 1 review pass fixture
- 2 review fail fixtures:
  - missing `orchestration_reachability_summary.toml`
  - `reachability_status = "block"`
- 1 merge-candidate fail fixture:
  - wrong `stage` in the summary

Broader regression evidence:

- 43 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- all earlier H3-H9 surfaces stayed green
- all-stage gate matrix still passes

## "Statistically Significant" Metaphor

This is not proof that the summary producer is closed.

What is proven here:

1. the gate language stayed closed
2. the bespoke reachability graph loop left the validator
3. direct scalar falsifiers behave cleanly
4. the broader regression suite remained green

## Result

H10 passes.

The reachability graph no longer lives in the validator gate logic.
The validator now consumes a scalarized reachability result and compares that scalar mechanically.

## Remaining Boundary

The remaining important question is no longer whether the gate language needs more power.

It is whether these scalar producer artifacts should themselves become first-class validated artifacts with one generic schema.
