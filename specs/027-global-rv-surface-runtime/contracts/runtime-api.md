# Runtime API Contract: Global RV Surface Runtime

## Purpose

The realized-volatility runtime is a shared process-level service, not a strategy-owned estimator. It owns all configured RV surface state, market-data source routing, refresh cadence, readiness, diagnostics, Option A robust estimation, and snapshot publication. Strategies, taker pricing, maker pricing, evidence, and future consumers select a `realized_volatility_surface_id` and consume immutable ready snapshots.

## Ownership Boundary

- `RealizedVolSurfaceRuntime` owns every `RealizedVolEngine`/surface state instance for the node.
- Strategies never instantiate `RealizedVolEngine` directly.
- Strategies never compute RV, quorum, aggregation, dispersion, jump components, future horizon blends, or future forecast adjustments.
- Pricing code only consumes `ReadyRealizedVol` or equivalent accessor output from a runtime-published snapshot.
- Missing, stale, blocked, or invalid snapshots fail closed at the consumer boundary.

## Threading and Ordering

The first implementation uses a single runtime owner on the node event loop / actor task that already serializes strategy and market-data handling.

Rules:

- Market-data callbacks enqueue or call into the runtime owner; they do not mutate surface engines directly from strategy state.
- `observe_market_data` and `refresh` are serialized by the runtime owner.
- A runtime-wide `refresh(now_ms)` visits surfaces in deterministic sorted `surface_id` order.
- `refresh(now_ms)` is monotonic per surface; a refresh with `now_ms` less than or equal to the last published snapshot timestamp is ignored or rejected deterministically.
- `snapshot(surface_id)` returns an immutable clone, reference, or handle that cannot mutate runtime state.
- Consumers may read snapshots concurrently only if the implementation uses immutable publication or explicit read locking.
- Future forecast state, when implemented, must advance only inside serialized `refresh`, never inside consumer reads.

If later implementation uses locks or a dedicated runtime actor thread, it must preserve the same observable ordering and publish immutable snapshots.

## Core Types

### RealizedVolSurfaceRuntime

Process-level runtime container for all configured surfaces.

Required responsibilities:

- Build all RV surfaces from root TOML before strategies are constructed.
- Validate surface IDs, source IDs, client references, instrument bindings, source classes, sample kinds, estimator policies, and aggregation settings.
- Produce deduplicated market-data subscription requests for every configured source.
- Route quote/trade/index observations to every matching surface source route. Mark sources are reserved and rejected until routing is implemented.
- Refresh all surfaces on a runtime-owned cadence.
- Publish latest snapshots by `surface_id`.
- Expose diagnostics for configured sources and routed rejections. PR #615 drops unconfigured raw source IDs at the runtime boundary; any future raw ingestion path must add explicit cardinality bounds before recording unknown IDs.

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
- `measured_annualized_realized_vol_decimal`
- `noise_robust_annualized_realized_vol_decimal`
- `continuous_annualized_realized_vol_decimal`
- `jump_annualized_realized_vol_decimal`
- `forecast_annualized_realized_vol_decimal` as an empty future placeholder in PR #615
- `horizon_estimates` as an empty future placeholder in PR #615
- `sources_used`
- `source_diagnostics`
- `blocked_reasons`
- `aggregation`
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

- Unconfigured raw source IDs are dropped at the runtime boundary. PR #615 route construction limits source IDs to configured routes; future raw ingestion must add explicit capacity before recording unknown IDs.
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
- Future EWMA/HAR forecast state must advance only when refresh publishes a ready current component and must preserve the same monotonic refresh contract.
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
- Unconfigured raw source ID: dropped at the runtime boundary. Future raw external ingestion must add bounded cardinality before merge.

## Required Regression Fences

- No `RealizedVolEngine` field or constructor call in `src/strategies/**`.
- No legacy `vol_*` knobs in production strategy TOMLs or production config structs.
- No raw `is_positive_finite` or `is_non_negative_finite` RV filtering in pricing/strategy consumers.
- No concrete venue/asset/provider literals in RV engine code or RV tests except TOML fixtures explicitly testing production config.
- Runtime subscription creation is centralized outside strategies.
- Global runtime construction is reachable from node/root build code, not from strategy constructors.
