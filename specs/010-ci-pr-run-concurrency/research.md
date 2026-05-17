# Research: CI PR Run Concurrency

## Decision: Use top-level workflow concurrency, not job-level concurrency

**Rationale**: #355 owns obsolete PR-head CI runs. Top-level concurrency cancels the prior workflow run before expensive jobs continue. Job-level concurrency would leave partial workflow state and more checks UI noise.

**Alternatives considered**:

- Job-level concurrency: rejected because it is easier to drift per job and weaker as a single workflow policy.
- No concurrency, only manual cancellation: rejected because it does not reduce Actions minutes automatically.

## Decision: Key PR runs by PR number

**Rationale**: `format('pr-{0}', github.event.number)` ties cancellation to one PR. It avoids treating two pushes to the same PR as independent useful evidence while avoiding branch-name ambiguity.

**Alternatives considered**:

- Branch name: rejected because names can collide across forks or stacked branch conventions.
- Head SHA: rejected for PR runs because every pushed head would get a unique group and nothing would cancel.

## Decision: Key non-PR runs by ref name plus SHA and disable cancellation

**Rationale**: Main/tag/deploy paths are evidence-bearing. `format('{0}-{1}', github.ref_name, github.sha)` keeps each non-PR run distinct, and `cancel-in-progress: ${{ github.event_name == 'pull_request' }}` prevents accidental cancellation of main/tag evidence.

**Alternatives considered**:

- Ref-only non-PR group: rejected because consecutive main/tag runs could cancel each other.
- `cancel-in-progress: true`: rejected because it can cancel non-PR evidence paths.

## Decision: Add standard-library verifier coverage

**Rationale**: The workflow already has the intended policy, but #355 acceptance needs drift prevention. Existing verifier style is line-based Python without YAML dependencies, so the root solution should extend that path.

**Alternatives considered**:

- Add a YAML parser: rejected because the repository verifier intentionally avoids new dependencies.
- Rely on review only: rejected because it does not fail closed on future drift.
