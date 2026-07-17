//! Bounded local workspace ownership for source-universe batch execution.

use std::{
    ffi::{CString, OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Read,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::OpenOptionsExt},
};

use crate::{
    atomic_artifact_write::for_each_directory_component,
    hashing::is_lowercase_sha256_hex,
    path_resolution::{resolve_output_dir, resolve_planned_write_path},
};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use crate::{
    atomic_artifact_write::unique_temp_target_component,
    source_universe_batch_execution::SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE,
};

pub const SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE: &str = ".source-universe-candidate-receipt.json";
pub const SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES: &[u8] =
    b"{\"schema_version\":\"source-universe-candidate-receipt.v1\"}\n";
pub const SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE: &str =
    ".source-universe-record-attempt-receipt.json";
pub const SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES: &[u8] =
    b"{\"schema_version\":\"source-universe-record-attempt-receipt.v1\"}\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceUniverseLifecycleCleanupLimits {
    pub max_entries: u64,
    pub max_depth: u64,
}

impl SourceUniverseLifecycleCleanupLimits {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            self.max_entries > 0 && self.max_entries != u64::MAX,
            "local_storage.max_lifecycle_cleanup_entries must be positive and finite"
        );
        ensure!(
            self.max_depth > 0 && self.max_depth != u64::MAX,
            "local_storage.max_lifecycle_cleanup_depth must be positive and finite"
        );
        ensure!(
            self.max_depth <= self.max_entries,
            "local_storage.max_lifecycle_cleanup_depth cannot exceed max_lifecycle_cleanup_entries"
        );
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseLocalStoragePolicy {
    pub workspace_root: PathBuf,
    pub owner_lock_path: PathBuf,
    pub max_workspace_bytes: u64,
    pub max_cache_bytes: u64,
    pub minimum_free_space_reserve_bytes: u64,
    pub one_record_worst_case_bytes: u64,
    pub cache_retention_age_seconds: u64,
    pub candidate_retention_age_seconds: u64,
    pub max_lifecycle_cleanup_entries: u64,
    pub max_lifecycle_cleanup_depth: u64,
}

impl SourceUniverseLocalStoragePolicy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.workspace_root.as_os_str().is_empty(),
            "local_storage.workspace_root must not be empty"
        );
        ensure!(
            !self.owner_lock_path.as_os_str().is_empty(),
            "local_storage.owner_lock_path must not be empty"
        );
        for (field, value) in [
            ("max_workspace_bytes", self.max_workspace_bytes),
            ("max_cache_bytes", self.max_cache_bytes),
            (
                "minimum_free_space_reserve_bytes",
                self.minimum_free_space_reserve_bytes,
            ),
            (
                "one_record_worst_case_bytes",
                self.one_record_worst_case_bytes,
            ),
            (
                "cache_retention_age_seconds",
                self.cache_retention_age_seconds,
            ),
            (
                "candidate_retention_age_seconds",
                self.candidate_retention_age_seconds,
            ),
            (
                "max_lifecycle_cleanup_entries",
                self.max_lifecycle_cleanup_entries,
            ),
            (
                "max_lifecycle_cleanup_depth",
                self.max_lifecycle_cleanup_depth,
            ),
        ] {
            ensure!(value > 0, "local_storage.{field} must be positive");
            ensure!(value != u64::MAX, "local_storage.{field} must be finite");
        }
        ensure!(
            self.max_cache_bytes <= self.max_workspace_bytes,
            "local_storage.max_cache_bytes cannot exceed max_workspace_bytes"
        );
        ensure!(
            self.one_record_worst_case_bytes <= self.max_cache_bytes,
            "local_storage.one_record_worst_case_bytes cannot exceed max_cache_bytes"
        );
        self.lifecycle_cleanup_limits().validate()?;
        Ok(())
    }

    #[must_use]
    pub fn lifecycle_cleanup_limits(&self) -> SourceUniverseLifecycleCleanupLimits {
        SourceUniverseLifecycleCleanupLimits {
            max_entries: self.max_lifecycle_cleanup_entries,
            max_depth: self.max_lifecycle_cleanup_depth,
        }
    }
}

#[derive(Debug)]
pub struct SourceUniverseLocalStorageLease {
    _owner_lock: File,
    workspace_directory: File,
    workspace_root: PathBuf,
    output_root: PathBuf,
    cache_root: PathBuf,
}

impl SourceUniverseLocalStorageLease {
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    #[must_use]
    pub fn output_root(&self) -> &Path {
        &self.output_root
    }

    #[must_use]
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn duplicate_owner_lock_for_worker(&self) -> Result<File> {
        // Keep the parent descriptor close-on-exec. The child-only pre-exec
        // hook clears that flag on this duplicate, avoiding a process-wide
        // inheritable-descriptor window in the multithreaded supervisor.
        // SAFETY: fcntl duplicates the live descriptor into one newly owned fd.
        let fd = unsafe {
            libc::fcntl(
                self._owner_lock.as_raw_fd(),
                libc::F_DUPFD_CLOEXEC,
                libc::STDERR_FILENO + 1,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("duplicate workspace owner lock for process-isolated worker");
        }
        // SAFETY: successful F_DUPFD_CLOEXEC returned one new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn duplicate_owner_lock_for_worker(&self) -> Result<File> {
        bail!("workspace owner-lock inheritance is unsupported on this platform")
    }

    /// Recheck the configured admission reserve immediately before the sole
    /// selected record can fetch bytes. This is an admission-time reserve,
    /// not a write-time filesystem quota: the terminal observation below is
    /// the fail-closed evidence that the completed run stayed within policy.
    pub fn verify_pre_record_admission(
        &self,
        policy: &SourceUniverseLocalStoragePolicy,
        checked_required_record_bytes: u64,
    ) -> Result<()> {
        self.verify_pre_record_admission_with_probe(
            policy,
            checked_required_record_bytes,
            available_filesystem_bytes,
        )
    }

    pub(crate) fn verify_pre_record_admission_with_probe(
        &self,
        policy: &SourceUniverseLocalStoragePolicy,
        checked_required_record_bytes: u64,
        available_bytes: impl FnOnce(&Path) -> Result<u64>,
    ) -> Result<()> {
        policy.validate()?;
        ensure!(
            checked_required_record_bytes <= policy.one_record_worst_case_bytes,
            "checked selected-record requirement {checked_required_record_bytes} exceeds local_storage.one_record_worst_case_bytes {}",
            policy.one_record_worst_case_bytes
        );
        let observation = self.observe_bounded(policy, available_bytes)?;
        validate_admission_observation(policy, &observation)
    }

    /// Observe completed local state after attempt compaction and/or report
    /// publication. This proves bounded terminal state at the observation
    /// points; it deliberately does not claim a hard write-time disk quota.
    pub fn verify_observed_terminal_boundedness(
        &self,
        policy: &SourceUniverseLocalStoragePolicy,
    ) -> Result<()> {
        self.verify_observed_terminal_boundedness_with_probe(policy, available_filesystem_bytes)
    }

    pub(crate) fn verify_observed_terminal_boundedness_with_probe(
        &self,
        policy: &SourceUniverseLocalStoragePolicy,
        available_bytes: impl FnOnce(&Path) -> Result<u64>,
    ) -> Result<()> {
        policy.validate()?;
        let observation = self.observe_bounded(policy, available_bytes)?;
        validate_terminal_observation(policy, &observation)
    }

    fn observe_bounded(
        &self,
        policy: &SourceUniverseLocalStoragePolicy,
        available_bytes: impl FnOnce(&Path) -> Result<u64>,
    ) -> Result<SourceUniverseLocalStorageObservation> {
        self.revalidate_workspace_identity()?;
        let mut traversal =
            LocalStorageTraversalProgress::new(policy.lifecycle_cleanup_limits().validate()?);
        let workspace_bytes = scan_allocated_bytes(
            &self.workspace_directory,
            &self.workspace_root,
            0,
            &mut traversal,
        )?;
        let cache_bytes = if self.cache_root.exists() {
            reject_existing_symlink(&self.cache_root, "object-cache root")?;
            let cache = open_real_directory(&self.cache_root, "object-cache root")?;
            scan_allocated_bytes(&cache, &self.cache_root, 0, &mut traversal)?
        } else {
            0
        };
        let free_bytes = available_bytes(&self.workspace_root)?;
        self.revalidate_workspace_identity()?;
        Ok(SourceUniverseLocalStorageObservation {
            workspace_bytes,
            cache_bytes,
            free_bytes,
        })
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn revalidate_workspace_identity(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let namespace = fs::symlink_metadata(&self.workspace_root).with_context(|| {
            format!(
                "reinspect local-storage workspace root {}",
                self.workspace_root.display()
            )
        })?;
        let handle = self
            .workspace_directory
            .metadata()
            .context("restat local-storage workspace handle")?;
        ensure!(
            namespace.file_type().is_dir()
                && !namespace.file_type().is_symlink()
                && handle.file_type().is_dir()
                && namespace.dev() == handle.dev()
                && namespace.ino() == handle.ino(),
            "local-storage workspace identity changed during the batch run"
        );
        ensure!(
            self.workspace_root.canonicalize()? == self.workspace_root,
            "local-storage workspace canonical identity changed during the batch run"
        );
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    fn revalidate_workspace_identity(&self) -> Result<()> {
        bail!("source-universe local-storage identity checks are unsupported on this platform")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceUniverseLocalStorageObservation {
    workspace_bytes: u64,
    cache_bytes: u64,
    free_bytes: u64,
}

fn validate_admission_observation(
    policy: &SourceUniverseLocalStoragePolicy,
    observation: &SourceUniverseLocalStorageObservation,
) -> Result<()> {
    let admitted_workspace_bytes = observation
        .workspace_bytes
        .checked_add(policy.one_record_worst_case_bytes)
        .context("local-storage workspace admission byte total overflow")?;
    ensure!(
        admitted_workspace_bytes <= policy.max_workspace_bytes,
        "local-storage current allocated bytes {} plus one_record_worst_case_bytes {} exceed max_workspace_bytes {}",
        observation.workspace_bytes,
        policy.one_record_worst_case_bytes,
        policy.max_workspace_bytes
    );
    let admitted_cache_bytes = observation
        .cache_bytes
        .checked_add(policy.one_record_worst_case_bytes)
        .context("local-storage cache admission byte total overflow")?;
    ensure!(
        admitted_cache_bytes <= policy.max_cache_bytes,
        "local-storage current cache bytes {} plus one_record_worst_case_bytes {} exceed max_cache_bytes {}",
        observation.cache_bytes,
        policy.one_record_worst_case_bytes,
        policy.max_cache_bytes
    );
    let required_free_bytes = policy
        .minimum_free_space_reserve_bytes
        .checked_add(policy.one_record_worst_case_bytes)
        .context("local-storage free-space admission total overflow")?;
    ensure!(
        observation.free_bytes >= required_free_bytes,
        "local-storage available bytes {} cannot preserve minimum_free_space_reserve_bytes {} after one_record_worst_case_bytes {}",
        observation.free_bytes,
        policy.minimum_free_space_reserve_bytes,
        policy.one_record_worst_case_bytes
    );
    Ok(())
}

fn validate_terminal_observation(
    policy: &SourceUniverseLocalStoragePolicy,
    observation: &SourceUniverseLocalStorageObservation,
) -> Result<()> {
    ensure!(
        observation.workspace_bytes <= policy.max_workspace_bytes,
        "observed terminal local-storage workspace bytes {} exceed max_workspace_bytes {}",
        observation.workspace_bytes,
        policy.max_workspace_bytes
    );
    ensure!(
        observation.cache_bytes <= policy.max_cache_bytes,
        "observed terminal local-storage cache bytes {} exceed max_cache_bytes {}",
        observation.cache_bytes,
        policy.max_cache_bytes
    );
    ensure!(
        observation.free_bytes >= policy.minimum_free_space_reserve_bytes,
        "observed terminal local-storage available bytes {} are below minimum_free_space_reserve_bytes {}",
        observation.free_bytes,
        policy.minimum_free_space_reserve_bytes
    );
    Ok(())
}

pub fn acquire_source_universe_local_storage(
    policy: &SourceUniverseLocalStoragePolicy,
    base_dir: &Path,
    output_dir: &Path,
    cache_dir: &Path,
) -> Result<SourceUniverseLocalStorageLease> {
    let now_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch for local-storage lifecycle")?
        .as_secs();
    acquire_source_universe_local_storage_with_probe(
        policy,
        base_dir,
        output_dir,
        cache_dir,
        now_seconds,
        available_filesystem_bytes,
    )
}

pub(crate) fn acquire_source_universe_local_storage_with_probe(
    policy: &SourceUniverseLocalStoragePolicy,
    base_dir: &Path,
    output_dir: &Path,
    cache_dir: &Path,
    now_seconds: u64,
    available_bytes: impl FnOnce(&Path) -> Result<u64>,
) -> Result<SourceUniverseLocalStorageLease> {
    policy.validate()?;
    let declared_workspace = resolve_output_dir(base_dir, &policy.workspace_root);
    reject_existing_symlink(&declared_workspace, "local-storage workspace root")?;
    let planned_workspace = resolve_planned_write_path(&declared_workspace)
        .context("resolve canonical local-storage workspace root")?;
    fs::create_dir_all(&planned_workspace).with_context(|| {
        format!(
            "create local-storage workspace root {}",
            planned_workspace.display()
        )
    })?;
    let workspace_root = open_real_directory(&planned_workspace, "local-storage workspace root")?;
    let canonical_workspace = planned_workspace.canonicalize().with_context(|| {
        format!(
            "canonicalize local-storage workspace root {}",
            planned_workspace.display()
        )
    })?;
    ensure!(
        canonical_workspace == planned_workspace,
        "local-storage workspace canonical identity changed"
    );

    let declared_lock = resolve_output_dir(base_dir, &policy.owner_lock_path);
    let planned_lock = resolve_planned_write_path(&declared_lock)
        .context("resolve canonical local-storage owner lock")?;
    ensure!(
        planned_lock.parent() == Some(canonical_workspace.as_path()),
        "local_storage.owner_lock_path must be one direct child of workspace_root"
    );
    reject_existing_symlink(output_dir, "batch output root")?;
    reject_existing_symlink(cache_dir, "object-cache root")?;
    let output_root =
        resolve_planned_write_path(output_dir).context("resolve canonical batch output root")?;
    let cache_root =
        resolve_planned_write_path(cache_dir).context("resolve canonical object-cache root")?;
    for (role, path) in [
        ("output", output_root.as_path()),
        ("cache", cache_root.as_path()),
        ("candidate", output_root.as_path()),
    ] {
        ensure!(
            path != canonical_workspace && path.starts_with(&canonical_workspace),
            "local-storage {role} root {} must be contained below workspace_root {}",
            path.display(),
            canonical_workspace.display()
        );
    }
    ensure!(
        output_root != cache_root
            && !output_root.starts_with(&cache_root)
            && !cache_root.starts_with(&output_root),
        "local-storage output and cache roots must be disjoint"
    );
    ensure!(
        !planned_lock.starts_with(&output_root) && !planned_lock.starts_with(&cache_root),
        "local-storage owner lock must remain outside output and cache roots"
    );
    let owner_lock = acquire_owner_lock(&planned_lock)?;

    let mut traversal =
        LocalStorageTraversalProgress::new(policy.lifecycle_cleanup_limits().validate()?);
    if cache_root.exists() {
        let cache = open_real_directory(&cache_root, "object-cache root")?;
        sweep_stale_cache_entries(
            &cache,
            &cache_root,
            now_seconds,
            policy.cache_retention_age_seconds,
            &mut traversal,
        )?;
    }
    if output_root.exists() {
        let output = open_real_directory(&output_root, "candidate root")?;
        sweep_stale_output_artifacts(
            &output,
            &output_root,
            now_seconds,
            policy.candidate_retention_age_seconds,
            &mut traversal,
        )?;
    }

    let workspace_bytes =
        scan_allocated_bytes(&workspace_root, &canonical_workspace, 0, &mut traversal)?;
    let cache_bytes = if cache_root.exists() {
        let cache = open_real_directory(&cache_root, "object-cache root")?;
        scan_allocated_bytes(&cache, &cache_root, 0, &mut traversal)?
    } else {
        0
    };
    let free_bytes = available_bytes(&canonical_workspace)?;
    validate_admission_observation(
        policy,
        &SourceUniverseLocalStorageObservation {
            workspace_bytes,
            cache_bytes,
            free_bytes,
        },
    )?;

    Ok(SourceUniverseLocalStorageLease {
        _owner_lock: owner_lock,
        workspace_directory: workspace_root,
        workspace_root: canonical_workspace,
        output_root,
        cache_root,
    })
}

fn reject_existing_symlink(path: &Path, role: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            "{role} must not be a symlink"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {role} {}", path.display()));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn open_real_directory(path: &Path, role: &str) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("open {role} {} without following", path.display()))?;
    let namespace = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {role} namespace {}", path.display()))?;
    let handle = file
        .metadata()
        .with_context(|| format!("stat {role} handle {}", path.display()))?;
    use std::os::unix::fs::MetadataExt;
    ensure!(
        namespace.file_type().is_dir()
            && handle.file_type().is_dir()
            && namespace.dev() == handle.dev()
            && namespace.ino() == handle.ino(),
        "{role} {} identity changed or is not a real directory",
        path.display()
    );
    Ok(file)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn open_real_directory(_path: &Path, _role: &str) -> Result<File> {
    bail!("source-universe local-storage ownership is unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn acquire_owner_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("open local-storage owner lock {}", path.display()))?;
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().context("stat local-storage owner lock")?;
    let namespace = fs::symlink_metadata(path).with_context(|| {
        format!(
            "inspect local-storage owner-lock namespace {}",
            path.display()
        )
    })?;
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.file_type().is_file()
            && namespace.file_type().is_file()
            && namespace.dev() == metadata.dev()
            && namespace.ino() == metadata.ino()
            && metadata.uid() == effective_uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0,
        "local-storage owner lock must be one owner-private regular file"
    );
    // SAFETY: flock operates on the live descriptor and LOCK_NB never waits.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            bail!("local-storage workspace is already owned by another batch run");
        }
        return Err(error).context("acquire nonblocking local-storage owner lease");
    }
    let namespace_after = fs::symlink_metadata(path).with_context(|| {
        format!(
            "reinspect local-storage owner-lock namespace {}",
            path.display()
        )
    })?;
    ensure!(
        namespace_after.file_type().is_file()
            && namespace_after.dev() == metadata.dev()
            && namespace_after.ino() == metadata.ino(),
        "local-storage owner-lock namespace changed during acquisition"
    );
    Ok(file)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn acquire_owner_lock(_path: &Path) -> Result<File> {
    bail!("source-universe local-storage owner leases are unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn component_c_string(name: &OsStr, role: &str) -> Result<CString> {
    let mut components = Path::new(name).components();
    ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none(),
        "{role} must be one normal path component"
    );
    CString::new(name.as_bytes()).with_context(|| format!("{role} contains an interior NUL"))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn stat_entry(parent: &File, name: &OsStr, role: &str) -> Result<libc::stat> {
    let name = component_c_string(name, role)?;
    let mut stat = MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: parent, name, and output storage remain live for the call.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("stat {role}"));
    }
    // SAFETY: fstatat initialized the structure on success.
    Ok(unsafe { stat.assume_init() })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn stat_kind(stat: &libc::stat) -> libc::mode_t {
    stat.st_mode & libc::S_IFMT
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn allocated_bytes(stat: &libc::stat) -> Result<u64> {
    let blocks = u64::try_from(stat.st_blocks).context("allocated block count is negative")?;
    blocks
        .checked_mul(512)
        .context("allocated byte count overflow")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn open_entry(parent: &File, name: &OsStr, directory: bool, role: &str) -> Result<File> {
    let name = component_c_string(name, role)?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent and name remain live; the returned descriptor is adopted.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("open {role}"));
    }
    // SAFETY: openat returned one newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn ensure_handle_identity(stat: &libc::stat, file: &File, role: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file
        .metadata()
        .with_context(|| format!("stat opened {role}"))?;
    let expected_device = u64::try_from(stat.st_dev).context("entry device is negative")?;
    let expected_inode = u64::try_from(stat.st_ino).context("entry inode is negative")?;
    ensure!(
        metadata.dev() == expected_device
            && metadata.ino() == expected_inode
            && metadata.mode() & u32::from(libc::S_IFMT) == u32::from(stat_kind(stat)),
        "{role} identity changed while open"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn scan_allocated_bytes(
    directory: &File,
    display_path: &Path,
    depth: u64,
    progress: &mut LocalStorageTraversalProgress,
) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let metadata = directory
        .metadata()
        .with_context(|| format!("stat scanned directory {}", display_path.display()))?;
    let mut total = u64::try_from(metadata.blocks())
        .context("directory allocated block count is negative")?
        .checked_mul(512)
        .context("directory allocated byte count overflow")?;
    for_each_directory_component(directory, |name| {
        let entry_depth = depth
            .checked_add(1)
            .context("local-storage scan depth overflow")?;
        progress.enter(entry_depth)?;
        let stat = stat_entry(directory, name, "local-storage scan entry")?;
        total = total
            .checked_add(allocated_bytes(&stat)?)
            .context("local-storage allocated byte total overflow")?;
        match stat_kind(&stat) {
            libc::S_IFREG => Ok(()),
            libc::S_IFDIR => {
                let child = open_entry(directory, name, true, "local-storage scan directory")?;
                ensure_handle_identity(&stat, &child, "local-storage scan directory")?;
                let child_total =
                    scan_allocated_bytes(&child, &display_path.join(name), entry_depth, progress)?;
                let child_root_bytes = u64::try_from(child.metadata()?.blocks())?
                    .checked_mul(512)
                    .context("child directory allocated byte count overflow")?;
                total = total
                    .checked_add(child_total.checked_sub(child_root_bytes).context(
                        "child directory scan total is smaller than its root allocation",
                    )?)
                    .context("local-storage allocated byte total overflow")?;
                Ok(())
            }
            libc::S_IFLNK => bail!(
                "local-storage scan rejects symlink {}",
                display_path.join(name).display()
            ),
            _ => bail!(
                "local-storage scan rejects special entry {}",
                display_path.join(name).display()
            ),
        }
    })?;
    Ok(total)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn scan_allocated_bytes(
    _directory: &File,
    _display_path: &Path,
    _depth: u64,
    _progress: &mut LocalStorageTraversalProgress,
) -> Result<u64> {
    bail!("allocated-byte scanning is unsupported on this platform")
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn entry_age_seconds(stat: &libc::stat, now_seconds: u64) -> Option<u64> {
    u64::try_from(stat.st_mtime)
        .ok()
        .and_then(|modified| now_seconds.checked_sub(modified))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn unlink_regular_entry(
    parent: &File,
    name: &OsStr,
    expected: &libc::stat,
    role: &str,
) -> Result<()> {
    ensure!(expected.st_nlink == 1, "{role} must have exactly one link");
    let file = open_entry(parent, name, false, role)?;
    ensure_handle_identity(expected, &file, role)?;
    let current = stat_entry(parent, name, role)?;
    ensure!(
        current.st_dev == expected.st_dev && current.st_ino == expected.st_ino,
        "{role} identity changed before unlink"
    );
    let name = component_c_string(name, role)?;
    // SAFETY: parent/name remain live and the namespace was revalidated above.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("unlink {role}"));
    }
    Ok(())
}

struct LocalStorageTraversalProgress {
    entries: u64,
    limits: SourceUniverseLifecycleCleanupLimits,
}

impl LocalStorageTraversalProgress {
    fn new(limits: SourceUniverseLifecycleCleanupLimits) -> Self {
        Self { entries: 0, limits }
    }

    fn enter(&mut self, depth: u64) -> Result<()> {
        self.ensure_depth(depth)?;
        self.entries = self
            .entries
            .checked_add(1)
            .context("local-storage lifecycle cleanup entry count overflow")?;
        ensure!(
            self.entries <= self.limits.max_entries,
            "local-storage traversal entry count exceeds configured maximum {}",
            self.limits.max_entries
        );
        Ok(())
    }

    fn ensure_depth(&self, depth: u64) -> Result<()> {
        ensure!(
            depth <= self.limits.max_depth,
            "local-storage traversal depth {depth} exceeds configured maximum {}",
            self.limits.max_depth
        );
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct ValidatedCleanupEntry {
    name: OsString,
    identity: libc::stat,
    kind: ValidatedCleanupEntryKind,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
enum ValidatedCleanupEntryKind {
    RegularFile,
    Directory(Vec<ValidatedCleanupEntry>),
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn ensure_cleanup_entry_identity(
    actual: &libc::stat,
    expected: &libc::stat,
    display_path: &Path,
) -> Result<()> {
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    ensure!(
        actual.st_dev == expected.st_dev
            && actual.st_ino == expected.st_ino
            && stat_kind(actual) == stat_kind(expected)
            && actual.st_uid == expected.st_uid
            && actual.st_uid == effective_uid
            && actual.st_mode & 0o7777 == expected.st_mode & 0o7777,
        "candidate cleanup entry identity changed or is foreign-owned: {}",
        display_path.display()
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn validate_cleanup_tree_entry(
    parent: &File,
    name: &OsStr,
    display_path: &Path,
    depth: u64,
    progress: &mut LocalStorageTraversalProgress,
    scanned_identity: Option<&libc::stat>,
) -> Result<ValidatedCleanupEntry> {
    progress.ensure_depth(depth)?;
    let expected = stat_entry(parent, name, "candidate cleanup entry")?;
    if let Some(scanned_identity) = scanned_identity {
        ensure_cleanup_entry_identity(&expected, scanned_identity, display_path)?;
    }
    ensure_cleanup_entry_identity(&expected, &expected, display_path)?;
    let kind = match stat_kind(&expected) {
        libc::S_IFREG => {
            ensure!(
                expected.st_nlink == 1,
                "candidate cleanup file must have exactly one link: {}",
                display_path.display()
            );
            ValidatedCleanupEntryKind::RegularFile
        }
        libc::S_IFDIR => {
            let directory = open_entry(parent, name, true, "candidate cleanup directory")?;
            ensure_handle_identity(&expected, &directory, "candidate cleanup directory")?;
            let child_depth = depth
                .checked_add(1)
                .context("candidate cleanup traversal depth overflow")?;
            let mut children = Vec::<OsString>::new();
            for_each_directory_component(&directory, |child| {
                progress.enter(child_depth)?;
                children
                    .try_reserve(1)
                    .context("reserve bounded candidate cleanup child inventory")?;
                children.push(child.to_os_string());
                Ok(())
            })?;
            let mut validated_children = Vec::new();
            validated_children
                .try_reserve(children.len())
                .context("reserve bounded validated cleanup child manifest")?;
            for child in children {
                validated_children.push(validate_cleanup_tree_entry(
                    &directory,
                    &child,
                    &display_path.join(&child),
                    child_depth,
                    progress,
                    None,
                )?);
            }
            ValidatedCleanupEntryKind::Directory(validated_children)
        }
        libc::S_IFLNK => {
            bail!(
                "candidate cleanup refuses symlink {}",
                display_path.display()
            )
        }
        _ => {
            bail!(
                "candidate cleanup refuses special entry {}",
                display_path.display()
            )
        }
    };
    Ok(ValidatedCleanupEntry {
        name: name.to_os_string(),
        identity: expected,
        kind,
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn ensure_directory_matches_cleanup_manifest(
    directory: &File,
    children: &[ValidatedCleanupEntry],
    display_path: &Path,
) -> Result<()> {
    let mut observed_count = 0_usize;
    for_each_directory_component(directory, |name| {
        observed_count = observed_count
            .checked_add(1)
            .context("candidate cleanup directory child count overflow")?;
        ensure!(
            observed_count <= children.len() && children.iter().any(|child| child.name == name),
            "candidate cleanup directory structure changed after bounded preflight: {}",
            display_path.display()
        );
        Ok(())
    })?;
    ensure!(
        observed_count == children.len(),
        "candidate cleanup directory structure changed after bounded preflight: {}",
        display_path.display()
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn revalidate_cleanup_tree_entry(
    parent: &File,
    entry: &ValidatedCleanupEntry,
    display_path: &Path,
) -> Result<()> {
    let current = stat_entry(parent, &entry.name, "prevalidated candidate cleanup entry")?;
    ensure_cleanup_entry_identity(&current, &entry.identity, display_path)?;
    match &entry.kind {
        ValidatedCleanupEntryKind::RegularFile => ensure!(
            current.st_nlink == 1,
            "candidate cleanup file link count changed after bounded preflight: {}",
            display_path.display()
        ),
        ValidatedCleanupEntryKind::Directory(children) => {
            let directory = open_entry(
                parent,
                &entry.name,
                true,
                "prevalidated candidate cleanup directory",
            )?;
            ensure_handle_identity(
                &entry.identity,
                &directory,
                "prevalidated candidate cleanup directory",
            )?;
            ensure_directory_matches_cleanup_manifest(&directory, children, display_path)?;
            for child in children {
                revalidate_cleanup_tree_entry(&directory, child, &display_path.join(&child.name))?;
            }
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn remove_prevalidated_cleanup_entry(
    parent: &File,
    entry: &ValidatedCleanupEntry,
    display_path: &Path,
) -> Result<()> {
    let current = stat_entry(parent, &entry.name, "prevalidated candidate cleanup entry")?;
    ensure_cleanup_entry_identity(&current, &entry.identity, display_path)?;
    match &entry.kind {
        ValidatedCleanupEntryKind::RegularFile => unlink_regular_entry(
            parent,
            &entry.name,
            &entry.identity,
            "prevalidated candidate cleanup file",
        ),
        ValidatedCleanupEntryKind::Directory(children) => {
            let directory = open_entry(
                parent,
                &entry.name,
                true,
                "prevalidated candidate cleanup directory",
            )?;
            ensure_handle_identity(
                &entry.identity,
                &directory,
                "prevalidated candidate cleanup directory",
            )?;
            ensure_directory_matches_cleanup_manifest(&directory, children, display_path)?;
            for child in children {
                remove_prevalidated_cleanup_entry(
                    &directory,
                    child,
                    &display_path.join(&child.name),
                )?;
            }
            let current = stat_entry(parent, &entry.name, "candidate cleanup directory")?;
            ensure_cleanup_entry_identity(&current, &entry.identity, display_path)?;
            let name = component_c_string(&entry.name, "candidate cleanup directory")?;
            // SAFETY: the exact prevalidated directory was re-opened by
            // descriptor, every child was identity-checked, and AT_REMOVEDIR
            // still fails closed if the trusted workspace changed.
            if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0
            {
                return Err(std::io::Error::last_os_error()).with_context(|| {
                    format!("remove candidate directory {}", display_path.display())
                });
            }
            Ok(())
        }
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn sweep_stale_cache_entries(
    cache: &File,
    cache_path: &Path,
    now_seconds: u64,
    retention_seconds: u64,
    progress: &mut LocalStorageTraversalProgress,
) -> Result<()> {
    let mut stale = Vec::<(OsString, libc::stat)>::new();
    for_each_directory_component(cache, |name| {
        progress.enter(1)?;
        let Some(name_text) = name.to_str() else {
            return Ok(());
        };
        if !is_lowercase_sha256_hex(name_text) {
            return Ok(());
        }
        let stat = stat_entry(cache, name, "object-cache lifecycle entry")?;
        // SAFETY: geteuid has no preconditions.
        let effective_uid = unsafe { libc::geteuid() };
        if stat_kind(&stat) == libc::S_IFREG
            && stat.st_uid == effective_uid
            && stat.st_nlink == 1
            && entry_age_seconds(&stat, now_seconds).is_some_and(|age| age >= retention_seconds)
        {
            stale
                .try_reserve(1)
                .context("reserve bounded stale object-cache inventory")?;
            stale.push((name.to_os_string(), stat));
        }
        Ok(())
    })?;
    for (name, stat) in stale {
        unlink_regular_entry(cache, &name, &stat, "stale object-cache entry").with_context(
            || {
                format!(
                    "sweep stale object-cache entry {}",
                    cache_path.join(name).display()
                )
            },
        )?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
enum CandidateReceiptState {
    Absent,
    Complete(libc::stat),
    Partial(libc::stat),
    Ambiguous,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn read_candidate_receipt_state_at(
    directory: &File,
    receipt_name: &str,
    receipt_bytes: &[u8],
) -> Result<CandidateReceiptState> {
    let name = OsStr::new(receipt_name);
    let stat = match stat_entry(directory, name, "candidate receipt") {
        Ok(stat) => stat,
        Err(error)
            if error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(CandidateReceiptState::Absent);
        }
        Err(error) => return Err(error),
    };
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if stat_kind(&stat) != libc::S_IFREG
        || stat.st_uid != effective_uid
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
    {
        return Ok(CandidateReceiptState::Ambiguous);
    }
    let mut file = open_entry(directory, name, false, "candidate receipt")?;
    ensure_handle_identity(&stat, &file, "candidate receipt")?;
    let max_bytes = u64::try_from(receipt_bytes.len())?
        .checked_add(1)
        .context("candidate receipt sentinel overflow")?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes)
        .read_to_end(&mut bytes)
        .context("read candidate receipt")?;
    if bytes == receipt_bytes {
        return Ok(CandidateReceiptState::Complete(stat));
    }
    if bytes.len() < receipt_bytes.len() && receipt_bytes.starts_with(&bytes) {
        return Ok(CandidateReceiptState::Partial(stat));
    }
    Ok(CandidateReceiptState::Ambiguous)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
enum StaleOutputArtifact {
    Tree {
        name: OsString,
        identity: libc::stat,
        receipt_name: &'static str,
        receipt_identity: libc::stat,
        validated_tree: Option<ValidatedCleanupEntry>,
    },
    EmptyDirectory {
        name: OsString,
        identity: libc::stat,
    },
    PartialReceiptDirectory {
        name: OsString,
        identity: libc::stat,
        receipt_name: &'static str,
        receipt_identity: libc::stat,
    },
    RegularFile {
        name: OsString,
        identity: libc::stat,
    },
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn directory_contains_only_receipt(
    directory: &File,
    receipt_name: &str,
    progress: &mut LocalStorageTraversalProgress,
) -> Result<bool> {
    let mut only_receipt = true;
    let mut entry_count = 0_u64;
    for_each_directory_component(directory, |name| {
        progress.enter(2)?;
        entry_count = entry_count
            .checked_add(1)
            .context("partial-receipt child count overflow")?;
        if name != OsStr::new(receipt_name) {
            only_receipt = false;
        }
        Ok(())
    })?;
    Ok(only_receipt && entry_count == 1)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn directory_is_empty_bounded(
    directory: &File,
    progress: &mut LocalStorageTraversalProgress,
) -> Result<bool> {
    let mut empty = true;
    for_each_directory_component(directory, |_| {
        progress.enter(2)?;
        empty = false;
        Ok(())
    })?;
    Ok(empty)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn remove_empty_candidate_directory(
    output: &File,
    name: &OsStr,
    expected: &libc::stat,
) -> Result<()> {
    let directory = open_entry(output, name, true, "empty candidate cleanup directory")?;
    ensure_handle_identity(expected, &directory, "empty candidate cleanup directory")?;
    let current = stat_entry(output, name, "empty candidate cleanup directory")?;
    ensure!(
        current.st_dev == expected.st_dev && current.st_ino == expected.st_ino,
        "empty candidate cleanup directory identity changed before unlink"
    );
    let name = component_c_string(name, "empty candidate cleanup directory")?;
    // SAFETY: parent/name remain live and AT_REMOVEDIR succeeds only while the
    // exact revalidated candidate is still empty.
    if unsafe { libc::unlinkat(output.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("remove empty candidate cleanup directory");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn remove_partial_receipt_candidate_directory(
    output: &File,
    name: &OsStr,
    expected: &libc::stat,
    receipt_name: &str,
    receipt_identity: &libc::stat,
) -> Result<()> {
    let directory = open_entry(output, name, true, "partial-receipt cleanup directory")?;
    ensure_handle_identity(expected, &directory, "partial-receipt cleanup directory")?;
    unlink_regular_entry(
        &directory,
        OsStr::new(receipt_name),
        receipt_identity,
        "partial candidate receipt",
    )?;
    let current = stat_entry(output, name, "partial-receipt cleanup directory")?;
    ensure!(
        current.st_dev == expected.st_dev && current.st_ino == expected.st_ino,
        "partial-receipt cleanup directory identity changed before unlink"
    );
    let name = component_c_string(name, "partial-receipt cleanup directory")?;
    // SAFETY: the bounded inventory proved that the partial receipt was the
    // only child; AT_REMOVEDIR still fails closed if that state changed.
    if unsafe { libc::unlinkat(output.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("remove partial-receipt cleanup directory");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn remove_prevalidated_candidate_tree(
    output: &File,
    tree: &ValidatedCleanupEntry,
    output_path: &Path,
    receipt_name: &str,
    receipt_identity: &libc::stat,
) -> Result<()> {
    let display_path = output_path.join(&tree.name);
    revalidate_cleanup_tree_entry(output, tree, &display_path)?;
    let ValidatedCleanupEntryKind::Directory(children) = &tree.kind else {
        bail!(
            "prevalidated stale candidate root is not a directory: {}",
            display_path.display()
        );
    };
    let mut receipt_matches = children
        .iter()
        .filter(|child| child.name == OsStr::new(receipt_name));
    let receipt = receipt_matches
        .next()
        .context("prevalidated stale candidate tree lost its lifecycle receipt")?;
    ensure!(
        receipt_matches.next().is_none(),
        "prevalidated stale candidate tree has duplicate lifecycle receipts"
    );
    ensure!(
        matches!(&receipt.kind, ValidatedCleanupEntryKind::RegularFile),
        "prevalidated stale candidate lifecycle receipt is not a regular file"
    );
    ensure_cleanup_entry_identity(
        &receipt.identity,
        receipt_identity,
        &display_path.join(receipt_name),
    )?;

    let directory = open_entry(
        output,
        &tree.name,
        true,
        "prevalidated stale candidate root",
    )?;
    ensure_handle_identity(
        &tree.identity,
        &directory,
        "prevalidated stale candidate root",
    )?;
    ensure_directory_matches_cleanup_manifest(&directory, children, &display_path)?;
    for child in children {
        if child.name != OsStr::new(receipt_name) {
            remove_prevalidated_cleanup_entry(&directory, child, &display_path.join(&child.name))?;
        }
    }

    ensure_directory_matches_cleanup_manifest(
        &directory,
        std::slice::from_ref(receipt),
        &display_path,
    )?;
    let expected_receipt_bytes: &[u8] = match receipt_name {
        SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE => SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES,
        SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE => SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES,
        _ => bail!("unknown stale candidate lifecycle receipt {receipt_name}"),
    };
    let current_receipt =
        match read_candidate_receipt_state_at(&directory, receipt_name, expected_receipt_bytes)? {
            CandidateReceiptState::Complete(identity) => identity,
            _ => bail!(
                "stale candidate lifecycle receipt changed before final cleanup: {}",
                display_path.join(receipt_name).display()
            ),
        };
    ensure_cleanup_entry_identity(
        &current_receipt,
        receipt_identity,
        &display_path.join(receipt_name),
    )?;
    unlink_regular_entry(
        &directory,
        OsStr::new(receipt_name),
        receipt_identity,
        "stale candidate lifecycle receipt",
    )?;

    let current = stat_entry(output, &tree.name, "prevalidated stale candidate root")?;
    ensure_cleanup_entry_identity(&current, &tree.identity, &display_path)?;
    let name = component_c_string(&tree.name, "prevalidated stale candidate root")?;
    // SAFETY: only the receipt remained, its exact bytes and identity were
    // revalidated, and it was removed last. AT_REMOVEDIR remains a final
    // fail-closed empty-directory check.
    if unsafe { libc::unlinkat(output.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("remove prevalidated stale candidate root");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn sweep_stale_output_artifacts(
    output: &File,
    output_path: &Path,
    now_seconds: u64,
    retention_seconds: u64,
    progress: &mut LocalStorageTraversalProgress,
) -> Result<()> {
    let mut stale = Vec::<StaleOutputArtifact>::new();
    for_each_directory_component(output, |name| {
        progress.enter(1)?;
        let Some(target_name) = unique_temp_target_component(name) else {
            return Ok(());
        };
        let stat = stat_entry(output, name, "candidate lifecycle entry")?;
        // SAFETY: geteuid has no preconditions.
        let effective_uid = unsafe { libc::geteuid() };
        if stat_kind(&stat) == libc::S_IFREG {
            if target_name == OsStr::new(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE)
                && stat.st_uid == effective_uid
                && stat.st_nlink == 1
                && entry_age_seconds(&stat, now_seconds).is_some_and(|age| age >= retention_seconds)
            {
                stale
                    .try_reserve(1)
                    .context("reserve bounded stale report-temp inventory")?;
                stale.push(StaleOutputArtifact::RegularFile {
                    name: name.to_os_string(),
                    identity: stat,
                });
            }
            return Ok(());
        }
        if stat_kind(&stat) != libc::S_IFDIR {
            return Ok(());
        }
        let directory = open_entry(output, name, true, "candidate lifecycle directory")?;
        ensure_handle_identity(&stat, &directory, "candidate lifecycle directory")?;
        if stat.st_uid != effective_uid || stat.st_mode & 0o777 != 0o700 {
            return Ok(());
        }
        let candidate_receipt = read_candidate_receipt_state_at(
            &directory,
            SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE,
            SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES,
        )?;
        let record_receipt = read_candidate_receipt_state_at(
            &directory,
            SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE,
            SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES,
        )?;
        let artifact = match (candidate_receipt, record_receipt) {
            (CandidateReceiptState::Complete(receipt), CandidateReceiptState::Absent)
                if entry_age_seconds(&receipt, now_seconds)
                    .is_some_and(|age| age >= retention_seconds) =>
            {
                Some(StaleOutputArtifact::Tree {
                    name: name.to_os_string(),
                    identity: stat,
                    receipt_name: SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE,
                    receipt_identity: receipt,
                    validated_tree: None,
                })
            }
            (CandidateReceiptState::Absent, CandidateReceiptState::Complete(receipt))
                if entry_age_seconds(&receipt, now_seconds)
                    .is_some_and(|age| age >= retention_seconds) =>
            {
                Some(StaleOutputArtifact::Tree {
                    name: name.to_os_string(),
                    identity: stat,
                    receipt_name: SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE,
                    receipt_identity: receipt,
                    validated_tree: None,
                })
            }
            (CandidateReceiptState::Absent, CandidateReceiptState::Absent)
                if entry_age_seconds(&stat, now_seconds)
                    .is_some_and(|age| age >= retention_seconds)
                    && directory_is_empty_bounded(&directory, progress)? =>
            {
                Some(StaleOutputArtifact::EmptyDirectory {
                    name: name.to_os_string(),
                    identity: stat,
                })
            }
            (CandidateReceiptState::Partial(receipt), CandidateReceiptState::Absent)
                if entry_age_seconds(&receipt, now_seconds)
                    .is_some_and(|age| age >= retention_seconds)
                    && directory_contains_only_receipt(
                        &directory,
                        SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE,
                        progress,
                    )? =>
            {
                Some(StaleOutputArtifact::PartialReceiptDirectory {
                    name: name.to_os_string(),
                    identity: stat,
                    receipt_name: SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE,
                    receipt_identity: receipt,
                })
            }
            (CandidateReceiptState::Absent, CandidateReceiptState::Partial(receipt))
                if entry_age_seconds(&receipt, now_seconds)
                    .is_some_and(|age| age >= retention_seconds)
                    && directory_contains_only_receipt(
                        &directory,
                        SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE,
                        progress,
                    )? =>
            {
                Some(StaleOutputArtifact::PartialReceiptDirectory {
                    name: name.to_os_string(),
                    identity: stat,
                    receipt_name: SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE,
                    receipt_identity: receipt,
                })
            }
            _ => None,
        };
        if let Some(artifact) = artifact {
            stale
                .try_reserve(1)
                .context("reserve bounded stale output-artifact inventory")?;
            stale.push(artifact);
        }
        Ok(())
    })?;

    // Validate every selected tree, including the complete configured bounds,
    // before the first unlink. A late depth/type/link violation therefore
    // leaves every candidate and its cleanup authority byte-for-byte intact.
    for artifact in &mut stale {
        if let StaleOutputArtifact::Tree {
            name,
            identity,
            receipt_name,
            receipt_identity,
            validated_tree,
        } = artifact
        {
            let tree = validate_cleanup_tree_entry(
                output,
                name,
                &output_path.join(&*name),
                1,
                progress,
                Some(identity),
            )?;
            let ValidatedCleanupEntryKind::Directory(children) = &tree.kind else {
                bail!("stale candidate cleanup preflight root is not a directory");
            };
            let receipt = children
                .iter()
                .find(|child| child.name == OsStr::new(*receipt_name))
                .context("stale candidate cleanup preflight lost its lifecycle receipt")?;
            ensure!(
                matches!(&receipt.kind, ValidatedCleanupEntryKind::RegularFile),
                "stale candidate cleanup receipt changed type during preflight"
            );
            ensure_cleanup_entry_identity(
                &receipt.identity,
                receipt_identity,
                &output_path.join(&*name).join(*receipt_name),
            )?;
            *validated_tree = Some(tree);
        }
    }
    // Recheck all captured manifests before mutation as one set. The workspace
    // lease excludes cooperating writers; the deletion pass repeats local
    // identity checks to fail closed against unexpected namespace drift.
    for artifact in &stale {
        if let StaleOutputArtifact::Tree {
            name,
            validated_tree,
            ..
        } = artifact
        {
            let tree = validated_tree
                .as_ref()
                .context("stale candidate cleanup tree was not prevalidated")?;
            revalidate_cleanup_tree_entry(output, tree, &output_path.join(name))?;
        }
    }
    for artifact in stale {
        match artifact {
            StaleOutputArtifact::Tree {
                receipt_name,
                receipt_identity,
                validated_tree,
                ..
            } => remove_prevalidated_candidate_tree(
                output,
                &validated_tree.context("stale candidate cleanup tree was not prevalidated")?,
                output_path,
                receipt_name,
                &receipt_identity,
            )?,
            StaleOutputArtifact::EmptyDirectory { name, identity } => {
                remove_empty_candidate_directory(output, &name, &identity)?;
            }
            StaleOutputArtifact::PartialReceiptDirectory {
                name,
                identity,
                receipt_name,
                receipt_identity,
            } => remove_partial_receipt_candidate_directory(
                output,
                &name,
                &identity,
                receipt_name,
                &receipt_identity,
            )?,
            StaleOutputArtifact::RegularFile { name, identity } => {
                unlink_regular_entry(output, &name, &identity, "stale batch-report temp")?;
            }
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn available_filesystem_bytes(path: &Path) -> Result<u64> {
    let path = CString::new(path.as_os_str().as_bytes())
        .context("local-storage workspace path contains an interior NUL")?;
    let mut stat = MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: path and output storage remain live for the call.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("read local-storage filesystem free space");
    }
    // SAFETY: statvfs initialized the structure on success.
    let stat = unsafe { stat.assume_init() };
    u64::from(stat.f_bavail)
        .checked_mul(u64::from(stat.f_frsize))
        .context("local-storage available byte count overflow")
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn available_filesystem_bytes(_path: &Path) -> Result<u64> {
    bail!("local-storage free-space probing is unsupported on this platform")
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests {
    use std::{
        ffi::CString,
        fs,
        os::unix::ffi::OsStrExt,
        path::{Path, PathBuf},
    };

    use super::*;

    fn policy(workspace_root: &Path) -> SourceUniverseLocalStoragePolicy {
        SourceUniverseLocalStoragePolicy {
            workspace_root: workspace_root.to_path_buf(),
            owner_lock_path: workspace_root.join("owner.lock"),
            max_workspace_bytes: 1 << 30,
            max_cache_bytes: 1 << 29,
            minimum_free_space_reserve_bytes: 1 << 20,
            one_record_worst_case_bytes: 1 << 20,
            cache_retention_age_seconds: 100,
            candidate_retention_age_seconds: 50,
            max_lifecycle_cleanup_entries: 10_000,
            max_lifecycle_cleanup_depth: 64,
        }
    }

    fn roots(workspace_root: &Path) -> (PathBuf, PathBuf) {
        (workspace_root.join("output"), workspace_root.join("cache"))
    }

    fn abundant_free_space(_: &Path) -> Result<u64> {
        Ok(u64::MAX)
    }

    fn set_mtime_seconds(path: &Path, seconds: i64) {
        let path = CString::new(path.as_os_str().as_bytes()).expect("mtime path CString");
        let times = [
            libc::timespec {
                tv_sec: seconds,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: seconds,
                tv_nsec: 0,
            },
        ];
        // SAFETY: the path and two timespec values remain live for the call.
        assert_eq!(
            unsafe {
                libc::utimensat(
                    libc::AT_FDCWD,
                    path.as_ptr(),
                    times.as_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            },
            0
        );
    }

    fn write_candidate_receipt(path: &Path, mtime_seconds: i64) {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(path).expect("create candidate root");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("make candidate root private");
        let receipt = path.join(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE);
        fs::write(&receipt, SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES)
            .expect("write candidate receipt");
        set_mtime_seconds(&receipt, mtime_seconds);
    }

    fn write_record_attempt_receipt(path: &Path, mtime_seconds: i64) {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(path).expect("create record attempt root");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("make record attempt root private");
        let receipt = path.join(SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE);
        fs::write(&receipt, SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES)
            .expect("write record attempt receipt");
        set_mtime_seconds(&receipt, mtime_seconds);
    }

    fn write_partial_record_attempt_receipt(path: &Path, mtime_seconds: i64) {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(path).expect("create partial record attempt root");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("make partial record attempt root private");
        let receipt = path.join(SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE);
        let partial_len = SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES.len() / 2;
        fs::write(
            &receipt,
            &SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_BYTES[..partial_len],
        )
        .expect("write partial record attempt receipt");
        fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600))
            .expect("make partial record attempt receipt private");
        set_mtime_seconds(&receipt, mtime_seconds);
    }

    #[test]
    fn admission_rejects_workspace_quota_before_output_or_cache_creation() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("create workspace");
        fs::write(workspace.join("occupied"), vec![7_u8; 4096]).expect("allocate bytes");
        let (output, cache) = roots(&workspace);
        let mut policy = policy(&workspace);
        policy.max_workspace_bytes = policy.one_record_worst_case_bytes;
        policy.max_cache_bytes = policy.one_record_worst_case_bytes;
        let error = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("workspace quota must reject admission");
        assert!(
            error.to_string().contains("max_workspace_bytes"),
            "{error:#}"
        );
        assert!(!output.exists() && !cache.exists());
    }

    #[test]
    fn admission_rejects_low_free_space_through_probe_seam() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        let policy = policy(&workspace);
        let available =
            policy.minimum_free_space_reserve_bytes + policy.one_record_worst_case_bytes - 1;
        let error = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            move |_| Ok(available),
        )
        .expect_err("low free space must reject admission");
        assert!(
            error
                .to_string()
                .contains("minimum_free_space_reserve_bytes"),
            "{error:#}"
        );
        assert!(!output.exists() && !cache.exists());
    }

    #[test]
    fn admission_rejects_traversal_entry_budget_exhaustion() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("create workspace");
        fs::write(workspace.join("first"), b"first").expect("write first entry");
        fs::write(workspace.join("second"), b"second").expect("write second entry");
        let (output, cache) = roots(&workspace);
        let mut policy = policy(&workspace);
        policy.max_lifecycle_cleanup_entries = 1;
        policy.max_lifecycle_cleanup_depth = 1;

        let error = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("entry-bounded traversal must fail closed");

        assert!(
            error.to_string().contains("entry count exceeds"),
            "{error:#}"
        );
        assert!(!output.exists() && !cache.exists());
    }

    #[test]
    fn admission_rejects_traversal_depth_budget_exhaustion() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join("nested/deeper")).expect("create nested workspace");
        let (output, cache) = roots(&workspace);
        let mut policy = policy(&workspace);
        policy.max_lifecycle_cleanup_entries = 100;
        policy.max_lifecycle_cleanup_depth = 1;

        let error = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("depth-bounded traversal must fail closed");

        assert!(error.to_string().contains("depth 2 exceeds"), "{error:#}");
        assert!(!output.exists() && !cache.exists());
    }

    #[test]
    fn second_owner_is_rejected_nonblocking() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        let policy = policy(&workspace);
        let first = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("first owner");
        let error = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("second owner must fail without waiting");
        assert!(error.to_string().contains("already owned"), "{error:#}");
        drop(first);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn orphan_worker_retains_workspace_ownership_after_parent_lease_drop() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        let policy = policy(&workspace);
        let lease = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("acquire parent workspace owner");
        let inherited_owner_lock = lease
            .duplicate_owner_lock_for_worker()
            .expect("duplicate exact workspace owner lock for worker");
        let child_pid_path = temp.path().join("orphan-worker.pid");
        let mut command = std::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$OWNER_LOCK_CHILD_PID\"; exit 0")
            .env("OWNER_LOCK_CHILD_PID", &child_pid_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::source_universe_batch_execution::configure_worker_workspace_owner_lock_inheritance(
            &mut command,
            &inherited_owner_lock,
        )
        .expect("configure inherited workspace owner lock");
        let status = command.status().expect("spawn orphaning worker leader");
        assert!(status.success(), "worker leader must exit successfully");
        let orphan_pid: i32 = fs::read_to_string(&child_pid_path)
            .expect("read orphan worker pid")
            .trim()
            .parse()
            .expect("parse orphan worker pid");

        drop(inherited_owner_lock);
        drop(lease);
        let competing = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        );
        // SAFETY: the recorded pid belongs to the bounded test worker.
        assert_eq!(unsafe { libc::kill(orphan_pid, libc::SIGKILL) }, 0);
        let error = competing.expect_err("orphan worker must retain workspace ownership");
        assert!(error.to_string().contains("already owned"), "{error:#}");

        let mut recovered = None;
        for _ in 0..100 {
            match acquire_source_universe_local_storage_with_probe(
                &policy,
                temp.path(),
                &output,
                &cache,
                200,
                abundant_free_space,
            ) {
                Ok(lease) => {
                    recovered = Some(lease);
                    break;
                }
                Err(error) if error.to_string().contains("already owned") => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("unexpected reacquisition failure: {error:#}"),
            }
        }
        let recovered = recovered.expect("workspace lock releases after orphan worker exits");
        drop(recovered);
        let reacquired = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("normal lease drop releases workspace ownership");
        drop(reacquired);
    }

    #[test]
    fn sweep_removes_only_stale_cache_and_candidate_entries() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&output).expect("create output");
        let stale_cache = cache.join("a".repeat(64));
        let fresh_cache = cache.join("b".repeat(64));
        fs::write(&stale_cache, b"stale").expect("write stale cache");
        fs::write(&fresh_cache, b"fresh").expect("write fresh cache");
        set_mtime_seconds(&stale_cache, 50);
        set_mtime_seconds(&fresh_cache, 150);
        let stale_candidate = output.join("run.1.1.tmp");
        let fresh_candidate = output.join("run.1.2.tmp");
        write_candidate_receipt(&stale_candidate, 100);
        write_candidate_receipt(&fresh_candidate, 175);
        fs::create_dir(stale_candidate.join("data")).expect("create residue dir");
        fs::write(stale_candidate.join("data/residue"), b"residue").expect("write residue");
        let policy = policy(&workspace);
        let _lease = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("sweep and admit");
        assert!(!stale_cache.exists() && fresh_cache.is_file());
        assert!(!stale_candidate.exists() && fresh_candidate.is_dir());
    }

    #[test]
    fn lifecycle_sweeps_visit_each_entry_once_within_exact_budget() {
        const ENTRY_COUNT: u64 = 4;
        let temp = tempfile::tempdir().expect("temporary parent");
        let cache_path = temp.path().join("cache");
        fs::create_dir(&cache_path).expect("create cache");
        for index in 0..ENTRY_COUNT {
            let name = format!("{index:064x}");
            let path = cache_path.join(name);
            fs::write(&path, b"stale").expect("write stale cache entry");
            set_mtime_seconds(&path, 1);
        }
        let cache = open_real_directory(&cache_path, "test cache").expect("open test cache");
        let mut cache_progress =
            LocalStorageTraversalProgress::new(SourceUniverseLifecycleCleanupLimits {
                max_entries: ENTRY_COUNT,
                max_depth: 1,
            });

        sweep_stale_cache_entries(&cache, &cache_path, 200, 100, &mut cache_progress)
            .expect("one-pass cache sweep must fit the exact entry budget");

        assert_eq!(cache_progress.entries, ENTRY_COUNT);
        assert!(
            fs::read_dir(&cache_path)
                .expect("read swept cache")
                .next()
                .is_none()
        );

        let output_path = temp.path().join("output");
        fs::create_dir(&output_path).expect("create output");
        let candidate = output_path.join("run.1.1.tmp");
        write_record_attempt_receipt(&candidate, 1);
        for index in 0..ENTRY_COUNT {
            fs::write(candidate.join(format!("payload-{index}")), b"residue")
                .expect("write candidate residue");
        }
        let output = open_real_directory(&output_path, "test output").expect("open test output");
        let exact_candidate_entries = ENTRY_COUNT + 2;
        let mut candidate_progress =
            LocalStorageTraversalProgress::new(SourceUniverseLifecycleCleanupLimits {
                max_entries: exact_candidate_entries,
                max_depth: 2,
            });

        sweep_stale_output_artifacts(&output, &output_path, 200, 100, &mut candidate_progress)
            .expect("one-pass candidate sweep must fit the exact entry budget");

        assert_eq!(candidate_progress.entries, exact_candidate_entries);
        assert!(!candidate.exists());
    }

    #[test]
    fn sweep_removes_stale_candidate_at_record_output_sibling_level() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (batch_output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&batch_output).expect("create batch output");
        let record_output = batch_output.join("source-universe-operator-run-00000");
        fs::create_dir(&record_output).expect("create record output");
        let candidate = batch_output.join("source-universe-operator-run-00000.42.99.tmp");
        assert_eq!(
            candidate.parent(),
            record_output.parent(),
            "catalog candidate must model the external projector's record-output sibling"
        );
        write_record_attempt_receipt(&candidate, 100);
        fs::create_dir(candidate.join("data")).expect("create candidate residue");

        let _lease = acquire_source_universe_local_storage_with_probe(
            &policy(&workspace),
            temp.path(),
            &batch_output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("sweep direct-child candidate and admit");

        assert!(!candidate.exists());
        assert!(record_output.is_dir());
    }

    #[test]
    fn sweep_reclaims_only_stale_empty_pre_receipt_attempt_directories() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&output).expect("create output");
        let stale_attempt = output.join("source-universe-operator-run-00000.42.99.tmp");
        let fresh_attempt = output.join("source-universe-operator-run-00001.42.100.tmp");
        let ambiguous_attempt = output.join("source-universe-operator-run-00002.tmp");
        let nonempty_attempt = output.join("source-universe-operator-run-00003.42.101.tmp");
        for attempt in [
            &stale_attempt,
            &fresh_attempt,
            &ambiguous_attempt,
            &nonempty_attempt,
        ] {
            fs::create_dir(attempt).expect("create pre-receipt attempt");
            fs::set_permissions(attempt, fs::Permissions::from_mode(0o700))
                .expect("make pre-receipt attempt private");
        }
        set_mtime_seconds(&stale_attempt, 100);
        set_mtime_seconds(&fresh_attempt, 175);
        set_mtime_seconds(&ambiguous_attempt, 100);
        fs::write(nonempty_attempt.join("unexpected"), b"ambiguous residue")
            .expect("write ambiguous pre-receipt child");
        set_mtime_seconds(&nonempty_attempt, 100);

        let _lease = acquire_source_universe_local_storage_with_probe(
            &policy(&workspace),
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("sweep pre-receipt attempts and admit");

        assert!(!stale_attempt.exists());
        assert!(fresh_attempt.is_dir());
        assert!(ambiguous_attempt.is_dir());
        assert!(nonempty_attempt.join("unexpected").is_file());
    }

    #[test]
    fn sweep_reclaims_only_unambiguous_stale_partial_receipts() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&output).expect("create output");
        let stale_partial = output.join("source-universe-operator-run-00000.42.99.tmp");
        let fresh_partial = output.join("source-universe-operator-run-00001.42.100.tmp");
        let ambiguous_partial = output.join("source-universe-operator-run-00002.42.101.tmp");
        let malformed_partial = output.join("source-universe-operator-run-00003.42.102.tmp");
        write_partial_record_attempt_receipt(&stale_partial, 100);
        write_partial_record_attempt_receipt(&fresh_partial, 175);
        write_partial_record_attempt_receipt(&ambiguous_partial, 100);
        write_partial_record_attempt_receipt(&malformed_partial, 100);
        fs::write(
            ambiguous_partial.join("unexpected"),
            b"retain the whole attempt",
        )
        .expect("write ambiguous attempt child");
        let malformed_receipt = malformed_partial.join(SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE);
        fs::write(&malformed_receipt, b"not a receipt prefix").expect("write malformed receipt");
        fs::set_permissions(&malformed_receipt, fs::Permissions::from_mode(0o600))
            .expect("keep malformed receipt private");
        set_mtime_seconds(&malformed_receipt, 100);

        let _lease = acquire_source_universe_local_storage_with_probe(
            &policy(&workspace),
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("sweep partial receipts and admit");

        assert!(!stale_partial.exists());
        assert!(fresh_partial.is_dir());
        assert!(ambiguous_partial.is_dir());
        assert!(ambiguous_partial.join("unexpected").is_file());
        assert!(malformed_partial.is_dir());
        assert_eq!(
            fs::read(malformed_receipt).expect("read retained malformed receipt"),
            b"not a receipt prefix"
        );
    }

    #[test]
    fn sweep_reclaims_only_stale_batch_report_atomic_temp_files() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&output).expect("create output");
        let stale_report = output.join(format!(
            "{SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE}.42.99.tmp"
        ));
        let fresh_report = output.join(format!(
            "{SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE}.42.100.tmp"
        ));
        let unrelated = output.join("unrelated-report.json.42.101.tmp");
        for artifact in [&stale_report, &fresh_report, &unrelated] {
            fs::write(artifact, b"retained atomic artifact bytes")
                .expect("write retained atomic artifact");
        }
        set_mtime_seconds(&stale_report, 100);
        set_mtime_seconds(&fresh_report, 175);
        set_mtime_seconds(&unrelated, 100);

        let _lease = acquire_source_universe_local_storage_with_probe(
            &policy(&workspace),
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("sweep retained report artifacts and admit");

        assert!(!stale_report.exists());
        assert!(fresh_report.is_file());
        assert!(unrelated.is_file());
    }

    #[test]
    fn crash_debris_sweep_charges_each_discovered_entry_once() {
        use std::os::unix::fs::PermissionsExt;

        const ROOT_ENTRY_COUNT: u64 = 3;
        const PARTIAL_RECEIPT_ENTRY_COUNT: u64 = 1;
        let temp = tempfile::tempdir().expect("temporary parent");
        let output_path = temp.path().join("output");
        fs::create_dir(&output_path).expect("create output");
        let empty_attempt = output_path.join("run-empty.42.99.tmp");
        fs::create_dir(&empty_attempt).expect("create empty attempt");
        fs::set_permissions(&empty_attempt, fs::Permissions::from_mode(0o700))
            .expect("make empty attempt private");
        set_mtime_seconds(&empty_attempt, 1);
        let partial_attempt = output_path.join("run-partial.42.100.tmp");
        write_partial_record_attempt_receipt(&partial_attempt, 1);
        let report_temp = output_path.join(format!(
            "{SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE}.42.101.tmp"
        ));
        fs::write(&report_temp, b"partial report").expect("write partial report");
        set_mtime_seconds(&report_temp, 1);
        let output = open_real_directory(&output_path, "test output").expect("open test output");
        let exact_entries = ROOT_ENTRY_COUNT + PARTIAL_RECEIPT_ENTRY_COUNT;
        let mut progress =
            LocalStorageTraversalProgress::new(SourceUniverseLifecycleCleanupLimits {
                max_entries: exact_entries,
                max_depth: 2,
            });

        sweep_stale_output_artifacts(&output, &output_path, 200, 100, &mut progress)
            .expect("one-pass crash-debris sweep must fit the exact entry budget");

        assert_eq!(progress.entries, exact_entries);
        assert!(!empty_attempt.exists());
        assert!(!partial_attempt.exists());
        assert!(!report_temp.exists());
    }

    #[test]
    fn stale_candidate_cleanup_rejects_depth_budget_exhaustion() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&output).expect("create output");
        let candidate = output.join("run.1.1.tmp");
        write_record_attempt_receipt(&candidate, 100);
        let receipt = candidate.join(SOURCE_UNIVERSE_RECORD_ATTEMPT_RECEIPT_FILE);
        let shallow = candidate.join("a-shallow-residue");
        fs::write(&shallow, b"shallow residue").expect("write shallow residue");
        let deep = candidate.join("z-nested/z-deeper/residue");
        fs::create_dir_all(deep.parent().expect("deep residue parent"))
            .expect("create deep nested residue");
        fs::write(&deep, b"deep residue").expect("write deep residue");
        let receipt_before = fs::read(&receipt).expect("read receipt before failed cleanup");
        let mut policy = policy(&workspace);
        policy.max_lifecycle_cleanup_entries = 100;
        policy.max_lifecycle_cleanup_depth = 2;

        let error = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("cleanup above the depth budget must fail closed");

        assert!(error.to_string().contains("depth 3 exceeds"), "{error:#}");
        assert!(
            candidate.exists(),
            "failed cleanup must not escape its root"
        );
        assert_eq!(
            fs::read(&receipt).expect("receipt survives failed cleanup"),
            receipt_before,
            "preflight failure must preserve cleanup authority"
        );
        assert_eq!(
            fs::read(&shallow).expect("shallow sibling survives failed cleanup"),
            b"shallow residue",
            "late structural failure must be lossless"
        );
        assert_eq!(
            fs::read(&deep).expect("deep residue survives failed cleanup"),
            b"deep residue",
            "violating subtree must remain unchanged"
        );
    }

    #[test]
    fn pre_record_admission_rejects_underdeclared_record_envelope() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        let policy = policy(&workspace);
        let lease = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("acquire local-storage lease");

        let error = lease
            .verify_pre_record_admission_with_probe(
                &policy,
                policy.one_record_worst_case_bytes + 1,
                abundant_free_space,
            )
            .expect_err("underdeclared one-record envelope must fail closed");

        assert!(
            error.to_string().contains("one_record_worst_case_bytes"),
            "{error:#}"
        );
    }

    #[test]
    fn observed_terminal_scan_rejects_cache_quota_and_free_space_postconditions() {
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        let mut policy = policy(&workspace);
        policy.one_record_worst_case_bytes = 4096;
        policy.max_cache_bytes = 8192;
        let lease = acquire_source_universe_local_storage_with_probe(
            &policy,
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect("acquire local-storage lease");
        fs::create_dir_all(&cache).expect("create cache");
        fs::write(cache.join("a".repeat(64)), vec![9_u8; 16_384])
            .expect("allocate over-quota cache entry");

        let quota_error = lease
            .verify_observed_terminal_boundedness_with_probe(&policy, abundant_free_space)
            .expect_err("observed terminal cache bytes above quota must fail closed");
        assert!(
            quota_error.to_string().contains("max_cache_bytes"),
            "{quota_error:#}"
        );

        fs::remove_file(cache.join("a".repeat(64))).expect("remove over-quota cache entry");
        let reserve_error = lease
            .verify_observed_terminal_boundedness_with_probe(&policy, |_| {
                Ok(policy.minimum_free_space_reserve_bytes - 1)
            })
            .expect_err("observed terminal free space below reserve must fail closed");
        assert!(
            reserve_error
                .to_string()
                .contains("minimum_free_space_reserve_bytes"),
            "{reserve_error:#}"
        );
    }

    #[test]
    fn scan_preserves_foreign_entries_and_rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let (output, cache) = roots(&workspace);
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&output).expect("create output");
        let foreign = cache.join("foreign-name");
        fs::write(&foreign, b"foreign").expect("write foreign entry");
        let target = temp.path().join("target");
        fs::write(&target, b"survives").expect("write target");
        let link = cache.join("c".repeat(64));
        symlink(&target, &link).expect("plant symlink");
        let error = acquire_source_universe_local_storage_with_probe(
            &policy(&workspace),
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("symlink must fail scan");
        assert!(error.to_string().contains("symlink"), "{error:#}");
        assert_eq!(fs::read(target).expect("read target"), b"survives");
        assert_eq!(fs::read(foreign).expect("read foreign"), b"foreign");
    }

    #[test]
    fn admission_rejects_output_root_symlink_before_lifecycle_mutation() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary parent");
        let workspace = temp.path().join("workspace");
        let target = workspace.join("actual-output");
        let output = workspace.join("output");
        let cache = workspace.join("cache");
        fs::create_dir_all(&target).expect("create symlink target");
        symlink(&target, &output).expect("plant output root symlink");

        let error = acquire_source_universe_local_storage_with_probe(
            &policy(&workspace),
            temp.path(),
            &output,
            &cache,
            200,
            abundant_free_space,
        )
        .expect_err("output root symlink must fail closed");

        assert!(
            error.to_string().contains("must not be a symlink"),
            "{error:#}"
        );
        assert!(
            fs::read_dir(target)
                .expect("read untouched symlink target")
                .next()
                .is_none()
        );
        assert!(!cache.exists());
    }
}
