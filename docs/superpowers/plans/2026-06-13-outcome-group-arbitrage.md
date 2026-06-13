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
- Every `OutcomeGroup` must enumerate every terminal state, including void/refund/fallback states. Missing terminal-state proof or missing terminal-state payout derivation is a hard rejection.
- Config values such as `expected_neg_risk_market_id` and `terminal_state_labels` are checked expectations, not proof. Venue metadata or operator attestation remains the proof source.
- Cross-venue matching is disabled unless each venue source supplies an operator-attested settlement contract and those contracts match exactly on source, timing, void, rounding, and terminal-state semantics.
- Same-venue baskets are the first execution unit: one basket belongs to one `OutcomeGroup`, one `source_client_id`, and one `execution_client_id`. Cross-venue work is read-only group comparison until a later multi-client basket model is explicitly designed.
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
- Bolt strategy runtime bindings live in `src/bolt_v3_archetypes/mod.rs`; `src/bolt_v3_strategy_registration.rs` is the generic binding-injected dispatcher.
- The kill-switch store's atomic-write helpers are private today, so basket persistence needs a shared atomic I/O helper instead of copying the helper body.
- Provider adapter validation rejects unsupported market families; Polymarket currently supports only `updown`, and Hyperliquid currently supports `updown` plus `hyperliquid_instrument`.
- Core startup validation currently requires `realized_volatility_surface_id` for every strategy before archetype-specific validation runs.
- `BoltV3RootConfig` uses `deny_unknown_fields`, so `outcome_group_sources` must be added to the root struct before the example root TOML can parse.

## Review-Driven Corrections

This revision incorporates the blocking architecture review. The implementation is not allowed to proceed until these corrections are represented in tests:

- Complete-set grouping is keyed by source-specific grouping proof, not event membership. For Polymarket, that proof requires a single non-null `negRiskMarketID` plus explicit terminal-state and void/refund proof.
- The plan wraps the existing `bolt_v3_market_families` routing layer with a new outcome-group family binding; it does not replace the family layer.
- The scanner composes existing per-leg executable-cost helpers; it does not reimplement depth walking, fee arithmetic, or slippage arithmetic.
- Fees are read through the existing `FeeProvider` and instrument metadata. `OutcomeLeg` does not carry a parallel fee model.
- Stateful single-order admission is not called once per leg as basket admission. Basket admission uses shared arithmetic helpers and owns one basket-level reservation.
- Basket state is durable and reconciled against NT reports on restart.
- Durable basket state reuses a promoted shared atomic-write helper; no second atomic-store implementation is allowed.
- Polymarket `negRiskMarketID` recovery uses one provider-local Bolt-owned Gamma metadata cache keyed by native token/condition/market identifiers and used to emit NT filters. The plan does not fork the pinned NT dependency and does not support a second runtime recovery path.
- Cross-venue matching requires operator-attested settlement contracts and remains fail-closed without them.
- Runtime registration is implemented as a new archetype binding under `src/bolt_v3_archetypes`, plus the binding-list entry in `src/bolt_v3_archetypes/mod.rs`. The generic registration dispatcher is not the concrete-builder home.
- Outcome-group provider support is explicit: every provider that may host outcome-group strategies must add `outcome_group::KEY` to `SUPPORTED_MARKET_FAMILIES` and prove adapter validation accepts it.
- Root `outcome_group_sources` are first-class root config fields and project into provider discovery filters through the outcome-group market-family binding; they are not inert strategy-only metadata.
- Complete-set archetype validation owns its own volatility requirement. Core startup validation must stop globally requiring `realized_volatility_surface_id`; `binary_oracle_edge_taker` keeps its RV requirement, while `complete_set_arbitrage` declares none.
- The config examples below use the real Bolt root `strategy_files` model and per-strategy files.

## Shared Model

Create a venue-neutral model that strategy and scanner code consume.

```rust
pub struct OutcomeGroup {
    pub group_id: OutcomeGroupId,
    pub source_client_id: ClientId,
    pub venue: Venue,
    pub source_kind: OutcomeGroupSourceKind,
    pub settlement_asset_id: SettlementAssetId,
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
    pub settlement_asset_id: SettlementAssetId,
    pub outcome_label: String,
    pub side_label: String,
    pub leg_role: OutcomeLegRole,
}

pub enum OutcomeLegRole {
    PaysOnTerminalState(TerminalStateId),
    PaysUnlessTerminalState(TerminalStateId),
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
    pub non_standard_terminal_payouts: BTreeMap<TerminalStateId, TerminalPayoutDerivation>,
    pub rounding_policy: RoundingPolicy,
    pub timing_policy: SettlementTimingPolicy,
}

pub enum TerminalPayoutDerivation {
    OperatorAttestedVector {
        convention: RefundConvention,
        cols: Vec<OutcomeLegId>,
        payout_per_unit: Vec<Decimal>,
        attestation_sha256: String,
    },
}
```

Rules:

- The basket scanner accepts only `OutcomeGroup`.
- Polymarket Gamma field names and Hyperliquid `outcomeMeta` field names are not visible to scanner, admission, or strategy modules.
- YES, NO, and venue-native side labels are metadata, not control-flow branches.
- Basket profitability is computed from state-wise payouts, not from a hardcoded "sum YES prices below one" formula.
- `OutcomeGroup.terminal_states` is the only terminal-state source of truth. `PayoutMatrix.payout_per_unit_by_state` must have exactly the same terminal-state IDs, and every row length must exactly match `PayoutMatrix.cols`.
- Standard terminal-state rows are derived only from `OutcomeLegRole` and `terminal_state_convention`. Under `exactly_one_winner`, `PaysOnTerminalState(T)` pays `1` when the row state is `T` and `0` otherwise; `PaysUnlessTerminalState(T)` pays `0` when the row state is `T` and `1` otherwise. Matrix builders must not inspect `side_label` or `outcome_label` strings.
- Void/refund/fallback rows must come from `SettlementRules.non_standard_terminal_payouts`, not from inferred labels. Each operator-attested vector must name the same columns as the payout matrix, match leg count exactly, declare its refund convention, use bounded settlement-currency values, and be covered by the settlement attestation hash.
- Operator-attested vectors resolve config leg references to `OutcomeLegId` using a unique native leg id or a unique `(outcome_label, side_label)` tuple. Validation requires order equality with `PayoutMatrix.cols`, not just set equality or count equality, and rejects ambiguous or duplicate label resolution.
- Attestation hashes are verified at validation time by hashing the canonical attestation payload after removing every digest field, including `attestation_sha256` and any nested digest fields. Validation requires lowercase 64-character SHA-256 hex and rejects mismatches, reordered columns, or payloads whose digest changes only by adding or changing digest fields.
- Square matrix dimensions are not enough. Validation must reject transposed, duplicate, missing, or unknown row/column mappings.
- Outcome labels are metadata only after validation. Unknown labels, duplicate labels that map to different states, or labels that cannot be mapped to an attested terminal state reject the group.
- Every `OutcomeGroup` must have one settlement asset. Every leg's `settlement_asset_id` must equal `OutcomeGroup.settlement_asset_id`; mixed-currency groups reject before scanning.
- Cost units are Decimal settlement-currency notionals. Any existing f64 cent helpers must be converted at one explicit boundary with tests for edge-threshold stability.
- Existing cent-based executable-cost helpers assume normalized binary prices where `1.0` payout equals 100 cents. Normalizers must scale supported outcome instruments into that convention before scanner input; instruments without that scale proof reject.
- `GroupingProof` is opaque outside normalizers and evidence serialization. Scanner, admission, execution, and strategy modules may check that proof exists and may include fingerprints in evidence, but must not branch on `GroupingProof` variants.
- `metadata_fingerprint` and `proof_fingerprint` use canonical serialization: sorted map keys, sorted leg IDs, sorted terminal-state IDs, normalized Decimal strings, and no insertion-order dependence.

## Files And Responsibilities

- Create `src/bolt_v3_outcome_groups.rs`: shared model, validation, metadata fingerprinting, payout matrix helpers.
- Create `src/bolt_v3_outcome_group_sources.rs`: TOML config model for enabled outcome-group sources and bounded discovery rules.
- Create `src/bolt_v3_atomic_io.rs`: shared temp-write, fsync, rename, and parent-directory sync helpers promoted from the kill-switch store pattern.
- Create `src/bolt_v3_market_families/outcome_group.rs`: market-family binding that lets the existing target-routing layer dispatch complete-set strategies without duplicating `MarketIdentityPlan`.
- Create `src/bolt_v3_outcome_group_polymarket.rs`: Polymarket Gamma/NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`: Hyperliquid HIP-4 NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_complete_set_scanner.rs`: shared payout-vector scanner that reuses `bolt_v3_executable_cost` for per-leg executable costs.
- Create `src/bolt_v3_basket_admission.rs`: basket-level admission, risk caps, freshness checks, monotonic submit-approval cap integration, releasable exposure reservation, and evidence payloads using existing submit-admission arithmetic helpers.
- Create `src/bolt_v3_basket_execution.rs`: shared durable multi-leg executor outside `src/strategies/*`; it owns fill/cancel-driven `Partial`/`Repair`/`Unwind`/`Stuck` state transitions.
- Create `src/bolt_v3_basket_store.rs`: basket-state persistence using `bolt_v3_atomic_io`, not copied kill-switch private helpers.
- Modify `src/bolt_v3_config.rs`: add `outcome_group_sources` to `BoltV3RootConfig`, add `risk.basket_execution`, and keep complete-set strategy settings in the per-strategy file's raw `[target]` and `[parameters]`.
- Modify `src/bolt_v3_validate.rs`: move global realized-volatility surface requirement behind archetype requirements so complete-set strategies do not need dummy RV surfaces.
- Modify `src/bolt_v3_providers/polymarket.rs`: add `outcome_group::KEY` to `SUPPORTED_MARKET_FAMILIES` and project enabled root outcome-group sources into NT discovery filters.
- Modify `src/bolt_v3_providers/hyperliquid.rs`: add `outcome_group::KEY` to `SUPPORTED_MARKET_FAMILIES` and expose HIP-4 outcome group source wiring through existing Hyperliquid adapter and approval gates.
- Modify `src/bolt_v3_kill_switch.rs`: add a basket-stuck halt trigger kind and constructor.
- Create `src/bolt_v3_archetypes/complete_set_arbitrage.rs`: validation binding, runtime binding, strategy builder, fee-provider resolution, execution-venue lookup, `StrategyBuildContext`, and NT `Trader.add_strategy()` handoff.
- Modify `src/bolt_v3_archetypes/mod.rs`: add the complete-set validation and runtime bindings.
- Modify `src/bolt_v3_strategy_registration.rs` only if the generic `StrategyRegistrationContext` needs a shared dependency that cannot live in the archetype binding.
- Modify `src/lib.rs`: export new shared modules.
- Add focused tests under `tests/` for each module and provider mapping boundary.

## Config Contract

Outcome-group sources are config-owned. Root config owns source definitions and `strategy_files`; each strategy is still loaded from its own strategy file. The following is the intended shape; production values are operator-provided in TOML.

```toml
# config/root.toml
strategy_files = ["config/strategies/complete_set_arbitrage.toml"]

[risk.basket_execution]
enabled = true
state_path = "operator_supplied_basket_state_path"
schema_version = 1
max_state_file_bytes = 1048576
recovery_policy = "fail_closed_reconcile_before_new_baskets"
max_recovery_age_ms = 300000

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

[outcome_group_sources.settlement_rules.non_standard_terminal_payouts.void_refund]
convention = "operator_attested_static_payout_per_unit"
terminal_state_label = "void_refund"
legs = [
  { outcome_label = "operator_state_a", side_label = "operator_yes_side_label", payout_per_unit = "1" },
  { outcome_label = "operator_state_b", side_label = "operator_yes_side_label", payout_per_unit = "1" },
  { outcome_label = "operator_state_c", side_label = "operator_yes_side_label", payout_per_unit = "1" },
]
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

[outcome_group_sources.settlement_rules.non_standard_terminal_payouts.fallback]
convention = "operator_attested_static_payout_per_unit"
terminal_state_label = "fallback"
legs = [
  { outcome_label = "operator_state_a", side_label = "operator_yes_side_label", payout_per_unit = "0" },
  { outcome_label = "operator_state_b", side_label = "operator_yes_side_label", payout_per_unit = "0" },
  { outcome_label = "operator_state_c", side_label = "operator_yes_side_label", payout_per_unit = "0" },
]
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
kind = "static_outcome_group"
rotating_market_family = "outcome_group"
group_sources = ["polymarket_event_moneyline"]

[reference_data]

[signal_data]

[parameters.runtime]
min_edge_bps = 25
max_basket_notional = "10"
max_open_baskets = 1
submit_mode = "taker_ioc"
vwap_depth_limit_bps = 2000
slippage_buffer_bps = 100
max_repair_attempts = 1
max_unwind_attempts = 1
```

Validation rules:

- `BoltV3RootConfig` must declare `outcome_group_sources` and `risk.basket_execution`; unknown-field denial must reject misspelled blocks.
- `risk.basket_execution` must declare `state_path`, `schema_version`, `max_state_file_bytes`, `recovery_policy`, and recovery age limits. Recovery is fail-closed: restart reconciles Bolt basket store state against NT order/fill/position reports before admitting any new basket.
- `source_id` must be unique.
- `client_id` must reference a configured client.
- Query-style sources must include bounded selectors and caps.
- Event-style sources must include explicit event slugs or a bounded event query.
- Provider mappings must extend `SUPPORTED_MARKET_FAMILIES` for every execution client allowed to host outcome-group strategies.
- Outcome-group provider mappings must project enabled `group_sources` into NT discovery filters per `client_id`: Polymarket event slugs, market slugs, event queries, and bounded Gamma queries; Hyperliquid HIP-4 question or product-surface selectors. `subscribe_new_markets` remains false unless a separately approved bounded source proves otherwise.
- Polymarket sources must use `expected_neg_risk_market_id` as a checked expectation. The normalizer still proves the same non-null `negRiskMarketID` from Gamma metadata for every admitted leg; event slugs scope discovery only.
- Sources must declare terminal-state labels and settlement rules, including void/refund policy and per-leg non-standard terminal payout vectors.
- Sources must declare `freshness.max_age_ms` and `freshness.max_clock_skew_ms` using the existing `GateProviderFreshnessBlock` semantics. The source validator treats absent or zero option fields as fatal for live outcome-group sources.
- Freshness compares the local node clock against the latest book/metadata receive timestamp; `max_clock_skew_ms` compares venue/provider event time to local receive time when the provider supplies an event time, and otherwise the source must mark the event-time clock unavailable in evidence and fail any rule that requires venue-time proof.
- HIP-4 sources must use a Hyperliquid client with `Hip4Outcomes` enabled and required live-submit approvals before live trading.
- Complete-set strategies must reference at least one enabled source.
- Complete-set strategies require positive edge, positive notional cap, max open basket cap, `vwap_depth_limit_bps`, `slippage_buffer_bps`, bounded repair/unwind attempts, and evidence output enabled.
- Complete-set archetype validation must enumerate `submit_mode` variants. The first slice supports `taker_ioc`; maker modes are rejected until maker quoting/admission is specified.
- Complete-set archetype validation declares no realized-volatility surface requirement; core validation must not force a dummy `realized_volatility_surface_id`.
- The `outcome_group` target family owns a strict `TargetBlock` with `configured_target_id`, `kind = "static_outcome_group"`, `rotating_market_family = "outcome_group"`, and `group_sources`; unknown fields reject.
- `[reference_data]` and `[signal_data]` remain present in strategy files, even when empty, unless the Rust strategy envelope is intentionally changed to default those maps.

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

- All-`PaysOnTerminalState` basket across N exhaustive states: guaranteed payout is one unit when exactly one state resolves true.
- All-`PaysUnlessTerminalState` basket across N exhaustive states: guaranteed payout is N - 1 units if exactly one state resolves true.
- Mixed role basket: guaranteed payout is computed from the payout matrix; no special-case branch is needed.
- Void/refund/fallback states: represented as explicit rows. If a source cannot prove them, no basket is admitted.
- Cross-venue matching: compares separately executable venue groups after settlement contracts prove equivalent terminal states and payout conventions. A single live basket does not span clients or venues in this plan.

Sizing rules:

- Basket size is capped by the minimum fillable depth across all selected legs after applying configured depth, slippage, fee, freshness, and per-leg notional constraints.
- The scanner must not request a quantity that any required leg cannot fill at the admitted cost.
- Submit re-checks the same depth/freshness constraints immediately before venue mutation.
- The scanner owns new basket aggregation only: NT book state is adapted into a timestamped basket book snapshot, each leg snapshot wraps `ExecutableBookQuote`, per-leg VWAP and adjusted cost come from `bolt_v3_executable_cost`, `FeeProvider::fee_bps` Decimal values convert to f64 bps at one tested boundary, and final basket edge compares Decimal settlement-currency values.
- Each basket book snapshot carries instrument id, local receive time, optional provider event time, and normalized binary price scale evidence. Freshness checks use those timestamps at scan, admission, and `Reserved -> Submitting`.

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

- `src/bolt_v3_basket_execution.rs` is the runtime owner for fill/cancel/reconcile-driven state transitions. The complete-set strategy shell may emit basket intent and strategy-local signal state only.
- Every in-flight-money transition persists before the next venue action.
- Abort, reject, cancel, completion, repair, unwind, and restart reconciliation release or retain reservation explicitly.
- Cancel rejection, stale submit re-check, settled market, and retry exhaustion are distinct transitions with evidence.
- Restart loads durable basket state and reconciles it against NT order/fill/position reports before admitting new baskets. The Bolt basket store records intended basket shape and last known transition; NT order/fill/position reports are authoritative for venue effects during reconciliation.
- `Stuck` trips a dedicated basket-execution kill-switch trigger, not a loss-governor breach trigger.
- Repair uses the current fill vector, payout matrix, fresh books, and configured retry budget to restore the admitted payout floor; unwind uses the same inputs to reduce residual directional exposure when repair is not admissible. Both paths re-run admission/freshness checks before submit.

## Implementation Tasks

### Task 1: Shared Outcome Model

**Files:**
- Create `src/bolt_v3_outcome_groups.rs`
- Modify `src/lib.rs`
- Test `tests/bolt_v3_outcome_groups.rs`

- [ ] Write tests for valid groups, duplicate leg IDs, empty terminal states, standard winner-row derivation, all-`PaysOnTerminalState`, all-`PaysUnlessTerminalState`, mixed role rows, missing void/fallback row, missing void/fallback payout vector, payout matrix dimension mismatch, transposed square matrix rejection, unknown terminal-state IDs, unknown leg IDs, unknown outcome labels, ambiguous attested leg references, mismatched attested payout column order, out-of-bounds payout values, mixed settlement assets, invalid normalized price scale evidence, grouping-proof opacity, attestation hash mismatch, and metadata fingerprint stability.
- [ ] Implement `OutcomeGroup`, `OutcomeLeg`, `TerminalState`, `PayoutMatrix`, `GroupingProof`, `SettlementRules`, `TerminalPayoutDerivation`, and validation helpers.
- [ ] Implement `OutcomeLegRole` and derive standard payout rows from `(OutcomeLegRole, TerminalStateConvention)` only; do not branch on side-label strings in the matrix builder.
- [ ] Enforce one terminal-state source of truth: `OutcomeGroup.terminal_states`.
- [ ] Derive non-standard terminal rows only from explicit `SettlementRules.non_standard_terminal_payouts`; reject inferred void/refund/fallback rows.
- [ ] Resolve attested payout-vector config legs to `OutcomeLegId` through unique native leg id or unique `(outcome_label, side_label)` tuples, then require exact order equality with `PayoutMatrix.cols`.
- [ ] Verify settlement-rule and payout-vector `attestation_sha256` values against canonical serialized payloads that exclude digest fields and use lowercase 64-character SHA-256 hex.
- [ ] Reject mixed settlement assets across legs.
- [ ] Canonically serialize metadata and proof fingerprints with deterministic ordering and normalized Decimal strings.
- [ ] Keep fees out of `OutcomeLeg`; costs must be resolved through `FeeProvider` and existing instrument metadata at scan time.
- [ ] Export the module from `src/lib.rs`.
- [ ] Run allowed local formatting checks and remote Rust verification according to repo policy after commit.

### Task 2: Shared Atomic I/O

**Files:**
- Create `src/bolt_v3_atomic_io.rs`
- Modify `src/bolt_v3_kill_switch_store.rs`
- Modify `src/lib.rs`
- Test `tests/bolt_v3_atomic_io.rs`

- [ ] Write tests for temp-write, fsync, rename, parent-directory sync, permission mode, write failure cleanup, and existing kill-switch store behavior through the shared helper.
- [ ] Promote the current kill-switch atomic-write pattern into a shared `pub(crate)` helper module.
- [ ] Update kill-switch store to use the shared helper before basket store exists.
- [ ] Do not duplicate atomic-write logic in basket storage.

### Task 3: Config-Driven Outcome Sources

**Files:**
- Create `src/bolt_v3_outcome_group_sources.rs`
- Modify `src/bolt_v3_config.rs`
- Modify `src/bolt_v3_validate.rs`
- Create `src/bolt_v3_market_families/outcome_group.rs`
- Modify `src/bolt_v3_market_families/mod.rs`
- Test `tests/config_parsing.rs`

- [ ] Write tests for root-level source parsing, root `outcome_group_sources` unknown-field closure, `risk.basket_execution` parsing, per-strategy file parsing, Polymarket event source parsing, Hyperliquid HIP-4 source parsing, duplicate `source_id` rejection, unbounded query rejection, missing `freshness`, missing settlement rules, missing terminal states, missing non-standard terminal payout vectors, missing `expected_neg_risk_market_id`, unknown client rejection, missing `[reference_data]`, missing `[signal_data]`, missing scanner depth/slippage parameters, unsupported `submit_mode`, no dummy realized-volatility surface requirement for complete-set strategies, and unknown target fields.
- [ ] Implement source config enums and validators.
- [ ] Define closed enums and validators for `settlement_source_kind`, `terminal_state_convention`, `void_policy`, `rounding_policy`, `timing_policy`, refund conventions, source kinds, recovery policy, and target kind.
- [ ] Add `outcome_group_sources` to `BoltV3RootConfig` and add `risk.basket_execution` to the risk config block.
- [ ] Keep all runtime selectors in TOML.
- [ ] Reject scan-all configs unless they include explicit bounded caps and freshness controls.
- [ ] Add an `outcome_group` market-family binding so existing `target.rotating_market_family` dispatch remains the single target-routing path.
- [ ] Define `outcome_group::TargetBlock` with `configured_target_id`, `kind = "static_outcome_group"`, `rotating_market_family = "outcome_group"`, and `group_sources`.
- [ ] Define `outcome_group::RotatingMarketFamily { OutcomeGroup }` inside the new family module and register `outcome_group::KEY` in `src/bolt_v3_market_families/mod.rs` validation bindings.
- [ ] For the existing `MarketFamilyValidationBinding` slots, support `validate_target` and `plan_strategy_target` only. Return typed unsupported errors from Result-returning single-market/updown slots including `target_runtime_fields`, `market_selection_candidate_windows`, and `selected_market_requirement`. Return `None` for Option-returning unsupported capabilities such as single-market selection, fair probability, and maker helpers.
- [ ] Ensure complete-set target validation does not require `TargetRuntimeFields`, `target.gate_subscriptions`, dummy `underlying_asset`, cadence fields, slug tokens, market-selection rules, or realized-volatility target fields. Static outcome groups use `plan_strategy_target` plus source discovery, not the up/down runtime-field contract.
- [ ] Treat `expected_neg_risk_market_id` and `terminal_state_labels` as expectations checked against proof metadata, never as proof.
- [ ] Add a fail-closed source validator for `GateProviderFreshnessBlock` option fields and define evidence fields for local receive time, provider event time, and clock skew.
- [ ] Move global realized-volatility-surface requiredness behind archetype validation; keep the requirement for binary-oracle taker and declare no RV requirement for complete-set arbitrage.

### Task 4: Polymarket OutcomeGroup Normalizer

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
- [ ] Implement one provider-local outcome-group Gamma discovery/cache pipeline for Polymarket sources. The pipeline fetches each configured Gamma event, market slug, event query, or bounded Gamma query once; caches the raw response keyed by native token id, condition id, and market slug; recovers `negRiskMarketID`; and emits the exact NT discovery filters/market slugs from the same cached response.
- [ ] Do not add a second independent Gamma HTTP path for outcome-group metadata. NT may still load instruments from the emitted filters, but the provider-local cache is the sole Bolt metadata source used for `negRiskMarketID` proof and conflict checks.
- [ ] Fail closed when any leg lacks a non-null `negRiskMarketID`, when recovered metadata conflicts with the NT instrument, or when Gamma cache freshness exceeds the configured source freshness.
- [ ] Add provider mapping for TOML-driven NT discovery filters: explicit event slugs, market slugs, event queries, and bounded Gamma queries.
- [ ] Extend Polymarket `SUPPORTED_MARKET_FAMILIES` with `outcome_group::KEY` and add adapter-mapping tests proving outcome-group sources produce the expected NT filters for the configured `client_id`.
- [ ] Avoid per-scope NT `accept()` outcome predicates because NT applies accept predicates globally across filters.

### Task 5: Read-Only Complete-Set Scanner

**Files:**
- Create `src/bolt_v3_complete_set_scanner.rs`
- Test `tests/bolt_v3_complete_set_scanner.rs`

- [ ] Write tests for all-role-true, all-role-false, mixed-role baskets, void/refund rows, insufficient depth, stale book, missing book timestamps, normalized price scale rejection, fee inclusion, minimum fillable depth sizing, Decimal/f64 fee conversion boundary, Decimal/f64 price conversion boundary, and non-positive edge.
- [ ] Implement payout-vector evaluation from `PayoutMatrix`.
- [ ] Reuse `bolt_v3_executable_cost::price_exact_size_vwap` and `bolt_v3_executable_cost::executable_cost_breakdown` for per-leg executable depth and adjusted cost.
- [ ] Implement only the timestamped NT-book-to-`ExecutableBookQuote` adapter and basket aggregation logic around the existing per-leg helper functions.
- [ ] Use existing `FeeProvider` to resolve fee inputs by instrument; do not introduce a parallel fee model.
- [ ] Convert `FeeProvider::fee_bps` Decimal outputs to f64 bps at one explicit boundary, then convert adjusted costs back to Decimal settlement-currency values for basket comparison.
- [ ] Use executable depth, not display or Gamma prices.
- [ ] Return scanner evidence with grouping proof, costs, fees, state payouts, guaranteed payout, min-depth cap, freshness readings, and block reason.

### Task 6: Basket Admission

**Files:**
- Create `src/bolt_v3_basket_admission.rs`
- Modify `src/bolt_v3_decision_evidence.rs`
- Test `tests/bolt_v3_basket_admission.rs`

- [ ] Write tests for basket notional cap, max open basket cap, stale evidence rejection, stale submit re-check rejection, negative edge rejection, missing grouping proof rejection, missing settlement rules rejection, reservation release, and retry-budget rejection.
- [ ] Implement basket-level admission that reserves the whole basket, not individual legs independently.
- [ ] Integrate with `BoltV3SubmitAdmissionState` as monotonic venue-order approval accounting: each submitted basket leg/order consumes one per-client admitted-order slot and is not decremented when the basket closes. Keep this separate from the releasable basket exposure/budget reservation, which releases only on terminal, abort, reject, or stuck transitions.
- [ ] Reuse `bolt_v3_submit_admission` pure arithmetic helpers; do not call stateful single-order `BoltV3SubmitAdmissionState::admit()` once per basket leg.
- [ ] Record evidence keyed by strategy id, basket id, group id, and leg instrument IDs.
- [ ] Persist reservation state before any venue mutation.

### Task 7: Basket Execution State Machine

**Files:**
- Create `src/bolt_v3_basket_execution.rs`
- Create `src/bolt_v3_basket_store.rs`
- Modify `src/bolt_v3_config.rs`
- Modify `src/bolt_v3_kill_switch.rs`
- Test `tests/bolt_v3_basket_execution.rs`

- [ ] Write state-transition tests for complete fill, partial fill, repair allowed, repair denied, repair quantity math, unwind allowed, unwind denied after settlement, cancel rejection, retry-budget exhaustion, restart reconciliation, stuck state, basket-stuck kill-switch trigger, reservation release, and terminal close.
- [ ] Implement state transitions as pure logic first.
- [ ] Implement durable state persistence using `bolt_v3_atomic_io`.
- [ ] Add `risk.basket_execution.state_path`, schema version, max state bytes, and fail-closed recovery policy validation.
- [ ] Keep NT submit/cancel calls outside pure state logic.
- [ ] Require bounded repair/unwind policies from TOML before any live submit path uses them.
- [ ] Add a dedicated basket-execution-stuck kill-switch trigger kind and trip it for `Stuck` baskets that leave unresolved real venue exposure.

### Task 8: Runtime Strategy Registration

**Files:**
- Create `src/bolt_v3_archetypes/complete_set_arbitrage.rs`
- Modify `src/bolt_v3_archetypes/mod.rs`
- Modify `src/strategies/registry.rs` only if a new concrete strategy entry point is required by the existing registry API.
- Modify `src/bolt_v3_strategy_registration.rs` only if the generic registration context needs a shared dependency.
- Test `tests/bolt_v3_strategy_registration.rs`

- [ ] Add `complete_set_arbitrage::KEY`, validation binding, runtime binding, and a `StrategyBuilder` implementation.
- [ ] Add the binding to `RUNTIME_BINDINGS` and `VALIDATION_BINDINGS` in `src/bolt_v3_archetypes/mod.rs`.
- [ ] Define complete-set archetype gate requirements: no realized-volatility surface, no required reference-data roles for the first complete-set taker slice, optional future signal gates only through explicit archetype validation, parameters schema, `submit_mode` variants, scanner depth/slippage validation, and order-template validation for `taker_ioc`.
- [ ] Resolve fee provider, execution venue, `StrategyBuildContext`, and NT `Trader.add_strategy()` handoff in the archetype binding.
- [ ] Add an end-to-end node-binding test proving the strategy is reachable from a root `strategy_files` entry.
- [ ] Keep strategy code limited to signal/intent and local signal state.
- [ ] Route all admission and execution mechanics through shared modules.

### Task 9: Hyperliquid HIP-4 OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`
- Modify `src/bolt_v3_providers/hyperliquid.rs`
- Test `tests/bolt_v3_hyperliquid_outcome_groups.rs`

- [ ] Write tests using synthetic NT HIP-4 `BinaryOption` metadata.
- [ ] Prove that the existing Hyperliquid adapter path remains responsible for discovery and execution.
- [ ] Prove that standalone HIP-4 outcomes without a parent question settlement signal are rejected unless an operator-attested settlement contract supplies terminal-state and void/fallback semantics.
- [ ] Convert HIP-4 outcome metadata into `OutcomeGroup`.
- [ ] Extend Hyperliquid `SUPPORTED_MARKET_FAMILIES` with `outcome_group::KEY` and add adapter-mapping tests proving configured HIP-4 outcome sources produce bounded discovery for the configured `client_id`.
- [ ] Enforce existing Hyperliquid product-surface and live-submit approval gates.

### Task 10: Cross-Venue Normalization

**Files:**
- Create `src/bolt_v3_cross_venue_outcome_groups.rs`
- Test `tests/bolt_v3_cross_venue_outcome_groups.rs`

- [ ] Write tests proving two venue groups can be matched only when their operator-attested settlement contracts are byte-equal on source, terminal-state convention, void policy, rounding policy, and timing policy.
- [ ] Implement semantic group matching with explicit evidence fields and no auto-match path without settlement contracts.
- [ ] Reject groups when cancellation, void, timing, settlement source, terminal states, rounding, or fallback semantics differ.
- [ ] Feed matched groups into the same read-only scanner/comparator, but keep live execution per single-client `OutcomeGroup` until a separate multi-client basket model exists.

## Delivery Order

1. Shared model with leg-role standard payout derivation, grouping proof, settlement rules, operator-attested non-standard terminal payout vectors, settlement-asset validation, label rejection, attestation hash verification, fingerprint canonicalization, and matrix validation.
2. Shared atomic I/O helper promoted from the kill-switch store pattern.
3. Config parsing and validation that adds root `outcome_group_sources`, `risk.basket_execution`, complete-set-specific RV validation, the `outcome_group::TargetBlock`, provider market-family support, freshness rules, and checked expectations.
4. Provider discovery mapping plus Polymarket normalizer with single-path Gamma metadata cache, `negRiskMarketID` grouping proof, and NT filter wiring.
5. Read-only complete-set scanner that reuses `bolt_v3_executable_cost` and `FeeProvider`, with timestamped book snapshots, normalized price scale evidence, and Decimal/f64 conversion boundaries.
6. Basket admission, evidence, monotonic submit-approval cap accounting, freshness re-check, releasable exposure reservation, and durable reservation state.
7. Basket execution state machine with `Stuck`, bounded repair/unwind, restart reconciliation, dedicated basket-stuck kill-switch trigger, and shared-executor ownership.
8. Runtime strategy registration through `src/bolt_v3_archetypes/complete_set_arbitrage.rs`, `src/bolt_v3_archetypes/mod.rs`, and a node-binding proof.
9. HIP-4 normalizer through existing Hyperliquid adapter and settlement-contract gates.
10. Cross-venue read-only matching after operator-attested settlement contracts exist; no multi-client live basket until separately designed.
11. Maker/taker enhancements using the same outcome and order layers.

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
- Non-standard terminal states need operator-attested per-leg payout vectors; a void/refund/fallback row with no derivation rule is not proof.
- Standard terminal states need a leg-role derivation rule; matrix builders may not branch on venue side-label strings.
- Same-venue baskets are one source client and one execution client; cross-venue work is read-only comparison until a multi-client basket model exists.
- The plan is expected to reuse existing Bolt engines: bolt_v3_executable_cost, FeeProvider, bolt_v3_submit_admission arithmetic helpers, GateProviderFreshnessBlock, bolt_v3_market_families, and a shared atomic I/O helper promoted from the kill-switch store pattern.
- Runtime strategy bindings live in src/bolt_v3_archetypes/mod.rs and per-archetype modules; src/bolt_v3_strategy_registration.rs is the generic binding-injected dispatcher.
- Provider `SUPPORTED_MARKET_FAMILIES` and provider discovery mapping must be extended for outcome_group; otherwise adapter validation or instrument discovery fails before runtime.
- Polymarket outcome-group Gamma discovery must use one provider-local cache pipeline that also emits the NT filters; a second independent Gamma metadata path is a dual path.
- Core realized-volatility validation currently leaks up/down assumptions and must be made archetype-conditional.

Review the plan for:
1. Any hidden hardcoding to Polymarket, HIP-4, World Cup, moneyline, YES-only, NO-only, a venue client, a market slug, or a specific strategy.
2. Any place where venue-specific metadata leaks past the normalizer into scanner, admission, execution, or strategy logic.
3. Any unresolved either-or or deferred choice in the Polymarket negRiskMarketID recovery path; the intended path is one provider-local Bolt-owned Gamma metadata cache that also projects NT filters, not a pinned-NT fork or second Gamma fetch path.
4. Any grouping proof that accepts Polymarket event membership without one shared non-null negRiskMarketID, or accepts HIP-4 standalone outcomes without a parent settlement signal or attested settlement contract.
5. Any terminal-state gap: missing standard row derivation from `OutcomeLegRole`, missing void/refund/fallback row, missing non-standard terminal payout vector, duplicate terminal-state source of truth, unrecognized outcome label, or payout-matrix row/column alignment bug.
6. Any config value masquerading as proof rather than a checked expectation against venue metadata or operator attestation.
7. Any cost-engine duplication instead of reusing bolt_v3_executable_cost, FeeProvider, submit-admission arithmetic helpers, GateProviderFreshnessBlock, market-family routing, and shared atomic I/O.
8. Any freshness gap where max_age_ms/max_clock_skew_ms are absent, optional in live trading, use undefined clocks, or are checked only before a non-atomic submit rather than also at Reserved -> Submitting.
9. Any provider support or discovery gap: missing `SUPPORTED_MARKET_FAMILIES`, missing `outcome_group::target_plans`, missing provider-local Gamma cache/filter projection, or a second Gamma fetch path.
10. Any unsafe scan-all, stale-book, fee-unit, slippage, minimum-depth, book-adapter, normalized price scale, or liquidity assumption that could admit a basket that cannot fill atomically enough to preserve the payout floor.
11. Any config-contract mismatch with Bolt's real root strategy_files plus per-strategy strategy_archetype/[target]/[reference_data]/[signal_data]/[parameters] shape, root `outcome_group_sources`, or `risk.basket_execution`.
12. Any outcome_group target-family mismatch: missing TargetBlock shape, missing kind, missing per-family RotatingMarketFamily type, accidental `TargetRuntimeFields` support, wrong unsupported-slot behavior for Result-returning or Option-returning functions, or missing binding-array registration.
13. Any missing runtime registration path, especially failure to add src/bolt_v3_archetypes/complete_set_arbitrage.rs and the src/bolt_v3_archetypes/mod.rs binding-list entries.
14. Any global realized-volatility validation that would force complete_set_arbitrage to define dummy RV surfaces or up/down target fields.
15. Any basket admission/execution gap: monotonic submit-approval cap accounting, separation from releasable exposure reservations, basket state TOML contract, dedicated kill-switch trigger, partial-fill repair math, cancel-reject, repair-recursion, settled-market unwind, restart reconciliation, reservation release, or shared executor ownership.
16. Any cross-venue design mismatch where one live basket spans multiple source/execution clients despite the single-client model.
17. Any attestation-hash gap: digest fields included in the hashed payload, non-canonical ordering, non-lowercase/non-64-character SHA-256 hex, or no mismatch/reorder tests.
18. Any architecture flaw that would prevent turning up Hyperliquid HIP-4 mostly through config once its OutcomeGroup normalizer exists.
19. Any violation of the repo constraints: no hardcodes, no dual paths, no debts, no credential display, pure Rust, SSM-only secrets, and strategy-intent-only boundaries.

Return:
- Blocking findings first, with severity.
- Concrete fixes.
- Remaining risks after fixes.
- A revised implementation order if the current order is unsafe.
```

## Self-Review

- Spec coverage: covers basket arbitrage, cross-venue arbitrage, taker/maker integration path, non-updown Polymarket, and HIP-4 support.
- Review corrections: standard payout derivation, grouping proof, provider support/discovery, root source parsing, provider-local Bolt-owned Gamma cache/filter projection, same-client basket scope, void/refund/fallback terminal states, non-standard payout derivation, digest-excluding settlement-attestation hashes, freshness clocks, cost-engine reuse, shared atomic I/O, durable basket state config, monotonic submit-admission caps separate from releasable exposure reservations, dedicated Stuck kill-switch trigger, config shape, market-family binding shape, unsupported `TargetRuntimeFields`, RV validation ownership, and archetype runtime registration are represented as explicit plan requirements.
- Placeholder scan: no deferred implementation placeholders are used as accepted behavior; each task names files, tests, and implementation scope.
- Type consistency: the shared model names are stable across normalizers, scanner, admission, execution, and review prompt.
