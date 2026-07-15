# Pack Artifact Integrity Design

## Scope

Issue #437 slice: make the existing source-universe batch consumer verify every control artifact already pinned by an execution pack before it fetches source data or invokes an operator.

This gate is source and venue neutral. Binance, Bybit, and all other configured venues use the same path. New sources, non-trade dispatch, S3 publication, AWS changes, and coverage-registry work are out of scope.

## Required behavior

For every record in an executable pack, the consumer resolves and reads:

- `run_spec_path`, matching `run_spec_sha256`
- `accepted_tranche_path`, matching `accepted_tranche_sha256`
- `execution_plan_path`, matching `execution_plan_sha256`

The preflight runs for the complete pack before resume processing, cache access, source fetches, worker creation, or operator execution. Expected hashes must be lowercase 64-character SHA-256 values. A missing file, invalid expected hash, or byte mismatch rejects the batch. Errors identify the record sequence, operator run ID, artifact role, path, and available expected/actual hash evidence.

Valid packs preserve existing execution, cache, resume, ordering, and reporting behavior. No schema or configuration changes are required because the pack already records all six fields.

## Evidence

- A valid pack executes normally.
- Tampering each of the three artifact roles fails before fetch or runner invocation.
- A missing pinned artifact fails before fetch or runner invocation.
- Existing batch integration tests remain green.

This temporary design file is implementation scaffolding and will be removed before the PR is presented, per repository owner direction.
