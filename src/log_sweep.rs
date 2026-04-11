use std::fs;
use std::path::Path;

const LOG_TARGET_DIR: &str = "var/logs";

/// Move stale NautilusTrader log files from the current directory into `var/logs/`.
///
/// Called before kernel init so only previous runs' logs exist. Errors are
/// logged to stderr and swallowed — a failed sweep must never prevent startup.
pub fn sweep_stale_logs() {
    sweep_logs_in(Path::new("."));
}

/// Entry point that accepts an explicit root directory.
/// Production code uses `sweep_stale_logs()` which defaults to CWD.
pub fn sweep_logs_in(root: &Path) {
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
            eprintln!("log_sweep: skipping {name} (already exists in {LOG_TARGET_DIR}/)");
            continue;
        }

        let source = entry.path();
        match fs::rename(&source, &dest) {
            Ok(()) => moved += 1,
            // EXDEV (18 on Linux/macOS): cross-filesystem rename not supported
            Err(e) if e.raw_os_error() == Some(18) => {
                if let Err(e) = fs::copy(&source, &dest) {
                    eprintln!("log_sweep: failed to copy {name}: {e}");
                    let _ = fs::remove_file(&dest); // clean up partial dest
                } else if let Err(e) = fs::remove_file(&source) {
                    // Copy is complete in dest — keep it. Source stays in root
                    // until the permission issue is resolved. Next launch skips
                    // this file because dest.exists() returns true.
                    eprintln!("log_sweep: copied {name} but failed to remove source: {e}");
                    moved += 1;
                } else {
                    moved += 1;
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

/// Returns true if the filename matches NautilusTrader's log naming convention:
/// `{trader_id}_{YYYY-MM-DD}_{UUID4}.log` (or `.json`).
///
/// The trader_id may contain underscores or hyphens, so we scan for the
/// `_YYYY-MM-DD_` window rather than splitting on `_`. The prefix before
/// the date must contain a hyphen (TraderId requires `NAME-TAG` format).
/// After the date window we require exactly a 36-character UUID4-shaped suffix.
pub fn is_nt_log_filename(name: &str) -> bool {
    let stem = match name.strip_suffix(".log") {
        Some(s) => s,
        None => match name.strip_suffix(".json") {
            Some(s) => s,
            None => return false,
        },
    };

    let bytes = stem.as_bytes();
    // Minimum: 1 char trader_id + '_' + 10-char date + '_' + 36-char UUID4 = 49
    if bytes.len() < 49 {
        return false;
    }

    // Scan for _YYYY-MM-DD_ pattern; require exactly 36-char UUID4 follows.
    // Need i + 12 (date window) + 36 (UUID4) <= len, i.e. i <= len - 48.
    // Exclusive upper bound is len - 47.
    for i in 0..bytes.len().saturating_sub(47) {
        if bytes[i] == b'_'
            && bytes[i + 1..i + 5].iter().all(|b| b.is_ascii_digit())
            && bytes[i + 5] == b'-'
            && bytes[i + 6..i + 8].iter().all(|b| b.is_ascii_digit())
            && bytes[i + 8] == b'-'
            && bytes[i + 9..i + 11].iter().all(|b| b.is_ascii_digit())
            && bytes[i + 11] == b'_'
        {
            let prefix = &stem[..i];
            let uuid_start = i + 12;
            // Prefix must contain a hyphen (TraderId requires NAME-TAG format)
            if prefix.contains('-') && stem.len() - uuid_start == 36 {
                return is_uuid4_format(&stem[uuid_start..]);
            }
        }
    }
    false
}

/// Returns true if `s` has the shape of a UUID4:
/// 36 chars, lowercase hex digits with dashes at exactly positions 8/13/18/23.
fn is_uuid4_format(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let b = s.as_bytes();
    b.iter().enumerate().all(|(i, &c)| match i {
        8 | 13 | 18 | 23 => c == b'-',
        _ => matches!(c, b'0'..=b'9' | b'a'..=b'f'),
    })
}
