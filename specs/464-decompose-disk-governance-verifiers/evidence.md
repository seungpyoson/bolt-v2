# Evidence Map: #464 Cargo Scanner Helper Decomposition

## Current State

| Evidence | Result |
|---|---|
| Issue #464 | Open: continue disk-governance verifier decomposition after PR #461. |
| Issue #454 | Closed as completed. |
| PR #461 | Merged into `main` as `817ddfc9af8cd835ee6143f0562595f73a1d2645`. |
| PR #465 | Open: `https://github.com/seungpyoson/bolt-v2/pull/465`. |
| Worktree | `.worktrees/464-verifier-decomposition` |
| Branch | `codex/464-verifier-decomposition` tracking `origin/codex/464-verifier-decomposition` |
| Branch base | `817ddfc9af8cd835ee6143f0562595f73a1d2645` |
| Planning review head | `1082acc40cfe12f0759aca951295046de198ca47` |
| Implementation start head | `a4ddac81d0009ef63f15b8f51b2994996f969237` |
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
| `command_tokens` | `scripts/rust_verification.py:524` | `scripts/verify_ci_workflow_hygiene.py:1233` | Divergent but characterizable. Remains local. |
| `shell_command_substitution_payloads` | `scripts/rust_verification.py:639` | `scripts/verify_ci_workflow_hygiene.py:1329` | Divergent but characterizable. Remains local. |
| `shell_command_substitution_at` | `scripts/rust_verification.py:672` | `scripts/verify_ci_workflow_hygiene.py:2200` | Divergent but characterizable. Remains local. |
| Python AST command helpers | Shared imports | Shared imports | Already extracted by PR #461. |
| Renamed cargo/rustc helpers | `scripts/rust_verification.py:1561` and adjacent | `scripts/verify_ci_workflow_hygiene.py:2466` and adjacent | Divergent filesystem boundary. Remains local. |
| Wrapper handling | `scripts/rust_verification.py:1442` and adjacent | `scripts/verify_ci_workflow_hygiene.py:1930` and adjacent | Insufficient equivalence evidence. Remains local. |
| Cargo subcommand scanning | Shared `scripts/command_understanding.py:118`, `:144`, `:151`, `:170`; runtime import `scripts/rust_verification.py:27` | Shared `scripts/command_understanding.py:118`, `:144`, `:151`, `:170`; static import `scripts/verify_ci_workflow_hygiene.py:19` | Extracted slice. Pure helper family with static start-offset support. |
| Full target-routing override detection | `scripts/rust_verification.py:2193` | `scripts/verify_ci_workflow_hygiene.py:1723` | Explicit non-goal. Policy and return shape differ. |
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
| Kimi | FAILED; OPERATOR-WAIVED | Job `a01bd07a-f3cf-4be2-8875-fe490ab63efd`; `step_limit_exceeded` at 32 steps, no verdict. This is not approval. Operator message `skip kimi` on 2026-05-24 explicitly waived this failed planning slot for implementation start. |
| Grok | APPROVE | Job `job_0cbe4ebc-cd48-4794-9915-e0b45484b621`; no blocking findings. Non-blocking: stale branch-head row, fixed by recording base and planning review head separately. |
| DeepSeek | APPROVE | Job `job_795182e6-4fff-452e-a905-b3f555357635`; no blocking findings. Non-blocking: prove option-set union or preserve client-local constants, stale branch-head row. |
| GLM | APPROVE | Job `job_f0bb0980-0e7a-4036-9c2c-25dd762b3849`; no blocking findings. Non-blocking: stale branch-head row, option-set fallback sentence, explicit guard around exported/non-exported helpers. |

Implementation gate is open because the operator explicitly waived the failed Kimi planning slot on 2026-05-24. Kimi is still recorded as failed, not approved.

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

## Implementation Evidence

| Task | Result |
|---|---|
| T018 RED focused characterization | Fail as expected: `AssertionError: shared cargo scanner helpers missing: cargo_subcommand_with_index, cargo_subcommand, nextest_subcommand_with_index, cargo_args_for_target_routing_scan`. |
| T022 focused characterization | Pass: `OK: command understanding self-tests passed.` |
| T023 runtime verifier suite | Pass: `OK: Rust verification cache retention self-tests passed.` |
| T024 static verifier suite | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| Duplicate-definition scan | Pass: selected helper definitions exist only in `scripts/command_understanding.py`. |
| T027 Python syntax | Pass: `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py`. |
| T028 whitespace | Pass: `git diff --check` produced no output. |
| T029 CI workflow verifier path | Pass: `just ci-lint-workflow` completed with `OK: No raw cargo workflow commands or explicit Rust-wrapper bypasses found`. |
| T030 unresolved-marker scan | Pass: no unresolved-marker matches in specs or touched Python files. |
| T031 implementation commit | `0aa53b3f62e3902f3d9a403ce21f6af2a1fe2c36` (`refactor: share cargo scanner helpers`). |
| T033 pull request | PR #465 opened for issue #464 slice without claiming to close #464. |
