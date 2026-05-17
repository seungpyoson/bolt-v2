# Implementation Plan: CI Cargo Cache Sharing

**Branch**: `codex/ci-366-cache-sharing` | **Date**: 2026-05-17 | **Spec**: `specs/012-ci-cargo-cache-sharing/spec.md`
**Input**: Feature specification from `specs/012-ci-cargo-cache-sharing/spec.md`

## Summary

Split the previous per-job rust-cache payloads into one shared Cargo registry/git cache plus isolated managed target caches for compile-heavy jobs, guarded by the workflow hygiene verifier.

## Technical Context

**Language/Version**: GitHub Actions YAML; Python 3 standard library verifier
**Primary Dependencies**: `Swatinem/rust-cache@c193711...` v2.9.1; pinned `actions/cache@005785...` v4.3.0
**Storage**: GitHub Actions cache
**Testing**: Python verifier self-tests, workflow verifier, `just ci-lint-workflow`, exact-head PR CI
**Target Platform**: GitHub Actions ubuntu-latest
**Project Type**: CI workflow/verifier maintenance
**Performance Goals**: Reduce duplicate Cargo registry/git cache save/restore overhead and keep target caches isolated
**Constraints**: No unsafe target-dir mixing; no unpinned dependencies; no merge-gate weakening
**Scale/Scope**: One workflow plus verifier/test coverage for issue #366

## Constitution Check

PASS. This is CI-only, preserves single managed Rust command path, adds no runtime code or secrets, and keeps one PR mapped to one issue.

## Project Structure

```text
.github/workflows/ci.yml
scripts/verify_ci_workflow_hygiene.py
scripts/test_verify_ci_workflow_hygiene.py
docs/ci/cargo-cache-sharing-evidence-2026-05-17.md
specs/012-ci-cargo-cache-sharing/
```

**Structure Decision**: Keep cache policy in `ci.yml` and enforce with the existing standard-library workflow verifier.

## Complexity Tracking

No constitution violations.
