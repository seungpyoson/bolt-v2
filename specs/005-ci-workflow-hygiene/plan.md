# Implementation Plan: CI Workflow Hygiene

**Branch**: `codex/ci-250-build-tool-cache` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/005-ci-workflow-hygiene/spec.md`

## Summary

Extend the #203 workflow hygiene verifier for #250 priority item 1 on top of landed #342 and #332 topology. Keep the existing deterministic, standard-library verifier and `just ci-lint-workflow` wiring, and add the prebuilt CI build-tool install contract so required PR lanes and scheduled advisory checks cannot regress to source-building `cargo-deny`, `cargo-nextest`, or `cargo-zigbuild`. Keep the slice limited to current workflow hygiene/tool-install surfaces and explicitly leave #195, #205, #344, and #340 to their own issues.

## Technical Context

**Language/Version**: Rust 2024 repository, GitHub Actions YAML, Bash/Just, Python 3 standard library
**Primary Dependencies**: Existing `.github/actions/setup-environment`, `just`, managed Rust verification owner, GitHub Actions `needs`/`gate` semantics
**Storage**: Workflow YAML, composite action YAML, justfile recipe, verifier scripts, spec-kit docs
**Testing**: TDD with `scripts/test_verify_ci_workflow_hygiene.py`, `scripts/verify_ci_workflow_hygiene.py`, `just ci-lint-workflow`, `just fmt-check`, `git diff --check`, exact-head CI
**Target Platform**: GitHub Actions `ubuntu-latest`
**Project Type**: Rust live trading binary with CI workflow automation
**Performance Goals**: Remove detector serialization from `fmt-check`; avoid managed target-dir resolution in non-target-cache lanes; preserve fail-closed merge/deploy gate semantics; prevent CI regressions back to source-building `cargo-deny`, `cargo-nextest`, or `cargo-zigbuild`
**Constraints**: no new unpinned dependencies, no runtime behavior changes, preserve the landed #332 sharded-test/check-aarch64 topology without adding new sharding behavior, no #195 artifact retention, no #205 deploy reuse, no #344 pass-stub, no #340 config relocation, no merge without approval
**Scale/Scope**: One #250 priority-item-1 verifier extension to the #203 workflow hygiene spec, on top of landed #342 source-fence and landed #332 sharded-test/check-aarch64 topology

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **NT-First Thin Layer**: PASS. CI workflow hygiene does not alter runtime trading behavior or NT surfaces.
- **Generic Core, Concrete Edges**: PASS. No provider, market, strategy, or adapter code changes.
- **Single Path And Config-Controlled Runtime**: PASS. Runtime config and secret path remain unchanged; managed Rust owner remains the one CI Rust path.
- **Test-First Safety Gates**: PASS. The verifier is introduced through failing self-tests before workflow changes are accepted.
- **Evidence Before Claims**: PASS. Exact current issue body, stacked base SHA, local lint output, and exact-head CI are required evidence.
- **Minimal Slice Discipline**: PASS. The primary slice is #203. Landed #332 topology is treated as a base invariant; residual #195/#205/#344/#340 surfaces remain out of scope.

## Phase 0 Research Summary

Detailed decisions are in [research.md](research.md).

- Stack on `origin/codex/ci-342-source-fence` because #342 topology is active for #203.
- Add a standard-library verifier instead of expanding opaque awk-only checks.
- Remove only `fmt-check` detector dependency; keep build detector gating and #342 source-fence ordering.
- Make managed target-dir resolution opt-in because only target-cache jobs use it.
- Add direct deploy needs while retaining the aggregate `gate`.
- Enforce only the landed #332 shard/check-aarch64 topology; do not pre-lint absent #205/#344 topology.
- Enforce pinned prebuilt installs for CI Rust helper tools: `taiki-e/install-action` for cargo-deny/nextest and an in-repo SHA256-checked cargo-zigbuild archive.

## Phase 1 Design Summary

Design details are in [data-model.md](data-model.md) and [quickstart.md](quickstart.md).

Implementation surfaces:

- `scripts/test_verify_ci_workflow_hygiene.py`: self-tests for missing job, needs, gate result, deploy direct needs, and target-dir opt-in failures.
- `scripts/verify_ci_workflow_hygiene.py`: parser and invariant verifier.
- `justfile`: run the self-tests and verifier inside `ci-lint-workflow`; keep the pinned cargo-zigbuild Linux x86_64 archive SHA256 beside the pinned tool version.
- `.github/actions/setup-environment/action.yml`: add target-dir opt-in and export the cargo-zigbuild archive SHA256 under `include-build-values`.
- `.github/workflows/ci.yml`: remove `fmt-check needs: detector`, set target-dir opt-ins, add deploy direct needs, and use prebuilt Rust helper-tool installs.
- `.github/workflows/advisory.yml`: use the same pinned prebuilt `cargo-deny` install path for the scheduled advisory job.

Explicitly out-of-scope surfaces:

- `.config/nextest.toml` and LiveNode-heavy test serialization remain owned by landed #332 and are not changed in this slice.
- Source-fence verifier internals remain #342 scope; this issue only lints the #342 job name, ordering, and gate dependency.

## Project Structure

### Documentation

```text
specs/005-ci-workflow-hygiene/
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
.github/actions/setup-environment/action.yml
.github/workflows/advisory.yml
.github/workflows/ci.yml
justfile
scripts/test_verify_ci_workflow_hygiene.py
scripts/verify_ci_workflow_hygiene.py
```

**Structure Decision**: Keep the new verifier in `scripts/` beside other repo-local verifiers and keep the entrypoint inside `just ci-lint-workflow` so local and CI checks share one command.

## Complexity Tracking

No constitution violations.
