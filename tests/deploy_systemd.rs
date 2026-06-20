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
fn systemd_unit_allows_reference_health_startup_window() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("TimeoutStartSec=180"),
        "systemd unit must allow reference current-price health plus connect/SSM overhead"
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
            "ExecStartPre=/opt/bolt-v2/bolt-v2 ops verify-live-config --profile \"${BOLT_LIVE_PROFILE}\" --config-root /opt/bolt-v2/config"
        ),
        "systemd unit must verify the deployed live.toml against the operator-selected tracked profile before start, \
         so the tracked production config policy is enforced at the prod entry point, not advisory (#768)"
    );
    assert!(
        unit.contains("EnvironmentFile=/etc/bolt-v2/live.env"),
        "systemd unit must load live profile selection from operator config, not hardcode a venue/market/strategy profile"
    );
    assert!(
        unit.contains("ExecStartPre=/usr/bin/test -n \"${BOLT_LIVE_PROFILE}\""),
        "systemd unit must fail closed when BOLT_LIVE_PROFILE is missing or empty"
    );
    assert!(
        !unit.contains("BOLT_CONFIG_ROOT"),
        "systemd must keep /opt/bolt-v2/config structural instead of accepting a config-root env escape"
    );
    assert!(
        !unit.contains("--deployed"),
        "systemd verify must derive /opt/bolt-v2/config/live.toml from the structural config root"
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
fn systemd_unit_runs_reference_current_price_health_before_start() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains(
            "ExecStartPre=/opt/bolt-v2/bolt-v2 ops reference-current-price-health --config /opt/bolt-v2/config/live.toml"
        ),
        "systemd unit must prove reference_current_price custom data reaches the strategy-free runtime path before starting"
    );
    assert!(
        !unit.contains("ops reference-live-probe"),
        "systemd must not rely on raw WebSocket frame probes as the reference readiness gate"
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
    // overlay (under config/profiles/) AND every referenced strategy file (under
    // config/strategies/), not just live.toml. Files/dirs copied by root under a restrictive
    // umask can land 0600/0700 root:root; the installer must repair the whole deploy bundle,
    // without broad-scanning ignored legacy files such as config/live.local.toml.
    let install_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let install = fs::read_to_string(&install_path).expect("install script should exist");

    assert!(
        install.contains("for config_subdir in strategies profiles; do"),
        "installer must repair BOTH required config subdirs — strategies AND the #768 overlay dir profiles"
    );
    assert!(
        install.contains("reject_symlinked_install_path()"),
        "installer must refuse symlinked config bundle paths before root-owned repair"
    );
    let install_root_guard = install
        .find("reject_symlinked_install_path \"${BOLT_INSTALL_ROOT}\" \"install root\"")
        .expect("installer must check install root for symlink before install -d");
    let install_root_create = install
        .find("install -d -m 0755 \"${BOLT_INSTALL_ROOT}\"")
        .expect("installer must create the structural install root");
    assert!(
        install_root_guard < install_root_create,
        "installer must reject symlinked install root before install -d can chmod/chown through it"
    );
    let config_root_guard = install
        .find("reject_symlinked_install_path \"${BOLT_INSTALL_ROOT}/config\" \"config root\"")
        .expect("installer must check config root for symlink before install -d");
    let config_root_create = install
        .find("install -d -o root -g \"${BOLT_GROUP}\" -m 0750 \"${BOLT_INSTALL_ROOT}/config\"")
        .expect("installer must create the structural config root");
    assert!(
        config_root_guard < config_root_create,
        "installer must reject symlinked config root before install -d can chmod/chown through it"
    );
    assert!(
        install.contains("repair_config_dir \"${config_subdir_path}\""),
        "installer must repair each top-level config bundle directory through the symlink guard"
    );
    assert!(
        install.contains("find \"${config_subdir_path}\" -type d -print0"),
        "installer must repair nested strategy/profile directories without following symlinked dirs"
    );
    assert!(
        install.contains("chmod 0750 \"${path}\""),
        "installer must make each config subdir (strategies, profiles) traversable by the bolt group (0750)"
    );
    assert!(
        install.contains("chown root:\"${BOLT_GROUP}\" \"${path}\""),
        "installer must own each config subdir (strategies, profiles) as root:bolt for the service user"
    );
    assert!(
        install.contains("\"${BOLT_INSTALL_ROOT}/config/root.toml\""),
        "installer must repair root.toml explicitly"
    );
    assert!(
        install.contains("\"${BOLT_INSTALL_ROOT}/config/live.toml\""),
        "installer must repair the generated live.toml explicitly"
    );
    assert!(
        install.contains("\"${BOLT_INSTALL_ROOT}/config/profiles/\"*.overlay.toml"),
        "installer must repair tracked profile overlays explicitly"
    );
    assert!(
        install.contains(
            "find \"${BOLT_INSTALL_ROOT}/config/strategies\" -type f -name '*.toml' -print0"
        ),
        "installer must repair strategy TOMLs under config/strategies, including nested files"
    );
    assert!(
        !install.contains("find \"${BOLT_INSTALL_ROOT}/config\" -type f -name '*.toml'"),
        "installer must not broad-scan config/*.toml because that touches ignored live.local.toml drift"
    );
    assert!(
        !install.contains("live.local.toml"),
        "installer must not repair or bless legacy live.local.toml"
    );
    assert!(
        install.contains("repair_config_file \"${config_bundle_file}\" \"config bundle file\"")
            && install.contains("chmod 0640 \"${path}\""),
        "installer must make each enumerated deploy-bundle file root:bolt and group-readable"
    );
}

#[test]
fn install_script_provisions_live_env_directory_without_profile_default() {
    let install_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let install = fs::read_to_string(&install_path).expect("install script should exist");

    assert!(
        install.contains("LIVE_ENV_DIR=\"/etc/bolt-v2\""),
        "install script must provision the systemd environment directory for live profile selection"
    );
    assert!(
        install.contains("install -d -o root -g \"${BOLT_GROUP}\" -m 0750 \"${LIVE_ENV_DIR}\""),
        "live environment directory must be readable by the service group"
    );
    assert!(
        !install.contains("BOLT_LIVE_PROFILE="),
        "installer must not silently choose any venue/market/strategy profile"
    );
    assert!(
        !install.contains("BOLT_CONFIG_ROOT"),
        "installer must not introduce a production config-root env override"
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
