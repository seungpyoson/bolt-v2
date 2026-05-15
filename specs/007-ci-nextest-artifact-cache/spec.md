# Feature Specification: CI Nextest Artifact Cache

**Feature Branch**: `codex/ci-195-nextest-artifact-cache`
**Created**: 2026-05-15
**Status**: Draft
**Input**: User description: "Address #195 from epic #333 without narrowing the issue body."

## User Scenarios & Testing

### User Story 1 - Preserve Warm Test Artifacts (Priority: P1)

As the maintainer, I can rerun the same CI test graph and reuse the test-profile artifacts that `cargo nextest` needs instead of rebuilding `bolt-v2` unnecessarily.

**Why this priority**: #195 exists because #193 fixed the managed target-dir cache path but warm `test` logs still showed `Compiling bolt-v2` and `Finished test profile ... in 1m47s`.

**Independent Test**: The workflow verifier rejects a `test` cache strategy that restores the managed target directory only as opaque `cache-directories` while keeping workspace test artifacts pruned by default.

**Acceptance Scenarios**:

1. **Given** a cold CI `test` shard has completed successfully, **When** the same test graph reruns warm, **Then** the cache strategy restores enough nextest/Cargo test-profile artifacts to avoid unnecessary `Compiling bolt-v2` rebuilds.
2. **Given** the warm rerun restores a cache, **When** the `test` step logs are inspected, **Then** they do not show an unnecessary workspace `Compiling bolt-v2` test-profile rebuild.
3. **Given** the cache is missing or stale, **When** a `test` shard runs, **Then** it falls back to a correct cold run and remains covered by the required aggregate gate.

### User Story 2 - Keep Sharded Cache Keys Correct And Bounded (Priority: P1)

As the maintainer, I can keep each #332 nextest partition warm without creating unbounded or stale cache entries.

**Why this priority**: #333 says #195 does not decide shard topology, but because #332 has landed first in the stack, #195 must preserve artifacts per `cargo nextest --partition` shard.

**Independent Test**: The verifier rejects a test cache key that omits shard topology, rust environment hashing, or the managed workspace target-dir mapping, and rejects keys that include unbounded commit-SHA dimensions.

**Acceptance Scenarios**:

1. **Given** the workflow has four `test` shards, **When** rust-cache builds its key, **Then** the key includes a bounded shard dimension and the rust environment hash inputs for lockfile, manifests, toolchain, target/config, and relevant compiler environment values.
2. **Given** a real Rust input changes, **When** the cache key is evaluated, **Then** the cache invalidates through rust-cache's rust-environment hash rather than reusing stale artifacts.
3. **Given** many PR commits are pushed, **When** the cache key shape is inspected, **Then** it does not create one new cache namespace per SHA for the same shard and Rust input set.

### User Story 3 - Prove Cache Size And Pruning Behavior (Priority: P1)

As the maintainer, I can see the archive-size and pruning tradeoff for preserving workspace artifacts before accepting the warm rerun improvement.

**Why this priority**: #195 imports the PR #194 external-review risk that opaque managed-target caching produced a large cache archive, about 7.1 GB in run `24609574202`, and could cause cache growth or eviction thrash.

**Independent Test**: The PR evidence must include before/after cache archive sizes and log/API evidence for pruning or eviction behavior before #195 can be called complete.

**Acceptance Scenarios**:

1. **Given** the pre-#195 baseline, **When** evidence is collected, **Then** it records existing cache archive size evidence from #343/#195 baseline runs.
2. **Given** the post-#195 CI run completes, **When** evidence is collected, **Then** it records the new per-shard cache archive size and whether rust-cache cleanup/pruning ran.
3. **Given** GitHub Actions cache storage is finite, **When** the cache strategy is reviewed, **Then** it records why the new key shape avoids cache thrash under the repository limit.

### User Story 4 - Preserve Required Test Gate Semantics (Priority: P1)

As the maintainer, I can improve warm rerun performance without weakening the required `test` signal or the aggregate `gate`.

**Why this priority**: #333 acceptance requires no child to weaken the merge gate, and #195 explicitly says missing or stale cache must still run correct tests.

**Independent Test**: `just ci-lint-workflow` passes only when `test` remains required by `gate`, `test` still depends on `source-fence`, and cache lookup/save failures do not become a success substitute for test execution.

**Acceptance Scenarios**:

1. **Given** a cache restore fails or misses, **When** the `test` shard runs, **Then** the shard still executes `just test -- --partition count:${{ matrix.shard }}/4`.
2. **Given** any shard fails, is cancelled, or is skipped unexpectedly, **When** `gate` evaluates CI results, **Then** the aggregate gate fails closed.
3. **Given** cache instrumentation is present, **When** a shard succeeds, **Then** the required success still comes from the test execution result, not from cache-hit status.

### User Story 5 - Record Exact Cold/Warm Evidence (Priority: P1)

As the maintainer, I can close #195 only with exact GitHub Actions evidence: cold/warm run IDs, log excerpts, timing comparison, and cache-size data.

**Why this priority**: The #195 body says completion depends on logs and exact run IDs, not inferred speed or local-only checks.

**Independent Test**: The PR remains draft or blocked until exact-head CI runs exist and the issue/PR evidence lists the cold and warm run IDs plus relevant log excerpts.

**Acceptance Scenarios**:

1. **Given** local verification passes, **When** exact PR-head CI has not run, **Then** the PR and issue status say evidence is pending instead of claiming #195 complete.
2. **Given** cold and warm CI runs are available, **When** the evidence is recorded, **Then** it includes run IDs, job/shard timing, cache-hit/restored-key lines, cache archive sizes, and `Compiling bolt-v2` log findings.
3. **Given** the warm run improves, **When** #195 is closed, **Then** the comparison is against the post-#193/#343 baseline, not against an unrelated workflow shape.

### Edge Cases

- #332 has landed first in this stack, so #195 must adapt to four nextest shards now rather than documenting a future adaptation point.
- `Swatinem/rust-cache` can include rust-environment hashes automatically, but a workflow verifier must still prevent disabling that key input.
- `cache-workspace-crates: "true"` preserves workspace artifacts but can enlarge caches; evidence must include size/pruning behavior before completion is claimed.
- Direct `actions/cache` use would create a second cache path unless there is a strong reason; the preferred design keeps one rust-cache strategy for the managed target directory.
- Cache keys must not include `github.sha` or an equivalent unbounded per-commit dimension for this test lane.
- Exact warm rerun proof cannot be produced while stacked PRs do not receive full `pull_request` CI on non-`main` bases.

## Requirements

### Functional Requirements

- **FR-001**: The `test` job MUST preserve the managed target directory through the existing managed Rust target-dir output.
- **FR-002**: The `test` job MUST preserve workspace `bolt-v2` test-profile artifacts needed by `cargo nextest` warm reruns.
- **FR-003**: The `test` cache MUST remain per-shard after #332 by including the bounded shard topology in the cache key.
- **FR-004**: The cache key MUST remain invalidated by real Rust inputs: `Cargo.lock`, all `Cargo.toml` manifests, `rust-toolchain.toml`, `.cargo/config.toml` when present, target/config values, and relevant compiler environment variables.
- **FR-005**: The cache key MUST NOT include an unbounded per-commit dimension such as `github.sha`.
- **FR-006**: The workflow MUST keep `add-rust-environment-hash-key` enabled for the `test` rust-cache step.
- **FR-007**: The workflow MUST avoid introducing a second test cache backend unless the design records a strong reason and a bounded migration path.
- **FR-008**: Missing, stale, or failed cache restore MUST fall back to the normal `just test -- --partition count:${{ matrix.shard }}/4` execution.
- **FR-009**: The `test` job MUST remain required by the aggregate `gate`, and `gate` MUST continue to accept only aggregate `test` success.
- **FR-010**: `test` MUST continue to depend on `source-fence`.
- **FR-011**: The workflow verifier MUST fail with actionable output if the #195 cache-artifact strategy loses managed target-dir mapping, workspace artifact preservation, shard-aware keying, rust-environment hashing, or gate semantics.
- **FR-012**: The verifier self-tests MUST include negative fixtures for missing workspace artifact preservation, disabled rust-environment hash, unbounded SHA keying, missing shard key dimension, and weakened gate/test execution.
- **FR-013**: The PR evidence MUST record exact cold and warm GitHub Actions run IDs for the final PR head or explicitly state that exact CI is blocked.
- **FR-014**: The PR evidence MUST include log excerpts proving whether warm runs do or do not show unnecessary `Compiling bolt-v2` test-profile rebuilds.
- **FR-015**: The PR evidence MUST include cold/warm timing comparison against the post-#193/#343 baseline.
- **FR-016**: The PR evidence MUST include before/after cache archive size data and any observed pruning/eviction behavior.
- **FR-017**: This branch MUST NOT implement #332 lane topology changes beyond adapting to the already-stacked #332 shape, #205 smoke-tag deduplication, #344 docs/pass-stub/evidence work, #340 config relocation, or generic #203 lint cleanup beyond #195-specific verifier rules.

### Key Entities

- **NextestArtifactCache**: The CI cache strategy for the managed target directory and workspace test-profile artifacts used by `cargo nextest`.
- **ShardCacheKey**: The bounded per-shard rust-cache key dimension layered on top of rust-cache's automatic rust-environment hash.
- **RustEnvironmentHash**: rust-cache's hash over Cargo manifests/lockfile, rust toolchain files, cargo config, and relevant compiler environment values.
- **WarmRerunEvidence**: Exact GitHub Actions run IDs, shard job timings, cache-hit/restored-key lines, cache archive sizes, and compile log excerpts.
- **CacheGrowthEvidence**: Before/after cache archive size and pruning/eviction observations.
- **GateInvariant**: Workflow and verifier contract proving cache behavior does not replace or weaken required test execution.

## Success Criteria

### Measurable Outcomes

- **SC-001**: `python3 scripts/test_verify_ci_workflow_hygiene.py` fails on fixtures that remove workspace artifact preservation, disable rust-environment hashing, remove shard keying, add SHA keying, or weaken the test gate.
- **SC-002**: `python3 scripts/verify_ci_workflow_hygiene.py` and `just ci-lint-workflow` pass after implementation.
- **SC-003**: A warm exact-head CI rerun for the same test graph does not show unnecessary `Compiling bolt-v2` test-profile rebuilds in `test` shard logs.
- **SC-004**: Warm exact-head CI wall time materially improves beyond the post-#193/#343 warm test baseline, with exact cold/warm run IDs and shard job durations.
- **SC-005**: The issue or PR records before/after cache archive sizes and any rust-cache/GitHub cache pruning or eviction observations.
- **SC-006**: Missing or stale cache behavior is evidenced or reasoned from workflow semantics as a normal cold `just test` run, not a skipped or substituted required check.

## Assumptions

- PR #348/#332 is the current stacked base, so #195 adapts to a four-shard `test` matrix.
- The repo keeps one managed Rust execution path through `rust_verification.py`; workflow YAML should continue to call `just` recipes, not raw cargo.
- `Swatinem/rust-cache` remains the preferred cache action unless direct evidence shows it cannot preserve the required artifacts safely.
- Exact-head CI and external reviews remain blocked until this stacked branch can receive a real full CI run or the user approves a different CI path.
