# Contract: Quote-Quantity Admission

## Public Behavior

Given a compiled NT order, Bolt submit admission derives one `BoltV3SubmitAdmissionRequest` before calling NT `submit_order`.

For non-quote-quantity orders:

- Admission notional remains `compiled_price * compiled_quantity`.

For quote-quantity Market-like orders:

- Admission uses cache quote or cache trade where current logic already requires it.

For quote-quantity Limit/StopLimit orders:

- BUY retains NT-compatible effective-price behavior unless a red test proves under-admission.
- SELL must not understate notional below submitted quote quantity when quote cache makes `effective_price > last_px`; the admission notional is the maximum of the calculated submit notional and submitted quote quantity.
- The maximum comparison must use the same exact decimal domain as parsed NT price, quantity, and notional values; no float or string comparison is allowed.
- The conservative quote-quantity floor applies to non-inverse instruments only; inverse instruments retain the existing NT-derived notional path.
- If quote cache is absent for a SELL Limit/StopLimit, admission falls back deterministically to submitted quote quantity.
- If quantity, price, or instrument context cannot be parsed or loaded enough to classify the order safely, admission fails closed before constructing `BoltV3SubmitAdmissionRequest`.

## Non-Goals

- No #451 generic wrapper extraction.
- No short-side strategy economics.
- No quote-sized exit enablement.
- No Polymarket, binary-oracle, up/down, or strategy-specific policy in a shared helper.
- No live/canary exchange execution claim.

## Required Tests

- Red regression for quote-quantity SELL Limit with `bid > limit_price`.
- Existing `quote_quantity_submit_admission_matches_nt_effective_notional_for_limit_buy` test remains green.
- Existing `quote_quantity_submit_admission_uses_limit_price_when_nt_cache_quote_missing` test remains green.
- Existing `quote_quantity_market_submit_admission_uses_nt_cache_quote_ask` test remains green.
- Existing `quote_quantity_market_submit_admission_uses_nt_cache_trade_when_quote_missing` test remains green.
- Existing `exit_quote_quantity_config_is_blocked_before_base_position_quantity_is_used` test remains green.
- Existing `exit_quote_quantity_order_build_is_rejected_before_nt_factory` test remains green.
- New SELL Limit missing-quote-cache test proves fallback to submitted quote quantity.
- New SELL Limit insufficient instrument/parse-context test proves fail-closed behavior before admission request construction.
- New SELL StopLimit floor test proves the conservative floor applies to StopLimit.
- New SELL StopLimit missing-quote-cache test proves StopLimit fallback to submitted quote quantity.
- New SELL StopLimit insufficient instrument/parse-context test proves StopLimit fail-closed behavior before admission request construction.
- New inverse-instrument helper test proves inverse quote-quantity notional stays on the existing NT path.
- New inverse-instrument StopLimit helper test proves the same inverse bypass for StopLimit.
- New inverse-instrument strategy regressions prove inverse Limit and StopLimit strategy wiring stays on the existing NT path.
- Source-fence test includes a positive-control fixture for forbidden tokens and proves the shared helper contains no Polymarket, binary-oracle, market-family, up/down, or strategy identity.
