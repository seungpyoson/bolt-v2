# BVS S3 Compiler Cache and Target-Cache Removal Design

**Status:** Draft pending Claude Fable 5/xhigh re-review. No implementation plan or code may start until that review approves this revision.

## Goal

Remove the unsafe Backtesting Vertical Slice (BVS) whole-target GitHub Actions caches that block exact-head Backtester proof for PR #1367. Preserve ordinary compile reuse through the repository's existing governed S3 `sccache`, without adding a cache backend, widening a limit, weakening a test, or changing trading behavior.

This is a separate CI blocker slice. It does not change Binance timestamps, realized-volatility pricing, the #1354 implementation, recovery, HMAC, or archival behavior.

## Incident Evidence

The current PR #1367 head has one directly verified full Backtester failure:

- Run `29185376240`, head `ff174ca186b9362a6f10239e6dd28746a188d930`, failed in `bvs-test archive`.
- The exact BVS target key missed. The broad `managed-target-bvs-v3-...-test-` fallback restored `managed-target-bvs-v3-Linux-ARM64-test-bootstrap-9f3b13f4c...`, a 6,419,854,292-byte compressed cache produced from another source/dependency state.
- The BVS nextest archive then built successfully into that restored target. Before the binary-sidecar build, the managed Rust preflight found the target above the unchanged 32 GiB soft limit and refused the command with `disk_pressure`.
- The runner still reported about 79 GB free. The refusal was caused by the managed target-size policy, not by exhaustion of the runner filesystem.
- The refusal payload reported `reclaimable_bytes: 0` even though that preflight did not classify retention candidates. It therefore could not distinguish a measured zero from an unmeasured value.

The failure repeated on the earlier PR head `a9b4fc62ea5e7f6fbf929de90ef820586ca5f47f` in full Backtester runs `29177797488` and `29180923906`. Run `29180025916` on that head was an iteration-policy run: `bvs-clippy` and `bvs-test archive` were skipped and only `backtester-gate-iteration` ran. Its success is not a successful heavy-lane counterexample.

The evidence supports one narrow root cause: a broad prefix restored a whole target tree whose compile provenance was not compatible with the current dependency graph. The logs do not prove a byte-by-byte split between old and new artifacts, and this design makes no such claim.

The 32 GiB refusal behaved correctly. The unsafe restore is removed; the limit remains.

## Storage Evidence

A read-only GitHub Actions cache inventory on 2026-07-12 contained five entries, all in the root `managed-target-v1-Linux-ARM64-test-archive-test-*` family:

- total listed bytes: `9,118,987,793`;
- repository warning threshold: `10,737,418,240` bytes (10 GiB);
- listed headroom: `1,618,430,447` bytes; and
- BVS whole-target entries: zero.

The prior 6,419,854,292-byte BVS cache had already been evicted. Reintroducing a BVS whole-target family would compete with a root family that is already churning near the repository threshold, invite eviction and repeated uploads, and transiently require old and new dependency seeds to coexist.

This slice therefore makes the steady-state BVS Actions target-cache budget exact: **zero entries, zero bytes, and zero saves**. Root-cache allocation and churn remain verified separate scope; this design does not alter or claim to solve them.

## Decision

Delete every BVS whole-target GitHub Actions restore and save from:

- the Backtester clippy job;
- the Backtester nextest-archive/sidecar producer;
- both BVS jobs in `flaky-test-detection.yml`; and
- both BVS jobs in `flaky-test-smoke.yml`.

No `managed-target-bvs-*` cache family replaces them. A BVS compile begins with the lane-owned managed target directory created by the existing setup path, never with a downloaded whole target tree.

Keep the existing `Swatinem/rust-cache` steps only in their current registry/git-only mode (`cache-targets: false`). They are not BVS target caches and must not be changed to store target directories.

Use the existing governed `./.github/actions/sccache-setup` action for the primary Backtester nextest-archive and binary-sidecar compilation:

- a push or trusted workflow dispatch on `refs/heads/main` receives the existing read/write role;
- pull requests and merge queues receive the existing read-only role;
- BVS flaky-smoke jobs remain read-only consumers and BVS flaky-detection jobs become read-only consumers; and
- an unavailable or unhealthy compiler cache fails open to the same required local compilation.

The Backtester clippy job loses its whole-target cache in this slice. This design does not add a new clippy cache path; the performance qualification below is specifically for the launch-blocking nextest-archive plus sidecar compile path.

`sccache` stores individual compiler outputs under the already governed S3 location. It does not upload a mutable Cargo target directory, does not create a new S3 target-artifact class, and does not consume GitHub's 10 GiB Actions-cache budget.

### Why this addresses both safety and latency

- A normal source-only change can reuse compatible compiler outputs.
- A dependency, toolchain, flag, or compiler change naturally misses incompatible compiler outputs and rebuilds them.
- No run can mix an arbitrary old Cargo target tree with a new dependency graph.
- If S3 or the cache action is unavailable, Rust still compiles and the full tests still run.

### Rejected alternatives

1. **Keep or narrow the BVS target-cache prefix.** A prefix is not compatibility evidence and recreates the failure class.
2. **Create exact whole-target compatibility keys.** The family does not fit the measured repository budget in steady state, including seed transitions and root churn.
3. **Key whole targets by exact source.** This is safe but makes ordinary edits cold, creates immutable per-head objects, and worsens the storage problem.
4. **Raise the 10 GiB or 32 GiB thresholds.** That hides pressure without fixing ownership and is rejected.
5. **Upload Cargo target directories to S3.** That creates a new mutable artifact class and a second target-cache contract; it is rejected.
6. **Disable all compile reuse.** This is the fail-open behavior during an outage, but not the intended steady state. The existing governed compiler cache is the bounded acceleration path.

## Cache Identities and Ownership

### Full-source nextest and sidecar artifacts

One head-resolver digest step, renamed to make its ownership explicit, computes:

```text
python3 scripts/ci_input_sets.py hash backtester_cache
```

That `backtester_cache` digest is used solely for:

- the BVS nextest-archive S3 key and metadata;
- the BVS binary-sidecar S3 key and metadata; and
- evidence that each executable payload came from the exact source/build input set.

The head version of the resolver always computes the digest. The `bootstrap-${GITHUB_SHA}` branch is eliminated from Backtester artifact identity as well as from all removed target-cache identity. When the resolver or `ci/rust-ci-inputs.toml` changes, the head resolver validates the head declaration and produces the digest used consistently by the restore key, save key, and metadata in that same run. A same-head restore is accepted only when object metadata carries the same digest. There is no mixed resolver or mixed metadata path.

The current workflow-hygiene contract is deliberately inverted and split:

- remove the rule that all BVS cache families share `bvs_cache_inputs`;
- remove the rule requiring an exact-head `bootstrap-${GITHUB_SHA}` namespace;
- require one full-source artifact-digest step for nextest/sidecar keys and metadata;
- require no BVS whole-target restore or save step in any of the three workflows; and
- require the governed sccache action and its event-specific authority on BVS compile steps.

Mutation tests must independently remove, substitute, or cross-wire each invariant so that one surviving rule cannot mask another.

### Compiler outputs

`sccache` owns compiler-output identity. The repository does not invent a parallel GitHub key or a `backtester_target_seed`. Compiler version, arguments, environment handled by the cache, and source inputs determine compiler-cache reuse through the existing action and `sccache` implementation.

The repository's authority is the existing governed location and role selection:

- `ci/sccache-location.toml` owns bucket, region, and key prefix;
- `scripts/sccache_eligibility.py` owns event/ref eligibility and read-only versus read/write roles;
- `.github/actions/sccache-setup/action.yml` owns fail-open installation and server start; and
- `.github/actions/sccache-stats/action.yml` owns post-compile statistics emission.

This design does not add delete authority, a second bucket, a new prefix, or another secret path.

### Intentional input-set asymmetry

`backtester_cache` intentionally forbids the workflow, resolver, input-set TOML, and setup action from becoming ordinary artifact-set members. Those governance surfaces are checked separately by workflow hygiene, input-set self-tests, source fences, and the reviewed action contract. The compiler cache is likewise governed by action/eligibility behavior rather than by adding those files to the executable-payload digest.

The implementation must preserve this asymmetry. It must not “fix” it by weakening `FORBIDDEN_BACKTESTER_CACHE_TARGETS` or by making the artifact digest self-referential.

## Workflow Behavior

### Primary Backtester archive/sidecar producer

1. Check out the exact head and compute the full-source `backtester_cache` digest with the head resolver.
2. Restore exact full-source nextest and sidecar artifacts from S3 under the existing metadata checks.
3. If either payload must be built, set up governed sccache once for the compilation phase. Main supplies both read and write role inputs; PR and merge-queue events supply only the read role selected by the action.
4. Compile the missing nextest archive and/or sidecars with `BOLT_RUST_VERIFICATION_SCCACHE=1` only when the action reports `enabled=true`. Otherwise the managed environment scrubs the wrapper and compiles locally.
5. Print one post-compilation stats record covering both compile steps.
6. Preserve the existing main-only, full-source S3 save and metadata rules for nextest and sidecar payloads.
7. Run the complete existing archive tests and required issue-specific tests. No partition, target, or test may be removed to improve timing.

The workflow contains no BVS target-cache restore, target-cache save, fallback prefix, target-cache digest, or target-cache hit condition.

### Backtester clippy

The clippy job retains the existing registry/git-only cache and the full clippy command. Its BVS target restore/save steps and target digest step are removed. It cold-builds its lane-owned target directory. Any future clippy acceleration requires its own measured, reviewed scope.

### Flaky detection and smoke

Both BVS flaky workflows remove their whole-target restores. Their compile jobs use the existing governed sccache action in read-only mode and print stats. Manual and scheduled jobs never receive write or delete authority. Their test matrices, partitions, JUnit behavior, MinIO smoke behavior, and Mergify reporting remain unchanged.

### Sccache outage or ineligibility

Eligibility resolution, AWS authentication, installation, or server startup may fail. The shared action remains `continue-on-error`, reports `enabled=false`, and the Rust command runs without `RUSTC_WRAPPER` through the existing managed environment.

An outage must never:

- skip clippy, compilation, sidecars, archives, or tests;
- restore a BVS target cache;
- switch to a second compiler-cache location;
- weaken the 32 GiB guard; or
- turn a required Backtester result into success without running it.

The cache is an optimization. Cold compilation is the correctness fallback.

## Actions-Cache Steady-State Contract

After rollout, the GitHub Actions cache inventory must show:

- no key beginning `managed-target-bvs-`;
- zero BVS target-cache bytes;
- zero BVS target-cache saves; and
- repository listed bytes at or below `10,737,418,240`.

Record API-backed inventory at four points: before rollout, after the implementation reaches main, and after each of two later dependency/toolchain-identical source-only heads. Those two heads must show that BVS did not reappear in Actions cache and that no BVS eviction/re-save loop exists.

The recorded 2026-07-12 baseline is five root entries totaling `9,118,987,793` bytes with no BVS entry. Changes in root-family population are reported, but root allocation and eviction policy are not changed in this slice. Because the BVS family is permanently zero, a root-family fluctuation cannot trigger a BVS whole-target re-save.

No workflow in this slice may delete cache entries. If the repository total exceeds the existing threshold because of root churn, the existing storage-audit owner reports it and a separate reviewed scope decides root allocation.

## Performance Qualification

Removing the unsafe target cache solves the correctness and merge-gate failure. `sccache` is accepted as the normal-change latency solution only when measured evidence shows at least a 20% improvement.

### Comparable measurements

Record two immutable remote runs on the same runner class, OS, architecture, Rust toolchain, dependency graph, profiles, job-count policy, and archive/sidecar commands:

1. **Exact cold baseline:** no BVS target restore exists; an immutable measurement head captured before final sccache wiring explicitly compiles without the wrapper (or an enabled run reports zero hits); both full-source S3 payloads miss; the archive and sidecars build and the full gate is green. The failed #1367 incident runs are not baselines because they restored an unowned target tree.
2. **Warm source-only run:** a later source-only head keeps dependencies and toolchain identical; both full-source S3 payloads miss because the source digest changed; governed sccache is enabled; the archive and sidecars build; and the full gate is green.

Compile wall-clock is the sum of the GitHub-reported durations of the required archive-build and sidecar-build steps. Queue time, checkout, artifact download, tests, stats, and S3 upload are reported separately and excluded. The same step names, timestamps, and formula are recorded for both runs:

```text
improvement_percent = 100 * (cold_compile_seconds - warm_compile_seconds) / cold_compile_seconds
```

The warm run must also report:

- `sccache` enabled;
- read-only or read/write mode appropriate to the event;
- nonzero compile requests;
- nonzero cacheable requests;
- nonzero cache hits;
- zero cache read errors; and
- successful nextest archive, sidecars, full tests, and `backtester-gate`.

These counters come from the existing `sccache --show-stats` output emitted into the immutable job log by `.github/actions/sccache-stats`. The qualification report records the exact run, job, step, and counter values; this slice does not add a second statistics path.

Because only trusted main runs may write, performance qualification is a post-rollout measurement. Exact-head PR proof establishes safe cold behavior and correct authority; the first trusted main compile populates compatible objects; a later dependency/toolchain-identical source-only main head supplies the warm measurement. The slice must not be reported as performance-complete before that evidence exists.

If improvement is below 20%, the no-target-cache safety correction remains valid, but the normal-change performance problem is **not solved**. Stop, report the measurements, and do not start a broader root+BVS cache-allocation overhaul without a new spec and explicit user approval. Do not raise limits, add a target cache, or reinterpret a cache hit count as wall-clock success.

## Disk-Pressure Telemetry

Two policy files independently govern managed targets and both retain the exact 32 GiB value:

- `ci/rust-verification.toml`: `soft_limit_bytes = 32212254720`; and
- `crates/backtesting-vertical-slice/ci/rust-verification.toml`: `soft_limit_bytes = 32212254720`.

The BVS-local file governs the incident lane. Static tests must pin both values so a root-only assertion cannot hide BVS drift.

`disk_preflight_refusal_payload` must report what it actually measured:

- `total_bytes`: the scanned managed target size used by the pressure decision;
- the existing filesystem, threshold, and pressure-reason fields;
- `reclaimability_measured: false`; and
- `reclaimable_bytes: null` when no retention-candidate classification ran.

Zero is reserved for a completed classification that found zero reclaimable bytes. A caller that performs the scan reports `reclaimability_measured: true` and an integer `reclaimable_bytes`.

The payload does not claim to identify which historical operation produced each byte. The target-cache removal prevents the known incompatible whole-tree restore; `total_bytes` explains any remaining refusal truthfully.

## Runner-Image Residual

The runner comes from `CI_RUNNER_MANAGED_HEAVY`; the checked-in registry owns the variable name, not a stable image digest. OS and architecture alone do not fully describe glibc or installed system-library state.

The existing `sccache` keying covers the compiler and compilation inputs it knows, but this design does not claim a cryptographic runner-image identity. If an image change makes a cached object incompatible, the expected result is a compile, link, or test failure caught by required CI, followed by a clean rebuild after the cache miss/correction—not silent acceptance of an untested executable. No reliable image identifier is currently available to add to the contract. A repeated image-specific failure requires a separate source-backed design rather than an invented key.

## Failure Semantics

| Condition | Required result |
|---|---|
| Full-source digest validation or hashing fails | Fail before S3 artifact restore. |
| Resolver or input-set declaration changes | Use the head resolver's `backtester_cache` digest; never use a SHA bootstrap namespace. |
| Exact full-source nextest/sidecar object hits with matching metadata | Restore it under the existing integrity contract. |
| Full-source object misses | Compile the missing payload. |
| S3 artifact metadata is missing or mismatched | Preserve the existing integrity failure. |
| BVS Actions target cache exists from history | Ignore it; no workflow step may restore or save it. |
| Sccache setup or remote I/O is unavailable | Report disabled/error state and cold-compile locally. |
| Sccache is enabled | Compile with the governed wrapper and emit stats. |
| Managed target exceeds either governing 32 GiB limit | Fail closed with truthful size/reclaimability telemetry. |
| Build, clippy, sidecar, archive, or test fails | Required job fails; cache status does not mask it. |
| GitHub Actions cache inventory exceeds 10 GiB | Existing storage governance reports it; this slice performs no deletion and creates no BVS target entry. |
| Warm compile improves by at least 20% | Record compiler-cache latency qualification as passed. |
| Warm compile improves by less than 20% | Do not call performance solved; stop before any broader redesign pending a new spec and user approval. |

## Exact Implementation Surface

The eventual implementation is limited to these files unless the reviewed plan proves another file mechanically necessary:

- `.github/workflows/backtester-ci.yml` — remove BVS target restore/save and SHA bootstrap identity; retain one full-source artifact digest; wire governed sccache to archive/sidecar compilation.
- `.github/workflows/flaky-test-detection.yml` — remove both BVS target restores; use governed read-only sccache for BVS compile jobs.
- `.github/workflows/flaky-test-smoke.yml` — remove both BVS target restores; preserve governed read-only sccache.
- `scripts/verify_ci_workflow_hygiene.py` — require no BVS target family, exact full-source S3 identity, correct sccache action/roles/fail-open wiring, full tests, and unchanged job authority.
- `scripts/test_verify_ci_workflow_hygiene.py` — mutation tests for every workflow invariant.
- `scripts/ci_workflow_hygiene_test_helpers.py` — only fixture changes mechanically required by the verifier tests.
- `scripts/rust_verification.py` — truthful target-size and reclaimability telemetry.
- `scripts/test_rust_verification_cache_retention.py` — telemetry behavior and both 32 GiB authority assertions.

These existing authorities are read-only unless the reviewed plan identifies a demonstrated contradiction:

- `.github/actions/sccache-setup/action.yml` and `.github/actions/sccache-stats/action.yml`;
- `scripts/sccache_eligibility.py` and its existing tests;
- `ci/sccache-location.toml`;
- `ci/rust-ci-inputs.toml` and `scripts/ci_input_sets.py`;
- both `ci/rust-verification.toml` policy files;
- `ci/storage-tripwire.toml`, storage-audit scripts/workflow, and storage governance docs; and
- `.github/workflows/ci.yml` and every root managed-target cache.

The existing shared sccache actions remain unchanged. Their exact-head log output supplies the performance counters; the implementation may not replace them with inline credentials, a second statistics path, or an alternate cache action.

No source, Rust test target, manifest, lockfile, runtime config, strategy, pricing, evidence, recovery, HMAC, or archival file belongs in this CI slice.

## Invariants

1. All three workflows contain zero BVS whole-target restore and save steps.
2. GitHub Actions stores zero BVS target entries and bytes after rollout.
3. The primary compile path uses the existing governed sccache action; main is read/write, PR/MQ and flaky consumers are read-only.
4. Cache failure falls open only to the same required local compilation, never to a skipped job or alternate cache.
5. Full-source nextest and sidecar keys and metadata use one head-resolver `backtester_cache` digest.
6. No `bootstrap-${GITHUB_SHA}` or other commit-SHA cache namespace remains in the BVS contract.
7. Full tests, partitions, sidecars, issue-specific coverage, and S3 metadata validation remain intact.
8. Both 32 GiB target limits and the 10 GiB Actions-cache threshold are unchanged.
9. Disk-pressure telemetry distinguishes measured target bytes from unmeasured reclaimability.
10. No cache delete authority, new S3 artifact class, bucket, prefix, credential source, or target-directory upload is added.
11. Performance success requires a comparable warm run with nonzero hits, zero read errors, full green proof, and at least 20% compile wall-clock improvement.
12. A sub-20% result stops the performance claim and requires new user-approved design work before any broader overhaul.

## Verification Matrix

### Architecture and static evidence — always required

| Requirement or risk | Required evidence |
|---|---|
| No BVS target cache | Workflow mutations add any `managed-target-bvs-*`, target restore/save action, target path, target digest, or `restore-keys`; hygiene fails in Backtester and both flaky workflows. |
| Exact S3 artifact identity | Mutations remove the head-resolver `backtester_cache` digest, use it outside nextest/sidecar keys or metadata, cross-wire key/metadata values, or add a SHA bootstrap; hygiene fails. |
| Resolver-policy asymmetry | Input-set tests retain forbidden governance paths; hygiene/source-fence tests independently cover resolver, TOML, workflows, and shared actions. |
| Sccache authority | Mutations give PR/MQ/flaky write authority, omit the shared action, substitute inline AWS setup, or change the governed location; hygiene fails. Main read/write and untrusted read-only configurations pass. |
| Fail-open compilation | Mutations let cache ineligibility skip or weaken a Rust step, retain `RUSTC_WRAPPER`, or choose an alternate backend; hygiene/action tests fail. |
| Test preservation | Mutations remove archive, sidecar, partitions, full tests, issue-specific tests, or gate dependencies; hygiene fails. |
| Both 32 GiB limits | Static tests assert `32212254720` in root and BVS policy; above-limit behavior remains fail-closed. |
| Truthful pressure payload | Unit tests assert exact `total_bytes`, `reclaimability_measured: false`, and `reclaimable_bytes: null`; a completed classification asserts true plus an integer. |
| Zero-budget steady state | API-backed inventories before rollout, after main, and after two source-only heads show no BVS cache keys/bytes/saves and repository total at or below 10 GiB. |
| Local static eligibility | `just fmt-check`, `just ci-lint-workflow`, targeted Python self-tests, `just source-fence-static`, and `git diff --check`. |
| Review | Fable 5/xhigh approves this spec before planning; a separate implementation reviewer approves the diff before publication; external review follows green exact-head CI. |

### Enabled-sccache evidence — required for the normal path

| Requirement or risk | Required evidence |
|---|---|
| Exact-head safety | Remote PR Backtester CI has no target restore, compiles required misses, runs full tests, and produces green `backtester-gate`. |
| Writer authority | Main reports `read_write`; PR/MQ and flaky jobs report `read_only`; no untrusted write occurs. |
| Compiler reuse | Later dependency/toolchain-identical source-only run reports enabled sccache, nonzero requests, nonzero cacheable requests, nonzero hits, zero read errors, and full green proof. |
| Latency | Comparable cold and warm compile intervals use the documented formula and show at least 20% improvement. |
| Stable rollout | Two source-only heads show zero BVS Actions-cache entries/saves and no cache-eviction/re-upload loop. |

### Fail-open/outage evidence — required for the fallback path

| Requirement or risk | Required evidence |
|---|---|
| Cache ineligible/unavailable | Action/unit and workflow mutation tests cover missing role/location, AWS failure, install failure, and server-start failure; each resolves to `enabled=false`. |
| Cold correctness | A remote run with sccache unavailable contains no BVS target restore, compiles locally, preserves the 32 GiB guard, runs full tests, and produces green `backtester-gate`. |
| Honest reporting | Summary/stats report cache disabled or the read error; no target-cache hit or performance claim is required. |

Enabled-lane evidence is not required from an outage run, and an outage run is never required to fabricate compiler-cache hits. Conversely, an enabled run cannot substitute a hit count for the required cold fallback proof or the 20% wall-clock criterion.

Local compile-heavy Rust checks remain prohibited by the repository's remote-first policy.

## Sequencing With #1367 and #1354

1. Claude Fable 5/xhigh re-reviews this spec. Every finding is resolved before an implementation plan exists.
2. A separate BVS CI issue/slice owns the plan and implementation. It does not close or bundle #1354.
3. A Codex 5.6-sol-medium implementor works in an isolated worktree after the reviewed plan is approved.
4. A separate implementation reviewer resolves all findings before publication.
5. Exact-head PR Backtester proof establishes the no-target-cache cold path and required gate.
6. After the slice reaches main, trusted main writes compiler outputs and the post-rollout inventory/performance qualification runs.
7. PR #1367 merges authoritative main into its branch and reruns exact-head root and Backtester proof.
8. After #1367 lands, #1354 consumes the corrected NautilusTrader pin and continues its own governed path.

This slice blocks launch/soak only because the repository requires green `backtester-gate` proof before #1367 can land. It is a CI ownership defect, not a production trading defect.

## Non-Goals

- No change to Binance SBE timestamps, the NautilusTrader pin, or #1354 behavior.
- No change to realized volatility, reference pricing, maker, entry, exit, evidence, recovery, HMAC, or archival behavior.
- No root target-cache allocation or eviction-policy change.
- No increase to either 32 GiB target limit or the 10 GiB Actions-cache threshold.
- No automatic GitHub/S3 deletion and no delete authority.
- No mutable Cargo target directory in S3.
- No new cache backend, bucket, prefix, credential path, target artifact, tolerance, or alternate build path.
- No weakening of full-source S3 archives, sidecar metadata, tests, partitions, gates, or fail-closed behavior.
- No claim that a target-size scan can apportion historical byte ownership.
- No claim that sccache fully identifies the external runner image.
- No broader performance overhaul if the 20% criterion misses without a new spec and explicit user approval.
- No local compile-heavy Rust verification.
