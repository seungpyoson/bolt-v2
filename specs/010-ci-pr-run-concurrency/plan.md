# Implementation Plan: CI PR Run Concurrency

**Branch**: `codex/ci-355-pr-concurrency` | **Date**: 2026-05-17 | **Spec**: `specs/010-ci-pr-run-concurrency/spec.md`
**Input**: Feature specification from `specs/010-ci-pr-run-concurrency/spec.md`

## Summary

Guard the existing #355 CI concurrency policy with workflow hygiene verifier coverage and record hard GitHub Actions evidence that superseded pull-request runs cancel while non-PR flows remain isolated by ref and SHA.

## Technical Context

**Language/Version**: Python 3 standard library; GitHub Actions YAML text
**Primary Dependencies**: Existing `scripts/verify_ci_workflow_hygiene.py`, `scripts/test_verify_ci_workflow_hygiene.py`, `just ci-lint-workflow`
**Storage**: GitHub issue/PR evidence only
**Testing**: TDD with Python verifier self-tests, verifier run, `just ci-lint-workflow`, exact-head PR CI
**Target Platform**: GitHub Actions CI for `bolt-v2`
**Project Type**: CI workflow/verifier maintenance
**Performance Goals**: Avoid wasted Actions minutes from obsolete PR-head runs
**Constraints**: No new dependencies; do not weaken main/tag/deploy/manual semantics; preserve aggregate gate fail-closed behavior
**Scale/Scope**: One CI workflow policy and one verifier/test path for issue #355

## Constitution Check

PASS. This slice uses one workflow policy, one verifier path, no new dependencies, no credentials, and no runtime binary changes.

## Project Structure

### Documentation

```text
specs/010-ci-pr-run-concurrency/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
└── checklists/requirements.md
```

### Source Code

```text
.github/workflows/ci.yml
scripts/verify_ci_workflow_hygiene.py
scripts/test_verify_ci_workflow_hygiene.py
```

**Structure Decision**: Keep the policy in the existing top-level CI workflow and guard it from the existing workflow hygiene verifier.

## Complexity Tracking

No constitution violations.
