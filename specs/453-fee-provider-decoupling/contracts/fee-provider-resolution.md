# Contract: Fee-Provider Resolution

## Public Behavior

Runtime strategy registration must resolve a strategy fee provider through a generic provider boundary:

1. Read the strategy's TOML-owned `execution_client_id`.
2. Resolve the matching loaded client.
3. Dispatch to a provider binding using the resolved client's existing TOML-owned `venue` provider key and the existing provider registry.
4. Return `Arc<dyn FeeProvider>`.
5. Build `StrategyBuildContext` from the resolved provider, decision evidence writer, and submit admission state.

Fee-provider resolution is a construction and binding step only. It must not call `FeeProvider::warm(...)` or move fee warm failures into registration; warm failures remain in the existing strategy runtime readiness path.
Resolver and binding errors must never log, format, or display raw secret material.
Generic resolver code must pass a borrowed or shared reference to the existing resolved secrets snapshot to concrete bindings without cloning or owning raw credential fields.

## Current Polymarket Compatibility

Existing Polymarket behavior must remain:

- SSM-resolved credentials only.
- NT `PolymarketClobHttpClient` for `/fee-rate`.
- CLOB token-id extraction remains provider-specific.
- `fee_bps(...)` returns cached bps only within TTL.
- `warm(...)` fetches and caches fee bps.

Future providers may resolve fees from NT instrument fee fields instead of HTTP. That provider must still register behind the same `Arc<dyn FeeProvider>` boundary and must not require strategy/archetype changes.

## Non-Goals

- No order-intent behavior changes.
- No submit-admission wrapper extraction.
- No strategy economics rewrite.
- No environment-variable secret fallback.
- No config migration or alternate provider-key source.
- No Polymarket or binary-oracle assumptions in shared provider resolver beyond concrete provider binding registration.

## Required Tests

- Red `fee_provider_source_fence_blocks_concrete_provider_in_shared_layers` source-fence test proving current archetype direct Polymarket call.
- Green `binary_oracle_registration_resolves_fee_provider_through_provider_boundary` registration test proving current Polymarket config still resolves a fee provider.
- Guard test proving strategy/archetype registration no longer calls `polymarket::build_fee_provider` directly.
- Guard test proving fee-provider resolution does not call `FeeProvider::warm(...)` during registration.
- Resolver error test for missing execution client id at the new resolver boundary.
- Resolver error tests for unsupported provider kind, no fee-provider binding, provider-specific config parse failure, missing or invalid resolved secret binding, and provider-specific client construction failure.
- Error-format test with sentinel secret values proving resolver and binding `Display`/`Debug` output does not contain raw secret material.
- Source-fence test proving every file under `src/bolt_v3_archetypes/`, strategy modules under `src/strategies/`, `src/bolt_v3_submit_admission.rs`, `src/bolt_v3_order_intent.rs`, and `src/bolt_v3_strategy_registration.rs` does not import or directly construct concrete providers; the fence must reject `bolt_v3_providers::polymarket`, `polymarket::`, and direct `build_fee_provider` usage outside allowlisted provider modules. The broad `polymarket::` rejection is intentional for prohibited shared-layer files. The same deterministic source-fence may serve as both the red proof and final guard if it first fails on the current direct call and then passes after decoupling.
