# Comparator Closure Experiment v2

## Hypothesis H12

The comparator family can remain explicitly closed if:

1. unknown comparator kinds fail closed
2. scalar summary artifacts used by gates must satisfy one common schema

## Validation Date

- 2026-04-20

## Treatment

1. Add explicit falsifiers for:
   - unknown top-level gate comparator
   - unknown clause comparator inside `all_of`
2. Add one generic scalar-summary schema validator for any gate-referenced file ending in:
   - `_summary.toml`
   - `_coverage.toml`
3. Require the common summary fields:
   - `status`
   - `summary_kind`
   - `summary_verdict`
   - `rule_version`
   - `source_refs`

## Evidence Set

Targeted H12 falsification corpus:

- 2 comparator-closure fail fixtures:
  - unknown gate comparator kind
  - unknown clause comparator kind
- 2 scalar-summary schema fail fixtures:
  - empty `claim_enforcement_coverage.summary_kind`
  - empty `orchestration_reachability_summary.source_refs`

Broader regression evidence:

- 47 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- H3-H11 migrated surfaces stayed green
- all-stage gate matrix still passes

## "Statistically Significant" Metaphor

This is not proof that no future comparator will ever be needed.

What is proven here:

1. the current comparator family is now explicitly defended by negative tests
2. the scalar-summary family is no longer just convention; it is validated generically
3. earlier migrations stayed green after the closure checks were added

## Result

H12 passes.

The harness now has:

- a closed comparator family in practice
- a controlled scalar-summary artifact family

## Remaining Product Question

The next question is no longer “can the validator absorb more custom logic?”

It is whether the upstream producers of scalar summaries should themselves become independently validated first-class process artifacts, or whether the current boundary is already the right stopping point.
