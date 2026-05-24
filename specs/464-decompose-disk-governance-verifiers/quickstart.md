# Quickstart: #464 Cargo Scanner Helper Decomposition

## Baseline

Run from the #464 worktree:

```bash
python3 scripts/test_command_understanding.py
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
```

Expected baseline:

```text
OK: command understanding self-tests passed.
OK: Rust verification cache retention self-tests passed.
OK: CI workflow hygiene verifier self-tests passed.
```

## Planning Gate

Before implementation:

Run the repo unresolved-marker scan over `specs/464-decompose-disk-governance-verifiers/`, then run:

```bash
git diff --check
```

Expected: no unresolved marker matches and no whitespace errors.

Then request adversarial planning review from Claude, Gemini, Grok, GLM, DeepSeek, and Kimi. Implementation starts only after all available reviewers approve or the operator explicitly waives a failed/skipped slot.

## TDD Implementation Gate

First edit only:

```bash
python3 scripts/test_command_understanding.py
```

Expected after RED test edit and before shared helper implementation:

```text
AssertionError
```

The failure must be caused by missing shared cargo scanner exports or an unrewired client, not by syntax/import errors.

## Green Verification

After mechanical extraction:

```bash
python3 scripts/test_command_understanding.py
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py
git diff --check
just ci-lint-workflow
```

Expected:

```text
Command-understanding, runtime verifier, static workflow verifier, py_compile, whitespace, and ci-lint-workflow checks pass.
```

## Completion Gate

Before merge readiness:

- Push the #464 branch.
- Open PR against `main`.
- Confirm exact-head GitHub CI is green.
- Request exact-head external implementation review from Claude, Gemini, Grok, GLM, DeepSeek, and Kimi.
- Record approvals and skipped/failed slots in `evidence.md` and the PR body.
- Stop for explicit operator merge approval.
