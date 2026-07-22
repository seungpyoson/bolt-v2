# Advisory Supersession Recovery Design

## Decision and scope

PR #1495 keeps its exact-current-`main` admission controller and default-branch watchdog. This revision closes the workflow-run search-limit class without replacing the controller with native concurrency or scanning the repository's complete Actions history.

The governed workflow-run census remains scoped to `advisory.yml`, `main`, and `push`. It adds one fixed lookback window and fail-closed result, cancellation-episode, and request-budget thresholds. This preserves a permanently bounded operating cost while ensuring the controller stops before GitHub's documented 1,000-result search ceiling or shared API budget can hide a run or strand a partial reconciliation.

This PR does not change advisory Cargo jobs, #1497, #1494, merge authority, or Mergify behavior.

This specification is temporary implementation scaffolding. After implementation findings are resolved, it is removed before merge and the resulting code-only head receives fresh exact-head verification and review. Lasting operator facts live as comments beside their governed keys in `ci/advisory-supersession.toml`, with loud controller diagnostics naming the threshold or budget that stopped admission; no permanent design document or duplicate runbook owns those values.

## Admission invariant

> An advisory push run may admit its evidence jobs only when its SHA is exact-current `main`, a GitHub-time-anchored governed census is complete and stable, cumulative request, mutation, cancellation-episode, point, round, and time budgets remain valid before every mutation, every discovered active push attempt for a different SHA identified by `(run_id, run_attempt)` has reached GitHub's exact terminal state after cancellation, and a final stable census finds no new stale attempt. Any ref movement, incomplete or oversized census, malformed response, unstable pagination, unknown run state, budget exhaustion, or unconfirmed cancellation fails admission. External failure may leave already-issued cancellations as visible evidence loss, but can never authorize admission or another mutation.

The default-branch watchdog applies the same SHA freshness and confirmed-cancellation rules to every `in_progress` advisory push attempt, including pre-controller first attempts and legacy reruns. Attempt number never authorizes or suppresses cancellation: current-main attempts are preserved and different-SHA attempts are cancelled. Pull-request, schedule, and manual-dispatch runs remain isolated from the push controller.

## Bounded census

GitHub documents three constraints that define the census:

- workflow-run searches using `branch`, `created`, or `event` return at most 1,000 results;
- a workflow run is cancelled after 35 days.
- a workflow run may be re-run for 30 days after its initial run.

The controller therefore queries only the configured workflow with these governed filters:

- `branch = "main"`;
- `event = "push"`;
- `created` at or after a cutoff derived from the exact-run response's validated GitHub HTTP `Date` header and captured once for the complete reconciliation;
- `runs_per_page = 100`.

`workflow_run_lifetime_days = 35` and `rerun_request_window_days = 30` record the documented platform limits. `created_lookback_days = 66` covers both limits plus one full day of margin, without depending on the undocumented question of whether a late re-run attempt receives a fresh execution clock. For the admission controller, the current advisory run is fetched by exact run ID; its repository ID, repository name, workflow, event, branch, attempt, head SHA, `created_at`, and `run_started_at` are validated. The response's HTTP `Date` is parsed once as GitHub server time, must not precede the validated run timestamps, and becomes the cutoff authority. That exact advisory run ID, attempt, and SHA must then appear in every accepted census. A missing sentinel makes the sweep incomplete and non-mutating; it may retry only within the governed sweep, request, interval, and total-deadline budgets, and exhaustion is fatal. The watchdog does not make an admission census; it validates its target run and current `main` directly under the same cancellation and freshness rules. The fixed controller cutoff is reused across all pages, stabilization sweeps, and reconciliation rounds so runner clock skew, an old rerun `created_at`, and a moving search boundary cannot silently distort the window.

`search_result_limit = 1000` records GitHub's documented search ceiling. `max_search_results = 900` is a reviewed fail-closed threshold below it. A response whose `total_count` is at least 900 fails immediately. This is deliberate capacity protection: the repository must revise the window or operating policy before the platform ceiling can make completeness ambiguous.

Each page must satisfy all of these conditions:

1. `total_count` is a non-negative integer and is identical on every page in the sweep.
2. Every returned run has a unique integer ID and the expected branch and event; the workflow-specific endpoint supplies the workflow boundary.
3. Pagination follows only a previously unvisited, same-origin `Link: rel="next"` URL whose decoded semantic identity matches the configured workflow and validated repository. The path may be either `/repos/{owner}/{repo}/actions/workflows/{workflow}/runs` or GitHub's canonical `/repositories/{repository_id}/actions/workflows/{workflow}/runs`, with the numeric ID bound to the exact-run sentinel response. Query parameters are parsed as decoded multimaps: branch, event, fixed `created` cutoff, and page size must each occur exactly once with the governed value, only one positive page cursor may change, and any other query key is fatal.
4. A missing next link while fewer than `total_count` records were fetched is fatal.
5. A next link after `total_count` records were fetched is fatal.
6. The final number of unique fetched records equals `total_count` exactly.
7. The sweep uses no more than `ceil(max_search_results / runs_per_page)` pages; empty intermediate pages, repeated URLs, changed queries, and cycles are fatal.

The controller never silently deduplicates a repeated run ID. A duplicate signals page-boundary drift and invalidates that sweep.

## Stabilization and cancellation

One complete census signature contains the active subset's run ID, attempt, head SHA, and `created_at` in run-ID order. Branch and event are validated invariants, not signature fields. Status selects membership only: a queued-to-in-progress transition does not destabilize the decision, while an active-to-completed transition, insertion, deletion, attempt change, or SHA change does. Admission requires `discovery_stable_sweeps = 2` identical active-subset signatures within `discovery_max_sweeps = 4`, with TOML-governed `sweep_interval_seconds = 5` between attempts. Missing-sentinel, total-count drift, duplicate boundaries, malformed pagination, or another incomplete sweep consumes an attempt without mutation; failure to stabilize is fatal.

The controller reads the live `main` ref before and after every complete sweep, before normal cancellation, before force-cancellation, and immediately before admission. Movement fails the controller immediately without another API mutation; the workflow's existing success gate skips its downstream evidence jobs. The watchdog uses the same guards and likewise fails without touching a newly current target.

One budget calculator is shared by loader validation and runtime reconciliation. With the shipped values, a full sweep costs at most nine page requests plus two `main` reads; a cancellation episode identified by `(run_id, run_attempt)` costs one freshness read, one normal-cancel request, up to six status polls, one pre-force freshness read, one force-cancel request, and up to six more polls. The total bound covers one initial exact-run fetch, up to four sweeps for the initial census, up to four sweeps after each of three reconciliation rounds, a cumulative maximum of ten cancellation episodes across all rounds, and the final admission read. A later attempt of an already-seen run ID is a new episode and consumes the full target, request, mutation, point, and time budgets again. The loader proves the shipped `max_reconciliation_requests = 400` can cover that configured topology; runtime counters, not a duplicate formula, record every actual request, mutation, cancellation episode, documented secondary point, round, and elapsed interval.

Before each mutation, the controller recomputes the known remaining work with the shared calculator. TOML records GitHub's documented `secondary_read_points = 1` and `secondary_mutation_points = 5`; the shipped `max_secondary_points = 500` bounds their cumulative total. The controller fails without that mutation if cumulative episodes would exceed `max_cancellation_targets = 10`, counters would exceed the configured request or secondary-point ceilings, the latest primary-rate-limit response cannot preserve `api_rate_limit_reserve = 100`, or the episode cannot be cancelled and confirmed within the remaining `reconciliation_timeout_seconds = 600`. Each request timeout is capped by the remaining reconciliation deadline. GitHub exposes no reservation for shared primary capacity and no remaining-secondary-budget endpoint, so this preflight is a local bound, not proof that all future calls will succeed. A timeout, ref movement, 403, 429, or other API failure stops further mutation and fails admission; prior successful cancellations remain visible evidence loss.

Normal cancellation is followed by bounded polling. If the target is not exactly `completed`, the controller rechecks freshness and budgets before requesting force-cancellation. It then polls again and fails unless that exact `(run_id, run_attempt)` is exactly `completed`. An attempt change between census and mutation or confirmation is not silently inherited: it either re-enters reconciliation as a newly budgeted cancellation episode or fails the current reconciliation. Unknown or future statuses are non-terminal. The controller then performs another complete stable census. A newly active different-SHA attempt starts another reconciliation round using the same cumulative counters; admission requires a final stable census with no such attempt within `max_reconciliation_rounds = 3`. Exhausting the episode, request, point, time, or round budget fails admission. The watchdog omits census and round logic but applies the same one-episode freshness, request-timeout, primary-reserve, secondary-point, total-deadline, mutation, and terminal-confirmation checks to every in-progress advisory push attempt.

`terminal_status` and `event` remain in TOML and are validated as the only supported contracts, `completed` and `push`. Reconciliation consumes the validated config and contains no duplicate string literals for either fact.

## Configuration ownership

`ci/advisory-supersession.toml` is the only home for:

- API version, branch, workflow, and event;
- request timeout, exact page size, sweep interval, total reconciliation deadline, primary-rate-limit reserve, documented secondary read/mutation point weights, and secondary-point ceiling;
- platform run-lifetime, rerun-window, and search-limit contracts, lookback days, and maximum accepted search results;
- stable-sweep and maximum-sweep counts;
- cancellation-episode, total-request, and reconciliation-round limits;
- cancellation poll attempts and interval;
- terminal status.

The loader rejects unknown keys, unsupported event or terminal values, non-positive durations/counts, any `runs_per_page` other than 100, a lookback not greater than the sum of the governed run lifetime and rerun window, `max_search_results` at or above the governed search limit, a maximum-sweep count smaller than the stable-sweep count, or request/point/time budgets that cannot cover the configured minimum reconciliation topology. The controller's TOML-owned deadline is the only operational time budget. The workflow timeout remains a deliberately generous external runner kill switch and is neither mirrored nor parsed by the controller.

## Failure behavior

Every uncertainty is fail-closed:

- API errors, redirects, foreign pagination origins, malformed JSON, or missing fields fail the controller;
- a threshold breach or incomplete/unstable census fails before any evidence job starts;
- an excessive cumulative cancellation-episode count or exhausted local request, point, rate-reserve, or deadline budget fails before the next mutation;
- a 409 cancellation response is logged, then terminal state is still confirmed;
- failure to confirm terminal state after force-cancellation fails admission;
- a new stale run found by the post-cancellation stable census is reconciled only within the governed total round and request budgets;
- ref movement fails the controller or watchdog without a later mutation; the controller's downstream evidence jobs remain gated off;
- no failure changes PR, schedule, or manual-dispatch concurrency behavior.

The controller continues to use only GitHub's ephemeral `GITHUB_TOKEN`, with `actions: write` and `contents: read` confined to cancellation jobs and checkout credentials disabled.

## Verification evidence

Behavior tests must cover:

- multi-page success with exact `total_count` accounting;
- configured and canonical numeric-repository link forms, wrong numeric repository IDs, decoded query reordering/encoding, duplicate query keys, dropped or changed filters, duplicate page boundaries, missing or extra continuation links (including an exact page-multiple boundary), foreign links, repeated URLs, empty-page cycles, and total-count drift;
- threshold values immediately below and at `max_search_results`;
- the 66-day lookback and its cross-field rejection cases, using the exact-run response's HTTP `Date` rather than runner time or an old rerun `created_at`;
- delayed sentinel visibility, paced retry, permanent absence, timestamp mismatch, and exhaustion without mutation;
- active-set convergence during multiple queued/in-progress transitions, and membership changes when a run arrives or completes;
- stabilization exhaustion;
- the single worst-case budget calculator, shipped-config self-validation, cumulative `(run_id, run_attempt)` cancellation episodes across rounds, a later attempt of one run ID consuming a second episode, attempt change between census and mutation, insufficient rate-limit reserve, request/secondary-point counters, mid-reconciliation 403/429/timeout after a successful cancellation, total reconciliation timeout, and round exhaustion, with no admission or later unauthorized mutation;
- a stale run arriving after initial stabilization or during cancellation and being found by the final stable census;
- exact validation of `push` and `completed`, with unknown statuses treated as active;
- movement of `main` before discovery, during discovery, before normal cancel, during polling before force-cancel, and before admission, each failing without self-cancellation or another later mutation;
- 202 and 409 cancellation responses, force-cancel escalation, and unconfirmed cancellation;
- watchdog cancellation of a stale pre-controller first attempt and a stale rerun, plus preservation of current-main first attempts and same-SHA reruns;
- controller and watchdog exit codes;
- strict TOML key and cross-field validation.

Targeted Python tests, Ruff, actionlint, and the advisory workflow at the exact branch head are required, followed by an internal adversarial review of the exact diff. A read-only exact-head probe must exercise the real workflow-runs endpoint with a page size small enough to observe GitHub's live `Link` form; this is review evidence, not a permanent alternate workflow path. Push-path admission and the legacy watchdog cannot execute before merge; their first post-merge events remain required live evidence. The first live controller record must also reconcile the observed `total_count`, fetched count, page count, computed request budget, and remaining primary rate limit without logging credentials.

## Accepted residual risks

- GitHub does not document snapshot isolation or the exactness semantics of `total_count` for paginated workflow-run searches. Two identical incomplete responses or a consistently inaccurate count are theoretically possible. Exact count checks, duplicate rejection, repeated complete signatures, a fixed cutoff, and the sub-ceiling threshold are layered hedges rather than a platform proof.
- A push can land in the API round trip between the final ref read and a cancellation request. GitHub offers no compare-and-cancel primitive. The repeated freshness checks minimize the window; any resulting loss is visible evidence loss, not stale admission authority.
- GitHub's primary and secondary limits are shared and cannot be reserved. Cumulative local budgets prevent deliberate overuse but cannot prevent another actor or platform throttling from exhausting capacity after cancellation begins; the controller stops further mutation and never admits.
- A stale run can appear after the final stable census and before admission. GitHub offers no atomic list-and-admit primitive; the all-attempt default-branch watchdog and next push reconciliation are the asynchronous compensating controls.
- Sustained volume reaching 900 matching runs within 66 days, more than 10 cumulative cancellation episodes, or an insufficient shared API budget stops advisory admission until the governed policy is revised. This is intentional and loud.
- The first push-controller and default-branch-watchdog executions remain structurally unprovable before merge.

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
- [REST API pagination](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api)
