use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn render_live_config_supports_relative_output_paths() {
    let tempdir = TempCaseDir::new("relative-output");
    let input_path = tempdir.path().join("live.local.toml");
    fs::write(&input_path, tracked_live_local_example())
        .expect("input config should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .current_dir(tempdir.path())
        .args(["--input", "live.local.toml", "--output", "live.toml"])
        .output()
        .expect("renderer should run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Generated live.toml from live.local.toml"));
    assert!(tempdir.path().join("live.toml").exists());
}

#[test]
fn render_live_config_reports_unchanged_for_matching_output() {
    let tempdir = TempCaseDir::new("unchanged-output");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    fs::write(&input_path, tracked_live_local_example())
        .expect("input config should be written");

    run_renderer(&input_path, &output_path);
    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));
    let second_run = run_renderer(&input_path, &output_path);

    let stdout = String::from_utf8_lossy(&second_run.stdout);
    assert!(stdout.contains("Generated config unchanged:"));
    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    assert_eq!(
        modified_before, modified_after,
        "unchanged renders should not rewrite the file"
    );
    assert_read_only(&output_path);
}

#[test]
fn render_live_config_rewrites_drifted_output_and_restores_read_only_permissions() {
    let tempdir = TempCaseDir::new("drifted-output");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    fs::write(&input_path, tracked_live_local_example())
        .expect("input config should be written");

    run_renderer(&input_path, &output_path);

    make_writable_if_needed(&output_path);
    fs::write(&output_path, "drifted = true\n").expect("drifted output should be written");

    let rewrite = run_renderer(&input_path, &output_path);
    let stdout = String::from_utf8_lossy(&rewrite.stdout);
    assert!(stdout.contains("Generated config updated:"));

    let rendered = fs::read_to_string(&output_path).expect("rendered output should be readable");
    assert!(rendered.contains("# GENERATED FILE - DO NOT EDIT."));
    assert!(rendered.contains("client_id = \"POLYMARKET\""));
    assert_read_only(&output_path);
}

#[test]
fn render_live_config_handles_nested_output_paths_without_rewriting_unchanged_files() {
    let tempdir = TempCaseDir::new("nested-output");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("nested/live.toml");
    fs::write(&input_path, tracked_live_local_example())
        .expect("input config should be written");

    let first = run_renderer(&input_path, &output_path);
    assert!(String::from_utf8_lossy(&first.stdout).contains("Generated "));
    assert!(output_path.exists());

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    let second = run_renderer(&input_path, &output_path);
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("Generated config unchanged:")
    );
    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    assert_eq!(modified_before, modified_after);

    make_writable_if_needed(&output_path);
    fs::write(&output_path, "drifted = true\n").expect("drifted output should be written");

    let third = run_renderer(&input_path, &output_path);
    assert!(String::from_utf8_lossy(&third.stdout).contains("Generated config updated:"));
    assert_read_only(&output_path);
}

fn run_renderer(input_path: &Path, output_path: &Path) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .args([
            "--input",
            input_path.to_str().expect("utf-8 input path"),
            "--output",
            output_path.to_str().expect("utf-8 output path"),
        ])
        .output()
        .expect("renderer should run");

    assert!(output.status.success());
    output
}

fn tracked_live_local_example() -> String {
    fs::read_to_string("config/live.local.example.toml")
        .expect("tracked operator template should be readable")
}

fn assert_read_only(path: &Path) {
    let metadata = fs::metadata(path).expect("output metadata should exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(metadata.permissions().mode() & 0o222, 0);
    }

    #[cfg(not(unix))]
    {
        assert!(metadata.permissions().readonly());
    }
}

fn make_writable_if_needed(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .expect("output metadata should exist")
            .permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(path, permissions).expect("output should become writable");
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)
            .expect("output metadata should exist")
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).expect("output should become writable");
    }
}

struct TempCaseDir {
    path: PathBuf,
}

impl TempCaseDir {
    fn new(label: &str) -> Self {
        let timestamp_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dirname = format!("bolt-v2-{label}-{timestamp_nanos}-{counter}");
        let path = std::env::temp_dir().join(dirname);
        fs::create_dir_all(&path).expect("temp case dir should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempCaseDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
