# World Cup MM Static Event Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add config-driven static binary-event market selection and Polymarket slug filtering as the first World Cup MM implementation slice.

**Architecture:** Introduce a `static_binary_event` market-family binding parallel to `updown`, reusing the existing `MarketFamilyValidationBinding` registry. Polymarket adapter mapping will collect slug filters from both rotating `updown` targets and static event targets. The strategy runtime config gains optional static-event fields for configured condition and YES/NO outcome labels.

**Tech Stack:** Rust, NautilusTrader model types, TOML config parsing, existing bolt-v3 market-family registry and Polymarket provider mapping.

---

### Task 1: Static Event Family Registration And Selection

**Files:**
- Create: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/bolt_v3_market_families/mod.rs`
- Test: `src/bolt_v3_market_families/static_binary_event.rs`

- [ ] **Step 1: Write failing selection tests**

Add tests proving that a target shaped like this selects matching `BinaryOption` instruments:

```toml
configured_target_id = "world_cup_fixture"
kind = "static_market"
rotating_market_family = "static_binary_event"
event_key = "world_cup"
market_slug = "configured-world-cup-market"
condition_id = "configured-condition-id"
yes_outcome = "Yes"
no_outcome = "No"
selection_window_secs = 1
market_selection_rule = "configured_static"
retry_interval_secs = 5
blocked_after_secs = 30
```

The positive test must assert selected `market_id`, `up_instrument_id`, `down_instrument_id`, `source_identity.market_slug`, and `seconds_to_end`.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --lib static_binary_event -- --nocapture
```

Expected: fail because `static_binary_event` module and binding do not exist.

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

Run:

```bash
cargo test --lib static_binary_event -- --nocapture
```

Expected: tests pass.

### Task 2: Strategy Config And Selection Dispatch

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/tests/config.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/selection.rs`

- [ ] **Step 1: Write failing strategy config test**

Add a test proving `BinaryOracleEdgeTakerBuilder::validate_config` accepts `rotating_market_family = "static_binary_event"` because it is registry-bound.

- [ ] **Step 2: Write failing strategy selection test**

Add a test proving `selection_snapshot_from_instruments` selects a configured static event from NT `BinaryOption` metadata when the strategy config uses `static_binary_event`.

- [ ] **Step 3: Run focused tests and verify RED or existing GREEN**

Run:

```bash
cargo test --lib builder_accepts_static_binary_event_market_family strategy_selects_configured_static_binary_event_target_from_nt_binary_option_metadata -- --nocapture
```

Expected before Task 1 GREEN: fail. Expected after Task 1 GREEN: pass or expose missing runtime projection.

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
- retry and blocked fields from TOML.

- [ ] **Step 5: Verify GREEN**

Run:

```bash
cargo test --lib builder_accepts_static_binary_event_market_family strategy_selects_configured_static_binary_event_target_from_nt_binary_option_metadata -- --nocapture
```

Expected: tests pass.

### Task 3: Polymarket Static Slug Filters

**Files:**
- Modify: `src/bolt_v3_providers/polymarket.rs`

- [ ] **Step 1: Write failing provider mapping test**

Add a test proving Polymarket data config installs a market-slug filter that includes configured static event slugs for the matching execution client.

- [ ] **Step 2: Run focused test and verify RED**

Run:

```bash
cargo test --lib market_slug_filters_include_static_binary_event_targets_for_matching_client -- --nocapture
```

Expected: fail because the provider currently builds filters only from `updown::target_plans`.

- [ ] **Step 3: Implement static target filter collection**

Update `build_market_slug_filters_for_client` to append filters from `static_binary_event::target_plans(plan)` for the same client. Static filters return a single configured `market_slug`.

- [ ] **Step 4: Verify GREEN**

Run:

```bash
cargo test --lib market_slug_filters_include_static_binary_event_targets_for_matching_client -- --nocapture
```

Expected: tests pass.

### Task 4: Focused Verification

**Files:**
- No code changes unless verification exposes a regression.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --check
```

Expected: pass.

- [ ] **Step 2: Source fence**

Run:

```bash
just source-fence
```

Expected: pass.

- [ ] **Step 3: Focused test set**

Run:

```bash
cargo test --lib static_binary_event -- --nocapture
cargo test --lib market_slug_filters_include_static_binary_event_targets_for_matching_client -- --nocapture
cargo test --lib static_binary_event -- --nocapture
```

Expected: pass.
