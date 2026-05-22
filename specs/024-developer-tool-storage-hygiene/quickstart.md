# Quickstart: Developer-Tool Storage Hygiene

## Planning Gate

1. Read `specs/024-developer-tool-storage-hygiene/evidence.md`.
2. Confirm #375 ownership is limited to Codex logs/sessions, Factory droid log, rustup toolchains, and report-only measurement of adjacent developer-tool surfaces.
3. Confirm no #454 branch, implementation, or refactor work has started.
4. Run Speckit tasks generation before implementation.
5. Send the plan/spec/tasks package to Claude, Gemini, GLM, and DeepSeek for source-proven blockers.

## TDD Shape

Use scratch directories only:

```text
scratch-home/
|-- .codex/
|   |-- log/codex-tui.log
|   |-- history.jsonl
|   |-- sessions/2026/05/*.jsonl
|   |-- archived_sessions/2026/*.jsonl
|   `-- logs_2.sqlite-wal
|-- .factory/logs/droid-log-single.log
`-- .rustup/toolchains/
    |-- 1.94.1-aarch64-apple-darwin/
    |-- 1.95.0-aarch64-apple-darwin/
    `-- stable-aarch64-apple-darwin/
```

RED tests should prove:
- Oversized Codex/Factory logs are rotation candidates.
- Stale Codex sessions are TTL prune candidates.
- Codex sqlite db/WAL files are report-only.
- Codex history is report-only and mapped to native config guidance.
- Codex archived sessions are report-only.
- Dry-run output includes the fields required by the contract.
- Active, default, and project-pinned rustup toolchains are protected.
- Stale unprotected toolchains are surfaced as candidates.
- Preflight fails closed when configured thresholds are breached.
- Apply revalidates policy immediately before mutation and fails closed if the policy changed.
- Apply re-scans immediately before mutation and fails closed if candidate state changed.
- Apply refuses mutable Codex and Factory actions when configured active writer processes are detected from synthetic process snapshots.

## Verification Commands

Planned targeted checks after implementation:

```bash
python3 scripts/test_developer_tool_storage_hygiene.py
python3 -m py_compile scripts/developer_tool_storage_hygiene.py scripts/test_developer_tool_storage_hygiene.py
git diff --check origin/main...HEAD
```

Run broader repo checks only after the implementation surface is known. If Rust/product files remain untouched, full Rust verification may be recorded as not applicable with source evidence; otherwise run the relevant `just` verification through managed entrypoints.

## Operator Approval Gate

If implementation needs a new operator-facing command such as status, dry-run, or apply, pause before coding that command surface and obtain explicit operator approval. Without that approval, keep implementation to inventory, config, verifier tests, and documentation.
