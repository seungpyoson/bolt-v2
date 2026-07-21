# Strict sccache Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and publish a reproducible ARM64/X64 sccache v0.16.0 derivative in which only explicit server classifications authorize client compiler execution and every cache, storage, IPC, or lifecycle failure is fatal.

**Architecture:** The repository stores one reviewed patch against the digest-pinned upstream archive, one build-provenance TOML, and one Python config/manifest verifier. A single workflow builds two replicas per architecture inside a pinned musl container without sccache, verifies byte equality and host execution, and permits publication only from exact-current `main` through a protected immutable-release environment. PR #1494 remains the sole consumer/adoption change and the sole owner of install IDs, binary digests, cache location, and runtime timeout values.

**Tech Stack:** Rust 1.97.1, sccache v0.16.0, Python 3.12 standard library, TOML, GitHub Actions, GitHub Releases REST API 2026-03-10, Docker/OCI, musl Linux ARM64/X64.

## Global Constraints

- Base upstream source is commit `b799af2eea02bba9e0ef2550775fe10296b62981`; archive SHA-256 is `a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3`.
- Builder image is `docker.io/clux/muslrust@sha256:76df925f30e106755517c78cd57b6ea890a73d6f59fcff842849006e734c174e`; it must report `rustc 1.97.1` at commit `8bab26f4f68e0e26f0bb7960be334d5b520ea452`.
- Build features, default-feature policy, profile, native targets, and normalized build settings come only from `ci/sccache-strict.toml` and are bound into every candidate manifest. Vendored OpenSSL is lockfile-pinned and removes a mutable host-library dependency from static musl builds.
- `SOURCE_DATE_EPOCH` is `1781869188`, the pinned upstream commit time.
- `RUSTC_WRAPPER`, sccache, incremental compilation, cache credentials, and AWS OIDC are absent from builders.
- Only `NotCompilation` and `CannotCache(reason)` responses may authorize client compiler execution. Reasons are telemetry, never runtime policy.
- Only an authoritative S3 `NotFound` is a cache miss. Every other storage, corruption, timeout, IPC, or configuration failure is fatal.
- The strict binary requires positive integer values for `SCCACHE_STRICT_STARTUP_TIMEOUT_MS`, `SCCACHE_STRICT_IPC_TIMEOUT_MS`, `SCCACHE_STRICT_CACHE_READ_TIMEOUT_MS`, and `SCCACHE_STRICT_CACHE_WRITE_TIMEOUT_MS`; it has no defaults. PR #1494 later exports these from `ci/sccache-location.toml`.
- Exactly one S3 backend and one configured `SCCACHE_S3_RW_MODE` are accepted. Disk fallback, alternate backends, multilevel storage, forced no-cache, forced recache, and automatic server startup are rejected.
- External Actions use full 40-character commit SHAs. Repository automation uses only `GITHUB_TOKEN`.
- Tests are behavioral. Do not add repository source-scanning tests.
- This PR does not edit PR #1494, #1495, #1496, Rust Probe, AWS IAM, repository settings, or release assets.

---

### Task 1: Build-Provenance Configuration and Manifest Verifier

**Files:**
- Create: `ci/sccache-strict.toml`
- Create: `scripts/sccache_strict.py`
- Create: `scripts/test_sccache_strict.py`

**Interfaces:**
- Produces: `StrictBuildConfig`, `TargetConfig`, `CandidateManifest`, `load_config()`, `write_candidate_manifest()`, `verify_candidate_set()`, `validate_publish_context()`, and `verify_release_record()`.
- Produces CLI commands used by the workflow: `show-target`, `candidate-manifest`, `verify-candidates`, `validate-publish-context`, and `verify-release-record`.
- Does not publish, call GitHub, mutate tracked files, or own consumer asset IDs/digests.

- [ ] **Step 1: Write configuration-validation tests**

Add `unittest` cases that load a temporary TOML and prove exact acceptance/rejection:

```python
class LoadConfigTests(unittest.TestCase):
    def test_loads_exact_pinned_build(self) -> None:
        config = load_config(REPO_ROOT / "ci/sccache-strict.toml")
        self.assertEqual(config.schema_version, 1)
        self.assertEqual(config.source_commit, "b799af2eea02bba9e0ef2550775fe10296b62981")
        self.assertEqual(config.source_sha256, "a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3")
        self.assertEqual(config.container_digest, "sha256:76df925f30e106755517c78cd57b6ea890a73d6f59fcff842849006e734c174e")
        self.assertEqual(set(config.targets), {"ARM64", "X64"})

    def test_rejects_unknown_key_and_unpinned_image(self) -> None:
        document = valid_document()
        document["build"]["extra"] = "forbidden"
        with self.assertRaisesRegex(ValueError, "unknown build key"):
            load_document(document)
        document = valid_document()
        document["build"]["container"] = "docker.io/clux/muslrust:stable"
        with self.assertRaisesRegex(ValueError, "container must use sha256 digest"):
            load_document(document)
```

- [ ] **Step 2: Run the tests and confirm the missing-module failure**

Run: `python3.12 -m unittest scripts/test_sccache_strict.py -v`

Expected: FAIL because `scripts.sccache_strict` and `ci/sccache-strict.toml` do not exist.

- [ ] **Step 3: Add the exact build-provenance TOML**

Use this schema and values:

```toml
schema_version = 1

[source]
version = "0.16.0"
commit = "b799af2eea02bba9e0ef2550775fe10296b62981"
archive_url = "https://github.com/mozilla/sccache/archive/b799af2eea02bba9e0ef2550775fe10296b62981.tar.gz"
archive_sha256 = "a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3"
source_date_epoch = 1781869188
patch = "ci/sccache-strict/sccache-v0.16.0-strict.patch"

[build]
container = "docker.io/clux/muslrust@sha256:76df925f30e106755517c78cd57b6ea890a73d6f59fcff842849006e734c174e"
rustc_release = "1.97.1"
rustc_commit = "8bab26f4f68e0e26f0bb7960be334d5b520ea452"
features = ["s3", "vendored-openssl"]
default_features = false
profile = "release"

[verification]
strict_timeout_ms = 1000

[targets.ARM64]
triple = "aarch64-unknown-linux-musl"
elf_machine = "AArch64"

[targets.X64]
triple = "x86_64-unknown-linux-musl"
elf_machine = "Advanced Micro Devices X86-64"
```

- [ ] **Step 4: Implement strict typed parsing**

Define immutable dataclasses and reject booleans-as-integers, missing keys, unknown keys, non-HTTPS source URLs, non-full SHAs, non-64-character digests, duplicate triples, and any target outside `ARM64`/`X64`:

```python
@dataclasses.dataclass(frozen=True)
class TargetConfig:
    triple: str
    elf_machine: str

@dataclasses.dataclass(frozen=True)
class StrictBuildConfig:
    schema_version: int
    source_version: str
    source_commit: str
    source_url: str
    source_sha256: str
    source_date_epoch: int
    patch_path: pathlib.Path
    container: str
    container_digest: str
    rustc_release: str
    rustc_commit: str
    features: tuple[str, ...]
    default_features: bool
    profile: str
    verification_timeout_ms: int
    targets: Mapping[str, TargetConfig]

def load_config(path: pathlib.Path) -> StrictBuildConfig:
    document = tomllib.loads(path.read_text(encoding="utf-8"))
    return load_document(document, repo_root=path.resolve().parents[1])
```

- [ ] **Step 5: Write failing manifest tests**

Cover same-head binding, architecture/replica uniqueness, binary SHA-256 recomputation, patch SHA-256 recomputation, exact four-candidate set, per-architecture byte equality, exact asset set, release tag target, `immutable: true`, and asset API digests:

```python
def test_verify_candidates_rejects_cross_run_manifest(self) -> None:
    manifests, binaries = candidate_fixture()
    manifests[0]["run_id"] = "other-run"
    with self.assertRaisesRegex(ValueError, "same workflow run"):
        verify_candidate_set(manifests, binaries)

def test_verify_release_record_requires_exact_immutable_assets(self) -> None:
    expected = verified_candidate_set()
    release = release_fixture(expected, immutable=False)
    with self.assertRaisesRegex(ValueError, "release is not immutable"):
        verify_release_record(expected, release, tag_ref={}, attestations={})
```

- [ ] **Step 6: Implement canonical manifests and pure publication gates**

Use sorted-key compact JSON and lowercase hex digests. The candidate manifest must contain exactly:

```python
REQUIRED_CANDIDATE_KEYS = {
    "schema_version", "repository", "run_id", "run_attempt", "head_sha", "architecture",
    "target", "replica", "source_commit", "source_sha256", "source_date_epoch",
    "patch_sha256", "container", "rustc_release", "rustc_commit", "features",
    "default_features", "profile", "verification_timeout_ms", "binary_name",
    "binary_sha256", "binary_size",
}
```

`validate_publish_context()` must require `event_name == "workflow_dispatch"`, `requested_sha == event_sha == remote_main_sha`, `event_ref == "refs/heads/main"`, and live environment JSON for `strict-sccache-publisher` whose deployment policy admits protected branches. A missing environment is failure; the workflow must not rely on GitHub implicitly creating it. GitHub's immutable-setting endpoint requires administration-read permission that `GITHUB_TOKEN` cannot request, so `verify_release_record()` is the executable immutability gate: it requires the actual tag ref to point directly to the exact head commit, the release's matching target field, `draft == false`, `immutable == true`, the exact three assets (ARM64 binary, X64 binary, provenance JSON), matching `sha256:` API digests, and at least one release attestation record per binary digest. A mutable publication result must be deleted with only its exact generated tag before the workflow fails.

- [ ] **Step 7: Run the targeted tests**

Run: `python3.12 -m unittest scripts/test_sccache_strict.py -v`

Expected: all config, manifest, and publication-gate tests PASS.

- [ ] **Step 8: Commit the config/verifier unit**

```bash
git add ci/sccache-strict.toml scripts/sccache_strict.py scripts/test_sccache_strict.py
git commit -m "feat(ci): govern strict sccache provenance"
```

### Task 2: Explicit Classification and Connect-Only Client Patch

**Files:**
- Create: `ci/sccache-strict/sccache-v0.16.0-strict.patch`
- Upstream files represented in the patch: `src/protocol.rs`, `src/server.rs`, `src/commands.rs`, `src/client.rs`, `src/net.rs`
- Upstream tests represented in the patch: `src/commands.rs` test module, `tests/system.rs`

**Interfaces:**
- Produces upstream protocol variants `CompileResponse::NotCompilation` and `CompileResponse::CannotCache(String)`.
- Produces `connect_required(addr, ipc_timeout) -> Result<ServerConnection>`.
- Produces `StrictTimeouts::from_env() -> Result<StrictTimeouts>` with four required positive durations.
- Preserves original client stdin, stdout, stderr, environment, cwd, and jobserver file descriptors for classified client execution.

- [ ] **Step 1: Prepare a disposable exact upstream tree**

Run:

```bash
STRICT_SCCACHE_TMP="$(mktemp -d)"
export STRICT_SCCACHE_TMP
curl -fL --proto '=https' --tlsv1.2 -o "$STRICT_SCCACHE_TMP/sccache.tar.gz" "https://github.com/mozilla/sccache/archive/b799af2eea02bba9e0ef2550775fe10296b62981.tar.gz"
printf '%s  %s\n' 'a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3' "$STRICT_SCCACHE_TMP/sccache.tar.gz" | sha256sum --check --strict
mkdir "$STRICT_SCCACHE_TMP/sccache-source"
tar -xzf "$STRICT_SCCACHE_TMP/sccache.tar.gz" --strip-components=1 -C "$STRICT_SCCACHE_TMP/sccache-source"
git -C "$STRICT_SCCACHE_TMP/sccache-source" init
git -C "$STRICT_SCCACHE_TMP/sccache-source" add .
git -C "$STRICT_SCCACHE_TMP/sccache-source" -c user.name=builder -c user.email=builder@example.invalid commit -m upstream
```

Expected: digest check succeeds and the disposable repository has one clean baseline commit.

- [ ] **Step 2: Add failing protocol and response-handler tests upstream**

The tests must assert:

```rust
#[test]
fn cannot_cache_reason_authorizes_exactly_one_client_command() {
    let response = CompileResponse::CannotCache("-".to_owned());
    let (result, commands) = run_response(response, client_command_that_reads_stdin());
    assert_eq!(result.unwrap(), 0);
    assert_eq!(commands.spawn_count(), 1);
}

#[test]
fn eof_after_compile_started_never_spawns() {
    let (result, commands) = run_compile_started_with_read_error(ErrorKind::UnexpectedEof);
    assert!(result.is_err());
    assert_eq!(commands.spawn_count(), 0);
}

#[test]
fn missing_server_never_starts_one() {
    let result = connect_required(&unused_socket(), strict_timeouts());
    assert!(result.is_err());
    assert_eq!(observed_server_processes(), 0);
}
```

Define the helpers inside the test module with `MockCommandCreator`, an in-memory fake connection, and an atomic spawn counter; do not inspect Rust source text.

- [ ] **Step 3: Run the focused tests and verify red state**

Run inside the pinned builder image:

```bash
cargo test --locked --no-default-features --features s3,vendored-openssl strict_ -- --nocapture
```

Expected: FAIL because the dedicated variants, required timeouts, and connect-only helper do not exist.

- [ ] **Step 4: Split explicit classifications from failures**

Change the response protocol and server classification to:

```rust
pub enum CompileResponse {
    CompileStarted,
    NotCompilation,
    CannotCache(String),
    Rejected(String),
    UnsupportedCompiler(OsString),
}
```

`check_compiler()` returns `CannotCache(why.to_string())` after incrementing `requests_not_cacheable` and `not_cached[why]`; it returns `NotCompilation` after incrementing `requests_not_compile`. `handle_compile_response()` runs the client command only for those two variants. `Rejected`, `UnsupportedCompiler`, EOF, other read errors, and unexpected responses return errors before `spawn()`.

- [ ] **Step 5: Add required strict timeout parsing**

Add:

```rust
#[derive(Clone, Copy, Debug)]
pub struct StrictTimeouts {
    pub startup: Duration,
    pub ipc: Duration,
    pub cache_read: Duration,
    pub cache_write: Duration,
}

impl StrictTimeouts {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            startup: required_millis("SCCACHE_STRICT_STARTUP_TIMEOUT_MS")?,
            ipc: required_millis("SCCACHE_STRICT_IPC_TIMEOUT_MS")?,
            cache_read: required_millis("SCCACHE_STRICT_CACHE_READ_TIMEOUT_MS")?,
            cache_write: required_millis("SCCACHE_STRICT_CACHE_WRITE_TIMEOUT_MS")?,
        })
    }
}
```

Reject missing, zero, negative, non-decimal, overflow, or whitespace-padded values. `StartServer`, `InternalStartServer`, `Compile`, `ShowStats`, `ZeroStats`, and `StopServer` load the same values. `--version` remains configuration-free.

- [ ] **Step 6: Make all non-start commands connect-only with governed IPC timeouts**

Extend `net::Connection` with `set_read_timeout()` and `set_write_timeout()`. Add `ServerConnection::set_timeout()` to apply both values to its cloned reader and writer streams. Replace `connect_or_start_server()` in compile, zero-stats, stats, and evidence paths with:

```rust
fn connect_required(addr: &SocketAddr, timeout: Duration) -> Result<ServerConnection> {
    let mut connection = connect_to_server(addr).context("governed sccache server is not running")?;
    connection.set_timeout(timeout)?;
    Ok(connection)
}
```

Delete the empty-statistics fallback from `ShowStats`. Keep `run_server_process()` reachable only from explicit `StartServer`.

- [ ] **Step 7: Add classified-command preservation tests**

Pass commands equivalent to the following through the dedicated `CannotCache` and `NotCompilation` responses:

```bash
printf 'pub fn probe() {}\n' | sccache rustc - --crate-name stdin_probe --crate-type lib --emit metadata -o stdin_probe.rmeta
sccache rustc probe.rs --crate-name asm_probe --crate-type lib --emit asm -o probe.s
```

Assert that each typed response authorizes exactly one client command with the original arguments and that EOF after `CompileStarted` authorizes none. The patch leaves the upstream client command construction unchanged, so inherited stdin, output file descriptors, cwd, environment, and jobserver descriptors remain structural equivalents rather than a second execution implementation.

- [ ] **Step 8: Regenerate the repository patch and commit**

Run `git -C "$STRICT_SCCACHE_TMP/sccache-source" diff --binary HEAD > ci/sccache-strict/sccache-v0.16.0-strict.patch`, then:

```bash
git add ci/sccache-strict/sccache-v0.16.0-strict.patch
git commit -m "feat(ci): make sccache classification explicit"
```

### Task 3: Strict S3, Cache Integrity, and Write Completion Patch

**Files:**
- Modify: `ci/sccache-strict/sccache-v0.16.0-strict.patch`
- Upstream files represented in the patch: `src/cache/cache.rs`, `src/cache/cache_io.rs`, `src/compiler/compiler.rs`, `src/server.rs`, `src/config.rs`
- Upstream tests represented in the patch: `src/cache/cache.rs`, `src/cache/cache_io.rs`, `src/compiler/compiler.rs`, `src/server.rs`

**Interfaces:**
- Consumes: `StrictTimeouts`, the dedicated compile classifications, and the existing `CacheMode`.
- Produces: exactly-one-S3 startup validation, fatal capability checks, authoritative `NotFound` misses, `CacheObjectError::{Missing, Corrupt}`, read-only zero-put, and read-write completed-put success.

- [ ] **Step 1: Write failing backend and capability tests**

Add behavior tests for missing S3, `SCCACHE_MULTILEVEL_CHAIN`, disk configuration, a read capability error, rate limiting, and a read-write capability failure:

```rust
#[test]
fn strict_storage_rejects_missing_and_multilevel_backends() {
    assert!(strict_storage_from_config(&config_without_remote()).is_err());
    with_env("SCCACHE_MULTILEVEL_CHAIN", "s3,disk", || {
        assert!(strict_storage_from_config(&valid_s3_config()).is_err());
    });
}

#[tokio::test]
async fn read_write_check_never_demotes() {
    let storage = remote_storage_with_write_error(ErrorKind::PermissionDenied);
    assert!(storage.check().await.is_err());
}
```

- [ ] **Step 2: Run focused storage tests and verify red state**

Run:

```bash
cargo test --locked --no-default-features --features s3,vendored-openssl strict_ -- --nocapture
```

Expected: FAIL because upstream permits disk/multilevel selection, rate-limit tolerance, and read-write demotion.

- [ ] **Step 3: Enforce one S3 backend and authoritative configured mode**

Replace permissive selection with a strict constructor:

```rust
pub fn strict_storage_from_config(config: &Config, pool: &Handle) -> Result<Arc<dyn Storage>> {
    ensure!(env::var_os("SCCACHE_MULTILEVEL_CHAIN").is_none(), "multilevel storage is forbidden");
    match &config.cache {
        Some(cache @ CacheType::S3(_)) => build_single_cache(cache, &config.basedirs, pool),
        Some(_) => bail!("only the governed S3 backend is supported"),
        None => bail!("governed S3 configuration is required"),
    }
}
```

Make every non-`NotFound` read error propagate. Make read capability rate limiting fatal. In configured read-write mode, propagate every failed write check rather than returning `ReadOnly`. The negotiated mode must equal configured `SCCACHE_S3_RW_MODE` before startup notification succeeds.

- [ ] **Step 4: Write failing corruption tests**

Create cache entries whose stdout, stderr, required object, and optional object members are separately missing or malformed. Assert every present-but-corrupt member fails and an absent optional object alone remains allowed:

```rust
#[test]
fn corrupt_optional_object_is_not_treated_as_absent() {
    let mut entry = cache_entry_with_corrupt_member("optional.o");
    let error = entry.extract_objects(optional_output("optional.o")).wait().unwrap_err();
    assert!(matches!(error.downcast_ref::<CacheObjectError>(), Some(CacheObjectError::Corrupt)));
}
```

- [ ] **Step 5: Make cache decoding distinguish absence from corruption**

Use a typed error:

```rust
#[derive(Debug, thiserror::Error)]
pub enum CacheObjectError {
    #[error("cache object is absent")]
    Missing,
    #[error("cache object is corrupt")]
    Corrupt,
}
```

Change `get_stdout()` and `get_stderr()` to return `Result<Vec<u8>>`. Map only `ZipError::FileNotFound` to `Missing`; compression mismatch, decode failure, malformed ZIP data, and persistence failure are `Corrupt`. Optional extraction ignores only `Missing`.

- [ ] **Step 6: Make cache-read failures and forced modes fatal**

Pass `StrictTimeouts.cache_read` into `get_cached_or_compile()`. Replace `MissType::CacheReadError` and `MissType::TimedOut` conversion with returned errors before the compiler future is created. Reject `SCCACHE_NO_CACHE` and `SCCACHE_RECACHE` in `check_compiler()` with `CompileResponse::Rejected`; delete the environment opt-out path from the governed build.

- [ ] **Step 7: Write failing read-only and read-write completion tests**

Use a counting mock storage:

```rust
#[tokio::test]
async fn read_only_miss_never_calls_put() {
    let storage = CountingStorage::read_only_miss();
    let result = compile_with(storage.clone(), CacheMode::ReadOnly).await;
    assert_eq!(result.retcode, Some(0));
    assert_eq!(storage.put_calls(), 0);
}

#[tokio::test]
async fn read_write_put_failure_invalidates_success() {
    let storage = CountingStorage::read_write_put_error();
    let result = compile_with(storage, CacheMode::ReadWrite).await;
    assert_ne!(result.retcode, Some(0));
}
```

- [ ] **Step 8: Thread cache mode and governed write timeout through the server**

Add `cache_mode` and `cache_write_timeout` to `SccacheService`. On a read-only miss, drop the unpolled write future and return the compiler result. On a read-write miss, await the future inside `tokio::time::timeout`; on timeout or error, set a nonzero result and append a stable strict-cache failure message. Do not return success before a completed write.

- [ ] **Step 9: Complete direct strict-boundary behavior tests**

Use the upstream library harness and counting mock storage to prove: a refused S3 capability check is fatal; disk and multilevel redirects are rejected; a cache-read error stops before compiler execution; read-only misses make zero `put` calls; required read-write errors and timeouts set a nonzero compile result; forced no-cache/recache inputs fail; corrupt stdout, stderr, required outputs, and optional outputs fail; and only typed classifications authorize the unchanged client command path. Build the debug binary before the library suite because the upstream `test_server_port_in_use` test resolves that binary from the target directory.

- [ ] **Step 10: Run the complete patched-tree behavior suite**

Run inside the pinned container:

```bash
cargo fmt --all -- --check
cargo build --locked --offline --no-default-features --features "$features" --target "$target"
cargo test --locked --no-default-features --features "$features" --target "$target" --lib
cargo build --locked --offline --profile "$profile" --no-default-features --features "$features" --target "$target"
```

The shell values above must be emitted from the validated TOML configuration; they are not independent workflow or documentation defaults.

Expected: formatting clean; all upstream and strict tests PASS; release build succeeds.

- [ ] **Step 11: Regenerate and commit the complete one-file patch**

Regenerate `ci/sccache-strict/sccache-v0.16.0-strict.patch` from the disposable upstream repository and verify it applies to a second clean extraction with `git apply --check`.

```bash
git add ci/sccache-strict/sccache-v0.16.0-strict.patch
git commit -m "feat(ci): make sccache storage fail closed"
```

### Task 4: Reproducible Builders and Immutable Publisher

**Files:**
- Create: `.github/workflows/sccache-strict-release.yml`
- Modify: `ci/github-actions-runners.toml`
- Modify: `scripts/test_sccache_strict.py`

**Interfaces:**
- Consumes: config/verifier CLI and complete upstream patch.
- Produces jobs `preflight`, `build-arm64`, `build-x64`, and `publish`.
- Uses `actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd`, `actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405`, `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`, and `actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c`.

- [ ] **Step 1: Add failing workflow-context and manifest tests**

Test publish refusal for PR events, non-main refs, stale requested SHA, cross-run artifacts, duplicate replicas, wrong architecture, wrong container/toolchain, pre-existing release namespace, absent environment marker, mutable release cleanup, and mismatched API digest.

Run: `python3.12 -m unittest scripts/test_sccache_strict.py -v`

Expected: new cases FAIL until the verifier exposes every required gate.

- [ ] **Step 2: Complete the verifier gates**

Make the pure functions return canonical JSON on success and a single nonzero CLI exit on any mismatch. Secrets and token values must never be included in errors or manifests.

- [ ] **Step 3: Add workflow triggers, permissions, and exact-main preflight**

Use this shape:

```yaml
name: Strict sccache release

on:
  pull_request:
  workflow_dispatch:
    inputs:
      sha:
        description: Full exact-current main SHA for a publication run
        required: true
        type: string
      publish:
        description: Publish immutable release assets
        required: true
        default: false
        type: boolean

permissions: {}

jobs:
  preflight:
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    permissions:
      actions: read
      contents: read
```

For PRs, preflight binds builders to `github.sha` and forces `publish=false`. For dispatch, `publish=true` requires a 40-character lowercase SHA equal to `github.sha`, `refs/heads/main`, and the SHA returned by `GET /repos/{repository}/branches/{default_branch}` using API version `2026-03-10`. With `actions: read`, preflight also reads `GET /repos/{repository}/environments/strict-sccache-publisher`; it emits `publisher_ready=true` only after `validate-publish-context` accepts the live document. The publish job's `if` condition requires that output, preventing a missing environment from being implicitly created by the honest workflow.

- [ ] **Step 4: Add two replicas for each native architecture**

`build-arm64` uses `${{ vars.CI_RUNNER_MANAGED_HEAVY }}` and `strategy.matrix.replica: [a, b]`. `build-x64` uses `${{ vars.CI_RUNNER_MANAGED_LIGHT }}` with the same replicas. Each job:

1. checks out the bound SHA with `persist-credentials: false`;
2. resolves its target from `sccache_strict.py show-target`;
3. downloads and digest-verifies the source archive;
4. applies the patch with `git apply --check` and `git apply`;
5. verifies the pinned container's exact Rust release and commit, then starts it once with network to run `cargo fetch --locked --target <triple>` into a fresh mounted `CARGO_HOME`;
6. starts the same container with `--network none`, read-only `CARGO_HOME`, fresh target directory, `CARGO_NET_OFFLINE=true`, `CARGO_INCREMENTAL=0`, empty `RUSTC_WRAPPER`, fixed locale/timezone/umask and mount paths, and the TOML-owned `SOURCE_DATE_EPOCH`;
7. runs the complete upstream library suite and the TOML-owned feature/profile release build (the legacy integration harnesses that require local-disk startup are inapplicable to the strict derivative);
8. verifies ELF machine, static musl linkage, and the TOML-owned sccache version on the host outside the container, then proves `--start-server` fails without governed S3 configuration instead of selecting a local cache;
9. creates a candidate manifest and uploads exactly one binary plus one manifest.

- [ ] **Step 5: Add protected publisher with exact input/output predicates**

The publisher has:

```yaml
  publish:
    if: ${{ github.event_name == 'workflow_dispatch' && inputs.publish && needs.preflight.outputs.publisher_ready == 'true' }}
    needs: [preflight, build-arm64, build-x64]
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    environment: strict-sccache-publisher
    permissions:
      actions: read
      attestations: read
      contents: write
```

It downloads only attempt-qualified artifacts from its own `needs` graph, rejects manifests from another run or attempt, recomputes all four binary digests, validates manifests, and refuses an existing tag/ref/release/draft. Immediately before creation it rechecks current main and the protected environment. It uses the ephemeral `GITHUB_TOKEN` with the pinned API version only in the release commands. It creates a draft, uploads the two agreed binaries and canonical provenance manifest, and publishes. If the response is mutable, it deletes only that exact newly created release and tag and fails. Otherwise it reads the actual Git tag ref and the user- or organization-owner release-attestation endpoints with `predicate_type=release`, then calls `verify-release-record`.

- [ ] **Step 6: Register the workflow's sole runner mapping**

Append:

```toml
[workflows.sccache_strict_release]
preflight = "github_hosted"
build-arm64 = "managed_heavy"
build-x64 = "managed_light"
publish = "github_hosted"
```

No `.github/actionlint.yaml` change is needed because the workflow uses only already-allowlisted variables and runner classes.

- [ ] **Step 7: Run targeted static verification**

Run:

```bash
python3.12 -m unittest scripts/test_sccache_strict.py -v
actionlint -config-file .github/actionlint.yaml .github/workflows/sccache-strict-release.yml
git diff --check
```

Expected: all Python tests PASS, actionlint emits no errors, and diff check is clean.

- [ ] **Step 8: Commit the workflow unit**

```bash
git add .github/workflows/sccache-strict-release.yml ci/github-actions-runners.toml scripts/sccache_strict.py scripts/test_sccache_strict.py
git commit -m "feat(ci): publish strict sccache assets"
```

### Task 5: Exact-Head Evidence and Review Handoff

**Files:**
- Modify only if review finds a defect: files introduced in Tasks 1-4
- Do not modify: `.github/actions/sccache-setup/action.yml`, `ci/sccache-location.toml`, `.github/workflows/advisory.yml`, `.github/workflows/rust-probe.yml`

**Interfaces:**
- Produces: one standalone prerequisite PR whose lasting body names #1494 as the remaining adoption scope.
- Does not publish a release or mutate repository settings.

- [ ] **Step 1: Recreate the source and prove patch identity from scratch**

Use a new temporary directory, download the archive, verify its digest, run `git apply --check`, apply the patch, and confirm the patched tree has no uncommitted generator residue after its tests.

- [ ] **Step 2: Run all local non-heavy checks fresh**

```bash
python3.12 -m unittest scripts/test_sccache_strict.py -v
actionlint -config-file .github/actionlint.yaml .github/workflows/sccache-strict-release.yml
git diff --check 27b6d20e1520956d721312b8f865fa0e2d31ffbf..HEAD
git status --short
```

Expected: Python tests PASS; actionlint and diff check emit no errors; status is clean.

- [ ] **Step 3: Perform an internal adversarial exact-diff review**

Review the complete base-to-head diff against every behavior-matrix row. Explicitly attempt to find: a client spawn reachable from an error, auto-start or empty-stats recovery, non-S3/multilevel/disk selection, RW-to-RO demotion, RO `put`, write success before completion, corrupt optional-output suppression, forced mode, ungoverned timeout, second install/publication channel, mutable Action, cross-run artifact acceptance, stale-main publication, or duplicated install pin. Resolve every substantive finding before push.

- [ ] **Step 4: Push the exact branch head without waiting for CI**

Run `git push -u origin fix/ci-strict-sccache-runtime` and record `git rev-parse HEAD`. Advisory workflow results belong to the reviewer; do not wait for them.

- [ ] **Step 5: Open the standalone PR and request the required reviewer**

The PR body must say:

```markdown
## Scope

Standalone prerequisite for #1494: build and publish the governed strict-sccache derivative. This PR does not adopt the binary in existing compile workflows.

## Remaining accepted scope

#1494 remains responsible for immutable release/asset/digest adoption, runtime timeout values, direct installation, fail-closed setup, governed wrapper routing, and exact-head negative controls. #1495, #1496, and Rust Probe are unchanged.

## Operator prerequisites

Before the first publication dispatch, the repository owner must enable immutable releases and create the `strict-sccache-publisher` environment restricted to `main`. A missing environment prevents publication. Disabled immutability causes the exact mutable release and tag created by the attempted dispatch to be removed before the workflow fails.
```

Resolve node ID `U_kgDOEZMFhA` to its current login, request that reviewer, and verify the live main ruleset has no missing required control. Do not merge.

- [ ] **Step 6: Hand off exact-head review evidence**

Provide reviewers the base SHA, head SHA, complete changed-file list, source/archive/container/toolchain pins, local command outputs, and the dedicated workflow run IDs once available. Ask them to review the complete diff and live immutable-release/environment state. CI is evidence, never merge authority.
