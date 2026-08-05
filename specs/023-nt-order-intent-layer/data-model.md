# Data Model: NT Order Intent Layer

> **Historical feature artifact — not current authority.** This model records
> the retired feature design. Current `main` and `AGENTS.md` are authoritative.

## StrategyPositionContract

Bolt-owned strategy semantics for how an entry and exit relate to a position.

Fields:

- `entry_position_side`: NT `PositionSide`
- `exit_position_side`: NT `PositionSide`
- `entry_order_side`: NT `OrderSide`
- `exit_order_side`: NT `OrderSide`
- `forced_exit_behavior`: configured behavior for urgent flattening

Validation:

- Long contract: entry buy, exit sell.
- Short contract: entry sell, exit buy.
- Forced exit cannot silently reuse passive maker behavior unless TOML explicitly configures it.

Not included:

- NT order lifecycle fields.
- Venue position-side translation.

## NtOrderTemplate

Config-owned NT order semantics shared by maker and taker for the currently enabled slice.

Fields:

- `order_type`: NT `OrderType`
- `time_in_force`: NT `TimeInForce`
- `is_post_only`: bool
- `is_reduce_only`: bool
- `is_quote_quantity`: bool
- `expire_time_unix_nanos` is passed through for enabled NT factories that accept expiry; GTD requires a positive expiry and Market rejects expiry because the pinned NT market factory has no expiry input
- `trigger_type`, `trigger_price`, and `trigger_instrument_id` only when triggered order slices are enabled
- `display_qty` only when an iceberg/display quantity slice is enabled
- `trailing_offset` and related fields only when a trailing slice is enabled
- `exec_algorithm_id`, `exec_algorithm_params`, and `tags` only when an execution algorithm slice is enabled
- `emulation_trigger` only when a separate NT order-emulation slice is enabled; it is not part of the current source/unit order-template support claim

Validation:

- Validates schema readability and the generic NT model invariants needed by enabled variants.
- Does not validate venue support.
- Does not encode maker/taker as a separate enum.

Factory-reachable order types with current source/unit construction coverage in Bolt. This is not a live or adapter-specific support claim:

- Market
- Limit
- StopMarket
- StopLimit
- MarketIfTouched
- LimitIfTouched
- TrailingStopMarket

Remaining factory-reachable order types that still need one positive construction/admission slice before Bolt claims support:

- None in the pinned single-order `OrderFactory` surface.

Factory gaps requiring separate approval:

- MarketToLimit
- TrailingStopLimit

## OrderBuildInputs

Runtime facts computed by strategy logic and market state.

Fields:

- `instrument_id`
- `quantity`
- `price` for limit-like orders
- `trigger_price` for triggered orders
- `activation_price` and offsets for trailing orders
- `client_order_id`

Source:

- Strategy signal, target, market data, configured sizing, and NT order id generation.

## SubmitContext

Submit-level context passed to NT after order construction and admission.

This is outside `bolt_v3_order_intent`; the shared NT order-template module stops at `OrderFactory -> OrderAny`.

Fields:

- `client_id`: optional NT `ClientId`
- `position_id`: optional NT `PositionId`
- `params`: optional NT submit params

Validation:

- Admission uses the compiled order and Bolt risk/admission config.
- NT execution engine validates venue/client match, OMS/position compatibility, and instrument presence.
- Provider bindings own any concrete submit param schema. The strategy-owned submit boundary only carries already-typed params to NT.

## OrderIntentEvidence

Bolt pre-admission audit record for the compiled NT order selected for admission.

Fields:

- strategy id
- intent kind
- instrument id
- client order id
- order side
- selected price from the compiled order price or trigger price when NT exposes one, otherwise the configured fallback price used by admission fallback paths
- compiled NT order quantity
- selected compiled NT order fields needed to explain Bolt admission, without duplicating NT `OrderInitialized`: order type, TIF, compiled price, trigger price, activation price, trigger type, trigger instrument id, trailing offset, trailing offset type, expiry, post-only, reduce-only, and quote-quantity flags

Boundary:

- Evidence records what Bolt decided before the submit-admission gate and before NT submit.
- Admission outcome is recorded after the gate in the linked `AdmissionDecisionEvidence` record, keyed by strategy id, client order id, and instrument id. It is not duplicated into the pre-admission intent record.

## AdmissionDecisionEvidence

Bolt post-gate audit record for the submit-admission decision.

Fields:

- strategy id
- client order id
- instrument id
- admitted/rejected notional
- admission outcome

Boundary:

- Evidence records the Bolt admission gate outcome only. NT owns exchange submission and execution lifecycle evidence.
- NT order events remain the authoritative order lifecycle record.

## AdapterProof

Evidence for venue-specific claims.

Fields:

- adapter or venue
- source file and line evidence
- smoke command or canary artifact when available
- exact git head
- claim proven
- residual claim not proven
