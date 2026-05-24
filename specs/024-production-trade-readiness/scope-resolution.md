# Scope Resolution

PR #478 is the single production trade-readiness PR. PR #479 remains the separate #466 verifier decomposition PR.

The following non-readiness verifier/decomposition/disk-governance paths were restored to `origin/main` content in the #478 worktree so they will not remain in the final PR #478 diff after commit:

- `ci/rust-verification.toml`
- `scripts/rust_verification.py`
- `scripts/test_command_understanding.py`
- `scripts/test_rust_verification_cache_retention.py`
- `specs/014-disk-pressure-governance/tasks.md`
- `specs/466-decompose-disk-governance-verifiers/evidence.md`
- `specs/466-decompose-disk-governance-verifiers/tasks.md`

The runtime-literal audit file remains in scope because the current readiness implementation diff still changes `src/bolt_v3_operator_artifacts.rs` and must keep runtime-literal verification evidence aligned with those code paths.
