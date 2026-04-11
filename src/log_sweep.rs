use std::fs;
use std::path::Path;

const LOG_TARGET_DIR: &str = "var/logs";

/// Move stale NautilusTrader log files from the current directory into `var/logs/`.
///
/// Called before kernel init so only previous runs' logs exist. Errors are
/// logged to stderr and swallowed — a failed sweep must never prevent startup.
pub fn sweep_stale_logs() {
    sweep_stale_logs_from(Path::new("."));
}

/// Test-only entry point that accepts an explicit root directory.
/// Production code uses `sweep_stale_logs()` which defaults to CWD.
pub fn sweep_logs_in(root: &Path) {
    sweep_stale_logs_from(root);
}

fn sweep_stale_logs_from(root: &Path) {
    if let Err(e) = sweep_inner(root) {
        eprintln!("log_sweep: {e}");
    }
}

fn sweep_inner(root: &Path) -> Result<(), std::io::Error> {
    let target = root.join(LOG_TARGET_DIR);
    fs::create_dir_all(&target)?;

    let mut moved = 0u32;
    let entries = fs::read_dir(root)?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("log_sweep: skipping unreadable entry: {e}");
                continue;
            }
        };

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !is_nt_log_filename(&name) {
            continue;
        }

        let dest = target.join(&*file_name);
        if dest.exists() {
            eprintln!("log_sweep: skipping {name} (already exists in {LOG_TARGET_DIR})");
            continue;
        }

        let source = entry.path();
        match fs::rename(&source, &dest) {
            Ok(()) => moved += 1,
            // EXDEV (18 on Linux/macOS): cross-filesystem rename not supported
            Err(e) if e.raw_os_error() == Some(18) => {
                match fs::copy(&source, &dest).and_then(|_| fs::remove_file(&source)) {
                    Ok(()) => moved += 1,
                    Err(e) => eprintln!("log_sweep: failed to copy+remove {name}: {e}"),
                }
            }
            Err(e) => eprintln!("log_sweep: failed to move {name}: {e}"),
        }
    }

    if moved > 0 {
        eprintln!("log_sweep: moved {moved} file(s) to {LOG_TARGET_DIR}/");
    }

    Ok(())
}

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
