# CI Baseline: Issue #343

Captured: 2026-05-15 KST from GitHub Actions metadata/logs.

Purpose: baseline current CI wall time, job durations, critical path, runner-minute estimate, and cache warmth before #333 topology changes. This document is measurement-only and does not change workflow behavior.

## Source State

- Epic: #333 `Epic: CI wall-time and topology cleanup`, open.
- Measurement child: #343 `CI: capture exact wall-time and Actions-minute baseline before topology changes`, open.
- Local branch: `codex/ci-333-baseline`.
- Local base: `origin/main` at `cece0f22c6b0e2a0c9141fd7325f720bff452911`.
- Root status at setup: clean worktree on `main`, then isolated worktree from `origin/main`.
- Link evidence: #333 comment [`4452104657`](https://github.com/seungpyoson/bolt-v2/issues/333#issuecomment-4452104657) and #343 comment [`4452106073`](https://github.com/seungpyoson/bolt-v2/issues/343#issuecomment-4452106073) link this baseline document and PR #345.

## Method

- Timing source: `gh run view <run> --json ... jobs`.
- Cache source: targeted `gh run view <run> --job <job> --log` excerpts.
- Raw runner minutes: sum of executed job durations.
- Rounded runner-minute estimate: each executed job rounded up to the next whole minute. Skipped jobs are counted as zero.
- Cache warmth: `warm` only when logs show cache hit/restored key; `cold miss` only when logs show `No cache found`; otherwise `unknown`.
- Cache reporting excludes jobs that do not invoke the rust-cache action (`detector`, `fmt-check`). Cache-capable jobs without captured log evidence are labeled `unknown`.
- Workflow wall time can exceed the named critical-path step because GitHub job duration includes setup, cache restore/save, artifact upload, post-job cleanup, and gate/deploy follow-on work.
- The summary table's critical-path column names the longest blocking lane; per-run detail tables below carry the exact `gate` and `deploy` status for each run.
- Evidence freshness: run `25866346320` for exact base `cece0f22` was rechecked after external review and is now completed success, so it is included below as the exact-base main-push row. Run `24623219988` is included as the #205 issue-cited same-SHA main-push pair for smoke tag run `24623274722`.

## Baseline Runs

| Shape | Run | Event | SHA | Created UTC | Updated UTC | Status | Conclusion | Wall | Critical path | Raw runner min | Rounded estimate | Cache state |
|---|---:|---|---|---|---|---|---|---:|---|---:|---:|---|
| PR, build skipped, #332 bottleneck | [25855655415](https://github.com/seungpyoson/bolt-v2/actions/runs/25855655415) | pull_request | `1cf7baae739fc8f288511cc9055d4b76adc82537` | 2026-05-14T10:41:42Z | 2026-05-14T10:52:53Z | completed | success | 11m11s | `clippy` 10m41s | 17.6 | 21 | `deny`/`test` warm cache hits; `clippy` cache state unknown |
| PR, build-affecting current shape | [25866930064](https://github.com/seungpyoson/bolt-v2/actions/runs/25866930064) | pull_request | `2300c78bbfd7a1e4551ab1ef5d794625b26dcd15` | 2026-05-14T14:51:21Z | 2026-05-14T15:11:48Z | completed | success | 20m27s | `build` 20m05s | 28.6 | 34 | `deny`/`test`/`build` warm cache hits; `clippy` cache state unknown |
| Main push exact base, cold build | [25866346320](https://github.com/seungpyoson/bolt-v2/actions/runs/25866346320) | push/main | `cece0f22c6b0e2a0c9141fd7325f720bff452911` | 2026-05-14T14:40:06Z | 2026-05-14T15:31:12Z | completed | success | 51m06s | `build` 50m46s | 83.3 | 87 | `deny` warm cache hit; `clippy`/`test`/`build` cold misses |
| Main push current completed, warm test/build | [25862551803](https://github.com/seungpyoson/bolt-v2/actions/runs/25862551803) | push/main | `fde50d3452859a51f7f27b807913b1f12697b273` | 2026-05-14T13:25:18Z | 2026-05-14T13:44:54Z | completed | success | 19m36s | `build` 19m11s | 27.7 | 33 | `deny`/`test`/`build` warm cache hits; `clippy` cache state unknown |
| Same-SHA main path for #205 | [24623219988](https://github.com/seungpyoson/bolt-v2/actions/runs/24623219988) | push/main | `a1a6be0d94e887538ebcd9afced6c94046a557d6` | 2026-04-19T06:52:43Z | 2026-04-19T07:03:11Z | completed | success | 10m28s | `build` 10m13s, then `gate` 3s | 18.3 | 24 | `deny`/`clippy`/`test`/`build` warm cache hits; older cache key shape |
| Smoke tag duplicate path | [24623274722](https://github.com/seungpyoson/bolt-v2/actions/runs/24623274722) | push/tag | `a1a6be0d94e887538ebcd9afced6c94046a557d6` | 2026-04-19T06:56:12Z | 2026-04-19T07:06:57Z | completed | success | 10m45s | `build` 10m15s, then `gate` 3s, then `deploy` 12s | 18.5 | 25 | `test`/`build` warm cache hits; `deny`/`clippy` cache state unknown; older cache key shape |
| Late source-fence failure | [25859831755](https://github.com/seungpyoson/bolt-v2/actions/runs/25859831755) | pull_request | `81e9d85f6c242cf6c73e13732da4c6f7c9d99f4d` | 2026-05-14T12:24:40Z | 2026-05-14T12:30:00Z | completed | failure | 5m20s | `test` failed | 5.7 | 10 | `deny`/`test` warm cache hits; `clippy` cache state unknown |

Notes:

- Run `25866346320` was initially observed as `in_progress`; the refreshed `gh run view` metadata now shows `status=completed`, `conclusion=success`, `updatedAt=2026-05-14T15:31:12Z`.
- The rounded estimate is approximate and was hand-recomputed from the job detail rows at commit time. It is intentionally separate from raw active runner time. All rows here are Ubuntu Linux jobs; if future topology adds non-Linux runners, runner-minute billing multipliers must be handled separately.
- GitHub log retention can expire old run logs. If any April run log returns 404 later, re-run the same-SHA main/tag measurement before using it as #205/#195 final evidence.

## Job Timing Details

### PR without build lane: run 25855655415

| Job | Result | Wall |
|---|---|---:|
| detector | success | 8s |
| fmt-check | success | 20s |
| deny | success | 31s |
| clippy | success | 10m41s |
| test | success | 5m51s |
| build | skipped | 0s |
| gate | success | 2s |
| deploy | skipped | 0s |

Critical path: `clippy`.

Observed inside `clippy`: `check-aarch64` ran from 10:42:35 to 10:47:57, then host `clippy` ran from 10:47:57 to 10:52:15. That is the sequential dual-target shape #332 targets.

Observed inside `test`: `cargo nextest run --locked` at 10:43:03; first nextest start line at 10:44:51 reported `882 tests across 45 binaries`; job completed at 10:47:49.

Cache evidence:

- `deny`: cache hit for `v0-rust-cargo-deny-deny-Linux-x64-34ce0762-7d508d2e`; restored archive size `373164931 B` (about 356 MB); restored full match.
- `test`: cache hit for `v0-rust-nextest-v2-test-Linux-x64-34ce0762-7d508d2e`; restored archive size `1612359237 B` (about 1538 MB); restored full match.

Skip meaning: `build` was skipped by the detector path decision; `deploy` was skipped because no build artifact lane ran.

### Build-affecting PR: run 25866930064

| Job | Result | Wall |
|---|---|---:|
| detector | success | 8s |
| fmt-check | success | 21s |
| deny | success | 31s |
| clippy | success | 1m21s |
| test | success | 6m07s |
| build | success | 20m05s |
| gate | success | 4s |
| deploy | skipped | 0s |

Critical path: `build`.

Observed inside `build`: `build` step ran from 14:52:31 to 15:11:35; upload completed at 15:11:37.

Observed inside `test`: `test` step ran from 14:52:46 to 14:57:39.

Cache evidence:

- `deny`: cache hit for `v0-rust-cargo-deny-deny-Linux-x64-71010953-7d508d2e`; restored archive size `375770771 B` (about 358 MB); restored full match.
- `test`: cache hit for `v0-rust-nextest-v2-test-Linux-x64-34ce0762-7d508d2e`; restored archive size `1612359237 B` (about 1538 MB); restored full match.
- `build`: cache hit for `v0-rust-cross-aarch64-build-Linux-x64-34ce0762-7d508d2e`; restored archive size `1423517900 B` (about 1358 MB); restored full match.

Skip meaning: `deploy` was skipped because this PR run does not deploy artifacts.

### Exact-base main push: run 25866346320

| Job | Result | Wall |
|---|---|---:|
| detector | success | 7s |
| fmt-check | success | 19s |
| deny | success | 31s |
| clippy | success | 10m39s |
| test | success | 20m54s |
| build | success | 50m46s |
| gate | success | 2s |
| deploy | skipped | 0s |

Critical path: `build`.

Observed inside `build`: `build` step ran from 14:41:50 to 15:30:33; upload completed at 15:30:35.

Observed inside `test`: `cargo nextest run --locked` started at 14:47:36; the job completed at 15:01:14.

Cache evidence:

- `deny`: cache hit for `v0-rust-cargo-deny-deny-Linux-x64-34ce0762-7d508d2e`; restored archive size `373164931 B` (about 356 MB); restored full match.
- `clippy`: `No cache found`; post-run saved cache size `1188287368 B` (about 1133 MB).
- `test`: `No cache found`; post-run saved cache size `1783129517 B` (about 1701 MB).
- `build`: `No cache found`; post-run saved cache size `1424043647 B` (about 1358 MB).

Interpretation for #195/#332: the exact-base main-push run is a cold-cache outlier and is the strongest evidence that missing or stale cache state can dominate current main/tag cost. Compare it separately from the warm-cache main row below.

Skip meaning: `deploy` was skipped because this main push was not a tag deploy event. It does not affect main-push timing comparisons.

### Main push: run 25862551803

| Job | Result | Wall |
|---|---|---:|
| detector | success | 5s |
| fmt-check | success | 22s |
| deny | success | 31s |
| clippy | success | 1m20s |
| test | success | 6m12s |
| build | success | 19m11s |
| gate | success | 3s |
| deploy | skipped | 0s |

Critical path: `build`.

Cache evidence:

- `deny`: cache hit for `v0-rust-cargo-deny-deny-Linux-x64-34ce0762-7d508d2e`; restored archive size `373164931 B` (about 356 MB); restored full match.
- `test`: cache hit for `v0-rust-nextest-v2-test-Linux-x64-34ce0762-7d508d2e`; restored archive size `1612359237 B` (about 1538 MB); restored full match.
- `build`: cache hit for `v0-rust-cross-aarch64-build-Linux-x64-34ce0762-7d508d2e`; restored archive size `1423517900 B` (about 1358 MB); restored full match.

Observed inside `test`: `cargo nextest run --locked` at 13:26:46; job completed at 13:31:49.

Skip meaning: `deploy` was skipped because this main push was not a tag deploy event. It does not affect main-push timing comparisons.

### Same-SHA main push for #205: run 24623219988

| Job | Result | Wall |
|---|---|---:|
| detector | success | 4s |
| fmt-check | success | 15s |
| deny | success | 22s |
| clippy | success | 1m06s |
| test | success | 6m13s |
| build | success | 10m13s |
| gate | success | 3s |
| deploy | skipped | 0s |

Critical path: `build`, then `gate`.

This is the #205 issue-cited `main` push run on SHA `a1a6be0d94e887538ebcd9afced6c94046a557d6`, paired with smoke tag run `24623274722` on the same SHA.

Cache evidence:

- `deny`: cache hit for `v0-rust-cargo-deny-deny-Linux-x64-b567c2b7-e9df6845`; restored archive size `360224434 B` (about 344 MB); restored full match.
- `clippy`: cache hit for `v0-rust-clippy-dual-target-clippy-Linux-x64-b567c2b7-e9df6845`; restored archive size `1136118775 B` (about 1083 MB); restored full match.
- `test`: cache hit for `v0-rust-nextest-test-Linux-x64-b567c2b7-e9df6845`; restored archive size `7479253178 B` (about 7133 MB); restored full match.
- `build`: cache hit for `v0-rust-cross-aarch64-build-Linux-x64-b567c2b7-e9df6845`; restored archive size `1437492588 B` (about 1371 MB); restored full match.

Observed inside `test`: `test` step ran from 06:55:55 to 06:59:02.

Observed inside `build`: `build` step ran from 06:53:38 to 07:03:00; artifact upload completed at 07:03:02.

Interpretation for #205: this run proves the already-green `main` side of the same-SHA pair. The tag run below proves the duplicate tag side.

Skip meaning: `deploy` was skipped because this main push was not a tag deploy event. It does not affect same-SHA main/tag comparison because the paired tag run below executes deploy.

### Smoke tag path: run 24623274722

| Job | Result | Wall |
|---|---|---:|
| detector | success | 5s |
| fmt-check | success | 17s |
| deny | success | 22s |
| clippy | success | 1m02s |
| test | success | 6m13s |
| build | success | 10m15s |
| gate | success | 3s |
| deploy | success | 12s |

Critical path: `build`, then `gate`, then `deploy`.

Paired main push: run `24623219988` above, same SHA `a1a6be0d94e887538ebcd9afced6c94046a557d6`.

Cache evidence:

- `deny`: cache state unknown; log excerpt was not captured for this older run.
- `clippy`: cache state unknown; log excerpt was not captured for this older run.
- `test`: cache hit for `v0-rust-nextest-test-Linux-x64-b567c2b7-e9df6845`; restored archive size `7479253178 B` (about 7133 MB); restored full match.
- `build`: cache hit for `v0-rust-cross-aarch64-build-Linux-x64-b567c2b7-e9df6845`; restored archive size `1437492588 B` (about 1371 MB); restored full match.

The April same-SHA main/tag pair uses the older `v0-rust-nextest-test-...` shape and a much larger archive than the May `v0-rust-nextest-v2-test-...` cache entries. This baseline records the difference for #195; it does not infer the cause. Because these paired runs are from 2026-04-19, #205 before/after work must verify toolchain, lockfile, and test-count vintage before treating them as equivalent to the May rows.

Observed inside `build`: `cargo zigbuild --release --target aarch64-unknown-linux-gnu --locked` at 06:57:07; build step completed at 07:06:31. `deploy` completed from 07:06:44 to 07:06:56.

### Late source-fence failure: run 25859831755

| Job | Result | Wall |
|---|---|---:|
| detector | success | 6s |
| fmt-check | success | 20s |
| deny | success | 31s |
| clippy | success | 1m28s |
| test | failure | 3m12s |
| build | skipped | 0s |
| gate | failure | 3s |
| deploy | skipped | 0s |

Failure evidence:

- `cargo nextest run --locked` started at 12:27:51.
- `bolt-v2::bolt_v3_controlled_connect live_node_module_only_runs_nt_after_live_canary_gate` failed at 12:29:49.
- nextest summary: `498/893 tests run: 497 passed, 1 failed, 3 skipped`; `395/893 tests were not run due to test failure`.
- `gate` failed after `test` failed.

Cache evidence:

- `deny`: cache hit for `v0-rust-cargo-deny-deny-Linux-x64-34ce0762-7d508d2e`; restored archive size `373164931 B` (about 356 MB); restored full match.
- `test`: cache hit for `v0-rust-nextest-v2-test-Linux-x64-34ce0762-7d508d2e`; restored archive size `1612359237 B` (about 1538 MB); restored full match.

Skip meaning: `build` and `deploy` were skipped after the failed `test` lane; `gate` failed because required lanes did not all pass.

Interpretation for #342: deterministic source-fence drift surfaced only inside the full `test` lane, after cache restore, nextest setup, and partial test execution.

## Child Issue State Map

| Child | Live state | Scope owner | Dependencies / blockers | Baseline consumer |
|---|---|---|---|---|
| #343 | open | Measurement only: current CI run baseline | None | This document |
| #342 | open | Fast source-fence / verifier lane before full tests | Must coordinate ownership with #332 and lint with #203 | run `25859831755` late failure |
| #332 | open | Split clippy/check-aarch64 and shard full tests | Needs #343 baseline; coordinate source-fence ownership with #342 and lint with #203 | run `25855655415` PR critical path |
| #195 | open | Preserve nextest artifacts across warm reruns | Must adapt to #332 sharding if #332 lands first | warm cache evidence in `25855655415`, `25862551803`, `24623219988`, `24623274722`; cold miss fallback evidence in `25866346320` |
| #205 | open | Same-SHA main/tag heavy-work dedup | Needs exact green main evidence and artifact/check reuse design | same-SHA main run `24623219988` plus smoke tag `24623274722` |
| #203 | open | Workflow hygiene and defense-in-depth lints | Must validate topology introduced by #342/#332/#205/#335 as they land | all topology rows |
| #335 | closed | Narrow PR paths-ignore workflow change | Delivered by PR #339 at `cece0f22`; residual work moved to #344 | not active except residual map |
| #344 | open | Residual #335 branch hygiene, dry-run docs, run evidence, pass-stub, post-epic re-baseline | Some items blocked by #332/#195/#205; branch hygiene/docs/pass-stub can proceed independently | post-#332/#195/#205 re-baseline |
| #340 | open, blocked | Move `rust-verification.toml` to neutral build path | Blocked on claude-config #677 or verified transition mechanism | path-filter safety and #335/#344 |

## Child Requirement Inventory

This section preserves the live issue-body intent for each child. It is not a reduction of scope; it names what this #343 baseline supports and what remains owned by the child issue.

### #343 - Baseline measurement

Required output:

- Exact GitHub Actions run IDs, commit SHAs, event types, timestamps, source URLs, job durations, critical path, and estimated billed runner minutes.
- Representative PR behavior and representative main/tag behavior where available.
- Cache state: mark warm only when logs prove cache hit/restored key/size; mark cold miss only when logs prove `No cache found`; otherwise mark unknown.
- Neutral findings that child issues can cite for before/after comparison.
- A linked baseline comment or document from #333 or #343.

Boundary: no workflow topology, runtime, or build behavior changes.

### #342 - Fast source-fence and verifier lane

Required future implementation:

- Add a top-level early structural lane such as `source-fence` or `structural-verifiers`.
- Run the current Bolt-v3 verifier scripts before full `nextest`: `verify_bolt_v3_runtime_literals.py`, `verify_bolt_v3_provider_leaks.py`, `verify_bolt_v3_core_boundary.py`, `verify_bolt_v3_naming.py`, `verify_bolt_v3_status_map_current.py`, and `verify_bolt_v3_pure_rust_runtime.py`.
- Run canonical source-fence filters, including `bolt_v3_controlled_connect live_node_module_only_runs_nt_after_live_canary_gate` and `bolt_v3_production_entrypoint`.
- Add the lane to the aggregate `gate`.
- Fail closed for failed, cancelled, timed-out, unexpectedly skipped, missing, or stale lane results.
- Keep the lane deterministic and about 1-2 minutes on a warm run, excluding first-run compilation variance.
- Coordinate with #332 so source-fence filters are not silently owned twice, and with #203 so the new gate invariant is linted.

Baseline support: run `25859831755` proves source-fence drift currently surfaced late inside `test`.

### #332 - Parallel heavy lanes

Required future implementation:

- Split the current serialized `clippy` job into host `clippy` and top-level `check-aarch64` jobs with independent cache keys.
- Shard full Rust tests with deterministic `cargo nextest --partition` partitions.
- Preserve one required aggregate signal that fails closed if any shard or required split lane fails, is cancelled, or is unexpectedly skipped.
- Make failing shard logs actionable with the exact local reproduction command.
- Update the managed `just test` path if passthrough partition args are needed.
- Update `ci-lint-workflow` for the specific new topology, and coordinate generic lint ownership with #203.
- Coordinate cache key shape with #195 and source-fence ownership with #342.
- Record before/after critical path with exact run IDs and job durations.

Baseline support: run `25855655415` shows `check-aarch64` and host clippy are sequential inside `clippy`; run `25866930064` shows build-required PR critical path.

### #195 - Preserve nextest artifacts across warm reruns

Required future implementation:

- Preserve the managed target directory and the test artifacts `cargo nextest` needs for a fully warm rerun.
- Prove warm reruns do not unnecessarily show `Compiling bolt-v2` test-profile rebuilds.
- Record exact cold/warm run IDs, relevant log excerpts, timing comparison, and cache archive sizes.
- Keep cache keys deterministic and invalidated by real Rust inputs: lockfile, manifests, toolchain, target triple, feature/profile, and shard topology after #332.
- Avoid unbounded cache growth, cache thrash, or weakened test gates. Missing or stale cache must fall back to a correct cold run.
- If #332 lands first, adapt artifact preservation per nextest partition shard; if #195 lands first, document the adaptation point for #332.

Baseline support: this document records current cache-hit runs and archive sizes for `25855655415`, `25862551803`, `24623219988`, and `24623274722`, plus exact-base cold cache misses and saved cache sizes from `25866346320`.

### #205 - Same-SHA main/tag dedup

Required future implementation:

- Let same-SHA smoke tags reuse already-green `main` push evidence only when the tag SHA exactly matches the green main SHA.
- Reused evidence must cover tests, build/artifact, and any structural verifier/gate lanes that exist when #205 lands.
- Fail closed if the source evidence is absent, stale, incomplete, cancelled, unexpectedly skipped, failed, or for a different SHA.
- Log the source run ID, artifact/check suite ID, and SHA used for reuse.
- Keep PR CI semantics unchanged; this issue applies only to post-merge main/tag topology.
- If artifact reuse is implemented, bind the artifact to the exact SHA and trusted main run.
- Include before/after real smoke-tag evidence proving reduced duplicate `test`/`build` work.

Baseline support: main run `24623219988` and smoke tag run `24623274722` both ran on SHA `a1a6be0d94e887538ebcd9afced6c94046a557d6`. The paired rows define the duplicate-work path: `main` already ran `test` 6m13s and `build` 10m13s, then the tag reran `test` 6m13s and `build` 10m15s before a 12s deploy.

### #203 - Workflow hygiene and defense-in-depth lints

Required future implementation:

- Keep scope to workflow correctness, defense-in-depth, and generic lint mechanisms rather than wall-time topology.
- Re-evaluate selected cleanup items such as `fmt-check` needs, lane-specific setup trimming, deploy direct/transitive `needs`, lane-existence assertions, and linter maintainability.
- Validate topology introduced by #342, #332, #205, and #335/#344 as those changes land.
- Preserve required-check semantics and deploy/tag safety.
- Ensure direct/transitive `needs` cleanup cannot let deploy run without required checks, gate, build, and source-fence/verifier lanes green or intentionally fail-closed.
- Make linter failures actionable with missing job/dependency/check names.
- Do not implement broad sharding, source-fence topology, same-SHA dedup, or pass-stub behavior here unless explicitly combined and declared.

Baseline support: all topology rows in this document provide before-state inputs for later lint validation.

### #335 - Narrow path-ignore workflow change

Live state:

- Closed as completed after PR #339 merged at `cece0f22`.
- Accepted scope is only `paths-ignore` on the `pull_request:` trigger for verified-safe paths.
- No `paths-ignore` on `push:`; tag and main pushes continue to run CI.
- Branch hygiene, dry-run table, run evidence, pass-stub, and post-speedup rebaseline moved to #344.
- Drift-detection lint was scoped and prototyped, then stripped. #333/#335 say it should be refiled only if build-input drift becomes a recurring problem.

Baseline support: #335 itself is no longer active implementation scope, but #344 consumes this baseline for residual minute work.

### #344 - Residual minute-consumption work

Required future implementation:

- Inventory non-`main` branches, classify each as active, reference-only, or dead-merged-prunable, and post a deletion plan before any deletion.
- Document representative `paths-ignore` behavior for docs-only, workflow, Rust source, managed config, lockfile, and mixed changes.
- Open a docs-only throwaway PR, capture real CI evidence that heavy lanes are skipped and the PR is mergeable, then close without merging.
- Add a pass-stub or equivalent fail-closed mechanism for future required-status-check compatibility on docs-only PRs.
- After #332, #195, and #205 land, rebaseline Actions minute consumption with run IDs and a target below 1000 min/month.

Live-source conflict to resolve before #344 implementation:

- #344's body says PR #339 shipped a drift-detection lint.
- #333 and #335's current text say that lint was stripped and should be refiled only with new justification.
- This baseline follows #333/#335 for current accepted scope and records the #344 body mismatch explicitly rather than treating drift detection as completed or silently dropping it.

Baseline support: this document provides the pre-#332/#195/#205 comparison shape that #344 should reuse after those changes land.

### #340 - Neutral CI config path

Required future implementation:

- Move `rust-verification.toml` from the agent-named directory to one neutral repo path.
- Update every repo-local consumer in the same PR: CI detector allow-list, `justfile` `ci-lint-workflow`, docs/scripts that name the path, and workflow path filters.
- Coordinate the managed Rust owner script consumer in `claude-config` before claiming completion.
- Avoid permanent dual read paths in this repo.
- Prove with `just ci-lint-workflow`, detector evidence, and exact-head CI that the new path is honored.
- Only broaden agent-directory `paths-ignore` after no build-affecting config remains there.

Current blocker: claude-config #677, unless an explicitly verified transition mechanism supersedes that blocker with a #340 and #333 update.

## Current Bottlenecks By Path

- PR with build skipped: `clippy` dominates because `check-aarch64` and host `clippy` run sequentially in one job.
- PR/main with build required: `build` dominates; observed current rows range from about 19-20 minutes with some cache evidence gaps to 50m46s on exact-base cold cache.
- Warm `test`: cache hits still spend several minutes in restore/install/nextest execution. The April smoke-tag run restored a much larger `7479253178 B` (about 7133 MB) nextest cache.
- Source-fence drift: cheap deterministic structural failure currently appears in `test`, not an early structural lane.
- Smoke tag deploy: deploy itself is short; heavy `test` and `build` work dominates before deploy, duplicating the same-SHA `main` run when the tag is pushed immediately after merge.

## Follow-On Use

- #342 should compare early source-fence lane behavior against run `25859831755`.
- #332 should compare PR critical path against run `25855655415` for build-skipped PRs and run `25866930064` if build remains required.
- #195 should compare warm nextest/cache behavior against runs `25855655415`, `25862551803`, `24623219988`, and `24623274722`, and cold-cache fallback behavior against exact-base main run `25866346320`.
- #205 should compare same-SHA main/tag behavior against paired runs `24623219988` and `24623274722`; the April rows are older and must be revalidated for toolchain/test-count vintage before final #205 evidence.
- #344 should re-baseline after #332/#195/#205 land using the same method and table shape.
