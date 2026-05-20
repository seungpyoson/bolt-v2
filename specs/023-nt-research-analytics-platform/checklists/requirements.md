# Specification Quality Checklist: NT-First Research Analytics Platform

**Purpose**: Validate the planning package before issue creation or implementation.
**Created**: 2026-05-20
**Feature**: `specs/023-nt-research-analytics-platform/spec.md`

## Evidence Quality

- [x] CHK001 Does every major claim trace to `evidence.md`? [Evidence, FR-017]
- [x] CHK002 Is Kalshi adapter support labeled as a user assumption, not source proof? [Evidence, E-008]
- [x] CHK003 Is upstream HIP-4 support treated as NT-first and not as a Bolt adapter build? [Evidence, E-003..E-006]
- [x] CHK004 Is HIP-4 historical-data support separated from live adapter support? [Evidence, E-007]
- [x] CHK005 Is external review treated as challenge input instead of authority by consensus? [Process, research.md]

## Requirement Completeness

- [x] CHK006 Are backtesting and catalog requirements NT-first? [FR-001, FR-002]
- [x] CHK007 Are provider cost and fidelity gates explicit? [FR-004, FR-005]
- [x] CHK008 Is the dashboard read-only and NT-derived? [FR-010..FR-012]
- [x] CHK009 Are existing issues identified before new issue payloads? [FR-014]
- [x] CHK010 Is venue/product/provider identity config/registry-selected, not hardcoded into core logic? [E-026, FR-016]
- [ ] CHK011 Is the exact selected NT pointer proven by compile/API checks? [Pending, T004]

## Cost And Fidelity

- [x] CHK012 Does the plan record Tardis Professional at `$900/month` before selection? [E-011]
- [x] CHK013 Does the plan distinguish Telonex personal price from commercial/team use? [E-015]
- [ ] CHK014 Is the selected all-in monthly mode proven under the approved cap? [Pending, cost-model.md]
- [x] CHK015 Does every venue/source family have an initial fidelity class? [Draft, fidelity-matrix.md]
- [ ] CHK016 Is Kalshi historical L2 replay proven or downgraded? [Pending, T006/T010]

## Scope Control

- [x] CHK017 Does #115 get treated as stale on NT HIP-4 support? [E-020]
- [x] CHK018 Does dashboard PnL completeness depend on #409, #77, #36 inclusion/exclusion, and #369 non-closure context? [E-018, E-020]
- [x] CHK019 Does the contract prohibit a Bolt backtest engine or executable order schema without evidence? [Contract]
- [x] CHK020 Do tasks avoid GitHub issue mutation without user approval? [T040]
- [x] CHK024 Is current SpecKit path resolution documented, including branch-local 023 pointer and explicit 023 read-only override? [analysis.md, tasks.md]
- [x] CHK025 Does dashboard planning evaluate existing BI/observability products before bespoke UI? [E-028, T030]

## Open Gates

- [x] CHK021 External/adversarial review has challenged the evidence ledger as review input, not source authority. [Review pass recorded in analysis.md]
- [ ] CHK022 User has approved issue payload creation or mutation. [Pending, T040]
- [ ] CHK023 Issue payload drafts plus cost/fidelity artifacts have received a follow-up adversarial review after latest fixes. [Pending, T039]
