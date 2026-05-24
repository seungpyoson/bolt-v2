# Research: #464 Disk-Governance Verifier Decomposition Follow-Up

## Current Authoritative State

| Evidence | Result |
|---|---|
| Issue #464 | Open: continue disk-governance verifier decomposition after PR #461. |
| Issue #454 | Closed as completed after PR #461. |
| PR #461 | Merged with merge commit `817ddfc9af8cd835ee6143f0562595f73a1d2645`. |
| Fresh branch/worktree | `codex/464-verifier-decomposition` at `origin/main` commit `817ddfc9af8cd835ee6143f0562595f73a1d2645`. |
| Active SpecKit pointer | `.specify/feature.json` still points at `specs/023-nt-order-intent-layer`; #464 follows the issue-local #454 artifact pattern. |

## Baseline Verification

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` | Pass: `OK: command understanding self-tests passed.` |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |

## Candidate Helper Family Classification

| Helper family | Runtime verifier | Static workflow verifier | Classification for this slice |
|---|---:|---:|---|
| `command_tokens` | `scripts/rust_verification.py:520` | `scripts/verify_ci_workflow_hygiene.py:1230` | Divergent but characterizable. Runtime uses simple `shlex.split`; static uses punctuation-aware lexing and line-boundary support. |
| `shell_command_substitution_payloads` | `scripts/rust_verification.py:635` | `scripts/verify_ci_workflow_hygiene.py:1326` | Divergent but characterizable. Runtime normalizes tokens first; static scans caller tokens. |
| `shell_command_substitution_at` | `scripts/rust_verification.py:668` | `scripts/verify_ci_workflow_hygiene.py:2271` | Divergent but characterizable. Runtime requires normalized exact `$`; static accepts prefix tokens ending in `$`. |
| Python AST command helpers | Shared import in both clients | Shared import in both clients | Already extracted by PR #461. Not part of this slice. |
| Renamed cargo/rustc path helpers | `scripts/rust_verification.py:1557` and adjacent | `scripts/verify_ci_workflow_hygiene.py:2537` and adjacent | Divergent but characterizable. Runtime resolves filesystem symlinks; static inspects raw path tokens. |
| Wrapper handling | `scripts/rust_verification.py:1438` and wrapper indices | `scripts/verify_ci_workflow_hygiene.py:2001` and inner-token helpers | Insufficient equivalence evidence. Similar purpose, different helper interface and policy context. |
| Cargo subcommand scanning | `scripts/rust_verification.py:2150`, `:2176`, `:2187`, `:2266` | `scripts/verify_ci_workflow_hygiene.py:1700`, `:1726`, `:1733`, `:1752` | Proven equivalent enough for this slice after characterization. Static keeps a `start` offset that can become an optional shared parameter. |
| Full target-routing override detection | `scripts/rust_verification.py:2288` | `scripts/verify_ci_workflow_hygiene.py:1794` | Explicit non-goal. Return shape and policy inputs differ. Only scan-boundary helper is selected. |
| Oversized verifier/test split | Large files listed in `evidence.md` | Large files listed in `evidence.md` | Insufficient evidence for this slice. No mechanical split selected. |
| Test-only `sys.path` setup hygiene | `scripts/test_command_understanding.py:22` and related import guards | Test helper import loading | Explicit non-goal for this slice. PR #461 already addressed import-order coupling; a broader setup-helper change needs separate evidence. |

## Decision: Cargo Scanner Helper Family

The selected slice is the cargo scanner helper family because:

- The helper bodies are near-identical across runtime and static verifier clients.
- Static's only extra requirement is a `start` argument on `cargo_subcommand_with_index`.
- Current PR #461 tests already characterize representative runtime/static cargo subcommand and target-routing scan behavior.
- The helper family is pure and does not read filesystem, environment, processes, Git state, network, credentials, or operator home paths.
- Full target-routing policy remains local, so the shared module does not take on runtime/static policy differences.

## Rejected Alternatives

- **Command tokenization**: rejected because current token boundary behavior intentionally differs.
- **Shell substitution parsing**: rejected because normalized-token and caller-token behavior differs.
- **Renamed executable detection**: rejected because runtime symlink resolution and static raw-token scanning differ.
- **Wrapper handling**: rejected because helper interfaces are different and extraction would require redesign.
- **Oversized file split**: rejected for this slice because a mechanical split does not directly reduce runtime/static drift.
- **Test `sys.path` helper**: rejected for this slice because it is test hygiene, not runtime/static verifier drift.

## Reviewer Availability

| Reviewer | Result |
|---|---|
| Claude | Ready: subscription OAuth, model `claude-opus-4-7`. |
| Gemini | Ready: subscription OAuth, model `gemini-3.1-pro-preview`. |
| Kimi | Ready: subscription OAuth, model `kimi-code/kimi-for-coding`. |
| Grok | Ready: subscription CLI, model `grok-build`. |
| DeepSeek | Ready: direct API, model `deepseek-v4-pro`, source-free probe HTTP 200. |
| GLM | Ready: direct API, model `glm-5.1`, source-free probe HTTP 200. |
