# Evidence Map: #464 Cargo Scanner Helper Decomposition

## Current State

| Evidence | Result |
|---|---|
| Issue #464 | Open: continue disk-governance verifier decomposition after PR #461. |
| Issue #454 | Closed as completed. |
| PR #461 | Merged into `main` as `817ddfc9af8cd835ee6143f0562595f73a1d2645`. |
| Worktree | `.worktrees/464-verifier-decomposition` |
| Branch | `codex/464-verifier-decomposition` tracking `origin/main` |
| Branch base | `817ddfc9af8cd835ee6143f0562595f73a1d2645` |
| Planning review head | `1082acc40cfe12f0759aca951295046de198ca47` |
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
| Claude | APPROVE | Job `d41fc5eb-6ee7-407a-bea7-5404fa6fc04a`; no blocking findings. Non-blocking: verify line references before Phase 3, strengthen unknown-flag and runtime-only option tests, implementation review needs precise scope paths. |
| Gemini | APPROVE | Job `f76bb83d-8144-4776-b9d2-d7ca892b9734`; no blocking findings. Non-blocking: add explicit negative/unknown leading flag coverage. |
| Kimi | FAILED | Job `a01bd07a-f3cf-4be2-8875-fe490ab63efd`; `step_limit_exceeded` at 32 steps, no verdict. This is not approval. |
| Grok | APPROVE | Job `job_0cbe4ebc-cd48-4794-9915-e0b45484b621`; no blocking findings. Non-blocking: stale branch-head row, fixed by recording base and planning review head separately. |
| DeepSeek | APPROVE | Job `job_795182e6-4fff-452e-a905-b3f555357635`; no blocking findings. Non-blocking: prove option-set union or preserve client-local constants, stale branch-head row. |
| GLM | APPROVE | Job `job_f0bb0980-0e7a-4036-9c2c-25dd762b3849`; no blocking findings. Non-blocking: stale branch-head row, option-set fallback sentence, explicit guard around exported/non-exported helpers. |

Implementation remains blocked because Kimi failed without a verdict. The gate needs either a successful Kimi retry or explicit operator waiver before Phase 3 starts.

## Review Note Resolution

| Review note | Resolution |
|---|---|
| Evidence branch head was stale or ambiguous. | Replaced the single branch-head row with `Branch base` and `Planning review head`. |
| Option-set union needs proof or fallback. | Added explicit characterization tests for unknown leading dash tokens and runtime-only no-argument Cargo options; added fallback in `plan.md` to preserve client-local constants if parity fails. |
| Export/non-export guard should be explicit. | T017 keeps shared-export parity tests and existing non-export guard coverage in `scripts/test_command_understanding.py`; full target-routing and wrapper helpers remain non-exports. |

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
