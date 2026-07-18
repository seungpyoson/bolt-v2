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
5. When Rust claims require remote proof, use exact-head PR CI evidence through `just verify-remote`; draft `verify-remote` is feedback only.

This policy does not move broad Rust verification back to local cargo. CI remains evidence governed by the changed requirement and does not grant merge authority.

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

The nextest cache/fingerprint producer lives in `scripts/nextest_fingerprint.py`, with tracked inputs declared in `ci/nextest-fingerprint.toml` and the workflow consuming the producer outputs for cache keys, fingerprint file names, and artifact names. The hygiene verifier enforces structural identity across the cache restore key, cache save key, fingerprint file, and fingerprint artifact name so version, shard, or input drift fails CI.

Fingerprint reuse is available on full-CI `pull_request` and `merge_group` consumers when the Rust-only nextest fingerprint matches a trusted archived `push` run on `main`. It is disabled for either consumer event when the branch changes the workflow, setup action, runner/provenance config, provenance resolver, or the resolver/hygiene self-tests, so branch-controlled reuse logic cannot decide to skip test execution outside the diff guard. `workflow_dispatch` runs are iteration feedback only and carry no merge authority.

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
- Default to a **draft** PR while iterating. Iterate with targeted checks such as `just rust-probe`; mark the PR ready only when its head is the intended merge candidate. This is a major run-volume lever: draft-stage was ~26% of `managed_heavy` minutes in the slice 2b meter (2374.694 / 9023.518) — an upper bound that mixes historical feedback and full-proof dispatches, i.e. heavy work spent on intermediate commits a later push replaces.
- Do not push exploratory or fixup commits to a **ready** PR. Each push can re-run heavy advisory CI on the new SHA and a prior result does not prove the new head, so return the PR to draft until the next coherent slice.
- Treat ready-PR full CI as high-cost evidence for the intended merge candidate, not a debug loop. Use draft iteration runs or `just rust-probe` (max two, per the [Rust Probe Policy](../../AGENTS.md#rust-probe-policy)) for mid-iteration feedback.
- Queue one merge-ready PR at a time with `just merge-queue <pr>`. The command checks exact remote identity and existing native-review mechanics, then posts the configured Mergify command only for `queue_as_one_wave`; blocked or inconclusive results stop there.

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

Policy override: set `[ci_provenance.policy.override].force_full_ci = true` in `ci/github-actions-runners.toml` only for an explicit break-glass pull-request investigation; checked-in config must keep the override default `false` because CI hygiene intentionally rejects a merge candidate with the override enabled. Revert the Slice A workflow/provenance commits if reuse itself must be removed. Branch `workflow_dispatch` runs always publish `gate-iteration` and skip full-CI lanes. Keep `[ci_provenance.policy.override].ignore_emit_failure = false` during normal operation; it does not make cache hits proof and does not bypass the validated reuse requirement.

### Lever B: full CI on demand

Decision: go for a separate focused follow-up PR under #648 and #333.

The one-day filtered lookback measured draft-stage runs at 709.484 `managed_heavy` minutes and 64.183 `managed_light` minutes. That is enough addressable spend to justify designing an on-demand heavy-lane flow, provided applicable Rust evidence still covers the exact final head SHA without being treated as merge authority.

Those draft-stage minutes are an upper bound for Lever B savings because they include explicit remote-first full-feedback runs such as `just verify-remote` that operators may still request before merge readiness. Normal Rust debugging should use targeted `just rust-probe ...` runs instead. The defensible lower bound from the same baseline is the intersection of `draft-stage` and `cancelled-superseded`: 53.034 `managed_heavy` minutes and 4.016 `managed_light` minutes. The follow-up Lever B PR must remeasure both bounds before changing CI topology.

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

Draft CI publishes `gate-iteration` / `backtester-gate-iteration` evidence. A draft PR push skips the root full-CI lanes (deny, clippy, build, test, check-aarch64, source-fence, nextest fingerprint/reuse, and test-archive — gated on `full_ci_required`, with runner tiers that vary). Manual `workflow_dispatch` runs are also advisory. Use `just rust-probe suggest` for debugging and request broader exact-head evidence only when the changed risk requires it.

For draft PRs, `just verify-remote` does not dispatch full CI. It directs operators to use `just rust-probe` for targeted feedback or mark the PR ready when broader exact-head evidence is warranted. Neither path replaces native review authority.

Draft PRs fail closed because `workflow_dispatch` no longer runs full CI. The operator message is:

```text
draft PRs cannot run full CI through workflow_dispatch; mark the PR ready for review to get pull_request full CI, or use just rust-probe for targeted Rust feedback
```

Policy switches:

- Set `[ci_provenance.policy.override].force_full_ci = true` in `ci/github-actions-runners.toml` only for an explicit break-glass pull-request investigation; revert it before merge because hygiene requires the checked-in default to stay `false`.
- Keep `[ci_provenance.policy.override].ignore_emit_failure = false` during normal operation; set it only for an explicit provenance-emitter incident response where full CI evidence should not be blocked by artifact emission, and revert it before merge because hygiene requires the checked-in default to stay `false`.
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
