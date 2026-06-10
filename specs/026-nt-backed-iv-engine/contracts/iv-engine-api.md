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

The public strategy-facing query types are:

- `IvQuery`
- `IvProductQuery`
- `IvRawPayloadQuery`
- `IvQueryProduct`
- `IvProjectedScalarIv`
- `IvQueryError`
- `IvQueryHandle`

Every `IvProductQuery` includes:

- `strategy_id`
- `profile_id`
- `product_kind`
- `selector`

The `selector` field is a typed `IvSelector` union. It is not an arbitrary key-value bag. Timestamp, basis, source filter, and product-specific query fields live in the selector variant.

Authorization is evaluated through `IvSelectorAuthorization`. A profile may use profile-wide access or selector-scoped access, but the effective rule must authorize the strategy ID, product kind, source filter, and selector fingerprint. Raw-payload product kinds are never authorized for strategy query handles.

Derived IV queries require engine-owned `IvDerivedInputSet` state with all required helper inputs. Strategy query handles name an instrument, helper policy, and timestamp; they do not receive raw payload DTOs or call NT helpers directly.

Derived IV queries also require an `IvHelperPolicy` reference. The helper policy selects the NT helper symbol, parameter signature, allowed output shape, output bounds, and helper provenance fields. The engine rejects derived queries when helper policy is missing, unsupported by the capability ledger, or incompatible with resolved inputs.

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
- `Err(IvQueryError)` for profile mismatch, product-kind mismatch, not found, missing projection/helper policy, missing derived input, raw-payload rejection, strategy authorization rejection, or unsupported product-kind routing

No query may silently fall back to another basis, convention, source, timestamp, projection, interpolation, fallback, quorum, extrapolation policy, rate input, carry input, or time convention.

Policy decisions are typed `IvPolicyDecision` variants. Free-form policy decision strings are not part of the contract.

## Raw Payload Access

Raw payload access returns preserved NT payloads through an audit/replay API. It does not grant strategy code direct ownership of NT subscription mechanics or raw IV-bearing DTOs.

Raw payloads are evidence, observability, replay, and test outputs. IV-shaped strategy decisions must use IV engine products, projections, or derived products so provenance, policy, freshness, retention, and source authorization are enforced in one place.

The strategy-facing `IvQueryHandle` rejects raw-payload product kinds. Full raw payload retrieval is limited to `IvRawAuditReader` or equivalent audit/test modules outside `src/strategies/**`. Strategy-facing products may include `raw_event_id` references in provenance, but the raw NT payload bytes or typed NT payload structs remain engine-owned.

Raw payload retrieval also requires the owning profile's `IvAuditPolicy` to authorize the raw product kind, source, audit handle, access purpose, and audit retention boundary.

For NT custom-data backed aggregate greeks and custom implied-volatility sources, the preserved raw payload includes the serialized custom-data JSON in addition to the typed indexed fields. Strategy-facing products still expose only typed products and provenance references.

## Registration And Lifecycle Types

Strategy registration exposes IV access through:

- `BoltV3IvQueryHandleRegistry`
- `build_iv_query_handle_registry_for_root`
- `build_iv_query_handle_registry`
- `StrategyRegistrationContext::iv_query_handles`

Live-node IV lifecycle planning exposes:

- `IvEngineLifecyclePlan`
- `plan_iv_engine_lifecycle`

The lifecycle plan derives start and stop subscription plans from the same root `[iv]` config used to build strategy query handles.

`IvQueryHandle` also exposes builder-style wiring for engine-owned projection policies, helper policies, derived input sets, and source-health snapshots. Registration starts with config-owned strategy/profile authorization; the runtime IV engine is responsible for refreshing handle state as source data, derived inputs, and source health change.

## Projection Contract

Projection is required when a query asks for a scalar value from a smile, surface, aggregate product with a configured aggregate IV value, or custom-IV-evidence product.

Projection:

- must name the configured projection kind
- must identify input products and selector fingerprints
- must record basis, convention, timestamp, source eligibility, and evidence mapping
- must enforce the configured `max_projection_input_skew_ns` across all input points, smiles, surfaces, aggregate products with aggregate IV values, or IV evidence
- must reject if required interpolation, fallback, or quorum policies are absent or fail

## Derived Input Contract

Derived IV and derived greeks:

- must use NT math helpers only inside the IV engine
- must select the helper through `IvHelperPolicy`
- must resolve option price, underlying price, strike, option side, time-to-expiry, rate, carry, timestamps, and convention through `IvDerivedInputPolicy`
- must allow query-supplied inputs only when the owning profile permits that source kind
- must record all resolved inputs and helper identity in provenance
- must reject incomplete, stale, skewed, non-finite, or convention-incompatible inputs
- must reject expired operator-configured rate or carry inputs rather than silently reusing stale operator values

## Policy Contract

Interpolation:

- must name the configured method and axes
- must record every input point used
- must reject if input count, source eligibility, axis, or extrapolation requirements are not satisfied
- treats extrapolation as strike/tenor axis extrapolation only; temporal behavior is governed by freshness and history policy

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
- records subscription failures, unsupported mappings, stale generations, unsupported conventions, missing IV basis, and malformed custom data in `IvSourceHealth`
- exposes IV root reload planning and runtime-state application so new subscription generations can satisfy current queries, old generations and removed profiles/sources cannot, and already-issued strategy handles for profiles present before reload do not need to be recreated. The production live-node runner does not add a config hot-reload trigger in this feature.

## Capability Ledger Contract

The capability ledger test resolves NT source evidence from Cargo, not from a hand-maintained local path.

Ledger generation:

- runs against the locked dependency graph from `cargo metadata --locked`
- cross-checks NT package source revisions in `Cargo.lock`
- resolves the Cargo git checkout for the locked NT revision
- scans model, data actor, data engine, msgbus, option-chain, greeks-helper, adapter, and custom-data surfaces as minimum seed families
- performs a whole-checkout Rust source sweep for public modules, types, functions, methods, topics, and data definitions whose path, symbol, doc comment, or enclosing module contains IV/options indicators such as option, options, greeks, implied, iv, volatility, smile, surface, chain, or custom data
- includes option-microstructure indicators such as strike, expiry, expiration, tenor, moneyness, skew, premium, and vol in the candidate sweep
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
