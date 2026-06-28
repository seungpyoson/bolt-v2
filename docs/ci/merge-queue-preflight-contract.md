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

- Mergify queue behavior is configured in `.mergify.yml`.
- The current queue rules use `merge_method: squash`.
- Any preflight implementation must either model the configured Mergify merge
  method and queue ordering, or mark its batching prediction as advisory in the
  residual-risk lane.
- The cheap deterministic verifier set must have one source of truth. Do not
  maintain a second hand-copied list of cheap gates.

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

## Decision Model

Each lane returns one of these statuses:

- `ready`: this lane proved its bounded contract.
- `blocked`: this lane found a deterministic reason not to queue this PR or
  wave.
- `inconclusive`: the verifier could not prove readiness because required
  evidence was unavailable or degraded.
- `residual_risk`: the lane is outside preflight's proof boundary and must be
  reported to the operator.

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

## Required Input Contract

An authoritative run requires:

- expected base branch name.
- expected base SHA.
- ordered PR list.
- expected head SHA for each PR.
- selected verifier profile.
- Mergify queue rule or an explicit statement that the queue rule is advisory.

If any expected SHA is missing, the run is non-authoritative and the verdict is
`inconclusive`.

Boundary validation must classify all input failures before any fetch, metadata
request, merge simulation, or verifier command runs.

`--no-gh` is a debug mode only. It disables the readiness lane and therefore must
produce an `inconclusive` verdict, never `queue_as_one_wave`.

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

Any mismatch is `blocked`. Any unavailable identity evidence is `inconclusive`.

## Readiness Lane

The readiness lane verifies:

- PR state is open.
- PR is not draft.
- required reviewer state is approved.
- required checks are green.
- native code-owner review, stale-review dismissal, last-push approval, and
  review-thread resolution are either directly verified or reported as residual
  risk.
- approval from the repo-governed required reviewer identity is either directly
  verified or reported as residual risk.

Check states must be exhaustively classified. Unknown, skipped, missing,
action-required, startup-failure, cancelled, pending, and API-unavailable states
are not implicit success. Each maps to `blocked` or `inconclusive` through the
central classifier.

The readiness lane does not own git mergeability. Mergeability belongs to the
integration lane. GitHub's async `mergeable` field may be reported as diagnostic
metadata, but it must not be a second source of truth for merge conflicts.

## Integration Lane

The integration lane verifies:

- each PR integrates with the expected base.
- the requested PR ordering integrates under the selected Mergify queue rule.
- any recommended split is reported as `split_advised`, not as a blocker.

Because the current Mergify queue rules use squash merge, the integration lane
must either:

- emulate the effective squash queue proof context, or
- explicitly report that conflict prediction is advisory and belongs to
  residual risk.

If the implementation cannot model Mergify's actual ordering, the output must
state that recommended batches are advisory and may diverge from Mergify's
selected queue order.

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
- Mergify queue ordering if the tool used CLI order.
- Mergify squash behavior if the integration lane used an advisory merge model.
- branch protection or ruleset state not directly verified by the run.

Residual risks do not make a successful run fail. They define the boundary of the
claim the operator may make.

## Diagnostic Policy

Diagnostics must be useful without violating the no-credential-display rule.

- Do not emit successful verifier streams.
- Bound failed streams by configured line and byte limits.
- Redact known token, key, and secret patterns before display.
- Treat a verifier profile that can print secrets as invalid for prequeue use.
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
- expected base SHA mismatch.
- expected PR head SHA mismatch.
- no use of `FETCH_HEAD` or another shared mutable fetch result.
- metadata unavailable.
- `--no-gh` returns `inconclusive`.
- open/draft/review/base/head readiness states.
- required check pass, fail, pending, skipped, missing, and unknown buckets.
- base conflict.
- clean split into multiple advised batches.
- verifier failure.
- verifier timeout.
- verifier executable missing.
- bounded and redacted verifier diagnostics.
- residual-risk output in JSON and plain text.
- Mergify squash/advisory merge-mode reporting.

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
