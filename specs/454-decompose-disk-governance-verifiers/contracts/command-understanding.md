# Contract: Shared Command Understanding

## Purpose

Provide one repo-local Python path for command/parser helpers that are proven equivalent across runtime disk-governance verification and static workflow/no-mistakes hygiene verification.

## Scope

In scope:

- Python inline command payload extraction and its AST support helpers, if the pre-extraction comparison confirms equivalent behavior.
- Any additional parser/scanner helper family only after source-body and representative-behavior comparison classifies it as equivalent.
- Surface-specific characterization fixtures for candidate helpers that are similar or analogous but divergent.

Out of scope:

- New shell grammar support.
- New recursive wrapper families.
- New regex edge cases.
- Policy-surface changes.
- Mechanical unification of divergent same-name helpers.
- no-mistakes execution or policy changes.
- Runtime trading code.

## Proposed Module Contract

The shared module path is:

```text
scripts/command_understanding.py
```

Initial expected exported helper groups:

```text
python_constant_string(node: ast.AST) -> str | None
python_command_string(node: ast.AST) -> str | None
python_call_name(node: ast.AST) -> str
python_call_command_argument(node: ast.Call) -> ast.AST | None
python_inline_command_payloads(tokens: list[str]) -> list[str]
```

Additional helpers may be exported only when they are moved mechanically from existing accepted behavior, classified as equivalent, and covered by characterization/parity tests.

## Candidate Helpers Not In Initial Export Set

These helpers are characterization targets but not initial exports:

| Candidate | Reason it is not an initial export |
|---|---|
| `command_tokens` | Runtime and static tokenizers use different token splitting behavior. |
| `shell_command_substitution_payloads` | Runtime normalizes tokens before scanning; static scans caller tokens. |
| `shell_command_substitution_at` | Runtime requires normalized exact `$`; static accepts tokens ending in `$`. |
| `shell_normalized_tokens` | Runtime-only helper without a matching static helper. |
| `shell_command_segments` / `shell_command_segments_from_tokens` | Similar role but different expansion and return semantics. |
| renamed cargo/rustc path helpers | Runtime resolves filesystem symlinks for executable paths; static does not. |
| wrapper handling | Current surfaces do not expose the same helper contract. |
| target-routing override scan | Analogous policy behavior, not a full duplicate helper. |

## Compatibility Rules

- The shared module must use only the Python standard library.
- Initial shared exports must not read the filesystem.
- The shared module must not inspect processes, environment variables, Git state, network state, credentials, or operator home paths.
- Existing verifier clients may keep local policy decisions; the shared module owns only command-understanding primitives.
- Public helper behavior must be covered by focused characterization tests before clients are rewired.
- Divergent candidates must preserve each current surface behavior unless the PR separately documents, tests, and obtains operator approval for a semantic change.

## Required Clients

Every extracted helper family must be used by both:

- `scripts/rust_verification.py`
- `scripts/verify_ci_workflow_hygiene.py`

## Required Tests

The PR must include tests that prove:

- Representative current inputs keep the same classifications before and after extraction.
- Pre-extraction comparison classifies each candidate helper as equivalent, divergent, or deferred with evidence.
- Both verifier clients still pass their existing issue-named suites.
- A synthetic drift in a shared helper would fail at least one characterization/parity test.
