# Contract: Developer-Tool Storage Hygiene

## Issue Boundary

This contract applies only to #375.

Included:
- Codex TUI log rotation policy.
- Codex session JSONL TTL policy.
- Factory droid log rotation policy or bounded-evidence report.
- Rustup toolchain retention policy.
- Report-only measurement for adjacent developer-tool storage discovered during Phase 1.

Excluded:
- #374 verifier/parser architecture, wrapper families, shell command prediction, repo target cleanup, and managed-cache preflight.
- #376 bolt-v3 runtime output, local CI/test artifacts, cargo registry, and cargo git steady-state.
- #454 verifier bloat decomposition.
- Out-of-repo machine-cache cleanup for browser profiles, IDE caches, npm/pnpm/yarn/Bun, Homebrew, Xcode, and cloud CLI caches.

## Policy Contract

The policy source must be TOML and must name every cleanup-managed path family explicitly. Runtime defaults are not allowed in code.

Required sections:

```toml
[codex.log]
max_bytes = 209715200
retained_rotations = 2

[codex.sessions]
ttl_days = 14

[factory.log]
max_bytes = 209715200
retained_rotations = 2

[rustup.toolchains]
stale_after_days = 14
retain_recent = 1

[preflight]
warning_bytes = 10737418240
error_bytes = 5368709120
```

Values above are example policy shape for review; implementation must use the committed TOML source as authority.

## Dry-Run Contract

Dry-run output must include:
- Policy file path.
- Evaluated home root or scratch root.
- Per-surface bytes and cleanup eligibility.
- Candidate actions with reason and estimated bytes.
- Protected rustup toolchains and reason.
- Report-only large surfaces and reason.
- Out-of-scope adjacent surfaces.

Dry-run must not modify files.

## Apply Contract

Apply behavior is allowed only after explicit operator approval for any new operator-facing command surface. If approved, apply must:
- Re-validate policy.
- Re-scan immediately before mutation.
- Refuse protected and report-only targets.
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
