# NT-Backed IV Engine Quickstart

This quickstart is a TOML shape, not a runtime fixture. Placeholder names and numeric values must be replaced by operator-owned values in the real config.

## Configure One IV Profile

An IV profile is the lifecycle boundary. Sources, strategy authorization, enabled products, freshness, retention, and query policies live together so changing a source does not require a second edit elsewhere.

```text
[iv]
schema_version = operator_schema_version

[[iv.profiles]]
profile_id = "configured_iv_profile"
strategy_ids = ["configured_strategy"]
enabled_products = [
  "raw_option_greeks",
  "raw_option_chain",
  "raw_aggregate_greeks",
  "raw_custom_volatility",
  "iv_point",
  "iv_greeks_point",
  "aggregate_greeks",
  "smile",
  "surface",
  "custom_evidence",
  "derived_iv",
  "source_health",
]
enabled_bases = ["configured_basis"]
accepted_conventions = ["configured_convention"]
max_age_ns = operator_positive_integer_ns
retention_events = operator_positive_integer

[iv.profiles.interpolation]
method = "configured_interpolation_method"
strike_axis = "configured_strike_axis"
tenor_axis = "configured_tenor_axis"
minimum_points = operator_positive_integer
extrapolation = "configured_extrapolation_policy"

[iv.profiles.fallback]
candidate_order = ["configured_candidate"]
maximum_timestamp_skew_ns = operator_nonnegative_integer_ns

[iv.profiles.quorum]
minimum_sources = operator_positive_integer
agreement_band = "configured_agreement_band"
tie_break = "configured_tie_break"

[[iv.profiles.sources]]
source_id = "configured_options_greeks_source"
data_client_id = "configured_options_client"
source_kind = "option_greeks"

[iv.profiles.sources.selectors]
instrument_ids = ["configured.instrument"]

[[iv.profiles.sources]]
source_id = "configured_option_chain_source"
data_client_id = "configured_options_client"
source_kind = "option_chain"

[iv.profiles.sources.selectors]
series_ids = ["configured.series"]
strike_range = "configured_range"

[[iv.profiles.sources]]
source_id = "configured_aggregate_greeks_source"
data_client_id = "configured_options_client"
source_kind = "aggregate_greeks"

[iv.profiles.sources.selectors]
underlying_selectors = ["configured.underlying"]

[[iv.profiles.sources]]
source_id = "configured_custom_volatility_source"
data_client_id = "configured_options_client"
source_kind = "custom_volatility"

[iv.profiles.sources.selectors]
custom_data_type = "configured_custom_type"
```

## Query Flow

1. Root TOML loads IV profiles with the rest of Bolt configuration.
2. Live-node startup creates the IV engine and NT subscription plans for configured sources.
3. Engine ingests NT events, preserves raw payloads, indexes IV products, and records source health.
4. Strategy registration gives authorized strategies an IV query handle for their configured profile.
5. Strategy queries IV products through the engine API.
6. Engine returns either a product with provenance or a typed rejection.

## Operator Checks

- Every runtime value is in TOML.
- One IV profile owns the source lifecycle and strategy authorization.
- No strategy owns IV subscription mechanics or NT helper-backed IV derivation.
- No concrete asset, venue, market, cadence, source ID, or instrument value is embedded in IV code.
- `Cargo.toml` remains the only source of truth for NT revision.
