# Implementation Plan: Phase 6 Submit Admission Recovery

**Branch**: `main` for planning artifacts only; fresh implementation branch required later  
**Date**: 2026-05-13  
**Spec**: `specs/002-phase6-submit-admission-recovery/spec.md`
**Input**: Feature specification from `specs/002-phase6-submit-admission-recovery/spec.md`

## Summary

Recover Phase 6 submit admission without continuing stale PR chain work. PR #317 is reference-only: keep proven Phase 6 ideas, rewrite them against current `main`, reject stale Phase 3-5 code. Before implementation, produce a durable recovery memo and get recovery-strategy review. Implementation happens later as one fresh narrow PR from current `main`.

## Technical Context

**Language/Version**: Rust for eventual implementation; Markdown for this planning slice  
**Primary Dependencies**: Existing bolt-v2 Rust crate, NautilusTrader Rust APIs, existing SpecKit docs  
**Storage**: TOML config for runtime values; Markdown planning artifacts for recovery decisions  
**Testing**: Planning checks by direct artifact inspection; future implementation by targeted `cargo test`, source-fence checks, CI  
**Target Platform**: Existing bolt-v2 production-shaped Rust binary environment  
**Project Type**: Rust application with SpecKit planning docs  
**Performance Goals**: No runtime performance target in this planning slice; future Phase 6 must add no per-tick cost and only gate submit attempts  
**Constraints**: No stale branch continuation, no direct cherry-pick, no Phase 3-5 churn, no Phase 7-8 behavior, no new secret source, no alternate submit path  
**Scale/Scope**: One recovery plan, one recovery memo, one future fresh Phase 6 PR

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **I. NT-First Thin Layer**: PASS. Plan keeps submit admission as pre-submit gate only. NT remains owner of order lifecycle, adapters, reconciliation, and runtime.
- **II. Generic Core, Concrete Edges**: PASS. Recovery rejects any concrete provider/strategy leakage from stale PRs into core admission or runtime loops.
- **III. Single Path And Config-Controlled Runtime**: PASS. Plan requires one fresh Phase 6 path from current `main`; stale helper paths and alternate registration surfaces are rejected.
- **IV. Test-First Safety Gates**: PASS. Future implementation requires TDD: failing submit-admission tests first, then minimal code.
- **V. Evidence Before Claims**: PASS. Recovery memo must contain exact SHAs, current-main evidence, stale PR context, and keep/rewrite/reject map before implementation.
- **VI. Minimal Slice Discipline**: PASS. Future PR limited to Phase 6 submit admission. Phase 7-8 work stays out of scope.

## Project Structure

### Documentation (this feature)

```text
specs/002-phase6-submit-admission-recovery/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── recovery-review.md
├── quickstart.md
├── contracts/
│   └── recovery-review-contract.md
└── checklists/
    └── requirements.md
```

### Future Source Code Touch Surface

```text
src/
├── bolt_v3_submit_admission.rs          # New or recovered Phase 6 admission state
├── bolt_v3_strategy_registration.rs     # Add admission to current single registration path only
├── bolt_v3_live_node.rs                 # Arm admission without dropping existing runtime capture
├── bolt_v3_archetypes/
│   └── binary_oracle_edge_taker.rs      # Pass current context + admission; no writer duplication
├── strategies/
│   ├── registry.rs                      # Add mandatory admission to build context
│   └── eth_chainlink_taker.rs           # Enforce evidence -> admission -> submit
└── lib.rs                               # Export admission module if needed

tests/
├── bolt_v3_submit_admission.rs          # Admission state and ordering tests
├── bolt_v3_strategy_registration.rs     # Current single path retains admission/evidence wiring
├── bolt_v3_decision_evidence.rs         # Source-fence order: evidence before admission before submit
└── live_node_run.rs                     # Build/run contract keeps gate/capture/admission ordering
```

**Structure Decision**: This planning feature changes docs only. Future implementation must use the source touch surface above and reject all stale #317 files outside that surface unless the recovery memo justifies them.

## Phase 0: Research

Goal: prove recovery strategy before code.

Outputs:
- `research.md` with decisions and rejected alternatives.
- Exact stale/current evidence:
  - current `main` SHA
  - PR #317 head SHA
  - PR #317 base context
  - merge-base / diff-size drift
  - stale files that must not be ported

Gate:
- No implementation until research has no open decision gaps.

## Phase 1: Design And Contracts

Goal: define durable recovery artifacts and reviewer contract.

Outputs:
- `data-model.md`: recovery entities and validation rules.
- `contracts/recovery-review-contract.md`: required contents of recovery memo and external review prompt.
- `quickstart.md`: exact operator workflow from stale PR audit to fresh Phase 6 branch.
- `recovery-review.md`: concrete keep/rewrite/reject map for #317.

Gate:
- Recovery memo complete before recovery-strategy review.
- Planning artifacts committed or captured as an exact immutable snapshot before recovery-strategy review.
- Recovery-strategy review complete before implementation branch.

## Phase 2: Future Task Generation

Goal: generate implementation tasks only after recovery strategy review is resolved.

Expected task groups:
1. TDD submit-admission state tests.
2. Minimal admission module.
3. Context wiring through current #322 architecture.
4. Strategy ordering: evidence -> admission -> submit.
5. Live-node ordering: gate -> arm admission -> runtime capture -> NT run.
6. Anti-slop cleanup pass.
7. Targeted verification and CI.

## Complexity Tracking

No constitution violations.
