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

Both root and strategy TOML files are subject to a pre-parse 1 MiB file-size
guard (`1_048_576` bytes). Files larger than that fail closed before TOML
parsing. This is a resource-exhaustion guard for operator-authored config
files, not a trading-policy parameter.

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
trader_id = "BOLT-001"

strategy_files = [
  "strategies/bitcoin_updown_main.toml",
]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
instance_id = "disabled"
cache = "disabled"
msgbus = "disabled"
portfolio = "disabled"
emulator = "disabled"
streaming = "disabled"
loop_debug = false
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

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
external_client_ids = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_seconds = 0
external_client_ids = []
debug = false
reconciliation = true
reconciliation_startup_delay_seconds = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_milliseconds = 2000
inflight_check_threshold_milliseconds = 5000
inflight_check_retries = 5
open_check_interval_seconds = 0
open_check_lookback_mins = 60
open_check_threshold_milliseconds = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_milliseconds = 100
position_check_interval_seconds = 0
position_check_lookback_mins = 60
position_check_threshold_milliseconds = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_seconds = 0
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"
nt_bypass = false
nt_max_order_submit_rate = "100/00:00:01"
nt_max_order_modify_rate = "100/00:00:01"
nt_max_notional_per_order = {}
nt_debug = false
nt_graceful_shutdown_on_error = false
nt_qsize = 100000

[logging]
standard_output_level = "INFO"
file_level = "INFO"
component_levels = {}
module_levels = {}
credential_module_level = "WARN"
log_components_only = false
is_colored = true
print_config = false
use_tracing = false
bypass_logging = false
file_config = "disabled"
clear_log_file = false
stale_log_source_directory = "/var/lib/bolt"
stale_log_archive_directory = "/var/log/bolt/nautilus-archive"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_milliseconds = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[live_canary]
approval_id = "operator-approved-canary-001"
no_submit_readiness_report_path = "reports/no-submit-readiness.json"
max_no_submit_readiness_report_bytes = 4096
max_live_order_count = 1
max_notional_per_order = "1.00"

[live_canary.operator_evidence]
ssm_manifest_path = "reports/ssm-manifest.redacted.json"
ssm_manifest_sha256 = "<sha256>"
strategy_input_evidence_path = "reports/strategy-input-evidence.json"
strategy_input_evidence_sha256 = "<sha256>"
canary_evidence_path = "reports/tiny-canary-evidence.json"
approval_not_before_unix_seconds = 1770000000
approval_not_after_unix_seconds = 1770000300
approval_nonce_path = "reports/approval-nonce.json"
approval_nonce_sha256 = "<sha256>"
approval_consumption_path = "reports/approval-consumed.json"
decision_evidence_path = "reports/decision-evidence.json"
client_order_id_hash = "<sha256>"
venue_order_id_hash = "<sha256>"
nt_submit_event_path = "reports/runtime/nt-submit-event.json"
venue_order_state_path = "reports/runtime/venue-order-state.json"
strategy_cancel_path = "reports/runtime/strategy-cancel.json"
restart_reconciliation_path = "reports/runtime/restart-reconciliation.json"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com" # NT: nautilus_polymarket::config::PolymarketDataClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market" # NT: PolymarketDataClientConfig.base_url_ws
base_url_gamma = "https://gamma-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_gamma
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_data_api
http_timeout_seconds = 60 # NT: PolymarketDataClientConfig.http_timeout_secs
ws_timeout_seconds = 30 # NT: PolymarketDataClientConfig.ws_timeout_secs
subscribe_new_markets = false # NT: PolymarketDataClientConfig.subscribe_new_markets — forced false in current bolt-v3 scope
auto_load_missing_instruments = false # NT: PolymarketDataClientConfig.auto_load_missing_instruments
update_instruments_interval_minutes = 60 # NT: PolymarketDataClientConfig.update_instruments_interval_mins
websocket_max_subscriptions_per_connection = 200 # NT: PolymarketDataClientConfig.ws_max_subscriptions
auto_load_debounce_milliseconds = 250 # NT: PolymarketDataClientConfig.auto_load_debounce_ms
transport_backend = "tungstenite" # NT: PolymarketDataClientConfig.transport_backend

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001" # NT: nautilus_model::identifiers::AccountId
signature_type = "poly_proxy" # NT: nautilus_polymarket::common::enums::SignatureType
funder_address = "0x1111111111111111111111111111111111111111" # NT: PolymarketExecClientConfig.funder
base_url_http = "https://clob.polymarket.com" # NT: PolymarketExecClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user" # NT: PolymarketExecClientConfig.base_url_ws
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketExecClientConfig.base_url_data_api
http_timeout_seconds = 60 # NT: PolymarketExecClientConfig.http_timeout_secs
max_retries = 3 # NT: PolymarketExecClientConfig.max_retries
retry_delay_initial_milliseconds = 250 # NT: PolymarketExecClientConfig.retry_delay_initial_ms
retry_delay_max_milliseconds = 2000 # NT: PolymarketExecClientConfig.retry_delay_max_ms
ack_timeout_seconds = 5 # NT: PolymarketExecClientConfig.ack_timeout_secs
transport_backend = "tungstenite" # NT: PolymarketExecClientConfig.transport_backend

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"

[venues.binance_reference]
kind = "binance"

[venues.binance_reference.data]
product_types = ["spot"] # NT: nautilus_binance::config::BinanceDataClientConfig.product_types
environment = "mainnet" # NT: BinanceDataClientConfig.environment
base_url_http = "https://api.binance.com" # NT: BinanceDataClientConfig.base_url_http
base_url_ws = "wss://stream.binance.com:9443/ws" # NT: BinanceDataClientConfig.base_url_ws
instrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs
transport_backend = "tungstenite" # NT: BinanceDataClientConfig.transport_backend

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
- allowed NautilusTrader `Environment` values:
  - `backtest`
  - `sandbox`
  - `live`
- maps directly to `Environment::Backtest`, `Environment::Sandbox`, or `Environment::Live`

### `[nautilus]`

The fields below map to top-level NautilusTrader `LiveNodeConfig` values. `data_clients` and `exec_clients` are derived from configured venues through the Bolt-v3 provider Adapter registration path, not from root-level NT client maps.

#### `load_state`

- type: boolean
- required: yes
- maps to Nautilus live-node state loading

#### `save_state`

- type: boolean
- required: yes
- maps to Nautilus live-node state saving

#### `instance_id`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LiveNodeConfig.instance_id = None`

#### `cache`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LiveNodeConfig.cache = None`

#### `msgbus`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LiveNodeConfig.msgbus = None`; the pinned NT Rust live runtime rejects configured message-bus config

#### `portfolio`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LiveNodeConfig.portfolio = None`

#### `emulator`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LiveNodeConfig.emulator = None`; the pinned NT Rust live runtime rejects configured emulator config

#### `streaming`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LiveNodeConfig.streaming = None`; Bolt-v3 capture streaming is configured under `[persistence.streaming]`

#### `loop_debug`

- type: boolean
- required: yes
- allowed values: `false`
- maps to `LiveNodeConfig.loop_debug`; the pinned NT Rust live runtime rejects `true`

#### `timeout_connection_seconds`

- type: positive integer
- required: yes
- bounds the explicit bolt-v3 controlled-connect boundary

#### `timeout_reconciliation_seconds`

- type: positive integer
- required: yes

#### `timeout_portfolio_seconds`

- type: positive integer
- required: yes

#### `timeout_disconnection_seconds`

- type: positive integer
- required: yes
- bounds the explicit bolt-v3 controlled-disconnect boundary

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

### `[nautilus.data_engine]`

All pinned `LiveDataEngineConfig` fields are explicit in TOML and mapped into the NautilusTrader Rust live-node config. Empty `external_client_ids` maps to Nautilus `None`. `time_bars_origins` keys must be Nautilus `BarAggregation` variant strings such as `Minute`, and values are origin offsets in nanoseconds.

Fields rejected by NautilusTrader's current Rust live runtime are still required in TOML at the only accepted value so upstream default drift cannot silently change the built node:

- `graceful_shutdown_on_error = false`
- `qsize` must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, currently `100000` at NT rev `38b912a8b0fe14e4046773973ff46a3b798b1e3e`

| Field | Type / Rule | Maps to |
|---|---|---|
| `time_bars_build_with_no_updates` | boolean | `LiveDataEngineConfig.time_bars_build_with_no_updates` |
| `time_bars_timestamp_on_close` | boolean | `LiveDataEngineConfig.time_bars_timestamp_on_close` |
| `time_bars_skip_first_non_full_bar` | boolean | `LiveDataEngineConfig.time_bars_skip_first_non_full_bar` |
| `time_bars_interval_type` | valid NT `BarIntervalType` string; current baseline `LEFT_OPEN` | `LiveDataEngineConfig.time_bars_interval_type` |
| `time_bars_build_delay` | non-negative integer microseconds | `LiveDataEngineConfig.time_bars_build_delay` |
| `time_bars_origins` | TOML inline table mapping valid NT `BarAggregation` strings to origin offsets in nanoseconds | `LiveDataEngineConfig.time_bars_origins` |
| `validate_data_sequence` | boolean | `LiveDataEngineConfig.validate_data_sequence` |
| `buffer_deltas` | boolean | `LiveDataEngineConfig.buffer_deltas` |
| `emit_quotes_from_book` | boolean | `LiveDataEngineConfig.emit_quotes_from_book` |
| `emit_quotes_from_book_depths` | boolean | `LiveDataEngineConfig.emit_quotes_from_book_depths` |
| `external_client_ids` | array of valid NT client IDs; empty maps to `None` | `LiveDataEngineConfig.external_clients` |
| `debug` | boolean | `LiveDataEngineConfig.debug` |
| `graceful_shutdown_on_error` | must be `false` | `LiveDataEngineConfig.graceful_shutdown_on_error` |
| `qsize` | must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, currently `100000` at NT rev `38b912a8b0fe14e4046773973ff46a3b798b1e3e` | `LiveDataEngineConfig.qsize` |

### `[nautilus.exec_engine]`

All `LiveExecEngineConfig` fields are explicit in TOML and mapped into the pinned NautilusTrader Rust live-node config. For fields documented below as optional, `0` maps to Nautilus `None`; other non-negative fields pass their numeric value through. Empty identifier arrays map to Nautilus `None`.

Fields rejected by NautilusTrader's current Rust live runtime are still required in TOML at the only accepted value so upstream default drift cannot silently change the built node:

- `snapshot_orders = false`
- `snapshot_positions = false`
- `purge_from_database = false`
- `graceful_shutdown_on_error = false`
- `qsize` must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, currently `100000` at NT rev `38b912a8b0fe14e4046773973ff46a3b798b1e3e`

#### `reconciliation_lookback_mins`

- type: non-negative integer
- required: yes
- `0` means unbounded lookback and maps to Nautilus `None`
- any positive value maps to that exact bounded minute count

#### `reconciliation_startup_delay_seconds`

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

#### `position_check_threshold_milliseconds`

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
| `snapshot_positions_interval_seconds` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.snapshot_positions_interval_secs` |
| `external_client_ids` | array of valid NT client IDs; empty maps to `None` | `LiveExecEngineConfig.external_clients` |
| `debug` | boolean | `LiveExecEngineConfig.debug` |
| `reconciliation` | boolean | `LiveExecEngineConfig.reconciliation` |
| `reconciliation_instrument_ids` | array of valid NT instrument IDs; empty maps to `None` | `LiveExecEngineConfig.reconciliation_instrument_ids` |
| `filter_unclaimed_external_orders` | boolean | `LiveExecEngineConfig.filter_unclaimed_external_orders` |
| `filter_position_reports` | boolean | `LiveExecEngineConfig.filter_position_reports` |
| `filtered_client_order_ids` | array of valid NT client order IDs; empty maps to `None` | `LiveExecEngineConfig.filtered_client_order_ids` |
| `generate_missing_orders` | boolean | `LiveExecEngineConfig.generate_missing_orders` |
| `inflight_check_interval_milliseconds` | non-negative integer | `LiveExecEngineConfig.inflight_check_interval_ms` |
| `inflight_check_threshold_milliseconds` | positive integer | `LiveExecEngineConfig.inflight_check_threshold_ms` |
| `inflight_check_retries` | non-negative integer | `LiveExecEngineConfig.inflight_check_retries` |
| `open_check_interval_seconds` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.open_check_interval_secs` |
| `open_check_lookback_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.open_check_lookback_mins` |
| `open_check_threshold_milliseconds` | positive integer | `LiveExecEngineConfig.open_check_threshold_ms` |
| `open_check_missing_retries` | non-negative integer | `LiveExecEngineConfig.open_check_missing_retries` |
| `open_check_open_only` | boolean | `LiveExecEngineConfig.open_check_open_only` |
| `single_order_query_delay_milliseconds` | non-negative integer | `LiveExecEngineConfig.single_order_query_delay_ms` |
| `position_check_interval_seconds` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.position_check_interval_secs` |
| `position_check_lookback_mins` | non-negative integer; NT pins this as `u32`, so `0` passes through as a 0-minute lookback rather than mapping to `None` | `LiveExecEngineConfig.position_check_lookback_mins` |
| `position_check_retries` | non-negative integer | `LiveExecEngineConfig.position_check_retries` |
| `purge_closed_orders_interval_mins` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.purge_closed_orders_interval_mins` |
| `purge_closed_orders_buffer_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.purge_closed_orders_buffer_mins` |
| `purge_closed_positions_interval_mins` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.purge_closed_positions_interval_mins` |
| `purge_closed_positions_buffer_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.purge_closed_positions_buffer_mins` |
| `purge_account_events_interval_mins` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.purge_account_events_interval_mins` |
| `purge_account_events_lookback_mins` | non-negative integer; `0` maps to `None` | `LiveExecEngineConfig.purge_account_events_lookback_mins` |
| `purge_from_database` | must be `false` | `LiveExecEngineConfig.purge_from_database` |
| `own_books_audit_interval_seconds` | non-negative integer; `0` disables the timer | `LiveExecEngineConfig.own_books_audit_interval_secs` |
| `graceful_shutdown_on_error` | must be `false` | `LiveExecEngineConfig.graceful_shutdown_on_error` |
| `qsize` | must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, currently `100000` at NT rev `38b912a8b0fe14e4046773973ff46a3b798b1e3e` | `LiveExecEngineConfig.qsize` |
| `allow_overfills` | boolean | `LiveExecEngineConfig.allow_overfills` |
| `manage_own_order_books` | boolean | `LiveExecEngineConfig.manage_own_order_books` |

### `[risk]`

This section owns both Bolt-v3 strategy-sizing limits and all pinned NautilusTrader live risk-engine fields. All `nt_*` fields are required in TOML and mapped into `LiveRiskEngineConfig`; `default_max_notional_per_order` is the Bolt-v3-owned strategy-sizing cap. Fields under `[nautilus]` do not use the prefix because the section name already carries the NT context.

#### `default_max_notional_per_order`

- type: decimal string
- required: yes
- root-level entity per-order notional cap
- must be greater than zero
- enforced by bolt-v3 strategy validation: each strategy file's `parameters.order_notional_target` must be `<=` this value
- not automatically expanded into NautilusTrader per-instrument maps; `nt_max_notional_per_order` is the explicit NT map when instrument-level caps are intentionally configured

#### `nt_bypass`

- type: boolean
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.bypass`
- must remain `false` for production configurations unless a separately reviewed safety exception is approved

#### `nt_max_order_submit_rate`

- type: rate-limit string in Nautilus `limit/HH:MM:SS` format
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.max_order_submit_rate`

#### `nt_max_order_modify_rate`

- type: rate-limit string in Nautilus `limit/HH:MM:SS` format
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.max_order_modify_rate`

#### `nt_max_notional_per_order`

- type: TOML inline table mapping Nautilus instrument IDs to decimal notional strings
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.max_notional_per_order`
- values must be positive decimal strings
- `{}` means no NT per-instrument cap is configured; Bolt-v3 still enforces `default_max_notional_per_order` at config validation time

#### `nt_debug`

- type: boolean
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.debug`
- current baseline value is `false`

#### `nt_graceful_shutdown_on_error`

- type: boolean
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.graceful_shutdown_on_error`
- must remain `false`; NautilusTrader rejects non-default values on the current Rust live runtime

#### `nt_qsize`

- type: positive integer
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.qsize`
- must equal the pinned NT `LiveRiskEngineConfig::default().qsize` value, currently `100000` at NT rev `38b912a8b0fe14e4046773973ff46a3b798b1e3e`

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

#### `component_levels`

- type: string enum map
- required: yes
- maps to NT `LoggerConfig.component_level`

#### `module_levels`

- type: string enum map
- required: yes
- maps to NT `LoggerConfig.module_level` before credential-module filters are applied

#### `credential_module_level`

- type: string enum
- required: yes
- allowed values: `WARN`, `ERROR`, `OFF`
- maps to credential-log modules supplied by provider bindings

#### `log_components_only`

- type: bool
- required: yes
- maps to NT `LoggerConfig.log_components_only`

#### `is_colored`

- type: bool
- required: yes
- maps to NT `LoggerConfig.is_colored`

#### `print_config`

- type: bool
- required: yes
- maps to NT `LoggerConfig.print_config`

#### `use_tracing`

- type: bool
- required: yes
- maps to NT `LoggerConfig.use_tracing`

#### `bypass_logging`

- type: bool
- required: yes
- maps to NT `LoggerConfig.bypass_logging`

#### `file_config`

- type: string enum
- required: yes
- allowed values: `disabled`
- maps to `LoggerConfig.file_config = None`; the pinned NT Rust live runtime rejects non-disabled file writer config

#### `clear_log_file`

- type: bool
- required: yes
- allowed values: `false`
- maps to NT `LoggerConfig.clear_log_file`; the pinned NT Rust live runtime rejects `true`

#### `stale_log_source_directory`

- type: absolute path string
- required: yes
- source directory for stale NT log sweep before live-node construction

#### `stale_log_archive_directory`

- type: absolute path string
- required: yes
- archive directory for stale NT log files moved before live-node construction

Bolt-v3 installs module-level filters for NT credential log modules from provider-owned bindings. `[logging].credential_module_level` owns the filter level and validation requires `WARN` or stricter because those NT modules log credential-derived material at info-level.

Bolt-v3 maps every pinned NautilusTrader `LoggerConfig` field from TOML before handing the config to `LiveNodeBuilder::from_config`. The stale-log sweep directories are separate from NT `LoggerConfig.file_config`; the pinned Rust live runtime rejects non-disabled `file_config` and `clear_log_file = true`, so TOML must explicitly request `file_config = "disabled"` and `clear_log_file = false`.

### `[persistence]`

#### `catalog_directory`

- type: absolute path string
- required: yes
- local Nautilus catalog root for structured decision events and raw NautilusTrader capture
- persistence behavior and local-evidence requirements are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 9.6, 9.7, and 10

#### `runtime_capture_start_poll_interval_milliseconds`

- type: positive integer
- required: yes
- controls the raw-capture worker poll interval while it is buffering startup messages before the NT runner reports running
- this is TOML-owned so the capture worker does not own a compiled timing policy

There is no `state_directory` in the current bolt-v3 scope. NT's pinned `LiveNodeBuilder` does not expose a state-directory wiring (load/save state are booleans only), so a TOML key would not flow to NT. A future slice may reintroduce this once a supported path exists.

### `[persistence.decision_evidence]`

This section is required.

#### `order_intents_relative_path`

- type: relative path string
- required: yes
- path under `catalog_directory` for bolt-v3 order-intent evidence
- the path is relative so changing `catalog_directory` moves the local evidence root in one place

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

### `[live_canary]`

This section is optional for parse/build-only checks and required before `run_bolt_v3_live_node` starts the NT runner. If it is absent, the bolt-v3 runtime gate fails closed before `LiveNode::run`.

#### `approval_id`

- type: non-empty string
- required: yes when `[live_canary]` is present
- operator approval identifier for the exact canary launch

#### `no_submit_readiness_report_path`

- type: path string
- required: yes when `[live_canary]` is present
- path to a prior no-submit readiness JSON report
- relative paths resolve from the root TOML directory

#### `max_no_submit_readiness_report_bytes`

- type: positive integer
- required: yes when `[live_canary]` is present
- maximum no-submit readiness JSON report size read by the fail-closed gate
- reports larger than this bound reject before JSON parsing

#### `max_live_order_count`

- type: positive integer
- required: yes when `[live_canary]` is present
- approved live canary order-count bound validated before `LiveNode::run`
- the run gate does not count orders; submit-admission code must consume this bound before any live submit

#### `max_notional_per_order`

- type: positive decimal string
- required: yes when `[live_canary]` is present
- approved per-order live canary notional bound validated before `LiveNode::run`
- must be less than or equal to `risk.default_max_notional_per_order`
- the run gate does not submit orders; submit-admission code must consume this bound before any live submit

### `[live_canary.operator_evidence]`

This section is optional for parse/build-only checks and required by the ignored tiny-canary operator harness before live-capital proof. Runtime values are read from TOML, not from a second env-var contract.

#### `ssm_manifest_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- redacted SSM manifest path used for checksum validation

#### `ssm_manifest_sha256`

- type: SHA-256 hex string
- required: yes when `[live_canary.operator_evidence]` is present
- checksum of the redacted SSM manifest

#### `strategy_input_evidence_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- strategy-input evidence path used for checksum validation

#### `strategy_input_evidence_sha256`

- type: SHA-256 hex string
- required: yes when `[live_canary.operator_evidence]` is present
- checksum of strategy-input evidence

#### `canary_evidence_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- output path for redacted tiny-canary evidence

#### `approval_not_before_unix_seconds`

- type: integer Unix seconds
- required: yes when `[live_canary.operator_evidence]` is present
- earliest accepted operator approval time

#### `approval_not_after_unix_seconds`

- type: integer Unix seconds
- required: yes when `[live_canary.operator_evidence]` is present
- latest accepted operator approval time

#### `approval_nonce_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- approval nonce evidence path used for checksum validation

#### `approval_nonce_sha256`

- type: SHA-256 hex string
- required: yes when `[live_canary.operator_evidence]` is present
- checksum of approval nonce evidence

#### `approval_consumption_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- output path for one-shot approval consumption evidence

#### `decision_evidence_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- decision evidence proof path

#### `client_order_id_hash`

- type: SHA-256 hex string
- required: yes when `[live_canary.operator_evidence]` is present
- expected client order id hash for proof joins

#### `venue_order_id_hash`

- type: SHA-256 hex string
- required: yes when `[live_canary.operator_evidence]` is present
- expected venue order id hash for proof joins

#### `nt_submit_event_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- NT submit event proof path under the runtime capture spool

#### `venue_order_state_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- venue order state proof path under the runtime capture spool

#### `strategy_cancel_path`

- type: optional path string
- required: no
- strategy cancel proof path under the runtime capture spool

#### `restart_reconciliation_path`

- type: path string
- required: yes when `[live_canary.operator_evidence]` is present
- restart reconciliation proof path under the runtime capture spool

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
- must be `false` in the current bolt-v3 scope: validation fails closed if set to `true`
- the pinned NautilusTrader Polymarket data client calls `ws_client.subscribe_market(vec![])` from inside its `connect()` when this flag is `true`, which is effectively an all-markets subscription and violates the bolt-v3 controlled-connect boundary
- this flag is forced `false` until the dedicated market-subscription slice owns the controlled-subscribe path

##### `auto_load_missing_instruments`

- type: boolean
- required: yes
- maps directly to `PolymarketDataClientConfig.auto_load_missing_instruments`

##### `update_instruments_interval_minutes`

- type: positive integer
- required: yes
- background Polymarket adapter refresh interval only
- not the sole mechanism keeping current rotating-market data loaded

##### `websocket_max_subscriptions_per_connection`

- type: positive integer
- required: yes

##### `auto_load_debounce_milliseconds`

- type: positive integer
- required: yes

##### `transport_backend`

- type: string enum
- required: yes
- allowed values: `tungstenite`, `sockudo`
- maps directly to the pinned NautilusTrader WebSocket transport backend field
- explicit TOML ownership prevents the adapter from silently inheriting the NT default backend

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

- type: optional string
- required: yes for Polymarket execution when `signature_type` is `poly_proxy` or `poly_gnosis_safe`
- allowed absent for `signature_type = "eoa"`
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

#### `transport_backend`

- type: string enum
- required: yes
- allowed values: `tungstenite`, `sockudo`
- maps directly to the pinned NautilusTrader WebSocket transport backend field
- explicit TOML ownership prevents the adapter from silently inheriting the NT default backend

#### Additional Polymarket execution fields

The current schema also requires these pinned adapter fields to be explicit:

- `base_url_http`
- `base_url_ws`
- `base_url_data_api`
- `http_timeout_seconds`

### `[venues.<identifier>.secrets]`

Presence of `[secrets]` means the venue requires credential resolution.
The block must be consumed by an adapter in the same venue:

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

##### `instrument_status_poll_seconds`

- type: positive integer
- required: yes
- maps to Nautilus `BinanceDataClientConfig.instrument_status_poll_secs`
- bolt-v3 rejects `0` rather than treating it as "polling disabled" so that the cadence stays explicit and NT cannot silently fall back to its own default poll interval

##### `transport_backend`

- type: string enum
- required: yes
- allowed values: `tungstenite`, `sockudo`
- maps directly to the pinned NautilusTrader WebSocket transport backend field
- explicit TOML ownership prevents the adapter from silently inheriting the NT default backend

## 6. Strategy File: Candidate Schema

```toml
schema_version = 1
strategy_instance_id = "bitcoin_updown_main"
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
market_exit_time_in_force = "gtc"
market_exit_reduce_only = true
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
venue = "polymarket_main"

[target]
configured_target_id = "btc_updown_5m"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "BTC"
cadence_seconds = 300
cadence_slug_token = "5m"
market_selection_rule = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60

[reference_data.spot]
venue = "binance_reference"
instrument_id = "BTCUSDT.BINANCE"

[parameters.runtime]
warmup_tick_count = 20
reentry_cooldown_secs = 30
book_impact_cap_bps = 50
risk_lambda = 0.5
exit_hysteresis_bps = 25
vol_window_secs = 600
vol_gap_reset_secs = 60
vol_min_observations = 5
vol_bridge_valid_secs = 30
pricing_kurtosis = 3.0
theta_decay_factor = 1.0
forced_flat_stale_reference_ms = 10000
forced_flat_thin_book_min_liquidity = 5.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250

[parameters.entry_order]
order_type = "limit"
time_in_force = "fok"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters.exit_order]
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

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

This string is resolved by the bolt-v3 archetype runtime binding table.
That binding supplies the NT `StrategyBuilder::kind()` used for strategy
registration.

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
- current allowed value:
  - `netting`
- maps directly to Nautilus `StrategyConfig.oms_type`

#### NT `StrategyConfig` fields

The following required fields map directly to the same-named Nautilus `StrategyConfig` fields:

- `use_uuid_client_order_ids`
- `use_hyphens_in_client_order_ids`
- `external_order_claims`
- `manage_contingent_orders`
- `manage_gtd_expiry`
- `manage_stop`
- `market_exit_interval_ms`
- `market_exit_max_attempts`
- `market_exit_time_in_force`
- `market_exit_reduce_only`
- `log_events`
- `log_commands`
- `log_rejected_due_post_only_as_warning`

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

##### `cadence_slug_token`

- type: string
- required: yes
- must be non-empty
- allowed characters:
  - lowercase ASCII letters
  - digits
- this is the exact slug segment used for the configured `cadence_seconds`

##### `market_selection_rule`

- type: string enum
- current allowed value:
  - `active_or_next`

##### `retry_interval_seconds`

- type: positive integer
- required for rotating-market targets
- configured per strategy; examples use `5`

##### `blocked_after_seconds`

- type: positive integer
- required for rotating-market targets
- configured per strategy; examples use `60`

These fields live in the strategy file because they control that strategy's market-selection behavior.
The schema does not hardcode `BTC`, `ETH`, or `300` as the only supported `updown` target values; those may appear in examples only.

The runtime projection of the strategy-file `[target]` block plus the top-level `venue` field into `configured_updown_target` is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 6.1.

### `[reference_data.<name>]`

This section is optional.

If present:

- each block references a root venue that includes `[data]`
- each block declares the exact NautilusTrader `instrument_id` the strategy subscribes to
- for the current `binary_oracle_edge_taker`, exactly one reference-data role must be configured; the role key is operator-owned

Fields:

#### `venue`

- type: keyed reference string
- required

#### `instrument_id`

- type: string
- required

The TOML value is the literal NautilusTrader `InstrumentId` string.
The field name maps one-to-one to `nautilus_model::identifiers::InstrumentId`; aliases are forbidden.
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

#### `is_post_only`

- type: boolean
- required

#### `is_reduce_only`

- type: boolean
- required

#### `is_quote_quantity`

- type: boolean
- required

Meaning:

- this is the NautilusTrader-native quote/base quantity toggle used by the archetype
- it is not a bolt-owned translation field
- the configured value is passed through to the runtime strategy order factory path

### Order construction for `binary_oracle_edge_taker`

The archetype must not impose a code-owned fixed order combination. These TOML fields are
projected into the runtime strategy config and then into NautilusTrader's order factory.

- `[parameters.entry_order]`
  - creates the configured entry order type and time-in-force
  - supplies configured post-only, reduce-only, and quote-quantity flags

- `[parameters.exit_order]`
  - creates the configured exit order type and time-in-force
  - supplies configured post-only, reduce-only, and quote-quantity flags

Market orders cannot be post-only because NautilusTrader's market-order factory has no
post-only parameter. That protocol constraint fails closed at runtime strategy construction.

### `[parameters.runtime]`

This block is archetype-specific and required for `binary_oracle_edge_taker`.
Every field is TOML-owned and maps into the runtime strategy config.

Required fields:

- `warmup_tick_count`: non-negative integer
- `reentry_cooldown_secs`: non-negative integer
- `book_impact_cap_bps`: non-negative integer
- `risk_lambda`: float
- `exit_hysteresis_bps`: integer
- `vol_window_secs`: non-negative integer
- `vol_gap_reset_secs`: non-negative integer
- `vol_min_observations`: non-negative integer
- `vol_bridge_valid_secs`: non-negative integer
- `pricing_kurtosis`: float
- `theta_decay_factor`: float
- `forced_flat_stale_reference_ms`: non-negative integer
- `forced_flat_thin_book_min_liquidity`: float
- `lead_agreement_min_corr`: float
- `lead_jitter_max_ms`: non-negative integer

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
- a `[secrets]` block is present without the same venue-kind's consuming adapter block
- an SSM parameter path is empty or does not start with `/`
- two listed strategy files declare the same `strategy_instance_id`
- two listed strategy files declare the same `order_id_tag`
- two configured targets declare the same `configured_target_id`
- `signature_type` is not one of the allowed strings
- Polymarket `signature_type = "poly_proxy"` or `signature_type = "poly_gnosis_safe"` is missing a non-zero `funder_address`
- Polymarket `funder_address`, when present, is not a `0x`-prefixed 40-hex-character non-zero EVM address
- `target.kind = "rotating_market"` includes fields not valid for rotating-market targets
- `target.kind = "instrument"` is selected before instrument targets are added by a future contract slice
- `target.underlying_asset` is empty or contains characters outside uppercase ASCII letters, digits, and underscore
- `target.cadence_seconds` is not positive
- `target.cadence_slug_token` is empty or contains characters outside lowercase ASCII letters and digits
- a field appears under `[venues.<identifier>.data]` or `[venues.<identifier>.execution]` that is not allowed for that venue `kind`
- archetype-specific parameter sections contain fields not allowed for the declared `strategy_archetype`
- archetype-specific order parameters contain any combination not explicitly allowed for that archetype
- `order_notional_target` exceeds `root risk.default_max_notional_per_order`
- `binary_oracle_edge_taker` has zero or more than one `[reference_data.<role>]` block

### Live validation

Live validation behavior, fatal-vs-warning classification, and the full failure-reason taxonomy are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 2 Phase 2.

## 9. Canonical Example: Minimal Live-Trading Pair

This example is structural.
It is not live-valid until the operator supplies real paths, SSM parameters, account identifiers, wallet addresses, a writable catalog directory, and venue credentials.

### Root

```toml
schema_version = 1
trader_id = "BOLT-001"

strategy_files = [
  "strategies/bitcoin_updown_main.toml",
]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
instance_id = "disabled"
cache = "disabled"
msgbus = "disabled"
portfolio = "disabled"
emulator = "disabled"
streaming = "disabled"
loop_debug = false
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

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
external_client_ids = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_seconds = 0
external_client_ids = []
debug = false
reconciliation = true
reconciliation_startup_delay_seconds = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_milliseconds = 2000
inflight_check_threshold_milliseconds = 5000
inflight_check_retries = 5
open_check_interval_seconds = 0
open_check_lookback_mins = 60
open_check_threshold_milliseconds = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_milliseconds = 100
position_check_interval_seconds = 0
position_check_lookback_mins = 60
position_check_threshold_milliseconds = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_seconds = 0
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"
nt_bypass = false
nt_max_order_submit_rate = "100/00:00:01"
nt_max_order_modify_rate = "100/00:00:01"
nt_max_notional_per_order = {}
nt_debug = false
nt_graceful_shutdown_on_error = false
nt_qsize = 100000

[logging]
standard_output_level = "INFO"
file_level = "INFO"
component_levels = {}
module_levels = {}
credential_module_level = "WARN"
log_components_only = false
is_colored = true
print_config = false
use_tracing = false
bypass_logging = false
file_config = "disabled"
clear_log_file = false
stale_log_source_directory = "/var/lib/bolt"
stale_log_archive_directory = "/var/log/bolt/nautilus-archive"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_milliseconds = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[live_canary]
approval_id = "operator-approved-canary-001"
no_submit_readiness_report_path = "reports/no-submit-readiness.json"
max_no_submit_readiness_report_bytes = 4096
max_live_order_count = 1
max_notional_per_order = "1.00"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com" # NT: nautilus_polymarket::config::PolymarketDataClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market" # NT: PolymarketDataClientConfig.base_url_ws
base_url_gamma = "https://gamma-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_gamma
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_data_api
http_timeout_seconds = 60 # NT: PolymarketDataClientConfig.http_timeout_secs
ws_timeout_seconds = 30 # NT: PolymarketDataClientConfig.ws_timeout_secs
subscribe_new_markets = false # NT: PolymarketDataClientConfig.subscribe_new_markets — forced false in current bolt-v3 scope
auto_load_missing_instruments = false # NT: PolymarketDataClientConfig.auto_load_missing_instruments
update_instruments_interval_minutes = 60 # NT: PolymarketDataClientConfig.update_instruments_interval_mins
websocket_max_subscriptions_per_connection = 200 # NT: PolymarketDataClientConfig.ws_max_subscriptions
auto_load_debounce_milliseconds = 250 # NT: PolymarketDataClientConfig.auto_load_debounce_ms
transport_backend = "tungstenite" # NT: PolymarketDataClientConfig.transport_backend

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001" # NT: nautilus_model::identifiers::AccountId
signature_type = "poly_proxy" # NT: nautilus_polymarket::common::enums::SignatureType
funder_address = "0x1111111111111111111111111111111111111111" # NT: PolymarketExecClientConfig.funder
base_url_http = "https://clob.polymarket.com" # NT: PolymarketExecClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user" # NT: PolymarketExecClientConfig.base_url_ws
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketExecClientConfig.base_url_data_api
http_timeout_seconds = 60 # NT: PolymarketExecClientConfig.http_timeout_secs
max_retries = 3 # NT: PolymarketExecClientConfig.max_retries
retry_delay_initial_milliseconds = 250 # NT: PolymarketExecClientConfig.retry_delay_initial_ms
retry_delay_max_milliseconds = 2000 # NT: PolymarketExecClientConfig.retry_delay_max_ms
ack_timeout_seconds = 5 # NT: PolymarketExecClientConfig.ack_timeout_secs
transport_backend = "tungstenite" # NT: PolymarketExecClientConfig.transport_backend

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"

[venues.binance_reference]
kind = "binance"

[venues.binance_reference.data]
product_types = ["spot"] # NT: nautilus_binance::config::BinanceDataClientConfig.product_types
environment = "mainnet" # NT: BinanceDataClientConfig.environment
base_url_http = "https://api.binance.com" # NT: BinanceDataClientConfig.base_url_http
base_url_ws = "wss://stream.binance.com:9443/ws" # NT: BinanceDataClientConfig.base_url_ws
instrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs
transport_backend = "tungstenite" # NT: BinanceDataClientConfig.transport_backend

[venues.binance_reference.secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
```

### Strategy

```toml
schema_version = 1
strategy_instance_id = "bitcoin_updown_main"
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
market_exit_time_in_force = "gtc"
market_exit_reduce_only = true
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
venue = "polymarket_main"

[target]
configured_target_id = "btc_updown_5m"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "BTC"
cadence_seconds = 300
cadence_slug_token = "5m"
market_selection_rule = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60

[reference_data.spot]
venue = "binance_reference"
instrument_id = "BTCUSDT.BINANCE"

[parameters.runtime]
warmup_tick_count = 20
reentry_cooldown_secs = 30
book_impact_cap_bps = 50
risk_lambda = 0.5
exit_hysteresis_bps = 25
vol_window_secs = 600
vol_gap_reset_secs = 60
vol_min_observations = 5
vol_bridge_valid_secs = 30
pricing_kurtosis = 3.0
theta_decay_factor = 1.0
forced_flat_stale_reference_ms = 10000
forced_flat_thin_book_min_liquidity = 5.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250

[parameters.entry_order]
order_type = "limit"
time_in_force = "fok"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters.exit_order]
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = false
is_quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"
```
