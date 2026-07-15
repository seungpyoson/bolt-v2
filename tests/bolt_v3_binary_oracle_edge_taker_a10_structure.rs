use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const EXPECTED_DECLARED_MODULES: &[&str] = &[
    "shared_fixture",
    "adverse_path_harness",
    "book_sizing",
    "config",
    "core_glue",
    "exposure",
    "orders_admission",
    "pricing",
    "reference_price",
    "selection",
    "source_evidence",
    "trade_flow",
];
const EXPECTED_TEST_FILES: &[&str] = &[
    "mod.rs",
    "shared_fixture.rs",
    "adverse_path_harness.rs",
    "book_sizing.rs",
    "config.rs",
    "core_glue.rs",
    "exposure.rs",
    "orders_admission.rs",
    "pricing.rs",
    "reference_price.rs",
    "selection.rs",
    "source_evidence.rs",
    "trade_flow.rs",
];

fn expected_set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn count_test_functions(source: &str) -> usize {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed == "#[test]" || trimmed.starts_with("#[tokio::test")
        })
        .count()
}

fn declared_modules(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            let module = line.trim().strip_prefix("mod ")?;
            Some(module.strip_suffix(';')?.to_owned())
        })
        .collect()
}

fn rust_files(root: &Path) -> BTreeSet<String> {
    std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("split test root {} should read: {error}", root.display()))
        .map(|entry| entry.expect("split test root entries should read"))
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .map(|entry| {
            entry
                .file_name()
                .into_string()
                .expect("split test file names should be UTF-8")
        })
        .collect()
}

#[test]
fn binary_oracle_edge_taker_tests_are_split_by_a10_ownership() {
    let strategy_root = repo_path("src/strategies/binary_oracle_edge_taker");
    let mod_rs = strategy_root.join("mod.rs");
    let mod_source = std::fs::read_to_string(&mod_rs).expect("strategy mod.rs should be readable");

    assert!(
        mod_source.contains("\n#[cfg(test)]\nmod tests;\n"),
        "A10 requires mod.rs to declare the external test module only"
    );
    assert!(
        !mod_source.contains("\n#[cfg(test)]\nmod tests {{"),
        "A10 requires embedded test bodies to leave mod.rs"
    );
    assert_eq!(
        count_test_functions(&mod_source),
        0,
        "A10 requires mod.rs to contain no embedded #[test] functions"
    );

    let tests_root = strategy_root.join("tests");
    let tests_mod = std::fs::read_to_string(tests_root.join("mod.rs"))
        .expect("split test harness module should be readable");
    assert_eq!(
        declared_modules(&tests_mod),
        expected_set(EXPECTED_DECLARED_MODULES),
        "tests/mod.rs must declare exactly the A10 split modules"
    );
    assert_eq!(
        rust_files(&tests_root),
        expected_set(EXPECTED_TEST_FILES),
        "A10 split test root must contain exactly the expected .rs files"
    );

    for file in EXPECTED_TEST_FILES {
        let path = tests_root.join(file);
        let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("split test file {} should read: {error}", path.display())
        });
        assert!(
            source.lines().next() == Some("#![cfg(test)]"),
            "split test file {} must be test-only for source/literal scanners",
            path.display()
        );
    }
}

#[test]
fn runtime_reconcile_and_reference_health_mechanics_are_shared_owned() {
    let strategy_source =
        std::fs::read_to_string(repo_path("src/strategies/binary_oracle_edge_taker/mod.rs"))
            .expect("strategy mod.rs should be readable");
    for moved_symbol in [
        "fn reconcile_runtime_venue_state(",
        "fn query_order_for_reconcile(",
        "fn reconcile_transition_for_order_status(",
        "fn observe_reference_price_update(",
        "fn select_current_reference_price(",
        "fn refresh_reference_price_source_statuses(",
    ] {
        assert!(
            !strategy_source.contains(moved_symbol),
            "A10 requires moved shared symbol `{moved_symbol}` to be absent from the taker"
        );
    }

    let reconcile_source = std::fs::read_to_string(repo_path("src/bolt_v3_runtime_reconcile.rs"))
        .expect("shared runtime reconcile module should be readable");
    assert!(reconcile_source.contains("pub struct IssueVenueOrderQuery"));
    assert!(reconcile_source.contains("pub fn reconcile_runtime_venue_state("));
    assert!(reconcile_source.contains("pub fn query_order_for_reconcile("));
    assert!(reconcile_source.contains("pub fn reconcile_transition_for_order_status("));
    assert!(!reconcile_source.contains("BinaryOracleEdgeTaker"));
    assert!(!reconcile_source.contains("self.cache()"));

    let health_source = std::fs::read_to_string(repo_path("src/bolt_v3_reference_price_health.rs"))
        .expect("shared reference-price health module should be readable");
    assert!(health_source.contains("pub fn observe_reference_price_update("));
    assert!(health_source.contains("pub fn select_current_reference_price("));
}
