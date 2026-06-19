use std::{fs, path::PathBuf};

#[test]
fn systemd_unit_sets_srv_working_directory() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("WorkingDirectory=/srv/bolt-v2"),
        "systemd unit must anchor cwd at /srv/bolt-v2"
    );
}

#[test]
fn systemd_unit_requires_srv_mountpoint() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("ExecStartPre=/usr/bin/mountpoint -q /srv/bolt-v2"),
        "systemd unit must fail fast if /srv/bolt-v2 is not mounted"
    );
}

#[test]
fn systemd_unit_allows_reference_live_probe_startup_window() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("TimeoutStartSec=180"),
        "systemd unit must allow the TOML-owned reference_live_probe duration plus connect/SSM overhead"
    );
}

#[test]
fn systemd_unit_runs_rust_prestart_storage_check() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains(
            "ExecStartPre=/opt/bolt-v2/bolt-v2 ops prestart-check --config /opt/bolt-v2/config/live.toml"
        ),
        "systemd unit must reject wrong-disk or low-space live config before starting"
    );
}

#[test]
fn systemd_unit_runs_reference_live_probe_before_start() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains(
            "ExecStartPre=/opt/bolt-v2/bolt-v2 ops reference-live-probe --config /opt/bolt-v2/config/live.toml"
        ),
        "systemd unit must prove Chainlink and PolyResearch reference streams before starting"
    );
}

#[test]
fn install_script_provisions_runtime_catalog_on_srv_volume() {
    let install_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let install = fs::read_to_string(&install_path).expect("install script should exist");

    assert!(
        install.contains("BOLT_HOME=\"/srv/bolt-v2\""),
        "install script must use the same data-volume root as systemd"
    );
    assert!(
        install.contains("BOLT_INSTALL_ROOT=\"/opt/bolt-v2\""),
        "install script must use the same install root as systemd"
    );
    assert!(
        !install.contains("BOLT_HOME=\"${BOLT_HOME:-"),
        "install script must not let BOLT_HOME drift from systemd"
    );
    assert!(
        !install.contains("BOLT_INSTALL_ROOT=\"${BOLT_INSTALL_ROOT:-"),
        "install script must not let BOLT_INSTALL_ROOT drift from systemd"
    );
    assert!(
        install.contains("\"${BOLT_HOME}/var/bolt-v3-live/catalog\""),
        "install script must provision the configured runtime catalog under /srv/bolt-v2"
    );
    assert!(
        install.contains("\"${BOLT_HOME}/var/bolt-v3-live/reports\""),
        "install script must provision runtime reports under /srv/bolt-v2"
    );
}

#[test]
fn root_config_sets_runtime_catalog_and_min_free_space_on_srv_volume() {
    let root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/root.toml");
    let root = fs::read_to_string(&root_path).expect("root config should exist");

    assert!(
        root.contains("catalog_directory = \"/srv/bolt-v2/var/bolt-v3-live/catalog\""),
        "root config must place runtime catalog under /srv/bolt-v2"
    );
    assert!(
        root.contains("required_catalog_prefix = \"/srv/bolt-v2\""),
        "root config must declare the required live catalog prefix"
    );
    assert!(
        root.contains("min_free_bytes = 10737418240"),
        "root config must declare the prestart free-space floor"
    );
}

#[test]
fn journald_caps_persistent_and_runtime_storage() {
    let journald_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/journald-bolt-v2.conf");
    let journald = fs::read_to_string(&journald_path).expect("journald config should exist");

    assert!(
        journald.contains("SystemMaxUse=500M"),
        "persistent journald storage must be capped"
    );
    assert!(
        journald.contains("RuntimeMaxUse=500M"),
        "volatile journald storage must be capped"
    );
}
