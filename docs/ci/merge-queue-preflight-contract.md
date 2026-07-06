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
- The authoritative `.mergify.yml` is the blob at the expected base SHA. The
  preflight must not read Mergify config from the local worktree, PR head, or a
  synthetic candidate.
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
- Mergify merge-condition check production is sourced from the CI workflow
  definitions or a machine-readable registry generated from or validated against
  those workflows at the expected base SHA. The mapping must not be inferred
  from PR-head absence, check-name suffixes, or comments alone.

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
| absent input | Required user-supplied input is not present | preflight usage error |
| absent evidence | Required external evidence is not present | `inconclusive` |
| empty input | Required user-supplied input is present but empty | preflight usage error |
| invalid | User-supplied required field is malformed or unsupported | preflight usage error |
| stale base | Expected base SHA differs from the live base branch | `inconclusive` |
| stale head | Expected PR head differs from live PR ref or metadata | `blocked` |
| unavailable | Required API, ref, config, or metadata cannot be read | `inconclusive` |
| timeout | Required operation exceeded its configured wall-clock budget | `inconclusive` |
| ambiguous | More than one valid interpretation exists | `inconclusive` |

## Decision Model

Required lanes are:

- `mergify_config`
- `identity`
- `readiness`
- `integration`
- `verifier`

Each required lane reduces to one terminal status:

- `ready`: this lane proved its bounded contract.
- `blocked`: this lane found a deterministic reason not to queue this PR or
  wave.
- `inconclusive`: the verifier could not prove readiness because required
  evidence was unavailable or degraded.

Residual risk is a finding annotation, not a terminal status for required lanes.
Diagnostic and residual-risk findings do not gate the verdict.

Every lane finding must use a stable structured shape:

- `lane`: one of `mergify_config`, `identity`, `readiness`, `integration`,
  `verifier`, `diagnostic`, or `residual_risk`.
- `scope`: `run`, `queue`, `pr`, `batch`, or `wave`.
- `status`: `ready`, `blocked`, `inconclusive`, or `residual_risk`.
- `reason_code`: stable machine-readable classifier.
- `message`: bounded human-readable diagnostic.
- `evidence`: structured metadata needed to reproduce the classification.

Lane status reduction is:

- any non-residual finding with status `blocked` makes the lane `blocked`.
- otherwise, any non-residual finding with status `inconclusive` makes the lane
  `inconclusive`.
- otherwise, the lane is `ready` only when every required assertion for that
  lane emitted ready evidence.
- a lane with only residual-risk findings is not ready; missing ready evidence is
  `inconclusive`.

The top-level verdict must be one of:

- `queue_as_one_wave`: all PRs have ready prequeue metadata and the requested
  wave's emitted batches are ready under the configured preflight contract.
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
| Any required lane is `blocked` | `blocked` | fix or remove blocked PRs |
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

The Mergify config snapshot identity is the `.mergify.yml` blob SHA at the
expected base SHA. Before completion, preflight must verify that the live base
branch still resolves to the expected base SHA; otherwise the identity lane is
`inconclusive` and the config snapshot must be re-pinned.

If any expected SHA is absent or empty, preflight exits with usage error. A
non-authoritative report mode requires an explicit future contract entry; it
must not be inferred from missing inputs.

`--no-gh` is a debug mode only. It disables the readiness lane and therefore must
produce `blocked` when a blocker is detected, otherwise `inconclusive`. It must
never produce `queue_as_one_wave`. It also disables the GitHub metadata
cross-checks in the identity lane.

## Mergify Config Lane

The Mergify config lane verifies:

- `.mergify.yml` is present, valid, and matches the expected config snapshot.
- every selected PR routes to exactly one effective supported queue rule.
- mixed queue-rule waves are not treated as one wave.
- queue conditions and priority rules used for effective queue routing are
  modeled or the run is `inconclusive`.
- the selected queue rule's merge method is supported.
- the selected queue rule's batch-size minimum and maximum are modeled.
- merge conditions identify the authoritative Mergify proof checks.
- the workflow check-production source used for check mapping is read at the
  expected base SHA and has a recorded identity.
- queue-relevant PR metadata, including labels used by queue conditions, is
  snapshotted for the run.

If a PR matches multiple queue rules and the implementation cannot model
Mergify's effective selection semantics, the result is `inconclusive`. If a PR
routes to no queue rule, it is `blocked` when proven ineligible and
`inconclusive` when routing evidence is unavailable. If selected PRs route to
different queue rules, the result is at most `split_advised`.

Effective queue selection must model Mergify's first matching queue rule in file
order. A later catch-all queue condition does not make an earlier more-specific
match ambiguous.

Queue priority and live queue order affect Mergify's eventual partition, not
the set-cleanliness proof. They are residual risk after effective queue routing
has been modeled. They must not force `inconclusive` unless routing itself
depends on unsupported priority semantics.

The required-check source for Mergify readiness is the selected queue rule's
`merge_conditions`. GitHub branch-protection required checks are additional
governance evidence when available, but they are not a substitute for Mergify's
merge conditions.

The source for classifying a Mergify `check-success` merge condition as
in-place mapped, proof-only, or unsupported is the workflow check-production
source at the expected base SHA. A check cannot be classified as proof-only by
absence from PR-head checks. Positive evidence from that source is required.
Each classification record must include the proof check name, classification
type, source workflow or registry blob SHA, producer workflow and job identity,
and mapped in-place check identity when applicable.

The implementation must classify each `.mergify.yml` field present in this repo:

| Field | Preflight handling |
| --- | --- |
| `merge_queue.max_parallel_checks` | parse and report residual cost impact |
| `merge_queue.reset_on_external_merge` | parse and report post-preflight invalidation risk |
| `queue_rules[].name` | required unique queue identity |
| `queue_rules[].queue_conditions` | model for effective PR-to-queue routing |
| `queue_rules[].merge_conditions` | model required reviewer and check evidence |
| `queue_rules[].branch_protection_injection_mode` | support explicitly or mark config inconclusive |
| `queue_rules[].batch_size` | model min, max, and scalar forms |
| `queue_rules[].batch_max_wait_time` | model below-min wait behavior |
| `queue_rules[].batch_max_failure_resolution_attempts` | support explicitly or mark config inconclusive |
| `queue_rules[].checks_timeout` | parse and report proof-time residual risk |
| `queue_rules[].draft_bot_account` | support explicitly or mark config inconclusive |
| `queue_rules[].merge_method` | support explicitly or mark config inconclusive |
| `priority_rules[].conditions` | model when needed for effective queue routing |
| `priority_rules[].name` | required unique priority-rule identity |
| `priority_rules[].priority` | parse and report live-order residual risk |
| `priority_rules[].allow_checks_interruption` | parse and report interruption residual risk |

Unknown fields, unknown condition operators, and unsupported values are
`inconclusive`; they are not ignored.

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
- required Mergify merge-condition checks with in-place equivalents are not
  already failed on the PR head.
- native code-owner review, stale-review dismissal, last-push approval, and
  review-thread resolution are directly verified when the GitHub API exposes
  them to the preflight run.

`Exposed to the preflight run` means obtainable through the authenticated GitHub
API surface available to the tool. Required evidence that the API can return must
be queried. Permission errors, rate limits, partial payloads, and omitted queries
are `inconclusive`, not residual risk.

Required, GitHub-queryable merge gates cannot be downgraded to residual risk.
Unavailable required-gate evidence is `inconclusive`; proven failed required
evidence is `blocked`.

If a required governance gate is not exposed to the preflight run, the readiness
lane is `inconclusive`. Residual risk is reserved for non-required or
out-of-boundary evidence.

Green in-place PR checks are negative evidence only: they show the PR head is not
already failed. They do not prove the Mergify proof context will pass. Mergify
merge-condition checks that are produced only in the proof context are residual
risk when absent in-place, not `inconclusive`.

The readiness lane must classify each `merge_conditions` check as one of:

- in-place mapped: an in-place PR-head check identity must be evaluated through
  the check-state table.
- proof-only: positive workflow-production evidence says the check is produced
  only in the proof context.
- unsupported mapping: `inconclusive`.

An in-place equivalent must be positively identified by the workflow
check-production source. A repo-designated feedback-only or non-required
iteration check is not an equivalent; its state is residual risk unless a future
contract update provides evidence that it predicts proof failure.

Required check states must be classified through one table:

| Check state | Classification |
| --- | --- |
| success or pass on expected head and expected check identity | ready evidence |
| failure, error, cancelled, action-required, or startup-failure | `blocked` |
| pending, queued, requested, waiting, or in-progress | `inconclusive` |
| skipped or neutral | `inconclusive` |
| missing in-place mapped check | `inconclusive` |
| missing proof-only check | residual risk |
| proof-only check unexpectedly present with terminal failure | `blocked` |
| proof-only check unexpectedly present with non-terminal state | `inconclusive` |
| feedback-only or non-required iteration check failed | residual risk |
| duplicate name without unique app/workflow identity | `inconclusive` |
| stale to a SHA other than expected head | `inconclusive` |
| unknown state or bucket | `inconclusive` |

Rows classified as residual risk create non-gating residual findings only. They
do not provide required-lane ready evidence or change the readiness lane status.

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

For a supported squash queue, the integration lane models the final combined
candidate tree produced by applying the selected PR changes to the expected base
under the selected queue rule. It does not need to materialize final squash
commit metadata. The implementation must include a design note and parity tests
for the squash-tree algorithm before claiming `queue_as_one_wave`.

A selected wave larger than the queue rule's maximum batch size is
`split_advised` when all PRs are otherwise ready, and the output must include a
size-valid partition. A selected wave below the queue rule's minimum batch size
is ready when the queue rule's wait-time behavior is parsed; the residual-risk
lane reports the expected wait. If wait-time behavior cannot be parsed, the
Mergify config lane is `inconclusive`.

When routing, size, and conflict constraints interact, verdict reduction applies
per routed queue subset first, then to the whole requested wave. Any blocked
subset makes the requested wave `blocked`; any inconclusive subset makes it
`inconclusive`; otherwise incompatible ready subsets produce `split_advised`.

## Verifier Lane

Verifier profiles are named contracts, not arbitrary shell escape hatches.

Each verifier command must have:

- configured timeout.
- bounded stdout and stderr diagnostics.
- classified execution failure.
- classified timeout failure.
- deterministic input scope bound to the synthetic candidate being checked.

Verifier proof is batch-scoped for emitted batches. A passing optimistic batch
does not imply that each constituent PR's standalone base+PR synthetic commit
was verifier-clean; standalone verifier failures are localized only after a
batch verifier failure triggers fallback. The residual-risk lane must disclose
this boundary.

Successful verifier output is suppressed. Failed verifier output is bounded and
passes through the repository diagnostic redaction policy before display.
Verifier command names may be emitted as audit metadata.

Verifier profiles must not be maintained as a second copy of the cheap gate set.
If the default profile claims to catch cheap deterministic Mergify waste, it must
cover the full cheap deterministic gate set or narrow its name and description.
The source-fence reduced-profile selector must read its full-profile pathspecs
from config, including source-fence governance files whose changes require the
full fixture test phase. Configured reduced-profile rewrite sources and targets
must be public `just` recipes with declared `local-gate:` labels. Verifier
profiles and ad hoc verifier extras must not invoke reduced-profile targets,
their private inners, or direct `--fences-only` commands; targets are reachable
only through the configured rewrite map.

## Residual-Risk Lane

Every output, JSON and plain text, must include residual risks that preflight did
not prove. At minimum:

- full CI result.
- verifier proof is batch-scoped, not standalone per-PR proof for passing
  optimistic batches.
- source-fence reduced-profile runs may skip fixture test suites for eligible
  diffs.
- Mergify proof PR behavior.
- remote runner availability and environment.
- flaky checks and external services.
- base or PR head drift after preflight.
- Mergify config or workflow check-production changes introduced by a selected
  PR and applied after merge.
- queue-relevant label or PR metadata drift after preflight.
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
- Redact and bound structured `evidence` fields before output.
- Secret-shaped command names or arguments are rejected before execution.
- Secrets read by a verifier from environment or files remain the verifier's
  responsibility; preflight still redacts all emitted diagnostics.
- Include enough structured metadata to identify the failing lane, classifier,
  PR number, batch, command, and timeout without exposing raw logs.

## Evidence Required Before Merge

An implementation of this contract must include tests for:

- absent required input.
- absent required external evidence.
- empty required input.
- invalid required input.
- stale base input.
- stale head input.
- unavailable required input.
- timed-out required input.
- ambiguous required input.
- verdict precedence when blocked and inconclusive findings coexist.
- residual-risk findings do not become terminal lane statuses.
- residual-risk findings do not repair missing required evidence.
- exit codes for every verdict, including `split_advised` as non-zero success.
- duplicate PR number input is a usage error.
- expected base SHA mismatch.
- expected PR head SHA mismatch.
- base drift is `inconclusive`.
- head drift is `blocked`.
- Mergify config missing, invalid, unsupported, and changed after snapshot.
- Mergify config read from expected base, not local worktree or PR head.
- unsupported Mergify condition operators.
- queue-rule routing for default, hotfix, zero-route, mixed-queue, and ambiguous
  waves.
- hotfix queue routing wins over a later catch-all default queue by file order.
- queue-condition metadata unavailable or changed during the run.
- selected PR changes `.mergify.yml` or workflow check production and reports
  post-merge residual risk.
- selected wave larger than queue batch-size maximum.
- selected wave smaller than queue batch-size minimum.
- mixed queue split recommendations respect each queue's batch bounds.
- static and behavioral checks for no use of `FETCH_HEAD` or another shared
  mutable fetch result.
- concurrent preflight runs sharing one clone.
- metadata unavailable.
- `--no-gh` returns `blocked` when it detects a blocker.
- `--no-gh` otherwise returns `inconclusive`.
- open, closed, draft, review-approved, and review-not-approved states.
- required reviewer identity absent, unavailable, and not approved.
- required check pass, fail, pending, skipped, neutral, missing, duplicate,
  stale-to-old-SHA, action-required, startup-failure, and unknown buckets.
- proof-only Mergify merge-condition check absent in-place is residual risk.
- proof-only classification requires positive workflow-production evidence.
- required check absent in-place without proof-only evidence is `inconclusive`.
- CI workflow check-production mapping rename or removal is `inconclusive`.
- mapping source drift between workflow and registry is `inconclusive`.
- missing in-place mapped merge-condition check is `inconclusive`.
- failed in-place mapped merge-condition check is `blocked`.
- feedback-only or non-required iteration check failure is residual risk.
- unsupported merge-condition check mapping is `inconclusive`.
- mapped check present under the right name but wrong app or workflow identity
  is `inconclusive`.
- base conflict.
- clean split into multiple advised batches.
- verifier failure.
- batch-level verifier failure.
- verifier timeout.
- verifier timeout kills child process groups.
- verifier executable missing.
- verifier command string containing secret-shaped arguments.
- temp ref and worktree cleanup failure.
- bounded and redacted verifier diagnostics.
- bounded and redacted structured evidence.
- residual-risk output in JSON and plain text.
- unsupported Mergify merge method prevents `queue_as_one_wave`.

Completion evidence requires:

- targeted unit tests for the preflight tool.
- Python syntax validation for changed scripts.
- targeted text/static checks for this document.
- `just ci-lint-workflow`.
- `just source-fence-static`.
- internal adversarial review after local findings are resolved.
- a generated implementation-branch audit checklist that lists every new `if`,
  `match`, `except`, `unwrap_or`, `unwrap_or_default`, `or_else`, and default
  branch, with a short explanation of why each branch is validation or
  classification rather than silent fallback.
