use bolt_v2::log_sweep;
use std::fs;
use tempfile::tempdir;

// ── unit tests ────────────────────────────────────────────────────────────────

#[test]
fn matches_standard_nt_log() {
    assert!(log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_e21185e1-8222-454d-ac77-62f3dad8e95c.log"
    ));
}

#[test]
fn matches_different_trader_id() {
    assert!(log_sweep::is_nt_log_filename(
        "TRADER-XYZ_2025-01-01_abcdef.log"
    ));
}

#[test]
fn rejects_no_date_pattern() {
    assert!(!log_sweep::is_nt_log_filename("application.log"));
}

#[test]
fn rejects_non_log_extension() {
    assert!(!log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_uuid.txt"
    ));
}

#[test]
fn rejects_date_without_surrounding_underscores() {
    assert!(!log_sweep::is_nt_log_filename("2026-04-11.log"));
}

#[test]
fn rejects_too_short() {
    assert!(!log_sweep::is_nt_log_filename("a.log"));
}

#[test]
fn rejects_empty() {
    assert!(!log_sweep::is_nt_log_filename(""));
}

// ── integration tests ─────────────────────────────────────────────────────────

#[test]
fn sweep_moves_matching_logs_to_target() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Plant fake NT log files
    let log1 = "BOLT-001_2026-04-10_aaaa-bbbb.log";
    let log2 = "BOLT-001_2026-04-11_cccc-dddd.log";
    fs::write(root.join(log1), "log content 1").unwrap();
    fs::write(root.join(log2), "log content 2").unwrap();

    // Plant a non-matching file that should NOT be moved
    let keep = "readme.log";
    fs::write(root.join(keep), "not a NT log").unwrap();

    log_sweep::sweep_logs_in(root);

    // Verify logs moved
    let target = root.join("var/logs");
    assert!(target.join(log1).exists(), "log1 should be in var/logs/");
    assert!(target.join(log2).exists(), "log2 should be in var/logs/");

    // Verify non-matching file stayed
    assert!(root.join(keep).exists(), "non-matching file should remain");

    // Verify originals removed from root
    assert!(!root.join(log1).exists(), "log1 should not be in root");
    assert!(!root.join(log2).exists(), "log2 should not be in root");
}

#[test]
fn sweep_skips_existing_destination() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let log_name = "BOLT-001_2026-04-10_aaaa.log";
    fs::write(root.join(log_name), "new content").unwrap();

    // Pre-create the destination
    let target = root.join("var/logs");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(log_name), "old content").unwrap();

    log_sweep::sweep_logs_in(root);

    // Original should still exist (not moved because dest exists)
    assert!(root.join(log_name).exists());
    // Destination should have old content (not overwritten)
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

    // var/logs/ should be created but empty
    let target = root.join("var/logs");
    assert!(target.exists(), "var/logs/ should be created");
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}
