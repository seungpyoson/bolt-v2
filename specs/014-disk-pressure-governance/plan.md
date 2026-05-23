# Implementation Plan: Disk Pressure Governance

**Branch**: `codex/123-disk-pressure-speckit` | **Date**: 2026-05-18 | **Spec**: `specs/014-disk-pressure-governance/spec.md`
**Input**: Feature specification from `/specs/014-disk-pressure-governance/spec.md`

## Summary

Turn epic #123 into a reliable disk-pressure governance plan before additional code changes. The first deliverable was docs/spec coverage: live issue map, operator walkthrough, verified local-vs-CI Rust verification policy, no-mistakes routing evidence, issue-to-PR decomposition, #286 completion evidence, and finite Phase 1 research gates. #375 is now implemented by the separate `specs/024-developer-tool-storage-hygiene/` slice; remaining implementation slices continue to follow TDD one issue per PR.

## Technical Context

**Language/Version**: Rust workspace with repo-local SpecKit docs and shell/Python verification wrappers
**Primary Dependencies**: `.specify`, `just`, `cargo-nextest`, `scripts/rust_verification.py`, `ci/rust-verification.toml`, GitHub Actions cache/artifacts, no-mistakes, external review plugins
**Storage**: Markdown specs/docs, managed Rust target cache, local temp paths, AI-agent logs/sessions, cargo registry/git caches
**Testing**: Speckit artifact checks, `git diff --check`, CI exact-head checks before review; local Cargo only by explicit exception
**Target Platform**: macOS developer machine plus GitHub Actions Linux runners
**Project Type**: Rust live-trading repo plus local developer tooling governance
**Performance Goals**: Prevent recurring disk exhaustion; keep routine targeted verification practical without duplicating mandatory CI by default
**Constraints**: no raw cargo workarounds, no no-mistakes worktree-local target drift, no S3 active target cache, no hardcoded runtime policy, no destructive cleanup without dry-run evidence, no credential display, one issue per PR
**Scale/Scope**: Epic #123 and child issues #48, #70, #124, #125, #286, #374, #375, #376, #377

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- NT-first thin layer: PASS. This planning slice does not add runtime trading machinery.
- Generic core: PASS. No provider, venue, market, wallet, or strategy policy branch is added.
- Single path and config-controlled runtime: PASS. The plan rejects raw cargo bypasses, S3 mutable target cache, and hardcoded thresholds.
- Test-first safety gates: PASS. Future implementation slices require red/green TDD before production code changes.
- Evidence before claims: PASS. Current issue bodies/comments and exact repo head are the evidence base; #286 completion is tied to PR #404 and merge commit `400dac8acc8ec04fc7b4aefc41bab10390d6404f`, while other child fixes remain open.
- Minimal slice discipline: PASS. This PR is a #123 planning/gate slice; child implementation remains one issue per PR.

## Project Structure

### Documentation (this feature)

```text
specs/014-disk-pressure-governance/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
├── checklists/
│   └── requirements.md
└── contracts/
    └── disk-pressure-governance.md
```

### Source Code

```text
AGENTS.md
CLAUDE.md
.specify/feature.json
docs/bolt-v3/
tests/
scripts/
```

**Structure Decision**: Keep the original #123 slice docs-only plus current-feature pointers. Later implementation PRs add tests/scripts only after each owning issue's Phase 1 gate is satisfied; #375 now does so through `specs/024-developer-tool-storage-hygiene/`.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| None | N/A | N/A |

## Phase 0 Research

Output: `research.md`

## Phase 1 Design

Outputs: `data-model.md`, `contracts/disk-pressure-governance.md`, `quickstart.md`

## Post-Design Constitution Check

- NT-first thin layer: PASS.
- Generic core: PASS.
- Single path and config-controlled runtime: PASS.
- Test-first safety gates: PASS.
- Evidence before claims: PASS.
- Minimal slice discipline: PASS.
