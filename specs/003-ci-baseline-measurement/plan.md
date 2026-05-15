# Implementation Plan: CI Baseline Measurement

**Branch**: `codex/ci-333-baseline` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-ci-baseline-measurement/spec.md`

## Summary

Measure current CI behavior for #343 with exact GitHub Actions evidence before any #333 topology changes. The deliverable is a durable baseline document under `docs/ci/` plus a full child-issue state map. This task is measurement-only: no workflow topology, runtime, or build behavior changes.

## Technical Context

**Language/Version**: Rust repository, GitHub Actions workflow, shell tooling
**Primary Dependencies**: GitHub Actions metadata/logs via `gh`, existing `just ci-lint-workflow`
**Storage**: Markdown docs and spec-kit artifacts only
**Testing**: `rg` evidence check, `git diff --check`, `just ci-lint-workflow`
**Target Platform**: GitHub Actions for `seungpyoson/bolt-v2`
**Project Type**: Rust live trading binary with CI workflow
**Performance Goals**: Baseline must distinguish wall time from runner-minute cost and identify true critical path per run shape
**Constraints**: No workflow behavior change, no merge, no secret display, no inferred cache warmth, no child scope narrowing
**Scale/Scope**: One #343 measurement slice covering all #333 children as consumers

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **NT-First Thin Layer**: PASS. No runtime code is touched.
- **Generic Core, Concrete Edges**: PASS. No Rust core or adapter code is touched.
- **Single Path And Config-Controlled Runtime**: PASS. No runtime path or config behavior is changed.
- **Test-First Safety Gates**: PASS with documentation-slice adaptation. This slice has no production behavior change; verification is evidence and lint based.
- **Evidence Before Claims**: PASS. All claims cite exact run IDs, SHAs, timestamps, job durations, and log excerpts.
- **Minimal Slice Discipline**: PASS. One branch addresses #343 measurement only and names downstream child issue consumers.

## Phase 0 Research Summary

Detailed decisions are in [research.md](research.md).

- Use `gh run view --json jobs` as primary timing evidence.
- Use raw active runner minutes plus rounded-per-job estimates.
- Mark cache warmth only from log-backed cache hit/restored key evidence.
- Include multiple run shapes because one run cannot represent PR, main, tag, source-fence, and build-required behavior.

## Phase 1 Design Summary

Design details are in [data-model.md](data-model.md) and [quickstart.md](quickstart.md).

The baseline artifact has these sections:

- Source state and method.
- Run summary table.
- Per-run job details.
- Cache observations.
- Child issue state map.
- Child issue requirement inventory and live-source conflict notes.
- Follow-on use per child issue.

## Project Structure

### Documentation (this feature)

```text
specs/003-ci-baseline-measurement/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
└── checklists/
    └── requirements.md

docs/ci/
└── ci-baseline-2026-05-15.md
```

### Source Code (repository root)

No source code changes. Verified by excluding the scoped docs/spec-kit paths from `git diff --name-only origin/main...HEAD`.

### Workflow Files

No `.github/workflows/` changes.

**Structure Decision**: Store the durable baseline in `docs/ci/` because later CI children can link one stable path without reading chat history.

## Complexity Tracking

No constitution violations.
