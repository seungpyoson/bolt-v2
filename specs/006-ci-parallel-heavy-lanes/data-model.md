# CI Parallel Heavy Lanes Data Model

## CheckAarch64Job

- **Represents**: A top-level CI job that runs the aarch64 cross-check independently from host clippy.
- **Required attributes**:
  - Job id and name: `check-aarch64`
  - Needs: `detector`
  - Setup action with managed target-dir opt-in
  - aarch64 cross compiler install step
  - Rust cache using managed target dir
  - Explicit cache key independent from host clippy
  - Command: `just check-aarch64`
- **Validation**: Workflow verifier fails if the job, setup/cache, gate need, or gate result check is absent.

## HostClippyJob

- **Represents**: A host-only top-level CI job for `just clippy`.
- **Required attributes**:
  - Job id and name: `clippy`
  - Needs: `detector`
  - Setup action with clippy component and managed target-dir opt-in
  - Rust cache using managed target dir
  - Explicit host-only cache key
  - Command: `just clippy`
  - No aarch64 cross compiler install step
- **Validation**: Workflow verifier and lint fail if clippy still owns `just check-aarch64` or installs the aarch64 cross compiler.

## TestShardMatrix

- **Represents**: The `test` job expanded into four deterministic nextest partitions.
- **Required attributes**:
  - Job id: `test`
  - Needs: `[detector, source-fence]`
  - `strategy.fail-fast: false`
  - `strategy.matrix.shard: [1, 2, 3, 4]`
  - Shared nextest cache key is bounded and saved only by shard 1
  - Command: `just test -- --partition count:${{ matrix.shard }}/4`
  - Reproduction log includes the same local command shape
- **Validation**: Workflow verifier fails on missing shard values, missing fail-fast false, missing partition command, missing shared nextest cache key, missing shard-1 cache save guard, or missing reproduction log.

## ShardReproductionCommand

- **Represents**: Actionable CI log evidence for rerunning one shard locally.
- **Required attributes**:
  - Includes `matrix.shard`
  - Includes total shard count `4`
  - Includes `just test -- --partition count:<shard>/4`
- **Validation**: Linter accepts a command template that resolves from `${{ matrix.shard }}` and fails if the test job has no reproduction command.

## AggregateGate

- **Represents**: The single required job that fails closed for required CI lanes.
- **Required attributes**:
  - Needs: `detector`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, `test`, `build`
  - `if: ${{ always() }}`
  - Result checks accepting only success for detector, fmt-check, deny, clippy, check-aarch64, source-fence, and aggregate test
  - Existing build-required handling: build must succeed when required; skipped is allowed only when detector says build is not required
- **Validation**: Verifier and justfile lint fail on missing need or result check.

## SourceFenceOwnershipDecision

- **Represents**: Explicit #342/#332 ownership of canonical source-fence filters.
- **Required attributes**:
  - Decision: full nextest intentionally duplicates #342 filters in this slice
  - `test` remains dependent on `source-fence`
  - `gate` requires both `source-fence` and aggregate `test`
- **Validation**: Spec, workflow comment, and PR body must state this decision so duplicate execution is intentional and reviewable.

## BeforeAfterTimingEvidence

- **Represents**: Required evidence for #332 impact.
- **Required attributes**:
  - Baseline reference: #343 run `25855655415` and `docs/ci/ci-baseline-2026-05-15.md`
  - Final PR-head run id, SHA, event type, timestamps, and job durations when available
  - Critical path before/after comparison
- **Validation**: PR body must not claim exact after timing until a real final-head CI run exists.
