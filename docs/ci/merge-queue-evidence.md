# Merge Queue Evidence

Issue: #929

## Scope

This note records the live evidence required before enabling ready-PR iteration deferral. It does not change `ready_pr` or `ready_for_review` policy values.

Issue #929's active rollout path is Mergify's external queue for a private repository. Mergify does not emit GitHub's native `merge_group` event for that path; native `merge_group` support remains future-use evidence for a GitHub-native merge queue.

Before ready-PR deferral, the active queue proof must show that Mergify tests the prospective merge with full CI and publishes both required gates before the original PR merges. Native `merge_group` evidence is acceptable only if the repository is moved to GitHub's native merge queue.

## Native GitHub Merge Queue Evidence

This table is for GitHub's native merge queue only:

| Evidence | Expected value |
| --- | --- |
| Event name | `merge_group` |
| Event action | `checks_requested` |
| Queue ref | `refs/heads/gh-readonly-queue/...` |
| Queue SHA | Different from the original PR head SHA unless GitHub reports an identical synthetic commit |
| CI workflow | Runs on the queue ref and publishes `gate` |
| Backtester CI workflow | Runs on the queue ref and publishes `backtester-gate` |
| Concurrency | Queue runs are not cancelled by PR or `workflow_dispatch` supersession |

### Capture Checklist

For the PR used as the live queue probe, record:

| Field | Source |
| --- | --- |
| PR URL and head SHA | `gh pr view <number> --json url,headRefOid` |
| Queue workflow run IDs | `gh run list --event merge_group --branch <queue-ref-name> --json databaseId,url,event,headSha,headBranch,status,conclusion,workflowName` |
| Event action and ref | Run API for `event`, `head_sha`, and `head_branch`; `Compute CI policy` / `Compute Backtester CI policy` log line for the expanded `--event-action` argument |
| Required gate jobs | `gh run view <run-id> --json jobs` |
| Check conclusions on queue SHA | `gh api repos/{owner}/{repo}/commits/<queue-sha>/check-runs` |
| Cancellation behavior | Verify CI and Backtester CI queue runs are `completed` with non-`cancelled` conclusions, or document any cancelled run and its cancelling workflow |

### Evidence Table

Fill this table only after a native GitHub merge queue run exists.

| Workflow | Run ID | URL | Event | Action | Head SHA | Ref/head branch | Required gate job | Conclusion |
| --- | ---: | --- | --- | --- | --- | --- | --- | --- |
| CI | TBD | TBD | `merge_group` | `checks_requested` | TBD | `gh-readonly-queue/...` | `gate` | TBD |
| Backtester CI | TBD | TBD | `merge_group` | `checks_requested` | TBD | `gh-readonly-queue/...` | `backtester-gate` | TBD |

## Mergify Queue Evidence

For the #929 rollout, record the Mergify queue path:

| Evidence | Expected value |
| --- | --- |
| Queue command | `@mergifyio queue` or the Mergify queue checkbox |
| Queue rule | Configured Mergify queue rule name |
| Queue branch or PR | A Mergify temp branch/PR, usually `mergify/merge-queue/...` or `tmp-mergify/merge-queue/...`, unless Mergify explicitly skips speculative checks |
| CI workflow | Runs full proof and publishes `gate` on the queue proof context |
| Backtester CI workflow | Runs full proof and publishes `backtester-gate` on the queue proof context |
| Original PR merge | Happens only after required gates and reviewer conditions match |

### Mergify Capture Checklist

| Field | Source |
| --- | --- |
| Original PR URL and head SHA | `gh pr view <number> --json url,headRefOid` |
| Mergify queue status | Mergify queue comment payload on the PR |
| Queue/temp PR identity | Mergify queue comment payload, related PR comments, and workflow run head branch |
| Queue workflow run IDs | `gh run list --branch <queue-head-branch> --json databaseId,url,event,headSha,headBranch,status,conclusion,workflowName` |
| Required gate jobs | `gh run view <run-id> --json jobs` |
| Check conclusions on proof SHA | `gh api repos/{owner}/{repo}/commits/<proof-sha>/check-runs` |

### PR #957 Observation

PR: https://github.com/seungpyoson/bolt-v2/pull/957

Original head SHA: `317e721663fa1e6921326960f83ed0683673aea2`

Merge commit: `6f031cea0cd07729c9c95f9673d6f8fee0afb9cc`

The PR-head required gates were green before merge:

| Workflow | Run ID | URL | Event | Head SHA | Required gate job | Conclusion |
| --- | ---: | --- | --- | --- | --- | --- |
| CI | 28087657739 | https://github.com/seungpyoson/bolt-v2/actions/runs/28087657739 | `pull_request` | `317e721663fa1e6921326960f83ed0683673aea2` | `gate` | success |
| Backtester CI | 28087657746 | https://github.com/seungpyoson/bolt-v2/actions/runs/28087657746 | `pull_request` | `317e721663fa1e6921326960f83ed0683673aea2` | `backtester-gate` | success |

Mergify accepted `@mergifyio queue` on PR #957, queued it under rule `default`, then left the queue after 10 minutes 27 seconds. Its status comment reported:

| Field | Observed value |
| --- | --- |
| Queue state | `dequeued` |
| Queued at | `2026-06-24T09:52:26.891445+00:00` |
| Left queue | `2026-06-24 10:02 UTC` |
| Queue checks | skipped because the PR was already up to date |
| Failure reason | GitHub could not merge after 10 minutes; `2 of 4 required status checks are expected` |
| Native `merge_group` runs | none; `gh run list --event merge_group` returned `[]` |

This proves the current repository queue path did not emit native GitHub `merge_group` evidence for PR #957. It also did not produce a durable Mergify temp-PR proof run to capture.

This PR adds the repo-side Mergify queue configuration and verifier guard. Because Mergify reads `.mergify.yml` from the default branch, those settings take effect only after this PR reaches `main`. The remaining live dependency is the GitHub ruleset shape: the Mergify-bypassed ruleset must enforce the required checks and review rules while rules that protect deletion/non-fast-forward remain non-bypassed. After that live settings check, requeue a PR and record the proof context where full `gate` and `backtester-gate` can be observed.

## Mismatch Handling

If live queue evidence contradicts the expected shape, stop the rollout and record the mismatch before changing policy. Do not flip `ready_pr` or `ready_for_review` until the active queue path has live proof that both required gates run full CI on the prospective merge context.
