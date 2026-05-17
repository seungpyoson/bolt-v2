# bolt-v3 Runtime Contracts

Status: draft for architecture review

This document defines the exact runtime contracts which must not be left to interpretation during implementation.

It covers:

- validation behavior
- market selection behavior
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

It runs the structural-validation rules defined by `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` Section 8.

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
- current market-selection machinery can load NautilusTrader venue/instrument state and attempt selection through that state only
- root risk config is enforced for Bolt-owned strategy sizing fields, and supported NautilusTrader risk-engine knobs are explicit and mapped rather than accepted as no-ops
- current `updown` instrument-filter readiness gates for each configured `updown` target
- local catalog evidence can round-trip a registered fixed decision-event type
- release identity manifest exists and matches the selected artifact set
- a NautilusTrader node can be assembled without starting the live event loop

Live validation must not:

- submit orders
- connect the full live node and run indefinitely
- start background processes that survive the command

If the current market state does not yield a selectable `active_or_next` target:

- live validation reports the exact `market_selection_failure_reason`
- this is loud and explicit
- this is a live operational warning, not a fatal live-validation failure
- this does not by itself block process startup, because runtime target retry is part of the current design

Implementation rule:

- live validation must call the same market-selection and venue-instrument-loading functions used by runtime startup
- it must not implement a second discovery path

Current `updown` readiness gates:

- for each configured `updown` target, derive the current and next `updown` market slugs from the NautilusTrader node clock and the current slug rule
- for the current first-live `updown` Polymarket scope, `event_page_slug` equals the selected CLOB market slug; this is scoped #244 evidence for sampled BTC/ETH 5m updown markets, not a universal Polymarket rule
- resolve `price_to_beat_value` from Chainlink Data Streams REST for the selected market boundary timestamp
- the Chainlink Data Streams response must identify exactly one usable report for the configured feed and boundary timestamp
- the decoded benchmark price must parse as numeric and be positive
- Gamma `eventMetadata.priceToBeat` is post-close forensic validation only; it is not a runtime order-readiness anchor
- if the Chainlink Data Streams report is missing, non-numeric, non-positive, or ambiguous, live validation fails for order readiness
- no readiness check may use broad Gamma polling, standalone market-selection services, midpoint/spot/question/threshold fallback, or strategy-side discovery HTTP

Market-selection validation result classes:

- no currently selectable `active_or_next` market is a live operational warning
- `request_instruments_failed`, `instruments_not_in_cache`, `no_selected_market`, and `ambiguous_selected_market` are live operational warning failures
- `price_to_beat_unavailable` and `price_to_beat_ambiguous` are fatal order-readiness failures for the current `updown` live-trading scope
- all runtime market-selection failures emit `market_selection_result` evidence and keep the strategy non-trading for that configured target

## 3. Secret Resolution Contract

Secrets are resolved with the Rust Amazon Web Services Systems Manager client.

Rules:

- all secrets are fetched from explicit Amazon Web Services Systems Manager paths in the root file
- no environment-variable fallback path is allowed
- no second secret source is allowed
- resolved secret values must never be written to logs

For every keyed venue which declares a `[secrets]` block, bolt must fail live validation and startup if any canonical credential environment variables for that venue kind are present.

Structural validation also rejects secret blocks that no configured adapter consumes:

- Polymarket `[secrets]` is allowed only alongside `[execution]`
- Binance `[secrets]` is allowed only alongside `[data]`

For current Polymarket live trading, the forbidden variables are:

- `POLYMARKET_PK`
- `POLYMARKET_FUNDER`
- `POLYMARKET_API_KEY`
- `POLYMARKET_API_SECRET`
- `POLYMARKET_PASSPHRASE`

For current Binance reference-data use, the forbidden variables are:

- `BINANCE_ED25519_API_KEY`
- `BINANCE_ED25519_API_SECRET`
- `BINANCE_API_KEY`
- `BINANCE_API_SECRET`

This per-venue environment-variable blocklist belongs to the venue-kind handler in bolt code.
It is not a generic secret framework.
The handler must derive the effective blocklist from the configured venue kind, environment, and product type before any NautilusTrader client constructor is called.

## 4. Root Risk Contract

Root risk settings are entity-wide safety settings.

Rules:

- strategy-local notional limits are not sufficient by themselves
- current root-level risk settings must be explicit in TOML
- NautilusTrader live data-engine defaults must be explicit in TOML and mapped into `LiveDataEngineConfig`; the builder path must not inherit `LiveDataEngineConfig::default()` silently
- NautilusTrader live risk-engine defaults must be explicit in TOML and mapped into `LiveRiskEngineConfig`; the builder path must not inherit `LiveRiskEngineConfig::default()` silently
- NautilusTrader live exec-engine defaults are explicit in TOML and mapped into `LiveExecEngineConfig`; the builder path must not inherit `LiveExecEngineConfig::default()` silently
- NautilusTrader logger fields accepted by the pinned Rust live runtime must be represented in TOML and mapped explicitly in the bolt-v3 builder path; the builder path must not inherit `LoggerConfig::default()` silently
- top-level `LiveNodeConfig` fields accepted as disabled/false in the current Bolt-v3 runtime must be explicit in TOML; `data_clients` and `exec_clients` remain derived from configured venues through provider Adapter registration

Current contract:

- `default_max_notional_per_order` is explicit
- every `LiveDataEngineConfig` field is explicit under `[nautilus.data_engine]` in TOML and mapped into NautilusTrader live data config
- every `LiveRiskEngineConfig` field is explicit under `[risk]` in TOML and mapped into NautilusTrader live risk config
- every `LiveExecEngineConfig` field is explicit under `[nautilus.exec_engine]` in TOML and mapped into NautilusTrader live exec config
- `[logging]` owns the complete pinned `LoggerConfig` surface that the Rust live runtime accepts: root levels, component/module overrides, credential-module level, boolean logging flags, explicit disabled file config, and clear-file policy
- unsupported top-level `LiveNodeConfig` surfaces (`instance_id`, `cache`, `msgbus`, `portfolio`, `emulator`, `streaming`, and `loop_debug`) are explicitly disabled in `[nautilus]`; `data_clients` and `exec_clients` are empty in `LiveNodeConfig` because provider Adapter registration owns client construction

Authority rule:

- `default_max_notional_per_order` is the hard entity-level cap
- strategy-local `order_notional_target` is the desired archetype sizing target
- validation requires `order_notional_target <= default_max_notional_per_order`

Current implementation behavior:

- `default_max_notional_per_order` is enforced by Bolt-v3 config validation against each strategy's `parameters.order_notional_target`
- Bolt-v3 maps the complete live data-engine block into NautilusTrader `LiveDataEngineConfig`
- Bolt-v3 maps the complete live risk-engine block into NautilusTrader `LiveRiskEngineConfig`
- Bolt-v3 maps the complete live exec-engine block into NautilusTrader `LiveExecEngineConfig`
- Bolt-v3 maps the complete pinned `LoggerConfig` field set from TOML without relying on `LoggerConfig::default()` inheritance; `file_config = "disabled"` maps to `None`, and validation rejects `clear_log_file = true` because the pinned Rust live runtime rejects it at build-time validation
- Bolt-v3 maps the remaining top-level `LiveNodeConfig` disabled/false fields from TOML and does not rely on top-level `LiveNodeConfig::default()` inheritance
- `scripts/verify_bolt_v3_runtime_literals.py` scans every current production Rust source file under `src/**/*.rs`; candidate runtime-bearing literals must be classified in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
- the baseline fixture asserts all explicit data-engine values, `nt_bypass = false`, `100/00:00:01` submit/modify rate limits, an empty NT per-instrument notional map, `nt_debug = false`, current NT-default `nt_qsize`, and all explicit exec-engine values

Future synchronization behavior:

- keyed execution venues own instrument loading and instrument refresh
- whenever a keyed execution venue loads or refreshes instruments, bolt synchronizes `default_max_notional_per_order` onto the currently loaded instrument set for that venue
- this synchronization is tied to venue instrument loading, not to strategy target-rotation callbacks
- when the loaded instrument set changes, old cap entries for instruments no longer present in the current loaded set are removed

Mechanism:

- bolt follows NautilusTrader instrument topics on the Nautilus message bus for keyed execution venues
- currently loaded instruments are learned from `data.instrument.{venue}.*`
- instrument removal or expiry is learned from `data.close.{venue}.*`
- there is no separate bolt poll loop and no strategy-to-bolt callback for this

### 4.1 NautilusTrader Portfolio and Bolt allocation state

`NT Portfolio` means the NautilusTrader-owned source of truth for account, position, order, fill, balance, average-price, and exposure state.

`Bolt allocation state` means strategy-local allocation facts computed from `NT Portfolio`, NautilusTrader cache state, and TOML configuration for a single decision evaluation.

Current `Bolt allocation state` projections include:

- `entry_filled_notional`
- `open_entry_notional`
- `strategy_remaining_entry_capacity`
- `has_selected_market_open_orders`
- the inputs used to emit `position_limit_reached`

`Bolt allocation state` is not a Bolt portfolio, execution ledger, or source of truth for fills, orders, positions, balances, average prices, or exposure.
It must be computed fresh for each decision evaluation.
It may be retained only as decision evidence.
It must not be event-sourced, reconciled, incrementally updated from fills or orders, or persisted as independent live-trading truth.

A future dashboard or analytics layer may maintain a separate append-only performance store, but that store is reporting-only and must never be submit authority.

### 4.2 Execution legs

Every strategy decision is modeled as one or more execution legs.
The current Phase 1 contract has exactly one implicit execution leg.

For the implicit Phase 1 leg:

- `venue` identifies the configured venue instance from TOML
- `venue_kind` identifies the venue family for that configured instance
- selected-market, mechanical, entry, pre-submit, and order-submission fields describe that same implicit leg

This is the `leg_count = 1` case of the execution-leg model, not a Bolt-wide single-venue architecture limit.
Future multi-venue strategies make execution legs explicit and repeat leg-scoped venue, market, allocation, and order evidence under the same `decision_trace_id`.

## 5. Market Selection Contract

### 5.1 Supported target kinds

The current frozen target-stack model supports only this live-trading target kind:

- `rotating_market`

Instrument targets are deferred until a future contract slice defines their configured-target shape, selected-market facts boundary, and event projection.

### 5.2 Strategy ownership

The strategy owns:

- when market selection runs
- the retry loop
- the blocked/degraded state

There is no standalone market-selection service.

### 5.3 Market-selection source of truth

Market-selection input order:

1. NautilusTrader-loaded venue and instrument state
2. Chainlink Data Streams runtime anchor for current `updown` `price_to_beat_value`
3. post-close Polymarket Gamma validation evidence for forensic comparison only

Current rule:

- bolt may query Chainlink Data Streams REST `GET /api/v1/reports?feedID=<feed_id>&timestamp=<boundary_unix>` for the selected market boundary timestamp
- the selected `feed_id` must come from the configured reference-data surface; raw feed-id ownership and catalog rules are tracked by the reference-data catalog slice
- for current first-live `updown` Polymarket scope, `event_page_slug` equals the selected CLOB market slug; this is scoped #244 evidence for sampled BTC/ETH 5m updown markets, not a universal Polymarket mapping rule
- bolt may call Gamma `GET /events?slug=<event_page_slug>` only after close to compare Gamma `eventMetadata.priceToBeat` against the Chainlink-derived runtime anchor for forensic validation
- Gamma must not be used as a runtime anchor, broad discovery source, order state, prices, `NT Portfolio` state, reference data, or strategy-side HTTP
- if the Chainlink Data Streams report is missing, non-numeric, non-positive, or ambiguous, market selection fails loud and the strategy remains non-trading

Current Polymarket loading contract:

- bolt derives NautilusTrader-native Polymarket instrument filters from the configured strategy targets
- for current `updown`, bolt installs a dynamic NautilusTrader `MarketSlugFilter` closure in the pinned `PolymarketDataClientConfig.filters`
- on each evaluation of that closure, it yields market slugs for the current and next cadence windows for each configured `underlying_asset`
- bolt invokes NautilusTrader's `request_instruments` path at startup and when the generated current/next slug pair changes under the NautilusTrader node clock
- normal retry ticks read NautilusTrader cache only
- if the `request_instruments` call for the current slug pair fails, retry that request at `target.retry_interval_seconds` until it succeeds or the slug pair changes
- `subscribe_new_markets` remains `false` in the current live-trading scope
- broad Polymarket instrument loading without target-derived filters is forbidden

Current `updown` slug derivation rule:

- slug format: `"{underlying_asset_lowercase}-updown-{cadence_slug_token}-{period_start_unix_seconds}"`
- `cadence_seconds` is the TOML-owned period duration
- `cadence_slug_token` is the TOML-owned slug segment for that period
- `now_unix_seconds` comes from the NautilusTrader node clock
- `current_period_start_unix_seconds = floor(now_unix_seconds / cadence_seconds) * cadence_seconds`
- `next_period_start_unix_seconds = current_period_start_unix_seconds + cadence_seconds`
- the current market-selection process yields exactly two market slugs per configured `updown` target on each evaluation:
  - current-period slug
  - next-period slug

This keeps the loading path inside the NautilusTrader Polymarket adapter rather than creating a second discovery system.

### 5.4 `active_or_next` semantics

For `market_selection_rule = "active_or_next"`:

- if exactly one loaded market satisfies the current role in the declared rotating market, select it
- if more than one loaded market satisfies the current role in the declared rotating market, fail with `ambiguous_selected_market` and do not evaluate the next role
- if zero loaded markets satisfy the current role in the declared rotating market, evaluate the next role
- if exactly one loaded market satisfies the next role in the declared rotating market, select it
- if more than one loaded market satisfies the next role in the declared rotating market, fail with `ambiguous_selected_market`
- if zero loaded markets satisfy the next role in the declared rotating market, fail with `no_selected_market`

For current `updown`:

- `current` means the role whose selected market time window satisfies `polymarket_market_start_timestamp_milliseconds <= market_selection_timestamp_milliseconds < polymarket_market_end_timestamp_milliseconds`

- `next` means the role whose selected market `polymarket_market_start_timestamp_milliseconds` is the smallest value greater than `market_selection_timestamp_milliseconds`

Prohibited behaviors:

- guessing among multiple matching loaded markets
- broadening search outside the declared rotating market
- selecting a fallback market
- silently suppressing ambiguity

### 5.5 Retry behavior

If a rotating-market target cannot be selected:

- emit `market_selection_result`
- remain non-trading
- retry at `target.retry_interval_seconds`
- no backoff

If market selection keeps failing for `target.blocked_after_seconds`:

- mark the strategy blocked/degraded
- continue retrying at `target.retry_interval_seconds` unless the strategy is stopped

Blocked/degraded operational behavior:

- the strategy continues to receive NautilusTrader callbacks
- the strategy must skip target-dependent evaluation and must not submit orders while blocked
- the strategy may continue market-selection retry attempts on its timer path

## 6. Target-Stack Data Model

The target stack has five exact in-process shapes.
They are separate boundaries:

- `configured_updown_target`
- `selected_market`
- `updown_selected_market_facts`
- `market_selection_result`
- `updown_market_mechanical_result`

Do not merge these shapes into a generic target envelope, market-selection framework, runtime gate service, or executable-order schema.

### 6.1 `configured_updown_target`

`configured_updown_target` is the runtime projection of one strategy-file `[target]` block for the current `updown` rotating-market scope.
It contains configuration only.

Exact fields:

- `configured_target_id`
- `target_kind`
- `venue`
- `venue_kind`
- `rotating_market_family`
- `underlying_asset`
- `cadence_seconds`
- `market_selection_rule`
- `retry_interval_seconds`
- `blocked_after_seconds`

Field constraints:

- `target_kind = "rotating_market"`
- `venue` is the exact strategy-file `venue` reference
- `venue_kind = "polymarket"` for the current `updown` scope
- `rotating_market_family = "updown"`
- `market_selection_rule = "active_or_next"`

Boundary:

- no selected-market identifiers
- no current/next role
- no generated market slugs
- no `event_page_slug` or `polymarket_event_slug`
- no price-to-beat fields
- no order, position, edge, readiness, or rejection fields

### 6.2 `selected_market`

The market-selection process must produce one concrete selected market before the strategy trades.
`selected_market` is identity and market-boundary data only.
It is not an observed-facts object and it does not include entry readiness or strategy decision fields.

#### Universal selected-market fields

Every selected market must contain:

- `target_kind`
- `venue`
- `venue_kind`

#### Current updown rotating-market fields

If `target_kind = "rotating_market"` and `rotating_market_family = "updown"`, the selected market must also contain:

- `rotating_market_family`
- `polymarket_condition_id`
- `polymarket_market_slug`
- `polymarket_question_id`
- `up_instrument_id`
- `down_instrument_id`
- `polymarket_market_start_timestamp_milliseconds`
- `polymarket_market_end_timestamp_milliseconds`

Definitions:

- `rotating_market_family` = `updown`
- `polymarket_condition_id` = the Polymarket condition identifier string for the selected market
- `polymarket_market_slug` = the Polymarket CLOB market slug used for instrument loading
- `polymarket_question_id` = the Polymarket question identifier string for the selected market
- `up_instrument_id` = the literal NautilusTrader instrument identifier string for the `Up` instrument
- `down_instrument_id` = the literal NautilusTrader instrument identifier string for the `Down` instrument
- `polymarket_market_start_timestamp_milliseconds` = selected market start timestamp in Unix milliseconds
- `polymarket_market_end_timestamp_milliseconds` = selected market end timestamp in Unix milliseconds

Boundary:

- no `event_page_slug` or `polymarket_event_slug`
- no `selected_market_observed_timestamp`
- no price-to-beat fields
- no `has_selected_market_open_orders`
- no entry-readiness summary field
- no generic market-status summary field

The strategy must not fetch venue metadata directly to fill missing selected-market fields.

### 6.3 `updown_selected_market_facts`

`updown_selected_market_facts` is the observed-facts layer for a selected `updown` market.
It contains the selected market plus the reference-price cluster needed by the current `binary_oracle_edge_taker`.

Exact fields:

- `selected_market`
- `selected_market_observed_timestamp`
- `price_to_beat_value`
- `price_to_beat_observed_timestamp`
- `price_to_beat_source`

Definitions:

- `selected_market` = the `selected_market` shape from Section 6.2
- `selected_market_observed_timestamp` = the timestamp when the selected market facts were observed
- `price_to_beat_value` = decoded Chainlink Data Streams benchmark price from `GET /api/v1/reports?feedID=<feed_id>&timestamp=<boundary_unix>`
- `price_to_beat_observed_timestamp` = the timestamp when `price_to_beat_value` was observed
- `price_to_beat_source` current launch-scope value:
  - `chainlink_data_streams.report_at_boundary`

Boundary:

- no `event_page_slug` or `polymarket_event_slug`
- no midpoint or spot fallback
- no open-order predicate
- no strategy edge, side, sizing, order, or rejection fields
- no entry-readiness summary field
- no generic market-status summary field

For the current `binary_oracle_edge_taker`, the price-to-beat cluster is required.

### 6.4 `market_selection_result`

`market_selection_result` is the market-selection output for a configured target.
Current and next are result roles, not object types.

Allowed variants:

- `current`
- `next`
- `failed`

`market_selection_timestamp_milliseconds` is the NautilusTrader node-clock Unix millisecond timestamp used for the market-selection attempt and for classifying a selected market as `current` or `next`.

`current` and `next` variants contain:

- `configured_updown_target`
- `market_selection_timestamp_milliseconds`
- `updown_selected_market_facts`

`selected_market` is reachable through `updown_selected_market_facts.selected_market`.
Do not duplicate it as a sibling field in a successful `market_selection_result`.

The `failed` variant contains:

- `configured_updown_target`
- `market_selection_timestamp_milliseconds`
- `market_selection_failure_reason`

Allowed `market_selection_failure_reason` values:

- `request_instruments_failed`
- `instruments_not_in_cache`
- `no_selected_market`
- `ambiguous_selected_market`
- `price_to_beat_unavailable`
- `price_to_beat_ambiguous`

The price-to-beat failure reasons belong here only because the current `updown` launch scope requires `updown_selected_market_facts` before market selection can succeed.

The `event_page_slug` used for post-close Gamma validation is mapping evidence only.
It is not a field on `selected_market` or `updown_selected_market_facts`.
For the current first-live `updown` Polymarket scope, scoped #244 evidence says it equals the selected CLOB market slug.
Future Polymarket market families must prove their own mapping rule before trading.

### 6.5 `updown_market_mechanical_result`

`updown_market_mechanical_result` is the family mechanical evaluation result for a selected `updown` market before strategy edge, side, sizing, and order construction.
It is not an order-readiness or pre-submit result.

Exact common fields:

- `updown_selected_market_facts`
- `seconds_to_market_end`
- `has_selected_market_open_orders`
- `updown_market_mechanical_outcome`
- `updown_market_mechanical_rejection_reason`

Allowed `updown_market_mechanical_outcome` values:

- `accepted`
- `rejected`

`updown_market_mechanical_rejection_reason` must be null when `updown_market_mechanical_outcome = "accepted"`.
It must be non-null when `updown_market_mechanical_outcome = "rejected"`.

Allowed `updown_market_mechanical_rejection_reason` values:

- `market_not_started`
- `market_ended`
- `selected_market_open_orders_present`

If multiple rejection conditions hold, the reported reason is the first matching value in this order:

1. `market_ended`
2. `market_not_started`
3. `selected_market_open_orders_present`

`has_selected_market_open_orders` must be false when `updown_market_mechanical_outcome = "accepted"`.
`has_selected_market_open_orders` must be true when `updown_market_mechanical_rejection_reason = "selected_market_open_orders_present"`.

Boundary:

- no selected side
- no edge metrics
- no sizing
- no order fields
- no `client_order_id`
- no pre-submit rejection reason
- no entry-readiness summary field
- no generic market-status summary field

## 7. Reference Data Contract

If an archetype requires reference data:

- the strategy file must declare it explicitly
- the root file must declare the keyed data venue explicitly
- the strategy subscribes directly through NautilusTrader data clients

There is no bolt-owned reference actor.

For the current `binary_oracle_edge_taker`:

- the declared reference-data instrument is subscribed as quote ticks
- the archetype derives spot-price input from that declared quote-tick stream

Reference-data resolution rule for validation:

- `resolvable` means that after NautilusTrader venue/instrument loading completes, the declared `instrument_id` exists in the NautilusTrader instrument cache for the referenced keyed venue
- `resolvable` does not require receiving a live quote before `just check` completes

### 7.1 Current `binary_oracle_edge_taker` pricing inputs

For the current `binary_oracle_edge_taker`, the reference stream and pricing inputs are mechanical:

- `spot_price`
  - derived from the latest two-sided midpoint on the configured reference-data role
  - midpoint formula: `(best_bid_price + best_ask_price) / 2`
  - if the latest quote tick does not contain both sides, midpoint is unavailable
  - if the latest midpoint sample is older than `target.retry_interval_seconds` seconds, the reference quote is stale
- `price_to_beat_source`
  - `chainlink_data_streams.report_at_boundary`

There is no fallback from missing price-to-beat metadata to midpoint.

### 7.2 Current `binary_oracle_edge_taker` realized volatility

The current realized-volatility estimator is defined as:

- input samples:
  - midpoint samples from the configured reference-data role
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

### 7.3 Current `binary_oracle_edge_taker` entry evaluation

The current entry evaluation is:

1. compute `fair_probability_up` from:
   - `spot_price`
   - `price_to_beat_value`
   - `seconds_to_market_end`
   - `realized_volatility`
2. current fair-probability formula:
   - `d2 = (ln(spot_price / price_to_beat_value) - (realized_volatility^2 / 2) * time_to_expiry_years) / (realized_volatility * sqrt(time_to_expiry_years))`
   - `fair_probability_up = standard_normal_cdf(d2)`
   - if `realized_volatility <= 0` or `time_to_expiry_years <= 0`, fair probability is unavailable and entry evaluation skips as not ready
3. derive side success probability:
   - `Up`: `fair_probability_up`
   - `Down`: `1.0 - fair_probability_up`
4. derive executable entry cost:
   - `Up`: current best ask on `up_instrument_id`
   - `Down`: current best ask on `down_instrument_id`
5. compute edge:
   - under the current NT CLOB V2 candidate pin, Polymarket fees are not proven by a pre-entry Bolt fee-rate path; live entry remains blocked until the runtime contract defines how match-time fees are represented in readiness and evidence
   - `expected_edge_basis_points` must account for the applicable Polymarket fee behavior before live entry is allowed
   - current rule: `worst_case_edge_basis_points = expected_edge_basis_points`
6. side selection:
   - choose the single side with the higher `worst_case_edge_basis_points`
   - if neither side is strictly greater than `parameters.edge_threshold_basis_points`, skip
   - if both sides are equal, skip
7. sizing:
   - notional fields are gross collateral entry-cost terms before fees; under the current NT CLOB V2 candidate pin, Polymarket instruments use `pUSD` collateral semantics, not the old adapter's `USDC` assumption
   - `entry_filled_notional` is summed across both Up and Down NautilusTrader instruments of the selected market from confirmed position state: `position.quantity * position.avg_px_open`
   - `open_entry_notional` is summed across both Up and Down NautilusTrader instruments of the selected market from open/inflight buy-order state: `order.leaves_qty * order.price`
   - remaining capacity = `parameters.maximum_position_notional - entry_filled_notional - open_entry_notional`
   - `sizing_cap = min(parameters.order_notional_target, strategy_remaining_entry_capacity, root risk.default_max_notional_per_order)`
   - if `strategy_remaining_entry_capacity <= 0`, skip with `position_limit_reached`
   - if the selected side clears the edge threshold, `sized_notional = sizing_cap`
8. quantity conversion:
   - `quantity_raw = sized_notional / executable_entry_cost`
   - convert with NautilusTrader instrument precision via `instrument.try_make_qty(quantity_raw, Some(true))`
   - local rejection if conversion fails or the resulting quantity is not positive
9. order construction:
   - entry price = current best ask of the selected side instrument
   - entry order uses the locked archetype order combination from the strategy schema

### 7.4 Current `binary_oracle_edge_taker` exit evaluation

The current exit rule is intentionally thin:

- no blind exits
- no bolt-owned exit engine
- exit mechanical availability is classified from NautilusTrader and venue-confirmed facts before any strategy exit decision
- strategy may submit an exit only when exit order mechanical evaluation accepts and a contract-defined active exit predicate chooses `exit`
- if NautilusTrader state or order construction inputs are insufficient, do not submit; emit decision/local-rejection evidence with the exact mechanical reason
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

For current Polymarket trading, because the pinned adapter rejects reduce-only orders:

- before emitting `exit_order_submission`, the strategy must read `authoritative_position_quantity` and `authoritative_sellable_quantity`
- if intended exit quantity exceeds `authoritative_sellable_quantity`, emit `exit_pre_submit_rejection`
- required local rejection reason: `exit_quantity_exceeds_sellable_quantity`
- in that case, do not submit an order

### 8.5 Strategy modularity boundary

Bolt-v3 must not port the current monolithic strategy shape directly.
The current `binary_oracle_edge_taker` behavior is the behavioral reference, not the file or module structure to copy.

The strategy actor orchestrates NautilusTrader interaction and runtime state updates.
It must not own every strategy concern internally.

Pricing, reference-data fusion, instrument filter, risk and sizing, decision evaluation, and execution mapping must live behind separately testable module boundaries.
Examples of concerns that belong outside the strategy actor body:

- active market selection, slug / cadence / asset mapping, and instrument identity
- Chainlink / reference feed handling, fused reference price, staleness, and timestamps
- strategy input snapshot assembly
- fair probability, realized volatility, kurtosis adjustment, theta scaling, and uncertainty-band math
- notional caps, exposure limits, sizing, and forced-flat predicates
- entry / exit decision evaluation from typed inputs to typed outputs
- conversion of accepted decisions into NautilusTrader-native order construction inputs

The strategy actor may compose these modules, maintain actor-local runtime state, emit decision evidence, and call the execution boundary.
It must remain thin enough that pricing, reference data, risk, decision, and execution behavior can be tested without running NautilusTrader.

## 9. Forensic Event Contract

### 9.1 Principle

NautilusTrader-native observability first.

bolt adds only:

- minimal structured strategy-decision events
- mirrored readable logs

Raw NautilusTrader facts are not re-modeled as bolt facts.
When NautilusTrader already provides a type, field, enum, identifier, timestamp, order state, report, or lifecycle concept, bolt documentation, emitted evidence, and persisted records use the NautilusTrader name unless the value is a bolt-derived calculation.
Derived bolt fields must identify the NautilusTrader source fact by identifier, timestamp, and type-specific key sufficient to find the raw record in local evidence.

For every NautilusTrader-owned fact, the canonical evidence name is the pinned Rust API path plus the Rust field or method name.
This rule applies across all bolt configuration mappings, structured decision events, raw capture records, logs, tests, and docs.
It is not limited to market data or order identifiers.

Examples are illustrative, not exhaustive:

- `nautilus_model::identifiers::InstrumentId`
- `nautilus_model::data::QuoteTick::instrument_id`
- `nautilus_model::data::QuoteTick::ts_event`
- `nautilus_model::events::order::OrderEventAny::venue_order_id()`
- `nautilus_model::reports::OrderStatusReport`

Configuration may use the corresponding snake-case field key only when it maps one-to-one to that NautilusTrader Rust name.
For example, a TOML `instrument_id` field maps to `nautilus_model::identifiers::InstrumentId`.
Aliases and renamed timestamps are forbidden for NautilusTrader-owned facts.

Before implementation is considered complete, every structured decision-event field and every TOML field that represents a NautilusTrader-owned fact must be audited against the pinned NautilusTrader Rust API path.
Fields that fail the one-to-one naming rule must be renamed before launch unless they are explicitly documented as bolt-derived calculations or venue/product facts not modeled by NautilusTrader.

Save broadly, decide narrowly:

- for every configured venue, target, and instrument that bolt activates, bolt subscribes to every NautilusTrader data, execution, order, position, account, report, and lifecycle stream exposed by the pinned Rust APIs for that activated scope
- if the pinned adapter does not expose a stream, the stream is recorded as unavailable evidence rather than silently ignored
- local evidence captures every NautilusTrader fact that reaches the live node through those broad subscriptions, and every NautilusTrader fact bolt emits, submits, or reads
- raw capture preserves NautilusTrader-native names, values, timestamps, identifiers, and event/report boundaries
- broad subscription and raw capture are outside the submit-critical hot path
- hot-path strategy decisions read only the minimal NautilusTrader cache, portfolio, risk, quote, book, clock, and configuration facts required by the current decision contract
- structured decision events stay compact and contain only strategy decisions, mechanical classifications, and derived fields needed to explain a decision
- structured decision events must not be the only owner of a raw NautilusTrader fact used by a decision
- raw capture is evidence and replay material, not submit authority; submit authority remains NautilusTrader cache, portfolio, risk, execution, and venue-confirmed state

There is no generic event framework.

### 9.2 Event model

There is a small fixed set of concrete event types.

Every event includes the same common required fields.
Each event type includes additional fixed fields specific to that event.
Where an event may be emitted for more than one target kind, target-kind-specific fields are conditional on `target_kind`.

### 9.3 Common required fields

These fields are required on every structured decision event:

- `schema_version`
- `decision_event_type`
- `ts_event`
- `decision_trace_id`
- `strategy_instance_id`
- `strategy_archetype`
- `trader_id`
- `venue`
- `venue_kind`
- `runtime_mode`
- `release_id`
- `config_hash`
- `nautilus_trader_revision`
- `configured_target_id`

Definitions:

- `schema_version`
  - the event-schema version
  - current value: `1`
- `ts_event`
  - NautilusTrader event timestamp for the decision custom-data value
  - serialized as the pinned NautilusTrader `UnixNanos` integer value
- `decision_trace_id`
  - generated once at the first market-selection or evaluation step for a potential trading lifecycle
  - format: UUID4 string
  - reused on all subsequent decision events for that lifecycle
  - for failed rotating-market selection attempts, one trace covers the current/next slug pair across retries
  - when the current/next slug pair changes, the next market-selection attempt starts a new trace
  - the lifecycle ends when the strategy reaches a terminal outcome for that opportunity: skip, local submit rejection, or flat-after-exit
- `strategy_archetype`
  - the exact `strategy_archetype` value from the strategy file
- `trader_id`
  - the exact root-file `trader_id` value
- `venue`
  - the keyed trading venue reference from the strategy file
  - not a reference-data venue key
- `venue_kind`
  - the exact `kind` value from the configured venue block referenced by `venue`
  - for the current `updown` scope, `polymarket`
  - describes the implicit execution leg in the Phase 1 execution-leg model
- `runtime_mode`
  - the exact root-file `[runtime].mode` value
- `release_id`
  - the exact deployed release directory name selected by deploy automation
  - current deployment rule: release directory names are the git commit SHA string for the built artifact
- `config_hash`
  - SHA-256 of the concatenation of:
    1. root-file bytes with line endings normalized to LF
    2. each listed strategy-file bytes in root `strategy_files` order, with line endings normalized to LF
  - if a file starts with a UTF-8 byte order mark, strip it before hashing
  - each normalized file byte sequence is hashed as if it ends with exactly one LF
  - files are concatenated with no separator
  - the emitted digest is lowercase hexadecimal
  - file paths are not included
- `nautilus_trader_revision`
  - the pinned git revision string from `Cargo.toml`
  - current value: `38b912a8b0fe14e4046773973ff46a3b798b1e3e`
- `configured_target_id`
  - the exact configured target identifier from the strategy configuration
  - reused on all decision events for the same configured target

### 9.4 Event type list

Event types are:

- `market_selection_result`
- `entry_evaluation`
- `entry_order_submission`
- `entry_pre_submit_rejection`
- `exit_evaluation`
- `exit_order_submission`
- `exit_pre_submit_rejection`

Extensions later are allowed, but these are the fixed current types.

`decision_event_type` values are the exact lowercase underscore-separated strings listed in this section.
Event schema version is independent of root-file and strategy-file schema versions.
Event-schema version is `1`.

### 9.5 Event-specific required fields

#### `market_selection_result`

Required additional fields:

- `target_kind`
- `market_selection_timestamp_milliseconds`
- `market_selection_outcome`
- `market_selection_failure_reason`

`market_selection_failure_reason` must be null when `market_selection_outcome` is `current` or `next`.
`market_selection_failure_reason` must be non-null when `market_selection_outcome = "failed"`.
`market_selection_timestamp_milliseconds` is the NautilusTrader node-clock Unix millisecond timestamp used to classify the selected market as `current` or `next`.

Allowed `market_selection_outcome` values are the variant names defined for `market_selection_result` in Section 6.4 (`current`, `next`, `failed`).

Allowed `market_selection_failure_reason` values are defined in Section 6.4.

If `target_kind = "rotating_market"`, the event must also contain these configured target fields:

- `rotating_market_family`
- `underlying_asset`
- `cadence_seconds`
- `market_selection_rule`
- `retry_interval_seconds`
- `blocked_after_seconds`

If `target_kind = "rotating_market"` and `rotating_market_family = "updown"` and market selection succeeds, the event must also contain these selected market and selected market facts fields:

- `polymarket_condition_id`
- `polymarket_market_slug`
- `polymarket_question_id`
- `up_instrument_id`
- `down_instrument_id`
- `selected_market_observed_timestamp`
- `polymarket_market_start_timestamp_milliseconds`
- `polymarket_market_end_timestamp_milliseconds`
- `price_to_beat_value`
- `price_to_beat_observed_timestamp`
- `price_to_beat_source`

#### `entry_evaluation`

The current `entry_evaluation` event is defined for `target_kind = "rotating_market"` and `rotating_market_family = "updown"`.

`entry_evaluation` is emitted only after `market_selection_result` has succeeded for the same configured target and decision trace.
Market-selection failure is represented by `market_selection_result` only and does not emit `entry_evaluation`.
If market selection succeeds and `updown` mechanical evaluation rejects, `entry_evaluation` is still emitted with `entry_decision = "no_action"` and `entry_no_action_reason = "updown_market_mechanical_rejection"`.

Required additional fields:

- `updown_side`
- `entry_decision`
- `entry_no_action_reason`
- `seconds_to_market_end`
- `has_selected_market_open_orders`
- `updown_market_mechanical_outcome`
- `updown_market_mechanical_rejection_reason`
- `entry_filled_notional`
- `open_entry_notional`
- `strategy_remaining_entry_capacity`
- `archetype_metrics`

`entry_no_action_reason` must be null when `entry_decision = "enter"`.
`entry_no_action_reason` must be non-null when `entry_decision = "no_action"`.
`updown_side` must be non-null when `entry_decision = "enter"`.
`updown_side` must be null when `entry_decision = "no_action"` unless a later archetype-specific contract explicitly allows retaining a computed side for no-action evidence.
`updown_market_mechanical_rejection_reason` must be null when `updown_market_mechanical_outcome = "accepted"`.
`updown_market_mechanical_rejection_reason` must be non-null when `updown_market_mechanical_outcome = "rejected"`.
`has_selected_market_open_orders` must be false when `updown_market_mechanical_outcome = "accepted"`.
`has_selected_market_open_orders` must be true when `updown_market_mechanical_rejection_reason = "selected_market_open_orders_present"`.
`entry_decision` must be `no_action` when `updown_market_mechanical_outcome = "rejected"`.
`entry_decision = "enter"` requires `updown_market_mechanical_outcome = "accepted"`.
If `entry_no_action_reason = "updown_market_mechanical_rejection"`, `updown_market_mechanical_outcome` must be `rejected` and `updown_market_mechanical_rejection_reason` must be non-null.
If `entry_no_action_reason` is `missing_reference_quote`, `stale_reference_quote`, `fee_rate_unavailable`, `fair_probability_unavailable`, `insufficient_edge`, or `position_limit_reached`, `updown_market_mechanical_outcome` must be `accepted` and `updown_market_mechanical_rejection_reason` must be null.
If `entry_no_action_reason = "position_limit_reached"`, `strategy_remaining_entry_capacity <= 0`.
`entry_filled_notional`, `open_entry_notional`, and `strategy_remaining_entry_capacity` are gross collateral entry-cost terms before fees. The current CLOB V2 pin-change slice has not yet renamed the external schema fields from their historical USDC wording to pUSD collateral wording.
`entry_filled_notional` and `open_entry_notional` are summed across both Up and Down NautilusTrader instruments of the selected market.

Allowed `entry_decision` values:

- `enter`
- `no_action`

Allowed `entry_no_action_reason` values:

- `updown_market_mechanical_rejection`
- `missing_reference_quote`
- `stale_reference_quote`
- `fee_rate_unavailable`
- `fair_probability_unavailable`
- `insufficient_edge`
- `position_limit_reached`

Allowed `updown_market_mechanical_outcome` values are defined in Section 6.5.
Allowed `updown_market_mechanical_rejection_reason` values and the first-match ordering rule are defined in Section 6.5.

For the current `binary_oracle_edge_taker`, `archetype_metrics` must contain:

- `spot_price`
- `price_to_beat_value`
- `price_to_beat_source`
- `realized_volatility`
- `expected_edge_basis_points`
- `worst_case_edge_basis_points`
- `fee_rate_basis_points`
- `reference_quote_ts_event`

Definitions for the current `binary_oracle_edge_taker` metrics:

- `spot_price`
  - latest valid midpoint from the configured reference-data role
- `price_to_beat_value`
  - selected market reference price used for updown evaluation
- `realized_volatility`
  - output of the realized-volatility estimator defined in Section 7.2
- `expected_edge_basis_points`
  - selected-side edge from Section 7.3 before any additional uncertainty haircut
- `worst_case_edge_basis_points`
  - selected-side edge used for thresholding and sizing
  - current rule defined in Section 7.3 step 5
- `fee_rate_basis_points`
  - selected-side fee rate used by Section 7.3
- `reference_quote_ts_event`
  - `ts_event` of the quote tick which produced `spot_price`

For the current `binary_oracle_edge_taker`, all listed metric keys must be present.
When `entry_decision` is `no_action`, values available at evaluation time must be non-null, and unavailable values must be serialized as explicit null rather than omitted or synthesized.

Allowed `price_to_beat_source` values:

- `chainlink_data_streams.report_at_boundary`

#### `entry_order_submission`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_id`
- `side`
- `price`
- `quantity`
- `is_quote_quantity`
- `is_post_only`
- `is_reduce_only`

This event records the exact NautilusTrader-native order semantics for a submit attempt.

This event must also contain:

- `client_order_id`

`client_order_id` is required and non-null.
This event is emitted only after a NautilusTrader-native order object has been constructed.

#### `entry_pre_submit_rejection`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_id`
- `side`
- `price`
- `quantity`
- `is_quote_quantity`
- `is_post_only`
- `is_reduce_only`
- `entry_pre_submit_rejection_reason`

This event must also contain:

- `client_order_id`

`client_order_id` may be null when local rejection happens before a NautilusTrader order object is constructed.

Allowed `entry_pre_submit_rejection_reason` values:

- `invalid_price`
- `invalid_quantity`
- `exceeds_order_notional_cap`

Pre-order states such as missing selected market or market-selection failure are not submit-rejection events.
Market-selection failure is represented by `market_selection_result` only.
No `entry_evaluation`, `entry_pre_submit_rejection`, or `entry_order_submission` event is emitted for that failed entry path.

#### `exit_evaluation`

`exit_evaluation` is emitted only when there is exit exposure to evaluate.
If authoritative position quantity is zero and there is no open exit-order state for the strategy, selected market, and side, no `exit_evaluation` is emitted.

Required additional fields:

- `authoritative_position_quantity`
- `authoritative_sellable_quantity`
- `open_exit_order_quantity`
- `uncovered_position_quantity`
- `exit_order_mechanical_outcome`
- `exit_order_mechanical_rejection_reason`
- `exit_decision`
- `exit_decision_reason`
- `archetype_metrics`

For the current `binary_oracle_edge_taker`, `archetype_metrics` may be an empty object.

`open_exit_order_quantity` is the sum of open sell-order quantity for the same strategy, selected market, and held side.
`uncovered_position_quantity = max(authoritative_position_quantity - open_exit_order_quantity, 0)`.
In `exit_evaluation`, these quantity fields are required keys. They must be numeric when confirmed. They must be explicit null only when the corresponding unconfirmed mechanical rejection reason applies.

Exit reason ownership:

- `exit_order_mechanical_rejection_reason` owns facts that prevent constructing a valid NautilusTrader-native sell order from current NautilusTrader / venue state.
- `exit_decision_reason` owns the strategy decision after exit order mechanical evaluation accepts.
- post-submit venue or adapter rejection is NautilusTrader order lifecycle evidence linked by `client_order_id`, not an `exit_evaluation` reason.

Allowed `exit_order_mechanical_outcome` values:

- `accepted`
- `rejected`

Allowed `exit_order_mechanical_rejection_reason` values:

- `position_quantity_unconfirmed`
- `open_exit_order_quantity_unconfirmed`
- `open_exit_order_quantity_covers_position`
- `sellable_quantity_unconfirmed`
- `sellable_quantity_zero`
- `exit_bid_unavailable`
- `exit_quantity_invalid`
- `exit_price_invalid`

`exit_order_mechanical_rejection_reason` must be null when `exit_order_mechanical_outcome = "accepted"`.
`exit_order_mechanical_rejection_reason` must be non-null when `exit_order_mechanical_outcome = "rejected"`.
`exit_order_mechanical_outcome = "rejected"` requires `exit_decision = "hold"` and `exit_decision_reason = "exit_order_mechanical_rejection"`.
If multiple exit mechanical rejection conditions hold, the reported reason is the first matching value in the allowed-value order above.
`position_quantity_unconfirmed` is used when authoritative position quantity cannot be confirmed from NautilusTrader / venue state.
`open_exit_order_quantity_unconfirmed` is used when open sell-order quantity cannot be confirmed from NautilusTrader state.
`open_exit_order_quantity_covers_position` is used when `authoritative_position_quantity > 0` and `open_exit_order_quantity >= authoritative_position_quantity`.
`sellable_quantity_unconfirmed` is used when sellable quantity cannot be confirmed from NautilusTrader / venue state.
`sellable_quantity_zero` is used when `authoritative_sellable_quantity = 0` after sellable quantity is confirmed.
`exit_bid_unavailable` is used when no executable bid is available for the held side.
`exit_quantity_invalid` is used when NautilusTrader instrument precision conversion produces a zero or invalid sell quantity.
`exit_price_invalid` is used when the executable exit price is invalid under NautilusTrader instrument rules.

Allowed `exit_decision` values:

- `exit`
- `hold`

Allowed `exit_decision_reason` values for the current `binary_oracle_edge_taker`:

- `exit_order_mechanical_rejection`
- `active_exit_not_defined`

For the current `binary_oracle_edge_taker`, no active exit predicate is defined. When `exit_order_mechanical_outcome = "accepted"`, `exit_decision` must be `hold` and `exit_decision_reason` must be `active_exit_not_defined`.
`exit_decision = "exit"` is not emitted for the current `binary_oracle_edge_taker` until a later contract slice defines an active exit predicate and its allowed `exit_decision_reason` values.

#### `exit_order_submission`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_id`
- `side`
- `price`
- `quantity`
- `is_quote_quantity`
- `is_post_only`
- `is_reduce_only`
- `authoritative_position_quantity`
- `authoritative_sellable_quantity`
- `open_exit_order_quantity`
- `uncovered_position_quantity`

This event must also contain:

- `client_order_id`

`client_order_id` is required and non-null.
This event is emitted only after a NautilusTrader-native order object has been constructed.
For the current `binary_oracle_edge_taker`, this event is not emitted until a later contract slice defines an active exit predicate.

#### `exit_pre_submit_rejection`

Required additional fields:

- `order_type`
- `time_in_force`
- `instrument_id`
- `side`
- `price`
- `quantity`
- `is_quote_quantity`
- `is_post_only`
- `is_reduce_only`
- `authoritative_position_quantity`
- `authoritative_sellable_quantity`
- `open_exit_order_quantity`
- `uncovered_position_quantity`
- `exit_pre_submit_rejection_reason`

This event must also contain:

- `client_order_id`

`client_order_id` may be null when local rejection happens before a NautilusTrader order object is constructed.
For `exit_order_submission` and `exit_pre_submit_rejection`, `authoritative_position_quantity`, `authoritative_sellable_quantity`, `open_exit_order_quantity`, and `uncovered_position_quantity` must be non-null.
For the current `binary_oracle_edge_taker`, this event is not emitted until a later contract slice defines an active exit predicate.

Allowed `exit_pre_submit_rejection_reason` values:

- `exit_quantity_exceeds_sellable_quantity`
- `invalid_quantity`

### 9.6 Transport

Primary machine-readable decision evidence:

- structured decision events encoded as NautilusTrader registered custom-data values

For live trading, these events are persisted to the local catalog directory as machine-readable evidence.

Registration and persistence mechanism:

- bolt registers the fixed decision-event custom-data types with NautilusTrader at startup
- registration happens before any strategy can emit decision evidence
- the current live-trading persistence path uses one canonical bolt call site which hands registered custom-data values to NautilusTrader's catalog API
- `[persistence.streaming]` supplies the catalog protocol, flush interval, replace behavior, and no-rotation policy for this call site
- `[persistence].runtime_capture_start_poll_interval_milliseconds` supplies the raw-capture startup poll interval while startup messages are buffered before NT reports running
- bolt does not implement a second writer, subscriber-writer loop, or parallel persistence path

Every decision event must be constructed as the fixed registered NautilusTrader custom-data value before emission.
The constructed value must include all Section 9.3 common fields and all Section 9.5 event-type-specific required fields, populated according to their stated nullability rules.

For order-submission events, the constructed value must be accepted by the single canonical in-process persistence handoff before the NautilusTrader order submit may proceed.
Order-submission events are `entry_order_submission` and `exit_order_submission`.

For pre-submit rejection events, the constructed value must be accepted by the single canonical in-process persistence handoff before the local rejection path completes.
Pre-submit rejection events are `entry_pre_submit_rejection` and `exit_pre_submit_rejection`.

Accepted handoff means the registered custom-data value has been accepted by the canonical bounded in-process catalog handoff without registration, encoding, capacity, or path rejection.
Accepted handoff does not require durable catalog flush completion.
Full handoff capacity is a handoff failure.
An unbounded queue is forbidden.
If construction, encoding, registration lookup, or accepted handoff fails for an order-submission event, the current submit is blocked.
If construction, encoding, registration lookup, or accepted handoff fails for a pre-submit rejection event, the strategy enters `persistence_failed` before completing the local rejection path.

For all other decision events, construction or handoff failure follows the Section 9.7 `persistence_failed` behavior: loud structured log, emitting strategy blocked/degraded, and no future submits until recovery.
If the process crashes after order submit but before durable catalog flush, the venue order remains live and local decision evidence for that submit may be absent; recovery authority comes from NautilusTrader state reconciliation and venue-confirmed state.

If a future NautilusTrader pin exposes a Rust live-node bus-to-catalog writer with equivalent failure behavior, migration to that path is a dedicated pin-update slice and not an implicit behavior change.

Fixed custom-data type contract:

- each decision event type in Section 9.4 has one Rust custom-data type
- each type must be registered with NautilusTrader before startup can trade
- every common field from Section 9.3 is represented on every type
- event-specific fields from Section 9.5 are represented on the corresponding type
- required event fields are non-optional Rust fields
- nullable event fields are `Option` fields and must serialize as explicit null when absent
- the NautilusTrader custom-data `ts_event` field is the event timestamp
- the NautilusTrader custom-data `ts_init` field is the timestamp when bolt constructs the custom-data value

Join rule:

- decision events join to NautilusTrader-native execution events through `client_order_id`
- if a local rejection occurs before order creation and `client_order_id` is null, the terminal local-rejection event is joined by `decision_trace_id` only
- `venue_order_id` is owned by NautilusTrader-native execution events once the venue acknowledges the order
- if NautilusTrader uses a different canonical Rust field name for a joined fact at the pinned revision, the NautilusTrader name wins and bolt aliases are forbidden

Readable mirror:

- logs containing the same `decision_trace_id` and key fields

No strategy may invent its own ad hoc forensic schema.

### 9.7 Catalog Failure Behavior

Decision-event persistence is a live-trading safety gate.

If decision-event construction, accepted handoff, or durable catalog persistence fails because of registration, encoding, write, flush, rotation, permission, path, or capacity error:

- emit a loud structured log with `decision_trace_id` when available
- mark the emitting strategy blocked/degraded with reason `persistence_failed`
- do not submit new orders while that strategy is in `persistence_failed`
- continue allowing market-selection retries and non-order callbacks
- require operator intervention or process restart to clear the state

`just check` Phase 2 must prove catalog write/read readiness by round-tripping a registered test decision-event custom-data value in the configured catalog directory.

## 10. Local Disk and Later Archival

Before live trading:

- local machine-readable decision evidence must exist
- local machine-readable raw NautilusTrader evidence must exist
- local logs must exist
- local evidence must be sufficient for reconstruction without relying on Amazon Simple Storage Service
- machine-readable decision evidence and raw NautilusTrader evidence live in the configured catalog directory

Raw NautilusTrader capture scope:

- every NautilusTrader market-data record from every subscribed or requested stream exposed by the pinned adapter for the activated venue, target, and instrument scope
- every NautilusTrader execution command, execution report, order event, position event, account event, and lifecycle message visible to bolt through the pinned Rust APIs
- every NautilusTrader cache, portfolio, risk, and execution state snapshot explicitly read to compute a strategy decision
- every NautilusTrader adapter configuration value after TOML-to-NT mapping, excluding secrets
- unavailable subscribed-stream attempts, including the NautilusTrader Rust API path or adapter capability that was missing

Broad capture callbacks must hand off raw facts to the catalog path without performing strategy evaluation, order construction, or broad synchronous scans.
The submit-critical hot path must not wait on non-required broad-capture streams, historical backfill, archive export, or dashboard/reporting work.

Raw capture must preserve the NautilusTrader Rust API path, type name, field names, identifier values, `ts_event`, `ts_init` when present, and the pinned `nautilus_trader_revision`.
If a raw fact does not have `ts_event` or `ts_init`, capture uses the closest NautilusTrader-provided timestamp and records the field name that supplied it.
If no NautilusTrader timestamp is available, capture records the bolt node-clock capture timestamp as a bolt capture timestamp, not as a replacement NautilusTrader timestamp.

The catalog writer configured by `[persistence]` and `[persistence.streaming]` is the single local persistence path for both structured decision events and raw NautilusTrader capture.
Runtime-capture timing policy used by that path must come from `[persistence]`, not from capture-worker literals.
bolt must not add a second raw-data writer, side database, subscriber-writer loop, or parallel persistence path.

Raw capture failure is a persistence failure.
If raw capture construction, encoding, accepted handoff, or durable catalog persistence fails because of registration, schema, write, flush, rotation, permission, path, or capacity error, Section 9.7 applies.

Immediate follow-up:

- export/archive those artifacts to Amazon Simple Storage Service

## 11. Deploy / Runtime Contract

### 11.1 Required controls before live trading

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

- `release_id`
- `git_commit_sha`
- `nautilus_trader_revision`
- `binary_sha256`
- `cargo_lock_sha256`
- `config_hash`
- `build_profile`
- `artifact_sha256`

Runtime rule:

- bolt reads `release_id` from that release manifest at startup
- bolt does not derive release identity from process working directory or ad hoc path parsing
- bolt does not perform a second manifest-signature verification step in the current live-trading scope; trust comes from the verified artifact set plus the deploy-identity-controlled release directory
- deploy automation verifies every `artifact_sha256` entry before startup
- bolt verifies that manifest `nautilus_trader_revision` matches the compiled pin before startup can trade

### 11.3 Required writable paths

The allow-list must include exactly the paths bolt needs to write:

- catalog directory
- runtime temporary directory used by the service wrapper

Current bolt-v3 config deliberately does not accept `log_directory` or `state_directory` fields because the pinned NT live API does not expose a supported wiring path for them. They must not appear in the write allow-list until a future slice adds real wiring and tests.

No other write path is allowed.

### 11.4 Current systemd policy

The current panic gate must run against a concrete systemd policy.
The accepted policy for the current live-trading scope is:

- `Restart=on-failure`
- `RestartSec=5s`
- `StartLimitIntervalSec=300s`
- `StartLimitBurst=3`
- `LimitCORE=0`

If deploy automation uses a different policy, live trading is blocked until this section and the panic-gate evidence are updated together.

### 11.5 NautilusTrader pin governance

The NautilusTrader pin is part of the runtime contract because bolt is a thin Rust layer over the pinned NautilusTrader behavior.

Governance rules:

- Cargo dependency metadata and lock/build metadata are the dependency source of truth
- Section 9.3 records the same `nautilus_trader_revision` value for emitted decision evidence and must match the compiled pin
- the release manifest `nautilus_trader_revision` must match the compiled pin before startup can trade
- a NautilusTrader pin change is a dedicated verification slice, not an incidental dependency update
- each pin-change slice must update the recorded revision, rerun the CLOB V2 readiness gate, rerun the panic gate test matrix, and update the contract ledger
- `just check` Phase 1 must fail if the recorded Section 9.3 revision disagrees with the Cargo dependency revision
- startup verification must fail if the compiled pin disagrees with the release manifest `nautilus_trader_revision`

### 11.6 Controlled-connect and controlled-disconnect boundary

The bolt-v3 build path returns a `LiveNode` in `Idle` state with NT data and execution clients registered but not connected. NT's connect dispatchers (`NautilusKernel::connect_data_clients` and `NautilusKernel::connect_exec_clients`) and NT's disconnect dispatcher (`NautilusKernel::disconnect_clients`) are reachable from bolt-v3 only through the explicit `connect_bolt_v3_clients` and `disconnect_bolt_v3_clients` boundaries defined in `src/bolt_v3_live_node.rs`.

Controlled-connect boundary contract:

- opt-in: `build_bolt_v3_live_node` and its `_with` / `_with_summary` siblings do not invoke this boundary; a caller must call it explicitly on a node previously returned by one of those builders
- bounded: the dispatched engine-level connect futures are wrapped in `tokio::time::timeout` driven by `nautilus.timeout_connection_seconds`; on timeout the boundary returns `BoltV3LiveNodeError::ConnectTimeout { timeout_seconds }` and the caller owns subsequent disconnect/teardown via `disconnect_bolt_v3_clients`
- dispatch + connected check: the pinned NT `DataEngine::connect` and `ExecutionEngine::connect` dispatchers swallow individual client `connect()` errors and only log them, so after the dispatch returns the boundary consults `NautilusKernel::check_engines_connected()` and returns `BoltV3LiveNodeError::ConnectIncomplete` if any registered client did not transition to `is_connected`; this slice intentionally keeps the error variant generic and does not synthesize a per-client failure list
- not NT cache or instrument readiness: the boundary does not gate on NT cache contents, instrument-availability checks, or any readiness predicate beyond `kernel.check_engines_connected()`; cache/instrument readiness is owned by a future slice
- no copying NT private drain logic: the boundary does not copy or reimplement NT's private `flush_pending_data`, `drive_with_event_buffering`, or runner/channel internals; it strictly composes the public `kernel.connect_*` and `kernel.check_engines_connected` API surface
- pinned-NT-only: the boundary reaches NT only through `LiveNode::kernel_mut().connect_data_clients`, `LiveNode::kernel_mut().connect_exec_clients`, and `LiveNode::kernel().check_engines_connected` (the pinned NT controlled-connect API surface)
- no-trade: the boundary never enters NT's runner loop, never invokes NT's trader entrypoint, never registers strategies, never selects markets, never constructs orders, never submits orders, and never invokes any user-level subscription API; `NodeState` therefore remains in whatever state the node was in before the call (typically `Idle`)
- credential-log filter preserved: in a bolt-v3-only process, NT's first-wins logger has already been initialized by the bolt-v3 `LoggerConfig` passed through `LiveNodeBuilder::build`, so the `NT_CREDENTIAL_LOG_MODULES` filter remains active during the connect dispatch; production callers preserve this first-initializer ordering

Errors from individual NT client `connect()` calls are surfaced via NT's logger; NT's engine-level dispatchers in `nautilus_data::engine::DataEngine::connect` and `nautilus_execution::engine::ExecutionEngine::connect` log individual `Err` values rather than propagating them. The bolt-v3 boundary returns `Ok(())` only when both dispatchers have returned within the configured bound **and** `kernel.check_engines_connected()` returns true. Otherwise it returns `ConnectTimeout` or `ConnectIncomplete` and the caller is expected to drive `disconnect_bolt_v3_clients` to drain any partially-connected NT clients.

Controlled-disconnect boundary contract:

- recovery counterpart to controlled-connect: callers should invoke `disconnect_bolt_v3_clients` after a `ConnectTimeout` or `ConnectIncomplete` to drain any partially-connected NT clients under a bounded timeout
- bounded: the `kernel.disconnect_clients` future is wrapped in `tokio::time::timeout` driven by `nautilus.timeout_disconnection_seconds`; on timeout the boundary returns `BoltV3LiveNodeError::DisconnectTimeout { timeout_seconds }`
- error-propagating: NT's engine-level disconnect aggregator returns `anyhow::Result<()>`, and the boundary surfaces any `Err(..)` as `BoltV3LiveNodeError::DisconnectFailed(error)` rather than silently swallowing it
- failure recovery: pinned NT's `NautilusKernel::disconnect_clients` calls data-engine disconnect before execution-engine disconnect and can short-circuit on a data-engine `Err`; after `DisconnectFailed`, the `LiveNode` is in an indeterminate cleanup state and production recovery should rebuild a fresh `LiveNode` rather than assuming every client disconnected
- pinned-NT-only: the boundary reaches NT only through `LiveNode::kernel_mut().disconnect_clients` (the pinned NT controlled-disconnect API surface); it does not call `LiveNode::stop` and never enters NT's runner-driven lifecycle
- no-trade: same constraints as the controlled-connect boundary; the boundary does not register strategies, select markets, construct orders, submit orders, or invoke any user-level subscription API
- no copying NT private drain logic: the boundary does not copy or reimplement NT's private drain or flush internals

The controlled-connect and controlled-disconnect boundaries alone do not enable live trading: order submission, strategy actors, reconciliation, and the runner loop remain blocked behind the still-absent supervised live-trading transition.

### 11.7 Startup readiness check boundary

The bolt-v3 startup readiness check is a library-level diagnostic surface. It reports explicit facts for the existing startup boundaries: forbidden credential environment variables, SSM secret resolution, adapter mapping, `LiveNodeBuilder` construction, NT client registration, and final `LiveNode` build. It does not return or encode an aggregate launch decision.

The check composes the same production boundaries used by the build path and stops before the controlled-connect boundary. A successful report means the configured venues can pass those startup checks and a `LiveNode` can be built with registered clients. It does not prove that clients are connected, NT caches are populated, instruments are available, strategies are registered, markets have been selected, orders can be constructed, or orders can be submitted.

If no venues are configured, the check still emits `Satisfied` root facts for stages that have no venue work to perform. Callers must inspect fact details and subjects instead of treating a uniform `Satisfied` status set as a venue-bearing launch decision.

The built `LiveNode` is discarded after the build fact is recorded. The check must not call `connect_bolt_v3_clients`, `disconnect_bolt_v3_clients`, any user-level subscription API, any runner API, any strategy actor API, or any order API. Controlled-connect remains an explicit, separate caller action under Section 11.6.

### 11.8 Live canary gate boundary

The bolt-v3 live canary gate is the fail-closed admission boundary before `run_bolt_v3_live_node` enters NT's `LiveNode::run` runner loop. Production code must call `run_bolt_v3_live_node` instead of calling `LiveNode::run` directly for the bolt-v3 path.

The gate validates only operator approval and prior no-submit readiness evidence. It checks that `[live_canary]` is present, `approval_id` is non-empty, `max_no_submit_readiness_report_bytes` is positive, `max_live_order_count` is positive, `max_notional_per_order` is a positive decimal, and `max_notional_per_order` is less than or equal to `risk.default_max_notional_per_order`.

The gate reads at most the configured `max_no_submit_readiness_report_bytes` from `no_submit_readiness_report_path` and requires a JSON object with a non-empty `stages` array. Each stage must expose `status = "satisfied"` case-insensitively; stage names may be carried by either `stage` or `name` for diagnostics. Missing, unreadable, oversized, unparsable, non-array, empty, or unsatisfied reports reject the run before NT's runner loop is entered.

The gate is read-only. It does not connect clients, subscribe to data, register strategies, select markets, construct orders, submit orders, cancel orders, or mutate NT state. The built `LiveNode` may already exist when the gate runs, but a gate rejection must occur before `LiveNode::run`.

The gate validates canary bounds before the runner starts; it does not itself count orders or enforce per-order notional at submit time. Submit-admission code must independently consume the validated `BoltV3LiveCanaryGateReport` bounds before any live canary order is allowed.

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
- the exact systemd restart policy intended for live trading

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

Live trading is allowed only if:

- panic behavior is empirically tested
- blast radius is documented
- resulting behavior is explicitly accepted

Unknown panic behavior is not acceptable.

## 13. CLOB V2 Readiness Gate

Polymarket CLOB signing compatibility is a live-trading launch gate.

Current status: this branch pins NautilusTrader to upstream release `v1.226.0`
(`38b912a8b0fe14e4046773973ff46a3b798b1e3e`), which contains upstream
Polymarket CLOB V2 adapter support. The compatibility evidence proves focused
Bolt-v3 compile and test compatibility only. It does not prove live order
signing, submission, fill parsing, collateral accounting, or fee behavior.

Live trading is allowed only if:

- the pinned NautilusTrader Polymarket adapter is verified against the currently required Polymarket CLOB signing version
- the verification records the order signing version, contract/domain requirements, collateral assumptions, and fee behavior checked
- Bolt's runtime collateral and fee contracts are updated from the old `USDC` and pre-entry fee-rate assumptions to the verified CLOB V2 behavior
- `just check` Phase 2 reports the gate result
- the release manifest records the verified CLOB signing version

If the pinned adapter is not verified for the current Polymarket CLOB requirement, live capital is blocked.

## 14. Current Live-Trading Invariants

The following invariants must remain true:

- no mixins
- no dual paths
- no bolt-owned order schema
- no bolt-owned standalone market-selection service
- no bolt-owned observability stack
- no strategy-specific venue shaping
- no hidden identity overlap
- no environment-variable secret fallback
