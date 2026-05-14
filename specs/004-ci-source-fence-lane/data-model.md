# Data Model: CI Source-Fence Lane

## SourceFenceJob

- **Fields**: job id, display name, needs, setup action wiring, rust-cache key, recipe command.
- **Validation**: job id must be `source-fence`; `needs` includes `detector`; command is `just source-fence`; no `cargo-nextest` install.

## SourceFenceRecipe

- **Fields**: verifier script sequence, targeted cargo test filters, managed Rust owner invocation.
- **Validation**: all six verifier scripts run before targeted cargo tests; targeted tests run through the managed Rust verification owner; full `just test` is not called.

## VerifierScriptSet

- **Fields**: script path, evidence purpose, dependency class.
- **Validation**: every script named by #342 exists, is executable by Python 3, and avoids unpinned non-standard Python packages.

## GateInvariant

- **Fields**: `gate.needs`, `needs.source-fence.result` check, `test.needs`, linter error messages.
- **Validation**: only `success` passes for `source-fence`; failed, cancelled, timed-out, skipped, missing, or stale same-workflow source-fence evidence cannot satisfy gate.

## TemporaryDuplicateOwnershipNote

- **Fields**: owning issue, current duplicate source, future resolution owner.
- **Validation**: states that #342 owns canonical early source-fence filters now, while #332 must later exclude them from full nextest shards or explicitly retain duplicate execution under the aggregate gate.
