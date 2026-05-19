use chrono::NaiveDate;
use std::fs;
use std::path::Path;

const NT_LOG_DATE_FORMAT: &str = "%Y-%m-%d";
const UUID_TEXT_LEN: usize = 36;
const UUID_DASH_POSITIONS: [usize; 4] = [8, 13, 18, 23];

/// Move stale NautilusTrader log files from the configured source directory
/// into the configured archive directory.
///
/// Called before kernel init so only previous runs' logs exist. Errors are
/// logged to stderr and swallowed — a failed sweep must never prevent startup.
pub fn sweep_logs_in(source_dir: &Path, archive_dir: &Path) {
    if let Err(e) = sweep_inner(source_dir, archive_dir) {
        eprintln!("log_sweep: {e}");
    }
}

fn sweep_inner(source_dir: &Path, archive_dir: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(archive_dir)?;

    let mut moved = 0u32;
    let entries = fs::read_dir(source_dir)?;

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

        let dest = archive_dir.join(&*file_name);
        if dest.exists() {
            eprintln!(
                "log_sweep: skipping {name} (already exists in {})",
                archive_dir.display()
            );
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
        eprintln!(
            "log_sweep: moved {moved} file(s) to {}",
            archive_dir.display()
        );
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

    for (separator_index, _) in stem.match_indices('_') {
        let prefix = &stem[..separator_index];
        let suffix = &stem[separator_index + 1..];
        let Some((date, uuid)) = suffix.split_once('_') else {
            continue;
        };
        if prefix.contains('-')
            && NaiveDate::parse_from_str(date, NT_LOG_DATE_FORMAT).is_ok()
            && is_uuid4_format(uuid)
        {
            return true;
        }
    }
    false
}

/// Returns true if `s` has the shape of a UUID4:
/// 36 chars, lowercase hex digits with dashes at exactly positions 8/13/18/23.
fn is_uuid4_format(s: &str) -> bool {
    if s.len() != UUID_TEXT_LEN {
        return false;
    }
    let b = s.as_bytes();
    b.iter().enumerate().all(|(i, &c)| match i {
        _ if UUID_DASH_POSITIONS.contains(&i) => c == b'-',
        _ => matches!(c, b'0'..=b'9' | b'a'..=b'f'),
    })
}
