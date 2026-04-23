# bolt-v3 Runtime Contracts

Status: draft for architecture review

This document defines the exact runtime contracts which must not be left to interpretation during implementation.

It covers:

- validation behavior
- target resolution behavior
- strategy / execution boundary
- forensic event contracts
- deploy/runtime controls
- panic gate acceptance criteria

## 1. Canonical Validation Path

The only supported validation entrypoint is:

- `just check`

`just check` must call one underlying validation implementation.

The `justfile` may wrap the command, but it must not contain its own validation logic.

## 2. Validation Phases

`just check` has two explicit phases.

Result classes:

- structural failure: fatal, non-zero exit
- live fatal failure: fatal, non-zero exit
- live operational warning: loud output, zero exit

### Phase 1: structural/config validation

This phase must not require live network access.

It validates:

- root file parses
- strategy files parse
- schema versions are supported
- unknown fields are rejected
- all required fields are present
- file references are valid
- keyed venue references are valid
- strategy-to-venue ownership rules are valid
- `signature_type` string enum is valid
- target shapes are valid
- order parameter enums are valid
- archetype-specific order-parameter combinations are valid for the declared archetype
- `order_notional_target <= root risk.default_max_notional_per_order`
- writable paths are syntactically valid

If structural validation fails:

- the command exits non-zero
- live validation does not run

### Phase 2: live/environment validation

This phase may use network access and real environment dependencies.

It validates:

- Amazon Web Services Systems Manager secret resolution
- forbidden venue-kind environment-variable fallbacks are absent
- keyed venue config can be converted into NautilusTrader client config
- required reference-data venues and instruments are resolvable
- current target-resolution machinery can load NautilusTrader venue/instrument state and attempt resolution through that state only
- root risk config can be synchronized to the current instrument set loaded for keyed execution venues
- a NautilusTrader node can be assembled without starting the live event loop

Live validation must not:

- submit orders
- connect the full live node and run indefinitely
- start background processes that survive the command

If the current market state does not yield a resolvable `active_or_next` target:

- live validation reports `unresolved_current_target`
- this is loud and explicit
- this is a live operational warning, not a fatal live-validation failure
- this does not by itself block process startup, because runtime target retry is part of the first-live design

Implementation rule:

- live validation must call the same target-resolution and venue-instrument-loading functions used by runtime startup
- it must not implement a second discovery path

## 3. Secret Resolution Contract

Secrets are resolved with the Rust Amazon Web Services Systems Manager client.

Rules:

- all secrets are fetched from explicit Amazon Web Services Systems Manager paths in the root file
- no environment-variable fallback path is allowed
- no second secret source is allowed
- resolved secret values must never be written to logs

For every keyed venue which declares a `[secrets]` block, bolt must fail live validation and startup if any canonical credential environment variables for that venue kind are present.

For first-live Polymarket, the forbidden variables are:

- `POLYMARKET_PK`
- `POLYMARKET_FUNDER`
- `POLYMARKET_API_KEY`
- `POLYMARKET_API_SECRET`
- `POLYMARKET_PASSPHRASE`

For first-live Binance reference-data use, the forbidden variables are:

- `BINANCE_API_KEY`
- `BINANCE_API_SECRET`

This per-venue environment-variable blocklist belongs to the venue-kind handler in bolt code.
It is not a generic secret framework.

## 4. Root Risk Contract

Root risk settings are entity-wide safety settings.

Rules:

- strategy-local notional limits are not sufficient by themselves
- root-level risk settings must be explicit in TOML
- NautilusTrader defaults must not silently determine live capital risk

Current contract:

- `bypass` is explicit
- submit and modify throttles are explicit
- `default_max_notional_per_order` is explicit

Authority rule:

- `default_max_notional_per_order` is the hard entity-level cap
- strategy-local `order_notional_target` is the desired archetype sizing target
- validation requires `order_notional_target <= default_max_notional_per_order`

Synchronization behavior:

- keyed execution venues own instrument loading and instrument refresh
- whenever a keyed execution venue loads or refreshes instruments, bolt synchronizes `default_max_notional_per_order` onto the currently loaded instrument set for that venue
- this synchronization is tied to venue instrument loading, not to strategy target-rotation callbacks
- when the loaded instrument set changes, old cap entries for instruments no longer present in the current loaded set are removed

Mechanism:

- bolt follows NautilusTrader instrument topics on the Nautilus message bus for keyed execution venues
- currently loaded instruments are learned from `data.instrument.{venue}.*`
- instrument removal or expiry is learned from `data.close.{venue}.*`
- there is no separate bolt poll loop and no strategy-to-bolt callback for this

## 5. Target Resolution Contract

### 5.1 Supported target kinds

Only these target kinds are supported in first live trade:

- `instrument`
- `series`

### 5.2 Strategy ownership

The strategy owns:

- when target resolution runs
- the retry loop
- the blocked/degraded state

There is no standalone selector service.

### 5.3 Resolution source of truth

Resolution order:

1. NautilusTrader-loaded venue and instrument state
2. narrow Polymarket Gamma supplement for first-live `updown` anchor extraction only

First-live rule:

- bolt may call Gamma `GET /events?slug=<event_slug>` only to extract `eventMetadata.priceToBeat` for an already-declared first-live `updown` event slug
- this supplement must not be used for broad discovery, order state, prices, portfolio, reference data, or strategy-side HTTP
- if `eventMetadata.priceToBeat` is missing, non-numeric, non-positive, or ambiguous, target resolution fails loud and the strategy remains non-trading

First-live Polymarket loading contract:

- bolt derives NautilusTrader-native Polymarket instrument filters from the configured strategy targets
- for first-live `updown`, bolt installs a dynamic NautilusTrader `MarketSlugFilter` closure in the pinned `PolymarketDataClientConfig.filters`
- on each evaluation of that closure, it yields market slugs for the current and next cadence windows for each configured `underlying_asset`
- bolt invokes NautilusTrader's `request_instruments` path at startup and when the generated current/next slug pair changes under the NautilusTrader node clock
- normal retry ticks read NautilusTrader cache only
- if the `request_instruments` call for the current slug pair fails, retry that request at `target.retry_interval_seconds` until it succeeds or the slug pair changes
- `subscribe_new_markets` remains `false` in first-live scope
- broad Polymarket instrument loading without target-derived filters is forbidden

First-live `updown` slug derivation rule:

- slug format: `"{underlying_asset_lowercase}-updown-{cadence_minutes}m-{period_start_unix_seconds}"`
- `cadence_minutes = cadence_seconds / 60`
- `now_unix_seconds` comes from the NautilusTrader node clock
- `current_period_start_unix_seconds = floor(now_unix_seconds / cadence_seconds) * cadence_seconds`
- `next_period_start_unix_seconds = current_period_start_unix_seconds + cadence_seconds`
- the first-live resolver's dynamic filter yields exactly two market slugs per configured `updown` target on each evaluation:
  - current-period slug
  - next-period slug

This keeps the loading path inside the NautilusTrader Polymarket adapter rather than creating a second discovery system.

### 5.4 `active_or_next` semantics

For `rotation_policy = "active_or_next"`:

- if exactly one active tradable market exists in the declared series, use it
- otherwise, if exactly one next tradable market exists in the declared series, use it
- otherwise, fail that resolution attempt loudly

For first-live `updown`:

- `active tradable market` means the single market in the declared series whose `tradable` flag is true and whose time window satisfies `market_start_timestamp_milliseconds <= resolver_time_milliseconds < market_end_timestamp_milliseconds`

- `next tradable market` means the single market in the declared series whose `market_start_timestamp_milliseconds` is the smallest value greater than the current resolver time and whose `tradable` flag is true

Prohibited behaviors:

- guessing among multiple candidates
- broadening search outside the declared series
- selecting a fallback market
- silently suppressing ambiguity

### 5.5 Retry behavior

If a series target cannot be resolved:

- emit `selector_retry`
- remain non-trading
- retry at `target.retry_interval_seconds`
- no backoff

If unresolved for `target.blocked_after_seconds`:

- emit `strategy_blocked`
- mark the strategy blocked/degraded
- continue retrying at `target.retry_interval_seconds` unless the strategy is stopped

Blocked/degraded operational behavior:

- the strategy continues to receive NautilusTrader callbacks
- the strategy must skip target-dependent evaluation and must not submit orders while blocked
- the strategy may continue target-resolution retry attempts on its timer path

## 6. Resolved Target Contract

The resolver must produce a concrete resolved target object before the strategy trades.

### 6.1 Universal resolved-target fields

Every resolved target must contain:

- `target_kind`
- `venue_key`
- `venue_kind`
- `resolved_market_identifier`
- `tradable`

If the resolved target carries a bounded market window, it must also contain:

- `market_start_timestamp_milliseconds`
- `market_end_timestamp_milliseconds`

### 6.2 Instrument-target fields

If `target_kind = "instrument"`, the resolved target must also contain:

- `instrument_identifier`
- `resolved_market_identifier = instrument_identifier`
- `market_start_timestamp_milliseconds` may be null
- `market_end_timestamp_milliseconds` may be null

### 6.3 First-live updown-series fields

If `target_kind = "series"` and `series_family = "updown"`, the resolved target must also contain:

- `condition_identifier`
- `event_slug`
- `market_slug`
- `up_instrument_identifier`
- `down_instrument_identifier`
- `anchor_price`
- `anchor_source`

Definitions:

- `condition_identifier` = the Polymarket condition identifier string for the resolved market shell
- `resolved_market_identifier = condition_identifier` for first-live `updown`
- `up_instrument_identifier` = the literal NautilusTrader instrument identifier string for the `Up` instrument
- `down_instrument_identifier` = the literal NautilusTrader instrument identifier string for the `Down` instrument
- `anchor_price` = Gamma `eventMetadata.priceToBeat` from `GET /events?slug=<event_slug>`
- `anchor_source` first-live value:
  - `event_metadata.priceToBeat`

For first-live `binary_oracle_edge_taker`, anchor metadata is required.

The strategy must not fetch venue metadata directly to fill missing resolved-target fields.

## 7. Reference Data Contract

If an archetype requires reference data:

- the strategy file must declare it explicitly
- the root file must declare the keyed data venue explicitly
- the strategy subscribes directly through NautilusTrader data clients

There is no bolt-owned reference actor.

For first-live `binary_oracle_edge_taker`:

- the declared reference-data instrument is subscribed as quote ticks
- the archetype derives spot-price input from that declared quote-tick stream

Reference-data resolution rule for validation:

- `resolvable` means that after NautilusTrader venue/instrument loading completes, the declared `instrument_identifier` exists in the NautilusTrader instrument cache for the referenced keyed venue
- `resolvable` does not require receiving a live quote before `just check` completes

### 7.1 First-live `binary_oracle_edge_taker` pricing inputs

For first-live `binary_oracle_edge_taker`, the reference stream and pricing inputs are mechanical:

- `spot_price`
  - derived from the latest two-sided midpoint on `reference_data.primary`
  - midpoint formula: `(best_bid_price + best_ask_price) / 2`
  - if the latest quote tick does not contain both sides, midpoint is unavailable
  - if the latest midpoint sample is older than `target.retry_interval_seconds` seconds, the reference quote is stale
- `strike_price`
  - `resolved_target.anchor_price`
- `strike_source`
  - `event_metadata.priceToBeat`

There is no first-live fallback from missing anchor metadata to midpoint.

### 7.2 First-live `binary_oracle_edge_taker` realized volatility

The first-live realized-volatility estimator is defined as:

- input samples:
  - midpoint samples from `reference_data.primary`
- retention window:
  - keep midpoint samples whose timestamps fall within the trailing `target.cadence_seconds` seconds
- reset rule:
  - if the gap between consecutive midpoint samples exceeds `target.retry_interval_seconds` seconds, reset the estimator state
- readiness rule:
  - at least two midpoint samples are required
  - sample timestamps used for the estimator must be strictly increasing
  - `elapsed_seconds`, measured from the first retained sample timestamp to the last retained sample timestamp, must be strictly positive
- return formula:
  - for each consecutive midpoint pair, compute `log(current_midpoint / previous_midpoint)`
- annualization formula:
  - `SECONDS_PER_YEAR = 31_536_000`
  - `realized_volatility = sqrt((sum_squared_log_returns / elapsed_seconds) * SECONDS_PER_YEAR)`
- bridge-valid rule:
  - if the last ready realized-volatility value is older than `target.retry_interval_seconds` seconds, realized volatility is not ready

### 7.3 First-live `binary_oracle_edge_taker` entry evaluation

The first-live entry evaluation is:

1. compute `fair_probability_up` from:
   - `spot_price`
   - `strike_price`
   - `seconds_to_expiry`
   - `realized_volatility`
2. first-live fair-probability formula:
   - `d2 = (ln(spot_price / strike_price) - (realized_volatility^2 / 2) * time_to_expiry_years) / (realized_volatility * sqrt(time_to_expiry_years))`
   - `fair_probability_up = standard_normal_cdf(d2)`
   - if `realized_volatility <= 0` or `time_to_expiry_years <= 0`, fair probability is unavailable and entry evaluation skips as not ready
3. derive side success probability:
   - `Up`: `fair_probability_up`
   - `Down`: `1.0 - fair_probability_up`
4. derive executable entry cost:
   - `Up`: current best ask on `up_instrument_identifier`
   - `Down`: current best ask on `down_instrument_identifier`
5. compute edge:
   - fee rate for the selected token must be available from the pinned Polymarket fee-rate path before entry may proceed
   - `expected_edge_basis_points` must account for the applicable Polymarket fee rate
   - first-live v1 rule: `worst_case_edge_basis_points = expected_edge_basis_points`
6. side selection:
   - choose the single side with the higher `worst_case_edge_basis_points`
   - if neither side is strictly greater than `parameters.edge_threshold_basis_points`, skip
   - if both sides are equal, skip
7. sizing:
   - notional fields are gross USDC entry-cost terms before fees
   - filled entry-cost exposure comes from NautilusTrader confirmed position state: `position.quantity * position.avg_px_open`
   - open buy-order entry-cost exposure comes from NautilusTrader open/inflight order state: `order.leaves_qty * order.price`
   - remaining capacity = `parameters.maximum_position_notional - filled_entry_cost_exposure - open_buy_entry_cost_exposure`
   - `sizing_cap = min(parameters.order_notional_target, remaining_capacity, root risk.default_max_notional_per_order)`
   - if `remaining_capacity <= 0`, skip with `position_limit_reached`
   - if the selected side clears the edge threshold, `sized_notional = sizing_cap`
8. quantity conversion:
   - `quantity_raw = sized_notional / executable_entry_cost`
   - convert with NautilusTrader instrument precision via `instrument.try_make_qty(quantity_raw, Some(true))`
   - local rejection if conversion fails or the resulting quantity is not positive
9. order construction:
   - entry price = current best ask of the selected side instrument
   - entry order uses the locked first-live archetype order combination from the strategy schema

### 7.4 First-live `binary_oracle_edge_taker` exit evaluation

The first-live exit rule is intentionally thin:

- no blind exits
- no bolt-owned exit engine
- strategy may submit an exit only when it can construct a valid NautilusTrader-native sell order from NautilusTrader state
- if NautilusTrader state or order construction inputs are insufficient, do not submit; emit decision/local-rejection evidence
- market end does not create a special bolt exit policy; stop new entries for that target and rely on NautilusTrader / venue reconciliation for final position lifecycle

## 8. Strategy / Execution Contract

This contract is intentionally strict.

### 8.1 Boundary

- strategies use NautilusTrader-native order types directly
- bolt does not define another executable-order schema
- bolt does not define a higher-level intent layer above NautilusTrader orders

### 8.2 Venue shaping

- venue-native wire translation belongs in NautilusTrader venue adapters
- bolt must not wrap that path with another venue-specific translation layer

### 8.3 If a gap exists

If Polymarket needs execution semantics not expressible cleanly by the current NautilusTrader adapter:

- the preferred fix is upstream in the NautilusTrader Polymarket adapter
- not a bolt-side wrapper schema

### 8.4 Exit authority

For exits:

- strategy-local remembered quantities are never authoritative
- current position and sellable state must come from NautilusTrader cache and venue-confirmed state
- if exit quantity cannot be validated locally, fail loud before submit when possible

For first-live Polymarket, because the pinned adapter rejects reduce-only orders:

- before emitting `exit_submit_intent`, the strategy must read `authoritative_position_quantity` and `authoritative_sellable_quantity`
- if intended exit quantity exceeds `authoritative_sellable_quantity`, emit `exit_submit_rejected_local`
- required local rejection reason: `insufficient_sellable_quantity`
- in that case, do not submit an order

## 9. Forensic Event Contract

### 9.1 Principle

NautilusTrader-native observability first.

bolt adds only:

- minimal structured strategy-decision events
- mirrored readable logs

There is no generic event framework.

### 9.2 Event model

There is a small fixed set of concrete event types.

Every event includes the same common required fields.
Each event type includes additional fixed fields specific to that event.
Where an event may be emitted for more than one target kind, target-kind-specific fields are conditional on `target_kind`.

### 9.3 Common required fields

These fields are required on every structured decision event:

- `schema_version`
- `event_kind`
- `event_timestamp_milliseconds`
- `decision_trace_identifier`
- `strategy_instance_identifier`
- `strategy_archetype`
- `trader_identifier`
- `venue`
- `runtime_mode`
- `release_identifier`
- `config_hash`
- `nautilus_trader_revision`
- `resolved_target_identifier`

Definitions:

- `schema_version`
  - the event-schema version
  - first-live value: `1`
- `event_timestamp_milliseconds`
  - wall-clock UTC Unix timestamp in milliseconds at emission time
- `decision_trace_identifier`
  - generated once at the first selector/evaluation step for a potential trading lifecycle
  - format: UUID4 string
  - reused on all subsequent decision events for that lifecycle
  - the lifecycle ends when the strategy reaches a terminal outcome for that opportunity: skip, local submit rejection, or flat-after-exit
- `strategy_archetype`
  - the exact `strategy_archetype` value from the strategy file
- `trader_identifier`
  - the exact root-file `trader_identifier` value
- `venue`
  - the keyed trading venue reference from the strategy file
  - not a reference-data venue key
- `runtime_mode`
  - the exact root-file `[runtime].mode` value
- `release_identifier`
  - the exact deployed release directory name selected by deploy automation
  - first-live deployment rule: release directory names are the git commit SHA string for the built artifact
- `config_hash`
  - SHA-256 of the concatenation of:
    1. root-file bytes with line endings normalized to LF
    2. each listed strategy-file bytes in root `strategy_files` order, with line endings normalized to LF
  - file paths are not included
- `nautilus_trader_revision`
  - the pinned git revision string from `Cargo.toml`
  - first-live value: `48d1c126335b82812ba691c5661aeb2e912cde24`
- `resolved_target_identifier`
  - deterministic string built from the resolved target's universal identity fields
  - first-live series shape: `"{venue_key}:{resolved_market_identifier}"`
  - first-live instrument-target shape: `"{venue_key}:{instrument_identifier}"`
  - nullable on unresolved selector events and blocked target-resolution events

### 9.4 First-live-trade event type list

First-live-trade event types are:

- `selector_decision`
- `selector_retry`
- `strategy_blocked`
- `entry_evaluation`
- `entry_submit_intent`
- `entry_submit_rejected_local`
- `exit_evaluation`
- `exit_submit_intent`
- `exit_submit_rejected_local`

Extensions later are allowed, but these are the fixed first-live types.

`event_kind` values are the exact lowercase underscore-separated strings listed in this section.
Event schema version is independent of root-file and strategy-file schema versions.
First-live event-schema version is `1`.

### 9.5 Event-specific required fields

#### `selector_decision`

Required additional fields:

- `target_kind`
- `resolved_market_identifier`
- `market_start_timestamp_milliseconds`
- `market_end_timestamp_milliseconds`
- `resolution_result`
- `failure_reason`

`failure_reason` may be null on success.
`resolved_market_identifier`, `market_start_timestamp_milliseconds`, and `market_end_timestamp_milliseconds` may be null on a failed resolution attempt.

Allowed `resolution_result` values for first live:

- `resolved_active`
- `resolved_next`
- `failed`

Allowed `failure_reason` values for first live:

- `request_instruments_failed`
- `cache_unresolved`
- `no_candidate`
- `multiple_candidates`
- `target_not_tradable`

If `target_kind = "series"` and resolution succeeds, the event must also contain:

- `series_family`
- `underlying_asset`
- `cadence_seconds`
- `rotation_policy`
- `condition_identifier`
- `up_instrument_identifier`
- `down_instrument_identifier`

If `target_kind = "instrument"` and resolution succeeds, the event must also contain:

- `instrument_identifier`

#### `selector_retry`

Required additional fields:

- `target_kind`
- `failure_reason`
- `next_retry_timestamp_milliseconds`

If `target_kind = "series"`, the event must also contain:

- `series_family`
- `underlying_asset`
- `cadence_seconds`
- `rotation_policy`

If `target_kind = "instrument"`, the event must also contain:

- `instrument_identifier`

#### `strategy_blocked`

Required additional fields:

- `blocked_reason`
- `blocked_since_timestamp_milliseconds`
- `last_failure_reason`

Allowed `blocked_reason` values for first live:

- `target_resolution_timeout`

#### `entry_evaluation`

Required additional fields:

- `selected_side`
- `decision_result`
- `skip_reason`
- `seconds_to_expiry`
- `archetype_metrics`

`skip_reason` may be null if `decision_result` is enter.
`selected_side` may be null if `decision_result` is skip.

Allowed `decision_result` values for first live:

- `enter`
- `skip`

Allowed `skip_reason` values for first live:

- `blocked_strategy`
- `target_not_tradable`
- `missing_reference_quote`
- `stale_reference_quote`
- `insufficient_edge`
- `position_limit_reached`

For first-live `binary_oracle_edge_taker`, `archetype_metrics` must contain:

- `spot_price`
- `strike_price`
- `strike_source`
- `realized_volatility`
- `expected_edge_basis_points`
- `worst_case_edge_basis_points`

Definitions for first-live `binary_oracle_edge_taker` metrics:

- `spot_price`
  - latest valid midpoint from `reference_data.primary`
- `strike_price`
  - resolved target anchor when present, otherwise `spot_price`
- `realized_volatility`
  - output of the realized-volatility estimator defined in Section 7.2
- `expected_edge_basis_points`
  - selected-side edge from Section 7.3 before any additional uncertainty haircut
- `worst_case_edge_basis_points`
  - selected-side edge used for thresholding and sizing
  - first-live v1 rule: equal to `expected_edge_basis_points`

When `decision_result` is `skip`, any unavailable first-live `binary_oracle_edge_taker` metric fields may be null rather than synthesized.

Allowed `strike_source` values for first live:

- `event_metadata.priceToBeat`

#### `entry_submit_intent`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_identifier`
- `side`
- `price`
- `quantity`
- `quote_quantity`
- `post_only`
- `reduce_only`

This event records the exact NautilusTrader-native order semantics being submitted.

This event must also contain:

- `client_order_identifier`

`client_order_identifier` may be null when the rejection happens before a NautilusTrader order object is constructed.

Allowed `local_rejection_reason` values for first live:

- `target_unresolved`
- `strategy_blocked`
- `invalid_price`
- `invalid_quantity`
- `exceeds_order_notional_cap`

#### `entry_submit_rejected_local`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_identifier`
- `side`
- `price`
- `quantity`
- `quote_quantity`
- `post_only`
- `reduce_only`
- `local_rejection_reason`

This event must also contain:

- `client_order_identifier`

#### `exit_evaluation`

Required additional fields:

- `decision_result`
- `exit_reason`
- `authoritative_position_quantity`
- `authoritative_sellable_quantity`
- `archetype_metrics`

For first-live `binary_oracle_edge_taker`, `archetype_metrics` may be an empty object.

Allowed `decision_result` values for first live:

- `exit`
- `hold`

Allowed `exit_reason` values for first live:

- `market_end`
- `strategy_exit_signal`
- `position_safety_exit`

#### `exit_submit_intent`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_identifier`
- `side`
- `price`
- `quantity`
- `quote_quantity`
- `post_only`
- `reduce_only`
- `authoritative_position_quantity`
- `authoritative_sellable_quantity`

This event must also contain:

- `client_order_identifier`

`client_order_identifier` may be null when the rejection happens before a NautilusTrader order object is constructed.

Allowed `local_rejection_reason` values for first live:

- `insufficient_sellable_quantity`
- `invalid_quantity`

#### `exit_submit_rejected_local`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_identifier`
- `side`
- `price`
- `quantity`
- `quote_quantity`
- `post_only`
- `reduce_only`
- `authoritative_position_quantity`
- `authoritative_sellable_quantity`
- `local_rejection_reason`

This event must also contain:

- `client_order_identifier`

### 9.6 Transport

Primary machine-readable decision evidence:

- structured decision events emitted as NautilusTrader custom-data events on the Nautilus message bus

For first live trade, these events are persisted to the local catalog directory as machine-readable evidence.

Registration and persistence mechanism:

- bolt registers the fixed decision-event custom-data types with NautilusTrader at startup
- registration happens before the Nautilus catalog/streaming path is initialized
- bolt configures NautilusTrader `StreamingConfig` from `[persistence.streaming]`
- bolt enables catalog persistence for those registered custom-data event types through NautilusTrader's streaming path
- bolt does not implement a second event writer or subscriber-writer persistence loop

Join rule:

- decision events join to NautilusTrader-native execution events through `client_order_identifier`
- if a local rejection occurs before order creation and `client_order_identifier` is null, the terminal local-rejection event is joined by `decision_trace_identifier` only
- `venue_order_identifier` is owned by NautilusTrader-native execution events once the venue acknowledges the order

Readable mirror:

- logs containing the same `decision_trace_identifier` and key fields

No strategy may invent its own ad hoc forensic schema.

## 10. Local Disk and Later Archival

Before first live trade:

- local machine-readable decision evidence must exist
- local logs must exist
- local evidence must be sufficient for reconstruction without relying on Amazon Simple Storage Service
- machine-readable decision evidence lives in the configured catalog directory

Immediate follow-up:

- export/archive those artifacts to Amazon Simple Storage Service

## 11. Deploy / Runtime Contract

### 11.1 Required controls before first live trade

- artifact verification before startup
- versioned release directories
- atomic release-pointer switch
- one automated deploy/restart path only
- restart permission limited to the deploy identity
- crash dumps disabled
- restart loops capped
- writable filesystem paths explicitly allow-listed

### 11.2 Release identity manifest

Deploy automation writes a release identity manifest inside the selected release directory after artifact verification succeeds.

The manifest must contain at minimum:

- `release_identifier`
- `git_commit_sha`
- `nautilus_trader_revision`

Runtime rule:

- bolt reads `release_identifier` from that release manifest at startup
- bolt does not derive release identity from process working directory or ad hoc path parsing
- bolt does not perform a second manifest-signature verification step in first-live scope; trust comes from the verified artifact set plus the deploy-identity-controlled release directory

### 11.3 Required writable paths

The allow-list must include exactly the paths bolt needs to write:

- log directory
- state directory
- catalog directory
- runtime temporary directory used by the service wrapper

No other write path is allowed.

## 12. Panic Gate: Issue `#239`

### 12.1 Required test matrix

Inject panics into:

- startup callback
- market-data callback
- order-event callback
- position-event callback
- timer callback

### 12.2 Test environment

Must run on:

- the exact pinned NautilusTrader revision
- the exact release build profile
- the exact systemd restart policy intended for first live trade

### 12.3 Required observations

For each injected panic, record:

- callback where panic occurred
- process exit behavior
- exit code or signal
- emitted logs
- systemd state transitions
- whether restart happened
- whether restart loop cap was reached
- whether service finished healthy, restarting, or failed

### 12.4 Acceptance rule

First live trade is allowed only if:

- panic behavior is empirically tested
- blast radius is documented
- resulting behavior is explicitly accepted

Unknown panic behavior is not acceptable.

## 13. First-Live-Trade Invariants

The following invariants must remain true:

- no mixins
- no dual paths
- no bolt-owned order schema
- no bolt-owned selector service
- no bolt-owned observability stack
- no strategy-specific venue shaping
- no hidden identity overlap
- no environment-variable secret fallback
