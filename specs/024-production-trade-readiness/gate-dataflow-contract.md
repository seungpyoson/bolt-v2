# T036H Gate Dataflow Contract

## Purpose

T036H replaces the current Chainlink-shaped `price_to_beat` path with a market, venue, and provider agnostic readiness gate path. The implementation is not allowed to start until this contract is represented in `tasks.md` as RED tests first.

The root problem is not only config validation. The current provider-specific value flows through config, archetype raw config, selected-market metadata, operator artifacts, provider collection, decision evidence, tiny-canary evidence, CLI commands, strategy registration, runtime strategy logic, and replay helpers.

## Current Hard Evidence

- `src/bolt_v3_config.rs:27-38` has `BoltV3RootConfig.clients` but no root `gate_providers`; `deny_unknown_fields` means `[gate_providers.*]` cannot parse today.
- `src/bolt_v3_market_families/updown.rs:72-83` has no `target.gate_subscriptions`; `deny_unknown_fields` means `[target.gate_subscriptions.*]` cannot parse today.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:94-113` deserializes `price_to_beat_source`, `price_to_beat_feed_id`, `price_to_beat_report_schema_version`, `price_to_beat_report_decimal_scale`, and `forced_flat_stale_chainlink_ms` as archetype runtime parameters.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:903-933` validates Chainlink feed id, schema version, and decimal scale inside the archetype path.
- `config/strategies/binary_oracle.toml:44-50` and `tests/fixtures/bolt_v3/strategies/binary_oracle.toml:44-50` store Chainlink-shaped fields under `[parameters.runtime]`.
- `src/bolt_v3_market_families/mod.rs:48-54`, `src/bolt_v3_market_families/mod.rs:179-186`, and `src/bolt_v3_market_families/mod.rs:277-288` keep the family dispatch provider-neutral but do not expose resolution requirement metadata in the selected-market result.
- `src/bolt_v3_operator_artifacts.rs:1545-1608` promotes decision evidence by matching `price_to_beat_source` against the financial envelope instead of a readiness gate session.
- `src/bolt_v3_operator_artifacts.rs:2407-2416`, `src/bolt_v3_operator_artifacts.rs:2450-2486`, and `src/bolt_v3_operator_artifacts.rs:2774-2911` materialize Chainlink report provenance through `ChainlinkDataStreamsReportSource`, `SourceBoundPriceToBeatSource`, and price-to-beat report binding.
- `src/bolt_v3_decision_evidence.rs:149-170` persists market identity plus `price_to_beat_source` and `price_to_beat_value`.
- `src/bolt_v3_tiny_canary_evidence.rs:157-165` and `src/bolt_v3_tiny_canary_evidence.rs:276-287` validate strategy input readiness by string equality against the approved `price_to_beat_source`.
- `src/bolt_v3_strategy_registration.rs:26-33` has no readiness-created gate session or normalized evidence in `StrategyRegistrationContext`.
- `src/strategies/binary_oracle_edge_taker.rs:73-115` includes raw `price_to_beat_source` in runtime config, `src/strategies/binary_oracle_edge_taker.rs:4082-4088` writes it into strategy input evidence, and `src/strategies/binary_oracle_edge_taker.rs:5400-5416` replay sets `market.price_to_beat` directly from source evidence.
- `src/main.rs:934-961` and `src/main.rs:980-1004` expose generic CLI paths for `price_to_beat_source`, `price_report`, and `expected_price_report_sha256`; `tests/bolt_v3_cli.rs:2385-2421` currently asserts those legacy generic flags exist.
- `src/bolt_v3_live_node.rs:1221-1228` registers strategies during build, while `src/bolt_v3_live_node.rs:751-759` checks the live canary gate later; this ordering requires registration/runtime to receive the readiness session instead of relying on the later canary gate.
- `src/bolt_v3_operator_artifacts.rs:8199-8408` final-packet replay re-derives market-selection evidence from decision evidence and instrument source; it must bind the gate session path/hash to avoid raw source replay bypass.

## Target Objects

This section defines the exact object and ownership contract that T036H RED tests
must pin before implementation starts.

## Canonical Schema

The contract below is normative for T036H. Field names in RED tests must match
these names unless the task records an explicit disposition before code changes.

### TOML Shape

Root gate providers are examples, not canonical runtime values. Provider ids are operator-owned labels.

```toml
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]
client_id = "chainlink_testnet"

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 10
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-reference-price"
value_kind = "price"
feed_id = "0x1111111111111111111111111111111111111111111111111111111111111111"
report_schema_version = 3
report_decimal_scale = 18

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-secondary-reference-price"
value_kind = "price"
feed_id = "0x2222222222222222222222222222222222222222222222222222222222222222"
report_schema_version = 3
report_decimal_scale = 18

[gate_providers.venue_metadata_primary]
provider_kind = "hyperliquid_hip4"
capabilities = ["market_metadata", "reference_value"]
client_id = "hyperliquid_mainnet"

[gate_providers.venue_metadata_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.venue_metadata_primary.hyperliquid_hip4]
metadata_scope = "asset_universe"
```

Target subscriptions:

```toml
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = ["chainlink_data_streams", "pyth", "exchange_index", "venue_native", "hyperliquid_hip4", "deribit_index", "outcome_oracle"]
allowed_value_kinds = ["price", "index", "outcome", "metadata"]
provider_preference = ["resolution_oracle_primary"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
provider_id = "resolution_oracle_primary"
```

No-resolution targets:

```toml
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = []
allow_no_resolution = true

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "venue-native-no-resolution"
market_class = "spot"
resolution_kind = "no_resolution"
resolution_identity = "none"
value_kind = "none"
```

Rules:

- `provider_kind` is registry-backed. The base contract must support known adapters such as `chainlink_data_streams`, `pyth`, `exchange_index`, `venue_native`, `hyperliquid_hip4`, `deribit_index`, `outcome_oracle`, and test-only `test_double` without making any one provider canonical.
- `capabilities` contains one or more semantic evidence classes such as `resolution_value`, `reference_value`, or `market_metadata`.
- `market_metadata` is a provider capability used to build or validate selected-market identity and `metadata_provenance_sha256`. It is not a `GateRole`, does not create a `[target.gate_subscriptions.market_metadata]` block, and does not satisfy entry readiness by itself unless a future archetype adds an explicit code-owned role.
- `target.gate_subscriptions.<role>` role is an archetype-declared semantic role such as `resolution` or `decision_reference`, not a provider or price-specific role.
- `value_kind` is the normalized value shape expected by the role, for example `price`, `index`, `outcome`, `metadata`, or `none`.
- Sports, politics, entertainment, crypto, and traditional market examples must enter through the same role/value-kind/provider-kind machinery. They do not get venue-specific strategy runtime fields.
- A subscription may declare `allowed_provider_ids`, `allowed_provider_kinds`, or both. When both are present, every listed provider id must exist and its provider kind must be in `allowed_provider_kinds`; the effective allowed set is the listed provider ids. It is invalid for `allowed_provider_ids` to name a provider whose kind is not in `allowed_provider_kinds`.
- If more than one provider/evidence item satisfies the same required role, the join must fail closed unless `provider_preference` gives a deterministic first matching provider id.
- Provider-specific fields are valid only under `[gate_providers.<id>.<provider_kind>]`.
- Strategy `[parameters.runtime]` may not contain provider ids, feed ids, report schema versions, decimal scales, provider endpoints, stale windows, or source strings.
- `test_double` is valid only in test fixtures and unit/integration test harnesses. Live/local operator TOML must reject `test_double` providers.

### Gate Provider Config

Owner: `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, and provider validators under `src/bolt_v3_providers/`.

TOML owner: root `[gate_providers.<provider_id>]`.

Fields:

- `provider_kind`: provider implementation key registered by the compiled provider adapter set.
- `capabilities`: semantic gate evidence classes the provider can produce.
- `client_id`: optional link to an existing `[clients.<id>]` when the provider reuses a venue/data client.
- `freshness.max_age_ms`: maximum collector-observed age at session creation time.
- `freshness.max_clock_skew_ms`: maximum accepted absolute difference between provider/source timestamp and collector timestamp when the provider exposes a timestamp.
- Exactly one provider-specific subtable. Feed ids, venue metadata scopes, schema versions, decimal scales, endpoints, and SSM credential references live under this provider-specific subtable, not under strategy runtime parameters.

Fail closed:

- Missing provider config for a required target subscription.
- Provider-specific fields outside the matching provider subtable.
- Credentials or endpoints supplied outside SSM/TOML-owned provider config.
- Provider kind that is unregistered or does not support the required capability/value kind.

### Target Gate Subscription

Owner: target config validation in `src/bolt_v3_market_families/` plus root validation in `src/bolt_v3_validate.rs`.

TOML owner: strategy `[target.gate_subscriptions.<role>]`.

Fields:

- `role`: semantic gate role consumed by the archetype, such as `resolution` or `decision_reference`.
- `required`: whether the role must be satisfied for this configured target.
- `allowed_provider_ids` or `allowed_provider_kinds`: set, not a single static id.
- `allowed_value_kinds`: optional set of normalized value kinds accepted by this subscription.
- `provider_preference`: optional deterministic provider id order when more than one provider satisfies the role.
- `allow_no_resolution`: only valid when the archetype and market class support no-resolution behavior.
- `market_mappings`: optional config-owned identity mappings used when venue metadata does not expose resolution identity. Each mapping has `family_key`, `market_class`, `resolution_kind`, `resolution_identity`, `value_kind`, and optional `provider_id`.

Fail closed:

- Required role has no subscription.
- Subscription role is a provider capability such as `market_metadata` rather than a code-owned `GateRole`.
- Subscription resolves to exactly one static provider when selected market requirements demand a different provider in a later rotation.
- `allow_no_resolution` is true for a strategy or market class that requires resolution evidence.
- Mapping is missing, ambiguous, or mismatched for the selected market identity, provider kind, or value kind.

### Archetype Gate Requirement

Owner: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`.

Fields:

- Required role names/classes only, exposed by the archetype as data equivalent to:

```rust
struct ArchetypeGateRequirement {
    role: GateRole,
    required: bool,
    accepted_value_kinds: BTreeSet<ValueKind>,
    allow_no_resolution: bool,
}
```

- No provider id, feed id, report schema version, decimal scale, endpoint, credential, or stale window.

Positive exposure mechanism:

- The binary-oracle archetype exposes static code-owned requirements through a function equivalent to `binary_oracle_edge_taker::gate_requirements() -> Vec<ArchetypeGateRequirement>`.
- The strategy TOML does not declare these archetype requirements. TOML selects the target subscription that may satisfy them.
- For binary oracle entry readiness, the positive declaration is:

```rust
vec![ArchetypeGateRequirement {
    role: GateRole::Resolution,
    required: true,
    accepted_value_kinds: BTreeSet::from([ValueKind::Price, ValueKind::Outcome]),
    allow_no_resolution: false,
}]
```

- A no-resolution-compatible archetype may return `allow_no_resolution: true`; a strategy with no resolution gate returns no `resolution` requirement.

Fail closed:

- Legacy provider-specific runtime fields appear under `[parameters.runtime]`.
- Archetype raw config still exposes a provider-specific `price_to_beat_source` string instead of receiving readiness-normalized evidence.

### Selected Market Requirement

Owner: `src/bolt_v3_market_families/mod.rs` and family modules such as `src/bolt_v3_market_families/updown.rs`.

Fields:

- `configured_target_id`
- `venue`
- `family_key`
- `market_id`
- `instrument_ids`: the market-complete, lexicographically sorted set of venue instrument/outcome ids that define the selected market. Strategy-specific traded subsets must be represented outside selected-market identity.
- `market_class`
- `resolution_kind`
- `resolution_identity`
- `value_kind`
- `metadata_provenance_sha256`: hash of source metadata used to build the selected-market identity.
- `selected_market_key`: canonical key derived from the normalized selected-market identity, not from venue-specific Polymarket-only fields.
- `selected_at_ms`: collector timestamp captured when the market was selected

Selected-market key canonicalization:

- `selected_market_key` is `hex(sha256(<canonical selected-market identity JSON bytes>))`.
- Canonical selected-market identity JSON uses the same UTF-8 canonical JSON rules as `session_hash`: sorted object keys, arrays in declared order, and no insignificant whitespace.
- The hash input object contains `configured_target_id`, `venue`, `family_key`, `market_id`, sorted `instrument_ids`, `market_class`, `resolution_kind`, `resolution_identity`, `value_kind`, and `metadata_provenance_sha256`.
- `selected_at_ms` is intentionally excluded from `selected_market_key`; it belongs in the gate session hash so the same market identity can be selected at different times without becoming a different market.
- `metadata_provenance_sha256` is `hex(sha256(<canonical market metadata provenance JSON bytes>))`. For venue-native/HIP-4/Deribit/outcome-oracle metadata, the provenance JSON must identify provider kind, venue or source family, source artifact hash, and any source-native identity scope used to derive the selected-market identity.

Canonical shape:

```rust
struct SelectedMarketRequirement {
    configured_target_id: String,
    venue: String,
    family_key: String,
    market_id: String,
    instrument_ids: Vec<String>,
    market_class: MarketClass,
    resolution_kind: ResolutionKind,
    resolution_identity: String,
    value_kind: ValueKind,
    metadata_provenance_sha256: String,
    selected_market_key: String,
    selected_at_ms: u64,
}
```

Fail closed:

- Selected market is missing required identity.
- `instrument_ids` is only the strategy-traded subset when the market has a larger complete instrument/outcome set.
- `selected_market_key` is not derived from the canonical selected-market identity JSON algorithm above.
- Venue metadata lacks resolution identity and no config-owned mapping resolves it.
- Config mapping resolves to a provider identity that does not match the selected market.
- Selected-market identity requires a venue-specific field not represented in the normalized identity/provenance payload.
- Evidence from a previous selected market is reused after rotation.
- Any selected-market key component contains `|`.

### Gate Evidence

Owner: provider-specific collectors under `src/bolt_v3_providers/` and normalization/verification under `src/bolt_v3_operator_artifacts.rs`.

Fields:

- `role`
- `provider_id`
- `provider_kind`
- `selected_market_identity`
- `collector_observed_at_ms`: host/operator-artifact timestamp captured after a successful provider fetch.
- `source_observed_at_ms`: provider/source timestamp when available; otherwise equal to `collector_observed_at_ms`.
- `fresh_until_ms`: `collector_observed_at_ms + freshness.max_age_ms`.
- `value_kind`: normalized value shape, such as price, index, outcome, metadata, or none.
- `normalized_value`: role-specific normalized payload. Price/index values use decimal string plus scale; outcome/metadata values use canonical JSON with hashes.
- `provider_provenance`: provider-kind-specific provenance payload. Chainlink, Pyth, exchange index, HIP-4/venue-native, Deribit/index, outcome-oracle, and future providers must supply equivalent provenance for their source.
- `artifact_refs`: source artifact path/hash references.

Canonical shape:

```rust
enum GateRole {
    Resolution,
    DecisionReference,
}

struct GateProviderKind(String);

enum MarketClass {
    BinaryOption,
    CategoricalOutcome,
    ScalarOutcome,
    OneTouch,
    Spot,
    Perpetual,
    Option,
}

struct ResolutionKind(String);

enum ValueKind {
    Price,
    Index,
    Outcome,
    Metadata,
    None,
}

enum GateValue {
    Decimal { value: String, scale: u32 },
    Json { canonical_json_sha256: String },
    None,
}

struct ArtifactRef {
    path: String,
    sha256: String,
}

struct ProviderProvenance {
    provider_kind: GateProviderKind,
    canonical_json_sha256: String,
}

struct GateEvidence {
    role: GateRole,
    provider_id: String,
    provider_kind: GateProviderKind,
    selected_market_key: String,
    collector_observed_at_ms: u64,
    source_observed_at_ms: u64,
    fresh_until_ms: u64,
    value_kind: ValueKind,
    normalized_value: GateValue,
    provider_provenance: ProviderProvenance,
    artifact_refs: Vec<ArtifactRef>,
}
```

Fail closed:

- Evidence role does not equal required role.
- Evidence provider does not satisfy selected market requirement and target subscription.
- Evidence selected-market identity does not equal the active selected market.
- Evidence is stale or timestamp-invalid.
- Provider provenance is missing, malformed, or provider-kind-incompatible.
- Reference evidence is used for resolution, or resolution evidence is used for reference.
- Price/index evidence is used for outcome/metadata requirements, or outcome/metadata evidence is used for numeric price/index requirements.
- Provider collection timeout, partial response, or API error produces no evidence and therefore fails closed. It must not synthesize default evidence.
- `source_observed_at_ms` differs from `collector_observed_at_ms` by more than `freshness.max_clock_skew_ms` when the source timestamp is available.

### Entry Readiness Gate Session

Owner: `src/bolt_v3_operator_artifacts.rs`, `src/bolt_v3_strategy_registration.rs`, and `src/strategies/binary_oracle_edge_taker.rs`.

Fields:

- `strategy_instance_id`
- `configured_target_id`
- selected market requirement
- satisfied role evidence map
- session hash or opaque id derived from config, selected market identity, provider evidence, and artifact hashes
- normalized strategy input fields consumed by runtime and final packet code
- `created_at_ms`: operator-artifact timestamp used as the staleness comparison clock.

Canonical shape:

```rust
struct EntryReadinessGateSession {
    strategy_instance_id: String,
    configured_target_id: String,
    selected_market: SelectedMarketRequirement,
    created_at_ms: u64,
    satisfied_roles: BTreeMap<GateRole, GateSatisfaction>,
    session_hash: String,
    artifact_refs: Vec<ArtifactRef>,
}

enum GateSatisfaction {
    Evidence(GateEvidence),
    NoResolution {
        selected_market_key: String,
        resolution_identity: String,
    },
}
```

Join algorithm:

1. For each required `ArchetypeGateRequirement`, load the matching `target.gate_subscriptions.<role>`.
2. Verify the selected market's `market_class`, `resolution_kind`, and `resolution_identity` are observed from venue metadata or resolved by exactly one `market_mappings` entry.
3. If the selected market has `resolution_kind = "no_resolution"` and the archetype plus target subscription both allow no-resolution, satisfy the role with `GateSatisfaction::NoResolution`. If either side does not allow no-resolution, fail closed. No provider evidence is required only for this explicit no-resolution satisfaction case.
4. Filter provider evidence by role, selected-market key, provider capability, provider id/kind subscription, provider provenance validity, and freshness where `collector_observed_at_ms <= created_at_ms <= fresh_until_ms`.
5. If no evidence remains, fail closed.
6. If multiple evidence items remain, select by `provider_preference`; if no deterministic first match exists, fail closed.
7. Build `session_hash` from the root config hash, strategy instance id, configured target id, selected-market key, selected-at timestamp, selected evidence artifact hashes, and normalized values.

Session hash canonicalization:

- `session_hash` is lowercase hex SHA-256 over UTF-8 canonical JSON.
- Canonical JSON uses lexicographically sorted object keys, arrays in the declared order below, decimal numbers rendered as strings when they are runtime values, and no insignificant whitespace.
- The hash input object is:

```json
{
  "schema_version": 1,
  "strategy_instance_id": "<strategy_instance_id>",
  "configured_target_id": "<configured_target_id>",
  "root_config_sha256": "<sha256>",
  "selected_market_key": "<selected_market_key>",
  "selected_at_ms": "<selected_at_ms>",
  "created_at_ms": "<created_at_ms>",
  "satisfied_roles": [
    {
      "role": "resolution",
      "satisfaction_kind": "evidence",
      "provider_id": "<provider_id>",
      "provider_kind": "<provider_kind>",
      "value_kind": "<value_kind>",
      "normalized_value_sha256": "<sha256>",
      "artifact_sha256s": ["<sha256>"],
      "provider_provenance_sha256": "<sha256>"
    },
    {
      "role": "resolution",
      "satisfaction_kind": "no_resolution",
      "resolution_identity": "none",
      "selected_market_key": "<selected_market_key>"
    }
  ]
}
```

- `satisfied_roles` is sorted by role name. `artifact_sha256s` is sorted by artifact path before hashing. `provider_provenance_sha256` is SHA-256 of the canonical JSON representation of the provider provenance payload.
- A session containing `GateSatisfaction::NoResolution` must include the no-resolution object above so no-resolution sessions cannot collide with evidence-backed sessions or with each other across markets.

Canonical provider provenance JSON:

- Every provenance object uses a flat tagged JSON object with `provider_kind` as the discriminator.
- Object keys are sorted lexicographically under the same canonical JSON rule as `session_hash`.
- Numeric schema/scale fields are JSON numbers; hashes and identifiers are strings.
- The exact provider provenance examples are:

```json
{"feed_id":"<feed_id>","provider_kind":"chainlink_data_streams","report_decimal_scale":18,"report_schema_version":3,"report_sha256":"<sha256>"}
{"feed_id":"<feed_id>","price_message_sha256":"<sha256>","provider_kind":"pyth"}
{"provider_kind":"exchange_index","response_sha256":"<sha256>","symbol":"<symbol>"}
{"asset_id":"<asset_id>","metadata_sha256":"<sha256>","provider_kind":"hyperliquid_hip4"}
{"instrument_name":"<instrument_name>","provider_kind":"deribit_index","response_sha256":"<sha256>"}
{"event_id":"<event_id>","outcome_id":"<outcome_id>","provider_kind":"outcome_oracle","source_sha256":"<sha256>"}
{"provider_kind":"venue_native","source_sha256":"<sha256>","venue":"<venue>"}
{"fixture_sha256":"<sha256>","provider_kind":"test_double"}
```

- `provider_provenance_sha256` is `hex(sha256(<canonical provider provenance JSON bytes>))`.

Fail closed:

- Strategy registration receives no readiness-created session for a required role.
- Runtime strategy constructs provider evidence directly from raw config.
- Source replay directly sets source-bound values without a readiness-created normalized evidence object.
- Decision evidence or tiny-canary evidence accepts a provider-specific string comparison instead of the session/normalized evidence identity.
- Final-packet or operator-evidence artifacts omit the gate session path/hash for a strategy instance that has required roles.

### Consumer Contracts

Decision evidence must store `gate_session_hash`, `selected_market_key`, and per-role normalized evidence identity. It must not store `price_to_beat_source` as the readiness proof.

Tiny-canary and live-canary gates must accept `gate_session_path` and `expected_gate_session_sha256`, load the session, and validate selected-market key plus required roles before the canary can pass. They must reject source-string equality as insufficient evidence.

Generic CLI artifact commands must accept provider-neutral gate session arguments:

- `--gate-session <path>`
- `--expected-gate-session-sha256 <sha256>`

Provider-specific collection commands may expose provider-specific arguments only under provider collector commands, for example `collect-gate-evidence --provider-id <id> --selected-market-requirement <path> --output <path>`. Generic entry-decision, final-packet, live-canary, and tiny-canary commands must reject `--price-report`, `--expected-price-report-sha256`, and `--price-to-beat-source`. Provider collector commands must positively accept provider-specific inputs only after binding them to a configured `provider_id` and selected-market requirement.

Strategy registration must receive an `EntryReadinessGateSession` or a path/hash pair that has already been verified into that session. It must not receive provider config fields directly.

Runtime replay must construct replay market state from `GateSatisfaction` values in the session. It must not write `market.price_to_beat` directly from `BinaryOracleEntryDecisionEvidenceSource` or any other raw provider-specific source.

Final-packet artifacts must bind the gate session by path and sha256 in the operator-evidence packet for each strategy instance with required roles.

## Boundary Plan

| Boundary | Current Source | Target Contract | RED Test Surface |
| --- | --- | --- | --- |
| Root config | `BoltV3RootConfig` has `clients` only | Add root `gate_providers` with registry-backed provider kind, capabilities, value kinds, freshness, and SSM-owned provider fields | `tests/config_parsing.rs` rejects provider fields outside `[gate_providers]` and accepts valid provider blocks for Chainlink and HIP-4/venue-native examples |
| Strategy target | raw `[target]` has market rotation fields only | Add `[target.gate_subscriptions.<role>]` provider set and no-resolution policy | `tests/config_parsing.rs` rejects missing required subscription and invalid no-resolution |
| Archetype params | binary oracle runtime owns Chainlink fields | Archetype declares roles/classes only | `tests/config_parsing.rs` rejects legacy fields in `[parameters.runtime]` |
| Example fixtures | shipped TOML keeps Chainlink runtime fields | Examples use gate providers/subscriptions | `tests/config_parsing.rs` proves shipped example and fixture load only with new schema |
| Market selection | selected identity lacks resolution metadata and still carries venue-shaped fields | selected market exposes/config-resolves generic market identity, market class, resolution identity, value kind, and metadata provenance | market-family tests reject missing/mismatched resolution identity or venue-specific identity leakage |
| Provider evidence | Chainlink report path is hardwired into operator artifacts | provider-specific collectors produce normalized `GateEvidence` with canonical payload, value kind, provenance, timestamp, and artifact refs | `tests/bolt_v3_operator_artifacts.rs` rejects provider-kind/value-kind mismatches and missing provider provenance |
| Entry readiness join | execution-client provider dispatch selects source-input collector | join archetype role, target subscription, selected market requirement, provider capability, evidence | `tests/bolt_v3_operator_artifacts.rs` rejects static-provider mismatch after market rotation |
| Decision evidence | `BoltV3StrategyInputEvidenceSnapshot` stores `price_to_beat_source` | evidence stores readiness session/normalized evidence identity | `tests/bolt_v3_operator_artifacts.rs` rejects decision evidence without matching gate session |
| Tiny-canary evidence | safety audit compares source string to expected source string | safety audit validates session/normalized evidence identity and selected-market binding | `tests/bolt_v3_tiny_canary_preconditions.rs` rejects stale/cross-market session and wrong role |
| Live-canary gate | live canary gate can validate source-shaped readiness inputs | live canary gate validates gate session path/hash and selected-market binding | `tests/bolt_v3_live_canary_gate.rs` rejects source-string-only readiness |
| CLI | commands expose `price_report`, `price_to_beat_source`, Chainlink hash names | CLI accepts `--gate-session` and `--expected-gate-session-sha256`; Chainlink names move under provider-specific collectors | `tests/bolt_v3_cli.rs` rejects legacy Chainlink-shaped generic entry-decision flags and accepts provider-neutral session flags |
| Strategy registration | context has no gate session | registration requires readiness-created session/evidence object for required roles | `tests/bolt_v3_strategy_registration.rs` rejects registration without required gate session |
| Runtime strategy and replay | raw config has `price_to_beat_source`; replay sets `market.price_to_beat` directly | runtime and replay consume normalized readiness evidence only | strategy and replay tests reject unchecked provider path |
| Final-packet binding | packet can bind decision evidence without a gate-session artifact hash | packet binds gate session path/hash per strategy instance | operator-artifact tests reject missing session binding |

## Implementation Order

1. RED config/schema tests for `gate_providers` and `target.gate_subscriptions`.
2. RED archetype and fixture migration tests rejecting legacy Chainlink-shaped runtime fields and hardcoded data-client/instrument references.
3. RED selected-market requirement tests for generic market identity, market class, resolution kind, resolution identity, value kind, metadata provenance, and config-owned mapping.
4. RED provider/evidence tests for role separation, selected-market binding, provider capability/value-kind matching, and stale evidence.
5. RED registration/runtime/replay/final-packet tests proving no unchecked provider path exists and required gate sessions are artifact-bound.
6. RED decision-evidence, tiny-canary, live-canary, and CLI tests proving old `price_to_beat_source` string contracts cannot satisfy readiness.
7. GREEN implementation in the same boundary order:
   - T036H13 config-only schema and validation: root gate providers, target subscriptions, registry-backed provider kinds/capabilities/value kinds, freshness, SSM-owned provider fields, provider-specific subtable ownership, `test_double` live rejection, and explicit old-schema migration errors.
   - T036H14 archetype refactor: provider-neutral `gate_requirements()` plus example/fixture migration, with no compatibility shim.
   - T036H15 selected-market identity: generic requirement metadata, market-complete sorted instrument/outcome ids, metadata provenance, and canonical `selected_market_key`.
   - T036H16 evidence and join: `GateEvidence`, `GateSatisfaction`, `EntryReadinessGateSession`, role/value-kind separation, selected-market binding, freshness/clock-skew checks, deterministic `provider_preference`, no-resolution satisfaction, and canonical `session_hash`.
   - T036H17 consumers: decision evidence, tiny canary, live canary, CLI, registration, runtime, replay, and final-packet binding consume the gate session instead of provider-specific strings.
8. Add thin provider readiness collection functions under the gate provider surface after the provider-neutral interfaces exist; initial coverage must include Chainlink Data Streams plus existing NT Hyperliquid HIP-4/venue-native metadata, without rebuilding upstream adapters, adding a trait/plugin provider framework, or making either provider canonical.
9. Add rotation/no-resolution final coverage proving no provider or venue is globally required.

## Non-Goals

- Do not add strategy alpha or EV checks.
- Do not run live/no-submit/tiny-canary operations as part of T036H.
- Do not add a second secret source.
- Do not keep compatibility shims that preserve provider-specific runtime fields in archetype config.
- Do not use a single static `strategy + target + role -> provider` mapping as the readiness proof.
