# Feature Specification: CI Slow Test Post-Stop Delay

**Feature Branch**: `codex/ci-357-delay-post-stop`
**Created**: 2026-05-17
**Status**: Draft
**Input**: GitHub issue #357 and latest epic #333 working-set update.

## User Scenarios & Testing

### User Story 1 - LiveNode Capture Tests Avoid Synthetic Post-Stop Waits (Priority: P1)

As the maintainer, I can run the slow NT runtime capture, venue contract, and lake batch tests without paying NautilusTrader's default 10s post-stop delay when those tests do not assert residual-event draining behavior.

**Independent Test**: A representative test changes from the pre-change baseline `finished in 10.04s` with `Awaiting residual events (10s)` to `finished in 0.04s` with `Awaiting residual events (0ns)`.

### User Story 2 - Slow-Test Ownership Remains Explicit (Priority: P1)

As the maintainer, I can see that #357 does not move tests out of the PR merge gate, skip coverage, or reclassify gate policy without approval.

**Independent Test**: The changed files keep the same test binaries in place and only centralize test-local `LiveNode` construction through a zero post-stop-delay helper.

## Requirements

- **FR-001**: Tests in `tests/venue_contract.rs`, `tests/nt_runtime_capture.rs`, and `tests/lake_batch.rs` that construct plain `LiveNode` instances MUST use a test helper that sets `with_delay_post_stop_secs(0)`.
- **FR-002**: The change MUST NOT alter production runtime config defaults, TOML semantics, or live-node setup code.
- **FR-003**: The change MUST NOT skip, quarantine, ignore, or move any slow test out of PR CI.
- **FR-004**: The implementation MUST record before/after evidence for at least one representative test.
- **FR-005**: Any test that explicitly depends on post-stop residual draining MUST be left out of this helper or documented.

## Success Criteria

- **SC-001**: Targeted tests for `venue_contract`, `nt_runtime_capture`, and `lake_batch` pass.
- **SC-002**: `cargo fmt --check` or `just fmt-check` passes for the changed Rust tests.
- **SC-003**: Evidence names the before/after command, runtime, and NT log line proving the delay was removed.
- **SC-004**: The PR body states #357 scope only and does not claim #333 closure.

## Assumptions

- Latest #333 comment is authoritative for this slice: #357 is a config-flag fix after investigation found 31 tests clustered around NT's 10s default post-stop delay.
- Current target files do not assert NT post-stop residual drain timing.
