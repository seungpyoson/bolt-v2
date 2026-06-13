# Shared Outcome Group Substrate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a global shared outcome-group substrate for Bolt v3 that normalizes venue outcome markets into common grouping, payout, basket-admission, execution, and evidence primitives; complete-set arbitrage is the first proving strategy, not the substrate owner.

**Architecture:** NautilusTrader owns venue adapters, instrument models, books, order construction, order-list submission, fee calculation, signing, submit/cancel, and fill parsing. Bolt owns shared outcome grouping, payout matrices, basket-level admission, evidence, durable multi-leg execution state, and strategy intent as reusable primitives. Venue-specific Bolt code stops at thin metadata normalizers that translate NT/venue metadata into a shared `OutcomeGroup`; strategy archetypes consume the shared substrate and must not own grouping, settlement, admission, execution, or venue-rule logic.

**Tech Stack:** Rust, NautilusTrader Rust adapters, TOML runtime config, Bolt v3 provider bindings, Bolt v3 admission/evidence/order-intent modules, Polymarket Gamma/CLOB metadata, Hyperliquid HIP-4 `outcomeMeta`.

---

## Non-Negotiable Invariants

- Runtime scope comes from TOML, not code.
- No code embeds World Cup, team names, event slugs, market slugs, token IDs, venue IDs, strategy IDs, or YES-only assumptions.
- Every claimed NT/Bolt capability gap must be backed by pinned-source evidence and an explicit disposition before implementation: reuse existing NT as-is, wrap existing NT in Bolt, surface the field/behavior through an accepted NT dependency path, or add a Bolt-owned shim with a documented reason. No feature may be deferred or reimplemented based only on assumption.
- The NT reuse requirement must be mechanically guarded. Outcome-group code may not become production-reachable unless a local non-compile verifier proves the required NT capability ledger exists, every required capability has a pinned-source disposition, and shared outcome-group modules do not bypass NT primitives where the ledger says `reuse_nt` or `wrap_nt`.
- Outcome grouping, settlement proof, payout matrices, price-scale proof, basket scanning, basket admission, durable multi-leg execution, restart reconciliation, kill-switch integration, and evidence are global shared Bolt primitives. They must not be owned by, named as, or coupled to Polymarket, Hyperliquid, moneyline markets, negative-risk markets, complete-set arbitrage, maker/taker strategies, YES/NO sides, or any other single strategy archetype.
- Strategies produce intent only. Venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, order-list submission, and submit gating remain in shared execution/admission modules or NT.
- NT remains the source of venue mechanics: instruments, books, fees, signing, order submit/cancel, and fill parsing.
- Bolt owns cross-instrument semantics: grouping proof, payout matrix, basket scanner, basket-level admission, partial-fill state, and decision evidence.
- Every `OutcomeGroup` must carry one `GroupingProof`. The proof variant carries the source-native grouping identity directly: Polymarket uses `negRiskMarketID`, HIP-4 uses its structured question/outcome metadata, and sources without a venue-native grouping signal require an operator-attested settlement contract id. Event slug, question text, sports type, labels, or token order are never grouping keys.
- Discovery may be broad only when bounded by config caps such as explicit slugs, max groups, max markets, freshness limits, and notional caps.
- Polymarket event slug, sports market type, and `neg_risk = true` do not prove a complete set. A Polymarket group is admissible only when the normalizer can prove a single non-null `negRiskMarketID`, mutually exclusive legs, exact terminal-state enumeration, and void/refund handling.
- Every `OutcomeGroup` must enumerate every terminal state, including void/refund/fallback states. Missing terminal-state proof or missing terminal-state payout derivation is a hard rejection.
- Config values such as `expected_neg_risk_market_id` and `terminal_state_labels` are checked expectations, not proof. Venue metadata or operator attestation remains the proof source.
- The first slice supports only `terminal_state_convention = "exactly_one_winner"`. Any other convention rejects until its standard-row derivation rule is specified and tested.
- `OutcomeLegRole` models single-terminal-state payoffs. A source leg whose venue metadata implies a union, spread, parlay, double-chance, or other multi-state payoff rejects until the shared model is extended.
- Cross-venue matching is disabled unless each venue source supplies an operator-attested settlement contract and those contracts match exactly on source, timing, void, rounding, and terminal-state semantics.
- Same-venue baskets are the first execution unit: one basket belongs to one `OutcomeGroup`, one `source_client_id`, and one `execution_client_id`. Cross-venue work is read-only group comparison until a later multi-client basket model is explicitly designed.
- Freshness is config-owned and mandatory for live basket scanning and submit. Freshness is checked at scan, admission, and `Reserved -> Submitting`.
- In-flight baskets are durable. Any state transition that can leave real venue exposure must be persisted before the next venue mutation.

## Explicit Task List

1. Shared outcome-group substrate: normalize venue outcome markets into `OutcomeGroup`, prove terminal-state semantics, and expose reusable payout, freshness, price-scale, order-constraint, basket-admission, execution, and evidence primitives.
2. First proving consumer: complete-set basket arbitrage buys a portfolio of outcome legs when the minimum terminal-state payout exceeds executable cost.
3. Cross-venue outcome comparison: compare normalized outcome groups across venues after each venue produces the same shared model and byte-equal settlement contract evidence.
4. Future strategy consumers: maker/taker and other outcome strategies reuse the shared outcome, basket, order/admission, execution, and evidence layers.
5. Non-updown outcome support: expose Polymarket and Hyperliquid HIP-4 through NT-backed, config-driven outcome-group sources.

## ASAP Scope: NT-First World Cup Slice

The first implementation slice prioritizes Polymarket same-client, same-venue outcome-group baskets over configured sources because that is the urgent World Cup path. It must not be taker-only or complete-set-owned at the substrate layer: order behavior is selected by TOML `submit_mode` and mapped onto NT-supported order templates, while complete-set basket arbitrage is only the first consumer that proves the shared substrate. Live Polymarket basket mutation is gated on the Task 0 `settlement_signals` disposition proving a reachable settled-market signal at the pinned NT revision; if that proof fails, Polymarket remains read-only scanner/comparator scope until an accepted NT surfacing path or controlled subscription slice lands.

Use NautilusTrader for every venue mechanic it already offers:

- NT instruments, instrument IDs, books, book subscriptions, order templates, order submit/cancel, fills, order status reports, instrument status events, signing, and venue execution remain the authority.
- Existing Bolt wrappers over NT, including `bolt_v3_executable_cost`, `FeeProvider`, provider adapters, market-family routing, submit admission, kill-switch store, and decision evidence, must be reused rather than duplicated.
- Bolt adds only the missing cross-instrument layer: grouping proof, terminal-state/settlement proof, payout matrix validation, basket aggregation around existing per-leg cost helpers, basket-level admission/reservation, and durable multi-leg state.
- Bolt adds a provider-local Gamma cache only if the evidence audit proves the pinned NT dependency cannot surface `negRiskMarketID` through an acceptable near-term NT/Bolt dependency path. If the field can be added or exposed through NT without violating dependency/source-integrity rules, prefer that over a Bolt-only recovery path.
- Config-owned order-size floors exist only for fields NT cannot supply for Polymarket at the pinned rev. NT still supplies instrument identity, precision/step where available, order construction, submit/cancel, and status/fill truth.

Explicitly deferred until the first slice is live:

- Hyperliquid HIP-4 live execution.
- Cross-venue matching beyond read-only comparison.
- Multi-client baskets or data/execution client splits.
- Alternative terminal-state conventions beyond `exactly_one_winner`.
- New NT APIs, NT forks, or venue-adapter extensions that do not already exist at the pinned NT revision. Existing NT order, quote, book, status, fill, signing, submit/cancel, and provider surfaces are in scope and should be used.

Definitions for the deferred items:

- A multi-client basket is one logical basket whose legs are submitted, reconciled, or risk-accounted through more than one `execution_client_id`, venue account, or provider order stream. Same-client baskets can still contain multiple instruments and multiple legs; they just share one execution client and one reconciliation/kill-switch/admission owner.
- An NT extension is any change to NautilusTrader itself or any dependency on a new NT API that does not exist at the pinned revision, such as adding a new bounded HIP-4 selector, a runtime external-claim registration API, or surfacing an extra Polymarket metadata field on `BinaryOption.info`. Using existing NT APIs is not an extension; it is required.

## Current Evidence

These bullets are orientation only. The machine-checkable Task 0 NT capability ledger is the accepted proof source for implementation decisions.

- Bolt pins NautilusTrader rev `6e059dcbb59ac1e582132fc431a581936c216c3c` in `Cargo.toml`.
- NT Polymarket parses Gamma markets into `BinaryOption` instruments in `crates/adapters/polymarket/src/http/parse.rs`.
- NT `PolymarketOutcome` is a free-form string in `crates/adapters/polymarket/src/common/enums.rs`; it is not limited to Up/Down or Yes/No.
- NT Polymarket filters support market slugs, event slugs, Gamma market queries, event queries, event params, search params, tags, and predicates in `crates/adapters/polymarket/src/filters.rs`.
- NT Gamma market params include `game_id`, `sports_market_types`, `tag_id`, active/closed/archive fields, liquidity/volume filters, and `max_markets` in `crates/adapters/polymarket/src/http/query.rs`.
- NT Polymarket execution handles negative-risk signing and market order book walking in the Polymarket execution modules. At pinned rev `6e059dcb`, `crates/adapters/polymarket/src/signing/eip712.rs` selects the negative-risk exchange contract when `neg_risk = true`, and `crates/adapters/polymarket/src/execution/mod.rs` builds a per-instrument `neg_risk_index` from `BinaryOption.info["neg_risk"]` before order submission.
- NT Polymarket market-resolved close events are not currently reachable through Bolt's allowed config path. At pinned rev `6e059dcb`, Polymarket `MarketResolved` is documented as received only when `subscribe_new_markets` is enabled; the websocket handler sets `custom_feature_enabled` from that global flag for all market subscriptions; the data client calls `subscribe_market(vec![])` when the flag is true; and Bolt currently rejects `subscribe_new_markets = true` because that all-markets subscription violates controlled-connect scope. `subscribe_instrument_status` alone only subscribes to the NT message-bus topic; it does not enable the upstream market-resolved feed. Polymarket live basket execution must therefore stay gated until Task 0 chooses `reject_for_now`, `surface_in_nt`, or a reviewed controlled-subscription `bolt_shim` for this settlement signal.
- NT Hyperliquid parses HIP-4 outcome metadata into `HyperliquidInstrumentDef` through `parse_outcome_instruments`, then converts those definitions into `BinaryOption` instruments in `crates/adapters/hyperliquid/src/http/parse.rs`.
- NT HIP-4 `OutcomeQuestion` uses `question: u32`, `name`, `description`, `fallback_outcome`, `named_outcomes`, and `settled_named_outcomes` in `crates/adapters/hyperliquid/src/http/models.rs`; `build_outcome_info` surfaces `question`, `question_name`, `outcome_index`, `outcome_side`, `named_index`, and `is_fallback` into `BinaryOption.info`.
- NT already has same-venue multi-order submission through `OrderList` and `SubmitOrderList`: `crates/model/src/orders/list.rs` permits multiple instruments on one venue and rejects mixed venues, `crates/trading/src/strategy/mod.rs` exposes `Strategy::submit_order_list`, `crates/risk/src/engine/mod.rs` and `crates/execution/src/engine/mod.rs` route and validate `TradingCommand::SubmitOrderList`, and both Polymarket and Hyperliquid execution adapters implement `submit_order_list` with adapter tests. Outcome-group basket execution must reuse that path when the selected source, venue, and submit mode support it.
- NT already has order-management commands for repair/unwind mechanics: `CancelOrder`, `BatchCancelOrders`, `CancelAllOrders`, and `ModifyOrder` flow through the Rust strategy/order-manager/risk/execution layers. Basket repair and unwind must use existing NT commands where supported, not venue-specific Bolt mutation code.
- NT already has book/depth primitives: `OrderBook`, `BookLevel`, `OrderBookDepth10`, quote ticks, and book deltas in `crates/model/src/orderbook` and `crates/model/src/data`. Outcome-group scanning must consume NT book/depth data and adapt it into the existing Bolt `ExecutableBookQuote` helper; it must not create a second order-book model.
- NT `LiveNodeConfig` has `cache: Option<CacheConfig>`, but Bolt's current `src/bolt_v3_live_node.rs` passes `cache: None`. The basket store must therefore be justified as basket-specific durable state for payout/admission/repair/restart semantics, not as a replacement general order cache. Task 0 must evaluate whether existing NT cache/config surfaces can be enabled or wrapped before adding duplicate order persistence.
- NT external-order claim evidence at pinned rev `6e059dcb`: `crates/trading/src/strategy/config.rs` exposes static `external_order_claims`, `crates/live/src/node.rs` registers those claims during `add_strategy` before the node starts, `crates/live/src/manager.rs` has `claim_external_orders` and routes claimed external reports by instrument id, and `crates/execution/src/engine/mod.rs` has `register_external_order_claims`. Task 11 must prove whether those existing surfaces are sufficient before any new runtime claim-registration API is designed or deferred.
- Bolt currently maps Polymarket data filters only from updown target plans in `src/bolt_v3_providers/polymarket.rs`.
- Bolt Hyperliquid already has a `Hip4Outcomes` product surface, with live submit approval-gated in `src/bolt_v3_providers/hyperliquid.rs`.
- NT Polymarket models parse `negRiskMarketID` on both `GammaMarket` and `GammaEvent`, but `build_info_json` carries only token/condition/market/question/slug metadata, `neg_risk`, fee schedule, and game id onto `BinaryOption.info` at pinned rev `6e059dcb`; Bolt cannot prove Polymarket grouping from NT instruments alone unless the plan first accepts an NT-field-surfacing path.
- Bolt root config loads strategies through `strategy_files`; strategy config is one file per strategy using `strategy_archetype`, raw `[target]`, and raw `[parameters]`.
- Bolt already has reusable primitives this plan must not duplicate: `bolt_v3_executable_cost`, `FeeProvider`, `bolt_v3_submit_admission` arithmetic helpers, `bolt_v3_kill_switch_store` durable-write pattern, `GateProviderFreshnessBlock`, and the `bolt_v3_market_families` target-routing layer.
- Bolt already runs non-compile source-fence verifiers from `just source-fence-static`; outcome-group implementation must add an NT-reuse verifier there rather than relying on review prose alone.
- Bolt strategy runtime bindings live in `src/bolt_v3_archetypes/mod.rs`; `src/bolt_v3_strategy_registration.rs` is the generic binding-injected dispatcher.
- The kill-switch store's atomic-write helpers are private today, so basket persistence needs a shared atomic I/O helper instead of copying the helper body.
- Provider adapter validation rejects unsupported market families; Polymarket currently supports only `updown`, and Hyperliquid currently supports `updown` plus `hyperliquid_instrument`.
- Core startup validation currently requires `realized_volatility_surface_id` for every strategy before archetype-specific validation runs.
- `BoltV3RootConfig` uses `deny_unknown_fields`, so `outcome_group_sources` must be added to the root struct before the example root TOML can parse.

## Review-Driven Corrections

This revision incorporates the blocking architecture review. The implementation is not allowed to proceed until these corrections are represented in tests:

- Outcome-group grouping is keyed by source-specific grouping proof, not event membership or strategy type. For Polymarket, that proof requires a single non-null `negRiskMarketID` plus explicit terminal-state and void/refund proof.
- The plan wraps the existing `bolt_v3_market_families` routing layer with a new outcome-group family binding; it does not replace the family layer.
- The scanner composes existing per-leg executable-cost helpers; it does not reimplement depth walking, fee arithmetic, or slippage arithmetic.
- Fees are read through the existing `FeeProvider` and instrument metadata. `OutcomeLeg` does not carry a parallel fee model.
- Stateful single-order admission is not called once per leg as basket admission. Basket admission uses shared arithmetic helpers and owns one basket-level reservation.
- Monotonic submit-approval slot consumption needs an explicit basket-aware API on `BoltV3SubmitAdmissionState`; per-leg calls to the existing single-order `admit()` remain forbidden for basket admission.
- Basket state is durable and reconciled against NT reports on restart.
- Durable basket state reuses a promoted shared atomic-write helper; no second atomic-store implementation is allowed.
- Polymarket `negRiskMarketID` handling must be decided by source evidence before implementation. First investigate whether to surface `negRiskMarketID` through NT instrument metadata or an accepted NT dependency update. If that is not viable for the first slice, use one provider-local Bolt-owned Gamma metadata cache keyed by native token/condition/market identifiers and used to emit NT filters. Do not allow both paths to be active at runtime.
- Cross-venue matching requires operator-attested settlement contracts and remains fail-closed without them.
- Runtime registration is implemented as a new archetype binding under `src/bolt_v3_archetypes`, plus the binding-list entry in `src/bolt_v3_archetypes/mod.rs`. The generic registration dispatcher is not the concrete-builder home.
- Runtime strategy construction also registers the concrete builder in `src/strategies/mod.rs` through `production_strategy_registry`; the archetype binding list alone is not sufficient for NT `Trader.add_strategy()`. That registration is the runtime-activation point and must happen only after the outcome-group source-integrity key covers the archetype and strategy shell.
- Outcome-group provider support is explicit: every provider that may host outcome-group strategies must add `outcome_group::KEY` to `SUPPORTED_MARKET_FAMILIES` and prove adapter validation accepts it.
- Root `outcome_group_sources` are first-class root config fields and project into provider discovery filters through the outcome-group market-family binding; they are not inert strategy-only metadata.
- Strategy archetype validation owns strategy-specific volatility requirements. Core startup validation must stop globally requiring `realized_volatility_surface_id`; `binary_oracle_edge_taker` keeps its RV requirement, while the first outcome-group consumer `complete_set_arbitrage` declares none.
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
    pub role_binding_proof: RoleBindingProof,
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
    pub price_scale: NormalizedPriceScaleEvidence,
    pub order_constraints: OutcomeLegOrderConstraints,
}

pub enum OutcomeLegRole {
    PaysOnTerminalState(TerminalStateId),
    PaysUnlessTerminalState(TerminalStateId),
}

pub enum NormalizedPriceScaleEvidence {
    BinaryOnePayoutEqualsOneSettlementUnit {
        settlement_asset_id: SettlementAssetId,
        payout_per_contract: Decimal,
        price_units_per_payout: Decimal,
        assertion_source: PriceScaleAssertionSource,
    },
}

pub enum PriceScaleAssertionSource {
    VenueStructuredFields {
        proof_fingerprint: String,
    },
    OperatorAttested {
        attestation_sha256: String,
    },
}

pub struct OutcomeLegOrderConstraints {
    pub min_quantity: Decimal,
    pub min_notional: Option<Decimal>,
    pub quantity_step: Decimal,
    pub constraint_source: OrderConstraintSource,
}

pub enum OrderConstraintSource {
    ConfigFloorWithNtPrecision {
        source_id: String,
    },
    NtInstrumentWithConfigFloor {
        source_id: String,
    },
}

pub struct PayoutMatrix {
    pub cols: Vec<OutcomeLegId>,
    pub payout_per_unit_by_state: BTreeMap<TerminalStateId, Vec<Decimal>>,
}

pub enum GroupingProof {
    PolymarketNegRisk {
        neg_risk_market_id: String,
        discovery_scope: PolymarketDiscoveryScopeEvidence,
        market_slugs: Vec<String>,
        proof_fingerprint: String,
    },
    HyperliquidOutcome {
        question: u32,
        outcome_indices: Vec<u32>,
        proof_fingerprint: String,
    },
    OperatorAttested {
        settlement_contract_id: String,
        attestation_id: String,
        attestation_sha256: String,
        proof_fingerprint: String,
    },
}

pub struct PolymarketDiscoveryScopeEvidence {
    pub source_id: String,
    pub event_slugs: Vec<String>,
    pub market_slugs: Vec<String>,
    pub gamma_query_fingerprint: Option<String>,
    pub cache_key_fingerprint: String,
}

pub enum RoleBindingProof {
    OperatorAttestedPositiveSide {
        attestation_sha256: String,
        bindings: BTreeMap<TerminalStateId, PositiveSideBinding>,
    },
    VenueStructuredFields {
        proof_fingerprint: String,
    },
}

pub struct PositiveSideBinding {
    pub terminal_state_id: TerminalStateId,
    pub pays_on_terminal_state_native_leg_id: String,
    pub pays_unless_terminal_state_native_leg_id: String,
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
        terminal_state_id: TerminalStateId,
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
- Validation rejects any `terminal_state_convention` other than `exactly_one_winner` in the first slice and rejects any leg that cannot be represented by exactly one `PaysOnTerminalState(T)` or `PaysUnlessTerminalState(T)` role.
- Void/refund/fallback rows must come from `SettlementRules.non_standard_terminal_payouts`, not from inferred labels. Each operator-attested vector must name the same columns as the payout matrix, match leg count exactly, declare its refund convention, use bounded settlement-currency values, and be covered by the settlement attestation hash.
- Operator-attested vectors resolve config leg references to `OutcomeLegId` using a unique native leg id or a unique `(outcome_label, side_label)` tuple. Validation requires order equality with `PayoutMatrix.cols`, not just set equality or count equality, and rejects ambiguous or duplicate label resolution.
- Operator-attested positive-side role bindings resolve config `terminal_state_label` values to `TerminalStateId` using the same checked terminal-state mapping as payout vectors. Validation requires one binding per standard terminal state, rejects missing/duplicate/unmapped labels, and rejects any `PositiveSideBinding` whose embedded `terminal_state_id` differs from the surrounding `BTreeMap` key.
- Attestation hashes are verified at validation time by hashing the canonical attestation payload after removing every digest field, including `attestation_sha256` and any nested digest fields. Validation requires lowercase 64-character SHA-256 hex and rejects mismatches, reordered columns, re-keyed terminal-state entries, or payloads whose digest changes only by adding or changing digest fields.
- Every attested terminal payout vector and every attested positive-side binding includes the governed terminal-state id inside the hashed payload. The map key and embedded `terminal_state_id` must be equal before hashing. Re-keying a valid vector or role binding under a different terminal state is a hard rejection.
- Attestation payload canonicalization is a dedicated deterministic byte representation implemented by one shared helper used for role-binding attestations, payout-vector attestations, `metadata_fingerprint`, and `proof_fingerprint`. The helper emits UTF-8 bytes with a fixed version prefix and length-prefixed binary frames for every field path and value, e.g. `u32_be(path_len) || path_bytes || u32_be(value_len) || value_bytes`; no delimiter-only `path=value\n` encoding is allowed. Field paths are sorted, enum variants are lowercase snake_case, list indices are zero-based, map keys are sorted and included as explicit length-prefixed fields, and `OutcomeLegId` columns are resolved before hashing. Decimals are parsed as `rust_decimal::Decimal`, normalized, and rendered as plain base-10 strings with no scientific notation, no plus sign, no negative zero, and no insignificant trailing zeroes. Operator-provided id, label, slug, and native-leg fields are also charset-bounded at config load to printable non-control UTF-8 before hashing. Use the existing `is_lowercase_sha256` helper for shape validation; do not use pretty JSON hashing for attestation payloads.
- Square matrix dimensions are not enough. Validation must reject transposed, duplicate, missing, or unknown row/column mappings.
- Outcome labels are metadata only after validation. Unknown labels, duplicate labels that map to different states, or labels that cannot be mapped to an attested terminal state reject the group.
- Every `OutcomeGroup` must have one settlement asset. Every leg's `settlement_asset_id` must equal `OutcomeGroup.settlement_asset_id`; mixed-currency groups reject before scanning.
- Cost units are Decimal settlement-currency notionals. Any existing f64 cent helpers must be converted at one explicit boundary with tests for edge-threshold stability.
- Existing cent-based executable-cost helpers assume normalized binary prices where `1.0` payout equals 100 cents. Normalizers must provide `NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit` with numeric scale fields and a structured-field or operator-attested assertion source; NT metadata alone is not treated as proof if it lacks a payout field. Instruments without a price-range, settlement-asset, and payout-scale assertion reject.
- NT Polymarket instruments intentionally leave `min_quantity` and `min_notional` unset, so live outcome-group sources must supply config-owned per-leg `min_quantity` floors and optional `min_notional` floors. Quantity step and precision come from NT instruments; the effective floor is the stricter of the config floor and any NT floor if a venue later supplies one.
- `GroupingProof`, `RoleBindingProof`, `OutcomeGroupSourceKind`/`source_kind`, `SettlementSourceKind`/`settlement_source_kind`, `NormalizedPriceScaleEvidence`, `PriceScaleAssertionSource`, and `OrderConstraintSource` are opaque outside normalizers and evidence serialization. Scanner, admission, execution, and strategy modules may check that proof exists and may include fingerprints in evidence, but must not branch on those venue-discriminating variants or fields.
- Venue-native grouping identifiers remain in the relevant `GroupingProof` variant rather than a Bolt-global key wrapper. Polymarket event slugs, sports market types, `neg_risk = true`, question text, and slug patterns are discovery evidence only; `GroupingProof::PolymarketNegRisk.neg_risk_market_id` must be one shared non-null `negRiskMarketID`. HIP-4 grouping must use structured NT outcome metadata such as `question`, `outcome_index`, `outcome_side`, and named/fallback indices, not label text. Event membership must not be a required grouping key, and market-slug-only or bounded-Gamma-query-only sources must be representable with empty `event_slugs`.
- `metadata_fingerprint` and `proof_fingerprint` use the same canonical byte helper as attestation payloads. `metadata_fingerprint` covers venue-identity and grouping metadata only; it excludes operator policy such as `order_constraints`, freshness windows, notional caps, and submit-mode settings so changing risk policy does not create a different market identity.

## Files And Responsibilities

- Create `src/bolt_v3_outcome_groups.rs`: shared model, validation, metadata fingerprinting, payout matrix helpers.
- Create `src/bolt_v3_outcome_group_sources.rs`: TOML config model for enabled outcome-group sources and bounded discovery rules.
- Create `src/bolt_v3_atomic_io.rs`: shared temp-write, fsync, rename, and parent-directory sync helpers promoted from the kill-switch store pattern.
- Create `src/bolt_v3_market_families/outcome_group.rs`: market-family binding that lets the existing target-routing layer dispatch outcome-group strategy consumers without duplicating `MarketIdentityPlan`.
- Modify `src/source_canonicalization.rs` and `src/bolt_v3_source_integrity.rs` only after every first-slice outcome-group source root exists: add a new `OUTCOME_GROUP_KEY` covering outcome-group model/scanner/admission/execution modules, the first consumer archetype source, and the concrete strategy shell source; refresh `GOLDEN_OUTCOME_GROUP_DIGEST` after the accepted source change. Keep `SUBMIT_ADMISSION_KEY` separate and refresh `GOLDEN_SUBMIT_ADMISSION_DIGEST` in the task that edits `src/bolt_v3_submit_admission.rs`. Deferred roots such as the HIP-4 normalizer join the key in their own creating commit.
- Create `src/bolt_v3_outcome_group_polymarket.rs`: Polymarket Gamma/NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_outcome_group_hyperliquid.rs` later in the deferred HIP-4 task: Hyperliquid HIP-4 NT metadata normalizer into `OutcomeGroup`.
- Create `src/bolt_v3_outcome_group_scanner.rs`: shared payout-vector scanner that consumes NT `OrderBook`/`OrderBookDepth10`/`BookLevel` data through a thin adapter into `bolt_v3_executable_cost` for per-leg executable costs. Complete-set arbitrage is one caller, not the module owner.
- Create `src/bolt_v3_basket_admission.rs`: basket-level admission, risk caps, freshness checks, monotonic submit-approval cap integration, releasable exposure reservation, and evidence payloads using existing submit-admission arithmetic helpers.
- Modify `src/bolt_v3_submit_admission.rs`: add a basket-aware monotonic submit-slot consumption API that reuses the same kill-switch, notional-cap, and per-client count-cap checks without calling single-order `admit()` once per leg.
- Create `src/bolt_v3_basket_execution.rs`: shared durable multi-leg executor outside `src/strategies/*`; it owns fill/cancel-driven `Partial`/`Repair`/`Unwind`/`Stuck` state transitions and submits same-venue baskets through NT `OrderList`/`SubmitOrderList` when the audited adapter and submit mode support that path.
- Create `src/bolt_v3_basket_store.rs`: basket-state persistence using `bolt_v3_atomic_io`, not copied kill-switch private helpers.
- Modify `src/bolt_v3_config.rs`: add `outcome_group_sources` to `BoltV3RootConfig`, add `risk.basket_execution`, and keep strategy-specific settings in each per-strategy file's raw `[target]` and `[parameters]`.
- Modify `src/bolt_v3_validate.rs`: move global realized-volatility surface requirement behind archetype requirements so outcome-group strategy consumers do not need dummy RV surfaces unless their archetype declares that requirement.
- Modify `src/bolt_v3_providers/polymarket.rs`: add `outcome_group::KEY` to `SUPPORTED_MARKET_FAMILIES` and project enabled root outcome-group sources into NT discovery filters.
- Modify `src/bolt_v3_providers/hyperliquid.rs` later in the deferred HIP-4 task: add `outcome_group::KEY` to `SUPPORTED_MARKET_FAMILIES` and expose HIP-4 outcome group source wiring through existing Hyperliquid adapter and approval gates.
- Modify `src/bolt_v3_kill_switch.rs`: add a basket-stuck halt trigger kind and constructor.
- Create `src/bolt_v3_archetypes/complete_set_arbitrage.rs`: first consumer validation/runtime builder source, fee-provider resolution, execution-venue lookup, `StrategyBuildContext`, and NT `Trader.add_strategy()` handoff code. Do not wire this file into binding arrays until the runtime-activation task after source-integrity registration.
- Create `src/strategies/complete_set_arbitrage/mod.rs`: thin NT strategy shell that emits basket intent and forwards NT events to shared basket execution.
- Modify `src/bolt_v3_archetypes/mod.rs` in the runtime-activation task after source-integrity registration: add the first consumer's complete-set validation and runtime bindings.
- Modify `src/strategies/mod.rs` in the runtime-activation task after source-integrity registration: register `CompleteSetArbitrageBuilder` in `production_strategy_registry`.
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
max_metadata_age_ms = 7200000

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

[outcome_group_sources.order_constraints]
default_min_quantity = "operator_supplied_min_quantity"
default_min_notional = "operator_supplied_min_notional"
per_leg = [
  { native_leg_id = "operator_positive_token_a", min_quantity = "operator_supplied_min_quantity", min_notional = "operator_supplied_min_notional" },
  { native_leg_id = "operator_inverse_token_a", min_quantity = "operator_supplied_min_quantity", min_notional = "operator_supplied_min_notional" },
  { native_leg_id = "operator_positive_token_b", min_quantity = "operator_supplied_min_quantity", min_notional = "operator_supplied_min_notional" },
  { native_leg_id = "operator_inverse_token_b", min_quantity = "operator_supplied_min_quantity", min_notional = "operator_supplied_min_notional" },
  { native_leg_id = "operator_positive_token_c", min_quantity = "operator_supplied_min_quantity", min_notional = "operator_supplied_min_notional" },
  { native_leg_id = "operator_inverse_token_c", min_quantity = "operator_supplied_min_quantity", min_notional = "operator_supplied_min_notional" },
]

[outcome_group_sources.role_bindings]
kind = "operator_attested_positive_side"
attestation_sha256 = "operator_supplied_lowercase_sha256"
legs = [
  { terminal_state_label = "operator_state_a", pays_on_terminal_state_native_leg_id = "operator_positive_token_a", pays_unless_terminal_state_native_leg_id = "operator_inverse_token_a" },
  { terminal_state_label = "operator_state_b", pays_on_terminal_state_native_leg_id = "operator_positive_token_b", pays_unless_terminal_state_native_leg_id = "operator_inverse_token_b" },
  { terminal_state_label = "operator_state_c", pays_on_terminal_state_native_leg_id = "operator_positive_token_c", pays_unless_terminal_state_native_leg_id = "operator_inverse_token_c" },
]

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
  { outcome_label = "operator_state_a", side_label = "operator_no_side_label", payout_per_unit = "1" },
  { outcome_label = "operator_state_b", side_label = "operator_yes_side_label", payout_per_unit = "1" },
  { outcome_label = "operator_state_b", side_label = "operator_no_side_label", payout_per_unit = "1" },
  { outcome_label = "operator_state_c", side_label = "operator_yes_side_label", payout_per_unit = "1" },
  { outcome_label = "operator_state_c", side_label = "operator_no_side_label", payout_per_unit = "1" },
]
attestation_sha256 = "operator_supplied_lowercase_sha256"

[[outcome_group_sources]]
source_id = "hyperliquid_hip4_outcomes"
client_id = "hyperliquid_main"
kind = "hyperliquid_hip4"
question = "operator_supplied_question_number"
terminal_state_labels = ["operator_state_a", "operator_state_b", "operator_state_c"]
max_groups = 20
enabled = false

[outcome_group_sources.freshness]
max_age_ms = 500
max_clock_skew_ms = 250

[outcome_group_sources.order_constraints]
default_min_quantity = "operator_supplied_min_quantity"
default_min_notional = "operator_supplied_min_notional"

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
submit_mode = "ioc"
vwap_depth_limit_bps = 2000
slippage_buffer_bps = 100
max_repair_attempts = 1
max_unwind_attempts = 1
```

Validation rules:

- `BoltV3RootConfig` must declare optional/default `outcome_group_sources` and optional `risk.basket_execution` fields; unknown-field denial must still reject misspelled blocks.
- `outcome_group_sources` defaults to an empty list for backward compatibility, and `risk.basket_execution` is `Option`/default absent. They become required only when a loaded strategy uses the outcome-group substrate for live basket scanning or execution. The first such consumer is `strategy_archetype = "complete_set_arbitrage"`.
- When present for live outcome-group basket execution, `risk.basket_execution` must declare `state_path`, `schema_version`, `max_state_file_bytes`, `recovery_policy`, recovery age limits, and a positive `max_metadata_age_ms`. Recovery is fail-closed: restart reconciles Bolt basket store state against NT order/fill/position reports before admitting any new basket.
- Cross-config validation lives in `src/bolt_v3_validate.rs` after root and strategy files are loaded. It iterates strategies that consume the outcome-group substrate and checks the root `outcome_group_sources`, `risk.basket_execution`, `persistence.decision_evidence`, provider clients, and market-identity target plans together; neither the root parser nor the family binding alone owns these cross-file checks.
- `source_id` must be unique.
- `client_id` must reference a configured client.
- Query-style sources must include bounded selectors and caps.
- Event-style sources must include explicit event slugs or a bounded event query.
- Provider mappings must extend `SUPPORTED_MARKET_FAMILIES` for every execution client allowed to host outcome-group strategies.
- For the first live slice, every `target.group_sources[]` entry used by an outcome-group basket strategy must resolve to an enabled root `outcome_group_sources` block whose `client_id` equals that strategy's `execution_client_id`. `OutcomeGroup.source_client_id` must also equal the execution client before scan, admission, and execution. Data/execution client splits require a later explicit paired-client design.
- Outcome-group provider mappings must project enabled `group_sources` into discovery per `client_id` by using `outcome_group::target_plans(plan)` and resolving `target.group_sources[]` against root `outcome_group_sources[]` by `source_id`, `enabled`, and `client_id`; emitting every root source for a client is forbidden. Polymarket uses provider-local Gamma cache results to emit NT event/market filters. Hyperliquid HIP-4 currently remains surface-wide through the existing `Hip4Outcomes` product surface; Bolt filters loaded HIP-4 outcomes by the configured NT `OutcomeQuestion.question` value and settlement attestation after NT discovery.
- Polymarket sources must use `expected_neg_risk_market_id` as a checked expectation. The normalizer still proves the same non-null `negRiskMarketID` from Gamma metadata for every admitted leg; event slugs scope discovery only.
- Sources must declare terminal-state labels and settlement rules, including void/refund policy and per-leg non-standard terminal payout vectors. Each standard venue outcome must bind to exactly one configured terminal-state label; missing, duplicate, extra, or unmapped standard outcomes reject.
- Role-binding requirements are source-kind-specific: `polymarket_gamma_*` sources require `[role_bindings] kind = "operator_attested_positive_side"` with attested native-leg bindings, while `hyperliquid_hip4` sources require `RoleBindingProof::VenueStructuredFields` from structured outcome fields and must reject operator label/order heuristics unless a separate operator-attested settlement contract is explicitly configured.
- Enabled live sources must declare `[order_constraints]` with a positive config-owned `default_min_quantity` or positive per-leg `min_quantity` for every native leg; `min_notional` is required-positive for any `submit_mode` that uses NT market/IOC-style execution or any venue path with a notional floor, and optional-positive for modes that prove no notional minimum applies. Polymarket cannot rely on NT instrument minimums because pinned NT leaves them unset; scanner/admission reject below the config floor before reservation.
- Sources must declare `freshness.max_age_ms` and `freshness.max_clock_skew_ms` using the existing `GateProviderFreshnessBlock` semantics. The source validator treats absent or zero option fields as fatal for live outcome-group sources.
- `freshness.max_age_ms` is for live book/quote freshness only. Outcome-group metadata cache expiry must use a separate metadata-refresh source: for Polymarket and HIP-4 this is the configured client data `update_instruments_interval_mins`, converted with checked minute-to-millisecond arithmetic to a positive millisecond TTL and capped by `risk.basket_execution.max_metadata_age_ms`. Validation rejects a max metadata age lower than the smallest configured metadata refresh interval because that would make the source permanently fail closed.
- Freshness compares the local node clock against the latest book/metadata receive timestamp; `max_clock_skew_ms` compares venue/provider event time to local receive time when the provider supplies an event time, and otherwise the source must mark the event-time clock unavailable in evidence and fail any rule that requires venue-time proof.
- HIP-4 sources must use a Hyperliquid client with `Hip4Outcomes` enabled and required live-submit approvals before live trading. A HIP-4 source has exactly one configured NT `OutcomeQuestion.question` value per settlement-rules block; multiple questions require multiple source blocks so settlement, fallback, and attestation cannot be mis-bound.
- Complete-set strategies must reference at least one enabled source.
- Complete-set strategies require positive edge, positive notional cap, max open basket cap, `vwap_depth_limit_bps`, `slippage_buffer_bps`, bounded repair/unwind attempts, and root `persistence.decision_evidence.order_intents_relative_path` configured.
- Complete-set archetype validation must enumerate `submit_mode` variants and map each supported mode to an NT order-template/admission contract. Do not reject maker/post-only modes categorically; reject only a mode whose NT order template, fillability proof, min-size/min-notional requirements, and admission semantics are not specified.
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
edge_bps = (edge / candidate_cost) * 10_000 when candidate_cost > 0
admit when edge_bps > min_edge_bps and every grouping/settlement/risk/freshness check passes
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
- Scanner and admission must validate every proposed leg quantity against the config-owned `OutcomeLegOrderConstraints.min_quantity`, optional config-owned `min_notional`, NT instrument quantity step/precision, and venue order-template constraints. If any leg is below its configured floor or cannot be represented exactly by the allowed quantity precision, reject the whole basket before reservation or submit.
- `candidate_cost <= 0`, non-finite converted cost, or any failed Decimal/f64 conversion is a hard rejection before `edge_bps` normalization.
- Submit re-checks the same depth/freshness constraints immediately before venue mutation.
- The scanner owns new basket aggregation only: NT `OrderBook`/`OrderBookDepth10`/`BookLevel` data is adapted into a timestamped `ExecutableBookQuote` view, per-leg VWAP and adjusted cost come from `bolt_v3_executable_cost`, `FeeProvider::fee_bps` Decimal values convert to f64 bps at one tested boundary, and final basket edge compares Decimal settlement-currency values. Do not create a parallel Bolt order-book model.
- Each basket book snapshot carries instrument id, local receive time, optional provider event time, and normalized binary price scale evidence. Freshness checks use those timestamps at scan, admission, and `Reserved -> Submitting`.
- Scanner and admission evidence must carry both absolute settlement-currency edge and normalized `edge_bps`; `min_edge_bps` is compared only to normalized `edge_bps`, never to the absolute Decimal edge.

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
- `Stuck`: cancel rejected, repair/unwind retry budget exhausted, market settled before exposure neutralized, or restart reconciliation finds unresolved exposure. This state blocks `Closed`, holds reservation, and trips the dedicated basket-stuck kill-switch path.
- `Closed`: final accounting and evidence written after every order, cancel, reservation, and position effect is reconciled.

State machine rules:

- `src/bolt_v3_basket_execution.rs` is the runtime owner for fill/cancel/reconcile/settlement-driven state transitions. The complete-set strategy shell may emit basket intent and strategy-local signal state only and must forward NT order, fill, cancel, instrument-status, and settlement events to the shared executor rather than owning submit mechanics.
- Every in-flight-money transition persists before the next venue action.
- Abort, reject, cancel, completion, repair, unwind, and restart reconciliation release or retain reservation explicitly.
- Cancel rejection, stale submit re-check, settled market, and retry exhaustion are distinct transitions with evidence. Polymarket settled-market detection is driven by NT instrument-status close/market-resolved events, and HIP-4 settled-market detection is driven by NT synthetic settlement fills classified separately from strategy fills; unwind is forbidden once the durable settled flag is true.
- Restart loads durable basket state and reconciles it against NT order/fill/position reports before admitting new baskets. The reconciliation query is instrument-scoped and includes strategy-owned, unclaimed, and engine-classified external venue reports for the basket's instrument IDs; strategy-id-only reconciliation is insufficient. The durable basket schema persists per-leg `client_order_id` and `venue_order_id` once known. The restart join is by client order id first, then by direct venue order id equality against the durable per-leg venue order id; do not rely on NT's process-local venue-to-client cache after restart, and do not heuristically join by instrument/side/quantity alone. The Bolt basket store records intended basket shape and last known transition; NT order/fill/position reports are authoritative for venue effects during reconciliation, and unjoinable orphan venue legs force `Stuck` before new admission.
- `Stuck` trips a dedicated basket-execution kill-switch trigger, not a loss-governor breach trigger.
- Repair uses the current fill vector, payout matrix, fresh books, and configured retry budget to restore the admitted payout floor; unwind uses the same inputs to reduce residual directional exposure when repair is not admissible. Both paths re-run admission/freshness checks before submit.

## Implementation Tasks

### Task 0: NT Capability Evidence Audit

**Files:**
- Create `docs/superpowers/plans/2026-06-13-outcome-group-nt-evidence.md`
- Create `scripts/verify_outcome_group_nt_reuse.py`
- Create `scripts/test_verify_outcome_group_nt_reuse.py`
- Modify `justfile`
- Modify this plan when the audit changes a design decision.

Audit result: `docs/superpowers/plans/2026-06-13-outcome-group-nt-evidence.md` is the accepted machine-checkable Task 0 ledger. It selects `surface_in_nt` for `neg_risk_market_id`, `bolt_shim` for Bolt-owned source grouping proof, and `reject_for_now` for Polymarket live settlement signals, so live Polymarket basket mutation stays disabled until a later reviewed surfacing or controlled-subscription slice changes that disposition.

- [x] For every place this plan claims NT lacks a field, selector, status event, settlement signal, order-template support, precision/min-size source, claim-registration path, or reconciliation surface, add a pinned-source anchor and disposition to the evidence doc.
- [x] Define the evidence doc as a machine-checkable NT capability ledger. At minimum it must contain entries for `neg_risk_market_id`, `source_grouping_proof`, `submit_order_list`, `order_book_depth`, `order_management_cancel_modify`, `nt_cache_reconciliation`, `external_order_claims`, `settlement_signals`, `hip4_discovery`, `min_size_precision`, and `provider_discovery`. Each entry must include pinned NT/Bolt source anchors, disposition (`reuse_nt`, `wrap_nt`, `surface_in_nt`, `bolt_shim`, or `reject_for_now`), owner module, and required tests.
- [x] For `negRiskMarketID`, trace pinned NT Polymarket Gamma models, `build_info_json`, `BinaryOption.info`, provider filter construction, execution `neg_risk_index`, and signing. Decide explicitly between: surface `negRiskMarketID` through NT metadata/dependency update, use exactly one Bolt provider-local cache, or reject the source. Do not implement the provider-local cache until this decision is recorded.
- [x] For every supported source kind, identify the `GroupingProof` variant and native identity fields. Polymarket must use `GroupingProof::PolymarketNegRisk { neg_risk_market_id, ... }`; HIP-4 must use `GroupingProof::HyperliquidOutcome` from structured NT `BinaryOption.info` outcome/question fields; sources without a venue-native grouping signal need `GroupingProof::OperatorAttested { settlement_contract_id, ... }` or must reject.
- [x] For submit modes, enumerate existing NT order-template support and define the Bolt admission/fillability contract per mode before accepting the mode in TOML.
- [x] For basket submission, trace NT `OrderList`, `Strategy::submit_order_list`, `TradingCommand::SubmitOrderList`, risk-engine routing, execution-engine routing, and adapter `submit_order_list` support for every first-slice venue and submit mode. Classify same-venue basket submission as `reuse_nt` when NT supports it. Any fallback to per-order submission must be a reviewed exception with a source-anchored reason and tests proving it does not bypass basket admission, kill-switch, reconciliation, or evidence.
- [x] For repair/unwind order management, trace NT `CancelOrder`, `BatchCancelOrders`, `CancelAllOrders`, and `ModifyOrder` support plus adapter behavior. Classify each basket repair/unwind mutation as `reuse_nt` or reject it; do not implement venue-specific Bolt cancel/modify paths.
- [x] For scanner depth data, trace NT `OrderBook`, `OrderBookDepth10`, `BookLevel`, quote ticks, and existing Bolt `ExecutableBookQuote` helpers. Classify the scanner as `wrap_nt`: NT owns book/depth state, Bolt owns only the conversion into executable-cost inputs plus basket aggregation.
- [x] For restart and durable state, trace NT `CacheConfig`, `OrderList` cache behavior, Bolt's current `LiveNodeConfig { cache: None, ... }`, reconciliation reports, and persisted order identifiers. Decide exactly what NT cache/state surfaces are reused or enabled, and constrain `bolt_v3_basket_store` to basket-specific proof/admission/execution state that NT does not model.
- [x] For external order/settlement visibility, trace static strategy `external_order_claims`, `LiveNode::add_strategy`, `ExecManager::claim_external_orders`, `ExecutionEngine::register_external_order_claims`, and report routing before deciding whether a runtime claim API is required.
- [x] For Polymarket settlement signals, trace `MarketResolved`, `InstrumentStatus::Close`, `subscribe_new_markets`, websocket `custom_feature_enabled`, `subscribe_market(vec![])`, Bolt's `subscribe_new_markets = true` rejection, and `subscribe_instrument_status`. The ledger must classify live Polymarket settlement as `reject_for_now`, `surface_in_nt`, or `bolt_shim`; `reuse_nt` is allowed only if the evidence proves a reachable close signal without violating Bolt's controlled-connect invariant.
- [x] For HIP-4, trace existing NT `Hip4Outcomes` discovery, outcome metadata parsing, synthetic settlement fills, and claim visibility before deciding live execution constraints.
- [x] For every audit item, classify the result as `reuse_nt`, `wrap_nt`, `surface_in_nt`, `bolt_shim`, or `reject_for_now`, with the concrete reason and tests required by the later task.
- [x] Implement `scripts/verify_outcome_group_nt_reuse.py` and wire it into `just source-fence-static`. The verifier must fail when the ledger is missing a required capability, a capability lacks source anchors or a disposition, a `bolt_shim` entry lacks a documented reason and test, or a source file covered by `OUTCOME_GROUP_KEY` violates the ledger's `reuse_nt`/`wrap_nt` decision.
- [x] The verifier scans every existing first-slice outcome-group path and fails on violations. Missing future paths are allowed only until their creating task; after `OUTCOME_GROUP_KEY` exists, exact-membership/source-integrity tests must fail on any missing, extra, or unscanned covered root.
- [x] The verifier must include static checks for the first-slice source files using the repo's existing comment-stripped production text helpers, not raw substring scans over comments or tests. Scanner code must reference NT book/depth primitives and `ExecutableBookQuote`; basket execution code must reference NT `OrderList`/`SubmitOrderList` for same-venue submit paths; repair/unwind mutation code must route through NT cancel/modify command types where supported; basket store code must not introduce a general order-cache/history model; shared scanner/admission/execution/strategy code must not import direct venue adapter execution clients, direct REST/WebSocket venue clients, or provider-local Gamma HTTP clients outside approved provider/normalizer files.
- [x] Presence checks must be paired with anti-pattern checks. A source file that mentions an NT primitive only in a comment, dead import, or test fixture while hand-rolling the equivalent behavior in production code must fail.
- [x] Add positive-control tests for the verifier: missing ledger entry, missing source anchor, undocumented `bolt_shim`, comment-only `SubmitOrderList` mention plus per-leg submit loop, custom scanner order-book struct, direct per-leg venue submit path, direct venue cancel path, and general order-cache duplication must each fail. A fixture that wraps NT book data into `ExecutableBookQuote`, uses `SubmitOrderList`, and persists only basket-specific ids/state must pass.

### Task 1: Shared Outcome Model

**Files:**
- Create `src/bolt_v3_outcome_groups.rs`
- Modify `src/lib.rs`
- Test `tests/bolt_v3_outcome_groups.rs`

Completion evidence: local `just fmt-check`, `git diff --check`, and `just source-fence-static` passed; exact-head remote verification passed for PR #703 at `04ef047390cc2fef61d91967d5a8bce85cd2f239`.

- [x] Write tests for valid groups, missing grouping proof rejection, duplicate source-native grouping identity with conflicting settlement rejection, duplicate leg IDs, empty terminal states, standard winner-row derivation, all-`PaysOnTerminalState`, all-`PaysUnlessTerminalState`, mixed role rows, unsupported terminal-state convention rejection, multi-state leg-role rejection, missing role-binding proof, missing void/fallback row, missing void/fallback payout vector, payout matrix dimension mismatch, non-standard vector length and column-order equality, transposed square matrix rejection, unknown terminal-state IDs, unknown leg IDs, unknown outcome labels, ambiguous attested leg references, mismatched attested payout column order, re-keyed attested vector rejection, re-keyed attested positive-side binding rejection, separator-in-string canonicalization non-collision, one-item-with-separator-vs-two-item canonicalization non-collision, out-of-bounds payout values, mixed settlement assets, invalid normalized price scale evidence, invalid order-constraint floor, grouping/role/source/settlement/price-scale-proof opacity, attestation hash mismatch, canonical byte fixture stability, attestation canonicalization stability, shared fingerprint helper stability, metadata fingerprint stability, and metadata fingerprint exclusion for operator policy fields.
- [x] Implement `OutcomeGroup`, `OutcomeLeg`, `TerminalState`, `PayoutMatrix`, `GroupingProof`, `RoleBindingProof`, `PositiveSideBinding`, `SettlementRules`, `TerminalPayoutDerivation`, `NormalizedPriceScaleEvidence`, `PriceScaleAssertionSource`, `OutcomeLegOrderConstraints`, `OrderConstraintSource`, and validation helpers.
- [x] Implement `OutcomeLegRole` and derive standard payout rows from `(OutcomeLegRole, TerminalStateConvention)` only; do not branch on side-label strings in the matrix builder.
- [x] Reject every `TerminalStateConvention` except `exactly_one_winner` until another convention has a documented role-to-row rule.
- [x] Enforce one terminal-state source of truth: `OutcomeGroup.terminal_states`.
- [x] Derive non-standard terminal rows only from explicit `SettlementRules.non_standard_terminal_payouts`; reject inferred void/refund/fallback rows.
- [x] Resolve attested payout-vector config legs to `OutcomeLegId` through unique native leg id or unique `(outcome_label, side_label)` tuples, then require exact order equality with `PayoutMatrix.cols`.
- [x] Resolve attested positive-side role-binding config entries through configured terminal-state labels to `TerminalStateId`; require exactly one binding per standard terminal state and reject missing, duplicate, re-keyed, or unmapped bindings.
- [x] Verify settlement-rule, role-binding, and payout-vector `attestation_sha256` values against canonical serialized payloads that exclude digest fields, include the governed terminal-state IDs, normalize Decimal strings, use resolved leg IDs, and use lowercase 64-character SHA-256 hex.
- [x] Implement one canonical attestation/fingerprint byte helper with fixed version prefix, sorted field-path records, length-prefixed field paths and values, explicit map keys, resolved IDs, and Decimal normalization; use it for attestations, `metadata_fingerprint`, and `proof_fingerprint`. Add tests proving embedded `\n`, `=`, and path separators in free-form fields cannot collide with any different logical payload.
- [x] Reject mixed settlement assets across legs.
- [x] Canonically serialize metadata and proof fingerprints with deterministic ordering and normalized Decimal strings.
- [x] Add a metadata-fingerprint exclusion test: two groups with identical venue identity/grouping metadata but different order constraints, freshness windows, notional caps, or submit modes must have the same `metadata_fingerprint`; changing a covered venue identity or grouping field must change it.
- [x] Keep fees out of `OutcomeLeg`; costs must be resolved through `FeeProvider` and existing instrument metadata at scan time.
- [x] Add a source-fence or equivalent static check over all new outcome-group source files, strategy files, scanner/admission/execution modules, provider modules, and `src/bolt_v3_cross_venue_outcome_groups.rs`. It must prevent `match`, `if let`, `matches!`, or destructuring on `GroupingProof`, `RoleBindingProof`, `OutcomeGroupSourceKind`/`source_kind`, `SettlementSourceKind`/`settlement_source_kind`, `NormalizedPriceScaleEvidence`, `PriceScaleAssertionSource`, and `OrderConstraintSource` outside normalizers and evidence serializers, with at least one positive control test.
- [x] Export the module from `src/lib.rs`.
- [x] Run allowed local formatting checks and remote Rust verification according to repo policy after commit.

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

- [ ] Write tests for root-level source parsing, root `outcome_group_sources` unknown-field closure, backward-compatible parsing when `outcome_group_sources` and `risk.basket_execution` are absent for binary-oracle-only roots, `risk.basket_execution` required for strategies that use live outcome-group basket execution, per-strategy file parsing, Polymarket event source parsing, Polymarket market-slug-only source parsing, bounded Gamma-query source parsing, Hyperliquid HIP-4 source parsing with one configured NT `OutcomeQuestion.question`, duplicate `source_id` rejection, unbounded query rejection, missing `freshness`, missing order constraints, non-positive `min_quantity`, missing `min_notional` for any submit mode whose NT template or venue path requires a notional floor, non-positive `min_notional`, missing settlement rules, missing terminal states, missing role bindings, missing non-standard terminal payout vectors, missing `expected_neg_risk_market_id`, unknown client rejection, source-client/execution-client mismatch rejection, group-source cross-reference rejection, missing `[reference_data]`, missing `[signal_data]`, missing scanner depth/slippage parameters, unsupported `submit_mode`, missing decision-evidence path, no dummy realized-volatility surface requirement for outcome-group consumers that declare no RV requirement, binary-oracle still requiring realized-volatility through archetype validation, no fatal `target_runtime_fields` dependency, and unknown target fields.
- [ ] Implement source config enums and validators.
- [ ] Define closed enums and validators for `settlement_source_kind`, `terminal_state_convention`, `void_policy`, `rounding_policy`, `timing_policy`, refund conventions, source kinds, recovery policy, and target kind.
- [ ] Add `outcome_group_sources` to `BoltV3RootConfig` as default-empty/optional and add `risk.basket_execution` to the risk config block as optional; cross-config validation requires both only when a loaded strategy consumes live outcome-group basket execution.
- [ ] Keep all runtime selectors in TOML.
- [ ] Reject scan-all configs unless they include explicit bounded caps and freshness controls.
- [ ] Add an `outcome_group` market-family binding so existing `target.rotating_market_family` dispatch remains the single target-routing path.
- [ ] Define `outcome_group::TargetBlock` with `configured_target_id`, `kind = "static_outcome_group"`, `rotating_market_family = "outcome_group"`, and `group_sources`.
- [ ] Define `outcome_group::RotatingMarketFamily { OutcomeGroup }` inside the new family module and register `outcome_group::KEY` in `src/bolt_v3_market_families/mod.rs` validation bindings.
- [ ] Define `OutcomeGroupTargetPlan` and `outcome_group::target_plans(plan)` so provider mapping can resolve `target.group_sources[]` against root sources by `source_id`, `enabled`, and `client_id`; add tests proving providers do not emit all root sources for a client.
- [ ] For the existing `MarketFamilyValidationBinding` slots, enumerate every function pointer from `src/bolt_v3_market_families/mod.rs`. Support `validate_target` and `plan_strategy_target` only. Return typed unsupported errors only from Result-returning slots; return `None` from Option-returning unsupported capabilities such as `select_binary_option_market`, fair probability, and maker helpers.
- [ ] Ensure outcome-group target validation does not require `TargetRuntimeFields`, `target.gate_subscriptions`, dummy `underlying_asset`, cadence fields, slug tokens, market-selection rules, or realized-volatility target fields. Static outcome groups use `plan_strategy_target` plus source discovery, not the up/down runtime-field contract.
- [ ] Treat `expected_neg_risk_market_id` and `terminal_state_labels` as expectations checked against proof metadata, never as proof.
- [ ] Bind configured `terminal_state_labels` to venue-derived standard outcomes through proof metadata and fail closed on missing, duplicate, extra, or unmapped labels.
- [ ] Validate `[order_constraints]`: every enabled live source has a positive default or per-leg `min_quantity`, every submit mode whose NT template or venue path requires a notional floor has a positive default or per-leg `min_notional`, optional `min_notional` is positive when present, and every venue leg resolves to an `OutcomeLegOrderConstraints` record. Quantity step and precision remain NT-derived; min floors are config-owned and cannot default to zero or `None`.
- [ ] Add a fail-closed source validator for `GateProviderFreshnessBlock` option fields and define evidence fields for local receive time, provider event time, and clock skew.
- [ ] Keep live book freshness (`GateProviderFreshnessBlock`) separate from metadata-cache TTL. Validate metadata TTL from the configured client data `update_instruments_interval_mins`; reject zero or absent metadata refresh for clients used by outcome-group sources.
- [ ] Move global realized-volatility-surface requiredness behind archetype validation; re-home the requirement into the binary-oracle taker archetype validator and declare no RV requirement for the first outcome-group consumer `complete_set_arbitrage`.
- [ ] Do not modify `GATED_SOURCE_ROOTS` in this task. A gated source root may be registered only in the same commit that creates every file under that root.

### Task 4: Polymarket OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_polymarket.rs`
- Modify `src/bolt_v3_providers/polymarket.rs`
- Test `tests/bolt_v3_polymarket_outcome_groups.rs`

- [ ] Write tests using synthetic Gamma event/market metadata for a three-way group with one shared non-null `GroupingProof::PolymarketNegRisk { neg_risk_market_id = <negRiskMarketID>, ... }`.
- [ ] Prove that both YES and NO token legs are preserved.
- [ ] Prove source-specific role mapping: each Polymarket market maps to exactly one configured terminal state; operator-attested positive-side binding names the native leg that becomes `PaysOnTerminalState(state_id)` and the inverse native leg that becomes `PaysUnlessTerminalState(state_id)`; unknown, missing, re-keyed, or multi-state side semantics reject before matrix construction.
- [ ] Forbid hardcoded "Yes", "No", "Up", "Down", or positional token-order heuristics in Polymarket role assignment; tests must use non-Yes/No labels to prove the normalizer reads only attested native-leg bindings plus venue metadata consistency.
- [ ] Prove terminal-state label binding rejects missing labels, duplicate labels, unmatched Gamma outcomes, and extra standard outcomes under the same `GroupingProof::PolymarketNegRisk.neg_risk_market_id`.
- [ ] Prove that Polymarket grouping requires one shared non-null `GroupingProof::PolymarketNegRisk.neg_risk_market_id`; it must not rely on event slug, sports market type, question text, or slug patterns alone.
- [ ] Prove market-slug-only and bounded-Gamma-query-only complete sets work with empty `event_slugs`; event membership is evidence, not a required proof field.
- [ ] Prove that event containers with unrelated markets are rejected unless all admitted markets share the same source-native grouping identity, one-to-one terminal-state binding, price-scale proof, and terminal-state contract.
- [ ] Prove that missing void/refund policy rejects the group.
- [ ] Map Polymarket Gamma/NT metadata to `OutcomeGroup`.
- [ ] Implement the evidence-selected `negRiskMarketID` path for Polymarket sources. If Task 0 selects NT metadata surfacing, consume the surfaced NT field and prove it is present in the loaded instruments. If Task 0 selects a Bolt provider-local cache, fetch each configured Gamma event, market slug, event query, or bounded Gamma query once; cache the raw response keyed by native token id, condition id, and market slug; recover `negRiskMarketID`; and emit the exact NT discovery filters/market slugs from the same cached response.
- [ ] Bind Gamma cache TTL to the Polymarket client data `update_instruments_interval_mins`, not to live book `freshness.max_age_ms`; refresh before scan/admission when stale; invalidate on NT instrument reload conflicts; and fail closed rather than using expired grouping proof.
- [ ] Do not add two active runtime paths for outcome-group metadata. NT may still load instruments from emitted filters. If Task 0 selects a provider-local cache, that cache is the sole Bolt metadata source used for `negRiskMarketID` proof, terminal-state binding, price-scale evidence, and conflict checks. If Task 0 selects NT metadata surfacing, do not keep a parallel Bolt proof cache for the same field.
- [ ] Fail closed when any leg lacks a non-null `negRiskMarketID`, when recovered metadata conflicts with the NT instrument, or when Gamma cache age exceeds `metadata_ttl_ms` derived from `clients.<id>.data.update_instruments_interval_mins` and capped by `risk.basket_execution.max_metadata_age_ms`. Never compare Gamma metadata age to live-book `freshness.max_age_ms`.
- [ ] Add provider mapping for TOML-driven NT discovery filters: explicit event slugs, market slugs, event queries, and bounded Gamma queries.
- [ ] Extend Polymarket `SUPPORTED_MARKET_FAMILIES` with `outcome_group::KEY` and add adapter-mapping tests proving outcome-group sources produce the expected NT filters for the configured `client_id`.
- [ ] Enforce first-slice live scope by rejecting any Polymarket outcome group whose source `client_id` differs from the strategy `execution_client_id`.
- [ ] Avoid per-scope NT `accept()` outcome predicates because NT applies accept predicates globally across filters.

### Task 5: Read-Only Outcome-Group Scanner

**Files:**
- Create `src/bolt_v3_outcome_group_scanner.rs`
- Test `tests/bolt_v3_outcome_group_scanner.rs`

- [ ] Write tests for all-role-true, all-role-false, mixed-role baskets, void/refund rows, NT `OrderBook`/`OrderBookDepth10`/`BookLevel` adapter coverage, insufficient depth, stale book, missing book timestamps, normalized price scale rejection, fee inclusion, minimum fillable depth sizing, config-owned minimum quantity rejection when NT Polymarket min quantity is `None`, config-owned minimum notional rejection, quantity-step/precision rejection, Decimal/f64 fee conversion boundary, Decimal/f64 price conversion boundary, non-positive candidate-cost rejection, non-positive edge, `edge_bps` threshold admission/rejection, and absolute-edge-vs-bps mismatch rejection.
- [ ] Implement payout-vector evaluation from `PayoutMatrix`.
- [ ] Reuse `bolt_v3_executable_cost::price_exact_size_vwap` and `bolt_v3_executable_cost::executable_cost_breakdown` for per-leg executable depth and adjusted cost.
- [ ] Implement only the timestamped NT `OrderBook`/`OrderBookDepth10`/`BookLevel` to `ExecutableBookQuote` adapter and basket aggregation logic around the existing per-leg helper functions; reject any attempt to persist or maintain an independent Bolt order book.
- [ ] Use existing `FeeProvider` to resolve fee inputs by instrument; do not introduce a parallel fee model.
- [ ] Convert `FeeProvider::fee_bps` Decimal outputs to f64 bps at one explicit boundary, then convert adjusted costs back to Decimal settlement-currency values for basket comparison.
- [ ] Use executable depth, not display or Gamma prices.
- [ ] Validate proposed leg quantities against `OutcomeLegOrderConstraints` floors plus NT instrument quantity step, precision, and order-template constraints; reject the entire basket if any leg cannot be submitted at the scanned size.
- [ ] Return scanner evidence with grouping proof, costs, fees, state payouts, guaranteed payout, absolute edge, normalized `edge_bps`, min-depth cap, freshness readings, and block reason.

### Task 6: Basket Admission

**Files:**
- Create `src/bolt_v3_basket_admission.rs`
- Modify `src/bolt_v3_submit_admission.rs`
- Modify `src/bolt_v3_decision_evidence.rs`
- Test `tests/bolt_v3_basket_admission.rs`

- [ ] Write tests for basket notional cap, max open basket cap, stale evidence rejection, stale submit re-check rejection, negative edge rejection, non-positive candidate-cost rejection, `edge_bps` threshold rejection, missing grouping proof rejection, missing settlement rules rejection, reservation release, retry-budget rejection, exact-cap submit-slot admission, current-count-plus-leg-count cap exhaustion, count-cap overflow rejection, kill-switch-latched entry rejection, risk-reducing repair/unwind proof handling, and rejection when a basket risk-reducing proof was minted for a different instrument, side, or quantity.
- [ ] Implement basket-level admission that reserves the whole basket, not individual legs independently.
- [ ] Integrate with `BoltV3SubmitAdmissionState` as monotonic venue-order approval accounting: each submitted basket leg/order consumes one per-client admitted-order slot and is not decremented when the basket closes. Keep this separate from the releasable basket exposure/budget reservation, which releases only on terminal, abort, reject, or stuck transitions.
- [ ] Extract the existing single-order common submit-gate checks into a shared `pub(crate)` evaluator used by both `admit()` and the basket API; do not copy the kill-switch, notional, lifecycle, or per-client count-cap logic.
- [ ] Add a basket-aware submit-slot API such as `reserve_basket_submit_slots(execution_client_id, claims, evidence)` on `BoltV3SubmitAdmissionState`, where each claim carries `instrument_id`, `order_side`, `order_quantity`, notional, submit intent kind, and optional risk-reducing-exit proof. The shared evaluator must build or borrow the same per-leg request view used by `BoltV3RiskReducingExitProof::is_valid_for`, so proofs remain bound to instrument, side, and quantity. It must perform the same kill-switch latch, lifecycle, intent-kind, risk-reducing proof, and per-leg notional cap checks as the single-order path, reject when `current_count.checked_add(leg_order_count)` overflows or exceeds `max_order_count`, increment the same monotonic counters atomically, and record basket admission evidence once.
- [ ] Reuse `bolt_v3_submit_admission` pure arithmetic helpers; do not call stateful single-order `BoltV3SubmitAdmissionState::admit()` once per basket leg.
- [ ] Because this task edits `src/bolt_v3_submit_admission.rs`, regenerate `GOLDEN_SUBMIT_ADMISSION_DIGEST` and keep the submit-admission value-stability tests green.
- [ ] Record evidence keyed by strategy id, basket id, group id, and leg instrument IDs.
- [ ] Persist reservation state before any venue mutation.

### Task 7: Basket Execution State Machine

**Files:**
- Create `src/bolt_v3_basket_execution.rs`
- Create `src/bolt_v3_basket_store.rs`
- Modify `src/bolt_v3_config.rs`
- Modify `src/bolt_v3_kill_switch.rs`
- Test `tests/bolt_v3_basket_execution.rs`

- [ ] Write state-transition tests for complete fill, partial fill, NT `OrderList`/`SubmitOrderList` submission for supported same-venue baskets, reviewed fallback/rejection when an audited submit mode cannot use `SubmitOrderList`, repair allowed, repair denied, repair quantity math, unwind allowed, unwind denied after settlement, Polymarket settlement-signal disposition handling, HIP-4 synthetic settlement-fill handling, cancel rejection, retry-budget exhaustion, restart reconciliation including durable order-list id and per-leg client/venue order ids, engine-classified external/unclaimed reports, orphan-leg adoption versus `Stuck`, stuck state, basket-stuck kill-switch trigger, reservation release, and terminal close.
- [ ] Implement state transitions as pure logic first.
- [ ] Define repair quantity math before live submit: inputs are the admitted target quantity vector, current filled quantity vector, payout matrix, filled-cost ledger, fresh executable books, configured slippage/depth bounds, and remaining retry budget. Repair submits only residual quantities that restore the admitted guaranteed-payout floor after new executable costs; otherwise transition to `Unwind` or `Stuck`.
- [ ] State the repair inequality explicitly: after applying proposed residual repair fills and executable costs, `min(M * (filled_qty + repair_qty)) - (filled_cost + repair_cost)` must preserve the admitted absolute edge floor and normalized `edge_bps` floor; otherwise repair is not admissible.
- [ ] Define unwind math before live submit: use the same fill vector, books, and settlement-state checks to reduce residual directional exposure without assuming the missing leg is still tradable; reject unwind after settlement or stale books and transition to `Stuck`.
- [ ] Do not treat a synthetic Polymarket handler call as proof of live settlement reachability. Polymarket live basket execution must reject at validation/runtime unless the Task 0 `settlement_signals` ledger proves an accepted reachable close signal that does not violate the controlled-connect invariant.
- [ ] Define the executor event integration contract: the complete-set strategy shell forwards NT order/fill/cancel/instrument-status/settlement events into `bolt_v3_basket_execution`, and shared executor code owns the resulting transitions. Strategy code must not call submit admission or venue mutation APIs directly.
- [ ] Build same-venue submit commands through NT `OrderList`/`SubmitOrderList` for every adapter/submit-mode combination classified as `reuse_nt` in Task 0. Persist the NT order-list id with the basket, keep per-leg `client_order_id` and `venue_order_id`, and route all order-list acknowledgements/rejections/fills into the shared executor state machine.
- [ ] Do not invent a parallel Bolt batch-submit path. If Task 0 proves a specific submit mode cannot use NT `SubmitOrderList`, reject that mode for live baskets or implement the reviewed fallback as an explicit exception that still uses the shared basket admission reservation and per-leg evidence.
- [ ] Use existing NT order-management commands for repair/unwind cancellation or modification: `CancelOrder`, `BatchCancelOrders`, `CancelAllOrders`, and `ModifyOrder` where the audited venue/adapter path supports them. Do not call venue REST/WebSocket clients directly from Bolt basket code.
- [ ] Implement durable state persistence using `bolt_v3_atomic_io`.
- [ ] Keep `bolt_v3_basket_store` scoped to basket-specific proof, reservation, payout, order-list id, per-leg order ids, filled-cost ledger, and recovery state. Do not duplicate NT's general order cache. If Task 0 proves existing NT cache configuration can provide a durable report source, use it as an input to reconciliation instead of creating a second order-history path.
- [ ] Add `risk.basket_execution.state_path`, schema version, max state bytes, and fail-closed recovery policy validation.
- [ ] Persist per-leg `client_order_id` before submit and `venue_order_id` as soon as NT reports it. Implement restart reconciliation as an instrument-scoped NT status query over the basket instrument IDs, including strategy-owned, unclaimed, and engine-classified external order/fill/position reports. Join by client order id first, then by direct venue order id equality against the durable per-leg venue order id; adopt only reports with a deterministic durable basket match, and force `Stuck` for unjoinable orphan reports before admitting new baskets.
- [ ] Keep NT submit/cancel calls outside pure state logic.
- [ ] Require bounded repair/unwind policies from TOML before any live submit path uses them.
- [ ] Add a dedicated basket-execution-stuck kill-switch trigger kind and trip it for `Stuck` baskets that leave unresolved real venue exposure.
- [ ] Wire `Stuck` end to end: persist the basket state, transition the kill-switch state, write the kill-switch store, call the live-node/runtime hook that updates `BoltV3SubmitAdmissionState::replace_kill_switch_state`, and prove new basket/order admission is blocked.

### Task 8: First Consumer Strategy Shell Source Files

**Files:**
- Create `src/bolt_v3_archetypes/complete_set_arbitrage.rs`
- Create `src/strategies/complete_set_arbitrage/mod.rs`
- Test `tests/bolt_v3_complete_set_strategy_shell.rs`

- [ ] Create the first consumer archetype and concrete strategy shell source files, but do not add `complete_set_arbitrage` to `RUNTIME_BINDINGS`, `VALIDATION_BINDINGS`, or `src/strategies/mod.rs::production_strategy_registry()` in this task. The source files must exist before the outcome-group source-integrity key names them; production reachability comes only after Task 9.
- [ ] Define complete-set archetype gate requirements in the archetype file: no realized-volatility surface, no required reference-data roles for the first complete-set consumer slice, optional future signal gates only through explicit archetype validation, parameters schema, `submit_mode` variants, scanner depth/slippage validation, and NT order-template validation for every supported submit mode.
- [ ] Implement the concrete complete-set strategy shell under `src/strategies/complete_set_arbitrage/` as intent/event-forwarding only; submit gating, venue mutation, fillability, sizing, rounding, and repair/unwind mechanics stay in shared modules.
- [ ] For Polymarket settlement handling, follow the Task 0 `settlement_signals` disposition. If it proves a reachable NT close path, the complete-set strategy shell's registered DataActor lifecycle must subscribe and add a live-path test proving the basket executor receives the NT market-resolved/close event through the strategy shell. If the pinned NT path still requires `subscribe_new_markets = true` and Bolt still rejects that flag, live Polymarket basket execution must stay disabled with a validation/runtime rejection test; a synthetic handler test alone is not sufficient.
- [ ] Keep strategy code limited to signal/intent and local signal state.
- [ ] Route all admission and execution mechanics through shared modules.

### Task 9: Outcome-Group Source Integrity Registration

**Files:**
- Modify `src/source_canonicalization.rs`
- Modify `src/bolt_v3_source_integrity.rs`
- Test source-integrity value-stability tests in `src/bolt_v3_source_integrity.rs`

- [ ] Confirm every first-slice outcome-group root exists on disk before touching `GATED_SOURCE_ROOTS`: `src/bolt_v3_outcome_groups.rs`, `src/bolt_v3_outcome_group_sources.rs`, `src/bolt_v3_outcome_group_polymarket.rs`, `src/bolt_v3_outcome_group_scanner.rs`, `src/bolt_v3_basket_admission.rs`, `src/bolt_v3_basket_execution.rs`, `src/bolt_v3_basket_store.rs`, `src/bolt_v3_archetypes/complete_set_arbitrage.rs`, and `src/strategies/complete_set_arbitrage/`. Exclude `src/bolt_v3_outcome_group_hyperliquid.rs` from this task because HIP-4 is deferred and the file is created in Task 11.
- [ ] Add a new `OUTCOME_GROUP_KEY` entry in `GATED_SOURCE_ROOTS` for all existing first-slice outcome-group model, Polymarket normalizer, scanner, admission, execution, first consumer archetype, and concrete strategy-shell files. Do not register any root in a commit where that root does not exist.
- [ ] Add `GOLDEN_OUTCOME_GROUP_DIGEST`, value-stability tests, exact-membership tests, and a one-byte-change test. Keep binary-oracle `STRATEGY_KEY` scope unchanged.
- [ ] Keep `SUBMIT_ADMISSION_KEY` separate; Task 6 owns `GOLDEN_SUBMIT_ADMISSION_DIGEST` regeneration because it edits `src/bolt_v3_submit_admission.rs`.

### Task 10: First Consumer Runtime Activation

**Files:**
- Modify `src/bolt_v3_archetypes/mod.rs`
- Modify `src/strategies/mod.rs`
- Modify `src/strategies/registry.rs` only if a new concrete strategy entry point is required by the existing registry API.
- Modify `src/bolt_v3_strategy_registration.rs` only if the generic registration context needs a shared dependency.
- Test `tests/bolt_v3_strategy_registration.rs`

- [ ] Add `complete_set_arbitrage::KEY`, validation binding, runtime binding, and a `StrategyBuilder` implementation that uses the source files created in Task 8.
- [ ] Add the binding to `RUNTIME_BINDINGS` and `VALIDATION_BINDINGS` in `src/bolt_v3_archetypes/mod.rs` only after `OUTCOME_GROUP_KEY` and `GOLDEN_OUTCOME_GROUP_DIGEST` exist and pass their value-stability/exact-membership tests.
- [ ] Register `CompleteSetArbitrageBuilder` in `src/strategies/mod.rs::production_strategy_registry()` only after the source-integrity key covers the archetype and strategy shell.
- [ ] Resolve fee provider, execution venue, `StrategyBuildContext`, and NT `Trader.add_strategy()` handoff in the archetype binding.
- [ ] Add an end-to-end node-binding test proving the strategy is reachable from a root `strategy_files` entry after source-integrity registration, not before it.

### Task 11: Hyperliquid HIP-4 OutcomeGroup Normalizer

**Files:**
- Create `src/bolt_v3_outcome_group_hyperliquid.rs`
- Modify `src/bolt_v3_providers/hyperliquid.rs`
- Test `tests/bolt_v3_hyperliquid_outcome_groups.rs`

- [ ] Write tests using synthetic NT HIP-4 `BinaryOption` metadata.
- [ ] Prove that the existing Hyperliquid adapter path remains responsible for discovery and execution.
- [ ] Document and test that HIP-4 discovery is surface-wide through the existing `Hip4Outcomes` product surface and `update_instruments_interval_mins`; Bolt narrows loaded outcome instruments by one configured NT `OutcomeQuestion.question`, live-book freshness, and settlement attestation after NT discovery. Do not promise Polymarket-style NT filter projection for HIP-4 unless a real NT selector exists.
- [ ] Reject HIP-4 source configs with multiple question IDs under one settlement-rules block or an empty selector; use multiple source blocks for multiple questions.
- [ ] Prove that standalone HIP-4 outcomes without a parent question settlement signal are rejected unless an operator-attested settlement contract supplies terminal-state and void/fallback semantics.
- [ ] Treat HIP-4 settlement as the NT synthetic settlement fill signal, not a generic status report. The normalizer/executor must classify settlement fills separately from strategy fills and use the 0/1 settlement price to set the durable settled flag.
- [ ] Investigate runtime external-order claim handling with hard source evidence before constraining the HIP-4 live design. The investigation must trace `StrategyConfig.external_order_claims`, `LiveNode::add_strategy`, `ExecManager::claim_external_orders`, `ExecutionEngine::register_external_order_claims`, and settlement-fill routing at pinned rev `6e059dcb`; add a written source-anchor note and tests proving whether existing NT static/startup or manager/engine claim APIs can make discovered HIP-4 settlement fills visible without an NT extension. Only if that proof fails may the plan require a static expected instrument-id union or a new runtime claim-registration API.
- [ ] Convert HIP-4 outcome metadata into `OutcomeGroup`, including `GroupingProof::HyperliquidOutcome` from structured NT `BinaryOption.info` fields such as `question`, `outcome_index`, `outcome_side`, `named_index`, and `is_fallback`, terminal-state label binding, `OutcomeLegRole` assignment from structured `outcome_index`/`outcome_side`/named-index fields rather than label strings, price-scale evidence, and rejection of unknown or multi-state side semantics.
- [ ] Extend Hyperliquid `SUPPORTED_MARKET_FAMILIES` with `outcome_group::KEY` and add adapter-mapping tests proving configured HIP-4 outcome sources validate the configured `client_id`, product surface, live-submit approval, and Bolt-side outcome-question filter.
- [ ] Enforce existing Hyperliquid product-surface and live-submit approval gates.
- [ ] In the same commit that creates `src/bolt_v3_outcome_group_hyperliquid.rs`, extend `OUTCOME_GROUP_KEY` to include the HIP-4 normalizer root and refresh `GOLDEN_OUTCOME_GROUP_DIGEST`. Do not name the HIP-4 root in Task 9 because it does not exist in the first-slice source-integrity commit.

### Task 12: Cross-Venue Normalization

**Files:**
- Create `src/bolt_v3_cross_venue_outcome_groups.rs`
- Test `tests/bolt_v3_cross_venue_outcome_groups.rs`

- [ ] Write tests proving two venue groups can be matched only when their operator-attested settlement contracts are byte-equal on source, terminal-state convention, void policy, rounding policy, and timing policy.
- [ ] Implement semantic group matching with explicit evidence fields and no auto-match path without settlement contracts.
- [ ] Reject groups when cancellation, void, timing, settlement source, terminal states, rounding, or fallback semantics differ.
- [ ] Feed matched groups into the same read-only scanner/comparator, but keep live execution per single-client `OutcomeGroup` until a separate multi-client basket model exists.

## Delivery Order

1. NT capability evidence audit and guard: source-anchor every claimed NT/Bolt gap, especially `negRiskMarketID` surfacing, submit modes, `OrderList`/`SubmitOrderList`, order-book/depth data, cancel/modify commands, NT cache configuration, external-order claims, settlement signals, HIP-4 discovery, min-size/precision, and reconciliation. Record each disposition before building dependent code, create the machine-checkable NT capability ledger, add `scripts/verify_outcome_group_nt_reuse.py`, and wire it into `just source-fence-static`.
2. Backward-compatible config parsing: `outcome_group_sources` defaults empty and `risk.basket_execution` is optional unless a strategy consumes live outcome-group basket execution; do not modify `GATED_SOURCE_ROOTS` before every outcome-group source root exists.
3. Shared atomic I/O helper promoted from the kill-switch store pattern.
4. Shared model with leg-role standard payout derivation, single-state payoff scope, event-optional grouping proof, terminal-state label binding, re-key-safe role-binding proof, settlement rules, terminal-state-keyed attestation payloads, length-prefixed injective canonical bytes, operator-attested non-standard terminal payout vectors, settlement-asset validation, numeric price-scale evidence, config-owned order constraints, label rejection, byte-canonical attestation hash verification, fingerprint canonicalization, and matrix validation.
5. Config validation that adds cross-file outcome-group substrate checks, same-client first-slice enforcement, archetype-owned RV validation, the `outcome_group::TargetBlock`, provider market-family support, config-owned min-size floors, live-book freshness rules, metadata-refresh rules from `update_instruments_interval_mins`, metadata TTL caps, and checked expectations.
6. Basket-aware submit-slot API on `BoltV3SubmitAdmissionState`, with extracted shared submit-gate evaluator, per-leg request-shape claims, intent kind and risk-reducing proof handling, overflow-safe batch count-cap inequality, `GOLDEN_SUBMIT_ADMISSION_DIGEST` regeneration, separate from exposure reservation.
7. Provider discovery mapping plus Polymarket normalizer with explicit `outcome_group::target_plans` source join, the evidence-selected `negRiskMarketID` path, metadata TTL/refresh, event-optional grouping proof, operator-attested positive-side role binding, price-scale evidence, same-client validation, and NT filter wiring.
8. Read-only outcome-group scanner that consumes NT `OrderBook`/`OrderBookDepth10`/`BookLevel` data through a thin `ExecutableBookQuote` adapter and reuses `bolt_v3_executable_cost` and `FeeProvider`, with timestamped book snapshots, normalized price scale evidence, config-owned min-size floors, NT step/precision checks, Decimal/f64 conversion boundaries, positive candidate-cost validation, and `edge_bps` thresholding.
9. Basket admission, evidence, monotonic submit-approval cap accounting, freshness re-check, releasable exposure reservation, and durable reservation state.
10. Basket execution state machine with NT `OrderList`/`SubmitOrderList` for audited same-venue submit paths, `Stuck`, bounded repair/unwind quantity math, settlement/status event handling gated by the Task 0 `settlement_signals` disposition, engine-classified external/unclaimed restart reconciliation, dedicated basket-stuck kill-switch trigger, kill-switch store/admission latch wiring through `replace_kill_switch_state`, and shared-executor ownership.
11. Create the first-consumer archetype and strategy-shell source files, including the complete-set shell's Polymarket settlement-signal subscription or validation rejection according to the Task 0 disposition, but do not add binding-array entries or production-registry entries yet.
12. Outcome-group source-integrity registration for first-slice roots only: after every covered root exists, add `OUTCOME_GROUP_KEY`, `GOLDEN_OUTCOME_GROUP_DIGEST`, exact-membership tests, value-stability tests, and a one-byte-change test. Keep `SUBMIT_ADMISSION_KEY` separate and exclude the deferred HIP-4 normalizer until its creating commit.
13. Runtime activation: add `src/bolt_v3_archetypes/mod.rs` bindings, `src/strategies/mod.rs` production registry entry, `StrategyBuildContext` handoff, NT `Trader.add_strategy()` handoff, and node-binding proof only after the outcome-group source-integrity key passes.
14. Deferred HIP-4 normalizer through existing surface-wide Hyperliquid adapter discovery, one-question-per-source Bolt-side filtering, structured role fields, synthetic settlement-fill handling, settlement-contract gates, and same-commit `OUTCOME_GROUP_KEY` digest extension.
15. Deferred cross-venue read-only matching after operator-attested settlement contracts exist; no multi-client live basket until separately designed.
16. Additional strategy consumers and advanced order-mode polish using the same shared outcome and order layers.

## Review Prompt

Use this prompt for an adversarial architecture review:

```text
You are reviewing a Rust trading-system architecture for Bolt v3.

Goal:
Evaluate whether the proposed global shared outcome-group substrate correctly supports reusable outcome grouping, payout matrices, basket scanning/admission/execution, cross-venue outcome comparison, and future strategy consumers without hardcoding venue, market, team, outcome, or strategy-specific behavior. Complete-set arbitrage is the first proving consumer, not the owner of the shared layer.

Context:
- NautilusTrader owns venue adapters, instruments, books, order-list submission, order-management commands, fee calculation, signing, submit/cancel, cache/reconciliation surfaces where configured, and fill parsing.
- Bolt must own only shared cross-instrument semantics: grouping proof, payout matrix, basket scanning, basket-level admission, partial-fill state, restart reconciliation, kill-switch integration, and evidence.
- Outcome grouping, settlement proof, payout matrices, price-scale proof, basket scanning, basket admission, durable multi-leg execution, restart reconciliation, kill-switch integration, and evidence must be global shared Bolt primitives. They must not be named after, owned by, or coupled to Polymarket, Hyperliquid, moneyline markets, negative-risk markets, complete-set arbitrage, maker/taker strategies, YES/NO sides, or any other single strategy archetype.
- Polymarket and Hyperliquid HIP-4 should both normalize into the same shared OutcomeGroup model.
- Runtime scope must come from TOML config.
- Strategies produce intent only; execution mechanics must stay in shared admission/execution modules or NT. Same-venue basket submission must use NT `OrderList`/`SubmitOrderList` whenever the audited adapter and submit mode support it.
- Every source must emit a `GroupingProof` whose variant carries the source-native grouping identity directly. Polymarket uses `GroupingProof::PolymarketNegRisk.neg_risk_market_id`; HIP-4 uses `GroupingProof::HyperliquidOutcome` from structured NT outcome/question fields; sources without a venue-native grouping signal require `GroupingProof::OperatorAttested.settlement_contract_id` or reject. Event slug, sports market type, neg_risk=true, question text, and outcome labels are discovery hints only. Market-slug-only and bounded-Gamma-query-only Polymarket sources must work without fake event slugs.
- Every admissible group must prove every terminal state, including void/refund/fallback behavior.
- Cross-venue auto-matching is forbidden unless operator-attested settlement contracts are byte-equal on source, terminal-state convention, void policy, rounding policy, and timing policy.
- Non-standard terminal states need operator-attested per-leg payout vectors; a void/refund/fallback row with no derivation rule is not proof.
- Standard terminal states need a leg-role derivation rule; matrix builders may not branch on venue side-label strings.
- Source normalizers must bind configured terminal-state labels to venue-derived outcomes, assign `OutcomeLegRole`, and reject unknown, duplicate, extra, or multi-state payoff legs before scanner input. Polymarket role assignment must come from re-key-safe operator-attested native-leg bindings checked against venue metadata, not from Yes/No strings or token order.
- Same-venue baskets are one source client and one execution client, and the first live slice requires them to be identical. Cross-venue work is read-only comparison until a multi-client basket model exists.
- Polymarket min order sizes cannot come from NT instruments because pinned NT leaves Polymarket `min_quantity` and `min_notional` unset. Live sources need config-owned positive min-size floors, while NT supplies quantity step and precision.
- The plan is expected to reuse existing Bolt engines: bolt_v3_executable_cost, FeeProvider, bolt_v3_submit_admission arithmetic helpers, GateProviderFreshnessBlock, bolt_v3_market_families, and a shared atomic I/O helper promoted from the kill-switch store pattern.
- Runtime strategy bindings live in src/bolt_v3_archetypes/mod.rs and per-archetype modules; src/bolt_v3_strategy_registration.rs is the generic binding-injected dispatcher.
- Provider `SUPPORTED_MARKET_FAMILIES` and provider discovery mapping must be extended for outcome_group; otherwise adapter validation or instrument discovery fails before runtime.
- Polymarket outcome-group Gamma discovery must use one provider-local cache pipeline that also emits the NT filters; a second independent Gamma metadata path is a dual path. Metadata cache TTL and refresh use the client data `update_instruments_interval_mins`, capped by `risk.basket_execution.max_metadata_age_ms`, not live-book `freshness.max_age_ms`.
- Hyperliquid HIP-4 discovery is surface-wide through `Hip4Outcomes` until a real bounded NT selector exists; Bolt narrows loaded instruments by one configured NT `OutcomeQuestion.question` per source and settlement attestation after NT discovery.
- Core realized-volatility validation currently leaks up/down assumptions and must be made archetype-conditional.
- All new outcome-group modules and first-consumer strategy shell files must be included under a dedicated `OUTCOME_GROUP_KEY` source-integrity/gated-source key before implementation is considered production-reachable, but that key must be registered only after every covered root exists on disk; placeholder roots are not acceptable. Edits to submit admission still require the existing submit-admission digest to be refreshed.
- `metadata_fingerprint` must cover venue identity and grouping metadata only; operator policy such as order constraints, freshness windows, notional caps, and submit mode must not change market identity fingerprints.
- Polymarket market-resolution ingestion must be subscribed from the concrete complete-set strategy shell's registered DataActor lifecycle, not from provider or normalizer code.
- At pinned NT rev `6e059dcb`, Polymarket market-resolution ingestion is not proven reachable through `subscribe_instrument_status` alone because `MarketResolved` requires `subscribe_new_markets`/`custom_feature_enabled`, while Bolt currently rejects `subscribe_new_markets = true` due all-markets subscription behavior. Reviewers must flag any plan text that treats a synthetic handler call or `subscribe_instrument_status` alone as live Polymarket settlement proof.
- Restart reconciliation must persist per-leg `client_order_id` and `venue_order_id`, join by client order id first and direct venue order id equality second, and must not rely on NT's process-local venue-to-client cache after restart.
- The ASAP World Cup path prioritizes Polymarket same-client, same-venue outcome-group baskets because that is the urgent configured venue/source path. It is not a taker-only substrate: `submit_mode` is config-driven and must map to existing NT-supported order templates. HIP-4 live execution, cross-venue matching, multi-client baskets, alternative terminal-state conventions, and new NT or venue-adapter extensions are deferred. Runtime external-order claim handling must be investigated against pinned NT source before deciding whether existing NT claim surfaces are sufficient or a new NT API is required.
- Runtime activation must not happen before source-integrity registration. Create archetype and strategy-shell source files first, register `OUTCOME_GROUP_KEY` over existing first-slice roots, then add binding-array and production-registry entries.
- Every claim that NT lacks a needed capability must cite pinned NT source and choose one disposition: reuse existing NT, wrap existing NT, surface the capability through an accepted NT dependency path, add a Bolt shim with a documented reason, or reject for now. Reviewers should treat assumption-based deferrals or reimplementations as findings. In particular, flag any basket execution path that bypasses NT `OrderList`/`SubmitOrderList` where supported, any repair/unwind path that bypasses NT `CancelOrder`/`BatchCancelOrders`/`CancelAllOrders`/`ModifyOrder` where supported, any scanner design that creates a parallel order-book model instead of adapting NT `OrderBook`/`OrderBookDepth10`/`BookLevel` into `ExecutableBookQuote`, or any basket store design that duplicates NT's general order cache instead of persisting only basket-specific proof/admission/recovery state.
- The NT reuse policy must be enforced by a local non-compile verifier wired into `just source-fence-static`. Reviewers should flag missing `scripts/verify_outcome_group_nt_reuse.py`, missing verifier tests, missing required capability-ledger entries, any undocumented `bolt_shim` disposition, or any production-reachable outcome-group code not covered by the NT reuse guard.

Review the plan for:
1. Any hidden hardcoding to Polymarket, HIP-4, World Cup, moneyline, YES-only, NO-only, a venue client, a market slug, or a specific strategy.
2. Any place where venue-specific metadata leaks past the normalizer into scanner, admission, execution, or strategy logic, including branching on `OrderConstraintSource`, `PriceScaleAssertionSource`, `GroupingProof`, `RoleBindingProof`, source kind, or settlement kind outside normalizers and evidence serializers.
3. Any unresolved either-or or deferred choice in the Polymarket `negRiskMarketID` recovery path or any other source-native grouping proof path. The plan must first investigate whether `negRiskMarketID` can be surfaced through NT metadata or an accepted NT dependency path, and if not must use exactly one provider-local Bolt-owned Gamma metadata cache that also projects NT filters. Other venues must provide equivalent structured native grouping proof or an operator-attested settlement contract before scanner input.
4. Any grouping proof that lacks the source-native identity required by its `GroupingProof` variant, hard-requires Polymarket event membership, accepts Polymarket event membership without one shared non-null `negRiskMarketID`, or accepts HIP-4 standalone outcomes without a parent settlement signal or attested settlement contract.
5. Any terminal-state gap: missing standard row derivation from `OutcomeLegRole`, missing source normalizer role assignment, Polymarket role assignment from labels/order instead of attested native-leg binding, re-keyable positive-side role bindings, missing terminal_state_labels-to-venue-outcome binding, unsupported terminal-state convention, multi-state payoff leg admitted as single-state, missing void/refund/fallback row, missing non-standard terminal payout vector, duplicate terminal-state source of truth, unrecognized outcome label, or payout-matrix row/column alignment bug.
6. Any config value masquerading as proof rather than a checked expectation against venue metadata or operator attestation.
7. Any cost-engine duplication instead of reusing bolt_v3_executable_cost, FeeProvider, submit-admission arithmetic helpers, GateProviderFreshnessBlock, market-family routing, and shared atomic I/O.
8. Any freshness gap where live-book max_age_ms/max_clock_skew_ms are absent, optional in live trading, use undefined clocks, are checked only before a non-atomic submit, or are incorrectly reused for metadata-cache TTL.
9. Any provider support or discovery gap: missing `SUPPORTED_MARKET_FAMILIES`, missing `outcome_group::target_plans`, missing provider-local Gamma cache/filter projection, missing metadata TTL/refresh via `update_instruments_interval_mins`, stale wording that compares Gamma metadata age to source/book freshness, a second Gamma fetch path for Bolt proof metadata, or HIP-4 promising bounded NT filters that do not exist.
10. Any unsafe scan-all, stale-book, fee-unit, slippage, minimum-depth, minimum-order-size, quantity-precision, book-adapter, normalized price scale, or liquidity assumption that could admit a basket that cannot fill atomically enough to preserve the payout floor. In particular, reject plans that source Polymarket min-size floors only from NT instruments.
11. Any config-contract mismatch with Bolt's real root strategy_files plus per-strategy strategy_archetype/[target]/[reference_data]/[signal_data]/[parameters] shape, root `outcome_group_sources`, `risk.basket_execution`, or backward-compatible parsing for existing binary-oracle-only roots.
12. Any outcome_group target-family mismatch: missing TargetBlock shape, missing kind, missing per-family RotatingMarketFamily type, accidental `TargetRuntimeFields` support, wrong unsupported-slot behavior for Result-returning or Option-returning functions, or missing binding-array registration.
13. Any missing runtime registration path for the first consumer, especially failure to add src/bolt_v3_archetypes/complete_set_arbitrage.rs, src/bolt_v3_archetypes/mod.rs binding-list entries, and src/strategies/mod.rs production_strategy_registry registration.
14. Any global realized-volatility validation that would force outcome-group consumers such as complete_set_arbitrage to define dummy RV surfaces or up/down target fields.
15. Any basket admission/execution gap: basket-aware submit-slot API, per-leg request shape needed for risk-reducing proof binding, per-leg intent kind and risk-reducing proof handling, duplicated submit-gate logic instead of extracted shared evaluator, incorrect or overflow-unsafe batch count-cap inequality, monotonic submit-approval cap accounting, separation from releasable exposure reservations, `edge_bps` thresholding, positive candidate-cost validation, basket state TOML contract, dedicated kill-switch trigger, partial-fill repair quantity math, unwind quantity math, kill-switch store/admission-latch wiring, cancel-reject, repair-recursion, settled-market unwind, settlement/status event ingestion, treating Polymarket `subscribe_instrument_status` or synthetic handler calls as sufficient settlement-signal proof while `subscribe_new_markets=true` is still rejected, Polymarket instrument-status subscription assigned to non-DataActor code, missing durable per-leg client/venue order ids, deterministic restart reconciliation join keys, reservation release, or shared executor ownership.
16. Any cross-venue design mismatch where one live basket spans multiple source/execution clients despite the single-client model.
17. Any attestation-hash gap: digest fields included in the hashed payload, governed terminal_state_id omitted from payout-vector or role-binding payloads, re-keyable vector or positive-side-binding entries, non-injective canonicalization such as delimiter-only `path=value` records, byte-canonicalization not reproducible by an operator, non-canonical ordering, non-normalized Decimal strings, non-lowercase/non-64-character SHA-256 hex, or no mismatch/reorder/re-key/collision tests.
18. Any architecture flaw that would prevent turning up Hyperliquid HIP-4 mostly through config once its OutcomeGroup normalizer exists, including treating synthetic settlement fills as ordinary strategy fills, claiming admission-time external-claim wiring that NT does not support, or missing a static expected instrument-id union/runtime-claim design for settlement visibility.
19. Any missing source-integrity/gated-source registration for the new shared outcome-group modules and concrete first-consumer strategy shell, premature registration before covered roots exist, placeholder roots, missing dedicated `GOLDEN_OUTCOME_GROUP_DIGEST`, or missing regeneration of `GOLDEN_SUBMIT_ADMISSION_DIGEST` when submit admission changes.
20. Any missing or ineffective NT reuse guard: no machine-checkable capability ledger, missing `scripts/verify_outcome_group_nt_reuse.py`, missing positive-control verifier tests, missing `just source-fence-static` wiring, required capabilities not covered, undocumented `bolt_shim` dispositions, or source files that violate a `reuse_nt`/`wrap_nt` decision without failing the verifier.
21. Any asserted NT gap that lacks pinned source evidence and a concrete disposition.
22. Any avoidable Bolt work that duplicates NT venue mechanics instead of using NT instruments, books, fees, signing, submit/cancel, fills, order status, instrument status, order-template support, and provider surfaces.
23. Any violation of the repo constraints: no hardcodes, no dual paths, no debts, no credential display, pure Rust, SSM-only secrets, source-integrity validation, and strategy-intent-only boundaries.

Return:
- Blocking findings first, with severity.
- Concrete fixes.
- Remaining risks after fixes.
- A revised implementation order if the current order is unsafe.
```

## Self-Review

- Spec coverage: covers the shared outcome-group substrate, basket arbitrage as the first consumer, cross-venue comparison, config-driven NT order-template integration, non-updown Polymarket, and HIP-4 support.
- Review corrections: NT capability evidence audit before dependent implementation, machine-checkable NT reuse guard, explicit `negRiskMarketID` surfacing-vs-cache disposition, source-native grouping identity carried directly by `GroupingProof` variants instead of a separate Bolt key wrapper, backward-compatible config parsing, deferred outcome-group source-integrity registration after covered roots exist, submit-admission digest regeneration, standard payout derivation, normalizer-owned role assignment, Polymarket event-optional grouping proof, Polymarket operator-attested positive-side binding with re-key rejection, terminal-state label binding, single-state payoff scope, grouping proof, provider support/discovery, explicit `outcome_group::target_plans` join, root source parsing, evidence-selected Polymarket metadata path and metadata TTL capped separately from live-book freshness, Polymarket live settlement signal gated on Task 0 evidence instead of assumed `subscribe_instrument_status` reachability, HIP-4 surface-wide one-question-per-source discovery semantics plus synthetic settlement-fill handling and source-evidenced claim visibility, same-client basket scope, void/refund/fallback terminal states, non-standard payout derivation, terminal-state-keyed digest-excluding settlement-attestation hashes, length-prefixed injective byte-canonical attestation serialization, freshness clocks, numeric price-scale assertion and `OrderConstraintSource` source fence, scoped `metadata_fingerprint` plus operator-policy exclusion test, config-owned min-order floors with notional floors required by submit mode/venue path and NT precision checks, cost-engine reuse, shared atomic I/O, durable basket state config with per-leg client/venue order-id persistence, basket-aware submit-slot API with full request-shape claims and shared evaluator, intent-kind/risk-reducing proof handling, overflow-safe batch cap inequality, `edge_bps` thresholding, monotonic submit-admission caps separate from releasable exposure reservations, dedicated Stuck kill-switch trigger with kill-switch/admission wiring, settlement/status event handling, deterministic direct venue-order-id restart reconciliation join keys, repair/unwind quantity math, config shape, market-family binding shape, unsupported `TargetRuntimeFields`, RV validation ownership, production strategy registry, and first-consumer archetype runtime registration are represented as explicit plan requirements.
- Placeholder scan: no deferred implementation placeholders are used as accepted behavior; each task names files, tests, and implementation scope.
- Type consistency: the shared model names are stable across normalizers, scanner, admission, execution, and review prompt.
