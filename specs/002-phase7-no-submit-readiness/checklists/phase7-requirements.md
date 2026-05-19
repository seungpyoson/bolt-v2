# Phase 7 Requirements Checklist

**Purpose**: Unit tests for Phase 7 requirements writing, not implementation behavior; unchecked boxes below are prompts for spec review, not evidence that implementation work is incomplete.
**Created**: 2026-05-14
**Feature**: [spec.md](../spec.md)

## Scope

- [ ] CHK001 Is no-submit readiness clearly bounded to authenticated readiness evidence, not live capital? [Scope, Spec FR-015]
- [ ] CHK002 Is startup/build readiness clearly distinguished from no-submit readiness? [Scope, Spec FR-002]
- [ ] CHK003 Are stale PR #318/#319/#320 reference-only constraints explicit? [Scope, Spec FR-014]
- [ ] CHK004 Is Phase 8 strategy-input safety review required before live action? [Scope, Spec US3]

## Fail-closed Behavior

- [ ] CHK005 Are missing, malformed, oversized, or unsatisfied reports covered by requirements? [Coverage, Spec Edge Cases]
- [ ] CHK006 Are SSM resolver failures and invalid secret material covered by requirements? [Coverage, Spec FR-005, FR-007]
- [ ] CHK007 Are venue auth, geo block, wrong market, wrong instrument, stale data, and missing reference readiness covered? [Coverage, Spec Edge Cases]
- [ ] CHK008 Is missing or whitespace configured approval required to fail before any secret or venue side effect? [Clarity, Spec FR-008]

## NT Boundary

- [ ] CHK009 Is NT ownership of adapter behavior, lifecycle, cache, order state, reconciliation, and venue wire behavior explicit? [Consistency, Spec FR-012]
- [ ] CHK010 Is the prohibition on raw mutable `LiveNode` escape hatches explicit? [Boundary, Spec FR-013]
- [ ] CHK011 Are allowed NT operations limited to controlled connect/readiness/disconnect? [Clarity, Spec US1, US2]

## SSM-only Secrets

- [ ] CHK012 Is AWS SSM through Rust SDK the only allowed secret source? [Security, Spec FR-007]
- [ ] CHK013 Is environment fallback excluded while keeping operator approval id in TOML config? [Consistency, Spec FR-008, FR-011]
- [ ] CHK014 Are secret redaction requirements measurable? [Measurability, Spec SC-004]

## No Live-capital Gate

- [ ] CHK015 Is live submit explicitly excluded from Phase 7? [Scope, Spec FR-015]
- [ ] CHK016 Is soak explicitly excluded from Phase 7? [Scope, Spec FR-015]
- [ ] CHK017 Are exact-head and exact-command approval requirements reserved for later live action? [Dependency, Spec FR-015]

## Evidence Artifacts

- [ ] CHK018 Is report schema compatibility with live-canary gate required? [Traceability, Spec FR-004]
- [ ] CHK019 Are exact command, SHA, config checksum, report path, and result required for real no-submit evidence? [Completeness, Spec FR-016]
- [ ] CHK020 Is report output path controlled by config instead of code literals? [Consistency, Spec FR-010]

## Testability

- [ ] CHK021 Are local tests required before real SSM/venue operation? [Testability, Spec SC-001 to SC-005]
- [ ] CHK022 Are source fences required for submit, cancel, replace, amend, subscribe, and runner-loop tokens? [Testability, Spec SC-002]
- [ ] CHK023 Is the real operator harness required to be ignored by default? [Testability, Spec SC-005]

## Out-of-scope Soak

- [ ] CHK024 Is any soak or live-capital action deferred to explicit later approval? [Scope, Spec FR-015]
- [ ] CHK025 Is Phase 8 still blocked without real no-submit report plus strategy-input safety approval? [Gate, Spec SC-007]
