# Implementation Plan: CI Nextest Artifact Cache

**Branch**: `codex/ci-195-nextest-artifact-cache` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/007-ci-nextest-artifact-cache/spec.md`

## Summary

Implement #195 on top of the current #332 stacked workflow by changing only the `test` lane cache strategy and its required verifier coverage. Preserve the managed Rust target directory as a rust-cache workspace target, enable workspace-crate artifact preservation for nextest warm reruns, keep bounded per-shard keys, keep rust-environment hashing enabled, and record exact CI evidence as pending until this stacked head can receive full CI.

## Technical Context

**Language/Version**: GitHub Actions YAML, Python 3 standard library verifier tests, Rust toolchain from `rust-toolchain.toml`
**Primary Dependencies**: `Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4`, `cargo-nextest`, managed Rust verification owner, `just`
**Storage**: GitHub Actions cache entries for Cargo registry/git plus managed target directory
**Testing**: `python3 scripts/test_verify_ci_workflow_hygiene.py`, `python3 scripts/verify_ci_workflow_hygiene.py`, `just ci-lint-workflow`, `git diff --check`, exact GitHub Actions cold/warm reruns when available
**Target Platform**: GitHub-hosted `ubuntu-latest` CI runners
**Project Type**: Rust binary repository with CI workflow and verification scripts
**Performance Goals**: Warm reruns of the same sharded nextest graph avoid unnecessary workspace `Compiling bolt-v2` test-profile rebuilds and improve beyond the post-#193/#343 warm baseline
**Constraints**: no runtime Rust behavior changes, no extra cache backend without justification, no unbounded SHA cache keys, no weakening required `test`/`gate` semantics, exact CI proof required before closure, no merge without approval
**Scale/Scope**: One #195 slice: `test` lane cache-artifact preservation plus #195-specific verifier/spec evidence

## Constitution Check

- **NO HARDCODES**: workflow cache key literals are CI topology identifiers, not runtime trading values. The shard count mirrors the #332 workflow contract and is verifier-protected.
- **NO DUAL PATHS**: keep one rust-cache strategy for the `test` lane instead of adding parallel direct `actions/cache` state.
- **NO DEBTS**: exact cold/warm CI evidence remains an explicit blocker, not a hidden TODO.
- **NO CREDENTIAL DISPLAY**: no secret values are read or printed.
- **PURE RUST BINARY / SSM**: unchanged; this is CI-only workflow hygiene.
- **GROUP BY CHANGE**: #195 touches cache-artifact behavior only. Lane topology remains #332; smoke-tag dedup remains #205; docs/pass-stub evidence remains #344.

## Project Structure

### Documentation

```text
specs/007-ci-nextest-artifact-cache/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
└── checklists/
    └── requirements.md
```

### Source Code

```text
.github/
├── actions/setup-environment/action.yml
└── workflows/ci.yml

scripts/
├── test_verify_ci_workflow_hygiene.py
└── verify_ci_workflow_hygiene.py

justfile
```

**Structure Decision**: Keep the existing CI workflow, setup action, Python verifier, and `just ci-lint-workflow` structure. No Rust source or runtime config changes are needed for #195.

## Research Decisions

- Use rust-cache workspace target mapping for the `test` lane instead of opaque `cache-directories` only.
- Add a relative managed target-dir output because rust-cache's `workspaces` target path is joined to the workspace root.
- Set `cache-workspace-crates: "true"` for the sharded `test` lane so workspace test artifacts are not cleaned before save.
- Keep `add-rust-environment-hash-key: "true"` explicit and verifier-protected so real Rust inputs invalidate caches.
- Keep per-shard bounded keys and reject SHA-shaped keys.
- Defer exact CI evidence until a real PR-head cold/warm run exists.

## Complexity Tracking

| Decision | Why Needed | Simpler Alternative Rejected Because |
|----------|------------|--------------------------------------|
| Add `managed_target_dir_relative` setup output | rust-cache `workspaces` target is relative to workspace root | Passing the absolute managed target path through `workspaces` would join it under the workspace path incorrectly |
| Enable `cache-workspace-crates` for `test` only | #195 requires preserving workspace nextest test artifacts | The default rust-cache cleanup removes workspace crates, matching the observed warm rebuild problem |
| Add verifier self-tests for cache semantics | Cache behavior is easy to regress silently in YAML | Relying on reviewer memory would not satisfy #203/#333 defense-in-depth |
