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
graceful_shutdown_on_error = false
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
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
bypass = false
max_order_submit_rate = "100/00:00:01"
max_order_modify_rate = "100/00:00:01"
max_notional_per_order = {}
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[live_canary]
approval_id = "operator-approved-canary-001"
no_submit_readiness_report_path = "reports/no-submit-readiness.json"
max_no_submit_readiness_report_bytes = 65536
readiness_report_max_age_seconds = 300
reference_quote_max_age_seconds = 10
reference_quote_wait_timeout_seconds = 20
reference_quote_probe_actor_id = "no-submit-reference-quote-probe"
reference_quote_probe_log_events = true
reference_quote_probe_log_commands = true
max_live_order_count = 1
max_notional_per_order = "1.00"

[live_canary.operator_evidence]
head_sha = "0123456789abcdef0123456789abcdef01234567"
max_operator_evidence_file_bytes = 65536
approval_consumption_max_age_seconds = 60
approval_envelope_path = "operator-evidence/approval-envelope.json"
approval_envelope_sha256 = "9999999999999999999999999999999999999999999999999999999999999999"
ssm_manifest_path = "operator-evidence/ssm-manifest.json"
ssm_manifest_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
strategy_input_evidence_path = "operator-evidence/strategy-input.json"
strategy_input_evidence_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
financial_envelope_path = "operator-evidence/financial-envelope.json"
financial_envelope_sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
pre_run_state_path = "operator-evidence/pre-run-state.json"
pre_run_state_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
abort_plan_path = "operator-evidence/abort-plan.json"
abort_plan_sha256 = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
canary_evidence_path = "operator-evidence/canary-evidence.json"
# Set this window immediately before an approved canary run. It must cover two
# operator-evidence validation rounds plus report read, parse, and validation.
approval_not_before_unix_seconds = 1893456000
approval_not_after_unix_seconds = 1893456300
approval_nonce_path = "operator-evidence/approval-nonce.json"
approval_nonce_sha256 = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
approval_consumption_path = "operator-evidence/approval-consumed.json"
decision_evidence_path = "operator-evidence/decision-evidence.jsonl"
nt_submit_event_path = "operator-evidence/nt-submit-event.json"
venue_order_state_path = "operator-evidence/venue-order-state.json"
restart_reconciliation_path = "operator-evidence/restart-reconciliation.json"
post_run_hygiene_path = "operator-evidence/post-run-hygiene.json"

[aws]
region = "eu-west-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.data]
base_url_http = "https://clob.polymarket.com" # NT: nautilus_polymarket::config::PolymarketDataClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market" # NT: PolymarketDataClientConfig.base_url_ws
base_url_gamma = "https://gamma-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_gamma
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_data_api
http_timeout_secs = 60 # NT: PolymarketDataClientConfig.http_timeout_secs
ws_timeout_secs = 30 # NT: PolymarketDataClientConfig.ws_timeout_secs
subscribe_new_markets = false # NT: PolymarketDataClientConfig.subscribe_new_markets — forced false in current bolt-v3 scope
auto_load_missing_instruments = false # NT: PolymarketDataClientConfig.auto_load_missing_instruments — forced false in current bolt-v3 scope
auto_load_debounce_ms = 250 # NT: PolymarketDataClientConfig.auto_load_debounce_ms
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
product_types = ["spot"] # NT: nautilus_binance::config::BinanceDataClientConfig.product_types
environment = "mainnet" # NT: BinanceDataClientConfig.environment
base_url_http = "https://api.binance.com" # NT: BinanceDataClientConfig.base_url_http
base_url_ws = "wss://stream-sbe.binance.com/ws" # NT: BinanceDataClientConfig.base_url_ws
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

Fields rejected by NautilusTrader's current Rust live runtime are still required in TOML at the only accepted value so upstream default drift cannot silently change the built node:

- `graceful_shutdown_on_error = false`
- `qsize` must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, currently `100000` at NT rev `7c2aafb30fb143069c915a3f2057bb12174405f6`

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
| `graceful_shutdown_on_error` | must be `false` | `LiveDataEngineConfig.graceful_shutdown_on_error` |
| `qsize` | must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, currently `100000` at NT rev `7c2aafb30fb143069c915a3f2057bb12174405f6` | `LiveDataEngineConfig.qsize` |

### `[nautilus.exec_engine]`

All `LiveExecEngineConfig` fields are explicit in TOML and mapped into the pinned NautilusTrader Rust live-node config. For fields documented below as optional, `0` maps to Nautilus `None`; other non-negative fields pass their numeric value through. Empty identifier arrays map to Nautilus `None`.

Fields rejected by NautilusTrader's current Rust live runtime are still required in TOML at the only accepted value so upstream default drift cannot silently change the built node:

- `snapshot_orders = false`
- `snapshot_positions = false`
- `purge_from_database = false`
- `graceful_shutdown_on_error = false`
- `qsize` must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, currently `100000` at NT rev `7c2aafb30fb143069c915a3f2057bb12174405f6`

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
| `graceful_shutdown_on_error` | must be `false` | `LiveExecEngineConfig.graceful_shutdown_on_error` |
| `qsize` | must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, currently `100000` at NT rev `7c2aafb30fb143069c915a3f2057bb12174405f6` | `LiveExecEngineConfig.qsize` |
| `allow_overfills` | boolean | `LiveExecEngineConfig.allow_overfills` |
| `manage_own_order_books` | boolean | `LiveExecEngineConfig.manage_own_order_books` |

### `[risk]`

This section owns both Bolt-v3 strategy-sizing limits and all pinned NautilusTrader live risk-engine fields. All `nt_*` fields are required in TOML and mapped into `LiveRiskEngineConfig`; `default_max_notional_per_order` is the Bolt-v3-owned strategy-sizing cap. Fields under `[nautilus]` do not use the prefix because the section name already carries the NT context.

#### `default_max_notional_per_order`

- type: decimal string
- required: yes
- root-level entity per-order notional cap
- enforced by bolt-v3 strategy validation: each strategy file's `parameters.order_notional_target` must be `<=` this value
- not automatically expanded into NautilusTrader per-instrument maps; `risk.nautilus.max_notional_per_order` is the explicit NT map when instrument-level caps are intentionally configured

#### `bypass` (inside `[risk.nautilus]`)

- type: boolean
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.bypass`
- must remain `false` for production configurations unless a separately reviewed safety exception is approved

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

#### `graceful_shutdown_on_error` (inside `[risk.nautilus]`)

- type: boolean
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.graceful_shutdown_on_error`
- must remain `false`; NautilusTrader rejects non-default values on the current Rust live runtime

#### `qsize` (inside `[risk.nautilus]`)

- type: positive integer
- required: yes
- maps to Nautilus `LiveRiskEngineConfig.qsize`
- must equal the pinned NT `LiveRiskEngineConfig::default().qsize` value, currently `100000` at NT rev `7c2aafb30fb143069c915a3f2057bb12174405f6`

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

Decision-evidence JSONL records use `schema_version = 5` for `order_intent`, `admission_decision`, and `strategy_input_snapshot` envelopes.
Each line is a single JSON object with `schema_version`, `recorded_at_utc_ns`, `gate_version`, `gate_id`, `kind`, and either `intent`, `decision`, or `snapshot`.
The `kind` field is `order_intent` for `intent` payloads and `admission_decision` for `decision` payloads.
`order_intent` payloads carry the configured strategy/order identity plus compiled NT order semantics under `order_fields`.
`admission_decision` payloads carry the submit-admission gate decision for the same `client_order_id`.
`strategy_input_snapshot` payloads carry source-bound entry decision inputs captured before order-intent recording.

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

#### `readiness_report_max_age_seconds`

- type: positive integer
- required: yes when `[live_canary]` is present
- maximum accepted age for the referenced no-submit readiness report at late gate evaluation time, after report read and parse
- reports older than this bound reject before live canary admission can arm
- operators must leave headroom for report I/O and parse latency; effective headroom is lower than the raw cap by that latency

#### `reference_quote_max_age_seconds`

- type: positive integer
- required: yes when `[live_canary]` is present
- maximum accepted age for each configured no-submit reference quote at readiness evaluation time
- cache-only instrument-ID membership is not accepted as freshness evidence

#### `reference_quote_wait_timeout_seconds`

- type: positive integer
- required: yes when `[live_canary]` is present
- maximum time the no-submit readiness runner waits for quote evidence from configured reference-data subscriptions before it stops the runner and fails closed
- this timeout does not authorize order submission, cancellation, or broader market-data subscriptions

#### `reference_quote_probe_actor_id`

- type: non-empty ASCII actor identifier string without surrounding whitespace
- required: yes when `[live_canary]` is present
- NT `DataActorConfig.actor_id` used by the no-submit reference quote probe
- this is an operator-visible runtime identifier and must be TOML-owned

#### `reference_quote_probe_log_events`

- type: boolean
- required: yes when `[live_canary]` is present
- NT `DataActorConfig.log_events` value used by the no-submit reference quote probe

#### `reference_quote_probe_log_commands`

- type: boolean
- required: yes when `[live_canary]` is present
- NT `DataActorConfig.log_commands` value used by the no-submit reference quote probe

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

These environment names belong to the ignored operator harness, not the production `src/bolt_v3_*` runtime literal audit. The production runtime literal verifier intentionally scans only production bolt-v3 sources.

### `[live_canary.operator_evidence]`

This section is required when `[live_canary]` is present. The production live canary gate treats `approval_envelope_path`, the sha256-bound pre-run artifacts, and `approval_consumption_path` as read-only operator evidence and rejects non-regular files, symlinks, directories, unreadable files, or files larger than `max_operator_evidence_file_bytes` before hashing or parsing. Relative paths resolve from the root TOML directory. Remaining output/result paths are required non-empty binding strings; `strategy_cancel_path` is additionally bound by `strategy_cancel_path_hash` in approval-consumption proof when configured, and later operator evidence validates produced contents.

Required control fields:

- `head_sha`: 40-character lowercase commit SHA for the exact approved head; it must match the build-owned head captured at compile time, and the approval-consumption proof must carry the same `head_sha`
- `max_operator_evidence_file_bytes`: positive integer cap applied to every operator evidence file read by the gate: `approval_envelope_path`, sha256-bound pre-run artifacts, and `approval_consumption_path`
- `approval_consumption_max_age_seconds`: positive integer maximum age between `consumed_unix_secs` and gate evaluation time

The static operator-artifact manifest is an input to packet assembly, not the final provenance authority. `assemble_operator_packet_from_static_manifest` refuses a manifest with non-empty `blockers`, a `config_bundle_checksum` that differs from the currently loaded TOML bundle, missing required artifact refs, configured path/hash drift, or artifact-file SHA drift. When those checks pass, the assembler writes `approval-envelope.json` using the same non-circular schema parsed by the live canary gate and writes an `operator-evidence-packet.json` containing the `[live_canary.operator_evidence]` path/SHA fields to copy into TOML. The approval-envelope JSON includes `schema_version = 1`, `record_kind = "phase8_operator_approval_envelope"`, `head_sha`, `ssm_manifest_sha256`, `strategy_input_evidence_sha256`, `financial_envelope_sha256`, `pre_run_state_sha256`, `abort_plan_sha256`, `approval_id_hash`, `approval_nonce_sha256`, `approval_not_before_unix_secs`, `approval_not_after_unix_secs`, `canary_evidence_path_hash`, and optional `strategy_cancel_path_hash`. It must not contain `root_toml_sha256`, `approval_envelope_sha256`, `config_bundle_checksum`, raw approval id, raw nonce material, raw SSM paths, or secret values.

The approval-consumption JSON at `approval_consumption_path` must be a JSON object with `schema_version = 1`, `record_kind = "phase8_operator_approval_consumption"`, `head_sha`, `root_toml_sha256`, all configured evidence sha256 fields including `approval_envelope_sha256`, `approval_id_hash`, `approval_not_before_unix_secs`, `approval_not_after_unix_secs`, `canary_evidence_path_hash`, optional `strategy_cancel_path_hash` when `strategy_cancel_path` is configured, and `consumed_unix_secs`. The gate compares `head_sha` to both TOML operator evidence and the build-owned head captured at compile time. The gate computes `root_toml_sha256` from the loaded root TOML path at evaluation time and compares it to the proof; this value is not configured in TOML because hashing the file into itself would be circular.

#### Approval and preflight fields

- `BOLT_V3_PHASE8_HEAD_SHA`: exact commit SHA approved for the attempt
- `BOLT_V3_PHASE8_ROOT_TOML_PATH`: approved root TOML path; the harness computes its sha256 from this file and reads `approval_envelope_sha256` from `[live_canary.operator_evidence]`
- `BOLT_V3_PHASE8_SSM_MANIFEST_PATH`: redacted SSM path manifest evidence
- `BOLT_V3_PHASE8_SSM_MANIFEST_SHA256`: sha256 of the redacted SSM manifest evidence
- `BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_PATH`: strategy-input safety evidence path
- `BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_SHA256`: sha256 of the strategy-input safety evidence
- When strategy input evidence reports `market_selection_outcome = "next"`, it must also include `market_selection_source_path` and `market_selection_source_sha256` for a `market_selection_result` artifact with `source = "nt_runtime_selection_snapshot"`. The audit derives nearest-next candidates from that source-bound artifact and rejects a self-reported or truncated candidate list.
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
- The evidence writer rejects live-order proof serialization if `live_order_ref.strategy_instance_id_hash` no longer matches the approved strategy-instance hash derived from the financial envelope.

#### Live-result fields

- `BOLT_V3_PHASE8_DECISION_EVIDENCE_PATH`: persisted decision evidence proof path under the NT runtime capture spool
- `BOLT_V3_PHASE8_NT_SUBMIT_EVENT_PATH`: NT submit-event evidence path
- `BOLT_V3_PHASE8_VENUE_ORDER_STATE_PATH`: venue accept/fill/reject evidence path
- `BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH`: optional strategy-driven cancel evidence path when an order remains open
- `BOLT_V3_PHASE8_RESTART_RECONCILIATION_PATH`: restart reconciliation evidence path
- `BOLT_V3_PHASE8_POST_RUN_HYGIENE_PATH`: post-run raw-secret residue scan and retention/purge evidence path
- Live client and venue order hashes are derived from post-run proof files, not from pre-run operator-provided values.

#### Phase 8 artifact JSON schemas

All Phase 8 operator JSON artifacts are strict: unknown fields reject before live runner entry. String fields named `*_hash`, `*_sha256`, or `*_id_hash` are lowercase hex sha256 values unless stated otherwise. Decimal values are encoded as strings so operator-approved precision is preserved exactly.

`strategy_input_evidence` fields:

- `realized_volatility`: decimal string, positive
- `seconds_to_market_end`: integer seconds, positive
- `spot_price`: decimal string, positive
- `price_to_beat_value`: decimal string, positive
- `expected_edge_basis_points`: decimal string, positive and equal to `worst_case_edge_basis_points`
- `worst_case_edge_basis_points`: decimal string, positive and equal to `expected_edge_basis_points`
- `fee_rate_basis_points`: decimal string, zero or positive
- `price_to_beat_source`: string, must equal the approved `[parameters.runtime].price_to_beat_source`
- `reference_quote_ts_event`: integer timestamp, non-zero
- `pricing_kurtosis`: decimal string, greater than `-6`
- `theta_decay_factor`: decimal string, zero or positive
- `theta_scaled_min_edge_bps`: decimal string, positive
- `market_selection_timestamp_ms`: integer milliseconds
- `candidate_market_start_timestamps_ms`: optional integer-millisecond list, retained for evidence but not trusted for nearest-next approval
- `market_selection_source_path`: required path when `market_selection_outcome = "next"`
- `market_selection_source_sha256`: required sha256 when `market_selection_outcome = "next"`
- `market_selection_outcome`: string enum, `current` or `next`
- `polymarket_condition_id`, `polymarket_market_slug`, `polymarket_question_id`, `up_instrument_id`, `down_instrument_id`: selected-market identifiers
- `selected_market_observed_timestamp_ms`: integer milliseconds, non-zero
- `polymarket_market_start_timestamp_ms`, `polymarket_market_end_timestamp_ms`: integer milliseconds, selected start must precede selected end

`market_selection_result` source artifact fields:

- `record_kind`: string, `market_selection_result`
- `source`: string, `nt_runtime_selection_snapshot`
- `market_selection_timestamp_ms`: integer milliseconds matching strategy-input evidence
- `candidate_market_start_timestamps_ms`: non-empty integer-millisecond list used for nearest-next approval
- `market_selection_outcome`: string enum, must match strategy-input evidence
- `polymarket_condition_id`, `polymarket_market_slug`, `polymarket_question_id`, `up_instrument_id`, `down_instrument_id`: selected-market identifiers matching strategy-input evidence
- `selected_market_observed_timestamp_ms`: integer milliseconds matching strategy-input evidence
- `polymarket_market_start_timestamp_ms`, `polymarket_market_end_timestamp_ms`: integer milliseconds matching strategy-input evidence

`financial_envelope` fields:

- `max_live_order_count`: integer, must equal `1`
- `max_notional_per_order`: decimal string matching `[live_canary].max_notional_per_order`
- `strategy_instance_id`, `oms_type`, `execution_client_id`, `configured_target_id`, `target_kind`, `rotating_market_family`, `underlying_asset`, `cadence_slug_token`: strings matching the loaded strategy/TOML
- `cadence_secs`, `retry_interval_secs`, `blocked_after_secs`: integer seconds matching the loaded target runtime
- `market_selection_rule`: string matching the loaded target runtime
- `price_to_beat_source`: string matching `[parameters.runtime].price_to_beat_source`
- `edge_threshold_basis_points`: integer matching loaded strategy parameters
- `order_notional_target`, `maximum_position_notional`: decimal strings matching loaded strategy parameters
- `book_impact_cap_bps`: integer matching `[parameters.runtime].book_impact_cap_bps`
- `entry_side`, `entry_position_side`, `entry_order_type`, `entry_time_in_force`: strings matching loaded `[parameters.entry_order]` values
- `entry_expire_time_unix_nanos`, `entry_trigger_price`, `entry_activation_price`, `entry_trigger_type`, `entry_trigger_instrument_id`, `entry_trailing_offset`, `entry_trailing_offset_type`: optional values matching loaded `[parameters.entry_order]` values
- `entry_is_post_only`, `entry_is_reduce_only`, `entry_is_quote_quantity`: booleans matching loaded `[parameters.entry_order]` values
- `exit_side`, `exit_position_side`, `exit_order_type`, `exit_time_in_force`: strings matching loaded `[parameters.exit_order]` values
- `exit_expire_time_unix_nanos`, `exit_trigger_price`, `exit_activation_price`, `exit_trigger_type`, `exit_trigger_instrument_id`, `exit_trailing_offset`, `exit_trailing_offset_type`: optional values matching loaded `[parameters.exit_order]` values
- `exit_is_post_only`, `exit_is_reduce_only`, `exit_is_quote_quantity`: booleans matching loaded `[parameters.exit_order]` values
- `forced_exit_side`, `forced_exit_position_side`, `forced_exit_order_type`, `forced_exit_time_in_force`: strings matching loaded `[parameters.forced_exit_order]` values
- `forced_exit_expire_time_unix_nanos`, `forced_exit_trigger_price`, `forced_exit_activation_price`, `forced_exit_trigger_type`, `forced_exit_trigger_instrument_id`, `forced_exit_trailing_offset`, `forced_exit_trailing_offset_type`: optional values matching loaded `[parameters.forced_exit_order]` values
- `forced_exit_is_post_only`, `forced_exit_is_reduce_only`, `forced_exit_is_quote_quantity`: booleans matching loaded `[parameters.forced_exit_order]` values

`pre_run_state` fields:

- `execution_client_id`, `configured_target_id`: strings matching the financial envelope
- `host_clock_skew_within_bound`, `conflicting_open_orders_absent`, `preexisting_position_absent`, `market_state_approved`, `market_window_approved`, `funding_margin_covers_max_notional_plus_fees`, `single_runner_lock_acquired`, `egress_identity_approved`, `clob_v2_adapter_signing_verified`, `clob_v2_collateral_accounting_verified`, `clob_v2_fee_behavior_verified`, `release_manifest_nt_revision_matches_compiled_pin`: booleans, all must be `true`
- `host_clock_skew_evidence_hash`, `venue_account_state_evidence_hash`, `market_state_evidence_hash`, `funding_margin_evidence_hash`, `single_runner_lock_evidence_hash`, `egress_identity_evidence_hash`, `clob_v2_adapter_signing_evidence_hash`, `clob_v2_collateral_accounting_evidence_hash`, `clob_v2_fee_behavior_evidence_hash`, `release_manifest_evidence_hash`: sha256 bindings to operator-held evidence artifacts
- `release_manifest_clob_signing_version`: non-empty string for the CLOB V2 signing release proof

`abort_plan` fields:

- `execution_client_id`, `configured_target_id`: strings matching the financial envelope
- `cancel_if_open_defined`, `nt_accepted_venue_pending_abort_defined`, `partial_fill_abort_defined`, `network_partition_during_submit_abort_defined`, `panic_gate_trip_abort_defined`: booleans, all must be `true`
- `cancel_if_open_evidence_hash`, `nt_accepted_venue_pending_abort_evidence_hash`, `partial_fill_abort_evidence_hash`, `network_partition_during_submit_abort_evidence_hash`, `panic_gate_trip_abort_evidence_hash`: sha256 bindings to operator-held evidence proving each abort path

Live-result proof JSON files:

- `decision_evidence`, `nt_submit_event`, `venue_order_state`, `strategy_cancel`, and `restart_reconciliation` proofs include `record_kind` set to the matching proof name.
- `decision_evidence` must include `run_id` and `strategy_instance_id_hash`, and its path must be under the NT runtime capture spool.
- `nt_submit_event` must include `run_id`, `strategy_instance_id_hash`, `client_order_id_hash`, and `venue_order_id_hash`.
- `venue_order_state` must include `run_id`, `strategy_instance_id_hash`, `client_order_id_hash`, `venue_order_id_hash`, `venue_order_outcome`, and `order_remains_open`; `venue_order_outcome` is `accepted`, `filled`, or `rejected`, and terminal outcomes require `order_remains_open = false`.
- `strategy_cancel` is required when `venue_order_state.order_remains_open = true` and includes `run_id`, `strategy_instance_id_hash`, `client_order_id_hash`, and `venue_order_id_hash`.
- `restart_reconciliation` must include `source_run_id`, `strategy_instance_id_hash`, `client_order_id_hash`, `venue_order_id_hash`, `venue_order_outcome`, and `order_remains_open`; `venue_order_outcome` must be terminal (`filled` or `rejected`), `order_remains_open` must be `false`, and its path must be under the NT runtime capture spool.

`post_run_hygiene` fields:

- `record_kind`: string, `post_run_hygiene`
- `run_id`: runtime capture run id
- `strategy_instance_id_hash`, `client_order_id_hash`, `venue_order_id_hash`: approved live-order hashes
- `raw_secret_residue_absent`: boolean, must be `true`
- `scanned_artifact_hashes`: non-empty list of sha256 values for scanned artifacts
- `retention_purge_path_hash`: sha256 binding for the retention/purge path proof

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

##### `auto_load_missing_instruments`

- type: boolean
- required: yes
- must be `false` in the current bolt-v3 scope
- missing-instrument auto-load can trigger ad-hoc Gamma loads outside the configured market-identity plan

##### `auto_load_debounce_ms`

- type: positive integer
- required: yes
- maps directly to `PolymarketDataClientConfig.auto_load_debounce_ms`

##### `update_instruments_interval_mins`

- type: positive integer
- required: yes
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
- must not use NautilusTrader's Binance Spot JSON WebSocket host; the bolt-v3 reference quote probe requires an SBE endpoint or compatible SBE proxy so NT `subscribe_quotes` can emit `QuoteTick` data

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
time_in_force = "gtc"
is_post_only = false
is_reduce_only = true
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
- the same `instrument_id` must not be declared under more than one `data_client_id`, because NautilusTrader `QuoteTick` carries the instrument but not the producing data-client identifier and no-submit quote evidence must remain source-disambiguated
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

- type: decimal string
- required
- strategy-local desired notional target used by the archetype's sizing logic
- not the global hard cap
- validation requires `order_notional_target <= root risk.default_max_notional_per_order`
- runtime sizing usage is defined by `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 7.3

#### `maximum_position_notional`

- type: decimal string
- required
- maximum cumulative gross pUSD entry-cost exposure the strategy may target for the selected market
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
- `price_to_beat_source`: string; configured source identifier that Phase 8 strategy-input evidence must match through the financial envelope
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
- more than one `[clients.<identifier>]` block declares the same `venue` (NT `Venue` identifier) in the current one-client-per-venue slice
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
- `order_notional_target` exceeds `root risk.default_max_notional_per_order`
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

[nautilus]
load_state = true
save_state = true
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
graceful_shutdown_on_error = false
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
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
bypass = false
max_order_submit_rate = "100/00:00:01"
max_order_modify_rate = "100/00:00:01"
max_notional_per_order = {}
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[live_canary]
approval_id = "operator-approved-canary-001"
no_submit_readiness_report_path = "reports/no-submit-readiness.json"
max_no_submit_readiness_report_bytes = 65536
readiness_report_max_age_seconds = 300
reference_quote_max_age_seconds = 10
reference_quote_wait_timeout_seconds = 20
reference_quote_probe_actor_id = "no-submit-reference-quote-probe"
reference_quote_probe_log_events = true
reference_quote_probe_log_commands = true
max_live_order_count = 1
max_notional_per_order = "1.00"

[live_canary.operator_evidence]
head_sha = "0123456789abcdef0123456789abcdef01234567"
max_operator_evidence_file_bytes = 65536
approval_consumption_max_age_seconds = 60
approval_envelope_path = "operator-evidence/approval-envelope.json"
approval_envelope_sha256 = "9999999999999999999999999999999999999999999999999999999999999999"
ssm_manifest_path = "operator-evidence/ssm-manifest.json"
ssm_manifest_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
strategy_input_evidence_path = "operator-evidence/strategy-input.json"
strategy_input_evidence_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
financial_envelope_path = "operator-evidence/financial-envelope.json"
financial_envelope_sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
pre_run_state_path = "operator-evidence/pre-run-state.json"
pre_run_state_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
abort_plan_path = "operator-evidence/abort-plan.json"
abort_plan_sha256 = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
canary_evidence_path = "operator-evidence/canary-evidence.json"
# Set this window immediately before an approved canary run. It must cover two
# operator-evidence validation rounds plus report read, parse, and validation.
approval_not_before_unix_seconds = 1893456000
approval_not_after_unix_seconds = 1893456300
approval_nonce_path = "operator-evidence/approval-nonce.json"
approval_nonce_sha256 = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
approval_consumption_path = "operator-evidence/approval-consumed.json"
decision_evidence_path = "operator-evidence/decision-evidence.jsonl"
nt_submit_event_path = "operator-evidence/nt-submit-event.json"
venue_order_state_path = "operator-evidence/venue-order-state.json"
restart_reconciliation_path = "operator-evidence/restart-reconciliation.json"
post_run_hygiene_path = "operator-evidence/post-run-hygiene.json"

[aws]
region = "eu-west-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.data]
base_url_http = "https://clob.polymarket.com" # NT: nautilus_polymarket::config::PolymarketDataClientConfig.base_url_http
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market" # NT: PolymarketDataClientConfig.base_url_ws
base_url_gamma = "https://gamma-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_gamma
base_url_data_api = "https://data-api.polymarket.com" # NT: PolymarketDataClientConfig.base_url_data_api
http_timeout_secs = 60 # NT: PolymarketDataClientConfig.http_timeout_secs
ws_timeout_secs = 30 # NT: PolymarketDataClientConfig.ws_timeout_secs
subscribe_new_markets = false # NT: PolymarketDataClientConfig.subscribe_new_markets — forced false in current bolt-v3 scope
auto_load_missing_instruments = false # NT: PolymarketDataClientConfig.auto_load_missing_instruments — forced false in current bolt-v3 scope
auto_load_debounce_ms = 250 # NT: PolymarketDataClientConfig.auto_load_debounce_ms
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
product_types = ["spot"] # NT: nautilus_binance::config::BinanceDataClientConfig.product_types
environment = "mainnet" # NT: BinanceDataClientConfig.environment
base_url_http = "https://api.binance.com" # NT: BinanceDataClientConfig.base_url_http
base_url_ws = "wss://stream-sbe.binance.com/ws" # NT: BinanceDataClientConfig.base_url_ws
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
time_in_force = "gtc"
is_post_only = false
is_reduce_only = true
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
price_to_beat_source = "chainlink_data_streams.report_at_boundary"
pricing_kurtosis = 3.0
theta_decay_factor = 1.0
forced_flat_stale_chainlink_ms = 10000
forced_flat_thin_book_min_liquidity = 5.0
lead_agreement_min_corr = 0.8
lead_jitter_max_ms = 250
```
