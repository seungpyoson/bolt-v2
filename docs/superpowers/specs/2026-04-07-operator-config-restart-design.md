# Operator Config Restart Design

## Status

Approved implementation baseline for the operator-config restart on top of `origin/main`.

This document replaces the incremental follow-up mindset from PR #13 with one coherent design for:

- generated-config materialization semantics
- honest library and binary boundaries
- single-source-of-truth verification
- truthful operator workflow documentation

## Core Decision

`bolt-v2` will expose one honest public materialization entrypoint for the operator-config lane.

The library will own:

- parsing `config/live.local.toml`
- rendering runtime TOML
- diffing against any existing output
- directory creation
- atomic replacement
- read-only enforcement
- materialization outcome reporting

The `render_live_config` binary will be a thin CLI wrapper around that library behavior.

`bolt_v2::live_config` will no longer be a public module boundary.

## Materialization Contract

The operator-config materializer has exactly four outcomes:

1. `Created`
2. `Updated`
3. `PermissionsRepaired`
4. `Unchanged`

### `Created`

- The output file does not exist.
- The materializer renders runtime TOML.
- It creates any missing parent directories.
- It writes the contents to a sibling temporary file.
- It marks that temporary file read-only.
- It atomically renames the temporary file into place.

### `Updated`

- The output file exists.
- The rendered contents differ from the existing file contents.
- The materializer writes the new contents to a sibling temporary file.
- It marks that temporary file read-only.
- It atomically renames the temporary file over the target.

Important:

- The materializer never attempts in-place writes to the existing output file.
- This avoids collisions with the intentional read-only state of generated artifacts.

### `PermissionsRepaired`

- The output file exists.
- The rendered contents exactly match the existing file contents.
- The output permissions have drifted writable.
- The materializer does not rewrite contents.
- It restores the output to read-only state and reports a distinct repair outcome.

### `Unchanged`

- The output file exists.
- The rendered contents exactly match the existing file contents.
- The output permissions already match the read-only contract.
- The materializer performs no filesystem mutation.

### Read-Only Definition

Read-only is defined in user-visible, portable terms:

- on Unix: the output has no write bits set
- on non-Unix targets: the readonly flag is set

## API Boundary

The public API for the operator-config lane will be one side-effecting function whose name matches its behavior.

Expected shape:

```rust
pub fn materialize_live_config(
    input_path: &Path,
    output_path: &Path,
) -> Result<MaterializationOutcome, Box<dyn std::error::Error>>
```

`MaterializationOutcome` will carry only the state transition, not the rendered contents.

The library keeps the operator-schema structs and pure render helpers internal.

This design intentionally avoids:

- public `bolt_v2::live_config`
- `include!()`
- `#[path = ...]`
- duplicate-source compilation tricks
- a public function whose name implies file effects but only returns a string

## Rendered Artifact Header

The generated runtime artifact will carry provenance, not workflow instructions.

It may state:

- that the file is generated
- which source file it came from

It will not embed operator workflow commands such as `cargo run --bin ...` or `just ...`.

Reason:

- workflow truth belongs in the `justfile`, docs, and runbooks
- embedding workflow commands in generated output creates a second documentation surface that drifts

## Single Source of Truth

The tracked operator template is:

- `config/live.local.example.toml`

The operator workflow source of truth is:

- `config/live.local.toml` for local operator edits
- `justfile` for the operational commands

The generated runtime artifact is:

- `config/live.toml`

`config/examples/polymarket-exec-tester.toml` is not allowed to remain a second verification truth for the operator lane.

If it has no distinct operational purpose after the seam tests move to the tracked template, it should be removed.

## Test Strategy

Testing is split into three explicit responsibilities.

### 1. Pure Render and Mapping Tests

These will be unit tests inside the live-config implementation module so they can validate internal mapping behavior without expanding the public API.

They must exercise the tracked template and verify:

- data client kind and name
- exec client kind and name
- strategy kind
- `client_name` triple-duty invariant:
  - data client name
  - exec client name
  - strategy `client_id`
- `event_slug` to `event_slugs`
- timeout threading
- `instrument_id`
- `signature_type`
- `funder`
- secret-path threading

There must also be one defaults-focused render test proving that minimal required operator input plus defaults still renders a valid runtime config.

### 2. Materialization Behavior Tests

These will be integration tests using temp directories, not repo-state paths.

They must cover:

- relative output path
- nested output path
- create
- update
- unchanged with no rewrite
- permission-only repair

These tests own the full four-state contract.

### 3. Runtime Seam Test

This will render from the tracked template into a temp runtime file, parse that output through the real `Config`, and then construct the real runtime seam strongly enough to catch wiring regressions.

It must cover:

- `Config` parsing
- data-client factory translation
- exec-client factory translation
- strategy translation
- `LiveNode` builder registration path

It remains an offline seam test and does not become a live exchange integration test.

## Operator Workflow Truth

The operator workflow must be described consistently everywhere.

- `config/live.local.toml` is the human-edited local source of truth.
- `config/live.local.example.toml` is the tracked template.
- `config/live.toml` is the generated runtime artifact.
- `just live-check` means generate first, then validate secret configuration completeness only.
- `just live-resolve` means generate first, then perform actual secret resolution.
- `just live` means generate first, then run with the generated config.

Future-work docs must also state clearly that sections unsupported by the operator schema do not survive into the generated artifact until the schema explicitly supports them.

## Non-Goals

- no speculative multi-venue redesign
- no second public render API unless a concrete consumer requires it
- no preservation of prior incremental patch structure for its own sake

## Success Criteria

The final diff should read as:

- one materialization contract
- one honest API boundary
- one single-source-of-truth test story
- one operator workflow story
