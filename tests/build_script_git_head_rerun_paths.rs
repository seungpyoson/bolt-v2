#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn temp_git_fixture(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("bolt-v3-build-rs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("fixture dir should be created");
    dir
}

/// A non-worktree checkout whose `HEAD` points at `head_ref`. `git init` leaves
/// `refs/heads` in place, and `git pack-refs` keeps it, so it always exists.
/// Callers add the loose ref, `packed-refs`, both, or neither.
fn plain_checkout_fixture(name: &str, head_ref: &str) -> PathBuf {
    let manifest_dir = temp_git_fixture(name);
    let git_dir = manifest_dir.join(".git");
    fs::create_dir_all(git_dir.join("refs").join("heads")).expect("refs dir should be created");
    fs::write(git_dir.join("HEAD"), format!("ref: {head_ref}\n")).expect("HEAD should be written");
    manifest_dir
}

fn write_file(path: PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir should be created");
    }
    fs::write(path, contents).expect("fixture file should be written");
}

fn write_loose_ref(git_dir: &Path, head_ref: &str) {
    write_file(git_dir.join(head_ref), "0\n");
}

fn write_packed_refs(git_dir: &Path) {
    write_file(git_dir.join("packed-refs"), "# pack-refs with: peeled\n");
}

fn canonical_git_dir(manifest_dir: &Path) -> PathBuf {
    fs::canonicalize(manifest_dir.join(".git")).expect("git dir should canonicalize")
}

#[test]
fn linked_worktree_head_ref_watches_common_ref_and_packed_refs() {
    let manifest_dir = temp_git_fixture("linked-worktree");
    let common_dir = manifest_dir.join("main-git");
    let worktree_git_dir = common_dir.join("worktrees").join("bolt-v3");
    // `git worktree add` leaves `<worktree_git_dir>/refs` empty: `refs/heads/*`
    // resolves against the common dir, and only per-worktree refs land here.
    fs::create_dir_all(worktree_git_dir.join("refs")).expect("worktree refs dir should be created");
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
    write_loose_ref(&common_dir, "refs/heads/topic");
    write_packed_refs(&common_dir);

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let common_dir = fs::canonicalize(&common_dir).expect("common dir should canonicalize");
    let worktree_git_dir =
        fs::canonicalize(&worktree_git_dir).expect("worktree git dir should canonicalize");

    assert_eq!(
        paths,
        vec![
            worktree_git_dir.join("HEAD"),
            common_dir.join("refs").join("heads").join("topic"),
            worktree_git_dir.join("refs"),
            common_dir.join("packed-refs"),
        ],
        "the branch ref is watched in the common dir; the worktree's own refs dir \
         is watched because every write beneath it moves this worktree's HEAD"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

/// Cargo treats a `rerun-if-changed` path that does not exist as permanently
/// dirty, which re-runs the build script and recompiles the crate on every
/// invocation. A ref is stored loosely, packed, both, or neither, so no single
/// candidate can be named unconditionally.
#[test]
fn every_watched_path_exists_in_every_ref_storage_state() {
    for (name, loose, packed) in [
        ("storage-loose", true, false),
        ("storage-packed", false, true),
        ("storage-both", true, true),
        ("storage-neither", false, false),
    ] {
        let manifest_dir = plain_checkout_fixture(name, "refs/heads/topic");
        let git_dir = manifest_dir.join(".git");
        if loose {
            write_loose_ref(&git_dir, "refs/heads/topic");
        }
        if packed {
            write_packed_refs(&git_dir);
        }

        let paths = build_script::git_head_rerun_paths(&manifest_dir);
        let git_dir = canonical_git_dir(&manifest_dir);

        assert_eq!(
            paths.first(),
            Some(&git_dir.join("HEAD")),
            "{name}: HEAD is always watched"
        );
        for path in &paths {
            assert!(
                path.exists(),
                "{name}: watched path does not exist: {path:?}"
            );
        }
        for (index, path) in paths.iter().enumerate() {
            assert!(
                !paths[..index].contains(path),
                "{name}: duplicate watched path: {path:?}"
            );
        }
        assert!(
            !paths.contains(&git_dir),
            "{name}: the git dir holds the index and the logs, which change without HEAD moving"
        );

        let _ = fs::remove_dir_all(manifest_dir);
    }
}

#[test]
fn loose_ref_checkout_watches_the_ref_and_not_absent_packed_refs() {
    let manifest_dir = plain_checkout_fixture("loose-only", "refs/heads/topic");
    write_loose_ref(&manifest_dir.join(".git"), "refs/heads/topic");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(
        paths,
        vec![
            git_dir.join("HEAD"),
            git_dir.join("refs").join("heads").join("topic")
        ],
        "a fresh clone has loose refs and no packed-refs"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn packed_ref_checkout_watches_the_refs_directory_so_a_new_loose_ref_is_noticed() {
    let manifest_dir = plain_checkout_fixture("packed-only", "refs/heads/topic");
    write_packed_refs(&manifest_dir.join(".git"));

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(
        paths,
        vec![
            git_dir.join("HEAD"),
            git_dir.join("refs").join("heads"),
            git_dir.join("packed-refs"),
        ],
        "committing on a packed branch writes a loose ref; without watching the \
         enclosing directory the embedded HEAD sha would go stale"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn loose_ref_shadowing_a_packed_entry_watches_both_storages() {
    let manifest_dir = plain_checkout_fixture("packed-and-loose", "refs/heads/topic");
    let git_dir = manifest_dir.join(".git");
    write_packed_refs(&git_dir);
    write_loose_ref(&git_dir, "refs/heads/topic");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(
        paths,
        vec![
            git_dir.join("HEAD"),
            git_dir.join("refs").join("heads").join("topic"),
            git_dir.join("packed-refs"),
        ],
        "`git pack-refs` leaves the packed entry behind when a later commit \
         rewrites the loose ref, so a branch can occupy both storages at once"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn nested_branch_name_without_a_loose_ref_watches_the_deepest_existing_directory() {
    let manifest_dir = plain_checkout_fixture("nested-deep", "refs/heads/fix/deep/topic");
    let git_dir = manifest_dir.join(".git");
    // A sibling branch `fix/deep/other` leaves the intermediate directories behind.
    fs::create_dir_all(git_dir.join("refs").join("heads").join("fix").join("deep"))
        .expect("nested refs dir should be created");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(
        paths,
        vec![
            git_dir.join("HEAD"),
            git_dir.join("refs").join("heads").join("fix").join("deep"),
        ],
        "Cargo walks a watched directory recursively, so the deepest existing one suffices"
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn nested_branch_name_falls_back_past_absent_intermediate_directories() {
    let manifest_dir = plain_checkout_fixture("nested-shallow", "refs/heads/fix/deep/topic");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(
        paths,
        vec![git_dir.join("HEAD"), git_dir.join("refs").join("heads")]
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn nested_branch_name_with_a_loose_ref_watches_the_ref_itself() {
    let manifest_dir = plain_checkout_fixture("nested-loose", "refs/heads/fix/deep/topic");
    write_loose_ref(&manifest_dir.join(".git"), "refs/heads/fix/deep/topic");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(
        paths,
        vec![
            git_dir.join("HEAD"),
            git_dir
                .join("refs")
                .join("heads")
                .join("fix")
                .join("deep")
                .join("topic"),
        ]
    );

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn missing_refs_directory_watches_head_only() {
    let manifest_dir = temp_git_fixture("no-refs-dir");
    let git_dir = manifest_dir.join(".git");
    fs::create_dir_all(&git_dir).expect("git dir should be created");
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/topic\n").expect("HEAD should be written");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(paths, vec![git_dir.join("HEAD")]);

    let _ = fs::remove_dir_all(manifest_dir);
}

#[test]
fn detached_head_watches_head_only() {
    let manifest_dir = temp_git_fixture("detached-head");
    let git_dir = manifest_dir.join(".git");
    fs::create_dir_all(git_dir.join("refs").join("heads")).expect("refs dir should be created");
    fs::write(
        git_dir.join("HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .expect("HEAD should be written");

    let paths = build_script::git_head_rerun_paths(&manifest_dir);
    let git_dir = canonical_git_dir(&manifest_dir);

    assert_eq!(paths, vec![git_dir.join("HEAD")]);

    let _ = fs::remove_dir_all(manifest_dir);
}

/// `Path::starts_with` matches components literally, so a `head_ref` carrying
/// `..` would otherwise walk the directory fallback out of `<git_dir>/refs` and
/// return the working tree root, which rebuilds the crate on every source edit.
#[test]
fn head_ref_escaping_the_refs_directory_watches_head_only() {
    for (name, head_ref) in [
        ("escape-git-dir", "refs/../.."),
        ("escape-worktree", "refs/../../.."),
        ("escape-absolute", "/etc/hosts"),
        ("escape-not-a-ref", "objects/info"),
    ] {
        let manifest_dir = plain_checkout_fixture(name, head_ref);

        let paths = build_script::git_head_rerun_paths(&manifest_dir);
        let git_dir = canonical_git_dir(&manifest_dir);

        assert_eq!(
            paths,
            vec![git_dir.join("HEAD")],
            "{name}: {head_ref} is not a ref we own"
        );

        let _ = fs::remove_dir_all(manifest_dir);
    }
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
