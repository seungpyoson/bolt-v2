use std::{fs, path::Path};

#[test]
fn production_rust_does_not_hardcode_sample_venue_or_instrument() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    for path in rust_files(&src) {
        let content = fs::read_to_string(&path).expect("read Rust source");
        let production = production_region(&content);
        let lower = production.to_ascii_lowercase();
        for needle in [
            "bybit",
            "binance",
            "bnbusdc",
            "pmxt",
            "polymarket",
            "public_archive",
            "upbit",
            "bithumb",
            "korbit",
            "coinone",
            "kimchi",
            "korean_spot",
            "reference_price",
            "fx_quote",
            "token_mapping",
        ] {
            if lower.contains(needle) && !needle_allowed_in_production_path(needle, &path, &src) {
                failures.push(format!("{} contains {needle:?}", path.display()));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "sample venue/instrument values must stay in TOML reference fixtures, tests, or explicit one-off proof modules, not generic production Rust:\n{}",
        failures.join("\n")
    );
}

#[test]
fn run_manifest_unit_tests_do_not_embed_accepted_sample_fixture_values() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/run_manifest.rs");
    let content = fs::read_to_string(&path).expect("read run manifest source");
    let unit_tests = content
        .split("\n#[cfg(test)]\nmod tests")
        .nth(1)
        .expect("run_manifest unit tests");
    let lower = unit_tests.to_ascii_lowercase();
    let mut failures = Vec::new();
    for needle in ["bybit", "bnbusdc", "public_archive"] {
        if lower.contains(needle) {
            failures.push(needle);
        }
    }

    assert!(
        failures.is_empty(),
        "generic run_manifest unit fixtures must use synthetic values, not the accepted sample proof values: {}",
        failures.join(", ")
    );
}

fn production_region(content: &str) -> &str {
    content
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(content)
}

fn needle_allowed_in_production_path(needle: &str, path: &Path, src: &Path) -> bool {
    if !matches!(needle, "pmxt" | "polymarket") {
        return false;
    }

    let relative = path.strip_prefix(src).expect("source-relative path");
    matches!(
        relative.to_str().expect("UTF-8 source path"),
        "lib.rs"
            | "pmxt_one_off_backfill_projection.rs"
            | "polymarket_metadata_gate.rs"
            | "polymarket_nt_surface_proof.rs"
            | "bin/pmxt_one_off_l2_artifact_root_run.rs"
            | "bin/polymarket_metadata_gate.rs"
    )
}

fn rust_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    visit_rust_files(root, &mut files);
    files.sort();
    files
}

fn visit_rust_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            visit_rust_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}
