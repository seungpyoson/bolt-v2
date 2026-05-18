# Quickstart: CI PR Run Concurrency

## Local Verification

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

## Runtime Evidence Collection

Find PR runs with superseded heads:

```bash
gh run list --workflow CI --event pull_request --limit 50 \
  --json databaseId,headBranch,headSha,status,conclusion,createdAt,updatedAt,displayTitle
```

For a target PR:

```bash
gh pr view <PR_NUMBER> --json number,headRefName,headRefOid,mergeStateStatus,statusCheckRollup
gh run list --workflow CI --branch <HEAD_BRANCH> \
  --json databaseId,headSha,status,conclusion,createdAt,updatedAt,displayTitle
```

Required evidence:

- old run ID and old head SHA
- newer run ID and newer head SHA
- old run `cancelled` conclusion where available
- newest head required checks passing
- no claim that main/tag/manual runs are cancelled
