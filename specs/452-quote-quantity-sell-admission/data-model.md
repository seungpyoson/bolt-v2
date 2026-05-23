# Data Model: Quote-Quantity SELL Limit Admission

## CompiledOrderAdmissionInput

- `order`: compiled NT `OrderAny`.
- `intent_price`: fallback price already recorded in order-intent evidence.
- `instrument`: NT `InstrumentAny` used for quote-to-base and notional math.
- `market_quote`: optional cache quote for side-adjusted effective price.
- `market_trade`: optional cache trade for market-order fallback.

Validation:

- Quantity and price strings from compiled order must parse as decimal before admission.
- Quote-quantity Limit/StopLimit conservative handling applies only for non-inverse instruments.
- Inverse instruments must keep the existing NT-derived notional path; the conservative quote-quantity floor is non-inverse only.
- Missing required context must not silently understate notional.

## QuoteQuantityAdmissionNotional

- `submitted_quote_quantity`: decimal parsed from `order.quantity()` when `order.is_quote_quantity()`.
- `last_px`: compiled order price, trigger price, cache quote, or cache trade depending on order type.
- `effective_price`: side-adjusted price used to estimate base quantity.
- `admission_notional`: value submitted to `BoltV3SubmitAdmissionRequest.notional`.

Invariant:

- For quote-quantity SELL Limit/StopLimit with `effective_price > last_px`, `admission_notional` must be at least `submitted_quote_quantity`.
- The comparison is performed in the parsed decimal price/quantity/notional domain; float and string comparisons are forbidden.
- Missing quote cache for SELL Limit/StopLimit falls back to `submitted_quote_quantity`.
- Missing parse or instrument context fails closed before an admission request is built.

## ReachabilityClassification

- `current_behavior`: reachable through validated current strategy config.
- `latent_risk`: present in code but blocked by current validation.
- `future_enablement_requirement`: must be satisfied before shorts or quote-sized exits are enabled.

Current classifications:

- Quote-quantity BUY entry: current behavior.
- Quote-quantity SELL short entry: latent risk.
- Quote-quantity normal exit: future enablement requirement.
- Quote-quantity forced exit: future enablement requirement.
