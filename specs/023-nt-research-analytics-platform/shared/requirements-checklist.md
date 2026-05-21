# Specification Quality Checklist: NT-First Research Planning Package

**Purpose**: Validate the planning package before issue creation or implementation.
**Created**: 2026-05-20
**Refreshed**: 2026-05-21
**Feature**: `specs/023-nt-research-analytics-platform/spec.md`

## Evidence Quality

- [x] CHK001 Does every major claim trace to `evidence.md`? [Evidence, root spec shared requirements]
- [x] CHK002 Is Kalshi adapter support labeled as a user assumption, not source proof? [Evidence, E-008]
- [x] CHK003 Is upstream HIP-4 support treated as NT-first and not as a Bolt adapter build? [Evidence, E-003..E-006]
- [x] CHK004 Is HIP-4 historical-data support separated from live adapter support? [Evidence, E-007]
- [x] CHK005 Is review feedback treated as challenge input instead of authority by consensus? [Process, research.md]

## Requirement Completeness

- [x] CHK006 Are backtesting and catalog requirements NT-first? [`1-backtesting-engine/spec.md`, E-001, E-002]
- [x] CHK007 Are provider cost-review and fidelity gates explicit? [`cost-model.md`, `fidelity-matrix.md`]
- [x] CHK008 Is the dashboard read-only and NT-derived? [`3-dashboard/spec.md`, E-017, E-018]
- [x] CHK009 Are existing issues identified before new issue payloads? [`issue-audit.md`, `github-issues.md`]
- [x] CHK010 Is venue/product/provider identity config/registry-selected, not hardcoded into core logic? [E-026, root spec shared requirements]
- [x] CHK026 Does the research include external OSS and commercial prior art beyond user-provided examples? [E-030, E-031, research.md]
- [x] CHK011 Does the plan require target `bolt-v2` NT-version compile/API proof before any vertical implementation starts? [E-027, analysis.md]

## Cost And Fidelity

- [x] CHK012 Does the plan model cost as a review/cut lever, not a first-pass architecture limiter? [`cost-model.md`, E-024]
- [x] CHK013 Does the plan distinguish personal/commercial/license proof for vendor sources? [Provider gates]
- [x] CHK014 Does the cost model require each selected all-in monthly mode to be estimated and flagged for user review if over target? [cost-model.md]
- [x] CHK015 Does every venue/source family have an initial fidelity class? [Draft, fidelity-matrix.md]
- [x] CHK016 Is Kalshi historical L2 replay either proven or explicitly downgraded until proof exists? [E-009, fidelity-matrix.md]

## Scope Control

- [x] CHK017 Does #115 get treated as stale on NT HIP-4 support? [E-020]
- [x] CHK018 Does dashboard PnL completeness depend on #409, #77, #36 inclusion/exclusion, and #369 non-closure context? [E-018, E-020]
- [x] CHK019 Does the contract prohibit a Bolt backtest engine or executable order schema without evidence? [Contract]
- [x] CHK020 Do tasks avoid GitHub issue mutation without user approval? [root `tasks.md` stop conditions]
- [x] CHK024 Is current SpecKit path resolution documented, including branch-local 023 pointer and explicit 023 read-only override? [analysis.md, tasks.md]
- [x] CHK025 Does dashboard planning evaluate existing BI/observability products before bespoke UI? [E-028, `3-dashboard/plan.md`]

## Open Gates

- [x] CHK021 External/adversarial review has challenged the evidence ledger as review input, not source authority. [Review pass recorded in analysis.md]
- [ ] CHK022 User has approved issue payload creation or mutation. [Pending, root `tasks.md` stop conditions]
- [x] CHK023 Issue payload drafts plus cost/fidelity artifacts have received a follow-up adversarial review after latest fixes. [analysis.md]
