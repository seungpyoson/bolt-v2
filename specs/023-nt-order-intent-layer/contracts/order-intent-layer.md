# Contract: NT Order Intent Layer

> **Historical feature artifact — not current authority.** This contract records
> the retired feature design. Current `main` and `AGENTS.md` are authoritative.

## Boundary

Bolt owns:

- TOML parsing and schema errors.
- Strategy position-contract validation.
- Strategy-derived runtime order inputs.
- Pre-submit admission.
- Bolt decision and admission evidence.
- Provider registration and secret resolution.

NT owns:

- Order model and order lifecycle.
- `OrderFactory` construction behavior.
- `OrderInitialized` and `SubmitOrder`.
- Risk engine checks.
- Execution engine routing.
- Adapter legality and wire translation.
- Cache, portfolio, account, order, fill, and reconciliation behavior.

## Public Behavior

Order configuration MUST compile through one path:

```text
TOML order table
  -> StrategyPositionContract + NtOrderTemplate
  -> OrderBuildInputs from strategy runtime state
  -> NT OrderFactory
  -> strategy-owned optional SubmitContext
  -> Bolt evidence
  -> Bolt admission
  -> NT submit_order(order, position_id, client_id, params)
```

The shared order-template module stops at NT `OrderFactory` construction. It MUST NOT know strategy archetypes, venue/provider names, market families, evidence, admission, or submit policy.

Maker behavior is not a mode. It is expressed through NT fields such as limit-like order type plus `is_post_only=true`.

Taker behavior is not a separate mode. It is expressed through NT fields such as market order type or aggressive limit/TIF choices.

## Validation Rules

Bolt MUST validate:

- Required TOML fields are present and parse as NT enums or documented Bolt config fields.
- Strategy-owned position contracts match the current strategy economics; `binary_oracle_edge_taker` accepts the long contract and rejects short-side contracts until short economics, collateral, and exit semantics exist.
- Required runtime price/quantity/trigger inputs are available for enabled order types.
- NT model crash-prevention invariants before `OrderFactory` calls for enabled order types.
- Admission caps and decision evidence completeness.

Bolt MUST NOT validate as runtime policy:

- Whether Polymarket supports a specific order shape.
- Whether Binance maps post-only as `LIMIT_MAKER` or GTX.
- Whether Deribit interprets GTD as venue day.
- Whether a venue supports options, binary options, spot, or perpetuals.

Those claims require NT adapter source evidence or runtime smoke evidence. The adapter set is determined by the claim being made; it is not fixed to Polymarket, Binance, or Deribit.

## GTD Contract

GTD support requires both:

1. NT model-valid expiry input from TOML-controlled configuration.
2. Venue-specific evidence for how the selected NT adapter handles GTD.

Source-level NT model validity is not the same as live venue support.

## Submit Params Contract

The shared order-template module does not carry NT submit params. The submit boundary may pass already-typed NT params to NT, but concrete param names and meanings belong to provider bindings or strategy-specific config. Bolt MUST NOT hardcode adapter param keys in the generic order-template module.

## Order Emulation Contract

`emulation_trigger` changes NT submit routing through the order-emulation path and is not enabled by this source/unit order-template slice. It requires a separate TDD slice with NT emulator source evidence, strategy configuration ownership, and no live-support claim without an exact smoke/canary artifact.

## Forced Exit Contract

Passive maker exit is not forced-flat. Forced exit behavior MUST be separately configured if the strategy requires urgent flattening.
For an already-open managed position, a forced-flat reason MUST take precedence over normal discretionary-exit guards such as a resting pending-entry remainder. The pending-entry guard still applies to normal non-forced exits.
When forced-flat submission finds a managed pending-entry remainder, it MUST use NT's cancel-order path for that pending entry before relying on the forced exit. If a pending-entry fill races while the forced exit is pending, the terminal exit update MUST recover to managed residual exposure instead of remaining exit-pending.

## Non-Goals

- No maker-only order intent layer.
- No Bolt venue capability matrix for runtime policy.
- No direct NT order constructors unless separately approved.
- No NT order-emulation surface without a separate approved slice.
- No second submit path.
- No mock exchange universe as live-readiness proof.
- No live-submit claim without exact-head strategy-free or live-submit artifact.
