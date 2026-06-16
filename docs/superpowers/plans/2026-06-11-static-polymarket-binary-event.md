# Static Polymarket Binary Event Implementation Plan

> **For agentic workers:** `AGENTS.md` governs implementation and verification. Use evidence-driven checks, local non-compile gates, and exact-head PR CI for Rust compile/test proof.

**Goal:** Add config-driven static binary-event market selection and Polymarket slug filtering. The original operator use case is World Cup market making, but the implementation is a generic static Polymarket binary event selector.

**Architecture:** Introduce a `static_binary_event` market-family binding parallel to `updown`, reusing the existing `MarketFamilyValidationBinding` registry. Polymarket adapter mapping will collect slug filters from both rotating `updown` targets and static event targets. The strategy runtime config gains optional static-event fields for configured condition and YES/NO outcome labels. The shared maker quote/order pipeline remains PR 716 scope; this slice only binds the static family to the existing registry hook surface.

**Tech Stack:** Rust, NautilusTrader model types, TOML config parsing, existing bolt-v3 market-family registry and Polymarket provider mapping.

---

### Task 1: Static Event Family Registration And Selection

**Evidence class:** Production behavior must be proven by automated Rust tests on exact-head PR CI; local static proof comes from formatting, source-fence, and targeted scope/leakage scans.

**Files:**
- Create: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/bolt_v3_market_families/mod.rs`
- Test: `src/bolt_v3_market_families/static_binary_event.rs`

- [ ] **Step 1: Write failing selection tests**

Add tests proving that a target shaped like this selects matching `BinaryOption` instruments:

```toml
configured_target_id = "sample_binary_event"
kind = "static_market"
rotating_market_family = "static_binary_event"
event_key = "sample_event"
market_slug = "configured-binary-event-market"
condition_id = "configured-condition-id"
yes_outcome = "Yes"
no_outcome = "No"
fair_probability_source = "reference_current_price"
selection_window_secs = 1
market_selection_rule = "configured_static"
retry_interval_secs = 5
blocked_after_secs = 30
```

The positive test must assert selected `market_id`, `up_instrument_id`, `down_instrument_id`, `source_identity.market_slug`, and `seconds_to_end`.

When `fair_probability_source = "reference_current_price"` is configured, this slice only preserves the static-event fair-probability source token and runtime projection. The reference-current-price source table and provider runtime are owned by PR 730.

- [ ] **Step 2: Record the focused regression target**

The focused regression target is `static_binary_event` family selection. Under repo verification policy, Rust compile/test proof is collected from exact-head PR CI rather than default local compile-heavy commands.

- [ ] **Step 3: Implement the family module**

Create a module with:

```rust
pub const KEY: &str = "static_binary_event";
pub fn validate_target_block(context: &str, target: &toml::Value) -> Vec<String>;
pub fn plan_strategy_target(strategy: &LoadedStrategy) -> Result<Option<Arc<dyn MarketIdentityTarget>>, InstrumentFilterError>;
pub fn target_runtime_fields(target: &toml::Value) -> Result<TargetRuntimeFields, InstrumentFilterError>;
pub fn select_binary_option_market(target: MarketSelectionTarget<'_>, instruments: &[InstrumentAny], now_milliseconds: u64) -> Option<SelectedBinaryOptionMarket>;
pub fn selected_market_requirement(target: &toml::Value, selected: &SelectedBinaryOptionMarket, selected_at_ms: u64) -> Result<SelectedMarketRequirement, InstrumentFilterError>;
```

Selection rules:

- Match `BinaryOption` instruments whose metadata `market_slug` equals configured `market_slug`.
- If `condition_id` is configured, require metadata `condition_id` to match.
- Use configured `yes_outcome` and `no_outcome` labels to identify the two outcome instruments.
- Require one distinct YES and one distinct NO instrument.
- Require both instruments share `market_id`, `condition_id`, `market_slug`, and `question_id`.
- Reject expired markets by returning `None`.

- [ ] **Step 4: Register the binding**

Modify `src/bolt_v3_market_families/mod.rs`:

```rust
pub mod static_binary_event;
```

Add a `MarketFamilyValidationBinding` entry with:

```rust
key: static_binary_event::KEY,
validate_target: static_binary_event::validate_target_block,
plan_strategy_target: static_binary_event::plan_strategy_target,
target_runtime_fields: static_binary_event::target_runtime_fields,
select_binary_option_market: static_binary_event::select_binary_option_market,
market_selection_candidate_windows: static_binary_event::market_selection_candidate_windows,
selected_market_requirement: static_binary_event::selected_market_requirement,
fair_probability_up: static_binary_event::fair_probability_up,
maker_quote_targets: static_binary_event::maker_quote_targets,
maker_settlement_payout: static_binary_event::maker_settlement_payout,
maker_binary_fee_curve: static_binary_event::maker_binary_fee_curve,
```

- [ ] **Step 5: Verify GREEN**

Expected exact-head PR CI evidence: static event family tests pass with the production registry binding included.

### Task 2: Strategy Config And Selection Dispatch

**Evidence class:** Production behavior must be proven by automated Rust tests on exact-head PR CI; fail-closed config validation is covered by targeted builder tests.

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/tests/config.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/selection.rs`

- [ ] **Step 1: Write failing strategy config test**

Add a test proving `BinaryOracleEdgeTakerBuilder::validate_config` accepts `rotating_market_family = "static_binary_event"` because it is registry-bound.

- [ ] **Step 2: Write failing strategy selection test**

Add a test proving `selection_snapshot_from_instruments` selects a configured static event from NT `BinaryOption` metadata when the strategy config uses `static_binary_event`.

- [ ] **Step 3: Record focused strategy regression targets**

The focused regression targets are `builder_accepts_static_binary_event_market_family` and `strategy_selects_configured_static_binary_event_target_from_nt_binary_option_metadata`.

- [ ] **Step 4: Adjust only projection gaps**

If runtime projection is missing, make `static_binary_event::target_runtime_fields` set:

- `configured_target_id` from TOML.
- `target_kind = "static_market"`.
- `rotating_market_family = "static_binary_event"`.
- `underlying_asset = event_key`.
- `cadence_seconds = selection_window_secs`.
- `cadence_seconds_source_field = "target.selection_window_secs"`.
- `cadence_slug_token = market_slug`.
- `market_selection_rule = "configured_static"`.
- `static_condition_id = condition_id`.
- `static_yes_outcome = yes_outcome`.
- `static_no_outcome = no_outcome`.
- `static_fair_probability_source = fair_probability_source`.
- retry and blocked fields from TOML.

- [ ] **Step 5: Verify GREEN**

Expected exact-head PR CI evidence: static-event config validation and strategy selection tests pass.

### Task 3: Polymarket Static Slug Filters

**Evidence class:** Production behavior must be proven by automated Rust tests on exact-head PR CI; behavior is the data-client slug filter generated from TOML-owned static targets.

**Files:**
- Modify: `src/bolt_v3_providers/polymarket.rs`

- [ ] **Step 1: Write failing provider mapping test**

Add a test proving Polymarket data config installs a market-slug filter that includes configured static event slugs for the matching execution client.

- [ ] **Step 2: Record the focused provider regression target**

The focused regression target is `market_slug_filters_include_static_binary_event_targets_for_matching_client`.

- [ ] **Step 3: Implement static target filter collection**

Update `build_market_slug_filters_for_client` to append filters from `static_binary_event::target_plans(plan)` for the same client. Static filters return a single configured `market_slug`.

- [ ] **Step 4: Verify GREEN**

Expected exact-head PR CI evidence: Polymarket slug-filter tests pass.

### Task 4: Focused Verification

**Files:**
- No code changes unless verification exposes a regression.

**Evidence class:** Refactor/scope safety is proven by static checks and exact-head PR CI; no live operation is part of this slice.

- [ ] **Step 1: Format**

Run:

```bash
just fmt-check
```

Expected: pass.

- [ ] **Step 2: Source fence**

Run:

```bash
just source-fence-static
```

Expected: pass.

- [ ] **Step 3: Scope and leakage scans**

Run:

```bash
git diff origin/main --name-status
rg -n "bolt_v3_reference_price|chainlink_reference|bolt_v3_maker_order|bolt_v3_maker_quote|maker_runtime" <changed-files>
```

Expected: the diff contains only static Polymarket binary-event selection/filtering/docs and no PR 730 or PR 716 implementation leakage.

- [ ] **Step 4: Exact-head CI**

Run after commit and push:

```bash
just verify-remote
```

Expected: exact-head PR CI is green.
