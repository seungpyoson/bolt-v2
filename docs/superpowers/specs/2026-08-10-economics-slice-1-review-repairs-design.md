# Economics Slice 1 Review Repairs

## Scope

This design repairs the four substantive findings on PR #1544 without expanding beyond issue #1445's atomic quote/admission cutover. It preserves the current `quote_only` posture and does not grant live, deploy, readiness, or trading authority.

## Design

### One sealed-order economics basis

`src/bolt_v3_order_execution.rs`, the shared NautilusTrader execution boundary, will own a private-field, fallible `FinalOrderEconomicsBasis`. A strategy cannot construct it from independently calculated absolute gross value, notional, or normalized legs. The constructor consumes:

- the post-clamp final `OrderAny` that will be submitted;
- the strategy's typed value scenario rather than a pre-scaled money amount;
- candidate executable fill levels;
- the minimum core-edge ratio and other admission context.

The constructor binds the final NT order first, handles base-quantity and quote-quantity orders explicitly, and deterministically truncates over-covering candidate levels to the final order. It rejects empty, invalid, limit-violating, or under-covering levels. It then derives the normalized fill legs, exact planned-fill notional, and gross expected value directly from those normalized levels. Multi-level gross value is summed per level; it is never obtained by scaling a prior aggregate value in proportion to notional.

There are two deliberately distinct money bases. Planned-fill notional prices the expected execution curve and provider economics. Conservative reservation liability comes from shared admission facts for the same final order and binding; for example, a buy limit may reserve its limit liability while economics quotes the lower expected fills. The type keeps both names distinct and never substitutes one for the other. Provider fees, adapter-resolved edge basis, gross value, and admission all refer to the same final order binding even when their legitimate money bases differ.

The seal occurs after NT rounding and after Bolt's risk-reducing exit clamp, but before strategy evidence, pending-exposure mutation, admission counters, or venue mutation. Entries and exits use this same choke point. A construction failure therefore leaves all those surfaces unchanged.

### One typed value and lifecycle scenario

Shared execution will expose a typed strategy economic scenario with private fields. Its constructors bind the value model to its lifecycle so a caller cannot provide them independently:

- the edge-taker entry uses `HoldToRedemption` because its adjusted success probability is explicitly an expected terminal binary payout per unit;
- a planned risk-reducing exit uses `PlannedExit` because its gross value compares normalized exit proceeds with the stored entry cost per unit.

The entry scenario carries expected terminal value per unit, not an absolute gross value. Candidate sizing and final sealing both derive gross value from their own fill legs through that same scenario. A future planned-exit entry policy would require a different typed scenario with its own value and horizon inputs; it cannot silently reuse the terminal-payout model. This semantic type rule is strategy intent, not a mutable venue/runtime value, so no free TOML lifecycle switch is added that could contradict the supplied value model.

For the current buy-entry scenario, each normalized leg contributes `(expected_terminal_value_per_unit - fill_price) * fill_quantity`. For the current sell-exit scenario, each normalized leg contributes `(fill_price - stored_entry_cost_per_unit) * fill_quantity`. Checked Decimal sums produce the absolute gross value. These formulas make multi-level truncation recompute value from retained levels instead of assuming linear equivalence with a discarded aggregate.

### One fee authority

The obsolete family fee seam will be deleted: `MarketFamilyValidationBinding::maker_binary_fee_curve`, all family formula implementations and the unsupported fallback, and the public lookup functions. Provider economics adapters remain the sole fee-formula authority. No compatibility wrapper or replacement family lookup will be added.

Only fee-specific fixtures and assertions are removed. Existing tests that also cover settlement payout, maker quote-target dispatch, or unknown-family failure remain and are narrowed to those still-valid responsibilities. Compilation plus direct diff and symbol inspection prove deletion; no source-scanning test is added.

### One resting-order cancellation coordinator

The `cancel_pending` boolean and the direct cancellation routes for tracked maker orders will be replaced by one shared `RestingOrderCancelCoordinator` inside the order-execution boundary. It owns cancellation-attempt metadata only; NT's cache and order events remain authoritative for order lifecycle and status.

Every normal cancellation origin for a tracked maker order uses this coordinator: economics refresh, per-leg quote lifecycle, side-scoped or instrument-scoped cancel-all, and strategy stop. The kill switch remains an external emergency override, but the coordinator observes the resulting NT status and never creates a competing retry path. The quote-lifecycle `CancelRejected -> Cancel` arm is changed to retain its lifecycle state without routing; retry authority belongs only to the coordinator.

The maker's existing NT `Strategy` callbacks are the event surface. `PendingCancel`, `CancelRejected`, `Canceled`, `Filled`, `Expired`, and terminal submit rejection are translated into coordinator observations for the matching tracked `ClientOrderId`. A `CancelRejected` event never routes directly. Because NT events do not carry a cancel-attempt identifier, the coordinator re-reads current NT status: a closed order retires, `PendingCancel` suppresses, and only an order still open or missing can become retryable. A delayed rejection cannot override a newer observed pending-cancel or terminal state.

The coordinator uses the NT actor clock for requests, event handling, and timer drives. Wall-clock reads are forbidden. Checked conversion and arithmetic failures, timestamp regression, and attempt-counter overflow fail closed.

#### Transition contract

| Current coordinator state | NT observation or result | Transition and routing |
| --- | --- | --- |
| `Ready` | cancellation requested while order is open or missing | Route once; on NT route success enter `AwaitingAck`, on synchronous failure remain retryable |
| `AwaitingAck` | before `retry_not_before_ns` | Suppress duplicate |
| `AwaitingAck` | NT reports `PendingCancel` | Enter `PendingCancel`; do not route |
| `AwaitingAck` | order remains open/missing at or after `retry_not_before_ns` | Route one retry and advance the checked attempt count |
| `AwaitingAck` | `CancelRejected` while current NT status is open/missing | Mark retryable at the existing configured not-before deadline; do not route from the callback |
| `PendingCancel` | pending status persists | Suppress duplicates; if the retained quote deadline passes, expose `StuckPendingCancel` health and fail loud without venue-paced re-cancel churn |
| Any | NT reports closed/filled/canceled/expired/terminal submit rejection | Remove the record |

Retries are timer-driven and rate-limited, never callback-driven. `cancel_retry_timeout_ms` is both the missing-ack timeout and the minimum interval between attempts. `cancel_retry_escalation_attempts` is a required positive TOML value. Every synchronous failure or rejection increments checked diagnostic state; reaching the configured threshold exposes typed unhealthy status and a loud error while later timer drives continue bounded retry attempts. A failing record does not abort processing of siblings: the drive attempts every due record, preserves each primary error, then returns one aggregate failure.

Cancel-all stamps only records selected by the routed `(instrument_id, order_side)` scope, and only after NT accepts the cancel-all request. Uncovered records stay retryable. `SkippedByPolicy` and synchronous route failure advance no cancellation state.

Strategy stop enters a draining state before `DataActor::on_stop`: it requests cancellation through the same coordinator and defers component stop while tracked NT orders remain. The existing maker timer and order callbacks continue driving the coordinator. Only an empty tracked set permits the final `on_stop` teardown and timer deregistration. An unhealthy or stuck order therefore blocks a silent stop and remains visible rather than losing its retry owner.

### Timeout and configuration invariants

`ExecutionEconomicsConfig` gains required, no-default `cancel_retry_timeout_ms` and `cancel_retry_escalation_attempts` fields. The timeout must be positive and strictly shorter than `resting_order_refresh_margin_ms`. Every shipped economics TOML section and fixture is updated in the same branch state; serde defaults are forbidden.

Maker startup performs the stronger cadence check with checked nanosecond arithmetic. If `C` is the maker timer cadence, `R` the retry timeout, and `ceil_to_cadence(R)` the first timer-observable retry delay, configuration is accepted only when:

```text
C + ceil_to_cadence(R) < resting_order_refresh_margin
```

Resting-order registration also rejects an admission whose actual `valid_until_ns - requested_at_ns` is shorter than the configured refresh margin; a nominal `quote_validity_ms` cannot mask an earlier provider/source deadline. Together, the actual-lifetime check and cadence inequality cover the worst timer phase and guarantee that a cancellation first discovered at the refresh boundary has a retry opportunity before the admitted quote's real expiry. Missing, zero, overflowing, clock-regressing, source-horizon-incompatible, or cadence-incompatible inputs fail closed. Timeout eligibility uses `elapsed >= timeout` in the one injected NT clock domain.

## Error and mutation ordering

Final-order basis construction and admission sealing occur before strategy evidence, pending-exposure state, admission counters, or venue mutation. Cancellation state advances only after NT accepts the corresponding cancel route. Synchronous failures and shadow-policy skips leave the record retryable. Multi-record drives isolate failures and report them after every due record has been processed.

## Behavioral evidence

Tests will be added before their production changes for:

1. multi-level coarse-precision entry rounding, proving normalized legs, directly recomputed gross value, provider fee, adapter edge basis, and conservative reservation liability all bind to the final order without being falsely equal;
2. over-covering levels truncating deterministically, and under-covering or limit-invalid levels failing before strategy evidence, pending exposure, admission counters, sink calls, or venue mutation;
3. the risk-reducing exit clamp occurring before basis construction and producing the clamped gross value and fill quantities;
4. candidate sizing and final admission sharing the typed `HoldToRedemption` entry scenario, with no separately supplied lifecycle;
5. missing, zero, overflowing, source-horizon-incompatible, and cadence-incompatible retry configuration, including the exact cadence/timeout boundary;
6. synchronous cancel failure staying retryable while later records still route, with aggregate error preservation;
7. explicit rejection becoming timer-retryable without callback routing, stale rejection suppression against newer pending/closed NT state, and configured retry-rate bounds;
8. a lost acknowledgement retrying at—not before—the configured deadline, with clock regression failing closed;
9. pending cancellation suppressing duplicates, persistent pending status surfacing unhealthy state at the quote deadline, and terminal order status retiring the record;
10. one-side cancel-all stamping only covered records, shadow skip stamping none, and stop deferring until all tracked orders close.

Existing provider formula, replay parity, fail-before-mutation admission, resting-order refresh, settlement dispatch, maker quote-target dispatch, and unknown-family failure tests remain regression evidence. Family fee-seam removal is confirmed through compilation and reviewer diff/symbol inspection, not a source-scanning test.

## Non-goals

This repair does not implement actual economics ledgers, supplemental actuals, lifecycle/carry actuals, reporting closure, live economics input publication, or live execution. Those remain outside Slice 1.
