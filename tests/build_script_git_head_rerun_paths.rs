#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use std::{env, fs, path::PathBuf};

fn temp_git_fixture(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("bolt-v3-build-rs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("fixture dir should be created");
    dir
}

#[test]
fn linked_worktree_head_ref_watches_common_ref_and_packed_refs() {
    let manifest_dir = temp_git_fixture("linked-worktree");
    let common_dir = manifest_dir.join("main-git");
    let worktree_git_dir = common_dir.join("worktrees").join("bolt-v3");
    fs::create_dir_all(worktree_git_dir.join("refs").join("heads"))
        .expect("worktree refs dir should be created");
    fs::create_dir_all(common_dir.join("refs").join("heads"))
        .expect("common refs dir should be created");
    let relative_worktree_git_dir = PathBuf::from("main-git")
        .join("worktrees")
        .join("bolt-v3")
        .join("..")
        .join("bolt-v3");
    fs::write(
        manifest_dir.join(".git"),
        format!("gitdir: {}\n", relative_worktree_git_dir.display()),
    )
    .expect(".git file should be written");
    fs::write(worktree_git_dir.join("commondir"), "../..\n").expect("commondir should be written");
    fs::write(worktree_git_dir.join("HEAD"), "ref: refs/heads/topic\n")
        .expect("HEAD should be written");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let common_dir = fs::canonicalize(&common_dir).expect("common dir should canonicalize");

    let worktree_git_dir =
        fs::canonicalize(&worktree_git_dir).expect("worktree git dir should canonicalize");

    assert!(paths.contains(&worktree_git_dir.join("HEAD")));
    assert!(paths.contains(&common_dir.join("refs").join("heads").join("topic")));
    assert!(paths.contains(&common_dir.join("packed-refs")));

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn build_script_watches_shared_source_canonicalization_module() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let paths = build_script::canonical_source_rerun_paths(&manifest_dir)
        .expect("canonical source rerun paths should collect");

    assert!(
        paths.contains(&manifest_dir.join("src/source_canonicalization.rs")),
        "build.rs must rerun when the shared source registry/canonicalizer changes"
    );
}

#[test]
fn build_script_reruns_when_manifest_dir_env_changes() {
    assert!(
        build_script::build_script_rerun_env_vars().contains(&"CARGO_MANIFEST_DIR"),
        "shared target dirs must not reuse source embeds from another checkout"
    );
}

#[test]
fn build_script_reads_manifest_dir_at_run_time() {
    assert_eq!(
        build_script::build_script_manifest_dir(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        "build script source embeds must use the checkout path Cargo gives the running build script"
    );
}

#[test]
fn build_script_watches_current_source_set_files() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let paths = build_script::canonical_source_rerun_paths(&manifest_dir)
        .expect("canonical source rerun paths should collect");

    assert!(paths.contains(&manifest_dir.join("src/source_canonicalization.rs")));
    assert!(paths.contains(&manifest_dir.join("src/bolt_v3_book_sizing.rs")));
    assert!(
        paths.contains(
            &manifest_dir
                .join("src/strategies/binary_oracle_edge_taker")
                .join("mod.rs")
        ),
        "build.rs must watch nested strategy source files, not only the root directory"
    );
}

#[test]
fn build_script_reports_missing_canonical_source_path() {
    let manifest_dir = temp_git_fixture("missing-canonical-source");

    let error = build_script::canonical_source_rerun_paths(&manifest_dir)
        .expect_err("missing canonical source path should fail closed");
    let message = error.to_string();

    assert!(
        message.contains("src/strategies/binary_oracle_edge_taker"),
        "missing canonical source error must identify the missing path: {message}"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[cfg(unix)]
#[test]
fn build_script_rejects_symlinked_canonical_source_paths() {
    let manifest_dir = temp_git_fixture("canonical-source-symlink");
    let strategy_dir = manifest_dir.join("src/strategies/binary_oracle_edge_taker");
    fs::create_dir_all(&strategy_dir).expect("strategy source dir should create");
    fs::write(strategy_dir.join("mod.rs"), "pub fn strategy() {}\n")
        .expect("strategy source should write");
    fs::write(
        manifest_dir.join("src/bolt_v3_book_sizing.rs"),
        "pub fn sizing() {}\n",
    )
    .expect("book sizing source should write");
    fs::write(
        manifest_dir.join("src/bolt_v3_submit_admission.rs"),
        "pub fn submit() {}\n",
    )
    .expect("submit admission source should write");
    std::os::unix::fs::symlink("mod.rs", strategy_dir.join("link.rs"))
        .expect("symlinked strategy source should create");

    let error = build_script::canonical_source_rerun_paths(&manifest_dir)
        .expect_err("symlinked canonical source path should fail closed");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    let _ = fs::remove_dir_all(manifest_dir);
}
