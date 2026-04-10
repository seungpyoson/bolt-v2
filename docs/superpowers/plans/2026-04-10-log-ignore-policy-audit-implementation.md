# Log Ignore Policy Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broad `*.log` ignore rule with explicit patterns that still ignore real Nautilus runtime logs in the main checkout and official worktrees, while no longer masking future tracked fixture logs.

**Architecture:** This change stays behavior-preserving by modifying only `.gitignore` and scratch artifacts. The implementation is driven by proof, not inference: confirm the real logger filename contract, narrow ignore rules to match it, remove the stale `tmp_tests` scratch artifact, and verify that runtime logs still ignore while fixture paths do not.

**Tech Stack:** Git ignore rules, Rust/Nautilus runtime naming contract verification, shell-based `git check-ignore`, Cargo baseline verification

---

### Task 1: Narrow The Ignore Rules To The Proven Runtime Contract

**Files:**
- Modify: `.gitignore`
- Delete: `tmp_tests/issue-522.log`

- [ ] **Step 1: Reconfirm the pre-change artifact inventory**

Run:

```bash
find . -maxdepth 3 \( -name '*.log' -o -path './tmp_tests' \) | sort
```

Expected:

- root or worktree-root runtime logs with shape `TRADERID_YYYY-MM-DD_INSTANCE.log`
- `tmp_tests/issue-522.log`
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
/*_????-??-??_*.log
/tmp_tests/*.log
.omx/
.claude/findings/
.DS_Store
```

- [ ] **Step 3: Remove the stale scratch log**

Delete:

```text
tmp_tests/issue-522.log
```

If `tmp_tests/` becomes empty after that deletion, remove the directory too.

- [ ] **Step 4: Prove the main-checkout ignore behavior**

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

- [ ] **Step 5: Prove the attached-worktree ignore behavior**

Run from the repository root:

```bash
git -C .worktrees/fix-48-rust-worktree-enforcement check-ignore -v ALPHA-999_2026-04-10_12345678-1234-1234-1234-123456789abc.log
git -C .worktrees/fix-48-rust-worktree-enforcement check-ignore -v tmp_tests/example.log
! git -C .worktrees/fix-48-rust-worktree-enforcement check-ignore -v tests/fixtures/example.log
```

Expected:

- same results as the main checkout
- proof that root-anchored patterns work in official worktrees too

- [ ] **Step 6: Reconfirm diff scope**

Run:

```bash
git diff --name-only origin/main..HEAD
git status --short
```

Expected:

- diff limited to `.gitignore` plus `tmp_tests/issue-522.log` removal
- no runtime/config/source-code drift

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
git add .gitignore tmp_tests
git commit -m "chore: narrow log ignore policy to explicit runtime patterns"
```

Expected:

- one commit containing only ignore-policy and scratch cleanup

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
git -C .worktrees/fix-48-rust-worktree-enforcement check-ignore -v ALPHA-999_2026-04-10_12345678-1234-1234-1234-123456789abc.log
git -C .worktrees/fix-48-rust-worktree-enforcement check-ignore -v tmp_tests/example.log
! git -C .worktrees/fix-48-rust-worktree-enforcement check-ignore -v tests/fixtures/example.log
git diff --name-only origin/main..HEAD
```

Expected:

- cargo build/test graph still passes
- ignore proofs succeed in both main checkout and attached worktree
- final diff remains limited to `.gitignore` and scratch cleanup

- [ ] **Step 2: Remove local build artifact before handoff**

Run:

```bash
rm -rf .target
git status --short
```

Expected:

- no untracked `.target`
- worktree clean except intended tracked diff
