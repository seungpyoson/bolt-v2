use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

pub fn tempdir_in_repo_target() -> tempfile::TempDir {
    let target_dir = repo_root().join("target");
    fs::create_dir_all(&target_dir).unwrap_or_else(|error| {
        panic!("create target temp root {}: {error}", target_dir.display())
    });
    tempfile::tempdir_in(&target_dir)
        .unwrap_or_else(|error| panic!("create temp dir in {}: {error}", target_dir.display()))
}

pub fn rewrite_assignment(source: &str, key: &str, value: &Path) -> String {
    let prefix = format!("{key} = ");
    let replacement = format!("{key} = \"{}\"", value.display());
    let mut replacement_count = 0usize;
    let rewritten = source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with(&prefix) {
                replacement_count += 1;
                replacement.as_str()
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_eq!(
        replacement_count, 1,
        "expected exactly one assignment for {key:?}, found {replacement_count}"
    );
    rewritten
}
