# Implementation Plan: Hyperliquid Execution Adapter

**Branch**: `codex/025-hyperliquid-execution-adapter` | **Date**: 2026-06-01 | **Spec**: `specs/025-hyperliquid-execution-adapter/spec.md`
**Input**: Feature specification from `specs/025-hyperliquid-execution-adapter/spec.md`

## Summary

Enable Hyperliquid as a production-grade NautilusTrader-backed data and execution provider for standard perps, spot, HIP-3 builder perps, and HIP-4 outcomes without adding a bespoke client or strategy-level submit mechanics. The first accepted slice registers the NT Hyperliquid Rust adapter behind Bolt's provider registry, maps the NT market-data client for live instruments, quotes, and order books, proves public product discovery and SSM-only credential handling, wires provider-owned production live-node approval loading, enforces bounded approval order limits in shared submit admission, opens the shared `updown` routing gate for HIP-4 outcome targets plus `hyperliquid_instrument` routing identity for static/direct Hyperliquid instruments, wires Hyperliquid fee-provider resolution through an NT `userFees` warmup boundary, and keeps every live submit path approval-gated by exact product-surface approval, bound product-proof evidence, product-compatible strategy runtime, and shared submit admission.

Colocated low-latency operation is planned as configuration and runbook surface only: local Hyperliquid info node and infrastructure placement may reduce latency, but the adapter must not hardcode endpoints, placement assumptions, fee weights, product IDs, or submit policy in code.

## Technical Context

**Language/Version**: Rust, current repository toolchain
**Primary Dependencies**: `nautilus_trader` pinned git revision already used by the repo; add the matching `nautilus-hyperliquid` crate from the same pinned revision after re-verifying current `Cargo.lock` compatibility
**Storage**: TOML runtime config and operator artifacts only; secrets resolve from AWS SSM through Rust SDK
**Testing**: TDD with focused Rust unit/integration tests, `cargo fmt --check`, `cargo clippy --locked --lib -- -D warnings`, relevant `cargo test` slices, and exact-head CI before any live path
**Target Platform**: Pure Rust `LiveNode` runtime
**Project Type**: Trading execution adapter integration inside existing Rust binary
**Performance Goals**: No hard latency claim. Provide TOML-driven local-info-node and region/AZ profile fields so ops can place the process near Hyperliquid infrastructure and measure actual latency.
**Constraints**: No Python layer, no raw Hyperliquid client, no environment secret fallback, no strategy submit mechanics, one signer owner per runtime, official rate-limit weights, product surfaces fail closed unless an exact surface-bound approval is consumed
**Scale/Scope**: One Hyperliquid provider family with four product surfaces: standard perps, spot, HIP-3 builder perps, HIP-4 outcomes

## Evidence Baseline

- Fresh base: `origin/main` is `2938bc6f6e7553e436f074163a9e5db8b4c56b11`, merge `#519` on top of PR 480 merge `92ef8e7dfeee7baa7f5eb4eb2d13017c18fa0afe`.
- NT source exists at the pinned checkout: `crates/adapters/hyperliquid`.
- NT adapter currently contains environment fallbacks for Hyperliquid credentials and account address; Bolt must fence these through SSM-only config before handoff.
- NT Hyperliquid source exposes public discovery surfaces for standard perp metadata, spot metadata, all perp metas, outcome metadata, and user fee queries.
- Public no-secret probes on 2026-06-01 showed non-empty `meta`, `spotMeta`, `allPerpMetas`, `outcomeMeta`, and `exchangeStatus` responses.
- Official Hyperliquid docs identify nonce/API wallet constraints, asset-id differences by product surface, `userFees` request weight, and latency optimization guidance.

## Plan Review Gate

- Relay-Claude adversarial plan review job `33d2b208-23d3-454b-9024-15719c585a09` approved exact planning head `3da058eea22a9863ddf3b068b625947fab88f004`.
- Implementation started only after PR 480 was present in `origin/main` at `2938bc6f6e7553e436f074163a9e5db8b4c56b11`.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- PASS - NT-first thin layer: use `nautilus-hyperliquid`, not a raw `src/clients/hyperliquid.rs` implementation.
- PASS - Generic core, concrete edge: Hyperliquid registers as one provider binding; provider registry and execution admission stay shared.
- PASS - Single config and secret source: runtime values come from TOML; secrets come from AWS SSM via Rust SDK.
- PASS - Live trading approval-gated: live submit paths remain disabled unless exact product proof, fee/rate policy reconciliation, product routing, and approval artifacts pass.
- PASS - TDD and evidence: every phase has tests or direct source/doc evidence before claims.
- PASS - Minimal slice: MVP stops at provider registration, NT market-data mapping, discovery, fees, credential fences, approval gates, and latency ops metadata. Live perps, spot, HIP-3, and HIP-4 submit remain bounded by explicit approval and submit-admission gates.
- PASS - Strategy intent only: no changes under `src/strategies/*` may submit, round, size, validate venue rules, or handle fills.

## Project Structure

### Documentation

```text
specs/025-hyperliquid-execution-adapter/
├── plan.md
├── spec.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── hyperliquid-provider-contract.md
└── tasks.md
```

### Source Code

```text
Cargo.toml
Cargo.lock
src/
├── bolt_v3_config.rs
├── bolt_v3_operator_artifacts.rs
├── bolt_v3_providers/
│   └── mod.rs
├── execution_admission/
└── lib.rs

tests/
├── bolt_v3_provider_binding.rs
├── bolt_v3_production_entrypoint.rs
└── hyperliquid_*.rs
```

**Structure Decision**: Extend the existing provider registry and shared execution/admission modules. Do not add a raw Hyperliquid client module and do not put venue mechanics in strategies.

## Phase Plan

### Phase 0 - Evidence And Research

1. Re-verify `origin/main`, `Cargo.lock`, and pinned NT checkout after PR 480 and any later base merges.
2. Inventory NT Hyperliquid Rust adapter source for config, credentials, signer, nonce, metadata, fees, submit, cancel, and product-surface support.
3. Confirm official Hyperliquid docs for latency, nonces/API wallets, asset IDs, rate limits, and supported request weights.
4. Record product-surface support as evidence, gap, or fail-closed.
5. Send this plan to relay-Claude adversarial review before implementation.

### Phase 1 - Design And Contracts

1. Define Hyperliquid provider contract at the Bolt provider-binding boundary.
2. Define TOML/SSM config entities and signer-owner lifecycle.
3. Define public discovery matrix for standard perps, spot, HIP-3 builder perps, and HIP-4 outcomes.
4. Define live-submit approval and latency-profile artifacts.
5. Define local-info-node and colocation profile as ops-only configuration.

### Phase 2 - MVP Implementation

MVP includes only provider registration, dependency wiring, config validation, Hyperliquid market-data adapter mapping, SSM-only credential resolution, environment-fallback fencing, signer ownership, product discovery, official fee/rate accounting, fail-closed approval gates, and latency ops metadata.

### Phase 3 - Gated Live Execution

Each Hyperliquid product surface may proceed only after MVP proof, product-specific order/fill/rounding/fee evidence, fee/rate policy reconciliation, provider-owned live-node approval artifact consumption, product-compatible strategy runtime, shared submit-admission enforcement of approval order limits, and an operator approval artifact bound to the exact product surface. HIP-4 additionally requires positive TOML-owned outcome settlement polling and uses the existing `updown` market-family route gate for outcome targets. Static/direct Hyperliquid instrument targets use `hyperliquid_instrument` for routing identity, must match one of the execution client's configured and approved product surfaces, own canary proof sizing constraints in TOML, and do not enable binary rotating-market selection. Hyperliquid owns a provider collector for the shared canary proof operator command so static targets can produce no-resolution gate sessions plus candidate/order-intent artifacts without a second artifact path.

## Implementation Rules

- Add `nautilus-hyperliquid` only from the same pinned NT revision used by the repo.
- Register Hyperliquid only through `ProviderBinding`.
- Map Hyperliquid `[data]` only through NT `HyperliquidDataClientFactory` with TOML-owned endpoints, timeouts, refresh cadence, environment, and transport backend.
- Reject raw secret material in TOML.
- Resolve all secrets from SSM before constructing NT config.
- Scrub or reject `HYPERLIQUID_*` environment variables before NT handoff.
- Require account address for API-wallet mode.
- Enforce one signer/API-wallet owner per process.
- Charge `userFees` using the official request weight.
- Treat local info node as read-only market-data optimization; it must not bypass rate accounting, submit gates, or provenance.
- Require exact base SHA, TOML checksum, provider id, product surface, order limits, expiry, and one-time id in live-submit approval artifacts.
- Load live-submit approvals only through provider binding hooks; core live-node code passes opaque provider-neutral approvals into adapter mapping.

## Complexity Tracking

No constitution violations are accepted for this feature. Any implementation pressure to add a raw client, strategy submit path, environment fallback, hardcoded runtime value, or ungated live submit path blocks the feature.
