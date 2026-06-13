# NT-Backed IV Engine Quickstart

This quickstart is a TOML shape, not a runtime fixture. Placeholder names and numeric values must be replaced by operator-owned values in the real config.

## Configure One IV Profile

An IV profile is the lifecycle boundary. Sources, strategy authorization, strategy-enabled products, audit-enabled raw products, freshness, retention, memory bounds, and query policies live together so changing a source does not require a second edit elsewhere.

```text
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "operator_iv_profile"
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
max_derived_points = operator_positive_integer
max_source_health_events = operator_positive_integer
max_source_event_age_ns = operator_positive_integer
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[iv.profiles.input_bounds]
finite_required = true
positive_required = true
inclusive_min = operator_min_iv
inclusive_max = operator_max_iv
unit = "unitless"

[iv.profiles.input_bounds.allowed_conventions]
allowed_conventions = ["operator_iv_convention"]

[iv.profiles.audit_policy]
enabled_raw_products = ["operator_raw_product_kind"]
authorized_audit_handles = ["operator_audit_handle"]
access_purposes = ["operator_access_purpose"]
eligible_sources = ["operator_source_id"]

[iv.profiles.audit_policy.audit_retention]
max_events = operator_positive_integer
max_age_ns = operator_positive_integer

[[iv.profiles.projection_policies]]
policy_id = "operator_projection_policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["operator_source_id"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = operator_positive_integer
max_projection_input_skew_ns = operator_positive_integer
fallback_policy_ref = "operator_fallback_policy"
interpolation_policy_ref = "operator_interpolation_policy"
quorum_policy_ref = "operator_quorum_policy"

[iv.profiles.projection_policies.output_bounds]
finite_required = true
positive_required = true
inclusive_min = operator_min_iv
inclusive_max = operator_max_iv
unit = "unitless"

[iv.profiles.projection_policies.output_bounds.allowed_conventions]
allowed_conventions = ["operator_iv_convention"]

[[iv.profiles.strategy_authorizations]]
strategy_id = "operator_strategy"
authorization_mode = "operator_authorization_mode"
allowed_product_kinds = ["operator_product_kind"]
allowed_selector_fingerprints = ["operator_selector_fingerprint"]
allowed_source_ids = ["operator_source_id"]

[[iv.profiles.sources]]
source_id = "operator_option_greeks_source"
selector_fingerprint = "operator_option_greeks_selector"
client_id = "operator_options_client"
source_kind = "option_greeks"
subscription_generation = operator_generation_integer
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "operator_pinned_nt_revision"
nt_evidence_path = "operator_nt_option_greeks_evidence_path"
nt_symbol = "OptionGreeks"

# Runtime provenance stamps the NT revision resolved from Cargo.lock.
# The TOML revision remains operator evidence and is not the runtime source of truth.

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
subscription_generation = operator_generation_integer
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "operator_pinned_nt_revision"
nt_evidence_path = "operator_nt_option_chain_evidence_path"
nt_symbol = "OptionChainSlice"

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
subscription_generation = operator_generation_integer
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "operator_pinned_nt_revision"
nt_evidence_path = "operator_nt_aggregate_greeks_evidence_path"
nt_symbol = "OptionGreekValues"

[iv.profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "operator_aggregate_key"
underlying_selectors = ["operator.underlying_selector"]
delta_field = "operator_delta_field"
gamma_field = "operator_gamma_field"
vega_field = "operator_vega_field"
theta_field = "operator_theta_field"
rho_field = "operator_rho_field"
iv_field = "operator_aggregate_iv_field"
iv_basis = "mark"
iv_convention = "operator_iv_convention"

[iv.profiles.sources.selector.nt_params]
operator_nt_param = "operator_value"

[iv.profiles.sources.params]
operator_source_param = "operator_value"

[[iv.profiles.sources]]
source_id = "operator_custom_implied_volatility_source"
selector_fingerprint = "operator_custom_implied_volatility_selector"
client_id = "operator_options_client"
source_kind = "custom_implied_volatility"
subscription_generation = operator_generation_integer
accepted_conventions = ["operator_convention"]

[iv.profiles.sources.nt_provenance]
nt_revision = "operator_pinned_nt_revision"
nt_evidence_path = "operator_nt_custom_data_evidence_path"
nt_symbol = "CustomData"

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
4. Engine ingests NT events, preserves raw payloads, including custom-data JSON for custom-data backed sources, indexes IV products, and records provenance.
5. Strategy registration gives authorized strategies an IV query handle for their configured profile.
6. Strategy queries IV products through the engine API with typed selectors.
7. Audit or replay modules can request raw payloads through the audit handle.
8. Engine returns either a product with provenance or a typed rejection.

## Operator Checks

- Every runtime value is in TOML.
- One IV profile owns the source lifecycle and strategy authorization.
- Strategy authorization is explicitly profile-wide or selector-scoped.
- Raw payload retrieval is audit/replay-only and not available through strategy query handles.
- Raw audit requests whose `as_of_ns` is before event receipt reject instead of underflowing age checks.
- Configured source conventions are enforced for NT option greeks and nested option-chain greeks.
- Option-greeks events without mark, bid, or ask IV preserve raw evidence but reject indexed products with typed source health.
- Custom-data backed aggregate greeks and custom IV evidence keep serialized custom-data JSON in the raw audit payload while exposing only typed products to strategies.
- Reload updates shared runtime source generations so old-generation events, old active source-health records, and removed profiles/sources cannot satisfy current queries through already-issued strategy handles, while new-generation events can.
- Derived IV helper calls remain inside the IV engine and require explicit `IvHelperPolicy`/`IvDerivedInputSet` inputs through the engine API.
- No strategy owns IV subscription mechanics or NT helper-backed IV derivation.
- No concrete asset, venue, market, cadence, source ID, or instrument value is embedded in IV code.
- `Cargo.toml` remains the only source of truth for NT revision.
