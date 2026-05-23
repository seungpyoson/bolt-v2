# Contract: Shared Command Understanding

## Purpose

Provide one repo-local Python path for command/parser helpers that are currently duplicated across runtime disk-governance verification and static workflow/no-mistakes hygiene verification.

## Scope

In scope:

- Shell command tokenization and normalization already accepted by current tests.
- Shell command substitution and command-boundary segmentation already accepted by current tests.
- Python inline command payload extraction already accepted by current tests.
- Renamed cargo/rustc executable detection already accepted by current tests.
- Cargo global-option and target-routing helper behavior already accepted by current tests, if extracted mechanically.

Out of scope:

- New shell grammar support.
- New recursive wrapper families.
- New regex edge cases.
- Policy-surface changes.
- no-mistakes execution or policy changes.
- Runtime trading code.

## Proposed Module Contract

The shared module path is:

```text
scripts/command_understanding.py
```

Expected exported helper groups:

```text
command_tokens(command: str) -> list[str]
shell_normalized_tokens(tokens: list[str]) -> list[str]
shell_command_substitution_payloads(tokens: list[str]) -> list[list[str]]
shell_command_substitution_at(tokens: list[str], index: int) -> tuple[list[str], int] | None
shell_command_segments(tokens: list[str]) -> list[list[str]]
python_inline_command_payloads(tokens: list[str]) -> list[str]
path_name_looks_like_renamed_cargo(name: str) -> bool
path_executable_looks_like_cargo(token: str) -> bool
path_name_looks_like_renamed_rustc(name: str) -> bool
path_executable_looks_like_rustc(token: str) -> bool
```

Additional helpers may be exported only when they are moved mechanically from existing accepted behavior and covered by characterization/parity tests.

## Compatibility Rules

- The shared module must use only the Python standard library.
- The shared module must not read the filesystem except for path-string parsing already done by current helpers.
- The shared module must not inspect processes, environment variables, Git state, network state, credentials, or operator home paths.
- Existing verifier clients may keep local policy decisions; the shared module owns only command-understanding primitives.
- Public helper behavior must be covered by focused characterization tests before clients are rewired.

## Required Clients

At least one extracted duplicated helper family must be used by both:

- `scripts/rust_verification.py`
- `scripts/verify_ci_workflow_hygiene.py`

## Required Tests

The PR must include tests that prove:

- Representative current inputs keep the same classifications before and after extraction.
- Both verifier clients still pass their existing issue-named suites.
- A synthetic drift in a shared helper would fail at least one characterization/parity test.
