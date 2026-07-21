# Strict sccache Runtime Design

## Decision and scope

Build and publish a repository-governed sccache v0.16.0 derivative that removes silent cache bypasses. This is a standalone prerequisite task for PR #1494. It does not modify #1494, the Rust Probe OIDC boundary, advisory-run cancellation, or runner allocation.

The prerequisite PR owns:

- the exact upstream source identity and source archive digest;
- one in-repository patch containing the strict behavior and its upstream tests;
- one cacheless, reproducible ARM64/X64 build path;
- one protected same-repository GitHub release publisher;
- immutable release assets and their provenance manifest.

PR #1494 remains the adoption PR. After the prerequisite is merged and its assets are published, #1494 will pin the release asset identities and SHA-256 values, install those assets directly, validate its `active` input, and provide controlled setup/runtime failure evidence.

## Runtime invariant

The governed binary enforces the following contract without an environment-variable opt-out:

> A non-compilation compiler probe may execute directly. Every compilation executes through the sccache server as a cache hit, a genuine miss, or an explicitly classified non-cacheable compilation. A client bypass, an IPC failure, an unexpected protocol response, a cache read error, a cache timeout, corrupt cache data, or a required cache write failure fails the compiler invocation and invalidates the job.

The phrase "compile through the sccache server" is intentional. On a genuine miss or a compiler invocation that sccache cannot cache, the server must invoke rustc on the runner to produce the result. The prohibited behavior is bypassing the governed server, hiding non-cacheable work as an unobserved client fallback, or converting a cache failure into an ordinary miss.

This distinction is required by live evidence. Exact-head run `29814080045` reported 123 non-cacheable calls in `build` and 144 in `test`, primarily linked crate types that sccache v0.16.0 does not cache. Treating every `CannotCache` as fatal would make the current Cargo workloads impossible. They must become an explicit server-owned path with reason statistics, not a client-side bypass.

## Exact upstream boundary

The derivative is based on sccache v0.16.0 at commit `b799af2eea02bba9e0ef2550775fe10296b62981`. The pinned source archive is:

- URL: `https://github.com/mozilla/sccache/archive/b799af2eea02bba9e0ef2550775fe10296b62981.tar.gz`
- SHA-256: `a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3`

The patch must close the complete observed class, not only the demonstrated EOF branch:

1. [`src/commands.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/commands.rs#L521-L577) currently starts the compiler locally after `CompileStarted` followed by `UnexpectedEof`, and after `UnhandledCompile`.
2. [`src/protocol.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/protocol.rs#L38-L47) has one `UnhandledCompile` response for two different cases.
3. [`src/server.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/server.rs#L1240-L1289) maps both `CannotCache` and `NotCompilation` to that response.
4. [`src/compiler/compiler.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/compiler/compiler.rs#L576-L813) treats cache read errors, timeouts, and corrupt extracted objects as misses, and supports caller-forced no-cache/recache paths.
5. [`src/cache/cache.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/cache/cache.rs#L210-L282) converts remote read errors into misses and tolerates some storage-check failures.
6. [`src/server.rs`](https://github.com/mozilla/sccache/blob/b799af2eea02bba9e0ef2550775fe10296b62981/src/server.rs#L1517-L1526) records cache write errors but still returns the successful compiler result.

## Required behavior matrix

| Event | Required result |
| --- | --- |
| `CompilerArguments::NotCompilation` | Direct compiler execution is allowed. |
| `CompilerArguments::CannotCache` | Start an explicitly non-cacheable server task, execute there, preserve the reason statistic, and never execute through the client fallback. |
| Unsupported compiler | Fail before compiler execution. |
| Genuine cache hit | Restore the cached output successfully and succeed. |
| Genuine cache miss in read-only mode | Compile through the server, do not attempt a cache write, and return the compiler result. |
| Genuine cache miss in read-write mode | Compile through the server and return success only after the cache write succeeds. |
| Cache read timeout/error/permission failure | Fail before launching the miss compilation. |
| Corrupt or unextractable cache hit | Fail; do not reinterpret it as a miss. |
| Client/server EOF, other IPC error, or unexpected response | Fail; do not spawn a client-side compiler. |
| Caller sets `SCCACHE_NO_CACHE` or `SCCACHE_RECACHE` | Reject the invocation; neither is an authorized miss. Internal sccache recache mechanics remain available where required by its server implementation. |
| Server discovers the result cannot be cached after compilation | Return the server-owned compiler result and record it as an explicitly non-cacheable compilation. |
| Read-write cache write failure | Return failure after compilation and invalidate the job. |

The protocol must distinguish `NotCompilation` from `CannotCache`. `NotCompilation` is the only response that may reach the client-side compiler execution block. `CannotCache` starts a server-owned compile task that returns a normal `CompileFinished` response without consulting or writing cache storage. The patch removes or makes unreachable the `SCCACHE_IGNORE_SERVER_IO_ERROR` opt-out.

## Repository artifacts

The prerequisite PR creates these bounded units:

- `ci/sccache-strict.toml`: source URL, source digest, upstream commit, release identity, Rust toolchain, build-container digest, feature set, targets, asset names, and runner-role mapping.
- `ci/sccache-strict/sccache-v0.16.0-strict.patch`: the complete upstream code and test delta.
- `scripts/sccache_strict.py`: the sole config reader and manifest verifier used by local checks and the workflow. It never publishes and never changes repository files.
- `scripts/test_sccache_strict.py`: behavior tests for configuration validation and manifest/digest verification. It does not scan repository source text.
- `.github/workflows/sccache-strict-release.yml`: unprivileged builders plus the protected publisher.
- `ci/github-actions-runners.toml`: governance entries for the new workflow's existing managed ARM64 and X64 runner classes.

The workflow does not use `.github/actions/sccache-setup`, `RUSTC_WRAPPER`, cache credentials, AWS OIDC, or any compilation cache. The derivative is built with `--locked --no-default-features --features s3` inside a container pinned by digest. The ARM64 and X64 targets are built natively on their governed runner architectures.

## Build and publication flow

1. A pull-request or manual build downloads the exact source archive and verifies its SHA-256 before extraction.
2. The build applies the repository patch with `git apply --check` followed by `git apply`.
3. Upstream strict behavior tests run before the release build.
4. Two isolated jobs build each architecture with the same source, patch, toolchain, container digest, features, normalized build environment, and path remapping.
5. The workflow fails unless the two binary SHA-256 values for each architecture match exactly.
6. Builder jobs upload binaries and a provenance manifest as workflow artifacts. Builders have `contents: read` only and no cache or publisher credentials.
7. The publisher runs only for `workflow_dispatch` at exact-current `main`, references a GitHub environment restricted to `main`, and receives `contents: write`. It does not execute Cargo, build scripts, or the produced binaries.
8. The publisher verifies the duplicate-build digests, creates a draft `tooling-sccache-*` release, uploads both binaries and the provenance manifest, and publishes the draft.
9. Publication is forbidden unless repository release immutability was enabled before the release. GitHub then locks the tag and assets and creates a release attestation. The consumer still verifies the repository-pinned SHA-256.

The repository's `GET /repos/seungpyoson/bolt-v2/immutable-releases` endpoint currently reports `enabled: false`, and the only existing environment is unrelated. Enabling release immutability and creating the main-restricted publisher environment are explicit operator prerequisites; the implementation must not silently weaken publication if either is absent. GitHub documents that immutable release assets cannot be modified or deleted after publication and recommends draft, upload, then publish: [Immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases).

## Trust boundary

- Repository review owns the upstream commit, source digest, patch, build recipe, and publisher workflow.
- Builder jobs are unprivileged and cannot publish or write the compilation cache.
- The protected publisher can create a release but cannot change the adoption pin in #1494.
- #1494 independently pins asset names and binary SHA-256 values after publication.
- PR and main cache roles cannot publish or replace the executable.
- A compromised publisher can create an unwanted release, but cannot activate it without a separate reviewed adoption change.
- Release deletion or GitHub unavailability intentionally stops installation; there is no S3, upstream-release, preinstalled-tool, or local-build fallback.

## Verification evidence

The prerequisite PR requires:

- upstream unit/integration tests proving EOF and other IPC failures return nonzero without a client-side compiler spawn;
- protocol tests proving `NotCompilation` remains allowed while `CannotCache` executes through the server and never spawns from the client;
- storage tests proving read errors, timeouts, corrupt hits, and read-write write failures return nonzero;
- a read-only miss test proving server compilation succeeds without a write attempt;
- an explicitly non-cacheable test proving server compilation succeeds, records its reason, and performs no cache operation;
- tests rejecting externally forced no-cache and recache modes;
- exact source archive and patch application evidence;
- two byte-identical clean builds per architecture;
- binary smoke tests on ARM64 and X64;
- actionlint and the repository's targeted Python self-tests;
- an internal adversarial review of the exact diff before completion is claimed.

The later #1494 adoption requires separate evidence that a missing asset, channel failure, wrong architecture, altered digest, setup failure, server death, cache read failure, and read-write write failure all stop before the job can claim valid compile evidence.

## Failure and recovery

Any verification, build, reproduction, environment, release, download, or digest failure stops the affected workflow. There is no fallback publication or installation channel.

Upgrades use a new reviewed upstream commit, source digest, rebased patch, immutable release, and adoption PR. Historical releases are retained but still depend on GitHub availability and the release not being deleted. Emergency revocation pauses affected CI or adopts a new immutable release; it never mutates an existing asset or restores the official permissive binary.

## Explicitly excluded work

- Changing PR #1495 or #1496.
- Changing Rust Probe checkout/OIDC behavior.
- Editing #1494 before immutable strict assets exist.
- Publishing to S3 or adding AWS roles, buckets, policies, or lifecycle exceptions.
- A stderr-watching wrapper or any control that detects fallback only after compiler execution.
- Upstreaming the patch; that may be proposed separately after the governed implementation is proven.
