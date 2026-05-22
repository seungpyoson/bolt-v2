# Contract: Developer-Tool Storage Hygiene

## Issue Boundary

This contract applies only to #375.

Included:
- Codex TUI log rotation policy.
- Codex session JSONL TTL policy.
- Codex history file native-configuration guidance as report-only #375 evidence.
- Factory droid log rotation policy or bounded-evidence report.
- Rustup toolchain retention policy.
- Report-only measurement for adjacent developer-tool storage discovered during Phase 1.

Excluded:
- #374 verifier/parser architecture, wrapper families, shell command prediction, repo target cleanup, and managed-cache preflight.
- #376 bolt-v3 runtime output, local CI/test artifacts, cargo registry, and cargo git steady-state.
- #454 verifier bloat decomposition.
- Out-of-repo machine-cache cleanup for browser profiles, IDE caches, npm/pnpm/yarn/Bun, Homebrew, Xcode, and cloud CLI caches.

## Policy Contract

The policy source must be TOML and must name every cleanup-managed and report-only path family explicitly. Runtime defaults are not allowed in code.

Required cleanup sections:

```toml
[codex.log]
max_bytes = 209715200
retained_rotations = 2
active_writer_processes = ["codex", "codex-tui"]

[codex.sessions]
ttl_days = 14

[factory.log]
max_bytes = 209715200
retained_rotations = 2
active_writer_processes = ["factory", "droid"]

[rustup.toolchains]
stale_after_days = 14
retain_recent = 1

[preflight]
free_disk_warning_bytes = 10737418240
free_disk_error_bytes = 5368709120
owned_storage_warning_bytes = 10737418240
owned_storage_error_bytes = 21474836480
```

Required native-guidance sections are report-only. They document native configuration values to surface in dry-run/preflight output and must never create cleanup candidates:

```toml
[native_guidance.codex_history]
max_bytes = 104857600
persistence = "save-all"
```

Values above are example policy shape for review; implementation must use the committed TOML source as authority.

## Dry-Run Contract

Dry-run output must include:
- Policy file path.
- Evaluated home root or scratch root.
- Per-surface bytes and cleanup eligibility.
- Candidate actions with reason and estimated bytes.
- Codex history native-config status and reason.
- Report-only Codex archived sessions and reason.
- Protected rustup toolchains and reason.
- Report-only large surfaces and reason.
- Out-of-scope adjacent surfaces.

Dry-run must not modify files.

## Apply Contract

Apply behavior is allowed only after explicit operator approval for any new operator-facing command surface. If approved, apply must:
- Re-validate policy immediately before mutation.
- Re-scan immediately before mutation.
- Abort if the immediate re-scan no longer matches the candidate state being applied.
- Refuse mutable log actions when configured active writer processes are detected.
- Refuse protected and report-only targets.
- Always refuse report-only Codex SQLite db/WAL, Codex history, and Codex archived-session targets.
- Preserve active, default, and project-pinned rustup toolchains.
- Emit a post-apply summary.

## Preflight Contract

Preflight is read-only. It must fail closed when configured free-disk or #375-owned storage thresholds are breached, and it must recommend exact follow-up classes without deleting data.

## Review Contract

Before implementation:
- Claude, Gemini, GLM, and DeepSeek must review the plan/spec/tasks package for source-proven blockers.
- Reviews must record model, head SHA, scope, verdict, blockers, and skipped slots.

Before PR ready:
- Targeted tests pass.
- Relevant full Rust verification passes or is explicitly not applicable with evidence.
- Source-fence/schema/runtime literal checks pass if touched.
- Changed files pass ai-slop cleanup.
- no-mistakes runs on exact PR head and the verified head equals PR head.
- Exact-head GitHub CI is green.
- External adversarial review covers the exact PR head.
