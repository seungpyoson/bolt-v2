use std::{fs, path::PathBuf, process::Command};

const CANONICAL_EXEC_START: &str = "ExecStart=/opt/bolt-v2/bolt-v2 ops launch --profile \"${BOLT_LIVE_PROFILE}\" --config-root /opt/bolt-v2/config";

fn validate_packaged_systemd_unit(rendered: &[u8], generated: &[u8]) -> Result<(), String> {
    if rendered != generated {
        return Err(
            "deploy/systemd/bolt-v2.service must match render_install_unit.py byte-for-byte"
                .to_string(),
        );
    }

    let unit = std::str::from_utf8(generated)
        .map_err(|error| format!("rendered systemd unit must be UTF-8: {error}"))?;
    let active_exec_starts: Vec<&str> = unit
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter(|line| line.starts_with("ExecStart="))
        .collect();
    if active_exec_starts != [CANONICAL_EXEC_START] {
        return Err(format!(
            "packaged systemd must have exactly the canonical ops launch ExecStart; got {active_exec_starts:?}"
        ));
    }

    Ok(())
}

fn rendered_install_unit() -> Vec<u8> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let renderer = repo_root.join("scripts/render_install_unit.py");
    let output = Command::new("python3")
        .arg(&renderer)
        .current_dir(&repo_root)
        .output()
        .expect("render_install_unit.py should execute with python3");
    assert!(
        output.status.success(),
        "render_install_unit.py failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn packaged_systemd_unit_matches_the_single_renderer_byte_for_byte() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let generated = fs::read(&unit_path).expect("generated systemd unit should exist");

    validate_packaged_systemd_unit(&rendered_install_unit(), &generated)
        .expect("renderer output and packaged unit must satisfy the deploy contract");
}

#[test]
fn packaged_systemd_evidence_rejects_executable_path_mutation() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let generated = fs::read_to_string(&unit_path).expect("generated systemd unit should exist");
    let mutated = generated.replace(
        "/opt/bolt-v2/bolt-v2 ops launch",
        "/opt/bolt-v2/alternate-binary ops launch",
    );
    assert_ne!(
        mutated, generated,
        "executable-path mutation must take effect"
    );

    let error = validate_packaged_systemd_unit(mutated.as_bytes(), mutated.as_bytes())
        .expect_err("executable-path mutation must fail deploy evidence");
    assert!(error.contains("canonical ops launch ExecStart"), "{error}");
}

#[test]
fn packaged_systemd_evidence_rejects_ops_launch_subcommand_mutation() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let generated = fs::read_to_string(&unit_path).expect("generated systemd unit should exist");
    let mutated = generated.replace(" ops launch --profile", " run --profile");
    assert_ne!(mutated, generated, "ops-launch mutation must take effect");

    let error = validate_packaged_systemd_unit(mutated.as_bytes(), mutated.as_bytes())
        .expect_err("ops-launch mutation must fail deploy evidence");
    assert!(error.contains("canonical ops launch ExecStart"), "{error}");
}

#[test]
fn packaged_systemd_evidence_rejects_rendered_byte_drift() {
    let rendered = rendered_install_unit();
    let generated = String::from_utf8(rendered.clone()).expect("rendered unit should be UTF-8");
    let mutated = generated.replace("Restart=on-failure", "Restart=always");
    assert_ne!(
        mutated, generated,
        "rendered-byte mutation must take effect"
    );

    let error = validate_packaged_systemd_unit(&rendered, mutated.as_bytes())
        .expect_err("rendered-byte mutation must fail byte-equality evidence");
    assert!(error.contains("byte-for-byte"), "{error}");
}

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
fn systemd_unit_delegates_to_ops_launch_without_redundant_prestart_paths() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("EnvironmentFile=/etc/bolt-v2/live.env"),
        "systemd unit must load live profile selection from operator config, not hardcode a venue/market/strategy profile"
    );
    assert!(
        unit.contains(CANONICAL_EXEC_START),
        "systemd unit must enter the same binary-owned ops launch lane as just live"
    );
    let active_exec_starts: Vec<&str> = unit
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter(|line| line.starts_with("ExecStart="))
        .collect();
    assert_eq!(
        active_exec_starts,
        vec![CANONICAL_EXEC_START],
        "systemd unit must have exactly one active ExecStart, and it must be the ops launch lane (no second ExecStart bypass)"
    );
    assert!(
        unit.contains("ExecStartPre=/usr/bin/mountpoint -q /srv/bolt-v2"),
        "systemd unit must keep the /srv/bolt-v2 mount precondition as a host gate"
    );
    for forbidden in [
        "ops verify-live-config",
        "ops prestart-check",
        "ops reference-current-price-health",
        "bolt-v2 run --config",
    ] {
        assert!(
            !unit.contains(forbidden),
            "systemd unit must not bypass ops launch through `{forbidden}`"
        );
    }
    assert!(
        !unit.contains("BOLT_CONFIG_ROOT"),
        "systemd must keep /opt/bolt-v2/config structural instead of accepting a config-root env escape"
    );
    assert!(
        !unit.contains("--deployed"),
        "systemd verify must derive /opt/bolt-v2/config/live.toml from the structural config root"
    );
    assert!(
        !unit.contains("ops reference-live-probe"),
        "systemd must not rely on raw WebSocket frame probes as the reference readiness gate"
    );
}

#[test]
fn systemd_unit_requires_srv_mountpoint() {
    let unit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("ExecStartPre=/usr/bin/mountpoint -q /srv/bolt-v2"),
        "systemd unit must fail fast if /srv/bolt-v2 is not a mounted device, so the runtime \
         catalog never silently lands on the root filesystem when the data volume fails to mount"
    );
}

#[test]
fn systemd_unit_template_single_sources_mountpoint_tool_path() {
    let template_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service.in");
    let template = fs::read_to_string(&template_path).expect("systemd unit template should exist");

    assert!(
        template.contains("ExecStartPre=@MOUNTPOINT_BIN@ -q @BOLT_HOME@"),
        "systemd unit template must source the mountpoint tool path from install-layout.env"
    );
    assert!(
        !template.contains("/usr/bin/mountpoint"),
        "systemd unit template must not hardcode the mountpoint tool path"
    );
}

#[test]
fn install_script_provisions_runtime_catalog_on_srv_volume() {
    let install_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let install = fs::read_to_string(&install_path).expect("install script should exist");

    // Install paths are single-sourced in deploy/install-layout.env; install.sh
    // sources that file instead of hardcoding the roots, so they cannot drift from
    // the systemd unit (which is generated from the same layout).
    assert!(
        install.contains("source \"${SCRIPT_DIR}/install-layout.env\""),
        "install script must source the single-source install layout"
    );
    assert!(
        !install.contains("BOLT_HOME=\""),
        "install script must not redefine BOLT_HOME; it comes from install-layout.env"
    );
    assert!(
        !install.contains("BOLT_INSTALL_ROOT=\""),
        "install script must not redefine BOLT_INSTALL_ROOT; it comes from install-layout.env"
    );
    // Service identity is single-sourced too: install.sh must not carry the old
    // BOLT_USER/BOLT_GROUP env-override lines, because the committed systemd unit
    // bakes User=/Group= at generate-time and a deploy-time override could never
    // reach it (it would split provisioning ownership from the running service).
    assert!(
        !install.contains("BOLT_USER=\""),
        "install script must not redefine BOLT_USER; it comes from install-layout.env"
    );
    assert!(
        !install.contains("BOLT_GROUP=\""),
        "install script must not redefine BOLT_GROUP; it comes from install-layout.env"
    );
    let layout_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install-layout.env");
    let layout = fs::read_to_string(&layout_path).expect("install layout should exist");
    assert!(
        layout.contains("BOLT_HOME=/srv/bolt-v2"),
        "install layout must anchor the data-volume root at /srv/bolt-v2 (matches systemd)"
    );
    assert!(
        layout.contains("BOLT_INSTALL_ROOT=/opt/bolt-v2"),
        "install layout must anchor the install root at /opt/bolt-v2 (matches systemd)"
    );
    assert!(
        layout.contains("BOLT_USER=bolt"),
        "install layout must single-source the service user the systemd unit runs as"
    );
    assert!(
        layout.contains("BOLT_GROUP=bolt"),
        "install layout must single-source the service group that owns the config bundle"
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
fn install_and_unit_share_the_same_config_subdir_component() {
    // The unit's `--config-root` is rendered from render_install_unit.py's
    // `@BOLT_CONFIG_DIR@` = `{install_root}/config`; install.sh provisions and
    // repairs the same bundle under `${BOLT_INSTALL_ROOT}/config`. Pin the shared
    // `config` component in both places so renaming the config subdir in one
    // surface without the other fails CI instead of silently splitting the path.
    let render_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/render_install_unit.py");
    let render = fs::read_to_string(&render_path).expect("render script should exist");
    assert!(
        render.contains("\"@BOLT_CONFIG_DIR@\": f\"{install_root}/config\""),
        "unit render must derive --config-root from the install root's `config` subdir"
    );

    let install_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let install = fs::read_to_string(&install_path).expect("install script should exist");
    assert!(
        install.contains("\"${BOLT_INSTALL_ROOT}/config\""),
        "install script must provision the same `config` subdir the unit's --config-root targets"
    );
}

#[test]
fn install_script_repairs_whole_config_bundle_for_service_user() {
    // `ops launch` runs as User=bolt and must read the tracked overlay (under
    // config/profiles/) AND every referenced strategy file (under
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

    // LIVE_ENV_DIR is single-sourced in deploy/install-layout.env (matches the
    // systemd unit's EnvironmentFile), not hardcoded in the installer.
    assert!(
        !install.contains("LIVE_ENV_DIR=\""),
        "install script must not redefine LIVE_ENV_DIR; it comes from install-layout.env"
    );
    let layout_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install-layout.env");
    let layout = fs::read_to_string(&layout_path).expect("install layout should exist");
    assert!(
        layout.contains("LIVE_ENV_DIR=/etc/bolt-v2"),
        "install layout must provision the systemd environment directory for live profile selection"
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
