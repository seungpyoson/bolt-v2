# Implementation Plan: CI Slow Test Post-Stop Delay

**Branch**: `codex/ci-357-delay-post-stop` | **Date**: 2026-05-17 | **Spec**: `specs/011-ci-slow-test-post-stop/spec.md`
**Input**: Feature specification from `specs/011-ci-slow-test-post-stop/spec.md`

## Summary

Remove NT's default 10s post-stop wait from test-local `LiveNode` builders in the #357 slow-test cluster by centralizing plain test node construction in `tests/support/mod.rs`.

## Technical Context

**Language/Version**: Rust with NautilusTrader Rust API
**Primary Dependencies**: `nautilus_live::node::LiveNode`, existing integration tests
**Storage**: N/A
**Testing**: `just test --test ...`, `just fmt-check`, exact-head PR CI
**Target Platform**: GitHub Actions and local macOS/Linux Rust test runners
**Project Type**: Rust integration-test performance slice
**Performance Goals**: Remove about 10s per affected LiveNode start/stop test without moving tests out of PR CI
**Constraints**: No production runtime config change; no skipped tests; no broad gate policy change
**Scale/Scope**: Three integration test files plus shared test support

## Constitution Check

PASS. This is test-only, preserves NT ownership, keeps one PR per issue, does not alter production config or live trading behavior, and uses TDD evidence.

## Project Structure

```text
tests/support/mod.rs
tests/venue_contract.rs
tests/nt_runtime_capture.rs
tests/lake_batch.rs
docs/ci/slow-test-post-stop-evidence-2026-05-17.md
specs/011-ci-slow-test-post-stop/
```

**Structure Decision**: Use existing integration test support instead of duplicating builder flags at each test site.

## Complexity Tracking

No constitution violations.
