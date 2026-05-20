# Tasks: NT-First Research Analytics Platform

**Input**: `spec.md`, `plan.md`, `research.md`, `evidence.md`,
`data-model.md`, `contracts/nt-research-analytics.md`

**Prerequisites**: Use branch `023-nt-research-analytics-platform` with
`.specify/feature.json` pointing at this package before implementation or issue
mutation. Read-only verification can also target this package with
`SPECIFY_FEATURE=023-nt-research-analytics-platform` and
`SPECIFY_FEATURE_DIRECTORY=specs/023-nt-research-analytics-platform`, but normal
review should rely on the branch-local feature pointer.

**Task format**: All task rows include exact artifact or code paths. Story labels
appear only in user-story phases.

## Phase 1: Setup

**Purpose**: Keep planning package reproducible before implementation.

- [ ] T001 Record current SpecKit branch-local pointer and explicit 023 read-only override in `specs/023-nt-research-analytics-platform/analysis.md`.
- [ ] T002 Refresh current external price/source links in `specs/023-nt-research-analytics-platform/evidence.md`.
- [ ] T003 Keep issue mutation approval gate visible in `specs/023-nt-research-analytics-platform/github-issues.md`.

## Phase 2: Foundational Gates

**Purpose**: Blocking evidence before any user-story implementation.

- [ ] T004 Freeze NT pointer choice in `specs/023-nt-research-analytics-platform/evidence.md`: current Bolt pin, upstream `develop`, or explicit future pointer.
- [ ] T005 Build all-in monthly cost model in `specs/023-nt-research-analytics-platform/cost-model.md`.
- [ ] T006 Build provider/source fidelity matrix in `specs/023-nt-research-analytics-platform/fidelity-matrix.md`.
- [ ] T007 Verify existing issue map in `specs/023-nt-research-analytics-platform/issue-audit.md` and `specs/023-nt-research-analytics-platform/github-issues.md` against #19, #20, #21, #22, #23, #24, #34, #36, #39, #75, #77, #88, #112, #115, #127, #148, #158, #176, #236, #254, #369, #385, #407, and #409.

## Phase 3: User Story 1 - NT Capability Proofs (Priority: P1)

**Goal**: Prove NT surfaces first, before Bolt-owned runtime or adapter work.

**Independent Test**: Reviewer can inspect `evidence.md` and see every venue
surface classified as `SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, or
`DECISION_NEEDED`.

- [ ] T008 [US1] Prove HIP-4 upstream support on selected NT pointer in `specs/023-nt-research-analytics-platform/evidence.md`.
- [ ] T009 [US1] Prove HIP-4 historical-data class separately from live adapter support in `specs/023-nt-research-analytics-platform/fidelity-matrix.md`; if class remains `FORWARD_CAPTURE_PENDING`, record forward-capture start/skip trigger.
- [ ] T010 [US1] Prove Kalshi adapter support from selected pointer/source in `specs/023-nt-research-analytics-platform/evidence.md`.
- [ ] T011 [US1] Prove NT Polymarket support and public API cap behavior in `specs/023-nt-research-analytics-platform/evidence.md`.
- [ ] T012 [US1] Prove selected perpetual-futures venues through generic venue gate in `specs/023-nt-research-analytics-platform/fidelity-matrix.md`.

## Phase 4: User Story 2 - Provider Cost And Fidelity Gate (Priority: P1)

**Goal**: Select no provider until cost, license, coverage, and fidelity are
proved.

**Independent Test**: Reviewer can inspect `cost-model.md` and
`fidelity-matrix.md` and see accepted/rejected claims per source.

- [ ] T013 [US2] Model Tardis subscription plus AWS/dashboard reserve in `specs/023-nt-research-analytics-platform/cost-model.md`.
- [ ] T014 [US2] Model Telonex personal/commercial license split in `specs/023-nt-research-analytics-platform/cost-model.md`.
- [ ] T015 [US2] Model Goldsky metered subgraph/pipeline costs in `specs/023-nt-research-analytics-platform/cost-model.md`.
- [ ] T016 [US2] Classify official archive/API capture per selected venue in `specs/023-nt-research-analytics-platform/fidelity-matrix.md`; include current price/license refresh and schema/contract test criteria proving venue/provider swaps are TOML/registry-only.
- [ ] T017 [US2] Run one Tardis replay-to-NT-catalog prototype in `src/nt_research_provider_probe.rs` and `tests/nt_research_provider_probe.rs` only after T008 and T013-T016 pass; include Cargo manifest/module wiring and CI-covered tests in the same change.
- [ ] T018 [US2] Run one Polymarket provider projection prototype in `src/nt_research_provider_probe.rs` and `tests/nt_research_provider_probe.rs` only after T008 and T017 land, or after T008 plus a separate-file probe split is accepted; include Cargo manifest/module wiring and CI-covered tests in the same change.

## Phase 5: User Story 3 - Research Runner And Analytics (Priority: P2)

**Goal**: Build thin NT research orchestration without second trading truth.

**Independent Test**: One accepted NT catalog backtest emits reports/results with
NT pointer, source hashes, and fidelity claim limits.

- [ ] T019 [US3] Define `ResearchRunManifest` schema in `specs/023-nt-research-analytics-platform/data-model.md`.
- [ ] T020 [US3] Define `RawEvidenceRecord` and `CatalogProjection` lineage in `specs/023-nt-research-analytics-platform/data-model.md`.
- [ ] T021 [US3] Draft runner contract in `specs/023-nt-research-analytics-platform/contracts/nt-research-analytics.md`.
- [ ] T022 [US3] Build thinnest NT `BacktestNode` runner in `src/nt_research_runner.rs` only after T008-T021 pass; include Cargo manifest/module wiring and CI-covered tests in the same change.
- [ ] T023 [US3] Add lower-fidelity claim-limit verification in `tests/nt_research_claim_limits.rs`.
- [ ] T024 [US3] Keep Python/Jupyter research-only workflow documented in `specs/023-nt-research-analytics-platform/research.md`.

## Phase 6: User Story 4 - Dashboard Source Contract And MVP (Priority: P2)

**Goal**: Plan read-only dashboard from NT-derived truth.

**Independent Test**: Source contract names NT-derived source and freshness rule
for each displayed panel before UI work starts.

- [ ] T025 [US4] Define dashboard source contract in `specs/023-nt-research-analytics-platform/contracts/nt-research-analytics.md`.
- [ ] T026 [US4] Prove selected NT pointer exposes dashboard source surfaces in `specs/023-nt-research-analytics-platform/evidence.md`: reports, `PortfolioSnapshot`, and msgbus subscription/publish APIs.
- [ ] T027 [US4] Confirm #409 prerequisite for account-wide MTM/PnL in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T028 [US4] Confirm #77 durable trade-history/PnL dependency in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T029 [US4] Include or exclude #36 redemption-realized-PnL and #369 production-readiness relationship in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T030 [US4] Evaluate dashboard/BI product fit in `specs/023-nt-research-analytics-platform/cost-model.md`, `specs/023-nt-research-analytics-platform/research.md`, and `specs/023-nt-research-analytics-platform/contracts/nt-research-analytics.md`: Grafana, Metabase, Preset/Superset, Plotly/Dash, or bespoke UI fallback.
- [ ] T031 [US4] Build read-only dashboard/read model in `src/nt_research_dashboard_read_model.rs` and `tests/nt_research_dashboard_read_model.rs` only after T025-T030 pass; include Cargo manifest/module wiring and CI-covered tests in the same change.
- [ ] T032 [US4] Add stale-source display tests in `tests/nt_research_dashboard_freshness.rs`.

## Phase 7: User Story 5 - Issue Payloads And Review (Priority: P1)

**Goal**: Convert planning into issue-ready slices without mutating GitHub before
approval.

**Independent Test**: Each issue payload has purpose, evidence rows, accepted
scope, out-of-scope list, acceptance evidence, dependency links, and residual
scope.

- [ ] T033 [US5] Validate Issue A payload in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T034 [US5] Validate Issue B payload in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T035 [US5] Validate Issue C payload in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T036 [US5] Validate Issue D payload in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T037 [US5] Validate Issue E payload in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T038 [US5] Validate Issue F/G dashboard payloads in `specs/023-nt-research-analytics-platform/github-issues.md`.
- [ ] T039 [US5] Re-run adversarial review on `.specify/memory/constitution.md`, `specs/023-nt-research-analytics-platform/spec.md`, `specs/023-nt-research-analytics-platform/plan.md`, `specs/023-nt-research-analytics-platform/research.md`, `specs/023-nt-research-analytics-platform/evidence.md`, `specs/023-nt-research-analytics-platform/cost-model.md`, `specs/023-nt-research-analytics-platform/fidelity-matrix.md`, `specs/023-nt-research-analytics-platform/data-model.md`, `specs/023-nt-research-analytics-platform/contracts/nt-research-analytics.md`, `specs/023-nt-research-analytics-platform/analysis.md`, `specs/023-nt-research-analytics-platform/tasks.md`, `specs/023-nt-research-analytics-platform/github-issues.md`, `specs/023-nt-research-analytics-platform/issue-audit.md`, and `specs/023-nt-research-analytics-platform/checklists/requirements.md`.
- [ ] T040 [US5] Stop before GitHub issue creation unless user approves `specs/023-nt-research-analytics-platform/github-issues.md`.

## Dependencies

- T004-T007 block all user-story implementation.
- US1 and US2 can progress in parallel after T004-T007.
- US3 depends on accepted US1 and US2 evidence.
- US4 source-contract work can start after T007, but dashboard MVP depends on
  T025-T030 plus #409/#77/#36/#369 decisions.
- US5 validation can run after each issue payload changes; issue mutation waits
  for explicit user approval.
- Any future Rust source file in this plan must be wired into the Cargo/module
  graph and covered by the existing Rust test path before review.

## Parallel Examples

- Do not parallelize tasks that edit the same authority file. `evidence.md`,
  `cost-model.md`, and `github-issues.md` updates must be sequenced or split
  into disjoint files first.
- T032 can run after T025-T030 if stale-source tests use a separate test file
  and do not edit the dashboard source contract.

## MVP Slice

MVP for implementation is US1 + US2 proof artifacts only. No runner or dashboard
code should start until NT pointer, provider, and fidelity gates are accepted.
