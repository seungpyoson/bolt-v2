# IV Engine API Contract

The IV engine exposes all strategy-facing IV access through one generic API. Strategy code must not subscribe directly to NT IV/options topics or derive IV through NT math helpers; those capabilities live behind the IV engine.

## Query Products

Strategies can request:

- raw NT option-greeks payloads
- raw NT option-chain payloads
- raw NT aggregate greeks payloads
- raw NT custom volatility payloads
- custom volatility evidence
- IV points
- IV plus greeks points
- aggregate greeks products
- smiles
- surfaces
- derived IV points
- source health

## Required Query Fields

Every query includes:

- `strategy_id`
- `profile_id`
- `selector`
- `product_kind`
- `as_of`
- history/current mode

Optional query fields are allowed only when the owning IV profile permits overrides:

- IV basis
- accepted convention
- source ID filter
- projection policy
- interpolation policy
- fallback policy
- quorum policy
- maximum age override

## Responses

Responses are either:

- `Ok(product)` with provenance, profile identity, source identity, timestamp units, and policy decisions
- `Rejected(reason)` with a typed `IvRejectReason`

No query may silently fall back to another basis, convention, source, timestamp, projection, interpolation, fallback, quorum, or extrapolation policy.

## Raw Payload Access

Raw payload access returns preserved NT payloads through the engine store. It does not grant the strategy direct ownership of NT subscription mechanics.

## Policy Contract

Interpolation:

- must name the configured method and axes
- must record every input point used
- must reject if input count, source eligibility, axis, or extrapolation requirements are not satisfied

Fallback:

- must follow the configured ordered candidate list
- must record rejected candidates and the accepted candidate
- must reject if no candidate qualifies

Quorum:

- must evaluate only configured eligible sources
- must record participating and rejected sources
- must reject when source count or agreement-band requirements are not met

## Source-Fence Contract

The IV source-fence must reject:

- strategy module imports of NT msgbus option-greeks subscription APIs
- strategy module imports of NT data-actor option-chain subscription APIs
- strategy module imports of NT greeks subscription APIs
- strategy module imports of NT IV or greeks math helpers for strategy-local IV derivation
- concrete venue, asset, market, cadence, source, or instrument constants in IV core logic

The IV source-fence must allow:

- strategy imports of the public IV query API
- IV engine imports of NT model, msgbus, data actor, data engine, option-chain, custom data, and greeks helper surfaces
