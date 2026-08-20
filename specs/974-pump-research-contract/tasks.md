# Tasks: Manipulated Pump Research Contract

**Input**: Design documents from `specs/974-pump-research-contract/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`,
`contracts/`, and `quickstart.md`

**Verification**: This feature changes research truth, source admission, custody,
and claim authority. Behavior and fail-closed tests are required. Compile-heavy
Rust evidence is remote-first under `AGENTS.md`; documentation changes require
targeted static checks and internal adversarial review.

**Organization**: Tasks follow the seven dependency-ordered slices in `plan.md`
while remaining grouped by user story. Each slice requires its own branch and PR
naming this spec, an exact-head review request, and explicit remaining scope.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can proceed in parallel because it changes a different file and has
  no dependency on an incomplete task.
- **[US1]–[US4]**: Maps to the four prioritized stories in `spec.md`.
- Every task names a primary file and applicable requirement/evidence scope.

## Slice Registry

| Slice | Branch suffix | Accepted scope | Residual scope after merge |
|---|---|---|---|
| Slice 1 | `pump-research-s1-definition` | Strict definition, canonical identity, artifact subfamily, caller identity, NT compile boundary | Slices 2–7 |
| Slice 2 | `pump-research-s2-admission` | Target frame, roster, identities, temporal assertions, source admission | Slices 3–7 |
| Slice 3 | `pump-research-s3-custody` | G/D, timestamp verification boundary, custody/disclosure checkpoints | Slices 4–7 |
| Slice 4 | `pump-research-s4-discovery` | Detector, controls, censored/complete episode manifest | Slices 5–7 |
| Slice 5 | `pump-research-s5-confirmation` | C, dual execution, semantic comparison, primary report/base claims | Slices 6–7 |
| Slice 6 | `pump-research-s6-enrichment` | E/P, authorization, content-neutral selection, bounded storage | Slice 7 and any separately authorized acquisition adapter |
| Slice 7 | `pump-research-s7-claims` | Remaining tiers, mechanism reports, invalidation/re-admission | Separately approved provider acquisition and predictive/trading work |

Every slice uses the evidence rows in `plan.md`, names this spec in its PR, and
cannot claim residual scope.

## Phase 1: Setup and Slice Boundaries

**Purpose**: Establish reviewable slice ownership without pre-registering future
code, creating a second project, or leaving placeholder modules.

- [x] T001 Record stable Slice-1 through Slice-7 identifiers, dependency links, accepted/residual scope, branch naming, and evidence classes in `specs/974-pump-research-contract/tasks.md`
- [x] T002 Add only Slice-1 credential-free definition, authority, timestamp-policy, and artifact fixtures under `config/research/pump-research-synthetic.toml`
- [x] T003 [P] Align the Research Analytics reference contract with one `experiment-contracts` subfamily and separate evidence-validity semantics in `specs/023-nt-research-analytics-platform/reference/data-model.md`

**Checkpoint**: Every implementation change has one declared owning spec slice
and no future slice is represented by empty code.

---

## Phase 2: Foundational Definition and Artifact Contract (Slice 1)

**Purpose**: Establish the one strict TOML authority, deterministic identity,
artifact registration, authenticated role binding, and NT compile boundary.

**Requirements**: FR-001–FR-006, FR-049–FR-052; SC-001, SC-004, SC-007, SC-015.

- [x] T004 Register only `research_experiment`, the `pump_research` Slice-1 commands, and the Slice-1 test module, and promote locked `aws-sdk-sts = 1.107.0` to a direct dependency in `crates/backtesting-vertical-slice/src/lib.rs`, `crates/backtesting-vertical-slice/src/bin/pump_research.rs`, `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs`, and `crates/backtesting-vertical-slice/Cargo.toml`
- [x] T005 Implement strict serde models for `ExperimentVersion`, role bindings, storage/timestamp/source/partition/detector/control/analysis/disclosure/confirmation/enrichment/claim policies, and all top-level TOML tables in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T006 Implement unknown-field rejection, parent/version chains, authorized append roles, cross-table references, ordered identifiers, and terminal-state transition rejection in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T007 Implement versioned deterministic semantic serialization with ordered maps/sets plus original-byte and semantic SHA-256 hashes in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T008 Extend `ResearchAnalyticsSubfamily` with exactly one `experiment-contracts` subfamily while preserving Artifact Index active/inactive storage semantics in `crates/backtesting-vertical-slice/src/artifact_index.rs`
- [x] T009 Implement typed research artifact envelopes, artifact types, evidence-validity state, URI/hash/lineage checks, and fail-on-dirty commit plans in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T010 Implement non-test AWS STS `GetCallerIdentity` principal resolution and TOML role matching that rejects missing/mismatched identity, payload/CLI self-assertion, and fixture principals/authorities outside tests in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T011 Implement immutable NT run-input compilation that preserves experiment/source/code/schema/NT/dependency/environment/numeric/seed identities and rejects semantic override in `crates/backtesting-vertical-slice/src/run_manifest.rs`
- [x] T012 Implement the single CLI with `validate` and `register-version`, structured non-secret output, and no provider/query/purchase/alternate-replay surface in `crates/backtesting-vertical-slice/src/bin/pump_research.rs`
- [x] T013 [P] Add strict TOML, canonical byte/hash, version-parent, role, terminal-state, fixture-authority, malformed-reference, and missing-value behavior tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_experiment.rs`
- [x] T014 [P] Add subfamily, evidence-vs-storage lifecycle, stale-pointer, bad URI/hash/lineage, and dirty-write behavior tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_artifact_index.rs`
- [x] T015 Add NT compile-boundary and CLI tests proving equal semantic inputs compile equally and alternate semantic flags/provider commands are rejected in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_experiment.rs`

**Checkpoint**: One strict experiment version can be validated and registered;
it cannot yet access data, discover, confirm, enrich, or publish.

---

## Phase 3: User Story 1 — Reproducible Episode Discovery (Priority: P1, Slices 2–4) 🎯 MVP

**Goal**: Admit bounded retained observations, account for all enumerated units,
prospectively protect E0, and produce a deterministic retrospective
episode/control manifest without manipulation or execution claims.

**Independent Verification**: Two fresh-directory runs over identical admitted
synthetic inputs produce identical semantic roster/episode/control manifests,
including every censored, missing, unmatched, and null outcome.

**Requirements**: FR-007–FR-040, FR-053–FR-060; SC-001–SC-006, SC-008,
SC-013, SC-015.

### Slice 2: Target Frame, Identity, and Source Admission

- [x] T016 [US1] Implement `TargetFrame`, deterministic outer-roster construction, inventory reconciliation/status precedence/conflict reasons, `RosterUnit`, exact four-state accounting, completeness, denominators, and attrition in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T017 [P] [US1] Implement `IdentityNode`, time-bounded `IdentityMapping`, evidence confidence, explicit splice rules, and ticker-only join rejection in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T018 [P] [US1] Implement append-only `TemporalAssertion` with valid/publication/availability/retrieval clocks, revision/retraction links, and `retrieval_time_attested` limits in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T019 [US1] Extend source admission with exact product/version, query/fields, rights/retention, provenance/transforms, fidelity/completeness, corrections, cost status, review/expiry, retained artifacts, and claim limits in `crates/backtesting-vertical-slice/src/source_proof.rs`
- [x] T020 [US1] Implement source evidence states `active`, `quarantined`, `revoked`, and `expired` independently of Artifact Index storage lifecycle in `crates/backtesting-vertical-slice/src/source_proof.rs`
- [x] T021 [US1] Register Slice-2 source/roster types and staged-only `register-source` behavior that never downloads and rejects E0 byte/metadata access before G in `crates/backtesting-vertical-slice/src/lib.rs` and `crates/backtesting-vertical-slice/src/bin/pump_research.rs`
- [x] T022 [P] [US1] Add public-boundary tests plus capability-private unit regressions for roster completeness, delist/relist, symbol reuse, migration, rebrand, conflicts, revision policy, coverage gaps, and E0-before-G failures in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_experiment.rs` and `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [x] T023 [P] [US1] Add source tests proving free/official inputs face identical gates, nominally free pilots require separate authorization, non-retainable inputs remain exploratory, confirmation remains unavailable before committed custody/unblinding evidence exists, and no network/provider call occurs in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_source_proof_admissibility.rs`

### Slice 3: Prospective G/D Custody Foundation

- [ ] T024 [US1] Register `research_commitment` and `research_custody` plus their test modules only when Slice 3 begins in `crates/backtesting-vertical-slice/src/lib.rs` and `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs`
- [ ] T025 [US1] Implement common commitment envelopes plus typed G and D payloads, predecessor/state rules, E0 containment/narrowing, full-span partitions, source/correction policy, detector grid, estimands, hypothesis families, dependence/multiplicity, controls, enrichment-strata functions, null rules, seeds, versions, prior disclosure receipts, range-feasibility record, and transactionally closed pre-D head in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T026 [US1] Implement timestamp receipt schemas and a registered verifier boundary that rejects missing, pending, expired, unknown, or invalid receipts and excludes test verifiers from non-test builds in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T027 [US1] Implement G-frozen separation among custodian, analysis authors, experiment decision-makers, governance approver, ingestion, disclosure, canonical, and verification roles; failed-run dual control; prior cache/notebook/export/provider/recipient/machine exposure inventory; retained-artifact quarantine/destruction evidence; contamination propagation; anchoring intervals; and disclosure budget in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T028 [US1] Implement append-only custody events for ingest, access, disclosure, execution, retry, comparison, credential events, authorization use, quarantine, and unsealing in `crates/backtesting-vertical-slice/src/research_custody.rs`
- [ ] T029 [US1] Implement custodian-controlled generation leases, authenticated principal/scope checks, stop-new/drain-active fencing, and distinct single-use role capabilities in `crates/backtesting-vertical-slice/src/research_custody.rs`
- [ ] T030 [US1] Implement checkpoint closure with zero active leases, anchored heads, compare-and-swap, single-use authorization, stale restart, registry/custody intervals, and omitted-tail rejection in `crates/backtesting-vertical-slice/src/research_custody.rs`
- [ ] T031 [US1] Implement noninteractive disclosure execution/receipts with exact tables/groupings/filters/schedule, suppression/rounding/censoring/boundary flags, cross-query/version differencing budget, recipients, checkpoint-before-delivery, and deterministic exploratory downgrade on unanchored/changed/adaptive delivery in `crates/backtesting-vertical-slice/src/research_custody.rs`
- [ ] T032 [US1] Add `append-custody-event`, `close-checkpoint`, and typed G/D `commit` subcommands with non-secret outcomes in `crates/backtesting-vertical-slice/src/bin/pump_research.rs`
- [ ] T033 [P] [US1] Add G-before-E0, timestamp, role separation, failed-run dual control, prior-exposure disposition, narrowing, contamination, anchoring, D completeness, robustness-grid, commitment-order, and correction-after-unblinding confirmation-invalidation tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_commitment.rs`
- [ ] T034 [P] [US1] Add chain-gap, bad predecessor, self-asserted role, stale capability, active lease, changed head, reused authorization, shared credential, governance-approved rotation/fresh head, pre-rotation compromise downgrade, omitted tail, differencing, and budget-exhaustion tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_custody.rs`

### Slice 4: Detector, Controls, and Manifest

- [ ] T035 [US1] Register `pump_episode` and its test module only when Slice 4 begins in `crates/backtesting-vertical-slice/src/lib.rs` and `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs`
- [ ] T036 [US1] Implement typed observation construction, clocks, quote normalization, return/reported-volume/baseline definitions, coverage, missingness, interruptions, warm-up, overlap, cooldown, ties, and deduplication in `crates/backtesting-vertical-slice/src/pump_episode.rs`
- [ ] T037 [US1] Implement abnormal-return plus abnormal-reported-volume plus completed-giveback detection with anchor, feature cutoff, completion, label availability, decision time, and reported-volume caveat in `crates/backtesting-vertical-slice/src/pump_episode.rs`
- [ ] T038 [US1] Implement complete-window eligibility, full purge, left/right censoring, D-frozen corrections, evaluation-to-discovery leakage rejection, and diagnostic-only robustness variants that cannot replace the primary in `crates/backtesting-vertical-slice/src/pump_episode.rs`
- [ ] T039 [US1] Implement same-time risk sets, pseudo-anchors, pre-cutoff features, frozen matching/distance/caliper/randomness/reuse/relaxation, balance/support, control-identity freeze before contamination, contamination, and unmatched retention in `crates/backtesting-vertical-slice/src/pump_episode.rs`
- [ ] T040 [US1] Implement stable semantic episode/control ids and complete manifests with bounded frame/universe/coverage language, denominators, missingness, exclusions, censored candidates, controls, attrition, nulls, and lineage in `crates/backtesting-vertical-slice/src/pump_episode.rs`
- [ ] T041 [US1] Implement `discover` from verified G/D and active admitted retained inputs with atomic artifact registration in `crates/backtesting-vertical-slice/src/bin/pump_research.rs`
- [ ] T042 [P] [US1] Add detector, clock, volume caveat, coverage, boundary, purge, correction, robustness-primary, deduplication, and null-result tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pump_episode.rs`
- [ ] T043 [US1] Add fresh-directory equality, same-time control, future-outcome exclusion, control-freeze-before-contamination, balance failure, unmatched episode, and semantic-manifest tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pump_episode.rs`

**Checkpoint**: US1 produces a reproducible bounded episode manifest and cannot
claim manipulation, fills, queue position, or executable PnL.

---

## Phase 4: User Story 2 — Sealed Confirmatory Result and Primary Report (Priority: P2, Slice 5)

**Goal**: Lock one C program, execute it twice under separated principals and
environments, and release a complete primary report only after semantic equality
and a closed checkpoint.

**Independent Verification**: Pending timestamps, later access, stale heads,
retries, partial output, human exposure, mismatch, reused evaluation, and missing
report disclosures all fail; only the clean synthetic run releases.

**Requirements**: FR-041–FR-052, FR-055–FR-057, FR-075, FR-080–FR-082;
SC-005, SC-007, SC-013, SC-015.

- [ ] T044 [US2] Implement typed C with one primary cell/D-frozen aggregation, unchanged boundaries, final program, conformance, comparator, normalization/exclusions/tolerance, retry/failure-code disclosure rules, closed custody head, later-access-new-head/new-C enforcement, post-C change forfeiture, and closed sequential-family adaptation/error allocation in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T045 [US2] Implement execution attempts with canonical/verification roles, exact inputs, environment/dependency/numeric/seed identities, capped identical-input retries, terminal/retryable outcomes, exposure, and quarantine in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T046 [US2] Implement conformance validation for partitions, purge, observations, clocks, deduplication, matching, multiplicity, comparison, and an independently derived primary-estimand fixture in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T047 [US2] Implement semantic normalization/comparison that excludes only C-declared volatile metadata, applies frozen tolerances, records both outputs, and never selects a preferred result in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T048 [US2] Implement sealed canonical and verification orchestration against identical admitted inputs through the existing NT/run-manifest path in `crates/backtesting-vertical-slice/src/runner.rs`
- [ ] T049 [US2] Implement release gating requiring both runs complete/equal, zero result exposure, complete event lineage, and an anchored transactional release checkpoint in `crates/backtesting-vertical-slice/src/research_custody.rs`
- [ ] T050 [US2] Implement base statement-level `AtomicClaim` records limited to `episode_detected` and `not_proven`, then implement the primary `ResearchReport` with bounded detection language, volume caveat, verification-as-reproduction disclosure, all attempts/overlaps, estimands/effects, dependence-aware uncertainty, diagnostics, null/small-sample outcomes, and generalization limits in `crates/backtesting-vertical-slice/src/research_claim.rs`
- [ ] T051 [US2] Register `research_claim` base report support plus `confirm` and `publish-report` commands without adaptive/result-selection flags in `crates/backtesting-vertical-slice/src/lib.rs`, `crates/backtesting-vertical-slice/src/bin/pump_research.rs`, and `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs`
- [ ] T052 [P] [US2] Add C state, later-access-new-C, failure-code count/accounting, identical retry, cap, partial output, human exposure, conformance, normalization, tolerance, mismatch, and evaluation-reuse tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_commitment.rs`
- [ ] T053 [P] [US2] Add separate-environment reproduction, semantic equality, no alternate replay, release checkpoint, bounded-language, report completeness, and missing-disclosure tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_confirmation.rs`
- [ ] T054 [US2] Add clean/adversarial end-to-end tests proving every attempt/access/comparison/unsealing event is anchored and a null, mismatch, leakage-failed, or insufficient report cannot become confirmatory success in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_confirmation.rs`

**Checkpoint**: US2 releases one non-adaptive primary report or a durable
failure/null report. That report exists before Stage 2 can begin.

---

## Phase 5: User Story 3 — Separately Authorized Selective Enrichment (Priority: P3, Slice 6)

**Goal**: Freeze hypotheses and sampling, obtain distinct authorization, admit
and mechanically select content-neutral candidate packets, and keep bytes
bounded without implementing provider acquisition.

**Independent Verification**: Stage-2 actions before the primary report, E,
authorization, or the required checkpoints fail. Clean fixture selection follows
E ranking/ties without a provider call; case-only evidence remains case-only.

**Requirements**: FR-061–FR-074; SC-009–SC-012.

- [ ] T055 [US3] Implement E mechanism predictions, ordering, tests, minimum evidence, falsifiers, alternatives, missing fields, estimands, multiplicity, scope, candidate packet, ranking/ties/multi-source, prior exposure, and Stage-1-visible tier caps in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T056 [US3] Implement the D-frozen pre-provider `EnrichmentDraw` over cases, matched controls, fixed near-threshold and negative/exogenous controls with strata, sizes, probabilities, randomization, weighting, substitutions, availability, and estimand in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T057 [US3] Implement candidate packets for rights, fields/dates/venues, corrections/completeness/gaps, timestamp/sequence/reset/disconnect/raw payload, checksums/replay invariants, quoted units/commitments/overages/expiry, reviewers/evidence hashes, and storage impact in `crates/backtesting-vertical-slice/src/source_proof.rs`
- [ ] T058 [US3] Implement acceptance only on discovery/negative-control windows or custody-released content-neutral metrics plus deterministic eligibility, ranking, ties, provider attrition, and case-only limits in `crates/backtesting-vertical-slice/src/source_proof.rs`
- [ ] T059 [US3] Implement immutable authorization receipts binding the completed primary report, counts/windows/coverage/storage/rights, operation, maximum cost, minimum coverage, and expiry without credentials in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T060 [US3] Implement anchored closing checkpoints before E, before P, and before mechanism-result release plus P selection before mechanism access with fields/windows/fusion/disagreement/precedence/exclusion/exposure in `crates/backtesting-vertical-slice/src/research_commitment.rs`
- [ ] T061 [US3] Enforce content-addressed retained/ephemeral byte bounds for event/control/near-threshold/negative/warm-up windows and reject universal/unbounded backfill in `crates/backtesting-vertical-slice/src/research_experiment.rs`
- [ ] T062 [US3] Add `authorize-enrichment`, E/P `commit`, and `select-source` commands that cannot quote, query, purchase, download, or display credentials in `crates/backtesting-vertical-slice/src/bin/pump_research.rs`
- [ ] T063 [P] [US3] Add primary-report prerequisite, E-before-solicitation, checkpoint ordering, authorization scope/cost/expiry, frozen draw, content-neutral windows, mechanical ranking/tie, prior exposure, and case-only tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_enrichment.rs`
- [ ] T064 [US3] Add storage-budget, unavailable coverage, provider attrition, L2/L3 fidelity, venue semantics, pre-P access, and zero-provider-call integration tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_enrichment.rs`

**Checkpoint**: US3 can authorize and select future bounded evidence, but cannot
itself contact a provider, spend money, or expose mechanism content early.

---

## Phase 6: User Story 4 — Auditable Claims and Evidence Lifecycle (Priority: P4, Slice 7)

**Goal**: Publish atomic evidence-scoped claims, extend reports for mechanism and
authority evidence, and invalidate all dependent artifacts when evidence changes
without deleting history.

**Independent Verification**: Trace each statement to active evidence,
commitments, attempts, report disclosures, scope, tier, alternatives, and
falsifiers; revoke one source and prove manifests, replay artifacts, reports, and
claims invalidate while history remains.

**Requirements**: FR-075–FR-084; SC-013–SC-015.

- [ ] T065 [US4] Extend immutable `AtomicClaim` versions beyond the Slice-5 base tiers with full scope, predictions, minimum evidence, certainty, alternatives, falsifiers, author/approver roles, conflict-of-interest analysis, and terminal states in `crates/backtesting-vertical-slice/src/research_claim.rs`
- [ ] T066 [US4] Implement tier validation for `episode_detected`, `not_proven`, `manipulation_alleged`, `venue_sanctioned`, `mechanism_consistent_with`, and authority-scoped `manipulation_proven` in `crates/backtesting-vertical-slice/src/research_claim.rs`
- [ ] T067 [US4] Extend reports with positive-unlabeled authoritative-case policy, population-recall prohibition, E-authorship disclosure, Stage-1-visible tier caps, mechanism falsifiers, and authority evidence scope in `crates/backtesting-vertical-slice/src/research_claim.rs`
- [ ] T068 [US4] Implement lineage traversal from source state changes through manifests, NT catalogs/replay artifacts, attempts, reports, and claims with immutable invalidation events in `crates/backtesting-vertical-slice/src/research_claim.rs`
- [ ] T069 [US4] Implement full fresh-packet re-admission as new source/experiment/report/claim versions without reactivating or deleting historical artifacts in `crates/backtesting-vertical-slice/src/research_claim.rs`
- [ ] T070 [US4] Add `publish-claims` and `invalidate` commands with active-evidence, conflict-of-interest, author/approver, and authority enforcement in `crates/backtesting-vertical-slice/src/bin/pump_research.rs`
- [ ] T071 [P] [US4] Add claim tier, authority scope, conflict-of-interest, allegation/sanction/proof separation, not-proven/non-manipulated, mechanism-falsifier, and L3 rejection tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_claim.rs`
- [ ] T072 [P] [US4] Add mechanism-report, positive-unlabeled, population-recall, E-authorship, tier-cap, prior-overlap, dependence, generalization, null, and small-sample tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_claim.rs`
- [ ] T073 [US4] Add quarantine/revocation/expiry traversal through replay/results/claims, preserved history, re-admission, and stale-active-claim rejection tests in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_claim.rs`

**Checkpoint**: US4 publishes auditable reports/claims whose active validity
follows evidence without rewriting history.

---

## Phase 7: Cross-Story Verification and Review

**Purpose**: Prove the combined contract without provider access, strategies,
deployment, or live trading.

- [ ] T074 Add a synthetic zero-spend end-to-end journey covering version registration, G/D discovery, C confirmation/report, E/P gating, claims, and invalidation in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pump_research_end_to_end.rs`
- [ ] T075 [P] Add CLI journey tests for every supported and explicitly unsupported command in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pump_research_cli.rs`
- [ ] T076 Add fresh-environment equality evidence for experiment bytes, episode manifests, NT inputs, confirmation outputs, reports, and claim registries in `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pump_research_end_to_end.rs`
- [ ] T077 Run the synthetic flow in `specs/974-pump-research-contract/quickstart.md` and record exact-head hashes, zero-provider-call/spend evidence, and storage bounds in the PR review request
- [ ] T078 Run local targeted formatting/schema/static checks and the exact advisory CI commands from `.github/workflows/` or the justfile; record raw exact-head results in the PR review request
- [ ] T079 Conduct an internal adversarial completion review against 84 FRs, 15 SCs, four stories, edge cases, plan/contracts, and every task; record resolved findings in `specs/974-pump-research-contract/reviews/internal-adversarial-review.md`
- [ ] T080 Commit/push each spec-bound slice, open its PR with accepted/residual scope, request the reviewer resolved from node ID `U_kgDOEZMFhA`, resolve every applicable thread, obtain approval, and merge only after explicit user authorization as recorded in `specs/974-pump-research-contract/reviews/external-review-status.md`

---

## Dependencies and Execution Order

```text
Slice 1 definition/artifacts
  -> Slice 2 roster/source admission
  -> Slice 3 G/D prospective custody
  -> Slice 4 deterministic discovery + manifest (US1)
  -> Slice 5 C confirmation + primary report (US2)
  -> Slice 6 E/P authorization + selection (US3)
  -> Slice 7 claims + mechanism report + lifecycle (US4)
  -> cross-story verification and review
```

- Task IDs are execution ordered; a later slice never precedes an earlier one.
- Each slice is independently reviewable and names remaining accepted scope.
- Full-goal completion requires every slice and all cross-story evidence.

### Parallel Opportunities

- T003 can proceed beside Slice-1 fixture preparation.
- T013 and T014 cover separate files after the Slice-1 interfaces stabilize.
- T017 and T018 implement distinct identity/assertion types.
- T022 and T023 cover roster and source-admission behavior separately.
- T033 and T034 cover commitment and custody failures separately.
- T042 can develop detector fixtures while control integration stabilizes.
- T052 and T053 cover state-machine and environment/report behavior separately.
- T071 and T072 cover claim and report behavior separately.
- T075 can proceed beside the end-to-end fixture after CLI contracts stabilize.

## Implementation Strategy

### MVP: User Story 1

Complete Slices 1–4 and verify the bounded deterministic discovery manifest. This
is useful research evidence but not the full goal and grants no confirmation,
enrichment, provider, strategy, deployment, or trading authority.

### Full Goal

Complete all 80 tasks in order. Do not mark the feature complete until all four
stories independently pass, all 84 FRs and 15 SCs have exact-head evidence, every
substantive review finding is resolved, and each spec-bound PR has the required
approval.

## Notes

- Fixture authorities and values are test-only and cannot satisfy non-test runs.
- No task authorizes or implements a Surf, CoinAPI, Tardis, Dune, Allium, Arkham,
  exchange, or other provider call.
- No task implements a predictive strategy, order submission, deployment, or
  live readiness.
- Mark a task `[x]` only after its named evidence exists at the exact head.
