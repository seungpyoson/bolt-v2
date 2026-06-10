# Implementation Plan: World Cup Production Profit Gate

**Branch**: `codex/032-world-cup-production-profit-gate`
**Date**: 2026-06-10
**Spec**: `specs/032-world-cup-production-profit-gate/spec.md`

## Summary

Build a production-profit gate for World Cup event markets. The gate is provider-neutral, source-proofed, and NT-first. It turns verified event/venue/provider/source evidence plus NT-backed profit evidence into disabled promotion config and later no-submit/canary eligibility. It does not authorize live capital.

## Technical Context

- **Language**: Rust.
- **Runtime**: Pure Rust binary using NautilusTrader Rust APIs.
- **Config**: Existing TOML config only.
- **Secrets**: AWS SSM through the Rust resolver only.
- **Existing modules to reuse**:
  - `src/bolt_v3_executable_edge.rs`
  - `src/bolt_v3_book_sizing.rs`
  - `src/bolt_v3_quote_lifecycle.rs`
  - `src/bolt_v3_maker_model.rs`
  - `src/bolt_v3_maker_inventory.rs`
  - `src/bolt_v3_maker_reservation.rs`
  - `src/bolt_v3_submit_admission.rs`
  - `src/bolt_v3_no_submit_readiness.rs`
  - `src/bolt_v3_live_canary_gate.rs`
  - `src/venue_contract.rs`
- **Existing contracts/specs to respect**:
  - `contracts/polymarket.toml`
  - `specs/023-nt-order-intent-layer/plan.md`
  - `specs/023-nt-research-analytics-platform/`
  - `specs/024-production-trade-readiness/`

## Constitution Check

- **NO HARDCODES**: PASS. World Cup rules, provider roles, thresholds, and source URLs are TOML/artifact-owned.
- **NO DUAL PATHS**: PASS. The package routes through existing NT-backed modules and shared gates.
- **NO DEBTS**: PASS. This plan defines explicit slices and rejection states.
- **NO CREDENTIAL DISPLAY**: PASS. Runtime secrets are never printed and are SSM-only.
- **PURE RUST BINARY**: PASS. No Python or notebook execution path.
- **SSM SINGLE SECRET SOURCE**: PASS. No runtime fallback source is introduced.
- **GROUP BY CHANGE**: PASS. Provider capability, reference quorum, venue rules, and promotion package boundaries are grouped by lifecycle.
- **DO NOT REFERENCE BOLT V1**: PASS. Only current repo and NT source are in scope.
- **STRATEGIES PRODUCE INTENT ONLY**: PASS. Strategy-local logic remains signal/intent only; execution/admission owns submit mechanics.
- **Guarded Spec Kit pointer**: PASS. `AGENTS.md` and `.specify/feature.json` remain pinned to `specs/023-nt-order-intent-layer/plan.md`; this package is referenced by explicit path.

## Build Target

The exact build is a new gate layer, not a new trading venue or strategy fork:

1. `src/bolt_v3_event_market_source_proof.rs`
2. `src/bolt_v3_provider_capability.rs`
3. `src/bolt_v3_profit_evidence.rs`
4. `src/bolt_v3_live_enablement_gate.rs`
5. CLI/operator-artifact commands that materialize and verify source proof, capability proof, profit evidence sessions, disabled promotion packages, and live enablement gate packets.

Names may be adjusted to existing local module naming during implementation, but responsibilities must remain separate and shared.

## Implementation Slices

### Slice 1 - Source-Proof Admission

Add types and validation for event-market source proof:

- official event/schedule source URL and hash
- venue market term URL/hash
- resolution rule fields
- jurisdiction/account/product availability
- source retrieval timestamp and expiry
- rejection reasons for missing, stale, conflicting, or unsupported proof

Output: candidate can enter `capture_eligible` or is rejected before strategy evaluation.

### Slice 2 - Provider Capability And Quorum

Add provider-neutral capability records and TOML-owned reference quorum policy:

- transport class: REST, SSE, WebSocket, notification-plus-refresh
- supported market/league/source coverage
- historical tick and order-book-depth support
- latency/freshness and plan entitlement
- primary/backup/veto role assignment

Output: source-proofed candidates receive accepted reference-data roles or fail closed.

### Slice 3 - Profit Evidence Session

Bind existing NT-backed evidence into one session artifact:

- candidate and no-trade observations
- exact-size VWAP executable-edge inputs/results
- quote lifecycle/cancel outcomes
- fills where available
- markouts
- settlement outcomes
- replay/shadow/no-submit fidelity class

Output: promotion-ready only when thresholds pass with accepted evidence class.

### Slice 4 - Disabled Promotion Package

Generate disabled typed TOML and a promotion report:

- source-proof hash
- provider-capability hash
- profit-evidence hash
- commit SHA and config checksum
- disabled-by-default strategy config
- explicit non-live status

Output: operator-reviewable package only; no live enablement.

### Slice 5 - No-Submit And Tiny Canary Eligibility

Integrate promotion package with existing production readiness gates:

- exact-head CI and source-fence
- current no-submit report
- operator approval packet
- geography/account/product availability
- tiny-capital canary proof
- unresolved finding review

Output: canary-ready state only for exact venue/account/market family/config hash.

## Acceptance Gate

Before implementation is considered ready for review:

- `cargo test --locked --lib`
- `cargo fmt --check`
- `git diff --check`
- `just source-fence`
- targeted tests for every rejection reason
- exact-head CI before external review or any no-submit/canary operation

## Out of Scope

- Live World Cup trading authorization.
- Provider purchase or plan upgrade.
- Legal advice.
- Undocumented endpoint scraping.
- Any runtime secret source outside AWS SSM.
- Any strategy-local submit mechanics.
