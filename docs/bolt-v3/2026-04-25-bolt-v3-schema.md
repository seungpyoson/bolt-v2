# bolt-v3 Schema Specification

Status: draft for architecture review

This document defines the current candidate TOML schemas for live trading.

Rules:

- every runtime value must be explicit in TOML
- no mixins
- no inheritance
- no second config path
- root and strategy schema versions are independent
- unknown fields fail

This document defines:

- root/entity TOML schema
- strategy TOML schema
- field ownership
- field semantics
- required validation behavior

## 1. Schema Version Policy

- `schema_version` in the root file versions the root file schema only
- `schema_version` in the strategy file versions the strategy file schema only

The versions are independent.

Changing one file schema does not automatically imply changing the other.

## 2. Root File: Ownership

The root file owns:

- canonical trader identity
- runtime mode
- Nautilus node/runtime settings
- entity-level risk settings
- logging configuration
- persistence paths
- keyed venue definitions
- venue secret references
- explicit strategy file list

The root file does not own:

- strategy target choice
- strategy retry/block timing for rotating-market selection
- strategy pricing thresholds
- strategy order parameters
- strategy-specific sizing policy

## 3. Strategy File: Ownership

The strategy file owns:

- strategy instance identity
- strategy archetype
- venue reference
- target definition
- target retry/block timing
- optional reference data declarations
- strategy-specific parameters
- archetype-specific order parameters

The strategy file does not own:

- venue client construction
- venue credentials
- process-wide logging settings
- process-wide state paths
- process-wide Nautilus runtime settings

## 4. Root File: Candidate Schema

This is a structural example, not a default configuration.
Values such as paths, SSM parameter names, account identifiers, wallet addresses, and venue keys must be operator-owned TOML values in a real deploy.

```toml
schema_version = 1
trader_identifier = "BOLT-001"

strategy_files = [
  "strategies/bitcoin_updown_main.toml",
]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
catalog_directory = "/var/lib/bolt/catalog"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
ws_timeout_seconds = 30
subscribe_new_markets = false
update_instruments_interval_minutes = 60
websocket_max_subscriptions_per_connection = 200

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
max_retries = 3
retry_delay_initial_milliseconds = 250
retry_delay_max_milliseconds = 2000
ack_timeout_seconds = 5

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"

[venues.binance_reference]
kind = "binance"

[venues.binance_reference.data]
product_types = ["spot"]
environment = "mainnet"
instrument_status_poll_seconds = 3600

[venues.binance_reference.secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
```

## 5. Root File: Field Semantics

### Top level

#### `schema_version`

- type: integer
- required: yes
- meaning: version of the root-file schema only

#### `trader_identifier`

- type: string
- required: yes
- canonical identity for:
  - Nautilus `TraderId`
  - keyed execution-client `trader_id` fields which require it
  - state namespace
  - runtime identity in forensic events
- current live-trading rule:
  - Nautilus node name is set equal to this value

#### `strategy_files`

- type: array of relative file paths
- required: yes
- each listed file must:
  - exist
  - parse as a strategy schema
  - not duplicate another listed path
- relative paths are resolved relative to the root file's parent directory
- no globbing
- no auto-discovery

### `[runtime]`

#### `mode`

- type: string enum
- required: yes
- current allowed value for live trading:
  - `live`
- any other value fails validation

### `[nautilus]`

#### `load_state`

- type: boolean
- required: yes
- maps to Nautilus live-node state loading

#### `save_state`

- type: boolean
- required: yes
- maps to Nautilus live-node state saving

#### `timeout_connection_seconds`

- type: positive integer
- required: yes

#### `timeout_reconciliation_seconds`

- type: positive integer
- required: yes

#### `reconciliation_lookback_mins`

- type: non-negative integer
- required: yes
- `0` means unbounded lookback and maps to Nautilus `None`
- any positive value maps to that exact bounded minute count

#### `timeout_portfolio_seconds`

- type: positive integer
- required: yes

#### `timeout_disconnection_seconds`

- type: positive integer
- required: yes

#### `delay_post_stop_seconds`

- type: non-negative integer
- required: yes
- maps to Nautilus `LiveNodeConfig.delay_post_stop`
- note: Nautilus builder helper naming uses `with_delay_post_stop_secs`, but the config field itself is `delay_post_stop`

#### `timeout_shutdown_seconds`

- type: positive integer
- required: yes
- maps to Nautilus live-node shutdown timeout, not a custom bolt concept
- exact mapping target: Nautilus `LiveNodeConfig.timeout_shutdown`
- note: Nautilus builder helper naming uses `with_delay_shutdown_secs`, but the config field itself is `timeout_shutdown`

### `[risk]`

This section exists because strategy-local limits are not enough by themselves.

#### `bypass`

- type: boolean
- required: yes
- must be explicit
- current expectation:
  - `false`

#### `max_order_submit_count`

- type: positive integer
- required: yes
- pairs with `max_order_submit_interval_seconds` to build Nautilus `RiskEngineConfig.max_order_submit = RateLimit(count, interval)`

#### `max_order_submit_interval_seconds`

- type: positive integer
- required: yes
- pairs with `max_order_submit_count` to build Nautilus `RiskEngineConfig.max_order_submit = RateLimit(count, interval)`

#### `max_order_modify_count`

- type: positive integer
- required: yes
- pairs with `max_order_modify_interval_seconds` to build Nautilus `RiskEngineConfig.max_order_modify = RateLimit(count, interval)`

#### `max_order_modify_interval_seconds`

- type: positive integer
- required: yes
- pairs with `max_order_modify_count` to build Nautilus `RiskEngineConfig.max_order_modify = RateLimit(count, interval)`

#### `default_max_notional_per_order`

- type: decimal string
- required: yes
- root-level entity per-order notional cap
- runtime synchronization to NautilusTrader per-instrument `max_notional_per_order` maps is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 4

### `[logging]`

#### `standard_output_level`

- type: string enum
- required: yes
- allowed values:
  - `TRACE`
  - `DEBUG`
  - `INFO`
  - `WARN`
  - `ERROR`
  - `OFF`

#### `file_level`

- type: string enum
- required: yes
- allowed values:
  - `TRACE`
  - `DEBUG`
  - `INFO`
  - `WARN`
  - `ERROR`
  - `OFF`

#### `log_directory`

- type: absolute path string
- required: yes

### `[persistence]`

#### `state_directory`

- type: absolute path string
- required: yes

#### `catalog_directory`

- type: absolute path string
- required: yes
- local Nautilus catalog root for structured decision events and raw NautilusTrader capture
- persistence behavior and local-evidence requirements are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 9.6, 9.7, and 10

### `[persistence.streaming]`

This section carries the current local catalog writer settings.
It is required in the current live-trading scope.
These settings apply to the single local persistence path for both structured decision events and raw NautilusTrader capture.
The schema does not expose a separate raw-capture backend, rotation policy, or writer path.

#### `catalog_fs_protocol`

- type: string enum
- required: yes
- current allowed value:
  - `file`

#### `flush_interval_milliseconds`

- type: positive integer
- required: yes
- controls the current catalog flush cadence for structured decision events and raw NautilusTrader capture

#### `replace_existing`

- type: boolean
- required: yes
- controls whether existing catalog evidence files may be replaced

#### `rotation_kind`

- type: string enum
- required: yes
- current allowed value:
  - `none`
- maps to the local catalog writer no-rotation behavior

### `[aws]`

#### `region`

- type: string
- required: yes
- used by the Rust Amazon Web Services Systems Manager client
- no implicit region fallback

### `[venues.<identifier>]`

#### Venue key

- type: keyed identifier
- required: yes for every configured venue
- examples:
  - `polymarket_main`
  - `binance_reference`

The key is a configuration reference name.
It is not the trader identifier.

#### `kind`

- type: string enum
- required: yes
- current allowed values:
  - `polymarket`
  - `binance`

### `[venues.<identifier>.data]`

Presence of `[data]` means a data client is configured.

#### Common rule

- any field here is owned by venue-client construction, not by strategies

#### Polymarket data fields

##### `base_url_http`

- type: string
- required: yes

##### `base_url_ws`

- type: string
- required: yes

##### `base_url_gamma`

- type: string
- required: yes

##### `base_url_data_api`

- type: string
- required: yes

##### `http_timeout_seconds`

- type: positive integer
- required: yes

##### `ws_timeout_seconds`

- type: positive integer
- required: yes

##### `subscribe_new_markets`

- type: boolean
- required: yes

##### `update_instruments_interval_minutes`

- type: positive integer
- required: yes
- background Polymarket adapter refresh interval only
- not the sole mechanism keeping current rotating-market data loaded

##### `websocket_max_subscriptions_per_connection`

- type: positive integer
- required: yes

No other Polymarket data-client fields are exposed in the current schema unless they are confirmed on the pinned NautilusTrader Rust adapter surface.

For current reference-data venues other than Polymarket, each venue kind defines its own allowed `[data]` field set.
Unknown fields fail validation against the venue-kind-specific set in Section 8.

### `[venues.<identifier>.execution]`

Presence of `[execution]` means an execution client is configured.

#### `account_id`

- type: string
- required: yes for execution-capable venues

Meaning:

- explicit account identity bolt uses when submitting and querying through NautilusTrader
- required so bolt does not depend on hidden account-id derivation inside an adapter

#### `signature_type`

- type: string enum
- required: yes for Polymarket execution
- allowed values:
  - `eoa`
  - `poly_proxy`
  - `poly_gnosis_safe`

bolt parses this string enum and maps it to the current pinned Nautilus/Polymarket integer enum required by the adapter.

#### `funder_address`

- type: string
- required: yes for Polymarket execution when the signature path requires it
- this is a public address, not a secret value
- it lives in the root venue execution config, not in `[secrets]`
- zero address is invalid when the selected signature path requires a real funder wallet

#### `max_retries`

- type: positive integer
- required: yes

#### `retry_delay_initial_milliseconds`

- type: positive integer
- required: yes

#### `retry_delay_max_milliseconds`

- type: positive integer
- required: yes

#### `ack_timeout_seconds`

- type: positive integer
- required: yes
- maps directly to the pinned Polymarket execution-client acknowledgment timeout field

#### Additional Polymarket execution fields

The current schema also requires these pinned adapter fields to be explicit:

- `base_url_http`
- `base_url_ws`
- `base_url_data_api`
- `http_timeout_seconds`

### `[venues.<identifier>.secrets]`

Presence of `[secrets]` means the venue requires credential resolution.

For Polymarket:

- `private_key_ssm_path`
- `api_key_ssm_path`
- `api_secret_ssm_path`
- `passphrase_ssm_path`

All are:

- type: string
- required: yes for Polymarket execution

No environment-variable fallback is allowed.

For current Binance reference-data use:

- `api_key_ssm_path` and `api_secret_ssm_path` are required
- the expected credential type is Ed25519, matching the pinned Binance data-client requirement for SBE WebSocket streams

#### Binance data fields

##### `product_types`

- type: array of string enums
- required: yes
- current allowed value:
  - `spot`
- maps to Nautilus `BinanceDataClientConfig.product_types`

##### `environment`

- type: string enum
- required: yes
- current allowed value:
  - `mainnet`
- maps to Nautilus `BinanceDataClientConfig.environment`

##### `instrument_status_poll_seconds`

- type: non-negative integer
- required: yes
- maps to Nautilus `BinanceDataClientConfig.instrument_status_poll_secs`
- `0` means disabled

## 6. Strategy File: Candidate Schema

```toml
schema_version = 1
strategy_instance_identifier = "bitcoin_updown_main"
strategy_archetype = "binary_oracle_edge_taker"
order_id_tag = "001"
oms_type = "netting"
venue = "polymarket_main"

[target]
configured_target_id = "btc_updown_5m"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "BTC"
cadence_seconds = 300
market_selection_rule = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60

[reference_data.primary]
venue = "binance_reference"
instrument_id = "BTCUSDT.BINANCE"

[parameters.entry_order]
order_type = "limit"
time_in_force = "fok"
post_only = false
reduce_only = false
quote_quantity = false

[parameters.exit_order]
order_type = "market"
time_in_force = "ioc"
post_only = false
reduce_only = false
quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"
```

## 7. Strategy File: Field Semantics

### Top level

#### `schema_version`

- type: integer
- required: yes
- versions the strategy-file schema only

#### `strategy_instance_identifier`

- type: string
- required: yes
- unique within a trader process
- operator-facing strategy instance identifier used in config and forensics

#### `strategy_archetype`

- type: string enum
- required: yes
- current supported value:
  - `binary_oracle_edge_taker`

This string binds to a compile-time Rust match in bolt's assembler.
There is no dynamic registry framework.

Nautilus strategy identity mapping for live trading:

- Nautilus `StrategyId` is derived as `"{strategy_archetype}-{order_id_tag}"`
- `strategy_instance_identifier` remains the operator-facing config and forensic identifier

#### `order_id_tag`

- type: string
- required: yes
- maps directly to Nautilus `StrategyConfig.order_id_tag`
- must be unique among all strategies under the same `trader_identifier`

#### `oms_type`

- type: string enum
- required: yes
- current allowed value:
  - `netting`
- maps directly to Nautilus `StrategyConfig.oms_type`

#### `venue`

- type: keyed reference string
- required: yes
- must reference a root venue block that includes `[execution]`

### `[target]`

#### `configured_target_id`

- type: string
- required: yes
- unique within a trader process
- maps to runtime `configured_updown_target.configured_target_id`
- reused on every decision event emitted for this configured target

This is the operator-facing target identifier used for forensics.
It is configuration, not a selected-market identifier.

#### `kind`

- type: string enum
- required: yes
- current allowed values:
  - `rotating_market`

#### Instrument target fields

Deferred.
Instrument targets are not part of the current frozen target-stack model.

If `kind = "instrument"`, validation must fail until a future contract slice defines the configured-target shape, selected-market facts boundary, and event projection.

#### Rotating-market target fields

If `kind = "rotating_market"`:

- `configured_target_id` is required
- `rotating_market_family` is required
- `underlying_asset` is required
- `cadence_seconds` is required
- `market_selection_rule` is required
- `retry_interval_seconds` is required
- `blocked_after_seconds` is required
- `instrument_id` is forbidden

##### `rotating_market_family`

- type: string enum
- current allowed value:
  - `updown`

##### `underlying_asset`

- type: string
- required: yes
- length: 1 to 32 characters
- must be a configured `updown` asset symbol
- allowed characters:
  - uppercase ASCII letters
  - digits
  - underscore
- runtime slug derivation lowercases this value for the `updown` market-slug asset segment

##### `cadence_seconds`

- type: integer
- required: yes
- must be positive
- must be divisible by `60`
- each supported value must have an explicit runtime slug-token mapping before it can trade
- current runtime slug-token mappings are defined in `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 5.3

##### `market_selection_rule`

- type: string enum
- current allowed value:
  - `active_or_next`

##### `retry_interval_seconds`

- type: positive integer
- required for rotating-market targets
- current expected value:
  - `5`

##### `blocked_after_seconds`

- type: positive integer
- required for rotating-market targets
- current expected value:
  - `60`

These fields live in the strategy file because they control that strategy's market-selection behavior.
The schema does not hardcode `BTC`, `ETH`, or `300` as the only supported `updown` target values; those may appear in examples only.

The runtime projection of the strategy-file `[target]` block plus the top-level `venue` field into `configured_updown_target` is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 6.1.

### `[reference_data.<name>]`

This section is optional.

If present:

- each block references a root venue that includes `[data]`
- each block declares the exact NautilusTrader `instrument_id` the strategy subscribes to
- for the current `binary_oracle_edge_taker`, the required role name is `primary`

Fields:

#### `venue`

- type: keyed reference string
- required

#### `instrument_id`

- type: string
- required

The TOML value is the literal NautilusTrader `InstrumentId` string.
The field name maps one-to-one to `nautilus_model::identifiers::InstrumentId`; aliases such as `instrument_identifier` are forbidden.
bolt does not define a second identifier format here.

No archetype may hardcode its reference data source in code.

### `[parameters.entry_order]` and `[parameters.exit_order]`

These are archetype-specific order-construction parameters for `binary_oracle_edge_taker`.
They are not a bolt-wide executable-order schema.

They must map directly to NautilusTrader-native order semantics used by the archetype.

#### `order_type`

- type: string enum
- allowed values for the current archetype:
  - `limit`
  - `market`

#### `time_in_force`

- type: string enum
- current allowed values:
  - `gtc`
  - `fok`
  - `ioc`

#### `post_only`

- type: boolean
- required

#### `reduce_only`

- type: boolean
- required

#### `quote_quantity`

- type: boolean
- required

Meaning:

- this is the NautilusTrader-native quote/base quantity toggle used by the archetype
- it is not a bolt-owned translation field
- for the current `binary_oracle_edge_taker` archetype, the only allowed value is `false`

### Current valid order combinations for `binary_oracle_edge_taker`

To avoid hidden policy, the current archetype supports only these combinations:

- `[parameters.entry_order]`
  - `order_type = "limit"`
  - `time_in_force = "fok"`
  - `post_only = false`
  - `reduce_only = false`
  - `quote_quantity = false`

- `[parameters.exit_order]`
  - `order_type = "market"`
  - `time_in_force = "ioc"`
  - `post_only = false`
  - `reduce_only = false`
  - `quote_quantity = false`

Any other combination fails validation for this archetype.

### `[parameters]`

This block is archetype-specific.

For the current `binary_oracle_edge_taker` archetype:

#### `edge_threshold_basis_points`

- type: integer
- required
- minimum selected-side edge required before the strategy may enter
- runtime evaluation against `worst_case_edge_basis_points` is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

#### `order_notional_target`

- type: decimal string
- required
- strategy-local desired notional target used by the archetype's sizing logic
- not the global hard cap
- validation requires `order_notional_target <= root risk.default_max_notional_per_order`
- runtime sizing usage is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

#### `maximum_position_notional`

- type: decimal string
- required
- maximum cumulative gross USDC entry-cost exposure the strategy may target for the selected market
- fees are not included in this cap
- runtime capacity computation is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

## 8. Validation Rules

### Structural validation

Must fail if:

- any required field is missing
- any unknown field is present
- a strategy file path is duplicated
- a referenced file does not exist
- a venue reference points to a missing venue
- a strategy `venue` points to a data-only venue
- a reference-data venue points to a venue without `[data]`
- two listed strategy files declare the same `strategy_instance_identifier`
- two listed strategy files declare the same `order_id_tag`
- two configured targets declare the same `configured_target_id`
- `signature_type` is not one of the allowed strings
- `target.kind = "rotating_market"` includes fields not valid for rotating-market targets
- `target.kind = "instrument"` is selected before instrument targets are added by a future contract slice
- `target.underlying_asset` is empty, longer than 32 characters, or contains characters outside uppercase ASCII letters, digits, and underscore
- `target.cadence_seconds` is not positive or is not divisible by `60`
- `target.cadence_seconds` does not have a runtime-contract-defined slug-token mapping
- a field appears under `[venues.<identifier>.data]` or `[venues.<identifier>.execution]` that is not allowed for that venue `kind`
- archetype-specific parameter sections contain fields not allowed for the declared `strategy_archetype`
- archetype-specific order parameters contain any combination not explicitly allowed for that archetype
- `order_notional_target` exceeds `root risk.default_max_notional_per_order`
- `binary_oracle_edge_taker` is missing `[reference_data.primary]`

### Live validation

Live validation behavior, fatal-vs-warning classification, and the full failure-reason taxonomy are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 2 Phase 2.

## 9. Canonical Example: Minimal Live-Trading Pair

This example is structural.
It is not live-valid until the operator supplies real paths, SSM parameters, account identifiers, wallet addresses, writable directories, and venue credentials.

### Root

```toml
schema_version = 1
trader_identifier = "BOLT-001"

strategy_files = [
  "strategies/bitcoin_updown_main.toml",
]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
catalog_directory = "/var/lib/bolt/catalog"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
ws_timeout_seconds = 30
subscribe_new_markets = false
update_instruments_interval_minutes = 60
websocket_max_subscriptions_per_connection = 200

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
max_retries = 3
retry_delay_initial_milliseconds = 250
retry_delay_max_milliseconds = 2000
ack_timeout_seconds = 5

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"

[venues.binance_reference]
kind = "binance"

[venues.binance_reference.data]
product_types = ["spot"]
environment = "mainnet"
instrument_status_poll_seconds = 3600

[venues.binance_reference.secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
```

### Strategy

```toml
schema_version = 1
strategy_instance_identifier = "bitcoin_updown_main"
strategy_archetype = "binary_oracle_edge_taker"
order_id_tag = "001"
oms_type = "netting"
venue = "polymarket_main"

[target]
configured_target_id = "btc_updown_5m"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "BTC"
cadence_seconds = 300
market_selection_rule = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60

[reference_data.primary]
venue = "binance_reference"
instrument_id = "BTCUSDT.BINANCE"

[parameters.entry_order]
order_type = "limit"
time_in_force = "fok"
post_only = false
reduce_only = false
quote_quantity = false

[parameters.exit_order]
order_type = "market"
time_in_force = "ioc"
post_only = false
reduce_only = false
quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"
```
