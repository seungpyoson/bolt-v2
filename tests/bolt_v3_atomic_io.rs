use std::{fs, path::Path};

use bolt_v2::{
    bolt_v3_atomic_io::{
        PRIVATE_ATOMIC_FILE_MODE, RUNTIME_CONFIG_FILE_MODE, private_atomic_temp_path,
        write_atomic_file_with_mode, write_private_atomic_file,
    },
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_kill_switch_store::{KillSwitchRecoveryState, KillSwitchStore},
};

fn atomic_temp_leftovers(path: &Path) -> Vec<String> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{file_name}.tmp.");
    fs::read_dir(parent)
        .expect("temp parent should read")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(prefix.as_str()))
        .collect()
}

#[test]
fn atomic_write_creates_parent_writes_exact_bytes_and_renames_temp() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("nested").join("state.json");

    write_private_atomic_file(&path, b"{\"ok\":true}\n").expect("atomic write should succeed");

    assert_eq!(
        fs::read(&path).expect("final file should read"),
        b"{\"ok\":true}\n"
    );
    assert!(
        atomic_temp_leftovers(&path).is_empty(),
        "temp file should be renamed away"
    );
}

#[test]
fn atomic_temp_path_never_collides_with_tmp_suffixed_target() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("state.tmp");

    assert_ne!(
        private_atomic_temp_path(&path),
        path,
        "atomic temp path must not degrade to in-place writes for .tmp targets"
    );
}

#[cfg(unix)]
#[test]
fn atomic_write_uses_private_file_mode() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("private.json");

    write_private_atomic_file(&path, b"private\n").expect("atomic write should succeed");

    let mode = fs::metadata(&path)
        .expect("metadata should read")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, PRIVATE_ATOMIC_FILE_MODE);
}

#[cfg(unix)]
#[test]
fn runtime_config_atomic_write_is_service_readable() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("live.toml");

    write_atomic_file_with_mode(&path, b"schema_version = 1\n", RUNTIME_CONFIG_FILE_MODE)
        .expect("atomic write should succeed");

    let mode = fs::metadata(&path)
        .expect("metadata should read")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, RUNTIME_CONFIG_FILE_MODE);
    assert_eq!(
        mode & 0o044,
        0o044,
        "generated runtime config must be group- and world-readable so the bolt service user can read it"
    );
}

#[test]
fn atomic_write_cleans_temp_file_when_rename_fails() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("state.json");
    fs::create_dir(&path).expect("directory target should create");

    let result = write_private_atomic_file(&path, b"state\n");

    assert!(result.is_err());
    assert!(
        path.is_dir(),
        "failed rename must not replace target directory"
    );
    assert!(
        atomic_temp_leftovers(&path).is_empty(),
        "failed rename should remove temp file"
    );
}

#[test]
fn atomic_write_fails_before_temp_when_parent_is_file() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let parent = temp.path().join("not-a-dir");
    fs::write(&parent, b"file").expect("parent fixture should write");
    let path = parent.join("state.json");

    let result = write_private_atomic_file(&path, b"state\n");

    assert!(result.is_err());
    assert!(
        !private_atomic_temp_path(&path).exists(),
        "no static temp file should be left behind"
    );
}

#[test]
fn kill_switch_store_round_trips_through_shared_atomic_writer() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let store = KillSwitchStore::new(path.clone());
    let state = KillSwitchState::Flat {
        halt_id: "halt-atomic".to_string(),
    };

    store.write_state(&state).expect("state should persist");

    assert_eq!(
        store.load_recovery_state().expect("state should load"),
        KillSwitchRecoveryState::Recovered(state)
    );
    assert!(
        atomic_temp_leftovers(&path).is_empty(),
        "kill-switch write should not leave temp file"
    );
    assert!(
        fs::read(&path)
            .expect("state file should read")
            .ends_with(b"\n"),
        "kill-switch state file should preserve trailing newline"
    );
}
