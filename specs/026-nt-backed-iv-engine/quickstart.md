# NT-Backed IV Engine Quickstart

This quickstart is a TOML shape, not a runtime fixture. Placeholder names and numeric values must be replaced by operator-owned values in the real config.

## Configure One IV Profile

An IV profile is the lifecycle boundary. Sources, strategy authorization, strategy-enabled products, audit-enabled raw products, freshness, retention, memory bounds, and query policies live together so changing a source does not require a second edit elsewhere.

```text
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "operator_iv_profile"
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
max_raw_events = operator_positive_integer
max_indexed_points = operator_positive_integer
max_smiles = operator_positive_integer
max_surfaces = operator_positive_integer
max_source_health_events = operator_positive_integer

[iv.profiles.selector_authorization]
authorization_mode = "operator_authorization_mode"
allowed_product_kinds = ["operator_product_kind"]
allowed_selector_fingerprints = ["operator_selector_fingerprint"]
allowed_source_ids = ["operator_source_id"]

[[iv.profiles.sources]]
source_id = "operator_option_greeks_source"
selector_fingerprint = "operator_option_greeks_selector"
client_id = "operator_options_client"
source_kind = "option_greeks"
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["operator.instrument_id"]

[iv.profiles.sources.selector.nt_params]
operator_nt_param = "operator_value"

[iv.profiles.sources.params]
operator_source_param = "operator_value"

[[iv.profiles.sources]]
source_id = "operator_option_chain_source"
selector_fingerprint = "operator_option_chain_selector"
client_id = "operator_options_client"
source_kind = "option_chain"
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.selector]
selector_kind = "source_option_chain"
series_ids = ["operator.series_id"]
strike_range_policy = "operator_strike_range"

[iv.profiles.sources.selector.nt_params]
operator_nt_param = "operator_value"

[iv.profiles.sources.params]
operator_source_param = "operator_value"

[[iv.profiles.sources]]
source_id = "operator_aggregate_greeks_source"
selector_fingerprint = "operator_aggregate_greeks_selector"
client_id = "operator_options_client"
source_kind = "aggregate_greeks"
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "operator_aggregate_key"
underlying_selectors = ["operator.underlying_selector"]

[iv.profiles.sources.selector.nt_params]
operator_nt_param = "operator_value"

[iv.profiles.sources.params]
operator_source_param = "operator_value"

[[iv.profiles.sources]]
source_id = "operator_custom_implied_volatility_source"
selector_fingerprint = "operator_custom_implied_volatility_selector"
client_id = "operator_options_client"
source_kind = "custom_implied_volatility"
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.selector]
selector_kind = "source_custom_implied_volatility"
custom_iv_data_type = "operator_custom_iv_data_type"
custom_iv_data_fields = ["operator_field"]

[iv.profiles.sources.selector.nt_params]
operator_nt_param = "operator_value"

[iv.profiles.sources.params]
operator_source_param = "operator_value"
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
- Raw payload retrieval is audit/replay-only and not available through strategy query handles.
- Derived IV helper calls remain inside the IV engine and require explicit `IvHelperPolicy`/`IvDerivedInputSet` inputs through the engine API.
- No strategy owns IV subscription mechanics or NT helper-backed IV derivation.
- No concrete asset, venue, market, cadence, source ID, or instrument value is embedded in IV code.
- `Cargo.toml` remains the only source of truth for NT revision.
