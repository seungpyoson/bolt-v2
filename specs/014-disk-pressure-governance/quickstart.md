# Quickstart: Saving Disk Reliably

## 1. Measure Before Cleanup

Run read-only checks first:

```bash
df -h ~
du -sh \
  ~/Projects/Claude/bolt-v2/target \
  ~/.cache/rust-verification/bolt-v2 \
  ~/.cargo/registry \
  ~/.cargo/git \
  ~/.codex/log \
  ~/.codex/sessions \
  ~/.rustup/toolchains \
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
| `~/Projects/Claude/bolt-v2/target` | 27G | unmanaged repo-local Cargo target |
| `/private/tmp/bolt-v2-shadow-target` | 10G | unmanaged temp Cargo target |
| `~/.cache/rust-verification/bolt-v2/target` | 16G | managed Rust target cache |
| `~/.no-mistakes/worktrees/.../target` | historical | no-mistakes worktree-local Cargo target; one recorded run failed with `No space left on device` |

| Path family | Owner | Default action |
|---|---|---|
| repo or worktree `target/` | #374 | Do not delete as "fix"; prove routing gap, then dry-run cleanup |
| no-mistakes worktree-local `target/` | #374 | Route no-mistakes through managed commands or exact-head CI evidence |
| `~/.cache/rust-verification/bolt-v2` | #286 | Status/prune policy, preserve hot cache |
| `/private/var/.../T/cargo-*` | #70 | Closed historical scratch-diagnostic class unless reproduced |
| `/private/tmp/claude-*/*/*/tasks/*.output` | #125 / claude-config | Do not unlink live files; follow Claude task-output guard work |
| Codex logs/sessions, factory logs, rustup toolchains | #375 | Rotation/TTL/pin-driven hygiene |
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
2. Use dry-run cleanup/status for owner-specific tools.
3. Apply only reviewed owner-specific cleanup.
4. Re-measure `df -h ~` and the affected path family.
5. Record evidence on the owning issue or PR.
