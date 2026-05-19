# Feature Specification: Managed Rust Cache Retention

**Feature Branch**: `codex/286-managed-rust-cache-retention`
**Created**: 2026-05-19
**Status**: Draft
**Issue**: `seungpyoson/bolt-v2#286`
**Input**: After PR #398 moved Rust verification into `scripts/rust_verification.py`, implement the managed cache status/retention path without touching runtime/trading Rust code.

## Current Evidence

- `bolt-v2 origin/main`: `eed95487f742ca0aacbcf80fb80c1dbafbb9b4e4` (`#398` merged).
- `claude-config origin/main`: `c09a63edbeb91ddde13543019788f742a0bfe050` (`#794` merged).
- `df -h ~`: `460Gi` size, `415Gi` used, `13Gi` available, `97%` capacity.
- Managed target root: `/Users/spson/.cache/rust-verification/bolt-v2/target` = `60G`.
- Managed debug profile: `target/debug` = `56G`.
- Managed release profile: `target/release` = `335M`.
- Managed cross target: `target/aarch64-unknown-linux-gnu` = `3.1G`.
- Implemented `cache-status --json` now reports `du -sk` allocated bytes and policy pressure: managed target `70.3GB`, debug `66.6GB`, cross target `3.3GB`, release `351MB`, free filesystem bytes `8.92GB`, pressure reasons `cache exceeds soft_limit_bytes` and `filesystem free below min_free_bytes`.
- Implemented `cache-prune --dry-run --json` currently finds only managed-root `tmp` as stale and reclaimable bytes `0`; hot debug/release/cross-target cache is preserved by age policy even while pressure is true.
- Repo-local bypass target: `/Users/spson/Projects/Claude/bolt-v2/target` = `27G`.
- `/private/tmp/bolt-v2-*` contains multiple large old target bundles: examples include `13G`, `10G`, `9.4G`, `9.3G`, `9.2G`.
- `#123` is the parent bolt-v2 disk-pressure epic. `#286` is the implementation issue for managed Rust cache retention. Final detailed evidence belongs on `#286`; `#123` gets a summary and link.

## Scope

This feature implements the `#286` managed-cache policy surface in `scripts/rust_verification.py`.

In scope:

- Report managed Rust cache size by subtree.
- Report free disk.
- Classify hot cache versus prune candidates.
- Define retention thresholds in `ci/rust-verification.toml`.
- Add pressure-gated, dry-run-first pruning for clearly rebuildable managed target subtrees.
- Refuse prune while active Rust verification/build processes appear to be using the cache.
- Preserve useful hot cache by default.

Out of scope:

- `#374` shell-agnostic cargo shim and worktree/tmp lifecycle sweep.
- Deleting repo-local `target/` directories or `/private/tmp/bolt-v2-*` bundles.
- no-mistakes daemon/review-loop behavior.
- Runtime/trading/source Cargo changes.
- S3 as active Cargo target storage.

## User Scenarios & Testing

### User Story 1 - Inspect Managed Cache (Priority: P1)

As the operator, I can run the repo-local Rust verifier and see the managed cache size, free disk, and profile/target breakdown before a heavy Rust run.

**Independent Test**: Python fixture creates fake target subtrees with files and asserts `cache-status --json` reports total bytes, subtree bytes, path, and free-disk fields.

### User Story 2 - Dry-Run Prune Candidates (Priority: P1)

As the operator, I can ask what would be pruned without deleting anything.

**Independent Test**: Python fixture creates stale fake target subtrees and asserts `cache-prune --dry-run --json` lists candidates and byte totals but leaves files present.

### User Story 3 - Refuse Unsafe Prune (Priority: P1)

As the operator, I cannot accidentally prune cache while a relevant Rust process is active.

**Independent Test**: Python fixture injects a fake active-process detector and asserts apply mode exits non-zero with no deletion.

### User Story 4 - Apply Conservative Prune (Priority: P2)

As the operator, I can explicitly apply a conservative prune after reviewing dry-run output.

**Independent Test**: Python fixture creates stale fake subtrees, runs apply mode, asserts only candidate paths are removed and a second dry-run is idempotent.

## Requirements

- **FR-001**: `scripts/rust_verification.py cache-status --repo <repo> --json` MUST report managed target root, total bytes, per-subtree bytes, latest mtime, policy path, and filesystem free bytes.
- **FR-002**: `cache-status` MUST NOT require running Cargo.
- **FR-003**: `cache-prune` MUST default to dry-run behavior unless an explicit apply flag is passed.
- **FR-004**: `cache-prune` MUST refuse apply when policy-configured active process patterns appear related to the managed cache/repo.
- **FR-005**: Prune policy MUST be documented by profile/target class: `debug`, `release`, `cross-target`, managed-root `tmp`, and `other`.
- **FR-006**: Prune policy MUST preserve useful hot cache by default and MUST NOT delete the whole managed target root in any mode.
- **FR-007**: Prune output MUST show estimated bytes reclaimable before deletion.
- **FR-008**: All cache status/prune tests MUST be Python-only and MUST NOT run local Cargo.
- **FR-009**: The implementation MUST NOT reintroduce `.claude/rust-verification.toml` or global `~/.claude/lib/rust_verification.py` ownership.
- **FR-010**: `#123` MUST be updated after implementation with measured before/after policy evidence.
- **FR-011**: Retention thresholds such as soft limit, minimum free disk, prune age, and prunable classes MUST come from `ci/rust-verification.toml` or explicit CLI flags, not hidden code constants.
- **FR-011A**: `cache-prune` MUST list/remove stale candidates only when managed target bytes exceed `soft_limit_bytes` or filesystem free bytes fall below `min_free_bytes`.
- **FR-012**: If retention config is missing or malformed, prune mode MUST fail closed; status may still report diagnostics that do not require prune policy.
- **FR-013**: `tmp` means only `<managed-target-root>/tmp`; `/private/tmp/bolt-v2-*` is `#374` scope and MUST NOT be pruned by this feature.
- **FR-014**: `other` class subtrees MUST be preserved unless policy explicitly marks `other` prunable.
- **FR-015**: Final before/after evidence MUST be partitioned: detailed command output on `#286`, parent summary/link on `#123`.
- **FR-016**: Active-process relatedness MUST require a configured process pattern plus repo/managed-target evidence from cwd or argv; insufficient process visibility MUST fail closed in apply mode.
- **FR-017**: `cross-target` MUST mean a direct managed target root child whose name has at least three non-empty hyphen-separated components, matching normal Rust target triples.
- **FR-018**: The scanner MUST report allocated disk bytes compatible with `du -sk`, MUST use `lstat` metadata without following symlinks, and MUST report skipped special entries.

## Success Criteria

- **SC-001**: `python3 scripts/test_rust_verification_cache_retention.py` passes without invoking real Cargo.
- **SC-002**: Existing PR #398 verification remains green: `scripts/test_rust_verification.py`, `scripts/test_rust_verification_decoupling.py`, CI workflow hygiene tests.
- **SC-003**: A dry-run command can explain current managed cache size and reclaim candidates on this machine.
- **SC-004**: No runtime/trading Rust files, `Cargo.toml`, or `Cargo.lock` are changed.
- **SC-005**: `ci/rust-verification.toml` is the only persistent policy file changed for retention thresholds.

## Assumptions

- Local Cargo remains necessary for dirty/unpushed/no-PR states; exact-head GitHub CI can replace local Cargo only when equivalent checks exist.
- S3 is not an active `target/` replacement because Cargo needs local filesystem semantics for incremental artifacts and locks.
- S3 or GitHub artifacts may later store logs/results/archive outputs, but that is not `#286`.
