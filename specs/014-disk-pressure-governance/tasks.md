# Tasks: Disk Pressure Governance

**Input**: Design documents from `/specs/014-disk-pressure-governance/`
**Prerequisites**: spec.md, plan.md, research.md, data-model.md, contracts/

**Tests**: TDD required for every implementation slice. This planning slice does not add runtime code.

**Organization**: Tasks are grouped by independently reviewable issue/story slices.

## Phase 1: Setup

**Purpose**: Create #123 Speckit structure and point current feature metadata at it.

- [x] T001 Create `specs/014-disk-pressure-governance/` artifact structure.
- [x] T002 Update `.specify/feature.json` to `specs/014-disk-pressure-governance`.
- [x] T003 Update `AGENTS.md` and `CLAUDE.md` Speckit plan references to `specs/014-disk-pressure-governance/plan.md`.

---

## Phase 2: Foundational

**Purpose**: Preserve current live issue evidence and prevent scope collapse.

- [x] T004 Record live #123 child issue map in `specs/014-disk-pressure-governance/contracts/disk-pressure-governance.md`.
- [x] T005 Record local-vs-CI Rust verification policy in `specs/014-disk-pressure-governance/quickstart.md`.
- [x] T006 Record verified no-mistakes raw-Cargo evidence in `specs/014-disk-pressure-governance/research.md`.
- [x] T007 Run `/speckit-analyze` after tasks exist and resolve any critical findings before implementation; see `checklists/requirements.md`.

---

## Phase 3: User Story 1 - Classify Disk Growth Before Acting (Priority: P1)

**Goal**: Operator can map large paths to owner issue and allowed action.

**Independent Test**: Requirements and quickstart classify every known #123 path family and out-of-scope class.

- [x] T008 [US1] Add known path-family table in `specs/014-disk-pressure-governance/quickstart.md`.
- [x] T009 [US1] Add issue-to-PR map in `specs/014-disk-pressure-governance/contracts/disk-pressure-governance.md`.
- [x] T010 [US1] Verify #48/#70/#124/#125 are not treated as direct bolt-v2 implementation PRs in `specs/014-disk-pressure-governance/spec.md`.

---

## Phase 4: User Story 2 - Prevent Unmanaged Rust Artifacts (Priority: P1)

**Goal**: Prepare #374 implementation without speculative shim design.

**Independent Test**: #374 cannot proceed until Phase 1 MECE cargo invocation enumeration is present and reviewed.

- [x] T011 [US2] Draft #374 Phase 1 cargo invocation enumeration in a follow-up #374 branch/PR.
- [x] T012 [US2] Include no-mistakes command/env behavior, worktree-local target proof, #404 wrapper residuals, and live #374 body pin/link evidence in #374 enumeration.
- [x] T013 [US2] Add failing verifier/test for #374 selected implementation seam before changing wrapper behavior.
- [x] T014 [US2] Add verifier coverage that blocks no-mistakes raw-Cargo drift and rejects any S3 active-target-cache path.
- [x] T015 [US2] Implement the scoped #374 wrapper/verifier slice after T011/T012/T013/T014 and local review; Phase 7 PR/CI/external gates remain tracked in T027-T032.

Explicit #374 residuals from #286 / PR #404 review:

- `env -iuLD_PRELOAD cargo build`
- `rustup run stable -- -- cargo build`
- depth-cap observability for deeply wrapped process detection
- wrapper inventory for `timeout`, `xargs`, `setsid`, `taskset`, `ionice`, `chrt`, `make`, `python -c` / `os.system(...)`, and symlink-renamed `cargo` or `rustc`
- destructive managed cargo subcommands, especially `cargo clean`, and required exclusive cache-clean/cache-reset behavior

---

## Phase 5: User Story 3 - Right-Size Known Caches And Logs (Priority: P2)

**Goal**: Preserve #286 completion evidence and prepare #375 retention/hygiene work.

**Independent Test**: Dry-run status/prune requirements protect active work and pinned/current toolchains.

- [x] T016 [US3] Record #286 managed-cache status/prune completion by PR #404 at merge commit `400dac8acc8ec04fc7b4aefc41bab10390d6404f`.
- [x] T017 [US3] Record #286 verifier coverage as completed by PR #404; residual wrapper inventory remains in #374.
- [ ] T018 [US3] Draft #375 developer-tool enumeration before rotation/TTL/toolchain code.
- [ ] T019 [US3] Add failing #375 hygiene verifier before implementation.

---

## Phase 6: User Story 4 - Cover Unmeasured And Unknown Consumers (Priority: P3)

**Goal**: Prepare #376 and #377 without duplicating known-class enforcement.

**Independent Test**: Inventory and detection requirements define paths, growth rates, owner policy, and failure modes.

- [ ] T020 [US4] Create #376 inventory doc with representative measurement procedure.
- [ ] T021 [US4] Draft #377 detection-surface enumeration and baseline definition.
- [ ] T022 [US4] Add failing #377 synthetic-large-dir detector test before implementation.

---

## Phase 7: Verification And Review Gates

**Purpose**: Prove any implementation PR is ready for external review, not merely locally plausible.

- [x] T023 Record why any local Cargo command is necessary; skip broad local Cargo when CI can provide the signal. Decision: no broad local Cargo run for this wrapper/verifier-only slice; exact-head CI remains the Cargo authority.
- [x] T024 Run relevant managed local red/green test command for the slice only when T023 justifies it. Relevant local gates are the Python verifier suites plus `just ci-lint-workflow`.
- [x] T025 Run `git diff --check`.
- [x] T026 Open draft PR early enough for exact-head CI to run. Draft PR #436 is the exact-head CI surface for this slice.
- [x] T027 Push branch and verify exact-head CI green. PR #436 exact-head CI is required before ready.
- [x] T028 Record explicit no-mistakes skip/CI-evidence policy for this PR slice: do not use no-mistakes per operator instruction; T027 exact-head CI remains required before review/ready.
- [x] T029 Get Claude review and Claude adversarial review. Exact-head job IDs are recorded in the PR body/final evidence ledger because committing source evidence changes the PR head.
- [x] T030 Get Gemini review and Gemini adversarial review. Exact-head job IDs are recorded in the PR body/final evidence ledger because committing source evidence changes the PR head.
- [x] T031 Get DeepSeek, GLM, and Kimi review plus adversarial review for every implementation PR. Exact-head DeepSeek/GLM job IDs and the Kimi timeout/skip rationale are recorded in the PR body/final evidence ledger because committing source evidence changes the PR head.
- [x] T032 Open/mark ready only after findings are resolved or explicitly waived. Final ready-state evidence is recorded in the PR body/final evidence ledger to avoid source-level exact-head churn.

## Dependencies & Execution Order

- Phase 1 before all other phases.
- Phase 2 before implementation.
- US1 is the MVP planning slice.
- US2 and US3 are highest-value implementation preparation after US1.
- US4 follows once known-class policy is stable enough to define unknown-class baseline.
- Phase 7 applies to every implementation PR.

## Implementation Strategy

MVP is US1: issue map, disk-saving walkthrough, and verification policy. #286 is complete via PR #404. Remaining implementation starts with #374/#375/#376/#377 only after each issue's Phase 1 gate is pinned and reviewed.
