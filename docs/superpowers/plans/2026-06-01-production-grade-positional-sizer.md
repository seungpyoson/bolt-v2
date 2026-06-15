# Production-Grade Positional Sizer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a production-grade NT-first positional sizing and admission layer that prevents Bolt from admitting risk beyond configured capital, current reservations, market-specific worst-case liability, and configured loss limits.

**Architecture:** Strategies continue to produce intent only. A shared Bolt admission layer sizes or rejects intent using TOML policy, NT-derived account/portfolio/order/position facts, a Bolt-owned reservation ledger, a Bolt-owned loss governor, and product-specific worst-case-liability calculators before NT submit. NT remains the authority for portfolio/account truth, order lifecycle, cache, adapters, execution, and RiskEngine trading state.

**Tech Stack:** Rust, existing Bolt v3 modules, NautilusTrader Rust crates pinned by `Cargo.toml`, TOML config, `rust_decimal`, `cargo test --locked`, GitHub CI as final broad verifier.

---

## Current State

- Current worktree branch: `codex/nt-loss-governor-circuit-breaker`.
- Current local loss-governor code is a pure policy module only. `cargo test --locked --lib bolt_v3_loss_governor` passed on 2026-06-01 in this worktree with seven focused tests, but the module is not wired into submit admission or live trading.
- Current branch has a loss-governor spec under `specs/505-nt-loss-governor/`, and its task file marks the pure loss-governor slice complete.
- Separate worktree branch `codex/nt-capital-reservation-ledger` is ahead of `origin/main` by three commits and contains a candidate pure reservation-ledger implementation in `src/bolt_v3_capital_reservation.rs`. Treat that branch as source material, not production completion.
- No current code proves production-grade positional sizing. There is no approved full sizer plan, no submit-admission integration, no exact-head CI, and no external approval for the combined design.

## Production Invariants

1. Never admit a new order intent if worst-case live commitments plus the new commitment can exceed its configured capital pool.
2. Never admit a new order intent if configured per-trade, daily, rolling-window, or drawdown loss policy is breached or cannot be freshly proven from NT-derived facts.
3. Never use Bolt as a second account, portfolio, fill, PnL, margin, liquidation, order-lifecycle, or venue truth source.
4. Never let strategy modules own execution admissibility, venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, submit gating, reservation, or loss stops.
5. Never silently change strategy intent unless TOML explicitly enables an auditable sizing mode. The first production mode rejects over-budget intent rather than clipping size.
6. Fail closed when product calculator, market metadata, account state, lifecycle state, allowance, inventory, timestamp freshness, or attribution evidence is missing.
7. Keep product-specific liability logic behind calculators. The shared sizer must not encode Polymarket, futures, options, or strategy-specific rules.
8. Keep venue/account/instrument-type blast-radius boundaries as configured capital pools. Separate venue accounts are operationally preferred, but code must enforce configured pools even when one account contains multiple instruments.

## What NT Provides

Use NT for:

- Portfolio/account balances, margins, realized PnL, unrealized PnL, total PnL, and equity.
- Position events and order lifecycle events.
- Portfolio snapshot publication/subscription.
- RiskEngine submit/modify enforcement and `TradingState::{Active,Halted,Reducing}`.
- Adapter, cache, execution, and order ownership.

Bolt must own:

- Configured capital pools.
- Reservation semantics for committed-but-unfilled capital.
- Product-specific worst-case-liability calculators.
- Snapshot freshness, attribution, and fail-closed evidence.
- Loss-governor thresholds and admission reasons.
- The composition step that turns strategy intent into accepted/rejected sized admission before NT submit.

## Target File Structure

### Existing Or Planned Shared Modules

- `src/bolt_v3_capital_reservation.rs`
  - Shared reservation ledger core.
  - Product-neutral pool accounting.
  - Prediction-market calculator as first product calculator.
  - Lifecycle event reconciliation for reserve, release, revalue, invalidate.

- `src/bolt_v3_loss_governor.rs`
  - Shared pure loss-admission evaluator.
  - Rejects missing/stale/unattributed loss facts.
  - Rejects configured per-trade, daily, rolling-window, and max-drawdown breaches.

- `src/bolt_v3_position_sizer.rs`
  - New shared composition layer.
  - Takes strategy order intent plus NT-derived state bundle.
  - Calls product calculator, reservation ledger, loss governor, and static sizing policy.
  - Returns `SizedAdmissionDecision`.
  - Does not submit orders.

- `src/bolt_v3_config.rs`
  - Add TOML-owned capital-pool, sizing, snapshot-freshness, and loss-governor config structures.
  - No runtime default thresholds.

- `src/bolt_v3_submit_admission.rs`
  - Integration only after PR #480 is settled or explicit integration approval is given.
  - Calls `bolt_v3_position_sizer` before NT submit.

- `src/bolt_v3_live_node.rs`
  - Integration only after PR #480 is settled or explicit integration approval is given.
  - Wires NT-derived facts into the sizer state bundle.

### Files That Must Stay Strategy-Local Only

- `src/strategies/*`
  - May emit order intent and strategy-local signal state only.
  - Must not gain sizing, reservation, loss-governor, execution, venue, or submit-gating logic.

## Public Interfaces To Build

These interfaces are the intended public shape. Names can change during TDD only if the plan is updated before implementation continues.

```rust
pub struct SizingPolicy {
    pub min_remaining_pool_balance: Option<Decimal>,
    pub fee_slippage_policy: Option<FeeSlippagePolicy>,
}

pub struct FeeSlippagePolicy {
    pub max_fee_liability: Decimal,
    pub max_slippage_liability: Decimal,
}

pub struct PositionSizingRequest {
    pub intent_id: String,
    pub strategy_id: String,
    pub instrument_id: String,
    pub pool_id: String,
    pub product_kind: ProductKind,
    pub side: IntentSide,
    pub quantity: Decimal,
    pub limit_price: Decimal,
    pub order_kind: IntentOrderKind,
    pub liquidity: IntentLiquidity,
    pub quote_set_id: Option<String>,
    pub now_ns: u64,
}

pub enum IntentSide {
    Buy,
    Sell,
}

pub enum IntentOrderKind {
    Limit,
}

pub enum IntentLiquidity {
    Taker,
    RestingMaker,
}

pub enum ProductKind {
    PredictionMarketBinary,
}

pub struct CapitalPoolConfig {
    pub pool_id: String,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub max_pool_liability: Decimal,
    pub sizing_policy: SizingPolicy,
}

pub struct NtDerivedSizingState {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioSizingSnapshot,
    pub order_lifecycle: OrderLifecycleSizingSnapshot,
    pub product_state: ProductSizingSnapshot,
    pub reservation_snapshot: ReservationLedgerSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

pub struct PortfolioSizingSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub free_collateral: Decimal,
    pub total_equity: Decimal,
}

pub struct OrderLifecycleSizingSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub open_order_count: usize,
    pub all_open_orders_attributed: bool,
}

pub enum ProductSizingSnapshot {
    PredictionMarketBinary(PredictionMarketSizingSnapshot),
}

pub struct PredictionMarketSizingSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub yes_position: Decimal,
    pub no_position: Decimal,
    pub collateral_allowance: Decimal,
    pub conditional_token_allowance: Decimal,
    pub collateral_coupled_group_id: String,
}

pub struct ReservationLedgerSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub all_live_reservations_attributed: bool,
}

pub trait WorstCaseLiabilityCalculator {
    fn product_kind(&self) -> ProductKind;
    fn worst_case_liability(
        &self,
        request: &PositionSizingRequest,
        state: &ProductSizingSnapshot,
        policy: &SizingPolicy,
    ) -> Result<LiabilityQuote, LiabilityError>;
}

pub struct LiabilityQuote {
    pub original_quantity: Decimal,
    pub sized_quantity: Decimal,
    pub liability_before_sizing: Decimal,
    pub liability_after_sizing: Decimal,
    pub evidence_label: String,
}

pub enum LiabilityError {
    MissingMarketState,
    MissingFeePolicy,
    MissingSlippagePolicy,
    InvalidIntentPrice,
    InvalidIntentQuantity,
    MissingLiquidityDiscriminator,
    InsufficientAllowance,
    InsufficientInventory,
}

pub enum ReservationRejectionReason {
    OverBudget,
    StaleRequest,
    MissingEvidence,
    MissingCalculator,
    MissingLifecycleState,
    MissingMarketMetadata,
    AttributionFailed,
    CollateralGroupSplitRejected,
    DuplicateRelease,
    UnknownRelease,
    EventOutOfOrder,
}

pub enum SizedAdmissionReason {
    Loss(LossHaltReason),
    Reservation(ReservationRejectionReason),
    Liability(LiabilityError),
    MissingNtState,
    UnsupportedProduct,
}

pub enum SizingEvidenceKind {
    Portfolio,
    OrderLifecycle,
    ProductState,
    ReservationLedger,
    LossSnapshot,
    LiabilityCalculator,
}

pub struct SizingEvidenceSource {
    pub kind: SizingEvidenceKind,
    pub source: String,
    pub observed_at_ns: u64,
}

pub struct SizedAdmissionEvidence {
    pub sources: Vec<SizingEvidenceSource>,
    pub original_quantity: Decimal,
    pub sized_quantity: Option<Decimal>,
    pub liability_before_sizing: Option<Decimal>,
    pub liability_after_sizing: Option<Decimal>,
}

pub struct SizedAdmissionDecision {
    pub accepted: bool,
    pub original_quantity: Decimal,
    pub sized_quantity: Option<Decimal>,
    pub liability_before_sizing: Option<Decimal>,
    pub liability_after_sizing: Option<Decimal>,
    pub pool_id: String,
    pub evidence: SizedAdmissionEvidence,
    pub reasons: Vec<SizedAdmissionReason>,
}
```

### Prediction-Market Liability Formula

For the first calculator, `ProductKind::PredictionMarketBinary`, the plan pins the binary liability formula so the shared sizer does not re-derive product math:

- BUY liability before fees/slippage: `quantity * limit_price`.
- SELL liability before fees/slippage: `quantity * (1 - limit_price)`.
- BUY requires fresh pUSD balance and allowance evidence.
- SELL requires fresh YES/NO inventory and conditional-token allowance evidence.
- `limit_price` must be in the closed interval `[0, 1]`; invalid prices reject.
- `quantity` must be positive after NT instrument metadata rounding; missing instrument metadata rejects before liability calculation.
- Fees and slippage are explicit TOML policy inputs carried by `SizingPolicy::fee_slippage_policy`. If fee or slippage policy is missing for a live-submit profile, the calculator rejects with `MissingFeePolicy` or `MissingSlippagePolicy` rather than assuming zero.
- `IntentLiquidity::Taker` means one-shot liability for the proposed order. `IntentLiquidity::RestingMaker` means the order can rest and must be evaluated with other live quotes in the same `quote_set_id` and collateral-coupled group.
- `IntentOrderKind::Limit` is the first supported order kind. Market, IOC, FOK, GTD, and other order kinds reject until a separate approved slice defines their liability semantics.
- Resting maker quote sets reserve simultaneous adverse-fill liability for all live quotes inside the same collateral-coupled group. Missing `quote_set_id` for `RestingMaker` rejects.
- If a configured loss policy is enabled and `loss_snapshot` is `None`, the sizer must call the loss governor and reject; callers must not bypass loss checks by omitting the snapshot.

## Task 1: Freeze Plan Approval Gate

**Files:**
- Create or modify: `docs/superpowers/plans/2026-06-01-production-grade-positional-sizer.md`

- [ ] **Step 1: Confirm no more production code is changed before approval**

Run:

```bash
git status --short --branch
cargo test --locked --lib bolt_v3_loss_governor
```

Expected:

- Branch status shows the current local implementation state.
- Focused loss-governor tests pass if the current pure slice is still intact.

- [ ] **Step 2: Run Claude adversarial review on this plan**

Run custom review with explicit plan and evidence paths:

```bash
CLAUDE_COMPANION="$HOME/.codex/plugins/cache/codex-plugin-multi/claude/0.1.0/scripts/claude-companion.mjs"
node "$CLAUDE_COMPANION" run \
  --mode=custom-review \
  --auth-mode subscription \
  --foreground \
  --lifecycle-events markdown \
  --cwd "$PWD" \
  --scope-paths docs/superpowers/plans/2026-06-01-production-grade-positional-sizer.md,specs/505-nt-loss-governor/spec.md,specs/505-nt-loss-governor/plan.md,specs/505-nt-loss-governor/research.md,specs/505-nt-loss-governor/data-model.md,specs/505-nt-loss-governor/contracts/loss-governor.md,specs/505-nt-loss-governor/tasks.md,src/bolt_v3_loss_governor.rs,src/lib.rs \
  -- "Adversarially review whether this plan is sufficient to reach a production-grade NT-first positional sizer. Return APPROVED only if the plan is implementation-ready. Otherwise return FLAWED with blocking corrections. Focus on NT ownership boundaries, risk of duplicating account truth, reservation/loss-governor composition, PR #480 integration risk, strategy-intent boundary, config/no-hardcode compliance, and missing production-grade requirements."
```

Expected:

- Claude returns `APPROVED`, or returns blocking findings that are addressed before any more implementation.

## Task 2: Harden Pure Loss-Governor Contract

**Files:**
- Modify: `src/bolt_v3_loss_governor.rs`
- Modify: `specs/505-nt-loss-governor/tasks.md`
- Modify: `specs/505-nt-loss-governor/contracts/loss-governor.md`

- [ ] **Step 1: RED equality-threshold semantics**

Add public API tests proving equality is a breach for per-trade, daily, rolling, and max-drawdown thresholds.

Run:

```bash
cargo test --locked --lib bolt_v3_loss_governor::tests::loss_threshold_equality_rejects_admission
```

Expected:

- FAIL before equality semantics are pinned or PASS if the current implementation already uses `>=` for every threshold. If it passes, record it as an existing green behavior and do not change production code.

- [ ] **Step 2: GREEN equality semantics**

If Step 1 is red, implement equality rejection with `loss >= limit` and `drawdown >= limit`. If Step 1 is already green, update only the contract.

- [ ] **Step 3: Update loss-governor contract**

State explicitly that configured threshold equality rejects admission. Missing snapshot, missing source, stale timestamp, future timestamp, and missing configured fields all return `stale_loss_snapshot`.

- [ ] **Step 4: Verify pure loss governor**

Run:

```bash
cargo test --locked --lib bolt_v3_loss_governor
```

Expected:

- PASS.

## Task 3: Add Strategy-Boundary Source Fence Early

**Files:**
- Modify existing source-fence verifier files if this repo has an established verifier for strategy boundaries.
- Modify: `justfile` or existing verifier command only if that is the established local pattern.

- [ ] **Step 1: Locate existing source-fence pattern**

Run:

```bash
rg -n "source-fence|strategy|submit|bolt_v3_submit_admission|bolt_v3_order_intent" justfile scripts src tests
```

Expected:

- Existing verifier location is identified before adding a new guard.

- [ ] **Step 2: RED strategies cannot own sizing or admission**

Add a source-fence case proving `src/strategies/*` cannot import or call `bolt_v3_capital_reservation`, `bolt_v3_loss_governor`, `bolt_v3_position_sizer`, submit admission internals, NT execution submit calls, venue rules, fillability, rounding, or minimum-size logic.

Run:

```bash
just source-fence
```

Expected:

- FAIL before the fence knows the new forbidden modules.

- [ ] **Step 3: GREEN strategy boundary fence**

Update the established source-fence verifier so strategies remain intent-only.

- [ ] **Step 4: Verify strategy boundary**

Run:

```bash
just source-fence
```

Expected:

- PASS.

## Task 4: Bring Capital Reservation Onto The Approved Path

**Files:**
- Source material only: `.worktrees/nt-capital-reservation-ledger/src/bolt_v3_capital_reservation.rs`
- Create or modify in current approved branch only after approval: `src/bolt_v3_capital_reservation.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Review the existing capital branch as source material**

Run:

```bash
CAPITAL_WORKTREE="$(git worktree list --porcelain | awk '/^worktree /{path=$2} /^branch refs\/heads\/codex\/nt-capital-reservation-ledger$/{print path}')"
test -n "$CAPITAL_WORKTREE"
git -C "$CAPITAL_WORKTREE" status --short --branch
git -C "$CAPITAL_WORKTREE" log --oneline --decorate -5
cargo test --locked --manifest-path "$CAPITAL_WORKTREE/Cargo.toml" --lib capital_reservation
```

Expected:

- Worktree is clean.
- Branch has the candidate reservation commits.
- Focused tests pass in that worktree before code is ported or reused.

- [ ] **Step 2: Decide port versus PR sequencing**

Use one of these two approved paths:

- If the capital-reservation branch is already reviewed and acceptable, make it its own PR and do not duplicate it on this branch.
- If this branch is the approved sizer branch, port only the reviewed source and docs needed for the reservation module, then re-run its tests.

- [ ] **Step 3: Verify product-neutral reservation behavior**

Run:

```bash
cargo test --locked --lib capital_reservation
```

Expected:

- Accepted reservations, over-budget rejection, missing evidence rejection, stale request rejection, existing live reservation aggregation, exact-full-budget behavior, and compact evidence tests pass.

## Task 5: Add NT-Derived State And Product Calculator Contracts

**Files:**
- Create: `src/bolt_v3_position_sizer.rs`
- Create: `src/bolt_v3_sizing_state.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: RED stale NT-derived state bundle fails closed**

Add `sizing_state_missing_nt_snapshot_fails_closed`.

Run:

```bash
cargo test --locked --lib bolt_v3_sizing_state::tests::sizing_state_missing_nt_snapshot_fails_closed
```

Expected:

- FAIL before state boundary exists.

- [ ] **Step 2: GREEN typed state bundle**

Create a typed state bundle matching the public interface in this plan. It carries NT-derived portfolio/account/order/position/product facts with source attribution and timestamps. It must not query venues or construct independent account truth.

- [ ] **Step 3: RED unattributed state transition fails**

Add a test proving unattributed balance, allowance, inventory, open-order, or position changes invalidate the sizing state.

Run:

```bash
cargo test --locked --lib bolt_v3_sizing_state::tests::unattributed_state_transition_fails_closed
```

Expected:

- FAIL before state invalidation exists.

- [ ] **Step 4: GREEN state invalidation**

Implement fail-closed invalidation for unattributed changes.

- [ ] **Step 5: RED binary prediction liability formula**

Add tests for BUY liability `quantity * limit_price`, SELL liability `quantity * (1 - limit_price)`, invalid price rejection, missing allowance rejection, missing inventory rejection, missing fee/slippage policy rejection, missing resting-maker `quote_set_id` rejection, and taker versus resting-maker liability separation.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::prediction_market_binary_liability_formula_is_pinned
```

Expected:

- FAIL before the calculator contract and formula exist.

- [ ] **Step 6: GREEN binary prediction liability calculator**

Implement `WorstCaseLiabilityCalculator` for `ProductKind::PredictionMarketBinary` with the formula pinned above. The calculator returns `LiabilityQuote` and does not read account state outside `ProductSizingSnapshot`.

## Task 6: Add Position-Sizer Composition Layer

**Files:**
- Modify: `src/bolt_v3_position_sizer.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: RED reject when loss governor rejects**

Add a public API test named `sizer_rejects_when_loss_governor_rejects`.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::sizer_rejects_when_loss_governor_rejects
```

Expected:

- FAIL because composition does not exist.

- [ ] **Step 2: GREEN minimal loss composition**

Create `evaluate_position_sizing` so a loss-governor rejection returns `SizedAdmissionDecision { accepted: false, reasons: vec![SizedAdmissionReason::Loss(...)] }`.

- [ ] **Step 3: RED reject when reservation ledger rejects**

Add `sizer_rejects_when_capital_reservation_rejects`.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::sizer_rejects_when_capital_reservation_rejects
```

Expected:

- FAIL before reservation composition exists.

- [ ] **Step 4: GREEN reservation composition**

Call the reservation ledger after liability calculation and loss policy. Preserve deterministic reason ordering: loss reasons first, liability reasons second, reservation reasons third.

- [ ] **Step 5: RED accepted path returns combined evidence**

Add `sizer_accepts_when_loss_liability_and_reservation_pass`.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::sizer_accepts_when_loss_liability_and_reservation_pass
```

Expected:

- FAIL before happy-path evidence composition exists.

- [ ] **Step 6: GREEN accepted path**

Return accepted decision with original quantity, sized quantity, liability before sizing, liability after sizing, pool id, and structured `SizedAdmissionEvidence` sources for portfolio, order lifecycle, product state, reservation ledger, loss snapshot when configured, and liability calculator.

- [ ] **Step 7: RED reject-only mode does not clip strategy intent**

Add `reject_only_mode_does_not_silently_clip_order_size`.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::reject_only_mode_does_not_silently_clip_order_size
```

Expected:

- FAIL before sizing-mode handling exists.

- [ ] **Step 8: GREEN pool reservation rejects over-budget sizing**

Keep over-budget handling in the capital-pool reservation ledger. Per-instrument order notional caps belong in NautilusTrader `LiveRiskEngineConfig.max_notional_per_order`, not in a Bolt sizing-mode or per-order-liability knob.

## Task 7: Add TOML Config Binding And Validation

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify fixtures under `tests/fixtures/bolt_v3/` only as required by existing config tests.

- [ ] **Step 1: RED config requires explicit pools**

Add a config validation test proving missing capital pool config fails for any strategy profile that enables live submit sizing.

Run:

```bash
cargo test --locked --lib bolt_v3_config
```

Expected:

- FAIL before config schema exists.

- [ ] **Step 2: GREEN config structures**

Add TOML-owned config structs for capital pools, per-pool sizing policy, sizing mode, max snapshot age, max order liability, min remaining pool balance, binary fee/slippage policy, and loss thresholds. Do not add runtime threshold defaults.

- [ ] **Step 3: RED invalid thresholds fail**

Add tests for zero/negative/NaN-equivalent, missing pool id, missing product kind, and duplicate pool ids.

- [ ] **Step 4: GREEN config validation**

Implement validation errors with stable reason names. Do not print secrets or live credential values.

## Task 8: Add Restart Reconciliation And TOCTOU Guard

**Files:**
- Modify: `src/bolt_v3_capital_reservation.rs`
- Modify: `src/bolt_v3_position_sizer.rs`
- Modify: `src/lib.rs`

- [x] **Step 1: RED startup stays closed until NT reconciliation completes**

Add `restart_requires_rebuilt_open_order_reservations_before_admission`.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::restart_requires_rebuilt_open_order_reservations_before_admission
```

Expected:

- FAIL before restart reconciliation state exists.

- [x] **Step 2: GREEN restart fail-closed state**

On process start, sizing admission remains closed until NT-derived open orders, positions, portfolio/account snapshots, and reservation reconstruction evidence are all fresh and attributed. Unsubmitted pre-crash reservations are discarded; submitted/open order reservations are reconstructed from NT open-order/cache evidence.

- [x] **Step 3: RED concurrent reserve-submit path is serialized**

Add `reserve_to_submit_is_single_serialized_critical_section`.

Run:

```bash
cargo test --locked --lib bolt_v3_position_sizer::tests::reserve_to_submit_is_single_serialized_critical_section
```

Expected:

- FAIL before serialization is modeled.

- [x] **Step 4: GREEN serialized critical section**

Use one actor, one mutex-guarded ledger, or a `&mut self` admission gate so `evaluate -> reserve -> submit handoff` is indivisible for a given pool. If NT submit rejects before venue acceptance, release the pending reservation with matched evidence.

## Task 9: Integrate With Submit Admission After PR #480 Boundary Is Clear

**PR #480 settlement predicate:** PR #480 is settled only when it is merged into `origin/main`, or it is closed and explicitly declared obsolete, and this branch has rebased onto the resulting `origin/main`. If PR #480 remains open or its owned files still conflict, integration tasks do not start.

**Files:**
- Modify only after explicit approval or PR #480 settlement:
  - `src/bolt_v3_submit_admission.rs`
  - `src/bolt_v3_live_node.rs`
- Do not modify:
  - `src/strategies/*` except tests proving strategies remain intent-only.

- [ ] **Step 1: Confirm integration base**

Run:

```bash
git status --short --branch
gh pr view 480 --json state,headRefOid,baseRefOid,mergeStateStatus,url
```

Expected:

- PR #480 is merged or closed obsolete and the branch is rebased onto the resulting `origin/main`. If not, stop integration and continue pure-module verification only.

- [ ] **Step 2: RED submit path calls sizer before NT submit**

Add a test that proves a rejected sizing decision prevents NT submit and records rejection evidence through the existing decision-evidence channel.

- [ ] **Step 3: GREEN submit path integration**

Wire the sizer into the single submit-admission path before NT submit. Do not add alternate submit routes.

- [ ] **Step 4: RED strategy boundary still holds after integration**

Run:

```bash
just source-fence
```

Expected:

- PASS.

## Task 10: Add Halt And Reducing Mode Routing

**Files:**
- Modify integration files only after Task 9 is complete.

- [ ] **Step 1: RED loss halt maps to NT trading-state request**

Add a test proving a configured loss halt produces an NT `TradingState::Halted` or `TradingState::Reducing` request instead of a venue-specific cancel/flatten call.

- [ ] **Step 2: GREEN NT-routed halt action**

Route halt/reducing intent through NT-owned controls. Keep cancel/flatten disabled unless TOML explicitly enables the action and the NT route is proven.

- [ ] **Step 3: RED no cancel/flatten by default**

Add a test proving default loss halt rejects new risk but does not cancel or flatten.

- [ ] **Step 4: GREEN default halt behavior**

Keep default behavior admission-only and evidence-recorded.

## Task 11: Verification, Reviews, And PR Discipline

**Files:**
- Modify docs and issue comments only after code verification.

- [ ] **Step 1: Local focused verification**

Run:

```bash
cargo fmt --check
cargo test --locked --lib bolt_v3_loss_governor
cargo test --locked --lib capital_reservation
cargo test --locked --lib bolt_v3_position_sizer
git diff --check
```

Expected:

- All focused checks pass.

- [ ] **Step 2: Branch scope audit**

Run:

```bash
git diff --name-only origin/main...HEAD
git diff --name-only origin/main...HEAD -- src/bolt_v3_submit_admission.rs src/bolt_v3_live_node.rs src/strategies/binary_oracle_edge_taker.rs
```

Expected:

- Changed files match the approved slice.
- PR #480 integration files are unchanged unless Task 9 was explicitly approved.

- [ ] **Step 3: Commit and push only a meaningful slice**

Run:

```bash
git add docs/superpowers/plans/2026-06-01-production-grade-positional-sizer.md specs/505-nt-loss-governor src/lib.rs src/bolt_v3_loss_governor.rs src/bolt_v3_capital_reservation.rs src/bolt_v3_position_sizer.rs src/bolt_v3_sizing_state.rs src/bolt_v3_config.rs
git commit -m "feat: add NT-first positional sizing slice"
git push -u origin "$(git branch --show-current)"
```

Expected:

- One reviewable slice is pushed.

- [ ] **Step 4: Use CI as broad source of truth**

Run:

```bash
gh pr checks "$(gh pr view --json number -q .number)" --watch
```

Expected:

- Exact-head CI passes before external implementation review is requested.

- [ ] **Step 5: External implementation reviews**

Request Claude first, then Gemini and GLM when practical. Grok, Kimi, and DeepSeek can be used when available, but the work does not wait indefinitely on weaker or unavailable reviewers.

Expected:

- No blocking findings remain.
- Any non-blocking findings are recorded with disposition and issue tracking.

## Production-Grade Definition

This goal is production-grade only when all of the following are true and verified on current head:

- Shared reservation ledger is present, tested, and integrated before NT submit.
- Loss governor is present, tested, and integrated before NT submit.
- Product-specific prediction-market liability calculator is present and tested.
- Sizer composition layer rejects over-budget and over-loss intent before NT submit.
- TOML config owns every runtime threshold and sizing mode.
- NT remains source of account, portfolio, order, position, execution, adapter, and RiskEngine truth.
- Strategies remain intent-only.
- Restart/lifecycle reconciliation is tested for open reservations and unattributed state changes.
- PR #480-owned integration files are either untouched or explicitly integrated after the PR #480 boundary is clear.
- Focused tests, `cargo fmt --check`, `git diff --check`, exact-head CI, and external reviews pass.
- The PR text states exactly which slice is complete and which production-grade surfaces remain outside that PR.

## Self-Review

- Spec coverage: This plan covers capital reservation, loss governor, product calculator, composition, config, NT-derived state, submit integration, halt routing, verification, and external review.
- Gap scan: This plan names each required task, file boundary, command, and expected result.
- Type consistency: Public interface names are declared in this plan before task references. Implementation may rename them only by updating this plan before coding continues.
