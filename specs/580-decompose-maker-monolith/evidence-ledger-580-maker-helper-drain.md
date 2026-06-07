# #580 Pure Maker Helper Drain Evidence Ledger

Base worktree: `codex/580-maker-helper-drain`
Final base commit: `2c1c4fdbbb0f920c8aa2c052576b7c82a303d38a` (`main == origin/main == HEAD`
before local changes)

Initial verification started from `a1010e7101a8963c04ca71eb16f8ee2c55309857`. During final verification,
`origin/main` advanced to `2c1c4fdbbb0f920c8aa2c052576b7c82a303d38a` (`docs: clarify Chainlink strike
subscription`); the worktree was fast-forwarded before final verification.

## Current-main Evidence

- Present on current main:
  - `src/bolt_v3_quote_lifecycle.rs`
  - `src/bolt_v3_maker_model.rs`
  - `src/bolt_v3_maker_microprice.rs`
  - `src/bolt_v3_trade_flow.rs`
  - `src/bolt_v3_numeric.rs` with `TWO_F64`
- Missing on current main:
  - `src/bolt_v3_requote_budget.rs`
  - `src/bolt_v3_quoting.rs`
  - `src/bolt_v3_maker_inventory.rs`
  - `HALF_F64` and `sanitize_open_probability` in `src/bolt_v3_numeric.rs`
  - shared `resolve_band`, `compose_binary_legs`, and quote target value types
- Existing source-fence scan found no `MakerFamily`, `dyn MakerFamily`, `HALF_F64`,
  `sanitize_open_probability`, `resolve_band`, `compose_binary_legs`, `QuoteSide`, or `QuoteTargets` on
  current main.

## Stale Source Refs

- #514 / `origin/feat/488-w2-lifecycle`: `09179f6682c13a5d2c86c4dab03919d4393dfd79`
- #515 / `origin/feat/488-w3-pricing`: `71e5e25634f3c26b6911454ce71cdaa8dfb47de8`
- Accepted requote-budget superset checked per #580 plan:
  `origin/feat/488-w4-settlement` at `2dee3ed3a0f68369ae062c262eab50ffc7f6bb2d`
- Later primitive reference for closure matrix only:
  `origin/feat/488-w5-w7-primitives` at `96e106032d835535132bd43ce3560bc9f11e644e`

## #580 Ordering Constraints

- Work is a port from stale sources onto fresh main, not a merge or rebase.
- Shared helpers must be flat `src/bolt_v3_*.rs` modules.
- TDD is required for every production behavior change.
- New numeric symbols travel with their first consumer to avoid `dead_code` under `-D warnings`.
- `bolt_v3_requote_budget` ports the accepted cost-weighted model.
- `bolt_v3_quoting` relocates agnostic scalar math and adds `HALF_F64` plus
  `sanitize_open_probability` when first used.
- `MakerFamily` / `&dyn MakerFamily` is rejected; family write-side behavior must fold into the canonical
  `MarketFamilyValidationBinding` fn-pointer table if implemented.

## Rejections

- The stale `MakerFamily` trait and `&dyn MakerFamily` tests in `maker_quote.rs` are rejected by
  `specs/580-decompose-maker-monolith/spec.md` FR-004/FR-005 and the repo NO-DUAL-PATHS rule.
- The stale source warning about a count-only requote-budget variant was rechecked in this workspace:
  `origin/feat/488-w2-lifecycle` and `origin/feat/488-w4-settlement` both currently expose the
  cost-weighted `try_acquire(now_ms, cost)` API with `cost_in_window()`.

## Baseline Verification

- `cargo test --locked bolt_v3_maker_model::tests --lib`: passed, 11 tests.

## Session Results

Landed:

- `src/bolt_v3_requote_budget.rs`: cost-weighted sliding-window requote budget with explicit constructor,
  min-interval enforcement, monotonic timestamp enforcement, fail-closed zero/oversize cost behavior, and
  focused tests.
- `src/bolt_v3_quoting.rs`: shared quote value types, `resolve_band`, `compose_binary_legs`,
  `time_widening_factor`, and `reward_shaping_offset`; no strategy imports and no family trait seam.
- `src/bolt_v3_numeric.rs`: `HALF_F64` and strict `sanitize_open_probability`, introduced with quoting as
  first consumer.
- `src/bolt_v3_maker_inventory.rs`: pure fill accumulator over shared `Leg` and `QuoteSide`.
- `src/bolt_v3_market_families/{mod.rs,updown.rs}`: maker quote targets, 0/1 settlement payout, and binary
  fee curve folded into the canonical `MarketFamilyValidationBinding` fn-pointer table.

Intentionally skipped:

- Maker strategy shell, live submit/admission, kill switch, circuit breaker, positional sizing, spendability,
  reservation, forced exit, and market-exit behavior.
- The rejected `MakerFamily` / `&dyn MakerFamily` stale seam.
- Broader W5/W7 reward accrual and reservation modules.

## TDD Evidence

- Requote RED: `cargo test --locked bolt_v3_requote_budget::tests --lib` failed with missing
  `RequoteBudget`.
- Requote GREEN: same command passed, 7 tests.
- Quoting/numeric RED: `cargo test --locked bolt_v3_quoting::tests --lib` and
  `cargo test --locked sanitize_open_probability --lib` failed with missing quoting/numeric symbols.
- Quoting/numeric GREEN: quoting passed 8 tests; numeric open-probability passed 2 tests.
- Inventory RED: `cargo test --locked bolt_v3_maker_inventory::tests --lib` failed with missing
  `MakerInventory`.
- Inventory GREEN: same command passed, 5 tests.
- Family fold RED: `cargo test --locked maker_ --lib` failed with missing maker-family dispatchers.
- Family fold GREEN: same command passed, including the new canonical binding tests.

## Verification

- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `rg -n "trait MakerFamily|dyn MakerFamily|MakerFamily" src`: no matches.
- `rg -n "crate::strategies|nautilus|NautilusTrader|OrderFactory|submit|admission"
  src/bolt_v3_requote_budget.rs src/bolt_v3_quoting.rs src/bolt_v3_maker_inventory.rs`: no matches.
- `cargo test --locked --lib`: passed, 619 tests.
- `cargo clippy --locked --lib -- -D warnings`: passed.
- `just source-fence`: passed.

## Stale PR Closure Matrix

| PR | Accepted scope now drained onto current main | Accepted scope still missing | Rejected stale scope | Closure recommendation |
|---|---|---|---|---|
| #514 / `origin/feat/488-w2-lifecycle` | Quote lifecycle was already on main; cost-weighted requote budget now lands as `src/bolt_v3_requote_budget.rs`. | Event-fence/repath specifics were not revisited beyond already-present lifecycle evidence. | Count-only budget fallback not ported; current W2/W4 refs both show cost-weighted API. | Likely ready to close as superseded after reviewer confirms no separate event-fence scope remains. |
| #515 / `origin/feat/488-w3-pricing` | Maker model and microprice were already on main; quoting math, numeric open-probability helper, inventory accumulator, and canonical family binding fold now land on current main. | Broader maker strategy/archetype, reservation, rewards, and portfolio scopes remain outside this drain. | `MakerFamily` trait/object seam and family-specific strategy-layer quote dispatch rejected by #580. | Ready to close for the drained W3 pricing/helper scope; keep later W5/W7 scopes reference-only in their own trackers. |
