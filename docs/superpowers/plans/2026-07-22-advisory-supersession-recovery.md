# Advisory Supersession Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reconcile PR #1495's existing advisory admission controller and watchdog with the approved cancellation-episode, bounded-census, and fail-closed budget architecture.

**Architecture:** Keep one TOML-owned configuration and one Python controller. The client captures an exact-run sentinel and GitHub-time cutoff, performs semantically validated stable censuses, and uses a single runtime ledger/calculator for requests, secondary points, rounds, episodes, reservations, and deadline. Every run-ID mutation is authorized by a freshness-verified attempt plus a reserved immediate successor, and only the first post-mutation read can bind that reservation.

**Tech Stack:** Python 3.12 standard library, `unittest`, TOML, GitHub Actions YAML, Ruff, actionlint.

## Global Constraints

- Preserve the approved specification at `docs/superpowers/specs/2026-07-22-advisory-supersession-recovery-design.md` until the temporary-document removal gate.
- Runtime policy values live only in `ci/advisory-supersession.toml`; do not duplicate `push` or `completed` literals in reconciliation code.
- Tests assert behavior, not source structure.
- Every uncertainty fails closed; requests, mutations, episodes/reservations, and secondary points are charged before dispatch.
- The shipped worst-case calculator must equal exactly 358 requests and 438 modeled secondary points.
- Keep PR #1497 and PR #1494 out of scope; preserve #1497's disjoint runner-map change during its later mechanical rebase.

---

### Task 1: Govern the complete configuration and topology calculator

**Files:**
- Modify: `ci/advisory-supersession.toml`
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Produces: immutable `Config` fields for event, sweep pacing/deadline, API budgets, platform windows, census thresholds, reconciliation rounds, episode/reservation limits, polling, and terminal status.
- Produces: one pure topology calculator used by both `load_config` validation and runtime ledger construction.

- [x] Add behavior tests for exact shipped values, unknown keys, booleans/non-positive values, `runs_per_page != 100`, invalid lookback/search/stability relationships, and request/point/time ceilings below the computed topology.
- [x] Expand `Config` and `load_config` with strict cross-field validation and supported values `event = "push"` and `terminal_status = "completed"`.
- [x] Implement the shared worst-case calculator from configured sweeps, rounds, cancellation polls, episodes, and per-request point weights; assert 358 requests and 438 points for the governed file.
- [x] Run `python3.12 -m unittest scripts/test_advisory_supersession.py -v`; expect all configuration and calculator tests to pass.

### Task 2: Add the exact-run sentinel and fixed GitHub-time authority

**Files:**
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Produces: validated exact-run record containing positive run ID/attempt, repository identity, workflow/event/branch, SHA, status, `created_at`, and `run_started_at`.
- Produces: one reconciliation context containing the parsed response `Date`, fixed 66-day cutoff, repository ID/name, sentinel `(run_id, run_attempt)`, and absolute deadline.

- [ ] Add behavior tests for valid exact-run capture, malformed identity/attempt/timestamps, GitHub `Date` before run timestamps, old rerun creation time, and proof that the cutoff is captured once and reused.
- [ ] Add exact-run fetching and validation before census or watchdog mutation; parse RFC-compliant GitHub `Date` once and reject missing/malformed/preceding values.
- [ ] Bind repository numeric ID and owner/name from the exact response for later canonical pagination validation.
- [ ] Run the exact-run test group; expect no mutation on every validation failure.

### Task 3: Replace open-ended pagination with a bounded semantic census

**Files:**
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Consumes: fixed reconciliation context from Task 2 and the shared request ledger from Task 1.
- Produces: one complete sweep with exact `total_count`, fetched count, page count, sentinel presence, and active signature `(run_id, run_attempt, head_sha, created_at)` sorted by run ID.

- [ ] Add behavior tests for configured and numeric-repository next links, decoded query order/encoding, duplicate/dropped/changed filters, foreign origins or repository IDs, repeated URLs, empty intermediate pages, duplicate run IDs, missing/extra links, exact page multiples, total-count drift, thresholds immediately below/at 900, and page-bound exhaustion.
- [ ] Validate decoded query multimaps: exactly one governed branch, event, cutoff, and page size; exactly one positive page cursor may vary; reject every other key.
- [ ] Require non-negative stable `total_count`, unique positive run IDs, matching branch/event, exact fetched count, sentinel presence, and no continuation mismatch.
- [ ] Charge each page request and its secondary point before dispatch; cap pages at `ceil(max_search_results / runs_per_page)`.
- [ ] Run the pagination/census test group; expect malformed and incomplete sweeps to fail closed before mutation.

### Task 4: Stabilize discovery under one fixed cutoff and current-main fence

**Files:**
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Consumes: complete sweeps from Task 3.
- Produces: two identical active-subset signatures within four attempts, paced by the TOML sweep interval.

- [ ] Add behavior tests for queued/in-progress status transitions, arrivals, completions, delayed sentinel visibility, paced retry, permanent absence, stabilization exhaustion, and `main` movement before/during discovery.
- [ ] Read exact `main` before and after every sweep, reuse the fixed cutoff, and count incomplete sweeps against the configured maximum without mutation.
- [ ] Sleep only between attempts and cap sleep/request timeouts by the absolute reconciliation deadline.
- [ ] Run the stabilization tests; expect admission only after the configured stable signature count.

### Task 5: Implement the cancellation-episode ledger and one-read successor binding

**Files:**
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Produces: cumulative ledger for requests, mutations, cancellation episodes, consumed/released immediate-successor reservations, secondary points, rounds, and elapsed deadline.
- Produces: episode cancellation accepting only a freshness-verified `(run_id, attempt N)` and returning only after that bound attempt is exactly terminal.

- [ ] Add behavior tests for reservation capacity, release on the first post-mutation observation of N, consumption by exactly N+1, N+2/larger, decreased, zero, malformed, timeout, 202, 409, lost response, normal cancel, force-cancel, exhaustion, and no later rebinding.
- [ ] Before each normal/force mutation, charge and fetch current `main`, fetch and exactly validate the target, and ensure the episode plus a complete immediate-successor reservation fits every cumulative ceiling and deadline.
- [ ] Charge the mutation request and five secondary points before dispatch, including ambiguous/error outcomes.
- [ ] Permit only the first post-mutation status read to bind: N releases the reservation, N+1 consumes it as its own episode, every other observation consumes and poisons authority.
- [ ] After same-N release, route any later attempt change through a fresh episode without inherited observations, budget, or terminal state.
- [ ] Escalate to force-cancel only after a fresh main/target preflight and a new successor reservation; require exact terminal confirmation for the currently bound episode.
- [ ] Run the episode-ledger tests; expect ambiguous outcomes to prevent admission and every later mutation.

### Task 6: Reconcile rounds and final admission

**Files:**
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Consumes: stable censuses and episode cancellation from Tasks 4–5.
- Produces: admission only after all stale attempts are terminal and a final stable census contains no different-SHA active attempt.

- [ ] Add behavior tests for stale attempts arriving after initial stabilization and during cancellation, later attempts of an existing run ID, round exhaustion, request/point/rate/deadline exhaustion, and `main` movement before normal cancel, force-cancel, and final admission.
- [ ] Iterate at most three TOML-governed reconciliation rounds while sharing the single cumulative ledger and cutoff.
- [ ] Preserve the invoking run, current-main attempts, and same-SHA reruns; cancel only active different-SHA push attempts.
- [ ] On invoking-run staleness, request self-cancellation under the same charged episode rules and return the existing superseded exit code only after the bounded mutation result.
- [ ] Run controller behavior tests; expect every exhausted or uncertain path to return non-admission.

### Task 7: Apply the same episode authority to the all-attempt watchdog

**Files:**
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`
- Modify: `.github/workflows/advisory-supersession-watchdog.yml`

**Interfaces:**
- Consumes: exact-run validation and episode cancellation without census/round logic.
- Produces: preservation of current-main/same-SHA attempts and bounded terminal cancellation of every stale first attempt or rerun.

- [ ] Add behavior tests for stale attempt 1, stale rerun, current-main attempt 1, same-SHA rerun, main movement, immediate successors, force escalation, budget exhaustion, and exit codes.
- [ ] Remove the `run_attempt > 1` workflow condition while retaining workflow event/SHA/run identity validation in the controller.
- [ ] Reuse the exact same request, point, deadline, reservation, and terminal-confirmation components as the admission controller.
- [ ] Run watchdog behavior tests; expect no recursion and no mutation of a newly current target.

### Task 8: Align workflow/config wiring and run exact-head static evidence

**Files:**
- Modify: `.github/workflows/advisory.yml`
- Modify: `.github/workflows/advisory-supersession-watchdog.yml`
- Modify: `ci/advisory-supersession.toml`
- Modify: `ci/github-actions-runners.toml` only if wiring requires it
- Modify: `scripts/advisory_supersession.py`
- Modify: `scripts/test_advisory_supersession.py`

**Interfaces:**
- Produces: one workflow path for controller/watchdog operation with checkout credentials disabled and narrowly scoped GitHub permissions.

- [ ] Run `python3.12 -m unittest scripts/test_advisory_supersession.py -v`; expect zero failures.
- [ ] Run `ruff check scripts/advisory_supersession.py scripts/test_advisory_supersession.py`; expect zero findings.
- [ ] Run `actionlint .github/workflows/advisory.yml .github/workflows/advisory-supersession-watchdog.yml`; expect zero findings.
- [ ] Parse `ci/advisory-supersession.toml` and `ci/github-actions-runners.toml` with Python 3.12 `tomllib`; expect success.
- [ ] Inspect the workflow diff for push-only admission gating, all-attempt watchdog coverage, non-recursion, permissions, and `persist-credentials: false`.
- [ ] Run `git diff --check`; expect no whitespace errors.
- [ ] Perform an internal adversarial review mapping every specification bullet to a behavior test or runtime check; resolve every substantive finding before push.
- [ ] Run the read-only live workflow-runs `Link` probe at the exact head and record the observed configured/canonical URL form without logging the token.
- [ ] Record evidence not locally available: first post-merge controller event and first post-merge watchdog event.

### Task 9: Settle the implementation head and execute the removal gate

**Files:**
- Delete only after complete implementation evidence: `docs/superpowers/specs/2026-07-22-advisory-supersession-recovery-design.md`
- Delete at the same temporary-document gate: `docs/superpowers/plans/2026-07-22-advisory-supersession-recovery.md`

**Interfaces:**
- Produces: behavior-neutral code/config/workflow/test-only final head ready for fresh independent review.

- [ ] Confirm all lasting contracts live in code, tests, TOML comments, workflow sequencing, or controller diagnostics.
- [ ] Delete the temporary spec and this plan in a behavior-neutral commit with no other file changes.
- [ ] Rerun every Task 8 static/native check against the new exact head.
- [ ] Push with plain `git push`, report the exact SHA, and detach without waiting on advisory CI.
- [ ] Request the native required reviewer only when the tree is clean, all local findings are resolved, the head is pushed, and all review comments are answered.
