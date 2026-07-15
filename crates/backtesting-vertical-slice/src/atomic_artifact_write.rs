//! Crash-safe artifact writes via temp-file + atomic rename.
//!
//! Invariant: a write is either fully visible (all `bytes`) or absent at the
//! target path — never truncated, never a torn interleave. Each call streams
//! into its own *uniquely named* temp sibling and publishes it with a single
//! `rename(2)`, which is atomic on a single filesystem. Because the temp name
//! is unique per call, this holds even when multiple writers target the same
//! path concurrently: each renames its own complete file, so the target always
//! contains exactly one writer's full bytes (last rename wins) and is never a
//! mix of two writers' bytes.
//!
//! Scope: this guards against *process* crashes and concurrent writers, not
//! power loss. There is no `fsync`, so after a successful return the OS may
//! still have the data or the rename buffered; a machine power-cut at that
//! instant can lose the write. "Crash-safe" here means process-crash-safe.
//!
//! Usage: replace bare `fs::write(path, bytes)` with `atomic_write(path, bytes)`.
//! The caller is still responsible for the "if path.exists() → mismatch-check"
//! guard that precedes any write; this helper only makes the write itself safe.

use std::{
    convert::Infallible,
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::operator_work_budget::{
    OperatorWorkBudgetCommitPermit, OperatorWorkBudgetGuard, OperatorWorkBudgetStage,
};

enum AtomicWriteInnerError<E> {
    Io(std::io::Error),
    Authorize(E),
}

/// Write `bytes` to `path` atomically via a uniquely named temp sibling in the
/// same directory.
///
/// The temp file carries a process- and call-unique suffix, so concurrent
/// writers never share a temp path. Each writer renames its own complete temp
/// file onto `path` (atomic on a single filesystem); the target is therefore
/// never a torn interleave, and concurrent same-target writers resolve to
/// last-rename-wins. On any error the temp file is removed so no orphan
/// remains. Returns `std::io::Error` on failure.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match atomic_write_inner(path, bytes, || {
        Ok::<Option<OperatorWorkBudgetCommitPermit>, Infallible>(None)
    }) {
        Ok(()) => Ok(()),
        Err(AtomicWriteInnerError::Io(error)) => Err(error),
        Err(AtomicWriteInnerError::Authorize(never)) => match never {},
    }
}

/// Write a completion object to a temp sibling, sample the deadline immediately
/// before rename, and consume the resulting one-use commit permit in the rename.
pub fn atomic_write_guarded(
    path: &Path,
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    atomic_write_inner(path, bytes, || {
        work_budget.authorize_commit(stage).map(Some)
    })
    .map_err(|error| match error {
        AtomicWriteInnerError::Io(error) => anyhow::Error::new(error),
        AtomicWriteInnerError::Authorize(error) => error,
    })
    .with_context(|| format!("guarded atomic write {}", path.display()))
}

fn atomic_write_inner<E, F>(
    path: &Path,
    bytes: &[u8],
    authorize_commit: F,
) -> std::result::Result<(), AtomicWriteInnerError<E>>
where
    F: FnOnce() -> std::result::Result<Option<OperatorWorkBudgetCommitPermit>, E>,
{
    let dir = path.parent().ok_or_else(|| {
        AtomicWriteInnerError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: path has no parent directory",
        ))
    })?;
    // Unique sibling name so concurrent writers for the same target never share
    // a temp file (which would let one writer's bytes overwrite another's
    // in-flight temp and commit a torn result). The atomic rename — not the
    // name — is the correctness guarantee; uniqueness only isolates writers.
    let tmp_name = format!(
        "{}.{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        unique_temp_token(),
    );
    let tmp_path = dir.join(&tmp_name);
    // Write to temp; clean up on any error so no orphan remains.
    if let Err(write_err) = fs::write(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AtomicWriteInnerError::Io(write_err));
    }
    let commit_permit = match authorize_commit() {
        Ok(permit) => permit,
        Err(error) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(AtomicWriteInnerError::Authorize(error));
        }
    };
    if let Err(rename_err) = commit_atomic_rename(&tmp_path, path, commit_permit) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AtomicWriteInnerError::Io(rename_err));
    }
    Ok(())
}

fn commit_atomic_rename(
    tmp_path: &Path,
    path: &Path,
    _permit: Option<OperatorWorkBudgetCommitPermit>,
) -> std::io::Result<()> {
    fs::rename(tmp_path, path)
}

/// Process-unique token for naming temp files. The monotonic counter guarantees
/// uniqueness within the process; the wall-clock nanos component reduces
/// collision risk across re-runs and across distinct processes that share a
/// target directory. The atomic rename — not the temp name — is the correctness
/// guarantee; uniqueness only ensures concurrent writers never clobber each
/// other's in-flight temp file.
fn unique_temp_token() -> u128 {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    nanos.wrapping_mul(1_000_003).wrapping_add(counter)
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

    /// No `.tmp` residue remains after a successful write. The temp sibling
    /// carries a unique suffix, so assert on the `.tmp` suffix, not a fixed name.
    #[test]
    fn atomic_write_leaves_no_tmp_residue() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("artifact.json");

        atomic_write(&target, b"hello").expect("atomic_write must succeed");

        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            residue.is_empty(),
            "no .tmp sibling may remain after a successful write: {residue:?}"
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

    /// Concurrent writers targeting the SAME path never commit a torn file: the
    /// final content equals exactly one writer's full payload, every writer
    /// succeeds (each owns its temp + atomic rename), and no `.tmp` residue
    /// remains. This regresses the deterministic-temp-name defect, where a
    /// shared `target.tmp` could be interleaved by concurrent `fs::write`s and
    /// committed torn, or trigger a spurious rename `ENOENT`.
    #[test]
    fn atomic_write_concurrent_same_target_never_torn() {
        use std::sync::Arc;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = Arc::new(dir.path().join("artifact.json"));
        // Distinct, equal-length payloads (uniform fill per writer): a torn
        // interleave would mix fill bytes and match no single payload.
        let payloads: Vec<Vec<u8>> = (0..8u8).map(|i| vec![b'A' + i; 8192]).collect();
        let valid: std::collections::HashSet<Vec<u8>> = payloads.iter().cloned().collect();

        let handles: Vec<_> = payloads
            .into_iter()
            .map(|payload| {
                let target = Arc::clone(&target);
                std::thread::spawn(move || atomic_write(target.as_path(), &payload))
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .expect("writer thread must not panic")
                .expect("each concurrent atomic_write must succeed (own temp, atomic rename)");
        }

        let on_disk = std::fs::read(target.as_path()).expect("read back");
        assert!(
            valid.contains(&on_disk),
            "target must hold exactly one writer's full payload, never a torn interleave"
        );
        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            residue.is_empty(),
            "no concurrent writer may leave a .tmp residue: {residue:?}"
        );
    }
}
