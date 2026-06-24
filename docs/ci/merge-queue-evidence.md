# Merge Queue Evidence

Issue: #929

## Scope

This note records the live evidence required before enabling ready-PR iteration deferral. It does not change `ready_pr` or `ready_for_review` policy values.

The queue proof must show that the prospective merge commit is tested on `merge_group` before any ready-PR deferral rollout:

| Evidence | Expected value |
| --- | --- |
| Event name | `merge_group` |
| Event action | `checks_requested` |
| Queue ref | `refs/heads/gh-readonly-queue/...` |
| Queue SHA | Different from the original PR head SHA unless GitHub reports an identical synthetic commit |
| CI workflow | Runs on the queue ref and publishes `gate` |
| Backtester CI workflow | Runs on the queue ref and publishes `backtester-gate` |
| Concurrency | Queue runs are not cancelled by PR or `workflow_dispatch` supersession |

## Capture Checklist

For the PR used as the live queue probe, record:

| Field | Source |
| --- | --- |
| PR URL and head SHA | `gh pr view <number> --json url,headRefOid` |
| Queue workflow run IDs | `gh run list --event merge_group --branch <queue-ref-name> --json databaseId,url,event,headSha,headBranch,status,conclusion,workflowName` |
| Event action and ref | Run API for `event`, `head_sha`, and `head_branch`; `Compute CI policy` / `Compute Backtester CI policy` log line for the expanded `--event-action` argument |
| Required gate jobs | `gh run view <run-id> --json jobs` |
| Check conclusions on queue SHA | `gh api repos/{owner}/{repo}/commits/<queue-sha>/check-runs` |
| Cancellation behavior | Verify CI and Backtester CI queue runs are `completed` with non-`cancelled` conclusions, or document any cancelled run and its cancelling workflow |

## Evidence Table

Fill this table in the PR description after the live queue run exists.

| Workflow | Run ID | URL | Event | Action | Head SHA | Ref/head branch | Required gate job | Conclusion |
| --- | ---: | --- | --- | --- | --- | --- | --- | --- |
| CI | TBD | TBD | `merge_group` | `checks_requested` | TBD | `gh-readonly-queue/...` | `gate` | TBD |
| Backtester CI | TBD | TBD | `merge_group` | `checks_requested` | TBD | `gh-readonly-queue/...` | `backtester-gate` | TBD |

## Mismatch Handling

If live queue evidence contradicts the expected shape, stop the rollout and record the mismatch before changing policy. The next PR may proceed to observational queue-temp detection only after both required queue gates are observed on the queue commit.
