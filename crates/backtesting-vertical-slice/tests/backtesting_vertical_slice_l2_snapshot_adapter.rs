//! End-to-end proof for the config-driven JSONL periodic-full-snapshot L2 delta
//! source adapter (format family S3).
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that an accepted JSONL snapshot object normalizes through
//! [`normalize_jsonl_snapshot_deltas`] into a validated
//! [`CanonicalOrderBookDeltasTable`] and projects into a local
//! `ParquetDataCatalog` as `OrderBookDelta` data that reads back with per-field
//! equality (action/side/price/size/flags/native-sequence/ts) and the snapshot
//! expansion shape preserved, and that a gzip envelope decodes to the same
//! normalized table.
//!
//! Fixtures are synthetic and venue-free: the adapter is data-driven and must
//! not be tied to any real venue, token, symbol, or incident value. The accepted
//! dataset is built through the public source-proof gate with a synthetic
//! source-binding registry, since [`AcceptedDataset`] cannot be constructed
//! outside that gate.

use std::io::{Read, Write};

use backtesting_vertical_slice::{
    canonical_market_data::{CanonicalOrderBookDeltasTable, DeltaAction, DeltaSide},
    canonical_order_book_deltas::{
        DeltaInstrumentIdentities, DeltaMappingConfig, DeltaPriceSignPolicy, DeltaSourceFormat,
        EmptyBookPolicy, InstrumentKeySpec, OrderingAuthority, SnapshotMappingFields,
        normalize_jsonl_snapshot_deltas,
    },
    canonical_trades::{CanonicalInstrumentIdentity, CsvTimestampUnit},
    catalog_projection::{
        SpotInstrumentSpec, project_canonical_order_book_deltas_to_catalog,
        read_back_order_book_deltas,
    },
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange, select_accepted_dataset_with_registry,
    },
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use nautilus_model::{
    enums::{BookAction, OrderSide, RecordFlag},
    types::{Price, Quantity},
};

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";
const SOURCE_URL: &str = "https://synthetic.invalid/data";

// Two full photos one minute apart. Each photo carries two bid levels and one
// ask level; the event time is unix-ms and becomes the read-back `ts_event`.
const SNAPSHOT_JSONL: &str = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"},{\"px\":\"0.48\",\"sz\":\"7\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
    {\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"},{\"px\":\"0.49\",\"sz\":\"6\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";

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

fn identities() -> DeltaInstrumentIdentities {
    DeltaInstrumentIdentities::Single(CanonicalInstrumentIdentity {
        instrument_id: INSTRUMENT_ID.to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
    })
}

fn mapping() -> DeltaMappingConfig {
    DeltaMappingConfig {
        format: DeltaSourceFormat::Snapshot(SnapshotMappingFields {
            bids_field: "bids".to_string(),
            asks_field: "asks".to_string(),
            level_price_field: "px".to_string(),
            level_size_field: "sz".to_string(),
            event_time_field: "time".to_string(),
            event_time_unit: CsvTimestampUnit::Milliseconds,
        }),
        instrument_key: InstrumentKeySpec {
            key_field: None,
            exclusion_filter: None,
        },
        ordering: OrderingAuthority::EventTime,
        price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
        empty_book_policy: EmptyBookPolicy::LoneClearLast,
    }
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
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.jsonl".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 4096,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec!["l2_snapshot_jsonl".to_string()],
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
        granularity: RequiredCheck::passed("l2_snapshot"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("OrderBookDelta"),
        cost: RequiredCheck::passed("free"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let proof = SourceProofReport {
        source_proof_id: "source-proof-synthetic-deltas".to_string(),
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

fn normalized_table() -> CanonicalOrderBookDeltasTable {
    let accepted = accepted_dataset();
    let tables = normalize_jsonl_snapshot_deltas(
        &accepted,
        &identities(),
        &mapping(),
        SNAPSHOT_JSONL,
        42,
        "ingest-run-test",
    )
    .expect("normalize jsonl snapshots");
    assert_eq!(tables.len(), 1, "single-instrument object yields one table");
    tables.into_iter().next().expect("one table")
}

#[test]
fn jsonl_snapshot_deltas_round_trip_to_catalog() {
    let table = normalized_table();
    // Two photos, each CLEAR + 2 bid ADD + 1 ask ADD = 4 rows => 8 rows.
    assert_eq!(table.rows.len(), 8);

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_canonical_order_book_deltas_to_catalog(&table, &spec(), dir.path())
        .expect("project deltas");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );
    assert!(!projection.catalog_hash.is_empty());

    let loaded =
        read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back deltas");
    assert_eq!(loaded.len(), table.rows.len());
    // This format has no venue-native sequence. NT therefore carries zero;
    // canonical `row.sequence` remains an audit-only encounter ordinal.
    for (delta, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(delta.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(delta.sequence, 0);
        assert_eq!(delta.flags, row.flags);
        assert_eq!(delta.ts_event.as_u64(), row.event_time as u64);
        if row.action == DeltaAction::Clear.as_str() {
            assert_eq!(delta.action, BookAction::Clear);
        } else {
            assert_eq!(delta.action, BookAction::Add);
            // Compare numerically: Display renders at instrument precision.
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
fn jsonl_snapshot_expansion_shape_survives_round_trip() {
    let table = normalized_table();
    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_order_book_deltas_to_catalog(&table, &spec(), dir.path()).expect("project");
    let loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    let last = RecordFlag::F_LAST as u8;
    // First photo: CLEAR, bid ADD, bid ADD, ask ADD (F_LAST on the ask).
    assert_eq!(loaded[0].action, BookAction::Clear);
    assert_eq!(loaded[0].flags & last, 0, "snapshot CLEAR does not close");
    assert_eq!(loaded[1].action, BookAction::Add);
    assert_eq!(loaded[2].action, BookAction::Add);
    assert_eq!(loaded[3].action, BookAction::Add);
    assert_ne!(
        loaded[3].flags & last,
        0,
        "final snapshot row carries F_LAST"
    );
    // Second photo begins at row 4 with a fresh CLEAR.
    assert_eq!(loaded[4].action, BookAction::Clear);
    assert_ne!(loaded[7].flags & last, 0, "second photo closes with F_LAST");
    // The source has no native sequence, so NT carries zero. The catalog
    // preserves event encounter order through the non-strict ts_init clock.
    let mut prev_ts = u64::MIN;
    for delta in &loaded {
        assert_eq!(delta.sequence, 0);
        assert!(delta.ts_init.as_u64() >= prev_ts);
        prev_ts = delta.ts_init.as_u64();
    }
}

#[test]
fn jsonl_snapshot_deltas_in_gzip_decodes_and_normalizes() {
    let gz = gzip(SNAPSHOT_JSONL);
    let jsonl_text = gunzip(&gz);
    assert_eq!(jsonl_text, SNAPSHOT_JSONL);

    let accepted = accepted_dataset();
    let tables = normalize_jsonl_snapshot_deltas(
        &accepted,
        &identities(),
        &mapping(),
        &jsonl_text,
        42,
        "ingest-run-test",
    )
    .expect("normalize deltas decoded from gzip");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].rows.len(), 8);
    assert_eq!(tables[0].rows[0].event_time, 1_700_000_000_000_000_000);
    assert_eq!(tables[0].rows[4].event_time, 1_700_000_060_000_000_000);
}

fn gzip(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

fn gunzip(bytes: &[u8]) -> String {
    let mut decoder = GzDecoder::new(bytes);
    let mut text = String::new();
    decoder.read_to_string(&mut text).expect("gunzip read");
    text
}
