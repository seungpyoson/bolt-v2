# Runtime API Contract: Global RV Surface Runtime

## Purpose

The realized-volatility runtime is a shared process-level service, not a strategy-owned estimator. It owns all configured RV surface state, market-data source routing, refresh cadence, readiness, diagnostics, robust estimation, forecast state, and snapshot publication. Strategies, taker pricing, maker pricing, evidence, and future consumers select a `realized_volatility_surface_id` and consume immutable ready snapshots.

## Ownership Boundary

- `RealizedVolSurfaceRuntime` owns every `RealizedVolEngine`/surface state instance for the node.
- Strategies never instantiate `RealizedVolEngine` directly.
- Strategies never compute RV, quorum, aggregation, dispersion, jump components, horizon blends, or forecast adjustments.
- Pricing code only consumes `ReadyRealizedVol` or equivalent accessor output from a runtime-published snapshot.
- Missing, stale, blocked, or invalid snapshots fail closed at the consumer boundary.

## Threading and Ordering

The first implementation uses a single runtime owner on the node event loop / actor task that already serializes strategy and market-data handling.

Rules:

- Market-data callbacks enqueue or call into the runtime owner; they do not mutate surface engines directly from strategy state.
- `observe_market_data` and `refresh` are serialized by the runtime owner.
- `refresh(now_ms)` is monotonic per surface; a refresh with `now_ms` less than or equal to the last published snapshot timestamp is ignored or rejected deterministically before any forecast-state update is considered.
- `snapshot(surface_id)` returns an immutable clone, reference, or handle that cannot mutate runtime state.
- Consumers may read snapshots concurrently only if the implementation uses immutable publication or explicit read locking.
- Forecast state advances only inside serialized `refresh`, never inside consumer reads. A refresh that passes surface-level monotonicity but has `now_ms` not greater than the previous forecast update timestamp must leave forecast state unchanged and report the deterministic no-advance reason in diagnostics/evidence.

If later implementation uses locks or a dedicated runtime actor thread, it must preserve the same observable ordering and publish immutable snapshots.

## Core Types

### RealizedVolSurfaceRuntime

Process-level runtime container for all configured surfaces.

Required responsibilities:

- Build all RV surfaces from root TOML before strategies are constructed.
- Validate surface IDs, source IDs, client references, instrument bindings, source classes, sample kinds, horizon policies, forecast policies, and aggregation settings.
- Produce deduplicated market-data subscription requests for every configured source.
- Route quote/trade/index/mark observations to every matching surface source route.
- Refresh all surfaces on a runtime-owned cadence.
- Publish latest snapshots by `surface_id`.
- Expose bounded diagnostics for unknown source IDs and rejected observations.

### RealizedVolSubscriptionRequest

Logical subscription request emitted by the runtime, not by a strategy.

Fields:

- `client_id`
- `instrument_id`
- `source_class`
- `sample_kind`
- `source_ids: Vec<String>`
- `surface_ids: Vec<String>`

The runtime may combine routes that share the same physical subscription key while preserving all source/surface fan-out mappings.

### RealizedVolSnapshot

Immutable snapshot published per surface.

Must include:

- `surface_id`
- `as_of_ms`
- `ready`
- `ready_realized_vol()` accessor output when ready
- `annualized_realized_vol_decimal` for backward-compatible evidence
- `continuous_annualized_realized_vol_decimal`
- `jump_annualized_realized_vol_decimal`
- `forecast_annualized_realized_vol_decimal` when forecast mode is enabled
- `forecast_cold_start`
- `horizon_estimates`
- `sources_used`
- `source_diagnostics`
- `blocked_reasons`
- `aggregation_method`
- `estimator_method`
- `config_fingerprint`

## Operations

### from_root_config

Input: validated root config and public data-client registry.

Output: `Result<RealizedVolSurfaceRuntime, ValidationError>`.

Rules:

- Fail loudly if any configured surface cannot be built.
- Fail loudly if a strategy references a missing surface.
- Fail loudly if a surface source references a missing client/instrument binding.
- No soft runtime degradation to `None` engine when root validation should have prevented the error.

### subscription_requests

Input: none after construction.

Output: deduplicated subscription requests.

Rules:

- The runtime, not each strategy, is the source of RV subscriptions.
- If multiple surfaces use the same physical stream, subscribe once and fan out observations internally.
- Disabled sources remain auditable but do not create subscriptions.
- Non-quorum diagnostic sources may subscribe if enabled.

### observe_market_data

Input: normalized market-data event containing route key, event timestamp, receive timestamp, and price fields.

Output: observation acceptance/rejection diagnostics.

Rules:

- Unknown route IDs are counted in bounded diagnostics.
- Every matching source route receives the normalized observation.
- Strategy-local signal data must not be required for RV ingestion.
- Event-time/receive-time causality and same-event update rules remain enforced by the shared engine.

### refresh

Input: `now_ms`.

Output: updated snapshots for every surface.

Rules:

- Refresh cadence is TOML-owned and runtime-owned.
- Surface readiness is computed from eligible contributors only.
- Per-source blockers remain diagnostics unless quorum/aggregation rules require a surface-level blocker.
- EWMA forecast state advances only when a new ready current component exists and `now_ms` is newer than the previous forecast update timestamp; non-monotonic forecast timestamps do not advance state.
- The runtime must not fallback to legacy or strategy-owned RV.

### snapshot

Input: `surface_id`.

Output: latest `RealizedVolSnapshot` if known.

Rules:

- Consumers receive snapshots or ready accessors only.
- Consumers do not revalidate raw `f64` RV values with ad hoc predicates.
- Consumers do not inspect source diagnostics to override readiness.

## Failure Semantics

- Invalid TOML: reject at validation/build time.
- Missing surface for strategy: reject at validation/build time.
- Missing snapshot: consumer blocks with `RealizedVolNotReady`.
- Not-ready snapshot: consumer blocks with `RealizedVolNotReady` and evidence records blockers.
- Invalid numeric output: blocked by the typed RV value constructor before publication.
- Unknown source route: bounded diagnostic counter with deterministic eviction; no unbounded memory growth.

## Required Regression Fences

- No `RealizedVolEngine` field or constructor call in `src/strategies/**`.
- No legacy `vol_*` knobs in production strategy TOMLs or production config structs.
- No raw `is_positive_finite` or `is_non_negative_finite` RV filtering in pricing/strategy consumers.
- No concrete venue/asset/provider literals in RV engine code or RV tests except TOML fixtures explicitly testing production config.
- Runtime subscription creation is centralized outside strategies.
- Global runtime construction is reachable from node/root build code, not from strategy constructors.
