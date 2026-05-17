# Contract: Phase 9 Audit Evidence

## Claim Rules

Every Phase 9 claim must cite at least one of:

- file path and line number from current main or this Phase 9 branch
- command output captured in this session
- test output captured on exact head
- PR metadata from GitHub
- external reviewer job record

Memory, old branch content, and reviewer prose without current-source trace are not sufficient proof.

## Severity Rules

- `blocker`: prevents implementation, review progression, live action, or readiness recommendation.
- `high`: likely incorrect architecture or safety behavior, but not necessarily blocking docs-only planning.
- `medium`: stale artifact, missing verifier, or cleanup candidate requiring later bounded work.
- `low`: clarity or documentation quality issue with no runtime effect.

## Recommendation Rules

The final recommendation must be exactly one of:

- `ready for no-submit only`
- `ready for tiny live order approval`
- `blocked with exact blockers`
- `stop`

## Cleanup Rules

Cleanup may not start unless:

- exact cleanup scope is named
- one behavior test or source fence is written first
- failing output is captured before implementation
- green output is captured after implementation
- external review blockers are resolved
- user approves the implementation step when GitHub or live side effects are involved
