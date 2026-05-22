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
    fs::write(
        manifest_dir.join(".git"),
        format!("gitdir: {}\n", worktree_git_dir.display()),
    )
    .expect(".git file should be written");
    fs::write(worktree_git_dir.join("commondir"), "../..\n").expect("commondir should be written");
    fs::write(worktree_git_dir.join("HEAD"), "ref: refs/heads/topic\n")
        .expect("HEAD should be written");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let common_dir = fs::canonicalize(&common_dir).expect("common dir should canonicalize");

    assert!(paths.contains(&worktree_git_dir.join("HEAD")));
    assert!(paths.contains(&common_dir.join("refs").join("heads").join("topic")));
    assert!(paths.contains(&common_dir.join("packed-refs")));

    let _ = fs::remove_dir_all(manifest_dir);
}
