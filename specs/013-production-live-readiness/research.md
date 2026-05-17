# Research: Production Live Readiness

## Decision: Separate Readiness Claims By Evidence Level

Production readiness must be expressed as three explicit levels: tiny-canary ready, staged-live ready, and production-live ready.

**Rationale**: Issue #360 proves one controlled canary path. Repeated live operation adds restart, replay, monitoring, deploy, incident, and hygiene risks that one canary cannot cover.

**Alternatives considered**:

- Single "live ready" checklist: rejected because it hides the difference between one canary and repeated operation.
- Production readiness as a comment-only convention: rejected because reviewers need a repo artifact and test gate.

## Decision: Canonical Contract In docs/bolt-v3

The canonical Issue #369 claim-level contract lives at `docs/bolt-v3/2026-05-18-production-readiness-contract.md`.

**Rationale**: `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` already acts as the source-backed roadmap. The new readiness contract needs to be visible from the same documentation authority and contract ledger.

**Alternatives considered**:

- SpecKit-only contract: rejected because active Bolt-v3 docs are the operator-facing control surface.
- Status-map row only: rejected because the row cannot hold all promotion evidence and runbook requirements.

## Decision: Link Missing Staged/Production Tooling As Blockers

Issue #369's first slice defines and links required tests/tooling rather than implementing every staged-live and production-live blocker.

**Rationale**: The issue acceptance asks to define production readiness and add or link tests/tooling requirements. Rows 34-48 of the status map already list source-backed missing implementation areas. Implementing all of them in this branch would violate minimal slice discipline.

**Alternatives considered**:

- Implement order lifecycle, restart reconciliation, monitoring, deploy provenance, and panic policy in one PR: rejected as too broad and unsafe.
- Close the issue with a prose-only doc: rejected because a regression test should protect the required artifact surface.

## Decision: No Live Capital In This Slice

Issue #369 implementation must not run tiny live canary, production live trading, capital transfer, or any live submit.

**Rationale**: The objective explicitly separates no-submit readiness, tiny live canary, and production readiness. Live submit still requires explicit operator approval.

**Alternatives considered**:

- Use a live run to validate production readiness: rejected because production readiness is broader than one run and live submit lacks explicit approval here.
