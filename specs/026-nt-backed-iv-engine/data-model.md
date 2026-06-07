# NT-Backed IV Engine Data Model

## IvCapabilityLedger

Source-backed inventory of IV/options surfaces discovered in the Cargo-pinned NautilusTrader checkout.

Fields:

- `nt_revision_source`: always `Cargo.toml`
- `surfaces`: discovered NT IV/options surfaces
- `classification`: supported, unreachable from Rust binary, or explicitly excluded by approved rationale
- `evidence_path`: NT source path and symbol name

Validation:

- Every discovered surface must have exactly one classification.
- Supported surfaces must map to an engine source kind, product kind, helper, or API.

## IvProfile

TOML lifecycle boundary for one configured IV access domain.

Fields:

- `profile_id`
- `strategy_ids`
- `sources`
- `enabled_products`
- `freshness`
- `retention`
- `accepted_conventions`
- `enabled_bases`
- `interpolation_policy`
- `fallback_policy`
- `quorum_policy`
- `projection_policy`
- `derived_input_policy`

Validation:

- Profile IDs are unique and non-empty.
- Strategy authorization, sources, source lifecycle, enabled products, and query policies live in this profile boundary.
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
- `selectors`
- `nt_params`
- `enabled_products`

Validation:

- Source IDs are unique within a profile and non-empty.
- Source kind is a known enum from the capability ledger.
- Selectors are non-empty and compatible with the source kind.
- Numeric bounds are positive where required.
- Source config does not carry strategy authorization outside its owning profile.

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

Validation:

- Every operation maps to one configured profile and source.
- Start, stop, reload, unsubscribe, and source removal operations are represented.
- No operation contains hardcoded instrument, venue, asset, cadence, source, or strategy values.

## IvRawEvent

Preserved NT payload or custom volatility event.

Fields:

- `profile_id`
- `source_id`
- `received_ts`
- `payload_kind`
- `payload`
- `provenance`

Validation:

- Payload kind must match the source kind or an allowed NT publication for that source.
- Raw payload is retained within configured bounds.
- Removed or stale sources cannot satisfy current queries.

## IvPoint

One timestamped IV value.

Fields:

- `profile_id`
- `source_id`
- `instrument_id`
- `basis`
- `iv`
- `convention`
- `ts_event`
- `ts_init`
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
- `ts_event`
- `ts_init`
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
- `ts_event`
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
- `as_of`
- `provenance`

Validation:

- Surface contains only smiles authorized by the query selector and profile.
- Stale smiles are excluded from current queries.

## IvEvidence

Custom volatility or broad volatility evidence not equivalent to instrument-level option IV.

Fields:

- `profile_id`
- `source_id`
- `evidence_kind`
- `value`
- `ts_event`
- `ts_init`
- `provenance`

Validation:

- Evidence kind must not be mislabeled as an option-chain point.
- Projection into IV requires explicit configured policy.

## IvDerivedPoint

IV and greeks produced by NT math helpers.

Fields:

- `profile_id`
- `source_id`
- `helper`
- `inputs`
- `output_iv`
- `output_greeks`
- `ts_event`
- `provenance`

Validation:

- Inputs include option price, underlying price, strike, option side, time-to-expiry, rate, carry, source timestamps, and convention policy.
- Missing or invalid input rejects.
- Output IV is finite, positive, and within configured bounds.

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
- `maximum_timestamp_skew`
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

## IvSourceHealth

Source status for subscriptions and data quality.

Fields:

- `profile_id`
- `source_id`
- `subscription_state`
- `last_event_ts`
- `last_reject_reason`
- `reject_counts`
- `stale_state`
- `retention_state`

Validation:

- Removed or unsubscribed sources cannot satisfy current queries.
- Stale state is computed from typed timestamp units.

## IvQuery

Strategy-facing request.

Fields:

- `strategy_id`
- `profile_id`
- `selector`
- `product_kind`
- `basis`
- `as_of`
- `projection_policy`
- `history_policy`

Validation:

- Strategy must be authorized for the profile and selector.
- Unknown product kinds reject.
- Current queries reject stale or retained-only data.
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
- `PayloadKindMismatch`
- `ProjectionRejected`
- `InterpolationRejected`
- `ExtrapolationRejected`
- `FallbackRejected`
- `QuorumNotMet`
