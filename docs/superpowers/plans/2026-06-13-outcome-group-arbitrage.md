# Outcome Group Arbitrage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a shared outcome-group trading layer that supports complete-set basket arbitrage, cross-venue outcome arbitrage, and future maker/taker outcome strategies without hardcoding venue, market, team, outcome, or strategy-specific behavior.

**Architecture:** NautilusTrader owns venue adapters, instrument models, books, order construction, fee calculation, signing, submit/cancel, and fill parsing. Bolt owns normalized outcome grouping, payout matrices, executable-cost scanning, basket-level admission, evidence, and multi-leg execution state. Venue-specific Bolt code stops at thin metadata normalizers that translate NT/venue metadata into a shared `OutcomeGroup`.

**Tech Stack:** Rust, NautilusTrader Rust adapters, TOML runtime config, Bolt v3 provider bindings, Bolt v3 admission/evidence/order-intent modules, Polymarket Gamma/CLOB metadata, Hyperliquid HIP-4 `outcomeMeta`.

---

## Non-Negotiable Invariants

- Runtime scope comes from TOML, not code.
- No code embeds World Cup, team names, event slugs, market slugs, token IDs, venue IDs, strategy IDs, or YES-only assumptions.
- Strategies produce intent only. Venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, and submit gating remain in shared execution/admission modules or NT.
- NT remains the source of venue mechanics: instruments, books, fees, signing, order submit/cancel, and fill parsing.
- Bolt owns cross-instrument semantics: grouping proof, payout matrix, basket scanner, basket-level admission, partial-fill state, and decision evidence.
- Discovery may be broad only when bounded by config caps such as explicit slugs, max groups, max markets, freshness limits, and notional caps.

## Explicit Task List

1. Basket / complete-set arbitrage: buy a portfolio of outcome legs when the minimum terminal-state payout exceeds executable cost.
2. Cross-venue arbitrage: compare normalized outcome groups across venues after each venue produces the same shared model.
3. Taker/maker strategy: reuse the shared outcome and order/admission layers; harden maker PR logic and IV/RV/FV inputs later.
4. Non-updown outcome support: expose Polymarket and Hyperliquid HIP-4 through NT-backed, config-driven outcome-group sources.

## Current Evidence

- Bolt pins NautilusTrader rev `6e059dcbb59ac1e582132fc431a581936c216c3c` in `Cargo.toml`.
- NT Polymarket parses Gamma markets into `BinaryOption` instruments in `crates/adapters/polymarket/src/http/parse.rs`.
- NT `PolymarketOutcome` is a free-form string in `crates/adapters/polymarket/src/common/enums.rs`; it is not limited to Up/Down or Yes/No.
- NT Polymarket filters support market slugs, event slugs, Gamma market queries, event queries, event params, search params, tags, and predicates in `crates/adapters/polymarket/src/filters.rs`.
- NT Gamma market params include `game_id`, `sports_market_types`, `tag_id`, active/closed/archive fields, liquidity/volume filters, and `max_markets` in `crates/adapters/polymarket/src/http/query.rs`.
- NT Polymarket execution handles negative-risk signing and market order book walking in the Polymarket execution modules.
- NT Hyperliquid parses HIP-4 outcome metadata into `BinaryOption` instruments through `outcomeMeta` parsing in `crates/adapters/hyperliquid/src/http/parse.rs`.
- Bolt currently maps Polymarket data filters only from updown target plans in `src/bolt_v3_providers/polymarket.rs`.
- Bolt Hyperliquid already has a `Hip4Outcomes` product surface, with live submit approval-gated in `src/bolt_v3_providers/hyperliquid.rs`.

## Shared Model

Create a venue-neutral model that strategy and scanner code consume.

```rust
pub struct OutcomeGroup {
    pub group_id: OutcomeGroupId,
    pub source_client_id: ClientId,
    pub venue: Venue,
    pub source_kind: OutcomeGroupSourceKind,
    pub terminal_states: Vec<TerminalState>,
    pub tradable_legs: Vec<OutcomeLeg>,
    pub payout_matrix: PayoutMatrix,
    pub settlement_rules: SettlementRules,
    pub metadata_fingerprint: String,
}

pub struct OutcomeLeg {
    pub instrument_id: InstrumentId,
    pub native_leg_id: String,
    pub outcome_label: String,
    pub side_label: String,
    pub leg_role: OutcomeLegRole,
    pub fee_model: OutcomeFeeModel,
}

pub struct PayoutMatrix {
    pub rows: Vec<TerminalState>,
    pub cols: Vec<OutcomeLegId>,
    pub payout_per_unit: Vec<Vec<Decimal>>,
}
```

Rules:

- The basket scanner accepts only `OutcomeGroup`.
- Polymarket Gamma field names and Hyperliquid `outcomeMeta` field names are not visible to scanner, admission, or strategy modules.
- YES, NO, and venue-native side labels are metadata, not control-flow branches.
- Basket profitability is computed from state-wise payouts, not from a hardcoded "sum YES prices below one" formula.

## Files And Responsibilities

- Create `src/bolt_v3_outcome_groups.rs`: shared model, validation, metadata fingerprinting, payout matrix helpers.
- Create `src/bolt_v3_outcome_group_sources.rs`: TOML config model for enabled outcome-group sources and bounded discovery rules.
- Create `src/bolt_v3_outcome_group_polymarket.rs`: Polymarket Gamma/NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`: Hyperliquid HIP-4 NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_complete_set_scanner.rs`: shared executable-cost and payout-vector scanner.
- Create `src/bolt_v3_basket_admission.rs`: basket-level admission, risk caps, stale-book checks, and evidence payloads.
- Create `src/bolt_v3_basket_execution.rs`: multi-leg execution state machine.
- Modify `src/bolt_v3_config.rs`: parse `outcome_group_sources` and complete-set strategy config.
- Modify `src/bolt_v3_providers/polymarket.rs`: expose NT discovery filters from TOML scopes without changing strategy logic.
- Modify `src/bolt_v3_providers/hyperliquid.rs`: expose HIP-4 outcome group source wiring through existing Hyperliquid adapter and approval gates.
- Modify `src/lib.rs`: export new shared modules.
- Add focused tests under `tests/` for each module and provider mapping boundary.

## Config Contract

Outcome-group sources are config-owned. The following is the intended shape; production values are operator-provided in TOML.

```toml
[[outcome_group_sources]]
source_id = "polymarket_event_moneyline"
client_id = "polymarket_main"
kind = "polymarket_gamma_event"
event_slugs = ["operator_supplied_event_slug"]
sports_market_types = ["moneyline"]
max_markets = 20
enabled = true

[[outcome_group_sources]]
source_id = "hyperliquid_hip4_outcomes"
client_id = "hyperliquid_main"
kind = "hyperliquid_hip4"
max_groups = 20
enabled = false

[[strategies]]
strategy_type = "complete_set_arbitrage"
strategy_instance_id = "complete_set_arb_main"
group_sources = ["polymarket_event_moneyline"]
min_edge_bps = 25
max_basket_notional = "10"
max_open_baskets = 1
submit_mode = "taker_ioc"
```

Validation rules:

- `source_id` must be unique.
- `client_id` must reference a configured client.
- Query-style sources must include bounded selectors and caps.
- Event-style sources must include explicit event slugs or a bounded event query.
- HIP-4 sources must use a Hyperliquid client with `Hip4Outcomes` enabled and required live-submit approvals before live trading.
- Complete-set strategies must reference at least one enabled source.
- Live mode requires positive edge, positive notional cap, max open basket cap, and evidence output enabled.

## Basket Math

The scanner evaluates generic payout vectors.

```text
candidate_cost = sum(executable_leg_costs + fees + buffers)
state_payouts = payout_matrix * leg_quantities
guaranteed_payout = min(state_payouts)
edge = guaranteed_payout - candidate_cost
admit when edge > configured_min_edge and every risk/freshness check passes
```

Examples represented by the same model:

- All-YES complete-set basket across N exhaustive states: guaranteed payout is one unit when exactly one state resolves true.
- All-NO basket across N exhaustive states: guaranteed payout is N - 1 units if exactly one state resolves true.
- Mixed YES/NO basket: guaranteed payout is computed from the payout matrix; no special-case branch is needed.
- Cross-venue basket: each leg may come from a different source client once terminal states are normalized to the same semantic group.

## Execution State Machine

Multi-leg execution is not atomic, so live trading requires a state machine.

```text
Candidate
  -> Reserved
  -> Submitting
  -> Partial
  -> Complete
  -> Repair
  -> Unwind
  -> Closed
```

State responsibilities:

- `Candidate`: scanner found an edge from fresh executable books.
- `Reserved`: basket-level notional and exposure budget reserved.
- `Submitting`: orders are in flight; no second basket can consume the same budget.
- `Partial`: at least one leg filled and at least one leg missing or short.
- `Complete`: all required leg quantities filled within tolerance.
- `Repair`: submit bounded corrective orders to restore guaranteed-payout shape.
- `Unwind`: reduce residual directional exposure when repair is not admissible.
- `Closed`: final accounting and evidence written.

## Implementation Tasks

### Task 1: Shared Outcome Model

**Files:**
- Create `src/bolt_v3_outcome_groups.rs`
- Modify `src/lib.rs`
- Test `tests/bolt_v3_outcome_groups.rs`

- [ ] Write tests for valid groups, duplicate leg IDs, empty terminal states, payout matrix dimension mismatch, and metadata fingerprint stability.
- [ ] Implement `OutcomeGroup`, `OutcomeLeg`, `TerminalState`, `PayoutMatrix`, and validation helpers.
- [ ] Export the module from `src/lib.rs`.
- [ ] Run allowed local formatting checks and remote Rust verification according to repo policy after commit.

### Task 2: Config-Driven Outcome Sources

**Files:**
- Create `src/bolt_v3_outcome_group_sources.rs`
- Modify `src/bolt_v3_config.rs`
- Test `tests/config_parsing.rs`

- [ ] Write tests for Polymarket event source parsing, Hyperliquid HIP-4 source parsing, duplicate `source_id` rejection, unbounded query rejection, and unknown client rejection.
- [ ] Implement source config enums and validators.
- [ ] Keep all runtime selectors in TOML.
- [ ] Reject scan-all configs unless they include explicit bounded caps and freshness controls.

### Task 3: Polymarket OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_polymarket.rs`
- Modify `src/bolt_v3_providers/polymarket.rs`
- Test `tests/bolt_v3_polymarket_outcome_groups.rs`

- [ ] Write tests using synthetic Gamma event/market metadata for a three-way moneyline group.
- [ ] Prove that both YES and NO token legs are preserved.
- [ ] Prove that grouping requires event-level proof and does not rely on slug patterns alone.
- [ ] Map Polymarket Gamma/NT metadata to `OutcomeGroup`.
- [ ] Add provider mapping for TOML-driven NT discovery filters: explicit event slugs, market slugs, event queries, and bounded Gamma queries.
- [ ] Avoid per-scope NT `accept()` outcome predicates because NT applies accept predicates globally across filters.

### Task 4: Read-Only Complete-Set Scanner

**Files:**
- Create `src/bolt_v3_complete_set_scanner.rs`
- Test `tests/bolt_v3_complete_set_scanner.rs`

- [ ] Write tests for all-YES, all-NO, mixed baskets, insufficient depth, stale book, fee inclusion, and non-positive edge.
- [ ] Implement payout-vector evaluation from `PayoutMatrix`.
- [ ] Use executable depth, not display or Gamma prices.
- [ ] Return scanner evidence with costs, fees, state payouts, guaranteed payout, and block reason.

### Task 5: Basket Admission

**Files:**
- Create `src/bolt_v3_basket_admission.rs`
- Modify `src/bolt_v3_decision_evidence.rs`
- Test `tests/bolt_v3_basket_admission.rs`

- [ ] Write tests for basket notional cap, max open basket cap, stale evidence rejection, negative edge rejection, and missing grouping proof rejection.
- [ ] Implement basket-level admission that reserves the whole basket, not individual legs independently.
- [ ] Record evidence keyed by strategy id, basket id, group id, and leg instrument IDs.
- [ ] Reuse shared submit-admission arithmetic where applicable.

### Task 6: Basket Execution State Machine

**Files:**
- Create `src/bolt_v3_basket_execution.rs`
- Test `tests/bolt_v3_basket_execution.rs`

- [ ] Write state-transition tests for complete fill, partial fill, repair allowed, repair denied, unwind, cancel failure, and terminal close.
- [ ] Implement state transitions as pure logic first.
- [ ] Keep NT submit/cancel calls outside pure state logic.
- [ ] Require bounded repair/unwind policies from TOML before any live submit path uses them.

### Task 7: Hyperliquid HIP-4 OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`
- Modify `src/bolt_v3_providers/hyperliquid.rs`
- Test `tests/bolt_v3_hyperliquid_outcome_groups.rs`

- [ ] Write tests using synthetic NT HIP-4 `BinaryOption` metadata.
- [ ] Prove that the existing Hyperliquid adapter path remains responsible for discovery and execution.
- [ ] Convert HIP-4 outcome metadata into `OutcomeGroup`.
- [ ] Enforce existing Hyperliquid product-surface and live-submit approval gates.

### Task 8: Cross-Venue Normalization

**Files:**
- Create `src/bolt_v3_cross_venue_outcome_groups.rs`
- Test `tests/bolt_v3_cross_venue_outcome_groups.rs`

- [ ] Write tests proving two venue groups can be matched only when terminal states, settlement rules, and resolution semantics are equivalent.
- [ ] Implement semantic group matching with explicit confidence/evidence fields.
- [ ] Reject groups when cancellation, void, timing, or settlement source semantics differ.
- [ ] Feed matched groups into the same complete-set scanner.

### Task 9: Strategy Registration

**Files:**
- Modify `src/strategies/registry.rs`
- Create strategy wrapper module only if existing registry requires a concrete strategy entry point.
- Test `tests/bolt_v3_strategy_registration.rs`

- [ ] Register a generic `complete_set_arbitrage` strategy that consumes `OutcomeGroup` sources.
- [ ] Keep strategy code limited to signal/intent and local signal state.
- [ ] Route all admission and execution mechanics through shared modules.

## Delivery Order

1. Shared model and config parsing.
2. Polymarket normalizer and discovery wiring.
3. Read-only complete-set scanner.
4. Basket admission and evidence.
5. Basket execution state machine with tiny caps.
6. HIP-4 normalizer through existing Hyperliquid adapter.
7. Cross-venue matching.
8. Maker/taker enhancements using the same outcome and order layers.

## Review Prompt

Use this prompt for an adversarial architecture review:

```text
You are reviewing a Rust trading-system architecture for Bolt v3.

Goal:
Evaluate whether the proposed outcome-group trading layer correctly supports complete-set basket arbitrage, cross-venue outcome arbitrage, and future maker/taker strategies without hardcoding venue, market, team, outcome, or strategy-specific behavior.

Context:
- NautilusTrader owns venue adapters, instruments, books, fee calculation, signing, submit/cancel, and fill parsing.
- Bolt must own only cross-instrument semantics: grouping proof, payout matrix, basket scanning, basket-level admission, partial-fill state, and evidence.
- Polymarket and Hyperliquid HIP-4 should both normalize into the same shared OutcomeGroup model.
- Runtime scope must come from TOML config.
- Strategies produce intent only; execution mechanics must stay in shared admission/execution modules or NT.

Review the plan for:
1. Any hidden hardcoding to Polymarket, HIP-4, World Cup, moneyline, YES-only, or a specific strategy.
2. Any place where venue-specific metadata leaks past the normalizer into scanner, admission, or strategy logic.
3. Any invalid assumption that negRisk, event slug, sports market type, or outcome labels alone prove mutual exclusivity and exhaustiveness.
4. Any missing metadata needed to prove terminal states, payout matrix correctness, cancellation behavior, void behavior, or settlement equivalence.
5. Any partial-fill or non-atomic execution failure mode not handled by the proposed state machine.
6. Any duplicated venue functionality that should be delegated to NautilusTrader.
7. Any unsafe scan-all, stale-book, fee, slippage, or liquidity assumptions.
8. Any architecture flaw that would prevent turning up Hyperliquid HIP-4 mostly through config once its OutcomeGroup normalizer exists.
9. Any violation of the repo constraints: no hardcodes, no dual paths, no debts, no credential display, pure Rust, SSM-only secrets, and strategy-intent-only boundaries.

Return:
- Blocking findings first, with severity.
- Concrete fixes.
- Remaining risks after fixes.
- A revised implementation order if the current order is unsafe.
```

## Self-Review

- Spec coverage: covers basket arbitrage, cross-venue arbitrage, taker/maker integration path, non-updown Polymarket, and HIP-4 support.
- Placeholder scan: no deferred implementation placeholders are used as accepted behavior; each task names files, tests, and implementation scope.
- Type consistency: the shared model names are stable across normalizers, scanner, admission, execution, and review prompt.
