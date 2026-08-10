# Economics Slice 1 Review Repairs

## Scope

This design repairs the four substantive findings on PR #1544 without expanding beyond issue #1445's atomic quote/admission cutover. It preserves the current `quote_only` posture and does not grant live, deploy, readiness, or trading authority.

## Design

### One final-order economics basis

The edge taker will construct one typed final-order economics input only after Nautilus has produced the final rounded order quantity and price. That input will contain the normalized planned fill legs, their exact planned-fill notional, the gross expected value scaled to that notional, the minimum core-edge ratio, and the lifecycle path.

Sizing may still evaluate candidate notionals, but final admission will not reuse a pre-rounding gross value or an oversized fill plan. The final order, gross value, provider fee quote, edge denominator, and admission request will therefore share one quantity/notional basis. If the executable fill levels cannot cover the final order exactly, construction fails before evidence, admission, or venue mutation.

### One lifecycle decision

The edge-taker intent will select its lifecycle path once and carry it through candidate sizing and final admission. No downstream route will replace it with a different hard-coded variant. Behavioral tests will assert that sizing and final admission observe the same lifecycle.

### One fee authority

The obsolete family fee seam will be deleted: `MarketFamilyValidationBinding::maker_binary_fee_curve`, all family formula implementations and unsupported fallback, the public lookup functions, and their tests. Provider economics adapters remain the sole fee-formula authority. No compatibility wrapper or replacement family lookup will be added.

### Recoverable resting-order cancellation

The boolean `cancel_pending` flag will be replaced by a typed cancellation state owned by the resting-order economics record. The state records when the latest cancellation was requested. NT order events/status remain authoritative for whether an order is open, pending cancellation, rejected, or closed.

- A successful cancellation request enters `Requested` with its request timestamp.
- `CancelRejected` returns the record to retryable state and immediately routes another cancellation while the economics violation still exists.
- If no terminal acknowledgement arrives before the TOML-backed `cancel_retry_timeout_ms`, the next economics drive retries the cancellation.
- An NT pending-cancel status suppresses duplicates until rejection, closure, or timeout.
- A closed order removes the record.

The timeout belongs to `ExecutionEconomicsConfig`, is required, must be positive, and must remain below quote validity. Maker construction additionally rejects a quote interval longer than the retry timeout, ensuring the configured loop can observe the deadline. There is no code default or hard-coded retry duration.

## Error and mutation ordering

Final-order economics construction, lifecycle validation, and quote/admission sealing occur before evidence, admission counters, or venue mutation. Cancellation state advances only after the NT cancel route accepts the request. A synchronous routing failure leaves the record retryable.

## Behavioral evidence

Tests will be added first and observed failing for:

1. coarse quantity precision reducing final notional while gross expected value and edge ratio use that reduced notional;
2. sizing and final admission receiving the same lifecycle path;
3. configuration rejecting a missing, zero, or cadence-incompatible cancellation retry timeout;
4. cancellation rejection causing a retry;
5. a lost acknowledgement causing a retry only after the configured timeout;
6. pending cancellation suppressing premature duplicates and closure retiring the record.

Existing provider formula, replay parity, fail-before-mutation admission, and resting-order refresh tests remain regression evidence. Removal of the family fee seam is compiler-enforced by deleting the field and its callers; no source-scanning test will be added.

## Non-goals

This repair does not implement actual economics ledgers, supplemental actuals, lifecycle/carry actuals, reporting closure, live economics input publication, or live execution. Those remain outside Slice 1.
