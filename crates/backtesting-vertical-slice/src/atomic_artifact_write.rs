//! Crash-safe artifact writes via temp-file + rename.
//!
//! Invariant: a write is either fully visible or absent. A process crash
//! mid-write never leaves a truncated or corrupt file at the target path,
//! because the OS rename(2) is atomic on a single filesystem.
//!
//! Usage: replace bare `fs::write(path, bytes)` with `atomic_write(path, bytes)`.
//! The caller is still responsible for the "if path.exists() → mismatch-check"
//! guard that precedes any write; this helper only makes the write itself safe.

use std::{fs, path::Path};

/// Write `bytes` to `path` atomically via a `.tmp` sibling in the same directory.
///
/// The temp file is written first; if the write succeeds it is renamed onto
/// `path` (atomic on a single filesystem). On any error the temp file is
/// removed so no orphan `.tmp` remains. Returns `std::io::Error` on failure.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: path has no parent directory",
        )
    })?;
    // Derive a deterministic sibling name so two concurrent writers for the
    // same target collide on the rename (last writer wins) rather than
    // silently forking output.
    let tmp_name = format!(
        "{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("artifact")
    );
    let tmp_path = dir.join(&tmp_name);
    // Write to temp; clean up on any error so no orphan remains.
    if let Err(write_err) = fs::write(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path);
        return Err(write_err);
    }
    if let Err(rename_err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(rename_err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::atomic_write;

    /// Target receives exactly the supplied bytes after a successful write.
    #[test]
    fn atomic_write_produces_correct_content() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("artifact.json");
        let payload = b"{\"v\":1}";

        atomic_write(&target, payload).expect("atomic_write must succeed");

        let on_disk = std::fs::read(&target).expect("read back");
        assert_eq!(on_disk, payload, "on-disk bytes must equal payload");
    }

    /// No `.tmp` residue remains after a successful write.
    #[test]
    fn atomic_write_leaves_no_tmp_residue() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("artifact.json");
        let tmp = dir.path().join("artifact.json.tmp");

        atomic_write(&target, b"hello").expect("atomic_write must succeed");

        assert!(
            !tmp.exists(),
            ".tmp sibling must not remain after successful write"
        );
    }

    /// A second write with identical bytes succeeds (idempotent; target already exists).
    #[test]
    fn atomic_write_idempotent_same_bytes() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("artifact.json");
        let payload = b"[1,2,3]";

        atomic_write(&target, payload).expect("first write");
        atomic_write(&target, payload).expect("second write with same bytes must succeed");

        let on_disk = std::fs::read(&target).expect("read back");
        assert_eq!(on_disk, payload);
    }

    /// No `.tmp` residue remains when the supplied path has no parent directory.
    /// (Error path: atomic_write returns Err, nothing is left behind.)
    #[test]
    fn atomic_write_error_leaves_no_residue_in_dir() {
        // Write to a directory that does not exist — the fs::write to the .tmp
        // will fail, and the cleanup must not panic.
        let dir = tempfile::TempDir::new().expect("temp dir");
        let nonexistent_subdir = dir.path().join("no_such_dir").join("artifact.json");

        let result = atomic_write(&nonexistent_subdir, b"data");
        assert!(result.is_err(), "write to missing dir must fail");

        // No orphan .tmp in a directory that does not even exist — trivially true,
        // but assert the temp dir itself has no unexpected files.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list temp dir")
            .collect();
        assert!(entries.is_empty(), "no files must be left in temp dir");
    }
}
