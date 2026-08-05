# Production Readiness Checklist: Thin Bolt-v3 Live Canary Path

> **Historical feature artifact — not an active checklist.** Do not execute or
> complete items from this file. Current `main`, `AGENTS.md`, and tracked issues
> are authoritative.

**Purpose**: Validate whether the current requirements are complete, clear, and measurable enough to support production-grade live trade readiness, not only a tiny-capital canary.
**Created**: 2026-05-20
**Feature**: [spec.md](../spec.md)

**Note**: This checklist is generated from `/speckit-checklist` intent. It tests the requirements writing and evidence plan, not implementation behavior.

**Issue-ledger note**: Issue #409 tracks PortfolioSnapshot observability and must be explicit in readiness ledgers when observability evidence is claimed. Issue #360 is closed historical tiny-canary tracking, but that closure is not evidence that T046 produced a tiny-capital canary artifact.

## Requirement Completeness

- [ ] CHK001 Are production-grade readiness requirements explicitly separated from tiny-capital canary proof, with success criteria for both stated independently? [Completeness, Spec §User Story 5, SC-006, Plan §Scale/Scope]
- [ ] CHK002 Are all production entrypoint stages specified end to end: TOML load, strategy load, validation, SSM resolution, provider adapter mapping, client registration, strategy registration, live gate, submit admission, NT runner, order evidence, venue state, cancel, restart reconciliation, and post-run hygiene? [Completeness, Spec §User Story 1, Spec §User Story 5, Quickstart §Tiny-capital Canary]
- [ ] CHK003 Are production-readiness requirements defined for authenticated no-submit connect/disconnect failures, including how failed NT engine connection states must be represented in the readiness report? [Gap, Spec §User Story 4, Quickstart §Operator No-submit Readiness]
- [ ] CHK004 Are requirements defined for production build-feature compatibility, including which transport backends are permitted and how unavailable Cargo features are rejected before operator runs? [Gap, Plan §Technical Context]
- [ ] CHK005 Are requirements defined for external venue protocol drift, including adapter schema-version mismatch, upstream dependency pinning, and escalation when NT cannot decode current venue responses? [Gap, Spec §Edge Cases]
- [ ] CHK006 Are requirements defined for production credential hygiene beyond "SSM-only", including exact secret value format, whitespace policy, parameter version capture, and non-disclosure evidence? [Gap, FR-004, Quickstart §Operator No-submit Readiness]

## End-to-End Traceability

- [ ] CHK007 Is every required live-readiness stage traceable to a named source file, command, artifact path, and expected evidence field? [Traceability, Spec §Success Criteria, Quickstart §Tiny-capital Canary]
- [ ] CHK008 Are source-level trace requirements explicit enough to prove there is no alternate submit, cancel-as-submit, direct `LiveNode::run`, legacy runtime, environment-secret, or adapter-bypass path? [Traceability, FR-001, FR-002, FR-003, FR-004, FR-009]
- [ ] CHK009 Are provider, market-family, strategy, reference-data, and admission registries required to expose enough metadata for a reviewer to trace concrete config selections without hardcoded BTC, Binance, Polymarket, Chainlink, or one market family in core logic? [Traceability, FR-005, FR-006, FR-007, FR-008]
- [ ] CHK010 Are readiness and canary artifacts required to include exact commit SHA, executable identity, config bundle checksum, report `generated_at_unix_seconds`, TOML-owned report max age, SSM manifest hash, strategy-input hash, financial envelope hash, operator approval id hash, operator approval window, and produced artifact SHA? [Completeness, Spec §Key Entities, Quickstart §Tiny-capital Canary]
- [ ] CHK011 Are issue and PR traceability requirements defined so every blocker discovered during live-readiness tracing has a durable issue link, owner, evidence, and acceptance gate, including explicit #409 PortfolioSnapshot ledger state? [Gap, User Goal]

## Requirement Clarity

- [ ] CHK012 Is "production-grade live trade readiness" defined with measurable thresholds rather than implied by a passing canary or a command exit code? [Clarity, Spec §Success Criteria]
- [ ] CHK013 Is "real SSM/venue connectivity" defined as successful NT engine client connection state, reference instrument cache availability, and clean disconnect, rather than merely constructing clients or starting NT? [Clarity, Spec §User Story 4, Quickstart §Operator No-submit Readiness]
- [ ] CHK014 Is "zero orders" defined with evidence sources that distinguish no submit calls, no venue order ids, no order-intent-only records, and no unclaimed external orders? [Clarity, FR-012, Quickstart §Operator No-submit Readiness]
- [ ] CHK015 Is "strategy-driven cancel if open" defined with exact evidence required to prove NT strategy path ownership and exclude direct exec-engine test machinery? [Clarity, Spec §User Story 5, Quickstart §Tiny-capital Canary]
- [ ] CHK016 Is restart reconciliation defined with exact pre-run, shutdown, restart, cache/import, venue state, duplicate-submit prevention, and post-run evidence requirements? [Clarity, Spec §User Story 5, SC-006]

## Requirement Consistency

- [ ] CHK017 Do requirements consistently say that canary mode is the production path with caps, not a separate architecture or harness-only route? [Consistency, Plan §Summary, Plan §Eight-slice Plan, FR-013]
- [ ] CHK018 Do no-hardcode requirements align between runtime TOML, strategy TOML, provider bindings, test fixtures, and operator artifact examples? [Consistency, FR-004, FR-005, FR-006, FR-007, FR-008]
- [ ] CHK019 Do no-submit readiness requirements align with live canary gate requirements so a failed, skipped, cache-only, stale, expired, or mismatched stage cannot be treated as usable gate evidence? [Consistency, Spec §User Story 4, SC-005]
- [ ] CHK020 Do production-readiness requirements avoid conflicting with the rule that NT owns lifecycle, cache, reconciliation, adapters, and venue protocol behavior? [Consistency, FR-011, Plan §Constitution Check]

## Acceptance Criteria Quality

- [ ] CHK021 Can SC-005 be objectively evaluated from the readiness report fields, NT logs, and artifact hashes without relying on a successful process exit alone? [Acceptance Criteria, SC-005]
- [ ] CHK022 Can SC-006 be objectively evaluated from redacted artifacts that bind live order count, notional cap, NT submit evidence, venue state, cancel if needed, and restart reconciliation to the same run, without treating #360 closure as T046 proof? [Acceptance Criteria, SC-006]
- [ ] CHK023 Are pass/fail criteria defined for each live-readiness blocker class: schema/config, SSM, build features, adapter protocol, reference cache, gate linkage, submit admission, venue response, cancel, and restart reconciliation? [Gap]
- [ ] CHK024 Are requirements explicit that local mock tests, fixture reports, or source fences are supporting evidence only and cannot replace real SSM/venue/operator artifacts? [Acceptance Criteria, Spec §User Story 4, Spec §User Story 5]
- [ ] CHK025 Are escalation criteria specified for when a blocker requires upstream NT changes rather than Bolt-side workaround code? [Acceptance Criteria, Spec §Edge Cases, FR-011]

## Scenario Coverage

- [ ] CHK026 Are primary-flow requirements complete for no-submit readiness: approved run, exact config, SSM resolution, all configured clients connected, live reference-data freshness evidence beyond cache-only instrument IDs, zero orders, clean disconnect, fresh generated timestamp, and redacted report accepted by the gate? [Coverage, Spec §User Story 4]
- [ ] CHK027 Are exception-flow requirements complete for no-submit readiness: credential shape failure, missing SSM parameter, unavailable transport feature, venue protocol mismatch, partial client connection, missing reference cache, report write failure, and stale report? [Coverage, Gap]
- [ ] CHK028 Are primary-flow requirements complete for production-grade live trading after canary: repeated approved runs, caps adjusted by config, monitoring evidence, reconciliation after restart, and no core code edits for new venues/strategies? [Coverage, Gap]
- [ ] CHK029 Are recovery-flow requirements complete for failed live startup, partially connected clients, open order after canary, rejected venue order, process crash, and restart reconciliation disagreement? [Coverage, Gap]
- [ ] CHK030 Are non-functional requirements complete for auditability, secret redaction, deterministic artifact hashing, latency/timeout bounds, dependency pinning, and operator rollback? [Non-Functional, Gap]

## Dependencies & Assumptions

- [ ] CHK031 Are assumptions about Binance SBE schema compatibility, Polymarket transport backend availability, and Chainlink/reference source availability documented as live-readiness dependencies rather than implicit code facts? [Assumption, Gap]
- [ ] CHK032 Are AWS account, region, SSM KMS key, parameter versioning, and permission requirements documented without exposing credential values? [Dependency, FR-004]
- [ ] CHK033 Are operator approval boundaries specified for SSM mutation, no-submit connectivity, tiny-capital submit, production-cap submit, issue/PR mutation, approval-window expiry, nonce consumption, and replay rejection? [Dependency, Quickstart §Tiny-capital Canary]
- [ ] CHK034 Is a production-readiness issue backlog required to remain open until each end-to-end blocker has current evidence and a passing gate, and do closed historical issues such as #360 avoid implying T046 completion? [Dependency, User Goal]

## TDD And Verification Discipline

- [ ] CHK035 Are implementation requirements written so each future fix can be delivered as one vertical TDD slice with a public behavior test before code changes? [TDD, FR-014]
- [ ] CHK036 Are red/green expectations required to be captured for fixes to misleading readiness stage status, transport backend validation, adapter protocol drift, and artifact gate acceptance? [TDD, Gap]
- [ ] CHK037 Are completion requirements explicit that `superpowers:verification-before-completion` and exact-head verification must run before claiming any readiness phase complete? [Verification, FR-014]
- [ ] CHK038 Are hard-evidence requirements explicit enough to prohibit speculative readiness assertions, stale branch evidence, or proxy-only success signals? [Verification, Plan §Constitution Check]
