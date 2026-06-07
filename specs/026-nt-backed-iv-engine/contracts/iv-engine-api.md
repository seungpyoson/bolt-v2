# IV Engine API Contract

The IV engine exposes all strategy-facing IV access through one generic product API. Strategy code must not subscribe directly to NT IV/options topics, dereference raw NT IV payloads, or derive IV through NT math helpers; those capabilities live behind the IV engine.

## Query Products

Strategy query handles can request:

- custom IV evidence
- IV points
- IV plus greeks points
- aggregate greeks products
- smiles
- surfaces
- projected scalar IV
- derived IV points
- source health

Audit, replay, and test handles can request:

- raw NT option-greeks payloads
- raw NT option-chain payloads
- raw NT aggregate greeks payloads
- raw NT custom implied-volatility payloads

Raw payload handles are not injected into strategy registration. Strategies can receive product provenance and raw event IDs, but cannot dereference raw NT payload DTOs from the strategy-facing handle.

## Required Query Fields

Every query includes:

- `strategy_id`
- `profile_id`
- `selector`
- `product_kind`
- `as_of_ns`
- history/current mode

The `selector` field is a typed `IvSelector` union. It is not an arbitrary key-value bag. The selector variant must match the requested product kind and the owning profile's configured source kinds.

Derived IV queries additionally include either:

- an `IvDerivedInputSet` with all required helper inputs, or
- permission to resolve every required input through the profile's `IvDerivedInputPolicy`

Optional query fields are allowed only when the owning IV profile permits overrides:

- IV basis
- accepted convention
- source ID filter
- projection policy
- interpolation policy
- fallback policy
- quorum policy
- maximum age override

## Responses

Responses are either:

- `Ok(product)` with `IvProvenance`, profile identity, source identity, timestamp units, and policy decisions
- `Rejected(reason)` with a typed `IvRejectReason` and rejection provenance

No query may silently fall back to another basis, convention, source, timestamp, projection, interpolation, fallback, quorum, extrapolation policy, rate input, carry input, or time convention.

## Raw Payload Access

Raw payload access returns preserved NT payloads through an audit/replay API. It does not grant strategy code direct ownership of NT subscription mechanics or raw IV-bearing DTOs.

Raw payloads are evidence, observability, replay, and test outputs. IV-shaped strategy decisions must use IV engine products, projections, or derived products so provenance, policy, freshness, retention, and source authorization are enforced in one place.

The strategy-facing `IvQueryHandle` rejects raw-payload product kinds. Full raw payload retrieval is limited to `IvRawAuditReader` or equivalent audit/test modules outside `src/strategies/**`. Strategy-facing products may include `raw_event_id` references in provenance, but the raw NT payload bytes or typed NT payload structs remain engine-owned.

## Projection Contract

Projection is required when a query asks for a scalar value from a smile, surface, aggregate, or custom-IV-evidence product.

Projection:

- must name the configured projection kind
- must identify input products and selector fingerprints
- must record basis, convention, timestamp, source eligibility, and evidence mapping
- must enforce the configured `max_projection_input_skew_ns` across all input points, smiles, surfaces, aggregate products, or IV evidence
- must reject if required interpolation, fallback, or quorum policies are absent or fail

## Derived Input Contract

Derived IV and derived greeks:

- must use NT math helpers only inside the IV engine
- must resolve option price, underlying price, strike, option side, time-to-expiry, rate, carry, timestamps, and convention through `IvDerivedInputPolicy`
- must allow query-supplied inputs only when the owning profile permits that source kind
- must record all resolved inputs and helper identity in provenance
- must reject incomplete, stale, skewed, non-finite, or convention-incompatible inputs

## Policy Contract

Interpolation:

- must name the configured method and axes
- must record every input point used
- must reject if input count, source eligibility, axis, or extrapolation requirements are not satisfied

Fallback:

- must follow the configured ordered candidate list
- must record rejected candidates and the accepted candidate
- must reject if no candidate qualifies

Quorum:

- must evaluate only configured eligible sources
- must record participating and rejected sources
- must reject when source count or agreement-band requirements are not met

## Runtime Binding Contract

The live IV engine must bind through NT runtime APIs, not through an offline-only store.

Runtime binding:

- maps option-greeks sources to NT option-greeks subscription operations
- maps option-chain sources to NT option-chain subscription operations
- maps aggregate-greeks sources to NT greeks topic subscription operations
- maps custom-implied-volatility sources to ledger-classified NT custom-data subscription operations
- routes incoming NT events into raw preservation before indexing or projection
- records subscription failures, unsupported mappings, and stale generations in `IvSourceHealth`

## Capability Ledger Contract

The capability ledger test resolves NT source evidence from Cargo, not from a hand-maintained local path.

Ledger generation:

- runs against the locked dependency graph from `cargo metadata --locked`
- cross-checks NT package source revisions in `Cargo.lock`
- resolves the Cargo git checkout for the locked NT revision
- scans model, data actor, data engine, msgbus, option-chain, greeks-helper, adapter, and custom-data surfaces as minimum seed families
- performs a whole-checkout Rust source sweep for public modules, types, functions, methods, topics, and data definitions whose path, symbol, doc comment, or enclosing module contains IV/options indicators such as option, options, greeks, implied, iv, volatility, smile, surface, chain, or custom data
- requires every candidate from the seed scan or whole-checkout sweep to be classified as supported, unreachable from the Rust binary, not IV/options related after inspection, or explicitly excluded with approved rationale
- fails if a discovered IV/options surface is unclassified

## Source-Fence Contract

The IV source-fence must be wired into `just source-fence` through a checked verifier or test target.

The IV source-fence must reject:

- strategy module imports of NT msgbus option-greeks subscription APIs
- strategy module imports of NT data-actor option-chain subscription APIs
- strategy module imports of NT greeks subscription APIs
- strategy module imports of NT IV or greeks math helpers for strategy-local IV derivation
- concrete venue, asset, market, cadence, source, or instrument constants in IV core logic
- strategy imports or calls of raw payload audit/replay readers
- strategy imports of raw NT IV/options payload DTOs through the IV engine
- strategy configs or tests that request raw-payload product kinds through the strategy query handle
- strategy-local derivation of IV-shaped products from copied raw payload values

The IV source-fence must allow:

- strategy imports of the public IV query API
- strategy access to IV product provenance containing raw event IDs
- IV engine and audit/test module imports of NT model, msgbus, data actor, data engine, option-chain, custom data, raw payload DTOs, and greeks helper surfaces
