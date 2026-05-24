# Evidence Map: #464 Cargo Scanner Helper Decomposition

## Current State

| Evidence | Result |
|---|---|
| Issue #464 | Open: continue disk-governance verifier decomposition after PR #461. |
| Issue #454 | Closed as completed. |
| PR #461 | Merged into `main` as `817ddfc9af8cd835ee6143f0562595f73a1d2645`. |
| Worktree | `.worktrees/464-verifier-decomposition` |
| Branch | `codex/464-verifier-decomposition` tracking `origin/main` |
| Branch head | `817ddfc9af8cd835ee6143f0562595f73a1d2645` |
| `.worktrees` gitignore status | Ignored. |

## Baseline Verification

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` | Pass: `OK: command understanding self-tests passed.` |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |

## Planning Validation

| Command | Result |
|---|---|
| Unresolved-marker scan over `specs/464-decompose-disk-governance-verifiers/` | Pass: no matches. |
| `git diff --check` | Pass: no output. |
| Task checklist format scan for missing IDs or malformed boxes | Pass: no output. |
| Task checklist path-reference scan | Pass: no output. |

## Candidate Helper Evidence

| Helper family | Runtime verifier | Static workflow verifier | Current classification |
|---|---:|---:|---|
| `command_tokens` | `scripts/rust_verification.py:520` | `scripts/verify_ci_workflow_hygiene.py:1230` | Divergent but characterizable. Remains local. |
| `shell_command_substitution_payloads` | `scripts/rust_verification.py:635` | `scripts/verify_ci_workflow_hygiene.py:1326` | Divergent but characterizable. Remains local. |
| `shell_command_substitution_at` | `scripts/rust_verification.py:668` | `scripts/verify_ci_workflow_hygiene.py:2271` | Divergent but characterizable. Remains local. |
| Python AST command helpers | Shared imports | Shared imports | Already extracted by PR #461. |
| Renamed cargo/rustc helpers | `scripts/rust_verification.py:1557` and adjacent | `scripts/verify_ci_workflow_hygiene.py:2537` and adjacent | Divergent filesystem boundary. Remains local. |
| Wrapper handling | `scripts/rust_verification.py:1438` and adjacent | `scripts/verify_ci_workflow_hygiene.py:2001` and adjacent | Insufficient equivalence evidence. Remains local. |
| Cargo subcommand scanning | `scripts/rust_verification.py:2150`, `:2176`, `:2187`, `:2266` | `scripts/verify_ci_workflow_hygiene.py:1700`, `:1726`, `:1733`, `:1752` | Selected slice. Pure helper family with static start-offset support. |
| Full target-routing override detection | `scripts/rust_verification.py:2288` | `scripts/verify_ci_workflow_hygiene.py:1794` | Explicit non-goal. Policy and return shape differ. |
| Oversized verifier/test file structure | Large files under `scripts/` | Large files under `scripts/` | Explicit non-goal for this slice. |
| Test-only `sys.path` setup hygiene | `scripts/test_command_understanding.py:22` | Test import setup | Explicit non-goal for this slice. |

## Chosen Slice

Extract only the cargo scanner helper family into `scripts/command_understanding.py`:

- `cargo_subcommand_with_index`
- `cargo_subcommand`
- `nextest_subcommand_with_index`
- `cargo_args_for_target_routing_scan`

The shared module remains pure scanner logic. Runtime/static policy wrappers remain local.

## Pre-Implementation Review Status

| Reviewer | Status | Evidence |
|---|---|---|
| Claude | Ready, review not run yet | Doctor ready with subscription OAuth. |
| Gemini | Ready, review not run yet | Doctor ready with subscription OAuth. |
| Kimi | Ready, review not run yet | Doctor ready with subscription OAuth. |
| Grok | Ready, review not run yet | Doctor ready with subscription CLI. |
| DeepSeek | Ready, review not run yet | Doctor ready with direct API HTTP 200. |
| GLM | Ready, review not run yet | Doctor ready with direct API HTTP 200. |

Implementation is blocked until the planning review gate records approvals or operator-waived skipped/failed slots.

## Residual #464 Scope After This Slice

- Command tokenization unification remains unselected because current behavior differs.
- Shell substitution unification remains unselected because input normalization differs.
- Renamed executable detection remains unselected because runtime resolves symlinks and static does not.
- Wrapper handling remains unselected because helper interfaces differ.
- Full target-routing policy unification remains unselected because runtime and static clients have different inputs and return shapes.
- Oversized verifier/test file splitting remains unselected because this slice is focused on drift-risk reduction through a proven helper family.
- Test-only `sys.path` setup cleanup remains unselected because it is separate test hygiene.

## Verification Plan

| Gate | Command |
|---|---|
| Focused characterization | `python3 scripts/test_command_understanding.py` |
| Runtime verifier suite | `python3 scripts/test_rust_verification_cache_retention.py` |
| Static verifier suite | `python3 scripts/test_verify_ci_workflow_hygiene.py` |
| Python syntax | `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py` |
| Whitespace | `git diff --check` |
| CI workflow verifier path | `just ci-lint-workflow` |
| Remote gate | Exact-head GitHub CI green |
| External gate | Exact-head implementation review from Claude, Gemini, Grok, GLM, DeepSeek, and Kimi |
