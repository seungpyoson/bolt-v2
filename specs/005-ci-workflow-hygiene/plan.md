# Implementation Plan: CI Workflow Hygiene

**Branch**: `codex/ci-203-workflow-hygiene` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/005-ci-workflow-hygiene/spec.md`

## Summary

Implement #203 as a stacked follow-up to #342. Add a deterministic, standard-library workflow hygiene verifier with self-tests; wire it into `just ci-lint-workflow`; remove unnecessary `fmt-check` detector serialization; make managed target-dir setup opt-in; and add direct deploy needs for defense-in-depth. Keep the slice limited to current #203 workflow hygiene surfaces and explicitly leave #332, #195, #205, #344, and #340 to their own issues.

## Technical Context

**Language/Version**: Rust 2024 repository, GitHub Actions YAML, Bash/Just, Python 3 standard library
**Primary Dependencies**: Existing `.github/actions/setup-environment`, `just`, managed Rust verification owner, GitHub Actions `needs`/`gate` semantics
**Storage**: Workflow YAML, composite action YAML, justfile recipe, verifier scripts, spec-kit docs
**Testing**: TDD with `scripts/test_verify_ci_workflow_hygiene.py`, `scripts/verify_ci_workflow_hygiene.py`, `just ci-lint-workflow`, `just fmt-check`, `git diff --check`, exact-head CI
**Target Platform**: GitHub Actions `ubuntu-latest`
**Project Type**: Rust live trading binary with CI workflow automation
**Performance Goals**: Remove detector serialization from `fmt-check`; avoid managed target-dir resolution in non-target-cache lanes; preserve fail-closed merge/deploy gate semantics
**Constraints**: no new unpinned dependencies, no runtime behavior changes, no #332 sharding, no #195 artifact retention, no #205 deploy reuse, no #344 pass-stub, no #340 config relocation, no merge without approval
**Scale/Scope**: One #203 workflow hygiene slice on top of #342 source-fence topology

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **NT-First Thin Layer**: PASS. CI workflow hygiene does not alter runtime trading behavior or NT surfaces.
- **Generic Core, Concrete Edges**: PASS. No provider, market, strategy, or adapter code changes.
- **Single Path And Config-Controlled Runtime**: PASS. Runtime config and secret path remain unchanged; managed Rust owner remains the one CI Rust path.
- **Test-First Safety Gates**: PASS. The verifier is introduced through failing self-tests before workflow changes are accepted; accepted verification-support co-scope carries focused tests or stabilizes existing no-mistakes/full-cargo test execution.
- **Evidence Before Claims**: PASS. Exact current issue body, stacked base SHA, local lint output, and exact-head CI are required evidence.
- **Minimal Slice Discipline**: PASS. The primary slice is #203. The only accepted co-scope is verification support for LiveNode-heavy test serialization and pure-Rust source-fence verifier alias detection; residual #332/#195/#205/#344/#340 surfaces remain out of scope.

## Phase 0 Research Summary

Detailed decisions are in [research.md](research.md).

- Stack on `origin/codex/ci-342-source-fence` because #342 topology is active for #203.
- Add a standard-library verifier instead of expanding opaque awk-only checks.
- Remove only `fmt-check` detector dependency; keep build detector gating and #342 source-fence ordering.
- Make managed target-dir resolution opt-in because only target-cache jobs use it.
- Add direct deploy needs while retaining the aggregate `gate`.
- Do not pre-lint absent #332/#205/#344 topology.

## Phase 1 Design Summary

Design details are in [data-model.md](data-model.md) and [quickstart.md](quickstart.md).

Implementation surfaces:

- `scripts/test_verify_ci_workflow_hygiene.py`: self-tests for missing job, needs, gate result, deploy direct needs, and target-dir opt-in failures.
- `scripts/verify_ci_workflow_hygiene.py`: parser and invariant verifier.
- `justfile`: run the self-tests and verifier inside `ci-lint-workflow`.
- `.github/actions/setup-environment/action.yml`: add target-dir opt-in.
- `.github/workflows/ci.yml`: remove `fmt-check needs: detector`, set target-dir opt-ins, and add deploy direct needs.

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
.github/workflows/ci.yml
justfile
scripts/test_verify_ci_workflow_hygiene.py
scripts/verify_ci_workflow_hygiene.py
```

**Structure Decision**: Keep the new verifier in `scripts/` beside other repo-local verifiers and keep the entrypoint inside `just ci-lint-workflow` so local and CI checks share one command.

## Complexity Tracking

No constitution violations.
