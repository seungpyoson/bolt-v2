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

fn plain_checkout_fixture(name: &str) -> PathBuf {
    let manifest_dir = temp_git_fixture(name);
    let git_dir = manifest_dir.join(".git");
    fs::create_dir_all(git_dir.join("refs").join("heads")).expect("refs dir should be created");
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/topic\n").expect("HEAD should be written");
    manifest_dir
}

#[test]
fn emitted_rerun_paths_skip_packed_refs_missing_from_a_loose_ref_checkout() {
    let manifest_dir = plain_checkout_fixture("loose-refs");
    let git_dir = manifest_dir.join(".git");
    fs::write(git_dir.join("refs").join("heads").join("topic"), "0\n")
        .expect("loose ref should be written");

    let emitted = build_script::emitted_git_head_rerun_paths(&manifest_dir);
    let git_dir = fs::canonicalize(&git_dir).expect("git dir should canonicalize");

    assert!(emitted.contains(&git_dir.join("HEAD")));
    assert!(emitted.contains(&git_dir.join("refs").join("heads").join("topic")));
    assert!(
        !emitted.contains(&git_dir.join("packed-refs")),
        "a rerun-if-changed path that does not exist is permanently dirty to Cargo, \
         which recompiles this crate on every invocation"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn emitted_rerun_paths_watch_packed_refs_when_the_ref_is_packed() {
    let manifest_dir = plain_checkout_fixture("packed-refs");
    let git_dir = manifest_dir.join(".git");
    fs::write(git_dir.join("packed-refs"), "# pack-refs with: peeled\n")
        .expect("packed-refs should be written");

    let emitted = build_script::emitted_git_head_rerun_paths(&manifest_dir);
    let git_dir = fs::canonicalize(&git_dir).expect("git dir should canonicalize");

    assert!(emitted.contains(&git_dir.join("packed-refs")));
    assert!(!emitted.contains(&git_dir.join("refs").join("heads").join("topic")));

    let _ = fs::remove_dir_all(manifest_dir);
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
