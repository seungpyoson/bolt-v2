use object_store::{ObjectStoreExt, memory::InMemory};

use backtesting_vertical_slice::{
    artifact_store::{
        ArtifactKind, ArtifactStoreConfig, CatalogDispatchConfig, CatalogProjectionBinding,
        CreateOnlyArtifactWriter,
    },
    run_manifest::MarketStructureFixture,
};

fn artifact_config() -> ArtifactStoreConfig {
    toml::from_str(
        r#"
artifact_root = "s3://bolt-ra-artifacts/prod"

[subpaths]
raw = "raw"
nt_catalog = "nt-catalog"
source_proofs = "source-proofs"
backtests = "backtests"
artifact_index = "artifact-index"
research_analytics = "research-analytics"
"#,
    )
    .expect("artifact config parses")
}

#[test]
fn resolves_nt_catalog_projection_root_from_single_toml_artifact_root() {
    let root = artifact_config().resolve().expect("valid artifact root");

    assert_eq!(
        root.nt_catalog_projection_root("projection-run-123"),
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=projection-run-123/"
    );
    assert_eq!(
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-123"),
        "s3://bolt-ra-artifacts/prod/backtests/v1/fixture=perps-spot/run=run-123/"
    );
    assert_eq!(
        root.latest_pointer(ArtifactKind::Backtests),
        "s3://bolt-ra-artifacts/prod/artifact-index/v1/pointers/kind=backtests/latest.json"
    );
}

#[test]
fn rejects_local_or_non_s3_canonical_artifact_roots() {
    let mut config = artifact_config();
    config.artifact_root = "/tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());

    config.artifact_root = "file:///tmp/not-canonical".to_string();
    assert!(config.resolve().is_err());
}

#[test]
fn dispatches_source_bindings_to_catalog_projection_roots_without_venue_paths() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let dispatch = CatalogDispatchConfig {
        bindings: vec![
            CatalogProjectionBinding {
                source_binding: "binary-official".to_string(),
                market_structure_fixture: MarketStructureFixture::BinaryOption,
                catalog_projection_id: "binary-projection-1".to_string(),
            },
            CatalogProjectionBinding {
                source_binding: "perps-official".to_string(),
                market_structure_fixture: MarketStructureFixture::PerpsSpot,
                catalog_projection_id: "perps-projection-1".to_string(),
            },
        ],
    };

    let binary = dispatch
        .catalog_root_for("binary-official", &root)
        .expect("binary binding dispatches");
    let perps = dispatch
        .catalog_root_for("perps-official", &root)
        .expect("perps binding dispatches");

    assert_eq!(
        binary,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=binary-projection-1/"
    );
    assert_eq!(
        perps,
        "s3://bolt-ra-artifacts/prod/nt-catalog/v1/projection=perps-projection-1/"
    );
    assert!(!binary.contains("official"));
    assert!(!perps.contains("official"));
    assert!(dispatch.catalog_root_for("missing-binding", &root).is_err());
}

#[tokio::test]
async fn create_only_writer_refuses_to_overwrite_existing_object() {
    let root = artifact_config().resolve().expect("valid artifact root");
    let store = InMemory::new();
    let writer = CreateOnlyArtifactWriter::new(&store);
    let object_uri =
        root.backtest_run_root(MarketStructureFixture::PerpsSpot, "run-123") + "result.json";
    let object_path = root
        .object_path_for_uri(&object_uri)
        .expect("uri under artifact root");

    writer
        .put_create_uri(&root, &object_uri, br#"{"status":"first"}"#.to_vec())
        .await
        .expect("first create succeeds");
    let err = writer
        .put_create_uri(&root, &object_uri, br#"{"status":"second"}"#.to_vec())
        .await
        .expect_err("second create must fail");
    assert!(err.to_string().contains("already exists"), "{err}");

    let stored = store
        .get(&object_path)
        .await
        .expect("created object")
        .bytes()
        .await
        .expect("object bytes");
    assert_eq!(stored.as_ref(), br#"{"status":"first"}"#);

    assert!(
        writer
            .put_create_uri(
                &root,
                "s3://other-bucket/prod/backtests/v1/run=run-123/result.json",
                br#"{"status":"outside"}"#.to_vec(),
            )
            .await
            .is_err()
    );
}
