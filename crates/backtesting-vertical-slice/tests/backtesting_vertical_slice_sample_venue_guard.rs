use std::{fs, path::Path};

#[test]
fn production_rust_does_not_hardcode_sample_venue_or_instrument() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    for path in rust_files(&src) {
        let content = fs::read_to_string(&path).expect("read Rust source");
        let production = production_region(&content);
        let lower = production.to_ascii_lowercase();
        for needle in ["bybit", "bnbusdc", "public_archive"] {
            if lower.contains(needle) {
                failures.push(format!("{} contains {needle:?}", path.display()));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "sample venue/instrument values must stay in TOML reference fixtures or tests, not production Rust:\n{}",
        failures.join("\n")
    );
}

fn production_region(content: &str) -> &str {
    content
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(content)
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
