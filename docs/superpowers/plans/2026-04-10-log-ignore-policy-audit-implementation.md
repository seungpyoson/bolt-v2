# Log Ignore Policy Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broad `*.log` ignore rule with explicit patterns that still ignore real Nautilus runtime logs in the current attached checkout context, while no longer masking future tracked fixture logs.

**Architecture:** This change stays behavior-preserving by modifying only `.gitignore` and scratch artifacts. The implementation is driven by proof, not inference: confirm the real logger filename contract, narrow ignore rules to match it, remove the stale `tmp_tests` scratch artifact, and verify from the issue-72 attached worktree itself that runtime logs still ignore while fixture paths do not.

**Tech Stack:** Git ignore rules, Rust/Nautilus runtime naming contract verification, shell-based `git check-ignore`, Cargo baseline verification

---

### Task 1: Narrow The Ignore Rules To The Proven Runtime Contract

**Files:**
- Modify: `.gitignore`
- Modify: `docs/superpowers/specs/2026-04-10-log-ignore-policy-audit-design.md`
- Modify: `docs/superpowers/plans/2026-04-10-log-ignore-policy-audit-implementation.md`
- Optional delete: `tmp_tests/issue-522.log`

- [ ] **Step 1: Reconfirm the pre-change artifact inventory**

Run:

```bash
find . -maxdepth 3 \( -name '*.log' -o -path './tmp_tests' \) | sort
```

Expected:

- root or worktree-root runtime logs with shape `TRADERID_YYYY-MM-DD_INSTANCE.log`
- `tmp_tests/issue-522.log` only if `tmp_tests/` exists in this checkout
- no tracked fixture `.log` paths

- [ ] **Step 2: Replace the broad ignore with explicit patterns**

Update `.gitignore` from:

```gitignore
/target
config/live.toml
config/live.local.toml
*.log
.omx/
.claude/findings/
.DS_Store
```

To:

```gitignore
/target
config/live.toml
config/live.local.toml
/*_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_*.log
/tmp_tests/*.log
.omx/
.claude/findings/
.DS_Store
```

- [ ] **Step 3: Remove the stale scratch log**

If `tmp_tests/` exists, delete:

```text
tmp_tests/issue-522.log
```

If `tmp_tests/` becomes empty after that deletion, remove the directory too. If `tmp_tests/` is absent, this step is a no-op.

- [ ] **Step 4: Prove the attached-worktree ignore behavior from the current issue-72 checkout**

Run:

```bash
git check-ignore -v ALPHA-999_2026-04-10_12345678-1234-1234-1234-123456789abc.log
git check-ignore -v tmp_tests/example.log
! git check-ignore -v tests/fixtures/example.log
```

Expected:

- first command reports the new root runtime rule from `.gitignore`
- second command reports the `tmp_tests/*.log` rule
- third command exits successfully without reporting an ignore match

- [ ] **Step 5: Record why no cross-branch worktree proof is required**

Verification note:

- this branch is checked out at `.worktrees/issue-72-log-ignore-policy`
- running Step 4 from this directory is already a real attached-worktree proof
- do not require another branch or worktree to adopt the new `.gitignore` before merge

Expected:

- the verification contract stays branch-local
- no unrelated worktree needs modification for this issue

- [ ] **Step 6: Reconfirm diff scope**

Run:

```bash
git diff --name-only
git status --short
```

Expected:

- working-tree diff limited to:
  - `.gitignore`
  - `docs/superpowers/specs/2026-04-10-log-ignore-policy-audit-design.md`
  - `docs/superpowers/plans/2026-04-10-log-ignore-policy-audit-implementation.md`
  - optional `tmp_tests/issue-522.log` removal or `tmp_tests/` removal only if present in this checkout
- no runtime/config/source-code drift
- branch-vs-base scope check happens after commit in Task 2

- [ ] **Step 7: Run baseline cargo verification**

Run:

```bash
CARGO_TARGET_DIR="$PWD/.target" ~/.cargo/bin/cargo test --no-run
```

Expected:

- pass

- [ ] **Step 8: Commit the implementation**

Run:

```bash
git add .gitignore docs/superpowers/specs/2026-04-10-log-ignore-policy-audit-design.md docs/superpowers/plans/2026-04-10-log-ignore-policy-audit-implementation.md
if [ -e tmp_tests ] || [ -n "$(git status --short -- tmp_tests)" ]; then git add -A -- tmp_tests; fi
git commit -m "chore: narrow log ignore policy to explicit runtime patterns"
```

Expected:

- one commit containing only the intentional ignore-policy diff, the spec/plan docs, and optional scratch cleanup if present

### Task 2: Final Verification And Review Freeze

**Files:**
- Modify: none

- [ ] **Step 1: Run final verification commands**

Run:

```bash
CARGO_TARGET_DIR="$PWD/.target" ~/.cargo/bin/cargo test --no-run
git check-ignore -v ALPHA-999_2026-04-10_12345678-1234-1234-1234-123456789abc.log
git check-ignore -v tmp_tests/example.log
! git check-ignore -v tests/fixtures/example.log
git diff --name-only
git diff --name-only origin/main...HEAD
```

Expected:

- cargo build/test graph still passes
- ignore proofs succeed from this attached worktree checkout
- final working-tree diff remains limited to intentional local edits
- final branch diff remains limited to:
  - `.gitignore`
  - `docs/superpowers/specs/2026-04-10-log-ignore-policy-audit-design.md`
  - `docs/superpowers/plans/2026-04-10-log-ignore-policy-audit-implementation.md`
  - optional scratch cleanup only if present in this checkout

- [ ] **Step 2: Remove local build artifact before handoff**

Run:

```bash
rm -rf .target
git status --short
```

Expected:

- no untracked `.target`
- worktree clean except intended tracked diff
