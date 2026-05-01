# Bolt-v3 CTO Framing: Dynamic Boundaries

Status: Approved Architecture Framing for Issue #278
Owner: CTO Role

## Context and Failure Mode
Prior attempts to implement Bolt-v3 failed because they assumed Bolt-v2 fixtures (e.g., closed enums for `Binance` or `Polymarket`, hardcoded secret configurations) were the settled architecture. As stated by the user, Bolt-v3 must be a completely generic, systematic, and dynamic thin wrapper around NautilusTrader (NT). It must scale easily across hundreds of markets and multiple providers without requiring core system changes.

This document serves as the exact framing for Bolt-v3. Before any further implementation continues, all PRs must conform to this dynamic trait-driven architecture.

## 1. Zero Closed Enums in Core
Core Bolt-v3 code (configuration parsing, validation, client assembly) MUST NOT contain any closed enum variants referring to concrete providers.

**Wrong (Bolt-v2):**
```rust
pub enum ReferenceVenueKind {
    Binance,
    Polymarket,
}
```

**Right (Bolt-v3):**
The config uses plain strings (`kind = "binance"`), and core logic simply passes that string identifier down to a dynamic provider registry.

## 2. Opaque Config and Secrets Boundary
Bolt-v3 core must not know the specific schema of a provider's configuration or secrets.

**Wrong (Bolt-v2):**
```rust
pub struct ExecClientSecrets {
    pub pk: Option<String>,
    pub api_key: Option<String>,
    // Hardcoded to Polymarket concepts
}
```

**Right (Bolt-v3):**
The root TOML provides an arbitrary key-value map for `config` and `secrets`. Bolt-v3 loads these as `toml::Value` (or similar opaque map) and passes them to the dynamic `SecretResolver` and `ProviderAdapter` boundaries. It is the responsibility of the registered provider module to parse and enforce its own required fields, NOT the core validator.

## 3. Dynamic Registration Traits
Bolt-v3 orchestrates via dynamically dispatched traits.

- `trait ProviderAdapter`: Capable of taking an opaque config and returning the instantiated NautilusTrader `ExecutionClient` and `DataClient`.
- `trait SecretResolver`: Capable of taking an opaque secret config, checking for completeness, and resolving to SSM without hardcoding the fallback rules centrally.
- `trait StrategyArchetype`: Receives an opaque strategy config and instantiates a NautilusTrader strategy that strictly uses native NT objects (no bolt-owned executable-order schemas).

## 4. Source-Level Neutrality
Concrete provider implementations (e.g., `src/v3/providers/binance.rs`, `src/v3/providers/polymarket.rs`) exist solely to implement these traits and register themselves with the central string-to-trait registry at startup. They are *fixtures* mapped onto the dynamic framework, not the framework itself.

## Immediate Process Enforcement
Any PR that introduces a new `enum` variant for a provider, or adds a specific field to a core struct for a specific exchange (e.g., `pk`, `chainlink`), will be rejected immediately as out of scope for Bolt-v3.
