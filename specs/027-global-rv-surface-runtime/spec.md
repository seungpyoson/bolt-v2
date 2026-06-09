# Feature Specification: Global RV Surface Runtime

**Feature Branch**: `027-global-rv-surface-runtime`  
**Created**: 2026-06-08  
**Status**: Implemented for PR #615 accepted slice
**Input**: User description: "Implement the first approved slice: make the realized-volatility engine globally usable outside taker, enable multiple venues using available configured data clients, and start mathematical robustness with Option A microstructure-noise robust RV."

## PR #615 Accepted Scope

PR #615 implements the first production slice:

- a global RV surface runtime outside strategy structs;
- multiple configured venue/source bindings for shipped binary-oracle surfaces where supported clients exist;
- per-source fixed-grid RV plus microstructure-noise robustness using TOML-owned coarser-grid or deterministic offset-grid subsampling;
- jump diagnostics that separate continuous and jump components without deleting jumps;
- robust cross-source aggregation methods and surface-level dispersion blockers;
- decision-evidence schema v9 fields for global runtime provenance, measured/noise-robust/continuous/jump components, sources, diagnostics, blockers, and config fingerprint.

Multi-horizon blending and forecast-oriented RV are intentionally future scope. Types may exist as reserved API surface, but PR #615 does not claim ready multi-horizon estimates, EWMA/HAR forecast state, or forecast pricing.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Global RV Runtime (Priority: P1)

As an operator, I need realized-volatility surfaces to be owned by a runtime-level service rather than by `BinaryOracleEdgeTaker`, so every strategy or shared component can consume the same surface snapshot by `surface_id` without rebuilding RV state inside a strategy.

**Why this priority**: This fixes the root architectural failure from PR #609: RV policy moved to a shared module, but the engine instance and market-data routing still lived inside the taker actor. A global runtime is the prerequisite for maker consumption, multi-strategy sharing, and no strategy-owned RV lifecycle.

**Independent Test**: Start a runtime context with a configured surface, feed observations through the global runtime, and prove taker pricing reads a ready snapshot without owning a `RealizedVolEngine` field.

**Acceptance Scenarios**:

1. **Given** a root TOML surface and a binary oracle taker strategy referencing `realized_volatility_surface_id`, **When** market data arrives for the surface sources, **Then** a runtime-level RV surface service updates the snapshot and taker pricing consumes it by `surface_id`.
2. **Given** a strategy module under `src/strategies/*`, **When** source-fence scans production strategy code, **Then** no strategy owns `RealizedVolEngine`, quorum, dispersion, sampling-window, multi-horizon, or source aggregation policy.
3. **Given** two strategies reference the same `surface_id`, **When** the global runtime receives observations, **Then** both consumers see the same latest surface snapshot instead of separate per-strategy RV states.

---

### User Story 2 - Multi-Venue RV Sources (Priority: P1)

As an operator, I need each production RV surface to use all configured and supported public data sources for the underlying asset, so realized volatility is not dependent on a single OKX midpoint feed.

**Why this priority**: Multi-source robustness in PR #609 is latent until production TOML configures multiple independent sources. The global runtime must subscribe and route sources once per surface, not once per strategy.

**Independent Test**: Configure a surface with at least two enabled quorum-counting sources from available data clients, warm quorum from any valid subset, and prove one stale/divergent source does not silently dominate the aggregate.

**Acceptance Scenarios**:

1. **Given** root config has multiple available public data clients for an asset, **When** the surface is built, **Then** all configured valid source bindings are subscribed by the global RV runtime and recorded by `source_id`.
2. **Given** one source is stale but quorum is satisfied by other ready sources, **When** a snapshot is requested, **Then** the stale source remains diagnostic-only/blocked in evidence and does not block the surface unless quorum or dispersion policy requires it.
3. **Given** one ready venue diverges beyond configured dispersion policy, **When** aggregation runs, **Then** the snapshot fails closed or downweights/rejects the bad source according to TOML policy and records the reason.

---

### User Story 3 - Multi-Horizon Robust RV (Future Scope)

As a pricing consumer, I need each source's RV estimate to combine short, medium, and long horizons, so a single quiet or noisy rolling window does not make binary-oracle pricing unstable.

**Why deferred**: The approved first math upgrade is Option A microstructure-noise robustness. Multi-horizon RV remains the next horizon/regime robustness layer and must be implemented in a later PR with its own red-green tests and evidence fields.

**Independent Test**: In a future PR, feed deterministic price paths where short-horizon RV is quiet/noisy and medium/long horizons disagree; verify per-horizon values, final blend, and regime diagnostics in the snapshot/evidence.

**Acceptance Scenarios**:

1. **Given** short RV is zero but longer horizons are non-zero, **When** the surface policy uses a long-horizon floor, **Then** the final RV does not collapse below the configured floor policy.
2. **Given** short RV sharply exceeds medium/long RV, **When** divergence exceeds TOML policy, **Then** evidence records a regime divergence diagnostic and final RV follows the configured blend/floor rule.
3. **Given** all horizons are flat with valid coverage, **When** the engine computes RV, **Then** zero remains a valid ready RV.

---

### User Story 4 - Microstructure-Noise Robust RV (Priority: P2)

As a pricing consumer using quote-midpoint feeds, I need RV estimators that reduce false volatility from bid/ask bounce, feed timing artifacts, and noisy quote alternation.

**Why this priority**: Quote midpoint RV can be inflated by high-frequency microstructure noise. This is the approved first estimator upgrade after global runtime and multi-source configuration.

**Independent Test**: Compare alternating noisy midpoint paths and smooth paths under baseline fixed-grid RV vs configured noise-robust estimator mode, and verify evidence records estimator method and parameters.

**Acceptance Scenarios**:

1. **Given** a quote midpoint alternates by tiny amounts around a stable fair value, **When** a noise-robust estimator is enabled, **Then** final source RV is lower than naive fixed-grid RV while preserving readiness diagnostics.
2. **Given** a genuine directional price move occurs, **When** the same estimator is enabled, **Then** the move is not erased as noise.

---

### User Story 5 - Jump Separation (Priority: P2)

As a pricing consumer, I need jump contribution separated from continuous volatility rather than blindly removed, because binary-oracle jumps can be real information.

**Why this priority**: Bad ticks and isolated spikes should not silently dominate continuous RV, but real jumps must remain visible for pricing decisions.

**Independent Test**: Feed a bad-tick spike, a real one-way jump, and a flat path; verify snapshot evidence distinguishes continuous RV, jump component, and final RV policy.

**Acceptance Scenarios**:

1. **Given** a single isolated spike reverts immediately, **When** jump diagnostics run, **Then** continuous RV and jump component are reported separately.
2. **Given** a one-way jump persists, **When** jump diagnostics run, **Then** the jump remains visible and final RV policy is explicit in evidence.

---

### User Story 6 - Robust Cross-Source Aggregation (Priority: P2)

As an operator, I need source aggregation to use robust statistics when multiple venues are configured, so one bad venue cannot silently drive the surface.

**Why this priority**: Multi-venue config without robust aggregation still leaves bad-source risk. PR #609 has upper-quantile and dispersion checks; this feature must extend that to median, trimmed mean, and median/upper-quantile guard policies as TOML-owned choices.

**Independent Test**: Configure three or more source RVs with one outlier, one stale source, and two agreeing sources; verify the configured robust aggregation produces the expected ready/blocking outcome.

**Acceptance Scenarios**:

1. **Given** one source RV is an outlier and others agree, **When** robust aggregation is configured, **Then** the aggregate is not silently dominated by the outlier.
2. **Given** sources are too dispersed under configured policy, **When** a snapshot is computed, **Then** the surface fails closed with auditable dispersion blockers.

---

### User Story 7 - Optional Forecast-Oriented RV Layer (Future Scope)

As a pricing consumer, I need an optional forward-looking RV forecast layer only after measured RV is robust, so pricing can use future uncertainty without replacing measured RV evidence.

**Why deferred**: Forecasting is useful but carries higher model risk. PR #615 keeps measured/noise-robust RV as the authority and explicitly rejects forecast pricing in validation.

**Independent Test**: In a future PR, enable an EWMA/HAR-style forecast policy and prove evidence distinguishes measured RV from forecast RV and fails closed when forecast inputs are invalid.

**Acceptance Scenarios**:

1. **Given** measured RV exists and forecast policy is enabled, **When** snapshot is computed, **Then** evidence contains both measured and forecast RV values.
2. **Given** forecast input is unavailable, **When** measured RV is ready, **Then** behavior follows TOML policy: either measured-only fallback or fail-closed, with evidence.

---

### Edge Cases

- Multiple strategies reference the same `surface_id`; no duplicate engine state or duplicate subscriptions should be created for that surface.
- A source is configured but its data client is unavailable; the source remains auditable and cannot satisfy quorum.
- A venue supports some assets but not all shipped binary-oracle assets; config validation must reject unsupported bindings rather than silently no-op.
- Two source IDs bind the same instrument/client stream; fan-out must update both source states without double-subscribing the physical stream.
- Disabled or non-quorum sources receive observations; they remain diagnostic/audit data and never enter aggregation.
- Future multi-horizon scope: short horizon is ready but long horizon is not; TOML policy determines whether final RV is ready, fallback, or blocked.
- Final RV exactly zero remains valid when all required estimators/horizons are valid and flat.
- A jump is real market information rather than a bad tick; evidence must preserve it rather than hide it.
- Unconfigured raw source IDs must be dropped at the runtime boundary; any raw external ingestion path that records unknown IDs must add TOML-owned cardinality limits before merge.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST introduce a globally usable RV surface runtime/service outside all strategy structs.
- **FR-002**: System MUST remove `RealizedVolEngine` ownership, source subscription ownership, source observation fan-out, and snapshot refresh responsibility from `BinaryOracleEdgeTaker` production state.
- **FR-003**: System MUST expose latest RV snapshots to taker, maker, and future consumers by opaque `surface_id` through a shared runtime/context API.
- **FR-004**: System MUST keep RV policy, source identity, estimator methods, aggregation, and implemented estimator settings TOML-owned. Future horizon and forecast settings must also be TOML-owned when those slices ship.
- **FR-005**: System MUST configure production RV surfaces with multiple enabled venue/source bindings wherever configured data clients and supported instruments exist for the asset.
- **FR-006**: System MUST validate that every configured RV source references an existing data client and supported instrument binding before live runtime starts.
- **FR-007**: System MUST deduplicate physical subscriptions while preserving per-source ID fan-out semantics.
- **FR-008**: System MUST compute per-source RV independently before cross-source aggregation; no source-switching composite price may be used for RV returns.
- **FR-009-FUTURE**: System SHOULD support multi-horizon per-source RV with TOML-owned horizon definitions and final blend/floor/regime policy in a later PR.
- **FR-010-FUTURE**: System SHOULD record per-horizon source RV values, final source RV, final surface RV, and regime diagnostics in snapshots/evidence when multi-horizon ships.
- **FR-011**: System MUST support at least one microstructure-noise robust estimator mode for quote midpoint sources while preserving the baseline fixed-grid estimator as an auditable component.
- **FR-012**: System MUST support jump separation diagnostics that expose continuous RV and jump component without blindly deleting jumps.
- **FR-013**: System MUST support robust cross-source aggregation beyond current upper-quantile, including median, trimmed mean, and median/upper-quantile guard policies with dispersion blocking.
- **FR-014**: System MUST keep zero RV valid when all configured readiness requirements are satisfied.
- **FR-015**: System MUST fail closed when source quorum, aggregation, estimator, or selected pricing-component inputs are unavailable or invalid. Future horizon and forecast policies must also fail closed when selected.
- **FR-016**: System MUST preserve source diagnostics for disabled, non-quorum, stale, rejected, divergent, and unknown sources.
- **FR-017**: System MUST update decision evidence schema and docs for all new RV fields, labels, and config fingerprint changes.
- **FR-018**: System MUST keep strategy code intent-only: no submit mechanics, venue rules, execution admissibility, or RV policy may be added under `src/strategies/*`.
- **FR-019**: System MUST include source-fence tests proving RV lifecycle and math policy do not live in strategy modules.
- **FR-020**: System MUST use TDD for every production behavior change: write failing tests, verify red, implement minimal green, then refactor.

### Key Entities

- **RealizedVolSurfaceRuntime**: Global owner of configured surfaces, subscriptions, observation routing, and latest snapshots.
- **RealizedVolSurfaceState**: Per-surface runtime state keyed by `surface_id`; owns engine state, source bindings, and latest snapshot.
- **RealizedVolSourceBinding**: TOML-derived mapping from data client/instrument/source class/sample kind to source ID and surface ID.
- **PhysicalSubscriptionKey**: Deduplicated market-data subscription key so multiple sources/consumers can share one feed subscription.
- **HorizonRvEstimate**: Reserved future per-source per-horizon RV output with readiness, coverage, gaps, and estimator diagnostics.
- **RobustRvEstimate**: Final per-source and per-surface RV with measured, noise-robust, continuous, jump, and selected pricing components. Forecast components are future scope.
- **RealizedVolSnapshot**: Public consumer/evidence payload containing ready value, diagnostics, blockers, sources used, estimator metadata, and fingerprint.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: No production strategy struct contains a `RealizedVolEngine` field or owns RV source subscriptions after implementation.
- **SC-002**: A test with two consumers referencing the same `surface_id` proves both receive the same global snapshot from one runtime surface state.
- **SC-003**: Each shipped production binary-oracle RV surface uses at least two configured sources where the asset is supported by existing public data clients; exceptions are documented in TOML/evidence, not code.
- **SC-004-FUTURE**: Multi-horizon tests must prove short/medium/long estimates and final blended RV are recorded and used according to TOML policy when that future slice ships.
- **SC-005**: Noise-robust estimator tests prove alternating midpoint noise is reduced relative to naive fixed-grid RV without erasing a real directional move.
- **SC-006**: Jump-separation tests prove bad-tick spike and persistent jump cases produce distinct diagnostics and auditable final policy.
- **SC-007**: Cross-source robust aggregation tests prove one divergent source cannot silently dominate the aggregate.
- **SC-008**: Exact-head CI passes `fmt-check`, `clippy`, `deny`, `source-fence`, all `nextest` shards, `test`, `gate`, CodeQL, actionlint, and backtester gate before external review and merge.

## Assumptions

- `main` after PR #609 is authoritative for current RV surface state.
- Existing public data clients in `config/root.toml` are candidates for multi-venue RV only if their instruments and adapters already support the asset/source class needed.
- This feature may add TOML schema fields and evidence schema fields but must not add hardcoded venue or asset behavior to Rust core.
- Maker consumption means exposing the global snapshot API for maker/future consumers; maker-specific pricing integration may be a later consumer PR if maker strategy runtime is not yet present.
- This feature must not reintroduce the deleted legacy internal taker RV estimator or any strategy-owned fallback path.

## Relations

Related to #609 and #614.
