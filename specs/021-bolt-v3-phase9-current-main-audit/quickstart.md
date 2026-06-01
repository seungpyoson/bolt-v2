# Quickstart: Phase 9 Audit Verification

Run from the root of the PR #331 worktree.

## Anchor

```bash
git status --short --branch
gh pr view 331 --json headRefOid,baseRefOid,mergeStateStatus,state
git rev-parse HEAD origin/main
```

Expected before P9 review:

- clean branch
- `git rev-parse HEAD` equals PR `headRefOid`
- PR state is `OPEN`
- merge state is `CLEAN`

## P7 And P8 Evidence

```bash
cargo test --test bolt_v3_no_submit_readiness -- --nocapture
cargo test --test bolt_v3_live_canary_gate -- --nocapture
cargo test --test bolt_v3_cli bolt_v3_cli_exposes_no_submit_readiness_operator_command -- --nocapture
gh pr checks 331 --json name,state,bucket,completedAt,link,workflow
```

Expected:

- no-submit readiness: 21 passed
- live-canary gate: 32 passed
- CLI command exposure: 1 passed
- PR checks green on the current pushed head

## Live-Readiness Blockers

```bash
ls -l config/live.local{.example,}.toml config/root.toml config/strategies/binary_oracle.example.toml
rg -n "T038|T046" specs/001-thin-live-canary-path/tasks.md
```

Expected:

- ignored `config/live.local.toml` may be present locally; `config/live.local.example.toml` is absent
- tracked root/strategy examples present only as examples
- real no-submit operator run T038 remains unchecked until explicit operator approval produces a satisfied report; the 2026-05-21 approved attempt failed `controlled_connect` and skipped `reference_readiness`, and a later non-secret probe narrowed the blocker to configured SSM target/API-key/IP/permission/account/environment state
- tiny-canary operator run T046 remains unchecked unless explicit operator approval and evidence are present

## Artifact Checks

The regexes split template and retired-reference tokens inside adjacent shell
strings so the commands do not match themselves.

```bash
rg -n "TB""D|TO""DO|FIX""ME|fix[[:space:]]+later|NE""EDS[[:space:]]+CLARIFICATION|\\[""FEATURE|\\[""###|\\[""ARGUMENTS\\]" specs/021-bolt-v3-phase9-current-main-audit .specify/memory/constitution.md
rg -n "019""-bolt-v3-phase9-audit-fresh|PR #""327|d6f""55774c32b71a242dcf78b8292a7f9e537afab|config/live.local"".example.toml" specs/021-bolt-v3-phase9-current-main-audit
git diff --check
```

Expected: no debt-template matches, no retired current-claim matches, and no whitespace errors.

## External Review Gate

Only after branch is clean, committed, pushed, and exact-head checks are green:

1. Claude custom review against P9 artifacts.
2. Gemini custom review against P9 artifacts.
3. Kimi custom review against P9 artifacts.
4. DeepSeek custom review with approval-token evidence.
5. GLM custom review with approval-token evidence.
6. Grok custom review unless explicitly waived.
7. Record exact-head findings in PR #331 evidence comments.

P9 source-review closure remains blocked until review disposition has no unresolved blockers.
