# BTE-022 PMXT Polymarket NT Mapping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove one PMXT Polymarket hourly order-book sample maps into NT-owned `InstrumentAny::BinaryOption`, `OrderBookDelta`, and `TradeTick` catalog data and can be read by `ParquetDataCatalog`/`BacktestNode` without venue hardcodes.

**Architecture:** Bolt owns only the source-proof gate, PMXT row normalization, grouping policy, and provenance binding. NautilusTrader owns instrument construction, checked market-data constructors, catalog format, and backtest consumption. The implementation must first wire the isolated BTE crate to pinned `nautilus-polymarket`, then prove a small bounded sample path before manifest/backfill admission is widened.

**Current caveat:** The price-change grouping tasks below predate `reference/source-proof-pmxt-polymarket-price-change-grouping-status.2026-06-08.json`. Before executing this plan, revise the price-change implementation steps to use timestamp_received as a boundary/ts_init input and prefer one PMXT `price_change` row to one single-change NT `PolymarketQuotes` parse call for pinned NT live-client parity. Grouped parser output requires its own proving test.

**Tech Stack:** Rust, pinned NautilusTrader crates at `6e059dcbb59ac1e582132fc431a581936c216c3c`, `ParquetDataCatalog`, PMXT Parquet samples, Cargo tests.

---

## Scope Gate

This plan touches `crates/backtesting-vertical-slice/Cargo.toml` and `crates/backtesting-vertical-slice/Cargo.lock`. Do not execute it until that dependency-boundary change is explicitly in scope for the branch. Do not start broad historical loading in this plan.

## File Structure

- Modify `crates/backtesting-vertical-slice/Cargo.toml`: add pinned `nautilus-polymarket` dependency only.
- Modify `crates/backtesting-vertical-slice/Cargo.lock`: update by `cargo update -p nautilus-polymarket --precise 0.1.0` only if Cargo requires it; otherwise let `cargo test` resolve the lock.
- Modify `crates/backtesting-vertical-slice/src/nt_dependency_proof.rs`: include Polymarket dependency presence in the NT proof.
- Modify `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_nt_dependency_proof.rs`: assert isolated crate has `nautilus-polymarket`.
- Create `crates/backtesting-vertical-slice/src/pmxt_polymarket.rs`: PMXT row structs, event grouping keys, timestamp conversion, and NT projection helpers.
- Modify `crates/backtesting-vertical-slice/src/lib.rs`: export `pmxt_polymarket`.
- Create `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs`: bounded unit/integration tests for grouping, parser reuse, catalog write/readback, and BacktestNode data config.
- Modify `crates/backtesting-vertical-slice/src/run_manifest.rs`: after isolated proof passes, admit `OrderBookDelta` only for `L2Replay`.

---

### Task 1: RED Dependency Proof For NT Polymarket

**Files:**
- Modify: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_nt_dependency_proof.rs`

- [ ] **Step 1: Write the failing test**

Add this assertion to `nt_dependency_proof_binds_revision_and_required_features`:

```rust
    assert!(
        proof
            .nt_dependency_names
            .contains(&"nautilus-polymarket".to_string()),
        "BTE must reuse pinned NT Polymarket parser/provider for binary-option instruments"
    );
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_nt_dependency_proof
```

Expected: FAIL because `proof.nt_dependency_names` does not contain `nautilus-polymarket`.

- [ ] **Step 3: Commit RED test**

```bash
git add crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_nt_dependency_proof.rs
git commit -m "test: require NT Polymarket dependency for BTE mapping"
```

### Task 2: GREEN Polymarket Dependency Wiring

**Files:**
- Modify: `crates/backtesting-vertical-slice/Cargo.toml`
- Modify: `crates/backtesting-vertical-slice/Cargo.lock`

- [ ] **Step 1: Add pinned dependency**

Add this line beside the other pinned NT dependencies in `crates/backtesting-vertical-slice/Cargo.toml`:

```toml
nautilus-polymarket = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "6e059dcbb59ac1e582132fc431a581936c216c3c" }
```

- [ ] **Step 2: Run dependency proof**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_nt_dependency_proof
```

Expected: PASS if the lock already contains `nautilus-polymarket`; otherwise Cargo reports the lock needs updating.

- [ ] **Step 3: Update lock only if required**

Run only if Step 2 reports a lock mismatch:

```bash
cargo update --manifest-path crates/backtesting-vertical-slice/Cargo.toml -p nautilus-polymarket
```

Then rerun:

```bash
cargo test --locked --test backtesting_vertical_slice_nt_dependency_proof
```

Expected: PASS.

- [ ] **Step 4: Commit dependency wiring**

```bash
git add crates/backtesting-vertical-slice/Cargo.toml crates/backtesting-vertical-slice/Cargo.lock
git commit -m "Add NT Polymarket dependency to BTE slice"
```

### Task 3: RED PMXT Price-Change Grouping Contract

**Files:**
- Create: `crates/backtesting-vertical-slice/src/pmxt_polymarket.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`
- Create: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs`

- [ ] **Step 1: Export empty module**

Add to `crates/backtesting-vertical-slice/src/lib.rs`:

```rust
pub mod pmxt_polymarket;
```

Create `crates/backtesting-vertical-slice/src/pmxt_polymarket.rs`:

```rust
//! PMXT Polymarket order-book row normalization into NautilusTrader data.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmxtPriceChangeRow {
    pub market: String,
    pub asset_id: String,
    pub timestamp_ms: String,
    pub timestamp_received_ns: u64,
    pub price: String,
    pub size: String,
    pub side: String,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
}
```

- [ ] **Step 2: Write failing grouping test**

Create `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs`:

```rust
use backtesting_vertical_slice::pmxt_polymarket::{
    PmxtPriceChangeRow, group_price_changes_for_nt,
};

#[test]
fn groups_price_changes_by_market_timestamp_asset_and_received_policy() {
    let rows = vec![
        row("0xabc", "token-yes", "1770000000000", 10, "0.45", "5.000000", "BUY"),
        row("0xabc", "token-no", "1770000000000", 10, "0.55", "5.000000", "SELL"),
        row("0xabc", "token-yes", "1770000000000", 11, "0.46", "7.000000", "BUY"),
    ];

    let grouped = group_price_changes_for_nt(rows).expect("grouped rows");

    assert_eq!(grouped.len(), 3);
    assert_eq!(grouped[0].market, "0xabc");
    assert_eq!(grouped[0].asset_id, "token-no");
    assert_eq!(grouped[0].timestamp_ms, "1770000000000");
    assert_eq!(grouped[0].timestamp_received_ns, 10);
    assert_eq!(grouped[0].rows.len(), 1);
    assert_eq!(grouped[1].asset_id, "token-yes");
    assert_eq!(grouped[1].timestamp_received_ns, 10);
    assert_eq!(grouped[1].rows.len(), 1);
    assert_eq!(grouped[2].asset_id, "token-yes");
    assert_eq!(grouped[2].timestamp_received_ns, 11);
    assert_eq!(grouped[2].rows.len(), 1);
}

fn row(
    market: &str,
    asset_id: &str,
    timestamp_ms: &str,
    timestamp_received_ns: u64,
    price: &str,
    size: &str,
    side: &str,
) -> PmxtPriceChangeRow {
    PmxtPriceChangeRow {
        market: market.to_string(),
        asset_id: asset_id.to_string(),
        timestamp_ms: timestamp_ms.to_string(),
        timestamp_received_ns,
        price: price.to_string(),
        size: size.to_string(),
        side: side.to_string(),
        best_bid: Some(price.to_string()),
        best_ask: Some(price.to_string()),
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket groups_price_changes_by_market_timestamp_asset_and_received_policy
```

Expected: FAIL with unresolved import `group_price_changes_for_nt`.

### Task 4: GREEN PMXT Price-Change Grouping Contract

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/pmxt_polymarket.rs`

- [ ] **Step 1: Add grouped type and implementation**

Add to `pmxt_polymarket.rs`:

```rust
use anyhow::{Result, ensure};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmxtPriceChangeGroup {
    pub market: String,
    pub asset_id: String,
    pub timestamp_ms: String,
    pub timestamp_received_ns: u64,
    pub rows: Vec<PmxtPriceChangeRow>,
}

pub fn group_price_changes_for_nt(
    rows: Vec<PmxtPriceChangeRow>,
) -> Result<Vec<PmxtPriceChangeGroup>> {
    let mut grouped: BTreeMap<(String, String, String, u64), Vec<PmxtPriceChangeRow>> =
        BTreeMap::new();
    for row in rows {
        ensure!(!row.market.trim().is_empty(), "PMXT market must not be empty");
        ensure!(!row.asset_id.trim().is_empty(), "PMXT asset_id must not be empty");
        ensure!(!row.timestamp_ms.trim().is_empty(), "PMXT timestamp_ms must not be empty");
        let key = (
            row.market.clone(),
            row.asset_id.clone(),
            row.timestamp_ms.clone(),
            row.timestamp_received_ns,
        );
        grouped.entry(key).or_default().push(row);
    }

    Ok(grouped
        .into_iter()
        .map(|((market, asset_id, timestamp_ms, timestamp_received_ns), rows)| {
            PmxtPriceChangeGroup {
                market,
                asset_id,
                timestamp_ms,
                timestamp_received_ns,
                rows,
            }
        })
        .collect())
}
```

- [ ] **Step 2: Run grouping test**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket groups_price_changes_by_market_timestamp_asset_and_received_policy
```

Expected: PASS.

- [ ] **Step 3: Commit grouping contract**

```bash
git add crates/backtesting-vertical-slice/src/lib.rs crates/backtesting-vertical-slice/src/pmxt_polymarket.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs
git commit -m "Add PMXT Polymarket grouping contract"
```

### Task 5: RED NT Parser Reuse For Price Changes

**Files:**
- Modify: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs`

- [ ] **Step 1: Add failing parser test**

Append:

```rust
use backtesting_vertical_slice::pmxt_polymarket::price_change_group_to_nt_deltas;
use nautilus_model::{enums::BookAction, identifiers::InstrumentId};

#[test]
fn converts_grouped_price_changes_to_nt_order_book_deltas() {
    let rows = vec![
        row("0xabc", "token-yes", "1770000000000", 10, "0.45", "5.000000", "BUY"),
        row("0xabc", "token-yes", "1770000000000", 10, "0.46", "0.000000", "BUY"),
    ];
    let group = group_price_changes_for_nt(rows)
        .expect("grouped")
        .pop()
        .expect("one group");
    let instrument_id: InstrumentId = "0xabc-token-yes.POLYMARKET".parse().unwrap();

    let deltas = price_change_group_to_nt_deltas(&group, instrument_id, 3, 6)
        .expect("NT deltas");

    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].action, BookAction::Update);
    assert_eq!(deltas[1].action, BookAction::Delete);
    assert_ne!(deltas[0].flags, deltas[1].flags);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket converts_grouped_price_changes_to_nt_order_book_deltas
```

Expected: FAIL with unresolved import `price_change_group_to_nt_deltas`.

### Task 6: GREEN NT Parser Reuse For Price Changes

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/pmxt_polymarket.rs`

- [ ] **Step 1: Implement conversion using NT Polymarket structs**

Add:

```rust
use anyhow::bail;
use nautilus_core::UnixNanos;
use nautilus_model::{data::OrderBookDelta, identifiers::InstrumentId};
use nautilus_polymarket::websocket::{
    messages::{PolymarketOrderSide, PolymarketQuote, PolymarketQuotes},
    parse::parse_book_deltas,
};
use ustr::Ustr;

pub fn price_change_group_to_nt_deltas(
    group: &PmxtPriceChangeGroup,
    instrument_id: InstrumentId,
    price_precision: u8,
    size_precision: u8,
) -> Result<Vec<OrderBookDelta>> {
    let mut price_changes = Vec::with_capacity(group.rows.len());
    for row in &group.rows {
        let side = match row.side.as_str() {
            "BUY" => PolymarketOrderSide::Buy,
            "SELL" => PolymarketOrderSide::Sell,
            other => bail!("unsupported PMXT Polymarket side {other:?}"),
        };
        price_changes.push(PolymarketQuote {
            asset_id: Ustr::from(row.asset_id.as_str()),
            price: row.price.clone(),
            side,
            size: row.size.clone(),
            hash: String::new(),
            best_bid: row.best_bid.clone(),
            best_ask: row.best_ask.clone(),
        });
    }
    let quotes = PolymarketQuotes {
        market: Ustr::from(group.market.as_str()),
        price_changes,
        timestamp: group.timestamp_ms.clone(),
    };
    let ts_init = UnixNanos::from(group.timestamp_received_ns);
    Ok(parse_book_deltas(
        &quotes,
        instrument_id,
        price_precision,
        size_precision,
        ts_init,
    )?
    .deltas)
}
```

- [ ] **Step 2: Run parser test**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket converts_grouped_price_changes_to_nt_order_book_deltas
```

Expected: PASS.

- [ ] **Step 3: Add source comment about neutral hash**

Add one comment above `hash: String::new()`:

```rust
// PMXT does not expose the live websocket quote hash; NT parse_book_deltas does not consume it.
```

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket
```

Expected: PASS.

- [ ] **Step 4: Commit parser reuse**

```bash
git add crates/backtesting-vertical-slice/src/pmxt_polymarket.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs
git commit -m "Map PMXT Polymarket price changes through NT"
```

### Task 7: RED Catalog Write/Readback For BinaryOption L2

**Files:**
- Modify: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs`

- [ ] **Step 1: Add failing catalog test**

Append:

```rust
use nautilus_model::{
    enums::AssetClass,
    instruments::{BinaryOption, InstrumentAny},
    identifiers::Symbol,
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;

#[test]
fn writes_and_reads_binary_option_order_book_deltas_from_nt_catalog() {
    let dir = tempfile::tempdir().expect("temp dir");
    let instrument_id: InstrumentId = "0xabc-token-yes.POLYMARKET".parse().unwrap();
    let instrument = BinaryOption::new_checked(
        instrument_id,
        Symbol::new("token-yes"),
        AssetClass::Alternative,
        Currency::from("USD"),
        1.into(),
        2.into(),
        3,
        6,
        Price::from("0.001"),
        Quantity::from("0.000001"),
        Some(Ustr::from("Yes")),
        Some(Ustr::from("Sample")),
        None,
        None,
        None,
        None,
        Some(Price::from("1.000")),
        Some(Price::from("0.000")),
        Some(Decimal::ZERO),
        Some(Decimal::ZERO),
        None,
        1.into(),
        1.into(),
    )
    .expect("instrument");
    let rows = vec![row("0xabc", "token-yes", "1770000000000", 10, "0.45", "5.000000", "BUY")];
    let group = group_price_changes_for_nt(rows)
        .expect("grouped")
        .pop()
        .expect("one group");
    let deltas = price_change_group_to_nt_deltas(&group, instrument_id, 3, 6).expect("deltas");

    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::BinaryOption(instrument)])
        .expect("write instrument");
    catalog
        .write_to_parquet(deltas.clone(), None, None, None)
        .expect("write deltas");
    let loaded = catalog
        .query_typed_data::<nautilus_model::data::OrderBookDelta>(
            Some(vec![instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("read deltas");

    assert_eq!(loaded, deltas);
}
```

- [ ] **Step 2: Run test**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket writes_and_reads_binary_option_order_book_deltas_from_nt_catalog
```

Expected: PASS after Task 6; if it fails, fix only the typed imports or BinaryOption construction. Do not change mapping semantics.

- [ ] **Step 3: Commit catalog proof**

```bash
git add crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_polymarket.rs
git commit -m "Prove PMXT Polymarket NT catalog readback"
```

### Task 8: RED/GREEN Manifest Admission For `OrderBookDelta`

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/run_manifest.rs`

- [ ] **Step 1: Add failing manifest tests**

Add tests next to `rejects_unsupported_data_type`:

```rust
#[test]
fn accepts_order_book_delta_for_l2_replay() {
    let mut manifest = test_manifest();
    let mut accepted = test_accepted_dataset();
    manifest.catalog_input.data_type = "OrderBookDelta".to_string();
    accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

    manifest
        .validate_against_accepted(&accepted)
        .expect("OrderBookDelta L2 replay should validate");
    let data_config = manifest.to_nt_data_config().expect("NT data config");
    assert_eq!(format!("{:?}", data_config.data_type()), "OrderBookDelta");
}

#[test]
fn rejects_order_book_delta_for_trade_replay() {
    let mut manifest = test_manifest();
    let accepted = test_accepted_dataset();
    manifest.catalog_input.data_type = "OrderBookDelta".to_string();

    let err = manifest
        .validate_against_accepted(&accepted)
        .expect_err("OrderBookDelta must require L2Replay");

    assert!(matches!(err, ManifestError::DataTypeFidelityMismatch { .. }));
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test --locked --lib run_manifest::tests::accepts_order_book_delta_for_l2_replay
cargo test --locked --lib run_manifest::tests::rejects_order_book_delta_for_trade_replay
```

Expected: first test FAILS with unsupported catalog data type.

- [ ] **Step 3: Implement minimal mapping**

Change `to_nt_data_config` data type match:

```rust
        let data_type = match self.catalog_input.data_type.as_str() {
            "TradeTick" => NautilusDataType::TradeTick,
            "OrderBookDelta" => NautilusDataType::OrderBookDelta,
            other => {
                return Err(ManifestError::UnsupportedDataType {
                    data_type: other.to_string(),
                });
            }
        };
```

Change `ensure_supported_data_type`:

```rust
fn ensure_supported_data_type(value: &str) -> Result<(), ManifestError> {
    match value {
        "TradeTick" | "OrderBookDelta" => Ok(()),
        other => Err(ManifestError::UnsupportedDataType {
            data_type: other.to_string(),
        }),
    }
}
```

Change `ensure_data_type_matches_fidelity`:

```rust
    match (data_type, fidelity_class) {
        ("TradeTick", SourceProofFidelityClass::TradeReplay) => Ok(()),
        ("OrderBookDelta", SourceProofFidelityClass::L2Replay) => Ok(()),
        (data_type, fidelity_class) => Err(ManifestError::DataTypeFidelityMismatch {
            data_type: data_type.to_string(),
            fidelity_class,
        }),
    }
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test --locked --lib run_manifest::tests::accepts_order_book_delta_for_l2_replay
cargo test --locked --lib run_manifest::tests::rejects_order_book_delta_for_trade_replay
```

Expected: PASS.

- [ ] **Step 5: Commit manifest admission**

```bash
git add crates/backtesting-vertical-slice/src/run_manifest.rs
git commit -m "Admit OrderBookDelta only for L2 replay"
```

### Task 9: Verification

**Files:**
- No new edits unless checks fail.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 2: Focused tests**

Run:

```bash
cargo test --locked --test backtesting_vertical_slice_nt_dependency_proof
cargo test --locked --test backtesting_vertical_slice_pmxt_polymarket
cargo test --locked --lib run_manifest::tests::accepts_order_book_delta_for_l2_replay
cargo test --locked --lib run_manifest::tests::rejects_order_book_delta_for_trade_replay
```

Expected: all PASS.

- [ ] **Step 3: BTE crate checks**

Run:

```bash
cargo clippy --locked --lib -- -D warnings
cargo test --locked
```

Expected: all PASS.

- [ ] **Step 4: Source fence**

Run:

```bash
just source-fence
```

Expected: PASS.

---

## Self-Review

- Spec coverage: This plan covers BTE-022 for the PMXT Polymarket candidate only. It does not close BTE-006 IAM scope, BTE-027 final source selection, production backfill, or Research Analytics.
- Placeholder scan: No task contains forbidden placeholder markers or an undefined future task.
- Type consistency: The plan uses existing `SourceProofFidelityClass::L2Replay`, NT `NautilusDataType::OrderBookDelta`, and existing `ParquetDataCatalog` write/read APIs.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-08-bte-022-pmxt-polymarket-nt-mapping.md`.

Execution options:

1. Subagent-Driven: dispatch a fresh subagent per task and review between tasks.
2. Inline Execution: execute tasks in this session with explicit RED/GREEN checkpoints.
