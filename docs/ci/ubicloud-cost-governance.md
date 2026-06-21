# Ubicloud Runner Cost Governance

Issue: #648
Measured: 2026-06-12
Slice 2b remeasured: 2026-06-13

## Scope

Remote-first Rust verification remains the invariant:

1. Run cheap local checks.
2. For Rust debugging, run `just rust-probe suggest` and choose the smallest targeted remote probe.
3. Commit and push.
4. Open or update a PR.
5. For final proof, use exact-head PR CI evidence through `just verify-remote`.

This policy does not move broad Rust verification back to local cargo and does not weaken the required final-head green CI gate.

## Meter

Use the runner-minute meter for GitHub Actions evidence:

```bash
just ci-runner-minutes --repo <owner>/<repo> --run-id <run-id>
just ci-runner-minutes --repo <owner>/<repo> --days 1 --json
```

The meter reads workflow runs, jobs, artifacts, and PR draft events through `gh` API data. Runner labels, metered workflows, and meter API limits are mapped from `ci/github-actions-runners.toml`; the script does not carry a second runner-label or page-limit registry. The configured meter workflows are `ci`, `backtester_ci`, and `ci_runner_debug`.

Fingerprint evidence is provenance-based. Runs before this instrumentation have no `nextest-archive-fingerprint-*` artifact, so the meter reports them as `fingerprint-unknown` instead of reconstructing cache keys from logs or historical checkouts. The meter reads the fingerprint from the artifact name; the workflow publishes the same GitHub-evaluated fingerprint as a `test-archive` job output for reuse decisions and as the artifact name/body for measurement before repo-controlled setup or build steps run. If one run ever publishes multiple matching fingerprint artifacts, the meter reports `fingerprint-ambiguous` and does not choose one.

### Meter Limitations

- `cancelled-superseded` is an inference, not a GitHub API field. The meter only emits it for fetched pull-request runs with the same resolved PR number and workflow when the newer run was created before the cancelled run finished. A single `--run-id` without the replacing run cannot establish supersession.
- Targeted `--run-id` reports are for forensics and fetch the requested runs directly. They do not apply the configured workflow allowlist, so do not mix arbitrary run IDs into governance totals.
- Draft-stage classification depends on GitHub PR timeline data. The fallback lookup uses the run's `head_repository` owner when GitHub does not include `workflow_run.pull_requests`; if GitHub omits both the PR list and head repository owner for a fork, draft state is unknown instead of guessed. The fallback PR lookup paginates with `meter.api_limits.branch_pull_requests_per_page`, then selects only PRs whose lifetime contains the run time. Draft timeline reconstruction reads `meter.api_limits.draft_timeline_items` ready-for-review and convert-to-draft events; if GitHub reports more events, the run is marked `draft-timeline-truncated` and excluded from `draft-stage` bounds instead of guessing from current PR draft state. If GitHub returns no usable timeline payload for a resolved PR, the run is marked `draft-timeline-unavailable` and also excluded from those bounds.
- Lookback reports issue one or more GitHub API requests per included run for jobs, artifacts, PR metadata, and draft events. Keep lookback windows bounded when using the meter interactively.
- Runner-minutes for labels absent from `ci/github-actions-runners.toml` are reported under `unknown` so reconciliation gaps stay visible.

The nextest cache/fingerprint expression remains inline in `.github/workflows/ci.yml` because GitHub Actions evaluates `hashFiles(...)` inside workflow YAML. The hygiene verifier enforces structural identity across the cache restore key, cache save key, fingerprint file, and fingerprint artifact name so version, shard, or input drift fails CI.

Fingerprint reuse is available only on pull request runs. It is disabled on pull requests that change the workflow, setup action, runner/provenance config, provenance resolver, or the resolver/hygiene self-tests. Those PRs, plus branch `workflow_dispatch` full-CI runs, must run the normal nextest archive lane so PR-controlled reuse logic cannot decide to skip test execution outside the diff guard.

## Baseline Evidence

Representative successful PR CI run: `27400354248`

| Tier | Runner-minutes |
| --- | ---: |
| `managed_heavy` | 71.600 |
| `managed_light` | 3.950 |
| `github_hosted` | 1.167 |

Heavy jobs in run `27400354248`:

| Job | Runner-minutes |
| --- | ---: |
| source-fence | 2.900 |
| nextest archive | 5.050 |
| check-aarch64 | 0.150 |
| build | 6.317 |
| nextest shard 1 of 4 | 11.683 |
| nextest shard 2 of 4 | 15.467 |
| nextest shard 3 of 4 | 15.933 |
| nextest shard 4 of 4 | 14.100 |

The four test shards total 57.183 managed-heavy minutes, about 80% of the managed-heavy minutes for that run.

Representative main CI run: `27401442229`

| Tier | Runner-minutes |
| --- | ---: |
| `managed_heavy` | 54.766 |
| `managed_light` | 4.834 |
| `github_hosted` | 0.666 |

Backtester CI examples:

| Run | Event | managed_heavy | managed_light |
| --- | --- | ---: | ---: |
| `27400354241` | pull_request | 3.733 | 0.934 |
| `27401442200` | push | 2.917 | 0.850 |

One-day filtered lookback generated at `2026-06-12T08:45:37Z`:

| Metric | Value |
| --- | ---: |
| Included runs | 69 |
| `managed_heavy` minutes | 2011.821 |
| `managed_light` minutes | 186.901 |
| `github_hosted` minutes | 35.688 |
| Debug sessions | 0 |
| CI runs with known fingerprint | 0 |
| CI runs missing fingerprint | 34 |
| Fingerprint-identical runs | 0 |

The current meter emits `lever_b_bounds`. For this baseline, the bounds below are derived from the same one-day run set and cancellation rows:

| Bound | managed_heavy | managed_light |
| --- | ---: | ---: |
| `draft_stage` | 709.484 | 64.183 |
| `draft_stage_cancelled_superseded` | 53.034 | 4.016 |

## Cancellation

Superseded PR cancellation is working.

| Run | Classification | managed_heavy | managed_light |
| --- | --- | ---: | ---: |
| `27400031109` | cancelled-superseded | 24.152 | 3.833 |
| `27362502913` | cancelled-superseded | 53.034 | 4.016 |

For run `27400031109`, the four nextest shards started between `2026-06-12T07:02:14Z` and `2026-06-12T07:02:20Z`, then cancelled at `2026-06-12T07:05:02Z` or `2026-06-12T07:05:03Z`. The replacing successful run `27400354248` was created at `2026-06-12T07:04:58Z`.

The workflow concurrency group remains PR-scoped and cancellation remains limited to pull request events. Main and tag pushes do not cancel, by design.

## Control Comparison

| Control | Decision | Reason |
| --- | --- | --- |
| Ubicloud-side cap | Recommended if available, not verified in this session | This is the only hard cap that cannot be bypassed by agents or operators. Dashboard access was not available from this session, so no setting was changed. |
| GitHub workflow concurrency | Keep current topology | Fresh evidence shows superseded PR runs cancel promptly. Redesign belongs only to Lever A or B after quantification. |
| Operator session policy | Adopt now | The spend multiplier is active sessions times surviving pushes. Policy is immediately enforceable without CI topology risk. |
| Runner tier adjustment | No-go now | The dominant test work is CPU-bound nextest archive execution. Downgrading likely trades lower rate for longer wall time and risks disk-pressure regressions. |

If Ubicloud exposes a per-repo or project runner/vCPU cap, set the first cap to allow at most two full CI runs at once: 8 concurrent `managed_heavy` runners, or 32 `managed_heavy` vCPUs if the cap is vCPU-based. Keep `managed_light` at 4 concurrent runners or 8 vCPUs. This queues excess verification instead of silently multiplying spend.

## Operator Policy

- Keep at most two active full remote-verification PRs/sessions running at once.
- Batch small edits locally and push once for verification rather than pushing every small change.
- Do not use CI pushes as a formatting loop; run local non-compile gates first.
- Current `CI_DEBUG_SSH_WAIT_MINUTES` is `30`. Do not raise it for normal debugging; cancel `ci-runner-debug` runs immediately after use.
- During the first week, run the meter daily and compare the runner-minute trend to the Ubicloud dashboard. After the first week, run weekly or before/after CI topology changes.
- Default to a **draft** PR while iterating. Draft pushes defer the full-CI merge proof — the heavy build/test lanes skip and the gate publishes `gate-deferred`, so a draft cannot merge — though always-on feedback (clippy on `managed_heavy`, deny on `managed_light`) still runs (see [Lever B](#lever-b-full-ci-on-demand)). Iterate on it freely. Mark the PR ready only when its head is the intended merge candidate; `ready_for_review` then triggers the full-CI merge proof on that exact head SHA. This is a major run-volume lever: draft-stage was ~26% of `managed_heavy` minutes in the slice 2b meter (2374.694 / 9023.518) — an upper bound that mixes always-on clippy minutes with full-proof dispatches, i.e. heavy work spent on intermediate commits a later push replaces.
- Do not push exploratory or fixup commits to a **ready** PR. Each push re-runs full heavy CI on the new SHA and a prior green does not carry over, so return the PR to draft (or keep iterating on draft) until the next coherent slice.
- Treat `just verify-remote` / full CI as a final-proof run once per coherent slice, not a debug loop. Use draft deferred runs or `just rust-probe` (max two, per the [Rust Probe Policy](../../AGENTS.md#rust-probe-policy)) for mid-iteration feedback rather than repeated full dispatches.

## Lever Decisions

### Lever A: test-result reuse by fingerprint

Decision: go. Slice A implements safe nextest archive reuse by fingerprint for `.github/workflows/ci.yml`.

Post-instrumentation evidence found real duplicate nextest spend:

| Metric | Count |
| --- | ---: |
| v1 fingerprint artifacts | 130 |
| unique fingerprints | 94 |
| repeated fingerprint groups | 19 |
| reruns beyond first occurrence | 36 |
| reruns after prior successful same-fingerprint run | 32 |
| duplicate nextest shard runner-minutes | ~1,422 |

The workflow now resolves the current nextest fingerprint from the secure `nextest-fingerprint` job output after publishing `nextest-archive-fingerprint-*` for metering evidence. If a bounded search finds a newer-prior successful CI run with exactly one matching fingerprint artifact, exactly one matching CI provenance artifact, matching workflow/config digests, successful required job evidence, and the same parsed nextest fingerprint, the managed-heavy `test-archive` job is skipped. The `test` aggregate and `gate` jobs accept that path only when resolver outputs identify the reused source run, source SHA, and provenance artifact. Fingerprint reuse is disabled on `refs/heads/main` so main pushes still emit exact-SHA CI provenance for tag deploy reuse. Missing, malformed, ambiguous, expired, failed, cancelled, in-progress, wrong-workflow, wrong-OS, wrong-arch, wrong-profile, wrong-shard-count, wrong-schema, or otherwise unverifiable evidence falls back to normal full nextest archive execution.

Policy override: set `[ci_provenance.policy.override].force_full_ci = true` in `ci/github-actions-runners.toml` to force the full-CI policy path for PRs while preserving the validated fingerprint reuse path for investigation. Revert the Slice A workflow/provenance commits if reuse itself must be removed. Branch `workflow_dispatch` runs always execute the nextest archive lane. Keep `[ci_provenance.policy.override].ignore_emit_failure = false` during normal operation; it does not make cache hits proof and does not bypass the validated reuse requirement.

### Lever B: full CI on demand

Decision: go for a separate focused follow-up PR under #648 and #333.

The one-day filtered lookback measured draft-stage runs at 709.484 `managed_heavy` minutes and 64.183 `managed_light` minutes. That is enough addressable spend to justify designing an on-demand heavy-lane flow, provided the required `gate` still blocks merge until a full green run or provenance-verified reuse exists on the exact final head SHA.

Those draft-stage minutes are an upper bound for Lever B savings because they include explicit remote-first final-proof runs such as `just verify-remote` that operators would still request before merge readiness. Normal Rust debugging should use targeted `just rust-probe ...` runs instead. The defensible lower bound from the same baseline is the intersection of `draft-stage` and `cancelled-superseded`: 53.034 `managed_heavy` minutes and 4.016 `managed_light` minutes. The follow-up Lever B PR must remeasure both bounds before changing CI topology.

Slice 2b remeasurement before the topology change:

```bash
just ci-runner-minutes --repo <owner>/<repo> --days 1 --json
```

The meter generated the report at `2026-06-13T05:10:26Z`. It reported `9023.518` total `managed_heavy` minutes, `881.573` total `managed_light` minutes, and the Lever B bounds below:

| Bound | managed_heavy | managed_light | github_hosted |
| --- | ---: | ---: | ---: |
| `draft_stage` | 2374.694 | 229.330 | 54.725 |
| `draft_stage_cancelled_superseded` | 364.180 | 45.766 | 10.202 |

Slice 2b implements Lever B only for `.github/workflows/ci.yml`. `backtester-ci.yml` remains measured by the same meter, but its draft-stage policy is out of scope for this slice.

Deferred draft CI publishes `gate-deferred`, not `gate`. A draft PR push skips the heavy merge-proof lanes (build, test, check-aarch64, source-fence, nextest fingerprint/archive) but still runs the always-on lanes (clippy on `managed_heavy`, deny on `managed_light`) for feedback, and exits `gate-deferred` successfully with the operator path to use `just rust-probe suggest` for debugging, then run `just verify-remote` only for final proof or mark the PR ready. This preserves branch-protection semantics: draft feedback is not represented as merge-ready green CI because the required `gate` check is not published by deferred draft runs.

For draft PRs, `just verify-remote` is final-proof-only: it dispatches configured full CI on the PR branch when no matching full-CI run already exists, then waits on the matching workflow run for the exact pushed head SHA. That dispatched run proves branch-head confidence for the operator. It is not the normal debug loop and is not a merge-readiness substitute; merge readiness still requires the ready/non-draft `pull_request` full run and branch protection to go green on that PR state.

Draft fork PRs fail closed because upstream `workflow_dispatch` cannot safely target arbitrary fork refs. The operator message is:

```text
draft fork PRs cannot dispatch upstream full CI; mark the PR ready for review or have a maintainer move the branch into the upstream repository
```

Policy switches:

- Set `[ci_provenance.policy.override].force_full_ci = true` in `ci/github-actions-runners.toml` to make draft PRs run full CI again without reverting the whole slice.
- Keep `[ci_provenance.policy.override].ignore_emit_failure = false` during normal operation; set it only for an explicit provenance-emitter incident response where full CI evidence should not be blocked by artifact emission.
- Revert the Slice 2b workflow-policy commits if the demand-shaping behavior itself must be removed.

## Reconciliation

Dashboard/API access to Ubicloud spend was not available in this session. No dollar amount is claimed here. Until dashboard access is available, runner-minutes from GitHub Actions are the unit of record.

When dashboard access is available:

1. Run `just ci-runner-minutes --repo <owner>/<repo> --days 1 --json`.
2. Record `managed_heavy` and `managed_light` minutes for the same UTC day.
3. Compare against Ubicloud dashboard spend for the repository or project.
4. Record the dashboard total, meter total, and delta in the PR body or operations log.

## Close Status

This Slice A PR should not close #648 by itself. Remaining close requirements:

- Verify and document the Ubicloud-side cap setting path, or document from dashboard/API evidence that no such control exists.
- Reconcile one day of meter output against Ubicloud dashboard spend.
- Implement Lever B only in a separate focused PR if the owner accepts the measured go decision.
