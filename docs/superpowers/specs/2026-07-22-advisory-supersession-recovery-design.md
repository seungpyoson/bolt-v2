# Advisory Supersession Recovery Design

## Decision and scope

PR #1495 keeps its exact-current-`main` admission controller and legacy-rerun watchdog. This revision closes the workflow-run search-limit class without replacing the controller with native concurrency or scanning the repository's complete Actions history.

The governed workflow-run census remains scoped to `advisory.yml`, `main`, and `push`. It adds one fixed lookback window and fail-closed result, cancellation-target, and request-budget thresholds. This preserves a permanently bounded operating cost while ensuring the controller stops before GitHub's documented 1,000-result search ceiling or shared API budget can hide a run or strand a partial reconciliation.

This PR does not change advisory Cargo jobs, #1497, #1494, merge authority, or Mergify behavior.

## Admission invariant

> An advisory push run may admit its evidence jobs only when its SHA is exact-current `main`, a GitHub-time-anchored governed census is complete and stable, the complete reconciliation fits its governed request and mutation budgets, every discovered active push run for a different SHA has reached GitHub's exact terminal state after cancellation, and a final stable census finds no new stale run. Any ref movement, incomplete or oversized census, malformed response, unstable pagination, unknown run state, budget exhaustion, or unconfirmed cancellation fails admission.

The legacy-rerun watchdog applies the same freshness and confirmed-cancellation rules. Pull-request, schedule, and manual-dispatch runs remain isolated from the push controller.

## Bounded census

GitHub documents three constraints that define the census:

- workflow-run searches using `branch`, `created`, or `event` return at most 1,000 results;
- a workflow run is cancelled after 35 days.
- a workflow run may be re-run for 30 days after its initial run.

The controller therefore queries only the configured workflow with these governed filters:

- `branch = "main"`;
- `event = "push"`;
- `created` at or after a cutoff derived from the invoking run's validated GitHub `created_at` and captured once for the complete reconciliation;
- `runs_per_page = 100`.

`workflow_run_lifetime_days = 35` and `rerun_request_window_days = 30` record the documented platform limits. `created_lookback_days = 66` covers both limits plus one full day of margin, without depending on the undocumented question of whether a late re-run attempt receives a fresh execution clock. For the admission controller, the current advisory run is fetched by exact run ID; its repository, workflow, event, branch, attempt, head SHA, and `created_at` are validated before `created_at` becomes the clock authority. That exact advisory run ID, attempt, and SHA must then appear in every complete census. The watchdog does not make an admission census; it validates its target run and current `main` directly under the same cancellation and freshness rules. The fixed controller cutoff is reused across all pages, stabilization sweeps, and reconciliation rounds so runner clock skew and a moving search boundary cannot silently exclude a run.

`search_result_limit = 1000` records GitHub's documented search ceiling. `max_search_results = 900` is a reviewed fail-closed threshold below it. A response whose `total_count` is at least 900 fails immediately. This is deliberate capacity protection: the repository must revise the window or operating policy before the platform ceiling can make completeness ambiguous.

Each page must satisfy all of these conditions:

1. `total_count` is a non-negative integer and is identical on every page in the sweep.
2. Every returned run has a unique integer ID and the expected branch and event; the workflow-specific endpoint supplies the workflow boundary.
3. Pagination follows only a previously unvisited, same-origin `Link: rel="next"` URL whose path is the exact configured workflow-runs endpoint and whose invariant query parameters exactly preserve branch, event, fixed `created` cutoff, and page size; only the page cursor may change.
4. A missing next link while fewer than `total_count` records were fetched is fatal.
5. A next link after `total_count` records were fetched is fatal.
6. The final number of unique fetched records equals `total_count` exactly.
7. The sweep uses no more than `ceil(max_search_results / runs_per_page)` pages; empty intermediate pages, repeated URLs, changed queries, and cycles are fatal.

The controller never silently deduplicates a repeated run ID. A duplicate signals page-boundary drift and invalidates that sweep.

## Stabilization and cancellation

One complete census signature contains the active subset's run ID, attempt, head SHA, and `created_at` in run-ID order. Branch and event are validated invariants, not signature fields. Status selects membership only: a queued-to-in-progress transition does not destabilize the decision, while an active-to-completed transition, insertion, deletion, attempt change, or SHA change does. Admission requires `discovery_stable_sweeps = 2` identical active-subset signatures within `discovery_max_sweeps = 4`. Total-count drift, duplicate boundaries, malformed pagination, or failure to stabilize is fatal.

The controller reads the live `main` ref before and after every complete sweep, before normal cancellation, before force-cancellation, and immediately before admission. Movement self-cancels the invoking run. The watchdog uses the same guards but fails without touching a newly current target.

Before the first cancellation, the controller calculates the complete reconciliation's worst-case request and mutation count from the discovered targets, governed sweep/round limits, poll attempts, and both cancellation endpoints. It fails before mutation if the target count exceeds `max_cancellation_targets = 10`, the calculated total exceeds `max_reconciliation_requests = 400`, the latest primary-rate-limit response cannot preserve `api_rate_limit_reserve = 100`, or the work cannot complete inside `reconciliation_timeout_seconds = 600`. These bounds apply across the whole reconciliation, not once per round.

Normal cancellation is followed by bounded polling. If the target is not exactly `completed`, the controller rechecks freshness and requests force-cancellation. It then polls again and fails unless the target is exactly `completed`. Unknown or future statuses are non-terminal. The controller then performs another complete stable census. A newly active different-SHA run starts another reconciliation round; admission requires a final stable census with no such run within `max_reconciliation_rounds = 3`. Exhausting the target, request, time, or round budget fails admission.

`terminal_status` and `event` remain in TOML and are validated as the only supported contracts, `completed` and `push`. Reconciliation consumes the validated config and contains no duplicate string literals for either fact.

## Configuration ownership

`ci/advisory-supersession.toml` is the only home for:

- API version, branch, workflow, and event;
- request timeout, exact page size, total reconciliation deadline, and primary-rate-limit reserve;
- platform run-lifetime, rerun-window, and search-limit contracts, lookback days, and maximum accepted search results;
- stable-sweep and maximum-sweep counts;
- cancellation-target, total-request, and reconciliation-round limits;
- cancellation poll attempts and interval;
- terminal status.

The loader rejects unknown keys, unsupported event or terminal values, non-positive durations/counts, any `runs_per_page` other than 100, a lookback not greater than the sum of the governed run lifetime and rerun window, `max_search_results` at or above the governed search limit, a maximum-sweep count smaller than the stable-sweep count, a request budget that cannot cover the configured discovery and polling bounds, and a reconciliation deadline that cannot fit within the workflow's existing 15-minute outer safety timeout with a reviewed margin. The controller's TOML-owned deadline is the operational bound; the workflow timeout remains only an outer platform kill switch.

## Failure behavior

Every uncertainty is fail-closed:

- API errors, redirects, foreign pagination origins, malformed JSON, or missing fields fail the controller;
- a threshold breach or incomplete/unstable census fails before any evidence job starts;
- an excessive target count or insufficient primary/secondary request budget fails before the first mutation;
- a 409 cancellation response is logged, then terminal state is still confirmed;
- failure to confirm terminal state after force-cancellation fails admission;
- a new stale run found by the post-cancellation stable census is reconciled only within the governed total round and request budgets;
- ref movement cancels the stale invoking run or stops the watchdog;
- no failure changes PR, schedule, or manual-dispatch concurrency behavior.

The controller continues to use only GitHub's ephemeral `GITHUB_TOKEN`, with `actions: write` and `contents: read` confined to cancellation jobs and checkout credentials disabled.

## Verification evidence

Behavior tests must cover:

- multi-page success with exact `total_count` accounting;
- duplicate page boundaries, missing or extra continuation links (including an exact page-multiple boundary), foreign or same-origin altered links, repeated URLs, empty-page cycles, and total-count drift;
- threshold values immediately below and at `max_search_results`;
- the 66-day lookback and its cross-field rejection cases, using the invoking run's GitHub `created_at` rather than runner time;
- active-set convergence during multiple queued/in-progress transitions, and membership changes when a run arrives or completes;
- stabilization exhaustion;
- excessive cancellation targets, insufficient rate-limit reserve, worst-case request calculation, total reconciliation timeout, and round exhaustion, all before unauthorized mutation;
- a stale run arriving after initial stabilization or during cancellation and being found by the final stable census;
- exact validation of `push` and `completed`, with unknown statuses treated as active;
- movement of `main` before discovery, during discovery, before normal cancel, during polling before force-cancel, and before admission;
- 202 and 409 cancellation responses, force-cancel escalation, and unconfirmed cancellation;
- controller and watchdog exit codes;
- strict TOML key and cross-field validation.

Targeted Python tests, Ruff, actionlint, and the advisory workflow at the exact branch head are required, followed by an internal adversarial review of the exact diff. Push-path admission and the legacy watchdog cannot execute before merge; their first post-merge events remain required live evidence. The first live controller record must also reconcile the observed `total_count`, fetched count, page count, computed request budget, and remaining primary rate limit without logging credentials.

## Accepted residual risks

- GitHub does not document snapshot isolation or the exactness semantics of `total_count` for paginated workflow-run searches. Two identical incomplete responses or a consistently inaccurate count are theoretically possible. Exact count checks, duplicate rejection, repeated complete signatures, a fixed cutoff, and the sub-ceiling threshold are layered hedges rather than a platform proof.
- A push can land in the API round trip between the final ref read and a cancellation request. GitHub offers no compare-and-cancel primitive. The repeated freshness checks minimize the window; any resulting loss is visible evidence loss, not stale admission authority.
- A stale run can appear after the final stable census and before admission. GitHub offers no atomic list-and-admit primitive; the legacy watchdog and next push reconciliation are the asynchronous compensating controls.
- Sustained volume reaching 900 matching runs within 66 days, more than 10 simultaneous cancellation targets, or an insufficient shared API budget stops advisory admission until the governed policy is revised. This is intentional and loud.
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
- [Re-running workflows and jobs](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs)
- [REST API rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)
