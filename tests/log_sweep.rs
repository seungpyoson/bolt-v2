use bolt_v2::log_sweep;
use std::fs;
use tempfile::tempdir;

// ── is_nt_log_filename unit tests ────────────────────────────────────────────

#[test]
fn accepts_canonical_nt_log() {
    assert!(log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_e21185e1-8222-454d-ac77-62f3dad8e95c.log"
    ));
}

#[test]
fn accepts_different_trader_id() {
    assert!(log_sweep::is_nt_log_filename(
        "TRADER-XYZ_2025-01-01_abcdef01-2345-6789-abcd-ef0123456789.log"
    ));
}

#[test]
fn accepts_trader_id_with_underscores() {
    assert!(log_sweep::is_nt_log_filename(
        "BOLT_001_A_2026-04-11_e21185e1-8222-454d-ac77-62f3dad8e95c.log"
    ));
}

#[test]
fn accepts_json_extension() {
    assert!(log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_e21185e1-8222-454d-ac77-62f3dad8e95c.json"
    ));
}

#[test]
fn rejects_non_uuid_suffix() {
    // The key false-positive guard: matches date pattern but suffix is not UUID4
    assert!(!log_sweep::is_nt_log_filename(
        "analysis_2026-04-11_summary.log"
    ));
}

#[test]
fn rejects_short_uuid_suffix() {
    assert!(!log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_abc123.log"
    ));
}

#[test]
fn rejects_no_date_pattern() {
    assert!(!log_sweep::is_nt_log_filename("application.log"));
}

#[test]
fn rejects_non_log_extension() {
    assert!(!log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_e21185e1-8222-454d-ac77-62f3dad8e95c.txt"
    ));
}

#[test]
fn rejects_date_without_surrounding_underscores() {
    assert!(!log_sweep::is_nt_log_filename("2026-04-11.log"));
}

#[test]
fn rejects_uuid_with_uppercase() {
    assert!(!log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_E21185E1-8222-454D-AC77-62F3DAD8E95C.log"
    ));
}

#[test]
fn rejects_too_short() {
    assert!(!log_sweep::is_nt_log_filename("a.log"));
}

#[test]
fn rejects_empty() {
    assert!(!log_sweep::is_nt_log_filename(""));
}

// ── sweep integration tests ──────────────────────────────────────────────────

#[test]
fn sweep_moves_matching_logs_to_target() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let log1 = "BOLT-001_2026-04-10_aaaa0000-bbbb-4ccc-8ddd-eeeeeeeeeeee.log";
    let log2 = "BOLT-001_2026-04-11_11111111-2222-4333-9444-555555555555.log";
    fs::write(root.join(log1), "log content 1").unwrap();
    fs::write(root.join(log2), "log content 2").unwrap();

    // Non-matching file that should NOT be moved
    let keep = "analysis_2026-04-11_summary.log";
    fs::write(root.join(keep), "not a NT log").unwrap();

    log_sweep::sweep_logs_in(root);

    let target = root.join("var/logs");
    assert!(target.join(log1).exists(), "log1 should be in var/logs/");
    assert!(target.join(log2).exists(), "log2 should be in var/logs/");
    assert_eq!(fs::read_to_string(target.join(log1)).unwrap(), "log content 1");

    assert!(root.join(keep).exists(), "non-matching file should remain");
    assert!(!root.join(log1).exists(), "log1 should not be in root");
    assert!(!root.join(log2).exists(), "log2 should not be in root");
}

#[test]
fn sweep_skips_existing_destination() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let log_name = "BOLT-001_2026-04-10_aaaa0000-bbbb-4ccc-8ddd-eeeeeeeeeeee.log";
    fs::write(root.join(log_name), "new content").unwrap();

    let target = root.join("var/logs");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(log_name), "old content").unwrap();

    log_sweep::sweep_logs_in(root);

    assert!(root.join(log_name).exists());
    assert_eq!(
        fs::read_to_string(target.join(log_name)).unwrap(),
        "old content"
    );
}

#[test]
fn sweep_noop_when_no_logs() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    fs::write(root.join("not_a_log.txt"), "hello").unwrap();

    log_sweep::sweep_logs_in(root);

    let target = root.join("var/logs");
    assert!(target.exists(), "var/logs/ should be created");
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}

#[test]
fn sweep_does_not_panic_on_unreadable_root() {
    // Trigger: pass a regular file as root. create_dir_all(file/var/logs) returns
    // ENOTDIR. sweep_inner returns Err, sweep_logs_in swallows it.
    let tmp = tempdir().unwrap();
    let file_not_dir = tmp.path().join("i-am-a-file");
    fs::write(&file_not_dir, b"not a directory").unwrap();
    assert!(file_not_dir.is_file());

    // Must not panic — error is swallowed
    log_sweep::sweep_logs_in(&file_not_dir);
}

#[test]
fn sweep_uses_hardcoded_target_dir() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let log_name = "TEST-001_2026-01-01_abcdef01-2345-6789-abcd-ef0123456789.log";
    fs::write(root.join(log_name), "data").unwrap();

    log_sweep::sweep_logs_in(root);

    assert!(root.join("var/logs").join(log_name).exists());
}
