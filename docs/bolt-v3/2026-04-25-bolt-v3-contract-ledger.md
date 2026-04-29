# bolt-v3 Contract Ledger

Status: working normalization ledger for architecture review

This file exists to keep the bolt-v3 spec set mechanically closed.
Each load-bearing rule has one canonical owner and zero duplicate owners.

This ledger is an index, not a spec. Each entry holds only:

- the invariant name
- the canonical owner pointer
- dependent references
- implementation status

Rule prose lives in the canonical owner doc. Do not restate rules here.

## 1. Canonical validation outcome

- invariant:
  - canonical validation entrypoint and result-class taxonomy
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 1 and 2
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 8
- implementation status:
  - contract accepted; implementation evidence required

## 2. Rotating-market load freshness

- invariant:
  - rotating-market instrument load mechanism and freshness
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 5.3
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 5 `[venues.<identifier>.data]`

## 3. Root risk authority

- invariant:
  - root risk authority for Bolt-owned strategy sizing is live now; NautilusTrader live risk-engine fields are explicit in TOML and mapped into `LiveRiskEngineConfig`
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 4
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Sections 5 and 7

## 4. Identity model

- invariant:
  - trader, strategy, and Nautilus `StrategyId` derivation rules
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Sections 5 and 7
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.3

## 5. Secret source and env fallback

- invariant:
  - SSM-only secret source and per-venue-kind environment-variable blocklist
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 3
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 5 `[venues.<identifier>.secrets]`
  - `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md` D1, D5, R17, R18
- implementation status:
  - contract accepted; env-blocking tests required

## 6. Order boundary

- invariant:
  - NautilusTrader-native order boundary; no bolt executable-order schema or intent layer
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 8
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 7 `[parameters.entry_order]` / `[parameters.exit_order]`

## 6a. NautilusTrader Portfolio, Bolt allocation state, and execution legs

- invariant:
  - NautilusTrader Portfolio truth boundary, Bolt allocation state recomputation rule, and the execution-leg model
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 4.1 and 4.2
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.3
- implementation status:
  - contract accepted; event-schema and allocation-state tests required

## 6b. Strategy modularity boundary

- invariant:
  - current strategy behavior is the behavioral reference, not the monolithic file structure to copy
  - pricing, reference-data fusion, market identity, risk and sizing, decision evaluation, and execution mapping live behind separately testable module boundaries
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 8.5
- implementation status:
  - contract accepted; strategy-related implementation slices must preserve thin actor orchestration and module-level tests

## 7. Target-stack data model

- invariant:
  - target-stack five-shape model and current `updown` selected-market identifier
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 6
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.5
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 7 `[target]`

## 8. Reference-data contract

- invariant:
  - reference-data contract and the Gamma narrow-supplement role for `price_to_beat_value`
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Sections 7 and 8
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 1 and 2
- implementation status:
  - contract accepted; event page slug mapping and live Gamma readiness evidence required

## 9. Decision-event contract

- invariant:
  - fixed decision-event set with common required fields and event-specific shapes
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9
- implementation status:
  - contract accepted; schema and round-trip tests required

## 9a. Current sizing and exit mechanics

- invariant:
  - current sizing and exit mechanics: gross collateral notional terms, NautilusTrader-derived exposure, and the mechanical/strategy split
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 7.3, 7.4, and 8.4
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 7 `[parameters]`
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.5 `entry_evaluation`
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.5 `entry_pre_submit_rejection`
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.5 `exit_evaluation`
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.5 `exit_order_submission`
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.5 `exit_pre_submit_rejection`
- implementation status:
  - contract accepted; S2/S3/S4 production-path coverage required

## 10. Local evidence persistence

- invariant:
  - broad NautilusTrader subscription/capture outside the lean submit-critical hot path; NT-owned facts retain NT names
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 9.1, 9.6, 9.7, and 10
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 5 `[persistence]` and `[persistence.streaming]`
- implementation status:
  - contract accepted; broad-subscription evidence, raw-capture catalog round-trip, persistence-failure tests, and full NT-owned field naming audit required

## 11. Release identity and deploy trust

- invariant:
  - release identity manifest and deploy-trust contract
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 11
- implementation status:
  - contract accepted; deploy evidence required

## 12. NautilusTrader pin governance

- invariant:
  - NautilusTrader pin governance and dedicated pin-change verification slice
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 11.5
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 9.3
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 11.2
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 12
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 13
- implementation status:
  - current branch contains a dedicated NT pin-bump compatibility slice for `56a438216442f079edf322a39cdc0d9e655ba6d8`; future pin changes still require re-verification

## 13. Panic and service policy

- invariant:
  - panic gate and current systemd service policy
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 11.4 and 12
- implementation status:
  - contract accepted; issue `#239` evidence required

## 14. CLOB V2 readiness

- invariant:
  - Polymarket CLOB V2 signing readiness gate
- canonical owner:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 13
- implementation status:
  - partial evidence: upstream NT CLOB V2 support is now pinned and focused compile/tests pass; live signing, order, fill, collateral, and fee verification still block live capital

## 15. Bolt-v3 NT-first boundary doctrine

- invariant:
  - Bolt-v3 NT-first boundary doctrine over NautilusTrader's Rust factory path,
    including approved decisions, open decisions, named residuals, verifier
    coverage, and future slice gates
- canonical owner:
  - `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md`
- dependent references:
  - `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 1, 3, and 11.5
  - `docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md` Entries 1, 5, and 12
- implementation status:
  - approved doctrine; verifier locations are not yet selected
