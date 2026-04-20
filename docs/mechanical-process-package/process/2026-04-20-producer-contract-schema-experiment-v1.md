# Producer Contract Schema Experiment v1

## Hypothesis H18

Producer contracts can be treated as a controlled first-class artifact family if they share one minimal common wrapper schema and the validator enforces that schema generically.

## Validation Date

- 2026-04-20

## Treatment

1. Standardize `workflow_reachability_contract.toml` with a common wrapper:
   - `artifact_kind`
   - `contract_kind`
   - `subject`
   - `contract_version`
   - `status`
   - `source_artifacts`
2. Keep domain-specific payload separate:
   - `[[reachability]]`
   - `trigger_job`
   - `trigger_result`
   - `reachable_jobs`
3. Add generic validator checks for producer contracts before replay uses them.

## Evidence Set

Targeted H18 falsification corpus:

- 1 review fail fixture:
  - `workflow_reachability_contract.artifact_kind = ""`
- 1 review fail fixture:
  - `workflow_reachability_contract.source_artifacts = []`

Broader regression evidence:

- 4 `summary_replay` unit tests passed
- 53 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- earlier H3-H17 surfaces stayed green

## "Statistically Significant" Metaphor

What is proven here:

1. producer contracts are no longer opaque replay inputs
2. the validator can enforce one common wrapper schema generically
3. direct schema falsifiers fail cleanly
4. replay and gate regressions remain green after the wrapper standardization

## Result

H18 passes.

The producer-contract family is now controlled in the same way scalar summaries are controlled:

- common wrapper schema
- generic validator checks
- domain-specific payload kept separate

## Remaining Boundary

The remaining question is no longer artifact-family shape.

It is whether you want a final capstone artifact that states:

- what is now mechanically guaranteed
- what still depends on frozen upstream human choices
- and what the stopping boundary is
