# Implementation Plan: Managed Rust Cache Retention

**Branch**: `codex/286-managed-rust-cache-retention` | **Date**: 2026-05-19 | **Spec**: `specs/014-managed-rust-cache-retention/spec.md`

## Summary

Extend the repo-local `scripts/rust_verification.py` with managed-cache status and conservative prune commands for `bolt-v2#286`. Keep the change inside verifier/tooling. Do not touch runtime/trading Rust code, `Cargo.toml`, or `Cargo.lock`.

## Technical Context

**Language/Version**: Python 3 stdlib
**Primary Files**: `scripts/rust_verification.py`, new `scripts/test_rust_verification_cache_retention.py`
**Policy File**: `ci/rust-verification.toml`
**Storage**: Local filesystem under `~/.cache/rust-verification/bolt-v2/target` or `RUST_VERIFICATION_ROOT_BASE` override
**Testing**: Python tests with temp fake target trees; no real Cargo runs
**Target Platform**: macOS local agent and GitHub Actions Linux runners
**Constraints**: dry-run by default, refuse active-process apply, no global claude-config owner path, no `.claude/rust-verification.toml`

## Constitution / Repo Rule Check

- Scope discipline: one PR maps to `bolt-v2#286` only.
- Source of truth: current `origin/main` after #398 is authoritative.
- No dual paths: add commands to existing repo-local verifier, not new wrapper.
- No hardcodes: thresholds must come from `ci/rust-verification.toml` or explicit CLI flags, not hidden machine-specific constants.
- Verification: Python tests plus existing verifier decoupling checks.

## Current Diagnosis

The managed cache is still the largest confirmed Rust-specific consumer: `70.3GB`, with `66.6GB` in `debug` by implemented `cache-status --json` allocated-byte reporting. This is not safe to blindly delete because it is active hot cache, but the pre-change verifier only had a stub `cleanup` command returning `{"status":"ok","removed":[]}`. The missing product surface is status + pressure policy + dry-run prune.

Repo-local bypass target and `/private/tmp/bolt-v2-*` bundles are real disk consumers, but those belong to `#374`, not this PR.

`#123` is the parent bolt-v2 disk-pressure epic. This PR should close only the `#286` managed-cache retention slice, then leave detailed evidence on `#286` and a summary/link on `#123`.

## Implementation Phases

1. Add status data model and byte/mtime tree scanner.
2. Extend policy parsing for `[cache]` retention thresholds.
3. Add `cache-status` CLI with JSON output.
4. Add pressure-gated prune candidate classifier with dry-run output.
5. Add active-process refusal seam. Pattern match alone is not enough; apply mode should refuse when a configured process pattern is paired with repo/managed-target evidence in process cwd or argv, and should fail closed when process visibility is too limited to rule that out.
6. Add explicit apply mode.
7. Update `#123` / `#286` evidence after review and PR.

## Proposed Policy Shape

```toml
[cache]
min_free_bytes = 10737418240
soft_limit_bytes = 53687091200
active_process_patterns = ["cargo", "cargo-clippy", "cargo-fmt", "cargo-nextest", "rustc", "nextest", "rust_verification.py"]

[cache.retention.debug]
prune_after_days = 14
prunable = true

[cache.retention.release]
prune_after_days = 30
prunable = true

[cache.retention.cross-target]
prune_after_days = 30
prunable = true

[cache.retention.tmp]
prune_after_days = 1
prunable = true

[cache.retention.other]
prunable = false
```

These policy values are implemented in `ci/rust-verification.toml`. Tests use tiny temp values. Classification is constrained to the managed target root only. The `bolt-v2` path component comes from the existing `target_namespace` policy field in `ci/rust-verification.toml:3`, not a hidden path constant.

Pruning is gated by pressure before per-class age policy applies. Pressure is true when the managed target exceeds `soft_limit_bytes` or the containing filesystem has less than `min_free_bytes` available. If both thresholds are healthy, stale cache is preserved and `cache-prune --dry-run --json` returns no candidates.

Classification rules:

- `debug`, `release`, and `tmp` are direct children of the managed target root with those exact names.
- `cross-target` is a direct child whose name is Rust target-triple shaped: at least three non-empty hyphen-separated components, such as `aarch64-unknown-linux-gnu`.
- `other` is any remaining direct child and is preserved unless policy explicitly marks it prunable.

Scanner rules:

- Walk only inside the managed target root.
- Use `du -sk` per direct subtree for allocated disk bytes, because APFS clone/shared-block accounting can make summed `st_blocks` disagree with `du`.
- Use `lstat` metadata and do not follow symlinks.
- Skip sockets, FIFOs, devices, and other special entries and report a skipped count so macOS/Linux behavior is deterministic.

## Review Gates

- Do not implement before user approves this `#286` mapping.
- After TDD implementation, request Claude, Gemini, DeepSeek, GLM, and Kimi review.
- Run no-mistakes only as a final pre-PR check after local tests and exact issue scope are clean; do not implement no-mistakes daemon behavior in this PR.
- Open PR only after review gates pass. Do not merge.
