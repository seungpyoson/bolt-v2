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

#[test]
fn committed_pack_completion_boundaries_are_registry_derived_not_venue_listed() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (relative_path, function_name) in [
        (
            "src/source_universe_batch_launch.rs",
            "discover_committed_source_universe_execution_packs",
        ),
        (
            "src/bin/source_universe_batch_execution.rs",
            "committed_one_record_launch_profiles_select_exact_staged_s3_packs",
        ),
        (
            "src/source_universe_object_transport.rs",
            "committed_tracers_plan_only_their_staged_s3_object",
        ),
        (
            "tests/backtesting_vertical_slice_source_universe_execution_acceptance.rs",
            "committed_execution_pack_registry_and_acceptance_ledger_are_an_exact_set",
        ),
    ] {
        let path = crate_root.join(relative_path);
        let source = fs::read_to_string(&path).expect("read registry-derived boundary source");
        let function = rust_function_region(&source, function_name);
        let lower = function.to_ascii_lowercase();
        for venue in ["binance", "bybit"] {
            assert!(
                !lower.contains(venue),
                "generic committed-pack boundary {function_name} in {} must discover registry entries, not list venue {venue}",
                path.display()
            );
        }
    }
}

fn rust_function_region<'a>(source: &'a str, function_name: &str) -> &'a str {
    let signature = format!("fn {function_name}(");
    let function_start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("missing function {function_name}"));
    let source = &source[function_start..];
    let body_start = source
        .find('{')
        .unwrap_or_else(|| panic!("missing body for function {function_name}"));
    let mut depth = 0_u64;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1).expect("balanced function braces");
                if depth == 0 {
                    return &source[..body_start + offset + character.len_utf8()];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated body for function {function_name}")
}

fn production_region(content: &str) -> &str {
    content
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(content)
}

fn needle_allowed_in_production_path(needle: &str, path: &Path, src: &Path) -> bool {
    let relative = path.strip_prefix(src).expect("source-relative path");
    let relative = relative.to_str().expect("UTF-8 source path");
    if relative == "reference_fixture_index.rs" {
        return matches!(needle, "binance" | "bybit" | "pmxt" | "polymarket");
    }
    if !matches!(needle, "pmxt" | "polymarket") {
        return false;
    }

    matches!(
        relative,
        "lib.rs"
            | "pmxt_one_off_backfill_projection.rs"
            | "polymarket_metadata_gate.rs"
            | "polymarket_nt_surface_proof.rs"
            | "bin/pmxt_one_off_l2_artifact_root_run.rs"
            | "bin/polymarket_metadata_gate.rs"
    )
}

#[test]
fn reference_fixture_index_sample_allowlist_is_limited_to_provenance_terms() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let path = src.join("reference_fixture_index.rs");

    for needle in ["binance", "bybit", "pmxt", "polymarket"] {
        assert!(needle_allowed_in_production_path(needle, &path, &src));
    }
    for needle in [
        "bnbusdc",
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
        assert!(!needle_allowed_in_production_path(needle, &path, &src));
    }
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
