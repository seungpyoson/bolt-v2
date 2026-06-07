# NT-Backed IV Engine Quickstart

This quickstart is a TOML shape, not a runtime fixture. Placeholder names and numeric values must be replaced by operator-owned values in the real config.

## Configure One IV Profile

An IV profile is the lifecycle boundary. Sources, strategy authorization, strategy-enabled products, audit-enabled raw products, freshness, retention, memory bounds, and query policies live together so changing a source does not require a second edit elsewhere.

```text
[iv]

[[iv.profiles]]
profile_id = "operator_iv_profile"
schema_version = operator_schema_version
strategy_ids = ["operator_strategy"]
enabled_products = [
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
schema_version_policy = "operator_schema_version_policy"

[iv.profiles.selector_authorization]
authorization_mode = "operator_authorization_mode"
allowed_product_kinds = ["operator_product_kind"]
allowed_selector_fingerprints = ["operator_selector_fingerprint"]
allowed_source_ids = ["operator_source_id"]

[iv.profiles.audit]
enabled_raw_products = [
  "raw_option_greeks",
  "raw_option_chain",
  "raw_aggregate_greeks",
  "raw_custom_implied_volatility",
]
authorized_audit_handles = ["operator_audit_handle"]
access_purposes = ["operator_audit_or_replay_purpose"]
eligible_sources = ["operator_source_id"]
audit_retention = operator_positive_integer

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
max_projection_input_skew_ns = operator_nonnegative_integer_ns

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

[iv.profiles.helper_policy]
helper_policy_id = "operator_helper_policy"
nt_helper_symbol = "operator_nt_helper_symbol"
parameter_signature = "operator_helper_parameter_signature"
allowed_outputs = ["operator_helper_output"]
output_bounds = "operator_output_bounds_policy"

[iv.profiles.derived_inputs]
option_price = { source = "query_supplied" }
underlying_price = { source = "profile_source_ref", source_id = "operator_source_ref" }
strike = { source = "instrument_metadata" }
option_side = { source = "instrument_metadata" }
time_to_expiry = { source = "instrument_metadata" }
rate = { source = "operator_configured_value", value = operator_rate_value, valid_until_ns = operator_timestamp_ns }
carry = { source = "operator_configured_value", value = operator_carry_value, valid_until_ns = operator_timestamp_ns }
max_input_skew_ns = operator_nonnegative_integer_ns

[iv.profiles.bounds]
iv = "operator_iv_bounds_policy"
rate = "operator_rate_bounds_policy"
carry = "operator_carry_bounds_policy"
time_to_expiry = "operator_time_to_expiry_bounds_policy"
strike = "operator_strike_bounds_policy"
price = "operator_price_bounds_policy"
agreement_band = "operator_agreement_band_bounds_policy"

[[iv.profiles.sources]]
source_id = "operator_option_greeks_source"
selector_fingerprint = "operator_option_greeks_selector"
client_id = "operator_options_client"
source_kind = "option_greeks"

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["operator.instrument_id"]

[[iv.profiles.sources]]
source_id = "operator_option_chain_source"
selector_fingerprint = "operator_option_chain_selector"
client_id = "operator_options_client"
source_kind = "option_chain"

[iv.profiles.sources.selector]
selector_kind = "source_option_chain"
series_ids = ["operator.series_id"]
strike_range_policy = "operator_strike_range"

[[iv.profiles.sources]]
source_id = "operator_aggregate_greeks_source"
selector_fingerprint = "operator_aggregate_greeks_selector"
client_id = "operator_options_client"
source_kind = "aggregate_greeks"

[iv.profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "operator_aggregate_key"
underlying_selectors = ["operator.underlying_selector"]

[[iv.profiles.sources]]
source_id = "operator_custom_implied_volatility_source"
selector_fingerprint = "operator_custom_implied_volatility_selector"
client_id = "operator_options_client"
source_kind = "custom_implied_volatility"

[iv.profiles.sources.selector]
selector_kind = "source_custom_implied_volatility"
custom_iv_data_type = "operator_custom_iv_data_type"
custom_iv_data_fields = ["operator_field"]
```

## Query Flow

1. Root TOML loads IV profiles with the rest of Bolt configuration.
2. Live-node startup creates the IV engine and NT subscription plans for configured sources.
3. Engine binds to NT runtime subscription APIs and records source health for each source.
4. Engine ingests NT events, preserves raw payloads, indexes IV products, and records provenance.
5. Strategy registration gives authorized strategies an IV query handle for their configured profile.
6. Strategy queries IV products through the engine API with typed selectors.
7. Audit or replay modules can request raw payloads through the audit handle.
8. Engine returns either a product with provenance or a typed rejection.

## Operator Checks

- Every runtime value is in TOML.
- One IV profile owns the source lifecycle and strategy authorization.
- Strategy authorization is explicitly profile-wide or selector-scoped.
- Raw audit/replay access is explicitly configured through profile audit policy.
- Projection and derived-input policies are explicit before scalar IV or derived IV can be returned.
- Helper policy explicitly selects NT helper identity and output bounds.
- Raw payload retrieval is audit/replay-only and not available through strategy query handles.
- No strategy owns IV subscription mechanics or NT helper-backed IV derivation.
- No concrete asset, venue, market, cadence, source ID, or instrument value is embedded in IV code.
- `Cargo.toml` remains the only source of truth for NT revision.
