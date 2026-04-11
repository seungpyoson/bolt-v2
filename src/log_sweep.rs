const LOG_TARGET_DIR: &str = "var/logs";

/// Returns true if the filename matches the NautilusTrader log naming convention:
/// `{anything}_{YYYY-MM-DD}_{anything}.log`
pub fn is_nt_log_filename(name: &str) -> bool {
    if !name.ends_with(".log") {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes.len() < 15 {
        return false;
    }
    // Scan for _YYYY-MM-DD_ pattern
    for window in bytes.windows(12) {
        if window[0] == b'_'
            && window[1..5].iter().all(|b| b.is_ascii_digit())
            && window[5] == b'-'
            && window[6..8].iter().all(|b| b.is_ascii_digit())
            && window[8] == b'-'
            && window[9..11].iter().all(|b| b.is_ascii_digit())
            && window[11] == b'_'
        {
            return true;
        }
    }
    false
}
