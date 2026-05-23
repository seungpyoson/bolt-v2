# Quickstart: Saving Disk Reliably

## 1. Measure Before Cleanup

Run read-only checks first:

Replace `REPO_ROOT_PATH` and `USER_HOME_DIR` with the local repo and home paths before running.

```bash
df -h USER_HOME_DIR
du -sh \
  REPO_ROOT_PATH/target \
  USER_HOME_DIR/.cache/rust-verification/bolt-v2 \
  USER_HOME_DIR/.cargo/registry \
  USER_HOME_DIR/.cargo/git \
  USER_HOME_DIR/.codex/log \
  USER_HOME_DIR/.codex/sessions \
  USER_HOME_DIR/.codex/history.jsonl \
  USER_HOME_DIR/.codex/logs_2.sqlite* \
  USER_HOME_DIR/.codex/archived_sessions \
  USER_HOME_DIR/.factory/logs \
  USER_HOME_DIR/.rustup/toolchains \
  /private/tmp/claude-* \
  /private/tmp/bolt-v2-* \
  2>/dev/null
```

Then check active writers before any apply-mode cleanup:

```bash
ps -axo pid,ppid,comm,args | rg 'cargo|rustc|rust_verification|nextest|claude|codex|gemini|aider' | rg -v 'rg '
```

## 2. Classify The Consumer

Verified local snapshot for operator `spson` on 2026-05-18:

| Path | Size | Classification |
|---|---:|---|
| `REPO_ROOT_PATH/target` | 27G | unmanaged repo-local Cargo target |
| `/private/tmp/bolt-v2-shadow-target` | 10G | unmanaged temp Cargo target |
| `USER_HOME_DIR/.cache/rust-verification/bolt-v2/target` | 16G | managed Rust target cache |
| `USER_HOME_DIR/.no-mistakes/worktrees/.../target` | historical | no-mistakes worktree-local Cargo target; one recorded run failed with `No space left on device` |

| Path family | Owner | Default action |
|---|---|---|
| repo or worktree `target/` | #374 | Do not delete as "fix"; prove routing gap, then dry-run cleanup |
| no-mistakes worktree-local `target/` | #374 | Route no-mistakes through managed commands or exact-head CI evidence |
| `USER_HOME_DIR/.cache/rust-verification/bolt-v2` | #286 | Completed by PR #404; use status/prune policy and preserve hot cache |
| `/private/var/.../T/cargo-*` | #70 | Closed historical scratch-diagnostic class unless reproduced |
| `/private/tmp/claude-*/*/*/tasks/*.output` | #125 / claude-config | Do not unlink live files; follow Claude task-output guard work |
| Codex log/session cleanup surfaces | #375 | Use `docs/ops/developer-tool-storage-hygiene.md` and `scripts/developer_tool_storage_hygiene.py` for TOML-owned status, dry-run, preflight, and apply |
| Codex SQLite, Codex history, Codex archived sessions | #375 | Report-only measurement and native-guidance evidence; no apply-mode cleanup |
| Factory logs | #375 | TOML-owned rotation workflow with active-writer refusal |
| Rustup toolchains | #375 | Exact-name retention/removal workflow with active/default/project-pinned protection |
| bolt-v3 runtime output, local CI/test artifacts, cargo registry/git | #376 | Inventory and cap |
| unclassified large tree | #377 | Unknown-class detection and triage |

## 3. Rust Tests: Local Or Remote?

CI is the broad verifier once a PR exists. Prefer opening a draft PR early and letting GitHub Actions run the full suite.

Use local Cargo only when it gives a signal CI cannot provide cheaply:

- narrow TDD red/green on a touched test;
- reproducing or debugging a CI failure;
- validating local Cargo routing/cache behavior itself;
- avoiding an obviously broken push when the expected build is small.

Use managed local tests for those narrow cases:

```bash
just test --test <test-name>
```

Do not run broad local Cargo just because a checklist says "run tests". CI already builds nextest archives and shards tests through managed workflows.

Do not use raw `cargo test` from arbitrary shells for this epic. That is part of the failure surface until #374 proves shell-agnostic routing.

Do not use S3 as active Cargo cache. Use S3 only for immutable deploy artifacts or evidence packages.

## 4. Safe Reclaim Order

1. Stop active runaway producers if any are proven.
2. Use dry-run cleanup/status for owner-specific tools. For #375 developer-tool storage, use `docs/ops/developer-tool-storage-hygiene.md` and `scripts/developer_tool_storage_hygiene.py`.
3. Apply only reviewed owner-specific cleanup.
4. Re-measure `df -h ~` and the affected path family.
5. Record evidence on the owning issue or PR.
