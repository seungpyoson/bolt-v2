# Implementation Plan: CI Parallel Heavy Lanes

**Branch**: `codex/ci-332-parallel-heavy-lanes` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/006-ci-parallel-heavy-lanes/spec.md`

## Summary

Implement #332 as a stacked follow-up to #342 and #203. Split the serialized `clippy` lane into top-level host `clippy` and `check-aarch64` jobs, shard the full nextest lane four ways through the existing managed `just test` path, preserve one fail-closed aggregate gate, and extend workflow lint only for the new #332 topology. Source-fence filters remain intentionally duplicated in full nextest for this slice, with `source-fence` still required before `test`; this preserves coverage without adding brittle exclusion filters.

## Technical Context

**Language/Version**: Rust 2024 repository, GitHub Actions YAML, Bash/Just, Python 3 standard library
**Primary Dependencies**: Existing `.github/actions/setup-environment`, `just`, `cargo-nextest`, managed Rust verification owner, GitHub Actions matrix and `needs` semantics
**Storage**: Workflow YAML, justfile recipe, verifier scripts, spec-kit docs
**Testing**: TDD with `scripts/test_verify_ci_workflow_hygiene.py`, `scripts/verify_ci_workflow_hygiene.py`, `just ci-lint-workflow`, `just test -- --partition count:1/4` dry-run/managed-path evidence where feasible, `git diff --check`, exact-head CI when the stacked PR can run it
**Target Platform**: GitHub Actions `ubuntu-latest`
**Project Type**: Rust live trading binary with CI workflow automation
**Performance Goals**: Reduce PR critical path from the #343 baseline `clippy` wall time of about 11 minutes toward 5-6 minutes by parallelizing host clippy/aarch64 and sharding full tests
**Constraints**: no new unpinned dependencies, no runtime behavior changes, no raw cargo workflow commands, no #195 cache-retention implementation, no #205 deploy dedup, no #344 pass-stub/evidence work, no #340 config relocation, no generic #203 cleanup beyond #332-specific lint extension, no merge without approval
**Scale/Scope**: One #332 heavy-lane topology slice on top of current #342/#203 stacked CI topology

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **NT-First Thin Layer**: PASS. CI topology changes do not alter runtime trading behavior or NT surfaces.
- **Generic Core, Concrete Edges**: PASS. No provider, market, strategy, or adapter code changes.
- **Single Path And Config-Controlled Runtime**: PASS. Workflow YAML still calls repo `just` recipes; managed Rust owner remains the one Rust verification path.
- **Test-First Safety Gates**: PASS. New topology invariants start as failing verifier self-tests.
- **Evidence Before Claims**: PASS. Issue #332 body/comment, #333 epic acceptance, #343 baseline, local lint, and exact-head CI are required evidence surfaces.
- **Minimal Slice Discipline**: PASS. The slice includes all #332 requirements and explicitly excludes #195/#205/#344/#340/generic #203 work.

## Phase 0 Research Summary

Detailed decisions are in [research.md](research.md).

- Keep #332 stacked on the current #203 branch because #342 source-fence and #203 verifier topology are prerequisites.
- Use the issue-requested `--partition count:${{ matrix.shard }}/4` even though nextest recommends `slice:` for new designs; the issue body is the contract and no strong reason exists to deviate.
- Set `strategy.fail-fast: false` so all shard results exist for the aggregate gate.
- Preserve the managed `just test` path by adding passthrough args instead of adding workflow raw cargo.
- Intentionally duplicate #342 source-fence filters inside full nextest for this slice, with explicit documentation and one aggregate gate.
- Add shard-aware but bounded cache keys: independent host clippy/aarch64 keys and a fixed four-shard nextest key dimension.

## Phase 1 Design Summary

Design details are in [data-model.md](data-model.md) and [quickstart.md](quickstart.md).

Implementation surfaces:

- `.github/workflows/ci.yml`: split `check-aarch64`, add four-shard `test` matrix, add shard reproduction log, update gate/deploy needs.
- `justfile`: add `test`/`managed-test` passthrough and extend `ci-lint-workflow` for the #332 topology.
- `scripts/test_verify_ci_workflow_hygiene.py`: add negative fixture coverage for missing `check-aarch64`, missing matrix/partition/fail-fast/reproduction command, and missing gate checks.
- `scripts/verify_ci_workflow_hygiene.py`: enforce the #332 topology with standard-library parsing.
- `specs/006-ci-parallel-heavy-lanes/`: capture plan, research, model, quickstart, checklist, and tasks.

## Project Structure

### Documentation

```text
specs/006-ci-parallel-heavy-lanes/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
└── checklists/
    └── requirements.md
```

### Source Code And Automation

```text
.github/workflows/ci.yml
justfile
scripts/test_verify_ci_workflow_hygiene.py
scripts/verify_ci_workflow_hygiene.py
```

**Structure Decision**: Keep topology verification in the existing workflow hygiene verifier and `just ci-lint-workflow` path so #203 remains the generic home and #332 contributes only the specific new topology requirements.

## Complexity Tracking

No constitution violations.
