# Bolt-v3 Implementation Plan: The Systematic Harness

Status: Proposed Plan for User Review
Objective: Establish a lean, purely dynamic, NT-first Bootstrapping Harness.

This plan details the exact sequence of steps to implement Bolt-v3 without polluting it with Bolt-v2 legacy architecture. We will build this foundation safely under `src/v3/` so that it remains isolated from existing v2 code until the new architecture is fully proven and live-ready.

## Phase 1: The Unified Envelopes (Configuration & Secrets)

**Goal:** Flatten the configuration model into completely generic, opaque blocks. Remove all closed enums and hardcoded venue validation from the core.

1. **Implement `src/v3/config.rs` (The Lean Model)**
   - Define the universal root TOML shape containing only `[runtime]`, `[nautilus]`, `[risk]`, `[logging]`, `[persistence]`, `[aws]`, `[venues]`, and `[strategies]`.
   - Remove `ReferenceVenueKind`, `RulesetVenueKind`, `ExecClientSecrets`.
   - Use `toml::Value` for `[venues.<name>.data]`, `[venues.<name>.execution]`, `[venues.<name>.secrets]`, and `[strategies.<name>.parameters]`.

2. **Implement `src/v3/secrets.rs` (The Generic Resolver)**
   - Define `trait SecretResolver: Send + Sync`.
   - Implement `check_config(opaque_toml) -> Result<()>` and `resolve(opaque_toml) -> Result<Box<dyn Any>>`.
   - Move SSM lookup logic here, enforcing that credentials *only* come from AWS SSM.

## Phase 2: The Dynamic Trait Registry

**Goal:** Create the boundaries where concrete implementations (Polymarket, Binance) plug in without modifying the core execution graph.

1. **Implement `src/v3/registry.rs` (The Dependency Injector)**
   - Define `trait ProviderAdapter`:
     - `build_data_client(data_config)`
     - `build_exec_client(exec_config, resolved_secrets)`
   - Define `trait StrategyArchetype`:
     - `build_strategy(target_config, parameters)`
   - Implement a generic `Registry` struct that holds HashMaps mapping string `kind`s to `Box<dyn Trait>` implementations.

2. **Implement concrete fixture modules (as proof of concept, not core architecture)**
   - Create `src/v3/providers/polymarket.rs` and `src/v3/providers/binance.rs` that solely implement the `ProviderAdapter` and `SecretResolver` traits.
   - Create `src/v3/archetypes/binary_oracle_edge_taker.rs` implementing `StrategyArchetype`.
   - *Crucial Check:* Ensure these files do *not* bleed into `src/v3/app.rs` or `src/v3/config.rs`.

## Phase 3: The Lean Assembly Graph (The App Engine)

**Goal:** Wire the configured NT components into the `LiveNode` systematically, proving that Bolt does not own execution state or intent routing.

1. **Implement `src/v3/app.rs` (The Bootstrapper)**
   - Load the `v3::config::Config`.
   - **Step 1:** Iterate over `venues`, lookup `SecretResolver` in registry, and resolve secrets.
   - **Step 2:** Iterate over `venues`, lookup `ProviderAdapter` in registry, and generate native `NautilusDataClient` and `NautilusExecutionClient` objects. Register them with a native NT `LiveNode`.
   - **Step 3:** Iterate over `strategies`, lookup `StrategyArchetype`, generate the native NT `Strategy`, and add it to the `LiveNode`.
   - **Step 4:** `node.run().await`.

2. **Implement Single Validation Path (`just check`)**
   - Create `src/v3/check.rs`.
   - Traverse the exact same graph as `app.rs`, but call `.check_config()` on the traits instead of fully building the clients, outputting explicit `StructuralResult` and `LiveResult`.

## Phase 4: Native Observability & Persistence

**Goal:** Prove that Bolt-v3 uses NT's native features for events and logging, completely dropping `normalized_sink`.

1. **Implement `src/v3/forensics/events.rs`**
   - Define the fixed set of `DecisionEvent` structs (e.g., `selector_decision`, `entry_evaluation`).
   - Register them as NautilusTrader `CustomData` at startup before the NT streaming path is initialized.

2. **Wire NT Catalog Persistence**
   - Configure NT's native `StreamingConfig` in the `LiveNode` assembly to persist these custom data events directly to the local catalog.

## Phase 5: Verification & Safety Gates

**Goal:** Enforce strict checks required by `AGENTS.md` and the architecture spec before considering the architecture "done."

1. **Panic Behavior Gate (Issue #239)**
   - Implement a test harness `src/v3/panic_gate.rs` injecting panics into startup, market-data, and timer callbacks to explicitly map NT's behavior on the pinned release.

2. **Integration Testing**
   - Write integration tests proving that adding a dummy venue `kind = "test_venue"` requires only registering a new trait in the `Registry`, with zero lines changed in `v3/app.rs` or `v3/config.rs`.

## Summary of Deleted Bolt-v2 Concepts
By executing this plan, we mechanically guarantee the deletion/avoidance of:
- `ReferenceVenueKind`, `RulesetVenueKind` closed enums.
- The `Ruleset` engine and Bolt-owned selector polling loops.
- `normalized_sink` and `raw_capture` layers.
- Bolt-owned executable order mapping.
- Hardcoded config structs for individual providers in the core.
