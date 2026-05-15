# Tasks: #344 Residual Minute-Consumption Work

**Input**: Design artifacts from `specs/009-ci-residual-minute-work/`

## Stage 1 - Evidence And Spec

- [x] T001 Re-read live #344, #335, #333, and #340 issue bodies/comments.
- [x] T002 Confirm `main` required status checks are currently null.
- [x] T003 Create spec-kit artifacts under `specs/009-ci-residual-minute-work/`.
- [x] T004 Create requirements checklist in `checklists/requirements.md`.

## Stage 2 - TDD Path Classifier

- [x] T005 Add failing path-filter classifier self-tests in `scripts/test_verify_ci_path_filters.py`.
- [x] T006 Implement `scripts/verify_ci_path_filters.py` for safe-pattern extraction, changed-file classification, docs row validation, and pass-stub workflow validation.
- [x] T007 Wire path-filter verifier into `just ci-lint-workflow`.

## Stage 3 - Docs And Pass-Stub

- [x] T008 Add `docs/ci/paths-ignore-behavior.md`.
- [x] T009 Add `.github/workflows/ci-docs-pass-stub.yml`.
- [x] T010 Extend CI path-filter verifier/self-tests for pass-stub job name and wiring.

## Stage 4 - Branch Hygiene

- [x] T011 Generate branch inventory artifact in `docs/ci/branch-hygiene-2026-05-15.md`.
- [x] T012 Comment on #344 with every non-`main` branch classification and no deletion action.

## Stage 5 - Verification And Handoff

- [x] T013 Run path-filter tests/verifier, workflow tests/verifier, YAML parse, `just ci-lint-workflow`, `just fmt-check`, and `git diff --check`.
- [x] T014 Push draft PR stacked on #350.
- [x] T015 Comment on #333/#344 with completed work and blocked evidence.
- [ ] T016 After stack lands, open docs-only throwaway PR and post real run evidence.
- [ ] T017 After #332/#195/#205 land, post monthly Actions minute rebaseline.
