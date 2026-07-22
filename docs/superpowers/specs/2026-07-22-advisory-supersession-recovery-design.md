# Advisory Supersession Recovery Design

## Decision and scope

PR #1495 keeps its exact-current-`main` admission controller and legacy-rerun watchdog. This revision closes the workflow-run search-limit class without replacing the controller with native concurrency or scanning the repository's complete Actions history.

The governed workflow-run census remains scoped to `advisory.yml`, `main`, and `push`. It adds one fixed lookback window and one fail-closed result threshold. This preserves the current one-page operating cost while ensuring the controller stops before GitHub's documented 1,000-result search ceiling can hide a run.

This PR does not change advisory Cargo jobs, #1497, #1494, merge authority, or Mergify behavior.

## Admission invariant

> An advisory push run may admit its evidence jobs only when its SHA is exact-current `main`, the governed census is complete and stable, and every discovered active push run for a different SHA has reached GitHub's exact terminal state after cancellation. Any ref movement, incomplete or oversized census, malformed response, unstable pagination, unknown run state, or unconfirmed cancellation fails admission.

The legacy-rerun watchdog applies the same freshness and confirmed-cancellation rules. Pull-request, schedule, and manual-dispatch runs remain isolated from the push controller.

## Bounded census

GitHub documents two constraints that define the census:

- workflow-run searches using `branch`, `created`, or `event` return at most 1,000 results;
- a workflow run is cancelled after 35 days.

The controller therefore queries only the configured workflow with these governed filters:

- `branch = "main"`;
- `event = "push"`;
- `created` at or after a cutoff captured once at census start;
- `runs_per_page = 100`.

`workflow_run_lifetime_days = 35` records GitHub's documented execution limit. `created_lookback_days = 36` supplies one full day of margin beyond it. A run older than the cutoff cannot still be active. The cutoff is calculated once and reused across all pages and stabilization sweeps so the search boundary cannot move during one admission decision.

`search_result_limit = 1000` records GitHub's documented search ceiling. `max_search_results = 900` is a reviewed fail-closed threshold below it. A response whose `total_count` is at least 900 fails immediately. This is deliberate capacity protection: the repository must revise the window or operating policy before the platform ceiling can make completeness ambiguous.

Each page must satisfy all of these conditions:

1. `total_count` is a non-negative integer and is identical on every page in the sweep.
2. Every returned run has a unique integer ID and the expected branch and event; the workflow-specific endpoint supplies the workflow boundary.
3. Pagination follows only GitHub's same-origin `Link: rel="next"` URL.
4. A missing next link while fewer than `total_count` records were fetched is fatal.
5. A next link after `total_count` records were fetched is fatal.
6. The final number of unique fetched records equals `total_count` exactly.

The controller never silently deduplicates a repeated run ID. A duplicate signals page-boundary drift and invalidates that sweep.

## Stabilization and cancellation

One complete census signature contains each run's ID, attempt, head SHA, branch, event, and status in run-ID order. Admission requires `discovery_stable_sweeps = 2` identical complete signatures within `discovery_max_sweeps = 4`. A transition, insertion, deletion, total-count change, duplicate boundary, or failure to stabilize is fatal.

The controller reads the live `main` ref before and after every complete sweep, before normal cancellation, before force-cancellation, and immediately before admission. Movement self-cancels the invoking run. The watchdog uses the same guards but fails without touching a newly current target.

Normal cancellation is followed by bounded polling. If the target is not exactly `completed`, the controller rechecks freshness and requests force-cancellation. It then polls again and fails unless the target is exactly `completed`. Unknown or future statuses are non-terminal.

`terminal_status` and `event` remain in TOML and are validated as the only supported contracts, `completed` and `push`. Reconciliation consumes the validated config and contains no duplicate string literals for either fact.

## Configuration ownership

`ci/advisory-supersession.toml` is the only home for:

- API version, branch, workflow, and event;
- request timeout and page size;
- platform run-lifetime and search-limit contracts, lookback days, and maximum accepted search results;
- stable-sweep and maximum-sweep counts;
- cancellation poll attempts and interval;
- terminal status.

The loader rejects unknown keys, unsupported event or terminal values, non-positive durations/counts, `runs_per_page` outside the governed page-size range, a lookback not greater than the governed run lifetime, `max_search_results` at or above the governed search limit, and a maximum-sweep count smaller than the stable-sweep count.

## Failure behavior

Every uncertainty is fail-closed:

- API errors, redirects, foreign pagination origins, malformed JSON, or missing fields fail the controller;
- a threshold breach or incomplete/unstable census fails before any evidence job starts;
- a 409 cancellation response is logged, then terminal state is still confirmed;
- failure to confirm terminal state after force-cancellation fails admission;
- ref movement cancels the stale invoking run or stops the watchdog;
- no failure changes PR, schedule, or manual-dispatch concurrency behavior.

The controller continues to use only GitHub's ephemeral `GITHUB_TOKEN`, with `actions: write` and `contents: read` confined to cancellation jobs and checkout credentials disabled.

## Verification evidence

Behavior tests must cover:

- multi-page success with exact `total_count` accounting;
- duplicate page boundaries, missing or extra continuation links, foreign links, and total-count drift;
- threshold values immediately below and at `max_search_results`;
- a run transitioning status, arriving, or completing between sweeps;
- stabilization exhaustion;
- exact validation of `push` and `completed`, with unknown statuses treated as active;
- movement of `main` before discovery, during discovery, before normal cancel, during polling before force-cancel, and before admission;
- 202 and 409 cancellation responses, force-cancel escalation, and unconfirmed cancellation;
- controller and watchdog exit codes;
- strict TOML key and cross-field validation.

Targeted Python tests, Ruff, actionlint, and the advisory workflow at the exact branch head are required. Push-path admission and the legacy watchdog cannot execute before merge; their first post-merge events remain required live evidence.

## Accepted residual risks

- GitHub does not document snapshot isolation for paginated workflow-run searches. Two identical incomplete responses are theoretically possible. Exact count checks, duplicate rejection, repeated complete signatures, a fixed cutoff, and the sub-ceiling threshold bound this platform risk.
- A push can land in the API round trip between the final ref read and a cancellation request. GitHub offers no compare-and-cancel primitive. The repeated freshness checks minimize the window; any resulting loss is visible evidence loss, not stale admission authority.
- Sustained volume reaching 900 matching runs within 36 days stops advisory admission until the governed policy is revised. This is intentional and loud.
- The first push-controller and legacy-watchdog executions remain structurally unprovable before merge.

## Explicitly rejected alternatives

- Repo-wide run discovery: the live repository history is too large and would exhaust the shared API budget.
- Unlimited workflow-scoped discovery: it grows with retained history and has no permanent request bound.
- Per-status searches: transitions can cross query partitions, and future active statuses could be missed.
- Relying on `total_count` at or above the platform ceiling: completeness is already ambiguous there.
- Replacing the controller with native concurrency: it would discard the accepted stale-rerun guarantees.

## Sources

- [Workflow runs REST API](https://docs.github.com/en/rest/actions/workflow-runs?apiVersion=2026-03-10)
- [GitHub Actions limits](https://docs.github.com/en/actions/reference/limits)
