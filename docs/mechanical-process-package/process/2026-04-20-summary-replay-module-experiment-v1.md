# Summary Replay Module Experiment v1

## Hypothesis H16

Replay producers can be made first-class code instead of hidden validator branches, without changing observed behavior.

## Validation Date

- 2026-04-20

## Treatment

1. Extract replay producer logic into a dedicated module:
   - `src/summary_replay.rs`
2. Keep the validator as an orchestrator:
   - load source artifacts
   - call producer functions
   - compare checked artifacts to recomputed outputs
3. Add direct unit tests for producer logic.

## Producer Functions

- `compute_claim_enforcement_coverage_summary`
- `compute_orchestration_reachability_summary`

## Evidence Set

Direct producer evidence:

- 3 unit tests in `summary_replay`
  - true-claim coverage counting
  - out-of-surface reachability counting
  - incomplete reachability case counting

Broader regression evidence:

- 49 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- prior H3-H15 surfaces stayed green

## "Statistically Significant" Metaphor

What is proven here:

1. replay logic is now isolated and directly testable
2. the validator no longer hides replay algorithms inside one large file
3. direct producer tests and the full validator regression suite both stayed green

## Result

H16 passes.

Replay producers are now first-class code.

## Remaining Boundary

The remaining open question is no longer structural.

It is whether the current process package and evidence are sufficient to justify calling this harness “done enough,” or whether you want one final explicit capstone artifact that states the closure boundary and the remaining assumptions.
