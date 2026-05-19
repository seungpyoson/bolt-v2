# Tasks: Managed Rust Cache Retention

**Input**: `specs/014-managed-rust-cache-retention/`
**Issue**: `bolt-v2#286`
**Tests**: TDD required. No real Cargo in tests.

## Phase 0: Planning Gate

- [x] T000 Confirm with user that this PR maps to `bolt-v2#286` only.
- [x] T001 Get adversarial plan review before implementation.

## Phase 1: Cache Status

- [x] T002 Add failing tests in `scripts/test_rust_verification_cache_retention.py` for fake target tree byte totals, per-subtree totals, latest mtime, and free disk fields.
- [x] T003 Add failing policy tests for `[cache]` retention thresholds in `ci/rust-verification.toml` and temp fixture policies.
- [x] T004 Implement policy parsing for retention thresholds, active-process patterns, and per-class `prunable` flags.
- [x] T005 Implement filesystem scanner and `cache-status --repo <repo> --json` in `scripts/rust_verification.py`.
- [x] T006 Run `python3 scripts/test_rust_verification_cache_retention.py`.

## Phase 2: Dry-Run Prune

- [x] T007 Add failing tests for dry-run prune candidates and byte totals with no deletion.
- [x] T008 Implement candidate classifier and `cache-prune --repo <repo> --dry-run --json`.
- [x] T009 Run cache retention tests.

## Phase 3: Safety Refusal

- [x] T010 Add failing tests for active-process refusal using an injected detector seam, including related cwd/argv matches and insufficient process visibility fail-closed behavior.
- [x] T011 Implement active-process detection/refusal for apply mode.
- [x] T012 Run cache retention tests.

## Phase 4: Explicit Apply

- [x] T013 Add failing tests for explicit apply removing only candidates and second dry-run idempotence.
- [x] T014 Implement explicit apply flag and deletion summary.
- [x] T015 Run cache retention tests.

## Phase 5: Regression / Scope Guard

- [x] T016 Run `python3 scripts/test_rust_verification.py`.
- [x] T017 Run `python3 scripts/test_rust_verification_decoupling.py`.
- [x] T018 Run `python3 scripts/test_verify_ci_workflow_hygiene.py`.
- [x] T019 Run `python3 scripts/verify_ci_workflow_hygiene.py`.
- [x] T020 Run `git diff --check`.
- [x] T021 Verify no runtime/trading Rust files, `Cargo.toml`, or `Cargo.lock` changed.
- [x] T021A Add/verify cross-platform scanner coverage for macOS/Linux free-disk, mtime, symlink, and special-file behavior.
- [x] T021B Address first external-review blockers: cross-target policy lookup, threshold-gated pruning, broken symlink handling, cwd process evidence, malformed-policy isolation, and conflicting prune modes.

## Phase 6: Review / PR

- [ ] T022 Request Claude, Gemini, DeepSeek, GLM, Kimi reviews.
- [ ] T023 Run no-mistakes as a final pre-PR check only; do not implement no-mistakes behavior.
- [ ] T024 Open PR mapped to `bolt-v2#286`. Do not merge.
- [ ] T025 Comment on `#123`/`#286` with final policy evidence.
