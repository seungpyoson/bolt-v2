# Claim Enforcement Closure Boundary v1

## Hypothesis H8

`claim_enforcement` coverage over true merge claims can be expressed cleanly inside the current gate model without introducing an ad hoc comparator.

## Validation Date

- 2026-04-20

## Treatment

Pressure-test the smallest possible generic relation comparator for claim coverage.

Candidate shape:

- iterate `merge_claims.claims`
- filter to `value = true`
- join on `claim_id`
- require matching `claim_enforcement.rows`
- require nonempty enforcement fields

## Evidence

Current bespoke validator logic:

- [src/delivery_validator.rs](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-process-validator/src/delivery_validator.rs:903)
  through
  [src/delivery_validator.rs](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-process-validator/src/delivery_validator.rs:975)

Current artifact shapes:

- [merge_claims.toml](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-process-validator/docs/mechanical-process-package/candidate-205-smoke-tag-ci/merge_claims.toml:1)
- [claim_enforcement.toml](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-process-validator/docs/mechanical-process-package/candidate-205-smoke-tag-ci/claim_enforcement.toml:1)

What the comparator would need to encode:

1. filter left rows by `value = true`
2. join left and right rows by `claim_id`
3. require at least one matching right row
4. require `enforcement_kind`, `enforced_at`, and `status` to be nonempty on that row

## Stop Rule

Stop immediately if the gate language needs collection filtering or joins across two row sets.

Concrete trigger phrases:

- "for each true claim"
- "matching claim_id row"
- "exists enforcement row"
- "all true claims covered"

Once those appear, the comparator layer is no longer a scalar exam.
It has crossed into bespoke policy logic.

## Result

H8 fails.

The proposed comparator would not be a clean extension of:

- `string_eq`
- `scalar_eq`
- `nonempty`
- `all_of`

It would introduce quantified join semantics.

That is the first clear closure boundary.

## "Statistically Significant" Metaphor

This is not runtime corpus evidence.
It is structural falsification evidence.

Why it is still meaningful:

1. the candidate rule was reduced to its minimal generic form
2. that minimal form still required filtering + joining + row coverage semantics
3. those semantics are qualitatively different from all previously accepted gate comparators

## Consequence

Do not add a `forall_join_exists` style comparator.

If claim coverage must become gate-owned later, the next experiment should be upstream:

- produce one scalar canonical coverage artifact
- compare that scalar in the gate

That keeps the gate language closed.
