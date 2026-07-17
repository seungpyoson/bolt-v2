//! End-to-end proof for the paged-JSON (F2) and JSONL multi-interval (F3) bar
//! source adapters.
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that an accepted bar object normalizes through the F2/F3 entry points
//! into validated [`CanonicalBarsTable`]s and projects into a local
//! `ParquetDataCatalog` as externally-aggregated `Bar` data that reads back with
//! `ts_event == close_time`, `ts_init == capture_time` (the receipt clock
//! NautilusTrader replays by), and per-field OHLCV equality.
//!
//! F2 (paged REST JSON) is per-instrument: one envelope (or several
//! newline-separated page envelopes) yields one table. F3 (line-delimited
//! multi-interval) emits one table per `(instrument, interval)` group; since each
//! interval is a distinct NautilusTrader bar type, each table projects into its
//! own clean catalog root.
//!
//! Fixtures are synthetic and venue-free: the adapters are data-driven and must
//! not be tied to any real venue, token, symbol, or incident value. The accepted
//! dataset is built through the public source-proof gate with a synthetic
//! source-binding registry, since [`AcceptedDataset`] cannot be constructed
//! outside that gate.

use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    canonical_bars::{
        BarInstrumentIdentities, BarIntervalToken, BarPriceSignPolicy, DeclaredBarInterval,
        JsonlBarMappingConfig, PagedJsonBarMappingConfig, PagedJsonRowShape,
        normalize_jsonl_multi_interval_bars, normalize_paged_json_bars,
    },
    canonical_market_data::CanonicalBarsTable,
    canonical_trades::{CanonicalInstrumentIdentity, CsvTimestampUnit},
    catalog_projection::{SpotInstrumentSpec, project_canonical_bars_to_catalog, read_back_bars},
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange, select_accepted_dataset_with_registry,
    },
};
use nautilus_model::{
    enums::BarAggregation,
    types::{Price, Quantity},
};

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

fn single_identity() -> CanonicalInstrumentIdentity {
    CanonicalInstrumentIdentity {
        instrument_id: INSTRUMENT_ID.to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
    }
}

fn source_binding_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(
        r#"[[source_binding]]
key = "testvenue-bars"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["bars"]
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

fn accepted_dataset(schema_columns: &[&str]) -> AcceptedDataset {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.json".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 4096,
        archive_date: "2026-05-22".to_string(),
        schema_columns: schema_columns.iter().map(ToString::to_string).collect(),
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
        granularity: RequiredCheck::passed("aggregated_bars"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("Bar"),
        cost: RequiredCheck::passed("free"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let proof = SourceProofReport {
        source_proof_id: "source-proof-synthetic-bars".to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "testvenue-bars".to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        table_family: "bars".to_string(),
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
        instrument_universe_id: "testvenue-bars-instruments-2026-05-22".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://synthetic.invalid/".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: None,
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
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

/// Project one table and read it back, asserting per-field OHLCV + timestamp
/// equality. Each table is its own bar type, so each gets a clean catalog root.
fn assert_round_trip(table: &CanonicalBarsTable) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_canonical_bars_to_catalog(table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project bars");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::TradeBarReplay
    );

    let mut loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back bars");
    assert_eq!(loaded.len(), table.rows.len());
    loaded.sort_by_key(|bar| bar.ts_event.as_u64());
    for (bar, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(bar.instrument_id().to_string(), NT_INSTRUMENT_ID);
        // ts_event is the canonical close_time; ts_init is the receipt clock
        // NautilusTrader replays by (capture_time here — no availability column).
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
            Quantity::from(row.volume.as_str()).as_decimal()
        );
    }
}

#[test]
fn paged_json_bars_round_trip_to_catalog() {
    let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
    // Two newline-separated page envelopes, rows newest-first, overlapping on the
    // boundary minute (which collapses). Positional array rows, no close_time.
    let page_one = r#"{"result":{"list":[["1700000060000","0.52","0.58","0.51","0.57","120"],["1700000000000","0.50","0.55","0.49","0.52","100"]]}}"#;
    let page_two = r#"{"result":{"list":[["1700000120000","0.57","0.60","0.56","0.59","90"],["1700000060000","0.52","0.58","0.51","0.57","120"]]}}"#;
    let json = format!("{page_one}\n{page_two}");

    let mapping = PagedJsonBarMappingConfig {
        rows_path: "result.list".to_string(),
        row_shape: PagedJsonRowShape::PositionalArray {
            open_time_index: 0,
            open_index: 1,
            high_index: 2,
            low_index: 3,
            close_index: 4,
            volume_index: 5,
            close_time_index: None,
        },
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        interval: DeclaredBarInterval {
            step: 1,
            aggregation: BarAggregation::Minute,
        },
        price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
    };

    let tables = normalize_paged_json_bars(
        &accepted,
        &single_identity(),
        &mapping,
        &json,
        42,
        "ingest-run-test",
    )
    .expect("normalize paged json bars");
    assert_eq!(tables.len(), 1, "paged REST is per-instrument: one table");
    let table = &tables[0];
    // Boundary minute collapsed: three distinct minutes across both pages.
    assert_eq!(table.rows.len(), 3);
    assert_eq!(table.bar_spec.aggregation, BarAggregation::Minute);
    assert_eq!(table.rows[0].open_time, 1_700_000_000_000_000_000);

    assert_round_trip(table);
}

#[test]
fn jsonl_multi_interval_bars_round_trip_to_catalog() {
    let accepted = accepted_dataset(&["interval", "t", "ct", "o", "h", "l", "c", "v"]);
    // One instrument, two intervals interleaved (1m + 1h). Each interval is a
    // distinct bar type, so each projects into its own clean catalog root.
    let jsonl = concat!(
        r#"{"interval":"1m","t":"1700000000000","ct":"1700000060000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100"}"#,
        "\n",
        r#"{"interval":"1h","t":"1700000000000","ct":"1700003600000","o":"0.50","h":"0.60","l":"0.48","c":"0.59","v":"500"}"#,
        "\n",
        r#"{"interval":"1m","t":"1700000060000","ct":"1700000120000","o":"0.52","h":"0.58","l":"0.51","c":"0.57","v":"120"}"#,
        "\n",
        r#"{"interval":"1h","t":"1700003600000","ct":"1700007200000","o":"0.59","h":"0.65","l":"0.55","c":"0.62","v":"450"}"#,
    );

    let mapping = JsonlBarMappingConfig {
        instrument_field: None,
        interval_field: "interval".to_string(),
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        open_time_field: "t".to_string(),
        close_time_field: Some("ct".to_string()),
        open_field: "o".to_string(),
        high_field: "h".to_string(),
        low_field: "l".to_string(),
        close_field: "c".to_string(),
        volume_field: "v".to_string(),
        interval_token_map: BTreeMap::from([
            (
                "1m".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Minute,
                },
            ),
            (
                "1h".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Hour,
                },
            ),
        ]),
        price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
    };

    let tables = normalize_jsonl_multi_interval_bars(
        &accepted,
        &BarInstrumentIdentities::Single(single_identity()),
        &mapping,
        jsonl,
        42,
        "ingest-run-test",
    )
    .expect("normalize jsonl multi-interval bars");
    assert_eq!(tables.len(), 2, "two intervals -> two tables");

    let minute_table = tables
        .iter()
        .find(|table| table.bar_spec.aggregation == BarAggregation::Minute)
        .expect("minute table present");
    let hour_table = tables
        .iter()
        .find(|table| table.bar_spec.aggregation == BarAggregation::Hour)
        .expect("hour table present");
    assert_eq!(minute_table.rows.len(), 2);
    assert_eq!(hour_table.rows.len(), 2);

    // Each interval is a distinct bar type and projects into its own root.
    assert_round_trip(minute_table);
    assert_round_trip(hour_table);
}
