# Strict sccache Runtime Design

## Decision and scope

Build and publish a repository-governed sccache v0.16.0 derivative that removes silent cache bypasses. This is a standalone prerequisite task for PR #1494. It does not modify #1494, the Rust Probe OIDC boundary, advisory-run cancellation, or runner allocation.

The prerequisite PR owns:

- the exact upstream source identity and source archive digest;
- one in-repository patch containing the strict behavior and its upstream tests;
- one cacheless, reproducible ARM64/X64 build path;
- one protected same-repository GitHub release publisher;
- immutable release assets and their provenance manifest.

PR #1494 remains the adoption PR. After the prerequisite is merged and its assets are published, #1494 will pin the immutable release and per-architecture asset IDs and SHA-256 values, own the runtime timeout values in `ci/sccache-location.toml`, install those assets directly, validate its `active` input, route Cargo-owned compiler invocations through the governed wrapper, and provide controlled setup/runtime failure evidence. Job-wide prevention of a build script or nested process invoking an unwrapped compiler belongs to #1494, not to this binary prerequisite.

## Runtime invariant

For every supported compiler invocation accepted by the governed wrapper, the binary enforces this contract without an environment-variable opt-out:

> The already-running governed server must classify the invocation before any client-side compiler execution. Dedicated `NotCompilation` and `CannotCache(reason)` responses may authorize intentional client execution, preserving the original stdin and file descriptors while recording the classification and reason. Cacheable compilation executes through the server as a hit or a genuine miss. No connection failure, server restart, IPC error, EOF, unexpected response, cache read error or timeout, corrupt entry, storage-capability failure, or required write failure may authorize compiler execution; each fails the invocation and invalidates the job.

Every client first performs a mandatory identity handshake. The server identity binds a derived identity of the exact source, patch, workflow, recipe, verifier driver, executor, toolchain, feature set, profile, and target, plus the strict protocol revision, effective S3 configuration, configured mode, basedirs, all four governed timeout values, and the governed IPC frame-size limit. An upstream, stale, differently built, or differently configured daemon is rejected before any compile or evidence request. Legacy `UnhandledCompile` keeps its upstream wire discriminant and is fatal; the two intentional classifications use new appended discriminants.

The control boundary is the authority for execution, not merely the process that spawns rustc. An explicitly classified invocation is governed even when the client executes it: the server observed it, selected a typed path, and recorded the applicable reason before execution. A failure-induced fall-through is prohibited because no server classification authorized it.

This distinction is required by live evidence. Exact-head run `29814080045` reported 123 non-cacheable calls in `build` and 144 in `test`, including stdin-fed `-` invocations. The v0.16.0 request protocol transports arguments and environment but not stdin or inherited file descriptors. Moving these calls into the daemon would change their behavior or require a new stdin/FD transport protocol. The smaller complete fix retains intentional classified client execution and makes every failure path fatal.

## Exact upstream boundary

The derivative is based on sccache v0.16.0 at commit `b799af2eea02bba9e0ef2550775fe10296b62981`. The pinned source archive is:

- URL: `https://github.com/mozilla/sccache/archive/b799af2eea02bba9e0ef2550775fe10296b62981.tar.gz`
- SHA-256: `a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3`

The patch must close the complete observed class, not only the demonstrated EOF branch:

1. [`src/commands.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/commands.rs#L302-L339) auto-starts a replacement server after connection refusal, and its [compile response handling](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/commands.rs#L521-L577) starts the compiler after both an explicit `UnhandledCompile` response and `CompileStarted` followed by `UnexpectedEof`.
2. [`src/protocol.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/protocol.rs#L38-L75) conflates `NotCompilation` and `CannotCache`, while the compile request carries no stdin or inherited file descriptors.
3. [`src/server.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/server.rs#L1240-L1289) maps both classifications to that response, although it already records `CannotCache` reasons.
4. [`src/compiler/compiler.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/compiler/compiler.rs#L576-L813) treats cache read errors, timeouts, and corrupt extracted objects as misses, and supports caller-forced no-cache/recache paths.
5. [`src/cache/cache.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/cache/cache.rs#L210-L282) converts remote read errors into misses and can silently negotiate configured read-write storage down to read-only. Its [backend selection](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/cache/cache.rs#L573-L613) permits multilevel storage and falls back to a local disk cache when no remote backend resolves.
6. [`src/cache/multilevel.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/cache/multilevel.rs#L607-L715) suppresses individual backend read failures and continues to later levels.
7. [`src/cache/cache_io.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/cache/cache_io.rs#L100-L195) can discard corrupt stdout/stderr data and treat corrupt optional outputs as absent.
8. [`src/server.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/server.rs#L1517-L1526) records cache write errors but still returns the successful compiler result.

## Required behavior matrix

| Event | Required result |
| --- | --- |
| Missing governed S3 configuration, alternate backend, disk fallback, or multilevel configuration | Fail server startup. Exactly one governed S3 backend is allowed. |
| Governed storage capability check fails, including rate limiting | Fail server startup; no read or write capability-check failure is tolerated. |
| Configured read-write storage negotiates any other mode | Fail server startup; never downgrade to read-only. |
| `CompilerArguments::NotCompilation` | Return its dedicated classification, record it, and authorize intentional client execution with the original stdin and file descriptors. |
| `CompilerArguments::CannotCache(reason)` | Return its dedicated typed classification, record the reason, and authorize intentional client execution with the original stdin and file descriptors. The reason is telemetry, not an allowlist or policy input. |
| Unsupported compiler | Fail before compiler execution. |
| Compiler or evidence client cannot reach the already-running server | Fail; never auto-start or replace the server. |
| Connected server has a different derivative identity, backend fingerprint, mode, timeout policy, or frame-size policy | Fail before the requested operation. |
| Genuine cache hit | Restore the cached output successfully and succeed. |
| Governed S3 reports the object is absent | Treat this authoritative `NotFound` result as the only genuine miss. |
| Genuine cache miss in read-only mode | Compile through the server, do not attempt a cache write, and return the compiler result. |
| Genuine cache miss in read-write mode | Compile through the server and return success only after the cache write succeeds. |
| Cache read timeout/error/permission failure | Fail before launching the miss compilation. |
| Corrupt cached stdout, stderr, required output, or optional output | Fail; do not discard it or reinterpret it as absence or a miss. |
| Client/server EOF, other IPC error, or unexpected response | Fail; do not spawn a client-side compiler. |
| Caller sets `SCCACHE_NO_CACHE` or `SCCACHE_RECACHE` | Reject the invocation unconditionally; neither is an authorized mode in the governed build. |
| Server discovers the result cannot be cached after compilation | Return the server-owned compiler result and record it as an explicitly non-cacheable compilation. |
| Read-write cache write failure | Return failure after compilation and invalidate the job. |
| Governed startup, absolute IPC-frame, cache-read, or required-write timeout expires, or an IPC frame exceeds the governed maximum | Fail using the adoption TOML-owned policy; the governed build has no code default or alternate environment fallback. |

The protocol must distinguish `NotCompilation` from `CannotCache(reason)`. Only those dedicated responses may reach the client-side compiler execution block. The legacy response remains distinct and fatal. EOF, IO errors, slow-drip frames, oversized frames, unexpected responses, identity mismatches, and failed or missing server connections return nonzero instead. The patch removes or makes unreachable `SCCACHE_IGNORE_SERVER_IO_ERROR` and server auto-start for compiler and evidence clients. Setup remains the sole explicit server-start path; it keeps the direct child handle until startup succeeds, terminates and reaps a timed-out child, refuses to replace a live Unix socket, and reclaims only a connection-refused stale socket whose device and inode remain unchanged. The direct child intentionally stays in the job's process group so cancellation cannot leave a cache server behind.

The server starts only when the effective backend and mode exactly match the governed S3 configuration. Read-only mode skips `put` entirely. Read-write mode withholds success until `put` completes. The patched binary requires typed startup, IPC, cache-read, required-write, and maximum-frame inputs but does not own their consumer values; #1494 supplies them from `ci/sccache-location.toml`. The strict path has no built-in operational defaults.

## Repository artifacts

The prerequisite PR creates these bounded units:

- `ci/sccache-strict.toml`: build-provenance inputs only: source URL and digest, upstream commit, patch identity, Rust toolchain, build-executor image digest, feature set, targets, replica topology, normalized build settings, and the test-only cache mode, strict timeout, and frame limit used while verifying the derivative. It does not own consumer release, asset, cache-location, runtime-policy, or runner values.
- `ci/sccache-strict/sccache-v0.16.0-strict.patch`: the complete upstream code and test delta.
- `scripts/sccache_strict.py`: the sole config reader and manifest verifier used by local checks and the workflow. It never publishes and never changes repository files.
- `scripts/test_sccache_strict.py`: behavior tests for configuration validation and manifest/digest verification. It does not scan repository source text.
- `scripts/build_strict_sccache.sh`: the one canonical, identity-hashed build recipe used by every architecture and replica. It stages all mutable build state under `RUNNER_TEMP`, not inside the repository.
- `.github/workflows/sccache-strict-release.yml`: unprivileged builders plus the protected publisher.
- `ci/github-actions-runners.toml`: the sole runner mapping for the new workflow's existing managed ARM64 and X64 runner classes.

The workflow does not use `.github/actions/sccache-setup`, `RUSTC_WRAPPER`, cache credentials, AWS OIDC, or any compilation cache. Every external action is pinned to a full commit SHA. Source, feature, target, profile, replicas, workflow identity, canonical recipe identity, verifier-driver identity, and normalized build settings come only from reviewed repository inputs and are recorded in each candidate manifest. The lockfile-pinned vendored OpenSSL source avoids an ungoverned host-library dependency in the static musl build. Docker is the current replaceable build executor only: it is not a developer prerequisite, consumer dependency, runtime component, protocol boundary, or adoption authority. Replacing that executor later requires equivalent reviewed build-provenance evidence but no binary runtime or #1494 contract change. The complete patched upstream library suite remains required; fixtures whose permissive startup premise no longer exists assert the strict preflight result instead, while focused listener tests preserve the displaced port/socket behavior. The ARM64 and X64 targets are built natively on their governed runner architectures.

## Build and publication flow

1. A pull-request build checks out and asserts the actual PR head SHA; a manual build checks out and asserts the dispatched exact-current-main SHA. It downloads the exact source archive and verifies its SHA-256 before extraction.
2. The canonical recipe extracts outside the enclosing repository, requires a non-empty zero-context patch file list, runs `git apply --unidiff-zero --check`, applies the patch, and proves the resulting tree accepts the exact reverse patch. The source archive digest supplies the exact context boundary; zero-context storage keeps the committed patch diff-clean. A skipped, empty, or wrong-source application is fatal.
3. The workflow builds the debug binary required by the upstream library harness, then runs the complete upstream library suite before the release build.
4. Dependency acquisition is checksum-locked. The build then runs network-disabled with fresh Cargo and target directories, no wrapper or incremental compilation, and an explicit normalized environment and path remapping.
5. Two isolated jobs build each architecture with the same verified source, patch, toolchain, container digest, features, and build settings.
6. Each builder verifies the expected ELF architecture, absence of a runtime interpreter and shared-library dependencies, and host execution outside the build container. Separate scrubbed-environment controls prove that missing S3 configuration and an injected disk backend both fail startup with their distinct diagnoses.
7. Builder jobs upload binaries and a provenance manifest as same-run, same-attempt artifacts bound to the exact head, run ID, architecture, configured replica, toolchain, container, source, patch, workflow, and canonical recipe. Attempt-qualified artifact names prevent a re-run from reusing or colliding with an earlier attempt. Builders have `contents: read` only and no cache or publisher credentials.
8. An unprivileged verification job runs for pull requests and dispatches, independently hashes all four candidates, validates every manifest binding, requires byte equality per architecture, and emits one attempt-qualified verified bundle. A protected publisher is never required to learn whether the replicas agree.
9. The publisher runs only for `workflow_dispatch` whose SHA is observed as exact-current `main` immediately before publication, references a GitHub environment restricted to `main`, and receives job-scoped `contents: write`. Checkout does not persist credentials. The token is removed from the environment before repository-controlled verification code runs and is supplied only to the individual GitHub API/CLI subprocesses. The publisher does not execute Cargo, build scripts, or the produced binaries.
10. A tokenless publisher-preparation step downloads only the verified bundle from its own workflow run, reruns the same manifest and byte verifier, and requires byte-identical verifier output. The release tag is derived from the full SHA-256 of the canonical governed build-input identity, including source, patch, executor, toolchain, workflow, recipe, replica set, build settings, verification threshold, and targets.
11. The token-bearing release step refuses any pre-existing tag, release, or draft, confirms observed-current `main` and the protected environment, arms exact draft/tag cleanup immediately, creates the draft, validates the returned release identity, and uploads the agreed binaries and provenance. It revalidates current `main` and the environment immediately before publishing. Cleanup never searches by tag or deletes a release whose exact returned ID and ownership fields were not validated. If GitHub reports `immutable: false`, it deletes only the exact release and generated tag it just created, then fails.
12. The token-bearing step rechecks `main` after publication, reads back the immutable release and actual Git tag, downloads and byte-compares every asset with the verified inputs, and uses GitHub CLI's cryptographic release and per-asset verification commands with bounded polling. If `main` moved in the unavoidable API-call gap, the immutable candidate is retained without adoption authority and the workflow fails. A following tokenless step evaluates the exact release/tag/asset predicate. The later consumer still verifies its independently reviewed SHA-256 pin.

The repository's admin-only immutable-release endpoint currently reports `enabled: false`, and the only existing environment is unrelated. The repository owner must enable release immutability and create the main-restricted publisher environment before the first publication dispatch. The workflow uses only `GITHUB_TOKEN`: it verifies the environment before publication and verifies the immutable release result immediately after publication. A disabled immutability setting therefore creates no retained release or tag and cannot silently weaken publication. These operator prerequisites are tracked here and do not belong to the prerequisite PR's diff. GitHub documents that immutable release assets and their tag cannot be changed individually after publication, although deletion of the whole release remains an availability risk, and recommends draft, upload, then publish: [Immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases).

## Trust boundary

- Repository review owns the upstream commit, source digest, patch, build recipe, and publisher workflow.
- Builder jobs are unprivileged and cannot publish or write the compilation cache.
- The publisher token can create releases and modify unprotected refs. Native protected-`main` governance, required review, and the separate #1494 change prevent those capabilities from activating a binary.
- #1494 makes `ci/sccache-location.toml` the sole install authority by independently pinning immutable release ID, per-architecture asset IDs, and binary SHA-256 values after publication. Build-provenance TOML does not duplicate those install facts.
- PR and main cache roles cannot publish or replace the executable.
- A compromised publisher can create an unwanted release, but cannot activate it without a separate reviewed adoption change.
- Release deletion or GitHub unavailability intentionally stops installation; there is no S3, upstream-release, preinstalled-tool, or local-build fallback.

## Verification evidence

The prerequisite PR requires:

- upstream library tests proving EOF and other IPC failures return nonzero without a client-side compiler spawn;
- protocol tests proving the upstream discriminants are preserved, the legacy response is fatal, the strict identity request is appended, and only explicit `NotCompilation` and `CannotCache(reason)` responses authorize client execution while preserving the original arguments;
- handshake tests proving an upstream, stale exact-policy derivative, differently configured, or differently timed server is rejected before the requested operation;
- IPC tests proving Unix and TCP slow-drip frames share one absolute deadline and oversized frames are rejected before allocation;
- negative tests proving refusal to auto-start after connection failure; the same connect-only helper governs every later compiler and evidence request, including after server death;
- startup tests rejecting missing S3 configuration, disk or alternate backends, multilevel configuration, rate-limited capability checks, and configured read-write to read-only demotion;
- storage tests proving read errors, complete hit-materialization timeouts, late corruption without earlier-output commit, post-timeout non-mutation, corrupt stdout/stderr/required/optional objects (including temp-file failure), and read-write write failures return nonzero;
- a read-only miss test proving server compilation succeeds without a write attempt;
- classified-client tests proving stdin-fed and output-producing arguments reach the unchanged client command path without consulting cache storage;
- tests rejecting externally forced no-cache and recache modes;
- exact source archive and patch application evidence;
- two byte-identical cacheless, network-closed clean builds per architecture;
- ELF/ABI checks and binary smoke tests outside the build container on governed ARM64 and X64 hosts;
- listener lifecycle tests proving live Unix sockets are preserved, unchanged stale sockets are reclaimed, clean shutdown removes only the owned inode, and occupied TCP ports remain fatal;
- publisher negative controls for cross-run or mismatched artifacts, stale or non-main heads, pre-existing tags/releases, absent environment approval, and exact-ID cleanup after a mutable publication result;
- actionlint and the repository's targeted Python self-tests;
- an internal adversarial review of the exact diff before completion is claimed.

Before #1494 may adopt the derivative, a successful publication dispatch that observed the dispatched SHA as current immediately before publication and again after publication must additionally prove the exact tag target, immutable release state, attestation, asset set, and digests. The later #1494 adoption requires separate evidence that a missing asset, channel failure, wrong architecture, altered digest, setup failure, server death, cache read failure, and read-write write failure all stop before the job can claim valid compile evidence. It also owns behavior evidence that Cargo's supported compiler invocations cannot silently bypass the governed wrapper.

## Accepted residual risks

- Explicitly classified `NotCompilation` and `CannotCache(reason)` invocations execute on the client by design. Their prior server classification and persisted statistics are the governance evidence; the strict binary does not claim daemon ownership of those processes.
- A required read-write cache write can fail after compiler outputs exist locally. The invocation and job still fail, so those outputs have no valid evidence authority.
- The binary contract begins when an invocation enters the governed wrapper. #1494 owns the job-level routing claim; an arbitrary process that never invokes the wrapper is outside this prerequisite's evidence boundary.
- Both replicas share the reviewed source, dependencies, toolchain, container, and runner provider. Byte equality does not detect an identical compromise in a shared input.
- GitHub archive or release unavailability, release deletion, or a changed auto-generated source archive stops the build or installation intentionally.
- Every upstream sccache upgrade requires a fresh source pin, patch review, behavior matrix run, reproducible publication, and adoption change.

## Failure and recovery

Any verification, build, reproduction, environment, release, download, or digest failure stops the affected workflow. A mutable publication result is removed together with only its exact generated tag before failure; an immutable result is retained for review even if later evidence retrieval fails. There is no fallback publication or installation channel.

Upgrades use a new reviewed upstream commit, source digest, rebased patch, immutable release, and adoption PR. Historical releases are retained but still depend on GitHub availability and the release not being deleted. Emergency revocation pauses affected CI or adopts a new immutable release; it never mutates an existing asset or restores the official permissive binary.

## Explicitly excluded work

- Changing PR #1495 or #1496.
- Changing Rust Probe checkout/OIDC behavior.
- Editing #1494 before immutable strict assets exist.
- Publishing to S3 or adding AWS roles, buckets, policies, or lifecycle exceptions.
- A stderr-watching wrapper or any control that detects fallback only after compiler execution.
- Upstreaming the patch; that may be proposed separately after the governed implementation is proven.
