# Venue-Agnostic Canary Proof Policy Design

## Purpose

T044 currently proves only a live order when the production strategy naturally emits an order. Recent source-owned canary packets reached strategy evaluation with fresh Chainlink/reference evidence, empty gate blockers, and empty pricing blockers, but the strategy repeatedly returned `no_side_selected` because neither side cleared the configured EV threshold. That is correct alpha behavior, but it makes infrastructure readiness depend on waiting for profitable alpha.

This design separates two claims:

- **Alpha readiness**: the production strategy selects trades only when its configured economics say to trade.
- **Canary proof readiness**: the live trading path can rotate markets, produce a normal order intent, pass submit admission, send at most one capped order through the configured execution adapter, and write post-run evidence.

The canary proof path must never be presented as profitable-alpha proof.

## Non-Negotiable Constraints

- No venue hardcodes. The policy must not mention any concrete venue in core behavior.
- No strategy hardcodes. The policy must not depend on any concrete strategy internals.
- No market hardcodes. The policy must not assume any concrete asset, cadence, outcome vocabulary, or market class.
- No hidden dual path. Proof orders must become the same normalized order-intent shape consumed by submit admission and the configured execution client.
- No source bypass. The policy may relax alpha selection only after normal strategy/source evidence exists and required readiness gates pass.
- No adapter constraint guesses. Price tick, quantity step, minimum quantity, minimum notional, order type support, and precision must come from normalized instrument/adapter metadata.
- No automatic live mutation. Proof mode requires explicit operator approval, a bounded approval window, max live order count, max notional, and final packet verification.
- No alpha overclaim. Every artifact and ledger row must label this as `proof_only`.

## Recommended Architecture

Add a generic canary proof layer between strategy evidence and submit admission:

```text
source evidence
  -> strategy evidence
  -> strategy-owned normalized proof candidates
  -> generic canary proof policy
  -> normalized order intent
  -> submit admission
  -> configured execution client
```

The proof policy does not fabricate venue orders. It can select only from strategy-owned normalized proof candidates. A strategy that does not expose candidates is unsupported for proof mode and fails closed.

## Config Ownership

Root/live TOML owns the proof policy under `[live_canary.proof_policy]`. The block is disabled by default and is active only when the final operator packet binds the same policy identity and approval window.

Proposed TOML shape:

```toml
[live_canary.proof_policy]
enabled = false
policy_kind = "least_bad_strategy_candidate"
proof_claim = "proof_only"
strategy_instance_id = "configured-strategy-id"
execution_client_id = "configured-execution-client-id"
notional_mode = "fixed"
proof_notional = "1.00"
candidate_score_source = "strategy_evidence"
allow_negative_expected_ev = true
rotation_observation_enabled = true
rotation_min_distinct_markets = 1
rotation_max_attempts = 1
```

Validation rules:

- `enabled = true` is rejected unless `[live_canary.operator_evidence]` binds a matching proof-policy evidence path and hash.
- `proof_claim` must be exactly `proof_only`.
- `proof_notional` must be positive, must be less than or equal to `[live_canary].max_notional_per_order`, and must be less than or equal to the root risk max for the selected strategy where applicable.
- `strategy_instance_id` must resolve to one configured strategy.
- `execution_client_id` must resolve to one configured execution client.
- The strategy's configured execution client must match the proof policy execution client unless the strategy explicitly declares execution-client override support.
- `allow_negative_expected_ev = false` rejects negative candidate scores when the candidate score kind represents expected value. The proof policy may allow negative EV only when this flag is true and `proof_claim = "proof_only"`.
- Source readiness, strategy candidate presence, instrument constraints, submit admission, and hash-bound operator evidence are mandatory runtime gates. They are not optional config toggles.
- `rotation_min_distinct_markets` and `rotation_max_attempts` must be positive, and `rotation_min_distinct_markets <= rotation_max_attempts`.

## Strategy Contract

Introduce a strategy-facing proof candidate contract that is generic over venue and market type:

```rust
pub struct BoltV3CanaryProofCandidate {
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub market_id: String,
    pub instrument_id: String,
    pub order_side: OrderSide,
    pub position_side: Option<PositionSide>,
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub post_only: bool,
    pub reduce_only: bool,
    pub limit_price: Option<Price>,
    pub sizing_price: Price,
    pub notional_hint: Decimal,
    pub candidate_score: Decimal,
    pub score_kind: CanaryProofCandidateScoreKind,
    pub candidate_priority: Option<u32>,
    pub source_evidence_refs: Vec<SourceEvidenceRef>,
}
```

Rules:

- Strategies may expose zero or more proof candidates after normal source/evaluation has run.
- Candidates must be derived from the same market selection and source evidence used by normal strategy evaluation.
- Candidates must include the configured execution client id they expect to route through.
- Strategies may rank candidates, but the generic policy chooses among them.
- A candidate is not an order until the generic proof policy validates it against config, instrument constraints, and submit admission.

Each strategy opts in by exposing candidates from its own existing evaluation state. That implementation belongs at the strategy boundary, not in the proof policy core. The core must work the same way for any future execution adapter if the selected strategy exposes candidates and the selected adapter provides constraints.

## Generic Policy Behavior

`least_bad_strategy_candidate` means:

1. Require normal source evidence and strategy candidate evidence.
2. Drop candidates whose instrument, execution client, order template, or side cannot be validated generically.
3. Reject candidates whose source evidence refs do not match the current source evidence packet.
4. Reject negative expected-value candidates unless `allow_negative_expected_ev` is true.
5. Normalize candidate size from `proof_notional`, derive quantity from the candidate's executable price, and round down through venue/instrument constraints.
6. Reject if rounding produces zero quantity, violates minimum quantity or minimum notional, or exceeds any canary or strategy risk cap.
7. Choose the candidate with the highest strategy-provided score.
8. Reject ties unless the strategy provides a deterministic candidate priority in the evidence.
9. Produce a normal order intent with `canary_proof_claim = "proof_only"`.

The policy may allow negative expected EV only for `proof_only` claims. It must not alter normal production strategy selection.

## Venue/Adapter Contract

Execution adapters expose normalized constraints for the selected instrument:

- supported order types
- supported time-in-force values
- price tick size and precision
- quantity step and precision
- minimum quantity
- minimum notional when available
- reduce-only/post-only/quote-quantity support
- whether the adapter accepts quote-notional sizing directly or requires base quantity
- whether position side is required, optional, or unsupported

The proof policy consumes these constraints through a generic interface. If an adapter cannot provide required constraints, proof mode fails closed for that venue. This avoids venue-specific assumptions and allows future execution adapters to plug into the same policy.

Sizing is normalized by adapter capability:

- base-quantity venues: derive base quantity from `proof_notional / sizing_price`, round down to quantity step, and submit rounded base quantity.
- quote-notional venues: submit quote notional directly when the adapter declares support, while still recording an estimated rounded base quantity for evidence and minimum-quantity checks when the adapter exposes a minimum quantity.

The policy must never guess which sizing mode a venue supports.

## Market Rotation Evidence

Proof mode should optionally record market rotation separately from order submission:

```json
{
  "record_kind": "bolt_v3_canary_rotation_observation",
  "proof_claim": "proof_only",
  "strategy_instance_id_hash": "...",
  "execution_client_id_hash": "...",
  "selected_market_id_hash": "...",
  "market_selection_outcome": "current",
  "candidate_count": 2,
  "source_ready": true,
  "attempt_index": 1
}
```

`rotation_min_distinct_markets` controls how many distinct selected markets must be observed before the proof policy can claim rotation readiness. `rotation_max_attempts` bounds the number of source-owned selection attempts that can be consumed to satisfy that proof. For T044 one distinct market is enough to prove the one-order path. Later staged readiness can raise the config-owned value without code changes.

When `rotation_observation_enabled = true`, the order-intent generator must require a hash-bound rotation observation artifact whose distinct selected market count is greater than or equal to `rotation_min_distinct_markets`. The artifact proves rotation observation only. It does not prove alpha readiness or market-selection quality.

Proof-only fills and positions must be tagged out of alpha PnL, alpha trade-count, strategy performance, and normal position-attribution summaries. They remain visible in canary/post-run evidence and accounting ledgers as `proof_only` operational events.

## Operator Evidence

The final packet gains these proof-only bindings:

- `canary_proof_policy_path`
- `canary_proof_policy_sha256`
- `canary_proof_candidate_source_path`
- `canary_proof_candidate_source_sha256`
- `canary_rotation_observation_path`
- `canary_rotation_observation_sha256`
- `canary_proof_order_intent_path`
- `canary_proof_order_intent_sha256`

Pre-run verification must reject:

- missing proof policy when enabled
- policy hash mismatch
- source evidence mismatch
- strategy instance mismatch
- execution client mismatch
- selected market mismatch
- stale evidence
- candidate generated from a different strategy or market
- proof order intent not labeled `proof_only`
- proof notional above approved cap

Post-run verification must continue to require normal submit event, venue order state, restart reconciliation, and post-run hygiene.

## Error Handling

Fail closed with specific reasons:

- `proof_policy_disabled`
- `proof_policy_invalid_claim`
- `proof_policy_not_approved`
- `proof_policy_strategy_mismatch`
- `proof_policy_execution_client_mismatch`
- `proof_candidate_missing`
- `proof_candidate_source_mismatch`
- `proof_policy_source_not_ready`
- `proof_policy_negative_ev_disallowed`
- `proof_policy_rotation_not_observed`
- `proof_candidate_tie_without_priority`
- `instrument_constraints_missing`
- `instrument_constraints_reject_order_type`
- `instrument_constraints_reject_time_in_force`
- `instrument_constraints_reject_position_side`
- `instrument_constraints_reject_price_tick`
- `instrument_constraints_reject_sizing_mode`
- `instrument_constraints_round_to_zero`
- `proof_notional_exceeds_cap`
- `proof_notional_exceeds_strategy_risk_cap`
- `submit_admission_rejected`

These reasons belong in the artifact and logs so operators can distinguish infrastructure failures from alpha refusal.

## Testing Requirements

- Config tests prove proof mode is disabled by default and cannot be enabled without complete operator-evidence bindings.
- Core policy tests prove the policy is strategy-id and execution-client-id driven, not venue-name driven.
- Strategy contract tests prove a strategy can expose proof candidates without producing a normal alpha-selected order.
- Constraint tests prove tick/lot/minimum validation comes from metadata and fails closed when metadata is absent.
- Submit admission tests prove proof intents still consume the same one-order and max-notional caps.
- Artifact tests prove every proof-only artifact is hash-bound into final verification.
- Regression tests prove no proof-policy code path references concrete venue names, asset symbols, outcome labels, or cadence values as defaults.

## Out Of Scope

- Proving the strategy is profitable.
- Adding new execution adapters.
- Adding a continuous live trading daemon.
- Changing production alpha thresholds.
- Bypassing T044 approval or submit admission.

## Acceptance Criteria

- A canary proof run can be configured without changing strategy source or venue adapter source for each venue.
- The first supported strategy can provide proof candidates through a generic interface.
- The proof policy can emit one normal capped order intent when alpha selection says `no_side_selected`.
- The same order intent passes through existing submit admission and the configured execution client.
- Evidence clearly separates `proof_only` from alpha/profit readiness.
- Adding a future execution adapter requires only adapter constraint support plus strategy candidates, not changes to proof-policy core.
