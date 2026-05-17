# Tasks: Production Live Readiness

**Input**: Design documents from `/specs/013-production-live-readiness/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: TDD required for the artifact-surface guard.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Phase 1: Setup

**Purpose**: Create Issue #369 SpecKit structure and pointer.

- [x] T001 Create `specs/013-production-live-readiness/` artifact structure.
- [x] T002 Update `.specify/feature.json` to point at `specs/013-production-live-readiness`.
- [x] T003 Update `AGENTS.md` Speckit plan reference to `specs/013-production-live-readiness/plan.md`.

---

## Phase 2: Foundational

**Purpose**: Lock the required artifact surface before filling docs.

- [x] T004 [P] Add failing artifact-surface test in `tests/bolt_v3_production_readiness_contract.rs`.
- [x] T005 [P] Record red test output for missing `specs/013-production-live-readiness/spec.md`.

---

## Phase 3: User Story 1 - Prevent Overstated Readiness Claims (Priority: P1)

**Goal**: Define and enforce claim levels so tiny canary is not production readiness.

**Independent Test**: `cargo test --test bolt_v3_production_readiness_contract -- --nocapture`

- [x] T006 [US1] Add `docs/bolt-v3/2026-05-18-production-readiness-contract.md`.
- [x] T007 [US1] Link readiness contract from `docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md`.
- [x] T008 [US1] Link readiness contract from row 48 and authority list in `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`.
- [x] T009 [US1] Fill `specs/013-production-live-readiness/spec.md`.

---

## Phase 4: User Story 2 - Gate Repeated Live Operation (Priority: P2)

**Goal**: Name repeated-live runbooks and required tests/tooling.

**Independent Test**: Contract and SpecKit artifact test names runbooks and test/tooling gates.

- [x] T010 [US2] Fill `specs/013-production-live-readiness/contracts/production-readiness.md`.
- [x] T011 [US2] Fill `specs/013-production-live-readiness/data-model.md`.
- [x] T012 [US2] Fill `specs/013-production-live-readiness/quickstart.md`.

---

## Phase 5: User Story 3 - Tie Production Claims To Deploy Provenance (Priority: P3)

**Goal**: Require deploy provenance and invalid-evidence rules before production claims.

**Independent Test**: Spec and contract define deploy provenance and invalid-evidence blockers.

- [x] T013 [US3] Fill `specs/013-production-live-readiness/research.md`.
- [x] T014 [US3] Fill `specs/013-production-live-readiness/plan.md`.
- [x] T015 [US3] Fill `specs/013-production-live-readiness/checklists/requirements.md`.

---

## Phase 6: Verification

**Purpose**: Prove docs and test gate satisfy Issue #369 readiness-definition scope.

- [x] T016 Run `cargo test --test bolt_v3_production_readiness_contract -- --nocapture`.
- [x] T017 Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`.
- [x] T018 Run `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`.
- [x] T019 Run `cargo fmt --check`.
- [x] T020 Run `git diff --check`.
- [x] T021 Run placeholder/debt scan over changed docs.
- [x] T022 Run `no-mistakes status`.

## Dependencies & Execution Order

- Phase 1 before all other phases.
- Phase 2 before docs are considered complete.
- US1 before US2/US3 because claim-level language is the base contract.
- US2 and US3 can be reviewed independently after US1.
- Phase 6 after all docs/tests are present.

## Implementation Strategy

MVP is US1 plus the artifact-surface test. US2 and US3 complete the Issue #369 acceptance criteria by adding runbook, tests/tooling, monitoring, alerting, deploy provenance, and invalid-evidence requirements.
