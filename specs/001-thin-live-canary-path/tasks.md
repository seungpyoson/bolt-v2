# Tasks: Thin Bolt-v3 Live Canary Path

**Input**: Design documents from `/specs/001-thin-live-canary-path/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`

All code tasks use TDD. For each behavior: write failing test, run it and capture expected failure, implement minimal code, run green test, run phase verification, then commit. Do not batch unrelated slices.

## Phase 1: Planning And Evidence

**Purpose**: Lock constraints before runtime code.

- [x] T001 Record current-state evidence in `specs/001-thin-live-canary-path/research.md`.
- [x] T002 Replace `.specify/memory/constitution.md` with bolt-v3 constitution.
- [x] T003 Create feature spec, implementation plan, data model, contracts, quickstart, and tasks under `specs/001-thin-live-canary-path/`.
- [x] T004 Update `AGENTS.md` SPECKIT block to point to `specs/001-thin-live-canary-path/plan.md`.
- [x] T005 Run the active no-mistakes binary's `status` and `runs --limit 5`; when an issue-specific soak binary is active, use the operator-provided override path and record triage result in final handoff and shared soak log if a run exists.
- [x] T006 Verify planning artifacts with `rg -n "(?i:TB[D]|TO[D]O|fix[[:space:]]+later|NE[E]DS[[:space:]]+CLARIFICATION)|\\[[A-Z][A-Z0-9]*(_[A-Z0-9]+)+\\]" .specify/memory/constitution.md specs/001-thin-live-canary-path` and `git diff --check`.

## Phase 2: Production Entrypoint Adoption (US1)

**Goal**: Production binary enters one bolt-v3 build/run path.

**Independent Test**: `cargo test --test bolt_v3_production_entrypoint` fails before implementation because `src/main.rs` still calls `node.run()` directly, then passes after legacy runtime path is removed or made unreachable.

- [x] T007 [US1] Write failing test `tests/bolt_v3_production_entrypoint.rs::main_uses_bolt_v3_runner_wrapper_only` asserting `src/main.rs` contains no production direct `node.run()` and imports/calls `run_bolt_v3_live_node`.
- [x] T008 [US1] Run `cargo test --test bolt_v3_production_entrypoint main_uses_bolt_v3_runner_wrapper_only -- --nocapture`; expected failure references current direct `node.run()` in `src/main.rs`.
- [x] T009 [US1] Refactor `src/main.rs` to load bolt-v3 TOML, validate, build via `build_bolt_v3_live_node`, and run via `run_bolt_v3_live_node`.
- [x] T010 [US1] Remove or isolate legacy production config/ruleset runtime so it cannot be selected in production.
- [x] T011 [US1] Run `cargo test --test bolt_v3_production_entrypoint`, `cargo test --test bolt_v3_live_canary_gate`, and `cargo test --test config_parsing`.
- [x] T012 [US1] Run the active no-mistakes binary's `status`; if unavailable, record that fact instead of blocking the code slice.

## Phase 3: Generic Strategy And Runtime Registration (US3)

**Goal**: Bolt-v3 live-node build path registers configured strategies through a strategy binding, without core concrete strategy leakage.

**Independent Test**: `cargo test --test bolt_v3_strategy_registration` proves injected fake strategy binding can register through core and unsupported strategy fails closed.

- [x] T013 [US3] Write failing tests in `tests/bolt_v3_strategy_registration.rs` for fake binding registration, unsupported strategy rejection, and no concrete strategy key in core registration code.
- [x] T014 [US3] Run `cargo test --test bolt_v3_strategy_registration -- --nocapture`; expected failures show missing bolt-v3 strategy registration surface.
- [x] T015 [US3] Add `src/bolt_v3_strategy_registration.rs` with a generic `StrategyBinding` interface and production binding table.
- [x] T016 [US3] Wire strategy registration into `src/bolt_v3_live_node.rs` after NT client registration and before runner entry.
- [x] T017 [US3] Run `cargo test --test bolt_v3_strategy_registration` and `cargo test --test bolt_v3_provider_binding`.

## Phase 4: Initial Binary-oracle Edge Taker Activation (US3)

**Goal**: Initial taker strategy is configured by reference roles and strategy parameters, not hardcoded provider assumptions.

**Independent Test**: Existing strategy tests plus new config tests prove Polymarket option venue, Chainlink primary reference, and multiple exchange reference roles are configured through TOML.

- [x] T018 [US3] Write failing validation tests proving strategy config accepts multiple exchange reference roles and rejects missing primary oracle/reference roles.
- [x] T019 [US3] Run targeted strategy/config tests; expected failure shows current config cannot express all required reference roles.
- [x] T020 [US3] Extend strategy-archetype validation in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` to validate reference roles generically.
- [x] T021 [US3] Extend fixtures under `tests/fixtures/bolt_v3/` only for operator-visible runtime values, not test-local timing scaffolding.
- [x] T022 [US3] Run `cargo test --test config_parsing` and targeted tests for the initial registered taker strategy.

## Phase 5: Mandatory Decision Evidence (US2)

**Goal**: No submit path can exist without bolt-v3 decision evidence.

**Independent Test**: Strategy construction and submit tests fail closed when evidence writer is absent or persistence fails.

- [x] T023 [US2] Write failing tests that construct the strategy without decision evidence and expect construction rejection.
- [x] T024 [US2] Write failing tests that simulate evidence persistence failure and expect submit rejection before NT submit.
- [x] T025 [US2] Remove optional/fallback evidence submit path from the registered strategy implementation.
- [x] T026 [US2] Make bolt-v3 strategy registration provide mandatory decision evidence.
- [x] T027 [US2] Run targeted strategy tests and source-fence search for fallback direct submit branches.

## Phase 6: Submit Admission Consumes Gate Report (US2)

**Goal**: `BoltV3LiveCanaryGateReport` bounds are enforced before every live submit.

**Independent Test**: `cargo test --test bolt_v3_submit_admission` proves order count, notional cap, cap equality, missing/unarmed report, double-arm/stale-arm, global submit budget, cancel exclusion, and evidence failure all reject before NT submit without consuming admission budget before evidence persists.

- [x] T028 [US2] Write one failing public behavior test in `tests/bolt_v3_submit_admission.rs` for unarmed admission rejecting before NT submit with `NotArmed`.
- [x] T029 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture`; expected failures show missing submit admission module.
- [x] T030 [US2] Add `src/bolt_v3_submit_admission.rs` with shared admission state armed only from `BoltV3LiveCanaryGateReport`.
- [x] T031 [US2] Continue vertical TDD, one behavior at a time, for count cap, notional cap, cap equality, evidence-failure-before-admission, success ordering, global entry/exit/replace-submit budget, cancel exclusion, and double-arm/stale-arm behavior.
- [x] T032 [US2] Wire one shared admission handle from live-node build through strategy contexts into `run_bolt_v3_live_node`, then wire strategy submit calls through decision evidence, submit admission, admission permit, and NT submit.
- [x] T033 [US2] Run `cargo test --test bolt_v3_submit_admission`, targeted strategy submit tests, and source-fence checks across `src/strategies/**/*.rs` and `src/bolt_v3_archetypes/**/*.rs` for direct `submit_order` bypasses.

## Phase 7: Authenticated No-submit Readiness (US4)

**Goal**: Real SSM/venue connect-disconnect produces a redacted report consumed by PR #305 gate.

**Independent Test**: Local tests cover report schema and zero-order guard. Ignored operator test produces real artifact only with explicit approval.

Live-readiness evidence note: T038 and T046 stay unchecked until explicit
operator-run evidence exists. PR #331 P9 cites those unchecked tasks only as
blockers, not as deferred approval to trade.

- [x] T034 [US4] Write failing schema tests for no-submit readiness report producer and gate consumer compatibility.
- [x] T035 [US4] Write zero-order source/behavior fence proving readiness code cannot call submit, cancel, replace, or amend order APIs.
- [x] T036 [US4] Implement minimal no-submit readiness runner using existing bolt-v3 build and controlled-connect/disconnect boundaries.
- [x] T037 [US4] Run local readiness tests with mock SSM resolver and no network.
- [ ] T038 [US4] With explicit operator approval, run ignored real SSM/venue no-submit readiness and store redacted report path outside tracked secrets.
- [x] T039 [US4] Run `cargo test --test bolt_v3_live_canary_gate` against the redacted report fixture shape.

## Phase 8: Tiny-capital Live Canary (US5)

**Goal**: One approved capped live order proves production-shaped bolt-v3 spine through NT.

**Independent Test**: Local tests prove all preconditions and fail-closed paths. Operator artifact proves real submit/venue result/cancel/reconciliation.

- [x] T040 [US5] Write failing canary precondition tests requiring exact config checksum, approval id, gate report, submit admission state, and decision evidence.
- [x] T041 [US5] Write ignored operator test or command harness that submits at most one configured canary order after explicit approval.
- [x] T042 [US5] Implement canary operator harness using the production bolt-v3 path and NT adapter submit only.
- [x] T043 [US5] Add strategy-driven cancel path evidence capture for open canary orders.
- [x] T044 [US5] Add restart reconciliation evidence capture through NT adapter state.
- [ ] T045 [US5] Run local fail-closed tests, exact-head CI, no-mistakes triage, and external review after branch is clean and pushed.
- [ ] T046 [US5] With explicit operator approval, run tiny-capital canary and store redacted artifact with exact SHA and config checksum.

## Phase 9: Review Remediation - No-submit Evidence Freshness (US4)

**Goal**: A no-submit readiness report is accepted only when it is fresh, exact-head, config-bound, and stage-complete.

**Independent Test**: `cargo test --test bolt_v3_live_canary_gate -- --nocapture` rejects stale, missing-freshness, wrong-binary, wrong-config, and unsatisfied readiness reports before runner entry.

- [x] T047 [P] [US4] Write failing stale-report rejection tests in `tests/bolt_v3_live_canary_gate.rs` for missing `generated_at_unix_seconds`, expired report age, and report age above TOML-owned `[live_canary].readiness_report_max_age_seconds`.
- [x] T048 [US4] Add TOML-owned readiness report age config in `src/bolt_v3_config.rs`, `config/root.example.toml`, `src/bolt_v3_no_submit_readiness.rs`, and `src/bolt_v3_live_canary_gate.rs`.
- [x] T049 [P] [US4] Write failing no-submit stage-detail tests in `tests/bolt_v3_no_submit_readiness.rs` proving partial connect, skipped reference readiness, and stale reference cache cannot produce a gate-acceptable report.
- [x] T050 [US4] Extend no-submit readiness stage evidence in `src/bolt_v3_no_submit_readiness.rs` and `src/bolt_v3_live_node.rs` so failed NT client connect/reference states stay failed and redacted stage details are preserved.
- [x] T051 [US4] Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture` and `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.

## Phase 10: Review Remediation - Production Canary Approval Envelope (US5)

**Goal**: Production `Run` enforces the same operator evidence envelope required by the tiny-canary contract, not only harness-local checks.

**Independent Test**: `cargo test --test bolt_v3_live_canary_gate -- --nocapture` and `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture` reject missing operator evidence, invalid approval windows, nonce mismatch, stale approval, SSM manifest mismatch, strategy-input mismatch, financial-envelope mismatch, and pre-run evidence mismatch.

- [x] T052 [P] [US5] Write failing production-gate tests in `tests/bolt_v3_live_canary_gate.rs` requiring `[live_canary].operator_evidence` for production `Run`.
- [x] T053 [P] [US5] Write failing operator-envelope regression tests in `tests/bolt_v3_tiny_canary_operator.rs` for approval window, nonce, SSM manifest hash, strategy-input hash, financial-envelope hash, and pre-run evidence hash.
- [x] T054 [US5] Validate `LiveCanaryOperatorEvidenceBlock` inside `src/bolt_v3_live_canary_gate.rs` before submit admission arms in `src/bolt_v3_live_node.rs`.
- [x] T055 [US5] Update `docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md`, `specs/001-thin-live-canary-path/checklists/production-readiness.md`, and `specs/001-thin-live-canary-path/quickstart.md` with the production-enforced operator evidence fields.
- [x] T056 [US5] Run `cargo test --test bolt_v3_live_canary_gate -- --nocapture` and `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`.

## Phase 11: Review Remediation - Submit Lifecycle Safety (US2)

**Goal**: The canary submit budget cannot strand exposure without an explicit config-owned lifecycle policy.

**Independent Test**: `cargo test --test bolt_v3_submit_admission -- --nocapture` proves entry, replace-submit, exit-submit, and cancel-only decisions follow configured lifecycle semantics and cannot bypass admission.

- [x] T057 [P] [US2] Write failing admission tests in `tests/bolt_v3_submit_admission.rs` for submit intent kind, risk-reducing exit after entry, cancel-only exclusion, and config-owned lifecycle policy.
- [x] T058 [US2] Add submit intent classification to `src/bolt_v3_submit_admission.rs` and `src/strategies/binary_oracle_edge_taker.rs` without adding venue, symbol, or strategy hardcodes.
- [x] T059 [US2] Update `specs/001-thin-live-canary-path/spec.md`, `specs/001-thin-live-canary-path/contracts/live-canary-gates.md`, and `specs/001-thin-live-canary-path/data-model.md` so FR-009 names the accepted lifecycle semantics.
- [x] T060 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture` and source-fence searches for direct `submit_order` bypasses in `src/strategies/` and `src/bolt_v3_archetypes/`.

## Phase 12: Review Remediation - Observability, Secrets, and Ledger Hygiene (US3, US5)

**Goal**: Operator evidence is diagnostic, secret-safe, and tracked by open readiness work.

**Independent Test**: Runtime-capture verification proves PortfolioSnapshot capture is represented, config parsing rejects ambiguous SSM paths, credential redaction tests still pass, and trace docs no longer imply closed issues prove live readiness.

- [x] T061 [P] [US5] Write failing runtime-capture test or verifier fixture in `scripts/test_verify_runtime_capture_yaml.py` for `PortfolioSnapshot` subscription and JSONL spool coverage.
- [x] T062 [US5] Implement PortfolioSnapshot capture or an explicit waiver gate in `src/nt_runtime_capture.rs`, `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml`, and `docs/bolt-v3/research/runtime-capture/bolt-current-capture.yaml`.
- [x] T063 [P] [US3] Write failing SSM path hygiene tests in `tests/config_parsing.rs` rejecting leading/trailing whitespace in `*_ssm_path` TOML values.
- [x] T064 [US3] Reject ambiguous SSM paths in `src/bolt_v3_validate.rs` and keep `src/secrets.rs` byte-exact for resolved secret values.
- [x] T065 [P] [US3] Write or update credential redaction tests in `tests/bolt_v3_credential_log_suppression.rs` proving provider credentials remain redacted and never printed.
- [x] T066 [US3] Replace raw provider credential storage with redacted/zeroizing types in `src/bolt_v3_providers/polymarket.rs` and `src/bolt_v3_providers/binance.rs`, or update docs to stop claiming that hardening if it is intentionally deferred.
- [x] T067 [US5] Update `docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md`, `docs/bolt-v3/2026-05-18-production-readiness-contract.md`, and `specs/001-thin-live-canary-path/checklists/production-readiness.md` so #409 is explicit and #360 closure is not used as proof that T046 is complete.
- [x] T068 [US5] With explicit user approval only, update GitHub issue links or successor tracking for T046 and #409; otherwise record required issue mutation as a blocked operator action in `docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md`.
- [ ] T069 [US3] Run `cargo test --test config_parsing -- --nocapture`, `cargo test --test bolt_v3_credential_log_suppression -- --nocapture`, `python3 scripts/test_verify_runtime_capture_yaml.py`, and `python3 scripts/verify_runtime_capture_yaml.py`.

## Phase 13: AI Slop Cleanup and Final Verification

**Goal**: Keep the remediation small, reviewable, TDD-proven, and free of stale AI-generated doc drift.

**Independent Test**: The final diff is scoped to the remediation tasks, each touched behavior has a targeted regression test, and broad source/docs checks pass.

- [ ] T070 [P] Run `rg -n "(?i:TODO|fix later|temporary|placeholder|AI|slop|stale|production-ready)" src tests specs/001-thin-live-canary-path docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md` and remove or justify stale prose in touched files.
- [ ] T071 Run `cargo fmt --check`, `git diff --check`, all targeted cargo tests from T051/T056/T060/T069, and `just source-fence` if available.
- [ ] T072 Run final spec-compliance and code-quality reviews for the remediation diff before claiming readiness status.
- [x] T073 [US5] Fix review-discovered quickstart evidence-binding drift in `specs/001-thin-live-canary-path/quickstart.md`: head binding must name `[live_canary.operator_evidence].head_sha` plus build-owned head, root TOML hash must be checked via `approval_consumption_path`, and `approval_envelope_path` must not be described as read by the production gate.
- [x] T074 [US5] After T073, run `git diff --check` and `just source-fence`, then rerun scoped config/schema/docs re-review before push or external review.
- [x] T075 [US5] Diagnose exact-head PR #388 CI clippy and gate failures with `gh pr checks 388 --repo seungpyoson/bolt-v2` plus failed job logs before editing `build.rs`.
- [x] T076 [US5] Fix the exact clippy failures in `build.rs` and `src/bolt_v3_live_canary_gate.rs`, run `just clippy`, `git diff --check`, and `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.
- [x] T077 [US5] Push the clippy-fix commit, re-check PR #388 exact-head CI is green, then start external quorum review.
- [x] T078 [US5] Review all Greptile PR #388 comments, then either patch and reply/resolve or disprove each comment with file/line and test evidence.
- [x] T079 [US5] Fix external-review consensus safety hardening in `src/bolt_v3_live_canary_gate.rs`, `src/bolt_v3_live_node.rs`, `tests/bolt_v3_live_canary_gate.rs`, and `tests/bolt_v3_tiny_canary_operator.rs`: reject symlinked no-submit readiness reports, fail closed on no-submit timeout sum overflow, and remove duplicate JSON keys from the unapproved-strategy-hash test.
- [x] T080 [US5] Reconcile external-review docs/ledger concerns: Chainlink provider availability vs runtime contracts, Phase 8 harness-only `BOLT_V3_PHASE8_*` env vars, PR #331 historical-anchor wording, P5 cadence-table wording, and runtime-contract freshness wording.
- [x] T081 [US5] After pushing T079, re-check PR #388 exact-head CI and retry Kimi on narrowed exact-head shards because the prior full-diff Kimi run timed out.
- [x] T082 [US5] Address Greptile post-`49d8ea3a` comments: remove broad `.git` build-script rerun trigger, align live-canary approval-consumption test fixture timestamps, and reply/resolve or disprove remaining review threads.
- [x] T083 [US5] Address Greptile post-`26c83db4` P1 comments: bind `approval_envelope_path` content via `approval_envelope_sha256`, bind client/venue order hashes into approval-consumption proof validation, and avoid re-aging approval consumption during late TOCTOU revalidation while still checking the approval window.
- [x] T084 [US5] Address local code-quality review follow-up: add a call-site regression test proving late live-canary gate revalidation still rejects an expired operator approval window without re-aging approval-consumption freshness.
- [x] T085 [US5] Address Greptile post-`5a88275b` comments: remove redundant required-stage condition, avoid stale no-submit readiness metadata fixtures, and surface `readiness_report_max_age_seconds` on `BoltV3LiveCanaryGateReport`.
- [x] T086 [US5] Address Greptile post-`ad4f2557` comments: move example approval-window timestamps into the future and document that the operator approval window must cover report validation plus late evidence re-read/re-hash latency.
- [x] T087 [US5] Address Greptile post-`d9258272` style finding: remove dead `satisfied_stage_names` bookkeeping from no-submit readiness required-stage validation.
- [x] T088 [US5] Address Greptile post-`ef72cd7b` style findings: clarify the standalone approval-consumption window guard and make bounded-read cap errors caller-neutral, then reply with the current `read_report_bytes_with_limit` call-path evidence.
- [x] T089 [US5] Address Greptile post-`a3314e8e` documentation findings: document the second operator-evidence re-read/re-hash intent and late timestamp freshness headroom, then reply with code/doc evidence.
- [x] T090 [US5] Address external-review checksum/docs drift: align `config_hash` prose with the actual framed `config_bundle_checksum`, clarify Chainlink provider availability vs runtime-contract target, mark `BOLT_V3_PHASE8_*` values as harness-only, clarify accepted updown cadence-table semantics, and align live-canary byte-cap examples.
- [x] T091 [US5] Add TDD regression coverage proving a missing operator evidence file fails closed as `OperatorEvidenceRead` before hashing.
- [x] T092 [US5] Add TDD regression coverage proving Phase 8 sha256 helpers reject uppercase hex and align helper behavior with the production live-canary gate.
- [ ] T093 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run external-review consensus for the T080/T090-T092 slice.
- [x] T094 [US5] Address shared Kimi/DeepSeek external-review test gap by adding production live-canary gate regression coverage for 64-character uppercase operator-evidence hash rejection.
- [ ] T095 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T094 coverage slice.
- [x] T096 [US5] Address Greptile current-head P1: stamp no-submit readiness `generated_at_unix_seconds` after controlled connect/reference/disconnect stages, immediately before report creation/write.
- [x] T097 [US5] Address Greptile current-head protocol-literal style finding: extract approval-consumption schema version and record-kind validation literals into named constants.
- [x] T098 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T096-T097 Greptile slice.
- [x] T099 [US5] Address external-review T096 test-hardening note: make the no-submit freshness source-shape guard pin `current_unix_seconds()` after controlled stages, not only the final report-builder call.
- [x] T100 [US5] Address Greptile current-head submit-lifecycle observation: investigate and either correct or formally constrain `ReplaceSubmit` counter semantics so replaces cannot inflate risk-reducing-exit budget in any supported multi-order configuration.
- [x] T101 [US5] Address Greptile current-head submit-admission serialization observation: investigate and either shorten the admission mutex critical section without weakening counter/evidence atomicity, or encode the serialization requirement with explicit tests and docs.
- [x] T102 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T099-T101 submit-admission/test-hardening slice.
- [x] T103 [US5] Address Greptile current-head tiny-canary proof incompatibility: make the Phase 8 approval-consumption writer emit the gate-required `approval_envelope_sha256`, `client_order_id_hash`, and `venue_order_id_hash` fields, with writer-to-gate regression coverage.
- [x] T104 [US5] Address Gemini T103 test-gap finding: add regression coverage proving an approval-consumption proof written by the Phase 8 harness is accepted by the actual live-canary gate.
- [x] T105 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T103/T104 proof-compatibility slice.

## Out Of Scope For MVP

- Backtesting engine.
- Research analytics platform.
- New Bolt-owned venue adapter.
- Bolt-owned order lifecycle or reconciliation implementation.
- Test-literal verifier expansion.

## Execution Order

Phase 1 must merge first. Phases 2-8 are sequential because each removes a live-submit blocker from the prior phase. Do not begin live operations until Phases 2-7 are complete and verified.

Review remediation phases 9-13 are source/test/doc tasks only unless T038, T046, or T068 receive explicit user approval. Implement in this order: Phase 9 freshness and no-submit truthfulness, Phase 10 production operator evidence, Phase 11 lifecycle safety, Phase 12 observability/secrets/ledger hygiene, Phase 13 cleanup and verification. T047, T049, T052, T053, T057, T061, T063, T065, and T070 are parallelizable only when workers touch disjoint files.
