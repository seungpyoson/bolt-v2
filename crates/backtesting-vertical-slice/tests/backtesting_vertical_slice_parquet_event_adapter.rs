//! End-to-end proof for the config-driven Parquet typed-event-stream L2 delta
//! adapter with dual emission (format family: interleaved typed events).
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that ONE accepted Parquet event-stream object normalizes through
//! [`normalize_parquet_event_stream_deltas`] into BOTH a validated
//! [`CanonicalOrderBookDeltasTable`] (L2_REPLAY) AND a validated
//! [`CanonicalTradesTable`] (TRADE_REPLAY), and that each family projects into a
//! local `ParquetDataCatalog` and reads back with per-field equality:
//!
//! - deltas -> `project_canonical_order_book_deltas_to_catalog` ->
//!   `read_back_order_book_deltas` (action/side/price/size/flags/sequence/ts),
//! - trades -> `project_canonical_trades_to_catalog` -> `read_back_trade_ticks`
//!   (price/size/aggressor/trade_id/ts).
//!
//! This is the dual-fidelity rule in action: the accepted dataset stays the L2
//! archive (L2_REPLAY) and the deltas inherit it, while the trades carry
//! TRADE_REPLAY with claims declared by the mapping.
//!
//! Fixtures are synthetic and venue-free: the adapter is data-driven and must not
//! be tied to any real venue, token, symbol, or incident value. The Parquet
//! object is built in memory through the arrow writer the crate already depends
//! on. The accepted dataset is built through the public source-proof gate with a
//! synthetic source-binding registry, since [`AcceptedDataset`] cannot be
//! constructed outside that gate.

use std::sync::Arc;

use arrow::{
    array::{ArrayRef, RecordBatch, StringArray},
    datatypes::{DataType, Field, Schema},
};
use backtesting_vertical_slice::{
    canonical_market_data::{CanonicalOrderBookDeltasTable, DeltaAction, DeltaSide},
    canonical_order_book_deltas::{
        DeltaInstrumentIdentities, DeltaMappingConfig, DeltaPriceSignPolicy, DeltaSourceFormat,
        EmptyBookPolicy, EventStreamMappingFields, InstrumentKeySpec, OrderingAuthority,
        normalize_parquet_event_stream_deltas,
    },
    canonical_trades::{CanonicalInstrumentIdentity, CanonicalTradesTable, CsvTimestampUnit},
    catalog_projection::{
        BinaryOptionInstrumentKind, BinaryOptionInstrumentSpec, SpotInstrumentSpec,
        project_canonical_order_book_deltas_to_catalog, project_canonical_trades_to_catalog,
        read_back_order_book_deltas, read_back_trade_ticks,
    },
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange, select_accepted_dataset_with_registry,
    },
};
use nautilus_model::{
    enums::{AggressorSide, BookAction, OrderSide, RecordFlag},
    instruments::InstrumentAny,
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use parquet::arrow::ArrowWriter;

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";

fn test_catalog_encoding() -> backtesting_vertical_slice::artifact_store::CatalogEncodingConfig {
    backtesting_vertical_slice::artifact_store::CatalogEncodingConfig::new(
        5000,
        5000,
        backtesting_vertical_slice::artifact_store::CatalogCompression::Snappy,
    )
    .expect("positive test catalog encoding")
}
const SOURCE_URL: &str = "https://synthetic.invalid/data";
const TRADE_CLAIM: &str = "No order-book-imbalance claims from trade prints.";

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
/// the SAME accepted prediction-market parquet object — once normalized —
/// projects through NT's `BinaryOption` constructor end-to-end (parquet ->
/// normalize -> project with a BinaryOption spec). The accepted dataset's
/// fixture type is already `BinaryOption`.
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
        // Optional risk and bound metadata is outside this adapter fixture's scope.
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

fn identities() -> DeltaInstrumentIdentities {
    DeltaInstrumentIdentities::Single(CanonicalInstrumentIdentity {
        instrument_id: INSTRUMENT_ID.to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
    })
}

fn mapping() -> DeltaMappingConfig {
    DeltaMappingConfig {
        format: DeltaSourceFormat::EventStream(Box::new(EventStreamMappingFields {
            event_type_field: "event_type".to_string(),
            snapshot_event_value: "book".to_string(),
            level_change_event_value: "price_change".to_string(),
            trade_event_value: "last_trade".to_string(),
            dropped_event_values: vec!["tick_size_change".to_string()],
            side_field: "side".to_string(),
            buy_side_values: vec!["BUY".to_string()],
            sell_side_values: vec!["SELL".to_string()],
            price_field: "price".to_string(),
            size_field: "size".to_string(),
            bids_field: "bids".to_string(),
            asks_field: "asks".to_string(),
            capture_time_field: "capture_time".to_string(),
            capture_time_unit: CsvTimestampUnit::Milliseconds,
            tiebreak_is_row_index: true,
            trade_price_field: "trade_price".to_string(),
            trade_size_field: "trade_size".to_string(),
            trade_id_field: None,
            event_time_field: None,
            event_time_unit: None,
            trade_forbidden_claims: vec![TRADE_CLAIM.to_string()],
        })),
        instrument_key: InstrumentKeySpec {
            key_field: None,
            exclusion_filter: None,
        },
        ordering: OrderingAuthority::CaptureTime,
        price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
        empty_book_policy: EmptyBookPolicy::LoneClearLast,
    }
}

/// One synthetic typed-event row. Every column is a nullable string.
#[derive(Clone, Default)]
struct Row {
    event_type: &'static str,
    capture_time: Option<&'static str>,
    bids: Option<&'static str>,
    asks: Option<&'static str>,
    price: Option<&'static str>,
    size: Option<&'static str>,
    side: Option<&'static str>,
    trade_price: Option<&'static str>,
    trade_size: Option<&'static str>,
}

/// Build the in-memory typed-event Parquet object the adapter normalizes.
fn build_parquet(rows: &[Row]) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("event_type", DataType::Utf8, true),
        Field::new("capture_time", DataType::Utf8, true),
        Field::new("bids", DataType::Utf8, true),
        Field::new("asks", DataType::Utf8, true),
        Field::new("price", DataType::Utf8, true),
        Field::new("size", DataType::Utf8, true),
        Field::new("side", DataType::Utf8, true),
        Field::new("trade_price", DataType::Utf8, true),
        Field::new("trade_size", DataType::Utf8, true),
    ]));
    let column = |pick: fn(&Row) -> Option<&'static str>| -> ArrayRef {
        Arc::new(StringArray::from(rows.iter().map(pick).collect::<Vec<_>>()))
    };
    let event_type_col: ArrayRef = Arc::new(StringArray::from(
        rows.iter().map(|r| Some(r.event_type)).collect::<Vec<_>>(),
    ));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            event_type_col,
            column(|r| r.capture_time),
            column(|r| r.bids),
            column(|r| r.asks),
            column(|r| r.price),
            column(|r| r.size),
            column(|r| r.side),
            column(|r| r.trade_price),
            column(|r| r.trade_size),
        ],
    )
    .expect("synthetic event record batch");
    let mut buffer = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buffer, schema, None).expect("parquet writer");
    writer.write(&batch).expect("write parquet batch");
    writer.close().expect("finalize parquet");
    buffer
}

fn fixture_rows() -> Vec<Row> {
    vec![
        // A full photo: two bids, one ask.
        Row {
            event_type: "book",
            capture_time: Some("1700000000000"),
            bids: Some("[[\"0.49\",\"10\"],[\"0.48\",\"7\"]]"),
            asks: Some("[[\"0.51\",\"12\"]]"),
            ..Row::default()
        },
        // A standalone level change (UPDATE).
        Row {
            event_type: "price_change",
            capture_time: Some("1700000001000"),
            side: Some("BUY"),
            price: Some("0.47"),
            size: Some("4"),
            ..Row::default()
        },
        // A trade print.
        Row {
            event_type: "last_trade",
            capture_time: Some("1700000002000"),
            side: Some("SELL"),
            trade_price: Some("0.50"),
            trade_size: Some("6"),
            ..Row::default()
        },
        // A dropped tick-size change: produces no row in either family.
        Row {
            event_type: "tick_size_change",
            capture_time: Some("1700000003000"),
            ..Row::default()
        },
    ]
}

fn source_binding_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(
        r#"[[source_binding]]
key = "testvenue-deltas"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["order_book_snapshot_deltas"]
"#,
    )
    .expect("synthetic source binding registry parses")
}

fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
    claims
        .iter()
        .enumerate()
        .map(|(index, claim)| SourceProofClaimLimit {
            id: format!("claim-limit-{}", index + 1),
            severity: "blocking".to_string(),
            claim: claim.clone(),
            reason: "source fidelity does not prove this claim".to_string(),
            evidence_ref: "source-proof://fidelity-class".to_string(),
        })
        .collect()
}

fn accepted_dataset() -> AcceptedDataset {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.parquet".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 4096,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec!["l2_event_stream_parquet".to_string()],
    };
    let forbidden_claims = vec!["No execution-quality claims.".to_string()];
    let checks = |evidence: &str| RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation"),
        schema: RequiredCheck::passed("schema"),
        time_semantics: RequiredCheck::passed("ms_to_nanos"),
        instrument_universe: RequiredCheck::passed("universe"),
        coverage: RequiredCheck::passed(evidence),
        retention_freshness: RequiredCheck::passed("retention"),
        granularity: RequiredCheck::passed("l2_event_stream"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("OrderBookDelta"),
        cost: RequiredCheck::passed("free"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let proof = SourceProofReport {
        source_proof_id: "source-proof-synthetic-event-stream".to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "testvenue-deltas".to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        table_family: "order_book_snapshot_deltas".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        source_candidate_class: SourceCandidateClass::OfficialFree,
        source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        official_free_gap_ref: None,
        paid_vendor_gap_ref: None,
        fixture_type: FixtureType::BinaryOption,
        requested_time_range: TimeRange {
            start_utc: "2025-06-01T00:00:00Z".to_string(),
            end_utc: "2026-06-01T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-05-22T00:00:00Z".to_string(),
            end_utc: "2026-05-23T00:00:00Z".to_string(),
        },
        instrument_universe_id: "testvenue-deltas-instruments-2026-05-22".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://synthetic.invalid/".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::L2Replay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: Some(
                "source-proof://no-tick-size-change-universe".to_string(),
            ),
            timed_instrument_epoch_replay_ref: None,
        },
        forbidden_claims: forbidden_claims.clone(),
        claim_limits: claim_limits_for(&forbidden_claims),
        cross_market_components: Vec::new(),
        acceptance_scope: Some(AcceptanceScope {
            planned_objects: 1,
            completed_objects: 1,
            failed_objects: 0,
            skipped_objects: 0,
            accepted_bytes: object.bytes,
            selector_scope_violations: 0,
        }),
        gap_policy_id: String::new(),
        required_checks: checks("manifest://synthetic"),
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept_with_registry(
        &source_binding_registry(),
        AcceptanceMode::Manual,
        "operator",
        "2026-06-02T00:00:00Z",
    )
    .expect("accept source proof");
    select_accepted_dataset_with_registry(
        &proof,
        &object,
        &object.sha256,
        &source_binding_registry(),
    )
    .expect("select accepted dataset")
}

fn normalized() -> (
    AcceptedDataset,
    CanonicalOrderBookDeltasTable,
    CanonicalTradesTable,
) {
    let accepted = accepted_dataset();
    let parquet = build_parquet(&fixture_rows());
    let (mut deltas, mut trades) = normalize_parquet_event_stream_deltas(
        &accepted,
        &identities(),
        &mapping(),
        parquet.into(),
        42,
        "ingest-run-test",
    )
    .expect("normalize parquet event stream");
    assert_eq!(
        deltas.len(),
        1,
        "single-instrument object => one deltas table"
    );
    assert_eq!(trades.len(), 1, "trade present => one trades table");
    (accepted, deltas.remove(0), trades.remove(0))
}

#[test]
fn event_stream_deltas_round_trip_to_catalog() {
    let (_accepted, table, _trades) = normalized();
    // Photo: CLEAR + 2 bid ADD + 1 ask ADD (4) + standalone UPDATE (1) = 5 rows.
    assert_eq!(table.rows.len(), 5);
    assert_eq!(table.fidelity_class, SourceProofFidelityClass::L2Replay);

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project deltas");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );

    let mut loaded =
        read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back deltas");
    assert_eq!(loaded.len(), table.rows.len());
    loaded.sort_by_key(|delta| delta.sequence);
    for (delta, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(delta.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(delta.sequence, row.sequence);
        assert_eq!(delta.flags, row.flags);
        assert_eq!(delta.ts_event.as_u64(), row.event_time as u64);
        if row.action == DeltaAction::Clear.as_str() {
            assert_eq!(delta.action, BookAction::Clear);
        } else {
            let expected_action = if row.action == DeltaAction::Add.as_str() {
                BookAction::Add
            } else if row.action == DeltaAction::Update.as_str() {
                BookAction::Update
            } else {
                BookAction::Delete
            };
            assert_eq!(delta.action, expected_action);
            assert_eq!(
                delta.order.price.as_decimal(),
                Price::from(row.price.as_str()).as_decimal()
            );
            assert_eq!(
                delta.order.size.as_decimal(),
                Quantity::from(row.size.as_str()).as_decimal()
            );
            let expected_side = if row.side == DeltaSide::Buy.as_str() {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };
            assert_eq!(delta.order.side, expected_side);
        }
    }
}

#[test]
fn event_stream_deltas_round_trip_through_binary_option_spec() {
    // End-to-end: an accepted prediction-market parquet object normalizes, then
    // projects through the generic catalog seam bound to a BinaryOption
    // instrument — the same path spot/perp/future already prove, now for the
    // binary-option family.
    let (_accepted, table, _trades) = normalized();
    assert_eq!(table.rows.len(), 5);
    assert_eq!(table.fidelity_class, SourceProofFidelityClass::L2Replay);

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_canonical_order_book_deltas_to_catalog(
        &table,
        &binary_option_spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project deltas via binary option spec");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);

    // The catalog instrument is an NT BinaryOption (not a CurrencyPair).
    let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let instruments = catalog
        .query_instruments(Some(&[NT_INSTRUMENT_ID.to_string()]))
        .expect("query instruments");
    assert_eq!(instruments.len(), 1);
    assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));

    let mut loaded =
        read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back deltas");
    assert_eq!(loaded.len(), table.rows.len());
    loaded.sort_by_key(|delta| delta.sequence);
    for (delta, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(delta.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(delta.sequence, row.sequence);
        assert_eq!(delta.flags, row.flags);
        if row.action == DeltaAction::Clear.as_str() {
            assert_eq!(delta.action, BookAction::Clear);
        } else {
            assert_eq!(
                delta.order.price.as_decimal(),
                Price::from(row.price.as_str()).as_decimal()
            );
            assert_eq!(
                delta.order.size.as_decimal(),
                Quantity::from(row.size.as_str()).as_decimal()
            );
        }
    }
}

#[test]
fn event_stream_expansion_shape_survives_round_trip() {
    let (_accepted, table, _trades) = normalized();
    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project");
    let mut loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    loaded.sort_by_key(|delta| delta.sequence);

    let last = RecordFlag::F_LAST as u8;
    // Snapshot: CLEAR, bid ADD, bid ADD, ask ADD (F_LAST on the ask).
    assert_eq!(loaded[0].action, BookAction::Clear);
    assert_eq!(loaded[0].flags & last, 0, "snapshot CLEAR does not close");
    assert_eq!(loaded[3].action, BookAction::Add);
    assert_ne!(
        loaded[3].flags & last,
        0,
        "final snapshot row carries F_LAST"
    );
    // The standalone level change is its own self-closing UPDATE event.
    assert_eq!(loaded[4].action, BookAction::Update);
    assert_ne!(loaded[4].flags & last, 0, "standalone delta carries F_LAST");
    assert_eq!(
        loaded[4].flags & RecordFlag::F_SNAPSHOT as u8,
        0,
        "standalone delta is not a snapshot row"
    );
}

#[test]
fn event_stream_trades_round_trip_to_catalog() {
    let (_accepted, _deltas, table) = normalized();
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.fidelity_class, SourceProofFidelityClass::TradeReplay);
    assert_ne!(table.fidelity_class, SourceProofFidelityClass::L2Replay);
    assert_eq!(table.forbidden_claims, vec![TRADE_CLAIM.to_string()]);

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_canonical_trades_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project trades");
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::TradeReplay
    );

    let loaded = read_back_trade_ticks(dir.path(), NT_INSTRUMENT_ID).expect("read back trades");
    assert_eq!(loaded.len(), table.rows.len());
    for (tick, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(tick.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(tick.trade_id.to_string(), row.trade_id);
        assert_eq!(tick.ts_event.as_u64(), row.event_time as u64);
        assert_eq!(
            tick.price.as_decimal(),
            Price::from(row.price.as_str()).as_decimal()
        );
        assert_eq!(
            tick.size.as_decimal(),
            Quantity::from(row.size.as_str()).as_decimal()
        );
        let expected_aggressor = if row.aggressor_side == "BUYER" {
            AggressorSide::Buyer
        } else {
            AggressorSide::Seller
        };
        assert_eq!(tick.aggressor_side, expected_aggressor);
    }
    // The trade in the fixture is a SELL print => Seller aggressor.
    assert_eq!(loaded[0].aggressor_side, AggressorSide::Seller);
    // Synthetic per-instrument ordinal trade id.
    assert_eq!(loaded[0].trade_id.to_string(), "0");
}
