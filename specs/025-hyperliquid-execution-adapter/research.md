# Research: Hyperliquid Execution Adapter

## Decision 1 - Use NT Hyperliquid Rust Adapter

**Decision**: Add Hyperliquid through the pinned `nautilus-hyperliquid` crate and Bolt provider registry.

**Rationale**: Repo rules require pure Rust, NT-first integration, and no bespoke Python or raw client layer.

**Rejected Alternatives**:
- Raw REST/WebSocket client in `src/clients/hyperliquid.rs`: violates NT-first and raw-client ban.
- Python Hyperliquid SDK: violates pure Rust binary rule.

## Decision 2 - Fence NT Environment Fallbacks

**Decision**: Resolve secrets from SSM and reject/scrub `HYPERLIQUID_*` environment variables before constructing NT Hyperliquid config.

**Rationale**: NT source contains environment fallback paths for private key, vault, and account address. Bolt rules require SSM as the single secret source and no environment fallback.

## Decision 3 - One Signer Owner Per Runtime

**Decision**: Add signer ownership validation before building execution clients.

**Rationale**: Hyperliquid nonce behavior and API-wallet docs require avoiding multiple trading processes sharing one API wallet/signer without coordination.

## Decision 4 - Product Surfaces Are Independently Gated

**Decision**: Standard perps, spot, HIP-3 builder perps, and HIP-4 outcomes each require their own discovery, admission, fee, order, cancel, fill, and settlement evidence before live submit.

**Rationale**: Product surfaces use different asset-id and market semantics. Discovery proof is not live execution proof.

## Decision 5 - MVP Is Fail-Closed Adapter Enablement

**Decision**: Initial implementation proves provider binding, discovery, secrets, fees, signer ownership, fail-closed approval gates, and latency ops metadata. It does not place live orders or publish Hyperliquid-specific no-submit readiness artifacts.

**Rationale**: Live exchange actions are irreversible side effects. Repo constitution requires evidence before claims and fail-closed live trading.

## Decision 6 - Approval Artifact Gates Live Submit

**Decision**: Live submit requires a one-time artifact bound to base SHA, provider id, product surface, TOML checksum, signer fingerprint, order limits, expiry, and a unique id.

**Rationale**: The artifact prevents stale review or config evidence from authorizing a different runtime surface.

## Decision 7 - Official Rate Limits Win

**Decision**: Use official Hyperliquid documented request weights for accounting, especially `userFees`.

**Rationale**: Adapter-local assumptions can drift. The implementation must bind to official docs and tests.

## Decision 8 - Priority Fees Out Of MVP

**Decision**: Do not implement priority-fee grouping until NT exposes and proves the required wire shape.

**Rationale**: Current NT source inventory did not prove priority-fee grouping support. Adding a parallel wire path would violate no-dual-path and raw-client constraints.

## Decision 9 - Colocation Is Ops Configuration

**Decision**: Provide TOML fields and artifacts for local info-node and region/AZ placement profile. Do not hardcode locations, endpoints, or latency assumptions.

**Rationale**: Hyperliquid docs describe latency optimization practices, but actual fastest path depends on deployment, network, and measurement.

## Evidence To Re-Verify During Implementation

- Current `origin/main` SHA and `Cargo.lock`.
- `nautilus_trader` pinned git revision and `nautilus-hyperliquid` crate path.
- NT config/env fallback lines for private key, vault, and account address.
- NT metadata and fee methods for `meta`, `spotMeta`, `allPerpMetas`, `outcomeMeta`, and `userFees`.
- Official docs for nonces/API wallets, asset IDs, latency optimization, and rate-limit weights.

## Implementation Refresh - 2026-06-02

- Branch: `codex/025-hyperliquid-execution-adapter`.
- Base: `origin/main` at `2938bc6f6e7553e436f074163a9e5db8b4c56b11`.
- PR 480 merge present at `92ef8e7dfeee7baa7f5eb4eb2d13017c18fa0afe`.
- NT pin: all Nautilus crates, including `nautilus-hyperliquid`, use `nautilus_trader` rev `6e059dcbb59ac1e582132fc431a581936c216c3c`.
- Implementation commits: `4355c974`, `bacb0a80`, `9fff450d`, `af8cb7b3`, `b3bc0965`, `cb7a2399`, `27046f0d`, `45ffdde4`.
