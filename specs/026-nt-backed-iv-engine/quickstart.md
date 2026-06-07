# NT-Backed IV Engine Quickstart

This quickstart is a TOML shape, not a runtime fixture. Placeholder names and numeric values must be replaced by operator-owned values in the real config.

## Configure One IV Profile

An IV profile is the lifecycle boundary. Sources, strategy authorization, enabled products, freshness, retention, memory bounds, and query policies live together so changing a source does not require a second edit elsewhere.

```text
[iv]
schema_version = operator_schema_version

[[iv.profiles]]
profile_id = "operator_iv_profile"
strategy_ids = ["operator_strategy"]
enabled_products = [
  "raw_option_greeks",
  "raw_option_chain",
  "raw_aggregate_greeks",
  "raw_custom_implied_volatility",
  "iv_point",
  "iv_greeks_point",
  "aggregate_greeks",
  "smile",
  "surface",
  "custom_iv_evidence",
  "projected_scalar_iv",
  "derived_iv",
  "source_health",
]
enabled_bases = ["operator_basis"]
accepted_conventions = ["operator_convention"]
max_age_ns = operator_positive_integer_ns
retention_events = operator_positive_integer

[iv.profiles.memory_bounds]
max_raw_events = operator_positive_integer
max_indexed_points = operator_positive_integer
max_smiles = operator_positive_integer
max_surfaces = operator_positive_integer
max_derived_points = operator_positive_integer
max_source_health_events = operator_positive_integer

[iv.profiles.projection]
projection_kind = "operator_projection_kind"
basis_selection = "operator_basis_selection"
strike_selection = "operator_strike_selection"
tenor_selection = "operator_tenor_selection"
evidence_mapping = "operator_evidence_mapping"

[iv.profiles.interpolation]
method = "operator_interpolation_method"
strike_axis = "operator_strike_axis"
tenor_axis = "operator_tenor_axis"
minimum_points = operator_positive_integer
extrapolation = "operator_extrapolation_policy"

[iv.profiles.fallback]
candidate_order = ["operator_candidate"]
maximum_timestamp_skew_ns = operator_nonnegative_integer_ns

[iv.profiles.quorum]
minimum_sources = operator_positive_integer
agreement_band = "operator_agreement_band"
tie_break = "operator_tie_break"

[iv.profiles.derived_inputs]
option_price = { source = "query_supplied" }
underlying_price = { source = "profile_source_ref", source_id = "operator_source_ref" }
strike = { source = "instrument_metadata" }
option_side = { source = "instrument_metadata" }
time_to_expiry = { source = "instrument_metadata" }
rate = { source = "operator_configured_value", value = operator_rate_value }
carry = { source = "operator_configured_value", value = operator_carry_value }
max_input_skew_ns = operator_nonnegative_integer_ns

[[iv.profiles.sources]]
source_id = "operator_option_greeks_source"
data_client_id = "operator_options_client"
source_kind = "option_greeks"
selector = { option_greeks = { instrument_ids = ["operator.instrument_id"] } }

[[iv.profiles.sources]]
source_id = "operator_option_chain_source"
data_client_id = "operator_options_client"
source_kind = "option_chain"
selector = { option_chain = { series_ids = ["operator.series_id"], strike_range_policy = "operator_strike_range" } }

[[iv.profiles.sources]]
source_id = "operator_aggregate_greeks_source"
data_client_id = "operator_options_client"
source_kind = "aggregate_greeks"
selector = { aggregate_greeks = { underlying_selectors = ["operator.underlying_selector"] } }

[[iv.profiles.sources]]
source_id = "operator_custom_implied_volatility_source"
data_client_id = "operator_options_client"
source_kind = "custom_implied_volatility"
selector = { custom_implied_volatility = { custom_iv_data_type = "operator_custom_iv_data_type", custom_iv_data_fields = ["operator_field"] } }
```

## Query Flow

1. Root TOML loads IV profiles with the rest of Bolt configuration.
2. Live-node startup creates the IV engine and NT subscription plans for configured sources.
3. Engine binds to NT runtime subscription APIs and records source health for each source.
4. Engine ingests NT events, preserves raw payloads, indexes IV products, and records provenance.
5. Strategy registration gives authorized strategies an IV query handle for their configured profile.
6. Strategy queries IV products through the engine API with typed selectors.
7. Engine returns either a product with provenance or a typed rejection.

## Operator Checks

- Every runtime value is in TOML.
- One IV profile owns the source lifecycle and strategy authorization.
- Projection and derived-input policies are explicit before scalar IV or derived IV can be returned.
- No strategy owns IV subscription mechanics or NT helper-backed IV derivation.
- No concrete asset, venue, market, cadence, source ID, or instrument value is embedded in IV code.
- `Cargo.toml` remains the only source of truth for NT revision.
