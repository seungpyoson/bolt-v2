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
schema_version = 1

[codex.log]
path_family = "~/.codex/log/codex-tui.log"
category = "AI agent"
growth_shape = "single_file"
owner = "owned"
native_policy = "partial"
cleanup_mode = "rotate"
max_bytes = 209715200
retained_rotations = 2
active_writer_processes = ["codex", "codex-tui"]

[codex.sessions]
path_family = "~/.codex/sessions/**/*.jsonl"
category = "AI agent"
growth_shape = "many_files"
owner = "owned"
native_policy = "none_found"
cleanup_mode = "ttl_prune"
ttl_days = 14
active_writer_processes = ["codex", "codex-tui"]

[factory.log]
path_family = "~/.factory/logs/droid-log-single.log"
category = "AI agent"
growth_shape = "single_file"
owner = "owned"
native_policy = "none_found"
cleanup_mode = "rotate"
max_bytes = 209715200
retained_rotations = 2
active_writer_processes = ["factory", "droid"]

[rustup.toolchains]
path_family = "~/.rustup/toolchains/*"
category = "version manager"
growth_shape = "tree"
owner = "owned"
native_policy = "yes"
cleanup_mode = "toolchain_retention"
retain_exact_names = ["1.95.0-aarch64-apple-darwin"]
remove_exact_names = []

[preflight]
free_disk_warning_bytes = 10737418240
free_disk_error_bytes = 5368709120
owned_storage_warning_bytes = 10737418240
owned_storage_error_bytes = 21474836480
```

Required report-only sections name path families that are measured and protected from apply:

```toml
[codex.sqlite]
path_family = "~/.codex/logs_2.sqlite*"
category = "AI agent"
growth_shape = "sqlite_with_wal"
owner = "report_only"
native_policy = "none_found"
cleanup_mode = "none"

[codex.archived_sessions]
path_family = "~/.codex/archived_sessions/**"
category = "AI agent"
growth_shape = "tree"
owner = "report_only"
native_policy = "none_found"
cleanup_mode = "none"
```

Required native-guidance sections are report-only. They document native configuration values to surface in dry-run/preflight output and must never create cleanup candidates:

```toml
[native_guidance.codex_history]
path_family = "~/.codex/history.jsonl"
category = "AI agent"
growth_shape = "single_file"
owner = "report_only"
native_policy = "yes"
cleanup_mode = "none"
max_bytes = 104857600
persistence = "save-all"
```

Values above are example policy shape for review; implementation must use the committed TOML source as authority.

## Path Handling Contract

Candidate enumeration must:
- Expand `~` only against the evaluated home root or scratch root.
- Stay inside configured `path_family` roots after normalization.
- Treat symlinks as protected/report-only entries and never follow symlink targets for deletion.
- Detect the project-pinned Rust toolchain from the repository-root `rust-toolchain.toml`; nested Rust toolchain files are out of scope unless explicitly added to policy.

## Dry-Run Contract

Dry-run output must include:
- Policy file path.
- Evaluated home root or scratch root.
- Per-surface bytes and cleanup eligibility.
- Candidate actions with reason and estimated bytes.
- Codex history native-config status and reason.
- Report-only Codex archived sessions and reason.
- Protected rustup toolchains and reason.
- Exact-name rustup removal candidates and reason.
- Report-only large surfaces and reason.
- Out-of-scope adjacent surfaces.

Dry-run must not modify files.

## Active Writer Contract

Active-writer detection must:
- Use exact process names configured in TOML for mutable Codex and Factory surfaces.
- Consume a process snapshot input rather than parsing shell command strings.
- Use synthetic process snapshots in tests.
- Treat host process-table collection for any operator-facing apply command as part of the T012 approval gate.

## Apply Contract

Apply behavior is allowed only after explicit operator approval for any new operator-facing command surface. If approved, apply must:
- Re-validate policy immediately before mutation.
- Re-scan immediately before mutation.
- Abort if the immediate re-scan no longer matches the candidate state being applied.
- Refuse mutable Codex and Factory actions when configured active writer processes are detected.
- Refuse protected and report-only targets.
- Always refuse report-only Codex SQLite db/WAL, Codex history, and Codex archived-session targets.
- Preserve active, default, and repository-root project-pinned rustup toolchains unconditionally, even if their exact names appear in removal config; TOML cannot disable these protections.
- Remove rustup toolchains only when their exact installed toolchain name appears in `remove_exact_names`; age, mtime, wildcard, or pattern matching must never create a rustup removal candidate.
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
