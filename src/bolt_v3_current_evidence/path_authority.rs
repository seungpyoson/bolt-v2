#[cfg(not(unix))]
use std::{fs::File, path::Path};

#[cfg(not(unix))]
use anyhow::{Result, bail};

#[cfg(unix)]
mod unix {
    use std::{
        ffi::{CString, OsStr},
        fs::File,
        io::{self, Write},
        mem::MaybeUninit,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
            unix::fs::MetadataExt,
        },
        path::{Component, Path},
    };

    use anyhow::{Context, Result, bail, ensure};

    use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;

    pub(crate) struct CatalogDirectory {
        directory: File,
        #[cfg(test)]
        sync_directory: fn(&File) -> io::Result<()>,
    }

    struct ParentDirectory {
        directory: File,
        basename: CString,
        display: String,
    }

    impl CatalogDirectory {
        pub(crate) fn open(path: &Path) -> Result<Self> {
            ensure!(
                path.is_absolute(),
                "decision-evidence catalog_directory must be absolute"
            );
            let mut directory = open_root_directory()?;
            for component in path.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(name) => {
                        directory = open_directory_at(&directory, name).with_context(|| {
                            format!(
                                "open decision-evidence catalog component `{}` without symlinks",
                                name.to_string_lossy()
                            )
                        })?;
                    }
                    Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                        bail!("decision-evidence catalog_directory is not normalized")
                    }
                }
            }
            Ok(Self {
                directory,
                #[cfg(test)]
                sync_directory: sync_directory_entry,
            })
        }

        pub(crate) fn open_under_prefix(prefix: &Path, catalog: &Path) -> Result<Self> {
            let prefix_authority = Self::open(prefix)
                .with_context(|| format!("open required catalog prefix `{}`", prefix.display()))?;
            let relative = catalog.strip_prefix(prefix).with_context(|| {
                format!(
                    "persistence.catalog_directory `{}` must be under `{}` for this service",
                    catalog.display(),
                    prefix.display()
                )
            })?;
            let directory = prefix_authority.walk_directory(relative).with_context(|| {
                format!(
                    "open persistence.catalog_directory `{}` beneath required prefix `{}`",
                    catalog.display(),
                    prefix.display()
                )
            })?;
            Ok(Self {
                directory,
                #[cfg(test)]
                sync_directory: sync_directory_entry,
            })
        }

        pub(crate) fn prestart_probe_and_available_bytes(&self) -> Result<u64> {
            let basename = CString::new(format!(
                ".bolt-v2-prestart-write-probe-{}",
                std::process::id()
            ))
            .expect("decimal process id cannot contain NUL");
            remove_file_if_present(&self.directory, &basename)
                .context("remove stale catalog write probe")?;
            let flags =
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW;
            // SAFETY: the directory descriptor and NUL-terminated basename are valid for this call.
            let raw = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    basename.as_ptr(),
                    flags,
                    PRIVATE_ARTIFACT_FILE_MODE,
                )
            };
            if raw < 0 {
                return Err(io::Error::last_os_error())
                    .context("catalog write probe failed during creation");
            }
            // SAFETY: `openat` returned a new owned descriptor.
            let mut probe = unsafe { File::from_raw_fd(raw) };
            let probe_result = probe
                .write_all(b"\n")
                .and_then(|()| probe.sync_all())
                .context("catalog write probe failed during write or synchronization");
            drop(probe);
            let cleanup_result = remove_file_if_present(&self.directory, &basename)
                .context("remove catalog write probe")
                .and_then(|()| {
                    self.synchronize_directory(&self.directory)
                        .context("synchronize catalog after write-probe cleanup")
                });
            probe_result?;
            cleanup_result?;
            available_bytes(&self.directory)
        }

        pub(crate) fn open_stream(&self, relative: &str) -> Result<File> {
            let Some(parent) = self.walk_parent(relative, false)? else {
                bail!("active decision-evidence stream parent is absent: `{relative}`")
            };
            parent.open_stream(self)
        }

        pub(crate) fn ensure_retired_absent(&self, relative: &str) -> Result<()> {
            let Some(parent) = self.walk_parent(relative, true)? else {
                return Ok(());
            };
            parent.ensure_absent()
        }

        fn walk_parent(
            &self,
            relative: &str,
            missing_parent_means_absent: bool,
        ) -> Result<Option<ParentDirectory>> {
            let path = Path::new(relative.trim());
            let mut components = path.components().peekable();
            let mut directory = self.directory.try_clone()?;
            while let Some(component) = components.next() {
                let Component::Normal(name) = component else {
                    bail!("decision-evidence relative path is not normalized")
                };
                if components.peek().is_none() {
                    return Ok(Some(ParentDirectory {
                        directory,
                        basename: component_name(name)?,
                        display: relative.trim().to_string(),
                    }));
                }
                match open_directory_at(&directory, name) {
                    Ok(next) => {
                        if !missing_parent_means_absent {
                            self.synchronize_directory(&directory).with_context(|| {
                                format!(
                                    "synchronize active decision-evidence parent component `{}`",
                                    name.to_string_lossy()
                                )
                            })?;
                        }
                        directory = next;
                    }
                    Err(error)
                        if missing_parent_means_absent
                            && error.kind() == io::ErrorKind::NotFound =>
                    {
                        return Ok(None);
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        create_directory_at(&directory, name).with_context(|| {
                            format!(
                                "create active decision-evidence parent component `{}`",
                                name.to_string_lossy()
                            )
                        })?;
                        self.synchronize_directory(&directory).with_context(|| {
                            format!(
                                "synchronize active decision-evidence parent component `{}`",
                                name.to_string_lossy()
                            )
                        })?;
                        directory = open_directory_at(&directory, name).with_context(|| {
                            format!(
                                "open created decision-evidence parent component `{}` without symlinks",
                                name.to_string_lossy()
                            )
                        })?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "open decision-evidence path component `{}` without symlinks",
                                name.to_string_lossy()
                            )
                        });
                    }
                }
            }
            bail!("decision-evidence relative path has no file name")
        }

        fn walk_directory(&self, relative: &Path) -> Result<File> {
            let mut directory = self.directory.try_clone()?;
            for component in relative.components() {
                let Component::Normal(name) = component else {
                    bail!("catalog descendant path is not normalized")
                };
                directory = open_directory_at(&directory, name).with_context(|| {
                    format!(
                        "open catalog descendant component `{}` without symlinks",
                        name.to_string_lossy()
                    )
                })?;
            }
            Ok(directory)
        }

        fn synchronize_directory(&self, directory: &File) -> io::Result<()> {
            #[cfg(test)]
            {
                (self.sync_directory)(directory)
            }
            #[cfg(not(test))]
            {
                sync_directory_entry(directory)
            }
        }
    }

    impl ParentDirectory {
        fn open_stream(self, catalog: &CatalogDirectory) -> Result<File> {
            let create_flags = libc::O_RDWR
                | libc::O_APPEND
                | libc::O_CREAT
                | libc::O_EXCL
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW;
            // SAFETY: the parent descriptor and NUL-terminated basename remain valid for the call.
            let created_raw = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    self.basename.as_ptr(),
                    create_flags,
                    PRIVATE_ARTIFACT_FILE_MODE,
                )
            };
            let raw = if created_raw >= 0 {
                created_raw
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EEXIST) {
                    let existing_flags =
                        libc::O_RDWR | libc::O_APPEND | libc::O_CLOEXEC | libc::O_NOFOLLOW;
                    // SAFETY: the parent descriptor and NUL-terminated basename remain valid.
                    let existing_raw = unsafe {
                        libc::openat(
                            self.directory.as_raw_fd(),
                            self.basename.as_ptr(),
                            existing_flags,
                        )
                    };
                    if existing_raw >= 0 {
                        existing_raw
                    } else {
                        return self.open_error(io::Error::last_os_error());
                    }
                } else {
                    return self.open_error(error);
                }
            };
            // SAFETY: `openat` returned a new owned descriptor on success.
            let file = unsafe { File::from_raw_fd(raw) };
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file(),
                "decision-evidence path is not a regular file: `{}`",
                self.display
            );
            ensure!(
                metadata.nlink() == 1,
                "decision-evidence path has external hard-link aliases: `{}`",
                self.display
            );
            catalog
                .synchronize_directory(&self.directory)
                .with_context(|| {
                    format!(
                        "synchronize decision-evidence stream namespace `{}`",
                        self.display
                    )
                })?;
            Ok(file)
        }

        fn open_error<T>(&self, error: io::Error) -> Result<T> {
            if matches!(error.raw_os_error(), Some(libc::ELOOP)) {
                bail!(
                    "decision-evidence path is not a regular file: symlink rejected at `{}`",
                    self.display
                );
            }
            if matches!(error.raw_os_error(), Some(libc::EISDIR)) {
                bail!(
                    "decision-evidence path is not a regular file: `{}`",
                    self.display
                );
            }
            Err(error).with_context(|| {
                format!(
                    "open decision-evidence stream `{}` without symlinks",
                    self.display
                )
            })
        }

        fn ensure_absent(self) -> Result<()> {
            let mut metadata = MaybeUninit::<libc::stat>::uninit();
            // SAFETY: all pointers are valid for the call and `metadata` is only read on success.
            let result = unsafe {
                libc::fstatat(
                    self.directory.as_raw_fd(),
                    self.basename.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                bail!(
                    "retired decision-evidence path is present: `{}`",
                    self.display
                );
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(());
            }
            Err(error).with_context(|| {
                format!(
                    "inspect retired decision-evidence path `{}` without symlinks",
                    self.display
                )
            })
        }
    }

    fn open_root_directory() -> Result<File> {
        let root = CString::new("/").expect("root path has no NUL");
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        // SAFETY: `root` is a valid NUL-terminated path for the duration of the call.
        let raw = unsafe { libc::open(root.as_ptr(), flags) };
        if raw < 0 {
            return Err(io::Error::last_os_error())
                .context("open filesystem root for decision-evidence catalog");
        }
        // SAFETY: `open` returned a new owned descriptor on success.
        Ok(unsafe { File::from_raw_fd(raw) })
    }

    fn sync_directory_entry(directory: &File) -> io::Result<()> {
        directory.sync_all()
    }

    fn remove_file_if_present(directory: &File, basename: &CString) -> io::Result<()> {
        // SAFETY: the directory descriptor and NUL-terminated basename are valid for this call.
        let result = unsafe { libc::unlinkat(directory.as_raw_fd(), basename.as_ptr(), 0) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn available_bytes(directory: &File) -> Result<u64> {
        let mut stat = MaybeUninit::<libc::statvfs>::zeroed();
        // SAFETY: the descriptor is valid and `stat` is initialized only on success.
        if unsafe { libc::fstatvfs(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("inspect catalog free space");
        }
        // SAFETY: `fstatvfs` initialized `stat` on success.
        let stat = unsafe { stat.assume_init() };
        let fragment_size = if stat.f_frsize == 0 {
            stat.f_bsize
        } else {
            stat.f_frsize
        };
        let available = u128::from(stat.f_bavail) * u128::from(fragment_size);
        Ok(available.min(u128::from(u64::MAX)) as u64)
    }

    fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
        let name = component_name(name)?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        // SAFETY: the parent descriptor and NUL-terminated component remain valid for the call.
        let raw = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new owned descriptor on success.
        Ok(unsafe { File::from_raw_fd(raw) })
    }

    fn create_directory_at(parent: &File, name: &OsStr) -> io::Result<()> {
        let name = component_name(name)?;
        // SAFETY: the parent descriptor and NUL-terminated component remain valid for the call.
        let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::AlreadyExists {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn component_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "decision-evidence path component contains NUL",
            )
        })
    }

    #[cfg(test)]
    mod tests {
        use std::{
            fs,
            io::Write,
            os::unix::fs::symlink,
            sync::atomic::{AtomicUsize, Ordering},
        };

        use super::*;

        #[test]
        fn retained_parent_descriptor_cannot_be_redirected_by_path_replacement() {
            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let original = canonical_root.join("original");
            let retained = canonical_root.join("retained");
            let outside = tempfile::tempdir().expect("outside tempdir must exist");
            fs::create_dir(&original).expect("original parent must exist");
            let catalog = CatalogDirectory::open(&canonical_root).expect("catalog must open");
            let parent = catalog
                .walk_parent("original/machine.jsonl", false)
                .expect("parent walk must succeed")
                .expect("parent must exist");

            fs::rename(&original, &retained).expect("original parent must be retained");
            symlink(outside.path(), &original).expect("old pathname must be redirected");
            let mut stream = parent
                .open_stream(&catalog)
                .expect("descriptor-relative open must succeed");
            stream.write_all(b"retained\n").expect("write must succeed");
            stream.sync_data().expect("write must sync");

            assert_eq!(
                fs::read(retained.join("machine.jsonl")).expect("retained file must read"),
                b"retained\n"
            );
            assert!(!outside.path().join("machine.jsonl").exists());
        }

        #[test]
        fn newly_created_stream_requires_parent_directory_sync() {
            fn reject_sync(_directory: &File) -> io::Result<()> {
                Err(io::Error::other("injected directory sync failure"))
            }

            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let mut catalog = CatalogDirectory::open(&canonical_root).expect("catalog must open");
            catalog.sync_directory = reject_sync;

            let error = catalog
                .open_stream("machine.jsonl")
                .expect_err("creation must fail when namespace sync fails");

            assert!(
                error
                    .to_string()
                    .contains("synchronize decision-evidence stream namespace")
            );
        }

        #[test]
        fn active_stream_open_creates_missing_parent_chain_through_catalog_authority() {
            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let catalog = CatalogDirectory::open(&canonical_root).expect("catalog must open");

            let stream = catalog
                .open_stream("bolt-v3/decision-evidence/current/machine.jsonl")
                .expect("active stream parents and stream must be created");

            assert!(
                stream
                    .metadata()
                    .expect("stream metadata must read")
                    .is_file()
            );
            assert!(
                root.path()
                    .join("bolt-v3/decision-evidence/current/machine.jsonl")
                    .is_file()
            );
        }

        #[test]
        fn retry_after_parent_creation_sync_failure_completes_the_same_descriptor_walk() {
            static PARENT_SYNC_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

            fn fail_first_parent_sync(_directory: &File) -> io::Result<()> {
                if PARENT_SYNC_ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(io::Error::other("injected parent-component sync failure"))
                } else {
                    Ok(())
                }
            }

            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let mut catalog = CatalogDirectory::open(&canonical_root).expect("catalog must open");
            catalog.sync_directory = fail_first_parent_sync;
            let relative = "bolt-v3/decision-evidence/current/machine.jsonl";

            catalog
                .open_stream(relative)
                .expect_err("first parent-component namespace sync must fail");
            let stream = catalog
                .open_stream(relative)
                .expect("retry must complete the descriptor-relative parent walk");

            assert!(
                stream
                    .metadata()
                    .expect("stream metadata must read")
                    .is_file()
            );
            assert!(
                PARENT_SYNC_ATTEMPTS.load(Ordering::SeqCst) >= 5,
                "retry must synchronize each retained or created parent and the stream namespace"
            );
        }

        #[test]
        fn retry_after_creation_sync_failure_reestablishes_namespace_durability() {
            static SYNC_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

            fn fail_once(_directory: &File) -> io::Result<()> {
                if SYNC_ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(io::Error::other("injected first directory sync failure"))
                } else {
                    Ok(())
                }
            }

            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let mut catalog = CatalogDirectory::open(&canonical_root).expect("catalog must open");
            catalog.sync_directory = fail_once;

            catalog
                .open_stream("machine.jsonl")
                .expect_err("first namespace sync must fail");
            let stream = catalog
                .open_stream("machine.jsonl")
                .expect("retry must synchronize the retained namespace");

            assert_eq!(
                stream.metadata().expect("stream metadata must read").len(),
                0
            );
            assert_eq!(SYNC_ATTEMPTS.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn catalog_path_rejects_final_and_intermediate_symlinks() {
            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let real = canonical_root.join("real");
            let child = real.join("child");
            fs::create_dir_all(&child).expect("real catalog must exist");
            let final_link = canonical_root.join("final-link");
            symlink(&child, &final_link).expect("final symlink must exist");
            let intermediate_link = canonical_root.join("intermediate-link");
            symlink(&real, &intermediate_link).expect("intermediate symlink must exist");

            assert!(
                CatalogDirectory::open(&final_link).is_err(),
                "final catalog symlink must fail closed"
            );
            assert!(
                CatalogDirectory::open(&intermediate_link.join("child")).is_err(),
                "intermediate catalog symlink must fail closed"
            );
        }

        #[test]
        fn catalog_under_prefix_rejects_symlinked_prefix_and_descendants() {
            let root = tempfile::tempdir().expect("tempdir must exist");
            let canonical_root = fs::canonicalize(root.path()).expect("tempdir must canonicalize");
            let real_prefix = canonical_root.join("real-prefix");
            let catalog = real_prefix.join("catalog");
            fs::create_dir_all(&catalog).expect("catalog must exist");
            let prefix_link = canonical_root.join("prefix-link");
            symlink(&real_prefix, &prefix_link).expect("prefix symlink must exist");
            let descendant_link = real_prefix.join("catalog-link");
            symlink(&catalog, &descendant_link).expect("descendant symlink must exist");

            assert!(
                CatalogDirectory::open_under_prefix(&prefix_link, &prefix_link.join("catalog"))
                    .is_err(),
                "symlinked required prefix must fail closed"
            );
            assert!(
                CatalogDirectory::open_under_prefix(&real_prefix, &descendant_link).is_err(),
                "symlinked catalog descendant must fail closed"
            );
        }
    }
}

#[cfg(unix)]
pub(super) use unix::CatalogDirectory;

#[cfg(not(unix))]
pub(super) struct CatalogDirectory;

#[cfg(not(unix))]
impl CatalogDirectory {
    pub(super) fn open(_path: &Path) -> Result<Self> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }

    pub(super) fn open_under_prefix(_prefix: &Path, _catalog: &Path) -> Result<Self> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }

    pub(super) fn prestart_probe_and_available_bytes(&self) -> Result<u64> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }

    pub(super) fn open_stream(&self, _relative: &str) -> Result<File> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }

    pub(super) fn ensure_retired_absent(&self, _relative: &str) -> Result<()> {
        bail!("decision-evidence runtime is unsupported on non-Unix targets")
    }
}
