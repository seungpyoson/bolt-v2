# Bolt-v3 Core Boundary Checkpoint

Status: review-readiness checkpoint after the corrective boundary slices.

This document records the intended ownership split after the provider,
market-family, and archetype behavior was moved out of core-facing
configuration, validation, and market-identity modules.

## Product Boundary

Bolt-v3 core is the assembly spine. It owns root and strategy envelope
loading, schema-version checks, explicit file references, neutral
dispatch identifiers, SSM-only startup invariants, common structural
validation, NT LiveNode assembly, client-registration orchestration, and
persistence wiring.

Core must not own provider configuration block shapes, market-family
target shapes, market slug derivation, provider discovery/filter
construction, reference metadata APIs, strategy parameter shapes, order
policy, risk policy, or execution policy.

## Core

Core-owned modules:

- `src/bolt_v3_config.rs` owns the TOML envelopes and neutral dispatch
  identifiers. Provider blocks, strategy parameters, and strategy
  targets remain raw `toml::Value` until the matching binding module
  deserializes them.
- `src/bolt_v3_validate.rs` owns root/common startup validation and
  dispatches target and archetype validation to their binding modules.
- `src/bolt_v3_market_identity.rs` is a neutral marker boundary. It
  does not own a concrete market-family identity model today.
- `src/bolt_v3_live_node.rs` and
  `src/bolt_v3_client_registration.rs` assemble NT runtime objects and
  registration intent without market selection or order construction.

## Provider Bindings

Provider-owned modules live under `src/bolt_v3_providers/` and the NT
adapter mapping layer in `src/bolt_v3_adapters.rs`.

They own provider-specific TOML block shapes, provider enum mappings,
provider credential block shapes, SSM secret mapping for the provider,
and NT adapter config translation. Polymarket-specific
`MarketSlugFilter` construction remains in `src/bolt_v3_adapters.rs`,
not in core market identity.

## Market Families

Market-family modules live under `src/bolt_v3_market_families/`.

The current `updown` binding owns its target shape, target
deserialization, target validation, cadence token table, period
arithmetic, slug formatting, candidate generation, and market-identity
projection. Core validation calls the family dispatcher and reads only
minimal cross-family metadata such as `configured_target_id`.

## Archetypes

Strategy-archetype modules live under `src/bolt_v3_archetypes/`.

The current `binary_oracle_edge_taker` binding owns its `[parameters]`
shape, typed parameter deserialization, required reference-data role,
entry/exit order-combination policy, parameter decimal checks, and root
risk-cap comparison for its own parameter fields. Core validation
dispatches by `StrategyArchetype` and does not access archetype
parameter fields directly.

## Preserved Behavior

The corrective slices preserve the existing launch behavior:

- Polymarket provider filters are still installed by the adapter layer.
- The dynamic updown slug pair still recomputes from the injected clock.
- Strategy declaration order is still preserved.
- `subscribe_new_markets = true` still fails closed before NT mapping.
- Existing root and strategy TOML field names and enum casings are
  unchanged.
- Unknown fields inside raw strategy `[target]` and `[parameters]`
  blocks now fail during binding deserialization instead of envelope
  parsing; the field name remains surfaced in the validation error.

## Deferred Risks

- `live_node_run` passed on retry during the final workspace run. Treat
  this as a residual test flake until a later slice proves otherwise.
- `bolt_v3_archetypes::binary_oracle_edge_taker` still calls
  `bolt_v3_validate::parse_decimal_string`. If decimal syntax becomes
  more than shared utility, move it to a neutral utility module.
- `BoltV3MarketIdentityError::TargetParseFailed` is defensive and
  currently reports strategy context only. It can be enriched later if
  the family planner needs better bypass-detection diagnostics.
- Target and archetype dispatch use similar raw-TOML binding patterns
  but not identical dispatch shapes. Harmonize only when a second
  family or second archetype forces the design.

## Review Gate

External review should wait until the corrective branch has coherent
commits, a clean worktree, pushed commits, and green CI on the exact PR
head. The review ask should be architectural: verify that Bolt-v3 core
is provider-neutral, market-family-neutral, and strategy-policy-neutral.
