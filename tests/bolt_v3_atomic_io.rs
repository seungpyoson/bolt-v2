use std::fs;

use bolt_v2::{
    bolt_v3_atomic_io::{
        PRIVATE_ATOMIC_FILE_MODE, private_atomic_temp_path, write_private_atomic_file,
    },
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_kill_switch_store::{KillSwitchRecoveryState, KillSwitchStore},
};

#[test]
fn atomic_write_creates_parent_writes_exact_bytes_and_renames_temp() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("nested").join("state.json");
    let temp_path = private_atomic_temp_path(&path);

    write_private_atomic_file(&path, b"{\"ok\":true}\n").expect("atomic write should succeed");

    assert_eq!(
        fs::read(&path).expect("final file should read"),
        b"{\"ok\":true}\n"
    );
    assert!(!temp_path.exists(), "temp file should be renamed away");
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

#[test]
fn atomic_write_cleans_temp_file_when_rename_fails() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("state.json");
    let temp_path = private_atomic_temp_path(&path);
    fs::create_dir(&path).expect("directory target should create");

    let result = write_private_atomic_file(&path, b"state\n");

    assert!(result.is_err());
    assert!(
        path.is_dir(),
        "failed rename must not replace target directory"
    );
    assert!(!temp_path.exists(), "failed rename should remove temp file");
}

#[test]
fn atomic_write_fails_before_temp_when_parent_is_file() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let parent = temp.path().join("not-a-dir");
    fs::write(&parent, b"file").expect("parent fixture should write");
    let path = parent.join("state.json");
    let temp_path = private_atomic_temp_path(&path);

    let result = write_private_atomic_file(&path, b"state\n");

    assert!(result.is_err());
    assert!(!temp_path.exists(), "no temp file should be left behind");
}

#[test]
fn kill_switch_store_round_trips_through_shared_atomic_writer() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("kill-switch-state.json");
    let temp_path = private_atomic_temp_path(&path);
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
        !temp_path.exists(),
        "kill-switch write should not leave temp file"
    );
    assert!(
        fs::read(&path)
            .expect("state file should read")
            .ends_with(b"\n"),
        "kill-switch state file should preserve trailing newline"
    );
}
