# Outcome Group Arbitrage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a shared outcome-group trading layer that supports complete-set basket arbitrage, cross-venue outcome arbitrage, and future maker/taker outcome strategies without hardcoding venue, market, team, outcome, or strategy-specific behavior.

**Architecture:** NautilusTrader owns venue adapters, instrument models, books, order construction, fee calculation, signing, submit/cancel, and fill parsing. Bolt owns normalized outcome grouping, payout matrices, basket-level admission, evidence, durable multi-leg execution state, and strategy intent. Venue-specific Bolt code stops at thin metadata normalizers that translate NT/venue metadata into a shared `OutcomeGroup`; scanner and admission code must reuse existing Bolt executable-cost, fee, freshness, market-family, submit-admission arithmetic, and durable-store patterns rather than creating parallel engines.

**Tech Stack:** Rust, NautilusTrader Rust adapters, TOML runtime config, Bolt v3 provider bindings, Bolt v3 admission/evidence/order-intent modules, Polymarket Gamma/CLOB metadata, Hyperliquid HIP-4 `outcomeMeta`.

---

## Non-Negotiable Invariants

- Runtime scope comes from TOML, not code.
- No code embeds World Cup, team names, event slugs, market slugs, token IDs, venue IDs, strategy IDs, or YES-only assumptions.
- Strategies produce intent only. Venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, and submit gating remain in shared execution/admission modules or NT.
- NT remains the source of venue mechanics: instruments, books, fees, signing, order submit/cancel, and fill parsing.
- Bolt owns cross-instrument semantics: grouping proof, payout matrix, basket scanner, basket-level admission, partial-fill state, and decision evidence.
- Discovery may be broad only when bounded by config caps such as explicit slugs, max groups, max markets, freshness limits, and notional caps.
- Polymarket event slug, sports market type, and `neg_risk = true` do not prove a complete set. A Polymarket group is admissible only when the normalizer can prove a single non-null `negRiskMarketID`, mutually exclusive legs, exact terminal-state enumeration, and void/refund handling.
- Every `OutcomeGroup` must enumerate every terminal state, including void/refund/fallback states. Missing terminal-state proof is a hard rejection.
- Cross-venue matching is disabled unless each venue source supplies an operator-attested settlement contract and those contracts match exactly on source, timing, void, rounding, and terminal-state semantics.
- Freshness is config-owned and mandatory for live basket scanning and submit. Freshness is checked at scan, admission, and `Reserved -> Submitting`.
- In-flight baskets are durable. Any state transition that can leave real venue exposure must be persisted before the next venue mutation.

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
- NT Hyperliquid parses HIP-4 outcome metadata into `HyperliquidInstrumentDef` through `parse_outcome_instruments`, then converts those definitions into `BinaryOption` instruments in `crates/adapters/hyperliquid/src/http/parse.rs`.
- Bolt currently maps Polymarket data filters only from updown target plans in `src/bolt_v3_providers/polymarket.rs`.
- Bolt Hyperliquid already has a `Hip4Outcomes` product surface, with live submit approval-gated in `src/bolt_v3_providers/hyperliquid.rs`.
- NT Polymarket models parse `negRiskMarketID`, but `build_info_json` does not carry it onto `BinaryOption.info`; Bolt cannot prove Polymarket grouping from NT instruments alone.
- Bolt root config loads strategies through `strategy_files`; strategy config is one file per strategy using `strategy_archetype`, raw `[target]`, and raw `[parameters]`.
- Bolt already has reusable primitives this plan must not duplicate: `bolt_v3_executable_cost`, `FeeProvider`, `bolt_v3_submit_admission` arithmetic helpers, `bolt_v3_kill_switch_store` durable-write pattern, `GateProviderFreshnessBlock`, and the `bolt_v3_market_families` target-routing layer.

## Review-Driven Corrections

This revision incorporates the blocking architecture review. The implementation is not allowed to proceed until these corrections are represented in tests:

- Complete-set grouping is keyed by source-specific grouping proof, not event membership. For Polymarket, that proof requires a single non-null `negRiskMarketID` plus explicit terminal-state and void/refund proof.
- The plan wraps the existing `bolt_v3_market_families` routing layer with a new outcome-group family binding; it does not replace the family layer.
- The scanner composes existing per-leg executable-cost helpers; it does not reimplement depth walking, fee arithmetic, or slippage arithmetic.
- Fees are read through the existing `FeeProvider` and instrument metadata. `OutcomeLeg` does not carry a parallel fee model.
- Stateful single-order admission is not called once per leg as basket admission. Basket admission uses shared arithmetic helpers and owns one basket-level reservation.
- Basket state is durable and reconciled against NT reports on restart.
- Cross-venue matching requires operator-attested settlement contracts and remains fail-closed without them.
- The config examples below use the real Bolt root `strategy_files` model and per-strategy files.

## Shared Model

Create a venue-neutral model that strategy and scanner code consume.

```rust
pub struct OutcomeGroup {
    pub group_id: OutcomeGroupId,
    pub source_client_id: ClientId,
    pub venue: Venue,
    pub source_kind: OutcomeGroupSourceKind,
    pub terminal_states: BTreeMap<TerminalStateId, TerminalState>,
    pub tradable_legs: BTreeMap<OutcomeLegId, OutcomeLeg>,
    pub payout_matrix: PayoutMatrix,
    pub grouping_proof: GroupingProof,
    pub settlement_rules: SettlementRules,
    pub freshness: GateProviderFreshnessBlock,
    pub metadata_fingerprint: String,
}

pub struct OutcomeLeg {
    pub leg_id: OutcomeLegId,
    pub instrument_id: InstrumentId,
    pub native_leg_id: String,
    pub outcome_label: String,
    pub side_label: String,
    pub leg_role: OutcomeLegRole,
}

pub struct PayoutMatrix {
    pub cols: Vec<OutcomeLegId>,
    pub payout_per_unit_by_state: BTreeMap<TerminalStateId, Vec<Decimal>>,
}

pub enum GroupingProof {
    PolymarketNegRisk {
        neg_risk_market_id: String,
        event_slug: String,
        market_slugs: Vec<String>,
        proof_fingerprint: String,
    },
    HyperliquidOutcome {
        outcome_question_id: String,
        outcome_indices: Vec<u64>,
        proof_fingerprint: String,
    },
    OperatorAttested {
        attestation_id: String,
        attestation_sha256: String,
    },
}

pub struct SettlementRules {
    pub settlement_contract_id: String,
    pub attestation_sha256: String,
    pub settlement_source_kind: SettlementSourceKind,
    pub terminal_state_convention: TerminalStateConvention,
    pub void_policy: VoidPolicy,
    pub rounding_policy: RoundingPolicy,
    pub timing_policy: SettlementTimingPolicy,
}
```

Rules:

- The basket scanner accepts only `OutcomeGroup`.
- Polymarket Gamma field names and Hyperliquid `outcomeMeta` field names are not visible to scanner, admission, or strategy modules.
- YES, NO, and venue-native side labels are metadata, not control-flow branches.
- Basket profitability is computed from state-wise payouts, not from a hardcoded "sum YES prices below one" formula.
- `OutcomeGroup.terminal_states` is the only terminal-state source of truth. `PayoutMatrix.payout_per_unit_by_state` must have exactly the same terminal-state IDs, and every row length must exactly match `PayoutMatrix.cols`.
- Square matrix dimensions are not enough. Validation must reject transposed, duplicate, missing, or unknown row/column mappings.
- Cost units are Decimal settlement-currency notionals. Any existing f64 cent helpers must be converted at one explicit boundary with tests for edge-threshold stability.

## Files And Responsibilities

- Create `src/bolt_v3_outcome_groups.rs`: shared model, validation, metadata fingerprinting, payout matrix helpers.
- Create `src/bolt_v3_outcome_group_sources.rs`: TOML config model for enabled outcome-group sources and bounded discovery rules.
- Create `src/bolt_v3_market_families/outcome_group.rs`: market-family binding that lets the existing target-routing layer dispatch complete-set strategies without duplicating `MarketIdentityPlan`.
- Create `src/bolt_v3_outcome_group_polymarket.rs`: Polymarket Gamma/NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`: Hyperliquid HIP-4 NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_complete_set_scanner.rs`: shared payout-vector scanner that reuses `bolt_v3_executable_cost` for per-leg executable costs.
- Create `src/bolt_v3_basket_admission.rs`: basket-level admission, risk caps, freshness checks, reservation release, and evidence payloads using existing submit-admission arithmetic helpers.
- Create `src/bolt_v3_basket_execution.rs`: durable multi-leg execution state machine.
- Create `src/bolt_v3_basket_store.rs`: basket-state persistence using the existing atomic-write pattern from `bolt_v3_kill_switch_store`.
- Modify `src/bolt_v3_config.rs`: parse root-level `outcome_group_sources`; complete-set strategy settings remain in the per-strategy file's raw `[target]` and `[parameters]`.
- Modify `src/bolt_v3_providers/polymarket.rs`: expose NT discovery filters from TOML scopes without changing strategy logic.
- Modify `src/bolt_v3_providers/hyperliquid.rs`: expose HIP-4 outcome group source wiring through existing Hyperliquid adapter and approval gates.
- Modify `src/bolt_v3_strategy_registration.rs`: bind the complete-set strategy into the live node path, not only the static strategy registry.
- Modify `src/lib.rs`: export new shared modules.
- Add focused tests under `tests/` for each module and provider mapping boundary.

## Config Contract

Outcome-group sources are config-owned. Root config owns source definitions and `strategy_files`; each strategy is still loaded from its own strategy file. The following is the intended shape; production values are operator-provided in TOML.

```toml
# config/root.toml
strategy_files = ["config/strategies/complete_set_arbitrage.toml"]

[[outcome_group_sources]]
source_id = "polymarket_event_moneyline"
client_id = "polymarket_main"
kind = "polymarket_gamma_event"
event_slugs = ["operator_supplied_event_slug"]
sports_market_types = ["moneyline"]
expected_neg_risk_market_id = "operator_supplied_neg_risk_market_id"
terminal_state_labels = ["operator_state_a", "operator_state_b", "operator_state_c"]
max_markets = 20
enabled = true

[outcome_group_sources.freshness]
max_age_ms = 500
max_clock_skew_ms = 250

[outcome_group_sources.settlement_rules]
settlement_contract_id = "operator_attested_contract_id"
settlement_source_kind = "polymarket_ctf_uma"
terminal_state_convention = "exactly_one_winner"
void_policy = "refund_all_legs"
rounding_policy = "decimal_exact"
timing_policy = "venue_final_resolution"
attestation_sha256 = "operator_supplied_lowercase_sha256"

[[outcome_group_sources]]
source_id = "hyperliquid_hip4_outcomes"
client_id = "hyperliquid_main"
kind = "hyperliquid_hip4"
outcome_question_ids = ["operator_supplied_question_id"]
terminal_state_labels = ["operator_state_a", "operator_state_b", "operator_state_c"]
max_groups = 20
enabled = false

[outcome_group_sources.freshness]
max_age_ms = 500
max_clock_skew_ms = 250

[outcome_group_sources.settlement_rules]
settlement_contract_id = "operator_attested_contract_id"
settlement_source_kind = "hyperliquid_outcome_question"
terminal_state_convention = "exactly_one_winner"
void_policy = "operator_attested_fallback"
rounding_policy = "decimal_exact"
timing_policy = "venue_final_resolution"
attestation_sha256 = "operator_supplied_lowercase_sha256"
```

Strategy file:

```toml
# config/strategies/complete_set_arbitrage.toml
schema_version = 2
strategy_instance_id = "complete_set_arb_main"
strategy_archetype = "complete_set_arbitrage"
order_id_tag = "003"
oms_type = "netting"
use_uuid_client_order_ids = true
use_hyphens_in_client_order_ids = false
external_order_claims = []
manage_contingent_orders = false
manage_gtd_expiry = false
manage_stop = false
market_exit_interval_ms = 100
market_exit_max_attempts = 100
log_events = true
log_commands = true
log_rejected_due_post_only_as_warning = true
execution_client_id = "polymarket_main"

[target]
configured_target_id = "complete_set_arb_target"
rotating_market_family = "outcome_group"
group_sources = ["polymarket_event_moneyline"]

[parameters.runtime]
min_edge_bps = 25
max_basket_notional = "10"
max_open_baskets = 1
submit_mode = "taker_ioc"
max_repair_attempts = 1
max_unwind_attempts = 1
```

Validation rules:

- `source_id` must be unique.
- `client_id` must reference a configured client.
- Query-style sources must include bounded selectors and caps.
- Event-style sources must include explicit event slugs or a bounded event query.
- Polymarket sources must prove grouping with a single non-null `expected_neg_risk_market_id`; event slugs scope discovery only.
- Sources must declare terminal-state labels and settlement rules, including void/refund policy.
- Sources must declare `freshness.max_age_ms` and `freshness.max_clock_skew_ms` using the existing `GateProviderFreshnessBlock` semantics.
- HIP-4 sources must use a Hyperliquid client with `Hip4Outcomes` enabled and required live-submit approvals before live trading.
- Complete-set strategies must reference at least one enabled source.
- Complete-set strategies require positive edge, positive notional cap, max open basket cap, bounded repair/unwind attempts, and evidence output enabled.

## Basket Math

The scanner evaluates generic payout vectors.

```text
candidate_cost = sum(per-leg executable costs from bolt_v3_executable_cost + fees from FeeProvider + buffers)
state_payouts = payout_matrix * leg_quantities
guaranteed_payout = min(state_payouts)
edge = guaranteed_payout - candidate_cost
admit when edge > configured_min_edge and every grouping/settlement/risk/freshness check passes
```

Examples represented by the same model:

- All-YES complete-set basket across N exhaustive states: guaranteed payout is one unit when exactly one state resolves true.
- All-NO basket across N exhaustive states: guaranteed payout is N - 1 units if exactly one state resolves true.
- Mixed YES/NO basket: guaranteed payout is computed from the payout matrix; no special-case branch is needed.
- Void/refund/fallback states: represented as explicit rows. If a source cannot prove them, no basket is admitted.
- Cross-venue basket: each leg may come from a different source client only after settlement contracts prove equivalent terminal states and payout conventions.

Sizing rules:

- Basket size is capped by the minimum fillable depth across all selected legs after applying configured depth, slippage, fee, freshness, and per-leg notional constraints.
- The scanner must not request a quantity that any required leg cannot fill at the admitted cost.
- Submit re-checks the same depth/freshness constraints immediately before venue mutation.

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
  -> Stuck
  -> Closed
```

State responsibilities:

- `Candidate`: scanner found an edge from fresh executable books.
- `Reserved`: basket-level notional and exposure budget reserved and durably written.
- `Submitting`: orders are in flight; no second basket can consume the same budget.
- `Partial`: at least one leg filled and at least one leg missing or short.
- `Complete`: all required leg quantities filled within tolerance.
- `Repair`: submit bounded corrective orders to restore guaranteed-payout shape; retry budget is config-owned.
- `Unwind`: reduce residual directional exposure when repair is not admissible and the market is still tradable.
- `Stuck`: cancel rejected, repair/unwind retry budget exhausted, market settled before exposure neutralized, or restart reconciliation finds unresolved exposure. This state blocks `Closed`, holds reservation, and trips the existing kill-switch path.
- `Closed`: final accounting and evidence written after every order, cancel, reservation, and position effect is reconciled.

State machine rules:

- Every in-flight-money transition persists before the next venue action.
- Abort, reject, cancel, completion, repair, unwind, and restart reconciliation release or retain reservation explicitly.
- Cancel rejection, stale submit re-check, settled market, and retry exhaustion are distinct transitions with evidence.
- Restart loads durable basket state and reconciles it against NT order/fill/position reports before admitting new baskets.

## Implementation Tasks

### Task 1: Shared Outcome Model

**Files:**
- Create `src/bolt_v3_outcome_groups.rs`
- Modify `src/lib.rs`
- Test `tests/bolt_v3_outcome_groups.rs`

- [ ] Write tests for valid groups, duplicate leg IDs, empty terminal states, missing void/fallback row, payout matrix dimension mismatch, transposed square matrix rejection, unknown terminal-state IDs, unknown leg IDs, and metadata fingerprint stability.
- [ ] Implement `OutcomeGroup`, `OutcomeLeg`, `TerminalState`, `PayoutMatrix`, `GroupingProof`, `SettlementRules`, and validation helpers.
- [ ] Enforce one terminal-state source of truth: `OutcomeGroup.terminal_states`.
- [ ] Keep fees out of `OutcomeLeg`; costs must be resolved through `FeeProvider` and existing instrument metadata at scan time.
- [ ] Export the module from `src/lib.rs`.
- [ ] Run allowed local formatting checks and remote Rust verification according to repo policy after commit.

### Task 2: Config-Driven Outcome Sources

**Files:**
- Create `src/bolt_v3_outcome_group_sources.rs`
- Modify `src/bolt_v3_config.rs`
- Test `tests/config_parsing.rs`

- [ ] Write tests for root-level source parsing, per-strategy file parsing, Polymarket event source parsing, Hyperliquid HIP-4 source parsing, duplicate `source_id` rejection, unbounded query rejection, missing `freshness`, missing settlement rules, missing terminal states, missing `expected_neg_risk_market_id`, and unknown client rejection.
- [ ] Implement source config enums and validators.
- [ ] Keep all runtime selectors in TOML.
- [ ] Reject scan-all configs unless they include explicit bounded caps and freshness controls.
- [ ] Add an `outcome_group` market-family binding so existing `target.rotating_market_family` dispatch remains the single target-routing path.

### Task 3: Polymarket OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_polymarket.rs`
- Modify `src/bolt_v3_providers/polymarket.rs`
- Test `tests/bolt_v3_polymarket_outcome_groups.rs`

- [ ] Write tests using synthetic Gamma event/market metadata for a three-way moneyline group with one shared non-null `negRiskMarketID`.
- [ ] Prove that both YES and NO token legs are preserved.
- [ ] Prove that grouping requires `negRiskMarketID` proof and does not rely on event slug, sports market type, question text, or slug patterns alone.
- [ ] Prove that event containers with unrelated markets are rejected unless all admitted markets share the same grouping key and terminal-state contract.
- [ ] Prove that missing void/refund policy rejects the group.
- [ ] Map Polymarket Gamma/NT metadata to `OutcomeGroup`.
- [ ] Preserve `negRiskMarketID` through one grouping metadata path: either extend pinned NT to carry it into instrument metadata or maintain one Bolt-owned Gamma metadata cache keyed by token/condition/market slug for the normalizer. Do not support both at runtime.
- [ ] Add provider mapping for TOML-driven NT discovery filters: explicit event slugs, market slugs, event queries, and bounded Gamma queries.
- [ ] Avoid per-scope NT `accept()` outcome predicates because NT applies accept predicates globally across filters.

### Task 4: Read-Only Complete-Set Scanner

**Files:**
- Create `src/bolt_v3_complete_set_scanner.rs`
- Test `tests/bolt_v3_complete_set_scanner.rs`

- [ ] Write tests for all-YES, all-NO, mixed baskets, void/refund rows, insufficient depth, stale book, fee inclusion, minimum fillable depth sizing, Decimal/f64 conversion boundary, and non-positive edge.
- [ ] Implement payout-vector evaluation from `PayoutMatrix`.
- [ ] Reuse `bolt_v3_executable_cost::price_exact_size_vwap` and `bolt_v3_executable_cost::executable_cost_breakdown` for per-leg executable depth and adjusted cost.
- [ ] Use existing `FeeProvider` to resolve fee inputs by instrument; do not introduce a parallel fee model.
- [ ] Use executable depth, not display or Gamma prices.
- [ ] Return scanner evidence with grouping proof, costs, fees, state payouts, guaranteed payout, min-depth cap, freshness readings, and block reason.

### Task 5: Basket Admission

**Files:**
- Create `src/bolt_v3_basket_admission.rs`
- Modify `src/bolt_v3_decision_evidence.rs`
- Test `tests/bolt_v3_basket_admission.rs`

- [ ] Write tests for basket notional cap, max open basket cap, stale evidence rejection, stale submit re-check rejection, negative edge rejection, missing grouping proof rejection, missing settlement rules rejection, reservation release, and retry-budget rejection.
- [ ] Implement basket-level admission that reserves the whole basket, not individual legs independently.
- [ ] Reuse `bolt_v3_submit_admission` pure arithmetic helpers; do not call stateful single-order `BoltV3SubmitAdmissionState::admit()` once per basket leg.
- [ ] Record evidence keyed by strategy id, basket id, group id, and leg instrument IDs.
- [ ] Persist reservation state before any venue mutation.

### Task 6: Basket Execution State Machine

**Files:**
- Create `src/bolt_v3_basket_execution.rs`
- Create `src/bolt_v3_basket_store.rs`
- Test `tests/bolt_v3_basket_execution.rs`

- [ ] Write state-transition tests for complete fill, partial fill, repair allowed, repair denied, unwind allowed, unwind denied after settlement, cancel rejection, retry-budget exhaustion, restart reconciliation, stuck state, reservation release, and terminal close.
- [ ] Implement state transitions as pure logic first.
- [ ] Implement durable state persistence using the existing atomic-store pattern from `bolt_v3_kill_switch_store`.
- [ ] Keep NT submit/cancel calls outside pure state logic.
- [ ] Require bounded repair/unwind policies from TOML before any live submit path uses them.
- [ ] Trip the existing kill-switch path for `Stuck` baskets that leave unresolved real venue exposure.

### Task 7: Hyperliquid HIP-4 OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`
- Modify `src/bolt_v3_providers/hyperliquid.rs`
- Test `tests/bolt_v3_hyperliquid_outcome_groups.rs`

- [ ] Write tests using synthetic NT HIP-4 `BinaryOption` metadata.
- [ ] Prove that the existing Hyperliquid adapter path remains responsible for discovery and execution.
- [ ] Prove that standalone HIP-4 outcomes without a parent question settlement signal are rejected unless an operator-attested settlement contract supplies terminal-state and void/fallback semantics.
- [ ] Convert HIP-4 outcome metadata into `OutcomeGroup`.
- [ ] Enforce existing Hyperliquid product-surface and live-submit approval gates.

### Task 8: Cross-Venue Normalization

**Files:**
- Create `src/bolt_v3_cross_venue_outcome_groups.rs`
- Test `tests/bolt_v3_cross_venue_outcome_groups.rs`

- [ ] Write tests proving two venue groups can be matched only when their operator-attested settlement contracts are byte-equal on source, terminal-state convention, void policy, rounding policy, and timing policy.
- [ ] Implement semantic group matching with explicit evidence fields and no auto-match path without settlement contracts.
- [ ] Reject groups when cancellation, void, timing, settlement source, terminal states, rounding, or fallback semantics differ.
- [ ] Feed matched groups into the same complete-set scanner.

### Task 9: Strategy Registration

**Files:**
- Modify `src/strategies/registry.rs`
- Modify `src/bolt_v3_strategy_registration.rs`
- Modify `src/bolt_v3_market_families/mod.rs`
- Create strategy wrapper module only if existing registry requires a concrete strategy entry point.
- Test `tests/bolt_v3_strategy_registration.rs`

- [ ] Register a generic `complete_set_arbitrage` strategy that consumes `OutcomeGroup` sources.
- [ ] Register the `outcome_group` market-family binding so `target.rotating_market_family = "outcome_group"` is reachable during adapter mapping.
- [ ] Add an end-to-end node-binding test proving the strategy is reachable from a root `strategy_files` entry.
- [ ] Keep strategy code limited to signal/intent and local signal state.
- [ ] Route all admission and execution mechanics through shared modules.

## Delivery Order

1. Shared model with grouping proof, settlement rules, void rows, and matrix validation.
2. Config parsing that matches Bolt root `strategy_files` plus per-strategy files.
3. Polymarket normalizer with `negRiskMarketID` grouping proof and discovery wiring.
4. Read-only complete-set scanner that reuses `bolt_v3_executable_cost` and `FeeProvider`.
5. Basket admission, evidence, freshness re-check, reservation release, and durable reservation state.
6. Basket execution state machine with `Stuck`, bounded repair/unwind, restart reconciliation, and kill-switch integration.
7. Runtime strategy registration and node-binding proof.
8. HIP-4 normalizer through existing Hyperliquid adapter and settlement-contract gates.
9. Cross-venue matching after operator-attested settlement contracts exist.
10. Maker/taker enhancements using the same outcome and order layers.

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
- Polymarket negRiskMarketID is the required exclusivity key; event slug, sports market type, neg_risk=true, question text, and outcome labels are discovery hints only.
- Every admissible group must prove every terminal state, including void/refund/fallback behavior.
- Cross-venue auto-matching is forbidden unless operator-attested settlement contracts are byte-equal on source, terminal-state convention, void policy, rounding policy, and timing policy.
- The plan is expected to reuse existing Bolt engines: bolt_v3_executable_cost, FeeProvider, bolt_v3_submit_admission arithmetic helpers, bolt_v3_kill_switch_store, GateProviderFreshnessBlock, and bolt_v3_market_families.

Review the plan for:
1. Any hidden hardcoding to Polymarket, HIP-4, World Cup, moneyline, YES-only, NO-only, a venue client, a market slug, or a specific strategy.
2. Any place where venue-specific metadata leaks past the normalizer into scanner, admission, execution, or strategy logic.
3. Any grouping proof that accepts Polymarket event membership without one shared non-null negRiskMarketID, or accepts HIP-4 standalone outcomes without a parent settlement signal or attested settlement contract.
4. Any terminal-state gap: missing void/refund/fallback row, duplicate terminal-state source of truth, unrecognized outcome label, or payout-matrix row/column alignment bug.
5. Any cost-engine duplication instead of reusing bolt_v3_executable_cost, FeeProvider, submit-admission arithmetic helpers, GateProviderFreshnessBlock, market-family routing, and atomic-store patterns.
6. Any freshness gap where max_age_ms/max_clock_skew_ms are absent, optional in live trading, or checked only before a non-atomic submit rather than also at Reserved -> Submitting.
7. Any partial-fill, cancel-reject, repair-recursion, settled-market, restart-reconciliation, reservation-release, or Stuck-state failure mode not handled by the proposed state machine.
8. Any unsafe scan-all, stale-book, fee-unit, slippage, minimum-depth, or liquidity assumption that could admit a basket that cannot fill atomically enough to preserve the payout floor.
9. Any config-contract mismatch with Bolt's real root strategy_files plus per-strategy strategy_archetype/[target]/[parameters] shape.
10. Any missing runtime registration path, especially registry.rs without bolt_v3_strategy_registration.rs and market-family binding.
11. Any architecture flaw that would prevent turning up Hyperliquid HIP-4 mostly through config once its OutcomeGroup normalizer exists.
12. Any violation of the repo constraints: no hardcodes, no dual paths, no debts, no credential display, pure Rust, SSM-only secrets, and strategy-intent-only boundaries.

Return:
- Blocking findings first, with severity.
- Concrete fixes.
- Remaining risks after fixes.
- A revised implementation order if the current order is unsafe.
```

## Self-Review

- Spec coverage: covers basket arbitrage, cross-venue arbitrage, taker/maker integration path, non-updown Polymarket, and HIP-4 support.
- Review corrections: grouping proof, void/refund/fallback terminal states, settlement attestation, freshness, cost-engine reuse, durable basket state, Stuck handling, config shape, and runtime registration are represented as explicit plan requirements.
- Placeholder scan: no deferred implementation placeholders are used as accepted behavior; each task names files, tests, and implementation scope.
- Type consistency: the shared model names are stable across normalizers, scanner, admission, execution, and review prompt.
