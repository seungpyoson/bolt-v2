# NT-Backed IV Engine Data Model

## IvCapabilityLedger

Source-backed inventory of IV/options surfaces discovered in the Cargo-pinned NautilusTrader checkout.

Fields:

- `nt_revision_source`: `Cargo.toml` dependency graph resolved with `cargo metadata --locked`
- `nt_lock_source`: `Cargo.lock` package source URL and revision for each NT crate
- `resolved_checkout_path`: Cargo git checkout path for the locked NT revision
- `surfaces`: discovered NT IV/options surfaces
- `candidate_sweep_terms`: source-wide IV/options discovery terms
- `classification`: supported, unreachable from the Rust binary, or explicitly excluded by approved rationale
- `evidence_path`: NT source path and symbol name
- `engine_mapping`: IV source kind, product kind, helper, runtime operation, or API surface that owns the capability

Validation:

- Every discovered surface must have exactly one classification.
- Supported surfaces must map to an engine source kind, product kind, helper, runtime operation, or API.
- The ledger must fail if it cannot resolve the checkout from Cargo metadata and lockfile evidence.
- The ledger must sweep the full resolved NT checkout for Rust public symbols and modules whose path, symbol, doc comment, or enclosing module matches IV/options discovery terms, not only a curated seed list.
- Every sweep candidate must be classified as supported, unreachable from the Rust binary, not IV/options related after inspection, or explicitly excluded with approved rationale.
- No handwritten NT revision or local checkout path is accepted as capability evidence.

## IvProfile

TOML lifecycle boundary for one configured IV access domain.

Fields:

- `profile_id`
- `schema_version`
- `strategy_ids`
- `sources`
- `enabled_products`
- `freshness`
- `retention`
- `memory_bounds`
- `accepted_conventions`
- `enabled_bases`
- `interpolation_policy`
- `fallback_policy`
- `quorum_policy`
- `projection_policy`
- `derived_input_policy`

Validation:

- Profile IDs are unique and non-empty.
- Unknown or unsupported schema versions reject at startup.
- Strategy authorization, sources, source lifecycle, enabled products, memory bounds, and query policies live in this profile boundary.
- Swapping, renaming, adding, or removing a source requires editing only this profile.
- A profile with no source and no derived-input policy rejects at startup.
- No runtime value is inferred from code when TOML omits it.

## IvSourceConfig

TOML-derived source definition owned by an `IvProfile`.

Fields:

- `profile_id`
- `source_id`
- `data_client_id`
- `source_kind`
- `selector`
- `nt_params`
- `enabled_products`

Validation:

- Source IDs are unique within a profile and non-empty.
- Source kind is a known enum from the capability ledger.
- Selector is a typed `IvSelector` variant compatible with the source kind.
- Numeric bounds are positive where required.
- Source config does not carry strategy authorization outside its owning profile.

## IvSelector

Typed union for source and query selectors. The selector is not an untyped map, and source-scope selectors are distinct from query-scope selectors.

Variants:

- `SourceOptionGreeksSelector`: `instrument_ids`, `nt_params`
- `SourceOptionChainSelector`: `series_ids`, `strike_range_policy`, `nt_params`
- `SourceAggregateGreeksSelector`: `aggregate_key`, `underlying_selectors`, `nt_params`
- `SourceCustomImpliedVolatilitySelector`: `custom_iv_data_type`, `custom_iv_data_fields`, `nt_params`
- `PointQuerySelector`: `instrument_ids`, `basis`, `as_of_ns`, `source_filter`
- `SmileQuerySelector`: `series_id`, `side`, `basis`, `as_of_ns`
- `SurfaceQuerySelector`: `series_selectors`, `basis`, `as_of_ns`
- `AggregateGreeksQuerySelector`: `aggregate_key`, `underlying_selectors`, `as_of_ns`
- `IvEvidenceQuerySelector`: `iv_evidence_kind`, `source_filter`, `as_of_ns`
- `ProjectedScalarIvQuerySelector`: `input_selector`, `projection_policy_id`, `as_of_ns`
- `DerivedIvQuerySelector`: `instrument_id`, `helper_policy_id`, `as_of_ns`
- `SourceHealthQuerySelector`: `source_filter`, `state_filter`

Product-kind mapping:

- `iv_point`: `PointQuerySelector`
- `iv_greeks_point`: `PointQuerySelector`
- `smile`: `SmileQuerySelector`
- `surface`: `SurfaceQuerySelector`
- `aggregate_greeks`: `AggregateGreeksQuerySelector`
- `custom_iv_evidence`: `IvEvidenceQuerySelector`
- `projected_scalar_iv`: `ProjectedScalarIvQuerySelector`
- `derived_iv`: `DerivedIvQuerySelector`
- `source_health`: `SourceHealthQuerySelector`

Validation:

- Exactly one selector variant is present.
- A source selector variant must match its `source_kind` and may only drive subscription planning.
- A query selector variant must match its requested `product_kind` and may only filter engine products.
- Empty selector collections reject.
- Source-scope selector fields are not reused as query filters without conversion into the matching query selector variant.
- Selector fingerprints are recorded in provenance so policy decisions can be audited.

## IvSubscriptionPlan

Concrete NT subscribe/unsubscribe operations derived from `IvProfile` and `IvSourceConfig`.

Fields:

- `profile_id`
- `source_id`
- `operation`
- `nt_source_kind`
- `client_id`
- `selector`
- `params`
- `subscription_generation`

Validation:

- Every operation maps to one configured profile and source.
- Start, stop, reload, unsubscribe, and source removal operations are represented.
- No operation contains hardcoded instrument, venue, asset, cadence, source, or strategy values.
- Reload increments `subscription_generation`; stale generations cannot satisfy current queries.

## IvRuntimeBinding

Live NT integration surface for the IV engine.

Fields:

- `profile_id`
- `source_id`
- `data_actor_handle`
- `msgbus_handler`
- `subscription_plan`
- `event_router`
- `source_health_sink`

Validation:

- Option-greeks sources map to NT `subscribe_option_greeks` runtime operations.
- Option-chain sources map to NT `subscribe_option_chain` runtime operations.
- Aggregate-greeks sources map to NT greeks topic subscription operations.
- Custom-implied-volatility sources map to ledger-classified NT custom-data subscription operations.
- Subscription failures, handler failures, and unsupported runtime mappings update `IvSourceHealth` and reject affected current queries.
- The event router preserves raw events before any indexing or policy projection.

## IvRawEvent

Preserved NT payload or custom implied-volatility event.

Fields:

- `profile_id`
- `source_id`
- `received_ts_ns`
- `payload_kind`
- `payload`
- `provenance`

Validation:

- Payload kind must match the source kind or an allowed NT publication for that source.
- Raw payload is retained within configured bounds.
- Removed or stale sources cannot satisfy current queries.

## IvRawAuditAccess

Audit, replay, and test-only raw payload reader for preserved NT payloads.

Fields:

- `raw_event_id`
- `profile_id`
- `source_id`
- `payload_kind`
- `payload`
- `provenance`
- `access_purpose`

Validation:

- Strategy query handles cannot request or receive `IvRawAuditAccess`.
- `src/strategies/**` source-fence rejects imports or calls of the raw audit reader and raw payload DTOs.
- Raw access is allowed for audit, replay, and tests only when provenance and access purpose are recorded.
- Strategy-facing products may expose `raw_event_id` through provenance but not the raw payload value.

## IvProvenance

Required audit schema attached to every raw event, indexed product, derived product, projection, policy output, and rejection.

Fields:

- `profile_id`
- `source_id`
- `source_kind`
- `selector_fingerprint`
- `nt_revision`
- `nt_evidence_path`
- `nt_symbol`
- `raw_event_id`
- `payload_kind`
- `input_event_ids`
- `helper_identity`
- `policy_decisions`
- `transformation_steps`
- `ts_event_ns`
- `ts_init_ns`
- `received_ts_ns`
- `ingest_sequence`
- `subscription_generation`
- `source_health_state`
- `reject_reason`

Validation:

- Every returned or rejected product has provenance.
- Always required: `profile_id`, `source_id`, `source_kind`, `selector_fingerprint`, `nt_revision`, `nt_evidence_path`, `nt_symbol`, `ts_event_ns`, `received_ts_ns`, `ingest_sequence`, `subscription_generation`, and `source_health_state`.
- Required when backed by raw input: `raw_event_id`, `payload_kind`.
- Required when derived: `input_event_ids`, `helper_identity`.
- Required when policy-produced or projected: `policy_decisions`, `transformation_steps`.
- Required when rejected: `reject_reason`.
- Derived products include all input references and helper identity.
- Policy outputs include candidate lists, rejected candidates, accepted candidates, and policy names.
- Timestamp fields are typed nanoseconds.
- Missing required provenance rejects the product even when the raw value is otherwise usable.

## IvPoint

One timestamped IV value.

Fields:

- `profile_id`
- `source_id`
- `instrument_id`
- `basis`
- `iv`
- `convention`
- `ts_event_ns`
- `ts_init_ns`
- `provenance`

Validation:

- IV is finite and positive.
- Basis is mark, bid, ask, or derived.
- Timestamp units are nanoseconds.

## IvGreeksPoint

IV point plus NT greek values and related fields.

Fields:

- all `IvPoint` fields
- `greeks`
- `underlying_price`
- `open_interest`

Validation:

- Greeks are preserved as NT provided them.
- Missing optional greeks do not discard raw payloads.

## IvAggregateGreeks

Queryable product derived from NT aggregate greeks events.

Fields:

- `profile_id`
- `source_id`
- `selector`
- `greeks`
- `ts_event_ns`
- `ts_init_ns`
- `provenance`

Validation:

- Raw NT aggregate greeks payload is preserved before indexing.
- Selector and provenance identify the configured aggregate source.
- Missing or malformed values reject indexing without discarding the raw event.

## IvSmile

Strike-indexed IV points for one option series, side, source, and event time.

Fields:

- `profile_id`
- `source_id`
- `series_id`
- `side`
- `basis`
- `points_by_strike`
- `atm_strike`
- `ts_event_ns`
- `provenance`

Validation:

- Strikes are retained as NT price values.
- Smile construction does not invent missing points.
- Interpolation is not part of construction; it is a query policy decision.

## IvSurface

Collection of retained smiles across configured series selectors.

Fields:

- `profile_id`
- `surface_selector`
- `source_id`
- `basis`
- `smiles`
- `as_of_ns`
- `provenance`

Validation:

- Surface contains only smiles authorized by the query selector and profile.
- Stale smiles are excluded from current queries.

## IvEvidence

Custom IV evidence not equivalent to instrument-level option IV.

Fields:

- `profile_id`
- `source_id`
- `iv_evidence_kind`
- `value`
- `ts_event_ns`
- `ts_init_ns`
- `provenance`

Validation:

- Evidence kind must not be mislabeled as an option-chain point.
- Projection into IV requires explicit configured policy.

## IvDerivedInputPolicy

Typed policy for resolving inputs needed by NT math helpers.

Fields:

- `required_fields`
- `field_sources`
- `freshness`
- `max_input_skew_ns`
- `bounds`
- `convention_policy`

Allowed field source kinds:

- `query_supplied`
- `profile_source_ref`
- `instrument_metadata`
- `operator_configured_value`

Validation:

- Required fields include option price, underlying price, strike, option side, time-to-expiry, rate, carry, source timestamps, and convention.
- Operator-configured values are TOML-owned and provenance-recorded.
- No rate, carry, time convention, or fallback input is guessed in code.
- Missing, stale, skewed, non-finite, or convention-incompatible inputs reject.

## IvDerivedInputSet

Resolved input bundle used for one NT helper invocation.

Fields:

- `option_price`
- `underlying_price`
- `strike`
- `option_side`
- `time_to_expiry`
- `rate`
- `carry`
- `input_timestamps_ns`
- `convention`
- `input_provenance`

Validation:

- All values satisfy `IvDerivedInputPolicy`.
- The bundle is immutable once attached to an `IvDerivedPoint`.
- Query-supplied inputs and profile-resolved inputs are both recorded in provenance.

## IvDerivedPoint

IV and greeks produced by NT math helpers.

Fields:

- `profile_id`
- `source_id`
- `helper`
- `inputs`
- `output_iv`
- `output_greeks`
- `ts_event_ns`
- `provenance`

Validation:

- Inputs come from `IvDerivedInputSet`.
- Missing or invalid input rejects.
- Output IV is finite, positive, and within configured bounds.

## IvProjectionPolicy

Typed policy for converting available products into the requested product shape.

Fields:

- `projection_kind`
- `basis_selection`
- `source_eligibility`
- `strike_selection`
- `tenor_selection`
- `evidence_mapping`
- `max_projection_input_skew_ns`
- `fallback_policy_ref`
- `interpolation_policy_ref`
- `quorum_policy_ref`

Validation:

- Scalar IV from a smile, surface, or evidence product requires explicit projection policy.
- Projection cannot silently change basis, convention, source eligibility, timestamp, product kind, or evidence semantics.
- Projection rejects when input products exceed `max_projection_input_skew_ns`.
- Unknown projection kinds reject at startup.
- Every projection records input products and policy decisions in provenance.

## IvInterpolationPolicy

Typed policy for query-time smile or surface interpolation.

Fields:

- `method`
- `strike_axis`
- `tenor_axis`
- `minimum_points`
- `eligible_sources`
- `extrapolation`

Validation:

- Unknown methods reject at startup.
- Interpolation rejects when required axes or minimum points are unavailable.
- Extrapolation is rejected unless TOML explicitly permits the configured mode.
- Every interpolation output records input points, axes, method, and source eligibility.

## IvFallbackPolicy

Typed policy for ordered source, product, basis, or evidence fallback.

Fields:

- `candidate_order`
- `eligible_sources`
- `maximum_timestamp_skew_ns`
- `required_provenance_fields`

Validation:

- Candidate order is non-empty when fallback is enabled.
- Fallback never changes basis, convention, source, or product kind silently.
- If no candidate qualifies, the query rejects with `FallbackRejected`.

## IvQuorumPolicy

Typed policy for multi-source agreement before returning a value.

Fields:

- `minimum_sources`
- `eligible_sources`
- `agreement_band`
- `tie_break`

Validation:

- Minimum source count is positive and cannot exceed eligible source count.
- Returned values record participating and rejected sources.
- If quorum is not met, the query rejects with `QuorumNotMet`.

## IvMemoryBounds

Per-profile memory and retention limits.

Fields:

- `max_raw_events`
- `max_indexed_points`
- `max_smiles`
- `max_surfaces`
- `max_derived_points`
- `max_source_health_events`

Validation:

- Every bound is TOML-owned and positive where retention is enabled.
- Eviction records provenance and source-health events.
- Retention misses reject current and historical queries with a typed reason.

## IvSourceHealth

Source status for subscriptions and data quality.

Fields:

- `profile_id`
- `source_id`
- `subscription_state`
- `last_event_ts_ns`
- `last_reject_reason`
- `reject_counts`
- `stale_state`
- `retention_state`
- `subscription_generation`

States:

- `configured`
- `subscribing`
- `active`
- `stale`
- `unsubscribing`
- `removed`
- `subscription_failed`
- `rejected`

Allowed transitions:

- `configured` -> `subscribing`
- `subscribing` -> `active`
- `subscribing` -> `subscription_failed`
- `active` -> `stale`
- `active` -> `unsubscribing`
- `stale` -> `active`
- `stale` -> `unsubscribing`
- `unsubscribing` -> `removed`
- `subscription_failed` -> `subscribing`
- any non-removed state -> `rejected` when config validation or runtime mapping fails
- `removed` is terminal for the subscription generation

Validation:

- Removed, failed, or unsubscribed sources cannot satisfy current queries.
- Stale state is computed from typed timestamp units.
- Reload races are resolved by `subscription_generation`.

## IvQuery

Strategy-facing request.

Fields:

- `strategy_id`
- `profile_id`
- `selector`
- `product_kind`
- `basis`
- `as_of_ns`
- `projection_policy`
- `derived_inputs`
- `history_policy`
- `policy_overrides`

Validation:

- Strategy must be authorized for the profile and selector.
- Unknown product kinds reject.
- Raw-payload product kinds reject on strategy-facing query handles.
- Current queries reject stale or retained-only data.
- Derived-product queries include `derived_inputs` or resolve them through the profile's `IvDerivedInputPolicy`.
- Query-time policy overrides are allowed only when the owning profile permits them.

## IvRejectReason

Typed rejection surface.

Required reasons:

- `SourceNotConfigured`
- `SelectorNotAuthorized`
- `UnsupportedSourceKind`
- `UnsupportedBasis`
- `UnsupportedConvention`
- `MissingIvBasis`
- `InvalidIvValue`
- `MissingDerivedInput`
- `InvalidDerivedInput`
- `HelperFailed`
- `StaleData`
- `ClockSkew`
- `RetentionMiss`
- `SourceRemoved`
- `SubscriptionFailed`
- `PayloadKindMismatch`
- `MissingProjectionPolicy`
- `ProjectionRejected`
- `InterpolationRejected`
- `ExtrapolationRejected`
- `FallbackRejected`
- `QuorumNotMet`
- `CapabilityUnclassified`
- `ProvenanceIncomplete`
