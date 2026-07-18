//! NautilusTrader catalog read proof for the canonical bar projection.
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that a validated [`CanonicalBarsTable`] projects into a local
//! `ParquetDataCatalog` as externally-aggregated `Bar` data and reads the exact
//! same OHLCV bars back, with `ts_event == close_time`, plus dirty-root refusal,
//! data-driven precision widening, and a stable logical catalog hash.
//!
//! The fixtures are synthetic and venue-free: the projection behaviour under
//! test is data-driven and must not be tied to any real venue, token, or
//! incident value.

use std::str::FromStr;

use backtesting_vertical_slice::{
    canonical_market_data::{
        CanonicalBarRow, CanonicalBarSpec, CanonicalBarsTable, NORMALIZED_SCHEMA_VERSION,
    },
    canonical_trades::TradesPartition,
    catalog_projection::{
        BinaryOptionInstrumentKind, BinaryOptionInstrumentSpec, SpotInstrumentSpec,
        project_canonical_bars_to_catalog, read_back_bars,
    },
    source_proof::SourceProofFidelityClass,
};
use nautilus_model::{
    enums::{AggregationSource, BarAggregation},
    instruments::InstrumentAny,
    types::Price,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const BASE_OPEN_TIME: i64 = 1_700_000_000_000_000_000;
const BAR_INTERVAL_NANOS: i64 = 60_000_000_000;

fn spec() -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: INSTRUMENT_ID.to_string(),
        base_currency: "BASE".to_string(),
        quote_currency: "QUOTE".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        min_quantity: "0.001".to_string(),
        max_quantity: "1000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "100000000".to_string(),
    }
}

/// A binary-option spec carrying the SAME nt_instrument_id as the spot spec, so
/// the same synthetic prediction-market bars project through NT's
/// `BinaryOption` constructor via the generic catalog seam. Binary-option bar
/// replay is a real family (the in-module `bars_table()` fixture is already
/// prediction-market shaped).
fn binary_option_spec() -> BinaryOptionInstrumentSpec {
    BinaryOptionInstrumentSpec {
        instrument_kind: BinaryOptionInstrumentKind::BinaryOption,
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: INSTRUMENT_ID.to_string(),
        asset_class: "ALTERNATIVE".to_string(),
        currency: "USDC".to_string(),
        activation_time_nanos: 1_700_000_000_000_000_000,
        expiration_time_nanos: 1_700_086_400_000_000_000,
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        outcome: Some("Yes".to_string()),
        description: Some("Bounded binary option fixture".to_string()),
        max_quantity: Some("1000000".to_string()),
        min_quantity: Some("0.001".to_string()),
        // Optional risk and bound metadata is outside this bar fixture's scope.
        max_notional: None,
        min_notional: None,
        max_price: None,
        min_price: None,
        margin_init: None,
        margin_maint: None,
        maker_fee: Some("0".to_string()),
        taker_fee: Some("0".to_string()),
    }
}

fn bar_row(
    open_time: i64,
    open: &str,
    high: &str,
    low: &str,
    close: &str,
    volume: &str,
) -> CanonicalBarRow {
    CanonicalBarRow {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        ingest_run_id: "ingest-run-test".to_string(),
        source_binding: "synthetic-archive".to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        instrument_id: INSTRUMENT_ID.to_string(),
        canonical_instrument_key: "testvenue/prediction-market/BASEQUOTE".to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: Some(NT_INSTRUMENT_ID.to_string()),
        open_time,
        close_time: open_time + BAR_INTERVAL_NANOS,
        capture_time: open_time + BAR_INTERVAL_NANOS,
        availability_time: None,
        source_sequence: Some(open_time.to_string()),
        raw_payload_id: "feedface".to_string(),
        source_proof_id: "source-proof-synthetic".to_string(),
        payload_hash: "feedface".to_string(),
        transform_hash: "0badc0de".to_string(),
        open: open.to_string(),
        high: high.to_string(),
        low: low.to_string(),
        close: close.to_string(),
        volume: volume.to_string(),
    }
}

fn table_with_rows(rows: Vec<CanonicalBarRow>) -> CanonicalBarsTable {
    CanonicalBarsTable {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        partition: TradesPartition {
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: INSTRUMENT_ID.to_string(),
            dt: "2026-05-22".to_string(),
        },
        source_proof_id: "source-proof-synthetic".to_string(),
        source_proof_version: 1,
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        forbidden_claims: vec!["No execution-quality claims.".to_string()],
        transform_hash: "0badc0de".to_string(),
        payload_hash: "feedface".to_string(),
        bar_spec: CanonicalBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        },
        rows,
    }
}

fn bars_table() -> CanonicalBarsTable {
    table_with_rows(vec![
        bar_row(BASE_OPEN_TIME, "0.50", "0.55", "0.49", "0.52", "100"),
        bar_row(
            BASE_OPEN_TIME + BAR_INTERVAL_NANOS,
            "0.52",
            "0.58",
            "0.51",
            "0.57",
            "120",
        ),
    ])
}

#[test]
fn bars_round_trip_through_nt_catalog() {
    let table = bars_table();
    let dir = tempfile::TempDir::new().expect("temp dir");

    let projection =
        project_canonical_bars_to_catalog(&table, &spec(), dir.path()).expect("project bars");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::TradeBarReplay
    );
    assert!(!projection.catalog_hash.is_empty());

    let mut loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), table.rows.len());

    loaded.sort_by_key(|bar| bar.ts_event.as_u64());
    for (bar, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(bar.instrument_id().to_string(), NT_INSTRUMENT_ID);
        // ts_event is the canonical close_time; ts_init is the receipt clock
        // NautilusTrader replays by (capture_time here — no availability column;
        // this fixture sets capture_time == close_time).
        assert_eq!(bar.ts_event.as_u64(), row.close_time as u64);
        assert_eq!(bar.ts_init.as_u64(), row.capture_time as u64);
        // Assert the full bar_type — step, aggregation, and AggregationSource
        // — so a hardcoded spec or wrong source in canonical_rows_to_bars
        // causes a test failure rather than silently passing.
        let bt = &bar.bar_type;
        let (bt_instrument_id, bt_spec, bt_source) = match bt {
            nautilus_model::data::bar::BarType::Standard {
                instrument_id,
                spec,
                aggregation_source,
            } => (instrument_id, spec, aggregation_source),
            nautilus_model::data::bar::BarType::Composite { .. } => {
                panic!("expected Standard bar type, got Composite");
            }
        };
        assert_eq!(
            bt_instrument_id.to_string(),
            NT_INSTRUMENT_ID,
            "bar_type instrument_id mismatch"
        );
        assert_eq!(
            bt_spec.step.get(),
            table.bar_spec.step as usize,
            "bar_type step mismatch"
        );
        assert_eq!(
            bt_spec.aggregation, table.bar_spec.aggregation,
            "bar_type aggregation mismatch"
        );
        assert_eq!(
            *bt_source,
            AggregationSource::External,
            "bar_type aggregation source must be External"
        );
        // Compare OHLCV numerically: Display renders at instrument precision.
        assert_eq!(
            bar.open.as_decimal(),
            Price::from(row.open.as_str()).as_decimal()
        );
        assert_eq!(
            bar.high.as_decimal(),
            Price::from(row.high.as_str()).as_decimal()
        );
        assert_eq!(
            bar.low.as_decimal(),
            Price::from(row.low.as_str()).as_decimal()
        );
        assert_eq!(
            bar.close.as_decimal(),
            Price::from(row.close.as_str()).as_decimal()
        );
        assert_eq!(
            bar.volume.as_decimal(),
            nautilus_model::types::Quantity::from(row.volume.as_str()).as_decimal()
        );
    }
}

#[test]
fn bars_round_trip_through_binary_option_spec() {
    // The same synthetic prediction-market bars must project through the generic
    // catalog seam when bound to a BinaryOption instrument, proving binary-option
    // bar replay reaches the catalog exactly like spot/perp/future.
    let table = bars_table();
    let dir = tempfile::TempDir::new().expect("temp dir");

    let projection = project_canonical_bars_to_catalog(&table, &binary_option_spec(), dir.path())
        .expect("project bars via binary option spec");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert!(!projection.catalog_hash.is_empty());

    // The catalog instrument is an NT BinaryOption (not a CurrencyPair).
    let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let instruments = catalog
        .query_instruments(Some(&[NT_INSTRUMENT_ID.to_string()]))
        .expect("query instruments");
    assert_eq!(instruments.len(), 1);
    assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));

    let mut loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), table.rows.len());
    loaded.sort_by_key(|bar| bar.ts_event.as_u64());
    for (bar, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(bar.instrument_id().to_string(), NT_INSTRUMENT_ID);
        // ts_event is the canonical close_time; ts_init is the receipt clock
        // (capture_time here — no availability column; capture_time == close_time).
        assert_eq!(bar.ts_event.as_u64(), row.close_time as u64);
        assert_eq!(bar.ts_init.as_u64(), row.capture_time as u64);
        assert_eq!(
            bar.open.as_decimal(),
            Price::from(row.open.as_str()).as_decimal()
        );
        assert_eq!(
            bar.high.as_decimal(),
            Price::from(row.high.as_str()).as_decimal()
        );
        assert_eq!(
            bar.low.as_decimal(),
            Price::from(row.low.as_str()).as_decimal()
        );
        assert_eq!(
            bar.close.as_decimal(),
            Price::from(row.close.as_str()).as_decimal()
        );
        assert_eq!(
            bar.volume.as_decimal(),
            nautilus_model::types::Quantity::from(row.volume.as_str()).as_decimal()
        );
    }
}

#[test]
fn bars_round_trip_preserves_non_default_bar_spec() {
    // Prove that bar_type step and aggregation are read from the canonical
    // bar_spec rather than hardcoded: use step=5, BarAggregation::Hour
    // (a valid periodic step) and assert the read-back bar_type reflects it.
    let mut table = table_with_rows(vec![bar_row(
        BASE_OPEN_TIME,
        "0.50",
        "0.55",
        "0.49",
        "0.52",
        "100",
    )]);
    // Override the bar spec: 4-HOUR instead of 1-MINUTE. The step must evenly
    // divide 24 to be a valid (periodic) NautilusTrader Hour specification, so
    // 4 is a non-default choice that survives the admissibility probe (5 would
    // not: 5 does not divide 24).
    table.bar_spec = CanonicalBarSpec {
        step: 4,
        aggregation: BarAggregation::Hour,
    };
    // Adjust close_time to respect the new interval (4 hours in nanos) so
    // that the periodic-step validation passes.
    let four_hours_nanos: i64 = 4 * 3_600_000_000_000;
    table.rows[0].close_time = BASE_OPEN_TIME + four_hours_nanos;
    table.rows[0].capture_time = BASE_OPEN_TIME + four_hours_nanos;

    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_bars_to_catalog(&table, &spec(), dir.path()).expect("project 4-HOUR bars");

    let loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), 1);

    let bt = &loaded[0].bar_type;
    let (bt_spec, bt_source) = match bt {
        nautilus_model::data::bar::BarType::Standard {
            spec,
            aggregation_source,
            ..
        } => (spec, aggregation_source),
        nautilus_model::data::bar::BarType::Composite { .. } => {
            panic!("expected Standard bar type, got Composite");
        }
    };
    assert_eq!(bt_spec.step.get(), 4, "step must be 4 not default 1");
    assert_eq!(
        bt_spec.aggregation,
        BarAggregation::Hour,
        "aggregation must be Hour not default Minute"
    );
    assert_eq!(
        *bt_source,
        AggregationSource::External,
        "aggregation source must be External"
    );
}

#[test]
fn bar_validate_rejects_ohlc_violation() {
    let mut table = bars_table();
    // high below open is an OHLC ordering violation.
    table.rows[0].high = "0.40".to_string();
    let error = table
        .validate()
        .expect_err("ohlc violation must be rejected");
    assert!(error.to_string().contains("high"), "{error}");
}

#[test]
fn bar_validate_rejects_non_increasing_open_time() {
    let mut table = bars_table();
    table.rows[1].open_time = table.rows[0].open_time;
    let error = table
        .validate()
        .expect_err("non-increasing open_time must be rejected");
    assert!(error.to_string().contains("strictly increase"), "{error}");
}

#[test]
fn projection_refuses_dirty_catalog_root() {
    let table = bars_table();
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
    let err = project_canonical_bars_to_catalog(&table, &spec(), dir.path())
        .expect_err("dirty catalog root must be refused");
    assert!(err.to_string().contains("not empty"), "{err}");
}

#[test]
fn bar_precision_widens_when_data_finer() {
    // The spec tick is 0.01, but the archive carries finer prices (scale 3).
    // The projection must widen the instrument to the data's actual scale.
    let table = table_with_rows(vec![bar_row(
        BASE_OPEN_TIME,
        "0.501",
        "0.559",
        "0.491",
        "0.523",
        "100.0001",
    )]);
    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_bars_to_catalog(&table, &spec(), dir.path())
        .expect("projection widens precision instead of rejecting accepted data");
    let loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded[0].high.as_decimal(),
        Price::from_str("0.559").expect("parse high").as_decimal()
    );
    assert_eq!(
        loaded[0].low.as_decimal(),
        Price::from_str("0.491").expect("parse low").as_decimal()
    );
    assert!(
        loaded[0].high > loaded[0].low,
        "moving bar must have high > low"
    );
}

#[test]
fn bar_catalog_hash_is_stable() {
    let table = bars_table();
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let a = project_canonical_bars_to_catalog(&table, &spec(), dir_a.path()).unwrap();
    let b = project_canonical_bars_to_catalog(&table, &spec(), dir_b.path()).unwrap();
    assert_eq!(
        a.catalog_hash, b.catalog_hash,
        "same bar data must hash identically regardless of root"
    );

    // A bar with different close price must change the catalog hash.
    let mut table_c = bars_table();
    table_c.rows[0].close = "0.50".to_string();
    let dir_c = tempfile::TempDir::new().unwrap();
    let c = project_canonical_bars_to_catalog(&table_c, &spec(), dir_c.path()).unwrap();
    assert_ne!(
        a.catalog_hash, c.catalog_hash,
        "different bar data must change the catalog hash"
    );
}
