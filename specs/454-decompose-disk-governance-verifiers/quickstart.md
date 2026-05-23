# Quickstart: Decompose Disk-Governance Verifiers

## Starting State

```bash
git status --short --branch
git rev-parse HEAD
gh issue view 375 --repo seungpyoson/bolt-v2 --json state,stateReason,closedAt
gh issue view 454 --repo seungpyoson/bolt-v2 --json state,title,url
```

Expected:

- Branch is `codex/454-decompose-disk-governance-verifiers`.
- Head is based on merge commit `f354efbaa2afc78575e9cc40580cf2b682bd66e6`.
- #375 is closed as completed.
- #454 is open.

## Baseline Verification

```bash
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
```

Both commands passed before planning on the fresh #454 branch.

## Planning Artifact Checks

```bash
marker_pattern='NEEDS'' CLARIFICATION|\[FEA''TURE|\[#''##|ACTION'' REQUIRED|TO''DO|fix'' later'
rg -n "$marker_pattern" specs/454-decompose-disk-governance-verifiers/spec.md specs/454-decompose-disk-governance-verifiers/plan.md specs/454-decompose-disk-governance-verifiers/research.md specs/454-decompose-disk-governance-verifiers/data-model.md specs/454-decompose-disk-governance-verifiers/evidence.md specs/454-decompose-disk-governance-verifiers/tasks.md specs/454-decompose-disk-governance-verifiers/contracts
git diff --check
```

Expected: no unresolved template markers or whitespace errors.

## Implementation Verification

Run after each RED/GREEN slice:

```bash
python3 scripts/test_command_understanding.py
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py
git diff --check
```

## PR Verification

Before review-ready handoff:

```bash
git status --short --branch
pr_number=$(gh pr view --json number --jq .number)
gh pr checks "$pr_number" --repo seungpyoson/bolt-v2
```

Then request external exact-head review. no-mistakes is intentionally not run unless the operator explicitly requests it.
