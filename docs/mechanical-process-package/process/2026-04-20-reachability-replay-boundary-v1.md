# Reachability Replay Boundary v1

## Hypothesis H14

`orchestration_reachability_summary.toml` can be replayed honestly from the currently frozen upstream source artifacts.

## Validation Date

- 2026-04-20

## Source Artifacts Examined

- `orchestration_reachability.toml`
- `ci_surface.toml`

## Result

H14 fails.

## Why It Fails

The current source artifacts do not encode enough semantics to replay:

- `unreachable_required_job_count`

What is available:

- declared cases
- trigger job/result
- required reachable jobs
- forbidden job results
- stage job set from `ci_surface.toml`

What is missing:

- the actual workflow dependency graph
- job-to-job reachability semantics
- skip/fail propagation semantics at the artifact level

So:

- `out_of_surface_required_job_count` is replayable as set membership
- `incomplete_case_count` is replayable as schema completeness
- `unreachable_required_job_count` is **not** replayable honestly from the current artifacts

## "Statistically Significant" Metaphor

This is a structural falsification result.

The evidence is the artifact shape itself:

1. the required output field exists
2. the declared source artifacts do not contain enough information to derive it
3. any replay would therefore invent semantics not frozen in the process package

## Consequence

Do not claim `orchestration_reachability_summary.toml` is fully replayable from the current source artifacts.

Two honest options exist:

1. shrink the summary to only fields that are replayable from current artifacts
2. add a new upstream artifact that freezes the missing workflow reachability semantics

Until one of those happens, full replay of reachability summary is a hard boundary.
