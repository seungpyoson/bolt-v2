use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("git should resolve repo root in tests");
    assert!(output.status.success(), "git rev-parse failed");
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("git output utf-8")
            .trim()
            .to_string(),
    )
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn unit_pins_working_directory() {
    let contents = read("deploy/systemd/bolt-v2.service");
    assert!(
        contents.contains("WorkingDirectory=/srv/bolt-v2"),
        "deploy/systemd/bolt-v2.service must contain 'WorkingDirectory=/srv/bolt-v2' \
        to prevent the service from running against the root volume; directive is missing"
    );
}

#[test]
fn unit_logs_to_journal() {
    let contents = read("deploy/systemd/bolt-v2.service");
    assert!(
        contents.contains("StandardOutput=journal"),
        "deploy/systemd/bolt-v2.service must contain 'StandardOutput=journal'; directive is missing"
    );
    assert!(
        contents.contains("StandardError=journal"),
        "deploy/systemd/bolt-v2.service must contain 'StandardError=journal'; directive is missing"
    );
}

#[test]
fn unit_runs_as_bolt_user() {
    let contents = read("deploy/systemd/bolt-v2.service");
    assert!(
        contents.contains("User=bolt"),
        "deploy/systemd/bolt-v2.service must contain 'User=bolt'; directive is missing"
    );
    assert!(
        contents.contains("Group=bolt"),
        "deploy/systemd/bolt-v2.service must contain 'Group=bolt'; directive is missing"
    );
}

#[test]
fn unit_execstart_points_to_opt_bolt_v2() {
    let contents = read("deploy/systemd/bolt-v2.service");
    assert!(
        contents.contains("ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml"),
        "deploy/systemd/bolt-v2.service must contain \
        'ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml'; \
        directive is missing"
    );
}

#[test]
fn unit_uses_private_tmp() {
    let contents = read("deploy/systemd/bolt-v2.service");
    assert!(
        contents.contains("PrivateTmp=true"),
        "deploy/systemd/bolt-v2.service must contain 'PrivateTmp=true' \
        (Task 1 hardening fix); directive is missing"
    );
}

#[test]
fn journald_drop_in_caps_growth() {
    let contents = read("deploy/systemd/journald-bolt-v2.conf");
    assert!(
        contents.contains("SystemMaxUse=500M"),
        "deploy/systemd/journald-bolt-v2.conf must contain 'SystemMaxUse=500M'; directive is missing"
    );
    assert!(
        contents.contains("SystemMaxFileSize=50M"),
        "deploy/systemd/journald-bolt-v2.conf must contain 'SystemMaxFileSize=50M'; directive is missing"
    );
    assert!(
        contents.contains("MaxRetentionSec=7day"),
        "deploy/systemd/journald-bolt-v2.conf must contain 'MaxRetentionSec=7day'; directive is missing"
    );
}

#[test]
fn install_script_targets_srv_bolt_v2() {
    let contents = read("deploy/install.sh");
    assert!(
        contents.contains("BOLT_DATA_DEVICE"),
        "deploy/install.sh must contain 'BOLT_DATA_DEVICE' (the required env var \
        for the data device); directive is missing"
    );
    assert!(
        contents.contains("/srv/bolt-v2"),
        "deploy/install.sh must contain '/srv/bolt-v2' (the data mount point); \
        directive is missing"
    );
    assert!(
        contents.contains("systemctl enable bolt-v2.service"),
        "deploy/install.sh must contain 'systemctl enable bolt-v2.service'; \
        directive is missing"
    );
}
