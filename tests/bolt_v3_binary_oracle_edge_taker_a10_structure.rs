use std::path::{Path, PathBuf};

const EXPECTED_SPLIT_TEST_COUNT: usize = 238;
const EXPECTED_TEST_MODULES: &[&str] = &[
    "book_sizing",
    "config",
    "core_glue",
    "exposure",
    "orders_admission",
    "pricing",
    "selection",
    "source_evidence",
    "trade_flow",
];
const EXPECTED_TEST_FILES: &[&str] = &[
    "mod.rs",
    "shared_fixture.rs",
    "book_sizing.rs",
    "config.rs",
    "core_glue.rs",
    "exposure.rs",
    "orders_admission.rs",
    "pricing.rs",
    "selection.rs",
    "source_evidence.rs",
    "trade_flow.rs",
];

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
    for module in EXPECTED_TEST_MODULES {
        assert!(
            tests_mod.contains(&format!("mod {module};")),
            "tests/mod.rs must declare ownership module `{module}`"
        );
    }

    let mut split_test_count = 0;
    for file in EXPECTED_TEST_FILES {
        let path = tests_root.join(file);
        let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("split test file {} should read: {error}", path.display())
        });
        assert!(
            source.starts_with("#![cfg(test)]\n"),
            "split test file {} must be test-only for source/literal scanners",
            path.display()
        );
        split_test_count += count_test_functions(&source);
    }
    assert_eq!(
        split_test_count, EXPECTED_SPLIT_TEST_COUNT,
        "A10 must preserve the current embedded test inventory exactly"
    );
}
