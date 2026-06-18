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
- keyed client definitions
- client secret references
- keyed gate-provider definitions
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

- client construction
- client credentials
- process-wide logging settings
- process-wide state paths
- process-wide Nautilus runtime settings

## 4. Root File: Candidate Schema

This is a structural example, not a default configuration.
Values such as paths, SSM parameter names, account identifiers, wallet addresses, and client keys must be operator-owned TOML values in a real deploy.

```toml
schema_version = 1
trader_id = "BOLT-001"

strategy_files = [
  "strategies/configured_updown_main.toml",
]

[runtime]
mode = "Live"

[nautilus]
load_state = true
save_state = true
shutdown_on_error = false
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 0
open_check_lookback_mins = 60
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 0
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
max_order_submit_rate = "40/00:01:00"
max_order_modify_rate = "40/00:01:00"
max_notional_per_order = {}
debug = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/srv/bolt-v2/var/bolt-v3-live/catalog"
required_catalog_prefix = "/srv/bolt-v2"
min_free_bytes = 10737418240
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.data]
base_url_http = "https://clob.polymarket.com" # NT: nautilus_polymarket::config::PolymarketDataClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market" # NT: PolymarketDataClientConfig.base_url_ws
base_url_rtds = "wss://ws-live-data.polymarket.com" # NT: PolymarketDataClientConfig.base_url_rtds
base_url_gamma = "https://gamma-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_gamma
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_data_api
http_timeout_secs = 60 # NT: PolymarketDataClientConfig.http_timeout_secs
ws_timeout_secs = 30 # NT: PolymarketDataClientConfig.ws_timeout_secs
subscribe_new_markets = false # NT: PolymarketDataClientConfig.subscribe_new_markets — forced false in current bolt-v3 scope
new_market_fetch_max_concurrency = 8 # NT: PolymarketDataClientConfig.new_market_fetch_max_concurrency
auto_load_missing_instruments = false # NT: PolymarketDataClientConfig.auto_load_missing_instruments — forced false in current bolt-v3 scope
auto_load_debounce_ms = 250 # NT: PolymarketDataClientConfig.auto_load_debounce_ms
auto_load_max_retries = 12 # NT: PolymarketDataClientConfig.auto_load_max_retries
auto_load_retry_delay_initial_secs = 5 # NT: PolymarketDataClientConfig.auto_load_retry_delay_initial_secs
auto_load_retry_delay_max_secs = 15 # NT: PolymarketDataClientConfig.auto_load_retry_delay_max_secs
resolve_poll_enabled = false # NT: PolymarketDataClientConfig.resolve_poll_enabled
resolve_poll_interval_secs = 30 # NT: PolymarketDataClientConfig.resolve_poll_interval_secs
resolve_poll_grace_secs = 10 # NT: PolymarketDataClientConfig.resolve_poll_grace_secs
resolve_poll_max_wait_secs = 1800 # NT: PolymarketDataClientConfig.resolve_poll_max_wait_secs
update_instruments_interval_mins = 60 # NT: PolymarketDataClientConfig.update_instruments_interval_mins
ws_max_subscriptions = 200 # NT: PolymarketDataClientConfig.ws_max_subscriptions
transport_backend = "sockudo" # NT: PolymarketDataClientConfig.transport_backend

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001" # NT: nautilus_model::identifiers::AccountId
signature_type = "poly_proxy" # NT: nautilus_polymarket::common::enums::SignatureType
funder = "0x1111111111111111111111111111111111111111" # NT: PolymarketExecClientConfig.funder
base_url_http = "https://clob.polymarket.com" # NT: PolymarketExecClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user" # NT: PolymarketExecClientConfig.base_url_ws
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketExecClientConfig.base_url_data_api
http_timeout_secs = 60 # NT: PolymarketExecClientConfig.http_timeout_secs
max_retries = 3 # NT: PolymarketExecClientConfig.max_retries
retry_delay_initial_ms = 250 # NT: PolymarketExecClientConfig.retry_delay_initial_ms
retry_delay_max_ms = 2000 # NT: PolymarketExecClientConfig.retry_delay_max_ms
ack_timeout_secs = 5 # NT: PolymarketExecClientConfig.ack_timeout_secs
fee_cache_ttl_secs = 300 # NT: PolymarketExecClientConfig fee cache TTL
transport_backend = "sockudo" # NT: PolymarketExecClientConfig.transport_backend

[clients.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"

[clients.binance_reference]
venue = "BINANCE"

[clients.binance_reference.data]
product_type = "spot" # NT: nautilus_binance::config::BinanceDataClientConfig.product_type
environment = "mainnet" # NT: BinanceDataClientConfig.environment
base_url_http = "https://api.binance.com" # NT: BinanceDataClientConfig.base_url_http
base_url_ws = "wss://stream-sbe.binance.com/ws" # NT: BinanceDataClientConfig.base_url_ws
spot_market_data_mode = "sbe" # NT: BinanceDataClientConfig.spot_market_data_mode
instrument_status_poll_secs = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs
transport_backend = "sockudo" # NT: BinanceDataClientConfig.transport_backend

[clients.binance_reference.secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
```

## 5. Root File: Field Semantics

### Top level

#### `schema_version`

- type: integer
- required: yes
- meaning: version of the root-file schema only

#### `trader_id`

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
  - `Live`
- any other value fails validation

#### `order_execution_mode`

- type: string enum
- required: yes
- allowed values:
  - `live`
  - `shadow`
- `live` records order intent, evaluates submit admission, and forwards admitted submits/cancels to NautilusTrader
- `shadow` records order intent and submit-admission evidence without consuming live submit capacity or forwarding submit/cancel mutations to NautilusTrader
- `shadow` rejects every loaded strategy unless `manage_stop`, `manage_gtd_expiry`, and `manage_contingent_orders` are `false` and `external_order_claims` is empty

### `[nautilus]`

The fields below map to top-level NautilusTrader `LiveNodeConfig` values. Top-level `LiveNodeConfig` surfaces not represented here are intentionally disabled or empty in the Bolt-v3 builder path (`instance_id`, `cache`, `msgbus`, `portfolio`, `emulator`, `streaming`, `loop_debug`, `data_clients`, and `exec_clients`). They are not inherited from `LiveNodeConfig::default()`. Duration-valued TOML fields use explicit `_secs` suffixes because the operator file stores integer seconds; the Rust mapper converts those integers into NautilusTrader `Duration` fields such as `delay_post_stop` and `timeout_shutdown`.

#### `load_state`

- type: boolean
- required: yes
- maps to Nautilus live-node state loading

#### `save_state`

- type: boolean
- required: yes
- maps to Nautilus live-node state saving

#### `shutdown_on_error`

- type: boolean
- required: yes
- maps to Nautilus `LiveNodeConfig.shutdown_on_error`
- current baseline value is `false`

#### `timeout_connection_secs`

- type: positive integer
- required: yes
- bounds the explicit bolt-v3 controlled-connect boundary

#### `timeout_reconciliation_secs`

- type: positive integer
- required: yes

#### `timeout_portfolio_secs`

- type: positive integer
- required: yes

#### `timeout_disconnection_secs`

- type: positive integer
- required: yes
- bounds the explicit bolt-v3 controlled-disconnect boundary

#### `delay_post_stop_secs`

- type: non-negative integer
- required: yes
- maps to Nautilus `LiveNodeConfig.delay_post_stop`
- note: Nautilus builder helper naming uses `with_delay_post_stop_secs`; bolt-v3 TOML uses `delay_post_stop_secs` and maps to NT `delay_post_stop`

#### `timeout_shutdown_secs`

- type: positive integer
- required: yes
- maps to Nautilus live-node shutdown timeout, not a custom bolt concept
- exact mapping target: Nautilus `LiveNodeConfig.timeout_shutdown`
- note: Nautilus builder helper naming uses `with_delay_shutdown_secs`; bolt-v3 TOML uses `timeout_shutdown_secs` and maps to NT `timeout_shutdown`

### `[nautilus.data_engine]`

All pinned `LiveDataEngineConfig` fields are explicit in TOML and mapped into the NautilusTrader Rust live-node config. Empty `external_clients` maps to Nautilus `None`. `time_bars_origins` keys must be Nautilus `BarAggregation` variant strings such as `Minute`, and values are origin offsets in nanoseconds.

Runtime-support guard fields are still required in TOML at the only accepted value so upstream default drift cannot silently change the built node:

- `qsize` must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, currently `100000` at NT rev `6be5a5094716790a8ca2875445fde4fa2586107e`

| Field | Type / Rule | Maps to |
|---|---|---|
| `time_bars_build_with_no_updates` | boolean | `LiveDataEngineConfig.time_bars_build_with_no_updates` |
| `time_bars_timestamp_on_close` | boolean | `LiveDataEngineConfig.time_bars_timestamp_on_close` |
| `time_bars_skip_first_non_full_bar` | boolean | `LiveDataEngineConfig.time_bars_skip_first_non_full_bar` |
| `time_bars_interval_type` | valid NT `BarIntervalType` string; current baseline `LEFT_OPEN` | `LiveDataEngineConfig.time_bars_interval_type` |
| `time_bars_build_delay` | non-negative integer microseconds | `LiveDataEngineConfig.time_bars_build_delay` |
| `time_bars_origins` | TOML inline table mapping valid NT `BarAggregation` strings to origin offsets in nanoseconds | `LiveDataEngineConfig.time_bars_origin_offset` |
| `validate_data_sequence` | boolean | `LiveDataEngineConfig.validate_data_sequence` |
| `buffer_deltas` | boolean | `LiveDataEngineConfig.buffer_deltas` |
| `emit_quotes_from_book` | boolean | `LiveDataEngineConfig.emit_quotes_from_book` |
| `emit_quotes_from_book_depths` | boolean | `LiveDataEngineConfig.emit_quotes_from_book_depths` |
| `external_clients` | array of valid NT client IDs; empty maps to `None` | `LiveDataEngineConfig.external_clients` |
| `debug` | boolean | `LiveDataEngineConfig.debug` |
| `qsize` | must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, currently `100000` at NT rev `6be5a5094716790a8ca2875445fde4fa2586107e` | `LiveDataEngineConfig.qsize` |

### `[nautilus.exec_engine]`

All `LiveExecEngineConfig` fields are explicit in TOML and mapped into the pinned NautilusTrader Rust live-node config. For fields documented below as optional, `0` maps to Nautilus `None`; other non-negative fields pass their numeric value through. Empty identifier arrays map to Nautilus `None`.

Fields rejected by NautilusTrader's current Rust live runtime are still required in TOML at the only accepted value so upstream default drift cannot silently change the built node:

- `snapshot_orders = false`
- `snapshot_positions = false`
- `purge_from_database = false`
- `qsize` must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, currently `100000` at NT rev `6be5a5094716790a8ca2875445fde4fa2586107e`

#### `reconciliation_lookback_mins`

- type: non-negative integer
- required: yes
- `0` means unbounded lookback and maps to Nautilus `None`
- any positive value maps to that exact bounded minute count

#### `reconciliation_startup_delay_secs`

- type: non-negative integer
- required: yes
- maps to Nautilus `LiveExecEngineConfig.reconciliation_startup_delay_secs`
- this is explicit to prevent inheriting upstream reconciliation startup timing changes silently
- `0` is valid and disables the post-startup reconciliation grace period before continuous reconciliation checks begin

#### `max_single_order_queries_per_cycle`

- type: positive integer
- required: yes
- maps to Nautilus `LiveExecEngineConfig.max_single_order_queries_per_cycle`
- current baseline value is `10`

#### `position_check_threshold_ms`

- type: positive integer
- required: yes
- maps to Nautilus `LiveExecEngineConfig.position_check_threshold_ms`
- current baseline value is `5000`

#### Remaining explicit exec-engine fields

| Field | Type / Rule | Maps to |
|---|---|---|
| `load_cache` | boolean | `LiveExecEngineConfig.load_cache` |
| `snapshot_orders` | must be `false` | `LiveExecEngineConfig.snapshot_orders` |
| `snapshot_positions` | must be `false` | `LiveExecEngineConfig.snapshot_positions` |
| `snapshot_positions_interval_secs` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.snapshot_positions_interval_secs` |
| `external_clients` | array of valid NT client IDs; empty maps to `None` | `LiveExecEngineConfig.external_clients` |
| `debug` | boolean | `LiveExecEngineConfig.debug` |
| `reconciliation` | boolean | `LiveExecEngineConfig.reconciliation` |
| `reconciliation_instrument_ids` | array of valid NT instrument IDs; empty maps to `None` | `LiveExecEngineConfig.reconciliation_instrument_ids` |
| `filter_unclaimed_external_orders` | boolean | `LiveExecEngineConfig.filter_unclaimed_external_orders` |
| `filter_position_reports` | boolean | `LiveExecEngineConfig.filter_position_reports` |
| `filtered_client_order_ids` | array of valid NT client order IDs; empty maps to `None` | `LiveExecEngineConfig.filtered_client_order_ids` |
| `generate_missing_orders` | boolean | `LiveExecEngineConfig.generate_missing_orders` |
| `inflight_check_interval_ms` | non-negative integer | `LiveExecEngineConfig.inflight_check_interval_ms` |
| `inflight_check_threshold_ms` | positive integer | `LiveExecEngineConfig.inflight_check_threshold_ms` |
| `inflight_check_retries` | non-negative integer | `LiveExecEngineConfig.inflight_check_retries` |
| `open_check_interval_secs` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.open_check_interval_secs` |
| `open_check_lookback_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.open_check_lookback_mins` |
| `open_check_threshold_ms` | positive integer | `LiveExecEngineConfig.open_check_threshold_ms` |
| `open_check_missing_retries` | non-negative integer | `LiveExecEngineConfig.open_check_missing_retries` |
| `open_check_open_only` | boolean | `LiveExecEngineConfig.open_check_open_only` |
| `single_order_query_delay_ms` | non-negative integer | `LiveExecEngineConfig.single_order_query_delay_ms` |
| `position_check_interval_secs` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.position_check_interval_secs` |
| `position_check_lookback_mins` | non-negative integer; NT pins this as `u32`, so `0` passes through as a 0-minute lookback rather than mapping to `None` | `LiveExecEngineConfig.position_check_lookback_mins` |
| `position_check_retries` | non-negative integer | `LiveExecEngineConfig.position_check_retries` |
| `purge_closed_orders_interval_mins` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.purge_closed_orders_interval_mins` |
| `purge_closed_orders_buffer_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.purge_closed_orders_buffer_mins` |
| `purge_closed_positions_interval_mins` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.purge_closed_positions_interval_mins` |
| `purge_closed_positions_buffer_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.purge_closed_positions_buffer_mins` |
| `purge_account_events_interval_mins` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.purge_account_events_interval_mins` |
| `purge_account_events_lookback_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.purge_account_events_lookback_mins` |
| `purge_from_database` | must be `false` | `LiveExecEngineConfig.purge_from_database` |
| `own_books_audit_interval_secs` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.own_books_audit_interval_secs` |
| `qsize` | must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, currently `100000` at NT rev `6be5a5094716790a8ca2875445fde4fa2586107e` | `LiveExecEngineConfig.qsize` |
| `allow_overfills` | boolean | `LiveExecEngineConfig.allow_overfills` |
| `manage_own_order_books` | boolean | `LiveExecEngineConfig.manage_own_order_books` |

### `[risk]`

This section owns both Bolt-v3 strategy-sizing limits and the configurable pinned NautilusTrader live risk-engine fields. All configurable `nt_*` fields are required in TOML and mapped into `LiveRiskEngineConfig`, except `bypass`, which is not a config field and is pinned to `false` directly in code (see below); `default_max_notional_per_order` is the Bolt-v3-owned strategy-sizing cap. Fields under `[nautilus]` do not use the prefix because the section name already carries the NT context.

#### `default_max_notional_per_order`

- type: positive decimal string
- required: yes
- root-level entity per-order notional cap
- enforced by bolt-v3 strategy validation: each strategy file's `parameters.order_notional_target` must be `<=` this value
- not automatically expanded into NautilusTrader per-instrument maps; `risk.nautilus.max_notional_per_order` is the explicit NT map when instrument-level caps are intentionally configured

#### NautilusTrader risk-engine bypass (removed config field)

- the previously configurable `bypass` field inside `[risk.nautilus]` has been removed
- NautilusTrader `LiveRiskEngineConfig.bypass` is now pinned to `false` directly in code (in the live-node config construction); there is no config knob and no "safety exception" path
- a stray `bypass` key under `[risk.nautilus]` is rejected at parse time via `deny_unknown_fields`

#### `max_order_submit_rate` (inside `[risk.nautilus]`)

- type: rate-limit string in Nautilus `limit/HH:MM:SS` format
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.max_order_submit_rate`

#### `max_order_modify_rate` (inside `[risk.nautilus]`)

- type: rate-limit string in Nautilus `limit/HH:MM:SS` format
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.max_order_modify_rate`

#### `max_notional_per_order` (inside `[risk.nautilus]`)

- type: TOML inline table mapping Nautilus instrument IDs to decimal notional strings
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.max_notional_per_order`
- values must be positive decimal strings
- `{}` means no NT per-instrument cap is configured; Bolt-v3 still enforces `default_max_notional_per_order` at config validation time

#### `debug` (inside `[risk.nautilus]`)

- type: boolean
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.debug`
- current baseline value is `false`

#### `qsize` (inside `[risk.nautilus]`)

- type: positive integer
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.qsize`
- must equal the pinned NT `LiveRiskEngineConfig::default().qsize` value, currently `100000` at NT rev `6be5a5094716790a8ca2875445fde4fa2586107e`

### `[logging]`

#### `stdout_level`

- type: string enum
- required: yes
- allowed values:
  - `TRACE`
  - `DEBUG`
  - `INFO`
  - `WARN`
  - `ERROR`
  - `OFF`

#### `fileout_level`

- type: string enum
- required: yes
- allowed values:
  - `TRACE`
  - `DEBUG`
  - `INFO`
  - `WARN`
  - `ERROR`
  - `OFF`

Bolt-v3 also installs unconditional module-level filters that suppress NT's credential info logs from `nautilus_polymarket::common::credential` and `nautilus_binance::common::credential` to `WARN`, regardless of `stdout_level` and `fileout_level`. These two NT modules log credential-derived material at info-level (Polymarket address/funder/api-key prefixes; Binance auto-detected key type), so bolt-v3 forces them lower than the root level rather than letting an `INFO` root level surface those prefixes in stdout or the file writer.

Bolt-v3 sets every pinned NautilusTrader `LoggerConfig` field explicitly before handing the config to `LiveNodeBuilder::from_config`. TOML owns `stdout_level` and `fileout_level`; bolt-v3 owns the credential module filters; `component_level` is empty, `log_components_only = false`, `is_colored = true`, `print_config = false`, `use_tracing = false`, and `bypass_logging = false`.

There is no separate `log_directory` knob in the current bolt-v3 scope. Bolt-v3 hands the complete `LoggerConfig` to NT through `LiveNodeBuilder::from_config`; the file-writer directory is owned by NT's `init_logging` path which bolt-v3 does not yet wire. `file_config` remains `None` and `clear_log_file` remains `false`; NT's pinned Rust live runtime rejects non-disabled values for those fields. A TOML field for either value would be a no-op or an invalid runtime request, so the schema deliberately omits it.

### `[persistence]`

#### `catalog_directory`

- type: absolute path string
- required: yes
- local Nautilus catalog root for structured decision events and raw NautilusTrader capture
- persistence behavior and local-evidence requirements are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 9.6, 9.7, and 10

#### `required_catalog_prefix`

- type: absolute path string
- required: yes for live startup and storage prestart checks
- canonical parent path that `catalog_directory` must stay under before a live node starts

#### `min_free_bytes`

- type: positive integer
- required: yes for live startup and storage prestart checks
- free-space floor for the filesystem that contains `catalog_directory`; the production root config uses `10737418240` bytes, or 10 GiB

#### `runtime_capture_start_poll_interval_ms`

- type: positive integer
- required: yes
- local poll interval used while waiting for runtime-capture startup evidence to appear
- stays in TOML so startup/capture timing is operator-owned and not hardcoded in code

### `[persistence.decision_evidence]`

#### `order_intents_relative_path`

- type: relative path string
- required: yes
- local decision-evidence JSONL path under `catalog_directory`
- must remain relative so a root catalog move changes only one config location

Decision-evidence JSONL records use `schema_version = 11` for `order_intent`, `admission_decision`, `strategy_input_snapshot`, `position_sizer_rebuild`, `submit_reservation_metadata`, and `submit_reservation_fill` envelopes.
Each line is a single JSON object with `schema_version`, `recorded_at_utc_ns`, `gate_version`, `gate_id`, `kind`, and the matching payload field: `intent`, `decision`, `snapshot`, `audit`, `metadata`, or `fill`.
The `kind` field is `order_intent` for `intent` payloads, `admission_decision` for `decision` payloads, `strategy_input_snapshot` for `snapshot` payloads, `position_sizer_rebuild` for startup rebuild audit payloads, `submit_reservation_metadata` for admitted reservation metadata, and `submit_reservation_fill` for fill metadata.
`order_intent` payloads carry the configured strategy/order identity plus compiled NT order semantics under `order_fields`.
`admission_decision` payloads carry the submit-admission gate decision for the same `client_order_id` and the `execution_client_id` whose submit-admission limits were evaluated.
`strategy_input_snapshot` payloads carry source-bound entry decision inputs captured before order-intent recording.
`position_sizer_rebuild`, `submit_reservation_metadata`, and `submit_reservation_fill` payloads support startup reservation recovery and fail closed on pre-schema-10 reservation records.
Schema 11 also records reference-current-price provenance fields in strategy-input snapshots.

`order_intent.order_fields` fields:

- `order_type`: compiled NT order type
- `time_in_force`: compiled NT time-in-force
- `price`: optional compiled limit price
- `trigger_price`: optional compiled trigger price
- `activation_price`: optional compiled activation price
- `trigger_type`: optional compiled trigger type
- `trigger_instrument_id`: optional compiled trigger instrument id
- `trailing_offset`: optional compiled trailing offset
- `trailing_offset_type`: optional compiled trailing offset type
- `expire_time_unix_nanos`: optional compiled NT expiry timestamp
- `is_post_only`: compiled NT post-only flag
- `is_reduce_only`: compiled NT reduce-only flag
- `is_quote_quantity`: compiled NT quote-quantity flag

There is no `state_directory` in the current bolt-v3 scope. NT's pinned `LiveNodeBuilder` does not expose a state-directory wiring (load/save state are booleans only), so a TOML key would not flow to NT. A future slice may reintroduce this once a supported path exists.

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

#### `flush_interval_ms`

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

### `[clients.<identifier>]`

#### Client key

- type: keyed identifier
- required: yes for every configured client
- examples:
  - `polymarket_main`
  - `binance_reference`

The key is a configuration reference name.
It is not the trader identifier.

`[clients]` is a map keyed by these identifiers, so each client key must be unique.
More than one client may target the same `venue`: the schema does not enforce a one-client-per-venue rule.
A common layout is a trade client and a separate reference-data client on the same venue, distinguished by their keys and their `[data]` / `[execution]` blocks.
Cross-client collisions are rejected only where two clients would produce indistinguishable runtime evidence: the same `instrument_id` must not be declared under more than one client's `readiness_probe.quote_targets`, and the same reference-data `instrument_id` must not be declared under more than one `data_client_id`, because NautilusTrader `QuoteTick` carries the instrument but not the producing data-client identifier.

#### `venue`

- type: NautilusTrader `Venue` identifier (string)
- required: yes
- current allowed values:
  - `POLYMARKET`
  - `BINANCE`

### `[clients.<identifier>.data]`

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

##### `base_url_rtds`

- type: string
- required: yes
- maps directly to `PolymarketDataClientConfig.base_url_rtds`

##### `base_url_gamma`

- type: string
- required: yes

##### `base_url_data_api`

- type: string
- required: yes

##### `http_timeout_secs`

- type: positive integer
- required: yes

##### `ws_timeout_secs`

- type: positive integer
- required: yes

##### `subscribe_new_markets`

- type: boolean
- required: yes
- must be `false` in the current bolt-v3 scope: validation fails closed if set to `true`
- the pinned NautilusTrader Polymarket data client calls `ws_client.subscribe_market(vec![])` from inside its `connect()` when this flag is `true`, which is effectively an all-markets subscription and violates the bolt-v3 controlled-connect boundary
- this flag is forced `false` until the dedicated market-subscription slice owns the controlled-subscribe path

##### `new_market_fetch_max_concurrency`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.new_market_fetch_max_concurrency`

##### `auto_load_missing_instruments`

- type: boolean
- required: yes
- must be `false` in the current bolt-v3 scope
- missing-instrument auto-load can trigger ad-hoc Gamma loads outside the configured market-identity plan

##### `auto_load_debounce_ms`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.auto_load_debounce_ms`

##### `auto_load_max_retries`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.auto_load_max_retries`

##### `auto_load_retry_delay_initial_secs`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.auto_load_retry_delay_initial_secs`

##### `auto_load_retry_delay_max_secs`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.auto_load_retry_delay_max_secs`

##### `resolve_poll_enabled`

- type: boolean
- required: yes
- maps directly to `PolymarketDataClientConfig.resolve_poll_enabled`

##### `resolve_poll_interval_secs`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.resolve_poll_interval_secs`

##### `resolve_poll_grace_secs`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.resolve_poll_grace_secs`

##### `resolve_poll_max_wait_secs`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.resolve_poll_max_wait_secs`

##### `update_instruments_interval_mins`

- type: positive integer
- required: yes
- maps to `PolymarketDataClientConfig.update_instruments_interval_mins` as `Some(value)`
- background Polymarket adapter refresh interval only
- not the sole mechanism keeping current rotating-market data loaded

##### `ws_max_subscriptions`

- type: positive integer
- required: yes

##### `transport_backend`

- type: string enum
- required: yes
- current allowed value:
  - `sockudo`
- maps directly to the pinned NT adapter `transport_backend` field

No other Polymarket data-client fields are exposed in the current schema unless they are confirmed on the pinned NautilusTrader Rust adapter surface.

For current reference-data clients other than Polymarket, each client's `venue` defines its own allowed `[data]` field set.
Unknown fields fail validation against the venue-specific set in Section 8.

### `[clients.<identifier>.execution]`

Presence of `[execution]` means an execution client is configured.

#### `account_id`

- type: string
- required: yes for execution-capable clients

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

#### `funder`

- type: optional string
- required: yes for Polymarket execution when `signature_type` is `poly_proxy` or `poly_gnosis_safe`
- allowed absent for `signature_type = "eoa"`
- this is a public address, not a secret value
- it lives in the root client execution config, not in `[secrets]`
- zero address is invalid when the selected signature path requires a real funder wallet

#### `max_retries`

- type: positive integer
- required: yes

#### `retry_delay_initial_ms`

- type: positive integer
- required: yes

#### `retry_delay_max_ms`

- type: positive integer
- required: yes

#### `ack_timeout_secs`

- type: positive integer
- required: yes
- maps directly to the pinned Polymarket execution-client acknowledgment timeout field

#### Additional Polymarket execution fields

The current schema also requires these pinned adapter fields to be explicit:

- `base_url_http`
- `base_url_ws`
- `base_url_data_api`
- `http_timeout_secs`
- `fee_cache_ttl_secs`
- `transport_backend`

`fee_cache_ttl_secs` is a positive integer and controls the provider fee cache lifetime.
`transport_backend` is a string enum with current allowed value `sockudo` and maps directly to the pinned NT adapter field.

#### `[clients.<identifier>.execution.on_chain_collateral]`

This subtable is optional for a Polymarket `[execution]` block. When present it configures the on-chain collateral-accounting source used by the provider CLOB V2 collateral proof; when absent the operator artifact path treats on-chain collateral accounting as unconfigured.

| Field | Type / Rule | Required |
|---|---|---|
| `rpc_url` | string; must start with `http://` or `https://` | yes when `[on_chain_collateral]` is present |
| `chain_id` | positive integer EVM chain id | yes when `[on_chain_collateral]` is present |
| `collateral_token_address` | `0x`-prefixed EVM public address (collateral token contract) | yes when `[on_chain_collateral]` is present |

### `[clients.<identifier>.secrets]`

Presence of `[secrets]` means the client requires credential resolution.
The block must be consumed by an adapter in the same client:

- Polymarket `[secrets]` is allowed only when `[execution]` is present
- Binance `[secrets]` is allowed only when `[data]` is present

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

##### `product_type`

- type: string enum
- required: yes
- current allowed value:
  - `spot`
- maps to Nautilus `BinanceDataClientConfig.product_type`

##### `environment`

- type: string enum
- required: yes
- current allowed value:
  - `mainnet`
- maps to Nautilus `BinanceDataClientConfig.environment`

##### `base_url_http`

- type: string
- required: yes
- maps to Nautilus `BinanceDataClientConfig.base_url_http`
- explicit TOML ownership prevents NautilusTrader from falling back to its compiled-in Binance HTTP URL

##### `base_url_ws`

- type: string
- required: yes
- maps to Nautilus `BinanceDataClientConfig.base_url_ws`
- explicit TOML ownership prevents NautilusTrader from falling back to its compiled-in Binance WebSocket URL
- current baseline uses Binance's SBE WebSocket endpoint, matching `spot_market_data_mode = "sbe"`

##### `spot_market_data_mode`

- type: string enum
- required: yes
- current allowed values:
  - `sbe`
  - `json`
- maps to Nautilus `BinanceDataClientConfig.spot_market_data_mode`
- current baseline value is `sbe`, matching the configured Binance SBE WebSocket endpoint

##### `instrument_status_poll_secs`

- type: positive integer
- required: yes
- maps to Nautilus `BinanceDataClientConfig.instrument_status_poll_secs`
- bolt-v3 rejects `0` rather than treating it as "polling disabled" so that the cadence stays explicit and NT cannot silently fall back to its own default poll interval

##### `transport_backend`

- type: string enum
- required: yes
- current allowed value:
  - `sockudo`
- maps directly to `BinanceDataClientConfig.transport_backend`

### `[clients.<identifier>.readiness_probe]`

This subtable is optional. When present it configures the strategy-free data-client readiness probe for that client and the client must also declare a `[data]` block. It does not authorize order submission; it only proves the client's market-data path delivers fresh observations.

| Field | Type / Rule | Required |
|---|---|---|
| `market_data_kind` | string enum: `quote`, `book`, or `trade` | yes when `[readiness_probe]` is present |
| `book_type` | string enum: `l1_mbp`, `l2_mbp`, or `l3_mbo` | required when `market_data_kind = "book"`; forbidden otherwise |
| `quote_target_source` | string enum: `configured` or `metadata_response` | yes when `[readiness_probe]` is present |
| `max_metadata_quote_targets` | positive integer; cap on sampled metadata targets | required when `quote_target_source = "metadata_response"` and not a trade chunk-count probe; forbidden when `quote_target_source = "configured"` |
| `allow_metadata_target_sampling` | boolean; whether broad metadata universes may be sampled | must be set explicitly when `quote_target_source = "metadata_response"` and not a trade chunk-count probe; forbidden when `quote_target_source = "configured"` |
| `min_observed_targets` | positive integer; minimum sampled targets that must produce a fresh observation. When unset the probe requires every sampled target (strict, fail-closed default) | required for a trade chunk-count probe; optional otherwise |
| `chunk_size` | positive integer; maximum instruments a trade chunk-count probe subscribes to at once while walking the venue universe in chunks | required (and only valid) when `market_data_kind = "trade"` and `quote_target_source = "metadata_response"` |
| `chunk_observation_window_seconds` | positive integer; seconds a trade chunk-count probe watches each chunk before advancing | required (and only valid) when `market_data_kind = "trade"` and `quote_target_source = "metadata_response"` |
| `quote_targets` | map of target-id to `{ instrument_id }` blocks | required and non-empty when `quote_target_source = "configured"`; forbidden when `quote_target_source = "metadata_response"` |

A trade chunk-count probe is the combination `market_data_kind = "trade"` with `quote_target_source = "metadata_response"`: it walks the venue's full instrument universe in chunks of `chunk_size`, watches each chunk for `chunk_observation_window_seconds`, and passes once `min_observed_targets` (`m`) distinct markets have traded. It has no fixed sample, so the sampling knobs (`max_metadata_quote_targets`, `allow_metadata_target_sampling`) are rejected for it and the chunk knobs are required instead.

#### `[clients.<identifier>.readiness_probe.quote_targets.<target-id>]`

- `instrument_id`: string; literal NautilusTrader `InstrumentId`, required. Its venue must match the client's `venue`.

The same `instrument_id` must not appear under more than one client's `readiness_probe.quote_targets`, because NautilusTrader `QuoteTick` does not carry the producing data-client identifier and readiness quote evidence must stay source-disambiguated.

### `[gate_providers.<identifier>]`

This root-level section is optional. It is a map keyed by provider identifier that declares the resolution/reference gate providers (such as the Chainlink Data Streams resolution anchor) the runtime may consume. Each provider key must be unique.

#### `provider_kind`

- type: string enum
- required: yes when the provider block is present
- registered kinds: `chainlink_data_streams`, `pyth`, `exchange_index`, `venue_native`, `hyperliquid_hip4`, `deribit_index`, `outcome_oracle`
- `test_double` is registered for source/unit tests only and is rejected in live/local operator TOML

#### `capabilities`

- type: array of string enums
- required: yes; must contain one or more entries
- registered capabilities: `resolution_value`, `reference_value`, `market_metadata`

#### `client_id`

- type: optional keyed reference string
- when present, must match a root `[clients.<id>]` key

#### `[gate_providers.<identifier>.freshness]`

- required: yes when the provider block is present
- `max_age_ms`: positive integer; maximum accepted observation age
- `max_clock_skew_ms`: positive integer; maximum accepted clock skew; must be `<=` `max_age_ms`

#### Provider-specific subtable

- a provider with `provider_kind = "<kind>"` must define exactly one provider-specific subtable named `[gate_providers.<identifier>.<kind>]`, and no other subtable
- a `ssm_credential_parameter` field inside a provider subtable, when present, must be a non-empty absolute-style SSM path starting with `/`
- the `chainlink_data_streams` subtable has its own required fields (`endpoint_id`, `rest_base_url`, `report_endpoint_path`, `http_timeout_secs`, `api_key_ssm_parameter`, `api_secret_ssm_parameter`, and one or more `[[...feed_bindings]]`); those provider-specific field rules are owned by the Chainlink gate-provider validator and are not restated here

## 6. Strategy File: Candidate Schema

```toml
schema_version = 2
strategy_instance_id = "configured_updown_main"
strategy_archetype = "binary_oracle_edge_taker"
order_id_tag = "001"
oms_type = "netting"
use_uuid_client_order_ids = true
use_hyphens_in_client_order_ids = false
external_order_claims = []
manage_contingent_orders = false
manage_gtd_expiry = false
manage_stop = false
market_exit_interval_ms = 100
market_exit_max_attempts = 100
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
execution_client_id = "polymarket_main"
realized_volatility_surface_id = "configured_rv_surface"

[target]
configured_target_id = "configured_updown_target"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "CONFIGURED_ASSET"
cadence_secs = 300
cadence_slug_token = "configuredwindow"
market_selection_rule = "active_or_next"
retry_interval_secs = 5
blocked_after_secs = 60

[reference_data]

[parameters.entry_order]
side = "buy"
position_side = "long"
order_type = "limit"
time_in_force = "fok"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters.exit_order]
side = "sell"
position_side = "long"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters.forced_exit_order]
side = "sell"
position_side = "long"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"

[parameters.runtime]
reference_publish_topic = "platform.runtime.selection.binary_oracle_edge_taker-001"
warmup_tick_count = 20
reentry_cooldown_secs = 30
book_impact_cap_bps = 50
risk_lambda = 0.5
sizing_ev_reference_bps = 500
exit_hysteresis_bps = 25
trade_flow_window_secs = 60
trade_flow_max_samples = 2000
spike_guard_return_threshold = 0.02
spike_guard_cooldown_secs = 30
price_to_beat_source = "chainlink_data_streams.report_at_boundary"
pricing_kurtosis = 3.0
theta_decay_factor = 1.0
forced_flat_stale_chainlink_ms = 10000
forced_flat_thin_book_min_liquidity = 5.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250
```

## 7. Strategy File: Field Semantics

### Top level

#### `schema_version`

- type: integer
- required: yes
- versions the strategy-file schema only
- current supported value: `2`

#### `strategy_instance_id`

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
- `strategy_instance_id` remains the operator-facing config and forensic identifier

#### `order_id_tag`

- type: string
- required: yes
- maps directly to Nautilus `StrategyConfig.order_id_tag`
- must be unique among all strategies under the same `trader_id`

#### `oms_type`

- type: string enum
- required: yes
- delegates accepted values to NautilusTrader `OmsType`
- maps directly to NautilusTrader `StrategyConfig.oms_type`

The current source-level tests prove `netting`, `hedging`, and `unspecified` parse and validate.
Phase 8 approval-envelope validation canonicalizes this field through NautilusTrader `OmsType` before comparing it with loaded TOML.
This is source/config validation proof only; it does not prove live venue behavior for every OMS mode.

#### Other Nautilus `StrategyConfig` fields

These fields map directly to pinned NautilusTrader strategy configuration and are explicit in TOML to avoid NT defaults:

- `use_uuid_client_order_ids`: boolean; required
- `use_hyphens_in_client_order_ids`: boolean; required
- `external_order_claims`: array of strings; required
- `manage_contingent_orders`: boolean; required
- `manage_gtd_expiry`: boolean; required
- `manage_stop`: boolean; required
- `market_exit_interval_ms`: positive integer; required
- `market_exit_max_attempts`: positive integer; required
- `log_events`: boolean; required
- `log_commands`: boolean; required
- `log_rejected_due_post_only_as_warning`: boolean; required

#### `execution_client_id`

- type: keyed reference string (one of the keys under root `[clients.<id>]`)
- required: yes
- must reference a root client block that includes `[execution]`

#### `realized_volatility_surface_id`

- type: keyed reference string (one of the keys under root `[realized_volatility_surfaces.<id>]`)
- required: yes
- selects the shared TOML-owned realized-volatility surface consumed by taker pricing

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
- `cadence_secs` is required
- `market_selection_rule` is required
- `retry_interval_secs` is required
- `blocked_after_secs` is required
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

##### `cadence_secs`

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

##### `retry_interval_secs`

- type: positive integer
- required for rotating-market targets
- configured per strategy; examples use `5`

##### `blocked_after_secs`

- type: positive integer
- required for rotating-market targets
- configured per strategy; examples use `60`

These fields live in the strategy file because they control that strategy's market-selection behavior.
The schema does not hardcode `BTC`, `ETH`, or `300` as the only supported `updown` target values; those may appear in examples only.

The runtime projection of the strategy-file `[target]` block plus the top-level `execution_client_id` field into `configured_updown_target` is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 6.1.

### `[reference_data.<name>]`

This section is optional.

If present:

- each block references a root client that includes `[data]`
- each block declares the exact NautilusTrader `instrument_id` the strategy subscribes to
- the same `instrument_id` must not be declared under more than one `data_client_id`, because NautilusTrader `QuoteTick` carries the instrument but not the producing data-client identifier and readiness quote evidence must remain source-disambiguated
- for the current `binary_oracle_edge_taker`, the required role name is `primary`

Fields:

#### `data_client_id`

- type: keyed reference string (one of the keys under root `[clients.<id>]`)
- required

#### `instrument_id`

- type: string
- required

The TOML value is the literal NautilusTrader `InstrumentId` string.
The field name maps one-to-one to `nautilus_model::identifiers::InstrumentId`; aliases are forbidden.
bolt does not define a second identifier format here.

No archetype may hardcode its reference data source in code.

### `[parameters.entry_order]`, `[parameters.exit_order]`, and `[parameters.forced_exit_order]`

These are archetype-specific order-construction parameters for `binary_oracle_edge_taker`.
They are not a bolt-wide executable-order schema.

They must map directly to NautilusTrader-native order semantics used by the archetype.

Entry orders use the configured `entry_order` template.
Normal exits use the configured `exit_order` template.
Forced-flat exits use the configured `forced_exit_order` template.
When `manage_stop = true`, pinned NautilusTrader `Strategy::close_all_positions` submits market close orders; config validation therefore requires `parameters.forced_exit_order.order_type = "market"` for that mode.
For Polymarket-routed market `exit_order` and `forced_exit_order` templates, use `time_in_force = "ioc"` or `"fok"`; shipped configs use `"ioc"`. Polymarket also rejects reduce-only exit templates before submit, so set `is_reduce_only = false`.
Set `manage_stop = false` to use a non-market `forced_exit_order` through the strategy forced-flat path.

#### `side`

- type: string enum
- required
- current allowed values:
  - `buy`
  - `sell`
- maps to the order side used by the archetype

#### `position_side`

- type: string enum
- required
- parsed values:
  - `long`
  - `short`
- maps to the position side used by the archetype

The current archetype accepts the long position contract only:

- long position contract: entry side `buy`, exit side `sell`, both with `position_side = "long"`

Short-side position contracts are parsed but rejected until strategy-owned short economics, collateral, and exit semantics exist.

#### `order_type`

- type: string enum
- enabled through the pinned NT single-order `OrderFactory` for current source/unit config validation:
  - `limit`
  - `market`
  - `stop_market`
  - `stop_limit`
  - `market_if_touched`
  - `limit_if_touched`
  - `trailing_stop_market`
- parsed by NT but unsupported for this archetype because the pinned NT single-order `OrderFactory` exposes no public constructor:
  - `market_to_limit`
  - `trailing_stop_limit`

#### `time_in_force`

- type: string enum
- current allowed values:
  - `gtc`
  - `fok`
  - `ioc`
  - `gtd`

GTD order templates require `expire_time_unix_nanos` when the selected NT order type supports GTD.
`market` order templates reject GTD.

#### `expire_time_unix_nanos`

- type: integer
- required: no
- required for GTD order templates accepted by the current archetype
- maps to the NT order factory `expire_time` argument

#### `trigger_price`

- type: decimal string or TOML number
- required for `stop_market`, `stop_limit`, `market_if_touched`, and `limit_if_touched`
- required for `trailing_stop_market` unless `activation_price` is provided
- forbidden on non-triggered `limit` and `market` order templates
- maps to the NT order factory `trigger_price` argument

#### `activation_price`

- type: decimal string or TOML number
- required for `trailing_stop_market` when `trigger_price` is absent
- forbidden on other order types
- maps to the NT order factory `activation_price` argument

#### `trigger_type`

- type: string enum backed by NautilusTrader `TriggerType`
- optional for triggered order types supported by the current archetype
- `trigger_type` is optional for `trailing_stop_market`; NT defaults omitted values to `TriggerType::Default`
- forbidden on non-triggered `limit` and `market` order templates
- maps to the NT order factory `trigger_type` argument

#### `trigger_instrument_id`

- type: string backed by NautilusTrader `InstrumentId`
- optional for triggered order types supported by the current archetype
- forbidden on non-triggered `limit` and `market` order templates
- maps to the NT order factory `trigger_instrument_id` argument

#### `trailing_offset`

- type: decimal string or TOML number
- required for `trailing_stop_market`
- forbidden on other order types
- maps to the NT order factory `trailing_offset` argument

#### `trailing_offset_type`

- type: string enum backed by NautilusTrader `TrailingOffsetType`
- optional for `trailing_stop_market`
- `trailing_offset_type` is optional for `trailing_stop_market`; NT defaults omitted values to `TrailingOffsetType::Price`
- forbidden on other order types
- maps to the NT order factory `trailing_offset_type` argument

#### `is_post_only`

- type: boolean
- required
- maps to the NT order factory post-only flag for order types that expose it
- must be false for current market-style order types that do not support post-only behavior

#### `is_reduce_only`

- type: boolean
- required
- maps to the NT order factory reduce-only flag

#### `is_quote_quantity`

- type: boolean
- required

Meaning:

- this is the NautilusTrader-native quote/base quantity toggle used by the archetype
- it is not a bolt-owned translation field
- maps to the NT order factory quote-quantity flag
- Entry `is_quote_quantity = true` is supported by sizing the entry quantity as quote notional
- Exit `is_quote_quantity = true` is rejected because exits are sized from held base position quantity

### Current order template validation for `binary_oracle_edge_taker`

Order construction uses one TOML-to-template-to-NT path for entry and exit orders.
The archetype validates current pinned NT model invariants before calling `OrderFactory`; it does not maintain a maker/taker tuple allowlist.

Current validation rejects:

- unsupported NT enum variants without a pinned single-order `OrderFactory` constructor
- short-side position contracts until strategy-owned short economics, collateral, and exit semantics exist
- exit order templates with `is_quote_quantity = true`
- GTD order templates without `expire_time_unix_nanos`
- `market` order templates with GTD, `expire_time_unix_nanos`, or post-only
- market-style triggered orders with post-only
- non-triggered `limit` and `market` order templates with trigger or trailing fields
- triggered order templates without a positive `trigger_price`
- `limit_if_touched` templates whose trigger/limit relationship violates the pinned NT side invariant
- `trailing_stop_market` templates without positive trigger or activation input or positive trailing offset

Forced-flat exits from freeze, stale-data, and thin-book predicates use the configured `forced_exit_order` template.

### `[parameters]`

This block is archetype-specific.

For the current `binary_oracle_edge_taker` archetype:

#### `edge_threshold_basis_points`

- type: integer
- required
- minimum selected-side edge required before the strategy may enter
- runtime evaluation against `worst_case_edge_basis_points` is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

#### `order_notional_target`

- type: positive decimal string
- required
- strategy-local desired notional target used by the archetype's sizing logic
- not the global hard cap
- validation requires `order_notional_target` is a positive decimal, `order_notional_target <= root risk.default_max_notional_per_order`, and `order_notional_target <= maximum_position_notional`
- runtime sizing usage is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

#### `maximum_position_notional`

- type: positive decimal string
- required
- maximum cumulative gross pUSD entry-cost exposure the strategy may target for the selected market
- fees are not included in this cap
- runtime capacity computation is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

#### `[parameters.runtime]`

- type: table
- required for `binary_oracle_edge_taker`
- all fields are required and unknown fields are rejected
- runtime strategy configuration consumed by the Rust strategy registration path
- `book_impact_cap_bps` is enforced by strategy runtime configuration and shared submit-admission checks

Runtime fields:

- `reference_publish_topic`: string; reference-data topic consumed by the runtime strategy
- `warmup_tick_count`: unsigned integer; fresh-reference warmup count before entry is allowed
- `reentry_cooldown_secs`: unsigned integer; cooldown after an entry attempt
- `book_impact_cap_bps`: unsigned integer; maximum allowed book-impact basis points for order construction
- `risk_lambda`: float; sizing risk coefficient
- `sizing_ev_reference_bps`: unsigned integer; sizing saturates at `order_notional_target` when worst-case EV reaches `2 * risk_lambda * sizing_ev_reference_bps` (must be 1..=10000)
- `exit_hysteresis_bps`: integer; exit hysteresis threshold
- `trade_flow_window_secs`: unsigned integer; rolling retention window for signed trade flow
- `trade_flow_max_samples`: unsigned integer; hard cap on retained signed trades per instrument (memory bound)
- `spike_guard_return_threshold`: float; single-step reference-spot relative-move threshold that arms the entry spike cooldown
- `spike_guard_cooldown_secs`: unsigned integer; entry-block cooldown duration after a reference-spot spike
- `price_to_beat_source`: string; configured source identifier recorded in strategy-input evidence
- `pricing_kurtosis`: float; kurtosis input for binary-oracle pricing
- `theta_decay_factor`: float; non-negative theta decay multiplier
- `forced_flat_stale_chainlink_ms`: unsigned integer; Chainlink staleness forced-flat threshold
- `forced_flat_thin_book_min_liquidity`: float; thin-book forced-flat liquidity threshold
- `lead_agreement_min_corr`: float; minimum lead-market agreement correlation
- `lead_jitter_max_ms`: unsigned integer; maximum lead-market jitter

## 8. Validation Rules

### Structural validation

Must fail if:

- any required field is missing
- any unknown field is present
- a strategy file path is duplicated
- a referenced file does not exist
- a client reference points to a missing client
- a strategy `execution_client_id` points to a data-only client (no `[execution]` block)
- a reference-data `data_client_id` points to a client without `[data]`
- a `[secrets]` block is present without the same client's consuming adapter block
- an SSM parameter path is empty or does not start with `/`
- two listed strategy files declare the same `strategy_instance_id`
- two listed strategy files declare the same `order_id_tag`
- two configured targets declare the same `configured_target_id`
- `signature_type` is not one of the allowed strings
- Polymarket `signature_type = "poly_proxy"` or `signature_type = "poly_gnosis_safe"` is missing a non-zero `funder`
- Polymarket `funder`, when present, is not a `0x`-prefixed 40-hex-character non-zero EVM address
- `target.kind = "rotating_market"` includes fields not valid for rotating-market targets
- `target.kind = "instrument"` is selected before instrument targets are added by a future contract slice
- `target.underlying_asset` is empty, longer than 32 characters, or contains characters outside uppercase ASCII letters, digits, and underscore
- `target.cadence_secs` is not positive or is not divisible by `60`
- `target.cadence_secs` does not have a runtime-contract-defined slug-token mapping
- a field appears under `[clients.<identifier>.data]` or `[clients.<identifier>.execution]` that is not allowed for that client's `venue`
- a Binance reference-data `base_url_ws` uses NautilusTrader's Binance Spot JSON WebSocket host instead of an SBE endpoint or compatible SBE proxy
- archetype-specific parameter sections contain fields not allowed for the declared `strategy_archetype`
- archetype-specific order parameters contain any combination not explicitly allowed for that archetype
- `order_notional_target` or `maximum_position_notional` is not a positive decimal
- `order_notional_target` exceeds `root risk.default_max_notional_per_order`
- `order_notional_target` exceeds `maximum_position_notional`
- `binary_oracle_edge_taker` is missing `[reference_data.primary]`

### Live validation

Live validation behavior, fatal-vs-warning classification, and the full failure-reason taxonomy are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 2 Phase 2.

## 9. Canonical Example: Minimal Live-Trading Pair

This example is structural.
It is not live-valid until the operator supplies real paths, SSM parameters, account identifiers, wallet addresses, a writable catalog directory, and client credentials.

### Root

```toml
schema_version = 1
trader_id = "BOLT-001"

strategy_files = [
  "strategies/configured_updown_main.toml",
]

[runtime]
mode = "Live"
order_execution_mode = "live"

[nautilus]
load_state = true
save_state = true
shutdown_on_error = false
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 0
open_check_lookback_mins = 60
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 0
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
max_order_submit_rate = "40/00:01:00"
max_order_modify_rate = "40/00:01:00"
max_notional_per_order = {}
debug = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/srv/bolt-v2/var/bolt-v3-live/catalog"
required_catalog_prefix = "/srv/bolt-v2"
min_free_bytes = 10737418240
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.data]
base_url_http = "https://clob.polymarket.com" # NT: nautilus_polymarket::config::PolymarketDataClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market" # NT: PolymarketDataClientConfig.base_url_ws
base_url_rtds = "wss://ws-live-data.polymarket.com" # NT: PolymarketDataClientConfig.base_url_rtds
base_url_gamma = "https://gamma-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_gamma
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_data_api
http_timeout_secs = 60 # NT: PolymarketDataClientConfig.http_timeout_secs
ws_timeout_secs = 30 # NT: PolymarketDataClientConfig.ws_timeout_secs
subscribe_new_markets = false # NT: PolymarketDataClientConfig.subscribe_new_markets — forced false in current bolt-v3 scope
new_market_fetch_max_concurrency = 8 # NT: PolymarketDataClientConfig.new_market_fetch_max_concurrency
auto_load_missing_instruments = false # NT: PolymarketDataClientConfig.auto_load_missing_instruments — forced false in current bolt-v3 scope
auto_load_debounce_ms = 250 # NT: PolymarketDataClientConfig.auto_load_debounce_ms
auto_load_max_retries = 12 # NT: PolymarketDataClientConfig.auto_load_max_retries
auto_load_retry_delay_initial_secs = 5 # NT: PolymarketDataClientConfig.auto_load_retry_delay_initial_secs
auto_load_retry_delay_max_secs = 15 # NT: PolymarketDataClientConfig.auto_load_retry_delay_max_secs
resolve_poll_enabled = false # NT: PolymarketDataClientConfig.resolve_poll_enabled
resolve_poll_interval_secs = 30 # NT: PolymarketDataClientConfig.resolve_poll_interval_secs
resolve_poll_grace_secs = 10 # NT: PolymarketDataClientConfig.resolve_poll_grace_secs
resolve_poll_max_wait_secs = 1800 # NT: PolymarketDataClientConfig.resolve_poll_max_wait_secs
update_instruments_interval_mins = 60 # NT: PolymarketDataClientConfig.update_instruments_interval_mins
ws_max_subscriptions = 200 # NT: PolymarketDataClientConfig.ws_max_subscriptions
transport_backend = "sockudo" # NT: PolymarketDataClientConfig.transport_backend

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001" # NT: nautilus_model::identifiers::AccountId
signature_type = "poly_proxy" # NT: nautilus_polymarket::common::enums::SignatureType
funder = "0x1111111111111111111111111111111111111111" # NT: PolymarketExecClientConfig.funder
base_url_http = "https://clob.polymarket.com" # NT: PolymarketExecClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user" # NT: PolymarketExecClientConfig.base_url_ws
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketExecClientConfig.base_url_data_api
http_timeout_secs = 60 # NT: PolymarketExecClientConfig.http_timeout_secs
max_retries = 3 # NT: PolymarketExecClientConfig.max_retries
retry_delay_initial_ms = 250 # NT: PolymarketExecClientConfig.retry_delay_initial_ms
retry_delay_max_ms = 2000 # NT: PolymarketExecClientConfig.retry_delay_max_ms
ack_timeout_secs = 5 # NT: PolymarketExecClientConfig.ack_timeout_secs
fee_cache_ttl_secs = 300 # NT: PolymarketExecClientConfig fee cache TTL
transport_backend = "sockudo" # NT: PolymarketExecClientConfig.transport_backend

[clients.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"

[clients.binance_reference]
venue = "BINANCE"

[clients.binance_reference.data]
product_type = "spot" # NT: nautilus_binance::config::BinanceDataClientConfig.product_type
environment = "mainnet" # NT: BinanceDataClientConfig.environment
base_url_http = "https://api.binance.com" # NT: BinanceDataClientConfig.base_url_http
base_url_ws = "wss://stream-sbe.binance.com/ws" # NT: BinanceDataClientConfig.base_url_ws
spot_market_data_mode = "sbe" # NT: BinanceDataClientConfig.spot_market_data_mode
instrument_status_poll_secs = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs
transport_backend = "sockudo" # NT: BinanceDataClientConfig.transport_backend

[clients.binance_reference.secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
```

### Strategy

```toml
schema_version = 2
strategy_instance_id = "configured_updown_main"
strategy_archetype = "binary_oracle_edge_taker"
order_id_tag = "001"
oms_type = "netting"
use_uuid_client_order_ids = true
use_hyphens_in_client_order_ids = false
external_order_claims = []
manage_contingent_orders = false
manage_gtd_expiry = false
manage_stop = false
market_exit_interval_ms = 100
market_exit_max_attempts = 100
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
execution_client_id = "polymarket_main"
realized_volatility_surface_id = "configured_rv_surface"

[target]
configured_target_id = "configured_updown_target"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "CONFIGURED_ASSET"
cadence_secs = 300
cadence_slug_token = "configuredwindow"
market_selection_rule = "active_or_next"
retry_interval_secs = 5
blocked_after_secs = 60

[reference_data]

[parameters.entry_order]
side = "buy"
position_side = "long"
order_type = "limit"
time_in_force = "fok"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters.exit_order]
side = "sell"
position_side = "long"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters.forced_exit_order]
side = "sell"
position_side = "long"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"

[parameters.runtime]
reference_publish_topic = "platform.runtime.selection.binary_oracle_edge_taker-001"
warmup_tick_count = 20
reentry_cooldown_secs = 30
book_impact_cap_bps = 50
risk_lambda = 0.5
sizing_ev_reference_bps = 500
exit_hysteresis_bps = 25
trade_flow_window_secs = 60
trade_flow_max_samples = 2000
spike_guard_return_threshold = 0.02
spike_guard_cooldown_secs = 30
price_to_beat_source = "chainlink_data_streams.report_at_boundary"
pricing_kurtosis = 3.0
theta_decay_factor = 1.0
forced_flat_stale_chainlink_ms = 10000
forced_flat_thin_book_min_liquidity = 5.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250
```
