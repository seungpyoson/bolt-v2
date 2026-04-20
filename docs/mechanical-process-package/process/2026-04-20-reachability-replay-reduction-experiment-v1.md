# Reachability Replay Reduction Experiment v1

## Hypothesis H15

If `orchestration_reachability_summary.toml` is reduced to only replayable fields, it can become mechanically reproducible evidence from the currently frozen source artifacts.

## Validation Date

- 2026-04-20

## Treatment

1. Remove the non-replayable field:
   - `unreachable_required_job_count`
2. Keep only the replayable fields:
   - `stage`
   - `summary_kind`
   - `summary_verdict`
   - `source_refs`
   - `rule_version`
   - `out_of_surface_required_job_count`
   - `incomplete_case_count`
   - `status`
3. Recompute the reduced summary from:
   - `orchestration_reachability.toml`
   - `ci_surface.toml`
   - selected stage

## Replay Rule

Compute:

- `incomplete_case_count`
  - count of cases with any required field empty
- `out_of_surface_required_job_count`
  - count of `required_reachable_jobs` entries outside the active stage job set

Then:

- `summary_verdict = "pass"` iff both counts are `0`
- otherwise `summary_verdict = "block"`

## Evidence Set

Targeted H15 replay corpus:

- reduced canonical summary for candidate `#205`
- replay comparison against recomputed output

Broader regression evidence:

- 49 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- earlier H3-H14 steps stayed green

## "Statistically Significant" Metaphor

What is proven here:

1. the H14 boundary was not bypassed; the non-replayable field was removed
2. the reduced summary is now replayable from current source artifacts
3. the validator checks replay rather than trusting the file on appearance
4. the broader regression surface remained green

## Result

H15 passes.

The reduced reachability summary is now mechanically reproducible evidence.

## Consequence

The remaining question is no longer whether reachability can be replayed at all.

It is whether the dropped field should be:

1. permanently excluded from the summary, or
2. reintroduced only after freezing an upstream artifact that encodes the missing workflow semantics
