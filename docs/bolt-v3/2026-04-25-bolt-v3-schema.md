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
trader_id = "BOLT-001"

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

[persistence]
catalog_directory = "/var/lib/bolt/catalog"

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
update_instruments_interval_minutes = 60 # NT: PolymarketDataClientConfig.update_instruments_interval_mins
websocket_max_subscriptions_per_connection = 200 # NT: PolymarketDataClientConfig.ws_max_subscriptions

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
- current allowed value for live trading:
  - `live`
- any other value fails validation

### `[nautilus]`

The fields below map to top-level NautilusTrader `LiveNodeConfig` values. Top-level `LiveNodeConfig` surfaces not represented here are intentionally disabled or empty in the Bolt-v3 builder path (`instance_id`, `cache`, `msgbus`, `portfolio`, `emulator`, `streaming`, `loop_debug`, `data_clients`, and `exec_clients`). They are not inherited from `LiveNodeConfig::default()`.

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

Bolt-v3 also installs unconditional module-level filters that suppress NT's credential info logs from `nautilus_polymarket::common::credential` and `nautilus_binance::common::credential` to `WARN`, regardless of `standard_output_level` and `file_level`. These two NT modules log credential-derived material at info-level (Polymarket address/funder/api-key prefixes; Binance auto-detected key type), so bolt-v3 forces them lower than the root level rather than letting an `INFO` root level surface those prefixes in stdout or the file writer.

Bolt-v3 sets every pinned NautilusTrader `LoggerConfig` field explicitly before handing the config to `LiveNodeBuilder::from_config`. TOML owns `stdout_level` and `fileout_level`; bolt-v3 owns the credential module filters; `component_level` is empty, `log_components_only = false`, `is_colored = true`, `print_config = false`, `use_tracing = false`, and `bypass_logging = false`.

There is no separate `log_directory` knob in the current bolt-v3 scope. Bolt-v3 hands the complete `LoggerConfig` to NT through `LiveNodeBuilder::from_config`; the file-writer directory is owned by NT's `init_logging` path which bolt-v3 does not yet wire. `file_config` remains `None` and `clear_log_file` remains `false`; NT's pinned Rust live runtime rejects non-disabled values for those fields. A TOML field for either value would be a no-op or an invalid runtime request, so the schema deliberately omits it.

### `[persistence]`

#### `catalog_directory`

- type: absolute path string
- required: yes
- local Nautilus catalog root for structured decision events and raw NautilusTrader capture
- persistence behavior and local-evidence requirements are defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Sections 9.6, 9.7, and 10

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

### Phase 8 operator-harness evidence envelope

The `[live_canary]` TOML block is necessary but not sufficient for the one tiny-capital canary operator harness. Before live runner entry, the ignored Phase 8 harness also requires an operator-supplied evidence envelope through these environment fields. Values are evidence paths, sha256s, timestamps, or hashed identifiers; do not put secret values in these fields.

#### Approval and preflight fields

- `BOLT_V3_PHASE8_HEAD_SHA`: exact commit SHA approved for the attempt
- `BOLT_V3_PHASE8_ROOT_TOML_PATH`: approved root TOML path
- `BOLT_V3_PHASE8_ROOT_TOML_SHA256`: sha256 of the approved root TOML
- `BOLT_V3_PHASE8_SSM_MANIFEST_PATH`: redacted SSM path manifest evidence
- `BOLT_V3_PHASE8_SSM_MANIFEST_SHA256`: sha256 of the redacted SSM manifest evidence
- `BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_PATH`: strategy-input safety evidence path
- `BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_SHA256`: sha256 of the strategy-input safety evidence
- `BOLT_V3_PHASE8_FINANCIAL_ENVELOPE_PATH`: financial-envelope evidence path
- `BOLT_V3_PHASE8_FINANCIAL_ENVELOPE_SHA256`: sha256 of the financial-envelope evidence
- `BOLT_V3_PHASE8_PRE_RUN_STATE_PATH`: pre-run host/account/market/funding/runner/egress state evidence path
- `BOLT_V3_PHASE8_PRE_RUN_STATE_SHA256`: sha256 of the pre-run state evidence
- The pre-run state evidence JSON must include sha256 bindings for the host clock-skew proof, venue account-state proof, market-state proof, funding/margin proof, single-runner lock proof, egress-identity proof, CLOB V2 signing/collateral/fee proofs, and release-manifest proof.
- `BOLT_V3_PHASE8_ABORT_PLAN_PATH`: operator abort/panic plan evidence path
- `BOLT_V3_PHASE8_ABORT_PLAN_SHA256`: sha256 of the abort/panic plan evidence
- `BOLT_V3_PHASE8_OPERATOR_APPROVAL_ID`: explicit operator approval identifier
- `BOLT_V3_PHASE8_APPROVAL_NOT_BEFORE_UNIX_SECONDS`: earliest allowed approval-consumption time
- `BOLT_V3_PHASE8_APPROVAL_NOT_AFTER_UNIX_SECONDS`: latest allowed approval-consumption time; must be greater than `BOLT_V3_PHASE8_APPROVAL_NOT_BEFORE_UNIX_SECONDS`
- `BOLT_V3_PHASE8_APPROVAL_NONCE_PATH`: one-shot approval nonce evidence path
- `BOLT_V3_PHASE8_APPROVAL_NONCE_SHA256`: sha256 of the approval nonce evidence
- `BOLT_V3_PHASE8_APPROVAL_CONSUMPTION_PATH`: path atomically created when the approval is consumed
- `BOLT_V3_PHASE8_EVIDENCE_PATH`: redacted canary evidence output path

#### Live-result fields

- `BOLT_V3_PHASE8_DECISION_EVIDENCE_PATH`: persisted decision evidence path
- `BOLT_V3_PHASE8_CLIENT_ORDER_ID_HASH`: client order identifier hash
- `BOLT_V3_PHASE8_VENUE_ORDER_ID_HASH`: venue order identifier hash
- `BOLT_V3_PHASE8_NT_SUBMIT_EVENT_PATH`: NT submit-event evidence path
- `BOLT_V3_PHASE8_VENUE_ORDER_STATE_PATH`: venue accept/fill/reject evidence path
- `BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH`: optional strategy-driven cancel evidence path when an order remains open
- `BOLT_V3_PHASE8_RESTART_RECONCILIATION_PATH`: restart reconciliation evidence path
- `BOLT_V3_PHASE8_POST_RUN_HYGIENE_PATH`: post-run raw-secret residue scan and retention/purge evidence path

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

## 6. Strategy File: Candidate Schema

```toml
schema_version = 1
strategy_instance_id = "bitcoin_updown_main"
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

[parameters.runtime]
reference_publish_topic = "platform.runtime.selection.binary_oracle_edge_taker-001"
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
- for the current `binary_oracle_edge_taker`, the required role name is `primary`

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
- for the current `binary_oracle_edge_taker` archetype, the only allowed value is `false`

### Current valid order combinations for `binary_oracle_edge_taker`

To avoid hidden policy, the current archetype supports only these combinations:

- `[parameters.entry_order]`
  - `order_type = "limit"`
  - `time_in_force = "fok"`
  - `is_post_only = false`
  - `is_reduce_only = false`
  - `is_quote_quantity = false`

- `[parameters.exit_order]`
  - `order_type = "market"`
  - `time_in_force = "ioc"`
  - `is_post_only = false`
  - `is_reduce_only = false`
  - `is_quote_quantity = false`

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

#### `[parameters.runtime]`

- type: table
- required for `binary_oracle_edge_taker`
- all fields are required and unknown fields are rejected
- runtime strategy configuration consumed by the Rust strategy registration path
- `book_impact_cap_bps` is also bound into the Phase 8 financial-envelope evidence and must match the loaded TOML before a tiny-capital canary proof can be written

Runtime fields:

- `reference_publish_topic`: string; reference-data topic consumed by the runtime strategy
- `warmup_tick_count`: unsigned integer; fresh-reference warmup count before entry is allowed
- `reentry_cooldown_secs`: unsigned integer; cooldown after an entry attempt
- `book_impact_cap_bps`: unsigned integer; maximum allowed book-impact basis points for order construction and Phase 8 financial-envelope proof
- `risk_lambda`: float; sizing risk coefficient
- `exit_hysteresis_bps`: integer; exit hysteresis threshold
- `vol_window_secs`: unsigned integer; realized-volatility window
- `vol_gap_reset_secs`: unsigned integer; gap that resets volatility history
- `vol_min_observations`: unsigned integer; minimum observations before volatility is live
- `vol_bridge_valid_secs`: unsigned integer; maximum bridge age for volatility input
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
- a venue reference points to a missing venue
- a strategy `venue` points to a data-only venue
- a reference-data venue points to a venue without `[data]`
- more than one `[venues.<identifier>]` block declares the same `kind` in the current one-venue-per-kind slice
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

[persistence]
catalog_directory = "/var/lib/bolt/catalog"

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
update_instruments_interval_minutes = 60 # NT: PolymarketDataClientConfig.update_instruments_interval_mins
websocket_max_subscriptions_per_connection = 200 # NT: PolymarketDataClientConfig.ws_max_subscriptions

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

[parameters.runtime]
reference_publish_topic = "platform.runtime.selection.binary_oracle_edge_taker-001"
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
forced_flat_stale_chainlink_ms = 10000
forced_flat_thin_book_min_liquidity = 5.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250
```
