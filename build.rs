use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.git");

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

fn emit_git_head_rerun_paths(manifest_dir: &PathBuf) {
    let dot_git = manifest_dir.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let Ok(dot_git_content) = fs::read_to_string(&dot_git) else {
            return;
        };
        let Some(git_dir) = dot_git_content.strip_prefix("gitdir:").map(str::trim) else {
            return;
        };
        manifest_dir.join(git_dir)
    };

    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());

    let Ok(head_content) = fs::read_to_string(&head_path) else {
        return;
    };
    let Some(head_ref) = head_content.strip_prefix("ref:").map(str::trim) else {
        return;
    };
    println!(
        "cargo:rerun-if-changed={}",
        git_dir.join(head_ref).display()
    );
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
