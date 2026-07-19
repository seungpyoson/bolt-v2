//! One fail-closed capability for local regular-file reads.
//!
//! The parent directory is canonicalized once (so a symlinked parent such as
//! `/tmp` remains usable), then pinned by descriptor. The final component is
//! never canonicalized or reopened by pathname: `fstatat` and `openat` operate
//! relative to the retained parent descriptor for the capability lifetime.

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::{
    fs::OpenOptions,
    path::{Component, PathBuf},
    sync::Arc,
};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
};

use anyhow::{Context, Result, ensure};

/// Detached, descriptor-free identity for comparing repeated pinned reads.
/// It retains no file or directory handle, so one proof per broad cache entry
/// cannot consume one live descriptor per object.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedRegularFileFingerprint {
    byte_len: u64,
    device: u64,
    inode: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    parent_device: u64,
    parent_inode: u64,
    name: Arc<std::ffi::CString>,
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedRegularFileFingerprint {
    byte_len: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct PinnedRegularFileParent {
    canonical_path: PathBuf,
    file: File,
    device: u64,
    inode: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl PinnedRegularFileParent {
    fn open(path: &Path) -> Result<Self> {
        let lexical_parent = path.parent().context("regular file path has no parent")?;
        let lexical_parent = if lexical_parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            lexical_parent
        };
        let canonical_path = fs::canonicalize(lexical_parent).with_context(|| {
            format!(
                "canonicalize pinned regular-file parent {}",
                lexical_parent.display()
            )
        })?;
        let path_metadata = fs::symlink_metadata(&canonical_path).with_context(|| {
            format!(
                "lstat canonical pinned regular-file parent {}",
                canonical_path.display()
            )
        })?;
        ensure!(
            path_metadata.file_type().is_dir(),
            "pinned regular-file parent is not a directory: {}",
            canonical_path.display()
        );
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&canonical_path)
            .with_context(|| {
                format!(
                    "open canonical pinned regular-file parent {}",
                    canonical_path.display()
                )
            })?;
        let handle_metadata = file.metadata().with_context(|| {
            format!(
                "fstat canonical pinned regular-file parent {}",
                canonical_path.display()
            )
        })?;
        ensure!(
            handle_metadata.file_type().is_dir()
                && handle_metadata.dev() == path_metadata.dev()
                && handle_metadata.ino() == path_metadata.ino(),
            "pinned regular-file parent identity changed while opening {}",
            canonical_path.display()
        );
        Ok(Self {
            canonical_path,
            file,
            device: handle_metadata.dev(),
            inode: handle_metadata.ino(),
        })
    }

    fn revalidate(&self) -> Result<()> {
        let handle_metadata = self.file.metadata().with_context(|| {
            format!(
                "fstat pinned regular-file parent {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            handle_metadata.file_type().is_dir()
                && handle_metadata.dev() == self.device
                && handle_metadata.ino() == self.inode,
            "pinned regular-file parent handle identity changed: {}",
            self.canonical_path.display()
        );
        let path_metadata = fs::symlink_metadata(&self.canonical_path).with_context(|| {
            format!(
                "re-lstat pinned regular-file parent {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            path_metadata.file_type().is_dir()
                && path_metadata.dev() == self.device
                && path_metadata.ino() == self.inode,
            "pinned regular-file parent namespace identity changed: {}",
            self.canonical_path.display()
        );
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NamespaceIdentity {
    device: u64,
    inode: u64,
    kind: libc::mode_t,
    byte_len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl NamespaceIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            kind: (metadata.mode() as libc::mode_t) & libc::S_IFMT,
            byte_len: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn namespace_identity_at(
    parent: &File,
    name: &std::ffi::CStr,
) -> std::io::Result<NamespaceIdentity> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated, `parent` is a live directory, and the
    // successful call initializes `stat` before it is read.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful `fstatat` initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "linux")]
    let (modified_seconds, modified_nanoseconds, changed_seconds, changed_nanoseconds) = (
        stat.st_mtime,
        stat.st_mtime_nsec,
        stat.st_ctime,
        stat.st_ctime_nsec,
    );
    #[cfg(target_vendor = "apple")]
    let (modified_seconds, modified_nanoseconds, changed_seconds, changed_nanoseconds) = (
        stat.st_mtimespec.tv_sec,
        stat.st_mtimespec.tv_nsec,
        stat.st_ctimespec.tv_sec,
        stat.st_ctimespec.tv_nsec,
    );
    let byte_len = u64::try_from(stat.st_size).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pinned regular-file namespace reports a negative byte length",
        )
    })?;
    #[cfg(target_os = "linux")]
    let device = stat.st_dev;
    #[cfg(target_vendor = "apple")]
    let device = u64::try_from(stat.st_dev).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pinned regular-file namespace reports a negative device identifier",
        )
    })?;
    Ok(NamespaceIdentity {
        device,
        inode: stat.st_ino,
        kind: stat.st_mode & libc::S_IFMT,
        byte_len,
        modified_seconds,
        modified_nanoseconds,
        changed_seconds,
        changed_nanoseconds,
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn final_component(path: &Path) -> Result<(&std::ffi::OsStr, std::ffi::CString)> {
    let component = path.file_name().with_context(|| {
        format!(
            "pinned regular file has no final component: {}",
            path.display()
        )
    })?;
    let mut components = Path::new(component).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
        "pinned regular-file final component must be exactly one normal component"
    );
    let c_component = std::ffi::CString::new(component.as_bytes())
        .context("pinned regular-file final component contains an interior NUL")?;
    Ok((component, c_component))
}

/// Immutable identity plus retained parent namespace capability for one opened
/// regular file. Clones share the same pinned parent descriptor.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone)]
pub(crate) struct PinnedRegularFileIdentity {
    pub(crate) byte_len: u64,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    parent: Arc<PinnedRegularFileParent>,
    name: Arc<std::ffi::CString>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl PartialEq for PinnedRegularFileIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.byte_len == other.byte_len
            && self.device == other.device
            && self.inode == other.inode
            && self.modified_seconds == other.modified_seconds
            && self.modified_nanoseconds == other.modified_nanoseconds
            && self.changed_seconds == other.changed_seconds
            && self.changed_nanoseconds == other.changed_nanoseconds
            && self.parent.device == other.parent.device
            && self.parent.inode == other.parent.inode
            && self.name.as_bytes() == other.name.as_bytes()
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl Eq for PinnedRegularFileIdentity {}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl PinnedRegularFileIdentity {
    fn from_metadata(
        metadata: &fs::Metadata,
        parent: Arc<PinnedRegularFileParent>,
        name: Arc<std::ffi::CString>,
    ) -> Self {
        Self {
            byte_len: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
            parent,
            name,
        }
    }

    fn namespace_identity(&self) -> NamespaceIdentity {
        NamespaceIdentity {
            device: self.device,
            inode: self.inode,
            kind: libc::S_IFREG,
            byte_len: self.byte_len,
            modified_seconds: self.modified_seconds,
            modified_nanoseconds: self.modified_nanoseconds,
            changed_seconds: self.changed_seconds,
            changed_nanoseconds: self.changed_nanoseconds,
        }
    }

    pub(crate) fn fingerprint(&self) -> PinnedRegularFileFingerprint {
        PinnedRegularFileFingerprint {
            byte_len: self.byte_len,
            device: self.device,
            inode: self.inode,
            modified_seconds: self.modified_seconds,
            modified_nanoseconds: self.modified_nanoseconds,
            changed_seconds: self.changed_seconds,
            changed_nanoseconds: self.changed_nanoseconds,
            parent_device: self.parent.device,
            parent_inode: self.parent.inode,
            name: Arc::clone(&self.name),
        }
    }

    /// Bind this file capability to one previously authorized parent
    /// directory identity before any bytes are consumed.
    pub(crate) fn revalidate_expected_parent(
        &self,
        expected_path: &Path,
        expected_metadata: &fs::Metadata,
    ) -> Result<()> {
        ensure!(
            expected_metadata.file_type().is_dir(),
            "authorized pinned-file parent is not a directory: {}",
            expected_path.display()
        );
        ensure!(
            expected_path.is_absolute()
                && expected_path == self.parent.canonical_path
                && expected_metadata.dev() == self.parent.device
                && expected_metadata.ino() == self.parent.inode,
            "pinned regular-file parent does not match authorized canonical directory identity: {}",
            expected_path.display()
        );
        self.parent.revalidate()
    }

    pub(crate) fn revalidate_path(&self, path: &Path) -> Result<()> {
        let (component, _) = final_component(path)?;
        ensure!(
            component.as_bytes() == self.name.as_bytes(),
            "pinned regular-file validation label has a different final component: {}",
            path.display()
        );
        self.parent.revalidate()?;
        let namespace =
            namespace_identity_at(&self.parent.file, &self.name).with_context(|| {
                format!(
                    "re-fstatat pinned regular-file namespace {}",
                    path.display()
                )
            })?;
        ensure!(
            namespace == self.namespace_identity(),
            "pinned regular-file namespace identity changed: {}",
            path.display()
        );
        Ok(())
    }

    pub(crate) fn revalidate_handle(&self, path: &Path, file: &File) -> Result<()> {
        let metadata = file
            .metadata()
            .with_context(|| format!("fstat pinned regular file {}", path.display()))?;
        ensure!(
            metadata.file_type().is_file()
                && metadata.len() == self.byte_len
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
                && metadata.mtime() == self.modified_seconds
                && metadata.mtime_nsec() == self.modified_nanoseconds
                && metadata.ctime() == self.changed_seconds
                && metadata.ctime_nsec() == self.changed_nanoseconds,
            "pinned regular-file handle identity changed: {}",
            path.display()
        );
        Ok(())
    }

    pub(crate) fn revalidate(&self, path: &Path, file: &File) -> Result<()> {
        self.revalidate_path(path)?;
        self.revalidate_handle(path, file)
    }
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedRegularFileIdentity {
    pub(crate) byte_len: u64,
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
impl PinnedRegularFileIdentity {
    pub(crate) fn fingerprint(&self) -> PinnedRegularFileFingerprint {
        PinnedRegularFileFingerprint {
            byte_len: self.byte_len,
        }
    }

    pub(crate) fn revalidate_expected_parent(
        &self,
        expected_path: &Path,
        _expected_metadata: &fs::Metadata,
    ) -> Result<()> {
        anyhow::bail!(
            "pinned regular-file parent capabilities are unsupported on this platform for {}",
            expected_path.display()
        )
    }

    pub(crate) fn revalidate_path(&self, path: &Path) -> Result<()> {
        anyhow::bail!(
            "pinned regular-file capabilities are unsupported on this platform for {}",
            path.display()
        )
    }

    pub(crate) fn revalidate_handle(&self, path: &Path, _file: &File) -> Result<()> {
        self.revalidate_path(path)
    }

    pub(crate) fn revalidate(&self, path: &Path, file: &File) -> Result<()> {
        self.revalidate_handle(path, file)
    }
}

/// Open one regular file as an fd-relative capability. The returned identity
/// owns the parent capability while the returned file is the exact opened inode.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn open_pinned_regular_file(path: &Path) -> Result<(File, PinnedRegularFileIdentity)> {
    let (_, name) = final_component(path)?;
    let parent = Arc::new(PinnedRegularFileParent::open(path)?);
    parent.revalidate()?;
    let before = namespace_identity_at(&parent.file, &name)
        .with_context(|| format!("pre-open fstatat pinned regular file {}", path.display()))?;
    ensure!(
        before.kind == libc::S_IFREG,
        "pinned regular-file final component is a symlink or special file: {}",
        path.display()
    );
    // SAFETY: `name` is one NUL-terminated component, `parent` is a retained
    // directory descriptor, and a successful fd is immediately owned by File.
    let fd = unsafe {
        libc::openat(
            parent.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("openat pinned regular file {}", path.display()));
    }
    // SAFETY: `openat` returned one new owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .with_context(|| format!("fstat pinned regular file {}", path.display()))?;
    let handle_identity = NamespaceIdentity::from_metadata(&metadata);
    ensure!(
        metadata.file_type().is_file() && handle_identity == before,
        "pinned regular-file namespace/handle identity changed while opening {}",
        path.display()
    );
    let after = namespace_identity_at(&parent.file, &name)
        .with_context(|| format!("post-open fstatat pinned regular file {}", path.display()))?;
    ensure!(
        before == after,
        "pinned regular-file namespace identity changed during open: {}",
        path.display()
    );
    let identity = PinnedRegularFileIdentity::from_metadata(&metadata, parent, Arc::new(name));
    identity.revalidate(path, &file)?;
    Ok((file, identity))
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn open_pinned_regular_file(path: &Path) -> Result<(File, PinnedRegularFileIdentity)> {
    anyhow::bail!(
        "pinned regular-file capabilities are unsupported on this platform for {}",
        path.display()
    )
}

/// Read exactly the pinned byte count from one already-opened regular-file
/// capability, with one trailing-byte sentinel and one fallible allocation.
pub(crate) fn read_exact_pinned_file(
    file: &mut File,
    path: &Path,
    expected_bytes: u64,
) -> Result<Vec<u8>> {
    let byte_count = usize::try_from(expected_bytes).with_context(|| {
        format!(
            "declared byte length {expected_bytes} for {} does not fit usize",
            path.display()
        )
    })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(byte_count).with_context(|| {
        format!(
            "reserve declared {expected_bytes} bytes for pinned artifact {}",
            path.display()
        )
    })?;
    bytes.resize(byte_count, 0);
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("rewind pinned artifact {}", path.display()))?;
    file.read_exact(&mut bytes).with_context(|| {
        format!(
            "read exactly {expected_bytes} bytes from {}",
            path.display()
        )
    })?;
    let mut trailing = [0_u8; 1];
    ensure!(
        file.read(&mut trailing)
            .with_context(|| format!("check trailing bytes for {}", path.display()))?
            == 0,
        "pinned artifact {} exceeds declared length {expected_bytes}",
        path.display()
    );
    Ok(bytes)
}

/// Structural helper retained for tests which independently acquire path and
/// handle metadata. Production openers use the stronger fd-relative capability.
#[cfg(test)]
pub(crate) fn validate_pinned_regular_file_identity(
    path: &Path,
    path_metadata: &fs::Metadata,
    handle_metadata: &fs::Metadata,
) -> Result<()> {
    ensure!(
        path_metadata.file_type().is_file(),
        "pinned regular file path {} must be a regular file, not a symlink",
        path.display()
    );
    ensure!(
        handle_metadata.file_type().is_file(),
        "pinned regular file handle {} must refer to a regular file",
        path.display()
    );
    ensure!(
        path_metadata.len() == handle_metadata.len(),
        "pinned regular file {} length changed between lstat and open: path has {} bytes, handle has {}",
        path.display(),
        path_metadata.len(),
        handle_metadata.len()
    );
    #[cfg(unix)]
    ensure!(
        {
            use std::os::unix::fs::MetadataExt;
            path_metadata.dev() == handle_metadata.dev()
                && path_metadata.ino() == handle_metadata.ino()
        },
        "pinned regular file {} device/inode identity changed between lstat and open",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read};

    use sha2::{Digest, Sha256};

    use super::open_pinned_regular_file;

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn final_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp root");
        let target = root.path().join("target");
        let link = root.path().join("link");
        fs::write(&target, b"target").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let error = open_pinned_regular_file(&link).expect_err("final symlink must fail closed");

        assert!(
            error.to_string().contains("symlink or special file"),
            "{error:#}"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn symlinked_parent_is_canonicalized_once_and_works() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp root");
        let real_parent = root.path().join("real");
        let alias_parent = root.path().join("alias");
        fs::create_dir(&real_parent).expect("create real parent");
        fs::write(real_parent.join("object"), b"payload").expect("write object");
        symlink(&real_parent, &alias_parent).expect("create parent symlink");
        let alias_path = alias_parent.join("object");

        let (mut file, identity) =
            open_pinned_regular_file(&alias_path).expect("open through canonicalized parent");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read pinned object");
        identity
            .revalidate(&alias_path, &file)
            .expect("revalidate pinned object");

        assert_eq!(bytes, b"payload");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn replacement_parent_does_not_satisfy_prior_authorization() {
        let root = tempfile::tempdir().expect("temp root");
        let authorized_parent = root.path().join("authorized");
        let displaced_parent = root.path().join("displaced");
        fs::create_dir(&authorized_parent).expect("create authorized parent");
        fs::write(authorized_parent.join("object"), b"authorized").expect("write object");
        let authorized_metadata =
            fs::symlink_metadata(&authorized_parent).expect("snapshot authorized parent");
        fs::rename(&authorized_parent, &displaced_parent).expect("displace authorized parent");
        fs::create_dir(&authorized_parent).expect("create replacement parent");
        let replacement_path = authorized_parent.join("object");
        fs::write(&replacement_path, b"replacement").expect("write replacement object");

        let (_file, identity) =
            open_pinned_regular_file(&replacement_path).expect("pin replacement object");
        let error = identity
            .revalidate_expected_parent(&authorized_parent, &authorized_metadata)
            .expect_err("replacement parent must not inherit prior authorization");

        assert!(
            error
                .to_string()
                .contains("authorized canonical directory identity")
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn same_length_inode_swap_after_open_is_rejected() {
        let root = tempfile::tempdir().expect("temp root");
        let selected = root.path().join("selected");
        let displaced = root.path().join("displaced");
        let replacement = root.path().join("replacement");
        fs::write(&selected, b"selected").expect("write selected");
        fs::write(&replacement, b"foreign!").expect("write replacement");
        let (mut file, identity) = open_pinned_regular_file(&selected).expect("pin selected");
        fs::rename(&selected, &displaced).expect("displace selected");
        fs::rename(&replacement, &selected).expect("replace selected");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read original inode");

        let error = identity
            .revalidate(&selected, &file)
            .expect_err("same-length inode replacement must fail");

        assert!(error.to_string().contains("namespace identity changed"));
        assert_eq!(bytes, b"selected");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn detached_fingerprint_matches_a_fresh_pinned_traversal() {
        let root = tempfile::tempdir().expect("temp root");
        let selected = root.path().join("selected");
        fs::write(&selected, b"selected").expect("write selected");
        let (first_file, first_identity) = open_pinned_regular_file(&selected).expect("first pin");
        let expected = first_identity.fingerprint();
        drop(first_file);
        drop(first_identity);

        let (_second_file, second_identity) =
            open_pinned_regular_file(&selected).expect("second pin");

        assert_eq!(second_identity.fingerprint(), expected);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn same_inode_mutation_during_hash_is_rejected_after_traversal() {
        let root = tempfile::tempdir().expect("temp root");
        let path = root.path().join("object");
        fs::write(&path, b"original").expect("write object");
        let (mut file, identity) = open_pinned_regular_file(&path).expect("pin object");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read for hash");
        let _hash = Sha256::digest(&bytes);
        fs::write(&path, b"mutated!").expect("mutate same inode");

        let error = identity
            .revalidate(&path, &file)
            .expect_err("mutation after hash traversal must fail");

        assert!(error.to_string().contains("identity changed"));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn same_inode_mutation_changes_the_namespace_snapshot() {
        let root = tempfile::tempdir().expect("temp root");
        let path = root.path().join("object");
        fs::write(&path, b"original").expect("write object");
        let (_file, identity) = open_pinned_regular_file(&path).expect("pin object");
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&path, b"mutated!").expect("mutate same inode");

        let error = identity
            .revalidate_path(&path)
            .expect_err("same-inode mutation must change the namespace snapshot");

        assert!(error.to_string().contains("namespace identity changed"));
    }
}
