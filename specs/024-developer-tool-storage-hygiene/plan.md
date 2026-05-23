# Implementation Plan: Developer-Tool Storage Hygiene

**Branch**: `codex/375-developer-tool-storage-hygiene` | **Date**: 2026-05-23 | **Spec**: `specs/024-developer-tool-storage-hygiene/spec.md`
**Input**: Feature specification from `/specs/024-developer-tool-storage-hygiene/spec.md`

**Note**: This template is filled in by the `/speckit.plan` command. See `.specify/templates/plan-template.md` for the execution workflow.

## Summary

#375 needs a source-backed Phase 1 developer-tool storage inventory before implementation, then deterministic dry-run-first cleanup/preflight behavior for the #375-owned surfaces: Codex TUI logs, Codex sessions, Factory droid log, and exact-name rustup toolchain retention/removal. The implementation direction is config-driven policy and scratch-fixture tests. It must not expand #374 verifier/parser architecture, #376 cargo/runtime inventory, or out-of-repo machine-cache cleanup.

## Technical Context

**Language/Version**: Python 3 for repo tooling and tests; Rust `1.95.0` remains the product/runtime pin and must not change for #375.
**Primary Dependencies**: Standard library for tooling; existing repo test pattern uses direct Python self-tests. No new dependency is planned.
**Storage**: Operator home path families measured by policy: Codex logs/sessions/report-only db files, Factory logs, rustup toolchains, and adjacent report-only surfaces.
**Testing**: TDD with scratch directories; targeted Python self-test for #375 policy; `git diff --check`; relevant existing path-filter/workflow checks if touched; full relevant Rust verification after implementation.
**Target Platform**: macOS operator workstation for measured path shapes; repo CI remains Linux for source checks.
**Project Type**: Developer-ops hygiene policy and verifier, not product runtime.
**Performance Goals**: Policy classification should complete over synthetic fixtures quickly enough for targeted tests; real preflight should be lightweight and read-only before heavy verification.
**Constraints**: No secret display; no mutation of real home data in tests; dry-run before apply; protect active/default/repository-root project-pinned rustup toolchains; rustup removal only by exact configured installed toolchain names after protection; no symlink target deletion or outside-root candidate mutation; report-only for Codex sqlite db/WAL until safe native cleanup is proven; no new shell parser or wrapper-family semantics; no #454 work.
**Scale/Scope**: One #375 PR covering enumeration, policy contract, tests, and bounded implementation. If direct cleanup requires a new operator-facing command surface, implementation pauses for explicit operator approval.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Gate | Status | Evidence |
|---|---|---|
| NT-first thin layer | Pass | #375 is developer-tool storage hygiene; `evidence.md` proves NT N/A for Codex/Factory/rustup cleanup mechanics. |
| Single path and config-controlled runtime | Pass with boundary | Product runtime is untouched. Hygiene policy must be config-driven and must not introduce alternate live submit or secret paths. |
| Test-first safety gates | Pass for plan | Implementation tasks require RED scratch-fixture tests before policy code. |
| Evidence before claims | Pass | `evidence.md` records issue graph, current measurements, Bolt source trace, pinned NT trace, and ownership. |
| Minimal slice discipline | Pass | This plan covers #375 only and names #454/#376/#374 as out of scope. |
| Pure Rust binary / SSM / TOML runtime | Pass | No runtime binary behavior or credential resolution changes are planned. |

Pre-implementation gate: Claude, Gemini, GLM, and DeepSeek must review `spec.md`, `evidence.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/`, `quickstart.md`, and `tasks.md` for source-proven blockers before implementation.

## Project Structure

### Documentation (this feature)

```text
specs/024-developer-tool-storage-hygiene/
|-- evidence.md
|-- spec.md
|-- plan.md
|-- research.md
|-- data-model.md
|-- quickstart.md
|-- contracts/
|   `-- developer-tool-storage-hygiene.md
`-- tasks.md
```

### Source Code (repository root)

```text
ci/
`-- developer-tool-storage-hygiene.toml      # proposed policy source

scripts/
|-- developer_tool_storage_hygiene.py        # proposed policy engine; operator-facing command use requires approval
`-- test_developer_tool_storage_hygiene.py   # scratch-fixture RED/GREEN tests

docs/ops/
`-- developer-tool-storage-hygiene.md        # operator policy and native-config guidance
```

**Structure Decision**: Use repo-local policy, docs, and tests. Product `src/` remains untouched. #374 verifier/parser scripts remain untouched unless pre-implementation review proves #375 cannot be satisfied otherwise and the operator approves.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Operator-facing cleanup command may be needed | #375 requires dry-run/apply or surfaced removal for sessions/logs/toolchains | Native Codex config only supports history size/persistence and log directory, not session TTL or log rotation. This remains gated on explicit operator approval before implementation. |
