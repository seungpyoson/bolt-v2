# Tasks: Bolt-v3 Nucleus Admission Audit

**Input**: Design documents from `specs/001-v3-nucleus-admission/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`, `quickstart.md`
**Tests**: Required by FR-009 and the constitution. Write tests before implementation.

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Establish the audit files without touching production runtime code.

- [X] T001 Create `scripts/test_verify_bolt_v3_nucleus_admission.py` with a minimal subprocess/import harness for the future verifier.
- [X] T002 Create `scripts/verify_bolt_v3_nucleus_admission.py` with CLI argument parsing for default mode, `--strict`, and `--repo-root`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Define the audit model and scan universe before blocker logic.

- [X] T003 Add tests for `AdmissionAuditRun`, `AdmissionBlocker`, `EvidenceRecord`, and `Waiver` validation in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T004 Add tests proving the scan universe includes V3 source, V3 tests, V3 fixtures, V3 docs when present, verifier scripts, `justfile`, and CI workflow files in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T005 Implement audit data classes, deterministic ordering, waiver validation, and secret-safe evidence excerpts in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T006 Implement scan-universe discovery and UTF-8 skip reporting in `scripts/verify_bolt_v3_nucleus_admission.py`.

**Checkpoint**: The verifier can scan the repository and produce an empty deterministic report model.

---

## Phase 3: User Story 1 - See Current Nucleus Blockers (Priority: P1) MVP

**Goal**: Report current admission blockers with evidence while exiting successfully by default.

**Independent Test**: Run `python3 scripts/verify_bolt_v3_nucleus_admission.py` on current `main` and observe a successful exit with a blocked verdict.

### Tests for User Story 1

- [X] T007 Add tests for `generic-contract-leak` detection using current updown plan and clock evidence in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T008 Add tests for `missing-contract-surface` detection for decision-event, conformance, and BacktestEngine/live parity surfaces in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T009 Add tests for `unowned-runtime-default`, `narrow-verifier-bypass`, and fixture-fencing blocker reporting in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T010 Add tests that default mode exits successfully while reporting blockers in `scripts/test_verify_bolt_v3_nucleus_admission.py`.

### Implementation for User Story 1

- [X] T011 Implement `generic-contract-leak` detection in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T012 Implement `missing-contract-surface` absence checks in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T013 Implement `unowned-runtime-default`, `narrow-verifier-bypass`, and `unfenced-concrete-fixture` detection in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T014 Implement deterministic text report output matching `specs/001-v3-nucleus-admission/contracts/admission-report.md` in `scripts/verify_bolt_v3_nucleus_admission.py`.

**Checkpoint**: User Story 1 is complete when the audit reports the current V3 admission blockers and exits 0 by default.

---

## Phase 4: User Story 2 - Fail Strictly When Admission Is Blocked (Priority: P2)

**Goal**: Provide strict mode for future CI promotion without wiring it into required CI yet.

**Independent Test**: Run `python3 scripts/verify_bolt_v3_nucleus_admission.py --strict` on current `main` and observe a nonzero exit with the same blocker report.

### Tests for User Story 2

- [X] T015 Add tests that strict mode exits nonzero when blockers exist in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T016 Add tests that strict mode exits nonzero when the scan universe is unproven in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T017 Add tests that strict mode exits zero against a temporary admitted fixture repository in `scripts/test_verify_bolt_v3_nucleus_admission.py`.

### Implementation for User Story 2

- [X] T018 Implement strict-mode exit status handling in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T019 Add `verify-bolt-v3-nucleus-admission` recipe to `justfile` in report-only mode only.
- [X] T020 Document strict CI promotion as a follow-up in `specs/001-v3-nucleus-admission/quickstart.md` without wiring strict mode into `fmt-check`.

**Checkpoint**: User Story 2 is complete when report-only and strict modes differ only by exit status policy.

---

## Phase 5: User Story 3 - Prove The Audit Cannot Be Fooled By A Narrow Scan (Priority: P3)

**Goal**: Make the audit reviewable by proving failure fixtures, allowed contexts, scan coverage, and waiver validation.

**Independent Test**: Run `python3 scripts/test_verify_bolt_v3_nucleus_admission.py` and inspect tests for both failing and allowed fixtures.

### Tests for User Story 3

- [X] T021 Add positive failing fixture tests for each blocker class in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T022 Add allowed-context fixture tests for provider-owned bindings, fenced fixtures, and documentation evidence in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T023 Add invalid-waiver tests for missing path, excerpt, blocker id, rationale, and retirement issue in `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T024 Add a test proving existing provider-leak allowlists cannot suppress nucleus admission blockers in `scripts/test_verify_bolt_v3_nucleus_admission.py`.

### Implementation for User Story 3

- [X] T025 Implement fixture classification and allowed-context policy in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T026 Implement waiver parsing and validation in `scripts/verify_bolt_v3_nucleus_admission.py`.
- [X] T027 Implement self-test fixture helpers without creating persistent repository artifacts outside test temp directories in `scripts/test_verify_bolt_v3_nucleus_admission.py`.

**Checkpoint**: User Story 3 is complete when the verifier has positive and negative tests for every blocker class.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Validate the slice and prepare review.

- [X] T028 Run `python3 scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T029 Run `python3 scripts/verify_bolt_v3_nucleus_admission.py` and capture the report-only blocker summary.
- [X] T030 Run `python3 scripts/verify_bolt_v3_nucleus_admission.py --strict` and confirm it exits nonzero on current blockers.
- [X] T031 Run `just verify-bolt-v3-nucleus-admission`.
- [X] T032 Run a placeholder/debt scan over `specs/001-v3-nucleus-admission/`, `.specify/memory/constitution.md`, `scripts/verify_bolt_v3_nucleus_admission.py`, and `scripts/test_verify_bolt_v3_nucleus_admission.py`.
- [X] T033 Run the existing Bolt-v3 verifier lane with `just verify-bolt-v3-runtime-literals` and `just verify-bolt-v3-provider-leaks`.
- [X] T034 Confirm the final diff does not modify production runtime behavior; `src/` changes are test-gated only.

---

## Dependencies & Execution Order

### Phase Dependencies

- Phase 1 has no dependencies.
- Phase 2 depends on Phase 1 and blocks all user stories.
- User Story 1 depends on Phase 2 and is the MVP.
- User Story 2 depends on User Story 1.
- User Story 3 depends on User Story 1 and may proceed in parallel with User Story 2 after the shared blocker model exists.
- Polish depends on selected user stories being complete.

### Parallel Opportunities

Parallelism is intentionally limited because the feature has one verifier file
and one test file. After T005 and T006 exist, User Story 2 tests and User Story
3 fixture tests can be drafted in parallel if different agents coordinate on
non-overlapping sections of `scripts/test_verify_bolt_v3_nucleus_admission.py`.

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1.
2. Complete Phase 2.
3. Complete User Story 1.
4. Stop and validate report-only output on current `main`.

### Full Slice

After MVP validation, add strict-mode exit policy, allowed-context fixtures,
waiver validation, the report-only `just` recipe, and final verification.
