# Quickstart: CI Baseline Measurement

## Evidence commands

```bash
git status --short --branch
git rev-parse HEAD
gh issue view 333 --repo seungpyoson/bolt-v2 --json number,title,state,body,comments
gh issue view 343 --repo seungpyoson/bolt-v2 --json number,title,state,body,comments
gh run view <RUN_ID> --repo seungpyoson/bolt-v2 --json databaseId,event,headSha,headBranch,displayTitle,status,conclusion,createdAt,updatedAt,url,jobs
gh run view <RUN_ID> --repo seungpyoson/bolt-v2 --job <JOB_ID> --log
```

Freshness spot-check before relying on a baseline row:

```bash
gh run view 25866346320 --repo seungpyoson/bolt-v2 --json databaseId,headSha,event,status,conclusion,createdAt,updatedAt,url
```

## Verification commands

```bash
rg -n "25855655415|25866930064|25866346320|25859831755|25862551803|24623274722|#343|#342|#332|#195|#205|#203|#335|#344|#340|#333|drift-detection" docs/ci/ci-baseline-2026-05-15.md specs/003-ci-baseline-measurement
just ci-lint-workflow
git diff --check
```

## Completion rule

#343 is complete only after the baseline document is committed, pushed, and linked from #343 or #333. This task does not change workflow topology.
