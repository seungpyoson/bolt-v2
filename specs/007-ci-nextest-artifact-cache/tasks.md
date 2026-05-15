# Tasks: CI Nextest Artifact Cache

**Input**: Design artifacts from `specs/007-ci-nextest-artifact-cache/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `quickstart.md`

## Setup

- [x] T001 Fetch current #195, #333, and #332 issue bodies/comments and map every requirement into `spec.md`
- [x] T002 Research primary docs/source for rust-cache workspace targets, workspace crate cleanup, rust-environment hash keys, nextest partitioning, and GitHub cache limits
- [x] T003 Create #195 spec-kit artifacts under `specs/007-ci-nextest-artifact-cache/`

## Tests First

**Goal**: Add failing verifier coverage before workflow changes.

- [x] T004 Add verifier self-test fixture requiring `managed_target_dir_relative` setup output
- [x] T005 Add verifier self-test fixture rejecting test cache `cache-directories` without `workspaces`
- [x] T006 Add verifier self-test fixture requiring `cache-workspace-crates: "true"` for `test`
- [x] T007 Add verifier self-test fixture requiring `add-rust-environment-hash-key: "true"` for `test`
- [x] T008 Add verifier self-test fixture rejecting `github.sha` or equivalent SHA-shaped cache keying in `test`

## Implementation

**Goal**: Preserve workspace nextest artifacts in the sharded managed target cache without weakening test execution.

- [x] T009 Add `managed_target_dir_relative` output to `.github/actions/setup-environment/action.yml`
- [x] T010 Change only the `test` rust-cache step in `.github/workflows/ci.yml` to use `workspaces: . -> ${{ steps.setup.outputs.managed_target_dir_relative }}`
- [x] T011 Set `cache-targets: true`, `cache-workspace-crates: "true"`, and `add-rust-environment-hash-key: "true"` on the `test` cache step
- [x] T012 Preserve existing per-shard bounded key `nextest-v3-shard-${{ matrix.shard }}-of-4`
- [x] T013 Preserve `test` execution command, `source-fence` dependency, and aggregate `gate` result checks unchanged

## Verifier

**Goal**: Make `just ci-lint-workflow` guard the #195 cache contract.

- [x] T014 Implement verifier checks for setup relative target-dir export
- [x] T015 Implement verifier checks for `test` workspace target mapping and absence of opaque `cache-directories`
- [x] T016 Implement verifier checks for workspace crate preservation and rust-environment hash enablement
- [x] T017 Implement verifier checks rejecting unbounded SHA cache key dimensions
- [x] T018 Update `justfile` action literal/output drift checks for `managed_target_dir_relative`

## Validation

- [x] T019 Run `python3 scripts/test_verify_ci_workflow_hygiene.py`
- [x] T020 Run `python3 scripts/verify_ci_workflow_hygiene.py`
- [x] T021 Run `just ci-lint-workflow`
- [x] T022 Run `git diff --check`
- [x] T023 Inspect final diff for scope boundaries: no #205, #344, #340, or generic #203 work

## Evidence And Handoff

- [x] T024 Record exact current head/base and local verification in PR/issue notes
- [x] T025 Mark exact cold/warm CI evidence as blocked if stacked PR CI still does not run full `pull_request`
- [ ] T026 After exact CI is available, record cold/warm run IDs, log excerpts, cache sizes, timings, and warm `Compiling bolt-v2` finding
- [ ] T027 Request external reviews only after exact PR-head CI is green, unless the user explicitly waives the repo rule
