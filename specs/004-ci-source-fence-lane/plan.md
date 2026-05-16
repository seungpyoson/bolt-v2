# Implementation Plan: CI Source-Fence Lane

**Branch**: `codex/ci-342-source-fence` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/004-ci-source-fence-lane/spec.md`

## Summary

Implement #342 as a direct follow-up to the now-merged #343 baseline. Add a first-class `source-fence` CI job and `just source-fence` recipe that run the Bolt-v3 verifier script set and canonical structural test binaries before full `test`. Extend `gate` and `just ci-lint-workflow` so the new lane is required and fail-closed. Do not implement #332 sharding, #195 artifact retention, #205 deploy deduplication, #335/#344 path-filter work, or #340 config-path migration.

## Technical Context

**Language/Version**: Rust 2024 repository, GitHub Actions, Bash/Just, Python 3 verifier scripts
**Primary Dependencies**: Existing `.github/actions/setup-environment`, `Swatinem/rust-cache`, `just`, managed Rust verification owner
**Storage**: Workflow YAML, `justfile`, verifier scripts, status-map documentation, spec-kit artifacts
**Testing**: TDD red via `just ci-lint-workflow`, local `just source-fence`, targeted verifier scripts, deliberate stale source-fence mutation, `git diff --check`, exact-head CI
**Target Platform**: GitHub Actions `ubuntu-latest`
**Project Type**: Rust live trading binary with CI workflow
**Performance Goals**: Warm `source-fence` lane about 1-2 minutes excluding first-run compile variance; failure must occur before `cargo-nextest` setup/full test execution dominates
**Constraints**: no requirement narrowing, no full test sharding, no path-filter changes, no unpinned Python packages, no raw cargo workflow commands, no merge without approval
**Scale/Scope**: One #342 topology and verifier slice covering source-fence job, recipe, gate, linter, and required missing verifier scripts

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **NT-First Thin Layer**: PASS. The branch adds source verifiers and CI topology only; no runtime trading path is introduced.
- **Generic Core, Concrete Edges**: PASS. The pure-Rust and status-map verifiers protect existing boundary evidence; they do not add provider logic.
- **Single Path And Config-Controlled Runtime**: PASS. The CI lane is a single recipe entrypoint and does not add runtime config paths.
- **Test-First Safety Gates**: PASS. The workflow linter invariant is added and observed failing before the workflow is updated.
- **Evidence Before Claims**: PASS. Acceptance requires local red/green proof and exact-head CI evidence.
- **Minimal Slice Discipline**: PASS. The slice is #342 only and records #332 duplicate-test ownership explicitly.

## Phase 0 Research Summary

Detailed decisions are in [research.md](research.md).

- Use job serialization `test needs: [detector, source-fence]` so a stale source fence blocks full test setup.
- Use one `just source-fence` recipe as the local/CI source of truth.
- Add the two missing verifier scripts instead of deleting them from the #342 contract.
- Keep temporary duplicate execution explicit until #332 changes full nextest ownership.
- Keep Python verifier dependencies deterministic with hashed CI requirements.

## Phase 1 Design Summary

Design details are in [data-model.md](data-model.md) and [quickstart.md](quickstart.md).

Implementation surfaces:

- `.github/workflows/ci.yml`: add `source-fence`, make `test` depend on it, add it to `gate`.
- `justfile`: add `source-fence` recipe and narrow linter invariants for job/gate/test dependencies.
- `scripts/verify_bolt_v3_pure_rust_runtime.py`: new pure-Rust runtime verifier.
- `scripts/verify_bolt_v3_status_map_current.py`: new status-map evidence verifier.
- `scripts/verify_bolt_v3_naming.py`: use PyYAML from the hashed source-fence CI requirements file.
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`: update row 3 to cite the new verifier.

## Project Structure

### Documentation (this feature)

```text
specs/004-ci-source-fence-lane/
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
scripts/verify_bolt_v3_pure_rust_runtime.py
scripts/verify_bolt_v3_status_map_current.py
scripts/verify_bolt_v3_naming.py
docs/bolt-v3/2026-04-28-source-grounded-status-map.md
```

**Structure Decision**: Keep the lane behind `just source-fence` so CI and local verification share one command, and keep linter invariants close to the existing `ci-lint-workflow` mechanism because #203 generic cleanup has not landed.

## Complexity Tracking

No constitution violations.
