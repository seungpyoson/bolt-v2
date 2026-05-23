use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set by Cargo"),
    );
    emit_git_head_rerun_paths(&manifest_dir);

    match Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&manifest_dir)
        .output()
    {
        Ok(output) if output.status.success() => {
            let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if is_git_head_sha(&head) {
                println!("cargo:rustc-env=BOLT_V3_BUILD_HEAD_SHA={head}");
            } else {
                println!("cargo:warning=git rev-parse HEAD returned invalid head shape");
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("cargo:warning=git rev-parse HEAD failed: {stderr}");
        }
        Err(error) => {
            println!("cargo:warning=failed to run git rev-parse HEAD: {error}");
        }
    }
}

fn emit_git_head_rerun_paths(manifest_dir: &Path) {
    for path in git_head_rerun_paths(manifest_dir) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

pub fn git_head_rerun_paths(manifest_dir: &Path) -> Vec<PathBuf> {
    let Some(git_dir) = git_dir_from_manifest(manifest_dir) else {
        return Vec::new();
    };

    let head_path = git_dir.join("HEAD");
    let mut paths = vec![head_path.clone()];

    let Ok(head_content) = fs::read_to_string(&head_path) else {
        return paths;
    };
    let Some(head_ref) = head_content.strip_prefix("ref:").map(str::trim) else {
        return paths;
    };

    let common_dir = git_common_dir(&git_dir);
    push_unique(&mut paths, common_dir.join(head_ref));
    push_unique(&mut paths, git_dir.join(head_ref));
    push_unique(&mut paths, common_dir.join("packed-refs"));
    paths
}

fn git_dir_from_manifest(manifest_dir: &Path) -> Option<PathBuf> {
    let dot_git = manifest_dir.join(".git");
    if dot_git.is_dir() {
        return Some(canonicalize_existing(dot_git));
    }

    let dot_git_content = fs::read_to_string(&dot_git).ok()?;
    let git_dir = dot_git_content.strip_prefix("gitdir:").map(str::trim)?;
    let git_dir = PathBuf::from(git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        manifest_dir.join(git_dir)
    };
    Some(canonicalize_existing(git_dir))
}

fn git_common_dir(git_dir: &Path) -> PathBuf {
    let Ok(common_dir) = fs::read_to_string(git_dir.join("commondir")) else {
        return git_dir.to_path_buf();
    };
    let common_dir = PathBuf::from(common_dir.trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        git_dir.join(common_dir)
    };
    canonicalize_existing(common_dir)
}

fn canonicalize_existing(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
