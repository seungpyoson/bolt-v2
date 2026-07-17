# Venue-Agnostic Economics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every decision-time fee path with one venue- and substrate-neutral economics quote/admission domain, while preserving actual accounting for later issue-owned slices and keeping all Polymarket and Hyperliquid live execution disabled.

**Architecture:** `src/economics/` owns Decimal-based canonical estimates, quote validation, valuation evidence, edge folding, and capability health without importing Bolt, NautilusTrader, venue, transport, persistence, or clock implementation types. Venue adapters under `src/bolt_v3_providers/*/economics.rs` own formulas and authoritative response parsing; execution-substrate adapters translate NT and replay objects into canonical requests. `src/bolt_v3_economics_runtime.rs` is the composition root, and `src/bolt_v3_order_execution.rs` remains the sole submit-routing owner.

**Tech Stack:** Rust 2024, `rust_decimal`, `serde`, NautilusTrader Rust API at adapter boundaries only, TOML configuration, Python static/source-fence verifiers, GitHub Actions remote Rust verification.

## Global Constraints

- Frozen architecture source: local read-only commit `00a5f9e6d7103b52ffcf210e96a3130150352f85`, read with `git show`; never merge, cherry-pick, or copy that commit into this branch.
- Authorized implementation: issue #1445, Slice 1 **Atomic quote/admission cutover** only.
- Direct user instruction on 2026-07-17 waives the otherwise-required Claude narrow closure review after the governed reviewer failed authentication; this is risk acceptance for the review gate, not permission to weaken any money-safety invariant.
- `src/economics/` imports no NautilusTrader, Bolt runtime, venue/provider SDK, transport, persistence, filesystem, database, or clock implementation type.
- Strategies emit intent and gross-value assumptions only. Economics quote, edge folding, sizing, and submit gating live in shared execution/admission.
- Estimates and actuals are different types. Slice 1 does not create actual entries or promote estimates into actuals.
- Unknown, missing, stale, contradictory, unsupported, or unvalued required economics never becomes zero.
- Forecast incentives never authorize admission.
- Runtime policy comes from TOML or authoritative live sources; no venue runtime constant, fallback, compatibility adapter, or parallel fee path is permitted.
- SSM remains the sole runtime secret source.
- Slice 1 leaves every Polymarket and Hyperliquid live execution surface disabled; only offline quote/replay is enabled.
- Local Rust compilation is prohibited by default. Use permitted local static gates and governed exact-head remote Rust evidence.

---

## Delivery and Issue Boundaries

| Slice | Deliverable | Owning issue | Dependencies | Live capability after merge |
| --- | --- | --- | --- | --- |
| 1 | Atomic quote/admission cutover | #1445 | Frozen design and fresh `main` | No Polymarket or Hyperliquid live execution; offline quote/replay only |
| 2 | Canonical actual ledger | A separate issue must explicitly own this slice before work begins; #1445 forbids it | Slice 1 canonical types and quote correlation | No Polymarket or Hyperliquid live execution; ledger infrastructure only |
| 3 | Supplemental venue actuals | A separate issue must explicitly own this slice before work begins; #1445 forbids it | Slice 2 durable ledger and authority plans | Only capability-complete surfaces may be armed after exact-source, backfill, and finality proof |
| 4 | Lifecycle and transfer economics | A separate issue must explicitly own this slice before work begins; #1445 forbids it | Slice 3 venue actual sources | No new surface unless every configured lifecycle/transfer action has quote and actual authority |
| 5 | Reporting closure | A separate issue must explicitly own this slice before work begins; #1445 forbids it | Slices 1–4 | Full program closure only after parity and boundary evidence; never inferred from Slice 1 |

The absence of Slice 2–5 issue numbers is an intentional execution block, not an implementation placeholder: this branch and PR must not create or implement those slices. Each later issue must restate remaining accepted scope and link #1445 and design `00a5f9e6…`.

## Requirement-to-Evidence Matrix

| Requirement | Implementation owner | Positive evidence | Fail-closed/removal evidence |
| --- | --- | --- | --- |
| Shared venue/substrate-neutral domain | `src/economics/*.rs` | Core unit and contract tests | `scripts/verify_economics_dependency_direction.py`; forbidden import fixtures |
| Canonical Decimal estimates and distinct actual type | `src/economics/types.rs` | construction/serialization tests | compile-time type separation; no conversion from estimate to actual |
| Quote validity, risk bounds, forecast separation | `src/economics/quote.rs`, `edge.rs` | guaranteed/risk-bound/forecast vectors | missing bound, stale source, forecast-only admission rejection |
| Explicit valuation route | `src/economics/valuation.rs` | identity and configured multi-leg routes | missing/stale/cyclic/disconnected route rejection; distinct stablecoin identities |
| Polymarket formula authority | `src/bolt_v3_providers/polymarket/economics.rs` | market-info fixtures, role and per-leg rounding vectors | missing fields, unsupported exponent, disagreement, stale snapshot rejection |
| Hyperliquid formula authority | `src/bolt_v3_providers/hyperliquid/economics.rs` | complete `userFees` spot/perp fixtures | missing product/account data, builder over-approval, aligned-status absence/schema drift rejection |
| Substrate isolation | `src/integrations/nautilus/economics.rs`, `crates/backtesting-vertical-slice/src/economics.rs` | NT mapping and replay mapping parity | synthetic non-NT substrate test; projection cannot become authority |
| Atomic admission cutover | `src/bolt_v3_economics_runtime.rs`, `src/bolt_v3_order_execution.rs`, `src/bolt_v3_submit_admission.rs` | quote-before-admission-before-submit ordering | stale/invalid quote prevents evidence permit and NT submit |
| Strategy intent only | strategy/archetype consumers | strategy tests pass with gross intent only | source fence rejects economics imports/math under `src/strategies/` and `src/bolt_v3_archetypes/` |
| One production path | deletions across provider, strategy, family, backtest, report | new runtime exercised by all consumers | zero matches for `FeeProvider`, `build_fee_provider`, family fee curve, flat `fee_bps`, scalar fee callback, and duplicate report math |
| No live arming | provider validation and readiness | offline replay contract passes | config validation rejects Polymarket/Hyperliquid live submit with `economics_slice = "quote_only"` |

## Planned File Ownership

### Shared domain created in Slice 1

- `src/economics/mod.rs`: exports the closed shared API.
- `src/economics/types.rs`: opaque identifiers, native units, signed effects, scopes, estimate/actual separation, request/quote/evidence types.
- `src/economics/ports.rs`: venue quote and valuation ports using shared types only.
- `src/economics/quote.rs`: request/component validation and core/forecast aggregation.
- `src/economics/edge.rs`: gross-to-core and gross-to-forecast edge folding.
- `src/economics/valuation.rs`: explicit route validation and native-to-reporting-currency conversion.
- `src/economics/health.rs`: capability completeness and fail-closed health state.

### Adapters and composition created in Slice 1

- `src/integrations/mod.rs`, `src/integrations/nautilus/mod.rs`, `src/integrations/nautilus/economics.rs`: NT intent/order translation only.
- `src/bolt_v3_providers/polymarket/economics.rs`: Polymarket schedule parser/formula/capabilities.
- `src/bolt_v3_providers/hyperliquid/economics.rs`: Hyperliquid account/product parser/formula/capabilities and observed aligned-status gate.
- `src/bolt_v3_economics_runtime.rs`: one configured client binding and quote orchestration.
- `crates/backtesting-vertical-slice/src/economics.rs`: replay substrate mapping and immutable historical snapshots.

### Existing owners modified in Slice 1

- `src/lib.rs`: export `economics`, `integrations`, and `bolt_v3_economics_runtime`.
- `src/bolt_v3_config.rs`: validate shared reporting policy and per-client economics selection without interpreting venue formulas.
- `src/bolt_v3_providers/mod.rs`: replace `build_fee_provider` with `build_economics_adapter` and fail closed for execution clients lacking a complete quote binding.
- `src/bolt_v3_providers/polymarket.rs`, `src/bolt_v3_providers/hyperliquid.rs`: parse provider-local economics config and build venue adapters.
- `src/bolt_v3_strategy_context.rs`, `src/bolt_v3_strategy_registration.rs`: remove fee providers; strategy contexts retain no quote/formula authority.
- `src/bolt_v3_order_execution.rs`, `src/bolt_v3_submit_admission.rs`: consume a validated `EconomicsAdmission` from the one runtime rather than a scalar maximum-fee callback.
- `src/strategies/binary_oracle_edge_taker/mod.rs`, `runtime_state.rs`: remove fee warming, fee calculation, historical fee fields, and net-edge adjustment; emit gross intent assumptions only.
- `src/bolt_v3_market_families/mod.rs` and current family bindings: remove `maker_binary_fee_curve`.
- `crates/backtesting-vertical-slice/src/lib.rs`, `runner.rs`: replace manifest `fee_bps` and `ManifestFeeProvider` with historical economics snapshots using the same core.
- `src/shadow_pnl.rs`, `tests/shadow_pnl_report.rs`: remove flat fee reconstruction; Slice 1 report consumes quote evidence for estimate-only shadow attribution and states that actual P&L is unavailable until later slices.
- `tests/bolt_v3_provider_binding.rs`, `tests/bolt_v3_strategy_registration.rs`, `tests/bolt_v3_strategy_substrate_structure.rs`, `tests/bolt_v3_submit_admission.rs`: pin the new registry and isolation boundaries.
- `scripts/verify_economics_dependency_direction.py`, `scripts/test_verify_economics_dependency_direction.py`, `scripts/run_fences.py`: static dependency and removal fence.
- `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`: synchronize only changed runtime literals through the governed verifier.

### Files reserved for later slices

- Slice 2: `src/economics/ledger.rs`, `reconcile.rs`, canonical actual-fact types, one infrastructure ledger-store adapter, quote reservation correlation.
- Slice 3: Polymarket versioned contract actual sources and Hyperliquid fill/funding/ledger sources under provider-local modules.
- Slice 4: lifecycle/transfer quote and actual adapters for settlement, redemption, liquidation, bridge, deposit, and withdrawal actions.
- Slice 5: canonical actual-ledger P&L reconstruction, live/replay/backtest parity closure, and removal of the temporary Slice 1 estimate-only report declaration.

## Later-Slice Deliverable and Evidence Contracts

These contracts are planning output only. They cannot be executed on #1445 and do not authorize production code.

### Slice 2 — Canonical actual ledger

**Exact planned files:** `src/economics/ledger.rs`, `src/economics/reconcile.rs`, additions to `src/economics/types.rs` and `ports.rs`, `src/integrations/nautilus/economics.rs`, `src/bolt_v3_economics_ledger_store.rs`, `tests/economics_ledger_contract.rs`, `tests/economics_reconciliation_contract.rs`, and `scripts/verify_economics_dependency_direction.py`.

**Interfaces:** `CanonicalFillFact`, `CanonicalPositionFact`, and `CanonicalSettlementFact` carry opaque correlations, Decimal native values, timestamps, and `EvidenceOrigin`; `LedgerStore` atomically appends typed records and advances a checkpoint root only after verified durability; `ActualAuthorityPlan` selects exactly one booking source per enabled kind; `append_actual` accepts only `ActualEconomicEntry` and has no estimate overload.

**Positive evidence:** append/read durability; signed currency and asset-quantity entries; correction entries referencing originals; strict-cursor and exact-ID deduplication; atomic observation deltas whose totals may decrease; checkpoint/segment rotation; quote correlation; native-first reconciliation; restart from a valid scrub attestation.

**Fail-closed evidence:** duplicate ID with divergent payload; estimate passed as actual (compile failure); partial tail; missing segment/index run; stale/missing scrub attestation requiring full rehash; hash mismatch; recovery-byte overflow; source without strict cursor or exact index; durability failure before in-memory advance; actual absent beyond lag. Locally recomputed Polymarket commission remains reconciliation-only. Live execution remains disabled.

### Slice 3 — Supplemental venue actuals

**Exact planned files:** `src/bolt_v3_providers/polymarket/economics_actuals.rs`, `src/bolt_v3_providers/polymarket/economics_contracts.rs`, `src/bolt_v3_providers/hyperliquid/economics_actuals.rs`, provider fixture directories under `tests/fixtures/economics/`, `tests/polymarket_economics_actuals.rs`, `tests/hyperliquid_economics_actuals.rs`, and boundary-registry entries selected by the then-current authoritative registry.

**Interfaces:** Polymarket maps configured V1/V2 contract epochs and confirmed CLOB/chain identities into `ActualEconomicEntry`; current V2 books one `Execution::CombinedVenueFee` unless complete order-correlated `FeeCharged` fixtures authorize a reviewed authority migration. Hyperliquid maps `userFills`, `userFunding`, user/node ledger events, staking/referral/builder income, and corrections through a closed `EconomicEventDisposition` table.

**Positive evidence:** standard and negative-risk V2 fills; V1 fixture only when coverage crosses its epoch; buy/sell, limit/FOK/FAK, multi-price/multi-log; confirmation and replacement; all current Polymarket activity variants; Hyperliquid fills/funding/rewards/ledger sources; strict history-depth and bootstrap receipts; backfilled credits since coverage start.

**Fail-closed evidence:** epoch overlap/gap; unknown event discriminator; ambiguous order/log join; incomplete `FeeCharged` receiver set; NT zero-schedule projection offered as booking authority; reorg before complete correction; terminal venue/chain divergence; stale actual source; history depth unable to reach coverage start; missing node source; unproved dust opt-out; aligned status without governed capture. Only fully complete product/account surfaces may be armed.

### Slice 4 — Lifecycle and transfer economics

**Exact planned files:** `src/economics/lifecycle.rs`, provider-local `economics_lifecycle.rs` modules, substrate lifecycle mapping additions, `tests/economics_lifecycle_contract.rs`, and `tests/economics_transfer_contract.rs`.

**Interfaces:** canonical action scopes distinguish settlement, redemption, split/merge, exercise, liquidation, activation, deposit, withdrawal, bridge, conversion, network/gas, intermediary, and inter-account transfer charges. Transfer principal is classified separately and never booked as economics.

**Positive evidence:** action quote plus actual booking; settlement payout retained as gross value; liquidation and transfer charges attributed only with authoritative causal keys; account-wide income remains account P&L; corrections retain the original economic kind.

**Fail-closed evidence:** missing action authority; principal misclassified as fee; unrelated account amount allocated to strategy; lifecycle charge lacking native unit; transfer route missing configured valuation; unsupported provider action; estimate copied into actual; actual duplicated through substrate and venue facts.

### Slice 5 — Reporting closure

**Exact planned files:** `src/economics/report.rs`, `src/shadow_pnl.rs`, `src/bin/shadow_pnl_report.rs`, backtester result/report modules, `tests/economics_reporting_contract.rs`, `tests/shadow_pnl_report.rs`, parity fixtures, and authoritative boundary-registry/source-fence entries.

**Interfaces:** one report folds canonical gross cash/inventory transitions with actual signed effects exactly once by native unit and `inventory_application`, then values the net transition once. Component normalization is attribution evidence, never another cash entry.

**Positive evidence:** live/replay/backtest parity for currency fees, asset-quantity fees, rebates, funding, settlement, and corrections; account-state reconciliation; historical point-in-time valuation; coverage-start disclosure; native unnormalized effects visible when normalization is unavailable.

**Fail-closed/removal evidence:** report cannot add actual ledger to an already-net NT P&L; no flat fee or current-price historical conversion; no lifetime claim before coverage start; missing permanent valuation gap is explicit; zero matches for duplicate report formulas; all provider/runtime boundary dependencies registered and fenced.

---

### Task 1: Canonical shared economics types and ports

**Files:**
- Create: `src/economics/mod.rs`
- Create: `src/economics/types.rs`
- Create: `src/economics/ports.rs`
- Modify: `src/lib.rs`
- Test: `tests/economics_core_contract.rs`
- Test support: `tests/economics_support/mod.rs`
- Test harness: `tests/wiring_registration.rs` (`mod economics_core_contract;`; root `autotests = false`)

**Interfaces:**
- Produces: `NativeUnitId`, `SignedNativeEffect`, `EconomicScope`, `AdmissionTreatment`, `EstimatedEconomicComponent`, `ActualEconomicEntry`, `EconomicQuoteRequest`, `EconomicQuote`, `NetEdgeQuote`, `EdgeBasisEvidence`, `ValuationEvidence`, `EconomicsUnavailable`.
- Produces: `VenueEconomicsAdapter::quote(&EconomicQuoteRequest) -> Result<Vec<EstimatedEconomicComponent>, EconomicsUnavailable>` and `ValuationProvider::value(&SignedNativeEffect, &ValuationRequest) -> Result<ValuationEvidence, EconomicsUnavailable>`.
- Constraint: `ActualEconomicEntry` has no constructor or `From` implementation accepting `EstimatedEconomicComponent`.

- [ ] **Step 1: Add contract tests that fail because the shared module does not exist**

```rust
use bolt_v2::economics::{
    ActualEconomicEntry, AdmissionTreatment, EstimatedEconomicComponent, NativeUnitId,
    SignedNativeEffect,
};
use rust_decimal::Decimal;

fn decimal(value: &str) -> Decimal {
    Decimal::from_str_exact(value).expect("fixture decimal must parse")
}

#[test]
fn signed_native_effect_preserves_sign_and_unit() {
    let effect = SignedNativeEffect::currency(
        decimal("-1.25"),
        NativeUnitId::new("pUSD").unwrap(),
    )
    .unwrap();
    assert_eq!(effect.amount(), decimal("-1.25"));
    assert_eq!(effect.unit().as_str(), "pUSD");
}

#[test]
fn forecast_component_is_not_admission_authority() {
    let component = economics_support::estimated_component(
        "forecast-reward",
        decimal("2.00"),
        AdmissionTreatment::ForecastOnly,
    );
    assert!(!component.authorizes_admission());
}

fn actuals_cannot_be_constructed_from_estimates(
    actual: ActualEconomicEntry,
    estimate: EstimatedEconomicComponent,
) {
    let _ = (actual, estimate); // Type separation is inspected by the source fence.
}
```

- [ ] **Step 2: Run the permitted remote-focused Rust feedback path and record the expected missing-module failure**

Run: `just rust-probe suggest`

Expected: the suggestion names `economics_core_contract`; dispatch only after the branch is clean, committed, and pushed as required by `AGENTS.md`.

- [ ] **Step 3: Implement the canonical type surface**

```rust
pub enum SignedNativeEffect {
    CurrencyAmount { amount: Decimal, currency_id: NativeUnitId },
    AssetQuantity {
        quantity: Decimal,
        asset_id: NativeUnitId,
        inventory_application: InventoryApplication,
    },
}

pub enum AdmissionTreatment {
    GuaranteedConditionalOnAction,
    RiskBound { authority: RiskBoundAuthority },
    ForecastOnly,
}

pub struct EstimatedEconomicComponent {
    pub component_id: EconomicComponentId,
    pub class: EconomicClass,
    pub kind: EconomicKind,
    pub scope: EconomicScope,
    pub point_effect: SignedNativeEffect,
    pub debit_risk_bound: Option<SignedNativeEffect>,
    pub admission_treatment: AdmissionTreatment,
    pub source: SourceValidity,
    pub normalized: Option<ValuationEvidence>,
}
```

All newtypes validate non-empty bounded strings. Zero effects are rejected instead of stored. Timestamps are integer nanoseconds supplied as data, never read from a clock implementation. `tests/economics_support/mod.rs` owns `decimal`, `estimated_component`, `guaranteed`, `risk_bound`, `risk_bound_without_debit_bound`, `forecast`, `quote_fixture`, `stale_quote_fixture`, `canonical_fixture_request`, and `synthetic_venue`; each helper constructs the public API directly and contains no alternate production calculation.

- [ ] **Step 4: Add compile/static isolation assertions**

Run: `python3 scripts/verify_economics_dependency_direction.py`

Expected: PASS and a positive-control fixture proving `use nautilus_model::...` beneath `src/economics/` is rejected.

- [ ] **Step 5: Commit the shared vocabulary**

```bash
git add src/economics src/lib.rs tests/economics_core_contract.rs tests/economics_support/mod.rs tests/wiring_registration.rs scripts/verify_economics_dependency_direction.py scripts/test_verify_economics_dependency_direction.py scripts/run_fences.py
git commit -m "feat(#1445): add canonical economics domain contracts"
```

### Task 2: Quote validation, valuation, edge folding, and health

**Files:**
- Create: `src/economics/quote.rs`
- Create: `src/economics/edge.rs`
- Create: `src/economics/valuation.rs`
- Create: `src/economics/health.rs`
- Modify: `src/economics/mod.rs`
- Test: `tests/economics_quote_contract.rs`
- Test: `tests/economics_valuation_contract.rs`

**Interfaces:**
- Consumes: Task 1 shared types and ports.
- Produces: `validate_and_aggregate_quote(request, components, valuations) -> Result<EconomicQuote, EconomicsUnavailable>`.
- Produces: `fold_net_edge(gross_expected_value, quote, edge_basis) -> Result<NetEdgeQuote, EconomicsUnavailable>`.
- Produces: `EconomicsCapabilityHealth::allows_admission(now_ns) -> Result<(), EconomicsUnavailable>`.

- [ ] **Step 1: Add positive and fail-closed quote tests**

```rust
#[test]
fn core_total_uses_guaranteed_point_and_risk_bound_debit() {
    let quote = quote_fixture([
        guaranteed(dec!(-1.00)),
        risk_bound(dec!(-0.25), dec!(-0.75)),
        forecast(dec!(2.00)),
    ]).unwrap();
    assert_eq!(quote.core_total(), dec!(-1.75));
    assert_eq!(quote.forecast_total(), dec!(0.75));
}

#[test]
fn missing_risk_bound_rejects_core_quote() {
    assert_matches!(
        quote_fixture([risk_bound_without_debit_bound()]),
        Err(EconomicsUnavailable::MissingDebitRiskBound { .. })
    );
}

#[test]
fn stale_required_component_rejects_admission() {
    assert_matches!(
        stale_quote_fixture(),
        Err(EconomicsUnavailable::StaleSource { .. })
    );
}
```

- [ ] **Step 2: Implement deterministic aggregation and edge folding**

Aggregation validates request identity, unique component IDs, matching scopes, strictly positive edge basis, source/fetch ordering, maximum age, native-unit valuation coverage, and risk-bound sign. Forecast values are retained but excluded from `core_total`; only `NetEdgeQuote::core_net_edge` is exposed to admission.

- [ ] **Step 3: Implement explicit valuation routes**

```rust
pub struct ValuationRoute {
    pub route_id: ValuationRouteId,
    pub from_unit: NativeUnitId,
    pub to_currency: NativeUnitId,
    pub legs: Vec<ValuationLegEvidence>,
    pub valid_until_ns: u64,
}
```

Identity conversion succeeds only for exactly equal IDs. Route validation rejects cycles, disconnected orientation, stale legs, wrong terminal currency, and implicit stablecoin equivalence.

- [ ] **Step 4: Implement proportional health**

Required execution economics or valuation failure blocks admission. Forecast-only capability failure marks forecast unavailable without invalidating core edge. Slice 1 always reports actual-accounting capability disabled and therefore rejects live execution bindings.

- [ ] **Step 5: Commit pure quote behavior**

```bash
git add src/economics tests/economics_quote_contract.rs tests/economics_valuation_contract.rs
git commit -m "feat(#1445): validate economics quotes and net edge"
```

### Task 3: Config contract, provider adapters, and capability gates

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Create: `src/bolt_v3_providers/polymarket/economics.rs`
- Modify: `src/bolt_v3_providers/polymarket.rs`
- Create: `src/bolt_v3_providers/hyperliquid/economics.rs`
- Modify: `src/bolt_v3_providers/hyperliquid.rs`
- Test: `tests/polymarket_economics_contract.rs`
- Test: `tests/hyperliquid_economics_contract.rs`
- Test: `tests/economics_config_contract.rs`

**Interfaces:**
- Produces provider-local `PolymarketEconomicsConfig` and `HyperliquidEconomicsConfig` nested under each execution block.
- Produces `PolymarketEconomicsAdapter` and `HyperliquidEconomicsAdapter`, each implementing `VenueEconomicsAdapter` with shared types at the inward boundary.
- No provider SDK response type is public from either adapter.

- [ ] **Step 1: Add strict config and fixture tests**

Config tests require reporting policy, source freshness, quote refresh margin, holding horizon/carry policy where applicable, edge-basis resolver IDs, valuation routes, and `economics_slice = "quote_only"`. Unknown keys, zero ages, duplicate route authority, missing product capability, and any live-submit block fail validation.

- [ ] **Step 2: Implement Polymarket market-info parsing and Decimal formula**

The adapter consumes a provider-private captured market-info DTO containing `feesEnabled`, `fd.r`, `fd.e`, `fd.to`, and applicable attached routing fields. Fixtures cover fee-free, maker/taker applicability, multi-level execution, per-level venue rounding, unsupported exponents, missing fields, and NT schedule disagreement. The retired `/fee-rate` cache is not consulted.

- [ ] **Step 3: Implement complete Hyperliquid account/product parsing**

The adapter parses the complete provider-private `userFees` fixture needed for spot/perp, maker/taker, tiers, staking/referral modifiers, stable/aligned scaling, HIP-3/growth, and builder approval. Missing product/account fields fail; no value falls back to `userCrossRate` alone.

- [ ] **Step 4: Implement the observed aligned-status block**

The repository currently has no governed dated `alignedQuoteTokenInfo` capture. The Slice 1 adapter therefore reports `BlockedUnsupported::MissingGovernedAlignedStatusCapture` for every affected Hyperliquid surface, and configuration cannot activate it. A later capture may be added only with date, redaction statement, content hash, exact response-shape fixture, and source-fence registration; schema drift still blocks.

- [ ] **Step 5: Prove no live arming**

Add differential config tests showing the same offline fixture passes with live submission absent and fails when either Polymarket or Hyperliquid live submission is configured.

- [ ] **Step 6: Commit provider quote authority**

```bash
git add src/bolt_v3_config.rs src/bolt_v3_providers tests/polymarket_economics_contract.rs tests/hyperliquid_economics_contract.rs tests/economics_config_contract.rs
git commit -m "feat(#1445): add venue economics quote adapters"
```

### Task 4: NT/replay substrate mapping and synthetic extension contracts

**Files:**
- Create: `src/integrations/mod.rs`
- Create: `src/integrations/nautilus/mod.rs`
- Create: `src/integrations/nautilus/economics.rs`
- Modify: `src/lib.rs`
- Create: `crates/backtesting-vertical-slice/src/economics.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`
- Test: `tests/economics_nautilus_adapter.rs`
- Test: `tests/economics_extension_contract.rs`
- Test: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_economics.rs`

**Interfaces:**
- Produces: `canonical_quote_request_from_nt(NtEconomicsIntent<'_>) -> Result<EconomicQuoteRequest, NtEconomicsMappingError>`.
- Produces: replay mapping from manifest/historical snapshots to the same `EconomicQuoteRequest`.
- Shared core sees only canonical IDs, Decimal values, integer timestamps, and evidence origins.

- [ ] **Step 1: Add NT mapping tests**

Cover order side, liquidity role, exact planned fill legs, quote/base quantity, effective price, holding horizon, attached route context, and source timestamps. Unsupported NT order/product shapes fail before quote.

- [ ] **Step 2: Add synthetic non-NT substrate contract**

```rust
struct SyntheticSubstrate;

impl SyntheticSubstrate {
    fn canonical_request(&self) -> EconomicQuoteRequest {
        canonical_fixture_request("synthetic-order", "synthetic-product")
    }
}

#[test]
fn non_nt_substrate_uses_core_without_nt_or_venue_changes() {
    let quote = synthetic_venue().quote(&SyntheticSubstrate.canonical_request()).unwrap();
    assert!(quote.core_total().is_sign_negative());
}
```

- [ ] **Step 3: Add synthetic new-venue contract**

The test implements `VenueEconomicsAdapter` in the test crate, quotes a new venue using only shared types, and asserts no strategy, NT adapter, or core venue enum is required.

- [ ] **Step 4: Replace flat replay parameters with immutable economics snapshots**

Delete `PARAM_FEE_BPS` and `ManifestFeeProvider`. The manifest references a historical snapshot ID plus exact native components, source time, fetch time, validity, formula ID, edge-basis evidence, and valuation evidence. Missing snapshot or stale historical evidence fails manifest validation.

- [ ] **Step 5: Commit substrate isolation**

```bash
git add src/integrations src/lib.rs crates/backtesting-vertical-slice/src crates/backtesting-vertical-slice/tests tests/economics_nautilus_adapter.rs tests/economics_extension_contract.rs
git commit -m "feat(#1445): map NT and replay economics intents"
```

### Task 5: Composition root and atomic submit-admission cutover

**Files:**
- Create: `src/bolt_v3_economics_runtime.rs`
- Modify: `src/lib.rs`
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/bolt_v3_strategy_context.rs`
- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Test: `tests/bolt_v3_economics_runtime.rs`
- Modify: `tests/bolt_v3_provider_binding.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_submit_admission.rs`

**Interfaces:**
- Produces: `build_economics_runtime(loaded, execution_client_id, resolved) -> Result<Arc<BoltV3EconomicsRuntime>, EconomicsRuntimeBuildError>`.
- Produces: `BoltV3EconomicsRuntime::quote_admission(intent) -> Result<EconomicsAdmission, EconomicsUnavailable>`.
- `EconomicsAdmission` contains immutable quote ID, core net edge, fee-inclusive reservation notional, validity deadline, source snapshot IDs, and canonical evidence. It does not expose a venue adapter or strategy callback.

- [ ] **Step 1: Add ordering and fail-closed integration tests**

Assert quote evidence is durable before admission evaluation, admission precedes submit, forecast-only edge cannot admit, stale quote prevents submit, and any order price/quantity/route/role change requires a new quote ID.

- [ ] **Step 2: Replace provider registry construction**

Remove `FeeProvider`, `FeeProviderBuilder`, `FeeProviderResolutionError`, `resolve_fee_provider`, and `ProviderBinding::build_fee_provider`. Add one `build_economics_adapter` capability and reject active execution clients without a complete binding.

- [ ] **Step 3: Remove economics from strategy context**

`StrategyBuildContext::new` no longer accepts or stores a fee provider. `assemble_strategy_build_context` builds economics at the execution composition root and injects only the generic order-routing handle needed by shared execution; strategies cannot call quote providers.

- [ ] **Step 4: Replace the scalar admission callback**

Change `build_submit_admission_request_from_order` from `FnOnce(Decimal) -> Result<Decimal>` to a required `EconomicsAdmission`. The request uses the admission's validated reservation notional and quote evidence; callers cannot pass `max_fee_bps`.

- [ ] **Step 5: Route maker, taker, and forced-reduction orders through the same quote path**

Update every call in `bolt_v3_order_execution.rs`. Risk-reducing exits and kill-switch reductions may use a separately typed emergency economics policy only if TOML config supplies its debit bound and the same quote evidence is recorded; no numeric constant or zero shortcut is allowed.

- [ ] **Step 6: Commit the sole runtime path**

```bash
git add src/bolt_v3_economics_runtime.rs src/lib.rs src/bolt_v3_providers/mod.rs src/bolt_v3_strategy_context.rs src/bolt_v3_strategy_registration.rs src/bolt_v3_order_execution.rs src/bolt_v3_submit_admission.rs tests/bolt_v3_economics_runtime.rs tests/bolt_v3_provider_binding.rs tests/bolt_v3_strategy_registration.rs tests/bolt_v3_submit_admission.rs
git commit -m "feat(#1445): cut admission over to economics runtime"
```

### Task 6: Remove strategy/family/reporting duplicates

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/runtime_state.rs`
- Modify: relevant files under `src/strategies/binary_oracle_edge_taker/tests/`
- Modify: `src/bolt_v3_market_families/mod.rs`
- Modify: current family binding modules containing `maker_binary_fee_curve`
- Modify: `src/shadow_pnl.rs`
- Modify: `tests/shadow_pnl_report.rs`
- Modify: `tests/bolt_v3_strategy_substrate_structure.rs`
- Modify: `scripts/verify_economics_dependency_direction.py`

**Interfaces:**
- Strategies retain gross fair value, holding assumptions, and order intent only.
- Shadow reporting consumes immutable quote evidence for estimate attribution and refuses to claim actual P&L until Slices 2–5 close actual accounting.

- [ ] **Step 1: Delete edge-taker fee ownership**

Remove fee warming, `entry_fee_bps_at_price`, `max_entry_fee_bps_for_admission`, `outcome_fees`, `historical_entry_fee_bps`, `fee_rate_basis_points`, and strategy-local fee-adjusted expected edge. Replace evidence fields with quote ID, core net edge, forecast net edge, and source snapshot IDs emitted by shared execution.

- [ ] **Step 2: Delete family-owned fee formulas**

Remove `MarketFamilyValidationBinding::maker_binary_fee_curve`, `maker_binary_fee_curve_for_family`, all family implementations, and their fixtures. Family code retains market identity and fair-value behavior only.

- [ ] **Step 3: Delete flat shadow fee calculation**

Remove `notional * fee_bps / basis_points`. The Slice 1 report displays gross outcome plus the immutable estimated native/normalized quote components and an explicit `actual_economics_status = unavailable_until_actual_ledger`; it returns an error if asked for actual net P&L.

- [ ] **Step 4: Enforce removal and strategy isolation**

The static verifier rejects:

```text
FeeProvider
build_fee_provider
resolve_fee_provider
maker_binary_fee_curve
PARAM_FEE_BPS
ManifestFeeProvider
fee_inclusive_admission_notional
```

outside historical documentation. It also rejects `economics::ports::VenueEconomicsAdapter`, formula identifiers, or Decimal fee math under strategy/archetype paths.

- [ ] **Step 5: Commit duplicate-path removal**

```bash
git add src/strategies src/bolt_v3_market_families src/shadow_pnl.rs tests scripts/verify_economics_dependency_direction.py
git commit -m "refactor(#1445): remove duplicate fee and reporting paths"
```

### Task 7: Verification, exact-head evidence, and review-ready handoff

**Files:**
- Modify only if required by governed verifiers: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
- Verify: all Slice 1 changed files

**Interfaces:**
- Produces no new runtime behavior; proves the implemented branch matches #1445 and the frozen design.

- [ ] **Step 1: Run targeted text/static checks**

```bash
git diff --check
python3 scripts/test_verify_economics_dependency_direction.py
python3 scripts/verify_economics_dependency_direction.py
rg -n 'FeeProvider|build_fee_provider|resolve_fee_provider|maker_binary_fee_curve|ManifestFeeProvider|PARAM_FEE_BPS' src tests crates
```

Expected: diff check and both verifiers pass; removal search returns no production/test/runtime matches except the verifier's deny-list fixtures.

- [ ] **Step 2: Run permitted broad local gates**

```bash
just fmt-check
just deny
just ci-lint-workflow
just source-fence-static
```

Expected: all commands exit 0. Do not invoke raw Cargo build/test/clippy locally.

- [ ] **Step 3: Self-review the full branch against the requirement matrix**

For every matrix row, cite the exact test, static fence, or removal output. Treat missing evidence as incomplete implementation. Confirm live submission is rejected for both venues and the branch contains no Slice 2 actual-ledger implementation.

- [ ] **Step 4: Commit final verifier synchronization and confirm cleanliness**

```bash
git add docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml
git commit -m "test(#1445): close economics cutover evidence"
git status --short
```

Expected: no output from `git status --short`.

- [ ] **Step 5: Publish through the sandbox-safe path and obtain remote evidence**

```bash
just sandbox-safe-push
gh pr create --draft --title "#1445: atomic economics quote/admission cutover" --body-file /private/tmp/bolt-v2-1445-pr.md
just verify-remote
```

Before remote dispatch, run `just rust-probe suggest` and use at most the two smallest-sufficient probes only if cheap checks cannot identify a compiler/test failure. Record the exact branch HEAD and remote HEAD. Do not mark ready, request external review, merge, deploy, or arm live execution until all repository prerequisites are satisfied.

## Plan Self-Review Record

- Spec coverage: all Slice 1 decision-time requirements map to Tasks 1–7; Slices 2–5 are explicitly isolated and blocked on separate issues.
- Placeholder scan: the plan contains no deferred implementation instruction inside Slice 1; the missing future issue numbers are explicit authorization blocks.
- Type consistency: venue and substrate adapters exchange only `EconomicQuoteRequest`, `EstimatedEconomicComponent`, and `EconomicsUnavailable`; admission consumes `EconomicsAdmission`; actual entries are reserved for later slices and cannot be constructed from estimates.
- Production-path audit: provider registry, strategy context, order execution, strategy math, family math, replay fees, and shadow reporting are all named deletion/migration surfaces.
- Live-state audit: Tasks 2, 3, 5, and 7 each independently prove Polymarket and Hyperliquid live execution remains disabled.
