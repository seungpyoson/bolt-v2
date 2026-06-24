# #942 — One trustworthy "is this safe to merge?" signal

*Plain-language plan. Verified against `origin/main @ 1ecb4affd`. Two pieces of this work
already shipped and are referenced, not redone here: the merge-queue evidence trigger (#957)
and retiring the custom review-gate checks (#959).*

You should be able to read sections 1–6 and section 8 top-to-bottom and approve.
Section 7 is implementer detail you can skip.

---

## 1. The problem (what you actually experience)

GitHub is supposed to give one clear answer: *"is this PR safe to merge?"* Today that
answer is unreliable. The symptoms you've hit:

- **Looks stuck the whole run.** The required `gate` check shows *"Expected — waiting for
  status to be reported"* for the entire CI run, and a re-run makes it look like it vanished.
  You can't tell *running* from *broken*. (This is the #960 experience — a normal code PR whose
  final `gate` runs last, after the slow test step, so the badge sits at "waiting" the whole time
  even though nothing is wrong.)
- **Stuck forever.** For some PRs (docs-only, no-code edits) a required check never reports at
  all, so the PR can never merge.
- **"Blocked" while everything is green.** Every check passes but the PR still won't merge.
- **Cryptic status.** Checks named `gate` / `gate-noop` / `gate-deferred` — you can't tell
  whether one truly passed, was skipped on purpose, or is stuck.
- **Stale green.** A PR can merge on checks that passed against an *old* `main`, because
  nothing forces a re-check against the latest base.

**Why it matters:** you can't trust the merge button. Sometimes it blocks good PRs (wasted
time, manual poking). Worse, in two cases a check can be green *without actually testing the
change* — so a real change could merge untested.

---

## 2. Why it happens (four root causes, each verified live)

There are exactly four required checks: `gate`, `backtester-gate`, `host-health`,
`actionlint`. Four defects sit underneath the symptoms above.

- **Defect A — a required check can pass without testing anything (unsafe).** Two
  workflows both publish a check literally named `gate`: real CI (`ci.yml`) and a cheap
  "docs-only, skip the heavy tests" stub (`ci-docs-pass-stub.yml`, still present). On a
  **mixed docs+code PR** the stub's command exits 0 and publishes `gate=success` having
  tested nothing (the same classifier *with* its forbidden flag exits 1 — that's the
  control). Because GitHub matches a required check by *name*, two same-named `gate` runs
  make the outcome ambiguous and unsafe.
- **Defect B — a required check is missing for some no-build PRs (stuck forever).**
  `host-health` is required but only produced by `ci.yml`. `ci.yml`'s `paths-ignore` is
  deliberately narrow — only root agent-docs and pure-config dirs (`AGENTS.md`, `.claude/**`,
  …), **not** `docs/**` or `**.md` (verified, `ci.yml` L23-42) — so most docs PRs *do* run
  `ci.yml` and get `host-health`. But a PR touching **only** that narrow ignored set produces
  no `host-health` (the stub doesn't either) and waits forever.
- **Defect C — stale green is open right now (unsafe).** The CI-gates ruleset has
  `strict_required_status_checks_policy = false` (verified). Required checks can be green
  against an old `main` and still merge after `main` moved, with no re-run.
- **Defect D — the required signal is late, unstable, and illegible (the read side).** Two
  things. **(i) Late + churn-prone.** `gate` and `backtester-gate` come from a **summary job that
  only starts after every heavy lane finishes**, so the required check is absent — shown as
  "Expected — waiting for status" — for the *whole* run; and because each new event cancels the
  in-flight run (concurrency), a busy PR can keep superseding its own late gate before it
  finalizes. **This is live on PR #960** (a *code* PR — it touches `scripts/**` and
  `.mergify.yml`): every heavy lane is already green, but the gate is still "Expected" because the
  summary job is waiting on the slow `nextest archive` lane, and two earlier runs were cancelled —
  so it *looks* stuck even though it isn't broken and will finalize on its own. **(ii) Per-event
  renaming.** The check **computes its name from the policy path**, so a no-code edit publishes
  `gate-noop` and a draft toggle publishes `gate-deferred` (verified via `ci_provenance.py
  ci-policy`). A PR that only ever sees no-code/draft events with no full run on that commit then
  has its required `gate` stranded on "Expected" with **no run that will ever produce it**.
  Nothing maps the head's required checks to their live status.

**Smaller structural issues (folded into the steps below):** `gate` and `actionlint` aren't
locked to the one app allowed to report them (`integration_id: null`, while
`backtester-gate`/`host-health` pin app `15368`); the gate **verdict is re-derived in a long
shell guard inside the job** *and* in Python, two copies that can drift; **two overlapping
rulesets** carry conflicting pull-request settings; a dead `is_mergify_temp_pr` field lingers.

**Already shipped — referenced, not redone:** `backtester-gate` now runs on the merge queue
(#957); the custom review-gate checks and their whole surface are retired (#959), so **no
custom commit-status publishers remain** (verified) — the "one contract" is now just ruleset
contexts + Actions check-runs.

---

## 3. The fix, in plain terms

**Core idea — a "coverage map" that CI enforces, the required gate reported in-workflow under
one stable name, and a progress comment for liveness.** Write down, in one machine-readable
place, a table that says: *for every required check and every way a PR can arrive, exactly one
thing proves it, and it only turns green with real proof.* Add a CI check that fails if reality
drifts from that table. Make the gate job emit its required name **stably** (so it's never
stranded or cryptic), and post a small **auto-updating PR comment** so you can always tell
*running* from *stuck*. The filled-in table is **section 4**.

Settled decisions:

- **The required gate is produced in-workflow under a stable, literal name — no separate
  watcher.** Today the gate job renames itself per policy path (`gate-noop` / `gate-deferred`),
  which strands the required `gate` on no-code/draft-only PRs (and is cryptic on all of them).
  Instead it emits the literal `gate` (and
  `backtester-gate`) on every `pull_request` / `merge_group`; manual runs keep *non-required*
  names. The verdict comes from the run's **own real lane results** (trusted GitHub data, the
  same source used today) — never a PR-produced file — so there's no forgeable green.
  `host-health` / `actionlint` already report themselves. This needs **no new privilege and no
  migration** (`gate` stays the required name; code PRs already emit it).
- **Liveness is shown by an auto-updating PR comment, not by animating the merge light.** The
  official required-check badge necessarily reads "waiting" until the late gate job runs; a
  sticky PR comment shows `running → passed/failed` meanwhile (and the lanes show in-progress
  in the checks list). We deliberately did **not** build a privileged watcher to animate the
  badge itself for every PR type — disproportionate machinery for a cosmetic badge. Nothing is
  ever stuck, fake-green, or cryptic; "running vs broken" is answered by the comment + lanes.
- **Stale green → `strict=true` now, then a Mergify merge queue as the durable fix.**
  `strict=true` is a one-call change that shuts the hole today. The durable closure is the
  **Mergify** queue, which is its own planned effort in **#929** (native GitHub queue needs
  Enterprise, unavailable on this private personal-account repo). #942 **defers the queue to
  #929** and does not duplicate it; `strict=true` stays until the queue is the sole merge
  path. The dead `is_mergify_temp_pr` field is removed.
- **Green only with proof.** A required check turns green only from *real proof* (a full run,
  or a docs-only run that proves every heavy lane was skipped) or from *mirroring a prior
  proof on the same commit* (a no-code edit / draft toggle carries the previous green forward
  — never a fresh one). Manually-triggered runs keep *non-required* names, so they can't
  satisfy the gate.
- **Oversized PRs stay fail-closed.** A PR too large for GitHub to deliver `pull_request`
  events can only run manually (a *non-required* name) — so it can't satisfy the gate. An
  operator runbook covers the rare case rather than adding a manual green path.
- **Your approval gate is untouched.** GitHub still requires code-owner approval + thread
  resolution on every merge. Retiring the *custom* review checks already happened in #959 and
  is out of scope here.

---

## 4. The coverage map (the evidence)

Every required check × every way a PR reaches the gate, the **one proof source**, and whether
its green means real testing. Each cell verified from live workflow triggers and the policy
resolver's own output. (`gate`/`backtester-gate` are reported by their own in-workflow summary
job under a stable literal name [step 4]; `host-health`/`actionlint` report as their own lane
check-runs. This table is the single *proof source* per arrival.)

| Required check | Docs-only PR | Code PR | Mixed PR | Merge queue | Oversized PR (manual) |
|---|---|---|---|---|---|
| `gate` | stub ✓ *(skip ok)* | `ci.yml` ✓ | **`ci.yml` + stub ✗ (Defect A)** | `ci.yml` ✓ | none ✗ *(non-required name, fail-closed)* |
| `backtester-gate` | `backtester-ci` ✓ | `backtester-ci` ✓ | `backtester-ci` ✓ | `backtester-ci` ✓ *(#957)* | none *(fail-closed)* |
| `host-health` | **none ✗ (Defect B, narrow ignored-set only)** | `ci.yml` ✓ | `ci.yml` ✓ | `ci.yml` ✓ | none *(fail-closed)* |
| `actionlint` | `actionlint.yml` ✓ | `actionlint.yml` ✓ | `actionlint.yml` ✓ | `actionlint.yml` ✓ | n/a |

**What the table proves:** the proof-source holes are *exactly two* — Defect A (`gate` ×
mixed) and Defect B (`host-health` missing only when a PR touches **solely** the narrow ignored
set) — plus the deliberate oversized-PR fail-closed cell. The merge-queue hole
(`backtester-gate`) is already closed by #957. No fourth surprise.

*This table is today's **evidence** (it shows the two live holes). Step 2 turns it into the
**target** registry — per required check: name, the **one** proof source, the **app allowed to
report it** (`integration_id`), how it's reported (the job's own check-run under a stable literal
name), the arrivals it must cover, and the proof rule. Step 6's enforcer adds a **fifth**
required context (itself, self-exempt).*

**Carry-forward is a state, not a proof source.** No-code edits and draft toggles don't
re-test; they rely on a prior full proof on the *same commit*. With `strict=false` that prior
green can outlive a moved base — the stale-green vector that step 1 (`strict=true`) and step 8
(queue) close. The gate job treats these paths as "prior required success still binding + base
fresh," never as a fresh proof.

---

## 5. The plan (eight steps, in order)

Correctness (is green actually safe?): steps 1, 3, 4, 6, 7, 8. Read side (is the signal honest
and readable at any moment?): step 5 (plus step 4's stable name). **Landing order:** steps **1
and 5 can start now**; the registry (2) lands before its dependents (3, 4, 6); step 3 (one proof
*source*) lands before step 4 (one *reporter* under a stable name); steps 7–8 last.

1. **Close stale green now.** Set `strict_required_status_checks_policy = true` on the CI-gates
   ruleset — one API call, independent of all code, shuts Defect C immediately. Capture
   before/after + rollback. Kept until step 8 makes the queue the sole merge path.
2. **Build the coverage map as a machine-readable registry** (promote section 4): per required
   check — name, the **one** proof source, the **app allowed to report it** (`integration_id`),
   how it's reported (the job's own check-run under a stable literal name), the arrivals it must
   cover, and the proof rule. This is what step 6 enforces.
3. **One proof source per check** (closes Defect A + B). Fold docs-only handling into `ci.yml`
   and **delete `ci-docs-pass-stub.yml`**, so a docs-only PR runs `ci.yml` (heavy lanes
   skipped, `host-health` runs) and there's exactly one proof source per arrival. High blast
   radius — design rules in section 7a.
4. **Stable, single-sourced required gate** (closes Defect D **part ii** — per-event renaming,
   cryptic names, and the no-code strand). Make `gate` / `backtester-gate` emit their **literal**
   required name on every `pull_request` / `merge_group` (never a `-noop`/`-deferred` sibling);
   manual runs keep *non-required* names. **This is not a blind rename:** the `-noop`/`-deferred`
   names are today's safety device (they avoid overwriting the required `gate` with a green a
   no-code event didn't earn), so the literal `gate` may turn green **only** from real proof or a
   *verified* same-commit carry-forward (section 7b). Consolidate the verdict logic (today in a
   shell guard *and* in Python) into **one module + a parity test**, keep deriving it from the
   run's trusted lane results, and delete the dead `is_mergify_temp_pr` field. In-workflow, no new
   privilege, no migration. Design in section 7b. *(This does not make the badge appear earlier —
   that "looks stuck" symptom of part i is step 5's job.)*
5. **Progress visibility** (the "is it running or stuck?" half of Defect D **part i** — the #960
   experience: a green-lane run whose late gate looks stuck). An in-workflow step upserts a
   **sticky PR comment** mapping the required checks to `running → passed/failed` (and "stalled"
   if a run dies), so you can always tell live from broken; the lanes also show in-progress
   meanwhile. Posted from a small job that **never runs PR code**. Optionally the same state is a
   `scripts/merge_readiness.py <pr>` command. No privileged watcher. Design in section 7b.
6. **Add the coverage-map enforcer** — a standalone workflow, no path filter, runs on every PR
   event and on the merge queue, **registered as a required check** (a fifth, self-exempt
   context). It fails if the live reporters/proof sources drift from the registry.
7. **Consolidate the two overlapping rulesets into one**, keeping the stricter pull-request
   settings (`dismiss_stale=true`, `last_push_approval=true`, code-owner, thread-resolution),
   **and pin each required context to the GitHub Actions app (`15368`)** — `gate` and
   `actionlint` are unpinned today (`integration_id: null`) while `backtester-gate` /
   `host-health` already pin `15368`; pin all four. Confirm with a test-merge; write a rollback.
   *(The custom review-check retirement that used to live here is done — #959.)*
8. **Durable stale-green closure — the Mergify merge queue, via #929.** #942 does not build the
   queue; it **defers to #929** (the Mergify plan) and only ensures every required check (incl.
   step 6's enforcer) reports on the queue commit — `backtester-gate` already does (#957). When
   #929's queue is the sole merge path, `strict` can relax.

---

## 6. Scope guard (what this is NOT)

- Not changing *what* the tests test.
- Not weakening your approval gate — it stays required; the custom review checks were already
  retired separately (#959).
- **Not building a privileged watcher** to animate the merge light to "running." The badge reads
  "waiting" until the run finishes; live progress is the step-5 PR comment. (A `workflow_run`
  watcher was considered and dropped — it would need its own verdict logic for CI *and*
  backtester, race-handling, and a one-time migration, all to animate a badge.)
- **Not building the merge queue here** — that's #929's Mergify work; #942 only makes the
  required checks queue-ready and defers the queue itself.
- **Not replacing the lane checks** — `host-health`/`actionlint` keep reporting themselves.
- Covers **PR events + the merge queue** — not direct pushes/tags, which aren't gated by
  required status checks.
- **Oversized PRs stay fail-closed.** They can't satisfy the required gate; an operator
  runbook covers the rare case rather than adding a manual green path.

---

## 7. Implementer design notes (skippable for approval)

The high-risk step is 3 (folding docs into `ci.yml`); step 4 is a contained naming + verdict
change. Everything here fails *closed* — a missing piece blocks the PR rather than
green-lighting it.

### 7a. One proof source per check (step 3)

The stub exists for a real reason: docs-only PRs must not run the heavy Rust lanes. You can't
fix Defect A by "tightening the stub" — a job that abstains via `if:` still emits a *skipped*
check, so two same-named sources persist. The only clean shape is to fold docs handling into
`ci.yml`: remove its `paths-ignore`, add a **`docs`** policy path where the heavy lanes skip,
`host-health` runs (closing Defect B), and the run records (via its provenance) that every
heavy lane was skipped — a legitimate docs-only proof. Then delete the stub.

Mandatory hardening (each re-verified live):

- **C1 — trust boundary.** Decide docs-only from the **trusted base tree**, not the PR head;
  else a PR could edit the classifier to declare *itself* docs-only and skip all proof. One of
  two fail-open vectors.
- **C2 — one source for the safe-path set + keep the guard.** The classifier today derives safe
  paths by reading `ci.yml`'s `paths-ignore`; removing that makes it crash. Move the safe-path
  set into **one config registry** consumed by both the classifier and the hygiene verifier (no
  hardcoded literal). **Preserve `FORBIDDEN_IGNORED_BUILD_PATHS`** — the guard that forces full
  CI if a build-input path lands in the safe set. Keep it narrow; never `docs/**` or `specs/**`.
  Update the existing hygiene scripts that hardcode these (`verify_ci_path_filters.py` —
  `EXPECTED_SAFE_PATHS`, `REQUIRED_PASS_STUB_JOBS`, and its stub reference — and
  `verify_ci_workflow_hygiene.py`) to read the registry and the new docs path, or step 3's own
  CI fails.
- **C3 — precedence.** `docs` is a pull-request-only override applied *after* the event branches
  and only to what would otherwise be `full`. It must **never** override the merge queue,
  push/tag, manual dispatch, or carry-forward. The critical case: a docs-only diff entering the
  **merge queue must stay full** (else it merges untested).
- **C4 — backtester compatibility.** The resolver is shared; `docs_only` defaults **false** and
  is passed only by `ci.yml`, so the backtester's truth table is unchanged.
- **C5 — explicit skip-assertion.** The docs path must assert *every* heavy lane resolved
  `skipped` (the second fail-open vector — else a stray lane that ran and failed is ignored).
- **C6 — classification placement.** Reuse the existing base/head fetch rather than adding a new
  network call to the metadata-only policy job.

`host-health` is **not** gated by `gate` — it blocks independently, so a docs PR with green
`gate` but red `host-health` is correctly blocked. C1 and C5 are jointly critical and tested
together on the live matrix (§7c).

### 7b. The in-workflow required-check fix (steps 4–5)

Two parts, both inside the existing workflows — no separate privileged watcher.

**Stable literal name (step 4 — closes Defect D part ii: per-event renaming, cryptic names, and
the no-code strand).** Today the gate summary job's check-run name is computed from the *policy
path* (`gate` / `gate-noop` / `gate-deferred`), so a no-code edit or draft toggle emits a name
that is **not** the required `gate`; a PR that only ever sees such events (no full run on that
commit) leaves the required `gate` stranded on "Expected." Fix: compute the name from the
**event**, not the policy path — `pull_request` and `merge_group` always emit the literal `gate`
(and `backtester-gate`); only manual `workflow_dispatch` keeps a *non-required* name
(`gate-dispatch` / `gate-iteration`) so feedback runs can't satisfy the gate.

**Critical — this is not a blind rename (a blind rename would create a fake-green).** The
`-noop`/`-deferred` names are a deliberate safety device: publishing under a *different* name is
how a no-code event avoids overwriting the required `gate` with a green it didn't earn — the
required `gate` is meant to persist from the prior real run on the same commit. So once the gate
job emits the literal `gate` on these paths, it must turn it green **only** from real proof (a
full run that passed, or a docs-only run with every heavy lane verified-skipped) or from a
*verified* carry-forward (a prior `gate=success` exists for this exact head SHA **and** the base
is fresh); otherwise the required `gate` stays non-success (blocked) — never a bare `exit 0`. The
verdict source is unchanged: the run's **own real lane results** (`needs.*.result`, trusted
GitHub data), not the PR-produced provenance file — so there's no forgeable-green surface.

Consolidate the verdict logic (today duplicated in a shell guard and in Python) into one module +
a parity test, and delete the dead `is_mergify_temp_pr` field. **No migration window:** `gate`
stays the required name; code PRs already emit it (so the step-4 PR proves itself); only
no-code/draft PRs change — strictly for the better. Note: step 4 does **not** make the badge
appear *earlier* — the summary job is still late; that "looks stuck" symptom (part i, the #960
case) is step 5's comment.

**Progress comment (step 5 — the "is it running or stuck?" half of Defect D part i, e.g. the #960
case).** The required badge necessarily reads "Expected/waiting" until the late summary job runs
(it is the single in-workflow producer). To show liveness without a privileged watcher, an in-workflow step
upserts **one sticky PR comment**: `⏳ CI running — N/M checks done` → `✅ all required checks
passed — safe to merge` / `❌ failed: <which>` (and `⚠️ stalled — no progress in N min` when a
run dies). The individual lanes also show "in progress" in the checks list meanwhile. Post it
from a **small dedicated job that never checks out or runs PR code**, least scope
(`pull-requests: write` only). Read-only **Dependabot** PRs can't post a comment from their own
run (rare, low-stakes) — they fall back to the visibly-running lanes. Optionally the same state
is a `scripts/merge_readiness.py <pr>` command.

**The honest trade.** The official required-check badge stays "waiting" until the run finishes —
we deliberately did **not** build a privileged `workflow_run` watcher to animate it to "running"
for every PR type (it would need its own verdict logic for CI *and* backtester, race-handling,
and a one-time migration — disproportionate to animate a badge). Nothing is ever **stuck**
(step 4), **fake-green** (verdict on trusted lane results), or **cryptic** (one literal name);
"running vs broken" is answered by the comment + lanes.

### 7c. Verification — the live scratch-PR matrix (#942's required cases)

Run #942's named cases and, for each, record `gh pr checks`, the check-runs API, the active
ruleset contexts, and the PR UI: **ready push, draft push, ready_for_review, edited
no-base-change, edited base-retarget, reopened, workflow_dispatch iteration, workflow_dispatch
full, re-run, cancelled/superseded run, and a merge-queue PR** — plus the step-3 fail-open tests
(docs-only, mixed, docs↔mixed transitions, merge-queue-on-docs stays full, and a PR that edits
the classifier itself cannot self-whitelist). Prove, for every PR / merge-queue arrival: the
required `gate` / `backtester-gate` appear under their **literal** names and resolve to pass/fail
(never stranded on a `-noop`/`-deferred` sibling); a no-code / draft edit carries the prior
same-commit proof forward (never a fresh green); a cancelled run leaves the gate non-success
(blocked); a manual dispatch stays non-required; missing heavy proof leaves merge blocked; and
the progress comment goes `running → passed/failed` and never sits on a stale "running" while
the lanes are also dead.

---

## 8. Coverage against #942's "Done means"

| #942 "Done means" | Delivered by |
|---|---|
| Required contexts visible **early** as pending/running, not only after terminal jobs | **Partially, by design (owner decision).** The required-context badge reads "waiting" until the late gate job runs; early progress is shown by an **auto-updating PR comment + the visibly-running lanes** (step 5). Animating the badge itself was dropped as disproportionate (it needs a privileged watcher) |
| Dynamic policy paths fail closed **but** explicit/understandable | Step 4 (one **literal** name per event; carry-forward mirrors a prior same-commit proof; verdict from trusted lane results, fail-closed) + step 5 (comment) |
| **Cancelled/superseded** runs don't confuse the signal | Step 4 (the gate job resolves from its own run; a cancelled run = non-success = blocked; a newer push's run supersedes the old) + step 5 |
| Ruleset contexts, check-runs, custom statuses are **one documented contract** | Steps 2 + 6 (registry + enforcer) + step 7 (pin each context to app `15368`) — and #959 left **zero** custom statuses, so the contract is contexts + check-runs |
| A reliable **command/report** mapping each required context → live source/status | Step 5 (the sticky PR comment, and/or `scripts/merge_readiness.py`) |
| Applies to the **class**, not just `gate`/`backtester-gate` | Steps 3–4 give `gate` + `backtester-gate` stable literal names; `host-health`/`actionlint` self-report; `backtester-gate`'s queue trigger (#957) completes queue coverage |
