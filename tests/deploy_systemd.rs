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
fn systemd_unit_verifies_live_config_against_profile_before_start() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains(
            "ExecStartPre=/opt/bolt-v2/bolt-v2 ops verify-live-config --profile /opt/bolt-v2/config/prod-btc-5m.toml --deployed /opt/bolt-v2/config/live.toml"
        ),
        "systemd unit must verify the deployed live.toml against the tracked profile before start, \
         so the fail-closed gate (incl. enabled loss rails) is enforced at the prod entry point, not advisory (#768)"
    );

    let verify_at = unit
        .find("ops verify-live-config")
        .expect("verify-live-config ExecStartPre must be present");
    let run_at = unit
        .find("bolt-v2 run --config")
        .expect("ExecStart run must be present");
    assert!(
        verify_at < run_at,
        "verify-live-config must run before the service starts"
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
fn install_script_repairs_whole_config_bundle_for_service_user() {
    // ExecStartPre runs `ops verify-live-config` as User=bolt and must read the tracked
    // profile AND every referenced strategy file, not just live.toml. Files copied by root
    // under a restrictive umask can land 0600 root:root; the installer must repair the whole
    // bundle (strategies dir + all *.toml) to root:bolt group-readable so verification does
    // not fail with a service-user lockout regardless of the deploy shell's umask (#768).
    let install_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let install = fs::read_to_string(&install_path).expect("install script should exist");

    assert!(
        install.contains("chmod 0750 \"${BOLT_INSTALL_ROOT}/config/strategies\""),
        "installer must make the strategies dir traversable by the bolt group"
    );
    assert!(
        install.contains(
            "find \"${BOLT_INSTALL_ROOT}/config\" -type f -name '*.toml' -exec chown root:\"${BOLT_GROUP}\" {} +"
        ),
        "installer must own every deployed config/strategy TOML as root:bolt"
    );
    assert!(
        install.contains(
            "find \"${BOLT_INSTALL_ROOT}/config\" -type f -name '*.toml' -exec chmod 0640 {} +"
        ),
        "installer must make every deployed config/strategy TOML group-readable (0640) for the service user"
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
