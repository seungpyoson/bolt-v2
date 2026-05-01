# Bolt-v3 CTO Framing: Comprehensive Systematic Architecture

Status: Comprehensive Review for Issue #278
Owner: CTO Role

This document expands beyond mere string replacement ("no hardcodes") to define the **structural leanness** required for Bolt-v3 to be a truly systematic, infinitely scalable harness around NautilusTrader (NT).

## The Fundamental Shift: From Framework to Harness

In Bolt-v2, Bolt acted as a framework: it owned the risk models, execution state, intent-to-order translation, and heavy routing logic.

In Bolt-v3, Bolt is strictly a **Bootstrapping Harness** and **Dependency Injector** for NT.

**Core Tenets of the Systematic Architecture:**

1. **Zero Intermediate State (The Thin Wrapper Rule):**
   - Bolt-v3 does not own an order schema.
   - Bolt-v3 does not own position state or sellable quantity.
   - Bolt-v3 does not own a normalized data sink or secondary observability loop.
   - *Everything* execution-related relies 100% on native NautilusTrader objects (`Order`, `Position`, custom data events).

2. **Systematic Component Lifecycle (The Plug-and-Play Rule):**
   - The entire system is built on a uniform, dynamic lifecycle graph: **Config -> Secrets -> Clients -> Runtime Assembly**.
   - If a new market (e.g., Kalshi) or new strategy is added, it must *only* require dropping a new Rust module into the registry that implements the uniform traits. The core assembly loop (`src/v3/app.rs`) does not change.

## The 5 Pillars of the Lean Architecture

### 1. Unified Configuration Envelope (No Dual Paths)
Instead of bespoke structures for every subsystem (`ExecClientEntry`, `ReferenceConfig`), Bolt-v3 uses a single unified shape for *all* venues (trading and data) and *all* strategies.

- **Venues:** A venue is simply a named block (`[venues.<name>]`) with optional `[data]`, `[execution]`, and `[secrets]` opaque TOML maps.
- **Strategies:** A strategy is an independent file containing an archetype name, target, and opaque `[parameters]` map.
- *Systematic Gain:* The core configuration parser no longer validates business logic. It only validates the structural presence of the required envelopes.

### 2. Systematic Secret Resolution (SSM Only)
Secret resolution is decoupled from both the config parser and the execution clients.
- `Bolt` reads the `[secrets]` TOML map.
- The `BoltRegistry` looks up the `SecretResolver` trait for the provider.
- The resolver queries AWS SSM and returns a strongly-typed `Box<dyn Any>` secret payload.
- *Systematic Gain:* Core never sees credentials. Environment variable fallbacks are structurally impossible.

### 3. Pure NT Data/Execution Adapters
- Adapters are factories. They take the opaque `config` and `resolved_secrets` and yield exactly two things: a native `NautilusDataClient` and a native `NautilusExecutionClient`.
- *Systematic Gain:* Bolt does not attempt to map order intents. It simply wires the NT clients into the NT LiveNode. If an adapter needs Kalshi-specific logic, that logic lives in the Kalshi NT adapter, not Bolt.

### 4. Strategy Archetype Instantiation
- Strategies are not Bolt-v2 `Runnable` traits. They are factories that yield a native NT Strategy.
- Target resolution (e.g., `active_or_next` for `updown` markets) is handled by passing dynamic `MarketSlugFilter` closures to NT's data client, not by a Bolt-owned polling service.
- *Systematic Gain:* Strategies manage their own targets via NT, scaling infinitely without a central Bolt bottleneck.

### 5. Native Observability (No Parallel Stacks)
- Bolt-v3 emits "Decision Events" directly into the NautilusTrader message bus as registered `CustomData`.
- Persistence uses the native NT catalog/streaming path.
- *Systematic Gain:* We delete the custom `normalized_sink` and `raw_capture` layers.

## The Execution Graph (How it actually runs)

1. Load Config (structural check only).
2. For each `[venues.<name>]`:
   - Lookup `ProviderAdapter` and `SecretResolver` in Registry.
   - Resolve Secrets.
   - Build NT Data/Exec Clients -> Add to NT `LiveNode`.
3. For each `[strategies.<name>]`:
   - Lookup `StrategyArchetype` in Registry.
   - Build NT Strategy -> Add to NT `LiveNode`.
4. Start NT `LiveNode`. Wait.

This is the entire architecture. It is lean, systematic, and infinitely horizontally scalable across Polymarket, Kalshi, Binance, and any future integration.
