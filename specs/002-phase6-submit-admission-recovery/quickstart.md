# Quickstart: Phase 6 Submit Admission Recovery

## Purpose

Use this workflow to recover Phase 6 from stale PR #317 without reviving stale Phase 3-5 work.

## Steps

1. Refresh baseline.

```bash
git fetch origin main
git rev-parse origin/main
git status --short --branch
```

2. Capture stale PR facts.

```bash
gh pr view 317 --json number,title,state,isDraft,baseRefName,baseRefOid,headRefName,headRefOid,changedFiles,additions,deletions,url
git merge-base origin/main origin/012-bolt-v3-phase6-submit-admission
git diff --stat origin/main..origin/012-bolt-v3-phase6-submit-admission
```

3. Write recovery memo.

```text
Edit specs/002-phase6-submit-admission-recovery/recovery-review.md.
Classify every Phase 6-relevant stale concept as keep, rewrite, or reject.
```

4. Generate recovery-strategy review prompt.

```text
Use specs/002-phase6-submit-admission-recovery/contracts/recovery-review-contract.md.
Ask reviewers to challenge the recovery strategy before implementation.
Do not send the prompt while planning artifacts are uncommitted or local findings are unresolved.
```

5. Resolve strategy findings.

```text
For each finding: fix memo, disprove with evidence, or defer explicitly.
Do not implement Phase 6 while findings remain unresolved.
```

6. Create fresh implementation branch only after strategy approval.

```bash
git fetch origin main
git switch -c <fresh-phase6-branch> origin/main
```

7. Implement Phase 6 with TDD.

```text
Failing tests first.
Minimal code second.
Anti-slop cleanup third.
Verification fourth.
```

8. Request code review only after exact-head CI is green.

```text
Include exact base/head SHAs and residual scope in review prompt.
```
