use backtesting_vertical_slice::first_proof_selector::{
    AssetEventCount, FIRST_PROOF_SELECTOR_REPORT_FILE, FirstProofEventCountLedger,
    FirstProofSelection, FirstProofSelectorReport, FirstProofSelectorStatus,
    evaluate_first_proof_selector, write_first_proof_selector_report_from_spec_file,
};

#[test]
fn first_proof_selector_uses_configured_event_roles_without_asset_constants() {
    let selection = FirstProofSelection {
        required_event_families: vec![
            "snapshot".to_string(),
            "update".to_string(),
            "execution".to_string(),
        ],
        excluded_event_families: vec!["instrument_epoch".to_string()],
        row_budget: 10,
        max_selected_assets: 2,
    };
    let event_counts = vec![
        count("asset-over-budget", "snapshot", 1),
        count("asset-over-budget", "update", 10),
        count("asset-over-budget", "execution", 1),
        count("asset-excluded", "snapshot", 1),
        count("asset-excluded", "update", 2),
        count("asset-excluded", "execution", 1),
        count("asset-excluded", "instrument_epoch", 1),
        count("asset-missing-required", "snapshot", 1),
        count("asset-missing-required", "update", 2),
        count("asset-two", "snapshot", 1),
        count("asset-two", "update", 3),
        count("asset-two", "execution", 1),
        count("asset-one", "snapshot", 1),
        count("asset-one", "update", 1),
        count("asset-one", "execution", 1),
    ];

    let report = evaluate_first_proof_selector("bounded-l2-first-proof", &event_counts, &selection);

    assert_eq!(report.status, FirstProofSelectorStatus::Selected);
    assert!(report.blocking_issues.is_empty());
    assert_eq!(report.total_assets, 5);
    assert_eq!(report.eligible_assets, 2);
    assert_eq!(report.excluded_event_asset_count, 1);
    assert_eq!(report.excluded_event_row_count, 1);
    assert!(!report.event_count_ledger_hash.is_empty());
    assert_eq!(
        report
            .selected_assets
            .iter()
            .map(|asset| (asset.asset_id.as_str(), asset.replay_rows))
            .collect::<Vec<_>>(),
        vec![("asset-one", 3), ("asset-two", 5)]
    );
    assert!(!report.selected_asset_ids_hash.is_empty());

    let second = evaluate_first_proof_selector("bounded-l2-first-proof", &event_counts, &selection);
    assert_eq!(
        report.selected_asset_ids_hash,
        second.selected_asset_ids_hash
    );
}

#[test]
fn first_proof_selector_writer_is_config_and_ledger_driven_and_idempotent() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let ledger_path = dir.path().join("event-count-ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("selector.toml");
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&FirstProofEventCountLedger {
            event_counts: vec![
                count("asset-two", "snapshot", 1),
                count("asset-two", "update", 3),
                count("asset-two", "execution", 1),
                count("asset-one", "snapshot", 1),
                count("asset-one", "update", 1),
                count("asset-one", "execution", 1),
            ],
        })
        .expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"selector_id = "bounded-l2-first-proof"
event_count_ledger_path = "{}"
output_dir = "{}"

[selection]
required_event_families = ["snapshot", "update", "execution"]
excluded_event_families = ["instrument_epoch"]
row_budget = 10
max_selected_assets = 1
"#,
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write selector spec");

    let first = write_first_proof_selector_report_from_spec_file(&spec_path).expect("first");
    let second = write_first_proof_selector_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(FIRST_PROOF_SELECTOR_REPORT_FILE)
    );
    assert_eq!(first.selected_asset_count, 1);

    let report: FirstProofSelectorReport =
        serde_json::from_slice(&std::fs::read(first.path).expect("read report"))
            .expect("selector report json");
    assert_eq!(report.status, FirstProofSelectorStatus::Selected);
    assert_eq!(
        report
            .selected_assets
            .iter()
            .map(|asset| asset.asset_id.as_str())
            .collect::<Vec<_>>(),
        vec!["asset-one"]
    );
    assert!(!report.event_count_ledger_hash.is_empty());
    assert!(!report.selected_asset_ids_hash.is_empty());
}

fn count(asset_id: &str, event_family: &str, rows: u64) -> AssetEventCount {
    AssetEventCount {
        asset_id: asset_id.to_string(),
        event_family: event_family.to_string(),
        rows,
    }
}
