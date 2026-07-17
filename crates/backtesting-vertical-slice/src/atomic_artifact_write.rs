//! Crash-safe artifact writes via atomic rename or anonymous-inode link.
//!
//! Invariant: a write is either fully visible (all `bytes`) or absent at the
//! target path — never truncated, never a torn interleave. The legacy byte
//! helper publishes a uniquely named sibling with one same-filesystem rename
//! and is last-writer-wins. Linux guarded streamed artifacts publish an
//! anonymous inode with create-only `linkat`, sync the completed inode before
//! publication, and sync the pinned parent after publication; they therefore
//! have exactly one power-loss-durable winner. Neither path can expose a mix
//! of two writers' bytes.
//!
//! Scope: this guards against *process* crashes and concurrent writers, not
//! power loss for the legacy rename helper. Guarded Linux create-only writes
//! additionally establish power-loss durability before they return.
//!
//! Usage: replace bare `fs::write(path, bytes)` with `atomic_write(path, bytes)`.
//! The caller is still responsible for the "if path.exists() → mismatch-check"
//! guard that precedes any write; this helper only makes the write itself safe.

use std::{
    convert::Infallible,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::{
    ffi::{OsStr, OsString},
    io::{Read, Seek, SeekFrom},
    mem::size_of,
};

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
};

use anyhow::{Context, Result, ensure};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
type ManifestSha256Digest = sha2::digest::Output<Sha256>;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use crate::operator_work_budget::CooperativeDeadlineReader;
use crate::operator_work_budget::{
    CooperativeDeadlineWriter, OperatorWorkBudgetCommitPermit, OperatorWorkBudgetGuard,
    OperatorWorkBudgetStage, guarded_operation_outcome,
};
pub(crate) use crate::pinned_regular_file::open_pinned_regular_file;
#[cfg(test)]
pub(crate) use crate::pinned_regular_file::validate_pinned_regular_file_identity;

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
/// last-rename-wins. A failed named-temp write is deliberately retained:
/// pathname deletion cannot be made conditional on the inode still being the
/// one validated by this process. Returns `std::io::Error` on failure.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match atomic_write_inner(path, bytes, None, || {
        Ok::<Option<OperatorWorkBudgetCommitPermit>, Infallible>(None)
    }) {
        Ok(()) => Ok(()),
        Err(AtomicWriteInnerError::Io(error)) => Err(error),
        Err(AtomicWriteInnerError::Authorize(never)) => match never {},
    }
}

/// Build one immutable local artifact and reconcile an identical create race.
///
/// The anonymous inode is synced before its create-only link and the pinned
/// parent is synced afterward. A post-link parent-sync failure is reported as
/// indeterminate; a retry pins, hashes, and syncs the existing regular file
/// and parent, accepting only identical bytes. The existing name is never
/// replaced.
#[cfg(target_os = "linux")]
pub fn atomic_file_create_or_verify_guarded<T>(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    write_temp: impl FnOnce(File) -> Result<T>,
) -> Result<T> {
    work_budget.check_deadline(stage)?;
    let temp = OwnedAnonymousTempFile::create_guarded(path, work_budget, stage)
        .with_context(|| format!("create anonymous temp artifact for {}", path.display()))?;
    let callback_file = temp
        .callback_file()
        .context("clone anonymous temp artifact handle")?;
    let value = match guarded_operation_outcome(work_budget, stage, || write_temp(callback_file)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return Err(error),
        Err(error) => return Err(error),
    };
    match temp.publish_guarded(work_budget, stage) {
        Ok(()) => Ok(value),
        Err(error)
            if error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
        {
            verify_anonymous_candidate_matches_existing_guarded(&temp, path, work_budget, stage)?;
            Ok(value)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "create-only publish anonymous artifact to {}",
                path.display()
            )
        }),
    }
}

/// Build one immutable local artifact and publish it exactly once.
///
/// Unlike [`atomic_file_create_or_verify_guarded`], an occupied destination is
/// always an error. The existing name is never opened, hashed, or accepted as
/// the result of the current publication attempt.
#[cfg(target_os = "linux")]
pub fn atomic_file_create_strict_guarded<T>(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    write_temp: impl FnOnce(File) -> Result<T>,
) -> Result<T> {
    work_budget.check_deadline(stage)?;
    let temp = OwnedAnonymousTempFile::create_guarded(path, work_budget, stage)
        .with_context(|| format!("create anonymous temp artifact for {}", path.display()))?;
    let callback_file = temp
        .callback_file()
        .context("clone anonymous temp artifact handle")?;
    let value = match guarded_operation_outcome(work_budget, stage, || write_temp(callback_file)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return Err(error),
        Err(error) => return Err(error),
    };
    temp.publish_guarded(work_budget, stage)
        .with_context(|| format!("strict create-only publish anonymous artifact to {}", path.display()))?;
    Ok(value)
}

#[cfg(target_os = "linux")]
fn verify_anonymous_candidate_matches_existing_guarded(
    candidate: &OwnedAnonymousTempFile,
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    work_budget.check_deadline(stage)?;
    let candidate_identity = EntryIdentity::from_metadata(
        &candidate
            .file
            .metadata()
            .context("stat anonymous create-conflict candidate")?,
    );
    ensure!(
        candidate_identity.is_file,
        "anonymous create-conflict candidate is not a regular file"
    );
    let (mut existing, existing_identity) = open_pinned_regular_file(path)
        .with_context(|| format!("pin existing immutable artifact {}", path.display()))?;
    existing_identity.revalidate(path, &existing)?;
    ensure!(
        existing
            .metadata()
            .with_context(|| format!("stat existing immutable artifact {}", path.display()))?
            .len()
            == candidate_identity.byte_len,
        "existing immutable artifact {} has different bytes",
        path.display()
    );

    let mut expected = candidate
        .file
        .try_clone()
        .context("clone anonymous create-conflict candidate for hashing")?;
    let expected_sha256 = sha256_digest_exact_sized_open_file_guarded(
        &mut expected,
        candidate_identity.byte_len,
        work_budget,
        stage,
    )?;
    let existing_sha256 = sha256_digest_exact_sized_open_file_guarded(
        &mut existing,
        candidate_identity.byte_len,
        work_budget,
        stage,
    )?;
    existing_identity.revalidate(path, &existing)?;
    ensure!(
        EntryIdentity::from_metadata(
            &candidate
                .file
                .metadata()
                .context("re-stat anonymous create-conflict candidate")?,
        ) == candidate_identity,
        "anonymous create-conflict candidate changed during verification"
    );
    ensure!(
        expected_sha256 == existing_sha256,
        "existing immutable artifact {} has different bytes",
        path.display()
    );
    existing
        .sync_all()
        .with_context(|| format!("sync existing immutable artifact {}", path.display()))?;
    let parent = PinnedParentDirectory::open(path)
        .with_context(|| format!("pin immutable artifact parent for {}", path.display()))?;
    parent.revalidate_path().with_context(|| {
        format!(
            "revalidate immutable artifact parent for {}",
            path.display()
        )
    })?;
    parent
        .file
        .sync_all()
        .with_context(|| format!("sync immutable artifact parent for {}", path.display()))?;
    work_budget.check_deadline(stage)
}

#[cfg(not(target_os = "linux"))]
pub fn atomic_file_create_or_verify_guarded<T>(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    write_temp: impl FnOnce(File) -> Result<T>,
) -> Result<T> {
    let _ = (path, work_budget, stage, write_temp);
    anyhow::bail!("fd-bound guarded create-or-verify publication requires Linux O_TMPFILE/linkat")
}

#[cfg(not(target_os = "linux"))]
pub fn atomic_file_create_strict_guarded<T>(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    write_temp: impl FnOnce(File) -> Result<T>,
) -> Result<T> {
    let _ = (path, work_budget, stage, write_temp);
    anyhow::bail!("fd-bound guarded strict create-only publication requires Linux O_TMPFILE/linkat")
}

/// Result of a directory staging attempt. A successful rename only stages the
/// directory; callers must exact-validate the staged tree before granting any
/// reader authority.
#[derive(Debug)]
pub(crate) enum DirectoryStageOutcome {
    NotStaged(std::io::Error),
    Staged,
}

/// A publication path whose prospective and actual allocations were admitted
/// by the work budget before any target namespace mutation.
#[derive(Debug)]
pub(crate) struct GuardedPublicationPath {
    path: PathBuf,
    retained_bytes: u64,
}

impl GuardedPublicationPath {
    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }
}

/// Capability for the uniquely-created temporary catalog root. Its directory
/// descriptor and namespace identity are retained from `mkdirat` through the
/// final child-directory rename.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
pub(crate) struct OwnedTempDirectory {
    path: PathBuf,
    parent: PinnedParentDirectory,
    name: std::ffi::CString,
    file: File,
    identity: EntryIdentity,
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[derive(Debug)]
pub(crate) struct OwnedTempDirectory {
    path: PathBuf,
}

impl OwnedTempDirectory {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

const MAX_OWNED_TEMP_RECEIPT_BYTES: usize = 4 * 1024;

/// A content-hashed, exact-set manifest of one child directory under an
/// [`OwnedTempDirectory`]. It retains only bounded identity/hash records; entry
/// handles are opened relative to pinned directory descriptors, verified, and
/// closed during each capture or staged-validation traversal.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
pub(crate) struct OwnedDirectoryManifest {
    child_name: OsString,
    entries: SegmentedManifestEntries,
    path_capacity_bytes: u64,
    inventory_bytes: u64,
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[derive(Debug)]
pub(crate) struct OwnedDirectoryManifest;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct OwnedDirectoryManifestEntry {
    relative_path: PathBuf,
    identity: EntryIdentity,
    sha256: Option<ManifestSha256Digest>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Default)]
struct SegmentedManifestEntries {
    segments: Vec<Vec<OwnedDirectoryManifestEntry>>,
    len: usize,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl SegmentedManifestEntries {
    fn len(&self) -> usize {
        self.len
    }

    fn segment_count(&self) -> usize {
        self.segments.len()
    }

    fn segment_control_capacity(&self) -> usize {
        self.segments.capacity()
    }

    fn entry_capacity(&self) -> Result<usize> {
        self.segments.iter().try_fold(0_usize, |total, segment| {
            total
                .checked_add(segment.capacity())
                .context("owned directory segmented entry capacity overflow")
        })
    }

    fn iter(&self) -> impl Iterator<Item = &OwnedDirectoryManifestEntry> {
        self.segments.iter().flat_map(|segment| segment.iter())
    }

    #[cfg(test)]
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut OwnedDirectoryManifestEntry> {
        self.segments
            .iter_mut()
            .flat_map(|segment| segment.iter_mut())
    }

    fn last(&self) -> Option<&OwnedDirectoryManifestEntry> {
        self.segments.last().and_then(|segment| segment.last())
    }

    fn sort_segments_guarded(
        &mut self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        for segment in &mut self.segments {
            work_budget.check_deadline(stage)?;
            segment.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
            work_budget.check_deadline(stage)?;
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct OpenedManifestEntry {
    record: OwnedDirectoryManifestEntry,
    file: File,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EntryIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
    byte_len: u64,
    is_file: bool,
    is_dir: bool,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl EntryIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
            mode: metadata.mode(),
            byte_len: metadata.len(),
            is_file: metadata.file_type().is_file(),
            is_dir: metadata.file_type().is_dir(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn revalidate_handle(&self, file: &File, label: &Path) -> std::io::Result<()> {
        let actual = Self::from_metadata(&file.metadata()?);
        if actual != *self {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("owned entry handle identity changed: {}", label.display()),
            ));
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NamespaceIdentity {
    device: u64,
    inode: u64,
    kind: libc::mode_t,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn namespace_identity_at(
    parent: &File,
    name: &std::ffi::CStr,
) -> std::io::Result<NamespaceIdentity> {
    namespace_identity_at_fd(parent.as_raw_fd(), name)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn namespace_identity_at_fd(
    parent_fd: std::ffi::c_int,
    name: &std::ffi::CStr,
) -> std::io::Result<NamespaceIdentity> {
    // SAFETY: `name` is NUL-terminated, `parent` is an open directory, and
    // `stat` is initialized by a successful `fstatat` before it is read.
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent_fd,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful `fstatat` initialized the structure.
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "linux")]
    let device = stat.st_dev;
    #[cfg(target_vendor = "apple")]
    let device = u64::try_from(stat.st_dev).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "namespace reports a negative device identifier",
        )
    })?;
    Ok(NamespaceIdentity {
        device,
        inode: stat.st_ino,
        kind: stat.st_mode & libc::S_IFMT,
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn namespace_identity_for_handle(metadata: &fs::Metadata) -> NamespaceIdentity {
    let kind = if metadata.file_type().is_file() {
        libc::S_IFREG
    } else if metadata.file_type().is_dir() {
        libc::S_IFDIR
    } else {
        (metadata.mode() as libc::mode_t) & libc::S_IFMT
    };
    NamespaceIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        kind,
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn validate_namespace_matches_handle(
    parent: &File,
    name: &std::ffi::CStr,
    file: &File,
    label: &Path,
) -> std::io::Result<EntryIdentity> {
    let metadata = file.metadata()?;
    let handle_identity = namespace_identity_for_handle(&metadata);
    let namespace_identity = namespace_identity_at(parent, name)?;
    if namespace_identity != handle_identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "owned entry namespace identity no longer matches its original handle: {}",
                label.display()
            ),
        ));
    }
    Ok(EntryIdentity::from_metadata(&metadata))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct OwnedTempFile {
    path: PathBuf,
    parent: PinnedParentDirectory,
    name: std::ffi::CString,
    file: File,
    device: u64,
    inode: u64,
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[derive(Debug)]
struct OwnedTempFile;

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct OwnedAnonymousTempFile {
    file: File,
    target_parent: PinnedParentDirectory,
    target_name: GuardedManifestComponent,
}

#[cfg(target_os = "linux")]
impl OwnedAnonymousTempFile {
    fn create_guarded(
        path: &Path,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        let (target_parent, target_parent_live_bytes) =
            open_manifest_target_parent_guarded(path, 0, work_budget, stage)
                .context("open anonymous temp target parent")?;
        let target_component = path
            .file_name()
            .context("anonymous temp publication target has no final component")?;
        let target_name = GuardedManifestComponent::new(
            target_component,
            "anonymous temp publication target",
            target_parent_live_bytes,
            work_budget,
            stage,
        )?;
        target_parent
            .revalidate_path()
            .context("revalidate anonymous temp target parent")?;
        work_budget.check_deadline(stage)?;
        let current = c".";
        // SAFETY: the directory descriptor and static component are live. A
        // successful O_TMPFILE descriptor owns an unnamed inode and is moved
        // directly into File; no source pathname exists to swap.
        let fd = unsafe {
            libc::openat(
                target_parent.file.as_raw_fd(),
                current.as_ptr(),
                libc::O_RDWR | libc::O_CLOEXEC | libc::O_TMPFILE,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("create Linux O_TMPFILE anonymous artifact");
        }
        // SAFETY: openat returned one newly owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        ensure!(
            file.metadata()?.file_type().is_file(),
            "Linux O_TMPFILE did not create a regular file"
        );
        work_budget.check_deadline(stage)?;
        Ok(Self {
            file,
            target_parent,
            target_name,
        })
    }

    fn callback_file(&self) -> std::io::Result<File> {
        self.file.try_clone()
    }

    fn publish_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        self.publish_with(
            work_budget,
            stage,
            || self.file.sync_all(),
            |source_fd, target_parent_fd, target_name| {
                let empty = c"";
                // SAFETY: source_fd is the live O_TMPFILE inode, target_parent_fd
                // is pinned, and both C strings remain live for the syscall.
                unsafe {
                    libc::linkat(
                        source_fd,
                        empty.as_ptr(),
                        target_parent_fd,
                        target_name,
                        libc::AT_EMPTY_PATH,
                    )
                }
            },
            || self.target_parent.file.sync_all(),
        )
    }

    fn publish_with(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
        sync_candidate: impl FnOnce() -> std::io::Result<()>,
        link: impl FnOnce(std::ffi::c_int, std::ffi::c_int, *const std::ffi::c_char) -> std::ffi::c_int,
        sync_parent: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<()> {
        ensure!(
            self.file.metadata()?.file_type().is_file(),
            "anonymous temp artifact stopped being a regular file"
        );
        self.target_parent
            .revalidate_path()
            .context("revalidate anonymous publication target parent")?;
        work_budget.check_deadline(stage)?;
        sync_candidate().context("sync anonymous artifact before create-only publication")?;
        work_budget.check_deadline(stage)?;
        let target_name = self.target_name.as_c_str()?;
        let permit = work_budget.authorize_commit(stage)?;
        let result = link_anonymous_file_with_permit(
            self.file.as_raw_fd(),
            self.target_parent.file.as_raw_fd(),
            target_name.as_ptr(),
            permit,
            link,
        );
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("link anonymous artifact create-only");
        }
        sync_parent().context("sync create-only publication parent directory")?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn link_anonymous_file_with_permit(
    source_fd: std::ffi::c_int,
    target_parent_fd: std::ffi::c_int,
    target_name: *const std::ffi::c_char,
    _permit: OperatorWorkBudgetCommitPermit,
    link: impl FnOnce(std::ffi::c_int, std::ffi::c_int, *const std::ffi::c_char) -> std::ffi::c_int,
) -> std::ffi::c_int {
    link(source_fd, target_parent_fd, target_name)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl OwnedTempFile {
    fn create(path: &Path) -> std::io::Result<Self> {
        let parent = PinnedParentDirectory::open(path)?;
        let name = path_component_c_string(path, "temp file")?;
        let path = fallible_owned_path(path)?;
        Self::create_from_parts(path, parent, name)
    }

    fn create_guarded(
        path: PathBuf,
        retained_path_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        let (parent, parent_live_bytes) =
            open_manifest_target_parent_guarded(&path, retained_path_bytes, work_budget, stage)?;
        let name_base_bytes = retained_path_bytes
            .checked_add(parent_live_bytes)
            .context("guarded named temp retained byte count overflow")?;
        let name = GuardedManifestComponent::new(
            path.file_name()
                .context("guarded named temp has no final component")?,
            "guarded named temp",
            name_base_bytes,
            work_budget,
            stage,
        )?;
        let name = std::ffi::CString::from_vec_with_nul(name.into_bytes())
            .context("guarded named temp component lost its NUL terminator")?;
        work_budget.check_deadline(stage)?;
        Self::create_from_parts(path, parent, name).map_err(anyhow::Error::new)
    }

    fn create_from_parts(
        path: PathBuf,
        parent: PinnedParentDirectory,
        name: std::ffi::CString,
    ) -> std::io::Result<Self> {
        parent.revalidate_path()?;
        // SAFETY: the parent descriptor and component are live for the call;
        // `O_EXCL` establishes unique ownership and a successful descriptor is
        // immediately transferred to `File`.
        let fd = unsafe {
            libc::openat(
                parent.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CLOEXEC | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
                0o666,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("owned temp entry is not a regular file: {}", path.display()),
            ));
        }
        let temp = Self {
            path,
            parent,
            name,
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        temp.revalidate_namespace()?;
        Ok(temp)
    }

    fn callback_file(&self) -> std::io::Result<File> {
        self.file.try_clone()
    }

    fn revalidate_namespace(&self) -> std::io::Result<()> {
        self.parent.revalidate_path()?;
        let handle_metadata = self.file.metadata()?;
        let namespace = namespace_identity_at(&self.parent.file, &self.name)?;
        if !handle_metadata.file_type().is_file()
            || handle_metadata.dev() != self.device
            || handle_metadata.ino() != self.inode
            || namespace.device != self.device
            || namespace.inode != self.inode
            || namespace.kind != libc::S_IFREG
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("owned temp file identity changed: {}", self.path.display()),
            ));
        }
        Ok(())
    }

    fn retention_outcome(&self) -> std::io::Result<OwnedTempRetentionOutcome> {
        self.parent.revalidate_path()?;
        let namespace = match namespace_identity_at(&self.parent.file, &self.name) {
            Ok(namespace) => namespace,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OwnedTempRetentionOutcome::Absent);
            }
            Err(error) => return Err(error),
        };
        if namespace.device != self.device
            || namespace.inode != self.inode
            || namespace.kind != libc::S_IFREG
        {
            return Ok(OwnedTempRetentionOutcome::ForeignEntryRetained);
        }
        // There is no portable conditional-unlink-by-inode primitive. Never
        // turn this observation into a pathname unlink: a replacement between
        // fstatat and unlinkat could otherwise delete a foreign entry.
        Ok(OwnedTempRetentionOutcome::OwnedEntryRetained)
    }
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
impl OwnedTempFile {
    fn create(_path: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "identity-owned temp files are unsupported on this platform",
        ))
    }

    fn callback_file(&self) -> std::io::Result<File> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "identity-owned temp files are unsupported on this platform",
        ))
    }

    fn retention_outcome(&self) -> std::io::Result<OwnedTempRetentionOutcome> {
        Ok(OwnedTempRetentionOutcome::ForeignEntryRetained)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnedTempRetentionOutcome {
    OwnedEntryRetained,
    Absent,
    ForeignEntryRetained,
}

#[derive(Debug)]
enum RenameCommitOutcome {
    NotCommitted(std::io::Error),
    Committed,
}

/// Create a unique temp directory through a pinned parent descriptor. A failed
/// create never grants ownership; a successful create retains both the original
/// handle and the `fstatat`-verified namespace identity.
#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn create_owned_temp_directory(path: &Path) -> std::io::Result<OwnedTempDirectory> {
    let parent = PinnedParentDirectory::open(path)?;
    let name = path_component_c_string(path, "temp directory")?;
    let path = fallible_owned_path(path)?;
    create_owned_temp_directory_from_parts(path, parent, name)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn create_owned_temp_directory_guarded(
    path: PathBuf,
    retained_path_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<OwnedTempDirectory> {
    let (parent, parent_live_bytes) =
        open_manifest_target_parent_guarded(&path, retained_path_bytes, work_budget, stage)?;
    let name_base_bytes = retained_path_bytes
        .checked_add(parent_live_bytes)
        .context("guarded temp-directory retained byte count overflow")?;
    let name = GuardedManifestComponent::new(
        path.file_name()
            .context("guarded temp directory has no final component")?,
        "guarded temp directory",
        name_base_bytes,
        work_budget,
        stage,
    )?;
    let name = std::ffi::CString::from_vec_with_nul(name.into_bytes())
        .context("guarded temp-directory component lost its NUL terminator")?;
    parent
        .revalidate_path()
        .context("revalidate guarded temp-directory parent")?;
    work_budget.check_deadline(stage)?;
    create_owned_temp_directory_from_parts(path, parent, name).map_err(anyhow::Error::new)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn create_owned_temp_directory_from_parts(
    path: PathBuf,
    parent: PinnedParentDirectory,
    name: std::ffi::CString,
) -> std::io::Result<OwnedTempDirectory> {
    parent.revalidate_path()?;
    // SAFETY: the parent descriptor and component are live, and `mkdirat`
    // creates exactly one new child or fails without establishing ownership.
    let mkdir_result = unsafe { libc::mkdirat(parent.file.as_raw_fd(), name.as_ptr(), 0o700) };
    if mkdir_result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the new entry is opened relative to the same pinned parent and a
    // successful descriptor is immediately transferred to `File`.
    let fd = unsafe {
        libc::openat(
            parent.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let identity = validate_namespace_matches_handle(&parent.file, &name, &file, &path)?;
    if !identity.is_dir {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("owned temp entry is not a directory: {}", path.display()),
        ));
    }
    // SAFETY: `geteuid` has no preconditions and cannot mutate memory.
    let effective_uid = unsafe { libc::geteuid() };
    if identity.uid != effective_uid || identity.mode & 0o777 != 0o700 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "owned temp directory must remain owned by the effective user with exact 0700 mode: {}",
                path.display()
            ),
        ));
    }
    Ok(OwnedTempDirectory {
        path,
        parent,
        name,
        file,
        identity,
    })
}

#[cfg(all(test, not(any(target_os = "linux", target_vendor = "apple"))))]
pub(crate) fn create_owned_temp_directory(path: &Path) -> std::io::Result<OwnedTempDirectory> {
    let _ = path;
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "identity-owned temp directories are unsupported on this platform",
    ))
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn create_owned_temp_directory_guarded(
    path: PathBuf,
    _retained_path_bytes: u64,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<OwnedTempDirectory> {
    let _ = path;
    anyhow::bail!("guarded identity-owned temp directories are unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl OwnedTempDirectory {
    pub(crate) fn revalidate_namespace(&self) -> std::io::Result<()> {
        self.parent.revalidate_path()?;
        let actual = validate_namespace_matches_handle(
            &self.parent.file,
            &self.name,
            &self.file,
            &self.path,
        )?;
        if !actual.is_dir
            || actual.device != self.identity.device
            || actual.inode != self.identity.inode
            || actual.uid != self.identity.uid
            || actual.mode & 0o777 != 0o700
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "owned temp directory identity changed: {}",
                    self.path.display()
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
impl OwnedTempDirectory {
    pub(crate) fn revalidate_namespace(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "identity-owned temp directories are unsupported on this platform: {}",
                self.path.display()
            ),
        ))
    }
}

/// Create one bounded receipt inside an identity-owned temporary directory.
/// The receipt is create-only and opened relative to the retained directory
/// descriptor, so a symlink or replaced pathname can never receive authority.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn initialize_owned_temp_directory_receipt_guarded(
    temp_root: &OwnedTempDirectory,
    receipt_name: &std::ffi::OsStr,
    receipt_bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    ensure_single_path_component(receipt_name, "owned temp receipt")?;
    ensure!(
        !receipt_bytes.is_empty() && receipt_bytes.len() <= MAX_OWNED_TEMP_RECEIPT_BYTES,
        "owned temp receipt must contain 1..={MAX_OWNED_TEMP_RECEIPT_BYTES} bytes"
    );
    temp_root
        .revalidate_namespace()
        .context("revalidate owned temp root before receipt create")?;
    let name = std::ffi::CString::new(receipt_name.as_bytes())
        .context("owned temp receipt name contains an interior NUL")?;
    work_budget.check_deadline(stage)?;
    // SAFETY: the retained directory descriptor and receipt component remain
    // live for the call; create-only + no-follow grants exactly one file.
    let fd = unsafe {
        libc::openat(
            temp_root.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("create identity-owned temp receipt");
    }
    // SAFETY: openat returned one newly owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let identity = validate_namespace_matches_handle(
        &temp_root.file,
        &name,
        &file,
        &temp_root.path.join(receipt_name),
    )?;
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        identity.is_file && identity.uid == effective_uid && identity.mode & 0o777 == 0o600,
        "identity-owned temp receipt must be an owner-only regular file"
    );
    let mut writer = CooperativeDeadlineWriter::new(file, work_budget, stage);
    writer
        .write_all(receipt_bytes)
        .context("write identity-owned temp receipt")?;
    writer
        .flush()
        .context("flush identity-owned temp receipt")?;
    let file = writer.into_inner();
    file.sync_all()
        .context("sync identity-owned temp receipt")?;
    temp_root
        .file
        .sync_all()
        .context("sync identity-owned temp receipt parent")?;
    work_budget.check_deadline(stage)?;
    temp_root
        .revalidate_namespace()
        .context("revalidate owned temp root after receipt create")?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn validate_owned_temp_directory_receipt(
    temp_root: &OwnedTempDirectory,
    receipt_name: &std::ffi::OsStr,
    expected_bytes: &[u8],
) -> Result<()> {
    ensure_single_path_component(receipt_name, "owned temp receipt")?;
    ensure!(
        !expected_bytes.is_empty() && expected_bytes.len() <= MAX_OWNED_TEMP_RECEIPT_BYTES,
        "owned temp receipt must contain 1..={MAX_OWNED_TEMP_RECEIPT_BYTES} bytes"
    );
    temp_root.revalidate_namespace()?;
    let name = std::ffi::CString::new(receipt_name.as_bytes())
        .context("owned temp receipt name contains an interior NUL")?;
    // SAFETY: the directory descriptor and single component remain live.
    let fd = unsafe {
        libc::openat(
            temp_root.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open owned temp receipt");
    }
    // SAFETY: openat returned one newly owned descriptor.
    let mut receipt = unsafe { File::from_raw_fd(fd) };
    let identity = validate_namespace_matches_handle(
        &temp_root.file,
        &name,
        &receipt,
        &temp_root.path.join(receipt_name),
    )?;
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        identity.is_file
            && identity.uid == effective_uid
            && identity.mode & 0o777 == 0o600
            && identity.byte_len == u64::try_from(expected_bytes.len())?,
        "owned temp receipt identity or length changed"
    );
    ensure!(
        receipt.metadata()?.nlink() == 1,
        "owned temp receipt must have exactly one link"
    );
    let mut actual = Vec::new();
    receipt
        .by_ref()
        .take(u64::try_from(MAX_OWNED_TEMP_RECEIPT_BYTES)?)
        .read_to_end(&mut actual)
        .context("read owned temp receipt")?;
    ensure!(actual == expected_bytes, "owned temp receipt bytes changed");
    validate_namespace_matches_handle(
        &temp_root.file,
        &name,
        &receipt,
        &temp_root.path.join(receipt_name),
    )?;
    temp_root.revalidate_namespace()?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn validate_owned_temp_directory_receipt(
    _temp_root: &OwnedTempDirectory,
    _receipt_name: &std::ffi::OsStr,
    _expected_bytes: &[u8],
) -> Result<()> {
    anyhow::bail!("identity-owned temp receipt validation is unsupported on this platform")
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn initialize_owned_temp_directory_receipt_guarded(
    _temp_root: &OwnedTempDirectory,
    _receipt_name: &std::ffi::OsStr,
    _receipt_bytes: &[u8],
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<()> {
    anyhow::bail!("identity-owned temp receipts are unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn remove_identity_owned_entry_at_guarded(
    parent: &File,
    name: &std::ffi::OsStr,
    display_path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    ensure_single_path_component(name, "owned temp cleanup entry")?;
    let name = std::ffi::CString::new(name.as_bytes())
        .context("owned temp cleanup entry contains an interior NUL")?;
    work_budget.check_deadline(stage)?;
    // Open without following before deciding whether this is a file or a
    // directory. A symlink therefore fails closed and remains untouched.
    let file_fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if file_fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("open owned temp cleanup entry {}", display_path.display()));
    }
    // SAFETY: openat returned one newly owned descriptor.
    let entry = unsafe { File::from_raw_fd(file_fd) };
    let identity = validate_namespace_matches_handle(parent, &name, &entry, display_path)?;
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        identity.uid == effective_uid,
        "owned temp cleanup entry {} is foreign-owned",
        display_path.display()
    );
    if identity.is_dir {
        loop {
            let mut selected = None;
            for_each_directory_component(&entry, |child| {
                if selected.is_none() {
                    selected = Some(child.to_os_string());
                }
                Ok(())
            })?;
            let Some(child) = selected else { break };
            remove_identity_owned_entry_at_guarded(
                &entry,
                &child,
                &display_path.join(&child),
                work_budget,
                stage,
            )?;
        }
        validate_namespace_matches_handle(parent, &name, &entry, display_path)?;
        work_budget.check_deadline(stage)?;
        // SAFETY: parent/name remain live; the directory is empty and its
        // descriptor was revalidated against the namespace immediately above.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!("remove owned temp directory {}", display_path.display())
            });
        }
    } else {
        ensure!(
            identity.is_file,
            "owned temp cleanup entry {} is neither a regular file nor directory",
            display_path.display()
        );
        validate_namespace_matches_handle(parent, &name, &entry, display_path)?;
        work_budget.check_deadline(stage)?;
        // SAFETY: parent/name remain live and the opened regular file was
        // revalidated against the namespace immediately above.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("remove owned temp file {}", display_path.display()));
        }
    }
    Ok(())
}

/// Remove every descendant except the exact receipt from an identity-owned
/// temporary directory. Traversal and removal remain descriptor-relative;
/// symlinks, special files, owner drift, and identity replacement fail closed.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn compact_owned_temp_directory_to_receipt_guarded(
    temp_root: &OwnedTempDirectory,
    receipt_name: &std::ffi::OsStr,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    ensure_single_path_component(receipt_name, "owned temp receipt")?;
    temp_root
        .revalidate_namespace()
        .context("revalidate owned temp root before receipt compaction")?;
    loop {
        let mut selected = None;
        for_each_directory_component(&temp_root.file, |name| {
            if name != receipt_name && selected.is_none() {
                selected = Some(name.to_os_string());
            }
            Ok(())
        })?;
        let Some(name) = selected else { break };
        remove_identity_owned_entry_at_guarded(
            &temp_root.file,
            &name,
            &temp_root.path.join(&name),
            work_budget,
            stage,
        )?;
    }
    temp_root
        .revalidate_namespace()
        .context("revalidate owned temp root after receipt compaction")?;
    work_budget.check_deadline(stage)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct OwnedTempCleanupProgress {
    entries: u64,
    max_entries: u64,
    max_depth: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl OwnedTempCleanupProgress {
    fn enter(&mut self, depth: u64) -> Result<()> {
        ensure!(
            depth <= self.max_depth,
            "owned temp cleanup depth {depth} exceeds configured maximum {}",
            self.max_depth
        );
        self.entries = self
            .entries
            .checked_add(1)
            .context("owned temp cleanup entry count overflow")?;
        ensure!(
            self.entries <= self.max_entries,
            "owned temp cleanup entry count exceeds configured maximum {}",
            self.max_entries
        );
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn remove_identity_owned_entry_at_bounded(
    parent: &File,
    name: &std::ffi::OsStr,
    display_path: &Path,
    depth: u64,
    progress: &mut OwnedTempCleanupProgress,
) -> Result<()> {
    progress.enter(depth)?;
    ensure_single_path_component(name, "owned temp cleanup entry")?;
    let name = std::ffi::CString::new(name.as_bytes())
        .context("owned temp cleanup entry contains an interior NUL")?;
    // SAFETY: parent and name remain live; O_NOFOLLOW rejects symlinks.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("open bounded owned temp entry {}", display_path.display()));
    }
    // SAFETY: openat returned one newly owned descriptor.
    let entry = unsafe { File::from_raw_fd(fd) };
    let identity = validate_namespace_matches_handle(parent, &name, &entry, display_path)?;
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        identity.uid == effective_uid,
        "owned temp cleanup entry {} is foreign-owned",
        display_path.display()
    );
    if identity.is_dir {
        loop {
            let mut selected = None;
            for_each_directory_component(&entry, |child| {
                if selected.is_none() {
                    selected = Some(child.to_os_string());
                }
                Ok(())
            })?;
            let Some(child) = selected else { break };
            remove_identity_owned_entry_at_bounded(
                &entry,
                &child,
                &display_path.join(&child),
                depth
                    .checked_add(1)
                    .context("owned temp cleanup depth overflow")?,
                progress,
            )?;
        }
        validate_namespace_matches_handle(parent, &name, &entry, display_path)?;
        // SAFETY: the directory is empty and its namespace identity was revalidated.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "remove bounded owned temp directory {}",
                    display_path.display()
                )
            });
        }
    } else {
        ensure!(
            identity.is_file && entry.metadata()?.nlink() == 1,
            "owned temp cleanup entry {} is not one regular file",
            display_path.display()
        );
        validate_namespace_matches_handle(parent, &name, &entry, display_path)?;
        // SAFETY: the regular-file namespace identity was revalidated.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!("remove bounded owned temp file {}", display_path.display())
            });
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn compact_owned_temp_directory_to_receipt_bounded(
    temp_root: &OwnedTempDirectory,
    receipt_name: &std::ffi::OsStr,
    receipt_bytes: &[u8],
    max_entries: u64,
    max_depth: u64,
) -> Result<()> {
    ensure!(
        max_entries > 0 && max_entries != u64::MAX,
        "owned temp cleanup max_entries must be positive and finite"
    );
    ensure!(
        max_depth > 0 && max_depth <= max_entries,
        "owned temp cleanup max_depth must be positive and no greater than max_entries"
    );
    validate_owned_temp_directory_receipt(temp_root, receipt_name, receipt_bytes)?;
    let mut progress = OwnedTempCleanupProgress {
        entries: 0,
        max_entries,
        max_depth,
    };
    loop {
        let mut selected = None;
        for_each_directory_component(&temp_root.file, |name| {
            if name != receipt_name && selected.is_none() {
                selected = Some(name.to_os_string());
            }
            Ok(())
        })?;
        let Some(name) = selected else { break };
        remove_identity_owned_entry_at_bounded(
            &temp_root.file,
            &name,
            &temp_root.path.join(&name),
            1,
            &mut progress,
        )?;
    }
    validate_owned_temp_directory_receipt(temp_root, receipt_name, receipt_bytes)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn compact_owned_temp_directory_to_receipt_bounded(
    _temp_root: &OwnedTempDirectory,
    _receipt_name: &std::ffi::OsStr,
    _receipt_bytes: &[u8],
    _max_entries: u64,
    _max_depth: u64,
) -> Result<()> {
    anyhow::bail!("bounded identity-owned temp cleanup is unsupported on this platform")
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn compact_owned_temp_directory_to_receipt_guarded(
    _temp_root: &OwnedTempDirectory,
    _receipt_name: &std::ffi::OsStr,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<()> {
    anyhow::bail!("identity-owned temp receipt compaction is unsupported on this platform")
}

/// Capture the exact physical entry set and every regular file's SHA-256 below
/// one child of the owned temp root. Inventory records and total physical bytes
/// are checked against the same work budget before allocation grows.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn capture_owned_directory_manifest_guarded(
    temp_root: &OwnedTempDirectory,
    child_name: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<OwnedDirectoryManifest> {
    temp_root
        .revalidate_namespace()
        .context("revalidate owned temp root before manifest capture")?;
    let manifest = capture_directory_manifest_at_guarded(
        &temp_root.file,
        std::ffi::OsStr::new(child_name),
        0,
        work_budget,
        stage,
    )?;
    temp_root
        .revalidate_namespace()
        .context("revalidate owned temp root after manifest capture")?;
    Ok(manifest)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn capture_directory_manifest_at_guarded(
    parent: &File,
    child_name: &std::ffi::OsStr,
    retained_inventory_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<OwnedDirectoryManifest> {
    capture_directory_manifest_at_with_post_traversal_hook_guarded(
        parent,
        child_name,
        retained_inventory_bytes,
        work_budget,
        stage,
        &mut |_| Ok(()),
    )
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn capture_directory_manifest_at_with_post_traversal_hook_guarded(
    parent: &File,
    child_name: &std::ffi::OsStr,
    retained_inventory_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    post_traversal: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<OwnedDirectoryManifest> {
    ensure_single_path_component(child_name, "manifest child")?;
    let child_name = clone_manifest_child_name_guarded(
        child_name,
        retained_inventory_bytes,
        work_budget,
        stage,
    )?;
    let mut manifest = OwnedDirectoryManifest {
        child_name,
        entries: SegmentedManifestEntries::default(),
        path_capacity_bytes: 0,
        inventory_bytes: 0,
    };
    manifest.inventory_bytes = exact_manifest_inventory_bytes(&manifest)?;
    let mut physical_file_bytes = 0_u64;
    let root_pending_bytes = u64::try_from(size_of::<OwnedDirectoryManifestEntry>())
        .context("owned directory root record bytes do not fit u64")?;
    let root_live_bytes = capture_live_bytes(
        retained_inventory_bytes,
        manifest.inventory_bytes,
        physical_file_bytes,
        root_pending_bytes,
    )?;
    let child_c_name = GuardedManifestComponent::new(
        manifest.child_name.as_os_str(),
        "manifest child",
        root_live_bytes,
        work_budget,
        stage,
    )?;
    let open_live_bytes = root_live_bytes
        .checked_add(child_c_name.live_bytes)
        .context("owned directory root component live byte count overflow")?;
    let root_entry = open_manifest_entry_at(
        parent,
        child_c_name.as_c_str()?,
        PathBuf::new(),
        open_live_bytes,
        work_budget,
        stage,
    )?;
    ensure!(
        root_entry.record.identity.is_dir,
        "owned manifest root must be a directory"
    );
    let root_identity = root_entry.record.identity;
    push_manifest_entry_guarded(
        &mut manifest,
        root_entry.record,
        &mut physical_file_bytes,
        retained_inventory_bytes,
        child_c_name.live_bytes,
        work_budget,
        stage,
    )?;
    let traversal_context = DirectoryManifestTraversalContext {
        retained_inventory_bytes,
        work_budget,
        stage,
    };
    accumulate_directory_manifest_guarded(
        &root_entry.file,
        Path::new(""),
        &mut manifest,
        &mut physical_file_bytes,
        child_c_name.live_bytes,
        &traversal_context,
        post_traversal,
    )?;
    manifest.entries.sort_segments_guarded(work_budget, stage)?;
    post_traversal(Path::new(""))?;
    revalidate_manifest_directory_namespace(
        parent,
        child_c_name.as_c_str()?,
        &root_entry.file,
        root_identity,
        Path::new(""),
    )?;
    Ok(manifest)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct DirectoryManifestTraversalContext<'a> {
    retained_inventory_bytes: u64,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn accumulate_directory_manifest_guarded(
    directory: &File,
    directory_relative_path: &Path,
    manifest: &mut OwnedDirectoryManifest,
    physical_file_bytes: &mut u64,
    ancestor_path_bytes: u64,
    context: &DirectoryManifestTraversalContext<'_>,
    post_traversal: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    context.work_budget.check_deadline(context.stage)?;
    for_each_directory_component(directory, |name| {
        let captured_live_bytes = capture_live_bytes(
            context.retained_inventory_bytes,
            manifest.inventory_bytes,
            *physical_file_bytes,
            ancestor_path_bytes,
        )?;
        let relative_path = join_manifest_path_guarded(
            directory_relative_path,
            name,
            captured_live_bytes,
            context.work_budget,
            context.stage,
        )?;
        let pending_entry_bytes = u64::try_from(size_of::<OwnedDirectoryManifestEntry>())
            .context("owned directory pending entry bytes do not fit u64")?
            .checked_add(
                u64::try_from(relative_path.capacity())
                    .context("owned directory pending path capacity does not fit u64")?,
            )
            .context("owned directory pending entry live byte count overflow")?;
        let pending_live_bytes = captured_live_bytes
            .checked_add(pending_entry_bytes)
            .context("owned directory pending manifest live byte count overflow")?;
        let c_name = GuardedManifestComponent::new(
            name,
            "manifest entry",
            pending_live_bytes,
            context.work_budget,
            context.stage,
        )?;
        let open_live_bytes = pending_live_bytes
            .checked_add(c_name.live_bytes)
            .context("owned directory component live byte count overflow")?;
        let opened = open_manifest_entry_at(
            directory,
            c_name.as_c_str()?,
            relative_path,
            open_live_bytes,
            context.work_budget,
            context.stage,
        )?;
        let entry_identity = opened.record.identity;
        let is_directory = opened.record.identity.is_dir;
        let component_ancestor_path_bytes = ancestor_path_bytes
            .checked_add(c_name.live_bytes)
            .context("owned directory component ancestor byte count overflow")?;
        push_manifest_entry_guarded(
            manifest,
            opened.record,
            physical_file_bytes,
            context.retained_inventory_bytes,
            component_ancestor_path_bytes,
            context.work_budget,
            context.stage,
        )?;
        if is_directory {
            let relative_path = manifest
                .entries
                .last()
                .context("manifest entry disappeared after append")?
                .relative_path
                .as_path();
            let live_bytes = capture_live_bytes(
                context.retained_inventory_bytes,
                manifest.inventory_bytes,
                *physical_file_bytes,
                component_ancestor_path_bytes,
            )?;
            let (recursive_path, recursive_path_bytes) = clone_manifest_path_guarded(
                relative_path,
                live_bytes,
                context.work_budget,
                context.stage,
            )?;
            let nested_ancestor_path_bytes = component_ancestor_path_bytes
                .checked_add(recursive_path_bytes)
                .context("owned directory recursive path live byte count overflow")?;
            accumulate_directory_manifest_guarded(
                &opened.file,
                &recursive_path,
                manifest,
                physical_file_bytes,
                nested_ancestor_path_bytes,
                context,
                post_traversal,
            )?;
            revalidate_manifest_directory_namespace(
                directory,
                c_name.as_c_str()?,
                &opened.file,
                entry_identity,
                &recursive_path,
            )?;
        }
        Ok(())
    })?;
    if !directory_relative_path.as_os_str().is_empty() {
        post_traversal(directory_relative_path)?;
    }
    context.work_budget.check_deadline(context.stage)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn revalidate_manifest_directory_namespace(
    parent: &File,
    name: &std::ffi::CStr,
    directory: &File,
    expected_identity: EntryIdentity,
    relative_path: &Path,
) -> Result<()> {
    expected_identity
        .revalidate_handle(directory, relative_path)
        .with_context(|| {
            format!(
                "manifest directory namespace changed during capture: {}",
                relative_path.display()
            )
        })?;
    let actual_identity = validate_namespace_matches_handle(parent, name, directory, relative_path)
        .with_context(|| {
            format!(
                "manifest directory namespace changed during capture: {}",
                relative_path.display()
            )
        })?;
    ensure!(
        actual_identity == expected_identity,
        "manifest directory namespace changed during capture: {}",
        relative_path.display()
    );
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn capture_owned_directory_manifest_guarded(
    _temp_root: &OwnedTempDirectory,
    _child_name: &str,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<OwnedDirectoryManifest> {
    anyhow::bail!("owned directory manifests are unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn push_manifest_entry_guarded(
    manifest: &mut OwnedDirectoryManifest,
    entry: OwnedDirectoryManifestEntry,
    physical_file_bytes: &mut u64,
    retained_inventory_bytes: u64,
    ancestor_path_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let path_capacity = entry.relative_path.capacity();
    let pending_record_bytes = size_of::<OwnedDirectoryManifestEntry>()
        .checked_add(path_capacity)
        .context("owned directory manifest pending record byte size overflow")?;
    let allocation_limit = work_budget
        .decoded_byte_limit()
        .map_or(usize::MAX, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        });
    ensure!(
        pending_record_bytes <= allocation_limit,
        "owned directory manifest record requires {pending_record_bytes} bytes, exceeding max_decoded_bytes {allocation_limit}"
    );
    let next_path_capacity_bytes = manifest
        .path_capacity_bytes
        .checked_add(
            u64::try_from(path_capacity).context("manifest path capacity does not fit u64")?,
        )
        .context("owned directory manifest path capacity byte count overflow")?;
    let next_physical_file_bytes = if entry.identity.is_file {
        physical_file_bytes
            .checked_add(entry.identity.byte_len)
            .context("owned directory manifest physical byte count overflow")?
    } else {
        *physical_file_bytes
    };
    let pending_record_bytes = u64::try_from(pending_record_bytes)
        .context("owned directory pending record bytes do not fit u64")?;
    let needs_segment = manifest
        .entries
        .segments
        .last()
        .is_none_or(|segment| segment.len() == segment.capacity());
    if needs_segment {
        let segment_control_bytes = size_of::<Vec<OwnedDirectoryManifestEntry>>();
        let max_segment_capacity = allocation_limit
            .checked_div(size_of::<OwnedDirectoryManifestEntry>().max(1))
            .context("owned directory manifest segment capacity divisor is zero")?;
        let previous_segment_capacity = manifest.entries.segments.last().map_or(0, Vec::capacity);
        let requested_segment_capacity = if previous_segment_capacity == 0 {
            1
        } else {
            previous_segment_capacity
                .checked_mul(2)
                .unwrap_or(max_segment_capacity)
                .min(max_segment_capacity)
        };
        ensure!(
            requested_segment_capacity > 0,
            "max_decoded_bytes cannot hold one manifest entry"
        );

        if manifest.entries.segments.len() == manifest.entries.segments.capacity() {
            let max_segment_control_capacity = allocation_limit
                .checked_div(segment_control_bytes.max(1))
                .context("owned directory manifest segment-control divisor is zero")?;
            ensure!(
                max_segment_control_capacity > manifest.entries.segments.len(),
                "max_decoded_bytes cannot grow segmented manifest controls"
            );
            let current_capacity = manifest.entries.segments.capacity();
            let requested_capacity = if current_capacity == 0 {
                1
            } else {
                current_capacity
                    .checked_mul(2)
                    .unwrap_or(max_segment_control_capacity)
                    .min(max_segment_control_capacity)
            };
            let requested_allocation_bytes = requested_capacity
                .checked_mul(segment_control_bytes)
                .context("owned directory segment-control allocation byte size overflow")?;
            verify_manifest_allocation_request(
                requested_allocation_bytes,
                "owned directory segmented manifest controls",
                work_budget,
                stage,
            )?;
            let prospective_peak = capture_live_bytes(
                retained_inventory_bytes,
                manifest.inventory_bytes,
                next_physical_file_bytes,
                ancestor_path_bytes,
            )?
            .checked_add(pending_record_bytes)
            .and_then(|bytes| bytes.checked_add(u64::try_from(requested_allocation_bytes).ok()?))
            .context("owned directory segment-control prospective peak overflow")?;
            work_budget.verify_decoded_bytes(prospective_peak, stage)?;
            manifest
                .entries
                .segments
                .try_reserve_exact(requested_capacity - manifest.entries.segments.len())
                .context("reserve owned directory segmented manifest controls")?;
            let actual_allocation_bytes = manifest
                .entries
                .segments
                .capacity()
                .checked_mul(segment_control_bytes)
                .context("owned directory segment-control actual bytes overflow")?;
            verify_manifest_allocation_request(
                actual_allocation_bytes,
                "owned directory actual segmented manifest controls",
                work_budget,
                stage,
            )?;
            work_budget.verify_decoded_bytes(
                capture_live_bytes(
                    retained_inventory_bytes,
                    manifest.inventory_bytes,
                    next_physical_file_bytes,
                    ancestor_path_bytes,
                )?
                .checked_add(pending_record_bytes)
                .and_then(|bytes| bytes.checked_add(u64::try_from(actual_allocation_bytes).ok()?))
                .context("owned directory segment-control actual peak overflow")?,
                stage,
            )?;
            manifest.inventory_bytes = exact_manifest_inventory_bytes(manifest)?;
        }

        let requested_segment_bytes = requested_segment_capacity
            .checked_mul(size_of::<OwnedDirectoryManifestEntry>())
            .context("owned directory manifest segment byte size overflow")?;
        verify_manifest_allocation_request(
            requested_segment_bytes,
            "owned directory manifest entry segment",
            work_budget,
            stage,
        )?;
        work_budget.verify_decoded_bytes(
            capture_live_bytes(
                retained_inventory_bytes,
                manifest.inventory_bytes,
                next_physical_file_bytes,
                ancestor_path_bytes,
            )?
            .checked_add(pending_record_bytes)
            .and_then(|bytes| bytes.checked_add(u64::try_from(requested_segment_bytes).ok()?))
            .context("owned directory manifest segment prospective peak overflow")?,
            stage,
        )?;
        let mut segment = Vec::new();
        segment
            .try_reserve_exact(requested_segment_capacity)
            .context("reserve owned directory manifest entry segment")?;
        let actual_segment_bytes = segment
            .capacity()
            .checked_mul(size_of::<OwnedDirectoryManifestEntry>())
            .context("owned directory actual manifest segment bytes overflow")?;
        verify_manifest_allocation_request(
            actual_segment_bytes,
            "owned directory actual manifest entry segment",
            work_budget,
            stage,
        )?;
        work_budget.verify_decoded_bytes(
            capture_live_bytes(
                retained_inventory_bytes,
                manifest.inventory_bytes,
                next_physical_file_bytes,
                ancestor_path_bytes,
            )?
            .checked_add(pending_record_bytes)
            .and_then(|bytes| bytes.checked_add(u64::try_from(actual_segment_bytes).ok()?))
            .context("owned directory manifest segment actual peak overflow")?,
            stage,
        )?;
        manifest.entries.segments.push(segment);
    }
    let next_inventory_bytes = exact_manifest_inventory_bytes_for(
        &manifest.child_name,
        &manifest.entries,
        next_path_capacity_bytes,
    )?;
    let post_reserve_peak_bytes = capture_live_bytes(
        retained_inventory_bytes,
        next_inventory_bytes,
        next_physical_file_bytes,
        ancestor_path_bytes,
    )?
    .checked_add(
        u64::try_from(size_of::<OwnedDirectoryManifestEntry>())
            .context("owned directory pending entry control bytes do not fit u64")?,
    )
    .context("owned directory manifest post-reserve peak byte count overflow")?;
    work_budget.verify_decoded_bytes(post_reserve_peak_bytes, stage)?;
    manifest.path_capacity_bytes = next_path_capacity_bytes;
    manifest.inventory_bytes = next_inventory_bytes;
    *physical_file_bytes = next_physical_file_bytes;
    manifest
        .entries
        .segments
        .last_mut()
        .context("owned directory manifest segment disappeared")?
        .push(entry);
    manifest.entries.len = manifest
        .entries
        .len
        .checked_add(1)
        .context("owned directory manifest entry count overflow")?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn open_manifest_entry_at(
    parent: &File,
    name: &std::ffi::CStr,
    relative_path: PathBuf,
    live_bytes_before_physical_file: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<OpenedManifestEntry> {
    let display_path = Path::new("manifest entry");
    let namespace = namespace_identity_at(parent, name).context("fstatat manifest entry")?;
    let flags = match namespace.kind {
        kind if kind == libc::S_IFDIR => {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW
        }
        kind if kind == libc::S_IFREG => {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK
        }
        _ => anyhow::bail!(
            "owned directory manifest entry is neither a regular file nor a directory"
        ),
    };
    // SAFETY: the parent and component are live; `O_NOFOLLOW` rejects symlinks;
    // a successful descriptor is immediately transferred to `File`.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open manifest entry");
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let identity = validate_namespace_matches_handle(parent, name, &file, display_path)
        .context("pin manifest entry")?;
    let sha256 = if identity.is_file {
        let accounted_bytes = live_bytes_before_physical_file
            .checked_add(identity.byte_len)
            .context("owned manifest live plus physical byte count overflow")?;
        work_budget.verify_decoded_bytes(accounted_bytes, stage)?;
        Some(sha256_digest_exact_sized_open_file_guarded(
            &mut file,
            identity.byte_len,
            work_budget,
            stage,
        )?)
    } else {
        None
    };
    identity
        .revalidate_handle(&file, display_path)
        .context("revalidate manifest entry")?;
    validate_namespace_matches_handle(parent, name, &file, display_path)
        .context("revalidate manifest namespace")?;
    Ok(OpenedManifestEntry {
        file,
        record: OwnedDirectoryManifestEntry {
            relative_path,
            identity,
            sha256,
        },
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn exact_manifest_inventory_bytes(manifest: &OwnedDirectoryManifest) -> Result<u64> {
    exact_manifest_inventory_bytes_for(
        &manifest.child_name,
        &manifest.entries,
        manifest.path_capacity_bytes,
    )
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn exact_manifest_inventory_bytes_for(
    child_name: &OsString,
    entries: &SegmentedManifestEntries,
    path_capacity_bytes: u64,
) -> Result<u64> {
    let control_and_allocations = size_of::<OwnedDirectoryManifest>()
        .checked_add(child_name.capacity())
        .and_then(|bytes| {
            entries
                .segment_control_capacity()
                .checked_mul(size_of::<Vec<OwnedDirectoryManifestEntry>>())
                .and_then(|segment_control_bytes| bytes.checked_add(segment_control_bytes))
        })
        .and_then(|bytes| {
            entries
                .entry_capacity()
                .ok()?
                .checked_mul(size_of::<OwnedDirectoryManifestEntry>())
                .and_then(|entry_bytes| bytes.checked_add(entry_bytes))
        })
        .context("owned directory manifest exact inventory byte size overflow")?;
    u64::try_from(control_and_allocations)
        .context("owned directory manifest exact inventory bytes do not fit u64")?
        .checked_add(path_capacity_bytes)
        .context("owned directory manifest exact path inventory byte size overflow")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn capture_live_bytes(
    retained_inventory_bytes: u64,
    manifest_inventory_bytes: u64,
    physical_file_bytes: u64,
    transient_bytes: u64,
) -> Result<u64> {
    retained_inventory_bytes
        .checked_add(manifest_inventory_bytes)
        .and_then(|bytes| bytes.checked_add(physical_file_bytes))
        .and_then(|bytes| bytes.checked_add(transient_bytes))
        .context("owned directory manifest aggregate live byte count overflow")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn verify_manifest_allocation_request(
    requested_bytes: usize,
    role: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let allocation_limit = work_budget
        .decoded_byte_limit()
        .map_or(usize::MAX, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        });
    ensure!(
        requested_bytes <= allocation_limit,
        "{role} allocation requires {requested_bytes} bytes, exceeding max_decoded_bytes {allocation_limit}"
    );
    work_budget.check_deadline(stage)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn clone_manifest_child_name_guarded(
    value: &std::ffi::OsStr,
    retained_inventory_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<OsString> {
    ensure!(
        !value.as_bytes().contains(&0),
        "manifest child contains an interior NUL"
    );
    let requested_bytes = value.as_bytes().len();
    verify_manifest_allocation_request(
        requested_bytes,
        "owned directory manifest child name",
        work_budget,
        stage,
    )?;
    let fixed_bytes = retained_inventory_bytes
        .checked_add(
            u64::try_from(size_of::<OwnedDirectoryManifest>())
                .context("owned directory manifest control bytes do not fit u64")?,
        )
        .context("retained manifest plus control byte count overflow")?;
    let prospective_bytes = fixed_bytes
        .checked_add(
            u64::try_from(requested_bytes)
                .context("owned directory child name bytes do not fit u64")?,
        )
        .context("owned directory child name prospective byte count overflow")?;
    work_budget.verify_decoded_bytes(prospective_bytes, stage)?;
    let mut child_name = OsString::new();
    child_name
        .try_reserve_exact(requested_bytes)
        .context("reserve owned directory manifest child name")?;
    verify_manifest_allocation_request(
        child_name.capacity(),
        "owned directory manifest actual child name allocation",
        work_budget,
        stage,
    )?;
    let allocated_bytes = fixed_bytes
        .checked_add(
            u64::try_from(child_name.capacity())
                .context("owned directory child name capacity does not fit u64")?,
        )
        .context("owned directory child name allocated byte count overflow")?;
    work_budget.verify_decoded_bytes(allocated_bytes, stage)?;
    child_name.push(value);
    Ok(child_name)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn join_manifest_path_guarded(
    parent: &Path,
    child: &std::ffi::OsStr,
    base_live_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<PathBuf> {
    ensure_single_path_component(child, "manifest entry")?;
    let parent_bytes = parent.as_os_str().as_bytes();
    let child_bytes = child.as_bytes();
    let separator_bytes = usize::from(!parent_bytes.is_empty());
    let requested_bytes = parent_bytes
        .len()
        .checked_add(separator_bytes)
        .and_then(|bytes| bytes.checked_add(child_bytes.len()))
        .context("owned directory manifest path byte size overflow")?;
    verify_manifest_allocation_request(
        requested_bytes,
        "owned directory manifest path",
        work_budget,
        stage,
    )?;
    let path_control_bytes = u64::try_from(size_of::<PathBuf>())
        .context("owned directory path control bytes do not fit u64")?;
    let prospective_bytes = base_live_bytes
        .checked_add(path_control_bytes)
        .and_then(|bytes| bytes.checked_add(u64::try_from(requested_bytes).ok()?))
        .context("owned directory manifest path prospective byte count overflow")?;
    work_budget.verify_decoded_bytes(prospective_bytes, stage)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(requested_bytes)
        .context("reserve owned directory manifest path")?;
    verify_manifest_allocation_request(
        bytes.capacity(),
        "owned directory manifest actual path allocation",
        work_budget,
        stage,
    )?;
    let allocated_bytes = base_live_bytes
        .checked_add(path_control_bytes)
        .and_then(|live| live.checked_add(u64::try_from(bytes.capacity()).ok()?))
        .context("owned directory manifest path allocated byte count overflow")?;
    work_budget.verify_decoded_bytes(allocated_bytes, stage)?;
    bytes.extend_from_slice(parent_bytes);
    if separator_bytes != 0 {
        bytes.push(b'/');
    }
    bytes.extend_from_slice(child_bytes);
    let path = PathBuf::from(OsString::from_vec(bytes));
    ensure!(
        path.capacity() >= requested_bytes,
        "owned directory manifest path lost its reserved capacity"
    );
    verify_manifest_allocation_request(
        path.capacity(),
        "owned directory manifest converted path allocation",
        work_budget,
        stage,
    )?;
    work_budget.verify_decoded_bytes(
        base_live_bytes
            .checked_add(path_control_bytes)
            .and_then(|bytes| bytes.checked_add(u64::try_from(path.capacity()).ok()?))
            .context("owned directory converted path live byte count overflow")?,
        stage,
    )?;
    Ok(path)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn guarded_publication_child_path(
    parent: &Path,
    child: &std::ffi::OsStr,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<GuardedPublicationPath> {
    let path = join_manifest_path_guarded(parent, child, 0, work_budget, stage)?;
    let retained_bytes = size_of::<PathBuf>()
        .checked_add(path.capacity())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("guarded publication path retained byte count overflow")?;
    work_budget.verify_decoded_bytes(retained_bytes, stage)?;
    work_budget.check_deadline(stage)?;
    Ok(GuardedPublicationPath {
        path,
        retained_bytes,
    })
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn guarded_publication_child_path(
    _parent: &Path,
    _child: &std::ffi::OsStr,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<GuardedPublicationPath> {
    anyhow::bail!("guarded publication paths are unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn clone_manifest_path_guarded(
    path: &Path,
    base_live_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(PathBuf, u64)> {
    let path_bytes = path.as_os_str().as_bytes();
    verify_manifest_allocation_request(
        path_bytes.len(),
        "owned directory recursive path",
        work_budget,
        stage,
    )?;
    let control_bytes = u64::try_from(size_of::<PathBuf>())
        .context("owned directory recursive path control bytes do not fit u64")?;
    let prospective_path_bytes = control_bytes
        .checked_add(
            u64::try_from(path_bytes.len())
                .context("owned directory recursive path bytes do not fit u64")?,
        )
        .context("owned directory recursive path prospective byte count overflow")?;
    work_budget.verify_decoded_bytes(
        base_live_bytes
            .checked_add(prospective_path_bytes)
            .context("owned directory recursive path live byte count overflow")?,
        stage,
    )?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(path_bytes.len())
        .context("reserve owned directory recursive path")?;
    verify_manifest_allocation_request(
        bytes.capacity(),
        "owned directory actual recursive path allocation",
        work_budget,
        stage,
    )?;
    let allocated_path_bytes = control_bytes
        .checked_add(
            u64::try_from(bytes.capacity())
                .context("owned directory recursive path capacity does not fit u64")?,
        )
        .context("owned directory recursive path allocated byte count overflow")?;
    work_budget.verify_decoded_bytes(
        base_live_bytes
            .checked_add(allocated_path_bytes)
            .context("owned directory recursive path allocated live byte count overflow")?,
        stage,
    )?;
    bytes.extend_from_slice(path_bytes);
    let path = PathBuf::from(OsString::from_vec(bytes));
    verify_manifest_allocation_request(
        path.capacity(),
        "owned directory converted recursive path allocation",
        work_budget,
        stage,
    )?;
    let allocated_path_bytes = control_bytes
        .checked_add(
            u64::try_from(path.capacity())
                .context("owned directory converted recursive path capacity does not fit u64")?,
        )
        .context("owned directory converted recursive path byte count overflow")?;
    work_budget.verify_decoded_bytes(
        base_live_bytes
            .checked_add(allocated_path_bytes)
            .context("owned directory converted recursive path live byte count overflow")?,
        stage,
    )?;
    Ok((path, allocated_path_bytes))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn open_manifest_target_parent_guarded(
    target: &Path,
    base_live_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(PinnedParentDirectory, u64)> {
    let parent = target
        .parent()
        .context("manifest publication target has no parent")?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let requested_path_bytes = parent.as_os_str().as_bytes().len();
    let requested_c_string_bytes = requested_path_bytes
        .checked_add(1)
        .context("manifest target parent C string byte size overflow")?;
    verify_manifest_allocation_request(
        requested_path_bytes,
        "manifest publication target parent path",
        work_budget,
        stage,
    )?;
    verify_manifest_allocation_request(
        requested_c_string_bytes,
        "manifest publication target parent C string",
        work_budget,
        stage,
    )?;
    let prospective_parent_bytes = u64::try_from(size_of::<PinnedParentDirectory>())
        .context("manifest target parent control bytes do not fit u64")?
        .checked_add(
            u64::try_from(requested_path_bytes)
                .context("manifest target parent path bytes do not fit u64")?,
        )
        .and_then(|bytes| bytes.checked_add(u64::try_from(requested_c_string_bytes).ok()?))
        .context("manifest target parent prospective byte count overflow")?;
    work_budget.verify_decoded_bytes(
        base_live_bytes
            .checked_add(prospective_parent_bytes)
            .context("manifest target parent prospective live byte count overflow")?,
        stage,
    )?;
    let (path, _) = clone_manifest_path_guarded(parent, base_live_bytes, work_budget, stage)?;
    let parent_without_c_string_bytes = u64::try_from(size_of::<PinnedParentDirectory>())
        .context("manifest target parent control bytes do not fit u64")?
        .checked_add(
            u64::try_from(path.capacity())
                .context("manifest target parent path capacity does not fit u64")?,
        )
        .context("manifest target parent path allocation byte count overflow")?;
    let parent_path_c_string = GuardedManifestComponent::new_path(
        path.as_os_str(),
        "manifest publication target parent path",
        base_live_bytes
            .checked_add(parent_without_c_string_bytes)
            .context("manifest target parent C string base byte count overflow")?,
        work_budget,
        stage,
    )?;
    let path_c_string_capacity = parent_path_c_string.bytes.capacity();
    let target_parent_live_bytes = u64::try_from(size_of::<PinnedParentDirectory>())
        .context("manifest target parent control bytes do not fit u64")?
        .checked_add(
            u64::try_from(path_c_string_capacity)
                .context("manifest target parent C string capacity does not fit u64")?,
        )
        .context("manifest target parent allocated byte count overflow")?;
    work_budget.verify_decoded_bytes(
        base_live_bytes
            .checked_add(target_parent_live_bytes)
            .context("manifest target parent allocated live byte count overflow")?,
        stage,
    )?;
    let path_c_string = parent_path_c_string.into_bytes();
    drop(path);
    let parent = PinnedParentDirectory::open_owned_path(path_c_string)
        .context("open manifest publication target parent")?;
    work_budget.check_deadline(stage)?;
    Ok((parent, target_parent_live_bytes))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct GuardedManifestComponent {
    bytes: Vec<u8>,
    live_bytes: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl GuardedManifestComponent {
    fn new(
        value: &std::ffi::OsStr,
        role: &str,
        base_live_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        ensure_single_path_component(value, role)?;
        Self::new_path(value, role, base_live_bytes, work_budget, stage)
    }

    fn new_path(
        value: &std::ffi::OsStr,
        role: &str,
        base_live_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        ensure!(
            !value.as_bytes().contains(&0),
            "{role} contains an interior NUL"
        );
        let requested_bytes = value
            .as_bytes()
            .len()
            .checked_add(1)
            .context("owned directory manifest component byte size overflow")?;
        verify_manifest_allocation_request(
            requested_bytes,
            "owned directory manifest component",
            work_budget,
            stage,
        )?;
        let control_bytes = u64::try_from(size_of::<Vec<u8>>())
            .context("owned directory component control bytes do not fit u64")?;
        let prospective_live_bytes = control_bytes
            .checked_add(
                u64::try_from(requested_bytes)
                    .context("owned directory component bytes do not fit u64")?,
            )
            .context("owned directory component prospective byte count overflow")?;
        work_budget.verify_decoded_bytes(
            base_live_bytes
                .checked_add(prospective_live_bytes)
                .context("owned directory component prospective live byte count overflow")?,
            stage,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(requested_bytes)
            .context("reserve owned directory manifest component")?;
        verify_manifest_allocation_request(
            bytes.capacity(),
            "owned directory manifest actual component allocation",
            work_budget,
            stage,
        )?;
        let live_bytes = control_bytes
            .checked_add(
                u64::try_from(bytes.capacity())
                    .context("owned directory component capacity does not fit u64")?,
            )
            .context("owned directory component allocated byte count overflow")?;
        work_budget.verify_decoded_bytes(
            base_live_bytes
                .checked_add(live_bytes)
                .context("owned directory component allocated live byte count overflow")?,
            stage,
        )?;
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
        Ok(Self { bytes, live_bytes })
    }

    fn as_c_str(&self) -> Result<&std::ffi::CStr> {
        std::ffi::CStr::from_bytes_with_nul(&self.bytes)
            .context("owned directory manifest component lost its NUL terminator")
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct ManifestSha256Writer<'a> {
    hasher: Sha256,
    observed_bytes: u64,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl Write for ManifestSha256Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|_| std::io::Error::other("manifest hash deadline exceeded"))?;
        self.observed_bytes = self
            .observed_bytes
            .checked_add(
                u64::try_from(bytes.len())
                    .map_err(|_| std::io::Error::other("manifest hash length does not fit u64"))?,
            )
            .ok_or_else(|| std::io::Error::other("manifest hash byte count overflow"))?;
        self.hasher.update(bytes);
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|_| std::io::Error::other("manifest hash deadline exceeded"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|_| std::io::Error::other("manifest hash deadline exceeded"))
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn sha256_digest_exact_sized_open_file_guarded(
    file: &mut File,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<ManifestSha256Digest> {
    let metadata = file.metadata().context("stat opened manifest object")?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() == expected_bytes,
        "manifest object byte length {} does not match pinned expected size {expected_bytes}",
        metadata.len()
    );
    file.seek(SeekFrom::Start(0))
        .context("seek manifest object before hashing")?;
    let sentinel_bytes = expected_bytes
        .checked_add(1)
        .context("manifest hash sentinel length overflow")?;
    let (observed_bytes, digest) = {
        let reader = (&mut *file).take(sentinel_bytes);
        let mut reader = CooperativeDeadlineReader::new(reader, work_budget, stage);
        let mut writer = ManifestSha256Writer {
            hasher: Sha256::new(),
            observed_bytes: 0,
            work_budget,
            stage,
        };
        std::io::copy(&mut reader, &mut writer).context("hash opened manifest object")?;
        let digest = writer.hasher.finalize();
        (writer.observed_bytes, digest)
    };
    ensure!(
        observed_bytes == expected_bytes,
        "manifest object byte length {observed_bytes} does not match pinned expected size {expected_bytes} while hashing"
    );
    let final_metadata = file.metadata().context("re-stat hashed manifest object")?;
    ensure!(
        final_metadata.file_type().is_file() && final_metadata.len() == expected_bytes,
        "opened manifest object identity or length changed while hashing"
    );
    work_budget.check_deadline(stage)?;
    Ok(digest)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn ensure_single_path_component(value: &std::ffi::OsStr, role: &str) -> Result<()> {
    let mut components = Path::new(value).components();
    ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none(),
        "{role} must be exactly one normal path component"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct DirectoryStream(*mut libc::DIR);

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: the pointer was returned by `fdopendir` and remains owned by
        // this guard until exactly one `closedir` call here.
        unsafe {
            libc::closedir(self.0);
        }
    }
}

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut std::ffi::c_int {
    // SAFETY: caller uses the platform C runtime's thread-local errno pointer.
    unsafe { libc::__errno_location() }
}

#[cfg(target_vendor = "apple")]
unsafe fn errno_location() -> *mut std::ffi::c_int {
    // SAFETY: caller uses the platform C runtime's thread-local errno pointer.
    unsafe { libc::__error() }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn for_each_directory_component(
    directory: &File,
    mut visit: impl FnMut(&std::ffi::OsStr) -> Result<()>,
) -> Result<()> {
    let current = c".";
    // SAFETY: opening `.` relative to the retained directory descriptor creates
    // a new open-file description with an independent enumeration offset.
    let stream_fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            current.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if stream_fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("open independent directory descriptor for enumeration");
    }
    // SAFETY: `fdopendir` consumes the independently-opened descriptor on success.
    let stream = unsafe { libc::fdopendir(stream_fd) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: `fdopendir` failed and therefore did not consume the fd.
        unsafe {
            libc::close(stream_fd);
        }
        return Err(error).context("open directory stream from descriptor");
    }
    let stream = DirectoryStream(stream);
    loop {
        // SAFETY: errno is thread-local and reset immediately before `readdir`.
        unsafe {
            *errno_location() = 0;
        }
        // SAFETY: the stream remains open for the call and the returned record
        // is consumed before the next `readdir` invocation.
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            // SAFETY: errno is read on the same thread immediately after the
            // null `readdir` result.
            let errno = unsafe { *errno_location() };
            if errno == 0 {
                break;
            }
            return Err(std::io::Error::from_raw_os_error(errno))
                .context("enumerate directory descriptor");
        }
        // SAFETY: `d_name` is NUL-terminated within the live dirent record.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        visit(std::ffi::OsStr::from_bytes(name))?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct SegmentedManifestCursor<'a> {
    entries: &'a SegmentedManifestEntries,
    positions: Vec<usize>,
    allocation_bytes: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl<'a> SegmentedManifestCursor<'a> {
    fn new_guarded(
        entries: &'a SegmentedManifestEntries,
        base_live_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        let requested_bytes = entries
            .segment_count()
            .checked_mul(size_of::<usize>())
            .context("owned directory segmented cursor byte size overflow")?;
        verify_manifest_allocation_request(
            requested_bytes,
            "owned directory segmented comparison cursor",
            work_budget,
            stage,
        )?;
        let control_bytes = u64::try_from(size_of::<Vec<usize>>())
            .context("owned directory segmented cursor control bytes do not fit u64")?;
        work_budget.verify_decoded_bytes(
            base_live_bytes
                .checked_add(control_bytes)
                .and_then(|bytes| bytes.checked_add(u64::try_from(requested_bytes).ok()?))
                .context("owned directory segmented cursor prospective bytes overflow")?,
            stage,
        )?;
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(entries.segment_count())
            .context("reserve owned directory segmented comparison cursor")?;
        let actual_capacity_bytes = positions
            .capacity()
            .checked_mul(size_of::<usize>())
            .context("owned directory segmented cursor actual bytes overflow")?;
        verify_manifest_allocation_request(
            actual_capacity_bytes,
            "owned directory actual segmented comparison cursor",
            work_budget,
            stage,
        )?;
        let allocation_bytes = control_bytes
            .checked_add(
                u64::try_from(actual_capacity_bytes)
                    .context("owned directory cursor capacity does not fit u64")?,
            )
            .context("owned directory cursor allocation bytes overflow")?;
        work_budget.verify_decoded_bytes(
            base_live_bytes
                .checked_add(allocation_bytes)
                .context("owned directory cursor live bytes overflow")?,
            stage,
        )?;
        positions.resize(entries.segment_count(), 0);
        Ok(Self {
            entries,
            positions,
            allocation_bytes,
        })
    }

    fn next_unique(&mut self) -> Result<Option<&'a OwnedDirectoryManifestEntry>> {
        let mut selected: Option<(usize, &OwnedDirectoryManifestEntry)> = None;
        for (segment_index, segment) in self.entries.segments.iter().enumerate() {
            let Some(candidate) = segment.get(self.positions[segment_index]) else {
                continue;
            };
            if selected.is_none_or(|(_, current)| candidate.relative_path < current.relative_path) {
                selected = Some((segment_index, candidate));
            }
        }
        let Some((selected_index, selected_entry)) = selected else {
            return Ok(None);
        };
        for (segment_index, segment) in self.entries.segments.iter().enumerate() {
            let position = self.positions[segment_index];
            if segment_index != selected_index
                && segment
                    .get(position)
                    .is_some_and(|entry| entry.relative_path == selected_entry.relative_path)
            {
                anyhow::bail!("owned directory exact-set manifest contains a duplicate path");
            }
        }
        let next_position = self.positions[selected_index]
            .checked_add(1)
            .context("owned directory segmented cursor position overflow")?;
        if self.entries.segments[selected_index]
            .get(next_position)
            .is_some_and(|entry| entry.relative_path == selected_entry.relative_path)
        {
            anyhow::bail!("owned directory exact-set manifest contains a duplicate path");
        }
        self.positions[selected_index] = next_position;
        Ok(Some(selected_entry))
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl OwnedDirectoryManifest {
    #[cfg(test)]
    fn ensure_matches(
        &self,
        current: &OwnedDirectoryManifest,
        allow_root_timestamp_change: bool,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        self.ensure_matches_with_retained(
            current,
            allow_root_timestamp_change,
            0,
            work_budget,
            stage,
        )
    }

    fn ensure_matches_with_retained(
        &self,
        current: &OwnedDirectoryManifest,
        allow_root_timestamp_change: bool,
        retained_live_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        self.ensure_matches_mode_with_retained(
            current,
            allow_root_timestamp_change,
            true,
            retained_live_bytes,
            work_budget,
            stage,
        )
    }

    fn ensure_content_matches_with_retained(
        &self,
        current: &OwnedDirectoryManifest,
        retained_live_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        self.ensure_matches_mode_with_retained(
            current,
            true,
            false,
            retained_live_bytes,
            work_budget,
            stage,
        )
    }

    fn ensure_matches_mode_with_retained(
        &self,
        current: &OwnedDirectoryManifest,
        allow_root_timestamp_change: bool,
        require_inode_identity: bool,
        retained_live_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        ensure!(
            current.entries.len() == self.entries.len(),
            "owned directory exact-set entry count changed: expected {}, got {}",
            self.entries.len(),
            current.entries.len()
        );
        let manifests_live_bytes = retained_live_bytes
            .checked_add(self.inventory_bytes)
            .context("owned directory retained plus expected manifest byte size overflow")?
            .checked_add(current.inventory_bytes)
            .context("owned directory exact-set manifest byte size overflow")?;
        let mut expected_cursor = SegmentedManifestCursor::new_guarded(
            &self.entries,
            manifests_live_bytes,
            work_budget,
            stage,
        )?;
        let current_cursor_base = manifests_live_bytes
            .checked_add(expected_cursor.allocation_bytes)
            .context("owned directory exact-set expected cursor bytes overflow")?;
        let mut current_cursor = SegmentedManifestCursor::new_guarded(
            &current.entries,
            current_cursor_base,
            work_budget,
            stage,
        )?;
        work_budget.verify_decoded_bytes(
            current_cursor_base
                .checked_add(current_cursor.allocation_bytes)
                .context("owned directory exact-set cursor aggregate bytes overflow")?,
            stage,
        )?;
        loop {
            work_budget.check_deadline(stage)?;
            let expected = expected_cursor.next_unique()?;
            let actual = current_cursor.next_unique()?;
            let (Some(expected), Some(actual)) = (expected, actual) else {
                ensure!(
                    expected.is_none() && actual.is_none(),
                    "owned directory exact-set path disappeared"
                );
                break;
            };
            ensure!(
                expected.relative_path == actual.relative_path,
                "owned directory exact-set path changed"
            );
            let is_root = expected.relative_path.as_os_str().is_empty()
                && actual.relative_path.as_os_str().is_empty();
            let identity_matches = if !require_inode_identity {
                expected.identity.is_file == actual.identity.is_file
                    && expected.identity.is_dir == actual.identity.is_dir
                    && (!expected.identity.is_file
                        || expected.identity.byte_len == actual.identity.byte_len)
            } else if is_root && allow_root_timestamp_change {
                expected.identity.device == actual.identity.device
                    && expected.identity.inode == actual.identity.inode
                    && expected.identity.byte_len == actual.identity.byte_len
                    && expected.identity.is_file == actual.identity.is_file
                    && expected.identity.is_dir == actual.identity.is_dir
            } else {
                expected.identity == actual.identity
            };
            ensure!(
                identity_matches && expected.sha256 == actual.sha256,
                if require_inode_identity {
                    "owned directory exact-set manifest changed"
                } else {
                    "owned directory exact-set content changed"
                },
            );
        }
        work_budget.check_deadline(stage)
    }

    fn root_entry(&self) -> Result<&OwnedDirectoryManifestEntry> {
        self.entries
            .iter()
            .find(|entry| entry.relative_path.as_os_str().is_empty())
            .context("owned directory manifest is missing its root entry")
    }
}

/// Atomically move the manifest-bound child directory into an absent target as
/// non-authoritative staging. The operating-system no-replace primitive is
/// mandatory. A `Staged` result never grants reader authority; callers must
/// next call [`validate_staged_directory_manifest_guarded`].
pub(crate) fn stage_directory_rename_create_only_guarded(
    temp_root: &OwnedTempDirectory,
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> DirectoryStageOutcome {
    rename_owned_directory_noreplace(temp_root, manifest, target, work_budget, stage)
}

/// Recapture the staged target through a pinned parent descriptor and compare
/// its exact entry set, identities, and regular-file hashes with the original
/// manifest. Only a successful return may be converted into reader authority.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn validate_staged_directory_manifest_guarded(
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    validate_staged_directory_manifest_with_post_traversal_hook_guarded(
        manifest,
        target,
        work_budget,
        stage,
        |_| Ok(()),
    )
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn validate_staged_directory_manifest_with_post_traversal_hook_guarded(
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    mut post_traversal: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let path_live_bytes = manifest
        .inventory_bytes
        .checked_add(target.retained_bytes)
        .context("staged manifest plus publication-path byte count overflow")?;
    let (target_parent, target_parent_live_bytes) =
        open_manifest_target_parent_guarded(target.as_path(), path_live_bytes, work_budget, stage)?;
    let target_component = target
        .as_path()
        .file_name()
        .context("staged manifest target has no final component")?;
    let retained_bytes = path_live_bytes
        .checked_add(target_parent_live_bytes)
        .context("staged manifest retained byte count overflow")?;
    let current = capture_directory_manifest_at_with_post_traversal_hook_guarded(
        &target_parent.file,
        target_component,
        retained_bytes,
        work_budget,
        stage,
        &mut post_traversal,
    )?;
    target_parent
        .revalidate_path()
        .context("revalidate staged manifest target parent")?;
    drop(target_parent);
    manifest.ensure_matches_with_retained(&current, true, target.retained_bytes, work_budget, stage)
}

/// Compare an independently created directory with the candidate manifest by
/// exact path/type/regular-file length/hash content while deliberately ignoring
/// inode and timestamp identity. This is used only after a create-only staging
/// conflict; it never grants authority to different bytes or extra paths.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn validate_existing_directory_manifest_identical_guarded(
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    validate_existing_directory_manifest_identical_with_post_traversal_hook_guarded(
        manifest,
        target,
        work_budget,
        stage,
        |_| Ok(()),
    )
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn validate_existing_directory_manifest_identical_with_post_traversal_hook_guarded(
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    mut post_traversal: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let path_live_bytes = manifest
        .inventory_bytes
        .checked_add(target.retained_bytes)
        .context("existing manifest plus publication-path byte count overflow")?;
    let (target_parent, target_parent_live_bytes) =
        open_manifest_target_parent_guarded(target.as_path(), path_live_bytes, work_budget, stage)?;
    let target_component = target
        .as_path()
        .file_name()
        .context("existing manifest target has no final component")?;
    let retained_bytes = path_live_bytes
        .checked_add(target_parent_live_bytes)
        .context("existing manifest retained byte count overflow")?;
    let current = capture_directory_manifest_at_with_post_traversal_hook_guarded(
        &target_parent.file,
        target_component,
        retained_bytes,
        work_budget,
        stage,
        &mut post_traversal,
    )?;
    target_parent
        .revalidate_path()
        .context("revalidate existing manifest target parent")?;
    drop(target_parent);
    manifest.ensure_content_matches_with_retained(
        &current,
        target.retained_bytes,
        work_budget,
        stage,
    )
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn validate_staged_directory_manifest_guarded(
    _manifest: &OwnedDirectoryManifest,
    _target: &GuardedPublicationPath,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<()> {
    anyhow::bail!("staged directory manifest validation is unsupported on this platform")
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn validate_existing_directory_manifest_identical_guarded(
    _manifest: &OwnedDirectoryManifest,
    _target: &GuardedPublicationPath,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<()> {
    anyhow::bail!("existing directory manifest validation is unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_owned_replace_with(
    temp: &OwnedTempFile,
    path: &Path,
    rename: impl FnOnce(
        std::ffi::c_int,
        *const std::ffi::c_char,
        std::ffi::c_int,
        *const std::ffi::c_char,
    ) -> std::ffi::c_int,
) -> RenameCommitOutcome {
    if let Err(error) = temp.revalidate_namespace() {
        return RenameCommitOutcome::NotCommitted(error);
    }
    let target_parent = match PinnedParentDirectory::open(path) {
        Ok(parent) => parent,
        Err(error) => return RenameCommitOutcome::NotCommitted(error),
    };
    let target_name = match path_component_c_string(path, "target") {
        Ok(name) => name,
        Err(error) => return RenameCommitOutcome::NotCommitted(error),
    };
    if let Err(error) = temp.parent.revalidate_path() {
        return RenameCommitOutcome::NotCommitted(error);
    }
    if let Err(error) = target_parent.revalidate_path() {
        return RenameCommitOutcome::NotCommitted(error);
    }
    let result = rename(
        temp.parent.file.as_raw_fd(),
        temp.name.as_ptr(),
        target_parent.file.as_raw_fd(),
        target_name.as_ptr(),
    );
    if result != 0 {
        return RenameCommitOutcome::NotCommitted(std::io::Error::last_os_error());
    }
    // Legacy replacement is last-writer-wins. Once rename succeeds, a later
    // valid writer may replace the target immediately; post-validating the
    // pathname would misreport this successful commit as an error.
    RenameCommitOutcome::Committed
}

#[cfg(target_os = "linux")]
fn rename_owned_directory_noreplace(
    temp_root: &OwnedTempDirectory,
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> DirectoryStageOutcome {
    const RENAME_NOREPLACE: std::ffi::c_uint = 1;
    unsafe extern "C" {
        fn renameat2(
            olddirfd: std::ffi::c_int,
            oldpath: *const std::ffi::c_char,
            newdirfd: std::ffi::c_int,
            newpath: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    rename_owned_directory_with(
        temp_root,
        manifest,
        target,
        work_budget,
        stage,
        |source_parent, source_name, target_parent, target_name| {
            // SAFETY: both components and directory descriptors remain live.
            unsafe {
                renameat2(
                    source_parent,
                    source_name,
                    target_parent,
                    target_name,
                    RENAME_NOREPLACE,
                )
            }
        },
    )
}

#[cfg(target_vendor = "apple")]
fn rename_owned_directory_noreplace(
    temp_root: &OwnedTempDirectory,
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> DirectoryStageOutcome {
    const RENAME_EXCL: std::ffi::c_uint = 0x0000_0004;
    unsafe extern "C" {
        fn renameatx_np(
            from_dirfd: std::ffi::c_int,
            from: *const std::ffi::c_char,
            to_dirfd: std::ffi::c_int,
            to: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    rename_owned_directory_with(
        temp_root,
        manifest,
        target,
        work_budget,
        stage,
        |source_parent, source_name, target_parent, target_name| {
            // SAFETY: both components and directory descriptors remain live.
            unsafe {
                renameatx_np(
                    source_parent,
                    source_name,
                    target_parent,
                    target_name,
                    RENAME_EXCL,
                )
            }
        },
    )
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn rename_owned_directory_noreplace(
    _temp_root: &OwnedTempDirectory,
    _manifest: &OwnedDirectoryManifest,
    _target: &GuardedPublicationPath,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> DirectoryStageOutcome {
    DirectoryStageOutcome::NotStaged(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic create-only directory rename is unsupported on this platform",
    ))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_owned_directory_with(
    temp_root: &OwnedTempDirectory,
    manifest: &OwnedDirectoryManifest,
    target: &GuardedPublicationPath,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    rename: impl FnOnce(
        std::ffi::c_int,
        *const std::ffi::c_char,
        std::ffi::c_int,
        *const std::ffi::c_char,
    ) -> std::ffi::c_int,
) -> DirectoryStageOutcome {
    if let Err(error) = temp_root.revalidate_namespace() {
        return DirectoryStageOutcome::NotStaged(error);
    }
    let root_entry = match manifest.root_entry() {
        Ok(entry) => entry,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let path_live_bytes = match manifest.inventory_bytes.checked_add(target.retained_bytes) {
        Some(bytes) => bytes,
        None => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::other(
                "manifest plus publication-path byte count overflow",
            ));
        }
    };
    let (target_parent, target_parent_live_bytes) = match open_manifest_target_parent_guarded(
        target.as_path(),
        path_live_bytes,
        work_budget,
        stage,
    ) {
        Ok(parent) => parent,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let target_component = match target.as_path().file_name() {
        Some(component) => component,
        None => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "manifest publication target has no final component",
            ));
        }
    };
    let target_name_base_bytes = match path_live_bytes.checked_add(target_parent_live_bytes) {
        Some(bytes) => bytes,
        None => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::other(
                "manifest target parent aggregate byte count overflow",
            ));
        }
    };
    let target_name = match GuardedManifestComponent::new(
        target_component,
        "manifest publication target",
        target_name_base_bytes,
        work_budget,
        stage,
    ) {
        Ok(name) => name,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let target_name_c_str = match target_name.as_c_str() {
        Ok(name) => name,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let target_live_bytes = match target_name_base_bytes.checked_add(target_name.live_bytes) {
        Some(bytes) => bytes,
        None => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::other(
                "manifest target component aggregate byte count overflow",
            ));
        }
    };
    let entry_control_bytes = match u64::try_from(size_of::<OwnedDirectoryManifestEntry>()) {
        Ok(bytes) => bytes,
        Err(_) => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::other(
                "manifest child entry control bytes do not fit u64",
            ));
        }
    };
    let child_name_base_bytes = match target_live_bytes.checked_add(entry_control_bytes) {
        Some(bytes) => bytes,
        None => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::other(
                "manifest child open live byte count overflow",
            ));
        }
    };
    let child_name = match GuardedManifestComponent::new(
        &manifest.child_name,
        "manifest child",
        child_name_base_bytes,
        work_budget,
        stage,
    ) {
        Ok(name) => name,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let child_name_c_str = match child_name.as_c_str() {
        Ok(name) => name,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let source_live_bytes = match child_name_base_bytes.checked_add(child_name.live_bytes) {
        Some(bytes) => bytes,
        None => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::other(
                "manifest child component live byte count overflow",
            ));
        }
    };
    let source = match open_manifest_entry_at(
        &temp_root.file,
        child_name_c_str,
        PathBuf::new(),
        source_live_bytes,
        work_budget,
        stage,
    ) {
        Ok(source) if source.record.identity == root_entry.identity => source,
        Ok(_) => {
            return DirectoryStageOutcome::NotStaged(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "manifest root identity changed immediately before directory staging",
            ));
        }
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    if let Err(error) = temp_root
        .revalidate_namespace()
        .and_then(|()| {
            validate_namespace_matches_handle(
                &temp_root.file,
                child_name_c_str,
                &source.file,
                Path::new("manifest child"),
            )
            .map(|_| ())
        })
        .and_then(|()| target_parent.revalidate_path())
    {
        return DirectoryStageOutcome::NotStaged(error);
    }
    let permit = match work_budget.authorize_commit(stage) {
        Ok(permit) => permit,
        Err(error) => return DirectoryStageOutcome::NotStaged(anyhow_to_io_error(error)),
    };
    let result = rename_directory_with_permit(
        temp_root.file.as_raw_fd(),
        child_name_c_str.as_ptr(),
        target_parent.file.as_raw_fd(),
        target_name_c_str.as_ptr(),
        permit,
        rename,
    );
    if result != 0 {
        DirectoryStageOutcome::NotStaged(std::io::Error::last_os_error())
    } else {
        // Do not perform any fallible work or another deadline check after the
        // namespace mutation succeeds. Exact validation is a separate gate.
        DirectoryStageOutcome::Staged
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_directory_with_permit(
    source_parent: std::ffi::c_int,
    source_name: *const std::ffi::c_char,
    target_parent: std::ffi::c_int,
    target_name: *const std::ffi::c_char,
    _permit: OperatorWorkBudgetCommitPermit,
    rename: impl FnOnce(
        std::ffi::c_int,
        *const std::ffi::c_char,
        std::ffi::c_int,
        *const std::ffi::c_char,
    ) -> std::ffi::c_int,
) -> std::ffi::c_int {
    rename(source_parent, source_name, target_parent, target_name)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn anyhow_to_io_error(error: anyhow::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{error:#}"))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct PinnedParentDirectory {
    path_c_string: Vec<u8>,
    file: File,
    device: u64,
    inode: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl PinnedParentDirectory {
    fn open(child_path: &Path) -> std::io::Result<Self> {
        let parent = child_path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "create-only rename path has no parent",
            )
        })?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let path = fallible_owned_path(parent)?;
        let path_c_string = fallible_path_c_string(&path)?;
        drop(path);
        Self::open_owned_path(path_c_string)
    }

    fn open_owned_path(path_c_string: Vec<u8>) -> std::io::Result<Self> {
        let path_c_str = std::ffi::CStr::from_bytes_with_nul(&path_c_string).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "create-only rename parent path is not NUL-terminated",
            )
        })?;
        let path_identity = namespace_identity_at_fd(libc::AT_FDCWD, path_c_str)?;
        if path_identity.kind != libc::S_IFDIR {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "create-only rename parent is not a directory",
            ));
        }
        // SAFETY: the retained parent path is NUL-terminated; `O_NOFOLLOW`
        // rejects a symlink at the final component and a successful descriptor
        // is immediately transferred to `File`.
        let fd = unsafe {
            libc::open(
                path_c_str.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `open` returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let handle_metadata = file.metadata()?;
        let handle_identity = namespace_identity_for_handle(&handle_metadata);
        if !handle_metadata.file_type().is_dir() || path_identity != handle_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "create-only rename parent identity changed",
            ));
        }
        Ok(Self {
            path_c_string,
            file,
            device: handle_metadata.dev(),
            inode: handle_metadata.ino(),
        })
    }

    fn revalidate_path(&self) -> std::io::Result<()> {
        let path_c_str =
            std::ffi::CStr::from_bytes_with_nul(&self.path_c_string).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "retained create-only rename parent path lost its NUL terminator",
                )
            })?;
        let identity = namespace_identity_at_fd(libc::AT_FDCWD, path_c_str)?;
        if identity.kind != libc::S_IFDIR
            || identity.device != self.device
            || identity.inode != self.inode
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "create-only rename parent path identity changed",
            ));
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn fallible_owned_path(path: &Path) -> std::io::Result<PathBuf> {
    let path_bytes = path.as_os_str().as_bytes();
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(path_bytes.len())
        .map_err(|_| std::io::Error::other("reserve create-only rename parent path"))?;
    bytes.extend_from_slice(path_bytes);
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn fallible_path_c_string(path: &Path) -> std::io::Result<Vec<u8>> {
    let path_bytes = path.as_os_str().as_bytes();
    if path_bytes.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "create-only rename parent path contains an interior NUL",
        ));
    }
    let requested_bytes = path_bytes.len().checked_add(1).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "create-only rename parent path byte size overflow",
        )
    })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(requested_bytes)
        .map_err(|_| std::io::Error::other("reserve create-only rename parent C string"))?;
    bytes.extend_from_slice(path_bytes);
    bytes.push(0);
    Ok(bytes)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn path_component_c_string(path: &Path, role: &str) -> std::io::Result<std::ffi::CString> {
    let component = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("create-only rename {role} has no final path component"),
        )
    })?;
    std::ffi::CString::new(component.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("create-only rename {role} contains an interior NUL"),
        )
    })
}

fn atomic_write_inner<E, F>(
    path: &Path,
    bytes: &[u8],
    cooperative_write: Option<(&OperatorWorkBudgetGuard, OperatorWorkBudgetStage)>,
    authorize_commit: F,
) -> std::result::Result<(), AtomicWriteInnerError<E>>
where
    F: FnOnce() -> std::result::Result<Option<OperatorWorkBudgetCommitPermit>, E>,
{
    let temp = if let Some((work_budget, stage)) = cooperative_write {
        let (tmp_path, retained_path_bytes) = unique_temp_path_guarded(path, work_budget, stage)
            .map_err(|error| AtomicWriteInnerError::Io(anyhow_to_io_error(error)))?;
        OwnedTempFile::create_guarded(tmp_path, retained_path_bytes, work_budget, stage)
            .map_err(|error| AtomicWriteInnerError::Io(anyhow_to_io_error(error)))?
    } else {
        let tmp_path = unique_temp_path(path).map_err(AtomicWriteInnerError::Io)?;
        OwnedTempFile::create(&tmp_path).map_err(AtomicWriteInnerError::Io)?
    };
    let file = match temp.callback_file() {
        Ok(file) => file,
        Err(error) => {
            let _ = temp.retention_outcome();
            return Err(AtomicWriteInnerError::Io(error));
        }
    };
    let write_result = if let Some((work_budget, stage)) = cooperative_write {
        let mut writer = CooperativeDeadlineWriter::new(file, work_budget, stage);
        writer.write_all(bytes).and_then(|()| writer.flush())
    } else {
        let mut writer = file;
        writer.write_all(bytes).and_then(|()| writer.flush())
    };
    if let Err(write_err) = write_result {
        let _ = temp.retention_outcome();
        return Err(AtomicWriteInnerError::Io(write_err));
    }
    let commit_permit = match authorize_commit() {
        Ok(permit) => permit,
        Err(error) => {
            let _ = temp.retention_outcome();
            return Err(AtomicWriteInnerError::Authorize(error));
        }
    };
    match commit_atomic_rename(&temp, path, commit_permit) {
        RenameCommitOutcome::Committed => Ok(()),
        RenameCommitOutcome::NotCommitted(rename_error) => {
            let _ = temp.retention_outcome();
            Err(AtomicWriteInnerError::Io(rename_error))
        }
    }
}

pub(crate) fn unique_temp_path(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: path has no parent directory",
        )
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
    Ok(dir.join(tmp_name))
}

/// Recover the target component from the exact canonical name emitted by
/// [`unique_temp_path`]. Lifecycle cleanup uses this parser instead of a loose
/// `.tmp` suffix match so foreign or malformed entries never gain deletion
/// authority.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn unique_temp_target_component(name: &OsStr) -> Option<&OsStr> {
    let bytes = name.as_bytes();
    let without_suffix = bytes.strip_suffix(b".tmp")?;
    let token_separator = without_suffix.iter().rposition(|byte| *byte == b'.')?;
    let (target_and_process, token_with_separator) = without_suffix.split_at(token_separator);
    let token = token_with_separator.strip_prefix(b".")?;
    let process_separator = target_and_process.iter().rposition(|byte| *byte == b'.')?;
    let (target, process_with_separator) = target_and_process.split_at(process_separator);
    let process = process_with_separator.strip_prefix(b".")?;
    if target.is_empty()
        || !is_canonical_decimal_component(process, u32::MAX.into())
        || !is_canonical_decimal_component(token, u128::MAX)
    {
        return None;
    }
    Some(OsStr::from_bytes(target))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn is_canonical_decimal_component(bytes: &[u8], maximum: u128) -> bool {
    if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
        return false;
    }
    bytes
        .iter()
        .try_fold(0_u128, |value, byte| {
            let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
            value
                .checked_mul(10)?
                .checked_add(u128::from(digit))
                .filter(|value| *value <= maximum)
        })
        .is_some()
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn unique_temp_path_guarded(
    path: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(PathBuf, u64)> {
    let dir = path
        .parent()
        .context("guarded atomic-write path has no parent directory")?;
    let base_name_bytes = path
        .file_name()
        .and_then(|name| name.to_str())
        .map_or("artifact".len(), str::len);
    let name_upper_bound = base_name_bytes
        .checked_add(1 + 20 + 1 + 39 + 4)
        .context("guarded temp-name byte upper bound overflow")?;
    let separator_bytes = usize::from(!dir.as_os_str().as_encoded_bytes().is_empty());
    let path_upper_bound = dir
        .as_os_str()
        .as_encoded_bytes()
        .len()
        .checked_add(separator_bytes)
        .and_then(|bytes| bytes.checked_add(name_upper_bound))
        .context("guarded temp-path byte upper bound overflow")?;
    verify_manifest_allocation_request(
        name_upper_bound,
        "guarded atomic-write temp name",
        work_budget,
        stage,
    )?;
    verify_manifest_allocation_request(
        path_upper_bound,
        "guarded atomic-write temp path",
        work_budget,
        stage,
    )?;
    let prospective_bytes = size_of::<String>()
        .checked_add(name_upper_bound)
        .and_then(|bytes| bytes.checked_add(size_of::<PathBuf>()))
        .and_then(|bytes| bytes.checked_add(path_upper_bound))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("guarded temp path prospective byte count overflow")?;
    work_budget.verify_decoded_bytes(prospective_bytes, stage)?;
    let tmp_name = format!(
        "{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        unique_temp_token(),
    );
    let tmp_path = dir.join(&tmp_name);
    verify_manifest_allocation_request(
        tmp_name.capacity(),
        "guarded atomic-write actual temp name",
        work_budget,
        stage,
    )?;
    verify_manifest_allocation_request(
        tmp_path.capacity(),
        "guarded atomic-write actual temp path",
        work_budget,
        stage,
    )?;
    let peak_bytes = size_of::<String>()
        .checked_add(tmp_name.capacity())
        .and_then(|bytes| bytes.checked_add(size_of::<PathBuf>()))
        .and_then(|bytes| bytes.checked_add(tmp_path.capacity()))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("guarded temp path actual byte count overflow")?;
    work_budget.verify_decoded_bytes(peak_bytes, stage)?;
    let retained_bytes = size_of::<PathBuf>()
        .checked_add(tmp_path.capacity())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("guarded retained temp path bytes do not fit u64")?;
    drop(tmp_name);
    work_budget.check_deadline(stage)?;
    Ok((tmp_path, retained_bytes))
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn unique_temp_path_guarded(
    _path: &Path,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<(PathBuf, u64)> {
    anyhow::bail!("guarded named temp paths are unsupported on this platform")
}

fn commit_atomic_rename(
    temp: &OwnedTempFile,
    path: &Path,
    _permit: Option<OperatorWorkBudgetCommitPermit>,
) -> RenameCommitOutcome {
    rename_owned_replace(temp, path)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_owned_replace(temp: &OwnedTempFile, path: &Path) -> RenameCommitOutcome {
    rename_owned_replace_with(
        temp,
        path,
        |source_parent, source_name, target_parent, target_name| {
            // SAFETY: both components and directory descriptors remain live.
            unsafe { libc::renameat(source_parent, source_name, target_parent, target_name) }
        },
    )
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn rename_owned_replace(_temp: &OwnedTempFile, _path: &Path) -> RenameCommitOutcome {
    RenameCommitOutcome::NotCommitted(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic pinned-parent rename is unsupported on this platform",
    ))
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
    use std::{fs, io::Write, sync::Arc};
    #[cfg(target_os = "linux")]
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use super::{
        DirectoryStageOutcome, atomic_file_create_or_verify_guarded, atomic_write,
        capture_owned_directory_manifest_guarded, create_owned_temp_directory,
        guarded_publication_child_path, stage_directory_rename_create_only_guarded,
        validate_existing_directory_manifest_identical_guarded,
        validate_staged_directory_manifest_guarded,
    };
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    use super::{
        ManifestSha256Digest, OwnedDirectoryManifest, OwnedDirectoryManifestEntry,
        unique_temp_path, unique_temp_target_component,
        validate_existing_directory_manifest_identical_with_post_traversal_hook_guarded,
        validate_staged_directory_manifest_with_post_traversal_hook_guarded,
    };

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn independently_account_manifest_inventory(manifest: &OwnedDirectoryManifest) -> u64 {
        let child_name_capacity = manifest.child_name.capacity();
        let segment_controls = manifest
            .entries
            .segment_control_capacity()
            .checked_mul(std::mem::size_of::<Vec<OwnedDirectoryManifestEntry>>())
            .expect("segment-control allocation byte size");
        let entry_allocation = manifest
            .entries
            .entry_capacity()
            .expect("segmented entry capacity")
            .checked_mul(std::mem::size_of::<OwnedDirectoryManifestEntry>())
            .expect("entry allocation byte size");
        let path_allocations = manifest
            .entries
            .iter()
            .map(|entry| {
                let _: Option<&ManifestSha256Digest> = entry.sha256.as_ref();
                entry.relative_path.capacity()
            })
            .try_fold(0_usize, |total, bytes| total.checked_add(bytes))
            .expect("path allocation byte size");
        std::mem::size_of::<OwnedDirectoryManifest>()
            .checked_add(child_name_capacity)
            .and_then(|bytes| bytes.checked_add(segment_controls))
            .and_then(|bytes| bytes.checked_add(entry_allocation))
            .and_then(|bytes| bytes.checked_add(path_allocations))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .expect("manifest inventory byte size")
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn unique_temp_name_parser_accepts_only_the_generated_canonical_shape() {
        let target = std::path::Path::new("/tmp/report.with.periods.json");
        let temp = unique_temp_path(target).expect("derive unique temp path");

        assert_eq!(
            unique_temp_target_component(temp.file_name().expect("temp file name")),
            target.file_name()
        );
        for malformed in [
            "report.with.periods.json.tmp",
            "report.with.periods.json.01.2.tmp",
            "report.with.periods.json.1.02.tmp",
            "report.with.periods.json.-1.2.tmp",
            "report.with.periods.json.1.-2.tmp",
        ] {
            assert_eq!(
                unique_temp_target_component(std::ffi::OsStr::new(malformed)),
                None,
                "malformed temp name must not gain cleanup authority: {malformed}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[derive(Default)]
    struct ManualClock {
        seconds: AtomicU64,
    }

    #[cfg(target_os = "linux")]
    impl crate::operator_work_budget::OperatorWorkBudgetClock for ManualClock {
        fn now(&self) -> Duration {
            Duration::from_secs(self.seconds.load(Ordering::SeqCst))
        }
    }

    #[cfg(target_os = "linux")]
    struct TargetVisibilityClock {
        target: std::path::PathBuf,
    }

    #[cfg(target_os = "linux")]
    impl crate::operator_work_budget::OperatorWorkBudgetClock for TargetVisibilityClock {
        fn now(&self) -> Duration {
            let target_is_visible = match fs::symlink_metadata(&self.target) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("inspect commit-boundary target: {error}"),
            };
            Duration::from_secs(u64::from(target_is_visible))
        }
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn owned_temp_directory_creation_rejects_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp root");
        let real_source_parent = root.path().join("real-source");
        let source_alias = root.path().join("source-alias");
        fs::create_dir(&real_source_parent).expect("create real source parent");
        symlink(&real_source_parent, &source_alias).expect("create source-parent symlink");

        let error = create_owned_temp_directory(&source_alias.join("catalog.tmp"))
            .expect_err("symlinked source parent must fail closed");

        assert!(
            error.to_string().contains("not a directory"),
            "unexpected error: {error}"
        );
        assert!(!real_source_parent.join("catalog.tmp").exists());
    }

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

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_atomic_write_stops_between_explicit_byte_chunks() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("large-cache-entry");
        let clock = Arc::new(ManualClock::default());
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: 1,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            clock.clone(),
        )
        .expect("guard");

        let error = atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::Fetch,
            |file| -> anyhow::Result<()> {
                let mut writer = crate::operator_work_budget::CooperativeDeadlineWriter::new(
                    file,
                    &guard,
                    crate::operator_work_budget::OperatorWorkBudgetStage::Fetch,
                );
                writer.write_all(&[b'x'; 64])?;
                clock.seconds.store(1, Ordering::SeqCst);
                let error = writer
                    .write_all(&[b'y'; 64])
                    .expect_err("second explicit chunk must observe the expired deadline");
                let file = writer.into_inner();
                assert_eq!(
                    file.metadata().expect("stat anonymous temp").len(),
                    64,
                    "the expired second chunk must not reach the anonymous file"
                );
                Err(error.into())
            },
        )
        .expect_err("guarded atomic write must expire between explicit output chunks");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(
            !target.exists(),
            "expired write must not publish the cache entry"
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
    /// final content equals exactly one writer's full payload, every legacy
    /// last-writer-wins call succeeds, and no `.tmp` residue remains.
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

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_file_write_removes_temp_when_deadline_expires_after_callback() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        let clock = Arc::new(ManualClock::default());
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            clock.clone(),
        )
        .expect("guard");

        let error = atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |mut file| {
                file.write_all(b"partial canonical bytes")?;
                clock.seconds.store(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .expect_err("post-callback expiry must reject the artifact");

        assert!(error.to_string().contains("canonical_write"), "{error:#}");
        assert!(!target.exists(), "failed write must not publish the target");
        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .collect();
        assert!(residue.is_empty(), "failed write left residue: {residue:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_file_write_checks_deadline_before_temp_file_create() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        let clock = Arc::new(ManualClock::default());
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            clock.clone(),
        )
        .expect("guard");
        clock.seconds.store(1, Ordering::SeqCst);

        let error = atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |_file| -> anyhow::Result<()> { panic!("expired guard must prevent file creation") },
        )
        .expect_err("pre-write deadline must reject the artifact");

        assert!(error.to_string().contains("canonical_write"), "{error:#}");
        assert!(
            std::fs::read_dir(dir.path())
                .expect("list dir")
                .next()
                .is_none(),
            "pre-write expiry must leave no temp or target"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn named_temp_retention_never_unlinks_a_foreign_replacement() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let temp_path = dir.path().join("owned.tmp");
        let foreign_bytes = b"foreign cleanup sentinel";
        let temp = super::OwnedTempFile::create(&temp_path).expect("create owned temp");

        fs::remove_file(&temp_path).expect("replace owned pathname");
        fs::write(&temp_path, foreign_bytes).expect("write foreign replacement");
        let outcome = temp.retention_outcome().expect("observe foreign entry");

        assert_eq!(
            outcome,
            super::OwnedTempRetentionOutcome::ForeignEntryRetained
        );
        assert_eq!(
            fs::read(&temp_path).expect("read foreign replacement"),
            foreign_bytes
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_staging_rejects_source_change_before_stage() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::write(temp_root.path().join("data/expected.parquet"), b"expected")
            .expect("write expected file");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture exact manifest");
        fs::write(temp_root.path().join("data/stray.parquet"), b"late stray")
            .expect("plant pre-stage stray");
        let final_root = root.path().join("final");
        let target = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect("guard target path");
        fs::create_dir(&final_root).expect("create final root");

        let error = match stage_directory_rename_create_only_guarded(
            &temp_root, &manifest, &target, &guard, stage,
        ) {
            DirectoryStageOutcome::NotStaged(error) => error,
            DirectoryStageOutcome::Staged => {
                panic!("source change before staging must fail closed")
            }
        };

        assert!(
            error.to_string().contains("manifest root identity changed"),
            "unexpected error: {error:#}"
        );
        assert!(
            !target.as_path().exists(),
            "changed source must not cross the create-only staging boundary"
        );
        assert!(
            temp_root.path().join("data/stray.parquet").is_file(),
            "rejected source evidence must remain retained"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_staging_requires_exact_validation_before_authority() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root_path = root.path().join("catalog.tmp");
        let temp_root = create_owned_temp_directory(&temp_root_path).expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::write(temp_root.path().join("data/expected.parquet"), b"expected")
            .expect("write expected file");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture exact manifest");
        let final_root = root.path().join("final");
        let target = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect("guard target path");
        fs::create_dir(&final_root).expect("create final root");
        let outcome = stage_directory_rename_create_only_guarded(
            &temp_root, &manifest, &target, &guard, stage,
        );

        assert!(matches!(outcome, DirectoryStageOutcome::Staged));
        fs::write(target.as_path().join("stray.parquet"), b"late stray")
            .expect("plant post-stage stray");
        let error = validate_staged_directory_manifest_guarded(&manifest, &target, &guard, stage)
            .expect_err("late exact-set change must deny reader authority");
        assert!(
            error.to_string().contains("exact-set entry count changed"),
            "unexpected error: {error:#}"
        );
        assert!(target.as_path().join("stray.parquet").is_file());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_staging_validates_before_reader_authority() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::write(temp_root.path().join("data/object.parquet"), b"expected")
            .expect("write expected file");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture exact manifest");
        let final_root = root.path().join("final");
        let target = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect("guard target path");
        fs::create_dir(&final_root).expect("create final root");
        let outcome = stage_directory_rename_create_only_guarded(
            &temp_root, &manifest, &target, &guard, stage,
        );

        assert!(matches!(outcome, DirectoryStageOutcome::Staged));
        validate_staged_directory_manifest_guarded(&manifest, &target, &guard, stage)
            .expect("exact validation grants reader authority");
        assert_eq!(
            fs::read(target.as_path().join("object.parquet")).expect("read committed object"),
            b"expected"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn existing_directory_with_distinct_inodes_accepts_identical_exact_content() {
        let root = tempfile::TempDir::new().expect("temp root");
        let candidate = create_owned_temp_directory(&root.path().join("candidate.tmp"))
            .expect("create candidate root");
        fs::create_dir(candidate.path().join("data")).expect("create candidate data root");
        fs::write(candidate.path().join("data/object.parquet"), b"identical")
            .expect("write candidate object");
        let existing = root.path().join("existing");
        fs::create_dir(&existing).expect("create existing root");
        fs::write(existing.join("object.parquet"), b"identical").expect("write existing object");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&candidate, "data", &guard, stage)
            .expect("capture candidate manifest");
        let target = guarded_publication_child_path(
            root.path(),
            std::ffi::OsStr::new("existing"),
            &guard,
            stage,
        )
        .expect("guard existing target");

        validate_existing_directory_manifest_identical_guarded(&manifest, &target, &guard, stage)
            .expect("identical content must reconcile despite distinct inode identities");

        fs::write(existing.join("object.parquet"), b"conflicting").expect("replace existing bytes");
        let error = validate_existing_directory_manifest_identical_guarded(
            &manifest, &target, &guard, stage,
        )
        .expect_err("different existing content must fail closed");
        assert!(error.to_string().contains("content changed"), "{error:#}");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn staged_manifest_validation_rejects_root_replacement_after_traversal() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::write(temp_root.path().join("data/object.parquet"), b"identical")
            .expect("write candidate object");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture candidate manifest");
        let final_root = root.path().join("final");
        fs::create_dir(&final_root).expect("create final root");
        let target = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect("guard target path");
        let outcome = stage_directory_rename_create_only_guarded(
            &temp_root, &manifest, &target, &guard, stage,
        );
        assert!(matches!(outcome, DirectoryStageOutcome::Staged));
        let displaced = final_root.join("displaced-data");

        let error = validate_staged_directory_manifest_with_post_traversal_hook_guarded(
            &manifest,
            &target,
            &guard,
            stage,
            |relative_path| {
                if relative_path.as_os_str().is_empty() {
                    fs::rename(target.as_path(), &displaced)?;
                    fs::create_dir(target.as_path())?;
                    fs::write(target.as_path().join("object.parquet"), b"identical")?;
                }
                Ok(())
            },
        )
        .expect_err("root replacement must deny staged reader authority");

        assert!(
            error
                .to_string()
                .contains("manifest directory namespace changed during capture"),
            "unexpected error: {error:#}"
        );
        assert!(displaced.is_dir(), "captured root must remain displaced");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn existing_manifest_validation_rejects_root_replacement_after_traversal() {
        let root = tempfile::TempDir::new().expect("temp root");
        let candidate = create_owned_temp_directory(&root.path().join("candidate.tmp"))
            .expect("create candidate root");
        fs::create_dir(candidate.path().join("data")).expect("create candidate data root");
        fs::write(candidate.path().join("data/object.parquet"), b"identical")
            .expect("write candidate object");
        let existing = root.path().join("existing");
        fs::create_dir(&existing).expect("create existing root");
        fs::write(existing.join("object.parquet"), b"identical").expect("write existing object");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&candidate, "data", &guard, stage)
            .expect("capture candidate manifest");
        let target = guarded_publication_child_path(
            root.path(),
            std::ffi::OsStr::new("existing"),
            &guard,
            stage,
        )
        .expect("guard existing target");
        let displaced = root.path().join("displaced-existing");

        let error =
            validate_existing_directory_manifest_identical_with_post_traversal_hook_guarded(
                &manifest,
                &target,
                &guard,
                stage,
                |relative_path| {
                    if relative_path.as_os_str().is_empty() {
                        fs::rename(target.as_path(), &displaced)?;
                        fs::create_dir(target.as_path())?;
                        fs::write(target.as_path().join("object.parquet"), b"identical")?;
                    }
                    Ok(())
                },
            )
            .expect_err("root replacement must deny identical-content reconciliation");

        assert!(
            error
                .to_string()
                .contains("manifest directory namespace changed during capture"),
            "unexpected error: {error:#}"
        );
        assert!(displaced.is_dir(), "captured root must remain displaced");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn staged_manifest_validation_rejects_nested_rename_after_traversal() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir_all(temp_root.path().join("data/nested"))
            .expect("create nested candidate root");
        fs::write(
            temp_root.path().join("data/nested/object.parquet"),
            b"identical",
        )
        .expect("write candidate object");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture candidate manifest");
        let final_root = root.path().join("final");
        fs::create_dir(&final_root).expect("create final root");
        let target = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect("guard target path");
        let outcome = stage_directory_rename_create_only_guarded(
            &temp_root, &manifest, &target, &guard, stage,
        );
        assert!(matches!(outcome, DirectoryStageOutcome::Staged));
        let displaced = final_root.join("displaced-nested");

        let error = validate_staged_directory_manifest_with_post_traversal_hook_guarded(
            &manifest,
            &target,
            &guard,
            stage,
            |relative_path| {
                if relative_path == std::path::Path::new("nested") {
                    fs::rename(target.as_path().join("nested"), &displaced)?;
                }
                Ok(())
            },
        )
        .expect_err("nested rename must deny staged reader authority");

        assert!(
            error
                .to_string()
                .contains("manifest directory namespace changed during capture"),
            "unexpected error: {error:#}"
        );
        assert!(displaced.is_dir(), "captured nested root must be displaced");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn existing_manifest_validation_rejects_nested_rename_after_traversal() {
        let root = tempfile::TempDir::new().expect("temp root");
        let candidate = create_owned_temp_directory(&root.path().join("candidate.tmp"))
            .expect("create candidate root");
        fs::create_dir_all(candidate.path().join("data/nested"))
            .expect("create nested candidate root");
        fs::write(
            candidate.path().join("data/nested/object.parquet"),
            b"identical",
        )
        .expect("write candidate object");
        let existing = root.path().join("existing");
        fs::create_dir_all(existing.join("nested")).expect("create nested existing root");
        fs::write(existing.join("nested/object.parquet"), b"identical")
            .expect("write existing object");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&candidate, "data", &guard, stage)
            .expect("capture candidate manifest");
        let target = guarded_publication_child_path(
            root.path(),
            std::ffi::OsStr::new("existing"),
            &guard,
            stage,
        )
        .expect("guard existing target");
        let displaced = root.path().join("displaced-nested");

        let error =
            validate_existing_directory_manifest_identical_with_post_traversal_hook_guarded(
                &manifest,
                &target,
                &guard,
                stage,
                |relative_path| {
                    if relative_path == std::path::Path::new("nested") {
                        fs::rename(target.as_path().join("nested"), &displaced)?;
                    }
                    Ok(())
                },
            )
            .expect_err("nested rename must deny identical-content reconciliation");

        assert!(
            error
                .to_string()
                .contains("manifest directory namespace changed during capture"),
            "unexpected error: {error:#}"
        );
        assert!(displaced.is_dir(), "captured nested root must be displaced");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn directory_staging_rejects_a_target_inserted_at_the_syscall_boundary() {
        const RENAME_NOREPLACE: std::ffi::c_uint = 1;
        unsafe extern "C" {
            fn renameat2(
                olddirfd: std::ffi::c_int,
                oldpath: *const std::ffi::c_char,
                newdirfd: std::ffi::c_int,
                newpath: *const std::ffi::c_char,
                flags: std::ffi::c_uint,
            ) -> std::ffi::c_int;
        }

        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture exact manifest");
        let final_root = root.path().join("final");
        let target = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect("guard target path");
        fs::create_dir(&final_root).expect("create final root");

        let outcome = super::rename_owned_directory_with(
            &temp_root,
            &manifest,
            &target,
            &guard,
            stage,
            |source_parent, source_name, target_parent, target_name| {
                // SAFETY: the target descriptor/name are live. This directory
                // is the deterministic competing insert immediately before the
                // no-replace rename syscall.
                assert_eq!(
                    unsafe { libc::mkdirat(target_parent, target_name, 0o700) },
                    0
                );
                // SAFETY: all descriptors and components are retained by the
                // staging helper for the syscall.
                unsafe {
                    renameat2(
                        source_parent,
                        source_name,
                        target_parent,
                        target_name,
                        RENAME_NOREPLACE,
                    )
                }
            },
        );

        match outcome {
            DirectoryStageOutcome::NotStaged(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists)
            }
            other => panic!("competing target must prevent staging: {other:?}"),
        }
        assert!(
            target.as_path().is_dir(),
            "competing target must be retained"
        );
        assert!(temp_root.path().join("data").is_dir(), "source must remain");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_staging_rejects_target_component_above_decoded_byte_limit() {
        let root = tempfile::TempDir::new().expect("temp root");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let final_root = root.path().join("final");
        fs::create_dir(&final_root).expect("create final root");
        let long_component = "x".repeat(240);
        let decoded_byte_limit = u64::try_from(
            std::mem::size_of::<OwnedDirectoryManifestEntry>().max(
                final_root
                    .as_os_str()
                    .len()
                    .checked_add(1)
                    .expect("target parent C string byte size"),
            ),
        )
        .expect("decoded byte limit");
        assert!(
            decoded_byte_limit < u64::try_from(long_component.len()).expect("component length")
        );
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: decoded_byte_limit,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct target-component guard");
        let error = guarded_publication_child_path(
            &final_root,
            std::ffi::OsStr::new(&long_component),
            &guard,
            stage,
        )
        .expect_err("oversized target component must fail before staging");

        assert!(
            error.to_string().contains("max_decoded_bytes"),
            "unexpected error: {error:#}"
        );
        assert!(!final_root.join(long_component).exists());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_staging_rejects_target_parent_above_decoded_byte_limit() {
        let root = tempfile::TempDir::new().expect("temp root");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let decoded_byte_limit = u64::try_from(std::mem::size_of::<OwnedDirectoryManifestEntry>())
            .expect("entry decoded byte limit");
        let mut target_parent = root.path().join("final");
        while target_parent.as_os_str().len()
            <= usize::try_from(decoded_byte_limit).expect("decoded byte limit")
        {
            target_parent = target_parent.join("p".repeat(48));
        }
        fs::create_dir_all(&target_parent).expect("create long target parent");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: decoded_byte_limit,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct target-parent guard");
        let error = guarded_publication_child_path(
            &target_parent,
            std::ffi::OsStr::new("data"),
            &guard,
            stage,
        )
        .expect_err("oversized target parent must fail before staging");

        assert!(
            error.to_string().contains("max_decoded_bytes"),
            "unexpected error: {error:#}"
        );
        assert!(!target_parent.join("data").exists());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_manifest_inventory_accounts_exact_vec_capacities() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::create_dir(temp_root.path().join("data/long-directory-name"))
            .expect("create nested manifest directory");
        fs::write(
            temp_root
                .path()
                .join("data/long-directory-name/empty.parquet"),
            b"",
        )
        .expect("write empty manifest object");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(
            &temp_root,
            "data",
            &crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded(),
            stage,
        )
        .expect("capture exact manifest");

        assert_eq!(
            manifest.inventory_bytes,
            independently_account_manifest_inventory(&manifest),
            "manifest inventory must use actual Vec and path capacities rather than lengths"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_manifest_comparison_rejects_a_changed_inline_digest() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::write(
            temp_root.path().join("data/object.parquet"),
            b"manifest bytes",
        )
        .expect("write manifest object");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture expected manifest");
        let mut current =
            capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
                .expect("capture current manifest");
        let digest = current
            .entries
            .iter_mut()
            .find_map(|entry| entry.sha256.as_mut())
            .expect("file entry digest");
        digest[0] ^= u8::MAX;

        let error = manifest
            .ensure_matches(&current, false, &guard, stage)
            .expect_err("a changed inline digest must fail exact-set comparison");
        assert!(
            error.to_string().contains("manifest changed"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn directory_manifest_comparison_accounts_for_both_segment_cursors() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        fs::write(temp_root.path().join("data/empty.parquet"), b"")
            .expect("write empty manifest object");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let manifest = capture_owned_directory_manifest_guarded(
            &temp_root,
            "data",
            &crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded(),
            stage,
        )
        .expect("capture exact manifest");
        let current = capture_owned_directory_manifest_guarded(
            &temp_root,
            "data",
            &crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded(),
            stage,
        )
        .expect("recapture exact manifest");
        assert_eq!(
            current.inventory_bytes,
            independently_account_manifest_inventory(&current)
        );
        let manifests_live_bytes = manifest
            .inventory_bytes
            .checked_add(current.inventory_bytes)
            .expect("manifest aggregate live byte size");
        let unbounded = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let expected_cursor = super::SegmentedManifestCursor::new_guarded(
            &manifest.entries,
            manifests_live_bytes,
            &unbounded,
            stage,
        )
        .expect("allocate expected cursor");
        let current_cursor_base = manifests_live_bytes
            .checked_add(expected_cursor.allocation_bytes)
            .expect("expected cursor aggregate bytes");
        let current_cursor = super::SegmentedManifestCursor::new_guarded(
            &current.entries,
            current_cursor_base,
            &unbounded,
            stage,
        )
        .expect("allocate current cursor");
        let aggregate_live_bytes = current_cursor_base
            .checked_add(current_cursor.allocation_bytes)
            .expect("both cursors aggregate live byte size");
        drop(expected_cursor);
        drop(current_cursor);
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: aggregate_live_bytes - 1,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct aggregate-memory guard");

        let error = manifest
            .ensure_matches(&current, false, &guard, stage)
            .expect_err("aggregate comparison memory above the ceiling must fail closed");

        assert!(
            error.to_string().contains("max_decoded_bytes"),
            "unexpected error: {error:#}"
        );

        let exact_guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: aggregate_live_bytes,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct exact aggregate-memory guard");
        manifest
            .ensure_matches(&current, false, &exact_guard, stage)
            .expect("the independently calculated exact ceiling must admit comparison");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn deep_long_directory_tree_fails_closed_below_exact_inventory_ceiling() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        let data_root = temp_root.path().join("data");
        fs::create_dir(&data_root).expect("create data root");
        let mut parent = data_root;
        for depth in 0..12_u8 {
            let component = format!("{depth:02}-{}", "x".repeat(48));
            parent = parent.join(component);
            fs::create_dir(&parent).expect("create deep manifest directory");
        }
        fs::write(parent.join("empty.parquet"), b"").expect("write deep manifest object");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;
        let unbounded = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let manifest =
            capture_owned_directory_manifest_guarded(&temp_root, "data", &unbounded, stage)
                .expect("capture deep manifest");
        let exact_inventory = independently_account_manifest_inventory(&manifest);
        assert_eq!(manifest.inventory_bytes, exact_inventory);
        drop(manifest);
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: exact_inventory - 1,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct deep-tree guard");

        let error = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect_err("deep manifest above the exact ceiling must fail closed");
        assert!(
            error.to_string().contains("max_decoded_bytes"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn long_manifest_component_fails_closed_before_exceeding_decoded_byte_limit() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        fs::create_dir(temp_root.path().join("data")).expect("create data root");
        let long_component = "x".repeat(240);
        fs::write(temp_root.path().join("data").join(&long_component), b"")
            .expect("write long-name manifest object");
        let decoded_byte_limit = u64::try_from(std::mem::size_of::<OwnedDirectoryManifestEntry>())
            .expect("entry decoded byte limit");
        assert!(
            decoded_byte_limit < u64::try_from(long_component.len()).expect("component length"),
            "fixture must isolate the adversarial path/name allocation"
        );
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: decoded_byte_limit,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct decoded-byte-limit guard");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;

        let error = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect_err("long component above max_decoded_bytes must fail closed");
        assert!(
            error.to_string().contains("max_decoded_bytes"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_file_write_commit_boundary_does_not_retract_published_target() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            Arc::new(TargetVisibilityClock {
                target: target.clone(),
            }),
        )
        .expect("guard");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite;
        atomic_file_create_or_verify_guarded(&target, &guard, stage, |mut file| {
            file.write_all(b"complete but expired canonical bytes")?;
            Ok(())
        })
        .expect("the authorized create-only commit must not be retracted");

        assert!(
            guard.check_deadline(stage).is_err(),
            "the semantic clock must be expired after publication"
        );

        assert_eq!(
            std::fs::read(&target).expect("read published artifact"),
            b"complete but expired canonical bytes"
        );
        assert!(
            std::fs::read_dir(dir.path())
                .expect("list dir")
                .filter_map(|entry| entry.ok())
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "authorized commit must leave no temp residue"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_create_or_verify_never_replaces_different_preexisting_target() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        std::fs::write(&target, b"previous canonical bytes").expect("seed target");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();

        let error = atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |mut file| {
                file.write_all(b"replacement canonical bytes")?;
                Ok(())
            },
        )
        .expect_err("create conflict with different bytes must reject");

        assert!(
            format!("{error:#}").contains("different bytes"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(&target).expect("read preserved target"),
            b"previous canonical bytes"
        );
        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            residue.is_empty(),
            "rejected writer left residue: {residue:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_create_or_verify_accepts_identical_existing_target() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();

        atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |mut file| {
                file.write_all(b"stable canonical bytes")?;
                Ok(())
            },
        )
        .expect("first writer creates the immutable target");
        atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |mut file| {
                file.write_all(b"stable canonical bytes")?;
                Ok(())
            },
        )
        .expect("identical create conflict is an idempotent success");

        assert_eq!(
            std::fs::read(&target).expect("read immutable target"),
            b"stable canonical bytes"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_create_or_verify_rejects_conflicting_existing_target() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        std::fs::write(&target, b"foreign canonical bytes").expect("seed foreign target");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();

        let error = atomic_file_create_or_verify_guarded(
            &target,
            &guard,
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |mut file| {
                file.write_all(b"expected canonical bytes")?;
                Ok(())
            },
        )
        .expect_err("different existing bytes must fail closed");

        assert!(error.to_string().contains("different bytes"), "{error:#}");
        assert_eq!(
            std::fs::read(&target).expect("read preserved foreign target"),
            b"foreign canonical bytes"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_create_or_verify_allows_concurrent_identical_writers() {
        use std::sync::Barrier;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = Arc::new(dir.path().join("canonical.parquet"));
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let target = Arc::clone(&target);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    atomic_file_create_or_verify_guarded(
                        target.as_path(),
                        &crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded(),
                        crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
                        |mut file| {
                            file.write_all(b"one deterministic artifact")?;
                            barrier.wait();
                            Ok(())
                        },
                    )
                })
            })
            .collect();

        for handle in handles {
            handle
                .join()
                .expect("writer thread must not panic")
                .expect("all identical writers must reconcile");
        }
        assert_eq!(
            std::fs::read(target.as_path()).expect("read reconciled artifact"),
            b"one deterministic artifact"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_file_write_concurrent_publish_has_exactly_one_winner() {
        use std::sync::Barrier;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = Arc::new(dir.path().join("canonical.parquet"));
        let barrier = Arc::new(Barrier::new(8));
        let payloads: Vec<Vec<u8>> = (0..8_u8).map(|index| vec![b'A' + index; 8_192]).collect();
        let valid: std::collections::HashSet<Vec<u8>> = payloads.iter().cloned().collect();
        let handles: Vec<_> = payloads
            .into_iter()
            .map(|payload| {
                let target = Arc::clone(&target);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    atomic_file_create_or_verify_guarded(
                        target.as_path(),
                        &crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded(),
                        crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
                        |mut file| {
                            file.write_all(&payload)?;
                            barrier.wait();
                            Ok(())
                        },
                    )
                })
            })
            .collect();

        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("writer thread must not panic"))
            .collect();
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "create-only publication must have one winner: {outcomes:?}"
        );
        let on_disk = std::fs::read(target.as_path()).expect("read winner");
        assert!(
            valid.contains(&on_disk),
            "published bytes must equal one complete payload"
        );
        assert!(
            std::fs::read_dir(dir.path())
                .expect("list dir")
                .filter_map(|entry| entry.ok())
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "concurrent publication must leave no temp residue"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn failed_named_temp_retention_never_unlinks_the_owned_entry() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("owned.tmp");
        let temp = super::OwnedTempFile::create(&path).expect("create owned temp");

        let outcome = temp.retention_outcome().expect("observe retained temp");

        assert_eq!(
            outcome,
            super::OwnedTempRetentionOutcome::OwnedEntryRetained
        );
        assert!(
            path.is_file(),
            "failure handling must retain even the still-owned pathname"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn legacy_successful_replace_is_not_reclassified_by_a_later_target_swap() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let temp_path = dir.path().join("legacy.tmp");
        let target = dir.path().join("artifact.json");
        let foreign_bytes = b"later valid writer";
        let temp = super::OwnedTempFile::create(&temp_path).expect("create legacy temp");
        temp.callback_file()
            .expect("clone legacy temp")
            .write_all(b"legacy writer")
            .expect("write legacy temp");

        let outcome = super::rename_owned_replace_with(
            &temp,
            &target,
            |source_parent, source_name, target_parent, target_name| {
                // SAFETY: all descriptors and components are retained by the
                // test. The second writer deliberately runs after rename.
                let result = unsafe {
                    libc::renameat(source_parent, source_name, target_parent, target_name)
                };
                assert_eq!(result, 0, "first legacy rename must succeed");
                // SAFETY: target_name remains live and refers to a test-only
                // directory. Every successful fd is closed below.
                assert_eq!(unsafe { libc::unlinkat(target_parent, target_name, 0) }, 0);
                let fd = unsafe {
                    libc::openat(
                        target_parent,
                        target_name,
                        libc::O_WRONLY | libc::O_CLOEXEC | libc::O_CREAT | libc::O_EXCL,
                        0o600,
                    )
                };
                assert!(fd >= 0, "create later writer target");
                // SAFETY: fd is live and foreign_bytes is valid for its length.
                assert_eq!(
                    unsafe { libc::write(fd, foreign_bytes.as_ptr().cast(), foreign_bytes.len(),) },
                    isize::try_from(foreign_bytes.len()).expect("foreign byte length")
                );
                // SAFETY: fd is owned by this closure.
                assert_eq!(unsafe { libc::close(fd) }, 0);
                result
            },
        );

        assert!(matches!(outcome, super::RenameCommitOutcome::Committed));
        assert_eq!(fs::read(target).expect("read later target"), foreign_bytes);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn anonymous_create_only_publish_syncs_inode_before_link_and_parent_after_link() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("durable-cache-entry");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::Fetch;
        let temp = super::OwnedAnonymousTempFile::create_guarded(&target, &guard, stage)
            .expect("create anonymous temp");
        temp.callback_file()
            .expect("clone anonymous temp")
            .write_all(b"durable bytes")
            .expect("write anonymous temp");
        let observed = std::sync::Mutex::new(Vec::new());

        temp.publish_with(
            &guard,
            stage,
            || {
                observed.lock().expect("lock order").push("inode");
                Ok(())
            },
            |source_fd, target_parent, target_name| {
                observed.lock().expect("lock order").push("link");
                let empty =
                    std::ffi::CStr::from_bytes_with_nul(b"\0").expect("static empty component");
                // SAFETY: the anonymous source fd and pinned target descriptor/name
                // remain live for this test-only create-only publication.
                unsafe {
                    libc::linkat(
                        source_fd,
                        empty.as_ptr(),
                        target_parent,
                        target_name,
                        libc::AT_EMPTY_PATH,
                    )
                }
            },
            || {
                observed.lock().expect("lock order").push("parent");
                Ok(())
            },
        )
        .expect("durable create-only publication");

        assert_eq!(
            *observed.lock().expect("lock order"),
            ["inode", "link", "parent"]
        );
        assert_eq!(fs::read(target).expect("read target"), b"durable bytes");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn anonymous_create_only_publish_rejects_a_target_inserted_at_the_syscall_boundary() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded();
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite;
        let temp = super::OwnedAnonymousTempFile::create_guarded(&target, &guard, stage)
            .expect("create anonymous temp");
        temp.callback_file()
            .expect("clone anonymous temp")
            .write_all(b"owned bytes")
            .expect("write anonymous temp");
        let foreign_bytes = b"boundary winner";

        let error = temp
            .publish_with(
                &guard,
                stage,
                || Ok(()),
                |source_fd, target_parent, target_name| {
                    // SAFETY: target parent/name are live. This insert is the
                    // deterministic competing writer immediately before linkat.
                    let fd = unsafe {
                        libc::openat(
                            target_parent,
                            target_name,
                            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_CREAT | libc::O_EXCL,
                            0o600,
                        )
                    };
                    assert!(fd >= 0, "insert competing target");
                    // SAFETY: fd and bytes are live for the write.
                    assert_eq!(
                        unsafe {
                            libc::write(fd, foreign_bytes.as_ptr().cast(), foreign_bytes.len())
                        },
                        isize::try_from(foreign_bytes.len()).expect("foreign byte length")
                    );
                    // SAFETY: fd is owned by this closure.
                    assert_eq!(unsafe { libc::close(fd) }, 0);
                    let empty =
                        std::ffi::CStr::from_bytes_with_nul(b"\0").expect("static empty component");
                    // SAFETY: the anonymous source fd and target descriptor/name
                    // remain live. The existing target must make linkat fail.
                    unsafe {
                        libc::linkat(
                            source_fd,
                            empty.as_ptr(),
                            target_parent,
                            target_name,
                            libc::AT_EMPTY_PATH,
                        )
                    }
                },
                || Ok(()),
            )
            .expect_err("competing target must win create-only publication");

        assert!(error.to_string().contains("create-only"), "{error:#}");
        assert_eq!(
            fs::read(target).expect("read winning target"),
            foreign_bytes
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guarded_create_only_file_never_exposes_a_named_temp_source() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("canonical.parquet");

        atomic_file_create_or_verify_guarded(
            &target,
            &crate::operator_work_budget::OperatorWorkBudgetGuard::unbounded(),
            crate::operator_work_budget::OperatorWorkBudgetStage::CanonicalWrite,
            |mut file| {
                assert!(
                    fs::read_dir(dir.path())?
                        .filter_map(|entry| entry.ok())
                        .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
                    "an O_TMPFILE-backed writer must not expose a swappable source pathname"
                );
                file.write_all(b"anonymous source bytes")?;
                Ok(())
            },
        )
        .expect("publish anonymous temp inode");

        assert_eq!(
            fs::read(target).expect("read published target"),
            b"anonymous source bytes"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn manifest_inventory_handles_large_entry_counts() {
        let root = tempfile::TempDir::new().expect("temp root");
        let temp_root = create_owned_temp_directory(&root.path().join("catalog.tmp"))
            .expect("create temp root");
        let data = temp_root.path().join("data");
        fs::create_dir(&data).expect("create data root");
        for index in 0..2_048_u16 {
            fs::write(data.join(format!("{index:04}.parquet")), b"")
                .expect("write empty manifest object");
        }
        let guard = crate::operator_work_budget::OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::MAX,
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct segmented manifest guard");
        let stage = crate::operator_work_budget::OperatorWorkBudgetStage::CatalogProjection;

        let manifest = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("capture a large manifest");
        let current = capture_owned_directory_manifest_guarded(&temp_root, "data", &guard, stage)
            .expect("recapture a large manifest");
        manifest
            .ensure_matches(&current, false, &guard, stage)
            .expect("compare a large manifest");
    }
}
