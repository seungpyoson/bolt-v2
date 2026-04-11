use bolt_v2::log_sweep;

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
