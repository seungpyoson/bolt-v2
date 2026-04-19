# Scalar Summary Schema Experiment v1

## Hypothesis H11

Scalar producer artifacts can be treated as a controlled first-class artifact family if they share one minimal common schema and the validator enforces that schema generically, without hardcoding specific filenames.

## Validation Date

- 2026-04-20

## Treatment

1. Define one common scalar-summary schema:
   - `status`
   - `summary_kind`
   - `summary_verdict`
   - `rule_version`
   - `source_refs`
2. Keep the detection rule generic:
   - any gate-referenced file ending in `_summary.toml` or `_coverage.toml`
3. Validate those artifacts generically before clause evaluation.
4. Standardize existing scalar artifacts to the schema:
   - `claim_enforcement_coverage.toml`
   - `orchestration_reachability_summary.toml`

## Evidence Set

Targeted H11 falsification corpus:

- 1 review fail fixture:
  - `claim_enforcement_coverage.summary_kind = ""`
- 1 review fail fixture:
  - `orchestration_reachability_summary.source_refs = []`

Broader regression evidence:

- 45 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- H3-H10 migrated surfaces stayed green
- all-stage gate matrix still passes

## "Statistically Significant" Metaphor

This is still not proof that every future summary is honest.

What is proven here:

1. scalar summaries are no longer ad hoc file shapes
2. the validator can recognize and validate them generically
3. direct schema falsifiers behave cleanly
4. earlier gate experiments remained stable after standardization

## Result

H11 passes.

The scalar-summary family is now a controlled pattern rather than a loose pile of special-case files.

## Remaining Boundary

The remaining open question is no longer about the gate language itself.

It is whether upstream producer logic should also be frozen and audited as explicit first-class process artifacts, rather than only as scalar outputs consumed by the validator.
