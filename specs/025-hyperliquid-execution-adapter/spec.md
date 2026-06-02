# Feature Specification: Hyperliquid Execution Adapter

**Feature Branch**: `codex/025-hyperliquid-execution-adapter`
**Created**: 2026-06-01
**Status**: Draft pending relay-Claude adversarial review
**Input**: User request: enable Hyperliquid execution adapter for spot, futures/perps, HIP-3, HIP-4 as a production-grade, colocated, fastest-latency adapter.

## Clarifications

- "Fastest latency" is treated as a measurable ops objective, not a guaranteed code property.
- "Colocated" means TOML-configured infrastructure profile and local info-node support; no region, endpoint, or facility value may be hardcoded.
- The first accepted implementation slice does not place live orders. It proves NT market-data adapter registration, execution adapter registration, discovery, secrets, signer ownership, fee/rate accounting, live-submit approval gating, and fail-closed submit behavior.

## User Stories And Tests

### User Story 1 - Register Hyperliquid Safely (Priority: P1)

As an operator, I can configure a Hyperliquid provider in the same provider registry used by other venues, with SSM-backed credentials and no raw client path, so the binary can construct the NT adapter without violating repo rules.

**Independent Test**: A TOML fixture with SSM paths validates and maps to an NT Hyperliquid config; fixtures with raw secrets, missing SSM paths, `HYPERLIQUID_*` env fallback, or raw `src/clients/hyperliquid.rs` are rejected.

### User Story 2 - Prove Product Discovery Matrix (Priority: P1)

As an operator, I can see an evidence-backed product matrix for standard perps, spot, HIP-3 builder perps, and HIP-4 outcomes before any submit path is opened.

**Independent Test**: Discovery tests prove each surface from NT-supported public metadata, record source evidence, and mark unsupported or unproven submit paths fail-closed.

### User Story 3 - Prove Standard-Perps Fail-Closed Preconditions (Priority: P2)

As an operator, I can prove standard-perps fee/rate policy and exchange-mutation guards without publishing a Hyperliquid-specific no-submit readiness artifact.

**Independent Test**: The shared exchange-mutation guard fails closed for submit, cancel, modify, transfer, or account-mutating counts; the pinned NT `userFees` weight mismatch blocks live submit at validation.

### User Story 4 - Gate Standard-Perps Adapter Mapping (Priority: P2)

As an operator, I can prove the standard-perps adapter mapper accepts NT execution config only when supplied an already-consumed approval artifact tied to current code, config, surface, limits, expiry, and provider identity.

**Independent Test**: Adapter mapping is rejected when the approval artifact is missing, stale, reused, mismatched, expired, or wider than configured order limits. Production live-node construction consumes the configured Hyperliquid approval artifact before adapter mapping and rejects replay after `used_at` is persisted.

### User Story 5 - Register Hyperliquid Market Data (Priority: P2)

As an operator, I can configure the NT Hyperliquid market-data client for live instruments, quotes, and order book data without enabling live submit or exposing signer material in the data path.

**Independent Test**: A Hyperliquid `[data]` block validates and maps to NT `HyperliquidDataClientConfig` plus `HyperliquidDataClientFactory`; data-only mapping requires no live-submit approval and leaves `private_key` unset.

### User Story 6 - Gate Spot, HIP-3, And HIP-4 Mapping By Surface Approval (Priority: P2)

As an operator, I can prove spot, HIP-3, and HIP-4 map through the same NT Hyperliquid execution adapter only when the configured product surface has a consumed approval artifact bound to that exact surface.

**Independent Test**: Attempts to map spot, HIP-3, or HIP-4 fail without a consumed surface-bound approval; a consumed approval for one surface cannot authorize a different surface.

### User Story 7 - Configure Latency Ops Separately (Priority: P3)

As an operator, I can configure local info-node and placement profile settings without changing execution semantics or adding hardcoded endpoints.

**Independent Test**: Latency profile config affects only market-data endpoints and exported artifacts; it cannot bypass submit gates, signer ownership, or rate accounting.

## Functional Requirements

- **FR-001**: The system MUST add Hyperliquid support through the pinned NT Hyperliquid Rust adapter from the same `nautilus_trader` revision used by the repo.
- **FR-002**: The system MUST register Hyperliquid only through `ProviderBinding`.
- **FR-003**: The system MUST forbid adding or using `src/clients/hyperliquid.rs`.
- **FR-004**: The system MUST source Hyperliquid private key, account address, vault address, and related credential material from AWS SSM only.
- **FR-005**: The system MUST reject or scrub `HYPERLIQUID_*` environment variables before NT Hyperliquid config handoff.
- **FR-006**: The system MUST require explicit account address configuration for API-wallet mode.
- **FR-007**: The system MUST model execution mode explicitly: direct account, vault, master-account API wallet, or subaccount API wallet.
- **FR-008**: The system MUST reject multiple execution clients sharing the same signer/API wallet unless a future signer lifecycle owner design explicitly allows it.
- **FR-009**: The system MUST account for `userFees` using the official request weight.
- **FR-010**: The system MUST inventory all NT `userFees` callers before enabling live submit.
- **FR-011**: The system MUST discover standard perps, spot, HIP-3, and HIP-4 through NT-supported or officially documented public surfaces.
- **FR-012**: The adapter mapper MUST keep spot execution blocked unless a current live-submit approval artifact is consumed for `spot`.
- **FR-013**: The adapter mapper MUST keep HIP-3 execution blocked unless a current live-submit approval artifact is consumed for `hip3_builder_perps`.
- **FR-014**: The adapter mapper MUST keep HIP-4 execution blocked unless a current live-submit approval artifact is consumed for `hip4_outcomes` and TOML config enables positive outcome settlement polling.
- **FR-015**: The adapter mapper MUST keep standard perps execution blocked unless a current live-submit approval artifact exists for `standard_perps`.
- **FR-016**: The live-submit approval artifact MUST bind base SHA, provider id, product surface, TOML checksum, signer fingerprint, order limits, expiry, and one-time id.
- **FR-017**: The system MUST treat Hyperliquid priority-fee grouping as out of MVP unless NT exposes and proves the required wire shape.
- **FR-018**: The system MUST provide TOML-configured local-info-node and colocation profile fields as ops metadata only.
- **FR-019**: The system MUST keep strategies intent-only and reject strategy-file changes that implement submit mechanics, sizing, rounding, fillability, or venue admission.
- **FR-020**: The system MUST map Hyperliquid `[data]` through NT `HyperliquidDataClientFactory` and explicit TOML-owned data endpoints, timeouts, refresh cadence, environment, and transport backend.
- **FR-021**: The Hyperliquid provider binding MUST advertise the `updown` market family so HIP-4 outcome targets can pass the shared execution-client routing gate before surface-bound approval mapping.

## Edge Cases

- NT adapter attempts to fall back to environment variables after Bolt resolved SSM credentials.
- API wallet omits account address, causing orders to bind to the wrong account context.
- Two runtime clients share one API wallet and produce nonce conflicts.
- Public discovery sees a HIP-3 or HIP-4 market but submit semantics are not proven.
- Local info node is stale or unavailable.
- Live-submit approval artifact matches provider id but not TOML checksum or product surface.
- Approval artifact is replayed after one use.

## Success Criteria

- **SC-001**: Hyperliquid provider config validates through existing provider-binding tests with no raw client module, including data-only market-data config.
- **SC-002**: Secret-resolution tests prove SSM-only behavior and no environment fallback at NT handoff.
- **SC-003**: Product matrix tests classify standard perps, spot, HIP-3, and HIP-4 with source evidence and fail-closed submit status.
- **SC-004**: Fail-closed tests prove latency ops metadata cannot bypass live-submit approval and the shared exchange-mutation guard rejects mutating request counts.
- **SC-005**: Relay-Claude adversarial review approves the Speckit plan before implementation begins.

## Assumptions

- The current pinned NT revision remains the source of truth unless current `main` requires a coordinated dependency update.
- Hyperliquid docs and public metadata endpoints are authoritative only for documented public behavior; Bolt proof gates decide live submit readiness.
- Live standard-perps submit is a later gated slice after fee/rate policy reconciliation, product proof, and standard-perps instrument routing, not part of the initial implementation claim.
