# Pack Artifact Integrity Implementation Plan

**Goal:** Reject any execution pack whose pinned run spec, accepted tranche, or execution plan is missing or has drifted, before any source fetch or operator work.

**Architecture:** Add one fail-closed preflight in `prepare_batch`. Reuse the existing pack-relative path resolver and SHA-256 implementation. Keep the batch runtime and pack schema otherwise unchanged.

## Task 1: Establish failing behavior tests

Files:

- Modify `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_source_universe_batch_execution.rs`

Steps:

1. Make normal pack fixtures create all three control files and record their real hashes.
2. Add tests for tampered run spec, accepted tranche, and execution plan.
3. Add a missing-artifact test.
4. Use `NeverFetcher` plus an empty runner call log to prove rejection precedes external work.
5. Commit and run the smallest Rust Probe integration-test target; confirm RED is caused by absent control-artifact verification.

## Task 2: Implement the consume-boundary preflight

Files:

- Modify `crates/backtesting-vertical-slice/src/source_universe_batch_execution.rs`

Steps:

1. Validate each expected artifact digest as lowercase SHA-256.
2. Resolve each path relative to the execution pack, read it, hash its exact bytes, and require equality.
3. Invoke the check for every pack record before resume loading or worker planning.
4. Report record, role, path, expected digest, and actual digest where one exists.
5. Commit, push, and use the second Rust Probe run to confirm GREEN.

## Task 3: Verify and hand off a code/test-only PR

Steps:

1. Run formatting and non-compile repository gates, including `just source-fence-static`.
2. Conduct local adversarial review and resolve findings.
3. Delete this plan and its temporary design before the final diff.
4. Open a scoped draft PR under #437, naming non-trade dispatch, coverage registry, and S3 publication/read receipt as remaining work.
5. Follow repository exact-head and native-review requirements; do not merge without the required human approval.
