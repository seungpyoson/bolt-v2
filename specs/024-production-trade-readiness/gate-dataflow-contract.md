# T036H Gate Dataflow Contract

## Purpose

T036H replaces the current Chainlink-shaped `price_to_beat` path with a provider-neutral readiness gate path. The implementation is not allowed to start until this contract is represented in `tasks.md` as RED tests first.

The root problem is not only config validation. The current provider-specific value flows through config, archetype raw config, selected-market metadata, operator artifacts, provider collection, decision evidence, tiny-canary evidence, CLI commands, strategy registration, runtime strategy logic, and replay helpers.

## Current Hard Evidence

- `src/bolt_v3_config.rs:27-38` has `BoltV3RootConfig.clients` but no root `gate_providers`; `deny_unknown_fields` means `[gate_providers.*]` cannot parse today.
- `src/bolt_v3_market_families/updown.rs:72-83` has no `target.gate_subscriptions`; `deny_unknown_fields` means `[target.gate_subscriptions.*]` cannot parse today.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:94-113` deserializes `price_to_beat_source`, `price_to_beat_feed_id`, `price_to_beat_report_schema_version`, `price_to_beat_report_decimal_scale`, and `forced_flat_stale_chainlink_ms` as archetype runtime parameters.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:903-933` validates Chainlink feed id, schema version, and decimal scale inside the archetype path.
- `config/strategies/binary_oracle.example.toml:44-50` and `tests/fixtures/bolt_v3/strategies/binary_oracle.toml:44-50` store Chainlink-shaped fields under `[parameters.runtime]`.
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

## Canonical Schema

The contract below is normative for T036H. Field names in RED tests must match
these names unless the task records an explicit disposition before code changes.

### TOML Shape

Root gate providers:

```toml
[gate_providers.chainlink_btc_5m]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_price"]
client_id = "chainlink_mainnet"

[gate_providers.chainlink_btc_5m.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.chainlink_btc_5m.chainlink_data_streams]
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8
endpoint_id = "mainnet-data-streams"
ssm_credential_parameter = "/bolt/gate-providers/chainlink/mainnet"
```

Target subscriptions:

```toml
[target.gate_subscriptions.resolution_price]
required = true
allowed_provider_kinds = ["chainlink_data_streams", "pyth", "binance_index", "venue_native"]
provider_preference = ["chainlink_btc_5m"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution_price.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "btc-usd-5m"
provider_id = "chainlink_btc_5m"
```

No-resolution targets:

```toml
[target.gate_subscriptions.resolution_price]
required = true
allowed_provider_kinds = []
allow_no_resolution = true

[[target.gate_subscriptions.resolution_price.market_mappings]]
family_key = "venue-native-no-resolution"
market_class = "spot"
resolution_kind = "no_resolution"
resolution_identity = "none"
```

Rules:

- `provider_kind` is one of `chainlink_data_streams`, `pyth`, `binance_index`, `venue_native`, or `test_double`.
- `capabilities` contains one or more of `resolution_price`, `reference_price`.
- `target.gate_subscriptions.<role>` role is one of `resolution_price` or `reference_price`.
- A subscription may declare `allowed_provider_ids` or `allowed_provider_kinds`. It may not declare both unless `allowed_provider_ids` is a strict subset used to pin the provider list.
- If more than one provider/evidence item satisfies the same required role, the join must fail closed unless `provider_preference` gives a deterministic first matching provider id.
- Provider-specific fields are valid only under `[gate_providers.<id>.<provider_kind>]`.
- Strategy `[parameters.runtime]` may not contain provider ids, feed ids, report schema versions, decimal scales, provider endpoints, stale windows, or source strings.

### Gate Provider Config

Owner: `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, and provider validators under `src/bolt_v3_providers/`.

TOML owner: root `[gate_providers.<provider_id>]`.

Fields:

- `provider_kind`: provider implementation key from the allowed enum above.
- `capabilities`: gate evidence classes the provider can produce from the allowed enum above.
- `client_id`: optional link to an existing `[clients.<id>]` when the provider reuses a venue/data client.
- `freshness.max_age_ms`: maximum collector-observed age at session creation time.
- `freshness.max_clock_skew_ms`: maximum accepted absolute difference between provider/source timestamp and collector timestamp when the provider exposes a timestamp.
- Exactly one provider-specific subtable. Chainlink feed id, report schema version, decimal scale, endpoint, and SSM credential references live under this provider-specific subtable, not under strategy runtime parameters.

Fail closed:

- Missing provider config for a required target subscription.
- Provider-specific fields outside the matching provider subtable.
- Credentials or endpoints supplied outside SSM/TOML-owned provider config.
- Provider kind that does not support the required capability.

### Target Gate Subscription

Owner: target config validation in `src/bolt_v3_market_families/` plus root validation in `src/bolt_v3_validate.rs`.

TOML owner: strategy `[target.gate_subscriptions.<role>]`.

Fields:

- `role`: semantic gate role consumed by the archetype, such as `resolution_price` or `reference_price`.
- `required`: whether the role must be satisfied for this configured target.
- `allowed_provider_ids` or `allowed_provider_kinds`: set, not a single static id.
- `provider_preference`: optional deterministic provider id order when more than one provider satisfies the role.
- `allow_no_resolution`: only valid when the archetype and market class support no-resolution behavior.
- `market_mappings`: optional config-owned identity mappings used when venue metadata does not expose resolution identity. Each mapping has `family_key`, `market_class`, `resolution_kind`, `resolution_identity`, and optional `provider_id`.

Fail closed:

- Required role has no subscription.
- Subscription resolves to exactly one static provider when selected market requirements demand a different provider in a later rotation.
- `allow_no_resolution` is true for a strategy or market class that requires resolution evidence.
- Mapping is missing, ambiguous, or mismatched for the selected market identity.

### Archetype Gate Requirement

Owner: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`.

Fields:

- Required role names/classes only, exposed by the archetype as data equivalent to:

```rust
struct ArchetypeGateRequirement {
    role: GateRole,
    required: bool,
    allow_no_resolution: bool,
}
```

- No provider id, feed id, report schema version, decimal scale, endpoint, credential, or stale window.

Fail closed:

- Legacy provider-specific runtime fields appear under `[parameters.runtime]`.
- Archetype raw config still exposes a provider-specific `price_to_beat_source` string instead of receiving readiness-normalized evidence.

### Selected Market Requirement

Owner: `src/bolt_v3_market_families/mod.rs` and family modules such as `src/bolt_v3_market_families/updown.rs`.

Fields:

- `configured_target_id`
- `family_key`
- `market_id`
- `source_condition_id`
- `market_slug`
- `question_id`
- `market_class`
- `resolution_kind`
- `resolution_identity`
- `selected_market_key`: canonical key equal to `configured_target_id|family_key|market_id|source_condition_id|market_slug|question_id|resolution_kind|resolution_identity`
- `selected_at_ms`: collector timestamp captured when the market was selected

Canonical shape:

```rust
struct SelectedMarketRequirement {
    configured_target_id: String,
    family_key: String,
    market_id: String,
    source_condition_id: String,
    market_slug: String,
    question_id: String,
    market_class: MarketClass,
    resolution_kind: ResolutionKind,
    resolution_identity: String,
    selected_market_key: String,
    selected_at_ms: u64,
}
```

Fail closed:

- Selected market is missing required identity.
- Venue metadata lacks resolution identity and no config-owned mapping resolves it.
- Config mapping resolves to a provider identity that does not match the selected market.
- Evidence from a previous selected market is reused after rotation.

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
- `normalized_value`: role-specific normalized payload. For price roles this is decimal string plus scale.
- `provider_provenance`: provider-kind-specific provenance payload. For Chainlink this includes report hash, feed id, schema version, decimal scale, and report artifact hash. Pyth/Binance/venue-native providers must supply equivalent provenance fields for their source.
- `artifact_refs`: source artifact path/hash references.

Canonical shape:

```rust
struct GateEvidence {
    role: GateRole,
    provider_id: String,
    provider_kind: GateProviderKind,
    selected_market_key: String,
    collector_observed_at_ms: u64,
    source_observed_at_ms: u64,
    fresh_until_ms: u64,
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
3. If the selected market has `resolution_kind = "no_resolution"`, satisfy only when the archetype and target subscription both allow no-resolution. Otherwise no provider evidence is required.
4. Filter provider evidence by role, selected-market key, provider capability, provider id/kind subscription, provider provenance validity, and freshness where `collector_observed_at_ms <= created_at_ms <= fresh_until_ms`.
5. If no evidence remains, fail closed.
6. If multiple evidence items remain, select by `provider_preference`; if no deterministic first match exists, fail closed.
7. Build `session_hash` from the root config hash, strategy instance id, configured target id, selected-market key, selected-at timestamp, selected evidence artifact hashes, and normalized values.

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

Provider-specific collection commands may expose provider-specific arguments only under provider collector commands, for example `collect-gate-evidence --provider-id <id> --selected-market-requirement <path> --output <path>`. Generic entry-decision, final-packet, live-canary, and tiny-canary commands must reject `--price-report`, `--expected-price-report-sha256`, and `--price-to-beat-source`.

Strategy registration must receive an `EntryReadinessGateSession` or a path/hash pair that has already been verified into that session. It must not receive provider config fields directly.

Runtime replay must construct replay market state from `GateSatisfaction` values in the session. It must not write `market.price_to_beat` directly from `BinaryOracleEntryDecisionEvidenceSource` or any other raw provider-specific source.

Final-packet artifacts must bind the gate session by path and sha256 in the operator-evidence packet for each strategy instance with required roles.

## Boundary Plan

| Boundary | Current Source | Target Contract | RED Test Surface |
| --- | --- | --- | --- |
| Root config | `BoltV3RootConfig` has `clients` only | Add root `gate_providers` with provider-kind, capabilities, SSM-owned provider fields | `tests/config_parsing.rs` rejects provider fields outside `[gate_providers]` and accepts a valid provider block |
| Strategy target | raw `[target]` has market rotation fields only | Add `[target.gate_subscriptions.<role>]` provider set and no-resolution policy | `tests/config_parsing.rs` rejects missing required subscription and invalid no-resolution |
| Archetype params | binary oracle runtime owns Chainlink fields | Archetype declares roles/classes only | `tests/config_parsing.rs` rejects legacy fields in `[parameters.runtime]` |
| Example fixtures | shipped TOML keeps Chainlink runtime fields | Examples use gate providers/subscriptions | `tests/config_parsing.rs` proves shipped example and fixture load only with new schema |
| Market selection | selected identity lacks resolution metadata | selected market exposes/config-resolves market class and resolution identity | market-family tests reject missing/mismatched resolution identity |
| Provider evidence | Chainlink report path is hardwired into operator artifacts | provider-specific collectors produce normalized `GateEvidence` with canonical payload, provenance, timestamp, and artifact refs | `tests/bolt_v3_operator_artifacts.rs` rejects provider-kind mismatches and missing provider provenance |
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
2. RED archetype and fixture migration tests rejecting legacy Chainlink-shaped runtime fields.
3. RED selected-market requirement tests for market class, resolution kind, resolution identity, and config-owned mapping.
4. RED provider/evidence tests for role separation, selected-market binding, provider capability matching, and stale evidence.
5. RED registration/runtime/replay/final-packet tests proving no unchecked provider path exists and required gate sessions are artifact-bound.
6. RED decision-evidence, tiny-canary, live-canary, and CLI tests proving old `price_to_beat_source` string contracts cannot satisfy readiness.
7. GREEN implementation in the same boundary order.
8. Add Chainlink Data Streams as one provider implementation under the gate provider surface after the provider-neutral interfaces exist.
9. Add rotation/no-resolution final coverage proving Chainlink is not globally required.

## Non-Goals

- Do not add strategy alpha or EV checks.
- Do not run live/no-submit/tiny-canary operations as part of T036H.
- Do not add a second secret source.
- Do not keep compatibility shims that preserve provider-specific runtime fields in archetype config.
- Do not use a single static `strategy + target + role -> provider` mapping as the readiness proof.
