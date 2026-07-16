use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    backfill_accepted_tranche::{
        BackfillAcceptedTrancheStatus, evaluate_backfill_accepted_tranche,
    },
    backfill_source_proof_scope::{
        BackfillSourceProofScopeReport, BackfillSourceProofScopeStatus,
        evaluate_backfill_source_proof_scope_for_selected_object,
    },
    hashing::sha256_hex,
    source_proof::{SourceProofReport, SourceProofStatus},
};

const UNIVERSE_SCOPE: &str = "bybit-public-archive-tick-trades-2025-06-01-2026-06-01";

#[test]
fn bybit_source_universe_plan_metadata_and_manifest_remain_exactly_aligned() {
    let reference_root = reference_root();
    let universe = read_json(reference_root.join(format!(
        "backfill-source-universes/{UNIVERSE_SCOPE}/bybit-public-archive-tick-trades-source-universe.json"
    )));
    let plan = read_json(reference_root.join(format!(
        "backfill-source-universe-conversion-plans/{UNIVERSE_SCOPE}/bybit-public-archive-tick-trades-conversion-plan.json"
    )));
    let metadata = read_json(reference_root.join(format!(
        "backfill-instrument-metadata/{UNIVERSE_SCOPE}/bybit-instrument-metadata-snapshot.json"
    )));
    let manifest = read_json(reference_root.join(format!(
        "backfill-source-universe-object-manifests/{UNIVERSE_SCOPE}/bybit-public-archive-tick-trades-object-manifest.json"
    )));

    assert_eq!(universe["schema_version"], "backfill-source-universe.v1");
    assert_eq!(universe["venue"], "bybit");
    assert_eq!(universe["source"], "public_archive");
    assert_eq!(universe["family"], "tick_trades");
    assert_eq!(universe["summary"]["object_count"], 5_857);
    assert_eq!(universe["summary"]["category_count"], 3);
    assert_eq!(universe["summary"]["unique_symbol_count"], 97);
    assert_eq!(universe["summary"]["category_symbol_count"], 106);
    assert_eq!(universe["summary"]["archive_date_count"], 94);
    assert_eq!(universe["summary"]["first_archive_date"], "2025-06-01");
    assert_eq!(universe["summary"]["last_archive_date"], "2026-06-01");
    assert_eq!(universe["summary"]["compressed_bytes"], 20_309_079_098_u64);
    assert_eq!(
        universe["accepted_scope"]["commit_granularity"],
        "venue_source_family_instrument_universe"
    );
    assert_eq!(
        universe["accepted_scope"]["symbol_or_day_commit_granularity"],
        "rejected"
    );

    assert_eq!(
        plan["schema_version"],
        "backfill-source-universe-conversion-plan.v1"
    );
    assert_eq!(plan["universe_id"], universe["universe_id"]);
    assert_eq!(
        plan["selection"]["instrument_universe"],
        "all_staged_category_symbols"
    );
    assert_eq!(
        plan["selection"]["commit_granularity"],
        "venue_source_family_instrument_universe"
    );
    for field in [
        "object_count",
        "category_count",
        "unique_symbol_count",
        "category_symbol_count",
        "archive_date_count",
        "compressed_bytes",
        "first_archive_date",
        "last_archive_date",
    ] {
        assert_eq!(
            plan["source_universe_summary"][field], universe["summary"][field],
            "plan summary field {field} drifted from source universe"
        );
    }

    let universe_categories = category_map(&universe["categories"]);
    let plan_categories = category_map(&plan["category_batches"]);
    assert_eq!(
        universe_categories.keys().copied().collect::<Vec<_>>(),
        vec!["inverse", "linear", "spot"]
    );
    assert_eq!(universe_categories.keys(), plan_categories.keys());
    for (category, universe_category) in &universe_categories {
        let plan_category = plan_categories
            .get(category)
            .expect("plan category must match source universe");
        for field in [
            "source_binding",
            "instrument_count",
            "object_count",
            "compressed_bytes",
            "converter_csv",
        ] {
            assert_eq!(
                plan_category[field], universe_category[field],
                "{category} {field} drifted"
            );
        }
    }

    assert_eq!(
        metadata["schema_version"],
        "bybit-instrument-metadata-snapshot.v1"
    );
    assert_eq!(metadata["universe_id"], universe["universe_id"]);
    assert_eq!(
        metadata["category_symbol_count"],
        universe["summary"]["category_symbol_count"]
    );
    let expected_category_symbols = universe_category_symbols(&universe);
    let metadata_category_symbols = metadata["records"]
        .as_array()
        .expect("metadata records")
        .iter()
        .map(|record| {
            (
                required_str(record, "category").to_string(),
                required_str(record, "symbol").to_string(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(metadata_category_symbols, expected_category_symbols);
    assert!(
        metadata["records"]
            .as_array()
            .expect("metadata records")
            .iter()
            .all(|record| {
                record["api_ret_code"] == 0
                    && record["instrument_count"] == 1
                    && record["instrument"]["symbol"] == record["symbol"]
            })
    );

    assert_eq!(
        manifest["schema_version"],
        "backfill-source-universe-object-manifest.v1"
    );
    assert_eq!(manifest["universe_id"], universe["universe_id"]);
    assert_eq!(
        manifest["object_count"],
        universe["summary"]["object_count"]
    );
    assert_eq!(
        manifest["accepted_bytes"],
        universe["summary"]["compressed_bytes"]
    );
    let payload_records = manifest["payload_records"]
        .as_array()
        .expect("manifest payload records");
    assert_eq!(payload_records.len(), 5_857);
    assert_eq!(
        payload_records
            .iter()
            .map(|record| record["bytes"].as_u64().expect("object bytes"))
            .sum::<u64>(),
        20_309_079_098
    );
    assert!(payload_records.iter().all(|record| {
        let category = required_str(record, "category");
        let symbol = required_str(record, "symbol");
        let date = required_str(record, "archive_date");
        let digest = required_str(record, "sha256");
        let uri = required_str(record, "s3_uri");
        expected_category_symbols.contains(&(category.to_string(), symbol.to_string()))
            && digest.len() == 64
            && uri.starts_with("s3://bolt-parquet/backfill-staging/")
            && uri.contains(&format!("/category={category}/"))
            && uri.contains(&format!("/dt={date}/"))
            && uri.contains(&format!("/symbol={symbol}/"))
            && uri.ends_with(&format!("/object={digest}.csv.gz"))
    }));
}

#[test]
fn bybit_category_proofs_and_manifests_cover_every_staged_object() {
    let reference_root = reference_root();
    let manifest_path = reference_root.join(format!(
        "backfill-source-universe-object-manifests/{UNIVERSE_SCOPE}/bybit-public-archive-tick-trades-object-manifest.json"
    ));
    let full_manifest_text = read_required_string(&manifest_path);
    let full_manifest: serde_json::Value =
        serde_json::from_str(&full_manifest_text).expect("full manifest parses");
    let full_records = full_manifest["payload_records"]
        .as_array()
        .expect("full manifest payload records");
    let summaries = category_map(&full_manifest["category_summaries"]);
    let proof_root = reference_root.join(format!("backfill-source-proofs/{UNIVERSE_SCOPE}"));
    let category_manifest_root = reference_root.join(format!(
        "backfill-source-universe-object-manifests/{UNIVERSE_SCOPE}/category-manifests"
    ));
    let mut covered_uris = BTreeSet::new();
    let mut covered_bytes = 0_u64;

    for (category, source_binding) in [
        ("inverse", "bybit-inverse-tick-trades"),
        ("linear", "bybit-linear-tick-trades"),
        ("spot", "bybit-spot-tick-trades"),
    ] {
        let proof_file = format!("source-proof-bybit-{category}-public-archive-tick-trades.json");
        let proof_text = read_required_string(&proof_root.join(&proof_file));
        let proof: SourceProofReport = serde_json::from_str(&proof_text)
            .unwrap_or_else(|error| panic!("parse {proof_file}: {error}"));
        proof
            .evaluate_acceptance()
            .unwrap_or_else(|error| panic!("{category} proof must remain accepted: {error}"));
        assert_eq!(proof.status, SourceProofStatus::Accepted);
        assert_eq!(proof.venue, "bybit");
        assert_eq!(proof.source_binding, source_binding);
        assert_eq!(proof.table_family, "trades");

        let category_manifest_file =
            format!("bybit-public-archive-tick-trades-object-manifest-{category}.json");
        let category_manifest_text =
            read_required_string(&category_manifest_root.join(&category_manifest_file));
        let category_manifest: serde_json::Value = serde_json::from_str(&category_manifest_text)
            .unwrap_or_else(|error| panic!("parse {category_manifest_file}: {error}"));
        let summary = summaries
            .get(category)
            .expect("category summary must cover every proof");
        assert_eq!(category_manifest["category"], category);
        assert_eq!(category_manifest["source_binding"], source_binding);
        assert_eq!(category_manifest["object_count"], summary["object_count"]);
        assert_eq!(
            category_manifest["accepted_bytes"],
            summary["compressed_bytes"]
        );

        let category_records = category_manifest["payload_records"]
            .as_array()
            .expect("category payload records");
        let expected_uris = full_records
            .iter()
            .filter(|record| record["category"] == category)
            .map(|record| required_str(record, "s3_uri"))
            .collect::<BTreeSet<_>>();
        let actual_uris = category_records
            .iter()
            .map(|record| required_str(record, "s3_uri"))
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_uris, expected_uris);

        let acceptance_scope = proof.acceptance_scope.as_ref().expect("acceptance scope");
        assert_eq!(
            acceptance_scope.completed_objects,
            category_records.len() as u64
        );
        assert_eq!(acceptance_scope.failed_objects, 0);
        assert_eq!(acceptance_scope.skipped_objects, 0);
        assert_eq!(acceptance_scope.selector_scope_violations, 0);
        let category_bytes = category_records
            .iter()
            .map(|record| record["bytes"].as_u64().expect("object bytes"))
            .sum::<u64>();
        assert_eq!(acceptance_scope.accepted_bytes, category_bytes);

        for record in [
            category_records.first().expect("first category record"),
            category_records.last().expect("last category record"),
        ] {
            let s3_uri = required_str(record, "s3_uri");
            let scope = evaluate_backfill_source_proof_scope_for_selected_object(
                format!("retained-{category}-{}", required_str(record, "sha256")),
                &proof_text,
                &category_manifest_text,
                s3_uri,
            )
            .unwrap_or_else(|error| panic!("{category} selected scope evaluates: {error}"));
            assert_eq!(scope.status, BackfillSourceProofScopeStatus::CandidateFound);
            assert!(scope.blocking_issues.is_empty());
            let tranche = evaluate_backfill_accepted_tranche(
                format!("retained-{category}-accepted-tranche"),
                &scope,
                &source_proof_scope_hash(&scope),
            )
            .unwrap_or_else(|error| panic!("{category} tranche evaluates: {error}"));
            assert_eq!(tranche.status, BackfillAcceptedTrancheStatus::Accepted);
            assert_eq!(tranche.object_count, 1);
            assert_eq!(
                tranche.accepted_bytes,
                record["bytes"].as_u64().expect("selected object bytes")
            );
        }

        for record in category_records {
            let uri = required_str(record, "s3_uri");
            assert!(
                covered_uris.insert(uri.to_string()),
                "duplicate object {uri}"
            );
            covered_bytes += record["bytes"].as_u64().expect("object bytes");
        }
    }

    assert_eq!(covered_uris.len(), full_records.len());
    assert_eq!(
        covered_bytes,
        full_manifest["accepted_bytes"]
            .as_u64()
            .expect("full accepted bytes")
    );
}

fn reference_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference")
}

fn read_json(path: PathBuf) -> serde_json::Value {
    serde_json::from_str(&read_required_string(&path))
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn read_required_string(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn required_str<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} must be a string"))
}

fn category_map<'a>(value: &'a serde_json::Value) -> BTreeMap<&'a str, &'a serde_json::Value> {
    value
        .as_array()
        .expect("category array")
        .iter()
        .map(|entry| (required_str(entry, "category"), entry))
        .collect()
}

fn universe_category_symbols(universe: &serde_json::Value) -> BTreeSet<(String, String)> {
    universe["categories"]
        .as_array()
        .expect("universe categories")
        .iter()
        .flat_map(|category| {
            let category_name = required_str(category, "category").to_string();
            category["instruments"]
                .as_array()
                .expect("category instruments")
                .iter()
                .map(move |instrument| {
                    (
                        category_name.clone(),
                        required_str(instrument, "symbol").to_string(),
                    )
                })
        })
        .collect()
}

fn source_proof_scope_hash(report: &BackfillSourceProofScopeReport) -> String {
    sha256_hex(&serde_json::to_vec(report).expect("scope report serializes"))
}
