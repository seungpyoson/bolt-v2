# Merge Queue Preflight Contract

## Purpose

The merge queue preflight is a prequeue verifier for Mergify waves. It decides
whether a selected set of PRs is safe to spend Mergify proof time on, whether the
wave should be split, or whether the result is inconclusive.

It does not replace Mergify, branch protection, required review, full CI, or the
Mergify proof PR. Its job is to fail closed on deterministic, cheap, prequeue
problems before a bad PR wastes a Mergify batch.

## Status

This document is the contract for reworking the preflight tool. A preflight
implementation is not authoritative until its output and tests satisfy this
contract.

## Source Of Truth

- Mergify queue behavior is configured in `.mergify.yml` and must be read from
  that file at runtime.
- Queue rules, queue conditions, merge conditions, priority rules, batch-size
  bounds, merge method, reset behavior, and max parallel checks must not be
  hardcoded or mirrored in preflight config.
- Any preflight implementation must model the configured Mergify queue rule,
  merge method, batch-size bounds, and queue routing before emitting an
  authoritative `queue_as_one_wave` or `split_advised` verdict.
- If a Mergify setting is missing, invalid, unsupported, changed after snapshot,
  or ambiguous, the Mergify config lane is `inconclusive`.
- The cheap deterministic verifier set must have one source of truth. Do not
  maintain a second hand-copied list of cheap gates.
- If the current cheap-lane registry cannot map labels to preflight verifier
  commands without ambiguity, extend the registry instead of copying values into
  the preflight implementation.

## Invariants

Every preflight result must be:

- exact-head: all conclusions bind to explicit base and PR head SHAs.
- isolated: no shared mutable git state, including `FETCH_HEAD`.
- bounded: external operations have output limits and wall-clock timeouts.
- fail-closed: unavailable metadata, unknown states, and tool failures never
  become implicit readiness.
- centrally classified: every decision uses the same typed status model.
- explicit about residual risk: the output states what preflight did not prove.

## No Silent Fallback Policy

Preflight must not continue by guessing, substituting defaults, widening scope,
or taking alternate paths that are not explicitly defined by this contract.

The implementation must treat missing, empty, invalid, stale, unavailable,
timed-out, and ambiguous inputs as contract failures unless this document assigns
a different status. These cases are distinct and must not collapse into a
generic falsey value.

Disallowed behavior:

- defaulting missing inputs to branch tips.
- treating missing lists as empty lists.
- accepting empty values as absent values.
- broad optional parsing that skips malformed fields.
- skipped, unknown, or missing checks counted as success.
- retrying through a different source without reporting degraded evidence.
- best-effort continuation after a required lane input fails validation.

Required pattern:

- validate inputs at the boundary.
- classify failures through the central lane/decision model.
- fail closed with a precise diagnostic.
- keep policy in one contract, lane, or table.
- distinguish absent, empty, invalid, stale, unavailable, timeout, and ambiguous.
- add negative tests for each failure class introduced by an implementation.

If implementation appears to need fallback behavior, stop and update this
contract before adding that behavior.

## Input Failure Matrix

Boundary validation must classify all required inputs before any fetch, metadata
request, merge simulation, or verifier command runs.

| Failure class | Definition | Status |
| --- | --- | --- |
| absent | Required field or evidence is not present | `inconclusive` |
| empty | Required field is present but empty | preflight usage error |
| invalid | User-supplied required field is malformed or unsupported | preflight usage error |
| stale base | Expected base SHA differs from the live base branch | `inconclusive` |
| stale head | Expected PR head differs from live PR ref or metadata | `blocked` |
| unavailable | Required API, ref, config, or metadata cannot be read | `inconclusive` |
| timeout | Required operation exceeded its configured wall-clock budget | `inconclusive` |
| ambiguous | More than one valid interpretation exists | `inconclusive` |

## Decision Model

Each lane returns one of these statuses:

- `ready`: this lane proved its bounded contract.
- `blocked`: this lane found a deterministic reason not to queue this PR or
  wave.
- `inconclusive`: the verifier could not prove readiness because required
  evidence was unavailable or degraded.
- `residual_risk`: the lane is outside preflight's proof boundary and must be
  reported to the operator.

Every lane finding must use a stable structured shape:

- `lane`: one of `mergify_config`, `identity`, `readiness`, `integration`,
  `verifier`, `diagnostic`, or `residual_risk`.
- `scope`: `run`, `queue`, `pr`, `batch`, or `wave`.
- `status`: `ready`, `blocked`, `inconclusive`, or `residual_risk`.
- `reason_code`: stable machine-readable classifier.
- `message`: bounded human-readable diagnostic.
- `evidence`: structured metadata needed to reproduce the classification.

The top-level verdict must be one of:

- `queue_as_one_wave`: all PRs and the requested wave are ready under the
  configured preflight contract.
- `split_advised`: each included PR is individually ready, but the requested wave
  should be split into smaller batches.
- `blocked`: one or more PRs or the wave has a deterministic blocker.
- `inconclusive`: required evidence was unavailable, degraded, timed out, or the
  run used a non-authoritative mode.

Exit codes must distinguish these outcomes:

- `0`: `queue_as_one_wave`
- `1`: `split_advised`
- `2`: `blocked`
- `3`: `inconclusive`
- `4`: preflight usage or internal tool error

Verdict reduction is centralized and ordered:

| Condition | Verdict | Operator action |
| --- | --- | --- |
| Preflight usage or internal tool error | none, exit `4` | fix invocation or tool |
| Any PR or wave finding is `blocked` | `blocked` | fix or remove blocked PRs |
| Any required lane is `inconclusive` | `inconclusive` | re-pin, re-run, or investigate |
| All lanes ready, but one-wave constraints fail | `split_advised` | queue compatible subsets separately |
| All required lanes are ready for one Mergify queue wave | `queue_as_one_wave` | queue the selected wave |

`blocked` takes precedence over `inconclusive`; the output still reports all
inconclusive findings. `split_advised` is a non-zero success-with-action, not a
tool failure. Callers must switch on the specific exit code rather than treating
only exit `0` as useful.

Residual-risk findings never repair missing required evidence. They can coexist
with `queue_as_one_wave` only when every required lane is `ready` and the risk is
outside preflight's proof boundary.

## Required Input Contract

An authoritative run requires:

- expected base branch name.
- expected base SHA.
- ordered PR list.
- expected head SHA for each PR.
- selected verifier profile.
- Mergify config snapshot identity.
- selected Mergify queue rule, or enough PR metadata to route every PR to a
  queue rule from `.mergify.yml`.

The Mergify config snapshot identity is the `.mergify.yml` blob SHA or a content
hash taken before evaluation. If the file changes before completion, the
Mergify config lane is `inconclusive`.

If any expected SHA is missing, the run is non-authoritative and the verdict is
`inconclusive`.

`--no-gh` is a debug mode only. It disables the readiness lane and therefore must
produce an `inconclusive` verdict, never `queue_as_one_wave`. It also disables
the GitHub metadata cross-checks in the identity lane.

## Mergify Config Lane

The Mergify config lane verifies:

- `.mergify.yml` is present, valid, and matches the expected config snapshot.
- every selected PR routes to exactly one supported queue rule.
- mixed queue-rule waves are not treated as one wave.
- queue priority and queue conditions are modeled or the run is
  `inconclusive`.
- the selected queue rule's merge method is supported.
- the selected queue rule's batch-size minimum and maximum are modeled.
- merge conditions identify the authoritative Mergify proof checks.

If a PR matches multiple queue rules and the implementation cannot model
Mergify's selection semantics, the result is `inconclusive`. If selected PRs
route to different queue rules, the result is at most `split_advised`.

The required-check source for Mergify readiness is the selected queue rule's
`merge_conditions`. GitHub branch-protection required checks are additional
governance evidence when available, but they are not a substitute for Mergify's
merge conditions.

## Isolation Lane

The implementation must avoid process-global git refs and shared mutable state.

Allowed approaches:

- fetch into a per-run private ref namespace and delete those refs in cleanup.
- use a temporary clone dedicated to one preflight run.

Disallowed approaches:

- reading `FETCH_HEAD`.
- relying on branch tips after expected SHAs were selected.
- using verifier commands that recursively invoke the preflight tool.
- using verifier profiles that acquire broad shared lane locks unless the timeout
  budget and residual-risk output explicitly account for that lock.

Cleanup must be best-effort and bounded. Cleanup failure is reported as
`inconclusive` when it prevents the tool from proving that the evaluated evidence
remained isolated.

## Identity Lane

The identity lane verifies:

- configured base branch resolves to the expected base SHA.
- every PR ref resolves to the expected PR head SHA.
- GitHub metadata head SHA matches the expected PR head SHA.
- GitHub metadata base branch matches the expected base branch.

Head mismatches are `blocked`. Base mismatches are `inconclusive` and require a
new base pin. Unavailable identity evidence is `inconclusive`.

## Readiness Lane

The readiness lane verifies:

- PR state is open.
- PR is not draft.
- required reviewer state is approved.
- required reviewer identity from the selected Mergify queue rule is approved.
- required Mergify merge-condition checks are not already failed on the PR head.
- native code-owner review, stale-review dismissal, last-push approval, and
  review-thread resolution are directly verified when the GitHub API exposes
  them to the preflight run.

Check states must be exhaustively classified. Unknown, skipped, missing,
action-required, startup-failure, cancelled, pending, and API-unavailable states
are not implicit success. Each maps to `blocked` or `inconclusive` through the
central classifier.

Required, GitHub-queryable merge gates cannot be downgraded to residual risk.
Unavailable required-gate evidence is `inconclusive`; proven failed required
evidence is `blocked`.

If a required governance gate is not exposed to the preflight run, the readiness
lane is `inconclusive`. Residual risk is reserved for non-required or
out-of-boundary evidence.

Green in-place PR checks are negative evidence only: they show the PR head is not
already failed. They do not prove the Mergify proof context will pass. The proof
context rerun remains residual risk.

The readiness lane does not own git mergeability. Mergeability belongs to the
integration lane. GitHub's async `mergeable` field may be reported as diagnostic
metadata, but it must not be a second source of truth for merge conflicts.

## Integration Lane

The integration lane verifies:

- each PR integrates with the expected base.
- selected PRs route to one queue rule before any one-wave verdict.
- the selected PR count satisfies the queue rule's batch-size bounds.
- the selected PR set integrates under the queue rule's merge method.
- cross-PR verifier behavior is checked for the combined candidate.
- any recommended split is reported as `split_advised`, not as a blocker.

The integration lane reports set cleanliness and conflicting-subset membership.
It must not over-claim that Mergify will choose a particular queue order unless
the implementation models Mergify ordering from `.mergify.yml` and live queue
state.

If the queue merge method is unsupported or only advisory, the integration lane
is `inconclusive`; `queue_as_one_wave` is forbidden.

A selected wave larger than the queue rule's maximum batch size is
`split_advised` when all PRs are otherwise ready. A selected wave below the queue
rule's minimum batch size is `inconclusive` unless the implementation models the
queue rule's wait-time behavior.

## Verifier Lane

Verifier profiles are named contracts, not arbitrary shell escape hatches.

Each verifier command must have:

- configured timeout.
- bounded stdout and stderr diagnostics.
- classified execution failure.
- classified timeout failure.
- deterministic input scope bound to the synthetic candidate being checked.

Successful verifier output is suppressed. Failed verifier output is bounded and
passes through the repository diagnostic redaction policy before display.
Verifier command names may be emitted as audit metadata.

Verifier profiles must not be maintained as a second copy of the cheap gate set.
If the default profile claims to catch cheap deterministic Mergify waste, it must
cover the full cheap deterministic gate set or narrow its name and description.

## Residual-Risk Lane

Every output, JSON and plain text, must include residual risks that preflight did
not prove. At minimum:

- full CI result.
- Mergify proof PR behavior.
- remote runner availability and environment.
- flaky checks and external services.
- base or PR head drift after preflight.
- live Mergify queue ordering changing after preflight.
- reset-on-external-merge invalidation after preflight.
- max-parallel-checks cost amplification for poisoned batches.
- branch protection or ruleset state only when not exposed to the preflight run.

Residual risks do not make a successful run fail. They define the boundary of the
claim the operator may make.

## Diagnostic Policy

Diagnostics must be useful without violating the no-credential-display rule.

- Do not emit successful verifier streams.
- Bound failed streams by configured line and byte limits.
- Redact known token, key, and secret patterns before display.
- Reject verifier profile command strings that contain secrets or secret-shaped
  arguments.
- Include enough structured metadata to identify the failing lane, classifier,
  PR number, batch, command, and timeout without exposing raw logs.

## Evidence Required Before Merge

An implementation of this contract must include tests for:

- absent required input.
- empty required input.
- invalid required input.
- stale required input.
- unavailable required input.
- timed-out required input.
- ambiguous required input.
- verdict precedence when blocked and inconclusive findings coexist.
- residual-risk findings do not repair missing required evidence.
- expected base SHA mismatch.
- expected PR head SHA mismatch.
- base drift is `inconclusive`.
- head drift is `blocked`.
- Mergify config missing, invalid, unsupported, and changed after snapshot.
- queue-rule routing for default, hotfix, mixed-queue, and ambiguous waves.
- selected wave larger than queue batch-size maximum.
- selected wave smaller than queue batch-size minimum.
- no use of `FETCH_HEAD` or another shared mutable fetch result.
- concurrent preflight runs sharing one clone.
- metadata unavailable.
- `--no-gh` returns `inconclusive`.
- open, closed, draft, review-approved, and review-not-approved states.
- required reviewer identity absent, unavailable, and not approved.
- required check pass, fail, pending, skipped, neutral, missing, duplicate,
  stale-to-old-SHA, action-required, startup-failure, and unknown buckets.
- base conflict.
- clean split into multiple advised batches.
- verifier failure.
- batch-level verifier failure.
- verifier timeout.
- verifier executable missing.
- verifier command string containing secret-shaped arguments.
- bounded and redacted verifier diagnostics.
- residual-risk output in JSON and plain text.
- unsupported Mergify merge method prevents `queue_as_one_wave`.

Completion evidence requires:

- targeted unit tests for the preflight tool.
- Python syntax validation for changed scripts.
- targeted text/static checks for this document.
- `just ci-lint-workflow`.
- `just source-fence-static`.
- internal adversarial review after local findings are resolved.
- an implementation-branch audit that lists every new `if`, `match`, `except`,
  `unwrap_or`, `unwrap_or_default`, `or_else`, and default branch, with a short
  explanation of why each branch is validation or classification rather than
  silent fallback.
