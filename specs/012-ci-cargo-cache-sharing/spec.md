# Feature Specification: CI Cargo Cache Sharing

**Feature Branch**: `codex/ci-366-cache-sharing`
**Created**: 2026-05-17
**Status**: Draft
**Input**: GitHub issue #366 and latest epic #333/#250 decomposition.

## User Scenarios & Testing

### User Story 1 - Cargo Registry/Git Cache Is Shared Safely (Priority: P1)

As the maintainer, I can run CI jobs with one shared Cargo registry/git cache key instead of per-job registry/git payloads, reducing duplicate restore/save work without sharing target artifacts across incompatible jobs.

**Independent Test**: `python3 scripts/test_verify_ci_workflow_hygiene.py` rejects missing shared registry/git cache, target directories inside the shared rust-cache payload, cargo-bin caching, and multi-owner saves.

### User Story 2 - Managed Target Caches Stay Isolated (Priority: P1)

As the maintainer, I can retain target-dir reuse for compile-heavy jobs while keeping host clippy, source-fence, standalone aarch64 check, and aarch64 release build caches separated by job/target/profile.

**Independent Test**: The workflow verifier rejects missing managed target cache paths or cache keys that do not name the correct job/target/profile lane.

## Requirements

- **FR-001**: `deny`, `clippy`, `check-aarch64`, `source-fence`, `test-archive`, and `build` MUST use shared Cargo registry/git caching with `shared-key: cargo-registry-git-v1`.
- **FR-002**: Shared Cargo registry/git cache steps MUST set `cache-targets: false` and `cache-bin: false`.
- **FR-003**: Shared Cargo registry/git cache steps MUST NOT include `cache-directories`.
- **FR-004**: Shared Cargo registry/git cache saves MUST be single-owner: `test-archive` only. Tag reuse paths MUST NOT claim a shared cache save owner because tag CI reuses same-SHA main artifacts instead of running the build/test archive lanes.
- **FR-005**: Managed target dirs MUST use separate `actions/cache` keys for `clippy-host`, `check-aarch64-dev`, `source-fence-test`, and `build-aarch64-release`.
- **FR-006**: The verifier MUST fail closed if any cache invariant above is weakened.
- **FR-007**: The change MUST NOT weaken source-fence, test, build, clippy, deny, deploy, or aggregate gate requirements.
- **FR-008**: Before/after PR evidence MUST record CI run IDs, job IDs, cache key behavior, and cache restore/save durations where GitHub logs expose them.

## Success Criteria

- **SC-001**: `python3 scripts/test_verify_ci_workflow_hygiene.py`, `python3 scripts/verify_ci_workflow_hygiene.py`, and `just ci-lint-workflow` pass.
- **SC-002**: Exact-head PR CI is green.
- **SC-003**: Evidence records shared registry/git cache and isolated target cache keys from the exact PR head.
- **SC-004**: PR scope is #366 only and does not claim #250 or #333 closure.

## Assumptions

- Swatinem/rust-cache v2.9.1 `shared-key` replaces the automatic job-based key and `cache-targets:false` limits the rust-cache payload to Cargo registry/git plus optional bin unless `cache-directories` is set.
- `actions/cache` is already pinned in the workflow for nextest archive caching, so using it for managed target dirs does not introduce a new unpinned action.
