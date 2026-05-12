# Contract: Review Gates

## Before Implementation

- Spec and plan exist.
- Current-state evidence names exact files and line references.
- no-mistakes status and recent runs are captured with `/private/tmp/no-mistakes-soak-bin`.
- Implementation task has an explicit failing test command.

## Before Each Code Commit

- Failing test was observed for the intended behavior.
- Minimal code change made the test pass.
- Relevant targeted test passes.
- `superpowers:verification-before-completion` gate is applied.

## Before External Review

- Working tree clean.
- Branch pushed.
- Exact PR head CI green.
- no-mistakes run status captured.
- Known findings resolved or explicitly waived by user.

## Before Merge

- User explicitly approves merge.
- Final head SHA recorded.
- Residual blockers are named in PR body.
- No claim says the PR proves broader live readiness unless the artifact proves it.
