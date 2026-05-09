use std::fs;

#[test]
fn systemd_unit_sets_srv_working_directory() {
    let unit_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("WorkingDirectory=/srv/bolt-v2"),
        "systemd unit must anchor cwd at /srv/bolt-v2"
    );
}

#[test]
fn systemd_unit_requires_srv_mountpoint() {
    let unit_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/systemd/bolt-v2.service");
    let unit = fs::read_to_string(&unit_path).expect("systemd unit should exist");

    assert!(
        unit.contains("ExecStartPre=/usr/bin/mountpoint -q /srv/bolt-v2"),
        "systemd unit must fail fast if /srv/bolt-v2 is not mounted"
    );
}
