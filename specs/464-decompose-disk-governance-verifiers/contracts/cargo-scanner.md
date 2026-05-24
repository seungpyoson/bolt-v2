# Contract: Shared Cargo Scanner Helpers

## Purpose

Provide one repo-local Python interface for Cargo argument scanning helpers that are equivalent across runtime disk-governance verification and static workflow hygiene verification.

## Module

```text
scripts/command_understanding.py
```

## Exported Interface

```text
cargo_subcommand_with_index(tokens: list[str], start: int = 0) -> tuple[int, str] | None
cargo_subcommand(tokens: list[str]) -> str | None
nextest_subcommand_with_index(nextest_args: list[str]) -> tuple[int, str] | None
cargo_args_for_target_routing_scan(cargo_args: list[str]) -> list[str]
```

## Required Constants

The module owns the option sets used by the exported helpers:

```text
CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT
CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT
NEXTEST_GLOBAL_OPTIONS_WITH_ARGUMENT
```

The no-argument Cargo option set may include the union of currently recognized runtime/static no-argument options only when characterization proves no behavior change for both clients.

## Compatibility Rules

- Use only Python standard library types and functions.
- Do not read files.
- Do not inspect environment variables.
- Do not inspect processes.
- Do not inspect Git state.
- Do not perform network operations.
- Do not inspect credentials, secret names beyond existing command inputs, or operator home paths.
- Do not parse shell strings.
- Do not decide whether a target-routing override exists.
- Do not format verifier refusal payloads or static verifier error messages.

## Client Rules

Runtime verifier:

- Imports the shared cargo scanner helpers.
- Keeps `cargo_target_routing_override`, `target_routing_refusal_payload`, and managed-command policy local.
- Keeps Cargo alias and cache-lock policy local.

Static workflow verifier:

- Imports the shared cargo scanner helpers.
- Keeps `tokens_have_target_routing_override`, environment-prefix policy, workflow scanning, and raw-storage error messages local.
- Keeps command tokenization and line-boundary handling local.

## Required Characterization

Tests must prove:

- Shared `cargo_subcommand_with_index` matches runtime and static current behavior for default scans.
- Shared `cargo_subcommand_with_index(..., start=1)` preserves static offset behavior.
- Shared `nextest_subcommand_with_index` matches both clients.
- Shared `cargo_args_for_target_routing_scan` matches both clients for `bench`, `run`, `test`, and `nextest run` separator handling.
- Runtime `cargo_target_routing_override` and static `tokens_have_target_routing_override` still pass representative policy cases after the shared helper import.

## Non-Exported Candidates

These helpers remain non-exports in this slice:

| Candidate | Reason |
|---|---|
| `command_tokens` | Runtime/static tokenization differs. |
| `shell_command_substitution_payloads` | Runtime/static input normalization differs. |
| `shell_command_substitution_at` | Runtime/static prefix handling differs. |
| Renamed cargo/rustc path helpers | Runtime resolves filesystem symlinks; static inspects raw path tokens. |
| Wrapper handling helpers | Current helper interfaces and policy context differ. |
| `cargo_target_routing_override` | Runtime-specific return shape and refusal policy. |
| `tokens_have_target_routing_override` | Static-specific workflow and environment-prefix policy. |
