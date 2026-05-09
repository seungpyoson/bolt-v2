# Implementation Plan: Bolt-v3 Nucleus Admission Audit

**Branch**: `001-v3-nucleus-admission` | **Date**: 2026-05-09 | **Spec**: `specs/001-v3-nucleus-admission/spec.md`
**Input**: Feature specification from `specs/001-v3-nucleus-admission/spec.md`

## Summary

Add a report-only Bolt-v3 nucleus admission audit that answers one question:
is the current V3 foundation admitted for further behavior work? The audit must
scan the generic V3 boundary, current verifier allowlists, fixture fences, and
required contract surfaces, then report blockers with evidence. Default mode is
non-blocking. Strict mode exits nonzero and is reserved for a follow-up CI gate.

This plan deliberately avoids implementing Polymarket, Binance, Kalshi,
Hyperliquid, Chainlink, BTC, updown, `binary_oracle_edge_taker`, live trading,
or legacy strategy migration. Those names are evidence targets only.

## Technical Context

**Language/Version**: Python 3 for repository verifier tooling; Rust production
runtime unchanged.
**Primary Dependencies**: Python standard library; existing `just` recipes;
existing Rust source/fixture tree. No new third-party dependencies.
**Storage**: Repository files only; no database or external service.
**Testing**: Python self-tests invoked directly by `python3`, matching existing
Bolt-v3 verifier tests.
**Target Platform**: Developer and CI shell on macOS/Linux.
**Project Type**: Internal repository governance/tooling.
**Performance Goals**: Scan tracked UTF-8 source, tests, fixtures, scripts, and
V3 docs quickly enough for a local pre-CI command; target under 5 seconds on the
current repository.
**Constraints**: No production runtime behavior changes; no required CI gate in
this slice; no credentials, AWS, network, venue, or live order access.
**Scale/Scope**: Current repository plus future Bolt-v3 files under the same
source/test/fixture/doc conventions.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- Evidence-First Architecture: PASS. The plan uses current source, fixture,
  verifier, issue, and CI evidence and names the cited paths.
- Generic Contract Boundaries: PASS. Concrete provider/family/strategy names
  are blocker evidence and fixture-fencing tokens only; they are not runtime
  architecture.
- Single Source Runtime Configuration: PASS. The feature does not add runtime
  config or secret paths.
- NT-First Pure Rust Runtime: PASS. Python is used only for repository verifier
  tooling, matching existing verifier scripts. The production runtime remains a
  standalone Rust binary.
- Empirical Readiness And Review Gates: PASS. The feature adds evidence gates
  and self-tests before any further V3 behavior work.
- Scope And Source Of Truth: PASS. The branch starts from current `main`; stale
  branch evidence is not used as implementation proof.
- Bolt-v3 Nucleus Admission Rules: PASS. This feature exists to enforce those
  rules and does not implement provider-specific behavior.
- Review And Delivery Workflow: PASS. This plan is backed by spec-kit artifacts
  and will produce tasks before implementation.

## Project Structure

### Documentation (this feature)

```text
specs/001-v3-nucleus-admission/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── admission-audit-cli.md
│   └── admission-report.md
└── tasks.md
```

### Source Code (repository root)

```text
scripts/
├── verify_bolt_v3_nucleus_admission.py
└── test_verify_bolt_v3_nucleus_admission.py

justfile
```

**Structure Decision**: Follow the existing Bolt-v3 verifier pattern in
`scripts/` and expose one `just` recipe. Do not edit production Rust modules in
this slice.

## Complexity Tracking

No constitution violations are introduced. The only notable tradeoff is using
Python for verifier tooling, which is already the repository pattern for
`scripts/verify_bolt_v3_runtime_literals.py` and
`scripts/verify_bolt_v3_provider_leaks.py`; it is not a production runtime
layer.

## Phase 0 Research

See `specs/001-v3-nucleus-admission/research.md`.

## Phase 1 Design

See:

- `specs/001-v3-nucleus-admission/data-model.md`
- `specs/001-v3-nucleus-admission/contracts/admission-audit-cli.md`
- `specs/001-v3-nucleus-admission/contracts/admission-report.md`
- `specs/001-v3-nucleus-admission/quickstart.md`

## Post-Design Constitution Check

- Evidence-First Architecture: PASS. The report contract requires path,
  excerpt or absence proof, blocker id, and retirement condition.
- Generic Contract Boundaries: PASS. Blocker classes distinguish generic-core
  leakage from fenced fixtures and provider-owned bindings.
- Single Source Runtime Configuration: PASS. The audit checks unowned defaults
  but does not add runtime values.
- NT-First Pure Rust Runtime: PASS. Production runtime remains untouched.
- Empirical Readiness And Review Gates: PASS. Default and strict modes make the
  gate measurable without prematurely failing required CI.
- Scope And Source Of Truth: PASS. The feature is one slice: report-only audit.
- Bolt-v3 Nucleus Admission Rules: PASS. The design maps directly to the
  nucleus admission blockers.
- Review And Delivery Workflow: PASS. Tasks and analysis remain next steps
  before implementation.
