# BVS Compile-Compatibility Cache Design

**Status:** Draft pending Claude Fable 5/xhigh plan/spec review. No implementation may start until that review returns and every finding is resolved.

## Goal

Make Backtester CI reuse compiled Rust artifacts only when the build environment and dependency graph are compatible, while keeping ordinary source and test edits fast. A cache miss must produce a clean cold build; it must never restore a merely similar target directory.

This design fixes the CI failure that blocks exact-head Backtester proof for PR #1367. It does not change Binance timestamps, realized-volatility pricing, trading behavior, or the #1354 implementation.

## Incident Evidence

PR #1367 exercised the live failure mode twice on the same head:

- The BVS archive producer had no exact target-cache hit.
- The broad `managed-target-bvs-v3-...-test-` restore prefix selected a non-exact bootstrap cache containing 6,419,854,292 compressed bytes from a different source/dependency state.
- The new NautilusTrader dependency graph then compiled into the restored target directory alongside artifacts whose provenance the lane could not establish.
- Before the binary-sidecar build, the managed Rust preflight measured the target above the unchanged 32 GiB soft limit and refused the command with `disk_pressure`.
- The runner still had approximately 79 GB of filesystem space. The refusal was caused by the managed target-size policy, not by exhaustion of the runner volume.
- The refusal payload reported `reclaimable_bytes: 0`, although that path had not performed a retention-candidate scan. The logs therefore could not distinguish "measured zero" from "not measured," nor apportion target bytes between restored and newly compiled artifacts.

The cache fallback caused the unsafe mixture. The 32 GiB refusal behaved correctly and remains in force.

## Decision

Use exact, immutable cache keys derived from a compile-compatibility seed. Remove every broad BVS managed-target `restore-keys` prefix from Backtester CI and both BVS flaky workflows.

The target cache is an optional acceleration layer:

- An exact hit may be restored.
- An exact miss starts from an empty managed target directory and cold-builds.
- Main may write the first cache for a compatibility seed.
- Pull requests, merge queues, manual flaky detection, and scheduled flaky smoke are read-only.
- If the repository's existing 10 GiB Actions-cache tripwire cannot accommodate the target-cache family, BVS managed-target caching is disabled. The limits are not raised and an approximate restore is never reintroduced.

### Rejected alternatives

1. **Keep the broad prefix fallback.** This is fast when the selected cache happens to be compatible, but the cache name does not prove compatibility. It reproduces the #1367 failure and is rejected.
2. **Key the target cache by the complete source digest.** This is safe but makes every source or test edit a cold build and creates one immutable Actions cache per source state. It does not address the operator's CI-latency concern or the repository cache budget and is rejected.
3. **Disable all target caching immediately.** This is safe and remains the bounded storage fallback, but it discards compatible dependency reuse. The exact compatibility seed is the preferred design when it fits the existing storage budget.

## Two Separate Cache Identities

Target directories and executable test payloads have different validity rules and must not share one digest.

### Full-source artifact digest

The existing full-source `backtester_cache` set remains the authority for:

- BVS nextest archives in S3;
- BVS binary sidecars in S3; and
- any metadata that asserts an executable payload was produced from the exact source state.

It continues to include root and BVS source, tests, manifests, locks, fixtures, target discovery, wrappers, toolchain, and build policy. S3 object metadata continues to carry this exact digest, and restore continues to reject missing or mismatched digest metadata. This design does not weaken or replace the full-source S3 contract.

### Compile-compatibility target seed

A new `backtester_target_seed` input set owns reusable target-directory compatibility. It excludes ordinary `src/**`, `tests/**`, and reference fixtures so a source-only change retains the same seed and Cargo recompiles only what changed.

The seed includes every checked-in surface that can change the dependency graph, compiler ABI, target layout, feature/profile selection, or managed build command:

- `Cargo.toml`
- `Cargo.lock`
- `crates/backtesting-vertical-slice/Cargo.toml`
- `crates/backtesting-vertical-slice/Cargo.lock`
- `rust-toolchain.toml`
- `build.rs`
- `ci/rust-verification.toml`
- `crates/backtesting-vertical-slice/ci/rust-verification.toml`
- `justfile`
- `crates/backtesting-vertical-slice/justfile`
- `scripts/rust_verification.py`
- `scripts/command_understanding.py`
- `scripts/rust_test_targets.py`
- `.github/actions/setup-environment/action.yml`
- `.github/workflows/backtester-ci.yml`
- `.github/workflows/flaky-test-detection.yml`
- `.github/workflows/flaky-test-smoke.yml`
- `ci/github-actions-runners.toml`

The input-set resolver already hashes both the ordered pathspec declarations and the tracked bytes selected by those pathspecs. Adding, removing, or changing a compatibility input therefore changes the seed without embedding a commit SHA.

The workflow files are deliberately included. They own build flags such as the test/dev debug settings and the exact commands that populate or consume the target directory. A workflow-only documentation edit may cause one conservative cold build, but normal Rust source/test edits do not.

The input set must reject accidental source or test inclusion. Its self-tests must also reject omission of either manifest, either lockfile, the toolchain, either Rust-verification policy, the setup action, the three consuming workflows, or the command wrappers.

## Key Contract

The target keys are exact and lane-specific:

```text
managed-target-bvs-v4-<os>-<arch>-clippy-<backtester_target_seed>
managed-target-bvs-v4-<os>-<arch>-test-<backtester_target_seed>
```

Requirements:

- `v4` is a cache-contract namespace, not a source revision.
- OS, architecture, and lane/profile are mandatory separators.
- The digest is produced only by `scripts/ci_input_sets.py hash backtester_target_seed`.
- No BVS managed-target restore step may declare `restore-keys`, a shorter prefix, a previous schema, a full-source digest, or `GITHUB_SHA`.
- Clippy never restores the test cache; tests and flaky consumers never restore the clippy cache.
- S3 nextest/sidecar keys continue to use the full-source `backtester_cache` digest, never `backtester_target_seed`.

## Lifecycle and Authority

### Pull request and merge-queue runs

PR and merge-queue jobs compute the target seed from their exact checked-out head and attempt only the exact key. On a hit, they restore read-only. On a miss, they cold-build in an empty managed target directory. They never save, refresh, overwrite, or alias a target cache.

### Pushes to main

Main uses the same exact seed. If the exact cache exists, it restores read-only and does not write another entry. If the exact cache is absent, a successful producer build may save one immutable cache for that lane and seed. Failed or refused builds never save.

This yields at most one clippy entry and one test entry for a compatibility seed, instead of one entry per commit. Later source-only main changes reuse the same seed and do not create new cache objects.

### Flaky workflows

Both BVS jobs in `flaky-test-detection.yml` and `flaky-test-smoke.yml` consume only the exact test key. They are read-only even when manually dispatched or scheduled. They do not create a separate flaky namespace and do not fall back to another seed.

### Empty-directory rule

After an exact miss, the workflow must prove that the managed target directory contains no restored payload before compiling. Setup-created empty directories are allowed. Any unexplained file on a reported miss is a hard failure; the workflow must not build beside an unowned target tree.

## Governance and Bootstrap

Changes to `ci/rust-ci-inputs.toml` or `scripts/ci_input_sets.py` continue to force the Backtester lanes to run. They do not create `bootstrap-<GITHUB_SHA>` target caches.

Bootstrap behavior is deterministic:

1. The head version of the resolver validates both `backtester_cache` and `backtester_target_seed`.
2. It computes both digests from the head's declared pathspecs and tracked bytes.
3. Because pathspec declarations are themselves hashed, changing the target-seed membership produces a new compatibility seed.
4. A change to digest semantics must advance the `managed-target-bvs-v4` schema and its fixture contract. Merely editing comments or tests in the resolver does not authorize a per-commit key.
5. The first run for a new seed is a normal exact miss. PR and merge-queue runs cold-build; the first successful main producer may save the stable seed.

The workflow-hygiene verifier must fail if it finds `bootstrap-${GITHUB_SHA}`, any other commit-SHA namespace, a broad restore prefix, a missing compatibility input, a non-main save condition, or a flaky-workflow save.

## Actions-Cache Storage Budget

The existing repository policy in `ci/storage-tripwire.toml` remains authoritative:

- Actions cache listed bytes warning threshold: 10 GiB (`10,737,418,240` bytes).
- The threshold is repository-wide, not reserved for BVS.
- This slice does not increase it and does not add another competing threshold.

Read-only inventory on 2026-07-12 found five cache entries totaling 9,118,987,793 bytes and no surviving `managed-target-bvs-*` entry. That leaves 1,618,430,447 bytes below the tripwire. The prior BVS test cache alone was 6,419,854,292 compressed bytes. On the evidence available to this design, an enabled BVS target family does not fit. Compatibility-key correctness does not by itself prove storage viability.

The implementation has a mandatory rollout tripwire:

1. Record repository-wide listed cache bytes and the BVS family bytes before implementation approval.
2. Use the most recent measured BVS cache size as a conservative candidate-size proxy. An enabled lane requires enough measured headroom before it may be selected in the implementation plan.
3. If an enabled lane passes that pre-merge gate, record the same API-backed values after its first main save using the existing storage-audit contract.
4. The rollout is accepted only if total listed bytes remain at or below 10 GiB and the cache family converges to at most one entry per enabled lane and compatibility seed.
5. If either the projection or post-save measurement crosses the threshold, the accepted fallback is to remove/disable BVS managed-target restore and save steps. BVS then uses a clean cold target plus the existing Cargo registry/git cache, sccache where already governed, and full-source S3 nextest/sidecar artifacts.
6. The fallback must not raise the threshold, delete unrelated caches automatically, add a prefix restore, move a mutable Cargo target to S3, or add a second cache backend.

The implementation plan must treat this as a decision gate, not deferred debt: either exact BVS target caching has measured budget evidence, or the final implementation contains no BVS target-cache steps. With the 2026-07-12 inventory, the default outcome is the disabled-cache fallback unless a fresh inventory or separately authorized operator cleanup establishes enough headroom before plan approval. Existing cache deletion remains an explicit operator action and is not authorized by this design.

## Disk-Pressure Telemetry

The 32 GiB managed target soft limit in `ci/rust-verification.toml` remains exactly `32,212,254,720` bytes. A target above that limit continues to fail closed before a managed Rust command.

`disk_preflight_refusal_payload` must report what it actually measured:

- `total_bytes`: the scanned managed target size used by the pressure decision;
- `filesystem`: the existing free/used/total filesystem measurements;
- `thresholds` and `pressure_reasons`: unchanged policy evidence;
- `reclaimability_measured: false`; and
- `reclaimable_bytes: null` when the preflight did not run retention-candidate classification.

Zero is reserved for a completed reclaimability scan that found zero reclaimable bytes. A later caller that does run the candidate scan reports `reclaimability_measured: true` with an integer `reclaimable_bytes`.

The payload does not claim to identify which bytes came from a restored cache versus the current compilation. Once Cargo uses the directory, that apportionment is not trustworthy without a separate before/after measurement. The exact key prevents incompatible restoration; `total_bytes` explains the refusal.

## Failure Semantics

| Condition | Required result |
|---|---|
| Target-seed validation or hashing fails | Fail the job before cache restore. |
| Exact target key hits | Restore that key only; record key and hit status. |
| Exact target key misses | Record a miss, prove the target is empty, and cold-build. |
| A broad/prefix/old-schema key is present | Ignore it; never restore it. |
| Cache restore reports corruption or a partial failure | Fail before compilation; do not build beside partial state. |
| Target is non-empty after a reported miss | Fail before compilation. |
| Managed target exceeds 32 GiB | Preserve the current `disk_pressure` refusal with truthful byte telemetry. |
| Build/test/clippy fails or is refused | Do not save a target cache. |
| Main cache save loses a race because the exact key now exists | Treat it as an already-seeded optimization; retain build/test result and report the race. |
| Cache service save fails for another reason | Report the cache-save failure without weakening the completed Rust result; the next run safely cold-builds or restores an existing exact key. |
| S3 full-source object is missing | Preserve the current cache-miss/build behavior. |
| S3 metadata digest is missing or mismatched | Preserve the current integrity failure. |
| Repository cache tripwire exceeds 10 GiB | Disable BVS target caching; do not widen limits or restore approximately. |

## Exact Implementation Surface

The eventual implementation is limited to these files unless the reviewed plan proves another file is mechanically required:

- `ci/rust-ci-inputs.toml` — add and own `backtester_target_seed`; keep the full-source artifact set.
- `scripts/ci_input_sets.py` — validate the target-seed policy and deterministic schema contract.
- `scripts/test_ci_input_sets.py` — positive expansion/digest tests and negative source/omission/bootstrap tests.
- `.github/workflows/backtester-ci.yml` — exact clippy/test keys, no prefixes, main-only saves, cold-miss proof, separate artifact/target digests.
- `.github/workflows/flaky-test-detection.yml` — exact read-only test key and no prefix/save.
- `.github/workflows/flaky-test-smoke.yml` — exact read-only test key and no prefix/save.
- `scripts/verify_ci_workflow_hygiene.py` — enforce key identity, no prefix/SHA bootstrap, writer authority, flaky read-only use, and full-source S3 identity.
- `scripts/test_verify_ci_workflow_hygiene.py` — mutation tests for every workflow invariant.
- `scripts/ci_workflow_hygiene_test_helpers.py` — fixture updates required by the verifier tests.
- `scripts/rust_verification.py` — truthful disk-preflight size/reclaimability fields.
- `scripts/test_rust_verification_cache_retention.py` — telemetry and unchanged 32 GiB threshold tests.

The implementation reads but does not change these authorities unless the Fable review identifies a contradiction:

- `ci/rust-verification.toml` — existing 32 GiB managed target soft limit.
- `ci/storage-tripwire.toml` — existing 10 GiB repository Actions-cache threshold.
- `scripts/ci_storage_audit.py` and `scripts/ci_storage_tripwire.py` — existing API-backed storage evidence and alerting contracts.
- `.github/workflows/ci-storage-tripwire.yml` — existing scheduled tripwire owner.
- `docs/ci/storage-tripwire-governance.md` — existing operator authority.

No source, test, manifest, lockfile, runtime configuration, strategy, or trading module belongs in this CI slice.

## Invariants

1. No BVS managed-target cache uses `restore-keys` or any non-exact fallback.
2. Source/test-only changes preserve the target seed; dependency, toolchain, or governed build-policy changes alter it.
3. Full-source S3 payload identity remains independent from target compatibility identity.
4. Only main writes target caches, and only after successful production of the relevant lane output.
5. PR, merge-queue, manual flaky, and scheduled flaky jobs are read-only.
6. One compatibility seed creates at most one immutable cache per OS/architecture/lane.
7. A cache miss never leaves unowned artifacts in the directory used for the cold build.
8. The 32 GiB target limit and 10 GiB repository Actions-cache tripwire are not increased.
9. Disk-pressure diagnostics distinguish measured target bytes from unmeasured reclaimability.
10. Storage pressure disables the optimization; it never weakens cache identity or Rust verification.

## Verification Matrix

| Requirement or risk | Required evidence |
|---|---|
| Source-only reuse | Input-set fixture mutates root/BVS source and tests without changing `backtester_target_seed`, while `backtester_cache` changes. |
| Dependency/toolchain invalidation | Fixtures mutate each manifest, lockfile, toolchain, and governed build-policy category and observe a new target seed. |
| Input-set membership | Negative tests remove every mandatory compatibility surface and add forbidden source/test pathspecs; validation fails. |
| No approximate restore | Workflow mutations add block/inline `restore-keys`, old v3 prefixes, shortened keys, full-source digests, or SHA namespaces; hygiene fails for Backtester and both flaky workflows. |
| Writer authority | Mutations allow PR, merge queue, or flaky saves; hygiene fails. Exact main-only conditions pass. |
| Lane isolation | Mutations make clippy consume the test key or a test consumer use the clippy key; hygiene fails. |
| Deterministic bootstrap | Mutations restore `bootstrap-${GITHUB_SHA}` or another per-commit key; hygiene fails. A config-only fixture yields a deterministic non-SHA seed. |
| Empty cold miss | Workflow contract tests require an empty-directory assertion on miss and reject compile steps that can run before it. |
| S3 identity separation | Mutations replace the full-source digest with target seed in any nextest/sidecar key or metadata; hygiene fails. |
| 32 GiB limit retained | Static/behavior test confirms `soft_limit_bytes = 32212254720` and refusal still occurs above it. |
| Truthful pressure payload | Unit test above the limit asserts exact `total_bytes`, `reclaimability_measured: false`, and `reclaimable_bytes: null`; a measured prune path asserts true plus an integer. |
| Storage budget | Read-only cache inventory before/after first main seed records total and BVS-family bytes; existing storage tripwire evaluates the 10 GiB threshold. |
| Fallback | A reviewed workflow variant with all BVS target restore/save steps absent still produces the required clippy/test/S3 proof and contains no alternate target cache. |
| Local static eligibility | `just fmt-check`, `just ci-lint-workflow`, input-set self-tests, workflow-hygiene self-tests, storage-tripwire self-tests if touched, `just source-fence-static`, and `git diff --check`. |
| Rust and workflow behavior | Exact-head remote Backtester CI: cold miss succeeds; a later same-seed source-only head gets an exact hit; a lockfile/NT-pin mutation gets an exact miss; required `backtester-gate` is green. |
| Review | Claude Fable 5/xhigh approves this spec before planning; a separate implementation reviewer approves the final diff before publication; external review occurs only after green exact-head CI. |

Local compile-heavy Rust checks remain prohibited by the repository's remote-first policy.

## Sequencing With #1367 and #1354

1. Claude Fable 5/xhigh reviews this spec. Findings are resolved in the spec before a plan exists.
2. A separate CI issue/slice owns the implementation. It does not close or bundle #1354.
3. The CI slice receives independent implementation review, is published through its own PR, and lands with exact-head static and remote proof.
4. Main becomes authoritative. PR #1367 merges that main state into its branch and re-runs exact-head root and Backtester proof.
5. After #1367 lands, #1354 consumes the corrected NautilusTrader pin and continues its own governed implementation/review path.

This CI slice is a launch/soak blocker only because the repository requires a green `backtester-gate` before #1367 can land. It is not a production trading defect and does not broaden #1367's timestamp scope.

## Non-Goals

- No change to the Binance SBE timestamp fix or NautilusTrader pin.
- No change to realized-volatility, reference-price, maker, entry, exit, evidence, recovery, HMAC, or S3 archival behavior.
- No increase to the 32 GiB managed-target limit or 10 GiB Actions-cache tripwire.
- No automatic deletion of GitHub caches and no destructive cleanup authorization.
- No mutable Cargo target directory in S3.
- No new cache backend, cache tolerance, prefix fallback, commit-keyed target cache, or alternate build path.
- No claim that a target-size scan can distinguish restored bytes from bytes compiled later in the run.
- No local compile-heavy Rust verification.
