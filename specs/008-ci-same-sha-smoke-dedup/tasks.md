# Tasks: #205 Same-SHA Smoke-Tag Dedup

**Input**: Design artifacts from `specs/008-ci-same-sha-smoke-dedup/`

## Stage 1 - Evidence And Spec

- [x] T001 Re-read live #205, #333, #344, and #340 issue bodies/comments and preserve scope boundaries in `spec.md`.
- [x] T002 Confirm GitHub Actions run/job/artifact and `download-artifact` cross-run inputs from primary sources in `research.md`.
- [x] T003 Create spec-kit artifacts under `specs/008-ci-same-sha-smoke-dedup/`.
- [x] T004 Create requirements checklist in `checklists/requirements.md`.

## Stage 2 - TDD Resolver

**Goal**: Select only exact trusted main-run evidence and fail closed otherwise.

- [x] T005 Add failing resolver self-tests in `scripts/test_find_same_sha_main_evidence.py`.
- [x] T006 Implement `scripts/find_same_sha_main_evidence.py` with run, job, artifact, output, and API logic.
- [x] T007 Verify resolver rejects wrong branch, wrong SHA, wrong workflow path, skipped source-fence, missing test shard, expired artifact, and artifact SHA mismatch.

## Stage 3 - Workflow Hygiene Contract

**Goal**: Make the #205 topology enforceable before changing YAML.

- [x] T008 Extend `scripts/test_verify_ci_workflow_hygiene.py` with #205 tag reuse fixtures and negative cases.
- [x] T009 Extend `scripts/verify_ci_workflow_hygiene.py` for the evidence job, tag skip rules, gate tag mode, deploy artifact-ID download, and required permissions.
- [x] T010 Wire resolver self-tests into `just ci-lint-workflow`.

## Stage 4 - Workflow Implementation

**Goal**: Reuse exact main-run evidence and artifact on smoke tags without weakening PR/main CI.

- [x] T011 Add `actions: read` permissions for workflow metadata and cross-run artifact download.
- [x] T012 Add tag-only `same-sha-main-evidence` job with source run/check/artifact/SHA outputs.
- [x] T013 Skip duplicate heavy lanes on tag refs.
- [x] T014 Update `gate` with explicit tag reuse and normal modes.
- [x] T015 Update `deploy` to require evidence success, log reused evidence, and download by source artifact ID.

## Stage 5 - Verification And Handoff

- [x] T016 Run resolver self-tests, workflow verifier self-tests, live verifier, YAML parse, `just ci-lint-workflow`, `just fmt-check`, and `git diff --check`.
- [x] T017 Inspect final diff for scope boundaries: no #344 pass-stub/docs-only PR, no #340 config relocation, no #195 cache changes beyond existing base, no runtime Rust behavior changes.
- [x] T018 Push draft PR and update PR body with exact local verification plus stacked-CI blocker if applicable.
- [x] T019 Comment on #205 and #333 with implementation state and real after-evidence blocker.
- [ ] T020 After the stack lands, run a real smoke tag and post before/after duplicate `test`/`build` timing evidence before closing #205.
