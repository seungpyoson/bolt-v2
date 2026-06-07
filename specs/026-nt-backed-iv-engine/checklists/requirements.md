# Specification Quality Checklist: NT-Backed IV Engine

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-06-07
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details leak into user-facing outcomes beyond required NT capability boundaries
- [x] Focused on operator and strategy-author value
- [x] Written without asset, venue, market, cadence, or strategy examples
- [x] Bolt-owned names use IV or implied-volatility terminology rather than standalone volatility terminology
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No unresolved clarification markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria avoid hardcoded asset, venue, market, cadence, and strategy examples
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded to IV and excludes FV/RV implementation
- [x] Dependencies and assumptions identified
- [x] Source lifecycle and strategy access satisfy group-by-change
- [x] Aggregate greeks are modeled as a first-class IV product
- [x] Interpolation, fallback, extrapolation, and quorum policies are testable
- [x] Projection and derived-input policies are explicit
- [x] Selector types are modeled as a Rust-validated union
- [x] Provenance schema is defined for raw, indexed, derived, projected, and rejected outputs
- [x] Live-node/config/strategy-registration integration is explicitly planned

## External Review Blockers

- [x] Source-fence enforcement mechanism is specified
- [x] NT capability ledger generation mechanism is specified
- [x] Selector type system is specified
- [x] Provenance schema is specified
- [x] NT runtime integration surface is specified
- [x] Derived IV query inputs are specified
- [x] Projection policy entity is specified
- [x] Derived-input policy entity is specified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] Blocking findings from internal and external review are represented as requirements

## Notes

- The spec intentionally requires a pinned NT capability ledger because "use all NT offers" is otherwise unverifiable.
- The spec intentionally exposes raw NT payloads through the IV engine so strategies can use the full NT capability set without owning subscription mechanics.
- The spec intentionally groups IV sources and strategy authorization inside one IV profile to avoid multi-section source swaps.
- Active Speckit pointers remain pinned to the existing source-fence-owned order-intent packet; this IV packet is addressed by explicit path.
