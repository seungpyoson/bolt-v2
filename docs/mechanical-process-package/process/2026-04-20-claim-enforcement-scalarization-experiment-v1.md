# Claim Enforcement Scalarization Experiment v1

## Hypothesis H9

The `claim_enforcement` coverage boundary can stay outside the gate language if it is pushed upstream into one canonical scalar artifact, and the gate compares only that scalar result.

## Validation Date

- 2026-04-20

## Treatment

1. Keep the gate language closed:
   - `string_eq`
   - `scalar_eq`
   - `nonempty`
   - `all_of`
2. Do **not** add a quantified join comparator.
3. Introduce one upstream scalar artifact:
   - `claim_enforcement_coverage.toml`
4. Bind gates to that artifact only through scalar/nonempty clauses:
   - `coverage_verdict == "pass"`
   - `status` is nonempty
5. Remove the bespoke claim-coverage join loop from the validator.

## Evidence Set

Targeted scalarization corpus:

- 1 review pass fixture
- 2 review fail fixtures:
  - `claim_enforcement_coverage.toml` missing
  - `claim_enforcement_coverage.status = ""`
- 1 merge-candidate fail fixture:
  - `claim_enforcement_coverage.coverage_verdict = "block"`

Broader regression evidence:

- 40 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- all-stage gate matrix still passes
- H3-H7 migrated surfaces stayed green
- the H8 boundary note remains valid: the join itself never entered the gate language

## "Statistically Significant" Metaphor

This is not proof that the upstream scalar producer is itself closed.

What is proven here:

1. the gate language stayed closed
2. the bespoke join loop was removed from the validator
3. targeted falsifiers on the scalar artifact behave cleanly
4. the wider regression surface remained green

## Result

H9 passes.

The `claim_enforcement` join is no longer inside the validator gate logic.
Instead, the validator consumes a scalarized upstream artifact and compares that scalar mechanically.

## Remaining Bespoke Boundary

The largest remaining bespoke branch is now:

- `orchestration_reachability` set/matrix coverage

That is the first obviously graph-shaped branch left in the validator.
