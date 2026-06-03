//! End-to-end proof for the NautilusTrader backtesting vertical slice.
//!
//! Drives gates 1 through 6 through [`run_backtest`] with a small deterministic
//! accepted dataset: source-proof acceptance, the accepted-data ledger,
//! canonical normalization + Parquet artifact, NautilusTrader catalog projection
//! + read-back, manifest validation + NautilusTrader config mapping, a
//! `BacktestNode` run of a compiled Rust strategy, and the objective result
//! contract. CI-safe (no network): this test exercises the pipeline with
//! committed synthetic data only. Real-object verification is operator-run-only
//! via the `backtesting_vertical_slice` binary (`--run-spec`, `--object-gz`,
//! `--output-dir`) and is not asserted here.

use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    canonical_trades::CanonicalInstrumentIdentity,
    catalog_projection::SpotInstrumentSpec,
    result_contract::ResultArtifactUris,
    run_manifest::{
        BacktestingRunManifest, ManifestCatalogInput, ManifestVenueConfig, MarketStructureFixture,
        RunPurpose, STRATEGY_HURST_VPIN_DIRECTIONAL, StrategySource,
    },
    runner::{BacktestRunInputs, run_backtest},
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};

const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
    1,1772323201665,617.2,0.3,buy,0\n\
    2,1772323312219,617.9,0.1456,sell,0\n\
    3,1772323312236,617,0.1544,sell,0\n";

const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";
const SOURCE_PROOF_ID: &str = "source-proof-bybit-spot-tick-trades";

fn passing_checks() -> RequiredChecks {
    let evidence = "manifest://bybit-backfill-run-fdcc0758bbd03113";
    RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation://bybit-public-archive"),
        schema: RequiredCheck::passed("schema://id,timestamp,price,volume,side,rpi"),
        time_semantics: RequiredCheck::passed("unix_ms_to_unix_nanos"),
        instrument_universe: RequiredCheck::passed("universe://bybit-spot-2026-03-01"),
        coverage: RequiredCheck::passed(evidence),
        granularity: RequiredCheck::passed("native_trade_prints"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("nt://TradeTick"),
        storage: RequiredCheck::passed("s3://bolt-parquet/.../source-proofs/"),
    }
}

fn accepted_dataset() -> AcceptedDataset {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/dt=2026-03-01/symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
        source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 8505,
        archive_date: "2026-03-01".to_string(),
        schema_columns: ["id", "timestamp", "price", "volume", "side", "rpi"]
            .iter()
            .map(ToString::to_string)
            .collect(),
    };
    let proof = SourceProofReport {
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "bybit-spot-tick-trades".to_string(),
        venue: "bybit".to_string(),
        product_family: "spot".to_string(),
        product_category: "spot".to_string(),
        table_family: "trades".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        fixture_type: FixtureType::PerpsSpot,
        requested_time_range: TimeRange {
            start_utc: "2025-06-01T00:00:00Z".to_string(),
            end_utc: "2026-06-01T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-03-01T00:00:00Z".to_string(),
            end_utc: "2026-03-02T00:00:00Z".to_string(),
        },
        instrument_universe_id: "bybit-spot-instruments-2026-03-01".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://bolt-parquet/.../schema-sample.json".to_string(),
        schema_sample_hash: "bf26db0b8fb8b62746b5724dccfb26a408d581f5598cb6be95c9173c8b1b5eed"
            .to_string(),
        license_ref: "https://public.bybit.com/ (attestation 2026-06-02)".to_string(),
        retention_ref: "https://public.bybit.com/ (retention reviewed 2026-06-02)".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        // Mirrors the committed reference source proof exactly so the fixture
        // carries the full set of fidelity constraints through to the contract.
        forbidden_claims: vec![
            "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
            "No L2/L3 order-book replay claims from trade prints.".to_string(),
            "Coverage is limited to BNBUSDC spot 2026-03-01; no multi-day or multi-instrument claims."
                .to_string(),
        ],
        gap_policy_id: String::new(),
        required_checks: passing_checks(),
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept(
        AcceptanceMode::Manual,
        "vertical-slice-operator",
        "2026-06-02T00:00:00Z",
    )
    .expect("accept source proof");
    select_accepted_dataset(&proof, &object, OBJECT_SHA256).expect("select accepted dataset")
}

fn instrument_spec() -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        raw_symbol: "BNBUSDC".to_string(),
        base_currency: "BNB".to_string(),
        quote_currency: "USDC".to_string(),
        price_increment: "0.1".to_string(),
        size_increment: "0.0001".to_string(),
        min_quantity: "0.0001".to_string(),
        max_quantity: "1400".to_string(),
        min_notional: "5".to_string(),
        max_notional: "200000".to_string(),
    }
}

fn manifest(catalog_path: &str) -> BacktestingRunManifest {
    BacktestingRunManifest {
        run_id: "backtesting-vertical-slice-end-to-end".to_string(),
        market_structure_fixture: MarketStructureFixture::PerpsSpot,
        venue_binding_key: "bybit-spot-tick-trades".to_string(),
        run_purpose: RunPurpose::Normal,
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        pins_non_latest_proof: false,
        strategy: StrategySource {
            registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
            parameters: BTreeMap::from([
                ("trade_size".to_string(), "0.01".to_string()),
                (
                    "bar_type".to_string(),
                    "BNBUSDC.BYBIT-1-MINUTE-LAST-INTERNAL".to_string(),
                ),
            ]),
        },
        venue: ManifestVenueConfig {
            nt_venue: "BYBIT".to_string(),
            oms_type: "NETTING".to_string(),
            account_type: "CASH".to_string(),
            book_type: "L1_MBP".to_string(),
            starting_balances: vec!["1_000_000 USDC".to_string()],
        },
        catalog_input: ManifestCatalogInput {
            catalog_path: catalog_path.to_string(),
            data_type: "TradeTick".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        },
        artifact_root: "s3://bolt-parquet/nt-research-analytics".to_string(),
        output_prefix:
            "s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-end-to-end"
                .to_string(),
        start_time: None,
        end_time: None,
    }
}

#[test]
fn accepted_data_flows_through_to_objective_result_contract() {
    let accepted = accepted_dataset();
    let identity = CanonicalInstrumentIdentity {
        instrument_id: "BNBUSDC".to_string(),
        venue_symbol: "BNBUSDC".to_string(),
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let canonical_path = temp.path().join("canonical-trades.parquet");
    let catalog_root = temp.path().join("nt-catalog");
    let catalog_path = catalog_root.to_str().unwrap().to_string();

    let output = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &identity,
        instrument_spec: &instrument_spec(),
        csv_text: SAMPLE_CSV,
        capture_time_nanos: 1_772_512_022_000_000_000,
        manifest: &manifest(&catalog_path),
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: canonical_path.to_string_lossy().to_string(),
            nt_catalog_uri: catalog_path.clone(),
            result_contract_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/end-to-end/result.json"
                    .to_string(),
        },
    })
    .expect("end-to-end backtest run");

    // Gate 2: canonical normalization.
    assert_eq!(output.canonical_table.rows.len(), 3);
    assert!(
        canonical_path.exists(),
        "canonical artifact must be written"
    );

    // Gate 3: catalog projection + read-back proof.
    assert_eq!(output.projection.trade_count, 3);
    assert_eq!(output.read_back_count, 3);
    assert!(!output.projection.catalog_hash.is_empty());

    // Gate 5: NautilusTrader BacktestNode produced a result over the trades.
    assert_eq!(
        output.nt_result.run_config_id.as_deref(),
        Some("backtesting-vertical-slice-end-to-end")
    );
    // Trade-only TRADE_REPLAY data carries no quotes, so NautilusTrader's
    // order/fill counters stay at zero; `total_events` counts execution events,
    // not trade ticks fed, so it is 0 here even though the engine consumed the
    // accepted trades. The proof that the engine ran over the accepted data is
    // the gate-3 read-back (3 ticks) bound to the same instrument id the engine
    // queries — not these counters.
    assert_eq!(
        output.nt_result.total_orders, 0,
        "trade-only TRADE_REPLAY data is quote-free, so the quote-driven strategy places no orders"
    );
    assert_eq!(output.nt_result.total_positions, 0);

    // Gate 6: objective result contract.
    let contract = &output.contract;
    contract
        .validate()
        .expect("contract is objective and complete");
    assert_eq!(contract.source_proof_id, SOURCE_PROOF_ID);
    assert_eq!(contract.source_proof_version, 1);
    assert_eq!(contract.nt_version.len(), 40);
    assert_eq!(
        contract.fidelity_class,
        SourceProofFidelityClass::TradeReplay
    );
    // The zero-orders warning is emitted with the honest TRADE_REPLAY rationale,
    // and the claim limits carry the full reference set forward.
    assert_eq!(contract.warnings.len(), 1);
    assert!(
        contract.warnings[0].contains("TRADE_REPLAY"),
        "warning must explain the trade-only fidelity: {:?}",
        contract.warnings[0]
    );
    assert_eq!(contract.claim_limits.len(), 3);
    assert_eq!(contract.catalog_hash, output.projection.catalog_hash);
}
