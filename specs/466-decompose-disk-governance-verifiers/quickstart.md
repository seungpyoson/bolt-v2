# Quickstart: #466 Disk-Governance Verifier Decomposition

## Current State Checks

```bash
git fetch origin
git status --short --branch
git rev-parse origin/main
gh issue view 466 --repo REPO_OWNER/REPO_NAME --json number,state,title,body,comments,labels,url,closedAt
gh pr view 465 --repo REPO_OWNER/REPO_NAME --json number,state,mergedAt,title,body,headRefName,baseRefName,commits,files,url
```

## Ledger Checks

```bash
rg -n "Command tokenization|Shell command substitution|Renamed|Wrapper|Target-routing|Mechanical splitting|import setup|consume_cargo_global_options" specs/466-decompose-disk-governance-verifiers/evidence.md
rg -n "Open\\.|insufficient evidence|unreviewed|partial|tracked later|TBD" specs/466-decompose-disk-governance-verifiers/evidence.md
```

## Local Verification

Run the smallest relevant set for the touched slice, then broaden:

```bash
python3 scripts/test_command_understanding.py
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 -m scripts.test_command_understanding
python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py
git diff --check
just source-fence
just ci-lint-workflow
```

## Proof Boundaries

- Tests alone do not prove #466 completion; results must map back to ledger rows.
- A merged PR proves only the ledger rows it explicitly covers.
- External review is required before implementation and again before each PR-ready slice.
- Final issue closure requires whole-#466 review and explicit operator approval.
