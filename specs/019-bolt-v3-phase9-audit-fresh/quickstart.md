# Quickstart: Phase 9 Audit Verification

Run from `/Users/spson/Projects/Claude/bolt-v2/.worktrees/019-bolt-v3-phase9-audit-fresh`.

## Anchor

```bash
git status --short --branch
git rev-parse HEAD main origin/main
```

Expected: clean branch at `d6f55774c32b71a242dcf78b8292a7f9e537afab` before artifact edits.

## no-mistakes

```bash
which no-mistakes
no-mistakes --version
no-mistakes daemon status
```

Expected: installed binary, version printed, daemon running or exact blocker recorded.

## Baseline Test

```bash
cargo test --lib
```

Expected for baseline captured in this session: 446 passed, 0 failed, 1 ignored.

## Artifact Checks

```bash
rg -n "TB""D|TO""DO|FIX""ME|fix[[:space:]]+later|NE""EDS[[:space:]]+CLARIFICATION|\\[""FEATURE|\\[""###|\\[""ARGUMENTS\\]" specs/019-bolt-v3-phase9-audit-fresh .specify/memory/constitution.md
git diff --check
```

Expected: no debt-template matches; no whitespace errors.

## External Review Gate

Only after branch is clean, committed, pushed, and exact-head checks are green:

1. Claude custom review against Phase 9 artifacts.
2. DeepSeek custom review with approval-token evidence.
3. GLM custom review with approval-token evidence.
4. Record findings in `external-review-phase9-disposition.md`.

Implementation remains blocked until review disposition has no unresolved blockers.
