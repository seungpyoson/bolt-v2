# Full Reachability Replay Experiment v1

## Hypothesis H17

If the missing workflow reachability semantics are frozen upstream in one contract artifact, the full `orchestration_reachability_summary.toml` can become mechanically replayable again, including `unreachable_required_job_count`.

## Validation Date

- 2026-04-20

## Treatment

1. Add one upstream artifact:
   - `workflow_reachability_contract.toml`
2. Freeze only the missing semantics:
   - `trigger_job`
   - `trigger_result`
   - `reachable_jobs`
3. Restore the full reachability summary field:
   - `unreachable_required_job_count`
4. Replay `orchestration_reachability_summary.toml` from:
   - `orchestration_reachability.toml`
   - `ci_surface.toml`
   - `workflow_reachability_contract.toml`

## Evidence Set

Targeted H17 replay corpus:

- producer unit test:
  - counts unreachable required jobs from the contract
- 1 review fail fixture:
  - missing `workflow_reachability_contract.toml`
- 1 review fail fixture:
  - drifted `unreachable_required_job_count`
- existing reachability scalar falsifiers remained green

Broader regression evidence:

- 4 `summary_replay` unit tests passed
- 51 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- earlier H3-H16 surfaces stayed green

## "Statistically Significant" Metaphor

What is proven here:

1. the H14 boundary was resolved by freezing the missing upstream semantics, not by making the validator smarter
2. the full reachability summary is replayable again
3. direct replay drift falsifiers fail cleanly
4. the broader regression surface remained green

## Result

H17 passes.

The full reachability summary is now mechanically reproducible evidence.

## Remaining Question

The remaining product question is whether producer contracts like:

- `workflow_reachability_contract.toml`

should themselves become a standardized validated artifact family, or whether the current level of explicitness is already the right stopping point.
