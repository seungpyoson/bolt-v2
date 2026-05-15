# Feature Specification: CI Parallel Heavy Lanes

**Feature Branch**: `codex/ci-332-parallel-heavy-lanes`
**Created**: 2026-05-15
**Status**: Draft
**Input**: User description: "Address #332 from epic #333 without narrowing the issue body."

## User Scenarios & Testing

### User Story 1 - Split Serialized Heavy Checks (Priority: P1)

As the maintainer, I can see host clippy and aarch64 check run as independent CI jobs instead of one serialized `clippy` job compiling the dependency graph twice on one runner.

**Why this priority**: #332 targets the measured `clippy` critical path from run `25855655415`, where `check-aarch64` and host `clippy` were serialized inside one job.

**Independent Test**: `just ci-lint-workflow` fails when `check-aarch64` is not a top-level job, lacks managed setup/cache ownership, is missing from `gate.needs`, or is not checked by the aggregate gate.

**Acceptance Scenarios**:

1. **Given** the workflow starts a build-affecting PR run, **When** heavy Rust checks start, **Then** host `clippy` and `check-aarch64` are separate top-level jobs that can run in parallel.
2. **Given** `check-aarch64` fails, is cancelled, or is skipped unexpectedly, **When** `gate` evaluates required lanes, **Then** the aggregate gate fails closed.
3. **Given** host `clippy` fails, is cancelled, or is skipped unexpectedly, **When** `gate` evaluates required lanes, **Then** the aggregate gate fails closed.

### User Story 2 - Shard Full Nextest Deterministically (Priority: P1)

As the maintainer, I can run the full Rust test lane as four deterministic `cargo nextest` partitions while preserving one required aggregate signal for merge protection.

**Why this priority**: The #343 baseline records one `test` job running 882 tests across 45 binaries. #332 exists to reduce that monolithic lane without weakening coverage.

**Independent Test**: The workflow hygiene self-test rejects a `test` job without `strategy.matrix.shard: [1, 2, 3, 4]`, without `fail-fast: false`, without `just test -- --partition count:${{ matrix.shard }}/4`, or without shard-specific cache keys/log labels.

**Acceptance Scenarios**:

1. **Given** the full test lane runs, **When** GitHub expands the matrix, **Then** it creates four shards using shard values 1, 2, 3, and 4.
2. **Given** a shard starts, **When** it logs its reproduction command, **Then** the logs include the exact local command using `cargo nextest --partition count:<shard>/4` semantics through the managed `just test --` path.
3. **Given** any shard fails, is cancelled, or is skipped unexpectedly, **When** `gate` evaluates `needs.test.result`, **Then** only aggregate `success` passes.

### User Story 3 - Preserve Source-Fence Ownership (Priority: P1)

As the maintainer, I can tell exactly which lane owns the canonical source-fence filters so #332 does not silently duplicate or remove #342 coverage.

**Why this priority**: #333 and #332 explicitly require #342/#332 source-fence ownership coordination. The current #342 branch documents temporary duplicate execution until #332 resolves it.

**Independent Test**: The spec, workflow comment, and linter contract document whether full nextest intentionally duplicates or excludes #342 source-fence filters, and the aggregate gate requires the lane that owns those filters.

**Acceptance Scenarios**:

1. **Given** #342 owns source-fence filters, **When** #332 sharding lands, **Then** the implementation either excludes those filters from the full nextest shards or explicitly documents intentional duplicate execution.
2. **Given** the implementation excludes source-fence filters from full nextest shards, **When** `gate` evaluates required lanes, **Then** `source-fence` remains required so coverage is not silently removed.
3. **Given** the implementation intentionally duplicates the filters, **When** reviewers inspect the workflow, **Then** the duplicate ownership is explicit and covered by one aggregate gate.

### User Story 4 - Keep Narrow Lint Ownership (Priority: P2)

As the maintainer, I can extend workflow lint only for the exact #332 topology so this PR does not absorb generic #203 hygiene.

**Why this priority**: The #332 comment under epic #333 says this issue owns lane parallelization and only the lint allow-list extension for the specific new lanes; generic lint hygiene belongs to #203.

**Independent Test**: `just ci-lint-workflow` enforces the new `check-aarch64` and sharded `test` topology, while the spec and PR body name #195, #205, #344, and #340 as residual non-#332 work.

**Acceptance Scenarios**:

1. **Given** `check-aarch64` lacks setup, managed target-dir cache, or a cache key, **When** lint runs, **Then** it fails with the exact missing topology item.
2. **Given** the test matrix omits a shard, uses the wrong partition syntax, or lacks actionable shard output, **When** lint runs, **Then** it fails with the exact missing topology item.
3. **Given** a requested change belongs to cache persistence, smoke-tag dedup, docs-only path filtering, or config relocation, **When** this #332 slice is reviewed, **Then** it is documented as out of scope and not silently implemented.

### Edge Cases

- #342 owns the early source-fence lane and remains a required gate dependency after #332.
- The #332 issue body proposes `--partition count:${{ matrix.shard }}/4`; nextest documentation recommends `slice:` over `count:`, but #332 explicitly requires `count:` unless there is a strong reason to deviate.
- Matrix `fail-fast` defaults to true in GitHub Actions. This feature must set it false so all shard outcomes are observable and cancellation is not an expected sibling-failure side effect.
- `just test -- --partition ...` must pass args through the managed recipe without introducing a second unmanaged test path.
- Cache keys must be shard-aware enough for #195 to reason about warm reruns, but not unbounded by commit SHA or arbitrary runtime values.
- Exact before/after CI timing cannot be completed until the final PR head receives a real CI run; local verification must not be presented as exact-head CI evidence.

## Requirements

### Functional Requirements

- **FR-001**: The workflow MUST split the current serialized `clippy` job into separate top-level `clippy` and `check-aarch64` jobs.
- **FR-002**: The `clippy` job MUST run host-only `just clippy` and MUST NOT install the aarch64 cross compiler.
- **FR-003**: The `check-aarch64` job MUST run `just check-aarch64`, install the aarch64 cross compiler, use managed setup, and use an explicit independent rust-cache key.
- **FR-004**: The `test` job MUST use `strategy.matrix.shard: [1, 2, 3, 4]`.
- **FR-005**: The `test` matrix MUST run `just test -- --partition count:${{ matrix.shard }}/4`.
- **FR-006**: The managed `just test` path MUST accept passthrough arguments and still use the existing Rust verification owner.
- **FR-007**: The `test` matrix MUST emit an actionable shard label or log line containing the exact local reproduction command for each shard.
- **FR-008**: The `test` matrix MUST set `strategy.fail-fast: false` so every shard result is observable by the aggregate gate.
- **FR-009**: The aggregate `gate` MUST need `detector`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, `test`, and `build`.
- **FR-010**: The aggregate `gate` MUST accept only `success` for `clippy`, `check-aarch64`, `source-fence`, and aggregate `test`, while preserving the existing build-required skip semantics for `build`.
- **FR-011**: `test` MUST continue to depend on `source-fence`, and `gate` MUST continue to require `source-fence` if #342 filters are excluded from full nextest shards.
- **FR-012**: The branch MUST explicitly document whether source-fence filters are excluded from or intentionally duplicated by full nextest shards.
- **FR-013**: `just ci-lint-workflow` MUST fail with actionable output if the new `check-aarch64` job, its setup/cache key, gate need, or gate result check is missing.
- **FR-014**: `just ci-lint-workflow` MUST fail with actionable output if the test matrix shard list, `fail-fast: false`, partition command, shard-aware cache key, or reproduction log command is missing.
- **FR-015**: The implementation MUST document before/after critical path expectations using the #343 baseline and MUST update exact run IDs/job durations once final PR-head CI exists.
- **FR-016**: This branch MUST NOT implement #195 cache artifact persistence, #205 smoke-tag deduplication, #344 pass-stub/docs-only evidence, #340 config relocation, or generic #203 lint cleanup beyond #332-specific topology.

### Key Entities

- **CheckAarch64Job**: Top-level CI job that owns the aarch64 cross-check previously serialized inside `clippy`.
- **HostClippyJob**: Top-level CI job that owns host-only `just clippy`.
- **TestShardMatrix**: Four GitHub Actions matrix jobs that run full nextest partitions.
- **ShardReproductionCommand**: Log output that maps each CI shard to its local `just test -- --partition count:<shard>/4` command.
- **AggregateGate**: Single required CI signal that validates every required lane and the aggregate matrix result fail-closed.
- **SourceFenceOwnershipDecision**: Explicit #342/#332 decision to either exclude canonical source-fence filters from full nextest or intentionally duplicate them under the aggregate gate.

## Success Criteria

### Measurable Outcomes

- **SC-001**: `just ci-lint-workflow` fails before implementation when `check-aarch64` and test-shard invariants are absent, then passes after implementation.
- **SC-002**: `python3 scripts/test_verify_ci_workflow_hygiene.py` includes failing fixture coverage for missing `check-aarch64`, missing shard matrix, missing partition command, missing `fail-fast: false`, and missing shard reproduction command.
- **SC-003**: `just test -- --partition count:1/4` reaches the existing managed test path locally instead of invoking raw cargo directly from the workflow.
- **SC-004**: The final PR body records before/after timing evidence placeholders tied to #343 baseline until exact final-head CI run IDs are available, then updates them with exact job durations.
- **SC-005**: The PR leaves #195, #205, #344, and #340 untouched except for explicit coordination notes where #332 affects their future work.

## Assumptions

- PR #346/#342 and PR #347/#203 are the stacked bases for this work, so `source-fence` and the standard-library workflow verifier already exist.
- The repo keeps one managed Rust execution path through `rust_verification.py`; workflow YAML must continue to call `just` recipes, not raw cargo.
- Exact-head CI and external reviews are deferred until the current stacked PR head can get a real CI run.
